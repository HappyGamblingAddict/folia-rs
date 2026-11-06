//! Focused end-to-end tests for pistons: push/pull, the 12-block resolver,
//! sticky chains, immovable blocks, plant destruction, retraction, and
//! deterministic update order.
//!
//! These drive the real `tick_world` loop over persisted chunks rather than
//! poking block storage directly.

use folia_core::anvil::Compression;
use folia_core::block_shapes;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::{BlockPos, Direction};
use folia_core::piston;
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-pistons-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
    storage
}

fn flat_storage(name: &str) -> WorldStorage {
    storage_with_chunk(name, flat_chunk(0, 0))
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

fn default_state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn air() -> BlockState {
    default_state("minecraft:air")
}

fn stone() -> BlockState {
    default_state("minecraft:stone")
}

fn redstone_block() -> BlockState {
    default_state("minecraft:redstone_block")
}

fn piston_state(sticky: bool, facing: Direction) -> BlockState {
    piston::piston_base_state(sticky, facing, false)
}

fn run_ticks(storage: &mut WorldStorage, from: u64, to: u64) {
    for tick_number in from..=to {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

#[test]
fn piston_extends_pushes_one_block_and_places_the_head() {
    let mut storage = flat_storage("push-one");
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    let destination = front.relative(Direction::East);

    place(&mut storage, base, piston_state(false, Direction::East));
    place(&mut storage, front, stone());
    place(
        &mut storage,
        base.relative(Direction::North),
        redstone_block(),
    );

    let extended = block(&storage, base);
    assert!(piston::is_piston(&extended));
    assert!(piston::piston_extended(&extended));
    assert_eq!(piston::piston_facing(&extended), Direction::East);
    assert!(piston::is_moving_piston(&block(&storage, front)));
    assert!(piston::is_moving_piston(&block(&storage, destination)));
    assert_eq!(
        piston::moving_collision_boxes(&storage, destination).len(),
        1
    );
    assert!(!piston::moving_collision_boxes(&storage, front).is_empty());

    run_ticks(&mut storage, 1, 3);

    let head = block(&storage, front);
    assert_eq!(head.name, "minecraft:piston_head");
    assert_eq!(piston::piston_facing(&head), Direction::East);
    assert_eq!(piston::piston_type(&head), "normal");
    assert_eq!(
        block_shapes::collision_boxes_for_state(&head).len(),
        2,
        "the head is a partial shape, not a full cube"
    );
    assert_eq!(block(&storage, destination), stone());
    assert!(storage.tick_state().pistons.is_empty());
}

#[test]
fn sticky_piston_pulls_the_block_back_one_cell() {
    let mut storage = flat_storage("sticky-pull");
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    let pushed = front.relative(Direction::East);
    let power = base.relative(Direction::North);

    place(&mut storage, base, piston_state(true, Direction::East));
    place(&mut storage, front, stone());
    place(&mut storage, power, redstone_block());

    run_ticks(&mut storage, 1, 3);
    assert_eq!(block(&storage, front).name, "minecraft:piston_head");
    assert_eq!(piston::piston_type(&block(&storage, front)), "sticky");
    assert_eq!(block(&storage, pushed), stone());

    place(&mut storage, power, air());
    let retracting = block(&storage, base);
    assert!(piston::is_moving_piston(&retracting));
    assert_eq!(piston::piston_facing(&retracting), Direction::East);
    assert!(piston::is_moving_piston(&block(&storage, front)));
    assert_eq!(block(&storage, pushed), air());

    run_ticks(&mut storage, 4, 6);
    assert_eq!(block(&storage, front), stone());
    assert_eq!(block(&storage, pushed), air());
    let retracted = block(&storage, base);
    assert!(piston::is_piston(&retracted));
    assert!(!piston::piston_extended(&retracted));
    assert!(storage.tick_state().pistons.is_empty());
}

#[test]
fn piston_respects_the_twelve_block_limit() {
    let mut storage = flat_storage("limit-twelve");
    let base = BlockPos::new(1, -60, 5);
    let power = base.relative(Direction::North);
    place(&mut storage, base, piston_state(false, Direction::East));
    for distance in 1..=12 {
        place(
            &mut storage,
            base.relative_n(Direction::East, distance),
            stone(),
        );
    }
    place(&mut storage, power, redstone_block());
    assert!(piston::piston_extended(&block(&storage, base)));
    run_ticks(&mut storage, 1, 3);
    assert_eq!(
        block(&storage, base.relative_n(Direction::East, 13)),
        stone()
    );
    assert_eq!(block(&storage, base.relative_n(Direction::East, 14)), air());
    assert!(storage.tick_state().pistons.is_empty());

    let mut storage = flat_storage("limit-thirteen");
    let base = BlockPos::new(1, -60, 5);
    let power = base.relative(Direction::North);
    place(&mut storage, base, piston_state(false, Direction::East));
    for distance in 1..=13 {
        place(
            &mut storage,
            base.relative_n(Direction::East, distance),
            stone(),
        );
    }
    place(&mut storage, power, redstone_block());
    let base_state = block(&storage, base);
    assert!(!piston::piston_extended(&base_state));
    for distance in 1..=13 {
        assert_eq!(
            block(&storage, base.relative_n(Direction::East, distance)),
            stone(),
            "block {distance} stays put when the line exceeds 12"
        );
    }
    assert!(storage.tick_state().pistons.is_empty());
}

#[test]
fn slime_chains_move_sideways_but_honey_does_not_stick_to_slime() {
    let mut slime = flat_storage("slime-chain");
    let slime_base = BlockPos::new(5, -59, 5);
    let slime_front = slime_base.relative(Direction::East);
    let slime_side = slime_front.relative(Direction::South);
    place(&mut slime, slime_base, piston_state(false, Direction::East));
    place(
        &mut slime,
        slime_front,
        default_state("minecraft:slime_block"),
    );
    place(&mut slime, slime_side, stone());
    place(
        &mut slime,
        slime_base.relative(Direction::North),
        redstone_block(),
    );
    run_ticks(&mut slime, 1, 3);
    assert_eq!(
        block(&slime, slime_front.relative(Direction::East)).name,
        "minecraft:slime_block"
    );
    assert_eq!(
        block(&slime, slime_side.relative(Direction::East)),
        stone(),
        "the side block travels with the slime chain"
    );
    assert_eq!(block(&slime, slime_side), air());

    let mut honey = flat_storage("honey-vs-slime");
    let honey_base = BlockPos::new(5, -59, 5);
    let honey_front = honey_base.relative(Direction::East);
    let honey_side = honey_front.relative(Direction::South);
    place(&mut honey, honey_base, piston_state(false, Direction::East));
    place(
        &mut honey,
        honey_front,
        default_state("minecraft:honey_block"),
    );
    place(
        &mut honey,
        honey_side,
        default_state("minecraft:slime_block"),
    );
    place(
        &mut honey,
        honey_base.relative(Direction::North),
        redstone_block(),
    );
    run_ticks(&mut honey, 1, 3);
    assert_eq!(
        block(&honey, honey_front.relative(Direction::East)).name,
        "minecraft:honey_block"
    );
    assert_eq!(
        block(&honey, honey_side).name,
        "minecraft:slime_block",
        "honey never sticks to slime"
    );
}

#[test]
fn immovable_blocks_reject_the_extension() {
    for name in ["minecraft:obsidian", "minecraft:bedrock", "minecraft:chest"] {
        let mut storage = flat_storage(&format!("immovable-{}", name.replace(':', "-")));
        let base = BlockPos::new(5, -60, 5);
        let front = base.relative(Direction::East);
        place(&mut storage, base, piston_state(false, Direction::East));
        place(&mut storage, front, default_state(name));
        place(
            &mut storage,
            base.relative(Direction::North),
            redstone_block(),
        );
        let base_state = block(&storage, base);
        assert!(
            !piston::piston_extended(&base_state),
            "{name} must stop extension"
        );
        assert_eq!(block(&storage, front), default_state(name));
        assert!(storage.tick_state().pistons.is_empty());
    }
}

#[test]
fn plants_in_front_are_destroyed_by_the_head() {
    let mut storage = flat_storage("plant-destroy");
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    place(&mut storage, base, piston_state(false, Direction::East));
    place(&mut storage, front, default_state("minecraft:torch"));
    place(
        &mut storage,
        base.relative(Direction::North),
        redstone_block(),
    );

    assert!(piston::piston_extended(&block(&storage, base)));
    assert!(piston::is_moving_piston(&block(&storage, front)));
    run_ticks(&mut storage, 1, 3);
    let head = block(&storage, front);
    assert_eq!(head.name, "minecraft:piston_head");
    assert_eq!(piston::piston_type(&head), "normal");
}

#[test]
fn piston_retracts_after_power_is_removed() {
    let mut storage = flat_storage("retract");
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    let power = base.relative(Direction::North);
    place(&mut storage, base, piston_state(false, Direction::East));
    place(&mut storage, power, redstone_block());
    run_ticks(&mut storage, 1, 3);
    assert!(piston::piston_extended(&block(&storage, base)));
    assert_eq!(block(&storage, front).name, "minecraft:piston_head");

    place(&mut storage, power, air());
    assert!(piston::is_moving_piston(&block(&storage, base)));
    assert_eq!(block(&storage, front), air());
    run_ticks(&mut storage, 4, 6);
    let retracted = block(&storage, base);
    assert!(piston::is_piston(&retracted));
    assert!(!piston::piston_extended(&retracted));
    assert_eq!(block(&storage, front), air());
    assert!(storage.tick_state().pistons.is_empty());
}

#[test]
fn removing_the_base_removes_the_head_through_shape_updates() {
    let mut storage = flat_storage("head-orphaned");
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    place(&mut storage, base, piston_state(false, Direction::East));
    place(
        &mut storage,
        base.relative(Direction::North),
        redstone_block(),
    );
    run_ticks(&mut storage, 1, 3);
    assert_eq!(block(&storage, front).name, "minecraft:piston_head");

    place(&mut storage, base, air());
    assert_eq!(
        block(&storage, front),
        air(),
        "the head is removed by the queued shape update"
    );
}

fn deterministic_scenario(name: &str) -> Vec<(BlockPos, BlockState)> {
    let mut storage = flat_storage(name);
    let base = BlockPos::new(5, -60, 5);
    let front = base.relative(Direction::East);
    place(&mut storage, base, piston_state(false, Direction::East));
    place(&mut storage, front, default_state("minecraft:slime_block"));
    place(&mut storage, base.relative_n(Direction::East, 2), stone());
    place(&mut storage, front.relative(Direction::South), stone());
    place(
        &mut storage,
        base.relative(Direction::North),
        redstone_block(),
    );
    run_ticks(&mut storage, 1, 3);
    place(&mut storage, base.relative(Direction::North), air());
    run_ticks(&mut storage, 4, 6);

    let mut snapshot = Vec::new();
    for x in 3..=10 {
        for y in -61..=-58 {
            for z in 3..=8 {
                let pos = BlockPos::new(x, y, z);
                snapshot.push((pos, block(&storage, pos)));
            }
        }
    }
    snapshot
}

#[test]
fn update_order_and_final_state_are_deterministic() {
    let first = deterministic_scenario("deterministic-a");
    let second = deterministic_scenario("deterministic-b");
    assert_eq!(first, second);
}
