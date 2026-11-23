//! Java-capture-backed differential test for the captured 1.21.11 play packet
//! sequence.
//!
//! `join_play.bin` is a checked-in oracle fixture captured from the Java
//! capture server. No Java runtime is needed to replay it: the framing and
//! packet ids are exactly what `join::play_packets()` produces at runtime.
//! This suite pins that fixture so a regeneration or corruption is caught
//! before it silently changes the join stream Rust replays for real clients.
//!
//! Every id is classified into one of two categories:
//! - Core join packets: byte-sensitive ids that `join.rs` filters out and
//!   rebuilds from live session state. Each must appear at least once.
//! - Opaque capture-only packets: ids the replay still forwards verbatim
//!   (branding/plugin messages, container sync, player-info, tag/recipe
//!   payloads, and similar join chatter). Their byte layout is not asserted
//!   here; only their presence and classification are pinned.

use folia_core::protocol::PacketReader;
use std::collections::BTreeMap;

const PLAY_ASSET: &[u8] = include_bytes!("../assets/join_play.bin");

// Literal clientbound ids from the 1.21.11 capture. These match the `PLAY_*`
// constants in `join.rs`; they are repeated here so the oracle is independent
// of the implementation being tested.
const JOIN_GAME: i32 = 48;
const SET_TIME: i32 = 111;
const CHUNK_DATA: i32 = 44;
const CHUNK_CACHE_CENTER: i32 = 92;
const SET_HEALTH: i32 = 102;
const PLAYER_ABILITIES: i32 = 62;
const POSITION: i32 = 70;
const SYSTEM_CHAT: i32 = 119;

/// Core join packets each 1.21.11 capture must contain at least once.
const CORE: &[(i32, &str)] = &[
    (JOIN_GAME, "Join Game"),
    (SET_TIME, "Set Time"),
    (CHUNK_DATA, "Level Chunk with Light"),
    (CHUNK_CACHE_CENTER, "Chunk Cache Center"),
    (SET_HEALTH, "Set Health"),
    (PLAYER_ABILITIES, "Player Abilities"),
    (POSITION, "Player Position"),
    (SYSTEM_CHAT, "System Chat"),
];

/// Every other id in the capture. The labels are best-effort hints for the
/// coverage report and are not byte-asserted; `join.rs` replays these frames
/// verbatim. If a regenerated capture contains a new id it will fail the
/// classification assertion in `replay_observer_covers_every_packet_id` and
/// must be added here (or promoted into `CORE`) deliberately.
const CAPTURE_ONLY: &[(i32, &str)] = &[
    (10, "chunk batch control"),
    (16, "container content"),
    (18, "container slot"),
    (20, "chat suggestions / branding"),
    (34, "held item change"),
    (38, "game event"),
    (42, "entity status/event"),
    (43, "keep alive"),
    (68, "player info update"),
    (72, "recipe/update payload"),
    (74, "player rotation"),
    (84, "level/state update"),
    (93, "simulation distance"),
    (95, "player info/effect update"),
    (97, "set entity data"),
    (101, "set experience"),
    (103, "level event"),
    (109, "view/simulation distance"),
    (125, "advancement/stat update"),
    (126, "update features/tags"),
    (128, "registry/tag payload"),
    (129, "recipe/advancement payload"),
    (131, "large registry payload"),
];

/// Mirror of `join::unpack`: a big-endian u32 length followed by that many
/// packet bytes, repeated. Returns the packets and the byte offset consumed
/// so the framing test can detect trailing garbage.
fn parse_frames(bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut packets = Vec::new();
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let mut length = [0u8; 4];
        length.copy_from_slice(&bytes[offset..offset + 4]);
        offset += 4;
        let length = u32::from_be_bytes(length) as usize;
        if offset + length > bytes.len() {
            break;
        }
        packets.push(bytes[offset..offset + length].to_vec());
        offset += length;
    }
    (packets, offset)
}

/// Read the packet id with the same `PacketReader` the runtime uses.
fn packet_id(packet: &[u8]) -> Option<i32> {
    PacketReader::new(packet).read_varint().ok()
}

fn id_counts(packets: &[Vec<u8>]) -> BTreeMap<i32, usize> {
    let mut counts = BTreeMap::new();
    for packet in packets {
        let id = packet_id(packet).expect("capture packet id must be readable");
        *counts.entry(id).or_insert(0) += 1;
    }
    counts
}

fn classification() -> BTreeMap<i32, &'static str> {
    let mut map = BTreeMap::new();
    for &(id, name) in CORE {
        assert!(
            map.insert(id, name).is_none(),
            "duplicate id {id} in core list"
        );
    }
    for &(id, name) in CAPTURE_ONLY {
        assert!(
            map.insert(id, name).is_none(),
            "id {id} is in both core and capture-only lists"
        );
    }
    map
}

#[test]
fn capture_framing_matches_join_unpack() {
    let (packets, consumed) = parse_frames(PLAY_ASSET);
    assert_eq!(
        consumed,
        PLAY_ASSET.len(),
        "the capture must be a whole number of frames with no trailing bytes"
    );
    assert!(!packets.is_empty(), "the capture must contain packets");
    assert!(
        packets.iter().all(|packet| !packet.is_empty()),
        "every frame must carry at least a packet id"
    );
}

#[test]
fn capture_contains_core_join_packets() {
    let (packets, _) = parse_frames(PLAY_ASSET);
    let counts = id_counts(&packets);

    for &(id, name) in CORE {
        assert!(
            counts.contains_key(&id),
            "missing core join packet {name} (id {id}); observed ids: {counts:?}"
        );
    }

    // The task accepts "chunk data and/or chunk cache center"; this capture
    // carries both, so assert the union is non-empty explicitly as well.
    assert!(
        counts.contains_key(&CHUNK_DATA) || counts.contains_key(&CHUNK_CACHE_CENTER),
        "the capture must contain a chunk/batch packet (44 and/or 92)"
    );
}

#[test]
fn every_packet_id_is_readable_by_packet_reader() {
    let (packets, _) = parse_frames(PLAY_ASSET);
    for packet in &packets {
        let mut reader = PacketReader::new(packet);
        let id = reader.read_varint().expect("packet id must be readable");
        assert!(id >= 0, "clientbound packet ids are non-negative, got {id}");
        assert!(
            reader.position() >= 1,
            "reading the id must advance the cursor"
        );
        // No trailing-garbage assertion: the u32 frame already bounds each
        // packet exactly, and the remaining bytes are the id-specific body
        // that only the runtime's per-packet parsers interpret.
    }
}

#[test]
fn replay_observer_covers_every_packet_id() {
    let (packets, _) = parse_frames(PLAY_ASSET);
    let counts = id_counts(&packets);
    let classes = classification();

    println!("captured play packet coverage ({} packets):", packets.len());
    for (&id, count) in &counts {
        let name = classes.get(&id).copied().unwrap_or("UNCLASSIFIED");
        println!("  id {id:3}  count {count:3}  {name}");
    }

    assert_eq!(
        counts.keys().copied().collect::<Vec<_>>(),
        classes.keys().copied().collect::<Vec<_>>(),
        "classification must cover exactly the ids present in the capture"
    );
    assert_eq!(
        counts.values().sum::<usize>(),
        packets.len(),
        "coverage counts must account for every packet"
    );
}
