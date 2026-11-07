//! Focused tests for P3.10: update flags, neighbour updates, scheduled ticks,
//! and random ticks.

use folia_core::block_update::set_block;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::{BlockPos, Direction};
use folia_core::tick::{self, TickPriority, UpdateFlags};
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-ticks-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn grass() -> BlockState {
    BlockState {
        name: "minecraft:grass_block".into(),
        properties: vec![("snowy".into(), "false".into())],
    }
}

fn dirt() -> BlockState {
    state("minecraft:dirt")
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn air() -> BlockState {
    state("minecraft:air")
}

fn sand() -> BlockState {
    state("minecraft:sand")
}

fn short_grass() -> BlockState {
    state("minecraft:short_grass")
}

fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage
        .save_chunk(&chunk, folia_core::anvil::Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();
    storage
}

fn flat_storage(name: &str) -> WorldStorage {
    storage_with_chunk(name, flat_chunk(0, 0))
}

fn block_name(storage: &WorldStorage, x: i32, y: i32, z: i32) -> String {
    storage
        .block_state_at(x, y, z)
        .expect("block is in a cached chunk")
        .name
}

/// A surface chunk one block above the flat fixture: stone at y=63, dirt at
/// y=64, and a single grass block at (8, 64, 8).
fn soil_chunk() -> Chunk {
    let mut chunk = flat_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x, 63, z, stone());
            chunk.set_block_state(x, 64, z, dirt());
        }
    }
    chunk.set_block_state(8, 64, 8, grass());
    chunk
}

#[test]
fn block_edit_queues_six_neighbor_updates() {
    let mut storage = flat_storage("edit-neighbors");
    let change = set_block(&mut storage, 5, -50, 5, stone())
        .unwrap()
        .expect("stone placement changed the block");
    assert_eq!(change.new_state.name, "minecraft:stone");

    let updates = storage.tick_state().last_neighbor_updates.clone();
    let source = BlockPos::new(5, -50, 5);
    let expected = [
        (Direction::West, source.relative(Direction::West)),
        (Direction::East, source.relative(Direction::East)),
        (Direction::Down, source.relative(Direction::Down)),
        (Direction::Up, source.relative(Direction::Up)),
        (Direction::North, source.relative(Direction::North)),
        (Direction::South, source.relative(Direction::South)),
    ];
    assert_eq!(updates.len(), expected.len());
    for (update, (_direction, pos)) in updates.iter().zip(expected) {
        assert_eq!(update.pos, pos);
        assert_eq!(update.source, source);
        assert!(UpdateFlags::has_neighbors(update.flags));
        assert!(UpdateFlags::has_shape(update.flags));
    }
    assert!(
        storage.tick_state().neighbors.is_empty(),
        "set_block drains its neighbour queue before returning"
    );
}

#[test]
fn breaking_support_removes_the_plant_above() {
    let mut storage = flat_storage("plant-support");
    set_block(&mut storage, 2, -61, 2, dirt())
        .unwrap()
        .expect("dirt replaced the flat grass block");
    set_block(&mut storage, 2, -60, 2, short_grass())
        .unwrap()
        .expect("short grass placed on dirt");
    assert_eq!(block_name(&storage, 2, -60, 2), "minecraft:short_grass");

    set_block(&mut storage, 2, -61, 2, air())
        .unwrap()
        .expect("dirt removal changed the block");

    assert_eq!(
        block_name(&storage, 2, -60, 2),
        "minecraft:air",
        "the unsupported plant is removed by its neighbour shape update"
    );
    assert!(
        storage
            .tick_state()
            .last_neighbor_updates
            .iter()
            .any(|update| update.pos == BlockPos::new(2, -60, 2)),
        "the plant position was actually visited"
    );
    assert!(
        storage.tick_state().unsupported_neighbor_blocks.is_empty(),
        "implemented plant support is not reported as unsupported"
    );
}

#[test]
fn falling_sand_moves_down_and_lands_on_its_support() {
    let mut storage = flat_storage("falling-sand");
    set_block(&mut storage, 0, 58, 0, stone())
        .unwrap()
        .expect("stone platform");
    set_block(&mut storage, 0, 60, 0, sand())
        .unwrap()
        .expect("sand placement");

    // FallingBlock.getDelayAfterPlace is 2 ticks. The sand starts at y=60 with
    // one free block below, so it lands on the stone at y=58 on tick 2.
    let mut landed_tick = None;
    for tick in 1..=10 {
        tick::tick_world(&mut storage, tick).unwrap();
        if block_name(&storage, 0, 59, 0) == "minecraft:sand" {
            landed_tick = Some(tick);
            break;
        }
    }
    let landed_tick = landed_tick.expect("sand reached the block above the platform");
    assert_eq!(landed_tick, 2, "one scheduled move delayed by 2 ticks");
    assert_eq!(block_name(&storage, 0, 59, 0), "minecraft:sand");
    assert_eq!(block_name(&storage, 0, 60, 0), "minecraft:air");
    assert_eq!(block_name(&storage, 0, 58, 0), "minecraft:stone");

    assert!(
        storage
            .tick_state()
            .last_neighbor_updates
            .iter()
            .any(|update| update.pos == BlockPos::new(0, 58, 0)),
        "landing notifies the block below the sand"
    );
    // The move scheduled the landing check two ticks later; run it out.
    for tick in landed_tick + 1..=landed_tick + 4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    assert!(storage.tick_state().scheduled.is_empty());
}

