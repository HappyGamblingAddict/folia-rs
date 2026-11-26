//! Sapling stage advance and tree-feature growth on random ticks.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::registry::default_block_state;
use folia_core::saplings;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::time::{SystemTime, UNIX_EPOCH};

fn storage(name: &str) -> WorldStorage {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-saplings-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.set_tick_seed(7);
    storage
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

fn state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn sapling_pos() -> BlockPos {
    BlockPos::new(4, -60, 4)
}

fn planted(label: &str, block_name: &str) -> WorldStorage {
    let mut storage = storage(label);
    place(&mut storage, sapling_pos(), state(block_name));
    storage
}

fn tree_log_count(storage: &WorldStorage, name: &str) -> usize {
    let mut count = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in -61..=-45 {
                let pos = BlockPos::new(x, y, z);
                if storage
                    .block_state_at(pos.x(), pos.y(), pos.z())
                    .is_some_and(|state| state.name == name)
                {
                    count += 1;
                }
            }
        }
    }
    count
}

fn grow(storage: &mut WorldStorage, pos: BlockPos, max_ticks: u64) -> bool {
    let mut saw_stage_one = false;
    for tick_number in 1..=max_ticks {
        tick::random_tick_at(storage, tick_number, pos).unwrap();
        let current = block(storage, pos);
        if !saplings::is_sapling(&current) {
            return saw_stage_one;
        }
        if saplings::stage(&current) == 1 {
            saw_stage_one = true;
        }
    }
    false
}

#[test]
fn oak_sapling_advances_stage_then_grows_a_tree() {
    let mut storage = planted("stage", "minecraft:oak_sapling");
    assert_eq!(saplings::stage(&block(&storage, sapling_pos())), 0);
    assert!(grow(&mut storage, sapling_pos(), 256));
    assert!(
        tree_log_count(&storage, "minecraft:oak_log") > 0,
        "oak tree wrote no logs"
    );
    assert!(tree_log_count(&storage, "minecraft:oak_leaves") > 0);
}

#[test]
fn every_species_routes_to_its_vanilla_tree_feature() {
    for (sapling, log, leaf) in [
        (
            "minecraft:oak_sapling",
            "minecraft:oak_log",
            "minecraft:oak_leaves",
        ),
        (
            "minecraft:spruce_sapling",
            "minecraft:spruce_log",
            "minecraft:spruce_leaves",
        ),
        (
            "minecraft:birch_sapling",
            "minecraft:birch_log",
            "minecraft:birch_leaves",
        ),
        (
            "minecraft:jungle_sapling",
            "minecraft:jungle_log",
            "minecraft:jungle_leaves",
        ),
        (
            "minecraft:acacia_sapling",
            "minecraft:acacia_log",
            "minecraft:acacia_leaves",
        ),
        (
            "minecraft:cherry_sapling",
            "minecraft:cherry_log",
            "minecraft:cherry_leaves",
        ),
        (
            "minecraft:pale_oak_sapling",
            "minecraft:pale_oak_log",
            "minecraft:pale_oak_leaves",
        ),
        (
            "minecraft:mangrove_propagule",
            "minecraft:mangrove_log",
            "minecraft:mangrove_leaves",
        ),
    ] {
        let mut storage = planted(sapling, sapling);
        if sapling == "minecraft:mangrove_propagule" {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    place(
                        &mut storage,
                        sapling_pos().below().offset(dx, 0, dz),
                        state("minecraft:mud"),
                    );
                }
            }
        }
        grow(&mut storage, sapling_pos(), 320);
        assert!(
            tree_log_count(&storage, log) > 0,
            "{sapling} did not place any {log}"
        );
        assert!(
            tree_log_count(&storage, leaf) > 0,
            "{sapling} did not place any {leaf}"
        );
    }
}

#[test]
fn dark_oak_needs_a_two_by_two_square() {
    let mut single = planted("dark-oak-single", "minecraft:dark_oak_sapling");
    grow(&mut single, sapling_pos(), 320);
    assert_eq!(tree_log_count(&single, "minecraft:dark_oak_log"), 0);

    let mut square = storage("dark-oak-square");
    let origin = sapling_pos();
    for dx in 0..=1 {
        for dz in 0..=1 {
            place(
                &mut square,
                origin.offset(dx, 0, dz),
                state("minecraft:dark_oak_sapling"),
            );
        }
    }
    grow(&mut square, origin, 320);
    assert!(
        tree_log_count(&square, "minecraft:dark_oak_log") > 0,
        "a 2x2 dark-oak square did not grow"
    );
}

#[test]
fn saplings_do_not_grow_without_valid_soil() {
    let mut storage = storage("invalid-soil");
    let pos = sapling_pos();
    place(&mut storage, pos.below(), state("minecraft:stone"));
    place(&mut storage, pos, state("minecraft:oak_sapling"));
    for tick_number in 1..=160 {
        tick::random_tick_at(&mut storage, tick_number, pos).unwrap();
    }
    assert_eq!(tree_log_count(&storage, "minecraft:oak_log"), 0);
}

#[test]
fn covered_saplings_stay_at_stage_zero() {
    let mut storage = planted("dark-sapling", "minecraft:oak_sapling");
    let pos = sapling_pos();
    place(&mut storage, pos.above(), state("minecraft:stone"));
    for tick_number in 1..=160 {
        tick::random_tick_at(&mut storage, tick_number, pos).unwrap();
    }
    assert_eq!(saplings::stage(&block(&storage, pos)), 0);
    assert_eq!(tree_log_count(&storage, "minecraft:oak_log"), 0);
}

#[test]
fn sapling_growth_is_deterministic_for_a_seed() {
    let mut first = planted("determinism-a", "minecraft:oak_sapling");
    let mut second = planted("determinism-b", "minecraft:oak_sapling");
    for tick_number in 1..=320 {
        let left = tick::random_tick_at(&mut first, tick_number, sapling_pos()).unwrap();
        let right = tick::random_tick_at(&mut second, tick_number, sapling_pos()).unwrap();
        assert_eq!(left, right, "tick {tick_number} disagreed");
        assert_eq!(
            block(&first, sapling_pos()),
            block(&second, sapling_pos()),
            "state diverged at tick {tick_number}"
        );
    }
    assert_eq!(
        tree_log_count(&first, "minecraft:oak_log"),
        tree_log_count(&second, "minecraft:oak_log")
    );
}

#[test]
fn unsupported_sapling_species_are_reported_not_silently_dropped() {
    let mut storage = storage("azalea");
    let pos = sapling_pos();
    place(&mut storage, pos, state("minecraft:azalea"));
    assert!(!tick::random_tick_at(&mut storage, 1, pos).unwrap());
    assert!(
        storage
            .tick_state()
            .unsupported_random_ticks
            .contains("minecraft:azalea")
    );
}

#[test]
fn behavior_table_wires_sapling_overrides() {
    let table =
        tick::BehaviorTable::vanilla().with("minecraft:oak_sapling", &saplings::SAPLING_BEHAVIOR);
    let sapling = state("minecraft:oak_sapling");
    assert!(table.get(&sapling).is_randomly_ticking(&sapling));
    assert!(
        !table
            .get(&state("minecraft:stone"))
            .is_randomly_ticking(&state("minecraft:stone"))
    );
}
