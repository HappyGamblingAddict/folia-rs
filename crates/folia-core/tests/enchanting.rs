//! Focused tests for enchanting-table offers and application.
//!
//! The offer/list assertions follow the two-phase seed order in
//! `EnchantmentMenu.slotsChanged`; application assertions pin the vanilla
//! click requirements (lapis + experience levels), the `slot + 1` debit, the
//! `minecraft:enchantments` merge rules, and component preservation.

use folia_core::item_components::enchanting::{
    EnchantError, EnchantingTable, MAX_OFFER_COST, MIN_OFFER_COST, OFFER_COUNT, apply_enchantments,
    available_enchantment_results, definition, enchantment_list, generate_offers,
    validate_registry_ids,
};
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::nbt::Tag;
use std::collections::{BTreeMap, BTreeSet};

fn sword() -> ItemStack {
    ItemStack::new("minecraft:diamond_sword", 1)
}

fn book() -> ItemStack {
    ItemStack::new("minecraft:book", 1)
}

fn instance(
    name: &str,
    level: i32,
) -> folia_core::item_components::enchanting::EnchantmentInstance {
    folia_core::item_components::enchanting::EnchantmentInstance::new(
        definition(name).unwrap(),
        level,
    )
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

#[test]
fn definition_table_matches_the_registry_and_tags() {
    validate_registry_ids().unwrap();
    assert_eq!(definition("sharpness").unwrap().id, 33);
    assert!(definition("minecraft:mending").unwrap().treasure);
    assert!(!definition("minecraft:sharpness").unwrap().treasure);
    assert!(
        definition("minecraft:protection")
            .unwrap()
            .exclusive_set
            .is_some()
    );
}

#[test]
fn offers_are_deterministic_for_a_seed() {
    let stack = sword();
    let first = generate_offers(&stack, 15, 12_345);
    let second = generate_offers(&stack, 15, 12_345);
    assert_eq!(first, second);
    assert_eq!(first.len(), OFFER_COUNT);
    for offer in first.iter() {
        assert!(offer.slot < OFFER_COUNT);
        assert!(offer.cost > offer.slot as i32);
        assert!(offer.cost <= MAX_OFFER_COST);
        let hint = offer.hint.expect("a sword offer should have a clue");
        assert!(hint.level >= 1);
        assert!(hint.level <= hint.enchantment.max_level);
    }
    assert_eq!(first.costs(), second.costs());

    let different = generate_offers(&stack, 15, 54_321);
    assert_ne!(first, different, "different seeds should change offers");
}

#[test]
fn power_changes_offer_costs_and_levels() {
    let stack = sword();
    let low = generate_offers(&stack, 0, 7);
    let high = generate_offers(&stack, 15, 7);
    assert_eq!(low.len(), OFFER_COUNT);
    assert_eq!(high.len(), OFFER_COUNT);

    let low_total: i32 = low.iter().map(|offer| offer.cost).sum();
    let high_total: i32 = high.iter().map(|offer| offer.cost).sum();
    assert!(
        high_total > low_total,
        "power 15 ({high_total}) should cost more than power 0 ({low_total})"
    );
    let low_levels: i32 = low
        .iter()
        .filter_map(|offer| offer.hint)
        .map(|hint| hint.level)
        .sum();
    let high_levels: i32 = high
        .iter()
        .filter_map(|offer| offer.hint)
        .map(|hint| hint.level)
        .sum();
    assert!(
        high_levels > low_levels,
        "power 15 should hint higher levels ({high_levels}) than power 0 ({low_levels})"
    );
    assert!(high.costs()[2] >= 30, "the top slot reaches cost 30");
}

#[test]
fn costs_stay_in_the_vanilla_one_to_thirty_range() {
    for seed in [0, 1, 2, 3, 7, 42, 9_999, -1, i32::MAX] {
        for power in [0, 1, 5, 10, 15, 30] {
            let offers = generate_offers(&sword(), power, seed);
            for offer in offers.iter() {
                assert!(
                    (offer.slot as i32 + 1..=MAX_OFFER_COST).contains(&offer.cost),
                    "slot {} cost {} is outside the vanilla range",
                    offer.slot,
                    offer.cost
                );
                assert!(offer.cost >= MIN_OFFER_COST);
            }
        }
    }
}

#[test]
fn sword_and_book_use_different_applicability_pools() {
    let sword_results = available_enchantment_results(30, &sword());
    let book_results = available_enchantment_results(30, &book());
    let sword_names: BTreeSet<&str> = sword_results
        .iter()
        .map(|instance| instance.enchantment.name)
        .collect();
    let book_names: BTreeSet<&str> = book_results
        .iter()
        .map(|instance| instance.enchantment.name)
        .collect();

    assert!(sword_names.contains("minecraft:sharpness"));
    assert!(sword_names.contains("minecraft:unbreaking"));
    assert!(!sword_names.contains("minecraft:power"));
    assert!(!sword_names.contains("minecraft:protection"));

    assert!(book_names.contains("minecraft:sharpness"));
    assert!(book_names.contains("minecraft:power"));
    assert!(book_names.contains("minecraft:protection"));
    assert!(book_names.len() > sword_names.len());

    let stone = ItemStack::new("minecraft:stone", 1);
    assert!(generate_offers(&stone, 15, 7).is_empty());
    assert!(available_enchantment_results(30, &stone).is_empty());
}

#[test]
fn click_consumes_lapis_and_debits_slot_plus_one_levels() {
    let mut stack = sword();
    stack.set_component(ComponentValue::CustomName(Tag::String("Excalibur".into())));
    stack.set_component(ComponentValue::Damage(12));
    let seed = 12_345;
    let offers = generate_offers(&stack, 15, seed);
    let offer = *offers
        .iter()
        .find(|offer| offer.hint.is_some())
        .expect("at least one real offer");
    let expected = enchantment_list(&stack, offer.slot, offer.cost, seed);
    assert!(!expected.is_empty());

    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::new("minecraft:lapis_lazuli", 5);
    let mut xp = 30;
    let outcome = table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, false, 77)
        .unwrap();

    let levels = offer.slot as i32 + 1;
    assert_eq!(outcome.levels_spent, levels);
    assert_eq!(outcome.lapis_spent, levels);
    assert_eq!(xp, 30 - levels);
    assert_eq!(lapis.count, 5 - levels);
    assert_eq!(outcome.application.applied, expected);
    assert_eq!(
        enchantments(&stack, "minecraft:enchantments"),
        outcome.application.enchantments
    );
    assert_eq!(
        stack.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("Excalibur".into())))
    );
    assert_eq!(
        stack.get_component("minecraft:damage"),
        Some(&ComponentValue::Damage(12))
    );
    assert_eq!(table.seed(), 77);
    assert_eq!(table.offers().len(), OFFER_COUNT);
}

