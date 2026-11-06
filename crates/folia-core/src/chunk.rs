use crate::nbt::Tag;
use std::collections::HashMap;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockState {
    pub name: String,
    pub properties: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedContainer<T> {
    pub palette: Vec<T>,
    pub indices: Vec<u16>,
}

impl<T> PalettedContainer<T> {
    pub fn get(&self, index: usize) -> Option<&T> {
        let palette_index = *self.indices.get(index)? as usize;
        self.palette.get(palette_index)
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSection {
    pub y: i8,
    pub block_states: PalettedContainer<BlockState>,
    pub biomes: Option<PalettedContainer<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub sections: Vec<ChunkSection>,
}

impl Chunk {
    pub fn section(&self, y: i8) -> Option<&ChunkSection> {
        self.sections.iter().find(|section| section.y == y)
    }

    fn section_mut(&mut self, y: i8) -> Option<&mut ChunkSection> {
        self.sections.iter_mut().find(|section| section.y == y)
    }

    /// Set a block state at absolute world coordinates. Creates an air section
    /// when the target section is absent. Returns true when the state changed.
    pub fn set_block_state(&mut self, x: usize, y: i32, z: usize, state: BlockState) -> bool {
        self.set_block_state_ref(x, y, z, &state)
    }

    /// Borrowed-state variant of [`Self::set_block_state`]. The state is only
    /// cloned when it becomes a new palette entry, which keeps bulk fill loops
    /// from allocating a string per block.
    pub fn set_block_state_ref(&mut self, x: usize, y: i32, z: usize, state: &BlockState) -> bool {
        if x >= 16 || z >= 16 {
            return false;
        }
        let section_y = y.div_euclid(16);
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&section_y) {
            return false;
        }
        let section_y = section_y as i8;
        let local_y = y.rem_euclid(16) as usize;
        let index = (local_y * 16 + z) * 16 + x;

        if self.section(section_y).is_none() {
            self.sections.push(ChunkSection {
                y: section_y,
                block_states: PalettedContainer {
                    palette: vec![BlockState {
                        name: "minecraft:air".into(),
                        properties: vec![],
                    }],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
                    indices: vec![0; 64],
                }),
            });
            self.sections.sort_by_key(|section| section.y);
        }

        let Some(section) = self.section_mut(section_y) else {
            return false;
        };
        if section.block_states.indices.len() != 4096 {
            return false;
        }
        if section.block_states.get(index) == Some(state) {
            return false;
        }

        let palette_index = match section
            .block_states
            .palette
            .iter()
            .position(|value| value == state)
        {
            Some(index) => index,
            None => {
                if section.block_states.palette.len() > u16::MAX as usize {
                    return false;
                }
                section.block_states.palette.push(state.clone());
                section.block_states.palette.len() - 1
            }
        };
        section.block_states.indices[index] = palette_index as u16;
        true
    }

    /// Absolute world Y coordinate, x/z in 0..16.
    pub fn block_state(&self, x: usize, y: i32, z: usize) -> Option<&BlockState> {
        if x >= 16 || z >= 16 {
            return None;
        }
        let section_y = y.div_euclid(16);
        let local_y = y.rem_euclid(16) as usize;
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&section_y) {
            return None;
        }
        let section = self.section(section_y as i8)?;
        let index = (local_y * 16 + z) * 16 + x;
        section.block_states.get(index)
    }

    pub fn biome(&self, x: usize, y: i32, z: usize) -> Option<&String> {
        if x >= 16 || z >= 16 {
            return None;
        }
        let section_y = y.div_euclid(16);
        let local_y = y.rem_euclid(16) as usize;
        if !(i8::MIN as i32..=i8::MAX as i32).contains(&section_y) {
            return None;
        }
        let section = self.section(section_y as i8)?;
        // Biomes are stored as 4x4x4 cells per section, in y/z/x order.
        let cell_x = x / 4;
        let cell_y = local_y / 4;
        let cell_z = z / 4;
        let index = (cell_y * 4 + cell_z) * 4 + cell_x;
        section.biomes.as_ref()?.get(index)
    }
}

