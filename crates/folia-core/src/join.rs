//! Minimal 1.21.11 login/configuration/play handshake.
//!
//! Configuration and initial play packets are captured from the real Folia
//! server (registry data, tags, flat-world chunks) and replayed here. Packet
//! IDs are the 1.21.11 play IDs; play has a bundle packet at id 0, so the
//! values are one higher than the `GameProtocols` addPacket index.
//!
//! This accepts offline-mode logins and puts the client into the captured flat
//! world. It is intentionally not a full gameplay implementation yet.

use crate::block_update::{
    ACTION_START_DESTROY_BLOCK, ACTION_STOP_DESTROY_BLOCK, SERVERBOUND_PLAYER_ACTION,
    SERVERBOUND_USE_ITEM_ON,
};
use crate::chunk_packet::ChunkPacketTemplate;
use crate::inventory;
use crate::player::{PlayerSession, uuid_string};
use crate::protocol::{
    Handshake, PacketReader, PacketWriter, build_login_disconnect, parse_login_start, read_packet,
    write_packet,
};
use crate::world::{Dimension, WorldStorage};
use std::io;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

const LOGIN_SUCCESS: i32 = 2;
const LOGIN_ACKNOWLEDGED: i32 = 3;

const CONFIG_CLIENT_INFORMATION: i32 = 0;
const CONFIG_FINISH: i32 = 3;
const CONFIG_KEEP_ALIVE: i32 = 4;
const CONFIG_SELECT_KNOWN_PACKS: i32 = 7;
const CONFIG_CLIENTBOUND_SELECT_KNOWN_PACKS: i32 = 14;
const CONFIG_CLIENTBOUND_FINISH: i32 = 3;

const PLAY_KEEP_ALIVE: i32 = 43;
const PLAY_POSITION: i32 = 70;
const PLAY_CHUNK: i32 = 44;
const PLAY_CHUNK_CACHE_CENTER: i32 = 92;
const LIVE_CHUNK_RADIUS: i32 = 3;

const SERVERBOUND_ACCEPT_TELEPORTATION: i32 = 0;
const SERVERBOUND_CHUNK_BATCH_RECEIVED: i32 = 10;
const SERVERBOUND_KEEP_ALIVE: i32 = 27;
const SERVERBOUND_PONG: i32 = 44;

const CONFIG_ASSET: &[u8] = include_bytes!("../assets/join_config.bin");
const PLAY_ASSET: &[u8] = include_bytes!("../assets/join_play.bin");

static CONFIG_PACKETS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
static PLAY_PACKETS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
static PENDING_JOINS: LazyLock<Mutex<Vec<PendingPlayerJoin>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static PLAYERDATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static WORLD_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPlayerJoin {
    pub name: String,
    pub uuid: String,
}

pub fn configure_playerdata(directory: impl Into<PathBuf>) {
    let _ = PLAYERDATA_DIR.set(directory.into());
}

pub fn configure_world_dir(directory: impl Into<PathBuf>) {
    let _ = WORLD_DIR.set(directory.into());
}

pub fn take_pending_joins() -> Vec<PendingPlayerJoin> {
    std::mem::take(
        &mut *PENDING_JOINS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

fn playerdata_dir() -> PathBuf {
    PLAYERDATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("run/playerdata"))
}

fn world_dir() -> PathBuf {
    WORLD_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("world"))
}

