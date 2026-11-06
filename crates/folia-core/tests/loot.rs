//! Focused tests for the shipped loot-table and recipe catalogs.

use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::loot::{
    CookingKind, Ingredient, LootCatalog, LootCondition, LootContext, LootEntryKind,
    LootFunctionKind, LootTableKind, NameTarget, RecipeCatalog, RecipeKind, roll_table,
};
use folia_core::random::LegacyRandomSource;
use std::collections::BTreeMap;

fn silk_touch_pickaxe() -> ItemStack {
    let mut tool = ItemStack::new("minecraft:diamond_pickaxe", 1);
    tool.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:silk_touch".to_owned(),
        1,
    )])));
    tool
}

#[test]
fn shipped_catalogs_report_their_counts() {
    let loot = LootCatalog::global();
    assert_eq!(loot.index().version, "1.21.11");
    assert_eq!(loot.index().data_version, 4788);
    assert_eq!(loot.index().total, 1_324);
    assert_eq!(loot.index().by_type.get("minecraft:block"), Some(&1_083));
    assert_eq!(loot.index().by_type.get("minecraft:entity"), Some(&114));
    assert!(loot.table("minecraft:blocks/diamond_ore").is_some());
    assert!(loot.table("blocks/diamond_ore").is_some());

    let recipes = RecipeCatalog::global();
    assert_eq!(recipes.index().total, 1_470);
    assert_eq!(
        recipes.index().by_type.get("minecraft:crafting_shaped"),
        Some(&706)
    );
    assert_eq!(recipes.index().by_type.get("minecraft:smelting"), Some(&73));
    assert!(recipes.recipe("minecraft:diamond_pickaxe").is_some());
    assert!(recipes.recipe("diamond_pickaxe").is_some());
}

#[test]
fn diamond_ore_roll_honors_silk_touch() {
    let catalog = LootCatalog::global();
    let table = catalog
        .table("minecraft:blocks/diamond_ore")
        .expect("diamond ore table ships");
    assert_eq!(table.kind, LootTableKind::Block);
    assert_eq!(table.pools.len(), 1);
    let entry = &table.pools[0].entries[0];
    assert!(matches!(entry.kind, LootEntryKind::Alternatives(_)));

    let mut random = LegacyRandomSource::new(7);
    let plain = roll_table(catalog, table, &mut random, &LootContext::new());
    assert_eq!(plain.stacks.len(), 1);
    assert_eq!(plain.stacks[0].item, "minecraft:diamond");
    assert_eq!(plain.stacks[0].count, 1);
    assert!(plain.report.is_empty());

    let mut random = LegacyRandomSource::new(7);
    let tool = silk_touch_pickaxe();
    let silk = roll_table(
        catalog,
        table,
        &mut random,
        &LootContext::new().with_tool(&tool),
    );
    assert_eq!(silk.stacks.len(), 1);
    assert_eq!(silk.stacks[0].item, "minecraft:diamond_ore");
    assert_eq!(silk.stacks[0].count, 1);
    assert!(silk.report.is_empty());
}

#[test]
fn stone_roll_survives_a_missing_explosion_radius() {
    let catalog = LootCatalog::global();
    let table = catalog
        .table("minecraft:blocks/stone")
        .expect("stone table ships");
    let mut random = LegacyRandomSource::new(11);
    let result = roll_table(catalog, table, &mut random, &LootContext::new());
    assert_eq!(result.stacks.len(), 1);
    assert_eq!(result.stacks[0].item, "minecraft:cobblestone");

    let mut random = LegacyRandomSource::new(11);
    let exploded = roll_table(
        catalog,
        table,
        &mut random,
        &LootContext::new().with_explosion_radius(4.0),
    );
    assert!(
        exploded.stacks.is_empty() || exploded.stacks[0].item == "minecraft:cobblestone",
        "an explosion can only remove the drop, never change its item"
    );
}

