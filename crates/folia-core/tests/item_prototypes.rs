//! Item prototype catalog tests.
//!
//! The catalog is a Java-oracle dump: `scripts/extract_item_prototypes.ps1`
//! boots the local 1.21.11 server jar and reads the prototype components from
//! `BuiltInRegistries.ITEM`. These tests pin the generated asset, spot-check
//! representative vanilla items, and prove the component-patch bridge into
//! `crate::item_components`.

use folia_core::item_components::{BlockSet, ComponentValue, ItemStack, Rarity, ToolRule};
use folia_core::item_prototypes::{
    ABSOLUTE_MAX_STACK_SIZE, FoodPrototype, IngredientSet, ItemCatalog, apply_defaults, catalog,
    defaults_for, prototype, prototype_by_id, prototype_or_default,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// FNV-1a 64, the dependency-free checksum used by `tests/assets.rs`.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn catalog_json(item: &str) -> String {
    format!(
        concat!(
            r#"{{"counts":{{"by_rarity":{{"common":1,"epic":0,"rare":0,"uncommon":0}},"#,
            r#""damageable":0,"enchantable":0,"equippable":0,"fire_resistant":0,"#,
            r#""food":0,"repairable":0,"tool":0,"total":1}},"#,
            r#""data_version":4788,"items":{{"minecraft:test":{}}},"version":"1.21.11"}}"#
        ),
        item
    )
}

#[test]
fn catalog_covers_the_vanilla_item_registry() {
    let catalog = catalog();
    let registry = String::from_utf8(asset("item_ids.tsv")).expect("item_ids.tsv is UTF-8");
    let registry_items = registry.lines().count() - 1; // minecraft:air is not an item

    assert_eq!(registry_items, 1_504);
    assert_eq!(catalog.len(), registry_items);
    assert_eq!(catalog.counts().total, registry_items);
    assert_eq!(catalog.counts().by_rarity.get("common"), Some(&1_390));
    assert_eq!(catalog.counts().by_rarity.get("uncommon"), Some(&77));
    assert_eq!(catalog.counts().by_rarity.get("rare"), Some(&18));
    assert_eq!(catalog.counts().by_rarity.get("epic"), Some(&19));
    for (field, count) in [
        ("damageable", 84),
        ("enchantable", 77),
        ("equippable", 84),
        ("fire_resistant", 16),
        ("food", 44),
        ("repairable", 75),
        ("tool", 38),
    ] {
        let value = match field {
            "damageable" => catalog.counts().damageable,
            "enchantable" => catalog.counts().enchantable,
            "equippable" => catalog.counts().equippable,
            "fire_resistant" => catalog.counts().fire_resistant,
            "food" => catalog.counts().food,
            "repairable" => catalog.counts().repairable,
            "tool" => catalog.counts().tool,
            _ => unreachable!(),
        };
        assert_eq!(value, count, "{field} count changed");
    }

    assert!(catalog.contains("minecraft:apple"));
    assert!(
        catalog.contains("apple"),
        "bare ids normalize to minecraft:"
    );
    assert!(!catalog.contains("minecraft:definitely_not_an_item"));
}

#[test]
fn diamond_sword_has_durability_tool_and_attack_damage() {
    let sword = prototype("minecraft:diamond_sword").expect("diamond sword exists");
    assert_eq!(sword.max_stack_size, 1);
    assert_eq!(sword.max_damage, Some(1_561));
    assert_eq!(sword.enchantability, Some(10));
    assert_eq!(sword.rarity, Rarity::Common);
    assert!(!sword.fire_resistant);

    let tool = sword.tool.as_ref().expect("diamond sword is a tool");
    assert_eq!(tool.tier.as_deref(), Some("diamond"));
    assert_eq!(tool.default_mining_speed, 1.0);
    assert_eq!(tool.damage_per_block, 2);
    assert!(!tool.can_destroy_blocks_in_creative);
    assert_eq!(tool.attack_damage, Some(6.0));
    assert_eq!(tool.attack_speed, Some(-2.4));
    assert_eq!(tool.rules.len(), 3);
    assert_eq!(
        tool.rules[0],
        folia_core::item_prototypes::ToolRulePrototype {
            blocks: IngredientSet::Items(vec!["minecraft:cobweb".into()]),
            speed: Some(15.0),
            correct_for_drops: Some(true),
        }
    );
    assert_eq!(
        tool.rules[1].blocks,
        IngredientSet::Tag("minecraft:sword_instantly_mines".into())
    );
    assert_eq!(tool.rules[1].speed, Some(f32::MAX));
    assert_eq!(
        tool.rules[2].blocks,
        IngredientSet::Tag("minecraft:sword_efficient".into())
    );
    assert_eq!(tool.rules[2].speed, Some(1.5));
}

