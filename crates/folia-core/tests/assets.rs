//! Guards the generated registry/worldgen assets against silent truncation or
//! regeneration drift.
//!
//! `registry.rs` embeds these files at compile time, so a missing file breaks
//! the build, but a *truncated* or regenerated file would silently corrupt
//! registry ids. The expected values live in `assets/ASSETS.md`; update both
//! together when an asset is legitimately regenerated.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const ASSETS: &str = "assets";

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(ASSETS)
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// FNV-1a 64: dependency-free, stable across Rust versions, enough to detect
/// truncation or partial rewrites.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn generated_assets_match_the_manifest() {
    let expected: [(&str, usize, u64); 7] = [
        ("block_states.tsv", 2_595_686, 0x3da1_dcdd_58b4_f738),
        ("block_defaults.tsv", 37_605, 0x7c29_fe7e_3a74_07a8),
        ("item_ids.tsv", 45_873, 0xb3fc_5940_89ab_5bf7),
        ("entity_type_ids.tsv", 3_772, 0x4d7c_78e0_dac3_9e50),
        ("biome_ids.txt", 1_681, 0x377b_853e_f131_1f77),
        ("mth_sin.bin", 262_144, 0x115b_15b6_ad9d_2006),
        ("feature_data.json", 295_053, 0xf68d_8c86_ee52_ea65),
    ];

    for (name, length, hash) in expected {
        let bytes = asset(name);
        assert_eq!(bytes.len(), length, "{name} byte length changed");
        assert_eq!(fnv1a64(&bytes), hash, "{name} contents changed");
    }
}

#[test]
fn registry_tables_have_the_expected_line_counts() {
    let cases = [
        ("block_states.tsv", 29_671usize),
        ("block_defaults.tsv", 1_166),
        ("item_ids.tsv", 1_505),
        ("entity_type_ids.tsv", 157),
        ("biome_ids.txt", 65),
    ];

    for (name, expected) in cases {
        let text = String::from_utf8(asset(name)).expect("registry table is UTF-8");
        let lines = text.lines().count();
        assert_eq!(lines, expected, "{name} line count changed");
    }
}

#[test]
fn block_states_cover_ids_from_zero_upward() {
    let text = String::from_utf8(asset("block_states.tsv")).expect("block states are UTF-8");
    let mut ids: Vec<i32> = text
        .lines()
        .filter_map(|line| line.split('\t').next())
        .map(|id| id.parse().expect("block state id is an integer"))
        .collect();
    ids.sort_unstable();

    assert_eq!(ids.len(), 29_671);
    assert_eq!(ids.first().copied(), Some(0), "first state id must be 0");
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            i32::try_from(index).expect("state count fits in i32"),
            *id,
            "block state ids must be dense and unique"
        );
    }
}

#[test]
fn feature_data_has_every_definition() {
    let text = String::from_utf8(asset("feature_data.json")).expect("feature data is UTF-8");
    let value: Value = serde_json::from_str(&text).expect("feature data is valid JSON");
    let object = value.as_object().expect("feature data is an object");

    for (key, expected) in [("configured", 224), ("placed", 258), ("biomes", 65)] {
        let section = object
            .get(key)
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("feature data is missing the `{key}` object"));
        assert_eq!(
            section.len(),
            expected,
            "`{key}` entry count changed in feature_data.json"
        );
    }
}
