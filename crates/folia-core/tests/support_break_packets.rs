//! Deterministic block-change and client-packet output for support loss.
//!
//! Kept in its own test binary because `register_session` is a process-global
//! broadcast sink; a single test here means no other test shares the channel.

use folia_core::anvil::Compression;
use folia_core::block_update::{self, CLIENTBOUND_BLOCK_UPDATE, set_block};
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::protocol::PacketReader;
use folia_core::registry::block_state_id;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-support-packets-{}-{}-{name}",
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

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    }
}

fn floor_torch() -> BlockState {
    BlockState {
        name: "minecraft:torch".into(),
        properties: Vec::new(),
    }
}

fn run(
    name: &str,
    receiver: &Receiver<Vec<u8>>,
) -> (folia_core::block_update::BlockChange, Vec<Vec<u8>>) {
    let mut storage = flat_storage(name);
    let torch_pos = BlockPos::new(2, -60, 2);
    set_block(
        &mut storage,
        torch_pos.x(),
        torch_pos.y(),
        torch_pos.z(),
        floor_torch(),
    )
    .unwrap()
    .expect("torch placement");
    while receiver.try_recv().is_ok() {}

    let support = torch_pos.below();
    let change = set_block(&mut storage, support.x(), support.y(), support.z(), air())
        .unwrap()
        .expect("support removal is a block change");
    block_update::broadcast_change(&change);

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
    (change, packets)
}

#[test]
fn support_removal_emits_deterministic_block_changes_and_packets() {
    let receiver = block_update::register_session();
    let (first_change, first_packets) = run("determinism-a", &receiver);
    let (second_change, second_packets) = run("determinism-b", &receiver);

    assert_eq!(first_change.state_id, second_change.state_id);
    assert_eq!(first_change.packet, second_change.packet);
    assert_eq!(first_packets, second_packets);
    assert!(
        first_packets.len() >= 2,
        "the support block and the broken torch are both broadcast"
    );

    let air_id = block_state_id(&air());
    let torch_pos = BlockPos::new(2, -60, 2);
    let mut decoded = Vec::new();
    for packet in &first_packets {
        let mut reader = PacketReader::new(packet);
        assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_BLOCK_UPDATE);
        let packed = reader.read_i64().unwrap();
        let state_id = reader.read_varint().unwrap();
        assert!(reader.is_empty());
        decoded.push((packed, state_id));
    }
    assert!(
        decoded.contains(&(torch_pos.as_long(), air_id)),
        "a deterministic air packet is sent for the broken torch"
    );
    assert!(
        decoded.contains(&(torch_pos.below().as_long(), air_id)),
        "the removed support is broadcast as air"
    );
}
