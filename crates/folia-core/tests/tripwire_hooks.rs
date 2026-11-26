//! Focused P3.10 tests for two-hook tripwire signal chains.
//!
//! Covers connection/disconnection, signal on/off, several wire lengths,
//! invalid hook orientations, and clearing the remaining hook when the chain
//! is broken. Packet ordering lives in `tripwire_hook_packets.rs` because the
//! broadcast sink is process-global.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::{BlockPos, Direction};
use folia_core::redstone::{self, PlateOccupancy, SignalReader};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-tripwire-hooks-{}-{}-{name}",
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

fn air() -> BlockState {
    state("minecraft:air")
}

fn hook(facing: &str) -> BlockState {
    redstone::with_property(&state("minecraft:tripwire_hook"), "facing", facing)
}

fn wire(powered: bool) -> BlockState {
    redstone::with_property(
        &state("minecraft:tripwire"),
        "powered",
        if powered { "true" } else { "false" },
    )
}

fn settle(storage: &mut WorldStorage) {
    let start = storage.tick_state().current_tick;
    for tick_number in start + 1..=start + 15 {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

fn prop(storage: &WorldStorage, pos: BlockPos, key: &str) -> Option<String> {
    redstone::property(&block(storage, pos), key).map(str::to_owned)
}

/// Builds a straight east-facing chain: a hook at x=0, `length` wires starting
/// at x=1, and a west-facing hook at x = length + 1.
fn chain(name: &str, length: i32) -> (WorldStorage, BlockPos, BlockPos) {
    let mut storage = flat_storage(name);
    let left = BlockPos::new(0, -60, 0);
    place(&mut storage, left, hook("east"));
    for i in 1..=length {
        place(&mut storage, BlockPos::new(i, -60, 0), wire(false));
    }
    let right = BlockPos::new(length + 1, -60, 0);
    place(&mut storage, right, hook("west"));
    (storage, left, right)
}

fn trip(entities: u32) -> PlateOccupancy {
    PlateOccupancy {
        entities,
        living: entities,
    }
}

#[test]
fn two_hooks_attach_across_multiple_wire_lengths() {
    for length in [1, 2, 3, 8] {
        let (mut storage, left, right) = chain(&format!("attach-{length}"), length);
        settle(&mut storage);
        assert_eq!(prop(&storage, left, "attached").as_deref(), Some("true"));
        assert_eq!(prop(&storage, right, "attached").as_deref(), Some("true"));
        assert_eq!(prop(&storage, left, "powered").as_deref(), Some("false"));
        assert_eq!(prop(&storage, right, "powered").as_deref(), Some("false"));
    }
}

#[test]
fn tripping_a_wire_powers_both_hooks_and_clears_on_release() {
    let (mut storage, left, right) = chain("signal-on-off", 3);
    settle(&mut storage);

    let middle = BlockPos::new(2, -60, 0);
    assert!(redstone::set_tripwire_occupancy(&mut storage, middle, trip(1)).unwrap());
    settle(&mut storage);
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("true"));
    assert_eq!(prop(&storage, right, "powered").as_deref(), Some("true"));

    let reader = SignalReader::world(&storage);
    assert_eq!(reader.direct_signal(left, Direction::East), 15);
    assert_eq!(reader.direct_signal(right, Direction::West), 15);
    assert_eq!(reader.signal(left, Direction::East), 15);

    storage.tick_state_mut().occupancy.insert(
        middle,
        PlateOccupancy {
            entities: 0,
            living: 0,
        },
    );
    settle(&mut storage);
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("false"));
    assert_eq!(prop(&storage, right, "powered").as_deref(), Some("false"));
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.direct_signal(left, Direction::East), 0);
}

#[test]
fn hook_removal_detaches_and_silences_remaining_hook() {
    let (mut storage, left, right) = chain("hook-removal", 3);
    settle(&mut storage);

    let middle = BlockPos::new(2, -60, 0);
    redstone::set_tripwire_occupancy(&mut storage, middle, trip(1)).unwrap();
    settle(&mut storage);
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("true"));

    place(&mut storage, right, air());
    settle(&mut storage);
    assert_eq!(prop(&storage, left, "attached").as_deref(), Some("false"));
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("false"));
    assert_eq!(
        SignalReader::world(&storage).direct_signal(left, Direction::East),
        0,
        "the remaining hook must stop emitting once the chain is broken"
    );
}

#[test]
fn wire_break_detaches_and_silences_both_hooks() {
    let (mut storage, left, right) = chain("wire-break", 4);
    settle(&mut storage);

    let powered_wire = BlockPos::new(3, -60, 0);
    redstone::set_tripwire_occupancy(&mut storage, powered_wire, trip(1)).unwrap();
    settle(&mut storage);
    assert_eq!(prop(&storage, right, "powered").as_deref(), Some("true"));

    // Break a wire between the left hook and the still-powered wire.
    place(&mut storage, BlockPos::new(2, -60, 0), air());
    settle(&mut storage);
    assert_eq!(prop(&storage, left, "attached").as_deref(), Some("false"));
    assert_eq!(prop(&storage, right, "attached").as_deref(), Some("false"));
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("false"));
    assert_eq!(prop(&storage, right, "powered").as_deref(), Some("false"));
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.direct_signal(left, Direction::East), 0);
    assert_eq!(reader.direct_signal(right, Direction::West), 0);
}

#[test]
fn misoriented_hook_does_not_connect_or_emit() {
    let mut storage = flat_storage("misoriented");
    let left = BlockPos::new(0, -60, 0);
    let right = BlockPos::new(4, -60, 0);
    // The left hook faces away from the wire run, so it must not attach or
    // emit even though a valid chain sits in front of it.
    place(&mut storage, left, hook("west"));
    for i in 1..=3 {
        place(&mut storage, BlockPos::new(i, -60, 0), wire(false));
    }
    place(&mut storage, right, hook("west"));
    settle(&mut storage);

    assert_eq!(prop(&storage, left, "attached").as_deref(), Some("false"));
    assert_eq!(
        SignalReader::world(&storage).direct_signal(left, Direction::West),
        0
    );
    // The wire never forms a connection property toward a hook facing away.
    assert_eq!(
        prop(&storage, BlockPos::new(1, -60, 0), "west").as_deref(),
        Some("false")
    );
}

#[test]
fn vertically_facing_hook_does_not_attach() {
    let mut storage = flat_storage("vertical-facing");
    let left = BlockPos::new(0, -60, 0);
    place(&mut storage, left, hook("up"));
    for i in 1..=3 {
        place(&mut storage, BlockPos::new(i, -60, 0), wire(false));
    }
    place(&mut storage, BlockPos::new(4, -60, 0), hook("west"));
    settle(&mut storage);

    assert_eq!(prop(&storage, left, "attached").as_deref(), Some("false"));
    assert_eq!(prop(&storage, left, "powered").as_deref(), Some("false"));
    assert_eq!(
        SignalReader::world(&storage).direct_signal(left, Direction::Up),
        0
    );
}
