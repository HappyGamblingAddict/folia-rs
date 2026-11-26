//! Leaf distance and random-tick decay.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::leaves;
use folia_core::math::BlockPos;
use folia_core::random::LegacyRandomSource;
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use folia_core::worldgen::feature::{FeatureData, WorldGenRegion};
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

fn tree_chunk() -> Chunk {
    let mut chunk = Chunk {
        status: folia_core::chunk::ChunkStatus::Full,
        light_present: false,
        x: 0,
        z: 0,
        sections: Vec::new(),
    };
    for x in 0..16 {
        for z in 0..16 {
            for y in -64..63 {
                chunk.set_block_state(x, y, z, state("minecraft:stone"));
            }
            chunk.set_block_state(x, 63, z, state("minecraft:grass_block"));
        }
    }
    chunk
}

fn leaves_in(chunk: &Chunk) -> Vec<(BlockPos, BlockState)> {
    let mut found = Vec::new();
    for section in &chunk.sections {
        for index in 0..section.block_states.indices.len() {
            let state = &section.block_states.palette[section.block_states.indices[index] as usize];
            if !leaves::is_leaves(state) {
                continue;
            }
            let local_y = index / 256;
            let remainder = index % 256;
            let local_z = remainder / 16;
            let local_x = remainder % 16;
            found.push((
                BlockPos::new(
                    chunk.x * 16 + local_x as i32,
                    section.y as i32 * 16 + local_y as i32,
                    chunk.z * 16 + local_z as i32,
                ),
                state.clone(),
            ));
        }
    }
    found
}

fn place_oak(chunk: &mut Chunk) {
    let data = FeatureData::vanilla();
    let feature = data
        .configured("minecraft:oak")
        .expect("shipped oak feature");
    let mut region = WorldGenRegion::new(chunk, -64, 384);
    let mut random = LegacyRandomSource::new(7);
    assert!(
        feature
            .place(&mut region, &mut random, BlockPos::new(8, 64, 8))
            .expect("oak placement is supported"),
        "oak feature did not place"
    );
}

#[test]
fn generated_tree_leaves_carry_registered_distance_states() {
    let mut chunk = tree_chunk();
    place_oak(&mut chunk);
    let leaves = leaves_in(&chunk);
    assert!(!leaves.is_empty(), "the tree placed no leaves");
    let logs = {
        let mut logs = Vec::new();
        for section in &chunk.sections {
            for index in 0..section.block_states.indices.len() {
                let state =
                    &section.block_states.palette[section.block_states.indices[index] as usize];
                if state.name == "minecraft:oak_log" {
                    let local_y = index / 256;
                    let remainder = index % 256;
                    logs.push(BlockPos::new(
                        (remainder % 16) as i32,
                        section.y as i32 * 16 + local_y as i32,
                        (remainder / 16) as i32,
                    ));
                }
            }
        }
        logs
    };
    let mut next_to_log = 0;
    for (_, state) in &leaves {
        let distance = leaves::distance_of(state);
        assert!((1..=7).contains(&distance), "bad leaf distance {distance}");
        assert_eq!(
            leaves::with_distance(state, distance),
            *state,
            "a generated leaf state must be a registered state"
        );
    }
    for (pos, state) in &leaves {
        if logs.iter().any(|log| {
            let delta = (log.x() - pos.x(), log.y() - pos.y(), log.z() - pos.z());
            matches!(
                delta,
                (-1, 0, 0) | (1, 0, 0) | (0, -1, 0) | (0, 1, 0) | (0, 0, -1) | (0, 0, 1)
            )
        }) {
            next_to_log += 1;
            assert_eq!(
                leaves::property(state, "distance"),
                Some("1"),
                "a leaf touching a log must be distance 1"
            );
        }
    }
    assert!(next_to_log > 0, "no leaf was placed next to a log");
}

