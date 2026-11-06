//! Minimal 1.21.11 login/configuration/play handshake.
//!
//! Configuration and initial play packets are captured from the real Folia
//! server (registry data, tags, flat-world chunks) and replayed here. Packet
//! IDs are the 1.21.11 play IDs; play has a bundle packet at id 0, so the
//! values are one higher than the `GameProtocols` addPacket index.
//!
//! This accepts offline-mode logins and streams chunks around the player. The
//! noise generator produces terrain, which is written back to the region files
//! (stamped with `folia:written`) and reloaded later, so exploration and block
//! edits survive a restart. Stored chunks are only trusted when the level has a
//! `level.dat` or the chunk carries that stamp; a flat placeholder is used only
//! when no generator is configured. New chunk columns are sent as the player
//! crosses chunk borders. It is intentionally not a full gameplay
//! implementation yet.

use crate::anvil::Compression;
use crate::block_update::{
    ACTION_START_DESTROY_BLOCK, ACTION_STOP_DESTROY_BLOCK, SERVERBOUND_PLAYER_ACTION,
    SERVERBOUND_USE_ITEM_ON,
};
use crate::chunk::Chunk;
use crate::chunk_packet::ChunkPacketTemplate;
use crate::inventory;
use crate::nbt::Tag;
use crate::player::{PlayerSession, uuid_string};
use crate::player_physics::{MovementCorrection, StoragePhysicsWorld};
use crate::protocol::{
    Handshake, PacketReader, PacketWriter, build_login_disconnect, parse_login_start, read_packet,
    write_packet,
};
use crate::world::WorldStorage;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
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
/// `ClientboundForgetLevelChunk`: unloads one column on the client.
const PLAY_FORGET_CHUNK: i32 = 37;
/// `ClientboundLoginPacket` (Join Game).
const PLAY_JOIN_GAME: i32 = 48;
/// `ClientboundGameEventPacket`.
const PLAY_GAME_EVENT: i32 = 38;
/// `ClientboundPlayerAbilitiesPacket`.
const PLAY_PLAYER_ABILITIES: i32 = 62;
const PLAY_SET_HEALTH: i32 = 102;
const PLAY_PLAYER_COMBAT_KILL: i32 = 66;
const PLAY_RESPAWN: i32 = 80;

/// `ClientboundPlayerCombatKill`/`SetHealth`/`Respawn` IDs are the live
/// 1.21.11 IDs; `PLAY_SET_HEALTH` is also present in the captured join stream.
const SERVERBOUND_CLIENT_COMMAND: i32 = 11;
const CLIENT_COMMAND_PERFORM_RESPAWN: i32 = 0;

/// Vanilla starts void damage below `minY - 64`; the overworld floor is -64.
const VOID_DAMAGE_Y: f64 = -128.0;
const VOID_DAMAGE_INTERVAL: Duration = Duration::from_millis(500);
const VOID_DAMAGE: f32 = 4.0;

const SERVERBOUND_CHANGE_GAME_MODE: i32 = 4;
const SERVERBOUND_CHAT_COMMAND: i32 = 6;
const SERVERBOUND_CHAT_COMMAND_SIGNED: i32 = 7;

/// `ClientboundGameEventPacket.CHANGE_GAME_MODE`.
const GAME_EVENT_CHANGE_GAME_MODE: u8 = 3;
/// Columns this far outside the live radius are unloaded, so chunks the client
/// may still be rendering at the edge are not dropped the moment the player
/// crosses a border.
const UNLOAD_MARGIN: i32 = 1;
/// View distance used when neither the server nor the client asks for one.
const DEFAULT_VIEW_DISTANCE: i32 = 6;

/// Vanilla's floor for a requested view distance.
const MIN_VIEW_DISTANCE: i32 = 2;

/// Hard cap on the live radius: 33×33 = 1089 columns. Chunks are generated
/// synchronously, so larger radii have to wait for async generation scheduling
/// (TODO P3.3).
const MAX_VIEW_DISTANCE: i32 = 16;

static VIEW_DISTANCE: OnceLock<i32> = OnceLock::new();

/// Set the server's `view-distance` (from `server.properties`). A session uses
/// the smaller of this and the client's request.
pub fn configure_view_distance(distance: i32) {
    let _ = VIEW_DISTANCE.set(distance);
}

fn server_view_distance() -> i32 {
    VIEW_DISTANCE
        .get()
        .copied()
        .unwrap_or(DEFAULT_VIEW_DISTANCE)
        .clamp(MIN_VIEW_DISTANCE, MAX_VIEW_DISTANCE)
}

static DEFAULT_GAME_MODE: OnceLock<i32> = OnceLock::new();

/// Set the game mode new players start in (from `server.properties`).
pub fn configure_game_mode(mode: i32) {
    let _ = DEFAULT_GAME_MODE.set(mode.clamp(0, 3));
}

fn default_game_mode() -> i32 {
    DEFAULT_GAME_MODE.get().copied().unwrap_or(0)
}

/// The client's requested view distance from a `ClientInformation` packet.
fn parse_client_view_distance(packet: &[u8]) -> Option<i32> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint().ok()? != CONFIG_CLIENT_INFORMATION {
        return None;
    }
    reader.read_string().ok()?;
    reader.read_u8().ok().map(i32::from)
}

/// How often generated terrain is written from the region cache to disk.
///
/// A region file save rewrites the whole `.mca`, so flushing after every chunk
/// batch would rewrite the same growing file many times per minute while a
/// player walks.
const TERRAIN_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// How often a connected session writes its playerdata.
const PLAYER_SAVE_INTERVAL: Duration = Duration::from_secs(5);

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

#[derive(Debug)]
enum SessionCommand {
    SetGameMode {
        mode: i32,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

struct ActiveConsoleSession {
    name: String,
    uuid: [u8; 16],
    sender: mpsc::Sender<SessionCommand>,
}

static CONSOLE_SESSIONS: LazyLock<Mutex<Vec<ActiveConsoleSession>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static OPERATORS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static OPS_PATH: OnceLock<PathBuf> = OnceLock::new();

struct ConsoleSessionGuard {
    uuid: [u8; 16],
}

impl ConsoleSessionGuard {
    fn register(name: &str, uuid: [u8; 16]) -> (Self, mpsc::Receiver<SessionCommand>) {
        let (sender, receiver) = mpsc::channel();
        CONSOLE_SESSIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(ActiveConsoleSession {
                name: name.to_owned(),
                uuid,
                sender,
            });
        (Self { uuid }, receiver)
    }
}

impl Drop for ConsoleSessionGuard {
    fn drop(&mut self) {
        CONSOLE_SESSIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|session| session.uuid != self.uuid);
    }
}

fn active_session_names() -> Vec<String> {
    let mut names: Vec<String> = CONSOLE_SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|session| session.name.clone())
        .collect();
    names.sort();
    names
}

pub fn configure_operators(path: impl Into<PathBuf>) {
    let path = path.into();
    let _ = OPS_PATH.set(path.clone());
    if let Ok(contents) = std::fs::read_to_string(path) {
        let mut operators = OPERATORS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for line in contents.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                operators.insert(line.to_ascii_lowercase());
            }
        }
    }
}

fn save_operators() {
    let Some(path) = OPS_PATH.get() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("[join] could not create operator directory: {error}");
        return;
    }
    let mut names: Vec<String> = OPERATORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .cloned()
        .collect();
    names.sort();
    let contents = if names.is_empty() {
        String::new()
    } else {
        format!(
            "{}
",
            names.join(
                "
"
            )
        )
    };
    if let Err(error) = std::fs::write(path, contents) {
        eprintln!("[join] could not save operators: {error}");
    }
}

pub fn is_operator(name: &str) -> bool {
    OPERATORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&name.to_ascii_lowercase())
}

fn add_operator(name: &str) -> bool {
    let inserted = {
        OPERATORS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.to_ascii_lowercase())
    };
    if inserted {
        save_operators();
    }
    inserted
}

fn remove_operator(name: &str) -> bool {
    let removed = {
        OPERATORS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&name.to_ascii_lowercase())
    };
    if removed {
        save_operators();
    }
    removed
}

