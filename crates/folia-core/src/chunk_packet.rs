use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use crate::nbt::Tag;
use crate::protocol::{PacketReader, PacketWriter};
use std::collections::HashMap;
use std::io;

pub const CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT: i32 = 44;
pub const OVERWORLD_MIN_SECTION: i8 = -4;
pub const OVERWORLD_SECTION_COUNT: usize = 24;
pub const OVERWORLD_MIN_Y: i32 = -64;
pub const OVERWORLD_HEIGHT: usize = 384;
pub const LIGHT_SECTION_COUNT: usize = OVERWORLD_SECTION_COUNT + 2;

const AIR: i32 = 0;
const PLAINS_BIOME: i32 = 40;

/// Captured heightmaps/light data reused for flat-world chunks. The chunk data
/// itself is generated from Rust `Chunk` values.
#[derive(Debug, Clone)]
pub struct ChunkPacketTemplate {
    heightmaps: Vec<u8>,
    light: Vec<u8>,
}

impl ChunkPacketTemplate {
    pub fn from_packet(packet: &[u8]) -> io::Result<Self> {
        let mut reader = PacketReader::new(packet);
        let id = reader.read_varint()?;
        if id != CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT {
            return Err(invalid(format!("expected chunk packet id 44, got {id}")));
        }
        let _x = reader.read_i32()?;
        let _z = reader.read_i32()?;

        let heightmap_start = reader.position();
        let heightmap_count = reader.read_varint()?;
        for _ in 0..heightmap_count {
            let _kind = reader.read_varint()?;
            let longs = reader.read_varint()?;
            let _ = reader.read_bytes(longs as usize * 8)?;
        }
        let heightmaps = packet[heightmap_start..reader.position()].to_vec();

        let chunk_data_length = reader.read_varint()?;
        let _ = reader.read_bytes(chunk_data_length as usize)?;
        let block_entities = reader.read_varint()?;
        for _ in 0..block_entities {
            let _ = reader.read_u8()?;
            let _ = reader.read_bytes(2)?;
            let _ = reader.read_varint()?;
            let _ = reader.read_nbt_any()?;
        }
        let light = packet[reader.position()..].to_vec();

        Ok(Self { heightmaps, light })
    }

    pub fn from_first_captured_chunk() -> io::Result<Self> {
        for packet in crate::join::play_packets() {
            if packet_id(packet)? == CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT {
                return Self::from_packet(packet);
            }
        }
        Err(invalid("captured play stream contains no chunk packet"))
    }

    pub fn captured_heightmaps(&self) -> &[u8] {
        &self.heightmaps
    }

    pub fn captured_light(&self) -> &[u8] {
        &self.light
    }

    pub fn build_with_captured(&self, chunk: &Chunk) -> io::Result<Vec<u8>> {
        let chunk_data = build_chunk_data(chunk)?;
        let mut writer = PacketWriter::new();
        writer.write_varint(CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT)?;
        writer.write_i32(chunk.x)?;
        writer.write_i32(chunk.z)?;
        writer.write_bytes(&self.heightmaps);
        writer.write_varint(chunk_data.len() as i32)?;
        writer.write_bytes(&chunk_data);
        writer.write_varint(0)?; // block entities
        writer.write_bytes(&self.light);
        Ok(writer.into_inner())
    }

    /// Build a packet using heightmaps and lighting computed from the chunk
    /// itself. The captured template is only needed for tests/differentials.
    pub fn build_dynamic(chunk: &Chunk) -> io::Result<Vec<u8>> {
        let chunk_data = build_chunk_data(chunk)?;
        let heightmaps = heightmap_bytes(chunk);
        let light = light_bytes(chunk);
        let mut writer = PacketWriter::new();
        writer.write_varint(CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT)?;
        writer.write_i32(chunk.x)?;
        writer.write_i32(chunk.z)?;
        writer.write_bytes(&heightmaps);
        writer.write_varint(chunk_data.len() as i32)?;
        writer.write_bytes(&chunk_data);
        writer.write_varint(0)?; // block entities
        writer.write_bytes(&light);
        Ok(writer.into_inner())
    }
}

