use folia_core::chunk_packet::{self, ChunkPacketTemplate, heightmap_bytes, light_bytes};
use folia_core::chunk_packet_diagnostics::{
    ChunkPacketCoverage, ChunkPacketDump, FIXTURES, HEIGHTMAP_TYPES, edge_chunk,
    missing_java_inputs, required_java_inputs,
};
use folia_core::lifecycle_diagnostics::{Coverage, digest_hex, fnv1a64};
use serde_json::Value;
use std::sync::OnceLock;

static DUMP: OnceLock<ChunkPacketDump> = OnceLock::new();

fn dump() -> &'static ChunkPacketDump {
    DUMP.get_or_init(|| ChunkPacketDump::build().expect("build dump"))
}

const GOLDEN: &str = include_str!("../assets/chunk_packet_golden.json");
const GOLDEN_HASH: u64 = 0x74d5_5555_e648_a0a4;

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden fixture is JSON")
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

fn chunk_record<'a>(value: &'a Value, name: &str) -> &'a Value {
    value["chunks"]
        .as_array()
        .expect("chunks array")
        .iter()
        .find(|chunk| chunk["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing chunk record {name}"))
}

#[test]
fn golden_dump_is_byte_stable_and_matches_current_chunks() {
    assert_eq!(fnv1a64(&normalized(GOLDEN.as_bytes())), GOLDEN_HASH);
    let dump = dump();
    assert_eq!(dump.to_value(), &golden());
    assert_eq!(dump.digest(), fnv1a64(dump.to_json_string().as_bytes()));
    assert_eq!(golden()["format_version"], 1);
    assert_eq!(golden()["data_version"], 4788);
    assert_eq!(golden()["chunks"].as_array().unwrap().len(), FIXTURES.len());
    assert_eq!(golden()["heightmap_types"].as_array().unwrap().len(), 6);
}

#[test]
fn fixtures_are_deterministic_across_rebuilds() {
    let first = ChunkPacketDump::build().expect("first dump");
    let second = ChunkPacketDump::build().expect("second dump");
    assert_eq!(first.to_value(), second.to_value());
    assert_eq!(first.digest(), second.digest());

    let chunk = edge_chunk(0, 0);
    let again = edge_chunk(0, 0);
    assert_eq!(heightmap_bytes(&chunk), heightmap_bytes(&again));
    assert_eq!(light_bytes(&chunk), light_bytes(&again));
    assert_eq!(
        ChunkPacketTemplate::build_dynamic(&chunk).unwrap(),
        ChunkPacketTemplate::build_dynamic(&again).unwrap()
    );
}

#[test]
fn all_exported_heightmap_types_are_recorded_for_every_fixture() {
    let dump = dump();
    let fixture_names: Vec<&str> = FIXTURES.iter().map(|fixture| fixture.name).collect();
    let seeds: Vec<i64> = FIXTURES.iter().filter_map(|fixture| fixture.seed).collect();
    assert!(seeds.len() >= 3);
    assert_eq!(
        seeds
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        seeds.len()
    );

    for name in fixture_names {
        let record = chunk_record(dump.to_value(), name);
        let heightmaps = &record["heightmaps"];
        for (kind, _) in HEIGHTMAP_TYPES {
            let entry = &heightmaps[kind];
            assert_eq!(entry["packed_longs"], 37, "{name} {kind} long count");
            assert_eq!(
                entry["packed_digest"].as_str().unwrap().len(),
                16,
                "{name} {kind} digest length"
            );
            assert!(entry["min_y"].is_i64(), "{name} {kind} min Y");
            assert!(entry["max_y"].is_i64(), "{name} {kind} max Y");
            assert!(
                entry["negative_columns"].is_u64(),
                "{name} {kind} negative columns"
            );
        }
    }

    let edge = chunk_record(dump.to_value(), "synthetic-edge");
    assert_ne!(
        edge["heightmaps"]["WORLD_SURFACE"]["packed_digest"],
        edge["heightmaps"]["MOTION_BLOCKING_NO_LEAVES"]["packed_digest"],
        "leaves must change at least one exported heightmap"
    );
    assert_ne!(
        edge["heightmaps"]["WORLD_SURFACE"]["packed_digest"],
        edge["heightmaps"]["OCEAN_FLOOR"]["packed_digest"],
        "fluids must change at least one exported heightmap"
    );
    assert_eq!(
        edge["heightmaps"]["WORLD_SURFACE_WG"]["packed_digest"],
        edge["heightmaps"]["WORLD_SURFACE"]["packed_digest"]
    );
    assert_eq!(
        edge["heightmaps"]["OCEAN_FLOOR_WG"]["packed_digest"],
        edge["heightmaps"]["OCEAN_FLOOR"]["packed_digest"]
    );
}

