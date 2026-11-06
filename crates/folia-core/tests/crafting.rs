//! Focused tests for crafting-grid matching, furnaces, stonecutting, and
//! smithing against the shipped 1.21.11 recipe catalog.

use folia_core::crafting::{
    CraftingError, CraftingGrid, FurnaceKind, FurnaceState, fuel_burn_time, smith, smith_recipe,
    stonecut, stonecut_recipe,
};
use folia_core::inventory::{INVENTORY_SLOTS, ItemStack};
use folia_core::item_components::{ComponentValue, plain_text};
use folia_core::loot::RecipeCatalog;
use serde_json::{Value, json};

fn catalog() -> &'static RecipeCatalog {
    RecipeCatalog::global()
}

fn grid_with(width: usize, height: usize, cells: &[(usize, usize, &str, i32)]) -> CraftingGrid {
    let mut grid = CraftingGrid::empty(width, height).unwrap();
    for (x, y, item, count) in cells {
        assert!(grid.set(*x, *y, ItemStack::new(*item, *count)));
    }
    grid
}

fn fill_2x2(grid: &mut CraftingGrid, origin_x: usize, origin_y: usize, item: &str) {
    for y in 0..2 {
        for x in 0..2 {
            assert!(grid.set(origin_x + x, origin_y + y, ItemStack::new(item, 1)));
        }
    }
}

fn custom_catalog(recipes: Value, by_type: &[(&str, usize)]) -> RecipeCatalog {
    let total: usize = by_type.iter().map(|(_, count)| count).sum();
    let counts = by_type
        .iter()
        .map(|(kind, count)| ((*kind).to_owned(), json!(count)))
        .collect::<serde_json::Map<String, Value>>();
    let document = json!({
        "version": "1.21.11",
        "data_version": 4788,
        "counts": { "total": total, "by_type": counts },
        "recipes": recipes,
    });
    RecipeCatalog::from_json(&document.to_string()).unwrap()
}

#[test]
fn shaped_planks_match_every_2x2_offset_and_3x3_corner() {
    let mut grid = CraftingGrid::two_by_two();
    fill_2x2(&mut grid, 0, 0, "minecraft:oak_planks");
    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:crafting_table");
    assert_eq!(output.result.item, "minecraft:crafting_table");
    assert!(grid.is_empty());

    for (origin_x, origin_y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let mut grid = CraftingGrid::three_by_three();
        fill_2x2(&mut grid, origin_x, origin_y, "minecraft:oak_planks");
        let output = grid.craft(catalog()).unwrap();
        assert_eq!(
            output.recipe, "minecraft:crafting_table",
            "offset ({origin_x}, {origin_y})"
        );
        assert!(grid.is_empty(), "offset ({origin_x}, {origin_y})");
    }
}

#[test]
fn shaped_stairs_match_the_mirrored_pattern() {
    let mirrored = [(2, 0), (2, 1), (1, 1), (2, 2), (1, 2), (0, 2)];
    let mut grid = CraftingGrid::three_by_three();
    for (x, y) in mirrored {
        assert!(grid.set(x, y, ItemStack::new("minecraft:oak_planks", 1)));
    }
    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:oak_stairs");
    assert_eq!(output.result.count, 4);
    assert!(grid.is_empty());
}

#[test]
fn shapeless_log_to_planks_keeps_extra_items_in_the_slot() {
    let mut grid = CraftingGrid::two_by_two();
    assert!(grid.set(1, 1, ItemStack::new("minecraft:oak_log", 3)));
    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:oak_planks");
    assert_eq!(output.result.item, "minecraft:oak_planks");
    assert_eq!(output.result.count, 4);
    assert_eq!(grid.slot(3).unwrap().count, 2);
    assert_eq!(output.consumed_slots, vec![3]);
}