pub fn flat_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
    let bedrock = BlockState {
        name: "minecraft:bedrock".into(),
        properties: vec![],
    };
    let dirt = BlockState {
        name: "minecraft:dirt".into(),
        properties: vec![],
    };
    let grass = BlockState {
        name: "minecraft:grass_block".into(),
        properties: vec![("snowy".into(), "false".into())],
    };
    let air = BlockState {
        name: "minecraft:air".into(),
        properties: vec![],
    };

    let mut indices = vec![3u16; 4096];
    for z in 0..16 {
        for x in 0..16 {
            for (local_y, palette) in [(0usize, 0u16), (1, 1), (2, 1), (3, 2)] {
                indices[(local_y * 16 + z) * 16 + x] = palette;
            }
        }
    }

    Chunk {
        x: chunk_x,
        z: chunk_z,
        sections: vec![ChunkSection {
            y: OVERWORLD_MIN_SECTION,
            block_states: PalettedContainer {
                palette: vec![bedrock, dirt, grass, air],
                indices,
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: crate::light::SectionLight::default(),
        }],
    }
}

pub use crate::registry::block_state_id;

pub fn biome_id(name: &str) -> i32 {
    static IDS: std::sync::OnceLock<std::collections::HashMap<&'static str, i32>> =
        std::sync::OnceLock::new();
    let ids = IDS.get_or_init(|| {
        include_str!("../assets/biome_ids.txt")
            .lines()
            .filter_map(|line| {
                let (id, name) = line.split_once('\t')?;
                Some((name, id.parse().ok()?))
            })
            .collect()
    });
    *ids.get(name).unwrap_or(&PLAINS_BIOME)
}