#[test]
fn weighted_chest_rolls_produce_valid_stacks_and_counts() {
    let catalog = LootCatalog::global();
    let table = catalog
        .table("minecraft:chests/simple_dungeon")
        .expect("simple dungeon table ships");
    let mut random = LegacyRandomSource::new(2026);
    let result = roll_table(catalog, table, &mut random, &LootContext::new());
    assert!(!result.stacks.is_empty());
    assert!(result.report.is_empty());
    for stack in &result.stacks {
        assert!(
            stack.item.starts_with("minecraft:"),
            "{} is not an item id",
            stack.item
        );
        assert!(stack.count >= 1);
    }

    let leather: Vec<_> = result
        .stacks
        .iter()
        .filter(|stack| stack.item == "minecraft:leather")
        .collect();
    for stack in leather {
        assert!((1..=5).contains(&stack.count), "set_count bounds changed");
    }
}

#[test]
fn repeated_rolls_with_the_same_seed_are_deterministic() {
    let catalog = LootCatalog::global();
    let table = catalog
        .table("minecraft:chests/simple_dungeon")
        .expect("simple dungeon table ships");
    let mut first = LegacyRandomSource::new(99);
    let mut second = LegacyRandomSource::new(99);
    let left = roll_table(catalog, table, &mut first, &LootContext::new());
    let right = roll_table(catalog, table, &mut second, &LootContext::new());
    assert_eq!(left.stacks, right.stacks);
    assert_eq!(left.report, right.report);
}

#[test]
fn unknown_functions_and_conditions_are_preserved_and_reported() {
    let json = r#"{
        "version": "1.21.11",
        "data_version": 4788,
        "counts": { "total": 1, "by_type": { "minecraft:block": 1 } },
        "tables": {
            "minecraft:test/future": {
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1.0,
                    "bonus_rolls": 0.0,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:stone",
                        "conditions": [{ "condition": "minecraft:future_condition" }],
                        "functions": [{ "function": "minecraft:future_function" }]
                    }]
                }]
            }
        }
    }"#;
    let catalog = LootCatalog::from_json(json).expect("synthetic catalog parses");
    let table = catalog.table("test/future").expect("table is present");
    let entry = &table.pools[0].entries[0];
    match &entry.conditions[0] {
        LootCondition::Opaque { kind, .. } => {
            assert_eq!(kind, "minecraft:future_condition");
        }
        other => panic!("condition was not preserved opaquely: {other:?}"),
    }
    match &entry.functions[0].kind {
        LootFunctionKind::Opaque { kind, .. } => {
            assert_eq!(kind, "minecraft:future_function");
        }
        other => panic!("function was not preserved opaquely: {other:?}"),
    }

    let mut random = LegacyRandomSource::new(1);
    let result = roll_table(&catalog, table, &mut random, &LootContext::new());
    assert!(result.stacks.is_empty());
    assert!(
        result
            .report
            .unsupported_conditions
            .contains("minecraft:future_condition")
    );
}

