//! Guards the generated registry/worldgen assets against silent truncation or
//! regeneration drift.
//!
//! `registry.rs` embeds these files at compile time, so a missing file breaks
//! the build, but a *truncated* or regenerated file would silently corrupt
//! registry ids. The expected values live in `assets/ASSETS.md`; update both
//! together when an asset is legitimately regenerated.

use folia_core::loot::{LootCatalog, RecipeCatalog};
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

/// Reads a generated asset and folds CRLF to LF on text files. The manifest
/// records the canonical LF bytes; Windows checkouts with `core.autocrlf`
/// would otherwise fail the guard even though the content is unchanged.
fn manifest_asset(name: &str) -> Vec<u8> {
    let bytes = asset(name);
    if name.ends_with(".bin") || name.ends_with(".gz") {
        return bytes;
    }
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
            continue;
        }
        normalized.push(bytes[index]);
        index += 1;
    }
    normalized
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
    let expected: [(&str, usize, u64); 27] = [
        ("block_states.tsv", 2_595_686, 0x3da1_dcdd_58b4_f738),
        (
            "block_state_behaviour.tsv.gz",
            284_415,
            0x11e9_f4a0_1cc0_afc7,
        ),
        ("block_shapes.tsv.gz", 161_523, 0x5c6a_d70b_8935_56e2),
        ("biome_climate.tsv", 2_096, 0x41c1_162e_e672_03b5),
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
        ("loot_tables.json", 662_482, 0x8c3b_7afa_c27e_3096),
        ("recipes.json", 356_848, 0x9b84_4794_502a_181f),
        ("item_prototypes.json", 79_369, 0xf179_dfed_29cd_cdad),
        ("structure_processors.json", 46_334, 0x446c_0e7b_069e_66c4),
        ("template_pools.json", 252_745, 0xaa64_3467_6fda_e64c),
        ("structure_templates.json", 1_063_517, 0x0dde_46f3_a786_3ed8),
        (
            "structure_templates_more.json",
            2_358_479,
            0xb45a_cf21_43b0_63bc,
        ),
        (
            "structure_templates_trial.json",
            996_296,
            0xe88c_a768_c8c4_1c9a,
        ),
    ];

    for (name, length, hash) in expected {
        let bytes = manifest_asset(name);
        assert_eq!(bytes.len(), length, "{name} byte length changed");
        assert_eq!(fnv1a64(&bytes), hash, "{name} contents changed");
    }
}

#[test]
fn dynamic_registry_assets_match_the_manifest() {
    let expected: [(&str, usize, u64); 23] = [
        (
            "registries/banner_pattern.tsv",
            1_085,
            0xb28b_4869_b8a9_3aeb,
        ),
        ("registries/cat_variant.tsv", 221, 0x71d5_f236_0275_8e57),
        ("registries/chat_type.tsv", 221, 0x889e_d8e3_1765_83eb),
        ("registries/chicken_variant.tsv", 56, 0x91ed_138a_1da0_880c),
        ("registries/cow_variant.tsv", 56, 0x91ed_138a_1da0_880c),
        ("registries/damage_type.tsv", 1_182, 0x9ca3_ed99_9634_2bb1),
        ("registries/dialog.tsv", 78, 0x8706_bf14_79ff_c1bc),
        ("registries/dimension_type.tsv", 93, 0xe19f_2618_5e6a_fb71),
        ("registries/enchantment.tsv", 1_020, 0x4317_03ed_a365_6761),
        ("registries/frog_variant.tsv", 56, 0x91ed_138a_1da0_880c),
        ("registries/instrument.tsv", 222, 0xeae6_2ed2_cafe_a822),
        ("registries/jukebox_song.tsv", 405, 0x3c92_ea32_6c62_00a7),
        (
            "registries/painting_variant.tsv",
            1_043,
            0x962b_525b_d33f_8528,
        ),
        ("registries/pig_variant.tsv", 56, 0x91ed_138a_1da0_880c),
        ("registries/test_environment.tsv", 20, 0x15a0_acd1_884e_492c),
        ("registries/test_instance.tsv", 24, 0xb749_aaa8_45c6_7f6a),
        ("registries/timeline.tsv", 86, 0x1be8_1df8_928d_6914),
        ("registries/trim_material.tsv", 213, 0x8101_fd69_b4ce_da06),
        ("registries/trim_pattern.tsv", 328, 0x495c_71f2_2448_7526),
        (
            "registries/wolf_sound_variant.tsv",
            125,
            0x4cd0_4d17_16fb_1647,
        ),
        ("registries/wolf_variant.tsv", 168, 0x4117_e7f6_0959_4e04),
        (
            "registries/worldgen_biome.tsv",
            1_681,
            0x377b_853e_f131_1f77,
        ),
        (
            "registries/zombie_nautilus_variant.tsv",
            39,
            0x05e2_f622_b497_7cf0,
        ),
    ];

    for (name, length, hash) in expected {
        let bytes = manifest_asset(name);
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
fn loot_and_recipe_catalogs_match_their_indexes() {
    let loot_text = String::from_utf8(asset("loot_tables.json")).expect("loot catalog is UTF-8");
    let loot = LootCatalog::from_json(&loot_text).expect("loot catalog parses");
    assert_eq!(loot.index().total, 1_324);
    assert_eq!(loot.index().by_type.get("minecraft:block"), Some(&1_083));
    assert_eq!(loot.index().by_type.get("minecraft:entity"), Some(&114));
    assert_eq!(loot.tables().count(), loot.index().total);

    let recipe_text = String::from_utf8(asset("recipes.json")).expect("recipe catalog is UTF-8");
    let recipes = RecipeCatalog::from_json(&recipe_text).expect("recipe catalog parses");
    assert_eq!(recipes.index().total, 1_470);
    assert_eq!(
        recipes.index().by_type.get("minecraft:crafting_shaped"),
        Some(&706)
    );
    assert_eq!(recipes.index().by_type.get("minecraft:smelting"), Some(&73));
    assert_eq!(recipes.recipes().count(), recipes.index().total);
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
