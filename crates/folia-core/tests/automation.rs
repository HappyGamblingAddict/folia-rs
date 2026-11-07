//! Target-block and dispenser/dropper automation tests.

use folia_core::anvil::Compression;
use folia_core::block_entity::{
    BlockEntity, BlockEntityPos, ChestBlockEntity, DispenserBlockEntity,
};
use folia_core::block_shapes::{Aabb, collision_boxes_for_state};
use folia_core::block_update;
use folia_core::chunk::BlockState;
use folia_core::chunk_packet::flat_chunk;
use folia_core::item_components::{ComponentMap, ComponentValue, ItemStack};
use folia_core::math::{BlockPos, Direction};
use folia_core::nbt::Tag;
use folia_core::redstone::{
    self, DispenseKind, DispensedProjectile, ProjectileSink, SignalReader, TargetProjectile,
};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-automation-{}-{}-{safe}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn flat_storage(name: &str) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();
    storage
}

fn state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn place(storage: &mut WorldStorage, pos: BlockPos, state: BlockState) {
    block_update::set_block(storage, pos.x(), pos.y(), pos.z(), state)
        .unwrap()
        .expect("placement changed the block");
}

fn block(storage: &WorldStorage, pos: BlockPos) -> BlockState {
    storage
        .block_state_at(pos.x(), pos.y(), pos.z())
        .expect("block is in a cached chunk")
}

fn facing_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        Direction::Up => "up",
        Direction::Down => "down",
    }
}

fn be_pos(pos: BlockPos) -> BlockEntityPos {
    BlockEntityPos::new(pos.x(), pos.y(), pos.z())
}

fn insert_dispenser(
    storage: &mut WorldStorage,
    pos: BlockPos,
    block_id: &str,
    facing: Direction,
    items: &[(usize, ItemStack)],
) {
    let mut entity = DispenserBlockEntity::with_state(be_pos(pos), block_id, facing, false);
    for (slot, stack) in items {
        assert!(entity.set_slot(*slot, stack.clone()));
    }
    storage
        .insert_block_entity(BlockEntity::dispenser(entity).unwrap())
        .unwrap();
}

fn dispenser(storage: &mut WorldStorage, pos: BlockPos) -> DispenserBlockEntity {
    storage
        .block_entity(be_pos(pos))
        .unwrap()
        .expect("dispenser block entity exists")
        .as_dispenser()
        .expect("typed dispenser state")
}

fn insert_chest(storage: &mut WorldStorage, pos: BlockPos, items: &[(usize, ItemStack)]) {
    let mut chest = ChestBlockEntity::new(be_pos(pos));
    for (slot, stack) in items {
        assert!(chest.set_slot(*slot, stack.clone()));
    }
    storage
        .insert_block_entity(BlockEntity::Chest(chest))
        .unwrap();
}

fn trigger_pulse(storage: &mut WorldStorage, source: BlockPos, from_tick: u64) -> u64 {
    place(storage, source, state("minecraft:redstone_block"));
    let mut tick_number = from_tick;
    for _ in 0..redstone::DISPENSER_TRIGGER_TICKS {
        tick_number += 1;
        tick::tick_world(storage, tick_number).unwrap();
    }
    tick_number
}

#[test]
fn target_tiers_shape_and_signal_output() {
    // `TargetBlock.getRedstoneStrength` never returns 0 for a hit.
    assert_eq!(
        redstone::target_output_power(Direction::Up, [0.5, 1.0, 0.5]),
        15
    );
    assert_eq!(
        redstone::target_output_power(Direction::Up, [0.5, 1.0, 0.68]),
        10
    );
    assert_eq!(
        redstone::target_output_power(Direction::Up, [0.5, 1.0, 0.84]),
        5
    );
    // A side face uses the other two in-face axes.
    assert_eq!(
        redstone::target_output_power(Direction::East, [1.0, 0.5, 0.68]),
        10
    );

    let target = state("minecraft:target");
    assert_eq!(redstone::target_power(&target), 0);
    assert!(redstone::is_signal_source(&target));
    assert!(folia_core::block_behaviour::is_solid(&target));
    assert_eq!(
        collision_boxes_for_state(&target),
        &[Aabb::FULL],
        "target blocks are full cubes"
    );

    let mut storage = flat_storage("target-shape");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, target);
    let powered = redstone::with_property(&block(&storage, pos), "power", "10");
    place(&mut storage, pos, powered);
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.signal(pos, Direction::South), 10);
    assert_eq!(redstone::neighbor_power(&storage, pos.east()), 10);
}