#[test]
fn shapeless_matching_assigns_overlapping_tag_and_exact_ingredients() {
    // Greedy matching would give the tag the oak plank and leave the exact
    // `oak_planks` ingredient unsatisfied; vanilla solves the matching.
    let recipes = json!({
        "minecraft:overlap_test": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": ["#minecraft:planks", "minecraft:oak_planks"],
            "result": { "id": "minecraft:stick" }
        }
    });
    let catalog = custom_catalog(recipes, &[("minecraft:crafting_shapeless", 1)]);
    let mut grid = CraftingGrid::two_by_two();
    assert!(grid.set(0, 0, ItemStack::new("minecraft:oak_planks", 1)));
    assert!(grid.set(1, 0, ItemStack::new("minecraft:birch_planks", 1)));
    let output = grid.craft(&catalog).unwrap();
    assert_eq!(output.result.item, "minecraft:stick");
}

#[test]
fn shapeless_matching_uses_item_tags() {
    let mut grid = CraftingGrid::three_by_three();
    // `minecraft:stick` accepts `#minecraft:planks`; birch is only a tag
    // member, not the recipe's concrete example item.
    assert!(grid.set(1, 0, ItemStack::new("minecraft:birch_planks", 1)));
    assert!(grid.set(1, 1, ItemStack::new("minecraft:birch_planks", 1)));
    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:stick");
    assert_eq!(output.result.count, 4);
    assert!(grid.is_empty());
}

#[test]
fn ingredient_alternatives_match_any_listed_item() {
    let recipes = json!({
        "minecraft:alternative_test": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": [[
                "minecraft:oak_planks",
                "minecraft:birch_planks"
            ]],
            "result": { "id": "minecraft:stick", "count": 1 }
        }
    });
    let catalog = custom_catalog(recipes, &[("minecraft:crafting_shapeless", 1)]);
    let mut grid = CraftingGrid::two_by_two();
    assert!(grid.set(1, 1, ItemStack::new("minecraft:birch_planks", 1)));
    let output = grid.craft(&catalog).unwrap();
    assert_eq!(output.result.item, "minecraft:stick");
}

#[test]
fn duplicate_shapeless_ingredients_need_duplicate_items() {
    let graph = json!({
        "minecraft:duplicate_test": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": ["minecraft:stick", "minecraft:stick"],
            "result": { "id": "minecraft:torch" }
        }
    });
    let catalog = custom_catalog(graph, &[("minecraft:crafting_shapeless", 1)]);

    let mut one_stick = CraftingGrid::two_by_two();
    one_stick.set(0, 0, ItemStack::new("minecraft:stick", 1));
    assert_eq!(one_stick.craft(&catalog), Err(CraftingError::NoRecipe));

    let mut two_sticks = CraftingGrid::two_by_two();
    two_sticks.set(0, 0, ItemStack::new("minecraft:stick", 1));
    two_sticks.set(1, 0, ItemStack::new("minecraft:stick", 1));
    let output = two_sticks.craft(&catalog).unwrap();
    assert_eq!(output.result.item, "minecraft:torch");
}

#[test]
fn conflicting_recipes_use_the_last_sorted_recipe_id() {
    // A 2x2 plank grid matches both a shapeless and a shaped recipe; vanilla
    // `RecipeManager.getRecipeFor` takes the last id in sorted order.
    let recipes = json!({
        "minecraft:conflict_a_shapeless": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": [
                "minecraft:oak_planks",
                "minecraft:oak_planks",
                "minecraft:oak_planks",
                "minecraft:oak_planks"
            ],
            "result": { "id": "minecraft:oak_button" }
        },
        "minecraft:conflict_z_shaped": {
            "type": "minecraft:crafting_shaped",
            "key": { "#": "minecraft:oak_planks" },
            "pattern": ["##", "##"],
            "result": { "id": "minecraft:crafting_table" }
        }
    });
    let catalog = custom_catalog(
        recipes,
        &[
            ("minecraft:crafting_shapeless", 1),
            ("minecraft:crafting_shaped", 1),
        ],
    );
    let mut grid = CraftingGrid::two_by_two();
    fill_2x2(&mut grid, 0, 0, "minecraft:oak_planks");
    let output = grid.craft(&catalog).unwrap();
    assert_eq!(output.recipe, "minecraft:conflict_z_shaped");

    // Reverse the ids: the shapeless recipe now has the winning id.
    let recipes = json!({
        "minecraft:conflict_a_shaped": {
            "type": "minecraft:crafting_shaped",
            "key": { "#": "minecraft:oak_planks" },
            "pattern": ["##", "##"],
            "result": { "id": "minecraft:crafting_table" }
        },
        "minecraft:conflict_z_shapeless": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": [
                "minecraft:oak_planks",
                "minecraft:oak_planks",
                "minecraft:oak_planks",
                "minecraft:oak_planks"
            ],
            "result": { "id": "minecraft:oak_button" }
        }
    });
    let catalog = custom_catalog(
        recipes,
        &[
            ("minecraft:crafting_shaped", 1),
            ("minecraft:crafting_shapeless", 1),
        ],
    );
    let mut grid = CraftingGrid::two_by_two();
    fill_2x2(&mut grid, 0, 0, "minecraft:oak_planks");
    let output = grid.craft(&catalog).unwrap();
    assert_eq!(output.recipe, "minecraft:conflict_z_shapeless");
}