#[test]
fn netherite_pickaxe_is_fire_resistant_and_mines_with_netherite_speed() {
    let pickaxe = prototype("minecraft:netherite_pickaxe").expect("netherite pickaxe exists");
    assert_eq!(pickaxe.max_stack_size, 1);
    assert_eq!(pickaxe.max_damage, Some(2_031));
    assert!(pickaxe.fire_resistant);
    assert_eq!(pickaxe.enchantability, Some(15));

    let tool = pickaxe.tool.as_ref().expect("netherite pickaxe is a tool");
    assert_eq!(tool.tier.as_deref(), Some("netherite"));
    assert_eq!(tool.default_mining_speed, 1.0);
    assert_eq!(tool.damage_per_block, 1);
    assert!(tool.can_destroy_blocks_in_creative);
    assert_eq!(tool.attack_damage, Some(5.0));
    assert_eq!(tool.attack_speed, Some(-2.8));
    assert_eq!(tool.rules.len(), 2);
    assert_eq!(
        tool.rules[0].blocks,
        IngredientSet::Tag("minecraft:incorrect_for_netherite_tool".into())
    );
    assert_eq!(tool.rules[0].correct_for_drops, Some(false));
    assert_eq!(
        tool.rules[1].blocks,
        IngredientSet::Tag("minecraft:mineable/pickaxe".into())
    );
    assert_eq!(tool.rules[1].speed, Some(9.0));
    assert_eq!(tool.rules[1].correct_for_drops, Some(true));
}

#[test]
fn apple_has_vanilla_food_values_and_eat_time() {
    let apple = prototype("minecraft:apple").expect("apple exists");
    assert_eq!(apple.max_stack_size, 64);
    assert_eq!(apple.max_damage, None);
    assert_eq!(
        apple.food.as_ref(),
        Some(&FoodPrototype {
            nutrition: 4,
            saturation: 2.4,
            can_always_eat: false,
            eat_seconds: 1.6,
        })
    );
    assert_eq!(apple.eat_ticks(), Some(32));

    let golden = prototype("minecraft:golden_apple").expect("golden apple exists");
    let food = golden.food.as_ref().expect("golden apple is food");
    assert_eq!(food.nutrition, 4);
    assert_eq!(food.saturation, 9.6);
    assert!(food.can_always_eat, "golden apples are edible when full");
}

#[test]
fn potions_stack_to_one_and_ender_pearls_to_sixteen() {
    assert_eq!(prototype("minecraft:potion").unwrap().max_stack_size, 1);
    assert_eq!(
        prototype("minecraft:ender_pearl").unwrap().max_stack_size,
        16
    );
    assert_eq!(
        prototype("minecraft:splash_potion").unwrap().max_stack_size,
        1
    );
    assert_eq!(
        prototype("minecraft:lingering_potion")
            .unwrap()
            .max_stack_size,
        1
    );
    assert_eq!(ABSOLUTE_MAX_STACK_SIZE, 99);
}

#[test]
fn elytra_has_durability_and_chest_slot() {
    let elytra = prototype("minecraft:elytra").expect("elytra exists");
    assert_eq!(elytra.max_stack_size, 1);
    assert_eq!(elytra.max_damage, Some(432));
    assert_eq!(elytra.equippable_slot.as_deref(), Some("chest"));
    assert_eq!(elytra.rarity, Rarity::Epic);
    assert_eq!(
        elytra.repairable,
        Some(IngredientSet::Items(vec![
            "minecraft:phantom_membrane".into()
        ]))
    );
}

