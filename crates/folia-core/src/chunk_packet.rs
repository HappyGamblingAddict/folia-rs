use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
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
const STONE: i32 = 1;
const GRASS_SNOWY: i32 = 8;
const GRASS: i32 = 9;
const DIRT: i32 = 10;
const BEDROCK: i32 = 85;
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
        }],
    }
}

pub fn block_state_id(state: &BlockState) -> i32 {
    match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => AIR,
        "minecraft:stone" => STONE,
        "minecraft:grass_block" => {
            if state
                .properties
                .iter()
                .any(|(key, value)| key == "snowy" && value == "true")
            {
                GRASS_SNOWY
            } else {
                GRASS
            }
        }
        "minecraft:dirt" => DIRT,
        "minecraft:bedrock" => BEDROCK,
        "minecraft:water" => 86,
        "minecraft:lava" => 102,
        "minecraft:sand" => 118,
        "minecraft:gravel" => 124,
        "minecraft:tuff" => 23250,
        "minecraft:andesite" => 6,
        "minecraft:granite" => 2,
        "minecraft:deepslate" => 27722,
        _ => STONE,
    }
}

pub fn biome_id(name: &str) -> i32 {
    match name {
        "minecraft:the_void" => 57,
        _ => PLAINS_BIOME,
    }
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
        let long_count = pack_heightmap(&values).len();
        writer.write_varint(key).unwrap();
        writer.write_varint(long_count as i32).unwrap();
        for value in pack_heightmap(&values) {
            writer.write_i64(value).unwrap();
        }
    }
    writer.into_inner()
}

pub fn light_bytes(chunk: &Chunk) -> Vec<u8> {
    let heights: Vec<i32> = (0..256)
        .map(|index| {
            let x = index % 16;
            let z = index / 16;
            column_height(chunk, x, z, HeightKind::WorldSurface)
        })
        .collect();

    let highest_section = highest_block_section(chunk);
    let max_sky_index =
        (i32::from(highest_section) - (i32::from(OVERWORLD_MIN_SECTION) - 1) + 1).max(0) as usize;

    let mut sky_mask = 0u64;
    let block_mask = 0u64;
    let mut empty_sky = 0u64;
    let mut empty_block = 0u64;
    let mut sky_updates: Vec<[u8; 2048]> = Vec::new();

    for index in 0..LIGHT_SECTION_COUNT {
        let section_y = i32::from(OVERWORLD_MIN_SECTION) - 1 + index as i32;
        let mut data = [0u8; 2048];
        let mut all_zero = true;
        let mut all_full = true;

        for z in 0..16 {
            for x in 0..16 {
                let height = heights[x + z * 16];
                for local_y in 0..16 {
                    let world_y = section_y * 16 + local_y as i32;
                    let light = if world_y >= height { 15u8 } else { 0u8 };
                    if light == 0 {
                        all_full = false;
                    } else {
                        all_zero = false;
                    }
                    let cell = ((local_y as usize) << 8) | (z << 4) | x;
                    let byte = cell >> 1;
                    if cell & 1 == 0 {
                        data[byte] |= light;
                    } else {
                        data[byte] |= light << 4;
                    }
                }
            }
        }

        if all_zero {
            empty_sky |= 1 << index;
        } else if !all_full || index <= max_sky_index {
            sky_mask |= 1 << index;
            sky_updates.push(data);
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

    let mut block_ids = Vec::with_capacity(4096);
    let mut non_empty = 0i16;
    for index in 0..4096 {
        let state = section
            .block_states
            .get(index)
            .ok_or_else(|| invalid("chunk block palette index is out of range"))?;
        let id = block_state_id(state);
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
}