/// True for the three air states a generated or stored chunk can contain.
pub fn is_air_state(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// Absolute Y of the first empty block above the highest non-air block in a
/// column, or the bottom of the chunk when the column is entirely air.
pub fn surface_y(chunk: &Chunk, x: usize, z: usize) -> i32 {
    if x >= 16 || z >= 16 {
        return 0;
    }
    for section in chunk.sections.iter().rev() {
        let base = i32::from(section.y) * 16;
        for local_y in (0..16).rev() {
            let index = (local_y * 16 + z) * 16 + x;
            let Some(state) = section.block_states.get(index) else {
                continue;
            };
            if !is_air_state(state) {
                return base + local_y as i32 + 1;
            }
        }
    }
    chunk
        .sections
        .first()
        .map_or(0, |section| i32::from(section.y) * 16)
}

pub fn decode_chunk(root: &Tag) -> io::Result<Chunk> {
    let compound = root
        .as_compound()
        .ok_or_else(|| invalid("chunk root must be a compound tag"))?;
    let x = get_int(compound, "xPos")?;
    let z = get_int(compound, "zPos")?;

    let sections_tag = root
        .get("sections")
        .and_then(Tag::as_list)
        .ok_or_else(|| invalid("chunk is missing its sections list"))?;

    let mut sections = Vec::with_capacity(sections_tag.len());
    for section_tag in sections_tag {
        sections.push(decode_section(section_tag)?);
    }
    sections.sort_by_key(|section| section.y);

    Ok(Chunk { x, z, sections })
}

pub fn encode_chunk(chunk: &Chunk) -> io::Result<Tag> {
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for section in &chunk.sections {
        sections.push(encode_section(section)?);
    }

    Ok(Tag::Compound(vec![
        ("DataVersion".into(), Tag::Int(crate::level::DATA_VERSION)),
        ("Status".into(), Tag::String("minecraft:full".into())),
        ("xPos".into(), Tag::Int(chunk.x)),
        ("zPos".into(), Tag::Int(chunk.z)),
        ("sections".into(), Tag::List(sections)),
    ]))
}

fn encode_section(section: &ChunkSection) -> io::Result<Tag> {
    if section.block_states.len() != 4096 {
        return Err(invalid(
            "chunk section must contain exactly 4096 block states",
        ));
    }

    let (block_palette, block_indices) = canonicalize_blocks(&section.block_states)?;
    if block_palette.is_empty() {
        return Err(invalid("chunk section palette is empty"));
    }

    let block_palette_len = block_palette.len();
    let mut block_states = vec![(
        "palette".into(),
        Tag::List(block_palette.iter().map(encode_block_state).collect()),
    )];
    if block_palette_len > 1 {
        block_states.push((
            "data".into(),
            Tag::LongArray(pack_indices(&block_indices, block_palette_len, 4)?),
        ));
    }

    let mut entries = vec![
        ("Y".into(), Tag::Byte(section.y)),
        ("block_states".into(), Tag::Compound(block_states)),
    ];

    if let Some(biomes) = &section.biomes {
        if biomes.len() != 64 {
            return Err(invalid("chunk section must contain exactly 64 biomes"));
        }
        let (biome_palette, biome_indices) = canonicalize_biomes(biomes)?;
        if biome_palette.is_empty() {
            return Err(invalid("biome palette is empty"));
        }
        let biome_palette_len = biome_palette.len();
        let mut biome_entries = vec![(
            "palette".into(),
            Tag::List(biome_palette.into_iter().map(Tag::String).collect()),
        )];
        if biome_palette_len > 1 {
            biome_entries.push((
                "data".into(),
                Tag::LongArray(pack_indices(&biome_indices, biome_palette_len, 1)?),
            ));
        }
        entries.push(("biomes".into(), Tag::Compound(biome_entries)));
    }

    Ok(Tag::Compound(entries))
}

fn encode_block_state(state: &BlockState) -> Tag {
    let mut entries = vec![("Name".into(), Tag::String(state.name.clone()))];
    if !state.properties.is_empty() {
        entries.push((
            "Properties".into(),
            Tag::Compound(
                state
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Tag::String(value.clone())))
                    .collect(),
            ),
        ));
    }
    Tag::Compound(entries)
}

