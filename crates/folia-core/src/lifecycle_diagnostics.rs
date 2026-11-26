use crate::anvil::Compression;
use crate::block_update::block_update_packet;
use crate::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use crate::chunk_packet::heightmap_bytes;
use crate::chunk_status::ChunkStatus;
use crate::chunk_status_pipeline::{ChunkStatusPipeline, NoOpStages};
use crate::chunk_tickets::{ChunkTicketManager, TicketChunk, UNLOAD_DELAY_TICKS};
use crate::light::{
    self, DataLayer, DirtySections, LightScene, LightSectionPos, SectionLight,
    light_update_for_sections,
};
use crate::region::ChunkPos;
use crate::world::{Dimension, WorldStorage};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DUMP_FORMAT_VERSION: u32 = 1;
pub const DUMP_DATA_VERSION: i32 = 4788;

pub const SCENARIO_NAMES: [&str; 6] = [
    "deterministic_recompute",
    "block_edit_incremental",
    "cross_chunk_propagation",
    "chunk_status_persistence",
    "chunk_ticket_footprint",
    "save_reload_equality",
];

pub const REQUIRED_JAVA_INPUTS: [(&str, &str); 6] = [
    (
        "java-light-sky.bin",
        "per-section SkyLight nibble arrays for the fixture chunk",
    ),
    (
        "java-light-block.bin",
        "per-section BlockLight nibble arrays for the fixture chunk",
    ),
    (
        "java-heightmap.txt",
        "MOTION_BLOCKING / MOTION_BLOCKING_NO_LEAVES / WORLD_SURFACE columns",
    ),
    (
        "java-light-packet.bin",
        "ClientboundLightUpdate bodies for before/after block edits",
    ),
    (
        "java-chunk-status.txt",
        "Status field after each generation stage and after a save/reload",
    ),
    (
        "java-chunk-lifecycle.txt",
        "loaded/ticking chunk sets and block-update packet sequence per view move",
    ),
];

#[derive(Clone)]
pub struct Digest(u64);

impl Digest {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    pub fn str(&mut self, value: &str) -> &mut Self {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes())
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.bytes(&value.to_le_bytes())
    }

    pub fn i32(&mut self, value: i32) -> &mut Self {
        self.bytes(&value.to_le_bytes())
    }

    pub fn i8(&mut self, value: i8) -> &mut Self {
        self.i32(i32::from(value))
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.bytes(&[value])
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}

