//! Chest loot-table rolling.

use folia_core::block_entity::{BlockEntityPos, ChestBlockEntity};
use folia_core::item_components::ItemStack;
use folia_core::loot::LootCatalog;
use folia_random::XoroshiroRandomSource;

fn chest_with_table(id: &str) -> ChestBlockEntity {
    let mut chest = ChestBlockEntity::new(BlockEntityPos::new(0, 64, 0));
    chest.set_loot_table(Some(id.to_owned()), 12345);
    chest
}

#[test]
fn chest_fills_from_a_shipped_table_and_clears_the_table_id() {
    let catalog = LootCatalog::global();
    let mut chest = chest_with_table("minecraft:chests/simple_dungeon");
    let mut random = XoroshiroRandomSource::new(7);
    assert!(chest.fill_loot(catalog, &mut random));
    assert!(
        chest.items.iter().any(|stack| !stack.is_empty()),
        "loot table rolled no items"
    );
    assert_eq!(chest.loot_table, None);
    assert_eq!(chest.loot_table_seed, 0);
}

#[test]
fn chest_loot_is_deterministic_for_a_seed() {
    let catalog = LootCatalog::global();
    let mut first = chest_with_table("minecraft:chests/simple_dungeon");
    let mut second = chest_with_table("minecraft:chests/simple_dungeon");
    first.fill_loot(catalog, &mut XoroshiroRandomSource::new(42));
    second.fill_loot(catalog, &mut XoroshiroRandomSource::new(42));
    assert_eq!(first.items, second.items);
}

#[test]
fn filling_is_idempotent_after_the_table_clears() {
    let catalog = LootCatalog::global();
    let mut chest = chest_with_table("minecraft:chests/simple_dungeon");
    let mut random = XoroshiroRandomSource::new(3);
    assert!(chest.fill_loot(catalog, &mut random));
    let snapshot = chest.items.clone();
    assert!(!chest.fill_loot(catalog, &mut random));
    assert_eq!(chest.items, snapshot);
}

#[test]
fn unknown_or_missing_tables_do_not_modify_the_chest() {
    let catalog = LootCatalog::global();
    let mut chest = chest_with_table("minecraft:chests/does_not_exist");
    assert!(!chest.fill_loot(catalog, &mut XoroshiroRandomSource::new(1)));
    assert!(chest.items.iter().all(ItemStack::is_empty));
    assert_eq!(
        chest.loot_table.as_deref(),
        Some("minecraft:chests/does_not_exist")
    );

    let mut empty = ChestBlockEntity::new(BlockEntityPos::new(0, 64, 0));
    assert!(!empty.fill_loot(catalog, &mut XoroshiroRandomSource::new(1)));
}

#[test]
fn existing_items_are_kept_when_loot_fills_the_rest() {
    let catalog = LootCatalog::global();
    let mut chest = chest_with_table("minecraft:chests/simple_dungeon");
    chest.set_slot(0, ItemStack::new("minecraft:stone", 5));
    assert!(chest.fill_loot(catalog, &mut XoroshiroRandomSource::new(9)));
    assert_eq!(chest.slot(0).unwrap().item, "minecraft:stone");
    assert_eq!(chest.slot(0).unwrap().count, 5);
}
