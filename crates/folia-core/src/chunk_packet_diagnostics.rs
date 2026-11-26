//! P3.11 deterministic heightmap and chunk-packet fixtures.
//!
//! The golden produced here is **Rust-only**: it pins the wire shape of the
//! heightmaps, light masks and section palettes for several generated chunks,
//! plus one hand-built edge chunk with negative Y. The Java captures in
//! `join_play.bin` are flat-world packets: they are parsed and digested here,
//! but no Java byte parity is claimed for these generated fixtures. Captures
//! for them are reported as missing by [`ChunkPacketCoverage`].

use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer, is_air_state};
use crate::chunk_packet::{
    self, ChunkPacketTemplate, OVERWORLD_HEIGHT, OVERWORLD_MIN_Y, block_state_id, heightmap_bytes,
    light_bytes, pack_heightmap,
};
use crate::lifecycle_diagnostics::{Coverage, Digest, body_value, digest_hex, fnv1a64};
use crate::light::SectionLight;
use crate::protocol::PacketReader;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::generator::WorldGenerator;
use folia_math::BlockPos;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

pub const DUMP_FORMAT_VERSION: u32 = 1;
pub const DUMP_DATA_VERSION: i32 = 4788;

/// Every exported `Heightmap.Types` value, in the order used by the dump.
pub const HEIGHTMAP_TYPES: [(&str, Heightmap); 6] = [
    ("WORLD_SURFACE", Heightmap::WorldSurface),
    ("WORLD_SURFACE_WG", Heightmap::WorldSurfaceWg),
    ("OCEAN_FLOOR", Heightmap::OceanFloor),
    ("OCEAN_FLOOR_WG", Heightmap::OceanFloorWg),
    ("MOTION_BLOCKING", Heightmap::MotionBlocking),
    (
        "MOTION_BLOCKING_NO_LEAVES",
        Heightmap::MotionBlockingNoLeaves,
    ),
];

/// Four distinct seeds and chunk positions, plus a hand-built edge chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureChunk {
    pub name: &'static str,
    pub seed: Option<i64>,
    pub chunk: (i32, i32),
}

pub const FIXTURES: [FixtureChunk; 5] = [
    FixtureChunk {
        name: "seed=12345 chunk=(0,-1)",
        seed: Some(12_345),
        chunk: (0, -1),
    },
    FixtureChunk {
        name: "seed=0 chunk=(0,0)",
        seed: Some(0),
        chunk: (0, 0),
    },
    FixtureChunk {
        name: "seed=1 chunk=(1,-1)",
        seed: Some(1),
        chunk: (1, -1),
    },
    FixtureChunk {
        name: "seed=-1 chunk=(-1,0)",
        seed: Some(-1),
        chunk: (-1, 0),
    },
    FixtureChunk {
        name: "synthetic-edge",
        seed: None,
        chunk: (0, 0),
    },
];

/// Inputs required before any of these fixtures can claim Java byte parity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaPacketInput {
    pub name: String,
    pub purpose: String,
    pub present: bool,
}

pub fn required_java_inputs() -> Vec<JavaPacketInput> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    FIXTURES
        .iter()
        .enumerate()
        .filter(|(_, fixture)| fixture.seed.is_some())
        .map(|(index, fixture)| {
            let name = format!("java-chunk-packet-fixture-{index}.bin");
            JavaPacketInput {
                present: root.join(&name).exists(),
                name,
                purpose: format!(
                    "captured ClientboundLevelChunkWithLight body for {} (heightmaps, light masks, block/biome palettes)",
                    fixture.name
                ),
            }
        })
        .collect()
}

pub fn missing_java_inputs() -> Vec<JavaPacketInput> {
    required_java_inputs()
        .into_iter()
        .filter(|input| !input.present)
        .collect()
}

/// Coverage audit for the P3.11 fixtures. Reuses the P3.12 coverage vocabulary
/// so both harnesses report `java-verified` / `rust-golden` / `unavailable`
/// consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPacketCoverage {
    records: BTreeMap<String, Coverage>,
    java_available: bool,
    golden_available: bool,
}

impl ChunkPacketCoverage {
    pub fn audit() -> Self {
        let java_available = required_java_inputs().iter().all(|input| input.present);
        let golden_available = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("chunk_packet_golden.json")
            .exists();
        let coverage = if java_available {
            Coverage::JavaVerified
        } else if golden_available {
            Coverage::RustGolden
        } else {
            Coverage::Unavailable
        };
        Self {
            records: FIXTURES
                .iter()
                .map(|fixture| (fixture.name.to_owned(), coverage))
                .collect(),
            java_available,
            golden_available,
        }
    }

    pub fn record(&self, fixture: &str) -> Option<Coverage> {
        self.records.get(fixture).copied()
    }

    pub fn java_differential(&self) -> Coverage {
        if self.java_available {
            Coverage::JavaVerified
        } else {
            Coverage::Unavailable
        }
    }

    pub fn records(&self) -> &BTreeMap<String, Coverage> {
        &self.records
    }

    pub fn golden_available(&self) -> bool {
        self.golden_available
    }
}