#[test]
fn removing_the_support_reschedules_a_falling_block() {
    let mut storage = flat_storage("falling-support-removed");
    set_block(&mut storage, 0, 56, 0, stone()).unwrap().unwrap();
    set_block(&mut storage, 0, 58, 0, stone()).unwrap().unwrap();
    set_block(&mut storage, 0, 59, 0, sand()).unwrap().unwrap();
    for tick in 1..=4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    assert_eq!(block_name(&storage, 0, 59, 0), "minecraft:sand");

    // Removing the support sends a shape update to the sand above; the falling
    // behaviour schedules a fresh tick rather than moving immediately.
    set_block(&mut storage, 0, 58, 0, air()).unwrap().unwrap();
    tick::tick_world(&mut storage, 5).unwrap();
    assert_eq!(
        block_name(&storage, 0, 59, 0),
        "minecraft:sand",
        "the delay after placement is 2 ticks"
    );
    for tick in 6..=9 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    assert_eq!(block_name(&storage, 0, 57, 0), "minecraft:sand");
    assert_eq!(block_name(&storage, 0, 59, 0), "minecraft:air");
    assert_eq!(block_name(&storage, 0, 56, 0), "minecraft:stone");
}

#[test]
fn falling_sand_on_a_support_does_not_move() {
    let mut storage = flat_storage("falling-sand-supported");
    set_block(&mut storage, 0, 58, 0, stone()).unwrap().unwrap();
    set_block(&mut storage, 0, 59, 0, sand()).unwrap().unwrap();

    for tick in 1..=6 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    assert_eq!(block_name(&storage, 0, 59, 0), "minecraft:sand");
    assert_eq!(block_name(&storage, 0, 60, 0), "minecraft:air");
}

#[test]
fn scheduled_ticks_follow_priority_delay_and_cancellation() {
    let mut storage = flat_storage("scheduled");
    let pos = BlockPos::new(1, 50, 1);
    let queue = &mut storage.tick_state_mut().scheduled;

    assert!(queue.schedule_after(10, pos, "minecraft:sand", 2, TickPriority::Normal));
    assert!(!queue.schedule_after(10, pos, "minecraft:sand", 1, TickPriority::Low));
    assert!(queue.has_scheduled_tick(pos, "minecraft:sand"));
    assert!(queue.tick(11).is_empty());
    let due = queue.tick(12);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].pos, pos);
    assert_eq!(due[0].trigger_tick, 12);

    assert!(queue.schedule_after(12, pos, "minecraft:gravel", 0, TickPriority::High));
    assert_eq!(queue.cancel(pos), 1);
    assert!(queue.tick(12).is_empty());
}

#[test]
fn random_tick_sampling_is_deterministic_across_runs() {
    let chunk = soil_chunk();
    let queue = tick::RandomTickQueue::new(99);
    let first: Vec<BlockPos> = queue.sample_positions(&chunk, 42);
    let second: Vec<BlockPos> = queue.sample_positions(&chunk, 42);
    assert_eq!(first, second);
    assert_eq!(
        first.len(),
        3 * chunk.sections.len(),
        "vanilla randomTickSpeed per section"
    );
    assert_ne!(first, queue.sample_positions(&chunk, 43));
}

#[test]
fn grass_spreads_to_dirt_deterministically() {
    let first = storage_with_chunk("grass-spread-a", soil_chunk());
    let second = storage_with_chunk("grass-spread-b", soil_chunk());
    let mut first = first;
    let mut second = second;
    first.set_tick_seed(7);
    second.set_tick_seed(7);

    let grass_pos = BlockPos::new(8, 64, 8);
    for tick in 0..200 {
        tick::random_tick_at(&mut first, tick, grass_pos).unwrap();
        tick::random_tick_at(&mut second, tick, grass_pos).unwrap();
    }

    let first_chunk = first.load_chunk(0, 0).unwrap().unwrap();
    let second_chunk = second.load_chunk(0, 0).unwrap().unwrap();
    assert_eq!(
        first_chunk, second_chunk,
        "identical seeds and ticks produce identical chunks"
    );

    let spread = (7..=9)
        .flat_map(|x| (7..=9).map(move |z| (x, z)))
        .filter(|&(x, z)| {
            first_chunk
                .block_state(x, 64, z)
                .is_some_and(|state| state.name == "minecraft:grass_block")
        })
        .count();
    assert!(
        spread > 1,
        "the grass block spread to at least one adjacent dirt block"
    );
}

#[test]
fn unsupported_random_ticks_are_reported_not_silently_dropped() {
    let mut storage = flat_storage("unsupported-random");
    set_block(&mut storage, 1, -60, 1, state("minecraft:oak_sapling"))
        .unwrap()
        .unwrap();
    assert!(
        !tick::random_tick_at(&mut storage, 1, BlockPos::new(1, -60, 1)).unwrap(),
        "sapling growth is not implemented yet"
    );
    assert!(
        storage
            .tick_state()
            .unsupported_random_ticks
            .contains("minecraft:oak_sapling"),
        "the gap is recorded instead of disappearing"
    );
}

#[test]
fn repeated_world_ticks_are_deterministic() {
    let first = storage_with_chunk("repeat-a", soil_chunk());
    let second = storage_with_chunk("repeat-b", soil_chunk());
    let mut first = first;
    let mut second = second;
    first.set_tick_seed(123);
    second.set_tick_seed(123);

    for tick in 1..=12 {
        let a = tick::tick_world(&mut first, tick).unwrap();
        let b = tick::tick_world(&mut second, tick).unwrap();
        assert_eq!(a, b);
    }
    assert_eq!(
        first.load_chunk(0, 0).unwrap().unwrap(),
        second.load_chunk(0, 0).unwrap().unwrap()
    );
}
