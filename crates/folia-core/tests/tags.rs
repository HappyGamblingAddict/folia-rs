//! Membership checks for the flattened vanilla 1.21.11 tag tables.
//!
//! The expected counts are the exact output of `scripts/extract_tags.ps1`.
//! Regenerating the assets requires updating these constants, the manifest in
//! `assets/ASSETS.md`, and the pinned values in `tests/assets.rs`.

use folia_core::tags::{self, TagRegistryKind};

fn members_of(registry: TagRegistryKind, tag: &str) -> Vec<&str> {
    tags::members(registry, tag)
        .iter()
        .map(String::as_str)
        .collect()
}

#[test]
fn block_tags_contain_the_expected_vanilla_members() {
    for (tag, member) in [
        ("minecraft:logs", "minecraft:oak_log"),
        ("minecraft:planks", "minecraft:oak_planks"),
        ("minecraft:dirt", "minecraft:grass_block"),
        ("minecraft:leaves", "minecraft:oak_leaves"),
        ("minecraft:saplings", "minecraft:oak_sapling"),
        ("minecraft:base_stone_overworld", "minecraft:stone"),
        ("minecraft:base_stone_overworld", "minecraft:deepslate"),
    ] {
        assert!(
            tags::block_has(tag, member),
            "{member} should be in block tag {tag}"
        );
    }

    assert!(!tags::block_has("minecraft:logs", "minecraft:stone"));
    assert!(!tags::block_has("minecraft:planks", "minecraft:stone"));
    assert!(!tags::block_has("minecraft:dirt", "minecraft:stone"));

    assert_eq!(
        members_of(TagRegistryKind::Block, "minecraft:stone_ore_replaceables"),
        [
            "minecraft:andesite",
            "minecraft:diorite",
            "minecraft:granite",
            "minecraft:stone",
        ]
    );
    assert_eq!(
        members_of(TagRegistryKind::Block, "minecraft:moss_replaceable"),
        [
            "minecraft:andesite",
            "minecraft:cave_vines",
            "minecraft:cave_vines_plant",
            "minecraft:coarse_dirt",
            "minecraft:deepslate",
            "minecraft:diorite",
            "minecraft:dirt",
            "minecraft:granite",
            "minecraft:grass_block",
            "minecraft:moss_block",
            "minecraft:mud",
            "minecraft:muddy_mangrove_roots",
            "minecraft:mycelium",
            "minecraft:pale_moss_block",
            "minecraft:podzol",
            "minecraft:rooted_dirt",
            "minecraft:stone",
            "minecraft:tuff",
        ]
    );
}

#[test]
fn item_fluid_and_entity_tags_match_vanilla() {
    assert!(tags::item_has("minecraft:logs", "minecraft:oak_log"));
    assert!(tags::item_has("minecraft:planks", "minecraft:oak_planks"));
    assert!(tags::item_has("logs", "stripped_oak_log"));
    assert!(!tags::item_has("minecraft:logs", "minecraft:stone"));
    assert!(!tags::item_has("minecraft:planks", "minecraft:stone"));

    assert_eq!(
        members_of(TagRegistryKind::Fluid, "minecraft:water"),
        ["minecraft:flowing_water", "minecraft:water"]
    );
    assert_eq!(
        members_of(TagRegistryKind::Fluid, "minecraft:lava"),
        ["minecraft:flowing_lava", "minecraft:lava"]
    );

    assert!(tags::has(
        TagRegistryKind::EntityType,
        "minecraft:arrows",
        "minecraft:arrow"
    ));
    assert!(tags::has(
        TagRegistryKind::EntityType,
        "minecraft:arrows",
        "minecraft:spectral_arrow"
    ));
    assert!(!tags::has(
        TagRegistryKind::EntityType,
        "minecraft:arrows",
        "minecraft:zombie"
    ));
}

#[test]
fn nested_tag_references_are_fully_flattened() {
    let logs = members_of(TagRegistryKind::Block, "minecraft:logs");

    // `logs` contains only `#logs_that_burn`, `#crimson_stems`, and
    // `#warped_stems`; every returned entry must therefore be a concrete block.
    assert!(
        logs.len() > 40,
        "logs should include all nested log variants"
    );
    assert!(logs.contains(&"minecraft:oak_log"));
    assert!(logs.contains(&"minecraft:stripped_oak_log"));
    assert!(logs.contains(&"minecraft:crimson_stem"));
    assert!(logs.contains(&"minecraft:warped_stem"));
    assert!(logs.contains(&"minecraft:pale_oak_log"));
    assert!(logs.iter().all(|member| member.starts_with("minecraft:")));
    assert!(logs.iter().all(|member| !member.starts_with('#')));
    assert!(!logs.contains(&"minecraft:stone"));

    let mut sorted = logs.clone();
    sorted.sort_unstable();
    assert_eq!(logs, sorted, "tag members are sorted in the asset");

    // Worldgen tag keys are relative to their subregistry directory, so the
    // biome, structure, flat-preset, and world-preset files share one asset.
    assert!(tags::has(
        TagRegistryKind::Worldgen,
        "minecraft:is_forest",
        "minecraft:forest"
    ));
    assert!(tags::has(
        TagRegistryKind::Worldgen,
        "minecraft:has_structure/village_plains",
        "minecraft:plains"
    ));
    assert!(tags::has(
        TagRegistryKind::Worldgen,
        "minecraft:village",
        "minecraft:village_plains"
    ));
    assert!(tags::has(
        TagRegistryKind::Worldgen,
        "minecraft:visible",
        "minecraft:classic_flat"
    ));
}

#[test]
fn every_registry_has_the_expected_tag_and_member_counts() {
    // `tag_count` counts non-empty tags: the asset format omits empty tags.
    // In 1.21.11 block has 204 files, of which `incorrect_for_diamond_tool`
    // and `incorrect_for_netherite_tool` are empty.
    let expected = [
        (TagRegistryKind::Block, 202usize, 3655usize),
        (TagRegistryKind::Item, 198, 1897),
        (TagRegistryKind::EntityType, 46, 368),
        (TagRegistryKind::Fluid, 2, 4),
        (TagRegistryKind::GameEvent, 5, 119),
        (TagRegistryKind::DamageType, 33, 196),
        (TagRegistryKind::Enchantment, 22, 308),
        (TagRegistryKind::PointOfInterestType, 3, 30),
        (TagRegistryKind::Worldgen, 91, 631),
    ];

    for (registry, tag_count, member_count) in expected {
        assert_eq!(
            tags::tag_count(registry),
            tag_count,
            "{} tag count changed",
            registry.name()
        );
        assert_eq!(
            tags::member_count(registry),
            member_count,
            "{} member count changed",
            registry.name()
        );
    }
}

#[test]
fn unknown_tags_and_members_are_empty_and_false() {
    assert!(tags::members(TagRegistryKind::Block, "minecraft:not_a_tag").is_empty());
    assert!(tags::members(TagRegistryKind::Item, "not_a_tag").is_empty());
    assert!(!tags::has(
        TagRegistryKind::Block,
        "minecraft:not_a_tag",
        "minecraft:stone"
    ));
    assert!(!tags::has(
        TagRegistryKind::Block,
        "minecraft:logs",
        "minecraft:not_a_block"
    ));
    assert!(!tags::block_has("minecraft:not_a_tag", "minecraft:stone"));
    assert!(!tags::block_has("minecraft:logs", "minecraft:not_a_block"));
    assert!(!tags::item_has("minecraft:logs", "minecraft:stone"));
}
