//! Item entities and XP orbs spawned from block drops.
//!
//! Covers spawner velocities/determinism, gravity arcs, magnet/attraction,
//! merge behaviour, XP splitting, pickup into an inventory (with components),
//! the 6000-tick despawn, and the Add Entity / metadata / position-sync /
//! remove packet path.

use folia_core::block_breaking::{
    BlockDrops, BlockLootContext, DigLootContext, DigStart, DigState, DigStop, MiningContext,
};
use folia_core::chunk::BlockState;
use folia_core::entity::{
    CLIENTBOUND_ADD_ENTITY, CLIENTBOUND_ENTITY_POSITION_SYNC, CLIENTBOUND_REMOVE_ENTITIES,
    FlatCollider,
};
use folia_core::entity_metadata::{MetadataValue, read_set_entity_data};
use folia_core::inventory::{ItemStack, add_stack};
use folia_core::item_components::{ComponentValue, plain_text};
use folia_core::item_entity::{
    DATA_ITEM, DATA_VALUE, DroppedEntity, ExperienceOrb, ITEM_LIFETIME, ITEM_MAGNET_DISTANCE,
    ITEM_PICKUP_DELAY, ITEM_PICKUP_DISTANCE, ItemEntity, PickupTarget, XP_FOLLOW_DISTANCE,
    XP_LIFETIME, get_experience_value, merge_experience_orbs, merge_item_entities, remove_packet,
    spawn_drops_at, spawn_experience_at, split_experience,
};
use folia_core::math::{BlockPos, Vec3};
use folia_core::protocol::PacketReader;
use folia_random::LegacyRandomSource;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn player_at(position: Vec3) -> TestPlayer {
    TestPlayer {
        position,
        inventory: vec![ItemStack::empty(); 36],
        experience: 0,
    }
}

struct TestPlayer {
    position: Vec3,
    inventory: Vec<ItemStack>,
    experience: i32,
}

impl PickupTarget for TestPlayer {
    fn position(&self) -> Vec3 {
        self.position
    }

    fn add_item(&mut self, stack: ItemStack) -> ItemStack {
        add_stack(&mut self.inventory, stack)
    }

    fn add_experience(&mut self, amount: i32) {
        self.experience += amount;
    }
}

fn read_add_entity(packet: &[u8]) -> (i32, i32, [f64; 3]) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_ADD_ENTITY);
    let id = reader.read_varint().unwrap();
    let _uuid = reader.read_bytes(16).unwrap();
    let entity_type = reader.read_varint().unwrap();
    let x = reader.read_f64().unwrap();
    let y = reader.read_f64().unwrap();
    let z = reader.read_f64().unwrap();
    (id, entity_type, [x, y, z])
}

#[test]
fn spawner_velocity_is_deterministic_for_a_seed() {
    let pos = BlockPos::new(3, 64, -2);
    let mut drops = BlockDrops::empty();
    drops.items = vec![
        ItemStack::new("minecraft:stone", 2),
        ItemStack::new("minecraft:dirt", 1),
    ];
    drops.experience = 7;

    let mut first = LegacyRandomSource::new(1234);
    let mut second = LegacyRandomSource::new(1234);
    let a = spawn_drops_at(pos, &drops, &mut first);
    let b = spawn_drops_at(pos, &drops, &mut second);
    assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(&b) {
        match (left, right) {
            (DroppedEntity::Item(x), DroppedEntity::Item(y)) => {
                assert_eq!(x.velocity, y.velocity);
                assert_eq!(x.stack, y.stack);
                assert_eq!(x.pickup_delay, ITEM_PICKUP_DELAY);
                assert_eq!(x.age, 0);
            }
            (DroppedEntity::ExperienceOrb(x), DroppedEntity::ExperienceOrb(y)) => {
                assert_eq!(x.velocity, y.velocity);
                assert_eq!(x.value, y.value);
            }
            _ => panic!("entity kind mismatch"),
        }
    }
    // Item positions are the block centre; item y velocity is 0.2.
    for entity in &a {
        let (x, y, z) = match entity {
            DroppedEntity::Item(item) => (item.x, item.y, item.z),
            DroppedEntity::ExperienceOrb(orb) => (orb.x, orb.y, orb.z),
        };
        assert!((x - 3.5).abs() < 1.0e-9);
        assert!((y - 64.5).abs() < 1.0e-9);
        assert!((z - (-1.5)).abs() < 1.0e-9);
    }
    let item = match &a[0] {
        DroppedEntity::Item(item) => item,
        _ => panic!("first drop is an item"),
    };
    assert_eq!(item.velocity[1], 0.2);
}