fn canonicalize_blocks(
    container: &PalettedContainer<BlockState>,
) -> io::Result<(Vec<BlockState>, Vec<u16>)> {
    let mut palette = Vec::new();
    let mut lookup: HashMap<BlockState, u16> = HashMap::new();
    let mut indices = Vec::with_capacity(container.indices.len());

    for &index in &container.indices {
        let state = container.palette.get(index as usize).ok_or_else(|| {
            invalid(format!(
                "palette index {index} is outside palette of {} entries",
                container.palette.len()
            ))
        })?;
        let next = match lookup.get(state) {
            Some(&value) => value,
            None => {
                let value = palette.len() as u16;
                palette.push(state.clone());
                lookup.insert(state.clone(), value);
                value
            }
        };
        indices.push(next);
    }

    Ok((palette, indices))
}

fn canonicalize_biomes(
    container: &PalettedContainer<String>,
) -> io::Result<(Vec<String>, Vec<u16>)> {
    let mut palette = Vec::new();
    let mut lookup: HashMap<String, u16> = HashMap::new();
    let mut indices = Vec::with_capacity(container.indices.len());

    for &index in &container.indices {
        let biome = container.palette.get(index as usize).ok_or_else(|| {
            invalid(format!(
                "biome index {index} is outside palette of {} entries",
                container.palette.len()
            ))
        })?;
        let next = match lookup.get(biome) {
            Some(&value) => value,
            None => {
                let value = palette.len() as u16;
                palette.push(biome.clone());
                lookup.insert(biome.clone(), value);
                value
            }
        };
        indices.push(next);
    }

    Ok((palette, indices))
}

fn pack_indices(indices: &[u16], palette_len: usize, minimum_bits: u32) -> io::Result<Vec<i64>> {
    let bits = minimum_bits.max(ceil_log2(palette_len));
    if bits > 32 {
        return Err(invalid("palette needs more than 32 bits per entry"));
    }
    let per_long = (64 / bits) as usize;
    let mut longs = vec![0u64; indices.len().div_ceil(per_long)];

    for (index, &value) in indices.iter().enumerate() {
        if usize::from(value) >= palette_len {
            return Err(invalid(format!(
                "palette index {value} is outside palette of {palette_len} entries"
            )));
        }
        let long_index = index / per_long;
        let shift = ((index % per_long) * bits as usize) as u32;
        longs[long_index] |= u64::from(value) << shift;
    }

    Ok(longs.into_iter().map(|value| value as i64).collect())
}

fn decode_section(tag: &Tag) -> io::Result<ChunkSection> {
    let compound = tag
        .as_compound()
        .ok_or_else(|| invalid("chunk section must be a compound tag"))?;
    let y = compound
        .iter()
        .find(|(key, _)| key == "Y")
        .and_then(|(_, value)| value.as_byte())
        .ok_or_else(|| invalid("chunk section is missing its Y byte"))?;

    let block_states = match tag.get("block_states") {
        Some(block_states) => decode_block_states(block_states)?,
        // Empty sections in real worlds may omit block_states entirely.
        None => PalettedContainer {
            palette: vec![BlockState {
                name: "minecraft:air".into(),
                properties: Vec::new(),
            }],
            indices: vec![0; 4096],
        },
    };

    let biomes = tag.get("biomes").map(decode_biomes).transpose()?;

    Ok(ChunkSection {
        y,
        block_states,
        biomes,
    })
}

fn decode_block_states(tag: &Tag) -> io::Result<PalettedContainer<BlockState>> {
    let palette_tag = tag
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or_else(|| invalid("block_states is missing its palette"))?;

    let mut palette = Vec::with_capacity(palette_tag.len());
    for entry in palette_tag {
        palette.push(decode_block_state(entry)?);
    }
    if palette.is_empty() {
        return Err(invalid("block palette is empty"));
    }

    let data = tag.get("data").and_then(Tag::as_long_array);
    let indices = decode_packed_indices(palette.len(), data, 4096, 4)?;
    Ok(PalettedContainer { palette, indices })
}

