//! Focused P3.10 tests for lava random ticks: metadata-driven ignition,
//! five-face water quenching, deterministic RNG, and enclosed lava.
//!
//! Blocks are written straight into the fixture chunk so no neighbour update
//! or scheduled fluid tick can pre-empt the random tick under test.

use folia_core::anvil::Compression;
use folia_core::block_behaviour;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::fluid;
use folia_core::math::Direction;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const LAVA: (i32, i32, i32) = (8, -60, 8);

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-lava-fire-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn named(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn fluid(name: &str, level: u8) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![("level".into(), level.to_string())],
    }
}

fn chunk_with(blocks: &[(i32, i32, i32, BlockState)]) -> Chunk {
    let mut chunk = flat_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x, -61, z, named("minecraft:stone"));
        }
    }
    for (x, y, z, state) in blocks {
        chunk.set_block_state(*x as usize, *y, *z as usize, state.clone());
    }
    chunk
}

fn storage_with(name: &str, blocks: &[(i32, i32, i32, BlockState)], seed: i64) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage
        .save_chunk(&chunk_with(blocks), Compression::Zlib)
        .expect("save fixture chunk");
    storage.flush().expect("flush fixture chunk");
    storage.set_tick_seed(seed);
    storage
}

fn floor_ring(name: &str, floor: &str, seed: i64) -> WorldStorage {
    let mut blocks = vec![(LAVA.0, LAVA.1, LAVA.2, fluid("minecraft:lava", 0))];
    for x in LAVA.0 - 1..=LAVA.0 + 1 {
        for z in LAVA.2 - 1..=LAVA.2 + 1 {
            if (x, z) != (LAVA.0, LAVA.2) {
                blocks.push((x, LAVA.1, z, named(floor)));
            }
        }
    }
    storage_with(name, &blocks, seed)
}

fn block_at(storage: &WorldStorage, x: i32, y: i32, z: i32) -> BlockState {
    storage
        .block_state_at(x, y, z)
        .expect("block is in the cached fixture chunk")
}

fn fire_positions(storage: &WorldStorage) -> Vec<(i32, i32, i32)> {
    let mut positions = Vec::new();
    for x in LAVA.0 - 4..=LAVA.0 + 4 {
        for y in LAVA.1 - 1..=LAVA.1 + 3 {
            for z in LAVA.2 - 4..=LAVA.2 + 4 {
                if block_at(storage, x, y, z).name == "minecraft:fire" {
                    positions.push((x, y, z));
                }
            }
        }
    }
    positions
}

type AreaRow = (i32, i32, i32, String, Vec<(String, String)>);

fn area_snapshot(storage: &WorldStorage) -> Vec<AreaRow> {
    let mut rows = Vec::new();
    for x in LAVA.0 - 3..=LAVA.0 + 3 {
        for y in LAVA.1 - 1..=LAVA.1 + 3 {
            for z in LAVA.2 - 3..=LAVA.2 + 3 {
                let state = block_at(storage, x, y, z);
                rows.push((x, y, z, state.name, state.properties));
            }
        }
    }
    rows
}

fn schedule_snapshot(storage: &WorldStorage) -> Vec<(i32, i32, i32, String, u64, i64)> {
    storage
        .scheduled_ticks_in_chunk(0, 0)
        .iter()
        .map(|scheduled| {
            (
                scheduled.pos.x(),
                scheduled.pos.y(),
                scheduled.pos.z(),
                scheduled.block.clone(),
                scheduled.trigger_tick,
                scheduled.sub_tick_order,
            )
        })
        .collect()
}

#[test]
fn shipped_odds_decide_which_neighbours_can_ignite() {
    let planks = named("minecraft:oak_planks");
    let stone = named("minecraft:stone");
    assert_eq!(block_behaviour::fire_odds(&planks), (5, 20));
    assert_eq!(block_behaviour::fire_odds(&stone), (0, 0));
    assert!(fluid::ignited_by_lava(&planks));
    assert!(!fluid::ignited_by_lava(&stone));

    let lava_pos = folia_core::math::BlockPos::new(LAVA.0, LAVA.1, LAVA.2);
    let mut flammable = floor_ring("planks-ignite", "minecraft:oak_planks", 12345);
    let mut inert = floor_ring("stone-no-ignite", "minecraft:stone", 12345);
    for tick_number in 0..=120 {
        tick::random_tick_at(&mut flammable, tick_number, lava_pos).unwrap();
        tick::random_tick_at(&mut inert, tick_number, lava_pos).unwrap();
    }

    let fires = fire_positions(&flammable);
    assert!(!fires.is_empty(), "lava must ignite the flammable ring");
    for (x, y, z) in &fires {
        let has_fuel = Direction::ALL.iter().any(|direction| {
            let neighbour = folia_core::math::BlockPos::new(*x, *y, *z).relative(*direction);
            let state = block_at(&flammable, neighbour.x(), neighbour.y(), neighbour.z());
            fluid::ignited_by_lava(&state)
        });
        assert!(
            has_fuel,
            "fire at ({x}, {y}, {z}) has no flammable neighbour"
        );
    }
    assert!(
        fire_positions(&inert).is_empty(),
        "a stone ring must never ignite"
    );
}