#[test]
fn shipped_component_recipe_result_is_selected() {
    let mut grid = grid_with(
        2,
        2,
        &[
            (0, 0, "minecraft:bowl", 1),
            (1, 0, "minecraft:brown_mushroom", 1),
            (0, 1, "minecraft:red_mushroom", 1),
            (1, 1, "minecraft:allium", 1),
        ],
    );
    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:suspicious_stew_from_allium");
    assert_eq!(output.result.item, "minecraft:suspicious_stew");
}

#[test]
fn component_bearing_recipe_result_is_preserved() {
    let recipes = json!({
        "minecraft:component_result_test": {
            "type": "minecraft:crafting_shapeless",
            "ingredients": ["minecraft:stick"],
            "result": {
                "id": "minecraft:torch",
                "count": 2,
                "components": { "minecraft:custom_name": "Forged Torch" }
            }
        }
    });
    let catalog = custom_catalog(recipes, &[("minecraft:crafting_shapeless", 1)]);
    let mut grid = CraftingGrid::two_by_two();
    assert!(grid.set(0, 0, ItemStack::new("minecraft:stick", 1)));

    let output = grid.craft(&catalog).unwrap();
    assert_eq!(output.result.item, "minecraft:torch");
    assert_eq!(output.result.count, 2);
    assert_eq!(
        output.result.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Forged Torch")))
    );

    let mut inventory = vec![ItemStack::empty(); INVENTORY_SLOTS];
    let leftover = output.deposit(&mut inventory);
    assert!(leftover.is_empty());
    assert_eq!(inventory[0].item, "minecraft:torch");
    assert_eq!(inventory[0].count, 2);
    assert_eq!(
        inventory[0].get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Forged Torch")))
    );
}

#[test]
fn crafting_consumes_one_item_and_keeps_slot_components() {
    let mut named_planks = ItemStack::new("minecraft:oak_planks", 2);
    named_planks.set_component(ComponentValue::CustomName(plain_text("Named Plank")));
    let mut grid = grid_with(
        2,
        2,
        &[
            (1, 0, "minecraft:oak_planks", 1),
            (0, 1, "minecraft:oak_planks", 1),
            (1, 1, "minecraft:oak_planks", 1),
        ],
    );
    assert!(grid.set(0, 0, named_planks));

    let output = grid.craft(catalog()).unwrap();
    assert_eq!(output.recipe, "minecraft:crafting_table");
    assert_eq!(output.consumed_slots, vec![0, 1, 2, 3]);

    let remaining = grid.slot(0).unwrap();
    assert_eq!(remaining.item, "minecraft:oak_planks");
    assert_eq!(remaining.count, 1);
    assert_eq!(
        remaining.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Named Plank")))
    );
}