#[test]
fn generated_tree_leaf_states_are_deterministic() {
    let mut first = tree_chunk();
    let mut second = tree_chunk();
    place_oak(&mut first);
    place_oak(&mut second);
    assert_eq!(
        leaves_in(&first),
        leaves_in(&second),
        "identical seeds must produce identical leaf states"
    );
}

#[test]
fn player_placed_persistent_leaf_keeps_its_distance_and_never_decays() {
    let mut storage = storage("placed_persistent");
    let log = BlockPos::new(0, -60, 0);
    let leaf = BlockPos::new(1, -60, 0);
    place(&mut storage, log, state("minecraft:oak_log"));
    place(
        &mut storage,
        leaf,
        leaves::persistent(state("minecraft:oak_leaves")),
    );
    assert_eq!(
        leaves::property(&block(&storage, leaf), "distance"),
        Some("1")
    );
    assert_eq!(
        leaves::property(&block(&storage, leaf), "persistent"),
        Some("true")
    );
    for tick_number in 1..=24 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(block(&storage, leaf).name, "minecraft:oak_leaves");
    assert_eq!(
        leaves::property(&block(&storage, leaf), "distance"),
        Some("1")
    );
}

#[test]
fn player_placed_non_persistent_leaf_near_a_log_survives() {
    let mut storage = storage("placed_normal");
    place(
        &mut storage,
        BlockPos::new(0, -60, 0),
        state("minecraft:oak_log"),
    );
    let leaf = BlockPos::new(1, -60, 0);
    place(&mut storage, leaf, state("minecraft:oak_leaves"));
    assert_eq!(
        leaves::property(&block(&storage, leaf), "distance"),
        Some("1")
    );
    for tick_number in 1..=16 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(block(&storage, leaf).name, "minecraft:oak_leaves");
}

fn build_chain(name: &str) -> (WorldStorage, BlockPos, Vec<BlockPos>) {
    let mut storage = storage(name);
    let log = BlockPos::new(0, -60, 0);
    let chain: Vec<BlockPos> = (1..=4).map(|x| BlockPos::new(x, -60, 0)).collect();
    place(&mut storage, log, state("minecraft:oak_log"));
    for pos in &chain {
        place(&mut storage, *pos, state("minecraft:oak_leaves"));
    }
    (storage, log, chain)
}

#[test]
fn removing_the_support_log_propagates_decay_along_the_leaf_chain() {
    let (mut storage, log, chain) = build_chain("decay_chain");
    for (offset, pos) in chain.iter().enumerate() {
        let expected = (offset as u8 + 1).to_string();
        assert_eq!(
            leaves::property(&block(&storage, *pos), "distance"),
            Some(expected.as_str()),
            "leaf {offset} should start at distance {expected}"
        );
    }

    place(&mut storage, log, state("minecraft:air"));
    let mut decayed = false;
    for tick_number in 1..=32 {
        tick::tick_world(&mut storage, tick_number).unwrap();
        if chain
            .iter()
            .all(|pos| block(&storage, *pos).name == "minecraft:air")
        {
            decayed = true;
            break;
        }
    }
    assert!(decayed, "the unsupported leaf chain did not decay");
    for pos in &chain {
        assert_eq!(block(&storage, *pos).name, "minecraft:air");
    }
}

#[test]
fn leaf_decay_is_deterministic_across_replayed_ticks() {
    let (mut first, first_log, first_chain) = build_chain("decay_determinism_a");
    let (mut second, second_log, second_chain) = build_chain("decay_determinism_b");
    place(&mut first, first_log, state("minecraft:air"));
    place(&mut second, second_log, state("minecraft:air"));
    for tick_number in 1..=12 {
        tick::tick_world(&mut first, tick_number).unwrap();
        tick::tick_world(&mut second, tick_number).unwrap();
        for (a, b) in first_chain.iter().zip(second_chain.iter()) {
            assert_eq!(
                block(&first, *a),
                block(&second, *b),
                "leaf state diverged at tick {tick_number}"
            );
        }
    }
}