#[test]
fn plain_block_items_keep_the_no_override_fallback() {
    for item in ["minecraft:stone", "minecraft:dirt", "minecraft:oak_planks"] {
        let prototype = prototype_or_default(item);
        assert_eq!(prototype.max_stack_size, 64, "{item} stack size");
        assert_eq!(prototype.max_damage, None, "{item} durability");
        assert!(prototype.tool.is_none(), "{item} tool");
        assert!(prototype.food.is_none(), "{item} food");
        assert!(prototype.equippable_slot.is_none(), "{item} slot");
        assert!(prototype.enchantability.is_none(), "{item} enchantability");
        assert!(prototype.repairable.is_none(), "{item} repairable");
        assert!(!prototype.fire_resistant, "{item} fire resistance");
        assert_eq!(prototype.rarity, Rarity::Common, "{item} rarity");
        assert!(
            defaults_for(item).is_empty(),
            "{item} has no prototype patch over the shared defaults"
        );
    }

    let unknown = prototype_or_default("minecraft:definitely_not_an_item");
    assert_eq!(unknown.max_stack_size, 64);
    assert_eq!(unknown.max_damage, None);
    assert!(prototype("minecraft:definitely_not_an_item").is_none());
}

#[test]
fn component_patch_applies_stack_limits_durability_food_and_tools() {
    let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
    assert_eq!(
        sword.max_stack_size(),
        64,
        "without prototype defaults the old fallback is 64"
    );
    let applied = apply_defaults(&mut sword);
    assert_eq!(applied, 4, "stack size, max damage, damage, and tool");
    assert_eq!(sword.max_stack_size(), 1);
    assert_eq!(sword.max_damage(), 1_561);
    assert_eq!(sword.damage(), 0);
    assert!(matches!(
        sword.get_component("minecraft:tool"),
        Some(ComponentValue::Tool(_))
    ));
    let ComponentValue::Tool(tool) = sword.get_component("minecraft:tool").unwrap() else {
        unreachable!()
    };
    assert_eq!(tool.rules.len(), 3);
    assert_eq!(
        tool.rules[0],
        ToolRule {
            blocks: BlockSet::Blocks(vec!["minecraft:cobweb".into()]),
            speed: Some(15.0),
            correct_for_drops: Some(true),
        }
    );
    assert_eq!(
        apply_defaults(&mut sword),
        0,
        "applying twice is idempotent"
    );

    let mut apple = ItemStack::new("minecraft:apple", 1);
    assert_eq!(apply_defaults(&mut apple), 1);
    assert_eq!(apple.max_stack_size(), 64);
    assert!(matches!(
        apple.get_component("minecraft:food"),
        Some(ComponentValue::Food(food)) if food.nutrition == 4 && food.saturation == 2.4
    ));

    let mut potion = ItemStack::new("minecraft:potion", 1);
    assert_eq!(apply_defaults(&mut potion), 1);
    assert_eq!(potion.max_stack_size(), 1);

    let mut pearl = ItemStack::new("minecraft:ender_pearl", 1);
    assert_eq!(apply_defaults(&mut pearl), 1);
    assert_eq!(pearl.max_stack_size(), 16);

    let mut stone = ItemStack::new("minecraft:stone", 64);
    assert_eq!(apply_defaults(&mut stone), 0);
    assert_eq!(stone.max_stack_size(), 64);

    let mut modded = ItemStack::new("example:custom_item", 1);
    assert_eq!(apply_defaults(&mut modded), 0);
    assert_eq!(modded.max_stack_size(), 64);
}

#[test]
fn apply_defaults_respects_explicit_component_removals() {
    let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
    sword.mark_component_removed("minecraft:max_damage");
    apply_defaults(&mut sword);
    assert!(sword.components.is_removed("minecraft:max_damage"));
    assert_eq!(sword.max_damage(), 0);
    assert_eq!(sword.get_component("minecraft:max_damage"), None);
}

#[test]
fn lookup_by_registry_id_matches_name_lookup() {
    let by_name = prototype("minecraft:diamond_sword").unwrap();
    let by_id = prototype_by_id(936).expect("diamond sword registry id");
    assert_eq!(by_name, by_id);
    assert_eq!(catalog().get_by_id(936), Some(by_name));
    assert!(prototype_by_id(-1).is_none());
    assert!(prototype_by_id(i32::MAX).is_none());
}