/// Execute one line from the server terminal. The caller prints the returned
/// lines; console commands never pretend to be a player chat message.
pub fn execute_console_command(line: &str) -> Vec<String> {
    let line = line.trim().strip_prefix('/').unwrap_or(line.trim());
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return Vec::new();
    };
    match command.to_ascii_lowercase().as_str() {
        "help" | "?" => vec![
            "console commands: gamemode <mode> [player], op <player>, deop <player>, list, ops, stop"
                .to_owned(),
        ],
        "list" => {
            let names = active_session_names();
            if names.is_empty() {
                vec!["no players online".to_owned()]
            } else {
                vec![format!("online ({}): {}", names.len(), names.join(", "))]
            }
        }
        "ops" => {
            let mut names: Vec<String> = OPERATORS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .cloned()
                .collect();
            names.sort();
            vec![if names.is_empty() {
                "no operators".to_owned()
            } else {
                format!("operators: {}", names.join(", "))
            }]
        }
        "op" | "deop" => {
            let Some(target) = parts.next() else {
                return vec![format!("usage: {command} <player>")];
            };
            if command.eq_ignore_ascii_case("op") {
                if add_operator(target) {
                    vec![format!("added {target} to operators")]
                } else {
                    vec![format!("{target} is already an operator")]
                }
            } else if remove_operator(target) {
                vec![format!("removed {target} from operators")]
            } else {
                vec![format!("{target} is not an operator")]
            }
        }
        "gamemode" | "gm" => {
            let Some(mode) = parts.next().and_then(parse_game_mode) else {
                return vec![
                    "usage: gamemode <survival|creative|adventure|spectator> [player]".to_owned(),
                ];
            };
            let target = parts.next().map(str::to_owned).or_else(|| {
                let names = active_session_names();
                (names.len() == 1).then(|| names[0].clone())
            });
            let Some(target) = target else {
                return vec![
                    "usage: gamemode <mode> <player> (target required when several players are online)"
                        .to_owned(),
                ];
            };
            let sender = CONSOLE_SESSIONS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .find(|session| session.name.eq_ignore_ascii_case(&target))
                .map(|session| session.sender.clone());
            let Some(sender) = sender else {
                return vec![format!("player {target} is not online")];
            };
            let (reply, response) = mpsc::channel();
            if sender
                .send(SessionCommand::SetGameMode { mode, reply })
                .is_err()
            {
                return vec![format!("player {target} disconnected")];
            }
            match response.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => vec![format!(
                    "set {target}'s game mode to {}",
                    game_mode_name(mode)
                )],
                Ok(Err(error)) => vec![format!("could not change {target}'s game mode: {error}")],
                Err(_) => vec![format!("timed out waiting for {target}")],
            }
        }
        "stop" | "shutdown" => vec!["use the server process stop control".to_owned()],
        other => vec![format!("unknown console command '{other}'; type help")],
    }
}

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

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static ACTIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// A login that is still running; the counter lets shutdown wait for sessions
/// to save and close instead of exiting under them.
struct SessionGuard;