/// Accept a login and walk the client through configuration into the captured
/// flat world. The caller has already read and parsed the handshake.
pub fn handle_login(mut stream: TcpStream, handshake: &Handshake) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let login_start = read_packet(&mut stream)?;
    let start = parse_login_start(&login_start)?;
    println!(
        "[join] login start name={} uuid={:?} protocol={}",
        start.name, start.uuid, handshake.protocol_version
    );

    let uuid = start.uuid.unwrap_or_else(|| offline_uuid(&start.name));
    let (spawn_x, spawn_y, spawn_z, spawn_yaw, spawn_pitch) = captured_spawn();
    let mut session = match PlayerSession::load(playerdata_dir(), uuid) {
        Ok(Some(session)) => session,
        Ok(None) => {
            let mut session =
                PlayerSession::new(start.name.clone(), uuid, spawn_x, spawn_y, spawn_z);
            session.yaw = spawn_yaw;
            session.pitch = spawn_pitch;
            session
        }
        Err(error) => {
            eprintln!("[join] could not read playerdata: {error}");
            PlayerSession::new(start.name.clone(), uuid, spawn_x, spawn_y, spawn_z)
        }
    };
    session.name = start.name.clone();
    session.uuid = uuid;

    send_login_success(&mut stream, &uuid, &start.name)?;

    // Login Acknowledged.
    loop {
        let packet = read_packet(&mut stream)?;
        let id = packet_id(&packet)?;
        if id == LOGIN_ACKNOWLEDGED {
            break;
        }
        if id == 0 {
            let mut reader = PacketReader::new(&packet);
            let _ = reader.read_varint();
            println!(
                "[join] login disconnect: {}",
                reader.read_string().unwrap_or_default()
            );
            return Ok(());
        }
    }

    // Configuration: brand/features first, then an empty known-pack selection
    // so the client asks for the full registry set.
    let config = config_packets();
    for packet in config {
        let id = packet_id(packet)?;
        if id == CONFIG_CLIENTBOUND_SELECT_KNOWN_PACKS || id == CONFIG_CLIENTBOUND_FINISH {
            break;
        }
        write_packet(&mut stream, packet)?;
    }
    write_packet(&mut stream, &empty_known_packs())?;

    // Wait for the client's known-pack selection and client information.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match read_packet(&mut stream) {
            Ok(packet) => {
                let id = packet_id(&packet)?;
                if id == CONFIG_SELECT_KNOWN_PACKS {
                    break;
                }
                if id == CONFIG_CLIENT_INFORMATION || id == CONFIG_KEEP_ALIVE {
                    continue;
                }
                if id == 0 {
                    return Ok(());
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }

    // Remaining registry data + tags.
    let mut after_known_packs = false;
    for packet in config {
        let id = packet_id(packet)?;
        if id == CONFIG_CLIENTBOUND_SELECT_KNOWN_PACKS {
            after_known_packs = true;
            continue;
        }
        if id == CONFIG_CLIENTBOUND_FINISH {
            continue;
        }
        if after_known_packs {
            write_packet(&mut stream, packet)?;
        }
    }
    write_packet(&mut stream, &simple_packet(CONFIG_CLIENTBOUND_FINISH))?;

    // Wait for Finish Configuration acknowledgement.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match read_packet(&mut stream) {
            Ok(packet) => {
                let id = packet_id(&packet)?;
                if id == CONFIG_FINISH {
                    break;
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }

    // Play: replay the non-chunk captured setup packets, then stream live
    // ClientboundLevelChunkWithLight packets generated from WorldStorage/flat
    // chunks around the player's chunk position.
    for packet in play_packets() {
        let id = packet_id(packet)?;
        if id == PLAY_CHUNK || id == PLAY_POSITION || id == PLAY_CHUNK_CACHE_CENTER {
            continue;
        }
        write_packet(&mut stream, packet)?;
    }

    let center_x = (session.x.floor() as i32).div_euclid(16);
    let center_z = (session.z.floor() as i32).div_euclid(16);
    let mut storage = if world_dir().join("region").is_dir() {
        Some(WorldStorage::for_dimension(
            world_dir(),
            Dimension::Overworld,
        ))
    } else {
        None
    };
    let mut from_world = 0usize;
    let mut generated = 0usize;

    write_packet(&mut stream, &chunk_cache_center(center_x, center_z))?;
    for dz in -LIVE_CHUNK_RADIUS..=LIVE_CHUNK_RADIUS {
        for dx in -LIVE_CHUNK_RADIUS..=LIVE_CHUNK_RADIUS {
            let chunk_x = center_x + dx;
            let chunk_z = center_z + dz;
            let loaded = storage
                .as_mut()
                .and_then(|storage| storage.load_chunk(chunk_x, chunk_z).ok().flatten());
            let chunk = match loaded {
                Some(chunk) => {
                    from_world += 1;
                    chunk
                }
                None => {
                    generated += 1;
                    if storage.is_none() {
                        match crate::worldgen::generator::configured() {
                            Some(generator) => generator.generate_chunk(chunk_x, chunk_z)?,
                            None => crate::chunk_packet::flat_chunk(chunk_x, chunk_z),
                        }
                    } else {
                        crate::chunk_packet::flat_chunk(chunk_x, chunk_z)
                    }
                }
            };
            write_packet(&mut stream, &ChunkPacketTemplate::build_dynamic(&chunk)?)?;
        }
    }

    let updates = crate::block_update::register_session();
    for packet in crate::entity::spawn_packets() {
        write_packet(&mut stream, &packet)?;
    }
    send_container_content(&mut stream, &session)?;
    write_packet(
        &mut stream,
        &inventory::held_slot_packet(session.selected_slot),
    )?;
    write_packet(&mut stream, &position_packet(&session, 1000))?;
    PENDING_JOINS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(PendingPlayerJoin {
            name: session.name.clone(),
            uuid: uuid_string(&session.uuid),
        });
    println!(
        "[join] streamed {} live chunks around ({}, {}) for {} at ({:.1}, {:.1}, {:.1})",
        from_world + generated,
        center_x,
        center_z,
        start.name,
        session.x,
        session.y,
        session.z
    );
    println!(
        "[join] chunk source: {from_world} from WorldStorage, {generated} generated terrain chunks"
    );

    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut last_keepalive = Instant::now();
    let mut last_save = Instant::now();
    let mut keepalive_nonce = 1i64;

    loop {
        while let Ok(update) = updates.try_recv() {
            write_packet(&mut stream, &update)?;
        }
        match read_packet(&mut stream) {
            Ok(packet) => {
                let id = packet_id(&packet)?;
                match id {
                    SERVERBOUND_KEEP_ALIVE
                    | SERVERBOUND_PONG
                    | SERVERBOUND_ACCEPT_TELEPORTATION
                    | SERVERBOUND_CHUNK_BATCH_RECEIVED => {}
                    SERVERBOUND_PLAYER_ACTION | SERVERBOUND_USE_ITEM_ON => {
                        if let Err(error) = handle_block_action(
                            &mut stream,
                            &mut storage,
                            &mut session,
                            &packet,
                            id,
                        ) {
                            eprintln!("[join] block action failed: {error}");
                        }
                    }
                    inventory::SERVERBOUND_SET_CARRIED_ITEM => {
                        if let Err(error) = handle_set_carried(&mut stream, &mut session, &packet) {
                            eprintln!("[join] carried slot update failed: {error}");
                        }
                    }
                    inventory::SERVERBOUND_SET_CREATIVE_MODE_SLOT => {
                        if let Some(creative) = inventory::parse_creative_slot(&packet) {
                            if let Some(index) = inventory::inventory_index(creative.slot) {
                                session.inventory[index] = creative.stack;
                                send_container_content(&mut stream, &session)?;
                                if let Err(error) = session.save(playerdata_dir()) {
                                    eprintln!("[join] could not save inventory: {error}");
                                }
                            }
                        }
                    }
                    inventory::SERVERBOUND_CONTAINER_CLICK => {
                        if let Some(click) = inventory::parse_container_click(&packet) {
                            inventory::apply_click(
                                &mut session.inventory,
                                &mut session.cursor,
                                click,
                            );
                            send_container_content(&mut stream, &session)?;
                            if let Err(error) = session.save(playerdata_dir()) {
                                eprintln!("[join] could not save inventory: {error}");
                            }
                        }
                    }
                    _ => {
                        if session.apply_movement(&packet)? {
                            if last_save.elapsed() >= Duration::from_secs(5) {
                                if let Err(error) = session.save(playerdata_dir()) {
                                    eprintln!("[join] could not save playerdata: {error}");
                                }
                                last_save = Instant::now();
                            }
                        }
                    }
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                if last_keepalive.elapsed() >= Duration::from_secs(10) {
                    write_packet(&mut stream, &keepalive_packet(keepalive_nonce))?;
                    keepalive_nonce += 1;
                    last_keepalive = Instant::now();
                }
            }
            Err(error) => {
                if let Err(save_error) = session.save(playerdata_dir()) {
                    eprintln!("[join] could not save playerdata: {save_error}");
                }
                println!("[join] {} disconnected: {error}", start.name);
                return Ok(());
            }
        }
    }
}

pub fn reject_login(mut stream: TcpStream, reason: &str) -> io::Result<()> {
    write_packet(&mut stream, &build_login_disconnect(reason)?)
}

fn captured_spawn() -> (f64, f64, f64, f32, f32) {
    for packet in play_packets() {
        if packet_id(packet).ok() != Some(PLAY_POSITION) {
            continue;
        }
        let mut reader = PacketReader::new(packet);
        let _ = reader.read_varint();
        let _ = reader.read_varint();
        let x = reader.read_f64().unwrap_or(0.0);
        let y = reader.read_f64().unwrap_or(64.0);
        let z = reader.read_f64().unwrap_or(0.0);
        let _ = reader.read_f64();
        let _ = reader.read_f64();
        let _ = reader.read_f64();
        let yaw = reader.read_f32().unwrap_or(0.0);
        let pitch = reader.read_f32().unwrap_or(0.0);
        return (x, y, z, yaw, pitch);
    }
    (0.0, 64.0, 0.0, 0.0, 0.0)
}

fn position_packet(session: &PlayerSession, teleport_id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_POSITION).unwrap();
    writer.write_varint(teleport_id).unwrap();
    writer.write_f64(session.x).unwrap();
    writer.write_f64(session.y).unwrap();
    writer.write_f64(session.z).unwrap();
    writer.write_f64(0.0).unwrap();
    writer.write_f64(0.0).unwrap();
    writer.write_f64(0.0).unwrap();
    writer.write_f32(session.yaw).unwrap();
    writer.write_f32(session.pitch).unwrap();
    writer.write_i32(0).unwrap(); // no relative axes
    writer.into_inner()
}

fn keepalive_packet(nonce: i64) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_KEEP_ALIVE).unwrap();
    writer.write_i64(nonce).unwrap();
    writer.into_inner()
}