#[test]
fn furnace_timing_fuel_stacking_and_experience() {
    let mut furnace = FurnaceState::new(FurnaceKind::Furnace);
    furnace.set_input(ItemStack::new("minecraft:iron_ore", 1));
    furnace.set_fuel(ItemStack::new("minecraft:coal", 1));

    for _ in 0..199 {
        furnace.tick(catalog());
    }
    assert!(furnace.output.is_empty());
    assert_eq!(furnace.cooking_timer, 199);
    assert_eq!(furnace.cooking_total_time, 200);
    assert_eq!(furnace.lit_time_remaining, 1402);
    assert_eq!(furnace.lit_total_time, 1600);
    assert_eq!(furnace.fuel.count, 0);

    furnace.tick(catalog());
    assert_eq!(furnace.output.item, "minecraft:iron_ingot");
    assert_eq!(furnace.output.count, 1);
    assert_eq!(furnace.input.count, 0);
    assert!(furnace.fuel.is_empty());
    assert_eq!(furnace.lit_time_remaining, 1401);
    assert_eq!(furnace.cooking_timer, 0);
    assert_eq!(
        furnace
            .recipes_used
            .get("minecraft:iron_ingot_from_smelting_iron_ore"),
        Some(&1)
    );
    assert!((furnace.experience - 0.7).abs() < 1e-6);

    let extract = furnace.extract().unwrap();
    assert_eq!(extract.output.item, "minecraft:iron_ingot");
    assert!((extract.experience - 0.7).abs() < 1e-6);
    assert!(furnace.extract().is_none());
}

#[test]
fn furnace_does_not_burn_without_a_recipe() {
    let mut furnace = FurnaceState::new(FurnaceKind::Furnace);
    furnace.set_input(ItemStack::new("minecraft:diamond", 1));
    furnace.set_fuel(ItemStack::new("minecraft:coal", 1));
    for _ in 0..400 {
        furnace.tick(catalog());
    }
    assert!(furnace.output.is_empty());
    assert_eq!(furnace.fuel.count, 1);
    assert_eq!(furnace.lit_time_remaining, 0);
}

#[test]
fn cooking_stations_use_their_own_recipe_type_and_time() {
    let mut blast = FurnaceState::new(FurnaceKind::BlastFurnace);
    blast.set_input(ItemStack::new("minecraft:iron_ore", 1));
    blast.set_fuel(ItemStack::new("minecraft:coal", 1));
    for _ in 0..100 {
        blast.tick(catalog());
    }
    assert_eq!(blast.output.item, "minecraft:iron_ingot");
    assert_eq!(
        blast
            .recipes_used
            .get("minecraft:iron_ingot_from_blasting_iron_ore"),
        Some(&1)
    );

    let mut smoker = FurnaceState::new(FurnaceKind::Smoker);
    smoker.set_input(ItemStack::new("minecraft:porkchop", 1));
    smoker.set_fuel(ItemStack::new("minecraft:coal", 1));
    for _ in 0..100 {
        smoker.tick(catalog());
    }
    assert_eq!(smoker.output.item, "minecraft:cooked_porkchop");

    let mut campfire = FurnaceState::new(FurnaceKind::Campfire);
    campfire.set_input(ItemStack::new("minecraft:porkchop", 1));
    for _ in 0..599 {
        campfire.tick(catalog());
    }
    assert!(campfire.output.is_empty());
    campfire.tick(catalog());
    assert_eq!(campfire.output.item, "minecraft:cooked_porkchop");
    assert!((campfire.experience - 0.35).abs() < 1e-6);
}

#[test]
fn fuel_burn_time_table_covers_common_fuels() {
    assert_eq!(fuel_burn_time("minecraft:coal"), Some(1600));
    assert_eq!(fuel_burn_time("minecraft:charcoal"), Some(1600));
    assert_eq!(fuel_burn_time("minecraft:coal_block"), Some(16000));
    assert_eq!(fuel_burn_time("minecraft:lava_bucket"), Some(20000));
    assert_eq!(fuel_burn_time("minecraft:oak_planks"), Some(300));
    assert_eq!(fuel_burn_time("minecraft:oak_log"), Some(300));
    assert_eq!(fuel_burn_time("minecraft:oak_slab"), Some(150));
    assert_eq!(fuel_burn_time("minecraft:stick"), Some(100));
    assert_eq!(fuel_burn_time("minecraft:oak_boat"), Some(1200));
    assert_eq!(fuel_burn_time("minecraft:white_wool"), Some(100));
    assert_eq!(fuel_burn_time("minecraft:dried_kelp_block"), Some(4001));
    assert_eq!(fuel_burn_time("minecraft:crimson_planks"), None);
    assert_eq!(fuel_burn_time("minecraft:warped_stem"), None);
    assert_eq!(fuel_burn_time("minecraft:diamond"), None);
}