impl Default for Digest {
    fn default() -> Self {
        Self::new()
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut digest = Digest::new();
    digest.bytes(bytes);
    digest.finish()
}

pub fn digest_hex(value: u64) -> String {
    format!("{value:016x}")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn air() -> BlockState {
    state("minecraft:air")
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn water() -> BlockState {
    state("minecraft:water")
}

fn glass() -> BlockState {
    state("minecraft:glass")
}

fn torch() -> BlockState {
    state("minecraft:torch")
}

pub fn base_chunk(x: i32, z: i32) -> Chunk {
    let sections = (0..=3i8)
        .map(|y| ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![air()],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        })
        .collect();
    let mut chunk = Chunk {
        x,
        z,
        sections,
        light_present: false,
        status: ChunkStatus::Full,
    };
    for lz in 0..16usize {
        for lx in 0..16usize {
            chunk.set_block_state(lx, 0, lz, stone());
        }
    }
    for y in 1..=5i32 {
        for lz in 4..=6usize {
            for lx in 4..=6usize {
                chunk.set_block_state(lx, y, lz, stone());
            }
        }
    }
    for lz in 10..=12usize {
        for lx in 10..=12usize {
            chunk.set_block_state(lx, 2, lz, water());
        }
    }
    for y in 2..=6i32 {
        chunk.set_block_state(15, y, 3, glass());
    }
    chunk
}

fn layer_value(layer: Option<&DataLayer>) -> Value {
    match layer {
        Some(layer) => json!({
            "present": true,
            "empty": layer.is_empty(),
            "digest": digest_hex(fnv1a64(layer.as_bytes())),
        }),
        None => json!({"present": false, "empty": true, "digest": null}),
    }
}

pub fn block_digest(chunk: &Chunk) -> u64 {
    let mut digest = Digest::new();
    digest.i32(chunk.x).i32(chunk.z);
    for section in &chunk.sections {
        digest.i8(section.y);
        for index in &section.block_states.indices {
            match section.block_states.get(*index as usize) {
                Some(state) => {
                    digest.str(&state.name);
                    for (key, value) in &state.properties {
                        digest.str(key).str(value);
                    }
                }
                None => {
                    digest.str("invalid");
                }
            }
        }
    }
    digest.finish()
}

pub fn light_nibble_digest(chunk: &Chunk) -> u64 {
    let mut digest = Digest::new();
    digest.i32(chunk.x).i32(chunk.z);
    for section in &chunk.sections {
        digest.i8(section.y);
        let base_y = i32::from(section.y) * 16;
        for y in base_y..base_y + 16 {
            for z in 0..16usize {
                for x in 0..16usize {
                    digest.u8(chunk.sky_light(x, y, z).unwrap_or(0));
                    digest.u8(chunk.block_light(x, y, z).unwrap_or(0));
                }
            }
        }
    }
    digest.finish()
}

pub fn light_digest(chunk: &Chunk) -> u64 {
    let mut digest = Digest::new();
    digest
        .i32(chunk.x)
        .i32(chunk.z)
        .u8(u8::from(chunk.light_present));
    for section in &chunk.sections {
        digest.i8(section.y);
        for layer in [section.light.sky.as_ref(), section.light.block.as_ref()] {
            match layer {
                Some(layer) => {
                    digest.u8(1 + u8::from(layer.is_empty()));
                    digest.bytes(layer.as_bytes());
                }
                None => {
                    digest.u8(0);
                }
            }
        }
    }
    digest.finish()
}

pub fn heightmap_digest(chunk: &Chunk) -> u64 {
    fnv1a64(&heightmap_bytes(chunk))
}

pub fn light_layers(chunk: &Chunk) -> Vec<Value> {
    chunk
        .sections
        .iter()
        .map(|section| {
            json!({
                "y": section.y,
                "sky": layer_value(section.light.sky.as_ref()),
                "block": layer_value(section.light.block.as_ref()),
            })
        })
        .collect()
}

pub fn body_value(body: &[u8]) -> Value {
    json!({
        "len": body.len(),
        "digest": digest_hex(fnv1a64(body)),
        "prefix_hex": hex(&body[..body.len().min(16)]),
    })
}

pub fn dirty_value(dirty: &DirtySections) -> Vec<Value> {
    dirty
        .iter()
        .map(|pos: &LightSectionPos| {
            json!({
                "chunk_x": pos.chunk_x,
                "chunk_z": pos.chunk_z,
                "section_y": pos.section_y,
            })
        })
        .collect()
}

fn recompute_scenario() -> Value {
    let mut chunk = base_chunk(0, 0);
    light::recompute_chunk_light_with_neighbours(&mut chunk, &[]);
    let mut second = base_chunk(0, 0);
    light::recompute_chunk_light_with_neighbours(&mut second, &[]);
    let light = light_digest(&chunk);
    let block = block_digest(&chunk);
    let heightmap = heightmap_digest(&chunk);
    let mut combined = Digest::new();
    combined.u64(light).u64(block).u64(heightmap);
    json!({
        "layers": light_layers(&chunk),
        "light_digest": digest_hex(light),
        "block_digest": digest_hex(block),
        "heightmap_digest": digest_hex(heightmap),
        "digest": digest_hex(combined.finish()),
        "stable_across_runs": light == light_digest(&second),
    })
}

fn block_edit_scenario() -> Value {
    let mut chunk = base_chunk(0, 0);
    light::recompute_chunk_light_with_neighbours(&mut chunk, &[]);
    let mut steps = Vec::new();
    for (name, state) in [
        ("place_torch", torch()),
        ("replace_with_stone", stone()),
        ("clear_to_air", air()),
    ] {
        let (changed, dirty, body) = {
            let mut scene = LightScene::new(&mut chunk, &mut []);
            scene.clear_dirty();
            let changed = scene.set_block_state(8, 8, 8, state.clone());
            let dirty = scene.dirty().clone();
            let body = light_update_for_sections(&chunk, &dirty).unwrap_or_default();
            (changed, dirty, body)
        };
        steps.push(json!({
            "name": name,
            "changed": changed,
            "dirty_sections": dirty_value(&dirty),
            "light_body": body_value(&body),
            "block_update_body": body_value(&block_update_packet(8, 8, 8, crate::chunk_packet::block_state_id(&state))),
            "light_digest": digest_hex(light_digest(&chunk)),
        }));
    }
    let mut bulk = chunk.clone();
    bulk.clear_light();
    light::recompute_chunk_light_with_neighbours(&mut bulk, &[]);
    {
        let _bulk_scene = LightScene::new(&mut bulk, &mut []);
    }
    json!({
        "steps": steps,
        "final_light_digest": digest_hex(light_digest(&chunk)),
        "bulk_light_digest": digest_hex(light_digest(&bulk)),
        "final_light_nibbles": digest_hex(light_nibble_digest(&chunk)),
        "bulk_light_nibbles": digest_hex(light_nibble_digest(&bulk)),
        "incremental_matches_bulk": light_nibble_digest(&chunk) == light_nibble_digest(&bulk),
    })
}

fn cross_chunk_scenario() -> Value {
    let mut center = base_chunk(0, 0);
    let mut east = base_chunk(1, 0);
    let center_copy = center.clone();
    let east_copy = east.clone();
    light::recompute_chunk_light_with_neighbours(&mut center, std::slice::from_ref(&east_copy));
    light::recompute_chunk_light_with_neighbours(&mut east, &[center_copy]);

    let mut neighbours = [east];
    let (border_sky, border_block, dirty) = {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.clear_dirty();
        assert!(scene.set_block_state(15, 8, 8, torch()));
        let dirty = scene.dirty().clone();
        (
            scene.sky_level(16, 8, 8),
            scene.block_level(16, 8, 8),
            dirty,
        )
    };
    let center_body = light_update_for_sections(&center, &dirty).unwrap_or_default();
    let east_body = light_update_for_sections(&neighbours[0], &dirty).unwrap_or_default();
    json!({
        "center_light_digest": digest_hex(light_digest(&center)),
        "neighbour_light_digest": digest_hex(light_digest(&neighbours[0])),
        "neighbour_border_sky": border_sky,
        "neighbour_border_block": border_block,
        "dirty_sections": dirty_value(&dirty),
        "center_light_body": body_value(&center_body),
        "neighbour_light_body": body_value(&east_body),
    })
}

fn chunk_status_scenario() -> Value {
    let mut chunk = base_chunk(0, 0);
    chunk.set_status(ChunkStatus::Biomes);
    let mut pipeline = ChunkStatusPipeline::new(NoOpStages);
    let report = pipeline
        .upgrade(&mut chunk, ChunkStatus::Full)
        .expect("upgrade chunk");
    let tag = encode_chunk(&chunk).expect("encode chunk");
    let nbt_status = tag
        .get("Status")
        .and_then(crate::nbt::Tag::as_str)
        .unwrap_or("")
        .to_owned();
    let decoded = decode_chunk(&tag).expect("decode chunk");
    json!({
        "from": report.from.name(),
        "target": report.target.name(),
        "reached": report.reached.name(),
        "completed": report.completed.iter().map(|status| status.name()).collect::<Vec<_>>(),
        "checkpoint_count": pipeline.checkpoints().len(),
        "nbt_status": nbt_status,
        "decoded_status": decoded.status().name(),
        "status_persisted": decoded.status() == chunk.status(),
    })
}

fn chunk_pos_value(chunk: &TicketChunk) -> Value {
    json!([chunk.pos.x, chunk.pos.z])
}

fn chunk_ticket_scenario() -> Value {
    let mut manager = ChunkTicketManager::new();
    manager.set_spawn_footprint(Dimension::Overworld, ChunkPos::new(0, 0), 0);
    let initial = manager.set_player_footprint(7, Dimension::Overworld, ChunkPos::new(2, 0), 1);
    for chunk in manager.loaded_chunks() {
        manager.mark_materialized(chunk);
    }
    manager.force_chunk(TicketChunk::overworld(5, 0));

    let loaded: Vec<Value> = manager
        .loaded_chunks()
        .iter()
        .map(chunk_pos_value)
        .collect();
    let ticking: Vec<Value> = manager
        .loaded_chunks()
        .iter()
        .filter(|chunk| manager.is_ticking(**chunk))
        .map(chunk_pos_value)
        .collect();
    let persistent: Vec<Value> = manager
        .persistent_chunks()
        .iter()
        .map(chunk_pos_value)
        .collect();
    let moved = manager.set_player_footprint(7, Dimension::Overworld, ChunkPos::new(3, 0), 1);
    let pending = manager.pending_unloads().len();
    let mut unloaded = Vec::new();
    for _ in 0..UNLOAD_DELAY_TICKS {
        unloaded.extend(manager.tick());
    }
    json!({
        "initial_loaded": loaded,
        "initial_ticking": ticking,
        "persistent": persistent,
        "initial_added": initial.added.iter().map(chunk_pos_value).collect::<Vec<_>>(),
        "move_added": moved.added.iter().map(chunk_pos_value).collect::<Vec<_>>(),
        "move_removed": moved.removed.iter().map(chunk_pos_value).collect::<Vec<_>>(),
        "pending_unloads_after_move": pending,
        "unloaded_after_delay": unloaded.iter().map(chunk_pos_value).collect::<Vec<_>>(),
        "materialized": manager.materialized_chunks().len(),
    })
}

fn save_reload_scenario() -> Value {
    let root = temp_dir();
    let region = root.join("region");
    let mut storage = WorldStorage::new(&region);
    let mut chunk = base_chunk(0, 0);
    chunk.set_status(ChunkStatus::Features);
    light::recompute_chunk_light_with_neighbours(&mut chunk, &[]);
    let before = chunk_signature(&chunk);
    storage
        .save_chunk(&chunk, Compression::Zlib)
        .expect("save chunk");
    storage.flush().expect("flush");

    let mut reopened = WorldStorage::new(&region);
    let loaded = reopened
        .load_chunk(0, 0)
        .expect("load chunk")
        .expect("stored chunk");
    let after = chunk_signature(&loaded);
    std::fs::remove_dir_all(&root).expect("remove temp dir");
    json!({
        "before": before,
        "after": after,
        "equal": before == after,
    })
}

pub fn chunk_signature(chunk: &Chunk) -> Value {
    json!({
        "status": chunk.status().name(),
        "light_digest": digest_hex(light_digest(chunk)),
        "block_digest": digest_hex(block_digest(chunk)),
        "heightmap_digest": digest_hex(heightmap_digest(chunk)),
    })
}

fn temp_dir() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("folia-rs-lifecycle-{}-{nanos}", std::process::id()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleDump {
    scenarios: BTreeMap<String, Value>,
}

impl LifecycleDump {
    pub fn build() -> Self {
        let scenarios = BTreeMap::from([
            ("deterministic_recompute".to_owned(), recompute_scenario()),
            ("block_edit_incremental".to_owned(), block_edit_scenario()),
            ("cross_chunk_propagation".to_owned(), cross_chunk_scenario()),
            (
                "chunk_status_persistence".to_owned(),
                chunk_status_scenario(),
            ),
            ("chunk_ticket_footprint".to_owned(), chunk_ticket_scenario()),
            ("save_reload_equality".to_owned(), save_reload_scenario()),
        ]);
        Self { scenarios }
    }

    pub fn scenario(&self, name: &str) -> Option<&Value> {
        self.scenarios.get(name)
    }

    pub fn to_value(&self) -> Value {
        json!({
            "format_version": DUMP_FORMAT_VERSION,
            "data_version": DUMP_DATA_VERSION,
            "scenarios": self.scenarios,
        })
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(&self.to_value()).expect("lifecycle dump is JSON serializable")
    }

    pub fn digest(&self) -> u64 {
        fnv1a64(self.to_json_string().as_bytes())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    JavaVerified,
    RustGolden,
    Unavailable,
}

impl Coverage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JavaVerified => "java-verified",
            Self::RustGolden => "rust-golden",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JavaInput {
    pub name: &'static str,
    pub purpose: &'static str,
    pub present: bool,
}

pub fn java_inputs() -> Vec<JavaInput> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    REQUIRED_JAVA_INPUTS
        .iter()
        .map(|(name, purpose)| JavaInput {
            name,
            purpose,
            present: root.join(name).exists(),
        })
        .collect()
}

pub fn missing_java_inputs() -> Vec<JavaInput> {
    java_inputs()
        .into_iter()
        .filter(|input| !input.present)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageReport {
    records: BTreeMap<String, Coverage>,
    java_available: bool,
}

impl CoverageReport {
    pub fn audit() -> Self {
        let java_available = java_inputs().iter().all(|input| input.present);
        let golden_present = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("lighting_golden.json")
            .exists();
        let coverage = if java_available {
            Coverage::JavaVerified
        } else if golden_present {
            Coverage::RustGolden
        } else {
            Coverage::Unavailable
        };
        Self {
            records: SCENARIO_NAMES
                .iter()
                .map(|name| ((*name).to_owned(), coverage))
                .collect(),
            java_available,
        }
    }

    pub fn record(&self, scenario: &str) -> Option<Coverage> {
        self.records.get(scenario).copied()
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
}

impl fmt::Display for CoverageReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for coverage in [
            Coverage::JavaVerified,
            Coverage::RustGolden,
            Coverage::Unavailable,
        ] {
            writeln!(formatter, "{} scenarios:", coverage.as_str())?;
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