#[test]
fn lapis_is_empty_after_the_exact_required_count() {
    let mut stack = sword();
    let seed = 7;
    let offer = generate_offers(&stack, 15, seed)
        .iter()
        .next()
        .copied()
        .unwrap();
    let levels = offer.slot as i32 + 1;
    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::new("minecraft:lapis_lazuli", levels);
    let mut xp = 30;
    table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, false, 1)
        .unwrap();
    assert!(lapis.is_empty());
}

#[test]
fn click_rejects_insufficient_levels_without_mutating_anything() {
    let mut stack = sword();
    let seed = 12_345;
    let offers = generate_offers(&stack, 15, seed);
    let offer = offers
        .iter()
        .find(|offer| offer.cost > offer.slot as i32 + 1)
        .copied()
        .expect("an offer whose cost exceeds the level debit");
    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::new("minecraft:lapis_lazuli", 5);
    let mut xp = offer.cost - 1;
    let original_stack = stack.clone();
    let original_lapis = lapis.clone();
    let original_xp = xp;

    let error = table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, false, 2)
        .unwrap_err();
    assert!(matches!(
        error,
        EnchantError::NotEnoughExperience {
            required_levels,
            required_cost,
            ..
        } if required_levels == offer.slot as i32 + 1 && required_cost == offer.cost
    ));
    assert_eq!(stack, original_stack);
    assert_eq!(lapis, original_lapis);
    assert_eq!(xp, original_xp);
}

#[test]
fn click_rejects_insufficient_lapis_without_mutating_anything() {
    let mut stack = sword();
    let seed = 12_345;
    let offer = generate_offers(&stack, 15, seed)
        .iter()
        .next()
        .copied()
        .unwrap();
    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::new("minecraft:lapis_lazuli", offer.slot as i32);
    let mut xp = 30;
    let original_stack = stack.clone();
    let original_xp = xp;

    let error = table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, false, 2)
        .unwrap_err();
    assert!(matches!(error, EnchantError::NotEnoughLapis { .. }));
    assert_eq!(stack, original_stack);
    assert_eq!(xp, original_xp);
}

#[test]
fn creative_clicks_skip_lapis_but_still_debit_levels_like_vanilla() {
    let mut stack = sword();
    let seed = 42;
    let offer = generate_offers(&stack, 15, seed)
        .iter()
        .find(|offer| offer.hint.is_some())
        .copied()
        .unwrap();
    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::empty();
    let mut xp = 5;
    let outcome = table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, true, 9)
        .unwrap();
    let levels = offer.slot as i32 + 1;
    assert_eq!(outcome.levels_spent, levels);
    assert_eq!(outcome.lapis_spent, 0);
    assert!(lapis.is_empty());
    assert_eq!(xp, 5 - levels);
    assert!(!outcome.application.enchantments.is_empty());

    // A creative player with no levels is still allowed to click.
    let mut stack = sword();
    let mut table = EnchantingTable::open(&stack, 15, seed);
    let mut lapis = ItemStack::empty();
    let mut xp = 0;
    let outcome = table
        .enchant(offer.slot, &mut stack, &mut lapis, &mut xp, true, 9)
        .unwrap();
    assert_eq!(outcome.levels_spent, 0);
    assert_eq!(xp, 0);
}

