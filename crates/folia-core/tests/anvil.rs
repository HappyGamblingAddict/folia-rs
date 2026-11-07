//! Focused tests for the anvil combine/repair/rename port.
//!
//! The expected values follow `AnvilMenu.createResult` in the 1.21.11
//! decompilation: material repair consumes at most a quarter of the maximum
//! durability per item, same-item combining adds 12% of the left maximum,
//! enchantment costs are `anvil_cost * level` (halved for enchanted books),
//! and `repair_cost` accumulates as `old * 2 + 1` with a 40-level hard limit.

use folia_core::item_components::enchanting::{
    ANVIL_MAXIMUM_REPAIR_COST, AnvilError, AnvilOptions, calculate_increased_repair_cost,
    combine_in_anvil, combine_in_anvil_with, definition,
};
use folia_core::item_components::{ComponentValue, ItemStack, Rarity};
use folia_core::nbt::Tag;
use std::collections::BTreeMap;

fn sword(damage: i32, max_damage: i32) -> ItemStack {
    let mut stack = ItemStack::new("minecraft:diamond_sword", 1);
    stack.set_component(ComponentValue::MaxDamage(max_damage));
    stack.set_component(ComponentValue::Damage(damage));
    stack
}

fn bare_sword(damage: i32) -> ItemStack {
    let mut stack = ItemStack::new("minecraft:diamond_sword", 1);
    stack.set_component(ComponentValue::Damage(damage));
    stack
}

fn enchanted_book(enchantments: BTreeMap<String, i32>) -> ItemStack {
    let mut stack = ItemStack::new("minecraft:enchanted_book", 1);
    stack.set_component(ComponentValue::StoredEnchantments(enchantments));
    stack
}

fn enchantments(stack: &ItemStack, component: &str) -> BTreeMap<String, i32> {
    match stack.get_component(component) {
        Some(ComponentValue::Enchantments(map)) | Some(ComponentValue::StoredEnchantments(map)) => {
            map.clone()
        }
        Some(other) => panic!("unexpected {component}: {other:?}"),
        None => panic!("missing {component}"),
    }
}

fn map(entries: &[(&str, i32)]) -> BTreeMap<String, i32> {
    entries
        .iter()
        .map(|(name, level)| ((*name).to_owned(), *level))
        .collect()
}

fn repair_cost(stack: &ItemStack) -> Option<i32> {
    match stack.get_component("minecraft:repair_cost") {
        Some(ComponentValue::RepairCost(cost)) => Some(*cost),
        Some(other) => panic!("unexpected repair_cost: {other:?}"),
        None => None,
    }
}

#[test]
fn sword_plus_sword_merges_durability_and_enchantments() {
    let mut left = sword(600, 1000);
    left.set_component(ComponentValue::Enchantments(map(&[
        ("minecraft:sharpness", 2),
        ("minecraft:unbreaking", 1),
    ])));
    left.set_component(ComponentValue::Lore(vec![Tag::String("A blade".into())]));
    left.set_component(ComponentValue::CustomData(Tag::Compound(vec![(
        "owner".into(),
        Tag::String("steve".into()),
    )])));
    left.set_component(ComponentValue::Rarity(Rarity::Epic));

    let mut right = sword(400, 1000);
    right.set_component(ComponentValue::Enchantments(map(&[
        ("minecraft:sharpness", 2),
        ("minecraft:mending", 1),
    ])));

    let left_before = left.clone();
    let right_before = right.clone();
    let outcome = combine_in_anvil(&left, &right).unwrap();

    assert_eq!(outcome.result.item, "minecraft:diamond_sword");
    assert_eq!(outcome.result.count, 1);
    // 400 + 600 remaining durability + 12% of 1000 repairs it completely.
    assert_eq!(outcome.result.damage(), 0);
    assert_eq!(
        enchantments(&outcome.result, "minecraft:enchantments"),
        map(&[
            ("minecraft:mending", 1),
            ("minecraft:sharpness", 3),
            ("minecraft:unbreaking", 1),
        ])
    );
    // 2 (durability) + 4 (mending) + 3 (sharpness 1 * level 3).
    assert_eq!(outcome.cost, 9);
    assert_eq!(outcome.level_requirement(), 9);
    assert_eq!(outcome.repair_item_count_cost, 0);
    assert!(!outcome.only_renaming);
    assert_eq!(repair_cost(&outcome.result), Some(1));
    assert_eq!(
        outcome.result.get_component("minecraft:lore"),
        Some(&ComponentValue::Lore(vec![Tag::String("A blade".into())]))
    );
    assert_eq!(
        outcome.result.get_component("minecraft:custom_data"),
        Some(&ComponentValue::CustomData(Tag::Compound(vec![(
            "owner".into(),
            Tag::String("steve".into())
        )])))
    );
    assert_eq!(
        outcome.result.get_component("minecraft:rarity"),
        Some(&ComponentValue::Rarity(Rarity::Epic))
    );
    assert!(outcome.can_take(9, false));
    assert!(!outcome.can_take(8, false));
    assert!(outcome.can_take(0, true));
    assert_eq!(left, left_before);
    assert_eq!(right, right_before);
}