impl SessionGuard {
    fn new() -> Self {
        ACTIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        ACTIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// True once the server has asked sessions to close.
pub fn shutdown_requested() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

/// Ask connected sessions to finish, then wait up to `timeout` for them to
/// save and return. Sessions still in login or configuration do not poll the
/// flag, so the timeout bounds those.
pub fn shutdown_sessions(timeout: Duration) {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + timeout;
    while ACTIVE_SESSIONS.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let remaining = ACTIVE_SESSIONS.load(Ordering::SeqCst);
    if remaining > 0 {
        eprintln!("[join] {remaining} session(s) still connected after the shutdown timeout");
    }
}

/// True once a session's playerdata is older than the autosave interval.
fn autosave_due(last_save: Instant, now: Instant, interval: Duration) -> bool {
    now.duration_since(last_save) >= interval
}

fn autosave_playerdata(session: &PlayerSession, last_save: &mut Instant) {
    if !autosave_due(*last_save, Instant::now(), PLAYER_SAVE_INTERVAL) {
        return;
    }
    if let Err(error) = session.save(playerdata_dir()) {
        eprintln!("[join] could not save playerdata: {error}");
    }
    *last_save = Instant::now();
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
    let _session = SessionGuard::new();
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let login_start = read_packet(&mut stream)?;
    let start = parse_login_start(&login_start)?;
    println!(
        "[join] login start name={} uuid={:?} protocol={}",
        start.name, start.uuid, handshake.protocol_version
    );

    let uuid = start.uuid.unwrap_or_else(|| offline_uuid(&start.name));
    let world = open_world();
    let (spawn_x, spawn_y, spawn_z, spawn_yaw, spawn_pitch) = match world.spawn {
        Some((spawn_x, spawn_y, spawn_z)) => (
            f64::from(spawn_x) + 0.5,
            f64::from(spawn_y),
            f64::from(spawn_z) + 0.5,
            0.0,
            0.0,
        ),
        None => captured_spawn(),
    };
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
    if session.game_mode < 0 {
        session.game_mode = default_game_mode();
    }
    unstick_player(&mut session, world.storage, world.trust_stored);
    // The simulation starts from the accepted login position, not from the
    // playerdata position before settling.
    session.reset_physics();

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
    let mut requested_view_distance = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match read_packet(&mut stream) {
            Ok(packet) => {
                let id = packet_id(&packet)?;
                if id == CONFIG_SELECT_KNOWN_PACKS {
                    break;
                }
                if id == CONFIG_CLIENT_INFORMATION {
                    requested_view_distance = parse_client_view_distance(&packet);
                    continue;
                }
                if id == CONFIG_KEEP_ALIVE {
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

    // Play: rebuild Join Game from the live session instead of replaying the
    // captured packet, which describes the capture server's flat creative
    // world (game mode 1, `isFlat = true`, chunk radius 3). Filter out every
    // packet this server sends itself below.
    let server_distance = server_view_distance();
    let radius = requested_view_distance
        .map_or(server_distance, |requested| {
            requested.clamp(MIN_VIEW_DISTANCE, MAX_VIEW_DISTANCE)
        })
        .min(server_distance);
    println!(
        "[join] view distance {radius} chunks (client asked for {requested_view_distance:?}, \
         server allows {server_distance})"
    );
    write_packet(
        &mut stream,
        &join_game_packet(session.game_mode, radius, radius)?,
    )?;
    for packet in play_packets() {
        let id = packet_id(packet)?;
        if matches!(
            id,
            PLAY_JOIN_GAME
                | PLAY_CHUNK
                | PLAY_POSITION
                | PLAY_CHUNK_CACHE_CENTER
                | PLAY_PLAYER_ABILITIES
                | PLAY_SET_HEALTH
        ) {
            continue;
        }
        if id == PLAY_GAME_EVENT && game_event_is_change_game_mode(packet) {
            continue;
        }
        write_packet(&mut stream, packet)?;
    }
    send_container_content(&mut stream, &session)?;
    write_packet(
        &mut stream,
        &inventory::held_slot_packet(session.selected_slot),
    )?;
    write_packet(&mut stream, &abilities_packet(session.game_mode))?;
    write_packet(&mut stream, &health_packet(&session))?;
    // The position has to arrive before the chunk batch. The client stays on
    // the loading screen until it has both; the old order generated every
    // column first, so a 441-column join sat on the loading screen for the
    // whole batch. The first column streamed below is this position's column.
    write_packet(&mut stream, &position_packet(&session, 1000))?;
    let updates = crate::block_update::register_session();
    let mut center_x = (session.x.floor() as i32).div_euclid(16);
    let mut center_z = (session.z.floor() as i32).div_euclid(16);
    let mut sent_chunks: HashSet<(i32, i32)> = HashSet::new();
    let streamed = stream_chunks_around(
        &mut stream,
        world.storage,
        world.trust_stored,
        radius,
        center_x,
        center_z,
        &mut sent_chunks,
    )?;
    let chunk_count = streamed.streamed;
    let counts = streamed.counts;

    for packet in crate::entity::spawn_packets() {
        write_packet(&mut stream, &packet)?;
    }
    PENDING_JOINS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(PendingPlayerJoin {
            name: session.name.clone(),
            uuid: uuid_string(&session.uuid),
        });
    println!(
        "[join] streamed {chunk_count} live chunks around ({}, {}) for {} at ({:.1}, {:.1}, {:.1})",
        center_x, center_z, start.name, session.x, session.y, session.z
    );
    println!("[join] chunk source: {}", counts.describe());

    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut teleport_id = 1001i32;
    let mut last_keepalive = Instant::now();
    let mut last_save = Instant::now();
    let mut last_flush = Instant::now();
    let mut keepalive_nonce = 1i64;
    let respawn_position = (spawn_x, spawn_y, spawn_z);
    let mut last_void_damage = Instant::now();
    let mut awaiting_respawn = false;
    let mut death_at: Option<Instant> = None;
    let (_console_guard, console_commands) =
        ConsoleSessionGuard::register(&session.name, session.uuid);

    loop {
        if shutdown_requested() {
            if let Err(error) = session.save(playerdata_dir()) {
                eprintln!("[join] could not save playerdata: {error}");
            }
            if let Err(error) = crate::world::lock(world.storage).flush() {
                eprintln!("[join] could not flush terrain: {error}");
            }
            println!("[join] {} disconnected for shutdown", start.name);
            return Ok(());
        }
        while let Ok(command) = console_commands.try_recv() {
            match command {
                SessionCommand::SetGameMode { mode, reply } => {
                    let result = apply_game_mode(&mut stream, &mut session, mode)
                        .map_err(|error| error.to_string());
                    let _ = reply.send(result);
                }
            }
        }
        if awaiting_respawn {
            if death_at.is_some_and(|time| time.elapsed() >= Duration::from_secs(2)) {
                respawn_player(
                    &mut stream,
                    &mut session,
                    world.storage,
                    world.trust_stored,
                    radius,
                    &mut center_x,
                    &mut center_z,
                    &mut sent_chunks,
                    respawn_position,
                    &mut teleport_id,
                )?;
                awaiting_respawn = false;
                death_at = None;
            }
        } else if apply_void_damage(&mut stream, &mut session, &mut last_void_damage)? {
            awaiting_respawn = true;
            death_at = Some(Instant::now());
            if let Err(error) = session.save(playerdata_dir()) {
                eprintln!("[join] could not save void death: {error}");
            }
            println!("[join] {} died in the void", start.name);
        }
        while let Ok(update) = updates.try_recv() {
            if let Err(error) = write_packet(&mut stream, &update) {
                if is_disconnect(&error) {
                    finish_disconnected(&session, world.storage, &start.name, &error);
                    return Ok(());
                }
                return Err(error);
            }
        }
        match read_packet(&mut stream) {
            Ok(packet) => {
                let id = packet_id(&packet)?;
                match id {
                    SERVERBOUND_KEEP_ALIVE
                    | SERVERBOUND_PONG
                    | SERVERBOUND_ACCEPT_TELEPORTATION
                    | SERVERBOUND_CHUNK_BATCH_RECEIVED => {}
                    SERVERBOUND_CHANGE_GAME_MODE => {
                        if !is_operator(&session.name) {
                            println!(
                                "[join] {}: you do not have permission to change game mode",
                                session.name
                            );
                        } else {
                            let mut reader = PacketReader::new(&packet);
                            let _ = reader.read_varint();
                            if let Ok(mode) = reader.read_varint()
                                && let Err(error) = apply_game_mode(&mut stream, &mut session, mode)
                            {
                                eprintln!("[join] game mode change failed: {error}");
                            }
                        }
                    }
                    SERVERBOUND_CLIENT_COMMAND => {
                        let mut reader = PacketReader::new(&packet);
                        let _ = reader.read_varint();
                        if awaiting_respawn
                            && reader.read_varint().ok() == Some(CLIENT_COMMAND_PERFORM_RESPAWN)
                        {
                            respawn_player(
                                &mut stream,
                                &mut session,
                                world.storage,
                                world.trust_stored,
                                radius,
                                &mut center_x,
                                &mut center_z,
                                &mut sent_chunks,
                                respawn_position,
                                &mut teleport_id,
                            )?;
                            awaiting_respawn = false;
                            death_at = None;
                        }
                    }
                    SERVERBOUND_CHAT_COMMAND | SERVERBOUND_CHAT_COMMAND_SIGNED => {
                        if let Err(error) = handle_command(&mut stream, &mut session, &packet) {
                            eprintln!("[join] command failed: {error}");
                        }
                    }
                    SERVERBOUND_PLAYER_ACTION | SERVERBOUND_USE_ITEM_ON => {
                        if let Err(error) = handle_block_action(
                            &mut stream,
                            world.storage,
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
                        if let Some(creative) = inventory::parse_creative_slot(&packet)
                            && let Some(index) = inventory::inventory_index(creative.slot)
                        {
                            session.inventory[index] = creative.stack;
                            send_container_content(&mut stream, &session)?;
                            if let Err(error) = session.save(playerdata_dir()) {
                                eprintln!("[join] could not save inventory: {error}");
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
                        let outcome = {
                            let storage = &mut *crate::world::lock(world.storage);
                            let mut collider = StoragePhysicsWorld::new(storage);
                            session.apply_movement(&packet, &mut collider)?
                        };
                        if !outcome.was_movement {
                            continue;
                        }
                        if let Some(correction) = outcome.correction {
                            teleport_id += 1;
                            write_packet(&mut stream, &position_packet(&session, teleport_id))?;
                            match correction {
                                MovementCorrection::TooFar { distance_squared } => println!(
                                    "[join] {} moved {:.1} blocks in one packet; sent back to \
                                     ({:.1}, {:.1}, {:.1})",
                                    start.name,
                                    distance_squared.sqrt(),
                                    session.x,
                                    session.y,
                                    session.z
                                ),
                                MovementCorrection::Blocked => println!(
                                    "[join] {}'s move was blocked by terrain; corrected to \
                                     ({:.1}, {:.1}, {:.1})",
                                    start.name, session.x, session.y, session.z
                                ),
                                MovementCorrection::Adjusted => println!(
                                    "[join] {}'s move landed inside terrain; lifted to \
                                     ({:.1}, {:.1}, {:.1})",
                                    start.name, session.x, session.y, session.z
                                ),
                            }
                        }
                        if outcome.fall_damage > 0.0 && !awaiting_respawn {
                            let died =
                                apply_fall_damage(&mut stream, &mut session, outcome.fall_damage)?;
                            if died {
                                awaiting_respawn = true;
                                death_at = Some(Instant::now());
                                if let Err(error) = session.save(playerdata_dir()) {
                                    eprintln!("[join] could not save fall death: {error}");
                                }
                                println!(
                                    "[join] {} died from falling ({:.0} damage)",
                                    start.name, outcome.fall_damage
                                );
                            }
                        }
                        autosave_playerdata(&session, &mut last_save);
                        let new_x = (session.x.floor() as i32).div_euclid(16);
                        let new_z = (session.z.floor() as i32).div_euclid(16);
                        if (new_x, new_z) != (center_x, center_z) {
                            center_x = new_x;
                            center_z = new_z;
                            let batch = match stream_chunks_around(
                                &mut stream,
                                world.storage,
                                world.trust_stored,
                                radius,
                                center_x,
                                center_z,
                                &mut sent_chunks,
                            ) {
                                Ok(batch) => batch,
                                // Clients close the connection mid-stream all the
                                // time; that ends the session, it is not a
                                // server failure.
                                Err(error) if is_disconnect(&error) => {
                                    if let Err(save_error) = session.save(playerdata_dir()) {
                                        eprintln!("[join] could not save playerdata: {save_error}");
                                    }
                                    if let Err(flush_error) =
                                        crate::world::lock(world.storage).flush()
                                    {
                                        eprintln!("[join] could not flush terrain: {flush_error}");
                                    }
                                    println!(
                                        "[join] {} disconnected while streaming chunks: \
                                         {error}",
                                        start.name
                                    );
                                    return Ok(());
                                }
                                Err(error) => return Err(error),
                            };
                            if batch.streamed > 0 || batch.unloaded > 0 {
                                println!(
                                    "[join] streamed {} chunks around ({center_x}, \
                                     {center_z}) for {}, unloaded {}: {}",
                                    batch.streamed,
                                    start.name,
                                    batch.unloaded,
                                    batch.counts.describe()
                                );
                            }
                        }
                    }
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                autosave_playerdata(&session, &mut last_save);
                if last_flush.elapsed() >= TERRAIN_FLUSH_INTERVAL {
                    if let Err(flush_error) = crate::world::lock(world.storage).flush() {
                        eprintln!("[join] could not flush terrain: {flush_error}");
                    }
                    last_flush = Instant::now();
                }
                if last_keepalive.elapsed() >= Duration::from_secs(10) {
                    if let Err(error) =
                        write_packet(&mut stream, &keepalive_packet(keepalive_nonce))
                    {
                        if is_disconnect(&error) {
                            finish_disconnected(&session, world.storage, &start.name, &error);
                            return Ok(());
                        }
                        return Err(error);
                    }
                    keepalive_nonce += 1;
                    last_keepalive = Instant::now();
                }
            }
            Err(error) => {
                if let Err(save_error) = session.save(playerdata_dir()) {
                    eprintln!("[join] could not save playerdata: {save_error}");
                }
                if let Err(flush_error) = crate::world::lock(world.storage).flush() {
                    eprintln!("[join] could not flush terrain: {flush_error}");
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

/// Rebuild the captured Join Game packet with this server's game mode and
/// live view distance. The captured packet otherwise describes the flat
/// creative capture world (game mode 1, `isFlat = true`, chunk radius 3).
fn join_game_packet(
    game_mode: i32,
    chunk_radius: i32,
    simulation_distance: i32,
) -> io::Result<Vec<u8>> {
    let source = play_packets()
        .iter()
        .find(|packet| packet_id(packet).ok() == Some(PLAY_JOIN_GAME))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "captured Join Game is missing")
        })?;
    let mut reader = PacketReader::new(source);
    if reader.read_varint()? != PLAY_JOIN_GAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "captured Join Game has the wrong id",
        ));
    }
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_JOIN_GAME)?;
    writer.write_i32(reader.read_i32()?)?;
    writer.write_u8(u8::from(reader.read_bool()?))?;
    let levels = reader.read_varint()?;
    writer.write_varint(levels)?;
    for _ in 0..levels {
        writer.write_string(&reader.read_string()?)?;
    }
    let _max_players = reader.read_varint()?;
    writer.write_varint(20)?;
    let _chunk_radius = reader.read_varint()?;
    writer.write_varint(chunk_radius)?;
    let _simulation_distance = reader.read_varint()?;
    writer.write_varint(simulation_distance)?;
    for _ in 0..3 {
        writer.write_u8(u8::from(reader.read_bool()?))?;
    }
    writer.write_varint(reader.read_varint()?)?; // dimension type
    writer.write_string(&reader.read_string()?)?; // dimension name
    writer.write_i64(reader.read_i64()?)?; // hashed seed
    let _captured_game_mode = reader.read_u8()?;
    writer.write_u8(game_mode.clamp(0, 3) as u8)?;
    let _previous_game_mode = reader.read_u8()?;
    writer.write_u8(0xff)?; // no previous game mode
    let _debug = reader.read_bool()?;
    writer.write_u8(0)?;
    let _flat = reader.read_bool()?;
    writer.write_u8(0)?;
    if reader.read_bool()? {
        writer.write_u8(1)?;
        writer.write_string(&reader.read_string()?)?;
        writer.write_i64(reader.read_i64()?)?;
    } else {
        writer.write_u8(0)?;
    }
    writer.write_varint(reader.read_varint()?)?; // portal cooldown
    writer.write_varint(reader.read_varint()?)?; // sea level
    writer.write_u8(u8::from(reader.read_bool()?))?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes in captured Join Game",
        ));
    }
    Ok(writer.into_inner())
}

/// `ClientboundPlayerAbilitiesPacket` matching a game mode.
fn abilities_packet(game_mode: i32) -> Vec<u8> {
    let flags = match game_mode {
        1 => 0b0000_1101, // invulnerable, may fly, instant build
        3 => 0b0000_0111, // invulnerable, flying, may fly
        _ => 0,
    };
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_PLAYER_ABILITIES).unwrap();
    writer.write_u8(flags).unwrap();
    writer.write_f32(0.05).unwrap();
    writer.write_f32(0.1).unwrap();
    writer.into_inner()
}

fn game_event_packet(event: u8, param: f32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_GAME_EVENT).unwrap();
    writer.write_u8(event).unwrap();
    writer.write_f32(param).unwrap();
    writer.into_inner()
}

fn game_event_is_change_game_mode(packet: &[u8]) -> bool {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().ok() == Some(PLAY_GAME_EVENT)
        && reader.read_u8().ok() == Some(GAME_EVENT_CHANGE_GAME_MODE)
}

fn game_mode_name(mode: i32) -> &'static str {
    match mode {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    }
}

fn parse_game_mode(value: &str) -> Option<i32> {
    match value.to_ascii_lowercase().as_str() {
        "0" | "survival" | "s" => Some(0),
        "1" | "creative" | "c" => Some(1),
        "2" | "adventure" | "a" => Some(2),
        "3" | "spectator" | "sp" => Some(3),
        _ => None,
    }
}

fn apply_game_mode(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    mode: i32,
) -> io::Result<()> {
    if !(0..=3).contains(&mode) || session.game_mode == mode {
        return Ok(());
    }
    session.game_mode = mode;
    write_packet(
        stream,
        &game_event_packet(GAME_EVENT_CHANGE_GAME_MODE, mode as f32),
    )?;
    write_packet(stream, &abilities_packet(mode))?;
    if let Err(error) = session.save(playerdata_dir()) {
        eprintln!("[join] could not save game mode: {error}");
    }
    println!(
        "[join] {} switched to {} mode",
        session.name,
        game_mode_name(mode)
    );
    Ok(())
}

fn handle_command(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    packet: &[u8],
) -> io::Result<()> {
    let mut reader = PacketReader::new(packet);
    let _ = reader.read_varint()?;
    let command = reader.read_string()?;
    let command = command.strip_prefix('/').unwrap_or(&command);
    let mut parts = command.split_whitespace();
    match parts.next() {
        Some("op") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /op",
                    session.name
                );
                return Ok(());
            }
            let Some(target) = parts.next() else {
                println!("[join] {}: usage: /op <player>", session.name);
                return Ok(());
            };
            if add_operator(target) {
                println!("[join] {} added operator {target}", session.name);
            }
            Ok(())
        }
        Some("deop") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /deop",
                    session.name
                );
                return Ok(());
            }
            let Some(target) = parts.next() else {
                println!("[join] {}: usage: /deop <player>", session.name);
                return Ok(());
            };
            if remove_operator(target) {
                println!("[join] {} removed operator {target}", session.name);
            }
            Ok(())
        }
        Some("gamemode" | "gm") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /gamemode",
                    session.name
                );
                return Ok(());
            }
            let Some(mode) = parts.next().and_then(parse_game_mode) else {
                println!(
                    "[join] {}: usage: /gamemode <survival|creative|adventure|spectator>",
                    session.name
                );
                return Ok(());
            };
            if let Some(target) = parts.next()
                && !target.eq_ignore_ascii_case(&session.name)
            {
                println!(
                    "[join] {}: only self game-mode changes are supported",
                    session.name
                );
                return Ok(());
            }
            apply_game_mode(stream, session, mode)
        }
        _ => Ok(()),
    }
}

