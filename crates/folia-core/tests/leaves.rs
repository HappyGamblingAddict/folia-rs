//! Leaf distance and random-tick decay.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::leaves;
use folia_core::math::BlockPos;
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-leaves-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage(name: &str) -> WorldStorage {
    let root = temp_dir(name);
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.set_tick_seed(42);
    storage.tick_state_mut().random.random_tick_speed = 4_096;
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

#[test]
fn leaves_adjacent_to_a_log_get_distance_one() {
    let mut storage = storage("log");
    let log = BlockPos::new(0, -60, 0);
    let leaf = BlockPos::new(1, -60, 0);
    place(&mut storage, log, state("minecraft:oak_log"));
    place(&mut storage, leaf, state("minecraft:oak_leaves"));
    assert_eq!(
        leaves::property(&block(&storage, leaf), "distance"),
        Some("1")
    );
}

#[test]
fn leaf_chains_increment_distance() {
    let mut storage = storage("chain");
    place(
        &mut storage,
        BlockPos::new(0, -60, 0),
        state("minecraft:oak_log"),
    );
    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:oak_leaves"),
    );
    place(
        &mut storage,
        BlockPos::new(2, -60, 0),
        state("minecraft:oak_leaves"),
    );
    assert_eq!(
        leaves::property(&block(&storage, BlockPos::new(2, -60, 0)), "distance"),
        Some("2")
    );
}

#[test]
fn isolated_non_persistent_leaves_decay_on_a_random_tick() {
    let mut storage = storage("decay");
    let leaf = BlockPos::new(5, -60, 5);
    place(&mut storage, leaf, state("minecraft:oak_leaves"));
    assert_eq!(
        leaves::property(&block(&storage, leaf), "distance"),
        Some("7")
    );
    tick::tick_world(&mut storage, 1).unwrap();
    assert_eq!(block(&storage, leaf).name, "minecraft:air");
}

#[test]
fn persistent_leaves_never_decay() {
    let mut storage = storage("persistent");
    let leaf = BlockPos::new(5, -60, 5);
    let mut persistent = state("minecraft:oak_leaves");
    persistent = leaves::persistent(persistent);
    place(&mut storage, leaf, persistent);
    for tick_number in 1..=20 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(block(&storage, leaf).name, "minecraft:oak_leaves");
}