fn build_chunk_data(chunk: &Chunk) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    for index in 0..OVERWORLD_SECTION_COUNT {
        let section_y = OVERWORLD_MIN_SECTION + index as i8;
        match chunk.section(section_y) {
            Some(section) => write_section(&mut writer, section)?,
            None => write_empty_section(&mut writer)?,
        }
    }
    Ok(writer.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeightKind {
    WorldSurface,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

pub fn heightmap_bytes(chunk: &Chunk) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(3).unwrap();
    for (kind, key) in [
        (HeightKind::MotionBlocking, 4),
        (HeightKind::MotionBlockingNoLeaves, 5),
        (HeightKind::WorldSurface, 1),
    ] {
        let mut values = Vec::with_capacity(256);
        for z in 0..16 {
            for x in 0..16 {
                let height = column_height(chunk, x, z, kind);
                values.push((height - OVERWORLD_MIN_Y) as u16);
            }
        }
        let packed = pack_heightmap(&values);
        writer.write_varint(key).unwrap();
        writer.write_varint(packed.len() as i32).unwrap();
        for value in packed {
            writer.write_i64(value).unwrap();
        }
    }
    writer.into_inner()
}

/// The same heightmap values the chunk packet carries, in the NBT shape a
/// stored chunk uses (three named 9-bit long arrays).
pub fn heightmap_nbt(chunk: &Chunk) -> Tag {
    let mut entries = Vec::with_capacity(3);
    for (kind, name) in [
        (HeightKind::MotionBlocking, "MOTION_BLOCKING"),
        (
            HeightKind::MotionBlockingNoLeaves,
            "MOTION_BLOCKING_NO_LEAVES",
        ),
        (HeightKind::WorldSurface, "WORLD_SURFACE"),
    ] {
        let values: Vec<u16> = (0..256)
            .map(|index| {
                let height = column_height(chunk, index % 16, index / 16, kind);
                (height - OVERWORLD_MIN_Y) as u16
            })
            .collect();
        entries.push((name.into(), Tag::LongArray(pack_heightmap(&values))));
    }
    Tag::Compound(entries)
}

pub fn light_bytes(chunk: &Chunk) -> Vec<u8> {
    if chunk.has_light() {
        stored_light_bytes(chunk)
    } else {
        // Preserve the original height-based payload for chunks that have no
        // real light data yet (for example a freshly generated column before it
        // is passed through the light engine).
        height_light_bytes(chunk)
    }
}

/// Per-column sky light computed from the heightmap plus a short attenuation
/// walk through the blocks directly below it. This is the fallback for chunks
/// that do not have stored light layers yet: unlike the old heightmap-only
/// approximation it keeps a soft shadow under tree canopies instead of making
/// the ground pitch black.
///
/// The walk is limited to [`FALLBACK_ATTENUATION_DEPTH`] blocks; deeper light
/// (caves, deep water) is dark until the chunk passes through the real light
/// engine. The cost is one heightmap scan plus at most eight block lookups per
/// column, so it stays cheap for large join batches.
const FALLBACK_ATTENUATION_DEPTH: i32 = 8;

fn column_sky_light(chunk: &Chunk, x: usize, z: usize) -> Vec<u8> {
    let height = column_height(chunk, x, z, HeightKind::WorldSurface);
    let mut levels = vec![0u8; OVERWORLD_HEIGHT];
    let mut light = 15u8;
    for y in (OVERWORLD_MIN_Y..OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32).rev() {
        let index = (y - OVERWORLD_MIN_Y) as usize;
        if y >= height {
            levels[index] = 15;
            continue;
        }
        if y < height - FALLBACK_ATTENUATION_DEPTH {
            levels[index] = 0;
            continue;
        }
        let opacity = chunk
            .block_state(x, y, z)
            .map(|state| crate::light::light_block(crate::registry::block_state_id(state)))
            .unwrap_or(0);
        light = light.saturating_sub(opacity);
        levels[index] = light;
    }
    levels
}

/// Build the light payload from the chunk's stored/computed layers. Sections
/// without stored layers inside the height approximation's range fall back to
/// the per-column sky walk, which keeps captured flat-world packets
/// byte-identical while real layers take precedence.
fn stored_light_bytes(chunk: &Chunk) -> Vec<u8> {
    let columns: Vec<Vec<u8>> = (0..256)
        .map(|index| column_sky_light(chunk, index % 16, index / 16))
        .collect();
    let max_sky_index = max_sky_index(chunk);

    let mut sky_mask = 0u64;
    let mut block_mask = 0u64;
    let mut empty_sky = 0u64;
    let mut empty_block = 0u64;
    let mut sky_updates: Vec<crate::light::DataLayer> = Vec::new();
    let mut block_updates: Vec<crate::light::DataLayer> = Vec::new();

    for index in 0..LIGHT_SECTION_COUNT {
        let section_y = i32::from(OVERWORLD_MIN_SECTION) - 1 + index as i32;
        let section_light = chunk.section(section_y as i8).map(|section| &section.light);
        let stored_sky = section_light.and_then(|light| light.sky.as_ref());
        let stored_block = section_light.and_then(|light| light.block.as_ref());
        let within_fallback = index <= max_sky_index;
        if stored_sky.is_none() && stored_block.is_none() && !within_fallback {
            continue;
        }

        let (sky, sky_from_fallback) = match stored_sky {
            Some(layer) => (layer.clone(), false),
            None => (fallback_sky_layer(&columns, section_y), true),
        };
        let block = match stored_block {
            Some(layer) => layer.clone(),
            None => crate::light::DataLayer::new(),
        };

        if sky.is_empty() {
            empty_sky |= 1 << index;
        } else if !sky_from_fallback || !sky.is_filled_with(15) || index <= max_sky_index {
            sky_mask |= 1 << index;
            sky_updates.push(sky);
        }

        if block.is_empty() {
            empty_block |= 1 << index;
        } else {
            block_mask |= 1 << index;
            block_updates.push(block);
        }
    }

    let mut writer = PacketWriter::new();
    write_bitset(&mut writer, sky_mask);
    write_bitset(&mut writer, block_mask);
    write_bitset(&mut writer, empty_sky);
    write_bitset(&mut writer, empty_block);
    writer.write_varint(sky_updates.len() as i32).unwrap();
    for update in sky_updates {
        writer.write_varint(2048).unwrap();
        writer.write_bytes(update.as_bytes());
    }
    writer.write_varint(block_updates.len() as i32).unwrap();
    for update in block_updates {
        writer.write_varint(2048).unwrap();
        writer.write_bytes(update.as_bytes());
    }
    writer.into_inner()
}

fn height_light_bytes(chunk: &Chunk) -> Vec<u8> {
    let columns: Vec<Vec<u8>> = (0..256)
        .map(|index| column_sky_light(chunk, index % 16, index / 16))
        .collect();
    let max_sky_index = max_sky_index(chunk);

    let mut sky_mask = 0u64;
    let block_mask = 0u64;
    let mut empty_sky = 0u64;
    let mut empty_block = 0u64;
    let mut sky_updates: Vec<[u8; 2048]> = Vec::new();

    for index in 0..LIGHT_SECTION_COUNT {
        let section_y = i32::from(OVERWORLD_MIN_SECTION) - 1 + index as i32;
        let layer = fallback_sky_layer(&columns, section_y);
        let all_zero = layer.is_empty();
        let all_full = layer.is_filled_with(15);

        if all_zero {
            empty_sky |= 1 << index;
        } else if !all_full || index <= max_sky_index {
            sky_mask |= 1 << index;
            sky_updates.push(*layer.as_bytes());
        }

        if index <= max_sky_index && index < LIGHT_SECTION_COUNT {
            empty_block |= 1 << index;
        }
    }

    let mut writer = PacketWriter::new();
    write_bitset(&mut writer, sky_mask);
    write_bitset(&mut writer, block_mask);
    write_bitset(&mut writer, empty_sky);
    write_bitset(&mut writer, empty_block);
    writer.write_varint(sky_updates.len() as i32).unwrap();
    for update in sky_updates {
        writer.write_varint(2048).unwrap();
        writer.write_bytes(&update);
    }
    writer.write_varint(0).unwrap(); // block updates
    writer.into_inner()
}

fn fallback_sky_layer(columns: &[Vec<u8>], section_y: i32) -> crate::light::DataLayer {
    let mut layer = crate::light::DataLayer::new();
    for z in 0..16 {
        for x in 0..16 {
            let column = &columns[x + z * 16];
            for local_y in 0..16 {
                let world_y = section_y * 16 + local_y;
                let index = world_y - OVERWORLD_MIN_Y;
                let light = if index >= OVERWORLD_HEIGHT as i32 {
                    15
                } else if index < 0 {
                    0
                } else {
                    column[index as usize]
                };
                if light > 0 {
                    layer.set(x, local_y as usize, z, light);
                }
            }
        }
    }
    layer
}

fn max_sky_index(chunk: &Chunk) -> usize {
    let highest_section = highest_block_section(chunk);
    (i32::from(highest_section) - (i32::from(OVERWORLD_MIN_SECTION) - 1) + 1).max(0) as usize
}

fn write_bitset(writer: &mut PacketWriter, mask: u64) {
    if mask == 0 {
        writer.write_varint(0).unwrap();
    } else {
        writer.write_varint(1).unwrap();
        writer.write_i64(mask as i64).unwrap();
    }
}

fn highest_block_section(chunk: &Chunk) -> i8 {
    chunk
        .sections
        .iter()
        .filter(|section| {
            section.block_states.indices.iter().any(|&index| {
                section
                    .block_states
                    .palette
                    .get(index as usize)
                    .is_some_and(|state| !is_air(state))
            })
        })
        .map(|section| section.y)
        .max()
        .unwrap_or(OVERWORLD_MIN_SECTION)
}

fn column_height(chunk: &Chunk, x: usize, z: usize, kind: HeightKind) -> i32 {
    let minimum = OVERWORLD_MIN_Y;
    let maximum = OVERWORLD_MIN_Y + OVERWORLD_HEIGHT as i32 - 1;
    for y in (minimum..=maximum).rev() {
        let Some(state) = chunk.block_state(x, y, z) else {
            continue;
        };
        let matches = match kind {
            HeightKind::WorldSurface => !is_air(state),
            HeightKind::MotionBlocking => !is_air(state),
            HeightKind::MotionBlockingNoLeaves => !is_air(state) && !is_leaves(state),
        };
        if matches {
            return y + 1;
        }
    }
    minimum
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

fn is_leaves(state: &BlockState) -> bool {
    state.name.ends_with("_leaves") || state.name.contains("leaves")
}

fn pack_heightmap(values: &[u16]) -> Vec<i64> {
    const BITS: usize = 9;
    const PER_LONG: usize = 64 / BITS;
    let mut longs = vec![0u64; values.len().div_ceil(PER_LONG)];
    for (index, &value) in values.iter().enumerate() {
        let shift = (index % PER_LONG) * BITS;
        longs[index / PER_LONG] |= u64::from(value) << shift;
    }
    longs.into_iter().map(|value| value as i64).collect()
}

fn write_empty_section(writer: &mut PacketWriter) -> io::Result<()> {
    writer.write_bytes(&0i16.to_be_bytes()); // non-empty block count
    writer.write_u8(0)?; // block states: single-value palette
    writer.write_varint(AIR)?;
    writer.write_u8(0)?; // biomes: single-value palette
    writer.write_varint(PLAINS_BIOME)?;
    Ok(())
}

fn write_section(writer: &mut PacketWriter, section: &ChunkSection) -> io::Result<()> {
    if section.block_states.len() != 4096 {
        return Err(invalid("chunk section must contain 4096 block states"));
    }

    // Resolve each palette entry once instead of hashing every block state's
    // name and properties 4096 times per section. A section usually has a
    // small palette, so this turns the packet's dominant cost into palette
    // lookups plus a linear index pass.
    let mut palette_ids = Vec::with_capacity(section.block_states.palette.len());
    for state in &section.block_states.palette {
        palette_ids.push(block_state_id(state));
    }

    let mut block_ids = Vec::with_capacity(4096);
    let mut non_empty = 0i16;
    for &palette_index in &section.block_states.indices {
        let id = palette_ids
            .get(palette_index as usize)
            .copied()
            .ok_or_else(|| invalid("chunk block palette index is out of range"))?;
        if id != AIR {
            non_empty = non_empty.saturating_add(1);
        }
        block_ids.push(id);
    }

    writer.write_bytes(&non_empty.to_be_bytes());
    write_paletted(writer, &block_ids, 4)?;

    let biome_ids: Vec<i32> = match &section.biomes {
        Some(biomes) if biomes.len() == 64 => {
            let mut ids = Vec::with_capacity(64);
            for index in 0..64 {
                let biome = biomes
                    .get(index)
                    .ok_or_else(|| invalid("chunk biome palette index is out of range"))?;
                ids.push(biome_id(biome));
            }
            ids
        }
        _ => vec![PLAINS_BIOME; 64],
    };
    write_paletted(writer, &biome_ids, 1)?;
    Ok(())
}

fn write_paletted(writer: &mut PacketWriter, values: &[i32], minimum_bits: u8) -> io::Result<()> {
    let mut palette = Vec::new();
    let mut lookup: HashMap<i32, u16> = HashMap::new();
    let mut indices = Vec::with_capacity(values.len());

    for &value in values {
        let palette_index = match lookup.get(&value) {
            Some(&index) => index,
            None => {
                let index = palette.len() as u16;
                palette.push(value);
                lookup.insert(value, index);
                index
            }
        };
        indices.push(palette_index);
    }

    if palette.len() == 1 {
        writer.write_u8(0)?;
        writer.write_varint(palette[0])?;
        return Ok(());
    }

    let bits = minimum_bits.max(ceil_log2(palette.len()));
    if bits > 32 {
        return Err(invalid("palette needs more than 32 bits per entry"));
    }
    writer.write_u8(bits)?;
    writer.write_varint(palette.len() as i32)?;
    for value in &palette {
        writer.write_varint(*value)?;
    }

    let per_long = (64 / bits) as usize;
    let mut longs = vec![0u64; indices.len().div_ceil(per_long)];
    for (index, value) in indices.iter().copied().enumerate() {
        let long_index = index / per_long;
        let shift = ((index % per_long) * bits as usize) as u32;
        longs[long_index] |= u64::from(value) << shift;
    }
    for value in longs {
        writer.write_bytes(&value.to_be_bytes());
    }
    Ok(())
}

fn ceil_log2(value: usize) -> u8 {
    if value <= 1 {
        0
    } else {
        (usize::BITS - (value - 1).leading_zeros()) as u8
    }
}

fn packet_id(packet: &[u8]) -> io::Result<i32> {
    let mut reader = PacketReader::new(packet);
    reader.read_varint()
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRASS: i32 = 9;
    const DIRT: i32 = 10;
    const BEDROCK: i32 = 85;

    #[test]
    fn biome_registry_ids_match_java() {
        assert_eq!(biome_id("minecraft:ocean"), 35);
        assert_eq!(biome_id("minecraft:plains"), 40);
        assert_eq!(biome_id("minecraft:the_void"), 57);
    }

    #[test]
    fn column_sky_light_attenuates_through_leaves() {
        let mut chunk = flat_chunk(0, 0);
        chunk.set_block_state(
            0,
            -60,
            0,
            BlockState {
                name: "minecraft:oak_leaves".into(),
                properties: vec![
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ],
            },
        );
        let column = column_sky_light(&chunk, 0, 0);
        let level = |y: i32| column[(y - OVERWORLD_MIN_Y) as usize];
        assert_eq!(level(-59), 15, "open sky above the leaf");
        assert_eq!(level(-60), 14, "leaf attenuates by one level");
        assert_eq!(level(-61), 0, "solid ground below the canopy");
    }

    #[test]
    fn live_flat_chunk_packet_has_block_and_biome_palettes() {
        let packet = ChunkPacketTemplate::build_dynamic(&flat_chunk(3, 4)).unwrap();
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), 44);
        assert_eq!(reader.read_i32().unwrap(), 3);
        assert_eq!(reader.read_i32().unwrap(), 4);

        let heightmap_count = reader.read_varint().unwrap();
        assert!(heightmap_count > 0);
        for _ in 0..heightmap_count {
            let _ = reader.read_varint().unwrap();
            let longs = reader.read_varint().unwrap();
            let _ = reader.read_bytes(longs as usize * 8).unwrap();
        }

        let chunk_data_length = reader.read_varint().unwrap();
        let chunk_data = reader
            .read_bytes(chunk_data_length as usize)
            .unwrap()
            .to_vec();
        assert!(chunk_data.len() > 2000);

        let mut data = PacketReader::new(&chunk_data);
        let non_empty = data.read_bytes(2).unwrap();
        assert!(i16::from_be_bytes([non_empty[0], non_empty[1]]) > 0);
        let block_bits = data.read_u8().unwrap();
        assert!(block_bits >= 4);
        let block_palette_size = data.read_varint().unwrap();
        assert!(block_palette_size >= 2);
        let mut block_palette = Vec::new();
        for _ in 0..block_palette_size {
            block_palette.push(data.read_varint().unwrap());
        }
        assert!(block_palette.contains(&BEDROCK));
        assert!(block_palette.contains(&DIRT));
        assert!(block_palette.contains(&GRASS));
        assert!(block_palette.contains(&AIR));
    }

    #[test]
    fn flat_chunk_has_bedrock_dirt_grass() {
        let chunk = flat_chunk(0, 0);
        assert_eq!(
            chunk.block_state(0, -64, 0).unwrap().name,
            "minecraft:bedrock"
        );
        assert_eq!(chunk.block_state(0, -63, 0).unwrap().name, "minecraft:dirt");
        assert_eq!(
            chunk.block_state(0, -61, 0).unwrap().name,
            "minecraft:grass_block"
        );
        assert_eq!(chunk.block_state(0, -60, 0).unwrap().name, "minecraft:air");
    }

    #[test]
    fn block_state_ids_match_12111() {
        assert_eq!(
            block_state_id(&flat_chunk(0, 0).sections[0].block_states.palette[0]),
            85
        );
        assert_eq!(
            block_state_id(&flat_chunk(0, 0).sections[0].block_states.palette[1]),
            10
        );
        assert_eq!(
            block_state_id(&flat_chunk(0, 0).sections[0].block_states.palette[2]),
            9
        );
        assert_eq!(
            block_state_id(&flat_chunk(0, 0).sections[0].block_states.palette[3]),
            0
        );
    }

    #[test]
    fn encoded_blocks_resolve_every_palette_entry_to_its_state_id() {
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        };
        let grass = BlockState {
            name: "minecraft:grass_block".into(),
            properties: vec![("snowy".into(), "false".into())],
        };
        let snowy_grass = BlockState {
            name: "minecraft:grass_block".into(),
            properties: vec![("snowy".into(), "true".into())],
        };
        let source_palette = vec![air.clone(), grass.clone(), snowy_grass.clone()];
        let source_indices: Vec<u16> = (0..4096).map(|index| (index % 3) as u16).collect();
        let chunk = Chunk {
            x: 0,
            z: 0,
            sections: vec![ChunkSection {
                y: OVERWORLD_MIN_SECTION,
                block_states: PalettedContainer {
                    palette: source_palette,
                    indices: source_indices,
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
                    indices: vec![0; 64],
                }),
                light: crate::light::SectionLight::default(),
            }],
        };

        let packet = ChunkPacketTemplate::build_dynamic(&chunk).unwrap();
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), 44);
        let _ = reader.read_i32().unwrap();
        let _ = reader.read_i32().unwrap();
        let height_map_count = reader.read_varint().unwrap();
        for _ in 0..height_map_count {
            let _ = reader.read_varint().unwrap();
            let longs = reader.read_varint().unwrap();
            let _ = reader.read_bytes(longs as usize * 8).unwrap();
        }
        let chunk_data_length = reader.read_varint().unwrap();
        let chunk_data = reader
            .read_bytes(chunk_data_length as usize)
            .unwrap()
            .to_vec();

        let mut data = PacketReader::new(&chunk_data);
        let _ = data.read_bytes(2).unwrap();
        let bits = data.read_u8().unwrap();
        assert!(bits >= 4);
        let palette_length = data.read_varint().unwrap();
        let encoded_palette: Vec<i32> = (0..palette_length)
            .map(|_| data.read_varint().unwrap())
            .collect();
        let expected_palette = vec![
            block_state_id(&air),
            block_state_id(&grass),
            block_state_id(&snowy_grass),
        ];
        assert_eq!(encoded_palette, expected_palette);

        let per_long = 64 / bits as usize;
        let long_count = 4096usize.div_ceil(per_long);
        let packed: Vec<u64> = (0..long_count)
            .map(|_| data.read_i64().unwrap() as u64)
            .collect();
        let mask = (1u64 << bits) - 1;
        for block in 0..4096 {
            let long = packed[block / per_long];
            let shift = (block % per_long) * bits as usize;
            let palette_index = ((long >> shift) & mask) as usize;
            assert_eq!(
                encoded_palette[palette_index],
                expected_palette[block % 3],
                "block {block} mapped to the wrong palette entry"
            );
        }
    }
}