#[test]
fn item_gravity_arc_matches_the_java_step() {
    let mut item = ItemEntity::new_with(
        9001,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        5.0,
        0.5,
        [0.0, 0.0, 0.0],
    );
    item.pickup_delay = 0;
    let mut collider = FlatCollider { ground_y: -1_000 };
    // Tick 1: vy = (0 - 0.04), move -0.04, drag -> -0.0392.
    item.tick_physics(&mut collider, None);
    assert!((item.y - 4.96).abs() < 1.0e-9, "y={}", item.y);
    assert!((item.velocity[1] - (-0.0392)).abs() < 1.0e-9);
    // Tick 2: vy = (-0.0392 - 0.04) = -0.0792, move, drag -> -0.077616.
    item.tick_physics(&mut collider, None);
    assert!((item.y - 4.8808).abs() < 1.0e-9, "y={}", item.y);
    assert!(item.velocity[1] < 0.0);
    assert!(!item.on_ground);
}

#[test]
fn item_lands_on_flat_ground() {
    let mut item = ItemEntity::new_with(
        9002,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        5.0,
        0.5,
        [0.0, 0.0, 0.0],
    );
    let mut collider = FlatCollider { ground_y: 0 };
    for _ in 0..300 {
        item.tick_physics(&mut collider, None);
    }
    assert!(item.y < 0.5, "y={}", item.y);
}

#[test]
fn item_magnet_pulls_toward_the_player_inside_four_blocks() {
    let mut item = ItemEntity::new_with(
        9003,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.5,
        0.5,
        [0.0, 0.0, 0.0],
    );
    item.pickup_delay = 10;
    // Ground at y=64 keeps the item near the player's height so the
    // horizontal pull dominates the distance check.
    let mut collider = FlatCollider { ground_y: 64 };
    let player = Vec3::new(3.5, 64.5, 0.5);
    assert!(player.distance_to(item.position()) <= ITEM_MAGNET_DISTANCE);
    let start_x = item.x;
    for _ in 0..10 {
        item.tick_physics(&mut collider, Some(player));
    }
    assert!(item.x > start_x, "x {} should exceed {}", item.x, start_x);
    assert!((player.x - item.x).abs() < (player.x - start_x).abs());
}

#[test]
fn item_magnet_ignores_a_player_beyond_range() {
    let mut item = ItemEntity::new_with(
        9004,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.5,
        0.5,
        [0.0, 0.0, 0.0],
    );
    item.pickup_delay = 10;
    let mut collider = FlatCollider { ground_y: -1_000 };
    let player = Vec3::new(20.5, 64.5, 0.5);
    // Only gravity applies; x stays at the start (velocity x is 0).
    for _ in 0..10 {
        item.tick_physics(&mut collider, Some(player));
    }
    assert!((item.x - 0.5).abs() < 1.0e-9, "x={}", item.x);
}

#[test]
fn xp_attraction_pulls_toward_the_player_inside_eight_blocks() {
    let mut orb = ExperienceOrb::new_with(9005, 0.5, 64.5, 0.5, [0.0, 0.0, 0.0], 3);
    let mut collider = FlatCollider { ground_y: -1_000 };
    let player = Vec3::new(5.5, 64.5, 0.5);
    assert!(player.distance_to(orb.position()) <= XP_FOLLOW_DISTANCE);
    let start_x = orb.x;
    for _ in 0..10 {
        orb.tick_physics(&mut collider, Some(player));
    }
    assert!(orb.x > start_x, "x {} should exceed {}", orb.x, start_x);
}