impl fmt::Display for ChunkPacketCoverage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for coverage in [
            Coverage::JavaVerified,
            Coverage::RustGolden,
            Coverage::Unavailable,
        ] {
            writeln!(formatter, "{} fixtures:", coverage.as_str())?;
            let names: Vec<&str> = self
                .records
                .iter()
                .filter(|(_, value)| **value == coverage)
                .map(|(name, _)| name.as_str())
                .collect();
            if names.is_empty() {
                writeln!(formatter, "  (none)")?;
            } else {
                for name in names {
                    writeln!(formatter, "  {name}")?;
                }
            }
        }
        writeln!(
            formatter,
            "java differential: {}",
            self.java_differential().as_str()
        )?;
        writeln!(
            formatter,
            "flat-world Java captures in join_play.bin: {} (not generated fixtures)",
            chunk_packet::captured_chunk_packets().len()
        )?;
        let missing = missing_java_inputs();
        if missing.is_empty() {
            writeln!(formatter, "missing Java capture inputs: (none)")?;
        } else {
            writeln!(formatter, "missing Java capture inputs:")?;
            for input in missing {
                writeln!(formatter, "  {}: {}", input.name, input.purpose)?;
            }
        }
        Ok(())
    }
}

/// A minimal read-only [`FeatureLevel`] used to compute all six heightmaps
/// from the same predicate the placement pipeline uses.
struct ChunkHeightLevel<'a> {
    chunk: &'a Chunk,
}

impl FeatureLevel for ChunkHeightLevel<'_> {
    fn min_y(&self) -> i32 {
        OVERWORLD_MIN_Y
    }

    fn height(&self) -> i32 {
        OVERWORLD_HEIGHT as i32
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        let x = pos.x() - self.chunk.x * 16;
        let z = pos.z() - self.chunk.z * 16;
        if !(0..16).contains(&x) || !(0..16).contains(&z) {
            return None;
        }
        self.chunk.block_state(x as usize, pos.y(), z as usize)
    }

    fn set_block(&mut self, _pos: BlockPos, _state: BlockState) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkPacketDump {
    value: Value,
}

impl ChunkPacketDump {
    pub fn build() -> io::Result<Self> {
        let mut chunks = Vec::with_capacity(FIXTURES.len());
        for fixture in FIXTURES {
            let chunk = match fixture.seed {
                Some(seed) => WorldGenerator::overworld(seed)?
                    .generate_chunk(fixture.chunk.0, fixture.chunk.1)?,
                None => edge_chunk(fixture.chunk.0, fixture.chunk.1),
            };
            chunks.push(fixture_value(&fixture, &chunk)?);
        }
        let flat = chunk_packet::captured_chunk_packets();
        let flat_coords: Vec<Value> = flat.iter().map(|(x, z, _)| json!([x, z])).collect();
        let mut flat_digest = Digest::new();
        for (_, _, packet) in &flat {
            flat_digest.bytes(packet);
        }
        Ok(Self {
            value: json!({
                "format_version": DUMP_FORMAT_VERSION,
                "data_version": DUMP_DATA_VERSION,
                "heightmap_types": HEIGHTMAP_TYPES.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                "chunks": chunks,
                "java_flat_captures": {
                    "count": flat.len(),
                    "coords": flat_coords,
                    "digest": digest_hex(flat_digest.finish()),
                },
            }),
        })
    }

    pub fn to_value(&self) -> &Value {
        &self.value
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(&self.value).expect("chunk packet dump is JSON serializable")
    }

    pub fn digest(&self) -> u64 {
        fnv1a64(self.to_json_string().as_bytes())
    }
}