#[test]
fn enchanted_book_plus_sword_uses_stored_enchantments_and_half_cost() {
    let mut left = sword(0, 1561);
    left.set_component(ComponentValue::Enchantments(map(&[(
        "minecraft:sharpness",
        2,
    )])));
    let right = enchanted_book(map(&[("minecraft:sharpness", 2), ("minecraft:mending", 1)]));

    let outcome = combine_in_anvil(&left, &right).unwrap();

    assert_eq!(outcome.result.item, "minecraft:diamond_sword");
    assert_eq!(
        enchantments(&outcome.result, "minecraft:enchantments"),
        map(&[("minecraft:mending", 1), ("minecraft:sharpness", 3),])
    );
    assert!(
        outcome
            .result
            .get_component("minecraft:stored_enchantments")
            .is_none()
    );
    // Mending: (4 / 2) * 1 = 2. Sharpness: max(1, 1 / 2) * 3 = 3.
    assert_eq!(outcome.cost, 5);
    assert_eq!(repair_cost(&outcome.result), Some(1));
}

#[test]
fn enchanted_book_plus_enchanted_book_merges_stored_enchantments() {
    let left = enchanted_book(map(&[("minecraft:sharpness", 3)]));
    let right = enchanted_book(map(&[("minecraft:sharpness", 3)]));

    let outcome = combine_in_anvil(&left, &right).unwrap();

    assert_eq!(outcome.result.item, "minecraft:enchanted_book");
    assert_eq!(
        enchantments(&outcome.result, "minecraft:stored_enchantments"),
        map(&[("minecraft:sharpness", 4)])
    );
    assert!(
        outcome
            .result
            .get_component("minecraft:enchantments")
            .is_none()
    );
    // max(1, sharpness anvil cost 1 / 2) * 4.
    assert_eq!(outcome.cost, 4);
}

#[test]
fn rename_only_costs_one_level_and_keeps_the_right_slot() {
    let left = sword(0, 1561);
    let right = ItemStack::empty();
    let options = AnvilOptions::survival().with_rename("Excalibur");

    let outcome = combine_in_anvil_with(&left, &right, &options).unwrap();

    assert_eq!(outcome.cost, 1);
    assert!(outcome.only_renaming);
    assert_eq!(outcome.repair_item_count_cost, 0);
    assert_eq!(
        outcome.result.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("Excalibur".into())))
    );
    // Rename-only results do not accumulate prior work.
    assert_eq!(repair_cost(&outcome.result), None);
}

#[test]
fn blank_rename_clears_an_existing_custom_name() {
    let mut left = sword(0, 1561);
    left.set_component(ComponentValue::CustomName(Tag::String("Old name".into())));
    let options = AnvilOptions::survival().with_rename("");

    let outcome = combine_in_anvil_with(&left, &ItemStack::empty(), &options).unwrap();

    assert_eq!(outcome.cost, 1);
    assert!(outcome.only_renaming);
    assert!(
        outcome
            .result
            .get_component("minecraft:custom_name")
            .is_none()
    );
}

#[test]
fn material_repair_consumes_units_and_returns_the_used_count() {
    let left = bare_sword(1000);
    let mut right = ItemStack::new("minecraft:diamond", 3);

    let outcome = combine_in_anvil(&left, &right).unwrap();

    // The prototype max damage is 1561, so each diamond repairs 390.
    assert_eq!(outcome.result.damage(), 0);
    assert_eq!(outcome.repair_item_count_cost, 3);
    assert_eq!(outcome.cost, 3);
    assert!(!outcome.only_renaming);

    // A single material unit leaves the remaining damage in place.
    right.count = 1;
    let outcome = combine_in_anvil(&left, &right).unwrap();
    assert_eq!(outcome.result.damage(), 1000 - 1561 / 4);
    assert_eq!(outcome.repair_item_count_cost, 1);
    assert_eq!(outcome.cost, 1);
}