/// Snap a player saved in mid-air down to the column's surface. Caves are
/// below the surface height, so this only affects players above the terrain.
fn floating_snap(y: f64, surface: i32, may_snap: bool) -> Option<f64> {
    if may_snap && y > f64::from(surface) {
        Some(f64::from(surface))
    } else {
        None
    }
}

fn health_packet(session: &PlayerSession) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_SET_HEALTH).unwrap();
    writer.write_f32(session.health.clamp(0.0, 20.0)).unwrap();
    writer
        .write_varint(session.food_level.clamp(0, 20))
        .unwrap();
    writer.write_f32(session.food_saturation.max(0.0)).unwrap();
    writer.into_inner()
}

fn player_entity_id() -> i32 {
    for packet in play_packets() {
        if packet_id(packet).ok() != Some(PLAY_JOIN_GAME) {
            continue;
        }
        let mut reader = PacketReader::new(packet);
        let _ = reader.read_varint();
        return reader.read_i32().unwrap_or(1);
    }
    1
}

fn combat_kill_packet(message: &str) -> Vec<u8> {
    let message = Tag::Compound(vec![("text".into(), Tag::String(message.to_owned()))]);
    let encoded = crate::nbt::write_root_to_vec(&message).expect("death message NBT encodes");
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_PLAYER_COMBAT_KILL).unwrap();
    writer.write_varint(player_entity_id()).unwrap();
    writer.write_bytes(&encoded);
    writer.into_inner()
}