fn fixture_value(fixture: &FixtureChunk, chunk: &Chunk) -> io::Result<Value> {
    let mut heightmaps = Map::new();
    let level = ChunkHeightLevel { chunk };
    let base_x = chunk.x * 16;
    let base_z = chunk.z * 16;
    for (name, kind) in HEIGHTMAP_TYPES {
        let mut values = Vec::with_capacity(256);
        for z in 0..16 {
            for x in 0..16 {
                let height = level
                    .heightmap(kind, base_x + x, base_z + z)
                    .unwrap_or(OVERWORLD_MIN_Y);
                values.push((height - OVERWORLD_MIN_Y) as u16);
            }
        }
        let packed = pack_heightmap(&values);
        let mut packed_bytes = Vec::with_capacity(packed.len() * 8);
        for value in &packed {
            packed_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let min_y = values
            .iter()
            .map(|&value| i32::from(value) + OVERWORLD_MIN_Y)
            .min()
            .unwrap_or(OVERWORLD_MIN_Y);
        let max_y = values
            .iter()
            .map(|&value| i32::from(value) + OVERWORLD_MIN_Y)
            .max()
            .unwrap_or(OVERWORLD_MIN_Y);
        let negative_columns = values
            .iter()
            .filter(|&&value| i32::from(value) + OVERWORLD_MIN_Y < 0)
            .count();
        heightmaps.insert(
            (*name).to_owned(),
            json!({
                "packed_longs": packed.len(),
                "packed_digest": digest_hex(fnv1a64(&packed_bytes)),
                "min_y": min_y,
                "max_y": max_y,
                "negative_columns": negative_columns,
            }),
        );
    }

    let packet = ChunkPacketTemplate::build_dynamic(chunk)?;
    let chunk_data = chunk_data_bytes(&packet)?;
    let sections: Vec<Value> = chunk
        .sections
        .iter()
        .filter(|section| {
            section.block_states.indices.iter().any(|&index| {
                section
                    .block_states
                    .get(index as usize)
                    .is_some_and(|state| !is_air_state(state))
            })
        })
        .map(|section| {
            let mut ids = Vec::with_capacity(section.block_states.palette.len());
            for state in &section.block_states.palette {
                ids.push(block_state_id(state));
            }
            let mut id_bytes = Vec::with_capacity(ids.len() * 4);
            for id in &ids {
                id_bytes.extend_from_slice(&id.to_le_bytes());
            }
            let biomes = section
                .biomes
                .as_ref()
                .map(|biomes| {
                    let mut names = biomes.palette.clone();
                    names.sort();
                    names
                })
                .unwrap_or_default();
            json!({
                "y": section.y,
                "block_palette_len": ids.len(),
                "block_palette_digest": digest_hex(fnv1a64(&id_bytes)),
                "block_palette_first": ids.iter().take(8).collect::<Vec<_>>(),
                "biome_palette": biomes,
            })
        })
        .collect();

    Ok(json!({
        "name": fixture.name,
        "seed": fixture.seed,
        "chunk": [chunk.x, chunk.z],
        "sections": sections,
        "heightmaps": heightmaps,
        "packet_heightmaps": body_value(&heightmap_bytes(chunk)),
        "chunk_data": body_value(&chunk_data),
        "light": body_value(&light_bytes(chunk)),
        "packet": body_value(&packet),
    }))
}

fn chunk_data_bytes(packet: &[u8]) -> io::Result<Vec<u8>> {
    let mut reader = PacketReader::new(packet);
    let _id = reader.read_varint()?;
    let _x = reader.read_i32()?;
    let _z = reader.read_i32()?;
    let heightmaps = reader.read_varint()?;
    for _ in 0..heightmaps {
        let _kind = reader.read_varint()?;
        let longs = reader.read_varint()?;
        let _ = reader.read_bytes(longs as usize * 8)?;
    }
    let len = reader.read_varint()?;
    Ok(reader.read_bytes(len as usize)?.to_vec())
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn edge_section(y: i8, biomes: &[&str]) -> ChunkSection {
    let palette = biomes.iter().map(|name| (*name).to_owned()).collect();
    let indices = if biomes.len() == 1 {
        vec![0; 64]
    } else {
        (0..64).map(|index| (index % biomes.len()) as u16).collect()
    };
    ChunkSection {
        y,
        block_states: PalettedContainer {
            palette: vec![state("minecraft:air")],
            indices: vec![0; 4096],
        },
        biomes: Some(PalettedContainer { palette, indices }),
        light: SectionLight::default(),
    }
}

/// Hand-built chunk that exercises negative Y, three separate sections,
/// fluids, leaves, snowy grass properties, and per-section biome palettes.
pub fn edge_chunk(x: i32, z: i32) -> Chunk {
    let mut chunk = Chunk {
        status: crate::chunk::ChunkStatus::Full,
        x,
        z,
        light_present: false,
        sections: vec![
            edge_section(-4, &["minecraft:deep_ocean"]),
            edge_section(0, &["minecraft:plains", "minecraft:forest"]),
            edge_section(4, &["minecraft:forest"]),
        ],
    };
    let bedrock = state("minecraft:bedrock");
    let water = state("minecraft:water");
    let stone = state("minecraft:stone");
    let grass = BlockState {
        name: "minecraft:grass_block".into(),
        properties: vec![("snowy".into(), "false".into())],
    };
    let leaves = BlockState {
        name: "minecraft:oak_leaves".into(),
        properties: vec![
            ("distance".into(), "4".into()),
            ("persistent".into(), "false".into()),
            ("waterlogged".into(), "false".into()),
        ],
    };
    for z in 0..16usize {
        for x in 0..16usize {
            chunk.set_block_state_ref(x, -64, z, &bedrock);
            chunk.set_block_state_ref(x, -62, z, &water);
            if x < 12 || z < 12 {
                for y in 0..=5 {
                    chunk.set_block_state_ref(x, y, z, &stone);
                }
                chunk.set_block_state_ref(x, 6, z, &grass);
                if x % 2 == 0 {
                    chunk.set_block_state_ref(x, 64, z, &stone);
                } else {
                    chunk.set_block_state_ref(x, 64, z, &leaves);
                }
            }
            if x < 8 {
                chunk.set_block_state_ref(x, -50, z, &leaves);
            }
        }
    }
    for x in 4..=6usize {
        for z in 4..=6usize {
            chunk.set_block_state_ref(x, 70, z, &leaves);
        }
    }
    chunk
}
