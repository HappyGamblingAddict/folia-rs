use folia_core::anvil::Compression;
use folia_core::block_update::block_update_packet;
use folia_core::chunk::{BlockState, Chunk, decode_chunk, encode_chunk};
use folia_core::chunk_packet::block_state_id;
use folia_core::chunk_status::ChunkStatus;
use folia_core::chunk_status_pipeline::{ChunkCoord, ChunkStatusPipeline, NoOpStages};
use folia_core::lifecycle_diagnostics::{
    Coverage, CoverageReport, LifecycleDump, REQUIRED_JAVA_INPUTS, SCENARIO_NAMES, base_chunk,
    chunk_signature, digest_hex, fnv1a64, light_digest, light_nibble_digest, missing_java_inputs,
};
use folia_core::light::{self, LightScene, LightSectionPos, light_update_for_sections};
use folia_core::nbt::Tag;
use folia_core::world::WorldStorage;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const GOLDEN: &str = include_str!("../assets/lighting_golden.json");
const GOLDEN_HASH: u64 = 0xad8b_2f64_9389_fb06;

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden fixture is JSON")
}

fn scenario(value: &Value, name: &str) -> Value {
    value["scenarios"][name].clone()
}

fn normalized(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn torch() -> BlockState {
    state("minecraft:torch")
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn air() -> BlockState {
    state("minecraft:air")
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "folia-rs-p312-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn dirty_json(dirty: &light::DirtySections) -> Value {
    Value::Array(
        dirty
            .iter()
            .map(|pos| {
                serde_json::json!({
                    "chunk_x": pos.chunk_x,
                    "chunk_z": pos.chunk_z,
                    "section_y": pos.section_y,
                })
            })
            .collect(),
    )
}

fn normalize_bulk(chunk: &mut Chunk) {
    let _scene = LightScene::new(chunk, &mut []);
}

#[test]
fn golden_dump_is_byte_stable_and_matches_current_scenarios() {
    assert_eq!(fnv1a64(&normalized(GOLDEN.as_bytes())), GOLDEN_HASH);
    let dump = LifecycleDump::build();
    assert_eq!(dump.to_value(), golden());
    assert_eq!(dump.digest(), fnv1a64(dump.to_json_string().as_bytes()));
    for name in SCENARIO_NAMES {
        assert!(dump.scenario(name).is_some(), "missing scenario {name}");
    }
}

#[test]
fn deterministic_light_recompute_is_stable_across_instances() {
    let expected = scenario(&golden(), "deterministic_recompute");
    assert_eq!(expected["stable_across_runs"], true);

    let mut first = base_chunk(0, 0);
    let mut second = base_chunk(0, 0);
    light::recompute_chunk_light_with_neighbours(&mut first, &[]);
    light::recompute_chunk_light_with_neighbours(&mut second, &[]);
    assert_eq!(light_digest(&first), light_digest(&second));
    assert_eq!(light_nibble_digest(&first), light_nibble_digest(&second));
    assert_eq!(digest_hex(light_digest(&first)), expected["light_digest"]);

    light::recompute_chunk_light_with_neighbours(&mut first, &[]);
    assert_eq!(light_digest(&first), light_digest(&second));
}

#[test]
fn block_edit_incremental_updates_match_golden_and_bulk_recompute() {
    let expected = scenario(&golden(), "block_edit_incremental");
    let steps = expected["steps"].as_array().expect("steps array");
    assert_eq!(steps.len(), 3);
    assert_eq!(expected["incremental_matches_bulk"], true);

    let mut chunk = base_chunk(0, 0);
    light::recompute_chunk_light_with_neighbours(&mut chunk, &[]);
    for (step, next) in steps.iter().zip([torch(), stone(), air()]) {
        let (dirty, body) = {
            let mut scene = LightScene::new(&mut chunk, &mut []);
            scene.clear_dirty();
            assert!(scene.set_block_state(8, 8, 8, next.clone()));
            let dirty = scene.dirty().clone();
            let body = light_update_for_sections(&chunk, &dirty).unwrap_or_default();
            (dirty, body)
        };
        assert_eq!(dirty_json(&dirty), step["dirty_sections"]);
        assert_eq!(
            body.len() as u64,
            step["light_body"]["len"].as_u64().unwrap()
        );
        assert_eq!(
            digest_hex(fnv1a64(&body)),
            step["light_body"]["digest"].as_str().unwrap()
        );
        let update = block_update_packet(8, 8, 8, block_state_id(&next));
        assert_eq!(
            digest_hex(fnv1a64(&update)),
            step["block_update_body"]["digest"].as_str().unwrap()
        );
    }

    let mut bulk = chunk.clone();
    bulk.clear_light();
    light::recompute_chunk_light_with_neighbours(&mut bulk, &[]);
    normalize_bulk(&mut bulk);
    assert_eq!(light_nibble_digest(&chunk), light_nibble_digest(&bulk));
    assert_eq!(
        digest_hex(light_nibble_digest(&chunk)),
        expected["final_light_nibbles"]
    );
    assert_eq!(
        digest_hex(light_digest(&chunk)),
        expected["final_light_digest"]
    );
}

#[test]
fn cross_chunk_propagation_reaches_neighbour_and_matches_bulk() {
    let expected = scenario(&golden(), "cross_chunk_propagation");
    let mut center = base_chunk(0, 0);
    let east = base_chunk(1, 0);
    let center_copy = center.clone();
    let east_copy = east.clone();
    light::recompute_chunk_light_with_neighbours(&mut center, &[east_copy]);
    let mut east = east;
    light::recompute_chunk_light_with_neighbours(&mut east, &[center_copy]);

    let mut neighbours = [east];
    let (dirty, border_block) = {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.clear_dirty();
        assert!(scene.set_block_state(15, 8, 8, torch()));
        (scene.dirty().clone(), scene.block_level(16, 8, 8))
    };
    let center_body = light_update_for_sections(&center, &dirty).unwrap_or_default();
    let east_body = light_update_for_sections(&neighbours[0], &dirty).unwrap_or_default();

    assert_eq!(
        border_block,
        expected["neighbour_border_block"].as_u64().unwrap() as u8
    );
    assert!(dirty.contains(&LightSectionPos::new(1, 0, 0)));
    assert_eq!(dirty_json(&dirty), expected["dirty_sections"]);
    assert_eq!(neighbours[0].block_light(0, 8, 8), Some(13));
    assert_eq!(
        digest_hex(fnv1a64(&east_body)),
        expected["neighbour_light_body"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        digest_hex(fnv1a64(&center_body)),
        expected["center_light_body"]["digest"].as_str().unwrap()
    );

    let mut bulk_center = center.clone();
    let mut bulk_east = neighbours[0].clone();
    bulk_center.clear_light();
    bulk_east.clear_light();
    let bulk_east_copy = bulk_east.clone();
    let bulk_center_copy = bulk_center.clone();
    light::recompute_chunk_light_with_neighbours(&mut bulk_center, &[bulk_east_copy]);
    light::recompute_chunk_light_with_neighbours(&mut bulk_east, &[bulk_center_copy]);
    let mut bulk_neighbours = [bulk_east];
    {
        let _scene = LightScene::new(&mut bulk_center, &mut bulk_neighbours);
    }
    bulk_east = bulk_neighbours[0].clone();
    assert_eq!(
        light_nibble_digest(&center),
        light_nibble_digest(&bulk_center)
    );
    assert_eq!(
        light_nibble_digest(&neighbours[0]),
        light_nibble_digest(&bulk_east)
    );
}

#[test]
fn chunk_status_persists_through_pipeline_and_nbt() {
    let expected = scenario(&golden(), "chunk_status_persistence");
    assert_eq!(expected["from"], "biomes");
    assert_eq!(expected["target"], "full");
    assert_eq!(expected["reached"], "full");
    assert_eq!(expected["nbt_status"], "minecraft:full");
    assert_eq!(expected["decoded_status"], "full");
    assert_eq!(expected["status_persisted"], true);
    assert_eq!(expected["checkpoint_count"], 8);

    let mut chunk = base_chunk(0, 0);
    chunk.set_status(ChunkStatus::Biomes);
    let mut pipeline = ChunkStatusPipeline::new(NoOpStages);
    let report = pipeline
        .upgrade(&mut chunk, ChunkStatus::Full)
        .expect("upgrade chunk");
    assert!(!report.ran(ChunkStatus::Biomes));
    assert!(report.ran(ChunkStatus::Light));
    assert_eq!(pipeline.status_at(ChunkCoord::new(0, 0)), ChunkStatus::Full);
    assert_eq!(chunk.status(), ChunkStatus::Full);

    let tag = encode_chunk(&chunk).expect("encode chunk");
    assert_eq!(
        tag.get("Status").and_then(Tag::as_str),
        Some("minecraft:full")
    );
    let decoded = decode_chunk(&tag).expect("decode chunk");
    assert_eq!(decoded.status(), ChunkStatus::Full);

    let again = pipeline
        .upgrade(&mut chunk, ChunkStatus::Full)
        .expect("resume chunk");
    assert!(again.already_complete);
}

#[test]
fn chunk_ticket_footprint_dump_is_stable() {
    let expected = scenario(&golden(), "chunk_ticket_footprint");
    let mut manager = folia_core::chunk_tickets::ChunkTicketManager::new();
    manager.set_spawn_footprint(
        folia_core::world::Dimension::Overworld,
        folia_core::region::ChunkPos::new(0, 0),
        0,
    );
    let initial = manager.set_player_footprint(
        7,
        folia_core::world::Dimension::Overworld,
        folia_core::region::ChunkPos::new(2, 0),
        1,
    );
    assert_eq!(
        initial.added.len(),
        expected["initial_added"].as_array().unwrap().len()
    );
    manager.force_chunk(folia_core::chunk_tickets::TicketChunk::overworld(5, 0));
    assert_eq!(
        manager.loaded_chunks().len(),
        expected["initial_loaded"].as_array().unwrap().len()
    );
    assert!(
        manager
            .loaded_chunks()
            .iter()
            .all(|chunk| manager.is_ticking(*chunk))
    );
    let moved = manager.set_player_footprint(
        7,
        folia_core::world::Dimension::Overworld,
        folia_core::region::ChunkPos::new(3, 0),
        1,
    );
    assert_eq!(
        moved.added.len(),
        expected["move_added"].as_array().unwrap().len()
    );
    assert_eq!(
        moved.removed.len(),
        expected["move_removed"].as_array().unwrap().len()
    );
    assert_eq!(
        manager.pending_unloads().len() as u64,
        expected["pending_unloads_after_move"].as_u64().unwrap()
    );
    let mut unloaded = Vec::new();
    for _ in 0..folia_core::chunk_tickets::UNLOAD_DELAY_TICKS {
        unloaded.extend(manager.tick());
    }
    assert_eq!(
        unloaded.len(),
        expected["unloaded_after_delay"].as_array().unwrap().len()
    );
    assert_eq!(
        manager.loaded_chunks().len(),
        expected["initial_loaded"].as_array().unwrap().len()
    );
}

#[test]
fn save_reload_round_trip_is_equal() {
    let expected = scenario(&golden(), "save_reload_equality");
    assert_eq!(expected["equal"], true);
    assert_eq!(expected["before"], expected["after"]);

    let root = temp_dir("round-trip");
    let region = root.join("region");
    let mut storage = WorldStorage::new(&region);
    let mut chunk = base_chunk(0, 0);
    chunk.set_status(ChunkStatus::Features);
    light::recompute_chunk_light_with_neighbours(&mut chunk, &[]);
    let before = chunk_signature(&chunk);
    assert_eq!(before, expected["before"]);

    storage
        .save_chunk(&chunk, Compression::Zlib)
        .expect("save chunk");
    storage.flush().expect("flush");
    let mut reopened = WorldStorage::new(&region);
    let loaded = reopened
        .load_chunk(0, 0)
        .expect("load chunk")
        .expect("stored chunk");
    assert_eq!(before, chunk_signature(&loaded));

    std::fs::remove_dir_all(root).expect("remove temp dir");
}

#[test]
fn coverage_report_marks_java_differential_unavailable_and_lists_exact_inputs() {
    let report = CoverageReport::audit();
    for name in SCENARIO_NAMES {
        assert_eq!(report.record(name), Some(Coverage::RustGolden));
    }
    assert_eq!(report.records().len(), SCENARIO_NAMES.len());
    assert_eq!(report.java_differential(), Coverage::Unavailable);
    assert_ne!(report.java_differential(), Coverage::JavaVerified);

    let rendered = report.to_string();
    assert!(rendered.contains("rust-golden scenarios:"));
    assert!(rendered.contains("java-verified scenarios:\n  (none)"));
    assert!(rendered.contains("java differential: unavailable"));
    assert!(rendered.contains("missing Java capture inputs:"));

    let missing = missing_java_inputs();
    assert_eq!(missing.len(), REQUIRED_JAVA_INPUTS.len());
    for (name, purpose) in REQUIRED_JAVA_INPUTS {
        assert!(
            missing
                .iter()
                .any(|input| input.name == name && input.purpose == purpose),
            "input {name} is not reported missing"
        );
        assert!(rendered.contains(name));
        assert!(rendered.contains(purpose));
    }
    assert_eq!(Coverage::JavaVerified.as_str(), "java-verified");
    assert_eq!(Coverage::RustGolden.as_str(), "rust-golden");
    assert_eq!(Coverage::Unavailable.as_str(), "unavailable");
}
