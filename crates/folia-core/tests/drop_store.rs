//! Focused tests for the session-free dropped-entity store.

use folia_core::block_breaking::BlockDrops;
use folia_core::entity::FlatCollider;
use folia_core::inventory::ItemStack;
use folia_core::item_entity::{
    DropEntityStore, DroppedEntity, ExperienceOrb, ITEM_LIFETIME, ItemEntity, PickupTarget,
};
use folia_core::math::{BlockPos, Vec3};
use folia_core::protocol::PacketReader;
use folia_core::random::LegacyRandomSource;
use std::collections::HashSet;

const ADD_ENTITY: i32 = 1;
const POSITION_SYNC: i32 = 35;
const REMOVE_ENTITIES: i32 = 75;

#[derive(Debug, Default)]
struct FakeTarget {
    position: Vec3,
    inventory: Vec<ItemStack>,
    xp: i32,
}

impl FakeTarget {
    fn at(x: f64, y: f64, z: f64) -> Self {
        Self {
            position: Vec3::new(x, y, z),
            ..Self::default()
        }
    }
}

impl PickupTarget for FakeTarget {
    fn position(&self) -> Vec3 {
        self.position
    }

    fn add_item(&mut self, stack: ItemStack) -> ItemStack {
        if stack.is_empty() {
            return stack;
        }
        if let Some(slot) = self
            .inventory
            .iter_mut()
            .find(|existing| existing.same_item_and_components(&stack))
        {
            let space = slot.max_stack_size().saturating_sub(slot.count);
            let take = space.min(stack.count);
            slot.count += take;
            let mut leftover = stack;
            leftover.count -= take;
            if leftover.is_empty() {
                return ItemStack::empty();
            }
            self.inventory.push(leftover);
            ItemStack::empty()
        } else {
            self.inventory.push(stack);
            ItemStack::empty()
        }
    }

    fn add_experience(&mut self, amount: i32) {
        self.xp += amount;
    }
}

fn packet_id(packet: &[u8]) -> i32 {
    PacketReader::new(packet).read_varint().expect("packet id")
}

fn remove_ids(packet: &[u8]) -> Vec<i32> {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), REMOVE_ENTITIES);
    let count = reader.read_varint().unwrap();
    let mut ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        ids.push(reader.read_varint().unwrap());
    }
    ids
}

#[test]
fn spawn_emits_add_and_metadata_for_each_entity() {
    let mut store = DropEntityStore::new();
    let drops = BlockDrops {
        items: vec![
            ItemStack::new("minecraft:stone", 1),
            ItemStack::new("minecraft:dirt", 2),
        ],
        experience: 5,
        ..BlockDrops::empty()
    };
    let mut rng = LegacyRandomSource::new(42);
    let packets = store.spawn(BlockPos::new(0, 64, 0), &drops, &mut rng);

    // experience 5 splits into 3 + 1 + 1 = 3 orbs
    assert_eq!(store.len(), 5);
    assert_eq!(packets.len(), 10);
    for pair in packets.chunks_exact(2) {
        assert_eq!(packet_id(&pair[0]), ADD_ENTITY);
        // second is metadata (set entity data); id differs by entity but packet exists
        assert!(!pair[1].is_empty());
    }
}

#[test]
fn tick_applies_gravity_and_emits_position_sync() {
    let mut store = DropEntityStore::new();
    let mut item = ItemEntity::new_with(
        100,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        5.0,
        0.5,
        [0.0, 0.0, 0.0],
    );
    item.pickup_delay = 20;
    store.insert(DroppedEntity::Item(item));
    let mut collider = FlatCollider { ground_y: -1000 };
    let mut target = FakeTarget::at(100.0, 100.0, 100.0);

    let tick = store.tick(&mut collider, &mut target);
    assert!(tick.removed.is_empty());
    assert!(tick.packets.iter().any(|p| packet_id(p) == POSITION_SYNC));
    assert!(!tick.picked_items);
    assert!(!tick.picked_experience);
    assert_eq!(store.len(), 1);
}