#[test]
fn target_hit_decays_after_eight_redstone_ticks() {
    let mut storage = flat_storage("target-decay");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, state("minecraft:target"));
    assert_eq!(
        SignalReader::world(&storage).signal(pos, Direction::South),
        0
    );

    let strength = redstone::target_hit(
        &mut storage,
        0,
        pos,
        Direction::Up,
        [0.5, -59.0, 0.5],
        TargetProjectile::Other,
    )
    .unwrap();
    assert_eq!(strength, 15);
    assert_eq!(redstone::target_power(&block(&storage, pos)), 15);
    assert_eq!(redstone::neighbor_power(&storage, pos.east()), 15);

    for tick_number in 1..=7 {
        tick::tick_world(&mut storage, tick_number).unwrap();
        assert_eq!(
            redstone::target_power(&block(&storage, pos)),
            15,
            "target reset early at tick {tick_number}"
        );
    }
    tick::tick_world(&mut storage, 8).unwrap();
    assert_eq!(redstone::target_power(&block(&storage, pos)), 0);
    assert_eq!(
        SignalReader::world(&storage).signal(pos, Direction::South),
        0
    );
}

#[test]
fn target_arrow_hit_holds_twenty_ticks() {
    let mut storage = flat_storage("target-arrow");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, state("minecraft:target"));
    redstone::target_hit(
        &mut storage,
        0,
        pos,
        Direction::Up,
        [0.5, -59.0, 0.5],
        TargetProjectile::Arrow,
    )
    .unwrap();
    for tick_number in 1..=20 {
        tick::tick_world(&mut storage, tick_number).unwrap();
        let expected = if tick_number < 20 { 15 } else { 0 };
        assert_eq!(
            redstone::target_power(&block(&storage, pos)),
            expected,
            "tick {tick_number}"
        );
    }
}

#[test]
fn placing_a_powered_target_clears_the_stale_output() {
    let mut storage = flat_storage("target-stale");
    let pos = BlockPos::new(0, -60, 0);
    let stale = redstone::with_property(&state("minecraft:target"), "power", "7");
    place(&mut storage, pos, stale);
    assert_eq!(redstone::target_power(&block(&storage, pos)), 0);
}

