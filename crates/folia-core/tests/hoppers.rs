//! Hopper persistence and slot-transfer semantics.

use folia_core::anvil::Compression;
use folia_core::block_entity::{
    BlockEntity, BlockEntityMap, BlockEntityPos, ChestBlockEntity, HopperBlockEntity,
    decode_block_entities, move_one_item, store_block_entities,
};
use folia_core::chunk::{Chunk, encode_chunk_with_block_entities};
use folia_core::chunk_packet::flat_chunk;
use folia_core::item_components::{ComponentMap, ComponentValue, ItemStack};
use folia_core::nbt::Tag;
use folia_core::tick;
use folia_core::world::WorldStorage;

fn pos() -> BlockEntityPos {
    BlockEntityPos::new(1, -60, 2)
}

fn chunk_at() -> Chunk {
    let mut chunk = Chunk {
        x: 0,
        z: 0,
        sections: Vec::new(),
    };
    chunk.set_block_state(
        1,
        -60,
        2,
        folia_core::chunk::BlockState {
            name: "minecraft:hopper".into(),
            properties: vec![],
        },
    );
    chunk
}

#[test]
fn hopper_items_and_cooldown_round_trip_through_chunk_nbt() {
    let mut hopper = HopperBlockEntity::new(pos());
    hopper.set_slot(0, ItemStack::new("minecraft:iron_ingot", 7));
    hopper.transfer_cooldown = 5;
    let chunk = chunk_at();
    let mut entities = BlockEntityMap::new();
    entities.insert(pos(), BlockEntity::Hopper(hopper.clone()));
    let mut root = encode_chunk_with_block_entities(&chunk, &entities).unwrap();
    let decoded = decode_block_entities(&root).unwrap();
    let Some(BlockEntity::Hopper(decoded)) = decoded.get(&pos()) else {
        panic!("hopper missing");
    };
    assert_eq!(decoded.block_id, "minecraft:hopper");
    assert_eq!(decoded.slot(0).unwrap().item, "minecraft:iron_ingot");
    assert_eq!(decoded.slot(0).unwrap().count, 7);
    assert_eq!(decoded.transfer_cooldown, 5);

    // `store_block_entities` writes the chunk-list shape as well.
    store_block_entities(&mut root, &entities).unwrap();
    let _ = &mut root;
}

#[test]
fn transfer_merges_into_an_existing_stack_first() {
    let mut source = vec![ItemStack::new("minecraft:iron_ingot", 2)];
    let mut target = vec![
        ItemStack::new("minecraft:iron_ingot", 5),
        ItemStack::empty(),
    ];
    assert!(move_one_item(&mut source, &mut target));
    assert_eq!(source[0].count, 1);
    assert_eq!(target[0].count, 6);
    assert!(target[1].is_empty());
}

#[test]
fn transfer_uses_an_empty_slot_when_no_stack_matches() {
    let mut source = vec![ItemStack::new("minecraft:diamond", 1)];
    let mut target = vec![ItemStack::empty(), ItemStack::new("minecraft:dirt", 3)];
    assert!(move_one_item(&mut source, &mut target));
    assert_eq!(target[0].item, "minecraft:diamond");
    assert_eq!(target[0].count, 1);
    assert!(source[0].is_empty());
}

#[test]
fn transfer_rejects_a_full_target_and_preserves_components() {
    let mut source = vec![ItemStack::new("minecraft:diamond", 1)];
    let mut target = vec![ItemStack::new("minecraft:diamond", 64)];
    assert!(!move_one_item(&mut source, &mut target));
    assert_eq!(source[0].count, 1);

    let mut custom = ItemStack::new("minecraft:paper", 4);
    let mut components = ComponentMap::new();
    components.set(ComponentValue::CustomName(Tag::String("note".into())));
    custom.components = components.clone();
    let mut source = vec![custom];
    let mut target = vec![ItemStack::empty()];
    assert!(move_one_item(&mut source, &mut target));
    assert_eq!(target[0].components, components);
    assert_eq!(target[0].count, 1);
    assert_eq!(source[0].count, 3);
}