#[test]
fn pickup_after_delay_adds_items_and_removes_entity() {
    let mut store = DropEntityStore::new();
    let drops = BlockDrops {
        items: vec![ItemStack::new("minecraft:diamond", 3)],
        experience: 0,
        ..BlockDrops::empty()
    };
    let mut rng = LegacyRandomSource::new(1);
    let spawn_packets = store.spawn(BlockPos::new(0, 64, 0), &drops, &mut rng);
    assert_eq!(spawn_packets.len(), 2);
    assert_eq!(store.len(), 1);

    let mut collider = FlatCollider { ground_y: 0 };
    let mut target = FakeTarget::at(0.5, 64.5, 0.5);
    let mut removed = false;
    for _ in 0..12 {
        let tick = store.tick(&mut collider, &mut target);
        if tick.packets.iter().any(|p| packet_id(p) == REMOVE_ENTITIES) {
            removed = true;
            assert_eq!(tick.removed.len(), 1);
            break;
        }
    }
    assert!(removed, "item should be picked up after delay");
    assert_eq!(store.len(), 0);
    assert_eq!(target.inventory.len(), 1);
    assert_eq!(target.inventory[0].item, "minecraft:diamond");
    assert_eq!(target.inventory[0].count, 3);
}

#[test]
fn xp_orb_pickup_adds_experience() {
    let mut store = DropEntityStore::new();
    let drops = BlockDrops {
        items: Vec::new(),
        experience: 5,
        ..BlockDrops::empty()
    };
    let mut rng = LegacyRandomSource::new(2);
    store.spawn(BlockPos::new(0, 64, 0), &drops, &mut rng);
    assert!(!store.is_empty());

    let mut collider = FlatCollider { ground_y: 0 };
    let mut target = FakeTarget::at(0.5, 64.5, 0.5);
    let mut total_removed = 0usize;
    let mut picked_experience = false;
    for _ in 0..40 {
        let tick = store.tick(&mut collider, &mut target);
        if tick.picked_experience {
            picked_experience = true;
        }
        total_removed += tick.removed.len();
        if store.is_empty() {
            break;
        }
    }
    assert!(picked_experience);
    assert_eq!(target.xp, 5);
    assert_eq!(store.len(), 0);
    assert!(total_removed >= 3);
}

#[test]
fn merge_absorbs_nearby_items() {
    let mut store = DropEntityStore::new();
    // Deterministic nearby identical items
    let a = ItemEntity::new_with(
        1,
        ItemStack::new("minecraft:stone", 10),
        0.5,
        64.5,
        0.5,
        [0.0; 3],
    );
    let b = ItemEntity::new_with(
        2,
        ItemStack::new("minecraft:stone", 5),
        1.0,
        64.5,
        0.5,
        [0.0; 3],
    );
    store.insert(DroppedEntity::Item(a));
    store.insert(DroppedEntity::Item(b));

    let mut collider = FlatCollider { ground_y: 0 };
    let mut target = FakeTarget::at(100.0, 100.0, 100.0);
    let tick = store.tick(&mut collider, &mut target);

    assert_eq!(store.len(), 1);
    assert!(tick.removed.contains(&1) || tick.removed.contains(&2));
    assert!(tick.packets.iter().any(|p| packet_id(p) == REMOVE_ENTITIES));
}

#[test]
fn retain_in_chunks_removes_outside_drops() {
    let mut store = DropEntityStore::new();
    let mut item = ItemEntity::new_with(
        10,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.0,
        0.5,
        [0.0; 3],
    );
    item.pickup_delay = 20;
    store.insert(DroppedEntity::Item(item));

    let mut keep = HashSet::new();
    keep.insert((0, 0));
    let packets = store.retain_in_chunks(&keep);
    assert!(packets.is_empty());
    assert_eq!(store.len(), 1);

    let packets = store.retain_in_chunks(&HashSet::new());
    assert_eq!(store.len(), 0);
    assert_eq!(packets.len(), 1);
    assert_eq!(packet_id(&packets[0]), REMOVE_ENTITIES);
    assert_eq!(remove_ids(&packets[0]), vec![10]);
}

#[test]
fn tick_despawns_at_lifetime() {
    let mut store = DropEntityStore::new();
    let mut item = ItemEntity::new_with(
        20,
        ItemStack::new("minecraft:stone", 1),
        0.5,
        64.0,
        0.5,
        [0.0; 3],
    );
    item.age = ITEM_LIFETIME;
    store.insert(DroppedEntity::Item(item));

    let mut collider = FlatCollider { ground_y: -1000 };
    let mut target = FakeTarget::at(100.0, 100.0, 100.0);
    let tick = store.tick(&mut collider, &mut target);

    assert_eq!(store.len(), 0);
    assert!(tick.removed.contains(&20));
    assert!(
        tick.packets
            .iter()
            .any(|p| { packet_id(p) == REMOVE_ENTITIES && remove_ids(p).contains(&20) })
    );
}
