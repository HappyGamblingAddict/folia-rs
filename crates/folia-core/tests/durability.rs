//! Unbreaking durability rolls.

use folia_core::block_breaking::{apply_block_durability, apply_block_durability_with_rng};
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_random::XoroshiroRandomSource;
use std::collections::BTreeMap;

fn enchanted_pickaxe(unbreaking: i32) -> ItemStack {
    let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
    let mut map = BTreeMap::new();
    map.insert("minecraft:unbreaking".to_owned(), unbreaking);
    pick.set_component(ComponentValue::Enchantments(map));
    pick
}

#[test]
fn unbreaking_skips_some_durability_losses() {
    let mut worn = 0;
    for seed in 0..100 {
        let mut pick = enchanted_pickaxe(3);
        let mut random = XoroshiroRandomSource::new(seed);
        let wear = apply_block_durability_with_rng(&mut pick, false, 1.5, &mut random);
        if wear.changed() {
            worn += 1;
        }
    }
    assert!(worn > 10, "unbreaking skipped almost everything: {worn}");
    assert!(worn < 60, "unbreaking skipped too little: {worn}");
}

#[test]
fn an_unenchanted_tool_always_wears() {
    let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
    let wear =
        apply_block_durability_with_rng(&mut pick, false, 1.5, &mut XoroshiroRandomSource::new(0));
    assert!(wear.changed());
    assert_eq!(wear.damage, 1);
}

#[test]
fn unbreakable_and_creative_tools_never_wear() {
    let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
    pick.set_component(ComponentValue::Unbreakable);
    assert!(
        !apply_block_durability(&mut pick, false, 1.5).changed(),
        "unbreakable tool took damage"
    );
    let mut plain = ItemStack::new("minecraft:diamond_pickaxe", 1);
    assert!(!apply_block_durability(&mut plain, true, 1.5).changed());
}

#[test]
fn a_tool_breaks_at_its_durability_limit() {
    let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
    let mut broke = false;
    for _ in 0..2000 {
        let wear = apply_block_durability(&mut pick, false, 1.5);
        if wear.broke {
            broke = true;
            break;
        }
    }
    assert!(broke, "diamond pickaxe never broke");
    assert!(pick.is_empty());
}