#[test]
fn negative_y_multiple_sections_and_palette_changes_are_covered() {
    let dump = dump();
    let edge = chunk_record(dump.to_value(), "synthetic-edge");
    let sections = edge["sections"].as_array().unwrap();
    assert_eq!(
        sections
            .iter()
            .map(|section| section["y"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![-4, 0, 4]
    );
    assert!(
        sections
            .iter()
            .all(|section| section["block_palette_len"].as_u64().unwrap() > 1)
    );
    let biome_palettes: Vec<Vec<String>> = sections
        .iter()
        .map(|section| {
            section["biome_palette"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect()
        })
        .collect();
    assert_eq!(biome_palettes[0], vec!["minecraft:deep_ocean"]);
    assert_eq!(biome_palettes[1].len(), 2);
    assert_ne!(biome_palettes[0], biome_palettes[1]);
    assert_ne!(biome_palettes[1], biome_palettes[2]);

    for (kind, _) in HEIGHTMAP_TYPES {
        let entry = &edge["heightmaps"][kind];
        assert!(
            entry["min_y"].as_i64().unwrap() < 0,
            "{kind} must reach a negative Y column"
        );
        assert_eq!(entry["negative_columns"], 16);
    }
    assert_eq!(
        edge["heightmaps"]["WORLD_SURFACE"]["min_y"], -61,
        "water surface is one block above the water"
    );
    assert_eq!(
        edge["heightmaps"]["OCEAN_FLOOR"]["min_y"], -63,
        "ocean floor ignores the water and finds bedrock"
    );
}

#[test]
fn chunk_packet_body_and_wire_sections_are_byte_stable() {
    let dump = dump();
    let expected_dump = golden();
    for fixture in FIXTURES {
        let actual = chunk_record(dump.to_value(), fixture.name);
        let expected = chunk_record(&expected_dump, fixture.name);
        for field in ["packet_heightmaps", "chunk_data", "light", "packet"] {
            assert_eq!(actual[field], expected[field], "{} {field}", fixture.name);
        }
        assert!(
            actual["packet"]["prefix_hex"]
                .as_str()
                .unwrap()
                .starts_with("2c"),
            "clientbound level chunk with light id"
        );
        assert!(
            actual["chunk_data"]["len"].as_u64().unwrap() > 2000,
            "{} must carry real sections",
            fixture.name
        );
        assert!(
            actual["light"]["len"].as_u64().unwrap() > 0,
            "{} must carry light masks",
            fixture.name
        );
    }
}

#[test]
fn captured_java_packets_are_flat_world_only() {
    let captured = chunk_packet::captured_chunk_packets();
    assert!(!captured.is_empty());
    let mut digest = folia_core::lifecycle_diagnostics::Digest::new();
    for (x, z, packet) in &captured {
        assert_eq!(packet[0], 0x2c, "chunk packet id at ({x},{z})");
        let template = ChunkPacketTemplate::from_packet(packet)
            .unwrap_or_else(|error| panic!("parse captured packet ({x},{z}): {error}"));
        assert!(!template.captured_heightmaps().is_empty());
        assert!(!template.captured_light().is_empty());
        digest.bytes(packet);
    }
    let dump = dump();
    assert_eq!(
        dump.to_value()["java_flat_captures"]["count"],
        captured.len()
    );
    assert_eq!(
        dump.to_value()["java_flat_captures"]["digest"],
        digest_hex(digest.finish())
    );
}

#[test]
fn coverage_marks_generated_fixtures_rust_golden_and_java_unavailable() {
    let report = ChunkPacketCoverage::audit();
    assert_eq!(report.java_differential(), Coverage::Unavailable);
    assert_ne!(report.java_differential(), Coverage::JavaVerified);
    assert!(report.golden_available());
    assert_eq!(report.records().len(), FIXTURES.len());
    for fixture in FIXTURES {
        assert_eq!(report.record(fixture.name), Some(Coverage::RustGolden));
    }

    let rendered = report.to_string();
    assert!(rendered.contains("java-verified fixtures:\n  (none)"));
    assert!(rendered.contains("java differential: unavailable"));
    assert!(rendered.contains("flat-world Java captures in join_play.bin:"));

    let missing = missing_java_inputs();
    assert_eq!(missing.len(), required_java_inputs().len());
    assert_eq!(missing.len(), 4);
    for input in missing {
        assert!(rendered.contains(&input.name));
        assert!(rendered.contains(&input.purpose));
    }
}
