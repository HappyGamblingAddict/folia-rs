//! Focused end-to-end tests for scheduled water/lava flow and the redstone
//! foundation. These drive the real `tick_world` loop over persisted chunks.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::fluid::{self, FluidKind};
use folia_core::math::BlockPos;
use folia_core::redstone;
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-fluids-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn solid(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn air() -> BlockState {
    solid("minecraft:air")
}

fn water(level: u8) -> BlockState {
    BlockState {
        name: "minecraft:water".into(),
        properties: vec![("level".into(), level.to_string())],
    }
}

fn lava(level: u8) -> BlockState {
    BlockState {
        name: "minecraft:lava".into(),
        properties: vec![("level".into(), level.to_string())],
    }
}

/// A flat stone floor at y=-61 over the flat fixture's dirt/bedrock column.
fn floor_chunk() -> Chunk {
    let mut chunk = flat_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x, -61, z, solid("minecraft:stone"));
        }
    }
    chunk
}

fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
    storage
}

fn floor_storage(name: &str) -> WorldStorage {
    storage_with_chunk(name, floor_chunk())
}

fn place(storage: &mut WorldStorage, x: i32, y: i32, z: i32, state: BlockState) {
    block_update::set_block(storage, x, y, z, state)
        .unwrap()
        .expect("placement changed the block");
}

fn block(storage: &WorldStorage, x: i32, y: i32, z: i32) -> BlockState {
    storage
        .block_state_at(x, y, z)
        .expect("block is in a cached chunk")
}

fn block_name(storage: &WorldStorage, x: i32, y: i32, z: i32) -> String {
    block(storage, x, y, z).name
}

fn level(storage: &WorldStorage, x: i32, y: i32, z: i32) -> Option<u8> {
    let state = block(storage, x, y, z);
    if state.name != "minecraft:water" && state.name != "minecraft:lava" {
        return None;
    }
    fluid::property(&state, "level").and_then(|value| value.parse::<u8>().ok())
}

fn run_ticks(storage: &mut WorldStorage, from: u64, to: u64) {
    for tick_number in from..=to {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

#[test]
fn water_spreads_to_a_hole_and_falls_down() {
    let mut storage = floor_storage("water-hole");
    for y in [-61, -62, -63] {
        place(&mut storage, 5, y, 8, air());
    }
    place(&mut storage, 2, -60, 8, water(0));

    run_ticks(&mut storage, 1, 200);

    assert_eq!(
        block_name(&storage, 5, -63, 8),
        "minecraft:water",
        "water flowed east toward the hole and fell to the bedrock"
    );
    assert_eq!(
        block_name(&storage, 5, -61, 8),
        "minecraft:water",
        "the falling column is full"
    );
}

#[test]
fn water_stops_after_seven_flowing_levels() {
    let mut storage = floor_storage("water-levels");
    place(&mut storage, 8, -60, 8, water(0));

    run_ticks(&mut storage, 1, 4);
    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:air",
        "water waits for its 5-tick delay"
    );

    run_ticks(&mut storage, 5, 200);

    for distance in 1..=7u8 {
        let x = 8 + i32::from(distance);
        assert_eq!(
            block_name(&storage, x, -60, 8),
            "minecraft:water",
            "water reaches level {distance}"
        );
        assert_eq!(
            level(&storage, x, -60, 8),
            Some(distance),
            "flowing level decays by one per block"
        );
    }
    assert_eq!(
        block_name(&storage, 15, -60, 9),
        "minecraft:air",
        "an 8-block Manhattan distance is out of water's reach"
    );
}

#[test]
fn lava_waits_thirty_ticks_before_spreading() {
    let mut storage = floor_storage("lava-slow");
    place(&mut storage, 8, -60, 8, lava(0));

    run_ticks(&mut storage, 1, 29);
    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:air",
        "lava's tick delay is 30, not water's 5"
    );

    tick::tick_world(&mut storage, 30).unwrap();
    assert_eq!(block_name(&storage, 9, -60, 8), "minecraft:lava");
    assert_eq!(
        level(&storage, 9, -60, 8),
        Some(2),
        "lava drops two levels per block"
    );
}