#[test]
fn generated_catalog_is_byte_stable() {
    let bytes = asset("item_prototypes.json");
    assert_eq!(bytes.len(), 79_369, "catalog byte length changed");
    assert_eq!(
        fnv1a64(&bytes),
        0xf179_dfed_29cd_cdad,
        "catalog bytes changed; regenerate with scripts/extract_item_prototypes.ps1"
    );
    assert!(
        bytes.ends_with(b"}\n"),
        "catalog must end with a single newline"
    );
    assert!(
        !bytes.contains(&b'\r'),
        "catalog must use LF line endings only"
    );
    // Parsing the same bytes twice must produce the same catalog; this is the
    // loader-side half of the determinism guarantee.
    let first = ItemCatalog::from_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let second = ItemCatalog::from_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(first.len(), second.len());
    assert_eq!(first.counts(), second.counts());
    for (name, prototype) in first.iter() {
        assert_eq!(Some(prototype), second.get(name));
    }
}

#[test]
fn minimal_entries_parse_and_malformed_entries_fail_loudly() {
    let minimal = ItemCatalog::from_json(&catalog_json(r#"{"max_stack_size":16}"#)).unwrap();
    let prototype = minimal.get("minecraft:test").unwrap();
    assert_eq!(prototype.max_stack_size, 16);
    assert_eq!(prototype.max_damage, None);

    for malformed in [
        r#"{"max_stack_size":0}"#,
        r#"{"max_stack_size":100}"#,
        r#"{"max_damage":10}"#,
        r#"{"max_damage":10,"max_stack_size":64}"#,
        r#"{"rarity":"legendary"}"#,
        r#"{"equippable":"tail"}"#,
        r#"{"enchantability":0}"#,
        r#"{"food":{"nutrition":-1,"saturation":1.0}}"#,
        r#"{"food":{"nutrition":4,"saturation":-1.0}}"#,
        r##"{"tool":{"rules":[{"blocks":{"tag":"#minecraft:mineable/pickaxe"}}]}}"##,
        r#"{"tool":{"rules":[{"blocks":{"items":[]}}]}}"#,
        r#"{"tool":{"rules":[{"blocks":{"items":["cobweb"]}}]}}"#,
        r#"{"unknown_field":true}"#,
    ] {
        assert!(
            ItemCatalog::from_json(&catalog_json(malformed)).is_err(),
            "malformed entry must fail: {malformed}"
        );
    }

    let mut wrong_counts = catalog_json(r#"{"max_stack_size":16}"#);
    wrong_counts = wrong_counts.replace(r#""total":1"#, r#""total":2"#);
    assert!(
        ItemCatalog::from_json(&wrong_counts).is_err(),
        "a counts index that disagrees with items must fail"
    );

    let wrong_version = catalog_json("{}").replace("1.21.11", "1.21.10");
    assert!(ItemCatalog::from_json(&wrong_version).is_err());
}

fn oracle_jar() -> Option<PathBuf> {
    [
        PathBuf::from(
            r"C:\Users\austr\Downloads\rustfol\java-worldgen\versions\1.21.11\folia-1.21.11.jar",
        ),
        PathBuf::from(r"C:\Users\austr\Downloads\rustfol\folia-1.21.11-14.jar"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[test]
fn extractor_regenerates_byte_identical_output() {
    if !cfg!(windows) {
        eprintln!("skipping extractor check: the local Java oracle is Windows-only");
        return;
    }
    if std::env::var_os("FOLIA_SKIP_ITEM_ORACLE").is_some() {
        eprintln!("skipping extractor check: FOLIA_SKIP_ITEM_ORACLE is set");
        return;
    }
    if oracle_jar().is_none() {
        eprintln!("skipping extractor check: local 1.21.11 server jar is not present");
        return;
    }
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/extract_item_prototypes.ps1");
    assert!(script.is_file(), "extractor script is missing");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Check")
        .output()
        .expect("run extract_item_prototypes.ps1 -Check");
    assert!(
        output.status.success(),
        "extractor check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