#[test]
fn repairing_a_full_durability_item_is_rejected() {
    let left = sword(0, 1561);
    let right = ItemStack::new("minecraft:diamond", 1);

    assert_eq!(
        combine_in_anvil(&left, &right),
        Err(AnvilError::FullDurability {
            item: "minecraft:diamond_sword".into()
        })
    );
}

#[test]
fn prior_work_penalty_grows_as_two_to_the_n_minus_one() {
    assert_eq!(calculate_increased_repair_cost(0), 1);
    assert_eq!(calculate_increased_repair_cost(1), 3);
    assert_eq!(calculate_increased_repair_cost(3), 7);
    assert_eq!(calculate_increased_repair_cost(7), 15);
    assert_eq!(calculate_increased_repair_cost(i32::MAX), i32::MAX);

    let mut left = sword(100, 1000);
    left.set_component(ComponentValue::RepairCost(5));
    let mut right = sword(200, 1000);
    right.set_component(ComponentValue::RepairCost(0));

    let outcome = combine_in_anvil(&left, &right).unwrap();

    // Cost = 5 (left prior work) + 0 (right) + 2 (durability merge).
    assert_eq!(outcome.cost, 7);
    // Output repair cost = max(5, 0), then doubled + 1.
    assert_eq!(repair_cost(&outcome.result), Some(11));
}

#[test]
fn too_expensive_at_forty_and_creative_bypass() {
    let mut left = sword(100, 1000);
    left.set_component(ComponentValue::RepairCost(38));
    let right = sword(100, 1000);

    assert_eq!(
        combine_in_anvil(&left, &right),
        Err(AnvilError::TooExpensive { cost: 40 })
    );

    let creative = AnvilOptions::creative();
    let outcome = combine_in_anvil_with(&left, &right, &creative).unwrap();
    assert_eq!(outcome.cost, 40);
    assert!(!outcome.result.is_empty());

    // 39 is still allowed in survival.
    left.set_component(ComponentValue::RepairCost(37));
    let outcome = combine_in_anvil(&left, &right).unwrap();
    assert_eq!(outcome.cost, 39);
}

#[test]
fn rename_only_is_clamped_below_the_hard_limit_with_prior_work() {
    let mut left = sword(0, 1561);
    left.set_component(ComponentValue::RepairCost(39));
    let options = AnvilOptions::survival().with_rename("Still renameable");

    let outcome = combine_in_anvil_with(&left, &ItemStack::empty(), &options).unwrap();

    assert_eq!(outcome.cost, ANVIL_MAXIMUM_REPAIR_COST - 1);
    assert!(outcome.only_renaming);
    // Rename-only keeps the existing repair cost instead of increasing it.
    assert_eq!(repair_cost(&outcome.result), Some(39));
}

#[test]
fn max_levels_are_capped_and_exclusive_books_are_rejected() {
    let mut left = sword(0, 1561);
    left.set_component(ComponentValue::Enchantments(map(&[(
        "minecraft:sharpness",
        5,
    )])));
    let right = enchanted_book(map(&[("minecraft:sharpness", 5)]));

    let outcome = combine_in_anvil(&left, &right).unwrap();
    assert_eq!(
        enchantments(&outcome.result, "minecraft:enchantments"),
        map(&[("minecraft:sharpness", 5)])
    );
    // Book half-cost: max(1, 1 / 2) * 5.
    assert_eq!(outcome.cost, 5);

    // Smite shares `exclusive_set/damage` with sharpness, so a book containing
    // only smite is rejected.
    let smite = enchanted_book(map(&[("minecraft:smite", 5)]));
    match combine_in_anvil(&left, &smite) {
        Err(AnvilError::IncompatibleEnchantments { first, second }) => {
            assert_eq!(first, "minecraft:smite");
            assert_eq!(second, "minecraft:sharpness");
        }
        other => panic!("unexpected result: {other:?}"),
    }

    // A compatible entry in the same book still applies; only the conflicting
    // entry is skipped and charged one extra level.
    let mixed = enchanted_book(map(&[("minecraft:mending", 1), ("minecraft:smite", 5)]));
    let outcome = combine_in_anvil(&left, &mixed).unwrap();
    assert_eq!(
        enchantments(&outcome.result, "minecraft:enchantments"),
        map(&[("minecraft:mending", 1), ("minecraft:sharpness", 5),])
    );
    // Mending 2 + skipped smite 1.
    assert_eq!(outcome.cost, 3);
}