fn respawn_packet(game_mode: i32) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_RESPAWN)?;
    writer.write_varint(0)?; // minecraft:overworld dimension type
    writer.write_string("minecraft:overworld")?;
    writer.write_i64(0)?; // hashed seed
    writer.write_u8(game_mode.clamp(0, 3) as u8)?;
    writer.write_u8(0xff)?; // no previous game mode
    writer.write_u8(0)?; // debug
    writer.write_u8(0)?; // flat
    writer.write_u8(0)?; // no last death location
    writer.write_varint(0)?; // portal cooldown
    writer.write_varint(63)?; // sea level
    writer.write_u8(0)?; // data to keep
    Ok(writer.into_inner())
}

fn void_damage_health(health: f32, y: f64, game_mode: i32) -> Option<f32> {
    (game_mode != 3 && y < VOID_DAMAGE_Y).then(|| (health - VOID_DAMAGE).max(0.0))
}

/// Apply vanilla fall damage. Creative and spectator players are immune
/// (`Player.causeFallDamage` returns false when the abilities allow flying);
/// survival and adventure players take damage and receive a health update.
fn fall_damage_health(health: f32, damage: f32, game_mode: i32) -> Option<f32> {
    if damage <= 0.0 || matches!(game_mode, 1 | 3) {
        return None;
    }
    Some((health - damage).max(0.0))
}

fn apply_fall_damage(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    damage: f32,
) -> io::Result<bool> {
    let Some(health) = fall_damage_health(session.health, damage, session.game_mode) else {
        return Ok(false);
    };
    session.health = health;
    write_packet(stream, &health_packet(session))?;
    if session.health <= 0.0 {
        write_packet(
            stream,
            &combat_kill_packet("Player hit the ground too hard"),
        )?;
        return Ok(true);
    }
    println!(
        "[join] {} took {:.0} fall damage; health {:.0}",
        session.name, damage, session.health
    );
    Ok(false)
}

fn apply_void_damage(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    last_damage: &mut Instant,
) -> io::Result<bool> {
    let Some(health) = void_damage_health(session.health, session.y, session.game_mode) else {
        return Ok(false);
    };
    let now = Instant::now();
    if now.duration_since(*last_damage) < VOID_DAMAGE_INTERVAL {
        return Ok(false);
    }
    *last_damage = now;
    session.health = health;
    write_packet(stream, &health_packet(session))?;
    if health <= 0.0 {
        write_packet(stream, &combat_kill_packet("Player fell out of the world"))?;
        return Ok(true);
    }
    println!(
        "[join] {} fell into the void at y={:.1}; health {:.0}",
        session.name, session.y, session.health
    );
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn respawn_player(
    stream: &mut TcpStream,
    session: &mut PlayerSession,
    storage: &Mutex<WorldStorage>,
    trust_stored: bool,
    radius: i32,
    center_x: &mut i32,
    center_z: &mut i32,
    sent_chunks: &mut HashSet<(i32, i32)>,
    spawn: (f64, f64, f64),
    teleport_id: &mut i32,
) -> io::Result<()> {
    session.teleport(spawn.0, spawn.1, spawn.2, true);
    session.health = 20.0;
    write_packet(stream, &respawn_packet(session.game_mode)?)?;
    *teleport_id += 1;
    write_packet(stream, &position_packet(session, *teleport_id))?;
    write_packet(stream, &health_packet(session))?;
    write_packet(stream, &abilities_packet(session.game_mode))?;
    *center_x = (session.x.floor() as i32).div_euclid(16);
    *center_z = (session.z.floor() as i32).div_euclid(16);
    let batch = stream_chunks_around(
        stream,
        storage,
        trust_stored,
        radius,
        *center_x,
        *center_z,
        sent_chunks,
    )?;
    if batch.streamed > 0 || batch.unloaded > 0 {
        println!(
            "[join] respawned {} at ({:.1}, {:.1}, {:.1}); streamed {} chunks",
            session.name, session.x, session.y, session.z, batch.streamed
        );
    }
    if let Err(error) = session.save(playerdata_dir()) {
        eprintln!("[join] could not save respawn: {error}");
    }
    Ok(())
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

/// Where a streamed chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkSource {
    /// Loaded from the region files in the level directory.
    World,
    /// Produced by the noise generator.
    Generated,
    /// Flat placeholder used only when no generator is configured.
    Flat,
}

/// Tally of the chunk sources used by one streaming batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ChunkCounts {
    world: usize,
    generated: usize,
    flat: usize,
}

impl ChunkCounts {
    fn record(&mut self, source: ChunkSource) {
        match source {
            ChunkSource::World => self.world += 1,
            ChunkSource::Generated => self.generated += 1,
            ChunkSource::Flat => self.flat += 1,
        }
    }

    fn describe(self) -> String {
        format!(
            "{} from WorldStorage, {} generated terrain, {} flat placeholders",
            self.world, self.generated, self.flat
        )
    }
}

/// Result of one streaming batch: how many columns went out, how many were
/// unloaded, and where the sent columns came from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ChunkBatch {
    streamed: usize,
    unloaded: usize,
    counts: ChunkCounts,
}

/// How many worker threads generation may use. Generation is CPU bound, so the
/// batch is split across the available cores, capped so a large server does not
/// spawn a thread per chunk batch.
const MAX_GENERATION_WORKERS: usize = 8;

/// How many generated columns may wait for the session to send them. A bounded
/// queue applies backpressure to the workers and, because a failed send is an
/// error, makes them stop as soon as the client is gone instead of generating
/// the rest of a batch nobody will receive.
const GENERATION_QUEUE: usize = 16;

/// How many columns a batch may generate at once.
fn generation_workers(pending: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .clamp(1, MAX_GENERATION_WORKERS)
        .min(pending)
}

/// Send a batch's columns in order while worker threads generate the missing
/// ones, so the first columns reach the client before the whole batch exists.
/// Stored columns keep their place in the same order.
fn send_columns(
    stream: &mut TcpStream,
    storage: &mut WorldStorage,
    generator: Option<&crate::worldgen::generator::WorldGenerator>,
    pending: &[(usize, i32, i32)],
    loaded: &mut [Option<(Chunk, ChunkSource)>],
    batch: &mut ChunkBatch,
) -> io::Result<()> {
    let Some(generator) = generator else {
        for &(index, chunk_x, chunk_z) in pending {
            loaded[index] = Some((
                crate::chunk_packet::flat_chunk(chunk_x, chunk_z),
                ChunkSource::Flat,
            ));
        }
        for column in loaded.iter_mut() {
            let (chunk, source) = column.take().expect("flat columns were just filled in");
            let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
            record_and_send(stream, batch, source, &packet)?;
        }
        return Ok(());
    };
    if pending.is_empty() {
        for column in loaded.iter_mut() {
            if let Some((chunk, source)) = column.take() {
                let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
                record_and_send(stream, batch, source, &packet)?;
            }
        }
        return Ok(());
    }

    let (sender, receiver) = std::sync::mpsc::sync_channel(GENERATION_QUEUE);
    let next = AtomicUsize::new(0);
    let next = &next;
    let workers = generation_workers(pending.len());
    std::thread::scope(|scope| -> io::Result<()> {
        for _ in 0..workers {
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    if shutdown_requested() {
                        break;
                    }
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(index, chunk_x, chunk_z)) = pending.get(position) else {
                        break;
                    };
                    let result = generator
                        .generate_chunk(chunk_x, chunk_z)
                        .and_then(|chunk| {
                            let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
                            Ok((index, chunk, packet))
                        });
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut finished: HashMap<usize, (Chunk, Vec<u8>)> = HashMap::new();
        for (index, column) in loaded.iter_mut().enumerate() {
            // The player already has a position, so a shutdown can arrive while
            // this first batch is still generating. Do not make shutdown wait
            // for hundreds of columns nobody will receive.
            if shutdown_requested() {
                return Ok(());
            }
            let (packet, source) = match column.take() {
                Some((chunk, source)) => (ChunkPacketTemplate::build_dynamic(&chunk)?, source),
                None => {
                    let (chunk, packet) = loop {
                        if let Some(ready) = finished.remove(&index) {
                            break ready;
                        }
                        match receiver.recv() {
                            Ok(Ok((produced, chunk, packet))) => {
                                finished.insert(produced, (chunk, packet));
                            }
                            Ok(Err(error)) => return Err(error),
                            Err(_) if shutdown_requested() => return Ok(()),
                            Err(_) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "chunk generation workers stopped before the batch finished",
                                ));
                            }
                        }
                    };
                    storage.save_chunk(&chunk, Compression::Zlib)?;
                    (packet, ChunkSource::Generated)
                }
            };
            record_and_send(stream, batch, source, &packet)?;
        }
        Ok(())
    })
}