#[test]
fn merge_combines_counts_within_three_blocks_only() {
    let mut a = ItemEntity::new_with(
        9101,
        ItemStack::new("minecraft:stone", 10),
        0.0,
        0.0,
        0.0,
        [0.0; 3],
    );
    let mut b = ItemEntity::new_with(
        9102,
        ItemStack::new("minecraft:stone", 5),
        2.5,
        0.0,
        0.0,
        [0.0; 3],
    );
    a.pickup_delay = 0;
    b.pickup_delay = 0;
    let mut entities = vec![a, b];
    let removed = merge_item_entities(&mut entities);
    assert_eq!(removed.len(), 1);
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].stack.count, 15);

    // Beyond 3 blocks: no merge.
    let mut c = ItemEntity::new_with(
        9103,
        ItemStack::new("minecraft:stone", 10),
        0.0,
        0.0,
        0.0,
        [0.0; 3],
    );
    let mut d = ItemEntity::new_with(
        9104,
        ItemStack::new("minecraft:stone", 5),
        4.0,
        0.0,
        0.0,
        [0.0; 3],
    );
    c.pickup_delay = 0;
    d.pickup_delay = 0;
    let mut far = vec![c, d];
    assert!(merge_item_entities(&mut far).is_empty());
    assert_eq!(far.len(), 2);

    // Same position but different components: no merge.
    let mut plain = ItemEntity::new_with(
        9105,
        ItemStack::new("minecraft:stone", 1),
        0.0,
        0.0,
        0.0,
        [0.0; 3],
    );
    let mut named = ItemStack::new("minecraft:stone", 1);
    named.set_component(ComponentValue::CustomName(plain_text("Named")));
    let mut named_entity = ItemEntity::new_with(9106, named, 0.5, 0.0, 0.0, [0.0; 3]);
    plain.pickup_delay = 0;
    named_entity.pickup_delay = 0;
    let mut different = vec![plain, named_entity];
    assert!(merge_item_entities(&mut different).is_empty());
}

#[test]
fn xp_merge_sums_counts_for_equal_values_within_half_a_block() {
    let mut a = ExperienceOrb::new_with(9201, 0.0, 0.0, 0.0, [0.0; 3], 7);
    let mut b = ExperienceOrb::new_with(9202, 0.4, 0.0, 0.0, [0.0; 3], 7);
    let c = ExperienceOrb::new_with(9203, 0.0, 0.0, 0.0, [0.0; 3], 3);
    let mut orbs = vec![a.clone(), b.clone(), c.clone()];
    let removed = merge_experience_orbs(&mut orbs);
    assert_eq!(removed.len(), 1);
    assert_eq!(orbs.len(), 2);
    assert!(orbs.iter().any(|orb| orb.value == 7 && orb.count == 2));
    assert!(orbs.iter().any(|orb| orb.value == 3 && orb.count == 1));
    a.age = 10;
    b.age = 5;
    let mut pair = vec![a, b];
    merge_experience_orbs(&mut pair);
    assert_eq!(pair[0].age, 5);
    assert_eq!(pair[0].count, 2);
}

#[test]
fn xp_split_thresholds_and_spawn_helper_sum() {
    assert_eq!(get_experience_value(1), 1);
    assert_eq!(get_experience_value(2), 1);
    assert_eq!(get_experience_value(3), 3);
    assert_eq!(get_experience_value(7), 7);
    assert_eq!(get_experience_value(17), 17);
    assert_eq!(get_experience_value(37), 37);
    assert_eq!(get_experience_value(73), 73);
    assert_eq!(get_experience_value(149), 149);
    assert_eq!(get_experience_value(307), 307);
    assert_eq!(get_experience_value(617), 617);
    assert_eq!(get_experience_value(1237), 1237);
    assert_eq!(get_experience_value(2477), 2477);
    assert_eq!(get_experience_value(4958), 4957);
    assert_eq!(get_experience_value(162_670_130), 162_570_130);
    for amount in [0, 1, 2, 10, 100, 500, 5_000] {
        let orbs = split_experience(amount);
        assert_eq!(orbs.iter().sum::<i32>(), amount, "amount {amount}");
    }
    let mut random = LegacyRandomSource::new(99);
    let spawned = spawn_experience_at(BlockPos::new(0, 64, 0), 500, &mut random);
    assert_eq!(spawned.iter().map(|orb| orb.value).sum::<i32>(), 500);
    assert!(!spawned.is_empty());
}