#[test]
fn dispenser_fires_once_per_rising_edge() {
    let mut storage = flat_storage("dispenser-pulse");
    let pos = BlockPos::new(0, -60, 0);
    let crop_pos = pos.above();
    place(
        &mut storage,
        pos,
        redstone::with_property(
            &state("minecraft:dispenser"),
            "facing",
            facing_name(Direction::Up),
        ),
    );
    place(&mut storage, crop_pos, state("minecraft:wheat"));
    insert_dispenser(
        &mut storage,
        pos,
        "minecraft:dispenser",
        Direction::Up,
        &[(0, ItemStack::new("minecraft:bone_meal", 4))],
    );

    let mut tick_number = trigger_pulse(&mut storage, BlockPos::new(1, -60, 0), 0);
    assert_eq!(dispenser(&mut storage, pos).slot(0).unwrap().count, 3);
    assert!(
        folia_core::crops::age(&block(&storage, crop_pos)) > 0,
        "bone meal did not grow the crop"
    );
    let age_after_first = folia_core::crops::age(&block(&storage, crop_pos));

    // Holding the signal must not schedule another dispense.
    for _ in 0..8 {
        tick_number += 1;
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(dispenser(&mut storage, pos).slot(0).unwrap().count, 3);
    assert_eq!(
        folia_core::crops::age(&block(&storage, crop_pos)),
        age_after_first
    );

    // Falling edge clears `triggered`; a new rising edge fires exactly once more.
    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:air"),
    );
    for _ in 0..2 {
        tick_number += 1;
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(
        redstone::property(&block(&storage, pos), "triggered"),
        Some("false")
    );
    trigger_pulse(&mut storage, BlockPos::new(1, -60, 0), tick_number);
    assert_eq!(dispenser(&mut storage, pos).slot(0).unwrap().count, 2);
    assert!(folia_core::crops::age(&block(&storage, crop_pos)) > age_after_first);
}

#[test]
fn dropper_moves_one_item_into_the_container_in_front() {
    let mut storage = flat_storage("dropper-transfer");
    let dropper_pos = BlockPos::new(0, -60, 0);
    let chest_pos = BlockPos::new(1, -60, 0);
    place(
        &mut storage,
        dropper_pos,
        redstone::with_property(
            &state("minecraft:dropper"),
            "facing",
            facing_name(Direction::East),
        ),
    );
    place(&mut storage, chest_pos, state("minecraft:chest"));

    let mut components = ComponentMap::new();
    components.set(ComponentValue::CustomName(Tag::String(
        "marked ingot".into(),
    )));
    let mut marked = ItemStack::new("minecraft:iron_ingot", 3);
    marked.components = components.clone();
    insert_dispenser(
        &mut storage,
        dropper_pos,
        "minecraft:dropper",
        Direction::East,
        &[(0, marked)],
    );
    insert_chest(&mut storage, chest_pos, &[]);

    let mut tick_number = trigger_pulse(&mut storage, BlockPos::new(0, -60, 1), 0);
    let moved = storage
        .block_entity(be_pos(chest_pos))
        .unwrap()
        .expect("chest exists")
        .container_items()
        .expect("chest items");
    assert_eq!(moved[0].item, "minecraft:iron_ingot");
    assert_eq!(moved[0].count, 1);
    assert_eq!(moved[0].components, components, "components were lost");
    assert_eq!(
        dispenser(&mut storage, dropper_pos).slot(0).unwrap().count,
        2
    );

    // Still the same pulse: no second item transfer.
    for _ in 0..6 {
        tick_number += 1;
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    let moved = storage
        .block_entity(be_pos(chest_pos))
        .unwrap()
        .unwrap()
        .container_items()
        .unwrap();
    assert_eq!(moved[0].count, 1);
    assert_eq!(
        dispenser(&mut storage, dropper_pos).slot(0).unwrap().count,
        2
    );
}

#[test]
fn full_crop_rejects_bone_meal_without_consuming_it() {
    let mut storage = flat_storage("full-crop");
    let pos = BlockPos::new(0, -60, 0);
    let crop_pos = pos.above();
    place(
        &mut storage,
        pos,
        redstone::with_property(
            &state("minecraft:dispenser"),
            "facing",
            facing_name(Direction::Up),
        ),
    );
    let mut full = state("minecraft:wheat");
    full = redstone::with_property(&full, "age", "7");
    place(&mut storage, crop_pos, full);
    insert_dispenser(
        &mut storage,
        pos,
        "minecraft:dispenser",
        Direction::Up,
        &[(0, ItemStack::new("minecraft:bone_meal", 1))],
    );

    trigger_pulse(&mut storage, BlockPos::new(1, -60, 0), 0);
    assert_eq!(
        folia_core::crops::age(&block(&storage, crop_pos)),
        folia_core::crops::max_age(&block(&storage, crop_pos))
    );
    assert_eq!(dispenser(&mut storage, pos).slot(0).unwrap().count, 1);
    assert_eq!(
        redstone::property(&block(&storage, pos), "triggered"),
        Some("true")
    );
}

#[test]
fn dispenser_slot_selection_is_deterministic() {
    let mut first = DispenserBlockEntity::new(be_pos(BlockPos::new(0, -60, 0)));
    let mut second = first.clone();
    first.set_slot(0, ItemStack::new("minecraft:bone_meal", 1));
    second.set_slot(0, ItemStack::new("minecraft:bone_meal", 1));
    first.set_slot(4, ItemStack::new("minecraft:arrow", 1));
    second.set_slot(4, ItemStack::new("minecraft:arrow", 1));
    first.set_slot(8, ItemStack::new("minecraft:egg", 1));
    second.set_slot(8, ItemStack::new("minecraft:egg", 1));
    assert_eq!(first.items.len(), 9);

    let mut left = folia_core::random::XoroshiroRandomSource::new(0x5eed);
    let mut right = folia_core::random::XoroshiroRandomSource::new(0x5eed);
    let left_slots: Vec<_> = (0..16).map(|_| first.get_random_slot(&mut left)).collect();
    let right_slots: Vec<_> = (0..16)
        .map(|_| second.get_random_slot(&mut right))
        .collect();
    assert_eq!(left_slots, right_slots);
    assert!(left_slots.iter().all(Option::is_some));
}

#[test]
fn projectile_dispense_uses_the_injected_sink_and_preserves_components() {
    #[derive(Default)]
    struct RecordingSink {
        shots: Vec<(DispensedProjectile, [f64; 3], [f64; 3], ItemStack)>,
    }

    impl ProjectileSink for RecordingSink {
        fn shoot(
            &mut self,
            projectile: DispensedProjectile,
            position: [f64; 3],
            velocity: [f64; 3],
            stack: &ItemStack,
        ) -> std::io::Result<()> {
            self.shots
                .push((projectile, position, velocity, stack.clone()));
            Ok(())
        }
    }

    let mut storage = flat_storage("dispenser-arrow");
    let pos = BlockPos::new(0, -60, 0);
    place(
        &mut storage,
        pos,
        redstone::with_property(
            &state("minecraft:dispenser"),
            "facing",
            facing_name(Direction::East),
        ),
    );
    let mut arrow = ItemStack::new("minecraft:arrow", 2);
    let mut components = ComponentMap::new();
    components.set(ComponentValue::CustomName(Tag::String(
        "signal arrow".into(),
    )));
    arrow.components = components.clone();
    insert_dispenser(
        &mut storage,
        pos,
        "minecraft:dispenser",
        Direction::East,
        &[(0, arrow)],
    );

    let mut sink = RecordingSink::default();
    let outcome = redstone::dispense_from(&mut storage, 10, pos, Some(&mut sink))
        .unwrap()
        .expect("dispense outcome");
    assert!(outcome.success);
    assert_eq!(sink.shots.len(), 1);
    let (projectile, position, velocity, shot) = &sink.shots[0];
    assert_eq!(*projectile, DispensedProjectile::Arrow);
    assert_eq!(*position, [1.2, -59.4, 0.5]);
    assert_eq!(*velocity, [1.1, 0.0, 0.0]);
    assert_eq!(shot.count, 1);
    assert_eq!(shot.components, components);
    assert_eq!(dispenser(&mut storage, pos).slot(0).unwrap().count, 1);
}

#[test]
fn unsupported_dispenser_behaviors_fail_loudly() {
    for item in [
        "minecraft:arrow",
        "minecraft:water_bucket",
        "minecraft:oak_boat",
        "minecraft:minecart",
        "minecraft:dirt",
    ] {
        let mut storage = flat_storage(&format!("unsupported-{item}"));
        let pos = BlockPos::new(0, -60, 0);
        place(
            &mut storage,
            pos,
            redstone::with_property(
                &state("minecraft:dispenser"),
                "facing",
                facing_name(Direction::East),
            ),
        );
        insert_dispenser(
            &mut storage,
            pos,
            "minecraft:dispenser",
            Direction::East,
            &[(0, ItemStack::new(item, 1))],
        );

        let error = redstone::dispense_from(&mut storage, 1, pos, None)
            .expect_err("unsupported behavior must fail");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::Unsupported,
            "{item}: {error}"
        );
        assert_eq!(
            dispenser(&mut storage, pos).slot(0).unwrap().count,
            1,
            "{item} was consumed by an unsupported behavior"
        );
    }

    assert_eq!(
        redstone::dispense_kind("minecraft:water_bucket"),
        DispenseKind::Bucket
    );
    assert_eq!(
        redstone::dispense_kind("minecraft:oak_boat"),
        DispenseKind::Boat
    );
    assert_eq!(
        redstone::dispense_kind("minecraft:minecart"),
        DispenseKind::Minecart
    );
    assert_eq!(
        redstone::dispense_kind("minecraft:snowball"),
        DispenseKind::Projectile(DispensedProjectile::Snowball)
    );
}

#[test]
fn shears_harvest_a_full_beehive_and_keep_their_components() {
    let mut storage = flat_storage("shears");
    let pos = BlockPos::new(0, -60, 0);
    let hive_pos = pos.east();
    place(
        &mut storage,
        pos,
        redstone::with_property(
            &state("minecraft:dispenser"),
            "facing",
            facing_name(Direction::East),
        ),
    );
    let mut hive = state("minecraft:beehive");
    hive = redstone::with_property(&hive, "honey_level", "5");
    place(&mut storage, hive_pos, hive);
    let mut shears = ItemStack::new("minecraft:shears", 1);
    shears.set_component(ComponentValue::Damage(3));
    insert_dispenser(
        &mut storage,
        pos,
        "minecraft:dispenser",
        Direction::East,
        &[(0, shears)],
    );

    let outcome = redstone::dispense_from(&mut storage, 4, pos, None)
        .unwrap()
        .expect("dispense outcome");
    assert!(outcome.success);
    assert_eq!(
        redstone::property(&block(&storage, hive_pos), "honey_level"),
        Some("0")
    );
    let entity = dispenser(&mut storage, pos);
    assert_eq!(entity.slot(0).unwrap().damage(), 4);
    assert!(
        entity
            .items
            .iter()
            .any(|stack| stack.item == "minecraft:honeycomb" && stack.count == 1),
        "honeycomb was not inserted into the dispenser"
    );
}