#[test]
fn removing_the_source_drains_flowing_water() {
    let mut storage = floor_storage("water-drain");
    place(&mut storage, 8, -60, 8, water(0));
    run_ticks(&mut storage, 1, 120);
    assert_eq!(block_name(&storage, 9, -60, 8), "minecraft:water");
    assert_eq!(block_name(&storage, 14, -60, 8), "minecraft:water");

    place(&mut storage, 8, -60, 8, air());
    run_ticks(&mut storage, 121, 600);

    let remaining = (0..16)
        .flat_map(|x| (0..16).map(move |z| (x, z)))
        .filter(|&(x, z)| block_name(&storage, x, -60, z) == "minecraft:water")
        .count();
    assert_eq!(remaining, 0, "no flowing water survives without a source");
    assert!(
        storage.tick_state().scheduled.is_empty(),
        "the drain finished its scheduled ticks"
    );
}

#[test]
fn water_fills_a_waterlogged_block_without_replacing_it() {
    let mut storage = floor_storage("waterlogged");
    let slab = default_block_state("minecraft:oak_slab").expect("oak slab state");
    assert_eq!(fluid::property(&slab, "waterlogged"), Some("false"));
    place(&mut storage, 7, -60, 7, water(0));
    place(&mut storage, 8, -60, 7, slab);

    run_ticks(&mut storage, 1, 20);

    let filled = block(&storage, 8, -60, 7);
    assert_eq!(filled.name, "minecraft:oak_slab");
    assert_eq!(fluid::property(&filled, "waterlogged"), Some("true"));
    assert_eq!(
        fluid::fluid_state(&filled).map(|state| state.kind),
        Some(FluidKind::Water)
    );
}

#[test]
fn two_sources_convert_a_flowing_block_to_a_source() {
    let mut storage = floor_storage("water-conversion");
    place(&mut storage, 7, -60, 7, water(0));
    place(&mut storage, 9, -60, 7, water(0));
    place(&mut storage, 8, -60, 7, water(7));

    run_ticks(&mut storage, 1, 20);

    assert_eq!(
        level(&storage, 8, -60, 7),
        Some(0),
        "two adjacent sources plus a solid floor convert the flow to a source"
    );
}

#[test]
fn neighbour_change_schedules_the_source_conversion() {
    let mut storage = floor_storage("water-neighbour-update");
    place(&mut storage, 7, -60, 8, water(0));
    run_ticks(&mut storage, 1, 5);

    let flow = BlockPos::new(8, -60, 8);
    assert_eq!(
        level(&storage, 8, -60, 8),
        Some(1),
        "the source spread one flowing level east"
    );

    // Drop the tick queued when the flowing block was created, so the next
    // scheduled entry can only come from the neighbour change below.
    assert_eq!(storage.tick_state_mut().scheduled.cancel(flow), 1);
    assert!(
        !storage
            .tick_state()
            .scheduled
            .has_scheduled_tick(flow, "minecraft:water")
    );

    place(&mut storage, 9, -60, 8, water(0));
    assert!(
        storage
            .tick_state()
            .scheduled
            .has_scheduled_tick(flow, "minecraft:water"),
        "the second source's neighbour update queues the flowing block"
    );

    run_ticks(&mut storage, 6, 10);
    assert_eq!(
        level(&storage, 8, -60, 8),
        Some(0),
        "two adjacent sources plus a solid floor convert the flow to a source"
    );
}

#[test]
fn fluid_and_redstone_ticks_are_deterministic() {
    let mut first = floor_storage("determinism-a");
    let mut second = floor_storage("determinism-b");

    for storage in [&mut first, &mut second] {
        place(storage, 8, -60, 8, water(0));
        place(
            storage,
            1,
            -60,
            1,
            default_block_state("minecraft:lever").unwrap(),
        );
        for x in 2..=5 {
            place(storage, x, -60, 1, redstone::wire_state(0));
        }
        assert!(redstone::toggle_lever(storage, BlockPos::new(1, -60, 1)).unwrap());
    }

    for tick_number in 1..=80 {
        let first_report = tick::tick_world(&mut first, tick_number).unwrap();
        let second_report = tick::tick_world(&mut second, tick_number).unwrap();
        assert_eq!(first_report, second_report, "tick {tick_number}");
        assert_eq!(
            first.tick_state().last_neighbor_updates,
            second.tick_state().last_neighbor_updates,
            "neighbour queue order at tick {tick_number}"
        );

        for x in 2..=5 {
            assert_eq!(
                redstone::wire_power(&block(&first, x, -60, 1)),
                redstone::wire_power(&block(&second, x, -60, 1))
            );
        }
    }

    assert_eq!(
        first.load_chunk(0, 0).unwrap().unwrap(),
        second.load_chunk(0, 0).unwrap().unwrap(),
        "identical inputs produce identical chunks"
    );
}
