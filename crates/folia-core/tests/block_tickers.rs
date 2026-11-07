//! Live block-entity ticking: furnaces run through `tick_world`.

use folia_core::anvil::Compression;
use folia_core::block_entity::{BlockEntity, BlockEntityPos, FurnaceBlockEntity};
use folia_core::chunk::Chunk;
use folia_core::chunk_packet::flat_chunk;
use folia_core::crafting::{
    FURNACE_FUEL_SLOT, FURNACE_INPUT_SLOT, FURNACE_RESULT_SLOT, FurnaceKind,
};
use folia_core::item_components::ItemStack;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-block-tickers-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage_with_furnace(name: &str) -> (WorldStorage, BlockEntityPos) {
    let root = temp_dir(name);
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();

    let pos = BlockEntityPos::new(0, -60, 0);
    let mut furnace = FurnaceBlockEntity::new(pos, FurnaceKind::Furnace);
    furnace.set_slot(FURNACE_INPUT_SLOT, ItemStack::new("minecraft:iron_ore", 1));
    furnace.set_slot(FURNACE_FUEL_SLOT, ItemStack::new("minecraft:coal", 1));
    storage
        .insert_block_entity(BlockEntity::Furnace(furnace))
        .unwrap();
    storage
        .save_chunk_with_block_entities(&chunk, Compression::Zlib)
        .unwrap();
    (storage, pos)
}

fn furnace(storage: &mut WorldStorage, pos: BlockEntityPos) -> &FurnaceBlockEntity {
    match storage.block_entity(pos).unwrap().expect("furnace exists") {
        BlockEntity::Furnace(furnace) => furnace,
        other => panic!("expected furnace, got {other:?}"),
    }
}

#[test]
fn furnace_cooks_and_consumes_fuel_during_world_ticks() {
    let (mut storage, pos) = storage_with_furnace("cook");
    for tick_number in 1..=220 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    let furnace = furnace(&mut storage, pos);
    assert!(
        !furnace.slot(FURNACE_RESULT_SLOT).unwrap().is_empty(),
        "furnace produced no result"
    );
    assert_eq!(
        furnace.slot(FURNACE_RESULT_SLOT).unwrap().item,
        "minecraft:iron_ingot"
    );
    assert!(furnace.slot(FURNACE_INPUT_SLOT).unwrap().is_empty());
}

#[test]
fn furnace_does_not_cook_without_fuel() {
    let root = temp_dir("no-fuel");
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    let pos = BlockEntityPos::new(0, -60, 0);
    let mut furnace_entity = FurnaceBlockEntity::new(pos, FurnaceKind::Furnace);
    furnace_entity.set_slot(FURNACE_INPUT_SLOT, ItemStack::new("minecraft:iron_ore", 1));
    storage
        .insert_block_entity(BlockEntity::Furnace(furnace_entity))
        .unwrap();

    for tick_number in 1..=220 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    let furnace = furnace(&mut storage, pos);
    assert_eq!(
        furnace.slot(FURNACE_RESULT_SLOT).unwrap().item,
        "",
        "furnace cooked without fuel"
    );
}