fn record_and_send(
    stream: &mut TcpStream,
    batch: &mut ChunkBatch,
    source: ChunkSource,
    packet: &[u8],
) -> io::Result<()> {
    batch.counts.record(source);
    batch.streamed += 1;
    write_packet(stream, packet)
}

/// Stored chunk when the region files may be trusted, otherwise generated
/// terrain. Generated chunks are written straight back, so terrain survives a
/// restart and block edits in it persist.
fn load_or_generate_chunk(
    storage: &mut WorldStorage,
    trust_stored: bool,
    chunk_x: i32,
    chunk_z: i32,
) -> io::Result<(Chunk, ChunkSource)> {
    let generated = crate::worldgen::generator::configured().is_some();
    let trusted = trust_stored || (generated && storage.is_written_chunk(chunk_x, chunk_z)?);
    if trusted && let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
        return Ok((chunk, ChunkSource::World));
    }
    match crate::worldgen::generator::configured() {
        Some(generator) => {
            let chunk = generator.generate_chunk(chunk_x, chunk_z)?;
            storage.save_chunk(&chunk, Compression::Zlib)?;
            Ok((chunk, ChunkSource::Generated))
        }
        None => Ok((
            crate::chunk_packet::flat_chunk(chunk_x, chunk_z),
            ChunkSource::Flat,
        )),
    }
}

/// The captured spawn packet puts new players at `y = -60`, four blocks above
/// bedrock, which is inside the generated terrain, and playerdata saved from
/// that spawn repeats the problem. Lift any player whose feet block is solid
/// to the first empty block above their column.
fn unstick_player(session: &mut PlayerSession, storage: &Mutex<WorldStorage>, trust_stored: bool) {
    let storage = &mut *crate::world::lock(storage);
    let feet = session.y.floor() as i32;
    let local_x = (session.x.floor() as i32).rem_euclid(16) as usize;
    let local_z = (session.z.floor() as i32).rem_euclid(16) as usize;
    let chunk_x = (session.x.floor() as i32).div_euclid(16);
    let chunk_z = (session.z.floor() as i32).div_euclid(16);
    let Ok((chunk, _)) = load_or_generate_chunk(storage, trust_stored, chunk_x, chunk_z) else {
        return;
    };
    let Some(state) = chunk.block_state(local_x, feet, local_z) else {
        return;
    };
    if crate::chunk::is_air_state(state) {
        // A position saved while falling, or from an older, higher spawn, can
        // leave the player in mid-air above the column. Settle survival and
        // adventure players onto the surface; creative and spectator players
        // are allowed to stay where they logged out.
        let surface = crate::chunk::surface_y(&chunk, local_x, local_z);
        if let Some(y) = floating_snap(session.y, surface, matches!(session.game_mode, 0 | 2)) {
            println!(
                "[join] settle {}: floating at y={:.1}; moved to the surface at y={surface}",
                session.name, session.y
            );
            session.y = y;
            session.on_ground = true;
        }
        return;
    }
    let surface = crate::chunk::surface_y(&chunk, local_x, local_z);
    println!(
        "[join] unstick {}: feet at y={feet} inside {}; moved to the surface at y={surface}",
        session.name, state.name
    );
    session.y = f64::from(surface);
    session.on_ground = true;
}

/// Whether an I/O error means the client is gone, as opposed to a failure this
/// server should report.
fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

/// Best-effort save/flush when a write discovers the client has gone away.
fn finish_disconnected(
    session: &PlayerSession,
    storage: &Mutex<WorldStorage>,
    name: &str,
    error: &io::Error,
) {
    if let Err(save_error) = session.save(playerdata_dir()) {
        eprintln!("[join] could not save playerdata: {save_error}");
    }
    if let Err(flush_error) = crate::world::lock(storage).flush() {
        eprintln!("[join] could not flush terrain: {flush_error}");
    }
    println!("[join] {name} disconnected: {error}");
}

/// Everything a session needs from the level directory.
struct WorldContext {
    storage: &'static Mutex<WorldStorage>,
    /// Whether every stored chunk may be served, not just the ones this server
    /// wrote. True for a foreign world and when `FOLIA_LOAD_STORED=1`.
    trust_stored: bool,
    /// World spawn from `level.dat`, when the level has one.
    spawn: Option<(i32, i32, i32)>,
}

/// Open the level directory, creating `level.dat` the first time a world is
/// generated.
///
/// A world this server created carries [`crate::level::OWN_LEVEL_MARKER`], so
/// only chunks it wrote itself (stamped with [`crate::world::WRITTEN_MARKER`])
/// are trusted; leftover flat chunks next to them are regenerated. A level that
/// came from somewhere else has no marker, and every stored chunk is served.
/// `FOLIA_LOAD_STORED=1` trusts every stored chunk in either case.
fn open_world() -> WorldContext {
    let directory = world_dir();
    let forced = std::env::var("FOLIA_LOAD_STORED")
        .map(|value| matches!(value.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let storage = crate::world::shared_overworld(directory.clone());
    let level_path = directory.join("level.dat");
    if forced {
        return WorldContext {
            storage,
            trust_stored: true,
            spawn: crate::level::read(&level_path)
                .ok()
                .and_then(|level| level.spawn),
        };
    }
    match crate::level::read(&level_path) {
        Ok(level) => {
            if level.own_level {
                println!(
                    "[join] level {} was created by this server; serving stored chunks stamped \
                     with the terrain marker",
                    level.level_name
                );
            } else {
                println!(
                    "[join] level {} comes from another server; serving its stored chunks",
                    level.level_name
                );
            }
            return WorldContext {
                storage,
                trust_stored: !level.own_level,
                spawn: level.spawn,
            };
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            // Never overwrite a level file we cannot read.
            eprintln!(
                "[join] could not read {}: {error}; serving stored chunks unmodified",
                level_path.display()
            );
            return WorldContext {
                storage,
                trust_stored: true,
                spawn: None,
            };
        }
    }

    let Some(generator) = crate::worldgen::generator::configured() else {
        if directory.join("region").is_dir() {
            println!(
                "[join] world has region files but no level.dat and no generator; serving stored \
                 chunks (set FOLIA_LOAD_STORED=1 to silence this)"
            );
        }
        return WorldContext {
            storage,
            trust_stored: true,
            spawn: None,
        };
    };

    let seed = generator.seed();
    // Anchor the world spawn on the terrain surface at the origin, so the spawn
    // is saved with the level instead of guessed per session.
    let spawn = match load_or_generate_chunk(&mut crate::world::lock(storage), false, 0, 0) {
        Ok((chunk, _)) => (8, crate::chunk::surface_y(&chunk, 8, 8), 8),
        Err(error) => {
            eprintln!("[join] could not generate the spawn chunk for level.dat: {error}");
            return WorldContext {
                storage,
                trust_stored: false,
                spawn: None,
            };
        }
    };
    let level_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("world")
        .to_owned();
    match crate::level::write_new_world(&level_path, &level_name, seed, spawn) {
        Ok(()) => {
            println!(
                "[join] created {} at seed {seed} with spawn ({}, {}, {})",
                level_path.display(),
                spawn.0,
                spawn.1,
                spawn.2
            );
            crate::world::lock(storage).flush().ok();
            WorldContext {
                storage,
                trust_stored: false,
                spawn: Some(spawn),
            }
        }
        Err(error) => {
            eprintln!("[join] could not write {}: {error}", level_path.display());
            WorldContext {
                storage,
                trust_stored: false,
                spawn: None,
            }
        }
    }
}

/// Sends the chunk-cache center, unloads columns that fell outside the live
/// radius, and sends every chunk in the radius that `sent` has not streamed
/// yet.
fn stream_chunks_around(
    stream: &mut TcpStream,
    storage: &Mutex<WorldStorage>,
    trust_stored: bool,
    radius: i32,
    center_x: i32,
    center_z: i32,
    sent: &mut HashSet<(i32, i32)>,
) -> io::Result<ChunkBatch> {
    let storage = &mut *crate::world::lock(storage);
    write_packet(stream, &chunk_cache_center(center_x, center_z))?;
    let mut batch = ChunkBatch::default();
    for (chunk_x, chunk_z) in chunks_to_unload(sent, center_x, center_z, radius) {
        write_packet(stream, &forget_chunk_packet(chunk_x, chunk_z))?;
        sent.remove(&(chunk_x, chunk_z));
        batch.unloaded += 1;
    }

    // Columns that go out with this batch, in send order.
    let mut columns = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let chunk_x = center_x + dx;
            let chunk_z = center_z + dz;
            if sent.insert((chunk_x, chunk_z)) {
                columns.push((chunk_x, chunk_z));
            }
        }
    }

    // Nearest columns first: they are the ones the client needs to stand on.
    columns.sort_by_key(|&(chunk_x, chunk_z)| {
        let dx = chunk_x - center_x;
        let dz = chunk_z - center_z;
        (dx * dx + dz * dz, dz, dx)
    });

    // Stored terrain first; everything else is generated on worker threads so a
    // cold batch uses more than one core.
    let generated = crate::worldgen::generator::configured().is_some();
    let mut loaded: Vec<Option<(Chunk, ChunkSource)>> = Vec::with_capacity(columns.len());
    let mut pending = Vec::new();
    for (index, (chunk_x, chunk_z)) in columns.iter().copied().enumerate() {
        let trusted = trust_stored || (generated && storage.is_written_chunk(chunk_x, chunk_z)?);
        if trusted && let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
            loaded.push(Some((chunk, ChunkSource::World)));
            continue;
        }
        loaded.push(None);
        pending.push((index, chunk_x, chunk_z));
    }
    let generator = crate::worldgen::generator::configured().map(|generator| generator.as_ref());
    send_columns(
        stream,
        storage,
        generator,
        &pending,
        &mut loaded,
        &mut batch,
    )?;
    // Generated terrain stays in the region cache until the session's scheduled
    // flush: a region file is rewritten as a whole file, so flushing per batch
    // would rewrite the same multi-megabyte file on every border crossing.
    Ok(batch)
}