#[test]
fn stonecutting_is_a_one_shot_transform() {
    let mut input = ItemStack::new("minecraft:stone", 2);
    let output = stonecut_recipe(
        catalog(),
        "minecraft:stone_slab_from_stone_stonecutting",
        &mut input,
    )
    .unwrap();
    assert_eq!(output.result.item, "minecraft:stone_slab");
    assert_eq!(output.result.count, 2);
    assert_eq!(input.count, 1);

    let mut input = ItemStack::new("minecraft:stone", 1);
    let output = stonecut(catalog(), &mut input).unwrap();
    assert_eq!(output.result.item, "minecraft:stone_stairs");
    assert!(input.is_empty());
}

#[test]
fn smithing_transform_preserves_base_components() {
    let mut template = ItemStack::new("minecraft:netherite_upgrade_smithing_template", 1);
    let mut base = ItemStack::new("minecraft:diamond_chestplate", 1);
    base.set_component(ComponentValue::CustomName(plain_text("Old Faithful")));
    let mut addition = ItemStack::new("minecraft:netherite_ingot", 1);

    let output = smith_recipe(
        catalog(),
        "minecraft:netherite_chestplate_smithing",
        &mut template,
        &mut base,
        &mut addition,
    )
    .unwrap();
    assert_eq!(output.result.item, "minecraft:netherite_chestplate");
    assert_eq!(output.result.count, 1);
    assert_eq!(
        output.result.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Old Faithful")))
    );
    assert!(template.is_empty());
    assert!(base.is_empty());
    assert!(addition.is_empty());
}

#[test]
fn automatic_smithing_uses_the_last_sorted_transform() {
    let mut template = ItemStack::new("minecraft:netherite_upgrade_smithing_template", 1);
    let mut base = ItemStack::new("minecraft:diamond_chestplate", 1);
    let mut addition = ItemStack::new("minecraft:netherite_ingot", 1);
    let output = smith(catalog(), &mut template, &mut base, &mut addition).unwrap();
    assert_eq!(output.recipe, "minecraft:netherite_chestplate_smithing");
    assert_eq!(output.result.item, "minecraft:netherite_chestplate");
}

#[test]
fn special_decorated_and_trim_recipes_fail_loudly() {
    let special = CraftingGrid::two_by_two()
        .craft_recipe(catalog(), "minecraft:firework_rocket")
        .unwrap_err();
    assert!(matches!(
        special,
        CraftingError::UnsupportedRecipe { kind, .. }
            if kind == "minecraft:crafting_special_firework_rocket"
    ));

    let decorated = CraftingGrid::three_by_three()
        .craft_recipe(catalog(), "minecraft:decorated_pot")
        .unwrap_err();
    assert!(matches!(
        decorated,
        CraftingError::UnsupportedRecipe { kind, .. }
            if kind == "minecraft:crafting_decorated_pot"
    ));

    let mut template = ItemStack::new("minecraft:bolt_armor_trim_smithing_template", 1);
    let mut base = ItemStack::new("minecraft:iron_chestplate", 1);
    let mut addition = ItemStack::new("minecraft:redstone", 1);
    let trim = smith_recipe(
        catalog(),
        "minecraft:bolt_armor_trim_smithing_template_smithing_trim",
        &mut template,
        &mut base,
        &mut addition,
    )
    .unwrap_err();
    assert!(matches!(trim, CraftingError::UnsupportedRecipe { .. }));

    let unknown = CraftingGrid::two_by_two()
        .craft_recipe(catalog(), "minecraft:not_a_real_recipe")
        .unwrap_err();
    assert!(matches!(unknown, CraftingError::UnknownRecipe(_)));
}