#[test]
fn pickup_adds_to_inventory_with_components() {
    let mut named = ItemStack::new("minecraft:diamond", 2);
    named.set_component(ComponentValue::CustomName(plain_text("Gem")));
    let position = Vec3::new(1.5, 64.0, 2.5);
    let mut item = ItemEntity::new_with(
        9301,
        named.clone(),
        position.x,
        position.y,
        position.z,
        [0.0; 3],
    );
    item.pickup_delay = 0;
    let mut player = player_at(position);
    assert!(item.try_pickup(&mut player));
    assert!(item.stack.is_empty());
    assert_eq!(player.inventory[0], named);
}

#[test]
fn pickup_waits_for_the_delay_and_keeps_overflows() {
    let position = Vec3::new(0.5, 64.0, 0.5);
    let mut delayed = ItemEntity::new_with(
        9302,
        ItemStack::new("minecraft:stone", 1),
        position.x,
        position.y,
        position.z,
        [0.0; 3],
    );
    delayed.pickup_delay = 10;
    let mut player = player_at(position);
    assert!(!delayed.try_pickup(&mut player));
    assert!(player.inventory.iter().all(ItemStack::is_empty));

    // Partial fit: 60 + 10 stone leaves 6 on the entity because the rest of
    // the inventory is full of dirt and cannot take the remainder.
    let mut player = player_at(position);
    player.inventory[0] = ItemStack::new("minecraft:stone", 60);
    for slot in player.inventory.iter_mut().skip(1) {
        *slot = ItemStack::new("minecraft:dirt", 64);
    }
    let mut item = ItemEntity::new_with(
        9303,
        ItemStack::new("minecraft:stone", 10),
        position.x,
        position.y,
        position.z,
        [0.0; 3],
    );
    item.pickup_delay = 0;
    assert!(!item.try_pickup(&mut player));
    assert_eq!(item.stack.count, 6);
    assert_eq!(player.inventory[0].count, 64);

    // Far player: no pickup.
    let mut far = ItemEntity::new_with(
        9304,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.0,
        0.5,
        [0.0; 3],
    );
    far.pickup_delay = 0;
    let mut remote = player_at(Vec3::new(10.5, 64.0, 0.5));
    assert!(remote.position.distance_to(far.position()) > ITEM_PICKUP_DISTANCE);
    assert!(!far.try_pickup(&mut remote));
}

#[test]
fn xp_pickup_grants_experience() {
    let position = Vec3::new(0.5, 64.0, 0.5);
    let mut orb = ExperienceOrb::new_with(9305, position.x, position.y, position.z, [0.0; 3], 13);
    let mut player = player_at(position);
    assert!(orb.try_pickup(&mut player));
    assert_eq!(player.experience, 13);
}

#[test]
fn despawns_at_6000_ticks() {
    let mut item = ItemEntity::new_with(
        9401,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.5,
        0.5,
        [0.0; 3],
    );
    item.age = ITEM_LIFETIME - 1;
    let mut collider = FlatCollider { ground_y: -1_000 };
    item.tick_physics(&mut collider, None);
    assert!(item.should_despawn());
    assert_eq!(item.age, ITEM_LIFETIME);

    let mut orb = ExperienceOrb::new_with(9402, 0.5, 64.5, 0.5, [0.0; 3], 1);
    orb.age = XP_LIFETIME - 1;
    orb.tick_physics(&mut collider, None);
    assert!(orb.should_despawn());
}