fn decode_biomes(tag: &Tag) -> io::Result<PalettedContainer<String>> {
    let palette_tag = tag
        .get("palette")
        .and_then(Tag::as_list)
        .ok_or_else(|| invalid("biomes is missing its palette"))?;

    let mut palette = Vec::with_capacity(palette_tag.len());
    for entry in palette_tag {
        palette.push(
            entry
                .as_str()
                .ok_or_else(|| invalid("biome palette entries must be strings"))?
                .to_owned(),
        );
    }
    if palette.is_empty() {
        return Err(invalid("biome palette is empty"));
    }

    let data = tag.get("data").and_then(Tag::as_long_array);
    let indices = decode_packed_indices(palette.len(), data, 64, 1)?;
    Ok(PalettedContainer { palette, indices })
}

fn decode_block_state(tag: &Tag) -> io::Result<BlockState> {
    let name = tag
        .get("Name")
        .and_then(Tag::as_str)
        .ok_or_else(|| invalid("block palette entry is missing its Name"))?
        .to_owned();

    let mut properties = Vec::new();
    if let Some(properties_tag) = tag.get("Properties").and_then(Tag::as_compound) {
        for (key, value) in properties_tag {
            let value = value
                .as_str()
                .ok_or_else(|| invalid("block state property value must be a string"))?;
            properties.push((key.clone(), value.to_owned()));
        }
    }

    Ok(BlockState { name, properties })
}

fn decode_packed_indices(
    palette_len: usize,
    data: Option<&[i64]>,
    count: usize,
    minimum_bits: u32,
) -> io::Result<Vec<u16>> {
    if palette_len == 0 {
        return Err(invalid("palette is empty"));
    }
    if palette_len == 1 {
        return Ok(vec![0; count]);
    }

    let bits = minimum_bits.max(ceil_log2(palette_len));
    let data = data.ok_or_else(|| invalid("paletted container is missing its data array"))?;
    let per_long = (64 / bits) as usize;
    let required_longs = count.div_ceil(per_long);
    if data.len() < required_longs {
        return Err(invalid(format!(
            "paletted container needs {required_longs} longs, found {}",
            data.len()
        )));
    }

    let mask = (1u64 << bits) - 1;
    let mut indices = Vec::with_capacity(count);
    for index in 0..count {
        let long = data[index / per_long] as u64;
        let shift = ((index % per_long) * bits as usize) as u32;
        let value = ((long >> shift) & mask) as usize;
        if value >= palette_len {
            return Err(invalid(format!(
                "palette index {value} is outside palette of {palette_len} entries"
            )));
        }
        indices.push(value as u16);
    }
    Ok(indices)
}

fn ceil_log2(value: usize) -> u32 {
    if value <= 1 {
        0
    } else {
        (usize::BITS - (value - 1).leading_zeros()).max(1)
    }
}

