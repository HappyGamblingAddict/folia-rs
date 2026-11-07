//! Brewing-stand block-entity persistence and ticking.

use folia_core::block_entity::{
    BlockEntity, BlockEntityMap, BlockEntityPos, BrewingStandBlockEntity, decode_block_entities,
};
use folia_core::chunk::encode_chunk_with_block_entities;
use folia_core::chunk_packet::flat_chunk;
use folia_core::crafting::{
    BREWING_FUEL_SLOT, BREWING_INGREDIENT_SLOT, potion_contents, potion_stack,
};
use folia_core::item_components::ItemStack;

fn stand() -> BrewingStandBlockEntity {
    BrewingStandBlockEntity::new(BlockEntityPos::new(0, -60, 0))
}

fn ready_stand() -> BrewingStandBlockEntity {
    let mut stand = stand();
    for slot in 0..3 {
        stand.set_slot(slot, potion_stack("minecraft:potion", "minecraft:water"));
    }
    stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:nether_wart", 1),
    );
    stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 1),
    );
    stand
}

#[test]
fn brewing_stand_round_trips_through_chunk_nbt() {
    let mut entity = ready_stand();
    entity.state.brew_time = 123;
    entity.state.fuel = 7;
    let chunk = flat_chunk(0, 0);
    let mut entities = BlockEntityMap::new();
    entities.insert(
        entity.pos,
        BlockEntity::BrewingStand(Box::new(entity.clone())),
    );
    let root = encode_chunk_with_block_entities(&chunk, &entities).unwrap();
    let decoded = decode_block_entities(&root).unwrap();
    let Some(BlockEntity::BrewingStand(decoded)) = decoded.get(&entity.pos) else {
        panic!("brewing stand missing");
    };
    assert_eq!(decoded.state.brew_time, 123);
    assert_eq!(decoded.state.fuel, 7);
    assert_eq!(decoded.slot(0).unwrap().item, "minecraft:potion");
    assert_eq!(decoded.slot(4).unwrap().item, "minecraft:blaze_powder");
}

#[test]
fn brewing_stand_entity_ticks_water_into_awkward() {
    let mut stand = ready_stand();
    for _ in 0..401 {
        stand.tick();
    }
    let contents = potion_contents(stand.slot(0).unwrap()).expect("potion contents");
    assert_eq!(contents.potion.as_deref(), Some("minecraft:awkward"));
}