#[test]
fn add_entity_metadata_and_sync_packets_for_item_and_xp() {
    let mut item = ItemEntity::new_with(
        9501,
        ItemStack::new("minecraft:dirt", 3),
        1.5,
        64.0,
        2.5,
        [0.1, 0.2, 0.3],
    );
    item.yaw = 90.0;
    let add = item.add_packet();
    let (id, entity_type, pos) = read_add_entity(&add);
    assert_eq!(id, 9501);
    assert_eq!(entity_type, folia_core::item_entity::item_entity_type_id());
    assert_eq!(pos, [1.5, 64.0, 2.5]);

    let (entity_id, values) = read_set_entity_data(&item.metadata_packet()).unwrap();
    assert_eq!(entity_id, 9501);
    let item_value = values
        .iter()
        .find(|value| value.index == DATA_ITEM)
        .expect("item metadata");
    assert_eq!(
        item_value.value,
        MetadataValue::ItemStack(ItemStack::new("minecraft:dirt", 3))
    );

    let sync_packet = item.position_sync_packet();
    let mut reader = PacketReader::new(&sync_packet);
    assert_eq!(
        reader.read_varint().unwrap(),
        CLIENTBOUND_ENTITY_POSITION_SYNC
    );
    assert_eq!(reader.read_varint().unwrap(), 9501);

    let orb = ExperienceOrb::new_with(9502, 1.5, 64.0, 2.5, [0.0; 3], 17);
    let (orb_id, orb_type, _) = read_add_entity(&orb.add_packet());
    assert_eq!(orb_id, 9502);
    assert_eq!(orb_type, folia_core::item_entity::experience_orb_type_id());
    let (entity_id, values) = read_set_entity_data(&orb.metadata_packet()).unwrap();
    assert_eq!(entity_id, 9502);
    let value = values
        .iter()
        .find(|value| value.index == DATA_VALUE)
        .expect("xp metadata");
    assert_eq!(value.value, MetadataValue::Int(17));

    let remove = remove_packet(&[9501, 9502]);
    let mut reader = PacketReader::new(&remove);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_REMOVE_ENTITIES);
    assert_eq!(reader.read_varint().unwrap(), 2);
    assert_eq!(reader.read_varint().unwrap(), 9501);
    assert_eq!(reader.read_varint().unwrap(), 9502);
}

#[test]
fn spawn_drops_at_splits_items_then_experience() {
    let pos = BlockPos::new(0, 64, 0);
    let mut drops = BlockDrops::empty();
    drops.items = vec![
        ItemStack::new("minecraft:stone", 1),
        ItemStack::new("minecraft:dirt", 4),
    ];
    drops.experience = 7;
    let mut random = LegacyRandomSource::new(7);
    let spawned = spawn_drops_at(pos, &drops, &mut random);
    assert_eq!(spawned.len(), 3);
    assert!(spawned[0].is_item());
    assert!(spawned[1].is_item());
    assert!(spawned[2].is_experience_orb());
    let xp = match &spawned[2] {
        DroppedEntity::ExperienceOrb(orb) => orb,
        _ => unreachable!(),
    };
    assert_eq!(xp.value, 7);
    for entity in &spawned {
        assert_eq!(entity.entity_type_id(), {
            match entity {
                DroppedEntity::Item(_) => folia_core::item_entity::item_entity_type_id(),
                DroppedEntity::ExperienceOrb(_) => {
                    folia_core::item_entity::experience_orb_type_id()
                }
            }
        });
    }
    // Empty drops spawn nothing.
    let mut empty = LegacyRandomSource::new(1);
    assert!(spawn_drops_at(pos, &BlockDrops::empty(), &mut empty).is_empty());
}

#[test]
fn stop_with_entities_returns_ground_drops_and_keeps_the_loot_path() {
    let stone = state("minecraft:stone");
    let pick = ItemStack::new("minecraft:wooden_pickaxe", 1);
    let mining = MiningContext::default();
    let pos = BlockPos::new(0, 64, 0);
    let mut dig = DigState::default();
    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &mining, 0),
        DigStart::Started
    );

    let loot_context = BlockLootContext::from_tool(Some(&pick));
    let loot = DigLootContext {
        context: &loot_context,
        provider: None,
    };
    let mut random = LegacyRandomSource::new(4);
    let result = dig.stop_with_entities(pos, &stone, &mining, 18, &loot, &mut random);
    assert_eq!(result.stop, DigStop::Destroyed);
    let drops = result.drops.expect("a destroyed block has drops");
    assert_eq!(drops.items[0].item, "minecraft:cobblestone");
    assert!(!result.entities.is_empty());
    let first = match &result.entities[0] {
        DroppedEntity::Item(item) => item,
        _ => panic!("stone drops items, not XP"),
    };
    assert_eq!(first.stack.item, "minecraft:cobblestone");
    // The legacy tuple path still works for inventory callers.
    let mut dig = DigState::default();
    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &mining, 0),
        DigStart::Started
    );
    let mut random = LegacyRandomSource::new(4);
    let (stop, drops) = dig.stop_with_loot(pos, &stone, &mining, 18, &loot, &mut random);
    assert_eq!(stop, DigStop::Destroyed);
    assert_eq!(
        drops.expect("legacy path rolls drops").items[0].item,
        "minecraft:cobblestone"
    );
}
