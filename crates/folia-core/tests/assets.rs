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
    let expected: [(&str, usize, u64); 18] = [
        ("block_states.tsv", 2_595_686, 0x3da1_dcdd_58b4_f738),
        (
            "block_state_behaviour.tsv.gz",
            284_415,
            0x11e9_f4a0_1cc0_afc7,
        ),
        ("biome_climate.tsv", 2_161, 0x70a2_dbb7_6140_8aaa),
        ("block_defaults.tsv", 37_605, 0x7c29_fe7e_3a74_07a8),
        ("item_ids.tsv", 45_873, 0xb3fc_5940_89ab_5bf7),
        ("entity_type_ids.tsv", 3_772, 0x4d7c_78e0_dac3_9e50),
        ("biome_ids.txt", 1_681, 0x377b_853e_f131_1f77),
        ("mth_sin.bin", 262_144, 0x115b_15b6_ad9d_2006),
        ("feature_data.json", 295_053, 0xf68d_8c86_ee52_ea65),
        ("tags/block.tsv", 195_706, 0x1a25_6d60_d13e_9b9e),
        ("tags/item.tsv", 94_764, 0x6a67_eb5a_b9ee_97f6),
        ("tags/entity_type.tsv", 17_999, 0xbd26_ce82_b892_592e),
        ("tags/fluid.tsv", 139, 0x1aa5_2d04_22f1_5857),
        ("tags/game_event.tsv", 5_652, 0x483b_5e4e_c73f_bd60),
        ("tags/damage_type.tsv", 9_146, 0x2678_e893_14c3_df1c),
        ("tags/enchantment.tsv", 14_614, 0x6ebb_8076_d622_d277),
        (
            "tags/point_of_interest_type.tsv",
            1_273,
            0x7808_a639_0eaa_7e0d,
        ),
        ("tags/worldgen.tsv", 35_799, 0xf2e0_3c73_7a04_1e7c),
    ];

    for (name, length, hash) in expected {
        let bytes = asset(name);
        assert_eq!(bytes.len(), length, "{name} byte length changed");
        assert_eq!(fnv1a64(&bytes), hash, "{name} contents changed");
    }
}

#[test]
fn dynamic_registry_assets_match_the_manifest() {
    let expected: [(&str, usize, u64); 23] = [
        (
            "registries/banner_pattern.tsv",
            1_128,
            0x4b33_0b3f_f4e5_08aa,
        ),
        ("registries/cat_variant.tsv", 232, 0x05cd_22d8_e030_50d4),
        ("registries/chat_type.tsv", 229, 0xacb1_f362_d3ae_4a4b),
        ("registries/chicken_variant.tsv", 59, 0x4802_9f4a_d65a_3a4b),
        ("registries/cow_variant.tsv", 59, 0x4802_9f4a_d65a_3a4b),
        ("registries/damage_type.tsv", 1_232, 0xde71_cad6_0a14_a36d),
        ("registries/dialog.tsv", 81, 0xc91a_6533_5bb9_ab3f),
        ("registries/dimension_type.tsv", 97, 0x858c_ec10_6a34_bab5),
        ("registries/enchantment.tsv", 1_063, 0xcab6_64fa_6191_9ac0),
        ("registries/frog_variant.tsv", 59, 0x4802_9f4a_d65a_3a4b),
        ("registries/instrument.tsv", 230, 0xbdf6_08bc_5edb_867a),
        ("registries/jukebox_song.tsv", 426, 0x0816_3fe8_59c0_36b0),
        (
            "registries/painting_variant.tsv",
            1_094,
            0x4965_bd6b_c4d5_9df3,
        ),
        ("registries/pig_variant.tsv", 59, 0x4802_9f4a_d65a_3a4b),
        ("registries/test_environment.tsv", 21, 0x0e4b_680a_9d03_6869),
        ("registries/test_instance.tsv", 25, 0x389b_5dee_903c_0053),
        ("registries/timeline.tsv", 90, 0x8184_152a_d74b_900a),
        ("registries/trim_material.tsv", 224, 0x57e7_5985_9709_6a73),
        ("registries/trim_pattern.tsv", 346, 0x9df9_47cc_5a98_ff0c),
        (
            "registries/wolf_sound_variant.tsv",
            132,
            0x5118_e273_9e78_63f0,
        ),
        ("registries/wolf_variant.tsv", 177, 0x54a3_a531_0b0c_95d7),
        (
            "registries/worldgen_biome.tsv",
            1_746,
            0x6e48_37e8_fc4a_38c0,
        ),
        (
            "registries/zombie_nautilus_variant.tsv",
            41,
            0x6188_bf5d_0dc1_6fd8,
        ),
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
        ("biome_climate.tsv", 65),
        ("tags/block.tsv", 3_655),
        ("tags/item.tsv", 1_897),
        ("tags/entity_type.tsv", 368),
        ("tags/fluid.tsv", 4),
        ("tags/game_event.tsv", 119),
        ("tags/damage_type.tsv", 196),
        ("tags/enchantment.tsv", 308),
        ("tags/point_of_interest_type.tsv", 30),
        ("tags/worldgen.tsv", 631),
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