#[test]
fn bypass_enchantment_level_restriction_allows_over_max_levels() {
    let mut left = sword(0, 1561);
    left.set_component(ComponentValue::Enchantments(map(&[(
        "minecraft:sharpness",
        5,
    )])));
    let right = enchanted_book(map(&[("minecraft:sharpness", 5)]));
    let mut options = AnvilOptions::survival();
    options.bypass_enchantment_level_restriction = true;

    let outcome = combine_in_anvil_with(&left, &right, &options).unwrap();
    assert_eq!(
        enchantments(&outcome.result, "minecraft:enchantments"),
        map(&[("minecraft:sharpness", 6)])
    );
}

#[test]
fn components_are_preserved_and_output_is_deterministic() {
    let mut left = sword(100, 1561);
    left.set_component(ComponentValue::CustomName(Tag::String("Old".into())));
    left.set_component(ComponentValue::ItemName(Tag::String("Blade".into())));
    left.set_component(ComponentValue::Lore(vec![Tag::String("line".into())]));
    left.set_component(ComponentValue::CustomData(Tag::Compound(vec![(
        "key".into(),
        Tag::Int(7),
    )])));
    left.set_component(ComponentValue::Rarity(Rarity::Rare));
    left.set_component(ComponentValue::RepairCost(2));
    let right = enchanted_book(map(&[("minecraft:sharpness", 1)]));
    let options = AnvilOptions::survival().with_rename("New");

    let first = combine_in_anvil_with(&left, &right, &options).unwrap();
    let second = combine_in_anvil_with(&left, &right, &options).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.result.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("New".into())))
    );
    assert_eq!(
        first.result.get_component("minecraft:item_name"),
        Some(&ComponentValue::ItemName(Tag::String("Blade".into())))
    );
    assert_eq!(
        first.result.get_component("minecraft:lore"),
        Some(&ComponentValue::Lore(vec![Tag::String("line".into())]))
    );
    assert_eq!(
        first.result.get_component("minecraft:custom_data"),
        Some(&ComponentValue::CustomData(Tag::Compound(vec![(
            "key".into(),
            Tag::Int(7)
        )])))
    );
    assert_eq!(
        first.result.get_component("minecraft:rarity"),
        Some(&ComponentValue::Rarity(Rarity::Rare))
    );
    assert_eq!(
        enchantments(&first.result, "minecraft:enchantments"),
        map(&[("minecraft:sharpness", 1)])
    );
    // 2 prior work + 1 sharpness + 1 rename.
    assert_eq!(first.cost, 4);
    assert_eq!(repair_cost(&first.result), Some(5));
}

#[test]
fn validation_rejects_incompatible_and_empty_inputs() {
    let sword = sword(0, 1561);

    assert_eq!(
        combine_in_anvil(&ItemStack::empty(), &sword),
        Err(AnvilError::EmptyInput)
    );
    assert_eq!(
        combine_in_anvil(&sword, &ItemStack::empty()),
        Err(AnvilError::MissingSecondSlot)
    );
    assert!(matches!(
        combine_in_anvil(&sword, &ItemStack::new("minecraft:stick", 1)),
        Err(AnvilError::IncompatibleItems { .. })
    ));
    // A sword cannot be pushed into an enchanted book.
    assert!(matches!(
        combine_in_anvil(&enchanted_book(map(&[("minecraft:mending", 1)])), &sword),
        Err(AnvilError::IncompatibleItems { .. })
    ));
    assert!(matches!(
        combine_in_anvil(
            &ItemStack::new("minecraft:book", 1),
            &enchanted_book(map(&[("minecraft:sharpness", 1)]))
        ),
        Err(AnvilError::IncompatibleEnchantments { .. })
    ));

    // Nothing to do: two undamaged swords with no enchantments.
    assert_eq!(combine_in_anvil(&sword, &sword), Err(AnvilError::NoChange));

    let long_name = "x".repeat(51);
    let options = AnvilOptions::survival().with_rename(long_name);
    assert_eq!(
        combine_in_anvil_with(&sword, &ItemStack::empty(), &options),
        Err(AnvilError::NameTooLong { length: 51 })
    );

    let mut removed = sword.clone();
    removed.mark_component_removed("minecraft:enchantments");
    assert!(matches!(
        combine_in_anvil(&removed, &enchanted_book(map(&[("minecraft:mending", 1)]))),
        Err(AnvilError::CannotStoreEnchantments { .. })
    ));
}

#[test]
fn definition_data_matches_the_costs_used_by_the_anvil() {
    assert_eq!(definition("minecraft:sharpness").unwrap().anvil_cost, 1);
    assert_eq!(definition("minecraft:mending").unwrap().anvil_cost, 4);
    assert_eq!(definition("minecraft:unbreaking").unwrap().anvil_cost, 2);
    assert_eq!(definition("minecraft:sharpness").unwrap().max_level, 5);
}