#[test]
fn water_quenches_lava_on_the_five_contact_faces() {
    let lava_pos = folia_core::math::BlockPos::new(LAVA.0, LAVA.1, LAVA.2);

    for (face, dx, dy, dz) in [
        ("above", 0, 1, 0),
        ("north", 0, 0, -1),
        ("south", 0, 0, 1),
        ("west", -1, 0, 0),
        ("east", 1, 0, 0),
    ] {
        let mut source = storage_with(
            &format!("water-{face}"),
            &[
                (LAVA.0, LAVA.1, LAVA.2, fluid("minecraft:lava", 0)),
                (
                    LAVA.0 + dx,
                    LAVA.1 + dy,
                    LAVA.2 + dz,
                    fluid("minecraft:water", 0),
                ),
            ],
            1,
        );
        assert!(tick::random_tick_at(&mut source, 0, lava_pos).unwrap());
        assert_eq!(
            block_at(&source, LAVA.0, LAVA.1, LAVA.2).name,
            "minecraft:obsidian",
            "water on the {face} face must quench a lava source"
        );
        assert_eq!(
            block_at(&source, LAVA.0 + dx, LAVA.1 + dy, LAVA.2 + dz).name,
            "minecraft:water",
            "quenching must not consume the water"
        );
    }

    let mut flowing = storage_with(
        "water-above-flow",
        &[
            (LAVA.0, LAVA.1, LAVA.2, fluid("minecraft:lava", 1)),
            (LAVA.0, LAVA.1 + 1, LAVA.2, fluid("minecraft:water", 0)),
        ],
        1,
    );
    assert!(tick::random_tick_at(&mut flowing, 0, lava_pos).unwrap());
    assert_eq!(
        block_at(&flowing, LAVA.0, LAVA.1, LAVA.2).name,
        "minecraft:cobblestone"
    );

    let mut below = storage_with(
        "water-below-only",
        &[
            (LAVA.0, LAVA.1, LAVA.2, fluid("minecraft:lava", 0)),
            (LAVA.0, LAVA.1 - 1, LAVA.2, fluid("minecraft:water", 0)),
        ],
        1,
    );
    assert!(!tick::random_tick_at(&mut below, 0, lava_pos).unwrap());
    assert_eq!(
        block_at(&below, LAVA.0, LAVA.1, LAVA.2).name,
        "minecraft:lava",
        "water below is not one of the five checked liquid faces"
    );
}

#[test]
fn lava_random_ticks_are_deterministic_for_a_seed_and_tick() {
    let mut a = floor_ring("determinism-a", "minecraft:oak_planks", 12345);
    let mut b = floor_ring("determinism-b", "minecraft:oak_planks", 12345);
    let mut other = floor_ring("determinism-other-seed", "minecraft:oak_planks", 999);
    let lava_pos = folia_core::math::BlockPos::new(LAVA.0, LAVA.1, LAVA.2);

    for tick_number in 0..=40 {
        tick::random_tick_at(&mut a, tick_number, lava_pos).unwrap();
        tick::random_tick_at(&mut b, tick_number, lava_pos).unwrap();
        tick::random_tick_at(&mut other, tick_number, lava_pos).unwrap();
    }

    assert_eq!(area_snapshot(&a), area_snapshot(&b));
    assert_eq!(schedule_snapshot(&a), schedule_snapshot(&b));
    assert_ne!(
        schedule_snapshot(&a),
        schedule_snapshot(&other),
        "a different world seed must draw a different random sequence"
    );
}

#[test]
fn enclosed_lava_has_no_random_tick_effect() {
    let mut blocks = vec![(LAVA.0, LAVA.1, LAVA.2, fluid("minecraft:lava", 0))];
    for dx in -2..=2 {
        for dy in 0..=2 {
            for dz in -2..=2 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                blocks.push((
                    LAVA.0 + dx,
                    LAVA.1 + dy,
                    LAVA.2 + dz,
                    named("minecraft:stone"),
                ));
            }
        }
    }
    let mut storage = storage_with("enclosed-lava", &blocks, 12345);
    let before = area_snapshot(&storage);
    let lava_pos = folia_core::math::BlockPos::new(LAVA.0, LAVA.1, LAVA.2);

    for tick_number in 0..=64 {
        assert!(!tick::random_tick_at(&mut storage, tick_number, lava_pos).unwrap());
    }

    assert_eq!(area_snapshot(&storage), before);
    assert!(fire_positions(&storage).is_empty());
    assert!(
        schedule_snapshot(&storage).is_empty(),
        "enclosed lava must not schedule fire ticks"
    );
    assert_eq!(
        block_at(&storage, LAVA.0, LAVA.1, LAVA.2).name,
        "minecraft:lava"
    );
}