#[test]
fn recipes_parse_all_vanilla_serializers() {
    let catalog = RecipeCatalog::global();

    let pickaxe = catalog
        .recipe("minecraft:diamond_pickaxe")
        .expect("diamond pickaxe recipe ships");
    let RecipeKind::Shaped(shaped) = &pickaxe.kind else {
        panic!("diamond pickaxe is not shaped: {:?}", pickaxe.kind);
    };
    assert_eq!(shaped.pattern, vec!["XXX", " # ", " # "]);
    assert_eq!(shaped.result.item, "minecraft:diamond_pickaxe");
    assert_eq!(shaped.result.count, 1);
    assert!(matches!(
        shaped.key.get(&'X'),
        Some(Ingredient::Tag(tag)) if tag == "minecraft:diamond_tool_materials"
    ));
    assert!(
        shaped
            .key
            .get(&'X')
            .expect("X ingredient")
            .matches("minecraft:diamond")
    );

    let button = catalog
        .recipe("minecraft:acacia_button")
        .expect("acacia button recipe ships");
    let RecipeKind::Shapeless(shapeless) = &button.kind else {
        panic!("acacia button is not shapeless: {:?}", button.kind);
    };
    assert_eq!(shapeless.ingredients.len(), 1);
    assert_eq!(shapeless.result.item, "minecraft:acacia_button");

    let smelting = catalog
        .recipe("minecraft:iron_ingot_from_smelting_raw_iron")
        .expect("iron smelting recipe ships");
    let RecipeKind::Cooking(cooking) = &smelting.kind else {
        panic!("iron recipe is not cooking: {:?}", smelting.kind);
    };
    assert_eq!(cooking.cooking, CookingKind::Smelting);
    assert_eq!(cooking.cooking_time, 200);
    assert!((cooking.experience - 0.7).abs() < f32::EPSILON);
    assert!(cooking.ingredient.matches("minecraft:raw_iron"));
    assert_eq!(cooking.result.item, "minecraft:iron_ingot");
    assert_eq!(
        catalog
            .smelting_result("minecraft:raw_iron")
            .map(|stack| stack.item.as_str()),
        Some("minecraft:iron_ingot")
    );

    let stonecutting = catalog
        .recipe("minecraft:andesite_slab_from_andesite_stonecutting")
        .expect("andesite stonecutting recipe ships");
    let RecipeKind::Stonecutting(cut) = &stonecutting.kind else {
        panic!(
            "andesite recipe is not stonecutting: {:?}",
            stonecutting.kind
        );
    };
    assert_eq!(cut.result.count, 2);
    assert!(cut.ingredient.matches("minecraft:andesite"));

    let smithing = catalog
        .recipe("minecraft:netherite_axe_smithing")
        .expect("netherite axe smithing recipe ships");
    let RecipeKind::SmithingTransform(transform) = &smithing.kind else {
        panic!(
            "netherite axe is not a smithing transform: {:?}",
            smithing.kind
        );
    };
    assert_eq!(transform.result.item, "minecraft:netherite_axe");
    assert!(matches!(
        &transform.addition,
        Ingredient::Tag(tag) if tag == "minecraft:netherite_tool_materials"
    ));

    let trim = catalog
        .recipe("minecraft:bolt_armor_trim_smithing_template_smithing_trim")
        .expect("bolt trim recipe ships");
    let RecipeKind::SmithingTrim(trim) = &trim.kind else {
        panic!("bolt recipe is not a smithing trim: {:?}", trim.kind);
    };
    assert_eq!(trim.pattern.as_deref(), Some("minecraft:bolt"));

    let opaque: Vec<&str> = catalog
        .recipes()
        .filter_map(|(id, recipe)| matches!(&recipe.kind, RecipeKind::Opaque { .. }).then_some(id))
        .collect();
    assert!(
        opaque.is_empty(),
        "shipped recipes still parse as opaque: {opaque:?}"
    );
}

#[test]
fn recipe_special_types_stay_explicit() {
    let catalog = RecipeCatalog::global();
    let recipe = catalog
        .recipe("minecraft:firework_rocket")
        .expect("firework rocket recipe ships");
    assert!(matches!(
        &recipe.kind,
        RecipeKind::Special { kind } if kind == "minecraft:crafting_special_firework_rocket"
    ));
}

#[test]
fn set_name_and_set_damage_functions_execute_with_item_components() {
    // A synthetic table keeps this focused on the two ported component
    // functions instead of depending on an unrelated shipped drop.
    let json = r#"{
        "version": "1.21.11",
        "data_version": 4788,
        "counts": { "total": 1, "by_type": { "minecraft:block": 1 } },
        "tables": {
            "minecraft:test/components": {
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1.0,
                    "bonus_rolls": 0.0,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:diamond_pickaxe",
                        "functions": [
                            { "function": "minecraft:set_components", "components": { "minecraft:max_damage": 100 } },
                            { "function": "minecraft:set_damage", "damage": 0.25 },
                            { "function": "minecraft:set_name", "name": { "text": "Miner" } }
                        ]
                    }]
                }]
            }
        }
    }"#;
    let catalog = LootCatalog::from_json(json).expect("synthetic catalog parses");
    let table = catalog.table("test/components").expect("table is present");
    let mut random = LegacyRandomSource::new(3);
    let result = roll_table(&catalog, table, &mut random, &LootContext::new());
    assert_eq!(result.stacks.len(), 1);
    let stack = &result.stacks[0];
    assert!(matches!(
        stack.get_component("minecraft:damage"),
        Some(ComponentValue::Damage(_))
    ));
    assert!(matches!(
        stack.get_component("minecraft:custom_name"),
        Some(ComponentValue::CustomName(_))
    ));
    let _ = NameTarget::ItemName;
}