/// Columns the client still holds that are more than `radius + UNLOAD_MARGIN`
/// chunks away from the player on either axis.
fn chunks_to_unload(
    sent: &HashSet<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> Vec<(i32, i32)> {
    let limit = radius + UNLOAD_MARGIN;
    let mut stale: Vec<(i32, i32)> = sent
        .iter()
        .copied()
        .filter(|(chunk_x, chunk_z)| {
            (chunk_x - center_x).abs() > limit || (chunk_z - center_z).abs() > limit
        })
        .collect();
    stale.sort_unstable();
    stale
}

fn forget_chunk_packet(chunk_x: i32, chunk_z: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_FORGET_CHUNK).unwrap();
    writer
        .write_i64((i64::from(chunk_x) & 0xffff_ffff) | (i64::from(chunk_z) << 32))
        .unwrap();
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
    storage: &Mutex<WorldStorage>,
    session: &mut PlayerSession,
    packet: &[u8],
    id: i32,
) -> io::Result<()> {
    if matches!(session.game_mode, 2 | 3) {
        return Ok(());
    }
    let storage = &mut *crate::world::lock(storage);
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
                if session.game_mode != 1 {
                    session.consume_selected();
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anvil::{Compression, RegionFile};
    use crate::chunk::encode_chunk;
    use crate::chunk_packet::flat_chunk;
    use crate::world::RegionPos;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_region_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "folia-rs-chunk-source-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn written_terrain_reloads_instead_of_regenerating() {
        crate::worldgen::generator::configure(4242).expect("configure generator");
        let directory = temp_region_dir("marked");
        let mut storage = WorldStorage::new(directory.clone());

        let (generated, source) = load_or_generate_chunk(&mut storage, false, 0, 0).unwrap();
        assert_eq!(source, ChunkSource::Generated);

        // A world without `level.dat` still trusts terrain this server wrote.
        let (reloaded, source) = load_or_generate_chunk(&mut storage, false, 0, 0).unwrap();
        assert_eq!(source, ChunkSource::World);
        assert_eq!(
            crate::chunk::surface_y(&reloaded, 0, 0),
            crate::chunk::surface_y(&generated, 0, 0),
            "same column height as the chunk that was generated"
        );
        assert_eq!(
            reloaded
                .block_state(0, -64, 0)
                .map(|state| state.name.as_str()),
            Some("minecraft:bedrock")
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn block_edits_persist_in_generated_terrain() {
        crate::worldgen::generator::configure(4242).expect("configure generator");
        let directory = temp_region_dir("edit");
        let mut storage = WorldStorage::new(directory.clone());
        let (chunk, _) = load_or_generate_chunk(&mut storage, false, 0, 0).unwrap();
        let surface = crate::chunk::surface_y(&chunk, 0, 0);

        let air = crate::chunk::BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        };
        let change = crate::block_update::set_block(&mut storage, 0, surface - 1, 0, air)
            .unwrap()
            .expect("block break on generated terrain");
        assert_ne!(
            change.old_state.map(|state| state.name),
            Some("minecraft:air".to_owned()),
            "the broken block was solid"
        );

        let mut reopened = WorldStorage::new(directory.clone());
        let (reloaded, source) = load_or_generate_chunk(&mut reopened, false, 0, 0).unwrap();
        assert_eq!(source, ChunkSource::World);
        assert_eq!(
            reloaded
                .block_state(0, surface - 1, 0)
                .map(|state| state.name.as_str()),
            Some("minecraft:air"),
            "the edit survives reopening the region files"
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn unmarked_leftover_chunks_are_ignored_unless_forced() {
        crate::worldgen::generator::configure(4242).expect("configure generator");
        let directory = temp_region_dir("leftover");
        let position = RegionPos::from_chunk(0, 0);
        let (local_x, local_z) = position.local_chunk(0, 0);
        {
            let path = directory.join(position.file_name());
            let mut region = RegionFile::open_or_create(&path).unwrap();
            let tag = encode_chunk(&flat_chunk(0, 0)).unwrap();
            region
                .write_chunk(local_x, local_z, &tag, Compression::Zlib)
                .unwrap();
            region.save().unwrap();
        }

        let mut storage = WorldStorage::new(directory.clone());
        let (stored, source) = load_or_generate_chunk(&mut storage, true, 0, 0).unwrap();
        assert_eq!(source, ChunkSource::World);
        assert_eq!(stored, flat_chunk(0, 0), "forced reads serve stored chunks");

        let (chunk, source) = load_or_generate_chunk(&mut storage, false, 0, 0).unwrap();
        assert_eq!(source, ChunkSource::Generated, "leftover chunk is ignored");
        assert!(
            chunk
                .block_state(0, -60, 0)
                .is_some_and(|state| !crate::chunk::is_air_state(state)),
            "generated terrain, not the flat placeholder"
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn playerdata_autosave_waits_for_the_interval() {
        let now = Instant::now();
        assert!(!autosave_due(now, now, PLAYER_SAVE_INTERVAL));
        assert!(!autosave_due(
            now - Duration::from_secs(4),
            now,
            PLAYER_SAVE_INTERVAL
        ));
        assert!(autosave_due(
            now - Duration::from_secs(5),
            now,
            PLAYER_SAVE_INTERVAL
        ));
    }

    #[test]
    fn only_columns_beyond_the_margin_are_unloaded() {
        let mut sent: HashSet<(i32, i32)> = HashSet::new();
        for chunk_x in -6..=6 {
            for chunk_z in -6..=6 {
                sent.insert((chunk_x, chunk_z));
            }
        }
        assert!(
            chunks_to_unload(&sent, 0, 0, 6).is_empty(),
            "the live radius itself is never unloaded"
        );
        assert!(
            chunks_to_unload(&sent, 1, 0, 6).is_empty(),
            "adjacent columns stay inside the unload margin"
        );

        let stale = chunks_to_unload(&sent, 3, 0, 6);
        assert_eq!(stale.len(), 26, "the two columns left behind are unloaded");
        assert!(
            stale
                .iter()
                .all(|(chunk_x, chunk_z)| (chunk_x - 3).abs() > 7 || chunk_z.abs() > 7),
            "every unloaded column is outside the margin"
        );
        let mut columns: Vec<i32> = stale.iter().map(|(chunk_x, _)| *chunk_x).collect();
        columns.dedup();
        assert_eq!(
            columns,
            vec![-6, -5],
            "the columns left behind are unloaded"
        );
    }

    #[test]
    fn a_smaller_view_distance_unloads_more_columns() {
        let mut sent: HashSet<(i32, i32)> = HashSet::new();
        for chunk_x in -6..=6 {
            for chunk_z in -6..=6 {
                sent.insert((chunk_x, chunk_z));
            }
        }
        // Radius 2 keeps a 5×5 area and one chunk of margin: |dx| <= 3.
        let stale = chunks_to_unload(&sent, 0, 0, 2);
        assert_eq!(stale.len(), 169 - 49);
        assert!(
            stale
                .iter()
                .all(|(chunk_x, chunk_z)| chunk_x.abs() > 3 || chunk_z.abs() > 3)
        );
    }

    #[test]
    fn client_view_distance_is_read_from_client_information() {
        let mut writer = PacketWriter::new();
        writer.write_varint(CONFIG_CLIENT_INFORMATION).unwrap();
        writer.write_string("en_us").unwrap();
        writer.write_u8(8).unwrap();
        writer.write_varint(0).unwrap();
        writer.write_u8(1).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_varint(0).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_varint(0).unwrap();
        assert_eq!(parse_client_view_distance(&writer.into_inner()), Some(8));

        let mut other = PacketWriter::new();
        other.write_varint(CONFIG_KEEP_ALIVE).unwrap();
        assert_eq!(parse_client_view_distance(&other.into_inner()), None);
    }

    #[test]
    fn forget_chunk_packet_packs_the_column_position() {
        let packet = forget_chunk_packet(-3, 7);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_FORGET_CHUNK);
        let packed = reader.read_i64().unwrap();
        assert_eq!(packed as i32, -3);
        assert_eq!((packed >> 32) as i32, 7);
        assert_eq!(reader.remaining(), 0, "nothing but the column is sent");
    }

    #[test]
    fn join_game_rewrites_the_captured_world_state() {
        let packet = join_game_packet(0, 8, 7).expect("join packet");
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_JOIN_GAME);
        let _ = reader.read_i32().unwrap();
        let _ = reader.read_bool().unwrap();
        let levels = reader.read_varint().unwrap();
        for _ in 0..levels {
            let _ = reader.read_string().unwrap();
        }
        let _ = reader.read_varint().unwrap();
        assert_eq!(reader.read_varint().unwrap(), 8, "chunk radius");
        assert_eq!(reader.read_varint().unwrap(), 7, "simulation distance");
        let _ = (
            reader.read_bool().unwrap(),
            reader.read_bool().unwrap(),
            reader.read_bool().unwrap(),
        );
        let _ = reader.read_varint().unwrap();
        let _ = reader.read_string().unwrap();
        let _ = reader.read_i64().unwrap();
        assert_eq!(reader.read_u8().unwrap(), 0, "survival game mode");
        assert_eq!(reader.read_u8().unwrap(), 0xff, "no previous game mode");
        assert!(!reader.read_bool().unwrap(), "not a debug world");
        assert!(!reader.read_bool().unwrap(), "not a flat world");
        assert!(!reader.read_bool().unwrap(), "no death location");
        let _ = reader.read_varint().unwrap();
        let _ = reader.read_varint().unwrap();
        let _ = reader.read_bool().unwrap();
        assert_eq!(reader.remaining(), 0, "the rewritten packet is complete");
    }

    #[test]
    fn abilities_follow_the_game_mode() {
        let creative = abilities_packet(1);
        let mut reader = PacketReader::new(&creative);
        assert_eq!(reader.read_varint().unwrap(), PLAY_PLAYER_ABILITIES);
        assert_eq!(reader.read_u8().unwrap() & 0x0d, 0x0d);

        let survival = abilities_packet(0);
        let mut reader = PacketReader::new(&survival);
        assert_eq!(reader.read_varint().unwrap(), PLAY_PLAYER_ABILITIES);
        assert_eq!(reader.read_u8().unwrap(), 0);
    }

    #[test]
    fn game_mode_names_and_ids_round_trip() {
        for (name, mode) in [
            ("survival", 0),
            ("creative", 1),
            ("adventure", 2),
            ("spectator", 3),
        ] {
            assert_eq!(parse_game_mode(name), Some(mode));
            assert_eq!(game_mode_name(mode), name);
        }
        assert_eq!(parse_game_mode("nope"), None);
    }

    #[test]
    fn survival_players_saved_in_mid_air_are_settled() {
        assert_eq!(floating_snap(120.0, 70, true), Some(70.0));
        assert_eq!(floating_snap(70.0, 70, true), None);
        assert_eq!(
            floating_snap(40.0, 70, true),
            None,
            "a player in a cave is below the surface"
        );
        assert_eq!(
            floating_snap(120.0, 70, false),
            None,
            "creative and spectator players may fly"
        );
    }
    #[test]
    fn void_damage_applies_below_the_world_and_spares_spectators() {
        assert_eq!(void_damage_health(20.0, -128.1, 0), Some(16.0));
        assert_eq!(void_damage_health(2.0, -129.0, 0), Some(0.0));
        assert_eq!(void_damage_health(20.0, -127.9, 0), None);
        assert_eq!(void_damage_health(20.0, -129.0, 3), None);
    }

    #[test]
    fn fall_damage_spares_creative_and_reports_survival_health() {
        assert_eq!(fall_damage_health(20.0, 7.0, 0), Some(13.0));
        assert_eq!(fall_damage_health(20.0, 7.0, 2), Some(13.0));
        assert_eq!(fall_damage_health(4.0, 7.0, 0), Some(0.0));
        assert_eq!(fall_damage_health(20.0, 7.0, 1), None);
        assert_eq!(fall_damage_health(20.0, 7.0, 3), None);
        assert_eq!(fall_damage_health(20.0, 0.0, 0), None);
    }

    #[test]
    fn health_and_respawn_packets_encode_live_session_state() {
        let mut session = PlayerSession::new("Alex".into(), [0x55; 16], 0.0, 0.0, 0.0);
        session.health = 13.5;
        session.food_level = 11;
        session.food_saturation = 2.25;
        let packet = health_packet(&session);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_SET_HEALTH);
        assert_eq!(reader.read_f32().unwrap(), 13.5);
        assert_eq!(reader.read_varint().unwrap(), 11);
        assert_eq!(reader.read_f32().unwrap(), 2.25);
        assert!(reader.is_empty());

        let packet = respawn_packet(1).unwrap();
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_RESPAWN);
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert_eq!(reader.read_string().unwrap(), "minecraft:overworld");
        assert_eq!(reader.read_i64().unwrap(), 0);
        assert_eq!(reader.read_u8().unwrap(), 1);
        assert_eq!(reader.read_u8().unwrap(), 0xff);
        assert!(!reader.read_bool().unwrap());
        assert!(!reader.read_bool().unwrap());
        assert!(!reader.read_bool().unwrap());
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert_eq!(reader.read_varint().unwrap(), 63);
        assert_eq!(reader.read_u8().unwrap(), 0);
        assert!(reader.is_empty());
    }

    #[test]
    fn console_op_is_case_insensitive() {
        let output = execute_console_command("op ConsoleTest");
        assert_eq!(output, vec!["added ConsoleTest to operators".to_owned()]);
        assert!(is_operator("consoletest"));
        assert_eq!(
            execute_console_command("deop consoletest"),
            vec!["removed consoletest from operators".to_owned()]
        );
        assert!(!is_operator("ConsoleTest"));
    }
    #[test]
    fn console_gamemode_reaches_a_registered_session() {
        let (guard, receiver) = ConsoleSessionGuard::register("ConsoleTarget", [0x9a; 16]);
        let worker =
            std::thread::spawn(
                move || match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(SessionCommand::SetGameMode { mode, reply }) => {
                        assert_eq!(mode, 1);
                        let _ = reply.send(Ok(()));
                    }
                    other => panic!("unexpected console command: {other:?}"),
                },
            );

        let output = execute_console_command("gamemode creative ConsoleTarget");
        assert_eq!(
            output,
            vec!["set ConsoleTarget's game mode to creative".to_owned()]
        );
        worker.join().unwrap();
        drop(guard);
    }
}