#[test]
fn merge_caps_at_max_level_and_supports_multiple_enchantments() {
    let mut stack = sword();
    let sharpness = definition("minecraft:sharpness").unwrap();
    stack.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        sharpness.name.to_owned(),
        5,
    )])));

    let application = apply_enchantments(
        &mut stack,
        &[
            instance("minecraft:sharpness", 5),
            instance("minecraft:unbreaking", 2),
        ],
    )
    .unwrap();
    assert_eq!(
        application.enchantments.get("minecraft:sharpness"),
        Some(&5)
    );
    assert_eq!(
        application.enchantments.get("minecraft:unbreaking"),
        Some(&2)
    );
    assert_eq!(
        enchantments(&stack, "minecraft:enchantments").get("minecraft:sharpness"),
        Some(&5)
    );

    // An over-maximum instance is clamped to the definition maximum.
    let application =
        apply_enchantments(&mut stack, &[instance("minecraft:sharpness", 9)]).unwrap();
    assert_eq!(
        application.enchantments.get("minecraft:sharpness"),
        Some(&sharpness.max_level)
    );
}

#[test]
fn incompatible_enchantments_are_rejected_without_changes() {
    let mut stack = sword();
    stack.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:sharpness".to_owned(),
        3,
    )])));
    let original = stack.clone();

    let error = apply_enchantments(&mut stack, &[instance("minecraft:smite", 3)]).unwrap_err();
    assert!(matches!(
        error,
        EnchantError::Incompatible {
            first: "minecraft:smite",
            ..
        }
    ));
    assert_eq!(stack, original);

    // Two rolled enchantments in one call are checked too.
    let mut fresh = sword();
    let error = apply_enchantments(
        &mut fresh,
        &[
            instance("minecraft:sharpness", 3),
            instance("minecraft:smite", 3),
        ],
    )
    .unwrap_err();
    assert!(matches!(error, EnchantError::Incompatible { .. }));
    assert!(fresh.components.is_empty());
}

#[test]
fn apply_preserves_unrelated_components() {
    let mut stack = sword();
    stack.set_component(ComponentValue::CustomName(Tag::String("Keeping".into())));
    stack.set_component(ComponentValue::Damage(77));
    stack.set_component(ComponentValue::Lore(vec![Tag::String("line".into())]));
    stack.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:unbreaking".to_owned(),
        2,
    )])));

    apply_enchantments(&mut stack, &[instance("minecraft:sharpness", 3)]).unwrap();

    assert_eq!(
        stack.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("Keeping".into())))
    );
    assert_eq!(
        stack.get_component("minecraft:damage"),
        Some(&ComponentValue::Damage(77))
    );
    assert_eq!(
        stack.get_component("minecraft:lore"),
        Some(&ComponentValue::Lore(vec![Tag::String("line".into())]))
    );
    let map = enchantments(&stack, "minecraft:enchantments");
    assert_eq!(map.get("minecraft:sharpness"), Some(&3));
    assert_eq!(map.get("minecraft:unbreaking"), Some(&2));
}

#[test]
fn books_become_enchanted_books_and_use_stored_enchantments() {
    let mut stack = book();
    stack.set_component(ComponentValue::CustomName(Tag::String("Tome".into())));
    let application = apply_enchantments(
        &mut stack,
        &[
            instance("minecraft:sharpness", 3),
            instance("minecraft:mending", 1),
        ],
    )
    .unwrap();

    assert_eq!(stack.item, "minecraft:enchanted_book");
    assert!(application.became_enchanted_book);
    assert_eq!(application.component, "minecraft:stored_enchantments");
    assert!(stack.get_component("minecraft:enchantments").is_none());
    assert_eq!(
        enchantments(&stack, "minecraft:stored_enchantments"),
        application.enchantments
    );
    assert_eq!(
        stack.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("Tome".into())))
    );

    // A sword uses the regular component instead.
    let mut sword = sword();
    let application =
        apply_enchantments(&mut sword, &[instance("minecraft:sharpness", 1)]).unwrap();
    assert_eq!(application.component, "minecraft:enchantments");
    assert!(
        sword
            .get_component("minecraft:stored_enchantments")
            .is_none()
    );
}

#[test]
fn anvil_combine_rejects_a_plain_book_and_sword() {
    let result = folia_core::item_components::enchanting::combine_in_anvil(&sword(), &book());
    assert!(matches!(
        result,
        Err(folia_core::item_components::enchanting::AnvilError::IncompatibleItems { .. })
    ));
}