fn get_int(compound: &[(String, Tag)], name: &str) -> io::Result<i32> {
    compound
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.as_int())
        .ok_or_else(|| invalid(format!("chunk is missing integer field {name}")))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(name: &str) -> Tag {
        Tag::Compound(vec![("Name".into(), Tag::String(name.into()))])
    }

    fn section(y: i8, palette: Vec<Tag>, bits: u32, pattern: impl Fn(usize) -> u16) -> Tag {
        let indices: Vec<u16> = (0..4096).map(pattern).collect();
        let data = pack(&indices, bits);
        Tag::Compound(vec![
            ("Y".into(), Tag::Byte(y)),
            (
                "block_states".into(),
                Tag::Compound(vec![
                    ("palette".into(), Tag::List(palette)),
                    ("data".into(), Tag::LongArray(data)),
                ]),
            ),
        ])
    }

    fn pack(indices: &[u16], bits: u32) -> Vec<i64> {
        let per_long = (64 / bits) as usize;
        let mut longs = vec![0u64; indices.len().div_ceil(per_long)];
        for (index, value) in indices.iter().copied().enumerate() {
            let long_index = index / per_long;
            let shift = (index % per_long) * bits as usize;
            longs[long_index] |= u64::from(value) << shift;
        }
        longs.into_iter().map(|value| value as i64).collect()
    }

    fn chunk(sections: Vec<Tag>) -> Tag {
        Tag::Compound(vec![
            ("xPos".into(), Tag::Int(12)),
            ("zPos".into(), Tag::Int(-34)),
            ("sections".into(), Tag::List(sections)),
        ])
    }

    #[test]
    fn decodes_paletted_block_states() {
        let tag = chunk(vec![section(
            0,
            vec![block("minecraft:stone"), block("minecraft:dirt")],
            4,
            |index| (index % 2) as u16,
        )]);
        let chunk = decode_chunk(&tag).unwrap();
        assert_eq!(chunk.x, 12);
        assert_eq!(chunk.z, -34);
        assert_eq!(chunk.sections.len(), 1);
        assert_eq!(chunk.block_state(0, 0, 0).unwrap().name, "minecraft:stone");
        assert_eq!(chunk.block_state(1, 0, 0).unwrap().name, "minecraft:dirt");
        assert_eq!(
            chunk.block_state(15, 15, 15).unwrap().name,
            "minecraft:dirt"
        );
    }

    #[test]
    fn single_entry_palette_does_not_need_data() {
        let tag = chunk(vec![Tag::Compound(vec![
            ("Y".into(), Tag::Byte(3)),
            (
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(vec![block("minecraft:air")]),
                )]),
            ),
        ])]);
        let chunk = decode_chunk(&tag).unwrap();
        for y in 48..64 {
            assert_eq!(chunk.block_state(7, y, 9).unwrap().name, "minecraft:air");
        }
    }

    #[test]
    fn decodes_biomes() {
        let tag = chunk(vec![Tag::Compound(vec![
            ("Y".into(), Tag::Byte(0)),
            (
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(vec![block("minecraft:air")]),
                )]),
            ),
            (
                "biomes".into(),
                Tag::Compound(vec![
                    (
                        "palette".into(),
                        Tag::List(vec![
                            Tag::String("minecraft:plains".into()),
                            Tag::String("minecraft:desert".into()),
                        ]),
                    ),
                    ("data".into(), Tag::LongArray(pack(&[1u16; 64], 1))),
                ]),
            ),
        ])]);
        let chunk = decode_chunk(&tag).unwrap();
        assert_eq!(chunk.biome(0, 0, 0).unwrap(), "minecraft:desert");
    }

    #[test]
    fn missing_block_states_is_treated_as_air() {
        let tag = chunk(vec![Tag::Compound(vec![("Y".into(), Tag::Byte(0))])]);
        let chunk = decode_chunk(&tag).unwrap();
        assert_eq!(chunk.block_state(0, 0, 0).unwrap().name, "minecraft:air");
        assert_eq!(chunk.block_state(15, 15, 15).unwrap().name, "minecraft:air");
    }

    #[test]
    fn biome_lookup_uses_4x4x4_cells() {
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        };
        let biomes = PalettedContainer {
            palette: (0..64).map(|index| format!("biome:{index}")).collect(),
            indices: (0..64).collect(),
        };
        let chunk = Chunk {
            x: 0,
            z: 0,
            sections: vec![ChunkSection {
                y: 0,
                block_states: PalettedContainer {
                    palette: vec![air],
                    indices: vec![0; 4096],
                },
                biomes: Some(biomes),
            }],
        };

        assert_eq!(chunk.biome(0, 0, 0).unwrap(), "biome:0");
        assert_eq!(chunk.biome(3, 0, 0).unwrap(), "biome:0");
        assert_eq!(chunk.biome(4, 0, 0).unwrap(), "biome:1");
        assert_eq!(chunk.biome(0, 0, 4).unwrap(), "biome:4");
        assert_eq!(chunk.biome(0, 4, 0).unwrap(), "biome:16");
        assert_eq!(chunk.biome(15, 15, 15).unwrap(), "biome:63");
    }

    #[test]
    fn rejects_out_of_range_palette_index() {
        let mut packed = pack(&vec![0u16; 4096], 4);
        packed[0] = 5; // palette has only two entries
        let tag = chunk(vec![Tag::Compound(vec![
            ("Y".into(), Tag::Byte(0)),
            (
                "block_states".into(),
                Tag::Compound(vec![
                    (
                        "palette".into(),
                        Tag::List(vec![block("minecraft:stone"), block("minecraft:dirt")]),
                    ),
                    ("data".into(), Tag::LongArray(packed)),
                ]),
            ),
        ])]);
        assert!(decode_chunk(&tag).is_err());
    }

    #[test]
    fn rejects_missing_data_for_multi_entry_palette() {
        let tag = chunk(vec![Tag::Compound(vec![
            ("Y".into(), Tag::Byte(0)),
            (
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List(vec![block("minecraft:stone"), block("minecraft:dirt")]),
                )]),
            ),
        ])]);
        assert!(decode_chunk(&tag).is_err());
    }

    #[test]
    fn block_state_lookup_uses_section_y() {
        let tag = chunk(vec![
            section(
                0,
                vec![block("minecraft:stone"), block("minecraft:dirt")],
                4,
                |_| 0,
            ),
            section(
                1,
                vec![block("minecraft:grass_block"), block("minecraft:sand")],
                4,
                |_| 1,
            ),
        ]);
        let chunk = decode_chunk(&tag).unwrap();
        assert_eq!(chunk.block_state(0, 0, 0).unwrap().name, "minecraft:stone");
        assert_eq!(chunk.block_state(0, 16, 0).unwrap().name, "minecraft:sand");
        assert_eq!(chunk.block_state(0, 32, 0), None);
        assert_eq!(chunk.block_state(16, 0, 0), None);
    }

    #[test]
    fn chunk_round_trips_through_the_encoder() {
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        let dirt = BlockState {
            name: "minecraft:dirt".into(),
            properties: vec![],
        };
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        };

        let chunk = Chunk {
            x: 7,
            z: -9,
            sections: vec![
                ChunkSection {
                    y: 0,
                    block_states: PalettedContainer {
                        palette: vec![stone.clone(), dirt.clone()],
                        indices: (0..4096).map(|index| (index % 2) as u16).collect(),
                    },
                    biomes: None,
                },
                ChunkSection {
                    y: 1,
                    block_states: PalettedContainer {
                        palette: vec![air.clone()],
                        indices: vec![0; 4096],
                    },
                    biomes: Some(PalettedContainer {
                        palette: vec!["minecraft:plains".into(), "minecraft:desert".into()],
                        indices: (0..64).map(|index| (index % 2) as u16).collect(),
                    }),
                },
            ],
        };

        let tag = encode_chunk(&chunk).unwrap();
        let decoded = decode_chunk(&tag).unwrap();
        assert_eq!(decoded.x, 7);
        assert_eq!(decoded.z, -9);
        assert_eq!(decoded.block_state(0, 0, 0).unwrap(), &stone);
        assert_eq!(decoded.block_state(1, 0, 0).unwrap(), &dirt);
        assert_eq!(decoded.block_state(0, 16, 0).unwrap(), &air);
        assert_eq!(decoded.biome(0, 16, 0).unwrap(), "minecraft:plains");
        assert_eq!(decoded.biome(4, 16, 0).unwrap(), "minecraft:desert");
        assert_eq!(decoded, chunk);
    }

    #[test]
    fn encoder_deduplicates_palette_entries() {
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        let duplicated = PalettedContainer {
            palette: vec![stone.clone(), stone.clone()],
            indices: (0..4096).map(|index| (index % 2) as u16).collect(),
        };
        let chunk = Chunk {
            x: 0,
            z: 0,
            sections: vec![ChunkSection {
                y: 0,
                block_states: duplicated,
                biomes: None,
            }],
        };

        let tag = encode_chunk(&chunk).unwrap();
        let section = &tag.get("sections").unwrap().as_list().unwrap()[0];
        let palette = section
            .get("block_states")
            .unwrap()
            .get("palette")
            .unwrap()
            .as_list()
            .unwrap();
        assert_eq!(palette.len(), 1);
        assert_eq!(
            decode_chunk(&tag).unwrap().block_state(0, 0, 0).unwrap(),
            &stone
        );
    }
}
