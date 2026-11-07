//! Focused tests for lamps, powered openables, daylight signal, and note pitch.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::redstone::{self, daylight_signal, neighbor_power, note_frequency};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-redstone-extra-{}-{}-{safe}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn flat_storage(name: &str) -> WorldStorage {
    let root = temp_dir(name);
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
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

fn tick_until_settled(storage: &mut WorldStorage) {
    for tick_number in 1..=4 {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

#[test]
fn redstone_lamp_lights_and_unlights_with_neighbour_power() {
    let mut storage = flat_storage("lamp");
    let lamp_pos = BlockPos::new(0, -60, 0);
    place(&mut storage, lamp_pos, state("minecraft:redstone_lamp"));
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("false")
    );

    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:redstone_block"),
    );
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("true")
    );
    assert!(neighbor_power(&storage, lamp_pos) > 0);

    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:air"),
    );
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("false")
    );
}

#[test]
fn doors_trapdoors_and_fence_gates_open_while_powered() {
    for name in [
        "minecraft:oak_door",
        "minecraft:oak_trapdoor",
        "minecraft:oak_fence_gate",
    ] {
        let mut storage = flat_storage(name);
        let pos = BlockPos::new(0, -60, 0);
        place(&mut storage, pos, state(name));
        place(
            &mut storage,
            BlockPos::new(1, -60, 0),
            state("minecraft:redstone_block"),
        );
        tick_until_settled(&mut storage);
        let open = block(&storage, pos);
        assert_eq!(redstone::property(&open, "open"), Some("true"), "{name}");
        assert_eq!(redstone::property(&open, "powered"), Some("true"), "{name}");

        place(
            &mut storage,
            BlockPos::new(1, -60, 0),
            state("minecraft:air"),
        );
        tick_until_settled(&mut storage);
        let closed = block(&storage, pos);
        assert_eq!(redstone::property(&closed, "open"), Some("false"), "{name}");
    }
}

#[test]
fn daylight_signal_matches_the_vanilla_scale() {
    assert_eq!(daylight_signal(0, false), 0);
    assert_eq!(daylight_signal(15, false), 15);
    assert_eq!(daylight_signal(7, false), 7);
    assert_eq!(daylight_signal(0, true), 15);
    assert_eq!(daylight_signal(15, true), 0);
    assert_eq!(daylight_signal(7, true), 8);
}

#[test]
fn note_frequency_uses_the_vanilla_octave_curve() {
    assert!((note_frequency(0) - 0.5).abs() < 1e-5);
    assert!((note_frequency(12) - 1.0).abs() < 1e-5);
    assert!((note_frequency(24) - 2.0).abs() < 1e-5);
}

#[test]
fn neighbours_can_supply_power_directly() {
    let mut storage = flat_storage("power");
    let pos = BlockPos::new(0, -60, 0);
    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:redstone_block"),
    );
    assert_eq!(neighbor_power(&storage, pos), 15);
}