#[test]
fn hopper_tick_counts_down_the_transfer_cooldown() {
    let mut hopper = HopperBlockEntity::new(pos());
    hopper.transfer_cooldown = 2;
    assert!(!hopper.tick());
    assert!(hopper.tick());
    assert!(hopper.tick());
}

fn world_with_hopper(
    name: &str,
    above: Option<ChestBlockEntity>,
    below: Option<ChestBlockEntity>,
    hopper: HopperBlockEntity,
) -> (WorldStorage, BlockEntityPos, BlockEntityPos, BlockEntityPos) {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-hopper-world-{}-{name}",
        std::process::id()
    ));
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();

    let hopper_pos = BlockEntityPos::new(0, -60, 0);
    let above_pos = BlockEntityPos::new(0, -59, 0);
    let below_pos = BlockEntityPos::new(0, -61, 0);
    if let Some(chest) = above {
        storage
            .insert_block_entity(BlockEntity::Chest(chest))
            .unwrap();
    }
    storage
        .insert_block_entity(BlockEntity::Hopper(hopper))
        .unwrap();
    if let Some(chest) = below {
        storage
            .insert_block_entity(BlockEntity::Chest(chest))
            .unwrap();
    }
    (storage, hopper_pos, above_pos, below_pos)
}

#[test]
fn hopper_pulls_from_above_and_pushes_below_on_the_world_tick() {
    let mut above = ChestBlockEntity::new(BlockEntityPos::new(0, -59, 0));
    above.set_slot(0, ItemStack::new("minecraft:iron_ingot", 5));
    let below = ChestBlockEntity::new(BlockEntityPos::new(0, -61, 0));
    let (mut storage, hopper_pos, above_pos, below_pos) = world_with_hopper(
        "pull-push",
        Some(above),
        Some(below),
        HopperBlockEntity::new(BlockEntityPos::new(0, -60, 0)),
    );

    tick::tick_world(&mut storage, 1).unwrap();
    let Some(BlockEntity::Chest(above)) = storage.block_entity(above_pos).unwrap() else {
        panic!("above chest missing");
    };
    assert_eq!(above.slot(0).unwrap().count, 4, "one item was pulled");
    let Some(BlockEntity::Chest(below)) = storage.block_entity(below_pos).unwrap() else {
        panic!("below chest missing");
    };
    assert_eq!(below.slot(0).unwrap().item, "minecraft:iron_ingot");
    assert_eq!(below.slot(0).unwrap().count, 1, "one item was pushed");
    let Some(BlockEntity::Hopper(hopper)) = storage.block_entity(hopper_pos).unwrap() else {
        panic!("hopper missing");
    };
    assert_eq!(hopper.transfer_cooldown, 8);

    for tick_number in 2..=10 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    let Some(BlockEntity::Chest(below)) = storage.block_entity(below_pos).unwrap() else {
        panic!("below chest missing");
    };
    assert!(
        below.slot(0).unwrap().count >= 2,
        "the hopper did not transfer again after the cooldown"
    );
}

#[test]
fn hopper_keeps_an_item_when_the_target_container_is_full() {
    let mut hopper = HopperBlockEntity::new(BlockEntityPos::new(0, -60, 0));
    hopper.set_slot(0, ItemStack::new("minecraft:iron_ingot", 1));
    let mut below = ChestBlockEntity::new(BlockEntityPos::new(0, -61, 0));
    for slot in 0..27 {
        below.set_slot(slot, ItemStack::new("minecraft:stone", 64));
    }
    let (mut storage, hopper_pos, _, _) = world_with_hopper("full", None, Some(below), hopper);

    tick::tick_world(&mut storage, 1).unwrap();
    let Some(BlockEntity::Hopper(hopper)) = storage.block_entity(hopper_pos).unwrap() else {
        panic!("hopper missing");
    };
    assert_eq!(hopper.slot(0).unwrap().item, "minecraft:iron_ingot");
    assert_eq!(hopper.slot(0).unwrap().count, 1);
}