fn chunk_cache_center(chunk_x: i32, chunk_z: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_CHUNK_CACHE_CENTER).unwrap();
    writer.write_varint(chunk_x).unwrap();
    writer.write_varint(chunk_z).unwrap();
    writer.into_inner()
}

fn handle_block_action(
    stream: &mut TcpStream,
    storage: &mut Option<WorldStorage>,
    session: &mut PlayerSession,
    packet: &[u8],
    id: i32,
) -> io::Result<()> {
    let Some(storage) = storage.as_mut() else {
        return Ok(());
    };

    match id {
        SERVERBOUND_PLAYER_ACTION => {
            let action = crate::block_update::parse_player_action(packet)?;
            if action.action != ACTION_START_DESTROY_BLOCK
                && action.action != ACTION_STOP_DESTROY_BLOCK
            {
                return Ok(());
            }
            let air = crate::chunk::BlockState {
                name: "minecraft:air".into(),
                properties: vec![],
            };
            if let Some(change) =
                crate::block_update::set_block(storage, action.x, action.y, action.z, air)?
            {
                crate::block_update::broadcast_change(&change);
            }
        }
        SERVERBOUND_USE_ITEM_ON => {
            let use_item = crate::block_update::parse_use_item_on(packet)?;
            let Some(held) = session.selected_item().cloned() else {
                return Ok(());
            };
            let Some(block) = inventory::block_for_item(&held.item) else {
                return Ok(());
            };
            let (dx, dy, dz) = crate::block_update::direction_offset(use_item.direction);
            if let Some(change) = crate::block_update::set_block(
                storage,
                use_item.x + dx,
                use_item.y + dy,
                use_item.z + dz,
                block,
            )? {
                crate::block_update::broadcast_change(&change);
                session.consume_selected();
                send_container_content(stream, session)?;
                if let Err(error) = session.save(playerdata_dir()) {
                    eprintln!("[join] could not save inventory: {error}");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn send_container_content(stream: &mut TcpStream, session: &PlayerSession) -> io::Result<()> {
    write_packet(
        stream,
        &inventory::container_content_packet(&session.inventory, &session.cursor),
    )
}

fn handle_set_carried(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    packet: &[u8],
) -> io::Result<()> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    if id != inventory::SERVERBOUND_SET_CARRIED_ITEM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected carried item id 52, got {id}"),
        ));
    }
    let slot = i32::from(reader.read_i16()?);
    if (0..inventory::HOTBAR_SLOTS as i32).contains(&slot) {
        session.selected_slot = slot;
        write_packet(stream, &inventory::held_slot_packet(slot))?;
        if let Err(error) = session.save(playerdata_dir()) {
            eprintln!("[join] could not save inventory: {error}");
        }
        println!("[join] selected hotbar slot {slot}");
    }
    Ok(())
}

fn send_login_success(stream: &mut TcpStream, uuid: &[u8; 16], name: &str) -> io::Result<()> {
    let mut writer = PacketWriter::new();
    writer.write_varint(LOGIN_SUCCESS)?;
    writer.write_bytes(uuid);
    writer.write_string(name)?;
    writer.write_varint(0)?; // game profile properties
    write_packet(stream, &writer.into_inner())
}

fn empty_known_packs() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CONFIG_CLIENTBOUND_SELECT_KNOWN_PACKS)
        .unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}

fn simple_packet(id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(id).unwrap();
    writer.into_inner()
}

fn config_packets() -> &'static [Vec<u8>] {
    CONFIG_PACKETS.get_or_init(|| unpack(CONFIG_ASSET))
}

pub(crate) fn play_packets() -> &'static [Vec<u8>] {
    PLAY_PACKETS.get_or_init(|| unpack(PLAY_ASSET))
}

fn unpack(bytes: &[u8]) -> Vec<Vec<u8>> {
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
    packets
}

fn packet_id(packet: &[u8]) -> io::Result<i32> {
    let mut reader = PacketReader::new(packet);
    reader.read_varint()
}

/// Vanilla offline UUID is MD5("OfflinePlayer:" + name) with version 3 bits.
/// A fixed local UUID is sufficient for this bootstrap; it remains stable for
/// the same name within the Rust host.
fn offline_uuid(name: &str) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let mut hash = 0xcbf29ce484222325u64;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    bytes[..8].copy_from_slice(&hash.to_be_bytes());
    bytes[8..].copy_from_slice(&hash.rotate_left(32).to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}
