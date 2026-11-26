//! Deterministic block-change and client-packet output for tripwire hook
//! connection and signal-chain teardown.
//!
//! Kept in its own test binary because `register_session` is a process-global
//! broadcast sink; a single test here means no other test shares the channel.

use folia_core::anvil::Compression;
use folia_core::block_update::{self, CLIENTBOUND_BLOCK_UPDATE, set_block};
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::protocol::PacketReader;
use folia_core::redstone::{self, PlateOccupancy};
use folia_core::registry::block_state_id;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-tripwire-packets-{}-{}-{name}",
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

fn state(name: &str) -> BlockState {
    folia_core::registry::default_block_state(name)
        .unwrap_or_else(|| panic!("missing block state {name}"))
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

fn place(storage: &mut WorldStorage, pos: BlockPos, block: BlockState) {
    set_block(storage, pos.x(), pos.y(), pos.z(), block)
        .unwrap()
        .expect("placement changed the block");
}

fn drain(receiver: &Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    while let Ok(packet) = receiver.try_recv() {
        let mut reader = PacketReader::new(&packet);
        if reader
            .read_varint()
            .is_ok_and(|id| id == CLIENTBOUND_BLOCK_UPDATE)
        {
            packets.push(packet);
        }
    }
    packets
}

fn decode(packet: &[u8]) -> (i64, i32) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_BLOCK_UPDATE);
    let pos = reader.read_i64().unwrap();
    let state_id = reader.read_varint().unwrap();
    assert!(reader.is_empty());
    (pos, state_id)
}

/// Builds a powered chain, then breaks a wire in the middle. Returns the
/// block-update packets emitted by the teardown in broadcast order.
fn break_chain(name: &str, receiver: &Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut storage = flat_storage(name);
    let left = BlockPos::new(0, -60, 0);
    let middle = BlockPos::new(2, -60, 0);
    let right = BlockPos::new(5, -60, 0);
    place(&mut storage, left, hook("east"));
    for i in 1..=4 {
        place(&mut storage, BlockPos::new(i, -60, 0), wire(false));
    }
    place(&mut storage, right, hook("west"));
    for tick_number in 1..=4 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }

    redstone::set_tripwire_occupancy(
        &mut storage,
        middle,
        PlateOccupancy {
            entities: 1,
            living: 1,
        },
    )
    .unwrap();
    for tick_number in 5..=8 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(
        redstone::property(
            &storage
                .block_state_at(left.x(), left.y(), left.z())
                .unwrap(),
            "powered"
        ),
        Some("true")
    );
    drain(receiver);

    let change = set_block(
        &mut storage,
        middle.x(),
        middle.y(),
        middle.z(),
        state("minecraft:air"),
    )
    .unwrap()
    .expect("wire break changed the block");
    block_update::broadcast_change(&change);
    for tick_number in 9..=24 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    drain(receiver)
}

#[test]
fn tripwire_teardown_emits_deterministic_block_changes_and_packets() {
    let receiver = block_update::register_session();
    let first = break_chain("determinism-a", &receiver);
    let second = break_chain("determinism-b", &receiver);
    assert_eq!(
        first, second,
        "two identical chains must emit identical packets"
    );
    assert!(
        first.len() >= 3,
        "the broken wire and both hooks must be broadcast"
    );

    let decoded: Vec<(BlockPos, i32)> = first
        .iter()
        .map(|packet| {
            let (packed, id) = decode(packet);
            (BlockPos::from_long(packed), id)
        })
        .collect();
    let air_id = block_state_id(&state("minecraft:air"));
    let middle = BlockPos::new(2, -60, 0);
    assert_eq!(
        decoded.last().copied().unwrap(),
        (middle, air_id),
        "the wire break is broadcast after the neighbour-driven updates"
    );

    let left = BlockPos::new(0, -60, 0);
    let right = BlockPos::new(5, -60, 0);
    let positions: Vec<BlockPos> = decoded.iter().map(|(pos, _)| *pos).collect();
    assert_eq!(
        positions,
        vec![
            left,
            BlockPos::new(1, -60, 0),
            right,
            BlockPos::new(3, -60, 0),
            middle,
        ],
        "hook, wire-shape, and break packets follow vanilla neighbour order"
    );
    let hook_updates: Vec<BlockPos> = positions
        .iter()
        .copied()
        .filter(|pos| *pos == left || *pos == right)
        .collect();
    assert_eq!(hook_updates, vec![left, right]);
}
