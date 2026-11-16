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
use crate::block_breaking::{self, DigStart, DigState, DigStop, MiningContext};
use crate::block_entity::{BlockEntity, BlockEntityPos};
use crate::block_update::{SERVERBOUND_PLAYER_ACTION, SERVERBOUND_USE_ITEM_ON};
use crate::chunk::{BlockState, Chunk};
use crate::chunk_packet::ChunkPacketTemplate;
use crate::combat::AttackCooldown;
use crate::crypto::{self, CryptoError, RsaKeyPair, constant_time_eq, server_hash};
use crate::dimension::{self, PortalKind, PortalRequest, SessionDimensionState};
use crate::entity::{BlockCollider, SessionEntityTracker, WorldCollider};
use crate::inventory;
use crate::item_entity::{DropEntityStore, PickupTarget};
use crate::math::{BlockPos, Vec3};
use crate::nbt::Tag;
use crate::player::{
    ChatPublicKey, PlayerSession, ProfilePublicKey, RemoteChatSession, SignedMessage, uuid_string,
};
use crate::player_physics::{MovementCorrection, StoragePhysicsWorld};
use crate::protocol::{
    Connection, EncryptionRequest, GameProfile, Handshake, LAST_SEEN_MESSAGES_WINDOW,
    LastSeenValidator, PacketReader, PacketWriter, ProfileProperty, SERVERBOUND_CHAT,
    SERVERBOUND_CHAT_SESSION_UPDATE, SignedMessageBody, build_disguised_chat_packet,
    build_encryption_request, build_login_disconnect, build_login_success,
    build_player_chat_packet, parse_encryption_response, parse_login_start, parse_serverbound_chat,
    parse_serverbound_chat_session_update,
};
use crate::world::WorldStorage;
use folia_random::LegacyRandomSource;
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The login/configuration/play connection. Encryption is enabled in place
/// after the encryption response, so every later packet is transformed at the
/// frame boundary.
type LoginStream = Connection<TcpStream>;

fn read_packet(stream: &mut LoginStream) -> io::Result<Vec<u8>> {
    stream.read_packet()
}

fn write_packet(stream: &mut LoginStream, payload: &[u8]) -> io::Result<()> {
    if shutdown_requested() {
        return Ok(());
    }
    stream.write_packet_while(payload, || !shutdown_requested())?;
    Ok(())
}

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
/// `ClientboundDisconnectPacket` in the play protocol (common packet id 32).
const PLAY_DISCONNECT: i32 = 32;
/// `ClientboundSetTimePacket` (game time, day time, tick-day-time flag).
const PLAY_SET_TIME: i32 = 111;
/// `ClientboundSoundPacket` (direct sound event, source, position, volume,
/// pitch, seed).
const PLAY_SOUND: i32 = 115;
/// `ClientboundLevelParticlesPacket`.
const PLAY_LEVEL_PARTICLES: i32 = 46;
/// `ClientboundSystemChatPacket` (component NBT, overlay flag).
const PLAY_SYSTEM_CHAT: i32 = 119;

/// `ClientboundPlayerCombatKill`/`SetHealth`/`Respawn` IDs are the live
/// 1.21.11 IDs; `PLAY_SET_HEALTH` is also present in the captured join stream.
const SERVERBOUND_CLIENT_COMMAND: i32 = 11;
const CLIENT_COMMAND_PERFORM_RESPAWN: i32 = 0;

/// Vanilla starts void damage below `minY - 64`; the overworld floor is -64.
const VOID_DAMAGE_Y: f64 = -128.0;
const VOID_DAMAGE_INTERVAL: Duration = Duration::from_millis(500);
const VOID_DAMAGE: f32 = 4.0;

/// Movement catch-up budget after a server stall. A client that has not sent a
/// movement packet for `elapsed` seconds may legitimately be this far away.
/// The base 10 blocks matches the normal per-packet guard; the rate covers
/// sprinting, creative flight, and most elytra travel without letting an
/// instant teleport through.
const CATCH_UP_BLOCKS_PER_SECOND: f64 = 32.0;
const MAX_CATCH_UP_SECONDS: f64 = 300.0;

const SERVERBOUND_CHANGE_GAME_MODE: i32 = 4;
const SERVERBOUND_CHAT_COMMAND: i32 = 6;
const SERVERBOUND_CHAT_COMMAND_SIGNED: i32 = 7;
/// `ServerboundInteractPacket` in the 1.21.11 play protocol.
const SERVERBOUND_INTERACT: i32 = 25;

/// `ClientboundGameEventPacket.CHANGE_GAME_MODE`.
const GAME_EVENT_CHANGE_GAME_MODE: u8 = 3;
/// Columns this far outside the live radius are unloaded, so chunks the client
/// may still be rendering at the edge are not dropped the moment the player
/// crosses a border.
const UNLOAD_MARGIN: i32 = 1;
/// View distance used when neither the server nor the client asks for one.
const DEFAULT_VIEW_DISTANCE: i32 = 6;

/// How many columns the initial join sends before entering the play loop. The
/// nearest columns go first; the rest are topped up while the client can move.
const INITIAL_STREAM_BATCH: usize = 64;
/// Columns generated/sent per play-loop top-up. Small enough that movement
/// packets are read between batches even on a slow debug build.
const STREAM_TOPUP_BATCH: usize = 24;
/// How often the play loop fills the remaining view-distance columns.
const CHUNK_TOPUP_INTERVAL: Duration = Duration::from_millis(50);

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

/// The play loop wakes at this cadence so tracked entities can tick at 20 Hz
/// while a blocking socket read waits for the next client packet.
const SESSION_TICK_INTERVAL: Duration = Duration::from_millis(50);

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
    /// `ClientboundSystemChat` with a plain-text component.
    Chat { message: String },
    /// A player chat message broadcast to every live session. Signed messages
    /// use `ClientboundPlayerChat`; unsigned messages use Disguised Chat.
    PlayerChat {
        sender: [u8; 16],
        sender_name: String,
        message: Box<SignedMessage>,
    },
    /// Play-state disconnect with a text component reason.
    Disconnect { reason: String },
    /// Authoritative move; the play loop writes `ClientboundPlayerPosition`.
    Teleport {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
    },
    /// `ClientboundSetTime` (game time, day time, tick-day-time flag).
    SetTime {
        game_time: i64,
        day_time: i64,
        tick_day_time: bool,
    },
    /// `ClientboundGameEvent` (rain start/stop, rain level, thunder level, ...).
    GameEvent { event: u8, param: f32 },
    /// `ClientboundSound` using an inline (direct) sound event, so no sound
    /// registry id table is needed. The name must already be namespaced.
    Sound {
        sound: String,
        category: i32,
        x: f64,
        y: f64,
        z: f64,
        volume: f32,
        pitch: f32,
        seed: i64,
    },
    /// `ClientboundLevelParticles` for a simple particle type.
    Particle {
        particle: String,
        x: f64,
        y: f64,
        z: f64,
        offset_x: f32,
        offset_y: f32,
        offset_z: f32,
        speed: f32,
        count: i32,
        force: bool,
    },
    /// Open the chest/furnace/brewing-stand menu backed by this position.
    OpenMenu { position: BlockEntityPos },
}

struct ActiveSession {
    /// Unique per connection, so a reconnect with the same UUID cannot have its
    /// registration removed by the previous connection's guard.
    id: u64,
    name: String,
    sender: mpsc::Sender<SessionCommand>,
}

static NEXT_SESSION_ID: AtomicUsize = AtomicUsize::new(1);
static SESSIONS: LazyLock<Mutex<HashMap<[u8; 16], ActiveSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static OPERATORS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static OPS_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Registration for one live play session. Dropping it removes the session
/// from the registry, so the bridge can never push commands to a closed socket.
struct SessionCommandGuard {
    id: u64,
    uuid: [u8; 16],
}

impl SessionCommandGuard {
    fn register(name: &str, uuid: [u8; 16]) -> (Self, mpsc::Receiver<SessionCommand>) {
        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst) as u64;
        let (sender, receiver) = mpsc::channel();
        SESSIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                uuid,
                ActiveSession {
                    id,
                    name: name.to_owned(),
                    sender,
                },
            );
        (Self { id, uuid }, receiver)
    }
}

impl Drop for SessionCommandGuard {
    fn drop(&mut self) {
        let mut sessions = SESSIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sessions
            .get(&self.uuid)
            .is_some_and(|session| session.id == self.id)
        {
            sessions.remove(&self.uuid);
        }
    }
}

fn session_sender(uuid: [u8; 16]) -> Option<mpsc::Sender<SessionCommand>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&uuid)
        .map(|session| session.sender.clone())
}

fn session_sender_by_name(name: &str) -> Option<mpsc::Sender<SessionCommand>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .find(|session| session.name.eq_ignore_ascii_case(name))
        .map(|session| session.sender.clone())
}

fn active_session_names() -> Vec<String> {
    let mut names: Vec<String> = SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .map(|session| session.name.clone())
        .collect();
    names.sort();
    names
}

/// Push one command to the live session for `uuid`. Returns false when the
/// player is not connected (or the session has already closed its channel).
pub fn send_player_chat(uuid: [u8; 16], message: &str) -> bool {
    session_sender(uuid).is_some_and(|sender| {
        sender
            .send(SessionCommand::Chat {
                message: message.to_owned(),
            })
            .is_ok()
    })
}

/// Name-keyed variant for callers that only have a player name.
pub fn send_player_chat_by_name(name: &str, message: &str) -> bool {
    session_sender_by_name(name).is_some_and(|sender| {
        sender
            .send(SessionCommand::Chat {
                message: message.to_owned(),
            })
            .is_ok()
    })
}

/// Broadcast one player chat message to every live session. The sender also
/// receives the command, matching vanilla's loopback chat delivery.
fn broadcast_player_chat(sender: [u8; 16], sender_name: &str, message: &SignedMessage) -> usize {
    broadcast_command(|_| {
        Some(SessionCommand::PlayerChat {
            sender,
            sender_name: sender_name.to_owned(),
            message: Box::new(message.clone()),
        })
    })
}

/// Ask the live session to open the container menu backed by `position`.
///
/// This is the server-side `open_menu` entry point (the Rust equivalent of
/// `Player.openInventory`); the play loop validates that a chest/furnace/
/// brewing-stand block entity exists and writes `ClientboundOpenScreen`,
/// `ClientboundContainerSetContent`, and the menu's data slots. Returns false
/// when the player is not connected or the command channel is closed.
pub fn open_menu(uuid: [u8; 16], position: BlockEntityPos) -> bool {
    session_sender(uuid)
        .is_some_and(|sender| sender.send(SessionCommand::OpenMenu { position }).is_ok())
}

/// Convenience variant for callers that only have block coordinates.
pub fn open_menu_at(uuid: [u8; 16], x: i32, y: i32, z: i32) -> bool {
    open_menu(uuid, BlockEntityPos::new(x, y, z))
}

/// Ask the live session to write a play-state disconnect and close. Returns
/// false when the player is not connected.
pub fn kick_player(uuid: [u8; 16], reason: &str) -> bool {
    session_sender(uuid).is_some_and(|sender| {
        sender
            .send(SessionCommand::Disconnect {
                reason: reason.to_owned(),
            })
            .is_ok()
    })
}

/// Teleport the live session. The play loop sets the authoritative position,
/// writes `ClientboundPlayerPosition` and re-streams chunks when the column
/// changed. Returns false when the player is not connected or the target is
/// not finite.
pub fn teleport_player(uuid: [u8; 16], x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> bool {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() || !yaw.is_finite() || !pitch.is_finite()
    {
        return false;
    }
    session_sender(uuid).is_some_and(|sender| {
        sender
            .send(SessionCommand::Teleport {
                x,
                y,
                z,
                yaw,
                pitch,
            })
            .is_ok()
    })
}

/// Broadcast `ClientboundSetTime` to every live session. Returns the number of
/// commands queued.
pub fn broadcast_world_time(game_time: i64, day_time: i64, tick_day_time: bool) -> usize {
    broadcast_command(|_| {
        Some(SessionCommand::SetTime {
            game_time,
            day_time,
            tick_day_time,
        })
    })
}

/// Broadcast one `ClientboundGameEvent` value to every live session.
pub fn broadcast_game_event(event: u8, param: f32) -> usize {
    broadcast_command(|_| Some(SessionCommand::GameEvent { event, param }))
}

/// Broadcast a sound to every live session. The sound event is sent inline
/// (direct holder), which the 1.21.11 client resolves by its resource location.
#[allow(clippy::too_many_arguments)]
pub fn broadcast_sound(
    sound: &str,
    category: i32,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> usize {
    broadcast_command(|_| {
        Some(SessionCommand::Sound {
            sound: namespaced_sound(sound)?,
            category,
            x,
            y,
            z,
            volume,
            pitch,
            seed,
        })
    })
}

/// Send a sound to one live session only (player-level `playSound`).
#[allow(clippy::too_many_arguments)]
pub fn send_player_sound(
    uuid: [u8; 16],
    sound: &str,
    category: i32,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> bool {
    let Some(sound) = namespaced_sound(sound) else {
        return false;
    };
    session_sender(uuid).is_some_and(|sender| {
        sender
            .send(SessionCommand::Sound {
                sound,
                category,
                x,
                y,
                z,
                volume,
                pitch,
                seed,
            })
            .is_ok()
    })
}

/// Send a simple particle to one live session only.
#[allow(clippy::too_many_arguments)]
pub fn send_player_particle(
    uuid: [u8; 16],
    particle: &str,
    x: f64,
    y: f64,
    z: f64,
    offset_x: f32,
    offset_y: f32,
    offset_z: f32,
    speed: f32,
    count: i32,
    force: bool,
) -> bool {
    let particle = particle.to_owned();
    session_sender(uuid).is_some_and(|sender| {
        sender
            .send(SessionCommand::Particle {
                particle,
                x,
                y,
                z,
                offset_x,
                offset_y,
                offset_z,
                speed,
                count,
                force,
            })
            .is_ok()
    })
}

/// Broadcast a simple particle to every live session. Names outside the simple
/// particle table are rejected loudly by the play loop.
#[allow(clippy::too_many_arguments)]
pub fn broadcast_particle(
    particle: &str,
    x: f64,
    y: f64,
    z: f64,
    offset_x: f32,
    offset_y: f32,
    offset_z: f32,
    speed: f32,
    count: i32,
    force: bool,
) -> usize {
    broadcast_command(|_| {
        Some(SessionCommand::Particle {
            particle: particle.to_owned(),
            x,
            y,
            z,
            offset_x,
            offset_y,
            offset_z,
            speed,
            count,
            force,
        })
    })
}

fn broadcast_command(mut command: impl FnMut(&ActiveSession) -> Option<SessionCommand>) -> usize {
    let sessions = SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut sent = 0;
    for session in sessions.values() {
        if let Some(command) = command(session)
            && session.sender.send(command).is_ok()
        {
            sent += 1;
        }
    }
    sent
}

/// Sound identifiers are resource locations. Bare Bukkit names are treated as
/// `minecraft:` keys; an already namespaced key is passed through.
fn namespaced_sound(sound: &str) -> Option<String> {
    let sound = sound.trim();
    if sound.is_empty() {
        return None;
    }
    Some(if sound.contains(':') {
        sound.to_owned()
    } else {
        format!("minecraft:{sound}")
    })
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
            let sender = SESSIONS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
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
    if let Err(error) = dimension::save_playerdata_preserving_dimension(session, playerdata_dir()) {
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

/// Vanilla's translatable kick reasons. Clients render these in their own
/// language, which is what a real server sends.
const DISCONNECT_UNVERIFIED_USERNAME: &str =
    r#"{"translate":"multiplayer.disconnect.unverified_username"}"#;
const DISCONNECT_AUTH_SERVERS_DOWN: &str =
    r#"{"translate":"multiplayer.disconnect.authservers_down"}"#;

static ONLINE_MODE: OnceLock<bool> = OnceLock::new();

/// Set `server.properties`' `online-mode`. The server binary calls this at
/// startup; when it does not, [`online_mode`] reads `server.properties`
/// directly so the login path still knows which mode it is in.
///
/// Enabling online mode generates the process-wide RSA key pair eagerly, like
/// vanilla's startup path, so the first login does not pay for key generation.
pub fn configure_online_mode(enabled: bool) {
    let _ = ONLINE_MODE.set(enabled);
    if enabled && let Err(error) = server_keypair() {
        eprintln!("[join] online mode is enabled but no RSA key pair exists: {error}");
    }
}

/// Whether logins must present a valid Mojang session.
pub fn online_mode() -> bool {
    *ONLINE_MODE.get_or_init(crate::config::online_mode)
}

fn prevent_proxy_connections() -> bool {
    crate::config::prevent_proxy_connections()
}

/// Result of the encryption exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionOutcome {
    /// Mojang accepted the session and returned the authoritative profile.
    Authenticated(GameProfile),
    /// Mojang rejected the session (`hasJoinedServer` returned no profile).
    Unverified,
    /// The authentication servers could not be reached.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Unavailable(String),
    Rejected,
}

/// Pluggable Mojang session check.
///
/// The real implementation is [`MojangSessionAuthenticator`]; tests inject a
/// mock so no test touches the network.
pub trait SessionAuthenticator {
    fn has_joined_server(
        &self,
        username: &str,
        server_hash: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<Option<GameProfile>, AuthError>;
}

/// Mojang's `sessionserver.mojang.com/session/minecraft/hasJoined` endpoint.
///
/// The workspace has no TLS stack (and no HTTP client dependency), so the
/// request is executed with the system `curl` binary, which ships with Windows
/// 10+ and virtually every Unix. Arguments are passed directly to the process
/// (never through a shell), and user-controlled values are percent-encoded.
/// This is the only network call in the login path.
pub struct MojangSessionAuthenticator {
    endpoint: String,
}

impl Default for MojangSessionAuthenticator {
    fn default() -> Self {
        Self {
            endpoint: "https://sessionserver.mojang.com/session/minecraft/hasJoined".to_owned(),
        }
    }
}

impl MojangSessionAuthenticator {
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    /// Build the `hasJoinedServer` URL. Exposed for tests; production uses it
    /// through [`SessionAuthenticator`].
    pub fn url_for(&self, username: &str, server_hash: &str, client_ip: Option<IpAddr>) -> String {
        let mut url = format!(
            "{}?username={}&serverId={}",
            self.endpoint,
            percent_encode(username),
            percent_encode(server_hash)
        );
        if let Some(ip) = client_ip {
            url.push_str("&ip=");
            url.push_str(&percent_encode(&ip.to_string()));
        }
        url
    }
}

impl SessionAuthenticator for MojangSessionAuthenticator {
    fn has_joined_server(
        &self,
        username: &str,
        server_hash: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<Option<GameProfile>, AuthError> {
        let url = self.url_for(username, server_hash, client_ip);
        let output = std::process::Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--fail",
                "--connect-timeout",
                "5",
                "--max-time",
                "10",
                "--user-agent",
                "Folia-RS/0.1",
            ])
            .arg(&url)
            .output()
            .map_err(|error| AuthError::Unavailable(format!("could not run curl: {error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AuthError::Unavailable(format!(
                "session server request failed ({}): {}",
                output.status,
                stderr.trim()
            )));
        }
        if output.stdout.is_empty() {
            // 204 No Content: the session is not valid.
            return Ok(None);
        }
        parse_session_profile(&output.stdout)
            .map(Some)
            .map_err(|error| AuthError::Unavailable(format!("invalid session response: {error}")))
    }
}

/// Parse Mojang's `hasJoinedServer` JSON profile.
pub fn parse_session_profile(json: &[u8]) -> Result<GameProfile, String> {
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|error| format!("malformed JSON: {error}"))?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing id".to_owned())?;
    let uuid = parse_undashed_uuid(id).ok_or_else(|| "invalid profile id".to_owned())?;
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing name".to_owned())?;
    if name.is_empty() || name.len() > 16 {
        return Err("invalid profile name".to_owned());
    }
    let mut properties = Vec::new();
    if let Some(entries) = value
        .get("properties")
        .and_then(serde_json::Value::as_array)
    {
        for entry in entries {
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(value) = entry.get("value").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let signature = entry
                .get("signature")
                .and_then(serde_json::Value::as_str)
                .filter(|signature| !signature.is_empty())
                .map(str::to_owned);
            properties.push(ProfileProperty {
                name: name.to_owned(),
                value: value.to_owned(),
                signature,
            });
        }
    }
    Ok(GameProfile {
        uuid,
        name: name.to_owned(),
        properties,
    })
}

fn parse_undashed_uuid(value: &str) -> Option<[u8; 16]> {
    let cleaned: String = value
        .chars()
        .filter(|character| *character != '-')
        .collect();
    if cleaned.len() != 32 {
        return None;
    }
    let mut uuid = [0u8; 16];
    for (index, byte) in uuid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&cleaned[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(uuid)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Send the encryption request, verify and decrypt the response, enable
/// encryption, then ask the [`SessionAuthenticator`] about the session.
///
/// The order matches vanilla: the challenge is verified before the shared
/// secret, the cipher is installed before any further packet is written, and
/// the authenticator sees the server hash computed from the DER public key and
/// the decrypted secret.
pub fn complete_encryption<S: Read + Write>(
    connection: &mut Connection<S>,
    server_id: &str,
    keypair: &RsaKeyPair,
    challenge: &[u8],
    username: &str,
    authenticator: &dyn SessionAuthenticator,
    client_ip: Option<IpAddr>,
) -> io::Result<EncryptionOutcome> {
    if challenge.len() != 4 {
        return Err(protocol_error("verify token must be four bytes"));
    }
    let public_key = keypair.public_key_der();
    let request = EncryptionRequest {
        server_id: server_id.to_owned(),
        public_key: public_key.clone(),
        verify_token: challenge.to_vec(),
        should_authenticate: true,
    };
    connection.write_packet(&build_encryption_request(&request)?)?;

    let packet = connection.read_packet()?;
    let response = parse_encryption_response(&packet)?;
    let token = keypair
        .decrypt_pkcs1v15(&response.verify_token)
        .map_err(crypto_error)?;
    if !constant_time_eq(&token, challenge) {
        return Err(protocol_error("encryption verify token mismatch"));
    }
    let secret = keypair
        .decrypt_pkcs1v15(&response.shared_secret)
        .map_err(crypto_error)?;
    let secret: [u8; crypto::AES_KEY_SIZE] = secret
        .try_into()
        .map_err(|_| protocol_error("shared secret is not 128 bits"))?;
    connection.enable_encryption(&secret)?;

    let hash = server_hash(server_id, &public_key, &secret);
    match authenticator.has_joined_server(username, &hash, client_ip) {
        Ok(Some(profile)) => Ok(EncryptionOutcome::Authenticated(profile)),
        Ok(None) | Err(AuthError::Rejected) => Ok(EncryptionOutcome::Unverified),
        Err(AuthError::Unavailable(message)) => {
            eprintln!("[join] session server unavailable for {username}: {message}");
            Ok(EncryptionOutcome::Unavailable)
        }
    }
}

fn protocol_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn crypto_error(error: CryptoError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn server_keypair() -> io::Result<&'static RsaKeyPair> {
    static KEYPAIR: OnceLock<Result<RsaKeyPair, CryptoError>> = OnceLock::new();
    KEYPAIR
        .get_or_init(RsaKeyPair::generate)
        .as_ref()
        .map_err(|error| {
            io::Error::other(format!(
                "could not generate the server RSA key pair: {error}"
            ))
        })
}

fn authenticate_login(stream: &mut LoginStream, username: &str) -> io::Result<EncryptionOutcome> {
    let keypair = server_keypair()?;
    let mut challenge = [0u8; 4];
    crypto::random_bytes(&mut challenge)
        .map_err(|error| io::Error::other(format!("no secure randomness: {error}")))?;
    let client_ip = if prevent_proxy_connections() {
        stream.peer_addr().ok().map(|address| address.ip())
    } else {
        None
    };
    complete_encryption(
        stream,
        "",
        keypair,
        &challenge,
        username,
        &MojangSessionAuthenticator::default(),
        client_ip,
    )
}

fn send_login_disconnect(stream: &mut LoginStream, reason: &str) -> io::Result<()> {
    stream.write_packet(&build_login_disconnect(reason)?)
}

/// Accept a login and walk the client through configuration into the captured
/// flat world. The caller has already read and parsed the handshake.
pub fn handle_login(stream: TcpStream, handshake: &Handshake) -> io::Result<()> {
    let _session = SessionGuard::new();
    let mut stream = Connection::new(stream);
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let login_start = read_packet(&mut stream)?;
    let mut start = parse_login_start(&login_start)?;
    println!(
        "[join] login start name={} uuid={:?} protocol={}",
        start.name, start.uuid, handshake.protocol_version
    );

    let mut profile_properties: Vec<ProfileProperty> = Vec::new();
    if online_mode() {
        match authenticate_login(&mut stream, &start.name)? {
            EncryptionOutcome::Authenticated(profile) => {
                println!(
                    "[join] authenticated {} as {}",
                    start.name,
                    uuid_string(&profile.uuid)
                );
                start.name = profile.name;
                start.uuid = Some(profile.uuid);
                profile_properties = profile.properties;
            }
            EncryptionOutcome::Unverified => {
                send_login_disconnect(&mut stream, DISCONNECT_UNVERIFIED_USERNAME)?;
                println!("[join] {} has an invalid session", start.name);
                return Ok(());
            }
            EncryptionOutcome::Unavailable => {
                send_login_disconnect(&mut stream, DISCONNECT_AUTH_SERVERS_DOWN)?;
                println!(
                    "[join] could not verify {} because the authentication servers are down",
                    start.name
                );
                return Ok(());
            }
        }
    }

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
    if session.health <= 0.0 {
        println!(
            "[join] {} saved no health; restoring a valid join state",
            start.name
        );
        restore_join_vitals(&mut session);
    }
    let data_dir = playerdata_dir();
    let initial_dimension = dimension::playerdata_dimension(&data_dir, &uuid);
    let mut dimension_state = SessionDimensionState::new(initial_dimension);
    let mut current_storage = crate::world::shared_dimension(world.root.clone(), initial_dimension);
    unstick_player(
        &mut session,
        current_storage,
        world.trust_stored,
        initial_dimension,
        world.seed,
    );
    // The simulation starts from the accepted login position, not from the
    // playerdata position before settling.
    session.reset_physics();

    send_login_success(&mut stream, &uuid, &start.name, &profile_properties)?;

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
        &join_game_packet(initial_dimension, session.game_mode, radius, radius)?,
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
                | PLAY_SET_TIME
                | PLAY_SYSTEM_CHAT
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
    let player_entity = player_entity_id();
    register_player_entity(&session, player_entity);
    write_packet(&mut stream, &health_packet(&session))?;
    write_packet(
        &mut stream,
        &crate::entity_metadata::set_equipment_packet(player_entity, &player_equipment(&session)),
    )?;
    write_packet(
        &mut stream,
        &crate::entity_metadata::set_experience_packet(
            session.xp_progress,
            session.xp_level,
            session.xp_total,
        ),
    )?;
    // The position has to arrive before the chunk batch. The client stays on
    // the loading screen until it has both; the old order generated every
    // column first, so a 441-column join sat on the loading screen for the
    // whole batch. The first column streamed below is this position's column.
    //
    // Use a short write timeout from here on: `write_packet` retries the
    // timeout and polls the shutdown flag, so a client that stops reading can
    // never keep a session from saving during shutdown. The retry loop still
    // disconnects a client that accepts no bytes for 15 seconds.
    stream.set_write_timeout(Some(SESSION_TICK_INTERVAL))?;
    write_packet(&mut stream, &position_packet(&session, 1000))?;
    // Movement catch-up starts counting here: the initial chunk batch can run
    // for a long time on a slow build, and the first queued movement packet may
    // report a large but legitimate distance.
    let mut last_move_at = Instant::now();
    let mut drop_entities = DropEntityStore::new();
    let mut projectiles = crate::projectile::ProjectileStore::new();
    let mut mob_spawner =
        crate::spawn::NaturalSpawner::new(289, crate::spawn::SpawnSettings::default());
    let mut game_tick: u64 = 0;
    let mut bow_draw: Option<u64> = None;
    let mut pending_teleport: Option<(f64, f64, f64)> = None;
    let mut pending_summon: Option<PendingSummon> = None;
    let mut pending_kill = false;
    let mut hazards = PlayerHazards::default();
    let seed = crate::worldgen::generator::configured()
        .map(|g| g.seed())
        .unwrap_or(0)
        ^ i64::from_le_bytes(session.uuid[..8].try_into().expect("uuid half"));
    let mut drop_rng = LegacyRandomSource::new(seed);
    // Session-owned mobs are announced between the position and the chunk
    // batch. The client keeps them until their columns arrive, and the join
    // test's captured chunk loop ignores non-chunk packets.
    let mut tracked_entities = {
        let mut storage = crate::world::lock(current_storage);
        SessionEntityTracker::spawn_near_player(
            &mut storage,
            (session.x, session.y, session.z),
            i64::from_le_bytes(session.uuid[0..8].try_into().expect("eight uuid bytes")),
        )
    };
    for packet in tracked_entities.join_packets() {
        write_packet(&mut stream, &packet)?;
    }
    let updates = crate::block_update::register_session();
    let mut center_x = (session.x.floor() as i32).div_euclid(16);
    let mut center_z = (session.z.floor() as i32).div_euclid(16);
    let mut sent_chunks: HashSet<(i32, i32)> = HashSet::new();
    let streamed = stream_chunks_around(
        &mut stream,
        current_storage,
        world.trust_stored,
        initial_dimension,
        world.seed,
        radius,
        center_x,
        center_z,
        &mut sent_chunks,
        INITIAL_STREAM_BATCH,
    )?;
    let chunk_count = streamed.streamed;
    let counts = streamed.counts;
    retain_session_entities(
        &mut stream,
        &mut tracked_entities,
        &mut drop_entities,
        &sent_chunks,
    )?;

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

    stream.set_read_timeout(Some(SESSION_TICK_INTERVAL))?;
    stream.set_write_timeout(Some(SESSION_TICK_INTERVAL))?;
    let mut teleport_id = 1001i32;
    let mut last_keepalive = Instant::now();
    let mut last_save = Instant::now();
    let mut last_flush = Instant::now();
    let mut last_entity_tick = Instant::now();
    let mut keepalive_nonce = 1i64;
    let respawn_position = (spawn_x, spawn_y, spawn_z);
    let mut last_void_damage = Instant::now();
    let mut awaiting_respawn = false;
    let mut death_at: Option<Instant> = None;
    let play_started = Instant::now();
    let mut last_chunk_topup = Instant::now();
    let mut dig = DigState::default();
    let mut menus = inventory::SessionMenus::new();
    let mut remote_chat: Option<RemoteChatSession> = None;
    let mut last_seen = LastSeenValidator::new(LAST_SEEN_MESSAGES_WINDOW);
    let mut next_chat_index = 0i32;
    let mut attack_cooldown = AttackCooldown::new(4.0);
    let (_console_guard, console_commands) =
        SessionCommandGuard::register(&session.name, session.uuid);

    loop {
        if shutdown_requested() {
            tracked_entities.clear();
            drop_entities.clear();
            projectiles.clear();
            menus.on_disconnect();
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
            {
                eprintln!("[join] could not save playerdata: {error}");
            }
            if let Err(error) = crate::world::lock(current_storage).flush() {
                eprintln!("[join] could not flush terrain: {error}");
            }
            println!("[join] {} disconnected for shutdown", start.name);
            return Ok(());
        }
        if last_entity_tick.elapsed() >= SESSION_TICK_INTERVAL {
            attack_cooldown.tick();
            let (
                tick,
                drop_tick,
                projectile_tick,
                inventory_before,
                submerged,
                spawn_packets,
                in_lava,
                in_fire,
                on_cactus,
                hazard_death_rewards,
                hazard_reward_packets,
            ) = {
                let mut storage = crate::world::lock(current_storage);
                let mut collider = WorldCollider::new(&mut storage);
                let eye_pos = BlockPos::new(
                    session.x.floor() as i32,
                    (session.y + 1.62).floor() as i32,
                    session.z.floor() as i32,
                );
                let submerged = collider
                    .block_at(eye_pos.x(), eye_pos.y(), eye_pos.z())
                    .is_some_and(|state| {
                        matches!(
                            crate::block_behaviour::fluid(&state),
                            Some("minecraft:water" | "minecraft:flowing_water")
                        ) || state.name == "minecraft:bubble_column"
                    });
                let feet_pos = BlockPos::new(
                    session.x.floor() as i32,
                    (session.y + 0.1).floor() as i32,
                    session.z.floor() as i32,
                );
                let head_pos = eye_pos;
                let below_pos = BlockPos::new(
                    session.x.floor() as i32,
                    (session.y - 0.1).floor() as i32,
                    session.z.floor() as i32,
                );
                let feet_state = collider.block_at(feet_pos.x(), feet_pos.y(), feet_pos.z());
                let head_state = collider.block_at(head_pos.x(), head_pos.y(), head_pos.z());
                let below_state = collider.block_at(below_pos.x(), below_pos.y(), below_pos.z());
                let is_lava = |state: &Option<crate::chunk::BlockState>| {
                    state.as_ref().is_some_and(|s| {
                        matches!(s.name.as_str(), "minecraft:lava" | "minecraft:flowing_lava")
                    })
                };
                let is_fire = |state: &Option<crate::chunk::BlockState>| {
                    state.as_ref().is_some_and(|s| {
                        matches!(s.name.as_str(), "minecraft:fire" | "minecraft:soul_fire")
                    })
                };
                let in_lava = is_lava(&feet_state) || is_lava(&head_state);
                let in_fire = is_fire(&feet_state) || is_fire(&head_state);
                let on_cactus = below_state
                    .as_ref()
                    .is_some_and(|s| s.name == "minecraft:cactus")
                    || feet_state
                        .as_ref()
                        .is_some_and(|s| s.name == "minecraft:cactus");
                let tick = tracked_entities.tick((session.x, session.y, session.z), &mut collider);
                let targets = tracked_entities.collision_targets();
                let inventory_before = session.inventory.clone();
                let drop_tick = drop_entities.tick(&mut collider, &mut session);
                let projectile_tick = projectiles.tick(&mut collider, &targets);
                let hazard_death_rewards: Vec<(String, crate::math::BlockPos)> = tick
                    .hazard_deaths
                    .iter()
                    .filter_map(|id| {
                        tracked_entities.entity(*id).and_then(|tracked| {
                            let name =
                                crate::registry::entity_type_name(tracked.entity.entity_type)?
                                    .to_owned();
                            let pos = crate::math::BlockPos::new(
                                tracked.entity.x.floor() as i32,
                                tracked.entity.y.floor() as i32,
                                tracked.entity.z.floor() as i32,
                            );
                            Some((name, pos))
                        })
                    })
                    .collect();
                let mut hazard_reward_packets = Vec::new();
                for (entity_type, pos) in &hazard_death_rewards {
                    let (stacks, xp) = entity_death_rewards(&session, entity_type, &mut drop_rng);
                    hazard_reward_packets.extend(drop_entities.spawn_stacks(
                        *pos,
                        &stacks,
                        &mut drop_rng,
                    ));
                    hazard_reward_packets.extend(drop_entities.spawn_experience(
                        *pos,
                        xp,
                        &mut drop_rng,
                    ));
                }
                let spawn_packets = if !awaiting_respawn && game_tick % 100 == 99 {
                    let chunk = crate::spawn::ChunkPos::new(
                        (session.x.floor() as i32).div_euclid(16),
                        (session.z.floor() as i32).div_euclid(16),
                    );
                    run_session_spawn_cycle(
                        &mut mob_spawner,
                        &mut storage,
                        &mut tracked_entities,
                        chunk,
                        (session.x, session.y, session.z),
                        &mut drop_rng,
                        12,
                    )
                } else {
                    Vec::new()
                };
                (
                    tick,
                    drop_tick,
                    projectile_tick,
                    inventory_before,
                    submerged,
                    spawn_packets,
                    in_lava,
                    in_fire,
                    on_cactus,
                    hazard_death_rewards,
                    hazard_reward_packets,
                )
            };
            for packet in &tick.packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        drop_entities.clear();
                        projectiles.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            for packet in &hazard_reward_packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        drop_entities.clear();
                        projectiles.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            for (entity_type, _) in &hazard_death_rewards {
                println!(
                    "[join] session mob {} died to environmental hazard",
                    entity_type
                );
            }
            for packet in &drop_tick.packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        drop_entities.clear();
                        projectiles.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            for packet in &projectile_tick.packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        drop_entities.clear();
                        projectiles.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            for packet in &spawn_packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        drop_entities.clear();
                        projectiles.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
                }
            }
            if drop_tick.picked_items {
                for slot in 0..session.inventory.len() {
                    if session.inventory[slot] != inventory_before[slot] {
                        write_packet(
                            &mut stream,
                            &inventory::inventory_packet(slot as i32, &session.inventory[slot]),
                        )?;
                    }
                }
            }
            if drop_tick.picked_experience {
                write_packet(
                    &mut stream,
                    &crate::entity_metadata::set_experience_packet(
                        session.xp_progress,
                        session.xp_level,
                        session.xp_total,
                    ),
                )?;
            }
            game_tick = game_tick.wrapping_add(1);
            let (food, air) = tick_player_vitals(&mut session, game_tick, submerged);
            let air_changed = matches!(
                air,
                crate::player::AirTick::Consumed
                    | crate::player::AirTick::Refilled
                    | crate::player::AirTick::Drowning
            );
            let health_changed = matches!(
                food,
                crate::player::FoodTick::Heal | crate::player::FoodTick::Starve
            ) || air == crate::player::AirTick::Drowning;
            if air_changed || health_changed {
                let mut store = crate::entity_metadata::living_entities();
                let player = store.entry_player(player_entity);
                if air_changed {
                    let _ = player.metadata.set(
                        crate::entity_metadata::DATA_AIR_SUPPLY,
                        crate::entity_metadata::MetadataValue::Int(i32::from(session.air)),
                    );
                }
                if health_changed {
                    player.set_health(session.health);
                }
                if let Some(packet) = player.metadata.dirty_packet() {
                    write_packet(&mut stream, &packet)?;
                }
            }
            if health_changed {
                write_packet(&mut stream, &health_packet(&session))?;
            }
            if session.health <= 0.0 && !awaiting_respawn {
                drop_player_death_items(
                    &mut stream,
                    &mut drop_entities,
                    &mut drop_rng,
                    &mut session,
                )?;
                if let Some(message) = vitals_death_message(food, air) {
                    write_packet(&mut stream, &combat_kill_packet(message))?;
                }
                awaiting_respawn = true;
                death_at = Some(Instant::now());
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                {
                    eprintln!("[join] could not save vitals death: {error}");
                }
            }
            if !awaiting_respawn
                && let Some((damage, kind, message)) = hazards.tick(in_lava, in_fire, on_cactus)
            {
                let result = damage_player_with_rng(
                    &mut session,
                    player_entity,
                    damage,
                    &mut drop_rng,
                    kind,
                );
                for packet in &result.packets {
                    write_packet(&mut stream, packet)?;
                }
                if result.died && !awaiting_respawn {
                    drop_player_death_items(
                        &mut stream,
                        &mut drop_entities,
                        &mut drop_rng,
                        &mut session,
                    )?;
                    write_packet(&mut stream, &combat_kill_packet(message))?;
                    awaiting_respawn = true;
                    death_at = Some(Instant::now());
                    if let Err(error) =
                        dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                    {
                        eprintln!("[join] could not save hazard death: {error}");
                    }
                } else if result.damaged {
                    println!(
                        "[join] {} took {:.1} hazard damage; health {:.0}",
                        session.name, damage, session.health
                    );
                }
            }
            for hit in &projectile_tick.hits {
                if !tracked_entities.tracks(hit.target_id) {
                    continue;
                }
                if let Some(result) = tracked_entities.damage(hit.target_id, hit.damage) {
                    for packet in &result.packets {
                        write_packet(&mut stream, packet)?;
                    }
                    if result.died {
                        let target_info = tracked_entities.entity(hit.target_id).map(|tracked| {
                            (
                                crate::registry::entity_type_name(tracked.entity.entity_type)
                                    .map(str::to_owned),
                                BlockPos::new(
                                    tracked.entity.x.floor() as i32,
                                    tracked.entity.y.floor() as i32,
                                    tracked.entity.z.floor() as i32,
                                ),
                            )
                        });
                        if let Some((Some(entity_type), pos)) = target_info {
                            spawn_mob_death_rewards(
                                &mut stream,
                                &mut drop_entities,
                                &mut drop_rng,
                                &session,
                                pos,
                                &entity_type,
                            )?;
                        }
                    }
                }
            }
            if tick.player_damage > 0.0 && !awaiting_respawn {
                let result = damage_player_with_rng(
                    &mut session,
                    player_entity,
                    tick.player_damage,
                    &mut drop_rng,
                    crate::effects::DamageKind::Physical,
                );
                for packet in &result.packets {
                    write_packet(&mut stream, packet)?;
                }
                if !result.died
                    && let Some(knockback) = tick.player_knockback
                {
                    write_packet(
                        &mut stream,
                        &crate::protocol::set_entity_motion_packet(player_entity, knockback),
                    )?;
                }
                if result.died {
                    drop_player_death_items(
                        &mut stream,
                        &mut drop_entities,
                        &mut drop_rng,
                        &mut session,
                    )?;
                    write_packet(
                        &mut stream,
                        &combat_kill_packet("Player was slain by a zombie"),
                    )?;
                    awaiting_respawn = true;
                    death_at = Some(Instant::now());
                    if let Err(error) =
                        dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                    {
                        eprintln!("[join] could not save melee death: {error}");
                    }
                    println!("[join] {} was slain by a zombie", start.name);
                } else if result.damaged {
                    println!(
                        "[join] {} took {:.1} melee damage; health {:.0}",
                        start.name, tick.player_damage, session.health
                    );
                }
            }
            last_entity_tick = Instant::now();
        }
        while let Ok(command) = console_commands.try_recv() {
            match command {
                SessionCommand::SetGameMode { mode, reply } => {
                    let result = apply_game_mode(&mut stream, &mut session, mode)
                        .map_err(|error| error.to_string());
                    let _ = reply.send(result);
                }
                SessionCommand::Chat { message } => {
                    write_packet(&mut stream, &system_chat_packet(&message)?)?;
                }
                SessionCommand::PlayerChat {
                    sender,
                    sender_name,
                    message,
                } => {
                    let name_component = component_nbt(&sender_name)?;
                    let packet = if let Some(signature) = &message.signature {
                        last_seen.add_pending(signature.clone());
                        let packet = build_player_chat_packet(
                            next_chat_index,
                            sender,
                            message.sequence,
                            Some(signature),
                            &message.body,
                            None,
                            &name_component,
                            None,
                        )?;
                        next_chat_index = next_chat_index.wrapping_add(1);
                        packet
                    } else {
                        let content = component_nbt(&message.body.content)?;
                        build_disguised_chat_packet(&content, &name_component, None)?
                    };
                    write_packet(&mut stream, &packet)?;
                }
                SessionCommand::Disconnect { reason } => {
                    write_packet(&mut stream, &disconnect_packet(&reason)?)?;
                    tracked_entities.clear();
                    drop_entities.clear();
                    projectiles.clear();
                    menus.on_disconnect();
                    if let Err(error) =
                        dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                    {
                        eprintln!("[join] could not save playerdata before kick: {error}");
                    }
                    if let Err(error) = crate::world::lock(current_storage).flush() {
                        eprintln!("[join] could not flush terrain before kick: {error}");
                    }
                    println!("[join] {} kicked: {reason}", start.name);
                    return Ok(());
                }
                SessionCommand::Teleport {
                    x,
                    y,
                    z,
                    yaw,
                    pitch,
                } => {
                    session.teleport(x, y, z, true);
                    session.yaw = yaw;
                    session.pitch = pitch;
                    session.physics.yaw = yaw;
                    session.physics.pitch = pitch;
                    teleport_id += 1;
                    write_packet(&mut stream, &position_packet(&session, teleport_id))?;
                    let new_x = (session.x.floor() as i32).div_euclid(16);
                    let new_z = (session.z.floor() as i32).div_euclid(16);
                    if (new_x, new_z) != (center_x, center_z) {
                        center_x = new_x;
                        center_z = new_z;
                        stream_chunks_around(
                            &mut stream,
                            current_storage,
                            world.trust_stored,
                            dimension_state.current(),
                            world.seed,
                            radius,
                            center_x,
                            center_z,
                            &mut sent_chunks,
                            STREAM_TOPUP_BATCH,
                        )?;
                        retain_session_entities(
                            &mut stream,
                            &mut tracked_entities,
                            &mut drop_entities,
                            &sent_chunks,
                        )?;
                    }
                    if let Err(error) =
                        dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                    {
                        eprintln!("[join] could not save teleport: {error}");
                    }
                    println!(
                        "[join] {} teleported to ({:.1}, {:.1}, {:.1})",
                        start.name, session.x, session.y, session.z
                    );
                }
                SessionCommand::SetTime {
                    game_time,
                    day_time,
                    tick_day_time,
                } => {
                    write_packet(
                        &mut stream,
                        &set_time_packet(game_time, day_time, tick_day_time),
                    )?;
                }
                SessionCommand::GameEvent { event, param } => {
                    write_packet(&mut stream, &game_event_packet(event, param))?;
                }
                SessionCommand::Sound {
                    sound,
                    category,
                    x,
                    y,
                    z,
                    volume,
                    pitch,
                    seed,
                } => {
                    write_packet(
                        &mut stream,
                        &sound_packet(&sound, category, x, y, z, volume, pitch, seed)?,
                    )?;
                }
                SessionCommand::Particle {
                    particle,
                    x,
                    y,
                    z,
                    offset_x,
                    offset_y,
                    offset_z,
                    speed,
                    count,
                    force,
                } => {
                    match particle_packet(
                        &particle, x, y, z, offset_x, offset_y, offset_z, speed, count, force,
                    ) {
                        Some(packet) => write_packet(&mut stream, &packet)?,
                        None => eprintln!(
                            "[join] refusing unsupported particle '{particle}' for {}: \
                             no simple 1.21.11 particle registry id",
                            start.name
                        ),
                    }
                }
                SessionCommand::OpenMenu { position } => {
                    if let Err(error) = open_menu_session(
                        &mut stream,
                        &mut menus,
                        &session,
                        current_storage,
                        position,
                    ) {
                        eprintln!(
                            "[join] {}: could not open menu at ({}, {}, {}): {error}",
                            start.name, position.x, position.y, position.z
                        );
                    }
                }
            }
        }
        if !awaiting_respawn && let Some(trigger) = dimension_state.ready(Instant::now()) {
            let now = Instant::now();
            let trigger = dimension_state.complete(now).unwrap_or(trigger);
            let new_storage = crate::world::shared_dimension(world.root.clone(), trigger.to);
            switch_dimension(
                &mut stream,
                &mut session,
                &mut menus,
                &mut dig,
                &mut tracked_entities,
                &mut drop_entities,
                &trigger,
                current_storage,
                new_storage,
                world.trust_stored,
                world.seed,
                radius,
                &mut center_x,
                &mut center_z,
                &mut sent_chunks,
                &mut teleport_id,
            )?;
            current_storage = new_storage;
            continue;
        }
        if awaiting_respawn {
            if death_at.is_some_and(|time| time.elapsed() >= Duration::from_secs(2)) {
                if dimension_state.current() != crate::world::Dimension::Overworld {
                    dimension_state.relocate(crate::world::Dimension::Overworld, Instant::now());
                    current_storage = crate::world::shared_overworld(world.root.clone());
                }
                respawn_player(
                    &mut stream,
                    &mut session,
                    current_storage,
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
                retain_session_entities(
                    &mut stream,
                    &mut tracked_entities,
                    &mut drop_entities,
                    &sent_chunks,
                )?;
            }
        } else if apply_void_damage(
            &mut stream,
            &mut session,
            &mut drop_entities,
            &mut drop_rng,
            &mut last_void_damage,
        )? {
            awaiting_respawn = true;
            death_at = Some(Instant::now());
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
            {
                eprintln!("[join] could not save void death: {error}");
            }
            println!("[join] {} died in the void", start.name);
        }
        if last_chunk_topup.elapsed() >= CHUNK_TOPUP_INTERVAL
            && has_unsent_columns(&sent_chunks, center_x, center_z, radius)
        {
            let batch = stream_chunks_around(
                &mut stream,
                current_storage,
                world.trust_stored,
                dimension_state.current(),
                world.seed,
                radius,
                center_x,
                center_z,
                &mut sent_chunks,
                STREAM_TOPUP_BATCH,
            )?;
            last_chunk_topup = Instant::now();
            if batch.streamed > 0 && !has_unsent_columns(&sent_chunks, center_x, center_z, radius) {
                println!(
                    "[join] view distance filled: {} chunks around ({center_x}, {center_z})",
                    sent_chunks.len()
                );
            }
        }
        while let Ok(update) = updates.try_recv() {
            if !entity_packet_visible(&update, &tracked_entities) {
                continue;
            }
            if let Err(error) = write_packet(&mut stream, &update) {
                if is_disconnect(&error) {
                    tracked_entities.clear();
                    finish_disconnected(&session, current_storage, &start.name, &error);
                    return Ok(());
                }
                return Err(error);
            }
        }
        // The world tick may have advanced an open furnace/brewing stand, or
        // another session may have changed the chest. Push item/data changes
        // before handling the next packet.
        if let Err(error) = sync_open_menu(&mut stream, &mut menus, &session, current_storage) {
            eprintln!("[join] could not sync open menu: {error}");
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
                            if dimension_state.current() != crate::world::Dimension::Overworld {
                                dimension_state
                                    .relocate(crate::world::Dimension::Overworld, Instant::now());
                                current_storage =
                                    crate::world::shared_overworld(world.root.clone());
                            }
                            respawn_player(
                                &mut stream,
                                &mut session,
                                current_storage,
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
                            retain_session_entities(
                                &mut stream,
                                &mut tracked_entities,
                                &mut drop_entities,
                                &sent_chunks,
                            )?;
                        }
                    }
                    SERVERBOUND_CHAT => {
                        if let Err(error) =
                            handle_plain_chat(&session, &mut remote_chat, &mut last_seen, &packet)
                        {
                            eprintln!("[join] {} chat rejected: {error}", start.name);
                        }
                    }
                    SERVERBOUND_CHAT_SESSION_UPDATE => {
                        match accept_chat_session_update(&session, &packet) {
                            Ok(remote) => {
                                println!("[join] {} registered a chat session key", start.name);
                                remote_chat = Some(remote);
                            }
                            Err(error) => {
                                eprintln!("[join] {} chat session rejected: {error}", start.name)
                            }
                        }
                    }
                    SERVERBOUND_CHAT_COMMAND | SERVERBOUND_CHAT_COMMAND_SIGNED => {
                        if let Err(error) = handle_command(
                            &mut stream,
                            &mut session,
                            &mut last_seen,
                            &mut pending_teleport,
                            &mut pending_summon,
                            &mut pending_kill,
                            player_entity,
                            game_tick,
                            &packet,
                        ) {
                            eprintln!("[join] command failed: {error}");
                        }
                        if let Some((x, y, z)) = pending_teleport.take() {
                            session.teleport(x, y, z, false);
                            session.reset_physics();
                            teleport_id += 1;
                            write_packet(&mut stream, &position_packet(&session, teleport_id))?;
                            center_x = (session.x.floor() as i32).div_euclid(16);
                            center_z = (session.z.floor() as i32).div_euclid(16);
                            if let Err(error) = dimension::save_playerdata_preserving_dimension(
                                &session,
                                playerdata_dir(),
                            ) {
                                eprintln!("[join] could not save teleport: {error}");
                            }
                        }
                        if let Some((name, pos)) = pending_summon.take() {
                            let summon_seed = game_tick as i64 ^ 0x5EED;
                            if let Some(id) = tracked_entities.spawn_mob(&name, pos, summon_seed) {
                                for packet in tracked_entities.enter_packets_for(id) {
                                    write_packet(&mut stream, &packet)?;
                                }
                                println!(
                                    "[join] {} summoned {name} at ({:.1}, {:.1}, {:.1})",
                                    session.name, pos.0, pos.1, pos.2
                                );
                            } else {
                                println!("[join] {} failed to summon {name}", session.name);
                            }
                        }
                        if pending_kill {
                            pending_kill = false;
                            if !awaiting_respawn && session.health > 0.0 {
                                let result = damage_player_with_rng(
                                    &mut session,
                                    player_entity,
                                    f32::MAX,
                                    &mut drop_rng,
                                    crate::effects::DamageKind::Void,
                                );
                                for packet in &result.packets {
                                    write_packet(&mut stream, packet)?;
                                }
                                if result.died {
                                    drop_player_death_items(
                                        &mut stream,
                                        &mut drop_entities,
                                        &mut drop_rng,
                                        &mut session,
                                    )?;
                                    write_packet(
                                        &mut stream,
                                        &combat_kill_packet("Player was killed"),
                                    )?;
                                    awaiting_respawn = true;
                                    death_at = Some(Instant::now());
                                    if let Err(error) =
                                        dimension::save_playerdata_preserving_dimension(
                                            &session,
                                            playerdata_dir(),
                                        )
                                    {
                                        eprintln!("[join] could not save /kill death: {error}");
                                    }
                                }
                            }
                        }
                    }
                    SERVERBOUND_INTERACT => {
                        if let Some(target) = parse_attack(&packet)
                            && let Err(error) = handle_entity_attack(
                                &mut stream,
                                &mut session,
                                &mut tracked_entities,
                                &mut attack_cooldown,
                                &mut drop_entities,
                                &mut drop_rng,
                                target,
                            )
                        {
                            eprintln!("[join] entity attack failed: {error}");
                        }
                    }
                    crate::protocol::SERVERBOUND_USE_ITEM => {
                        if let Err(error) = handle_use_item(
                            &mut stream,
                            &mut session,
                            &mut projectiles,
                            player_entity,
                            game_tick,
                            &mut bow_draw,
                            &packet,
                        ) {
                            eprintln!("[join] use item failed: {error}");
                        }
                    }
                    SERVERBOUND_PLAYER_ACTION | SERVERBOUND_USE_ITEM_ON => {
                        if id == SERVERBOUND_USE_ITEM_ON {
                            if let Some(entity_name) = session
                                .selected_item()
                                .and_then(|held| spawn_egg_entity(&held.item))
                            {
                                let use_item = match crate::block_update::parse_use_item_on(&packet)
                                {
                                    Ok(use_item) => use_item,
                                    Err(error) => {
                                        eprintln!("[join] could not parse spawn-egg use: {error}");
                                        continue;
                                    }
                                };
                                let (dx, dy, dz) =
                                    crate::block_update::direction_offset(use_item.direction);
                                let (dx, dy, dz) = if (dx, dy, dz) == (0, 0, 0) {
                                    (0, 1, 0)
                                } else {
                                    (dx, dy, dz)
                                };
                                let clicked = BlockPos::new(
                                    use_item.x + dx,
                                    use_item.y + dy,
                                    use_item.z + dz,
                                );
                                let centre = (
                                    f64::from(clicked.x()) + 0.5,
                                    f64::from(clicked.y()),
                                    f64::from(clicked.z()) + 0.5,
                                );
                                let seed = game_tick as i64 ^ 0x5A17;
                                if let Some(egg_id) =
                                    tracked_entities.spawn_mob(entity_name, centre, seed)
                                {
                                    for packet in tracked_entities.enter_packets_for(egg_id) {
                                        write_packet(&mut stream, &packet)?;
                                    }
                                    if session.game_mode != 1 {
                                        session.consume_selected();
                                    }
                                    let slot = session.selected_slot.max(0) as usize;
                                    write_packet(
                                        &mut stream,
                                        &inventory::inventory_packet(
                                            session.selected_slot,
                                            &session.inventory[slot],
                                        ),
                                    )?;
                                    if let Err(error) =
                                        dimension::save_playerdata_preserving_dimension(
                                            &session,
                                            playerdata_dir(),
                                        )
                                    {
                                        eprintln!(
                                            "[join] could not save spawn-egg inventory: {error}"
                                        );
                                    }
                                }
                                continue;
                            }
                            let portal = {
                                let mut storage = crate::world::lock(current_storage);
                                portal_click(&mut storage, &packet)?
                            };
                            if let Some(kind) = portal {
                                let now = Instant::now();
                                match dimension_state.request(
                                    kind,
                                    [session.x, session.y, session.z],
                                    now,
                                ) {
                                    PortalRequest::Started => println!(
                                        "[join] {} used a {} portal; transfer to {:?} pending",
                                        start.name,
                                        kind.block_name(),
                                        kind.destination(dimension_state.current())
                                    ),
                                    PortalRequest::AlreadyPending => {}
                                    PortalRequest::CoolingDown => println!(
                                        "[join] {} ignored a {} portal re-trigger during cooldown",
                                        start.name,
                                        kind.block_name()
                                    ),
                                }
                                continue;
                            }
                        }
                        if let Err(error) = handle_block_action(
                            &mut stream,
                            current_storage,
                            &mut session,
                            &mut menus,
                            &mut dig,
                            play_started,
                            &mut projectiles,
                            &mut drop_entities,
                            &mut drop_rng,
                            game_tick,
                            &mut bow_draw,
                            player_entity,
                            &tracked_entities,
                            &packet,
                            id,
                        ) {
                            eprintln!("[join] block action failed: {error}");
                        }
                    }
                    inventory::SERVERBOUND_SET_CARRIED_ITEM => {
                        if let Err(error) =
                            handle_set_carried(&mut stream, &mut session, &mut bow_draw, &packet)
                        {
                            eprintln!("[join] carried slot update failed: {error}");
                        }
                    }
                    inventory::SERVERBOUND_SET_CREATIVE_MODE_SLOT => {
                        if let Some(creative) = inventory::parse_creative_slot(&packet)
                            && let Some(index) = inventory::inventory_index(creative.slot)
                        {
                            session.inventory[index] = creative.stack;
                            send_container_content(&mut stream, &session)?;
                            if let Err(error) = dimension::save_playerdata_preserving_dimension(
                                &session,
                                playerdata_dir(),
                            ) {
                                eprintln!("[join] could not save inventory: {error}");
                            }
                        }
                    }
                    inventory::SERVERBOUND_CONTAINER_CLICK => {
                        if let Some(click) = inventory::parse_container_click(&packet) {
                            let open_container_id = menus.open().map(|menu| menu.container_id);
                            if let Some(open_container_id) = open_container_id {
                                // While a container is open, clicks for any
                                // other container id (including the player's
                                // inventory menu 0) are ignored, as vanilla's
                                // `containerMenu.containerId == packet.containerId`
                                // guard does.
                                if open_container_id == click.container_id
                                    && let Err(error) = handle_menu_click(
                                        &mut stream,
                                        &mut menus,
                                        &mut session,
                                        current_storage,
                                        &click,
                                    )
                                {
                                    eprintln!("[join] container click failed: {error}");
                                }
                            } else if click.container_id == 0 {
                                // Player inventory menu (container id 0).
                                let armor_before = session.armor.clone();
                                if inventory::apply_player_click(
                                    &mut session.inventory,
                                    &mut session.armor,
                                    &mut session.cursor,
                                    click,
                                ) {
                                    send_container_content(&mut stream, &session)?;
                                    if session.armor != armor_before {
                                        write_packet(
                                            &mut stream,
                                            &inventory::container_set_slot_packet(
                                                5,
                                                &session.armor[3],
                                            ),
                                        )?;
                                        write_packet(
                                            &mut stream,
                                            &inventory::container_set_slot_packet(
                                                6,
                                                &session.armor[2],
                                            ),
                                        )?;
                                        write_packet(
                                            &mut stream,
                                            &inventory::container_set_slot_packet(
                                                7,
                                                &session.armor[1],
                                            ),
                                        )?;
                                        write_packet(
                                            &mut stream,
                                            &inventory::container_set_slot_packet(
                                                8,
                                                &session.armor[0],
                                            ),
                                        )?;
                                        write_packet(
                                            &mut stream,
                                            &crate::entity_metadata::set_equipment_packet(
                                                player_entity,
                                                &player_equipment(&session),
                                            ),
                                        )?;
                                    }
                                    if let Err(error) =
                                        dimension::save_playerdata_preserving_dimension(
                                            &session,
                                            playerdata_dir(),
                                        )
                                    {
                                        eprintln!("[join] could not save inventory: {error}");
                                    }
                                }
                            }
                        }
                    }
                    inventory::SERVERBOUND_CONTAINER_CLOSE => {
                        if let Ok(container_id) = parse_container_close(&packet)
                            && menus
                                .open()
                                .is_some_and(|menu| menu.container_id == container_id)
                        {
                            menus.close();
                        }
                    }
                    crate::protocol::SERVERBOUND_PLAYER_INPUT => {
                        match crate::protocol::parse_serverbound_player_input(&packet) {
                            Ok(input) => apply_player_input(&mut session, input.flags),
                            Err(error) => eprintln!("[join] player input failed: {error}"),
                        }
                    }
                    _ => {
                        let elapsed = last_move_at
                            .elapsed()
                            .as_secs_f64()
                            .clamp(0.05, MAX_CATCH_UP_SECONDS);
                        let allowance = (10.0 + elapsed * CATCH_UP_BLOCKS_PER_SECOND).powi(2);
                        let outcome = {
                            let storage = &mut *crate::world::lock(current_storage);
                            let mut collider = StoragePhysicsWorld::new(storage);
                            session.apply_movement_with_allowance(
                                &packet,
                                &mut collider,
                                allowance,
                            )?
                        };
                        if !outcome.was_movement {
                            continue;
                        }
                        last_move_at = Instant::now();
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
                            let died = apply_fall_damage(
                                &mut stream,
                                &mut session,
                                outcome.fall_damage,
                                &mut drop_rng,
                            )?;
                            if died {
                                awaiting_respawn = true;
                                death_at = Some(Instant::now());
                                if let Err(error) = dimension::save_playerdata_preserving_dimension(
                                    &session,
                                    playerdata_dir(),
                                ) {
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
                                current_storage,
                                world.trust_stored,
                                dimension_state.current(),
                                world.seed,
                                radius,
                                center_x,
                                center_z,
                                &mut sent_chunks,
                                STREAM_TOPUP_BATCH,
                            ) {
                                Ok(batch) => batch,
                                // Clients close the connection mid-stream all the
                                // time; that ends the session, it is not a
                                // server failure.
                                Err(error) if is_disconnect(&error) => {
                                    if let Err(save_error) =
                                        dimension::save_playerdata_preserving_dimension(
                                            &session,
                                            playerdata_dir(),
                                        )
                                    {
                                        eprintln!("[join] could not save playerdata: {save_error}");
                                    }
                                    if let Err(flush_error) =
                                        crate::world::lock(current_storage).flush()
                                    {
                                        eprintln!("[join] could not flush terrain: {flush_error}");
                                    }
                                    tracked_entities.clear();
                                    drop_entities.clear();
                                    projectiles.clear();
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
                            retain_session_entities(
                                &mut stream,
                                &mut tracked_entities,
                                &mut drop_entities,
                                &sent_chunks,
                            )?;
                        }
                    }
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut =>
            {
                if let Err(tick_error) = tick_delayed_destroy(
                    current_storage,
                    &mut stream,
                    &mut session,
                    &mut menus,
                    &mut dig,
                    play_started,
                    &mut drop_entities,
                    &mut drop_rng,
                ) {
                    eprintln!("[join] delayed block break failed: {tick_error}");
                }
                autosave_playerdata(&session, &mut last_save);
                if last_flush.elapsed() >= TERRAIN_FLUSH_INTERVAL {
                    if let Err(flush_error) = crate::world::lock(current_storage).flush() {
                        eprintln!("[join] could not flush terrain: {flush_error}");
                    }
                    last_flush = Instant::now();
                }
                if last_keepalive.elapsed() >= Duration::from_secs(10) {
                    if let Err(error) =
                        write_packet(&mut stream, &keepalive_packet(keepalive_nonce))
                    {
                        if is_disconnect(&error) {
                            finish_disconnected(&session, current_storage, &start.name, &error);
                            return Ok(());
                        }
                        return Err(error);
                    }
                    keepalive_nonce += 1;
                    last_keepalive = Instant::now();
                }
            }
            Err(error) => {
                menus.on_disconnect();
                if let Err(save_error) =
                    dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
                {
                    eprintln!("[join] could not save playerdata: {save_error}");
                }
                if let Err(flush_error) = crate::world::lock(current_storage).flush() {
                    eprintln!("[join] could not flush terrain: {flush_error}");
                }
                println!("[join] {} disconnected: {error}", start.name);
                return Ok(());
            }
        }
    }
}

pub fn reject_login(mut stream: TcpStream, reason: &str) -> io::Result<()> {
    crate::protocol::write_packet(&mut stream, &build_login_disconnect(reason)?)
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

/// Rebuild the captured Join Game packet with this server's game mode, live
/// view distance, and the session's current dimension. For an overworld login
/// the dimension type/name/sea level are byte-identical to the captured
/// packet, so the captured-world join sequence is unchanged.
fn join_game_packet(
    dimension: crate::world::Dimension,
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
    let _dimension_type = reader.read_varint()?;
    writer.write_varint(dimension.type_id())?;
    let _dimension_name = reader.read_string()?;
    writer.write_string(dimension.resource_name())?;
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
    let _sea_level = reader.read_varint()?;
    writer.write_varint(dimension.sea_level())?;
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

/// 1.21.11 component serialization is a single network NBT tag (no name), where
/// a literal component may be either a string or `{ text: "..." }`.
fn component_nbt(message: &str) -> io::Result<Vec<u8>> {
    let tag = Tag::Compound(vec![("text".into(), Tag::String(message.to_owned()))]);
    let mut encoded = Vec::new();
    crate::nbt::write_any_tag(&mut encoded, &tag)?;
    Ok(encoded)
}

fn accept_chat_session_update(
    session: &PlayerSession,
    payload: &[u8],
) -> io::Result<RemoteChatSession> {
    let update = parse_serverbound_chat_session_update(payload)?;
    let key = ProfilePublicKey {
        expires_at_ms: update.expires_at_ms,
        key: ChatPublicKey::Rsa(update.public_key),
        key_signature: update.key_signature,
    };
    // The client's key is signed by Mojang's root; this server does not ship
    // the Mojang trust root yet, so only the expiry/algorithm checks run here.
    RemoteChatSession::new(
        session.uuid,
        update.session_id,
        key,
        epoch_millis(),
        crate::config::enforces_secure_chat(),
        None,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}

/// Validate and broadcast one `ServerboundChat` message. The last-seen window
/// is advanced before signature verification, matching
/// `ServerGamePacketListenerImpl.handleChat`.
fn handle_plain_chat(
    session: &PlayerSession,
    remote_chat: &mut Option<RemoteChatSession>,
    last_seen: &mut LastSeenValidator,
    payload: &[u8],
) -> io::Result<()> {
    let chat = parse_serverbound_chat(payload)?;
    if chat.message.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty chat message",
        ));
    }
    let now_ms = epoch_millis();
    let seen = last_seen.apply_update(chat.last_seen)?;
    let body = SignedMessageBody::new(chat.message, chat.timestamp_ms, chat.salt, seen)?;
    let message = match chat.signature {
        Some(signature) => {
            let Some(remote) = remote_chat.as_mut() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "signed chat before a chat session update",
                ));
            };
            let mut message = SignedMessage::new(
                remote.next_sequence(),
                session.uuid,
                *remote.session_id(),
                body,
            );
            message.signature = Some(signature);
            remote
                .verify(&message, now_ms)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            message
        }
        None => {
            if crate::config::enforces_secure_chat() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsigned chat rejected while enforces-secure-chat is on",
                ));
            }
            SignedMessage::unsigned(0, session.uuid, [0u8; 16], body)
        }
    };
    broadcast_player_chat(session.uuid, &session.name, &message);
    Ok(())
}

fn epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Build a bounded surface-band spawn snapshot for one chunk.
///
/// This is a deliberate simplification: only `[min_surface-16, max_surface+4]`
/// (clamped to world bounds) is copied, so deep-cave spawning below that band
/// is not modeled here. Sky light defaults to 15 (daylight approximation) and
/// is overwritten by stored sky light when available.
fn spawn_snapshot(
    storage: &mut crate::world::WorldStorage,
    chunk_pos: crate::spawn::ChunkPos,
) -> Option<crate::entity_ai::MemoryWorld> {
    let chunk = storage
        .load_chunk(chunk_pos.x, chunk_pos.z)
        .ok()
        .flatten()?;
    let mut min_surface = i32::MAX;
    let mut max_surface = i32::MIN;
    for lz in 0..16 {
        for lx in 0..16 {
            let surface = crate::chunk::surface_y(&chunk, lx, lz);
            min_surface = min_surface.min(surface);
            max_surface = max_surface.max(surface);
        }
    }
    if min_surface == i32::MAX {
        return None;
    }
    let world_min = crate::chunk_packet::OVERWORLD_MIN_Y;
    let world_max = world_min + crate::chunk_packet::OVERWORLD_HEIGHT as i32 - 1;
    let min_y = (min_surface - 16).max(world_min);
    let max_y = (max_surface + 4).min(world_max);
    let mut world = crate::entity_ai::MemoryWorld::new(min_y, max_y + 1);
    world.set_default_sky_light(15);
    let mut default_biome_set = false;
    for lz in 0..16usize {
        for lx in 0..16usize {
            let gx = chunk_pos.min_block_x() + lx as i32;
            let gz = chunk_pos.min_block_z() + lz as i32;
            let surface = crate::chunk::surface_y(&chunk, lx, lz);
            world.set_surface(gx, gz, surface);
            let biome_y = surface.clamp(world_min, world_max);
            let biome = chunk
                .biome(lx, biome_y, lz)
                .or_else(|| chunk.biome(lx, 64.clamp(world_min, world_max), lz))
                .cloned();
            if let Some(biome) = biome {
                if !default_biome_set {
                    world.set_default_biome(&biome);
                    default_biome_set = true;
                }
                world.set_biome(gx, surface, gz, &biome);
            }
            for y in min_y..=max_y {
                if let Some(state) = chunk.block_state(lx, y, lz) {
                    world.set_block(gx, y, gz, state.clone());
                }
                if let Some(sky) = storage.sky_light_at(gx, y, gz) {
                    world.set_sky_light(gx, y, gz, sky);
                }
            }
        }
    }
    Some(world)
}

fn run_session_spawn_cycle(
    spawner: &mut crate::spawn::NaturalSpawner,
    storage: &mut crate::world::WorldStorage,
    tracker: &mut crate::entity::SessionEntityTracker,
    chunk: crate::spawn::ChunkPos,
    player: (f64, f64, f64),
    rng: &mut dyn folia_random::RandomSource,
    max_mobs: usize,
) -> Vec<Vec<u8>> {
    if tracker.len() >= max_mobs {
        return Vec::new();
    }
    let Some(snapshot) = spawn_snapshot(storage, chunk) else {
        return Vec::new();
    };
    for category in crate::spawn::MobCategory::ALL {
        spawner
            .state
            .set_count(category, tracker.count_category(category));
    }
    let results = spawner.run_cycle(
        &snapshot,
        chunk,
        &[Vec3::new(player.0, player.1, player.2)],
        rng,
    );
    let mut packets = Vec::new();
    for result in results {
        for mob in result.spawned {
            if tracker.len() >= max_mobs {
                return packets;
            }
            let pos = (
                mob.pos.x() as f64 + 0.5,
                mob.pos.y() as f64,
                mob.pos.z() as f64 + 0.5,
            );
            if let Some(id) = tracker.spawn_mob(mob.entity_type.name, pos, rng.next_long()) {
                packets.extend(tracker.enter_packets_for(id));
            }
        }
    }
    packets
}

/// Simplified environmental hazard timer state. Fire immunity ticks, armor,
/// and enchantment details beyond `DamageKind::Fire` are not modeled here.
#[derive(Default)]
struct PlayerHazards {
    lava_hurt: u32,
    fire_hurt: u32,
    cactus_hurt: u32,
}

impl PlayerHazards {
    fn tick(
        &mut self,
        in_lava: bool,
        in_fire: bool,
        on_cactus: bool,
    ) -> Option<(f32, crate::effects::DamageKind, &'static str)> {
        let mut result = None;
        if in_lava {
            if self.lava_hurt == 0 {
                self.lava_hurt = 10;
                result = Some((
                    4.0,
                    crate::effects::DamageKind::Fire,
                    "Player tried to swim in lava",
                ));
            }
        } else if in_fire {
            if self.fire_hurt == 0 {
                self.fire_hurt = 20;
                result = Some((
                    1.0,
                    crate::effects::DamageKind::Fire,
                    "Player burned to death",
                ));
            }
        } else if on_cactus && self.cactus_hurt == 0 {
            self.cactus_hurt = 10;
            result = Some((
                1.0,
                crate::effects::DamageKind::Physical,
                "Player was pricked to death",
            ));
        }
        if self.lava_hurt > 0 {
            self.lava_hurt -= 1;
        }
        if self.fire_hurt > 0 {
            self.fire_hurt -= 1;
        }
        if self.cactus_hurt > 0 {
            self.cactus_hurt -= 1;
        }
        result
    }
}

fn bow_power_for_ticks(ticks: u64) -> f64 {
    let f = (ticks.min(20) as f64) / 20.0;
    ((f * f + f * 2.0) / 3.0).min(1.0)
}

fn tick_player_vitals(
    session: &mut PlayerSession,
    game_tick: u64,
    submerged: bool,
) -> (crate::player::FoodTick, crate::player::AirTick) {
    // Simplified: vanilla applies sprint exhaustion per movement distance/tick
    // conditions; we approximate with a flat 0.1 per live session tick while
    // sprinting and alive.
    if session.physics.sprinting && session.health > 0.0 {
        session.add_exhaustion(0.1);
    }
    let food = session.tick_food(game_tick);
    let air = session.tick_air(submerged);
    // Drowning/starvation can reduce health to 0 here; we do not trigger the
    // full death/respawn flow in this slice and leave it to existing void/death
    // paths to observe on later ticks/conditions.
    (food, air)
}

fn vitals_death_message(
    food: crate::player::FoodTick,
    air: crate::player::AirTick,
) -> Option<&'static str> {
    if air == crate::player::AirTick::Drowning {
        Some("Player drowned")
    } else if food == crate::player::FoodTick::Starve {
        Some("Player starved to death")
    } else {
        None
    }
}

fn apply_player_input(session: &mut PlayerSession, flags: u8) {
    session.physics.sprinting = flags & crate::protocol::PLAYER_INPUT_SPRINT != 0;
    session.physics.sneaking = flags & crate::protocol::PLAYER_INPUT_SNEAK != 0;
}

fn look_vector(session: &PlayerSession) -> [f64; 3] {
    let yaw = session.yaw.to_radians() as f64;
    let pitch = session.pitch.to_radians() as f64;
    // Minecraft yaw 0 looks +Z.
    [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

/// `ServerboundInteractPacket` with action `ATTACK` (`ActionType` 1).
fn use_item_projectile(
    session: &PlayerSession,
) -> Option<(crate::projectile::ProjectileKind, [f64; 3], bool)> {
    let item = session.selected_item()?;
    let dir = look_vector(session);
    match item.item.as_str() {
        "minecraft:bow" => Some((
            crate::projectile::ProjectileKind::Arrow,
            [dir[0] * 3.0, dir[1] * 3.0, dir[2] * 3.0],
            false,
        )),
        "minecraft:snowball" => Some((
            crate::projectile::ProjectileKind::Snowball,
            [dir[0] * 1.5, dir[1] * 1.5, dir[2] * 1.5],
            true,
        )),
        "minecraft:egg" => Some((
            crate::projectile::ProjectileKind::Egg,
            [dir[0] * 1.5, dir[1] * 1.5, dir[2] * 1.5],
            true,
        )),
        _ => None,
    }
}

fn has_arrow(session: &PlayerSession) -> bool {
    session.inventory.iter().any(|stack| {
        !stack.is_empty()
            && matches!(
                stack.item.as_str(),
                "minecraft:arrow" | "minecraft:tipped_arrow" | "minecraft:spectral_arrow"
            )
    })
}

fn take_arrow(session: &mut PlayerSession) -> Option<(usize, crate::inventory::ItemStack)> {
    for (slot, stack) in session.inventory.iter_mut().enumerate() {
        if stack.is_empty() {
            continue;
        }
        if matches!(
            stack.item.as_str(),
            "minecraft:arrow" | "minecraft:tipped_arrow" | "minecraft:spectral_arrow"
        ) {
            stack.shrink(1);
            return Some((slot, stack.clone()));
        }
    }
    None
}

/// Maps a spawn-egg item id to the entity id it spawns.
///
/// Accepts `minecraft:<name>_spawn_egg` and bare `<name>_spawn_egg`; the
/// derived id is returned only when `crate::spawn::entity_type` knows the mob.
/// Non-eggs and eggs for unsupported entity types return `None`.
fn spawn_egg_entity(item: &str) -> Option<&'static str> {
    let name = item.strip_prefix("minecraft:").unwrap_or(item);
    let mob = name.strip_suffix("_spawn_egg")?;
    if mob.is_empty() {
        return None;
    }
    Some(crate::spawn::entity_type(mob)?.name)
}

fn handle_use_item(
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    projectiles: &mut crate::projectile::ProjectileStore,
    player_entity: i32,
    game_tick: u64,
    bow_draw: &mut Option<u64>,
    packet: &[u8],
) -> io::Result<()> {
    let use_item = crate::protocol::parse_serverbound_use_item(packet)?;
    session.yaw = use_item.yaw;
    session.pitch = use_item.pitch;
    let Some((kind, velocity, consume_item)) = use_item_projectile(session) else {
        return Ok(());
    };
    if matches!(kind, crate::projectile::ProjectileKind::Arrow) {
        if session.game_mode != 1 {
            // Require ammo at draw start (Infinity still needs an arrow in
            // inventory). Do not consume yet; release handles consumption/spawn.
            if !has_arrow(session) {
                return Ok(());
            }
        }
        *bow_draw = Some(game_tick);
        return Ok(());
    } else if consume_item {
        let slot = session.selected_slot.max(0) as usize;
        if let Some(stack) = session.inventory.get_mut(slot) {
            stack.shrink(1);
            write_packet(stream, &inventory::inventory_packet(slot as i32, stack))?;
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
            {
                eprintln!("[join] could not save projectile use: {error}");
            }
        }
    }
    let packets = projectiles.spawn(
        kind,
        Some(player_entity),
        [session.x, session.y + 1.62, session.z],
        velocity,
    );
    for packet in packets {
        write_packet(stream, &packet)?;
    }
    Ok(())
}

fn parse_attack(payload: &[u8]) -> Option<i32> {
    let mut reader = PacketReader::new(payload);
    if reader.read_varint().ok()? != SERVERBOUND_INTERACT {
        return None;
    }
    let entity_id = reader.read_varint().ok()?;
    if reader.read_varint().ok()? != 1 {
        return None;
    }
    reader.read_bool().ok()?;
    Some(entity_id)
}

fn mob_motion_packet(tracked: &SessionEntityTracker, target: i32) -> Option<Vec<u8>> {
    let velocity = tracked.entity(target)?.entity.velocity;
    (velocity != [0.0; 3]).then(|| crate::protocol::set_entity_motion_packet(target, velocity))
}

/// Apply the held item's attack damage to one session-owned entity. The
/// vanilla attack-range check is approximated by the player's 3-block reach
/// plus the target width.
fn handle_entity_attack(
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    tracked: &mut SessionEntityTracker,
    cooldown: &mut AttackCooldown,
    drop_entities: &mut DropEntityStore,
    drop_rng: &mut LegacyRandomSource,
    target: i32,
) -> io::Result<()> {
    if session.health <= 0.0 {
        return Ok(());
    }
    let Some(entity) = tracked.entity(target) else {
        return Ok(());
    };
    let reach = 3.0 + entity.entity.width;
    let dx = entity.entity.x - session.x;
    let dy = entity.entity.y - session.y;
    let dz = entity.entity.z - session.z;
    if dx * dx + dy * dy + dz * dz > reach * reach {
        return Ok(());
    }
    let entity_type_name =
        crate::registry::entity_type_name(entity.entity.entity_type).map(str::to_owned);
    let entity_pos = BlockPos::new(
        entity.entity.x.floor() as i32,
        entity.entity.y.floor() as i32,
        entity.entity.z.floor() as i32,
    );
    let (damage, knockback_strength, critical, scale) = resolve_player_attack(session, cooldown);
    let held = session.selected_item().cloned();
    let attack_result = if knockback_strength > 0.0 {
        let (knockback_x, knockback_z) = attack_knockback_direction(session.yaw);
        tracked.damage_with_knockback(target, damage, knockback_strength, knockback_x, knockback_z)
    } else {
        tracked.damage(target, damage)
    };
    let Some(result) = attack_result else {
        return Ok(());
    };
    if !result.damaged {
        return Ok(());
    }
    for packet in &result.packets {
        write_packet(stream, packet)?;
    }
    // Simplification: sweeping applies flat secondary damage without
    // knockback/particles/sound and is evaluated before primary motion and
    // death rewards are handled below.
    if is_sweeping_attack(scale, critical, session.physics.sprinting, held.as_ref()) {
        for sweep_id in tracked.sweep_targets(target, 1.0) {
            if let Some(sweep) = tracked.damage(sweep_id, 1.0) {
                for packet in &sweep.packets {
                    write_packet(stream, packet)?;
                }
            }
        }
    }
    // Send motion after damage metadata/death packets so knockback updates the
    // client's velocity, but before the death log/reward side effects.
    if let Some(packet) = mob_motion_packet(tracked, target) {
        write_packet(stream, &packet)?;
    }
    if session.game_mode != 1 {
        let slot = session.selected_slot.max(0) as usize;
        if let Some(stack) = session.inventory.get_mut(slot) {
            let wear =
                crate::block_breaking::apply_item_durability_with_rng(stack, 1, false, drop_rng);
            if wear.changed() {
                write_packet(stream, &inventory::inventory_packet(slot as i32, stack))?;
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                {
                    eprintln!("[join] could not save weapon durability: {error}");
                }
                if wear.broke {
                    println!("[join] {} broke their held weapon", session.name);
                }
            }
        }
    }
    if result.died {
        if let Some(entity_type) = entity_type_name.as_deref() {
            spawn_mob_death_rewards(
                stream,
                drop_entities,
                drop_rng,
                session,
                entity_pos,
                entity_type,
            )?;
        }
        println!("[join] {} killed entity {target}", session.name);
    }
    Ok(())
}

fn entity_loot_roll(
    session: &PlayerSession,
    entity_type: &str,
    rng: &mut dyn folia_random::RandomSource,
) -> Option<crate::loot::LootRoll> {
    let looting_level = session
        .inventory
        .get(session.selected_slot.max(0) as usize)
        .map_or(0, |stack| stack.enchantment_level("minecraft:looting"));
    let mut context = crate::loot::LootContext::new()
        .with_killed_by_player()
        .with_looting_level(looting_level);
    if let Some(held) = session.selected_item() {
        context = context.with_tool(held);
    }
    context = context.with_entity_on_fire(false);
    crate::loot::LootCatalog::global().roll_entity(entity_type, rng, &context)
}

fn entity_experience_reward(entity_type: &str, rng: &mut dyn folia_random::RandomSource) -> i32 {
    match entity_type {
        "minecraft:zombie" => 5,
        "minecraft:cow" | "minecraft:pig" => 1 + rng.next_int_bound(3),
        _ => 0,
    }
}

fn entity_death_rewards(
    session: &PlayerSession,
    entity_type: &str,
    rng: &mut dyn folia_random::RandomSource,
) -> (Vec<crate::item_components::ItemStack>, i32) {
    let stacks = entity_loot_roll(session, entity_type, rng)
        .map(|roll| roll.stacks)
        .unwrap_or_default();
    let experience = entity_experience_reward(entity_type, rng);
    (stacks, experience)
}

fn spawn_mob_death_rewards(
    stream: &mut LoginStream,
    drops: &mut DropEntityStore,
    rng: &mut LegacyRandomSource,
    session: &PlayerSession,
    pos: BlockPos,
    entity_type: &str,
) -> io::Result<()> {
    let (stacks, xp) = entity_death_rewards(session, entity_type, rng);
    for packet in drops.spawn_stacks(pos, &stacks, rng) {
        write_packet(stream, &packet)?;
    }
    for packet in drops.spawn_experience(pos, xp, rng) {
        write_packet(stream, &packet)?;
    }
    Ok(())
}

fn resolve_player_attack(
    session: &PlayerSession,
    cooldown: &mut AttackCooldown,
) -> (f32, f64, bool, f32) {
    let speed = session
        .inventory
        .get(session.selected_slot.max(0) as usize)
        .and_then(|stack| crate::item_prototypes::prototype(&stack.item))
        .and_then(|prototype| prototype.tool.as_ref())
        .and_then(|tool| tool.attack_speed)
        .map(|modifier| f64::from(modifier) + 4.0)
        .unwrap_or(4.0);
    cooldown.set_attack_speed(speed);
    let scale = cooldown.strength_scale(0.5);
    let conditions = crate::combat::CriticalConditions {
        falling: session.physics.fall_distance > 0.0,
        on_ground: session.physics.on_ground,
        in_water: session.physics.swimming,
        on_climbable: false,
        blind: false,
        passenger: false,
        sprinting: session.physics.sprinting,
    };
    let critical = crate::combat::is_critical(scale, &conditions);
    let damage = crate::combat::attack_damage(held_attack_damage(session), scale, critical);
    let knockback_strength = if scale > 0.9 {
        attack_knockback_strength(session)
    } else {
        0.0
    };
    cooldown.reset();
    (damage, knockback_strength, critical, scale)
}

fn is_sweeping_attack(
    scale: f32,
    critical: bool,
    sprinting: bool,
    held: Option<&crate::inventory::ItemStack>,
) -> bool {
    scale > 0.9
        && !critical
        && !sprinting
        && held.is_some_and(|stack| stack.item.ends_with("_sword"))
}

fn held_attack_damage(session: &PlayerSession) -> f32 {
    session
        .inventory
        .get(session.selected_slot.max(0) as usize)
        .and_then(|stack| crate::item_prototypes::prototype(&stack.item))
        .and_then(|prototype| prototype.tool.as_ref())
        .and_then(|tool| tool.attack_damage)
        .unwrap_or(1.0)
}

/// `Player.attack` knockback strength: the vanilla base 0.4 plus 0.5 per
/// `minecraft:knockback` level.
fn attack_knockback_strength(session: &PlayerSession) -> f64 {
    let level = session
        .inventory
        .get(session.selected_slot.max(0) as usize)
        .map_or(0, |stack| stack.enchantment_level("minecraft:knockback"));
    0.4 + 0.5 * f64::from(level)
}

/// `Player.attack` direction arguments for `LivingEntity.knockback`: the
/// horizontal look vector is `(sin(yaw), -cos(yaw))`; `knockback` subtracts it
/// so the victim travels along the player's facing direction.
fn attack_knockback_direction(yaw: f32) -> (f64, f64) {
    let radians = f64::from(yaw) * std::f64::consts::PI / 180.0;
    (radians.sin(), -radians.cos())
}

fn system_chat_packet(message: &str) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_SYSTEM_CHAT)?;
    writer.write_bytes(&component_nbt(message)?);
    writer.write_u8(0)?; // overlay = false (chat box)
    Ok(writer.into_inner())
}

fn disconnect_packet(reason: &str) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_DISCONNECT)?;
    writer.write_bytes(&component_nbt(reason)?);
    Ok(writer.into_inner())
}

fn set_time_packet(game_time: i64, day_time: i64, tick_day_time: bool) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_SET_TIME).unwrap();
    writer.write_i64(game_time).unwrap();
    writer.write_i64(day_time).unwrap();
    writer.write_u8(u8::from(tick_day_time)).unwrap();
    writer.into_inner()
}

#[allow(clippy::too_many_arguments)]
fn sound_packet(
    sound: &str,
    category: i32,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_SOUND)?;
    // Holder.direct: id 0 followed by SoundEvent.DIRECT_STREAM_CODEC
    // (resource location + optional fixed range).
    writer.write_varint(0)?;
    writer.write_string(sound)?;
    writer.write_u8(0)?; // no fixed range
    writer.write_varint(category)?;
    writer.write_i32((x * 8.0) as i32)?;
    writer.write_i32((y * 8.0) as i32)?;
    writer.write_i32((z * 8.0) as i32)?;
    writer.write_f32(volume)?;
    writer.write_f32(pitch)?;
    writer.write_i64(seed)?;
    Ok(writer.into_inner())
}

/// Simple particle registry ids from the 1.21.11 built-in `particle_type`
/// registration order (`ParticleTypes`). Particles that carry option payloads
/// are intentionally absent: sending only their id would be a malformed packet,
/// so callers get an explicit unsupported error instead of a guess.
fn particle_registry_id(name: &str) -> Option<i32> {
    let path = name
        .trim()
        .split_once(':')
        .map_or(name.trim(), |(_, path)| path);
    match path {
        "angry_villager" => Some(0),
        "bubble" => Some(3),
        "cloud" => Some(4),
        "copper_fire_flame" => Some(5),
        "crit" => Some(6),
        "damage_indicator" => Some(7),
        "dripping_lava" => Some(9),
        "falling_lava" => Some(10),
        "landing_lava" => Some(11),
        "dripping_water" => Some(12),
        "falling_water" => Some(13),
        "elder_guardian" => Some(17),
        "enchanted_hit" => Some(18),
        "enchant" => Some(19),
        "end_rod" => Some(20),
        "explosion_emitter" => Some(22),
        "explosion" => Some(23),
        "gust" => Some(24),
        "small_gust" => Some(25),
        "gust_emitter_large" => Some(26),
        "gust_emitter_small" => Some(27),
        "sonic_boom" => Some(28),
        "firework" => Some(30),
        "fishing" => Some(31),
        "flame" => Some(32),
        "infested" => Some(33),
        "cherry_leaves" => Some(34),
        "pale_oak_leaves" => Some(35),
        "sculk_soul" => Some(37),
        "sculk_charge_pop" => Some(39),
        "soul_fire_flame" => Some(40),
        "soul" => Some(41),
        "happy_villager" => Some(43),
        "composter" => Some(44),
        "heart" => Some(45),
        "item_slime" => Some(50),
        "item_cobweb" => Some(51),
        "item_snowball" => Some(52),
        "large_smoke" => Some(53),
        "lava" => Some(54),
        "mycelium" => Some(55),
        "note" => Some(56),
        "poof" => Some(57),
        "portal" => Some(58),
        "rain" => Some(59),
        "smoke" => Some(60),
        "white_smoke" => Some(61),
        "sneeze" => Some(62),
        "spit" => Some(63),
        "squid_ink" => Some(64),
        "sweep_attack" => Some(65),
        "totem_of_undying" => Some(66),
        "underwater" => Some(67),
        "splash" => Some(68),
        "witch" => Some(69),
        "bubble_pop" => Some(70),
        "current_down" => Some(71),
        "bubble_column_up" => Some(72),
        "nautilus" => Some(73),
        "dolphin" => Some(74),
        "campfire_cosy_smoke" => Some(75),
        "campfire_signal_smoke" => Some(76),
        "dripping_honey" => Some(77),
        "falling_honey" => Some(78),
        "landing_honey" => Some(79),
        "falling_nectar" => Some(80),
        "falling_spore_blossom" => Some(81),
        "ash" => Some(82),
        "crimson_spore" => Some(83),
        "warped_spore" => Some(84),
        "spore_blossom_air" => Some(85),
        "dripping_obsidian_tear" => Some(86),
        "falling_obsidian_tear" => Some(87),
        "landing_obsidian_tear" => Some(88),
        "reverse_portal" => Some(89),
        "white_ash" => Some(90),
        "small_flame" => Some(91),
        "snowflake" => Some(92),
        "dripping_dripstone_lava" => Some(93),
        "falling_dripstone_lava" => Some(94),
        "dripping_dripstone_water" => Some(95),
        "falling_dripstone_water" => Some(96),
        "glow_squid_ink" => Some(97),
        "glow" => Some(98),
        "wax_on" => Some(99),
        "wax_off" => Some(100),
        "electric_spark" => Some(101),
        "scrape" => Some(102),
        "egg_crack" => Some(104),
        "dust_plume" => Some(105),
        "trial_spawner_detection" => Some(106),
        "trial_spawner_detection_ominous" => Some(107),
        "vault_connection" => Some(108),
        "ominous_spawning" => Some(110),
        "raid_omen" => Some(111),
        "trial_omen" => Some(112),
        "firefly" => Some(114),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn particle_packet(
    particle: &str,
    x: f64,
    y: f64,
    z: f64,
    offset_x: f32,
    offset_y: f32,
    offset_z: f32,
    speed: f32,
    count: i32,
    force: bool,
) -> Option<Vec<u8>> {
    let id = particle_registry_id(particle)?;
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_LEVEL_PARTICLES).unwrap();
    writer.write_u8(0).unwrap(); // override limiter = false
    writer.write_u8(u8::from(force)).unwrap(); // always show
    writer.write_f64(x).unwrap();
    writer.write_f64(y).unwrap();
    writer.write_f64(z).unwrap();
    writer.write_f32(offset_x).unwrap();
    writer.write_f32(offset_y).unwrap();
    writer.write_f32(offset_z).unwrap();
    writer.write_f32(speed).unwrap();
    writer.write_i32(count).unwrap();
    writer.write_varint(id).unwrap();
    Some(writer.into_inner())
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
    stream: &mut LoginStream,
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
    if let Err(error) = dimension::save_playerdata_preserving_dimension(session, playerdata_dir()) {
        eprintln!("[join] could not save game mode: {error}");
    }
    println!(
        "[join] {} switched to {} mode",
        session.name,
        game_mode_name(mode)
    );
    Ok(())
}

fn parse_command_packet(packet: &[u8]) -> io::Result<(String, crate::protocol::LastSeenUpdate)> {
    let mut reader = PacketReader::new(packet);
    match reader.read_varint()? {
        crate::protocol::SERVERBOUND_CHAT_COMMAND => {
            let command = crate::protocol::parse_serverbound_chat_command(packet)?;
            Ok((command.command, command.last_seen))
        }
        crate::protocol::SERVERBOUND_CHAT_COMMAND_SIGNED => {
            let command = crate::protocol::parse_serverbound_chat_command_signed(packet)?;
            Ok((command.command, command.last_seen))
        }
        id => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a chat command packet, got id {id}"),
        )),
    }
}

fn parse_teleport_coords(parts: &mut std::str::SplitWhitespace<'_>) -> Option<(f64, f64, f64)> {
    let x = parts.next()?.parse::<f64>().ok()?;
    let y = parts.next()?.parse::<f64>().ok()?;
    let z = parts.next()?.parse::<f64>().ok()?;
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return None;
    }
    Some((x, y, z))
}

fn parse_effect_give(
    parts: &mut std::str::SplitWhitespace<'_>,
) -> Option<(crate::effects::StatusEffectKind, i32, u8)> {
    let effect = parts.next()?;
    let kind = crate::effects::StatusEffectKind::from_id(effect)?;
    let seconds = if let Some(seconds) = parts.next() {
        seconds.parse::<i32>().ok()?
    } else {
        30
    };
    if !(1..=1_000_000).contains(&seconds) {
        return None;
    }
    let amplifier = if let Some(amplifier) = parts.next() {
        amplifier.parse::<i32>().ok()?
    } else {
        0
    };
    if !(0..=255).contains(&amplifier) {
        return None;
    }
    Some((kind, seconds.saturating_mul(20), amplifier as u8))
}

type SummonPosition = (f64, f64, f64);
type ParsedSummon = (String, Option<SummonPosition>);
type PendingSummon = (String, SummonPosition);

fn parse_summon(parts: &mut std::str::SplitWhitespace<'_>) -> Option<ParsedSummon> {
    let name = parts.next()?;
    let name = if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    };
    crate::spawn::entity_type(&name)?;
    let first = parts.next();
    let coords = match first {
        None => None,
        Some(x) => {
            let y = parts.next()?;
            let z = parts.next()?;
            let x = x.parse::<f64>().ok()?;
            let y = y.parse::<f64>().ok()?;
            let z = z.parse::<f64>().ok()?;
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return None;
            }
            Some((x, y, z))
        }
    };
    if parts.next().is_some() {
        return None;
    }
    Some((name, coords))
}

fn parse_give(parts: &mut std::str::SplitWhitespace<'_>) -> Option<(String, i32)> {
    let item = parts.next()?;
    let name = if item.contains(':') {
        item.to_string()
    } else {
        format!("minecraft:{item}")
    };
    crate::item_prototypes::prototype(&name)?;
    let count = if let Some(count) = parts.next() {
        count.parse::<i32>().ok()?
    } else {
        1
    };
    if !(1..=64).contains(&count) {
        return None;
    }
    Some((name, count))
}

fn clear_player_items(session: &mut PlayerSession) -> bool {
    let mut had_items = false;
    for slot in session.inventory.iter_mut() {
        if !slot.is_empty() {
            had_items = true;
            *slot = crate::inventory::ItemStack::empty();
        }
    }
    for slot in session.armor.iter_mut() {
        if !slot.is_empty() {
            had_items = true;
            *slot = crate::inventory::ItemStack::empty();
        }
    }
    if !session.cursor.is_empty() {
        had_items = true;
        session.cursor = crate::inventory::ItemStack::empty();
    }
    had_items
}

fn seed_message(seed: i64) -> String {
    format!("Seed: [{seed}]")
}

fn kill_targets_self(parts: &mut std::str::SplitWhitespace<'_>, player_name: &str) -> bool {
    match parts.next() {
        None => true,
        Some(name) => name.eq_ignore_ascii_case(player_name) && parts.next().is_none(),
    }
}

fn apply_enchantment_to_selected(
    session: &mut PlayerSession,
    name: &str,
    level: i32,
) -> Result<(), String> {
    let name = if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    };
    let Some(definition) = crate::item_components::enchanting::definition(&name) else {
        return Err(format!("unknown enchantment {name}"));
    };
    if level < 1 || level > definition.max_level {
        return Err(format!(
            "level out of range for {name}: {level} (max {})",
            definition.max_level
        ));
    }
    let slot = session.selected_slot.max(0) as usize;
    let Some(stack) = session.inventory.get_mut(slot) else {
        return Err("selected item slot is unavailable".to_string());
    };
    if stack.is_empty() {
        return Err("selected item is empty".to_string());
    }
    let mut enchantments = match stack.get_component("minecraft:enchantments") {
        Some(crate::item_components::ComponentValue::Enchantments(map)) => map.clone(),
        _ => std::collections::BTreeMap::new(),
    };
    enchantments.insert(definition.name.to_string(), level);
    stack.set_component(crate::item_components::ComponentValue::Enchantments(
        enchantments,
    ));
    Ok(())
}

/// Resolve a `/time set` value to a non-negative day time in ticks.
///
/// The named presets match vanilla (day/noon/night/midnight). Numeric values
/// are accepted as-is because vanilla `/time set` accepts arbitrary ticks.
/// Empty, negative, and unparsable values are rejected: vanilla would wrap
/// negatives modulo 24000, but this slice has no persistent world clock to
/// reconcile that against, so negatives are rejected explicitly rather than
/// silently misrendering the sky.
fn time_set_ticks(value: &str) -> Option<i64> {
    let ticks = match value {
        "day" => 1_000,
        "noon" => 6_000,
        "night" => 13_000,
        "midnight" => 18_000,
        other => other.parse::<i64>().ok()?,
    };
    (ticks >= 0).then_some(ticks)
}

/// Build the `ClientboundGameEvent` sequence for a `/weather` preset.
///
/// Event ids: 1 = START_RAINING, 2 = STOP_RAINING, 7 = RAIN_LEVEL_CHANGE,
/// 8 = THUNDER_LEVEL_CHANGE. Unknown kinds return `None`.
fn weather_events(kind: &str) -> Option<Vec<(u8, f32)>> {
    match kind {
        "clear" => Some(vec![(2, 0.0), (7, 0.0), (8, 0.0)]),
        "rain" => Some(vec![(1, 0.0), (7, 1.0), (8, 0.0)]),
        "thunder" => Some(vec![(1, 0.0), (7, 1.0), (8, 1.0)]),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    last_seen: &mut LastSeenValidator,
    pending_teleport: &mut Option<(f64, f64, f64)>,
    pending_summon: &mut Option<PendingSummon>,
    pending_kill: &mut bool,
    player_entity: i32,
    game_tick: u64,
    packet: &[u8],
) -> io::Result<()> {
    let (command, update) = parse_command_packet(packet)?;
    last_seen.apply_update(update).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid command last-seen update: {error}"),
        )
    })?;
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
        Some("xp") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /xp",
                    session.name
                );
                return Ok(());
            }
            if parts.next() != Some("add") {
                println!("[join] {}: usage: /xp add <points>", session.name);
                return Ok(());
            }
            let Some(amount) = parts.next().and_then(|value| value.parse::<i32>().ok()) else {
                println!("[join] {}: usage: /xp add <points>", session.name);
                return Ok(());
            };
            if amount <= 0 {
                println!(
                    "[join] {}: /xp add requires a positive point amount",
                    session.name
                );
                return Ok(());
            }
            session.add_experience(amount);
            write_packet(
                stream,
                &crate::entity_metadata::set_experience_packet(
                    session.xp_progress,
                    session.xp_level,
                    session.xp_total,
                ),
            )?;
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
            {
                eprintln!("[join] could not save xp change: {error}");
            }
            println!(
                "[join] {} added {amount} xp (level {}, total {})",
                session.name, session.xp_level, session.xp_total
            );
            Ok(())
        }
        Some("tp" | "teleport") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /tp",
                    session.name
                );
                return Ok(());
            }
            let Some((x, y, z)) = parse_teleport_coords(&mut parts) else {
                println!("[join] {}: usage: /tp <x> <y> <z>", session.name);
                return Ok(());
            };
            *pending_teleport = Some((x, y, z));
            println!(
                "[join] {} requested teleport to ({:.1}, {:.1}, {:.1})",
                session.name, x, y, z
            );
            Ok(())
        }
        Some("give") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /give",
                    session.name
                );
                return Ok(());
            }
            let Some((name, count)) = parse_give(&mut parts) else {
                println!("[join] {}: usage: /give <item> [count]", session.name);
                return Ok(());
            };
            let mut stack = crate::inventory::ItemStack::new(name, count);
            crate::item_prototypes::apply_defaults(&mut stack);
            let leftover = crate::inventory::add_stack(&mut session.inventory, stack);
            send_container_content(stream, session)?;
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
            {
                eprintln!("[join] could not save give: {error}");
            }
            if !leftover.is_empty() {
                println!(
                    "[join] {}: inventory full, could not fit {}x {}",
                    session.name, leftover.count, leftover.item
                );
            }
            Ok(())
        }
        Some("enchant") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /enchant",
                    session.name
                );
                return Ok(());
            }
            let Some(enchantment) = parts.next() else {
                println!(
                    "[join] {}: usage: /enchant <enchantment> <level>",
                    session.name
                );
                return Ok(());
            };
            let Some(level) = parts.next().and_then(|value| value.parse::<i32>().ok()) else {
                println!(
                    "[join] {}: usage: /enchant <enchantment> <level>",
                    session.name
                );
                return Ok(());
            };
            match apply_enchantment_to_selected(session, enchantment, level) {
                Ok(()) => {
                    let slot = session.selected_slot.max(0) as usize;
                    if let Some(stack) = session.inventory.get(slot) {
                        write_packet(stream, &inventory::inventory_packet(slot as i32, stack))?;
                    }
                    if let Err(error) =
                        dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                    {
                        eprintln!("[join] could not save enchant: {error}");
                    }
                    println!(
                        "[join] {} enchanted selected item with {} {}",
                        session.name, enchantment, level
                    );
                }
                Err(message) => {
                    println!("[join] {}: {message}", session.name);
                }
            }
            Ok(())
        }
        Some("effect") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /effect",
                    session.name
                );
                return Ok(());
            }
            if parts.next() != Some("give") {
                println!(
                    "[join] {}: usage: /effect give <effect> [seconds] [amplifier]",
                    session.name
                );
                return Ok(());
            }
            let Some((kind, duration, amplifier)) = parse_effect_give(&mut parts) else {
                println!(
                    "[join] {}: usage: /effect give <effect> [seconds] [amplifier]",
                    session.name
                );
                return Ok(());
            };
            let result = {
                let mut store = crate::entity_metadata::living_entities();
                let player = store.entry_player(player_entity);
                player.apply_effect(crate::effects::StatusEffect::new(kind, duration, amplifier))
            };
            for packet in result.packets {
                write_packet(stream, &packet)?;
            }
            println!(
                "[join] {} applied effect {} for {} ticks (amplifier {})",
                session.name,
                kind.registry_id(),
                duration,
                amplifier
            );
            Ok(())
        }
        Some("summon") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /summon",
                    session.name
                );
                return Ok(());
            }
            let Some((name, coords)) = parse_summon(&mut parts) else {
                println!("[join] {}: usage: /summon <entity> [x y z]", session.name);
                return Ok(());
            };
            let pos = coords.unwrap_or((session.x + 2.0, session.y, session.z));
            *pending_summon = Some((name, pos));
            Ok(())
        }
        Some("kill") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /kill",
                    session.name
                );
                return Ok(());
            }
            if kill_targets_self(&mut parts, &session.name) {
                *pending_kill = true;
                println!("[join] {} requested /kill", session.name);
            } else {
                println!("[join] {}: usage: /kill [self]", session.name);
            }
            Ok(())
        }
        Some("clear") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /clear",
                    session.name
                );
                return Ok(());
            }
            let armor_before = session.armor.clone();
            let cleared = clear_player_items(session);
            if cleared {
                send_container_content(stream, session)?;
                if armor_before != session.armor {
                    write_packet(
                        stream,
                        &inventory::container_set_slot_packet(5, &session.armor[3]),
                    )?;
                    write_packet(
                        stream,
                        &inventory::container_set_slot_packet(6, &session.armor[2]),
                    )?;
                    write_packet(
                        stream,
                        &inventory::container_set_slot_packet(7, &session.armor[1]),
                    )?;
                    write_packet(
                        stream,
                        &inventory::container_set_slot_packet(8, &session.armor[0]),
                    )?;
                    write_packet(
                        stream,
                        &crate::entity_metadata::set_equipment_packet(
                            player_entity,
                            &player_equipment(session),
                        ),
                    )?;
                }
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                {
                    eprintln!("[join] could not save clear: {error}");
                }
                println!("[join] {} cleared their items", session.name);
            } else {
                println!("[join] {} has no items to clear", session.name);
            }
            Ok(())
        }
        Some("seed") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /seed",
                    session.name
                );
                return Ok(());
            }
            let seed = crate::worldgen::generator::configured()
                .map(|generator| generator.seed())
                .unwrap_or(0);
            write_packet(stream, &system_chat_packet(&seed_message(seed))?)?;
            Ok(())
        }
        Some("time") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /time",
                    session.name
                );
                return Ok(());
            }
            match parts.next() {
                Some("set") => {}
                Some("add") => {
                    let message = "/time add is not modelled yet";
                    println!("[join] {}: {message}", session.name);
                    write_packet(stream, &system_chat_packet(message)?)?;
                    return Ok(());
                }
                _ => {
                    println!(
                        "[join] {}: usage: /time set <day|noon|night|midnight|ticks>",
                        session.name
                    );
                    return Ok(());
                }
            }
            let Some(value) = parts.next() else {
                println!(
                    "[join] {}: usage: /time set <day|noon|night|midnight|ticks>",
                    session.name
                );
                return Ok(());
            };
            let Some(day_time) = time_set_ticks(value) else {
                let message = format!("invalid time value: {value}");
                println!("[join] {}: {message}", session.name);
                write_packet(stream, &system_chat_packet(&message)?)?;
                return Ok(());
            };
            let sent = broadcast_world_time(game_tick as i64, day_time, true);
            let message = format!("Set time to {day_time} (broadcast to {sent} session(s))");
            println!("[join] {}: {message}", session.name);
            write_packet(stream, &system_chat_packet(&message)?)?;
            Ok(())
        }
        Some("weather") => {
            if !is_operator(&session.name) {
                println!(
                    "[join] {}: you do not have permission to use /weather",
                    session.name
                );
                return Ok(());
            }
            let Some(kind) = parts.next() else {
                println!(
                    "[join] {}: usage: /weather <clear|rain|thunder> [duration]",
                    session.name
                );
                return Ok(());
            };
            let Some(events) = weather_events(kind) else {
                let message = format!("unknown weather kind: {kind}");
                println!("[join] {}: {message}", session.name);
                write_packet(stream, &system_chat_packet(&message)?)?;
                return Ok(());
            };
            let duration = parts.next();
            let parsed_duration = duration.and_then(|value| value.parse::<u64>().ok());
            for (event, param) in &events {
                broadcast_game_event(*event, *param);
            }
            let ignored_note = match (duration, parsed_duration) {
                (Some(_raw), Some(seconds)) => format!(
                    " (duration {seconds}s supplied but ignored: no persistent weather clock)"
                ),
                (Some(raw), None) => {
                    format!(" (duration '{raw}' ignored: no persistent weather clock)")
                }
                (None, _) => String::new(),
            };
            let message = format!("Weather set to {kind}{ignored_note}");
            println!("[join] {}: {message}", session.name);
            write_packet(stream, &system_chat_packet(&message)?)?;
            Ok(())
        }
        Some(other) => {
            println!("[join] {}: unknown command /{other}", session.name);
            Ok(())
        }
        None => Ok(()),
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

/// A playerdata file can hold `health = 0` when the player disconnected on the
/// death screen or after a void fall. Vanilla resumes that state through the
/// respawn flow; this server has no persisted death screen, so restore a valid
/// join state instead of leaving a zero-health session stuck in the void.
fn restore_join_vitals(session: &mut PlayerSession) {
    session.health = 20.0;
    session.food_level = 20;
    session.food_saturation = 5.0;
    session.food_exhaustion = 0.0;
    session.air = 300;
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

/// Result of one player damage application. `packets` follows the
/// living-entity order: synched-data health/pose, then `ClientboundSetHealth`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerDamage {
    pub damaged: bool,
    pub died: bool,
    pub packets: Vec<Vec<u8>>,
}

/// Create or reset the metadata-store record that mirrors this player session.
/// The process-wide store is used because the entity bridge and plugin layer
/// already address players by network id.
pub fn register_player_entity(session: &PlayerSession, entity_id: i32) {
    let mut player = crate::entity_metadata::LivingEntity::new_player(entity_id);
    player.food = session.food_level.clamp(0, 20);
    player.saturation = session.food_saturation.max(0.0);
    player.set_health(session.health.clamp(0.0, player.max_health()));
    // Join sends the session's own health packet; the store's initial health
    // must not be flushed as a second update on the first tick.
    player.metadata.pack_dirty();
    crate::entity_metadata::living_entities().insert(player);
}

/// Reset the metadata-store player after a respawn, matching the existing
/// `session.health = 20.0` respawn behavior.
pub fn reset_player_entity(session: &mut PlayerSession, entity_id: i32) {
    let mut store = crate::entity_metadata::living_entities();
    let player = store.entry_player(entity_id);
    player.dead = false;
    player.death_time = 0;
    player.death_broadcasted = false;
    player.removed = false;
    player.invulnerable_time = 0;
    player.hurt_time = 0;
    player.last_hurt = 0.0;
    player.food = session.food_level.clamp(0, 20);
    player.saturation = session.food_saturation.max(0.0);
    player.set_health(20.0);
    player.metadata.pack_dirty();
    session.health = player.health;
}

fn player_equipment(session: &PlayerSession) -> crate::entity_metadata::Equipment {
    let mut equipment = crate::entity_metadata::Equipment::new();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Feet) = session.armor[0].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Legs) = session.armor[1].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Chest) = session.armor[2].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Head) = session.armor[3].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::MainHand) = session
        .selected_item()
        .cloned()
        .unwrap_or_else(crate::inventory::ItemStack::empty);
    equipment
}

fn take_player_death_experience(session: &mut PlayerSession) -> i32 {
    let amount = (session.xp_level.max(0) * 7).min(100);
    session.xp_level = 0;
    session.xp_progress = 0.0;
    session.xp_total = 0;
    amount
}

fn take_player_death_drops(session: &mut PlayerSession) -> Vec<crate::item_components::ItemStack> {
    let mut drops = Vec::new();
    for slot in session.inventory.iter_mut() {
        if !slot.is_empty() {
            drops.push(std::mem::replace(
                slot,
                crate::inventory::ItemStack::empty(),
            ));
        }
    }
    for slot in session.armor.iter_mut() {
        if !slot.is_empty() {
            drops.push(std::mem::replace(
                slot,
                crate::inventory::ItemStack::empty(),
            ));
        }
    }
    if !session.cursor.is_empty() {
        drops.push(std::mem::replace(
            &mut session.cursor,
            crate::inventory::ItemStack::empty(),
        ));
    }
    drops
}

fn drop_player_death_items(
    stream: &mut LoginStream,
    drops: &mut DropEntityStore,
    rng: &mut dyn crate::random::RandomSource,
    session: &mut PlayerSession,
) -> io::Result<()> {
    let stacks = take_player_death_drops(session);
    let pos = BlockPos::new(
        session.x.floor() as i32,
        session.y.floor() as i32,
        session.z.floor() as i32,
    );
    for packet in drops.spawn_stacks(pos, &stacks, rng) {
        write_packet(stream, &packet)?;
    }
    let experience = take_player_death_experience(session);
    for packet in drops.spawn_experience(pos, experience, rng) {
        write_packet(stream, &packet)?;
    }
    send_container_content(stream, session)?;
    Ok(())
}

/// Apply damage through the living-entity model so the client receives the
/// same metadata health update and `ClientboundSetHealth` body as vanilla.
/// Armor mitigation uses the raw amount; worn armor loses durability only when
/// the hit is not rejected.
pub fn damage_player_with_rng(
    session: &mut PlayerSession,
    entity_id: i32,
    amount: f32,
    rng: &mut dyn folia_random::RandomSource,
    kind: crate::effects::DamageKind,
) -> PlayerDamage {
    let mut store = crate::entity_metadata::living_entities();
    let player = store.entry_player(entity_id);
    player.food = session.food_level.clamp(0, 20);
    player.saturation = session.food_saturation.max(0.0);
    if (player.health - session.health).abs() > f32::EPSILON {
        player.set_health(session.health);
        player.metadata.pack_dirty();
    }
    let mut equipment = crate::entity_metadata::Equipment::new();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Feet) = session.armor[0].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Legs) = session.armor[1].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Chest) = session.armor[2].clone();
    *equipment.get_mut(crate::entity_metadata::EquipmentSlot::Head) = session.armor[3].clone();
    let (armor, toughness) = crate::effects::armor_values_from_equipment(&equipment);
    let after_armor = crate::effects::damage_after_armor(amount, armor, toughness);
    let protection = crate::effects::enchantment_damage_protection(&equipment, kind);
    let after_magic = crate::effects::damage_after_enchantments(after_armor, protection);
    let after_resistance = after_magic * player.effects.resistance_multiplier();
    let mut result = player.hurt(after_resistance);
    session.health = player.health;

    if result.damaged && amount > 0.0 {
        let points = ((amount / 4.0).floor() as i32).max(1);
        let mut armor_changed = false;
        for slot in session.armor.iter_mut() {
            if slot.is_empty()
                || slot.get_component("minecraft:unbreakable").is_some()
                || slot.max_damage() <= 0
            {
                continue;
            }
            let mut wear = 0;
            let unbreaking = slot.enchantment_level("minecraft:unbreaking");
            for _ in 0..points {
                if unbreaking > 0 && rng.next_int_bound(unbreaking + 1) != 0 {
                    continue;
                }
                wear += 1;
            }
            if wear <= 0 {
                continue;
            }
            let next = slot.damage() + wear;
            armor_changed = true;
            if next >= slot.max_damage() {
                *slot = crate::inventory::ItemStack::empty();
            } else {
                slot.set_damage(next);
            }
        }
        if armor_changed {
            result
                .packets
                .push(crate::entity_metadata::set_equipment_packet(
                    entity_id,
                    &player_equipment(session),
                ));
        }
    }

    PlayerDamage {
        damaged: result.damaged,
        died: result.died,
        packets: result.packets,
    }
}

pub fn damage_player(session: &mut PlayerSession, entity_id: i32, amount: f32) -> PlayerDamage {
    let mut rng = folia_random::LegacyRandomSource::new(0);
    damage_player_with_rng(
        session,
        entity_id,
        amount,
        &mut rng,
        crate::effects::DamageKind::Physical,
    )
}

/// Void damage repeats every half second; the living model keeps a player's
/// invulnerability timer at 20 ticks, so reset it before each void hit to keep
/// the existing damage cadence.
fn clear_player_invulnerability(entity_id: i32) {
    if let Some(player) = crate::entity_metadata::living_entities().get_mut(entity_id) {
        player.invulnerable_time = 0;
        player.last_hurt = 0.0;
    }
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

/// Whether a broadcast packet belongs to this session's entity view: either one
/// of its own tracked mobs or a process-wide entity the bridge spawned. Other
/// entity packets are dropped instead of leaking across sessions.
fn entity_packet_visible(packet: &[u8], tracked: &SessionEntityTracker) -> bool {
    let mut reader = PacketReader::new(packet);
    let Ok(packet_id) = reader.read_varint() else {
        return true;
    };
    let entity_id = match packet_id {
        crate::entity::CLIENTBOUND_ADD_ENTITY | crate::entity::CLIENTBOUND_ENTITY_POSITION_SYNC => {
            reader.read_varint().ok()
        }
        crate::entity_metadata::CLIENTBOUND_SET_ENTITY_DATA
        | crate::entity_metadata::CLIENTBOUND_SET_EQUIPMENT => reader.read_varint().ok(),
        crate::entity_metadata::CLIENTBOUND_ENTITY_EVENT => reader.read_i32().ok(),
        // Removals are always forwarded: the broadcast entity map has already
        // forgotten the id, and removing an unknown id is harmless on the wire.
        crate::entity::CLIENTBOUND_REMOVE_ENTITIES => return true,
        _ => return true,
    };
    match entity_id {
        Some(id) => tracked.tracks(id) || crate::entity::is_tracked(id),
        None => true,
    }
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

/// `ClientboundRespawnPacket` for a dimension transition.
///
/// Body: `CommonPlayerSpawnInfo` (dimension type registry id, dimension key,
/// seed, game mode, previous game mode, debug/flat flags, optional last death
/// location, portal cooldown, sea level) followed by `dataToKeep`
/// (1 = keep attribute modifiers, which is what a portal change uses).
pub fn dimension_respawn_packet(
    dimension: crate::world::Dimension,
    game_mode: i32,
    seed: i64,
    data_to_keep: u8,
) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAY_RESPAWN)?;
    writer.write_varint(dimension.type_id())?;
    writer.write_string(dimension.resource_name())?;
    writer.write_i64(seed)?;
    writer.write_u8(game_mode.clamp(0, 3) as u8)?;
    writer.write_u8(0xff)?; // no previous game mode
    writer.write_u8(0)?; // debug
    writer.write_u8(0)?; // flat
    writer.write_u8(0)?; // no last death location
    writer.write_varint(0)?; // portal cooldown
    writer.write_varint(dimension.sea_level())?;
    writer.write_u8(data_to_keep)?;
    Ok(writer.into_inner())
}

/// Death respawn always returns to the overworld spawn.
fn respawn_packet(game_mode: i32) -> io::Result<Vec<u8>> {
    dimension_respawn_packet(crate::world::Dimension::Overworld, game_mode, 0, 0)
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
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    damage: f32,
    rng: &mut dyn crate::random::RandomSource,
) -> io::Result<bool> {
    if fall_damage_health(session.health, damage, session.game_mode).is_none() {
        return Ok(false);
    }
    let result = damage_player_with_rng(
        session,
        player_entity_id(),
        damage,
        rng,
        crate::effects::DamageKind::Fall,
    );
    if !result.damaged {
        return Ok(false);
    }
    for packet in &result.packets {
        write_packet(stream, packet)?;
    }
    if result.died {
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
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    drops: &mut DropEntityStore,
    rng: &mut dyn crate::random::RandomSource,
    last_damage: &mut Instant,
) -> io::Result<bool> {
    if session.y < VOID_DAMAGE_Y && session.health <= 0.0 {
        // A player restored from a zero-health playerdata file can already be
        // below the void line. `hurt` rejects zero-health entities, so drive
        // the death/respawn flow directly instead of falling forever.
        *last_damage = Instant::now();
        drop_player_death_items(stream, drops, rng, session)?;
        write_packet(stream, &combat_kill_packet("Player fell out of the world"))?;
        return Ok(true);
    }
    if void_damage_health(session.health, session.y, session.game_mode).is_none() {
        return Ok(false);
    }
    let now = Instant::now();
    if now.duration_since(*last_damage) < VOID_DAMAGE_INTERVAL {
        return Ok(false);
    }
    *last_damage = now;
    let entity_id = player_entity_id();
    clear_player_invulnerability(entity_id);
    let result = damage_player_with_rng(
        session,
        entity_id,
        VOID_DAMAGE,
        rng,
        crate::effects::DamageKind::Void,
    );
    if !result.damaged {
        return Ok(false);
    }
    for packet in &result.packets {
        write_packet(stream, packet)?;
    }
    if result.died {
        drop_player_death_items(stream, drops, rng, session)?;
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
    stream: &mut LoginStream,
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
    reset_player_entity(session, player_entity_id());
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
        crate::world::Dimension::Overworld,
        0,
        radius,
        *center_x,
        *center_z,
        sent_chunks,
        usize::MAX,
    )?;
    if batch.streamed > 0 || batch.unloaded > 0 {
        println!(
            "[join] respawned {} at ({:.1}, {:.1}, {:.1}); streamed {} chunks",
            session.name, session.x, session.y, session.z, batch.streamed
        );
    }
    if let Err(error) = dimension::save_playerdata_with_dimension(
        session,
        playerdata_dir(),
        crate::world::Dimension::Overworld,
    ) {
        eprintln!("[join] could not save respawn: {error}");
    }
    Ok(())
}

/// Move a live session to another dimension.
///
/// The old position is saved first, the destination chunk is loaded/generated
/// and used to lift the player out of terrain, then the client receives the
/// dimension `Respawn`, position, health, abilities, and a fresh chunk batch
/// from the target dimension's storage. Playerdata is written twice: once with
/// the old dimension/position and once with the destination.
#[allow(clippy::too_many_arguments)]
fn switch_dimension(
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    menus: &mut inventory::SessionMenus,
    dig: &mut DigState,
    tracked: &mut SessionEntityTracker,
    drops: &mut DropEntityStore,
    trigger: &dimension::PortalTrigger,
    old_storage: &Mutex<WorldStorage>,
    new_storage: &'static Mutex<WorldStorage>,
    trust_stored: bool,
    seed: i64,
    radius: i32,
    center_x: &mut i32,
    center_z: &mut i32,
    sent_chunks: &mut HashSet<(i32, i32)>,
    teleport_id: &mut i32,
) -> io::Result<()> {
    // 1. Persist the old position in the old dimension.
    if let Err(error) =
        dimension::save_playerdata_with_dimension(session, playerdata_dir(), trigger.from)
    {
        eprintln!("[join] could not save the old dimension position: {error}");
    }

    // 2. A container menu cannot survive a dimension change.
    if let Some(menu) = menus.close() {
        let mut storage = crate::world::lock(old_storage);
        if let Err(error) = persist_menu_container(&mut storage, &menu) {
            eprintln!("[join] could not persist the menu before a dimension change: {error}");
        }
        write_packet(stream, &menu.close_packet())?;
    }
    tracked.clear();
    drops.clear();
    *dig = DigState::default();

    // 3. Load the destination column first so the arrival can be lifted out of
    //    solid blocks before the position packet goes out.
    let destination = arrival_position(
        new_storage,
        trust_stored,
        trigger.to,
        seed,
        trigger.destination,
    )?;
    session.teleport(destination[0], destination[1], destination[2], false);
    session.reset_physics();

    // 4. Dimension change packets.
    write_packet(
        stream,
        &dimension_respawn_packet(trigger.to, session.game_mode, seed, 1)?,
    )?;
    *teleport_id += 1;
    write_packet(stream, &position_packet(session, *teleport_id))?;
    write_packet(stream, &health_packet(session))?;
    write_packet(stream, &abilities_packet(session.game_mode))?;

    // 5. Persist the new position and dimension.
    if let Err(error) =
        dimension::save_playerdata_with_dimension(session, playerdata_dir(), trigger.to)
    {
        eprintln!("[join] could not save the new dimension position: {error}");
    }

    // 6. The client drops every column on Respawn; stream the destination
    //    dimension from scratch.
    *center_x = (session.x.floor() as i32).div_euclid(16);
    *center_z = (session.z.floor() as i32).div_euclid(16);
    sent_chunks.clear();
    let batch = stream_chunks_around(
        stream,
        new_storage,
        trust_stored,
        trigger.to,
        seed,
        radius,
        *center_x,
        *center_z,
        sent_chunks,
        usize::MAX,
    )?;
    *tracked = {
        let mut storage = crate::world::lock(new_storage);
        SessionEntityTracker::spawn_near_player(
            &mut storage,
            (session.x, session.y, session.z),
            i64::from_le_bytes(session.uuid[0..8].try_into().expect("eight uuid bytes")),
        )
    };
    for packet in tracked.join_packets() {
        write_packet(stream, &packet)?;
    }
    retain_session_entities(stream, tracked, drops, sent_chunks)?;
    println!(
        "[join] {} switched {:?} -> {:?} at ({:.1}, {:.1}, {:.1}); streamed {} chunks",
        session.name, trigger.from, trigger.to, session.x, session.y, session.z, batch.streamed
    );
    Ok(())
}

fn retain_session_entities(
    stream: &mut LoginStream,
    tracked: &mut SessionEntityTracker,
    drops: &mut DropEntityStore,
    sent_chunks: &HashSet<(i32, i32)>,
) -> io::Result<()> {
    for packet in tracked.retain_in_chunks(sent_chunks) {
        write_packet(stream, &packet)?;
    }
    for packet in drops.retain_in_chunks(sent_chunks) {
        write_packet(stream, &packet)?;
    }
    Ok(())
}

/// Loads the destination chunk and lifts a player whose feet or head would be
/// inside a colliding block. The End platform is already built by the End
/// generator, so this normally leaves the platform position untouched.
fn arrival_position(
    storage: &Mutex<WorldStorage>,
    trust_stored: bool,
    dimension: crate::world::Dimension,
    seed: i64,
    destination: [f64; 3],
) -> io::Result<[f64; 3]> {
    let block_x = destination[0].floor() as i32;
    let block_y = destination[1].floor() as i32;
    let block_z = destination[2].floor() as i32;
    let chunk_x = block_x.div_euclid(16);
    let chunk_z = block_z.div_euclid(16);
    let local_x = block_x.rem_euclid(16) as usize;
    let local_z = block_z.rem_euclid(16) as usize;
    let mut storage = crate::world::lock(storage);
    let (chunk, _) = load_or_generate_chunk(
        &mut storage,
        trust_stored,
        dimension,
        chunk_x,
        chunk_z,
        seed,
    )?;
    if let Some(y) = first_free_y(&chunk, local_x, local_z, block_y, dimension)
        && y != block_y
    {
        return Ok([destination[0], f64::from(y), destination[2]]);
    }
    Ok(destination)
}

/// Parses `ServerboundUseItemOn` and reports the clicked block when it is a
/// portal block.
fn portal_click(storage: &mut WorldStorage, packet: &[u8]) -> io::Result<Option<PortalKind>> {
    let use_item = crate::block_update::parse_use_item_on(packet)?;
    let Some(state) = block_at(storage, BlockPos::new(use_item.x, use_item.y, use_item.z))? else {
        return Ok(None);
    };
    Ok(dimension::portal_kind_for_block(&state.name))
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
#[allow(clippy::too_many_arguments)]
fn send_columns(
    stream: &mut LoginStream,
    storage: &mut WorldStorage,
    generator: Option<Arc<crate::worldgen::generator::WorldGenerator>>,
    dimension: crate::world::Dimension,
    seed: i64,
    pending: &[(usize, i32, i32)],
    loaded: &mut [Option<(Chunk, ChunkSource)>],
    batch: &mut ChunkBatch,
) -> io::Result<()> {
    if shutdown_requested() {
        return Ok(());
    }
    let overworld_generator = if dimension == crate::world::Dimension::Overworld {
        generator
    } else {
        None
    };
    if dimension == crate::world::Dimension::Overworld && overworld_generator.is_none() {
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
    }
    if pending.is_empty() {
        for column in loaded.iter_mut() {
            if let Some((chunk, source)) = column.take() {
                let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
                record_and_send(stream, batch, source, &packet)?;
            }
        }
        return Ok(());
    }

    let pending: Arc<Vec<(usize, i32, i32)>> = Arc::new(pending.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let workers = generation_workers(pending.len());
    let generation: Option<Arc<crate::worldgen::generator::WorldGenerator>> = overworld_generator;
    // Generation is detached: a chunk that is already being generated cannot
    // be cancelled, and shutdown must not wait for it. The bounded channel
    // still applies backpressure, and a dropped receiver wakes every worker.
    let (sender, receiver) = mpsc::sync_channel(GENERATION_QUEUE);
    for _ in 0..workers {
        let sender = sender.clone();
        let pending = Arc::clone(&pending);
        let next = Arc::clone(&next);
        let generator = generation.clone();
        std::thread::spawn(move || {
            loop {
                if shutdown_requested() {
                    break;
                }
                let position = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(index, chunk_x, chunk_z)) = pending.get(position) else {
                    break;
                };
                let result = match generator.as_deref() {
                    Some(generator) => {
                        generator
                            .generate_chunk(chunk_x, chunk_z)
                            .and_then(|chunk| {
                                let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
                                Ok((index, chunk, packet))
                            })
                    }
                    None => crate::worldgen::generator::generate_dimension(
                        dimension::generator_dimension(dimension),
                        chunk_x,
                        chunk_z,
                        seed,
                    )
                    .and_then(|chunk| {
                        let packet = ChunkPacketTemplate::build_dynamic(&chunk)?;
                        Ok((index, chunk, packet))
                    }),
                };
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
                    match receiver.recv_timeout(SESSION_TICK_INTERVAL) {
                        Ok(Ok((produced, chunk, packet))) => {
                            finished.insert(produced, (chunk, packet));
                        }
                        Ok(Err(error)) => return Err(error),
                        Err(mpsc::RecvTimeoutError::Timeout) if shutdown_requested() => {
                            return Ok(());
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) if shutdown_requested() => {
                            return Ok(());
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
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
}

fn record_and_send(
    stream: &mut LoginStream,
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
/// restart and block edits in it persist. Non-overworld dimensions use their
/// own generator and ignore the configured overworld generator.
fn load_or_generate_chunk(
    storage: &mut WorldStorage,
    trust_stored: bool,
    dimension: crate::world::Dimension,
    chunk_x: i32,
    chunk_z: i32,
    seed: i64,
) -> io::Result<(Chunk, ChunkSource)> {
    let generated = dimension != crate::world::Dimension::Overworld
        || crate::worldgen::generator::configured().is_some();
    let trusted = trust_stored || (generated && storage.is_written_chunk(chunk_x, chunk_z)?);
    if trusted && let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
        return Ok((chunk, ChunkSource::World));
    }
    if dimension == crate::world::Dimension::Overworld {
        match crate::worldgen::generator::configured() {
            Some(generator) => {
                let chunk = generator.generate_chunk(chunk_x, chunk_z)?;
                storage.save_chunk(&chunk, Compression::Zlib)?;
                return Ok((chunk, ChunkSource::Generated));
            }
            None => {
                return Ok((
                    crate::chunk_packet::flat_chunk(chunk_x, chunk_z),
                    ChunkSource::Flat,
                ));
            }
        }
    }
    let chunk = crate::worldgen::generator::generate_dimension(
        dimension::generator_dimension(dimension),
        chunk_x,
        chunk_z,
        seed,
    )?;
    storage.save_chunk(&chunk, Compression::Zlib)?;
    Ok((chunk, ChunkSource::Generated))
}

/// The first Y where both the player's feet and head blocks are non-colliding,
/// searching upward from `from`.
fn first_free_y(
    chunk: &Chunk,
    local_x: usize,
    local_z: usize,
    from: i32,
    dimension: crate::world::Dimension,
) -> Option<i32> {
    let top = dimension.min_y() + dimension.logical_height() - 2;
    for y in from..=top {
        let feet_blocked = chunk
            .block_state(local_x, y, local_z)
            .is_some_and(crate::block_behaviour::has_collision);
        let head_blocked = chunk
            .block_state(local_x, y + 1, local_z)
            .is_some_and(crate::block_behaviour::has_collision);
        if !feet_blocked && !head_blocked {
            return Some(y);
        }
    }
    None
}

/// The captured spawn packet puts new players at `y = -60`, four blocks above
/// bedrock, which is inside the generated terrain, and playerdata saved from
/// that spawn repeats the problem. Lift any player whose feet block is solid
/// to the first empty block above their column.
fn unstick_player(
    session: &mut PlayerSession,
    storage: &Mutex<WorldStorage>,
    trust_stored: bool,
    dimension: crate::world::Dimension,
    seed: i64,
) {
    let storage = &mut *crate::world::lock(storage);
    let feet = session.y.floor() as i32;
    let local_x = (session.x.floor() as i32).rem_euclid(16) as usize;
    let local_z = (session.z.floor() as i32).rem_euclid(16) as usize;
    let chunk_x = (session.x.floor() as i32).div_euclid(16);
    let chunk_z = (session.z.floor() as i32).div_euclid(16);
    let Ok((chunk, _)) =
        load_or_generate_chunk(storage, trust_stored, dimension, chunk_x, chunk_z, seed)
    else {
        return;
    };
    let Some(state) = chunk.block_state(local_x, feet, local_z) else {
        // A saved position outside the column (for example after falling far
        // below the world) cannot be inspected. Overworld survivors and
        // creative players return to the column surface; spectators keep
        // their position.
        if dimension == crate::world::Dimension::Overworld && !matches!(session.game_mode, 3) {
            let surface = crate::chunk::surface_y(&chunk, local_x, local_z);
            println!(
                "[join] settle {}: saved y={:.1} is outside the world; moved to the surface at y={surface}",
                session.name, session.y
            );
            session.y = f64::from(surface);
            session.on_ground = true;
        }
        return;
    };
    if crate::chunk::is_air_state(state) {
        // A position saved while falling, or from an older, higher spawn, can
        // leave the player in mid-air above the column. Settle survival and
        // adventure players onto the surface; creative and spectator players
        // are allowed to stay where they logged out. Non-overworld dimensions
        // have no comparable surface, so they keep the saved Y.
        if dimension != crate::world::Dimension::Overworld {
            return;
        }
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
    let surface = if dimension == crate::world::Dimension::Overworld {
        crate::chunk::surface_y(&chunk, local_x, local_z)
    } else {
        first_free_y(&chunk, local_x, local_z, feet, dimension).unwrap_or(feet)
    };
    println!(
        "[join] unstick {}: feet at y={feet} inside {}; moved to y={surface} in {:?}",
        session.name, state.name, dimension
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
    if let Err(save_error) =
        dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
    {
        eprintln!("[join] could not save playerdata: {save_error}");
    }
    if let Err(flush_error) = crate::world::lock(storage).flush() {
        eprintln!("[join] could not flush terrain: {flush_error}");
    }
    println!("[join] {name} disconnected: {error}");
}

impl PickupTarget for PlayerSession {
    fn position(&self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    fn add_item(
        &mut self,
        stack: crate::item_components::ItemStack,
    ) -> crate::item_components::ItemStack {
        crate::inventory::add_stack(&mut self.inventory, stack)
    }

    fn add_experience(&mut self, amount: i32) {
        PlayerSession::add_experience(self, amount);
    }
}

/// Everything a session needs from the level directory.
struct WorldContext {
    /// World root; per-dimension storages hang off this directory.
    root: PathBuf,
    /// Whether every stored chunk may be served, not just the ones this server
    /// wrote. True for a foreign world and when `FOLIA_LOAD_STORED=1`.
    trust_stored: bool,
    /// World spawn from `level.dat`, when the level has one.
    spawn: Option<(i32, i32, i32)>,
    /// Overworld generator seed, or the level's stored seed. Dimension
    /// generation uses the same seed for every dimension.
    seed: i64,
}

/// Open the level directory, creating `level.dat` the first time a world is
/// generated.
///
/// A world this server created carries [`crate::level::OWN_LEVEL_MARKER`], so
/// only chunks it wrote itself (stamped with [`crate::world::WRITTEN_MARKER`])
/// are trusted; leftover flat chunks next to them are regenerated. A level that
/// came from somewhere else has no marker, and every stored chunk is served.
/// `FOLIA_LOAD_STORED=1` trusts every stored chunk in either case.
///
/// Only the overworld `level.dat` is read or written. Nether/End sessions reuse
/// this level data for time, weather, and spawn; no per-dimension level file is
/// created, so a switch cannot corrupt the primary file.
fn open_world() -> WorldContext {
    let directory = world_dir();
    let forced = std::env::var("FOLIA_LOAD_STORED")
        .map(|value| matches!(value.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let storage = crate::world::shared_overworld(directory.clone());
    let generator_seed = crate::worldgen::generator::configured().map(|generator| generator.seed());
    let level_path = directory.join("level.dat");
    if forced {
        let level = crate::level::read(&level_path).ok();
        let seed = generator_seed
            .or_else(|| level.as_ref().and_then(|level| level.seed))
            .unwrap_or(0);
        return WorldContext {
            root: directory,
            trust_stored: true,
            spawn: level.and_then(|level| level.spawn),
            seed,
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
            let seed = generator_seed.or(level.seed).unwrap_or(0);
            return WorldContext {
                root: directory,
                trust_stored: !level.own_level,
                spawn: level.spawn,
                seed,
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
                root: directory,
                trust_stored: true,
                spawn: None,
                seed: generator_seed.unwrap_or(0),
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
            root: directory,
            trust_stored: true,
            spawn: None,
            seed: generator_seed.unwrap_or(0),
        };
    };

    let seed = generator.seed();
    // Anchor the world spawn on the terrain surface at the origin, so the spawn
    // is saved with the level instead of guessed per session.
    let spawn = match load_or_generate_chunk(
        &mut crate::world::lock(storage),
        false,
        crate::world::Dimension::Overworld,
        0,
        0,
        seed,
    ) {
        Ok((chunk, _)) => (8, crate::chunk::surface_y(&chunk, 8, 8), 8),
        Err(error) => {
            eprintln!("[join] could not generate the spawn chunk for level.dat: {error}");
            return WorldContext {
                root: directory,
                trust_stored: false,
                spawn: None,
                seed,
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
                root: directory,
                trust_stored: false,
                spawn: Some(spawn),
                seed,
            }
        }
        Err(error) => {
            eprintln!("[join] could not write {}: {error}", level_path.display());
            WorldContext {
                root: directory,
                trust_stored: false,
                spawn: None,
                seed,
            }
        }
    }
}

/// Sends the chunk-cache center, unloads columns that fell outside the live
/// radius, and sends every chunk in the radius that `sent` has not streamed
/// yet. Chunks are loaded/generated from the selected dimension's storage.
#[allow(clippy::too_many_arguments)]
fn stream_chunks_around(
    stream: &mut LoginStream,
    storage: &Mutex<WorldStorage>,
    trust_stored: bool,
    dimension: crate::world::Dimension,
    seed: i64,
    radius: i32,
    center_x: i32,
    center_z: i32,
    sent: &mut HashSet<(i32, i32)>,
    max_columns: usize,
) -> io::Result<ChunkBatch> {
    let storage = &mut *crate::world::lock(storage);
    write_packet(stream, &chunk_cache_center(center_x, center_z))?;
    let mut batch = ChunkBatch::default();
    for (chunk_x, chunk_z) in chunks_to_unload(sent, center_x, center_z, radius) {
        write_packet(stream, &forget_chunk_packet(chunk_x, chunk_z))?;
        sent.remove(&(chunk_x, chunk_z));
        batch.unloaded += 1;
    }

    // Columns that go out with this batch, in send order. Do not mark them as
    // sent until the budget is applied, or a truncated batch would leave holes.
    let mut columns = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let chunk_x = center_x + dx;
            let chunk_z = center_z + dz;
            if !sent.contains(&(chunk_x, chunk_z)) {
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
    if max_columns != usize::MAX && columns.len() > max_columns {
        columns.truncate(max_columns.max(1));
    }
    for &(chunk_x, chunk_z) in &columns {
        sent.insert((chunk_x, chunk_z));
    }

    // Stored terrain first; everything else is generated on worker threads so a
    // cold batch uses more than one core. Non-overworld dimensions always
    // generate because the configured overworld generator does not apply.
    let generated = dimension != crate::world::Dimension::Overworld
        || crate::worldgen::generator::configured().is_some();
    let mut loaded: Vec<Option<(Chunk, ChunkSource)>> = Vec::with_capacity(columns.len());
    let mut pending = Vec::new();
    for (index, (chunk_x, chunk_z)) in columns.iter().copied().enumerate() {
        // Checking every 16 columns keeps shutdown responsive even when the
        // region files are slow to inspect; the normal send loop checks every
        // column.
        if index % 16 == 0 && shutdown_requested() {
            return Ok(ChunkBatch::default());
        }
        let trusted = trust_stored || (generated && storage.is_written_chunk(chunk_x, chunk_z)?);
        if trusted && let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
            loaded.push(Some((chunk, ChunkSource::World)));
            continue;
        }
        loaded.push(None);
        pending.push((index, chunk_x, chunk_z));
    }
    let generator = crate::worldgen::generator::configured().cloned();
    send_columns(
        stream,
        storage,
        generator,
        dimension,
        seed,
        &pending,
        &mut loaded,
        &mut batch,
    )?;
    // Generated terrain stays in the region cache until the session's scheduled
    // flush: a region file is rewritten as a whole file, so flushing per batch
    // would rewrite the same multi-megabyte file on every border crossing.
    Ok(batch)
}

fn has_unsent_columns(
    sent: &HashSet<(i32, i32)>,
    center_x: i32,
    center_z: i32,
    radius: i32,
) -> bool {
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if !sent.contains(&(center_x + dx, center_z + dz)) {
                return true;
            }
        }
    }
    false
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

/// Decode the block entity into the menu kind, its container slots, and the
/// `ClientboundContainerSetData` values. Unknown block-entity types return
/// `None` and are never guessed into a menu.
fn menu_contents(
    entity: &BlockEntity,
) -> Option<(inventory::MenuKind, Vec<inventory::ItemStack>, Vec<i16>)> {
    match entity {
        BlockEntity::Chest(chest) => {
            Some((inventory::MenuKind::Chest, chest.items.clone(), Vec::new()))
        }
        BlockEntity::Furnace(furnace) => Some((
            inventory::MenuKind::Furnace(inventory::FurnaceMenuKind::from_block_kind(
                furnace.kind,
            )?),
            furnace.items.clone(),
            furnace.menu_data().to_vec(),
        )),
        BlockEntity::BrewingStand(stand) => Some((
            inventory::MenuKind::BrewingStand,
            stand.state.slots().to_vec(),
            stand.menu_data().to_vec(),
        )),
        BlockEntity::Hopper(_) | BlockEntity::Sign(_) | BlockEntity::Spawner(_) => None,
        BlockEntity::Unknown(_) => None,
    }
}

/// Open `position` in the session's menu slot and write the open/content/data
/// packets. Returns false when there is no supported container block entity.
fn open_menu_from_entity(
    stream: &mut LoginStream,
    menus: &mut inventory::SessionMenus,
    session: &PlayerSession,
    position: BlockEntityPos,
    entity: &BlockEntity,
) -> io::Result<bool> {
    let Some((kind, container, data)) = menu_contents(entity) else {
        return Ok(false);
    };
    let packets = menus
        .open_menu(
            kind,
            position,
            container,
            &data,
            &session.inventory,
            &session.cursor,
        )
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cannot open menu at ({}, {}, {}): {error}",
                    position.x, position.y, position.z
                ),
            )
        })?;
    write_packet(stream, &packets.open_screen)?;
    write_packet(stream, &packets.content)?;
    for packet in &packets.data {
        write_packet(stream, packet)?;
    }
    Ok(true)
}

fn open_menu_session(
    stream: &mut LoginStream,
    menus: &mut inventory::SessionMenus,
    session: &PlayerSession,
    storage: &Mutex<WorldStorage>,
    position: BlockEntityPos,
) -> io::Result<bool> {
    let entity = crate::world::lock(storage).block_entity(position)?.cloned();
    let Some(entity) = entity else {
        return Ok(false);
    };
    open_menu_from_entity(stream, menus, session, position, &entity)
}

/// Persist a menu's container slots through the WorldStorage block-entity
/// APIs. The chest keeps its loot fields and the brewing stand keeps its brew
/// timers because only the item list is replaced.
pub fn persist_menu_container(
    storage: &mut WorldStorage,
    menu: &inventory::OpenMenu,
) -> io::Result<bool> {
    let Some(mut entity) = storage.remove_block_entity(menu.pos)? else {
        return Ok(false);
    };
    let applied = entity.set_container_items(&menu.container);
    storage.insert_block_entity(entity)?;
    if applied && let Some(chunk) = storage.load_chunk(menu.pos.chunk_x(), menu.pos.chunk_z())? {
        // `update_chunk` preserves top-level fields this port does not decode
        // and writes the current position-keyed block-entity map into the
        // region file. The region cache is flushed by the normal terrain
        // interval (or on disconnect/shutdown).
        storage.update_chunk(&chunk, Compression::Zlib, &[])?;
    }
    Ok(applied)
}

fn send_menu_content(
    stream: &mut LoginStream,
    menus: &inventory::SessionMenus,
    session: &PlayerSession,
) -> io::Result<()> {
    let Some(menu) = menus.open() else {
        return Ok(());
    };
    write_packet(
        stream,
        &menu.content_packet(&session.inventory, &session.cursor),
    )
}

fn handle_menu_click(
    stream: &mut LoginStream,
    menus: &mut inventory::SessionMenus,
    session: &mut PlayerSession,
    storage: &Mutex<WorldStorage>,
    click: &inventory::ContainerClick,
) -> io::Result<()> {
    let creative = session.game_mode == 1;
    match menus.click(&mut session.inventory, &mut session.cursor, click, creative) {
        Ok(outcome) => {
            if outcome.changed {
                let position = menus.open().map(|menu| menu.pos);
                let persisted = {
                    let mut storage = crate::world::lock(storage);
                    let Some(menu) = menus.open() else {
                        return Ok(());
                    };
                    persist_menu_container(&mut storage, menu)?
                };
                if !persisted {
                    if let Some(position) = position {
                        eprintln!(
                            "[join] {}: container at ({}, {}, {}) disappeared during a click",
                            session.name, position.x, position.y, position.z
                        );
                    }
                    if let Some(menu) = menus.close() {
                        write_packet(stream, &menu.close_packet())?;
                    }
                    return Ok(());
                }
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                {
                    eprintln!("[join] could not save inventory: {error}");
                }
            }
            if outcome.changed || outcome.state_mismatch {
                send_menu_content(stream, menus, session)?;
            }
            sync_open_menu(stream, menus, session, storage)?;
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "[join] {}: rejected container click (slot {}, type {}): {error}",
                session.name, click.slot, click.click_type
            );
            // Vanilla resynchronises the whole menu after a desync/invalid
            // click instead of silently drifting from the client.
            send_menu_content(stream, menus, session)
        }
    }
}

/// Push block-entity state into the open menu. Closes the menu when its block
/// entity is gone or its type changed.
fn sync_open_menu(
    stream: &mut LoginStream,
    menus: &mut inventory::SessionMenus,
    session: &PlayerSession,
    storage: &Mutex<WorldStorage>,
) -> io::Result<()> {
    let Some(menu) = menus.open().cloned() else {
        return Ok(());
    };
    let entity = crate::world::lock(storage).block_entity(menu.pos)?.cloned();
    let Some(entity) = entity else {
        if let Some(closed) = menus.close() {
            write_packet(stream, &closed.close_packet())?;
        }
        return Ok(());
    };
    let Some((kind, container, data)) = menu_contents(&entity) else {
        if let Some(closed) = menus.close() {
            write_packet(stream, &closed.close_packet())?;
        }
        return Ok(());
    };
    if kind != menu.kind {
        if let Some(closed) = menus.close() {
            write_packet(stream, &closed.close_packet())?;
        }
        return Ok(());
    }
    let (contents_changed, data_packets) = menus
        .sync(container, &data)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if contents_changed {
        send_menu_content(stream, menus, session)?;
    }
    for packet in &data_packets {
        write_packet(stream, packet)?;
    }
    Ok(())
}

/// Close the open menu when its backing block is removed by this session.
fn close_menu_for_block(
    stream: &mut LoginStream,
    menus: &mut inventory::SessionMenus,
    pos: BlockPos,
) -> io::Result<()> {
    if let Some(closed) = menus.on_block_removed(BlockEntityPos::new(pos.x(), pos.y(), pos.z())) {
        write_packet(stream, &closed.close_packet())?;
    }
    Ok(())
}

fn parse_container_close(packet: &[u8]) -> io::Result<i32> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    if id != inventory::SERVERBOUND_CONTAINER_CLOSE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "expected container close id {}, got {id}",
                inventory::SERVERBOUND_CONTAINER_CLOSE
            ),
        ));
    }
    let container_id = reader.read_varint()?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after container close",
        ));
    }
    Ok(container_id)
}

#[allow(clippy::too_many_arguments)]
fn handle_block_action(
    stream: &mut LoginStream,
    storage: &Mutex<WorldStorage>,
    session: &mut PlayerSession,
    menus: &mut inventory::SessionMenus,
    dig: &mut DigState,
    play_started: Instant,
    projectiles: &mut crate::projectile::ProjectileStore,
    drop_entities: &mut DropEntityStore,
    drop_rng: &mut LegacyRandomSource,
    game_tick: u64,
    bow_draw: &mut Option<u64>,
    player_entity: i32,
    tracked_entities: &SessionEntityTracker,
    packet: &[u8],
    id: i32,
) -> io::Result<()> {
    // Adventure and spectator block actions are not modelled yet; survival and
    // creative share the vanilla dig state machine below.
    if matches!(session.game_mode, 2 | 3) {
        return Ok(());
    }
    let storage = &mut *crate::world::lock(storage);
    match id {
        SERVERBOUND_PLAYER_ACTION => {
            let action = crate::block_update::parse_player_action(packet)?;
            if action.action == block_breaking::ACTION_RELEASE_USE_ITEM {
                if let Some(start) = bow_draw.take() {
                    let ticks = game_tick.saturating_sub(start);
                    let power = bow_power_for_ticks(ticks);
                    if power > 0.0
                        && let Some((crate::projectile::ProjectileKind::Arrow, _, _)) =
                            use_item_projectile(session)
                    {
                        if session.game_mode != 1 {
                            let infinity = session
                                .selected_item()
                                .map_or(0, |stack| stack.enchantment_level("minecraft:infinity"));
                            if infinity > 0 {
                                if !has_arrow(session) {
                                    return Ok(());
                                }
                            } else {
                                let Some((slot, stack)) = take_arrow(session) else {
                                    return Ok(());
                                };
                                write_packet(
                                    stream,
                                    &inventory::inventory_packet(slot as i32, &stack),
                                )?;
                                if let Err(error) = dimension::save_playerdata_preserving_dimension(
                                    session,
                                    playerdata_dir(),
                                ) {
                                    eprintln!("[join] could not save bow release: {error}");
                                }
                            }
                        }
                        let direction = look_vector(session);
                        let eye = [session.x, session.y + 1.62, session.z];
                        let velocity = [
                            direction[0] * 3.0 * power,
                            direction[1] * 3.0 * power,
                            direction[2] * 3.0 * power,
                        ];
                        let packets = projectiles.spawn(
                            crate::projectile::ProjectileKind::Arrow,
                            Some(player_entity),
                            eye,
                            velocity,
                        );
                        for projectile_packet in packets {
                            write_packet(stream, &projectile_packet)?;
                        }
                    }
                }
                return Ok(());
            }
            let pos = BlockPos::new(action.x, action.y, action.z);
            let now = mining_tick(play_started);
            match action.action {
                block_breaking::ACTION_START_DESTROY_BLOCK => {
                    // Vanilla resynchronises the old destroy position when the
                    // player starts mining a different block mid-swing.
                    if let Some(previous) = dig.active_target()
                        && previous != pos
                    {
                        resync_block(stream, storage, previous)?;
                    }
                    let Some(state) = block_at(storage, pos)? else {
                        return Ok(());
                    };
                    if block_breaking::is_air(&state) {
                        return Ok(());
                    }
                    let creative = session.game_mode == 1;
                    let held = session.selected_item().cloned();
                    let context = mining_context(session, storage);
                    match dig.start(pos, creative, &state, held.as_ref(), &context, now) {
                        DigStart::Instant => {
                            let packets = finish_break(
                                storage,
                                session,
                                pos,
                                &state,
                                held.as_ref(),
                                drop_entities,
                                drop_rng,
                            )?;
                            for packet in &packets {
                                write_packet(stream, packet)?;
                            }
                            close_menu_for_block(stream, menus, pos)?;
                            if !creative {
                                send_container_content(stream, session)?;
                                save_after_break(session);
                            }
                        }
                        DigStart::Started | DigStart::Rejected => {}
                    }
                }
                block_breaking::ACTION_STOP_DESTROY_BLOCK => {
                    let Some(state) = block_at(storage, pos)? else {
                        return Ok(());
                    };
                    if block_breaking::is_air(&state) {
                        return Ok(());
                    }
                    let held = session.selected_item().cloned();
                    let context = mining_context(session, storage);
                    let had_target =
                        dig.active_target().is_some() || dig.delayed_target().is_some();
                    let creative = session.game_mode == 1;
                    match dig.stop(pos, &state, held.as_ref(), &context, now) {
                        DigStop::Destroyed => {
                            let packets = finish_break(
                                storage,
                                session,
                                pos,
                                &state,
                                held.as_ref(),
                                drop_entities,
                                drop_rng,
                            )?;
                            for packet in &packets {
                                write_packet(stream, packet)?;
                            }
                            close_menu_for_block(stream, menus, pos)?;
                            if !creative {
                                send_container_content(stream, session)?;
                                save_after_break(session);
                            }
                        }
                        DigStop::Mismatch { active } => {
                            resync_block(stream, storage, active)?;
                        }
                        DigStop::Ignored if !had_target => {
                            // A real client always sends START first. Probe
                            // clients (and the captured join fixture) may send
                            // a lone STOP; treat it as a destroy that starts
                            // now, so it still waits out the computed break
                            // time instead of insta-mining.
                            match dig.start(pos, creative, &state, held.as_ref(), &context, now) {
                                DigStart::Instant => {
                                    let packets = finish_break(
                                        storage,
                                        session,
                                        pos,
                                        &state,
                                        held.as_ref(),
                                        drop_entities,
                                        drop_rng,
                                    )?;
                                    for packet in &packets {
                                        write_packet(stream, packet)?;
                                    }
                                    close_menu_for_block(stream, menus, pos)?;
                                    if !creative {
                                        send_container_content(stream, session)?;
                                        save_after_break(session);
                                    }
                                }
                                DigStart::Started => {
                                    if dig.stop(pos, &state, held.as_ref(), &context, now)
                                        == DigStop::Destroyed
                                    {
                                        let packets = finish_break(
                                            storage,
                                            session,
                                            pos,
                                            &state,
                                            held.as_ref(),
                                            drop_entities,
                                            drop_rng,
                                        )?;
                                        for packet in &packets {
                                            write_packet(stream, packet)?;
                                        }
                                        close_menu_for_block(stream, menus, pos)?;
                                        send_container_content(stream, session)?;
                                        save_after_break(session);
                                    }
                                }
                                DigStart::Rejected => {}
                            }
                        }
                        DigStop::Delayed | DigStop::Ignored => {}
                    }
                }
                block_breaking::ACTION_ABORT_DESTROY_BLOCK => {
                    if let Some(previous) = dig.abort(pos) {
                        resync_block(stream, storage, previous)?;
                    }
                }
                _ => {}
            }
        }
        SERVERBOUND_USE_ITEM_ON => {
            let use_item = crate::block_update::parse_use_item_on(packet)?;
            let clicked = BlockEntityPos::new(use_item.x, use_item.y, use_item.z);
            if let Some(entity) = storage.block_entity(clicked)?.cloned()
                && open_menu_from_entity(stream, menus, session, clicked, &entity)?
            {
                return Ok(());
            }
            let Some(held) = session.selected_item().cloned() else {
                return Ok(());
            };
            let Some(block) = inventory::block_for_item(&held.item) else {
                return Ok(());
            };
            let clicked_pos = BlockPos::new(use_item.x, use_item.y, use_item.z);
            let air = BlockState {
                name: "minecraft:air".into(),
                properties: Vec::new(),
            };
            let clicked_state = block_at(storage, clicked_pos)?;
            let clicked_ref = clicked_state.as_ref().unwrap_or(&air);
            let target_is_clicked =
                crate::block_update::is_replaceable_for_item(clicked_ref, &block);
            let target_pos = crate::block_update::placement_target(
                clicked_pos,
                use_item.direction,
                clicked_ref,
                &block,
            );
            let target_current = block_at(storage, target_pos)?;
            let new_state = crate::block_update::state_for_placement_with_yaw(
                clicked_state.as_ref(),
                target_current.as_ref(),
                &block,
                use_item.direction,
                use_item.cursor_y,
                target_is_clicked,
                session.yaw,
            );
            // Vanilla `Level.isUnobstructed`: refuse a placement whose solid
            // block would overlap the player or any session mob. This applies
            // to survival and creative alike; no consumption or save happens.
            let targets = tracked_entities.collision_targets();
            let mut entity_boxes = Vec::with_capacity(targets.len() + 1);
            entity_boxes.push((
                [session.x - 0.3, session.y, session.z - 0.3],
                [session.x + 0.3, session.y + 1.8, session.z + 0.3],
            ));
            for (_, bounds) in targets {
                entity_boxes.push((
                    [bounds.min_x, bounds.min_y, bounds.min_z],
                    [bounds.max_x, bounds.max_y, bounds.max_z],
                ));
            }
            if crate::block_update::placement_obstructed(target_pos, &new_state, &entity_boxes) {
                return Ok(());
            }
            let door_hinge = crate::block_update::door_hinge_from_click(
                session.yaw,
                f64::from(use_item.x) + f64::from(use_item.cursor_x),
                f64::from(use_item.z) + f64::from(use_item.cursor_z),
                f64::from(target_pos.x()),
                f64::from(target_pos.z()),
            );
            if let Some((lower, upper)) =
                crate::block_update::door_states_with_hinge(&new_state, session.yaw, door_hinge)
            {
                let above_pos = target_pos.above();
                let above_state = block_at(storage, above_pos)?;
                let replaceable = |state: Option<&BlockState>| {
                    state.is_none_or(|state| {
                        state.name == "minecraft:air" || crate::block_behaviour::replaceable(state)
                    })
                };
                if !replaceable(target_current.as_ref()) || !replaceable(above_state.as_ref()) {
                    return Ok(());
                }
                if crate::block_update::placement_obstructed(above_pos, &upper, &entity_boxes) {
                    return Ok(());
                }
                if let Some(change) = crate::block_update::set_block(
                    storage,
                    target_pos.x(),
                    target_pos.y(),
                    target_pos.z(),
                    lower,
                )? {
                    crate::block_update::broadcast_change(&change);
                }
                if let Some(change) = crate::block_update::set_block(
                    storage,
                    above_pos.x(),
                    above_pos.y(),
                    above_pos.z(),
                    upper,
                )? {
                    crate::block_update::broadcast_change(&change);
                }
                if session.game_mode != 1 {
                    session.consume_selected();
                }
                send_container_content(stream, session)?;
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                {
                    eprintln!("[join] could not save door inventory: {error}");
                }
                return Ok(());
            }
            let position = BlockEntityPos::new(target_pos.x(), target_pos.y(), target_pos.z());
            // Derive the container entity before `new_state` is moved into
            // `set_block`; sign/spawner and other untyped blocks yield `None`.
            let placed_entity = crate::block_entity::for_placed_block(&new_state.name, position);
            if let Some(change) = crate::block_update::set_block(
                storage,
                target_pos.x(),
                target_pos.y(),
                target_pos.z(),
                new_state,
            )? {
                crate::block_update::broadcast_change(&change);
                if let Some(entity) = placed_entity {
                    storage.insert_block_entity(entity)?;
                    if let Some(chunk) =
                        storage.load_chunk(position.chunk_x(), position.chunk_z())?
                    {
                        // `update_chunk` keeps the cached chunk and top-level
                        // fields while writing the new block-entity map. Do not
                        // use `save_chunk`, which clears the map.
                        storage.update_chunk(&chunk, Compression::Zlib, &[])?;
                    }
                }
                if session.game_mode != 1 {
                    session.consume_selected();
                }
                send_container_content(stream, session)?;
                if let Err(error) =
                    dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
                {
                    eprintln!("[join] could not save inventory: {error}");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Ticks since the play loop started, at vanilla's 20 ticks per second.
fn mining_tick(started: Instant) -> u64 {
    (started.elapsed().as_millis() / 50) as u64
}

fn block_at(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<Option<BlockState>> {
    let chunk_x = pos.x().div_euclid(16);
    let chunk_z = pos.z().div_euclid(16);
    let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? else {
        return Ok(None);
    };
    Ok(chunk
        .block_state(
            pos.x().rem_euclid(16) as usize,
            pos.y(),
            pos.z().rem_euclid(16) as usize,
        )
        .cloned())
}

fn mining_context(session: &PlayerSession, storage: &mut WorldStorage) -> MiningContext {
    let eye = BlockPos::new(
        session.x.floor() as i32,
        (session.y + 1.62).floor() as i32,
        session.z.floor() as i32,
    );
    let eye_in_water = block_at(storage, eye).ok().flatten().is_some_and(|state| {
        matches!(
            crate::block_behaviour::fluid(&state),
            Some("minecraft:water" | "minecraft:flowing_water")
        )
    });
    MiningContext {
        on_ground: session.on_ground,
        eye_in_water,
        // Status effects and the block-break-speed attribute are not modelled
        // on the session yet (TODO P4.x); vanilla defaults are used.
        ..MiningContext::default()
    }
}

/// Finish a block break: clear the world block, broadcast the existing block
/// update, apply tool durability, and capture the drops.
///
/// Vanilla spawns `ItemEntity`s. Drops are spawned into the session drop
/// store and their add/metadata packets are returned for the caller to send.
fn finish_break(
    storage: &mut WorldStorage,
    session: &mut PlayerSession,
    pos: BlockPos,
    state: &BlockState,
    held: Option<&crate::inventory::ItemStack>,
    drop_entities: &mut DropEntityStore,
    drop_rng: &mut LegacyRandomSource,
) -> io::Result<Vec<Vec<u8>>> {
    let creative = session.game_mode == 1;
    let rolled = if creative {
        block_breaking::BlockDrops::empty()
    } else {
        block_breaking::block_drops(state, held, None)
    };

    let air = crate::chunk::BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    };
    let Some(change) = crate::block_update::set_block(storage, pos.x(), pos.y(), pos.z(), air)?
    else {
        return Ok(Vec::new());
    };
    storage.remove_block_entity(BlockEntityPos::new(pos.x(), pos.y(), pos.z()))?;
    crate::block_update::broadcast_change(&change);
    if creative {
        return Ok(Vec::new());
    }

    if let Some(stack) = session
        .inventory
        .get_mut(session.selected_slot.max(0) as usize)
    {
        let wear = block_breaking::apply_block_durability(
            stack,
            false,
            crate::block_behaviour::hardness(state),
        );
        if wear.broke {
            println!("[join] {}'s tool broke", session.name);
        }
    }

    let packets = drop_entities.spawn(pos, &rolled, drop_rng);
    Ok(packets)
}

/// Continue a vanilla `hasDelayedDestroy` target when the client stopped
/// before its local progress reached the server's 0.7 threshold.
#[allow(clippy::too_many_arguments)]
fn tick_delayed_destroy(
    storage: &Mutex<WorldStorage>,
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    menus: &mut inventory::SessionMenus,
    dig: &mut DigState,
    play_started: Instant,
    drop_entities: &mut DropEntityStore,
    drop_rng: &mut LegacyRandomSource,
) -> io::Result<()> {
    let Some(pos) = dig.delayed_target() else {
        return Ok(());
    };
    let storage = &mut *crate::world::lock(storage);
    let Some(state) = block_at(storage, pos)? else {
        dig.cancel_delayed();
        return Ok(());
    };
    if block_breaking::is_air(&state) {
        dig.cancel_delayed();
        return Ok(());
    }
    let held = session.selected_item().cloned();
    let context = mining_context(session, storage);
    if dig.tick_delayed(&state, held.as_ref(), &context, mining_tick(play_started)) {
        let packets = finish_break(
            storage,
            session,
            pos,
            &state,
            held.as_ref(),
            drop_entities,
            drop_rng,
        )?;
        for packet in &packets {
            write_packet(stream, packet)?;
        }
        close_menu_for_block(stream, menus, pos)?;
        send_container_content(stream, session)?;
        save_after_break(session);
    }
    Ok(())
}

fn save_after_break(session: &PlayerSession) {
    if let Err(error) = dimension::save_playerdata_preserving_dimension(session, playerdata_dir()) {
        eprintln!("[join] could not save inventory after a block break: {error}");
    }
}

/// Send the current block state back to the client, clearing a block-damage
/// overlay without introducing a new packet type.
fn resync_block(
    stream: &mut LoginStream,
    storage: &mut WorldStorage,
    pos: BlockPos,
) -> io::Result<()> {
    let Some(state) = block_at(storage, pos)? else {
        return Ok(());
    };
    let state_id = crate::chunk_packet::block_state_id(&state);
    write_packet(
        stream,
        &crate::block_update::block_update_packet(pos.x(), pos.y(), pos.z(), state_id),
    )
}

fn send_container_content(stream: &mut LoginStream, session: &PlayerSession) -> io::Result<()> {
    write_packet(
        stream,
        &inventory::container_content_packet_with_armor(
            &session.inventory,
            &session.armor,
            &session.cursor,
        ),
    )
}

fn handle_set_carried(
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    bow_draw: &mut Option<u64>,
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
        let changed = session.selected_slot != slot;
        session.selected_slot = slot;
        write_packet(stream, &inventory::held_slot_packet(slot))?;
        if let Err(error) =
            dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
        {
            eprintln!("[join] could not save inventory: {error}");
        }
        if changed {
            *bow_draw = None;
        }
        println!("[join] selected hotbar slot {slot}");
    }
    Ok(())
}

fn send_login_success(
    stream: &mut LoginStream,
    uuid: &[u8; 16],
    name: &str,
    properties: &[ProfileProperty],
) -> io::Result<()> {
    let profile = GameProfile {
        uuid: *uuid,
        name: name.to_owned(),
        properties: properties.to_vec(),
    };
    write_packet(stream, &build_login_success(&profile)?)
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
    use crate::player::ChatSigningSession;
    use crate::protocol::{
        LastSeenMessages, LastSeenUpdate, ServerboundChat, build_serverbound_chat,
    };
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
    fn time_set_ticks_accepts_named_and_numeric_values() {
        assert_eq!(time_set_ticks("day"), Some(1_000));
        assert_eq!(time_set_ticks("noon"), Some(6_000));
        assert_eq!(time_set_ticks("night"), Some(13_000));
        assert_eq!(time_set_ticks("midnight"), Some(18_000));
        assert_eq!(time_set_ticks("0"), Some(0));
        assert_eq!(time_set_ticks("12345"), Some(12_345));
        assert_eq!(time_set_ticks("24000"), Some(24_000));
    }

    #[test]
    fn time_set_ticks_rejects_invalid_values() {
        assert_eq!(time_set_ticks(""), None);
        assert_eq!(time_set_ticks("-1"), None);
        assert_eq!(time_set_ticks("-24000"), None);
        assert_eq!(time_set_ticks("abc"), None);
        assert_eq!(time_set_ticks("1.5"), None);
        assert_eq!(time_set_ticks(" "), None);
    }

    #[test]
    fn weather_events_match_vanilla_sequence() {
        assert_eq!(
            weather_events("clear"),
            Some(vec![(2, 0.0), (7, 0.0), (8, 0.0)])
        );
        assert_eq!(
            weather_events("rain"),
            Some(vec![(1, 0.0), (7, 1.0), (8, 0.0)])
        );
        assert_eq!(
            weather_events("thunder"),
            Some(vec![(1, 0.0), (7, 1.0), (8, 1.0)])
        );
    }

    #[test]
    fn weather_events_reject_unknown_kind() {
        assert_eq!(weather_events("sunny"), None);
        assert_eq!(weather_events(""), None);
        assert_eq!(weather_events("RAIN"), None);
    }

    #[test]
    fn written_terrain_reloads_instead_of_regenerating() {
        crate::worldgen::generator::configure(4242).expect("configure generator");
        let directory = temp_region_dir("marked");
        let mut storage = WorldStorage::new(directory.clone());

        let (generated, source) = load_or_generate_chunk(
            &mut storage,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
        assert_eq!(source, ChunkSource::Generated);

        // A world without `level.dat` still trusts terrain this server wrote.
        let (reloaded, source) = load_or_generate_chunk(
            &mut storage,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
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
        let (chunk, _) = load_or_generate_chunk(
            &mut storage,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
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
        let (reloaded, source) = load_or_generate_chunk(
            &mut reopened,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
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
        let (stored, source) = load_or_generate_chunk(
            &mut storage,
            true,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
        assert_eq!(source, ChunkSource::World);
        assert_eq!(stored, flat_chunk(0, 0), "forced reads serve stored chunks");

        let (chunk, source) = load_or_generate_chunk(
            &mut storage,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            4242,
        )
        .unwrap();
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
        let packet =
            join_game_packet(crate::world::Dimension::Overworld, 0, 8, 7).expect("join packet");
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
    fn a_saved_position_below_the_world_is_settled_onto_the_surface() {
        crate::worldgen::generator::configure(99).expect("configure generator");
        let seed = crate::worldgen::generator::configured()
            .expect("generator")
            .seed();
        let directory = temp_region_dir("below-world");
        let mut storage =
            WorldStorage::for_dimension(directory.clone(), crate::world::Dimension::Overworld);
        let (chunk, _) = load_or_generate_chunk(
            &mut storage,
            false,
            crate::world::Dimension::Overworld,
            0,
            0,
            seed,
        )
        .expect("generate spawn chunk");
        let surface = crate::chunk::surface_y(&chunk, 8, 8);
        let storage = Mutex::new(storage);
        let mut session = PlayerSession::new("Void".into(), [0x94; 16], 8.5, -10_628.0, 8.5);
        unstick_player(
            &mut session,
            &storage,
            false,
            crate::world::Dimension::Overworld,
            seed,
        );
        assert_eq!(session.y, f64::from(surface));
        assert!(session.on_ground);
    }

    #[test]
    fn zero_health_join_state_is_restored() {
        let mut session = PlayerSession::new("Revive".into(), [0x95; 16], 0.0, 0.0, 0.0);
        session.health = 0.0;
        session.food_level = 0;
        session.food_saturation = 0.0;
        session.air = 0;
        restore_join_vitals(&mut session);
        assert_eq!(session.health, 20.0);
        assert_eq!(session.food_level, 20);
        assert_eq!(session.food_saturation, 5.0);
        assert_eq!(session.air, 300);
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
        let (guard, receiver) = SessionCommandGuard::register("ConsoleTarget", [0x9a; 16]);
        let worker = std::thread::spawn(move || {
            loop {
                match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(SessionCommand::SetGameMode { mode, reply }) => {
                        assert_eq!(mode, 1);
                        let _ = reply.send(Ok(()));
                        break;
                    }
                    Ok(_) => continue,
                    Err(error) => panic!("unexpected console command: {error:?}"),
                }
            }
        });

        let output = execute_console_command("gamemode creative ConsoleTarget");
        assert_eq!(
            output,
            vec!["set ConsoleTarget's game mode to creative".to_owned()]
        );
        worker.join().unwrap();
        drop(guard);
    }

    #[test]
    fn bridge_commands_reach_the_registered_live_session() {
        let uuid = [0x9b; 16];
        let (guard, receiver) = SessionCommandGuard::register("BridgeQueue", uuid);

        assert!(send_player_chat(uuid, "hello"));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Chat { message }) if message == "hello"
        ));

        assert!(send_player_chat_by_name("bridgequeue", "by name"));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Chat { message }) if message == "by name"
        ));

        assert!(teleport_player(uuid, 1.5, 70.0, -2.5, 90.0, 10.0));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Teleport { x, y, z, yaw, pitch })
                if (x, y, z, yaw, pitch) == (1.5, 70.0, -2.5, 90.0, 10.0)
        ));
        assert!(!teleport_player(uuid, f64::NAN, 0.0, 0.0, 0.0, 0.0));

        assert!(kick_player(uuid, "bye"));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Disconnect { reason }) if reason == "bye"
        ));

        assert!(broadcast_world_time(6_000, 18_000, true) >= 1);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::SetTime {
                game_time: 6_000,
                day_time: 18_000,
                tick_day_time: true
            })
        ));

        assert!(broadcast_game_event(1, 0.5) >= 1);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::GameEvent { event: 1, param }) if param == 0.5
        ));

        assert!(broadcast_sound("block.note_block.pling", 0, 1.0, 2.0, 3.0, 1.0, 1.0, 7) >= 1);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Sound { sound, seed, .. })
                if sound == "minecraft:block.note_block.pling" && seed == 7
        ));

        assert!(broadcast_particle("END_ROD", 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1, false) >= 1);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::Particle { particle, count: 1, .. }) if particle == "END_ROD"
        ));

        let menu_position = BlockEntityPos::new(1, 64, -2);
        assert!(open_menu(uuid, menu_position));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)),
            Ok(SessionCommand::OpenMenu { position }) if position == menu_position
        ));
        assert!(open_menu_at(uuid, 1, 64, -2));

        drop(guard);
        assert!(!send_player_chat(uuid, "after close"));
    }

    #[test]
    fn open_menu_writes_the_open_screen_and_initial_content() {
        use crate::block_entity::ChestBlockEntity;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client_stream = TcpStream::connect(address).unwrap();
        let (server_stream, _) = listener.accept().unwrap();
        let mut server = Connection::new(server_stream);
        let mut client = Connection::new(client_stream);

        let position = BlockEntityPos::new(0, 64, 0);
        let mut chest = ChestBlockEntity::new(position);
        chest.set_slot(0, inventory::ItemStack::new("minecraft:diamond", 3));
        let entity = BlockEntity::Chest(chest);
        let mut menus = inventory::SessionMenus::new();
        let session = PlayerSession::new("MenuTest".into(), [0x22; 16], 0.5, 64.0, 0.5);

        assert!(
            open_menu_from_entity(&mut server, &mut menus, &session, position, &entity).unwrap()
        );
        assert!(menus.is_open());

        let open = read_packet(&mut client).unwrap();
        let mut reader = PacketReader::new(&open);
        assert_eq!(
            reader.read_varint().unwrap(),
            inventory::CLIENTBOUND_OPEN_SCREEN
        );
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(
            reader.read_varint().unwrap(),
            inventory::MENU_TYPE_GENERIC_9X3
        );
        let title = reader.read_nbt_any().unwrap();
        assert_eq!(
            title.get("translate").and_then(Tag::as_str),
            Some("container.chest")
        );

        let content = read_packet(&mut client).unwrap();
        let mut reader = PacketReader::new(&content);
        assert_eq!(
            reader.read_varint().unwrap(),
            inventory::CLIENTBOUND_CONTAINER_SET_CONTENT
        );
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert_eq!(reader.read_varint().unwrap(), 63);
        let first = inventory::read_item_stack(&mut reader).expect("first chest slot");
        assert_eq!(first.item, "minecraft:diamond");
        assert_eq!(first.count, 3);

        // Unknown block-entity types are not guessed into a menu.
        let unknown = BlockEntity::Unknown(crate::block_entity::UnknownBlockEntity {
            pos: position,
            id: "mod:not_a_menu".into(),
            data: Tag::Compound(Vec::new()),
        });
        let mut menus = inventory::SessionMenus::new();
        assert!(
            !open_menu_from_entity(&mut server, &mut menus, &session, position, &unknown).unwrap()
        );
        assert!(!menus.is_open());

        // Serverbound close uses the same play id and a VarInt container id.
        let mut close = PacketWriter::new();
        close
            .write_varint(inventory::SERVERBOUND_CONTAINER_CLOSE)
            .unwrap();
        close.write_varint(7).unwrap();
        assert_eq!(parse_container_close(&close.into_inner()).unwrap(), 7);
    }

    #[test]
    fn system_chat_and_disconnect_packets_use_component_nbt() {
        let packet = system_chat_packet("Hello, bridge").expect("chat packet");
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_SYSTEM_CHAT);
        let tag = reader.read_nbt_any().unwrap();
        assert_eq!(tag.get("text").and_then(Tag::as_str), Some("Hello, bridge"));
        assert!(!reader.read_bool().unwrap(), "system chat is not overlay");
        assert!(reader.is_empty());

        let packet = disconnect_packet("Kicked by a plugin").expect("disconnect packet");
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_DISCONNECT);
        let tag = reader.read_nbt_any().unwrap();
        assert_eq!(
            tag.get("text").and_then(Tag::as_str),
            Some("Kicked by a plugin")
        );
        assert!(reader.is_empty());
    }

    #[test]
    fn unsigned_player_chat_is_broadcast_as_disguised_chat() {
        let mut session = PlayerSession::new("ChatSender".into(), [0x91; 16], 0.0, 64.0, 0.0);
        session.uuid = [0x91; 16];
        let (_guard, receiver) = SessionCommandGuard::register(&session.name, session.uuid);
        let mut remote = None;
        let mut last_seen = LastSeenValidator::new(LAST_SEEN_MESSAGES_WINDOW);
        let update =
            LastSeenUpdate::new(0, 0, LastSeenMessages::empty().compute_checksum()).unwrap();
        let payload = build_serverbound_chat(&ServerboundChat {
            message: "hello world".into(),
            timestamp_ms: 1,
            salt: 7,
            signature: None,
            last_seen: update,
        })
        .unwrap();

        handle_plain_chat(&session, &mut remote, &mut last_seen, &payload).unwrap();
        let (sender_name, message) = next_player_chat(&receiver, [0x91; 16]);
        assert_eq!(sender_name, "ChatSender");
        assert_eq!(message.body.content, "hello world");
        assert!(message.signature.is_none());
    }

    #[test]
    fn signed_player_chat_verifies_the_session_key_and_sequence() {
        let uuid = [0x92; 16];
        let session = PlayerSession::new("SignedSender".into(), uuid, 0.0, 64.0, 0.0);
        let (_guard, receiver) = SessionCommandGuard::register(&session.name, session.uuid);
        let now_ms = epoch_millis();
        let mut signing =
            ChatSigningSession::deterministic_for_tests(uuid, [0x24; 16], [0x5A; 32], now_ms)
                .unwrap();
        let signed = signing.sign("signed hello", 11, now_ms).unwrap();
        let remote = RemoteChatSession::new(
            uuid,
            [0x24; 16],
            signing.public_key().clone(),
            now_ms,
            false,
            None,
        )
        .unwrap();
        let mut remote = Some(remote);
        let mut last_seen = LastSeenValidator::new(LAST_SEEN_MESSAGES_WINDOW);
        let update =
            LastSeenUpdate::new(0, 0, LastSeenMessages::empty().compute_checksum()).unwrap();
        let payload = build_serverbound_chat(&ServerboundChat {
            message: "signed hello".into(),
            timestamp_ms: now_ms,
            salt: 11,
            signature: signed.signature.clone(),
            last_seen: update,
        })
        .unwrap();

        handle_plain_chat(&session, &mut remote, &mut last_seen, &payload).unwrap();
        let (sender_name, message) = next_player_chat(&receiver, uuid);
        assert_eq!(sender_name, "SignedSender");
        assert_eq!(message.sequence, 0);
        assert_eq!(message.body.content, "signed hello");
        assert_eq!(message.signature, signed.signature);

        // A replay reuses sequence 0, so the remote chain rejects it.
        assert!(handle_plain_chat(&session, &mut remote, &mut last_seen, &payload).is_err());
    }

    #[test]
    fn unsent_columns_cover_the_requested_radius() {
        let mut sent = HashSet::new();
        assert!(has_unsent_columns(&sent, 0, 0, 1));
        for dz in -1..=1 {
            for dx in -1..=1 {
                sent.insert((dx, dz));
            }
        }
        assert!(!has_unsent_columns(&sent, 0, 0, 1));
        assert!(has_unsent_columns(&sent, 1, 0, 1));
    }

    #[test]
    fn attack_packet_parses_only_the_attack_action() {
        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_INTERACT).unwrap();
        writer.write_varint(42).unwrap();
        writer.write_varint(1).unwrap();
        writer.write_u8(0).unwrap();
        assert_eq!(parse_attack(&writer.into_inner()), Some(42));

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_INTERACT).unwrap();
        writer.write_varint(42).unwrap();
        writer.write_varint(0).unwrap();
        writer.write_varint(0).unwrap(); // hand
        writer.write_u8(0).unwrap();
        assert_eq!(parse_attack(&writer.into_inner()), None);
    }

    #[test]
    fn attack_knockback_strength_adds_half_per_enchantment_level() {
        let mut session = PlayerSession::new("Attacker".into(), [0x94; 16], 0.0, 64.0, 0.0);
        assert!((attack_knockback_strength(&session) - 0.4).abs() < f64::EPSILON);
        let mut sword = crate::item_components::ItemStack::new("minecraft:diamond_sword", 1);
        sword.set_component(crate::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:knockback".to_string(), 2)]),
        ));
        session.inventory[session.selected_slot.max(0) as usize] = sword;
        assert!((attack_knockback_strength(&session) - 1.4).abs() < f64::EPSILON);
    }

    #[test]
    fn attack_knockback_direction_follows_the_player_facing() {
        let (x, z) = attack_knockback_direction(0.0);
        assert!(x.abs() < 1e-12, "yaw 0 has no x component: {x}");
        assert!((z + 1.0).abs() < 1e-12, "yaw 0 looks south: {z}");
        let (x, z) = attack_knockback_direction(90.0);
        assert!((x - 1.0).abs() < 1e-12, "yaw 90 has sin = 1: {x}");
        assert!(z.abs() < 1e-12, "yaw 90 has no z component: {z}");
    }

    #[test]
    fn held_attack_damage_reads_the_item_prototype() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        assert_eq!(held_attack_damage(&session), 1.0);
        session.inventory[session.selected_slot.max(0) as usize] =
            crate::item_components::ItemStack::new("minecraft:diamond_sword", 1);
        let expected = crate::item_prototypes::prototype("minecraft:diamond_sword")
            .and_then(|prototype| prototype.tool.as_ref())
            .and_then(|tool| tool.attack_damage)
            .expect("diamond sword has attack damage");
        assert_eq!(held_attack_damage(&session), expected);
        assert!(expected > 1.0);
    }

    fn equip_diamond_sword(session: &mut PlayerSession) {
        session.inventory[session.selected_slot.max(0) as usize] =
            crate::item_components::ItemStack::new("minecraft:diamond_sword", 1);
    }

    #[test]
    fn first_swing_after_reset_uses_partial_charge() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        equip_diamond_sword(&mut session);
        let mut cooldown = AttackCooldown::new(4.0);
        let (damage, knockback, critical, _scale) = resolve_player_attack(&session, &mut cooldown);
        assert!((damage - 1.2071006).abs() < 1e-5, "damage {damage}");
        assert!(knockback.abs() < 1e-5, "knockback {knockback}");
        assert!(!critical);
    }

    #[test]
    fn fully_charged_sword_deals_full_damage_and_knockback() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        equip_diamond_sword(&mut session);
        let mut cooldown = AttackCooldown::new(1.6);
        for _ in 0..cooldown.cooldown_ticks() {
            cooldown.tick();
        }
        let (damage, knockback, critical, scale) = resolve_player_attack(&session, &mut cooldown);
        assert!((damage - 6.0).abs() < 1e-5, "damage {damage}");
        assert!((knockback - 0.4).abs() < 1e-5, "knockback {knockback}");
        assert!(!critical);
        assert!(scale > 0.9);
    }

    #[test]
    fn knockback_enchantment_adds_half_per_level() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        let mut sword = crate::item_components::ItemStack::new("minecraft:diamond_sword", 1);
        sword.set_component(crate::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:knockback".to_string(), 2)]),
        ));
        session.inventory[session.selected_slot.max(0) as usize] = sword;
        let mut cooldown = AttackCooldown::new(1.6);
        for _ in 0..cooldown.cooldown_ticks() {
            cooldown.tick();
        }
        let (_damage, knockback, _critical, _scale) =
            resolve_player_attack(&session, &mut cooldown);
        assert!((knockback - 1.4).abs() < 1e-5, "knockback {knockback}");
    }

    #[test]
    fn falling_sprintless_full_charge_is_critical() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        equip_diamond_sword(&mut session);
        session.physics.fall_distance = 1.0;
        session.physics.on_ground = false;
        session.physics.sprinting = false;
        let mut cooldown = AttackCooldown::new(1.6);
        for _ in 0..cooldown.cooldown_ticks() {
            cooldown.tick();
        }
        let (damage, knockback, critical, _scale) = resolve_player_attack(&session, &mut cooldown);
        assert!(critical, "expected critical");
        assert!((damage - 9.0).abs() < 1e-5, "damage {damage}");
        assert!((knockback - 0.4).abs() < 1e-5, "knockback {knockback}");
    }

    #[test]
    fn sprinting_cancels_critical() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        equip_diamond_sword(&mut session);
        session.physics.fall_distance = 1.0;
        session.physics.on_ground = false;
        session.physics.sprinting = true;
        let mut cooldown = AttackCooldown::new(1.6);
        for _ in 0..cooldown.cooldown_ticks() {
            cooldown.tick();
        }
        let (damage, _knockback, critical, _scale) = resolve_player_attack(&session, &mut cooldown);
        assert!(!critical, "sprinting should cancel critical");
        assert!((damage - 6.0).abs() < 1e-5, "damage {damage}");
    }

    #[test]
    fn resolve_resets_the_cooldown() {
        let mut session = PlayerSession::new("Attacker".into(), [0x93; 16], 0.0, 64.0, 0.0);
        equip_diamond_sword(&mut session);
        let mut cooldown = AttackCooldown::new(1.6);
        for _ in 0..cooldown.cooldown_ticks() {
            cooldown.tick();
        }
        let _ = resolve_player_attack(&session, &mut cooldown);
        assert!((cooldown.strength_scale(0.5) - 0.03846154).abs() < 1e-5);
    }

    #[test]
    fn pickup_target_moves_items_into_the_session_inventory() {
        let mut session = PlayerSession::new("Collector".into(), [0x93; 16], 0.0, 64.0, 0.0);
        let leftover = PickupTarget::add_item(
            &mut session,
            crate::item_components::ItemStack::new("minecraft:stone", 5),
        );
        assert!(leftover.is_empty());
        assert!(
            session
                .inventory
                .iter()
                .any(|stack| stack.item == "minecraft:stone" && stack.count == 5)
        );
    }

    #[test]
    fn pickup_target_adds_experience() {
        let mut session = PlayerSession::new("Collector".into(), [0x93; 16], 0.0, 64.0, 0.0);
        PickupTarget::add_experience(&mut session, 7);
        assert_eq!(session.xp_total, 7);
    }

    /// Chat is broadcast to every registered session, so skip commands that
    /// belong to sessions owned by other tests running in parallel.
    fn next_player_chat(
        receiver: &mpsc::Receiver<SessionCommand>,
        sender: [u8; 16],
    ) -> (String, SignedMessage) {
        loop {
            match receiver.try_recv().expect("chat command") {
                SessionCommand::PlayerChat {
                    sender: from,
                    sender_name,
                    message,
                } if from == sender => return (sender_name, *message),
                _ => {}
            }
        }
    }

    #[test]
    fn entity_loot_roll_for_cow_uses_the_shipped_table() {
        let session = PlayerSession::new("Killer".into(), [0xA1; 16], 0.0, 64.0, 0.0);
        let mut rng = folia_random::LegacyRandomSource::new(1);
        let roll = entity_loot_roll(&session, "minecraft:cow", &mut rng);
        assert!(roll.is_some(), "cow entity loot table should exist");
    }

    #[test]
    fn entity_loot_roll_for_unknown_entity_is_none() {
        let session = PlayerSession::new("Killer".into(), [0xA1; 16], 0.0, 64.0, 0.0);
        let mut rng = folia_random::LegacyRandomSource::new(1);
        assert!(entity_loot_roll(&session, "minecraft:not_a_real_mob", &mut rng).is_none());
    }

    #[test]
    fn looting_enchantment_reaches_the_context() {
        let mut session = PlayerSession::new("Killer".into(), [0xA1; 16], 0.0, 64.0, 0.0);
        let mut sword = crate::item_components::ItemStack::new("minecraft:diamond_sword", 1);
        sword.set_component(crate::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:looting".to_string(), 3)]),
        ));
        session.inventory[session.selected_slot.max(0) as usize] = sword;
        let mut rng = folia_random::LegacyRandomSource::new(7);
        let roll = entity_loot_roll(&session, "minecraft:cow", &mut rng);
        assert!(roll.is_some(), "looting roll should still resolve a table");
    }

    #[test]
    fn experience_reward_matches_the_three_session_mobs() {
        let mut rng = folia_random::LegacyRandomSource::new(1);
        assert_eq!(entity_experience_reward("minecraft:zombie", &mut rng), 5);

        let mut cow_rng = folia_random::LegacyRandomSource::new(2);
        let cow = entity_experience_reward("minecraft:cow", &mut cow_rng);
        assert!((1..=3).contains(&cow), "cow xp {cow}");

        let mut pig_rng = folia_random::LegacyRandomSource::new(3);
        let pig = entity_experience_reward("minecraft:pig", &mut pig_rng);
        assert!((1..=3).contains(&pig), "pig xp {pig}");

        let mut unknown_rng = folia_random::LegacyRandomSource::new(4);
        assert_eq!(
            entity_experience_reward("minecraft:not_a_real_mob", &mut unknown_rng),
            0
        );
    }

    #[test]
    fn time_game_event_sound_and_particle_packets_pin_their_wire_layout() {
        let packet = set_time_packet(12_345, 6_000, true);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_SET_TIME);
        assert_eq!(reader.read_i64().unwrap(), 12_345);
        assert_eq!(reader.read_i64().unwrap(), 6_000);
        assert!(reader.read_bool().unwrap());
        assert!(reader.is_empty());

        let packet = game_event_packet(8, 0.75);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_GAME_EVENT);
        assert_eq!(reader.read_u8().unwrap(), 8);
        assert_eq!(reader.read_f32().unwrap(), 0.75);
        assert!(reader.is_empty());

        let packet = sound_packet(
            "minecraft:block.note_block.pling",
            1,
            1.0,
            -2.0,
            3.5,
            0.8,
            1.2,
            42,
        )
        .expect("sound packet");
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_SOUND);
        assert_eq!(reader.read_varint().unwrap(), 0, "direct sound holder");
        assert_eq!(
            reader.read_string().unwrap(),
            "minecraft:block.note_block.pling"
        );
        assert!(!reader.read_bool().unwrap(), "no fixed range");
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_i32().unwrap(), 8);
        assert_eq!(reader.read_i32().unwrap(), -16);
        assert_eq!(reader.read_i32().unwrap(), 28);
        assert_eq!(reader.read_f32().unwrap(), 0.8);
        assert_eq!(reader.read_f32().unwrap(), 1.2);
        assert_eq!(reader.read_i64().unwrap(), 42);
        assert!(reader.is_empty());

        let packet = particle_packet(
            "minecraft:end_rod",
            1.0,
            2.0,
            3.0,
            0.1,
            0.2,
            0.3,
            0.4,
            5,
            true,
        )
        .expect("simple particle packet");
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), PLAY_LEVEL_PARTICLES);
        assert!(
            !reader.read_bool().unwrap(),
            "override limiter defaults off"
        );
        assert!(reader.read_bool().unwrap(), "force maps to always-show");
        assert_eq!(reader.read_f64().unwrap(), 1.0);
        assert_eq!(reader.read_f64().unwrap(), 2.0);
        assert_eq!(reader.read_f64().unwrap(), 3.0);
        assert_eq!(reader.read_f32().unwrap(), 0.1);
        assert_eq!(reader.read_f32().unwrap(), 0.2);
        assert_eq!(reader.read_f32().unwrap(), 0.3);
        assert_eq!(reader.read_f32().unwrap(), 0.4);
        assert_eq!(reader.read_i32().unwrap(), 5);
        assert_eq!(reader.read_varint().unwrap(), 20, "minecraft:end_rod id");
        assert!(reader.is_empty());

        assert!(
            particle_packet(
                "minecraft:dust",
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1,
                false
            )
            .is_none(),
            "option-carrying particles fail loudly"
        );
    }

    #[test]
    fn parse_command_packet_accepts_unsigned_commands() {
        let last_seen = crate::protocol::LastSeenUpdate::new(
            0,
            0,
            crate::protocol::LastSeenMessages::empty().compute_checksum(),
        )
        .unwrap();
        let packet = crate::protocol::build_serverbound_chat_command(
            &crate::protocol::ServerboundChatCommand {
                command: "gamemode creative".to_string(),
                timestamp_ms: 1,
                salt: 2,
                last_seen,
            },
        )
        .unwrap();
        let (command, parsed_last_seen) = parse_command_packet(&packet).unwrap();
        assert_eq!(command, "gamemode creative");
        assert_eq!(parsed_last_seen.offset, 0);
        assert_eq!(parsed_last_seen.acknowledged, 0);
        assert_eq!(
            parsed_last_seen.checksum,
            crate::protocol::LastSeenMessages::empty().compute_checksum()
        );
    }

    #[test]
    fn parse_command_packet_accepts_signed_commands() {
        let last_seen = crate::protocol::LastSeenUpdate::new(
            0,
            0,
            crate::protocol::LastSeenMessages::empty().compute_checksum(),
        )
        .unwrap();
        let packet = crate::protocol::build_serverbound_chat_command_signed(
            &crate::protocol::ServerboundChatCommandSigned {
                command: "msg".to_string(),
                timestamp_ms: 7,
                salt: 9,
                argument_signatures: vec![
                    crate::protocol::SignedCommandArgument {
                        name: "target".to_string(),
                        signature: crate::protocol::MessageSignature::from_slice(
                            &[1u8; crate::protocol::MESSAGE_SIGNATURE_BYTES],
                        )
                        .unwrap(),
                    },
                    crate::protocol::SignedCommandArgument {
                        name: "text".to_string(),
                        signature: crate::protocol::MessageSignature::from_slice(
                            &[4u8; crate::protocol::MESSAGE_SIGNATURE_BYTES],
                        )
                        .unwrap(),
                    },
                ],
                last_seen,
            },
        )
        .unwrap();
        let (command, _parsed_last_seen) = parse_command_packet(&packet).unwrap();
        assert_eq!(command, "msg");
    }

    #[test]
    fn parse_command_packet_rejects_other_packet_ids() {
        let mut writer = PacketWriter::new();
        writer.write_varint(8).unwrap();
        let error = parse_command_packet(&writer.into_inner()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn command_packet_codec_rejects_missing_last_seen() {
        let mut writer = PacketWriter::new();
        writer
            .write_varint(crate::protocol::SERVERBOUND_CHAT_COMMAND)
            .unwrap();
        writer.write_string("op Steve").unwrap();
        let error = parse_command_packet(&writer.into_inner()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn player_equipment_maps_the_armor_array() {
        let mut session = PlayerSession::new("Equip".to_string(), [0xE1; 16], 0.0, 64.0, 0.0);
        session.inventory[session.selected_slot.max(0) as usize] =
            crate::inventory::ItemStack::empty();
        session.armor[0] =
            crate::inventory::ItemStack::new("minecraft:diamond_boots".to_string(), 1);
        session.armor[1] =
            crate::inventory::ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
        session.armor[2] =
            crate::inventory::ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
        session.armor[3] =
            crate::inventory::ItemStack::new("minecraft:diamond_helmet".to_string(), 1);

        let equipment = player_equipment(&session);
        assert_eq!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::Feet)
                .item,
            "minecraft:diamond_boots"
        );
        assert_eq!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::Legs)
                .item,
            "minecraft:diamond_leggings"
        );
        assert_eq!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::Chest)
                .item,
            "minecraft:diamond_chestplate"
        );
        assert_eq!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::Head)
                .item,
            "minecraft:diamond_helmet"
        );
        assert!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::MainHand)
                .is_empty(),
            "empty selected slot maps to empty main hand"
        );

        session.inventory[session.selected_slot.max(0) as usize] =
            crate::inventory::ItemStack::new("minecraft:diamond_sword".to_string(), 1);
        let equipment = player_equipment(&session);
        assert_eq!(
            equipment
                .get(crate::entity_metadata::EquipmentSlot::MainHand)
                .item,
            "minecraft:diamond_sword"
        );
    }

    #[test]
    fn initial_experience_packet_uses_session_values() {
        let mut session = PlayerSession::new("XpJoin".to_string(), [0xE2; 16], 0.0, 64.0, 0.0);
        session.xp_progress = 0.5;
        session.xp_level = 4;
        session.xp_total = 37;
        let packet = crate::entity_metadata::set_experience_packet(
            session.xp_progress,
            session.xp_level,
            session.xp_total,
        );
        let (progress, level, total) =
            crate::entity_metadata::read_set_experience(&packet).unwrap();
        assert!((progress - 0.5).abs() < 1e-6);
        assert_eq!(level, 4);
        assert_eq!(total, 37);
    }

    #[test]
    fn death_drops_take_inventory_armor_and_cursor() {
        let mut session = PlayerSession::new("DeathDrops".to_string(), [0xD1; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:stone".to_string(), 5);
        session.armor[2] =
            crate::inventory::ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
        session.cursor = crate::inventory::ItemStack::new("minecraft:dirt".to_string(), 3);

        let drops = take_player_death_drops(&mut session);
        assert_eq!(drops.len(), 3);
        assert_eq!(drops[0].item, "minecraft:stone");
        assert_eq!(drops[0].count, 5);
        assert_eq!(drops[1].item, "minecraft:diamond_chestplate");
        assert_eq!(drops[1].count, 1);
        assert_eq!(drops[2].item, "minecraft:dirt");
        assert_eq!(drops[2].count, 3);

        assert!(session.inventory[0].is_empty());
        assert!(session.armor[2].is_empty());
        assert!(session.cursor.is_empty());
        assert!(session.inventory.iter().all(|stack| stack.is_empty()));
        assert!(session.armor.iter().all(|stack| stack.is_empty()));
    }

    #[test]
    fn death_drops_skip_empty_slots() {
        let mut session = PlayerSession::new("NoDrops".to_string(), [0xD2; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        let drops = take_player_death_drops(&mut session);
        assert!(drops.is_empty());
        assert!(session.inventory.iter().all(|stack| stack.is_empty()));
        assert!(session.armor.iter().all(|stack| stack.is_empty()));
        assert!(session.cursor.is_empty());
    }
    #[test]
    fn death_experience_is_seven_per_level_capped_at_100() {
        let mut session = PlayerSession::new("DeathXp".to_string(), [0xD3; 16], 0.0, 64.0, 0.0);
        session.xp_level = 3;
        session.xp_progress = 0.5;
        session.xp_total = 21;
        assert_eq!(take_player_death_experience(&mut session), 21);
        assert_eq!(session.xp_level, 0);
        assert!(session.xp_progress.abs() < 1e-6);
        assert_eq!(session.xp_total, 0);

        session.xp_level = 20;
        session.xp_progress = 0.25;
        session.xp_total = 400;
        assert_eq!(take_player_death_experience(&mut session), 100);
        assert_eq!(session.xp_level, 0);
        assert!(session.xp_progress.abs() < 1e-6);
        assert_eq!(session.xp_total, 0);

        session.xp_level = 0;
        session.xp_progress = 0.0;
        session.xp_total = 0;
        assert_eq!(take_player_death_experience(&mut session), 0);
        assert_eq!(session.xp_level, 0);
        assert!(session.xp_progress.abs() < 1e-6);
        assert_eq!(session.xp_total, 0);
    }

    #[test]
    fn xp_add_updates_level_and_builds_experience_packet() {
        let mut session = PlayerSession::new("XpAdd".to_string(), [0xD4; 16], 0.0, 64.0, 0.0);
        session.xp_level = 0;
        session.xp_progress = 0.0;
        session.xp_total = 0;

        session.add_experience(9);
        assert_eq!(session.xp_level, 1);
        assert_eq!(session.xp_total, 9);
        let expected_progress = 2.0 / PlayerSession::xp_needed_for_next_level(1) as f32;
        assert!((session.xp_progress - expected_progress).abs() < 1e-5);

        let packet = crate::entity_metadata::set_experience_packet(
            session.xp_progress,
            session.xp_level,
            session.xp_total,
        );
        let (progress, level, total) =
            crate::entity_metadata::read_set_experience(&packet).unwrap();
        assert!((progress - session.xp_progress).abs() < 1e-6);
        assert_eq!(level, 1);
        assert_eq!(total, 9);
    }

    #[test]
    fn bow_use_spawns_arrow_speed_three() {
        let mut session = PlayerSession::new("Bow".to_string(), [0xB1; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        session.selected_slot = 0;
        session.yaw = 0.0;
        session.pitch = 0.0;
        let (kind, velocity, consume) = use_item_projectile(&session).expect("projectile");
        assert_eq!(kind, crate::projectile::ProjectileKind::Arrow);
        assert!((velocity[0]).abs() < 1e-6);
        assert!((velocity[1]).abs() < 1e-6);
        assert!((velocity[2] - 3.0).abs() < 1e-6);
        assert!(!consume);
    }

    #[test]
    fn snowball_use_consumes_one_item() {
        let mut session = PlayerSession::new("Snow".to_string(), [0xB2; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] =
            crate::inventory::ItemStack::new("minecraft:snowball".to_string(), 2);
        session.selected_slot = 0;
        session.yaw = 0.0;
        session.pitch = 0.0;
        let (kind, velocity, consume) = use_item_projectile(&session).expect("projectile");
        assert_eq!(kind, crate::projectile::ProjectileKind::Snowball);
        assert!((velocity[0]).abs() < 1e-6);
        assert!((velocity[1]).abs() < 1e-6);
        assert!((velocity[2] - 1.5).abs() < 1e-6);
        assert!(consume);
    }

    #[test]
    fn unusable_item_has_no_projectile() {
        let mut session = PlayerSession::new("Stone".to_string(), [0xB3; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:stone".to_string(), 1);
        session.selected_slot = 0;
        assert!(use_item_projectile(&session).is_none());
    }

    #[test]
    fn yaw_90_throws_west() {
        let mut session = PlayerSession::new("West".to_string(), [0xB4; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        session.selected_slot = 0;
        session.yaw = 90.0;
        session.pitch = 0.0;
        let (kind, velocity, consume) = use_item_projectile(&session).expect("projectile");
        assert_eq!(kind, crate::projectile::ProjectileKind::Arrow);
        assert!((velocity[0] + 3.0).abs() < 1e-6);
        assert!(velocity[1].abs() < 1e-6);
        assert!(velocity[2].abs() < 1e-6);
        assert!(!consume);
    }

    #[test]
    fn entity_death_rewards_for_cow_has_items_and_xp() {
        let session = PlayerSession::new("Killer".to_string(), [0xE1; 16], 0.0, 64.0, 0.0);
        let mut saw_items = false;
        let mut saw_xp = false;
        let mut saw_both = false;
        for seed in 0i64..64 {
            let mut rng = folia_random::LegacyRandomSource::new(seed);
            let (stacks, xp) = entity_death_rewards(&session, "minecraft:cow", &mut rng);
            if !stacks.is_empty() {
                saw_items = true;
            }
            if (1..=3).contains(&xp) {
                saw_xp = true;
            }
            if !stacks.is_empty() && (1..=3).contains(&xp) {
                saw_both = true;
                break;
            }
        }
        assert!(saw_items, "expected some cow seed to yield item stacks");
        assert!(saw_xp, "expected cow xp in 1..=3");
        assert!(saw_both, "expected some seed to yield both items and xp");
    }

    #[test]
    fn entity_death_rewards_for_unknown_is_empty() {
        let session = PlayerSession::new("Killer".to_string(), [0xE2; 16], 0.0, 64.0, 0.0);
        let mut rng = folia_random::LegacyRandomSource::new(1);
        let (stacks, xp) = entity_death_rewards(&session, "minecraft:not_a_real_mob", &mut rng);
        assert!(stacks.is_empty());
        assert_eq!(xp, 0);
    }

    #[test]
    fn take_arrow_consumes_one_from_the_first_stack() {
        let mut session = PlayerSession::new("Arrow".to_string(), [0xA1; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[4] = crate::inventory::ItemStack::new("minecraft:arrow".to_string(), 3);
        session.inventory[5] = crate::inventory::ItemStack::new("minecraft:arrow".to_string(), 1);

        let (slot, stack) = take_arrow(&mut session).expect("arrow");
        assert_eq!(slot, 4);
        assert_eq!(stack.count, 2);

        let (slot, stack) = take_arrow(&mut session).expect("arrow");
        assert_eq!(slot, 4);
        assert_eq!(stack.count, 1);

        let (slot, stack) = take_arrow(&mut session).expect("arrow");
        assert_eq!(slot, 4);
        assert!(stack.is_empty());
        assert_eq!(session.inventory[5].count, 1);
    }

    #[test]
    fn take_arrow_accepts_tipped_and_spectral() {
        let mut session = PlayerSession::new("Tipped".to_string(), [0xA2; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[2] =
            crate::inventory::ItemStack::new("minecraft:tipped_arrow".to_string(), 1);
        assert!(take_arrow(&mut session).is_some());

        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[7] =
            crate::inventory::ItemStack::new("minecraft:spectral_arrow".to_string(), 2);
        let (slot, stack) = take_arrow(&mut session).expect("spectral arrow");
        assert_eq!(slot, 7);
        assert_eq!(stack.count, 1);
    }

    #[test]
    fn take_arrow_returns_none_without_arrows() {
        let mut session = PlayerSession::new("NoArrow".to_string(), [0xA3; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        assert!(take_arrow(&mut session).is_none());
    }

    #[test]
    fn bow_without_arrows_does_not_spawn() {
        let mut session = PlayerSession::new("NoAmmo".to_string(), [0xA4; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        session.selected_slot = 0;
        session.game_mode = 0;
        assert_eq!(
            use_item_projectile(&session).map(|(kind, _, _)| kind),
            Some(crate::projectile::ProjectileKind::Arrow)
        );
        assert!(take_arrow(&mut session).is_none());
    }

    #[test]
    fn mob_motion_packet_after_knockback() {
        let mut tracker = SessionEntityTracker::deterministic((0.5, 63.0, 0.5), 63, 7);
        let zombie = tracker
            .tracked_ids()
            .into_iter()
            .find(|id| {
                tracker.entity(*id).is_some_and(|tracked| {
                    tracked
                        .ai
                        .as_ref()
                        .is_some_and(|agent| agent.mob.kind == crate::entity_ai::ActorKind::Zombie)
                })
            })
            .expect("session tracker spawns a zombie");
        let result = tracker
            .damage_with_knockback(zombie, 1.0, 0.4, 1.0, 0.0)
            .expect("zombie takes damage");
        assert!(result.damaged);
        let packet = mob_motion_packet(&tracker, zombie).expect("knockback sets velocity");
        let (entity_id, velocity) =
            crate::protocol::read_set_entity_motion(&packet).expect("parse motion");
        assert_eq!(entity_id, zombie);
        assert!(
            velocity != [0.0; 3],
            "knockback should yield non-zero velocity"
        );
    }

    #[test]
    fn mob_motion_packet_is_none_without_velocity() {
        let tracker = SessionEntityTracker::deterministic((0.5, 63.0, 0.5), 63, 7);
        let zombie = tracker
            .tracked_ids()
            .into_iter()
            .find(|id| {
                tracker.entity(*id).is_some_and(|tracked| {
                    tracked
                        .ai
                        .as_ref()
                        .is_some_and(|agent| agent.mob.kind == crate::entity_ai::ActorKind::Zombie)
                })
            })
            .expect("session tracker spawns a zombie");
        assert!(mob_motion_packet(&tracker, zombie).is_none());
    }

    #[test]
    fn has_arrow_detects_regular_tipped_and_spectral() {
        let mut session = PlayerSession::new("Arrows".to_string(), [0xB0; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        assert!(!has_arrow(&session));

        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:arrow".to_string(), 1);
        assert!(has_arrow(&session));

        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[1] =
            crate::inventory::ItemStack::new("minecraft:tipped_arrow".to_string(), 1);
        assert!(has_arrow(&session));

        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[2] =
            crate::inventory::ItemStack::new("minecraft:spectral_arrow".to_string(), 1);
        assert!(has_arrow(&session));

        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        assert!(!has_arrow(&session));
    }

    #[test]
    fn infinity_bow_does_not_consume_arrows() {
        let mut session = PlayerSession::new("InfBow".to_string(), [0xB1; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        let mut bow = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        bow.set_component(crate::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:infinity".to_string(), 1)]),
        ));
        session.inventory[0] = bow;
        session.inventory[9] = crate::inventory::ItemStack::new("minecraft:arrow".to_string(), 2);
        session.selected_slot = 0;
        session.game_mode = 0;

        let infinity = session
            .selected_item()
            .map_or(0, |stack| stack.enchantment_level("minecraft:infinity"));
        assert_eq!(infinity, 1);
        assert!(has_arrow(&session));
        assert_eq!(
            use_item_projectile(&session).map(|(kind, _, _)| kind),
            Some(crate::projectile::ProjectileKind::Arrow)
        );
        // Infinity path: no consumption/inventory update, so arrow count remains.
        assert_eq!(session.inventory[9].count, 2);
    }

    #[test]
    fn infinity_bow_without_arrows_cannot_fire() {
        let mut session = PlayerSession::new("NoInfAmmo".to_string(), [0xB2; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        let mut bow = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        bow.set_component(crate::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:infinity".to_string(), 1)]),
        ));
        session.inventory[0] = bow;
        session.selected_slot = 0;
        session.game_mode = 0;

        let infinity = session
            .selected_item()
            .map_or(0, |stack| stack.enchantment_level("minecraft:infinity"));
        assert_eq!(infinity, 1);
        assert!(!has_arrow(&session));
    }

    #[test]
    fn apply_player_input_tracks_sprint_and_sneak() {
        let mut session = PlayerSession::new("Input".to_string(), [0xB3; 16], 0.0, 64.0, 0.0);
        apply_player_input(&mut session, 0);
        assert!(!session.physics.sprinting);
        assert!(!session.physics.sneaking);

        apply_player_input(&mut session, crate::protocol::PLAYER_INPUT_SPRINT);
        assert!(session.physics.sprinting);
        assert!(!session.physics.sneaking);

        apply_player_input(&mut session, crate::protocol::PLAYER_INPUT_SNEAK);
        assert!(!session.physics.sprinting);
        assert!(session.physics.sneaking);

        apply_player_input(
            &mut session,
            crate::protocol::PLAYER_INPUT_SPRINT | crate::protocol::PLAYER_INPUT_SNEAK,
        );
        assert!(session.physics.sprinting);
        assert!(session.physics.sneaking);
    }

    #[test]
    fn vitals_sprint_burns_food() {
        let mut session = PlayerSession::new("Sprint".to_string(), [0xC0; 16], 0.0, 64.0, 0.0);
        session.health = 20.0;
        session.food_level = 20;
        session.food_saturation = 0.0;
        session.physics.sprinting = true;
        for tick in 1u64..=50 {
            let _ = tick_player_vitals(&mut session, tick, false);
        }
        assert!(
            session.food_level < 20,
            "food should decrease from sprint exhaustion"
        );
    }

    #[test]
    fn vitals_food_heal_at_full_saturation() {
        let mut session = PlayerSession::new("Heal".to_string(), [0xC1; 16], 0.0, 64.0, 0.0);
        session.health = 10.0;
        session.food_level = 20;
        session.food_saturation = 5.0;
        let (food, _) = tick_player_vitals(&mut session, 10, false);
        assert_eq!(food, crate::player::FoodTick::Heal);
        assert!((session.health - 11.0).abs() < f32::EPSILON);
    }

    #[test]
    fn vitals_starvation_at_zero_food() {
        let mut session = PlayerSession::new("Starve".to_string(), [0xC2; 16], 0.0, 64.0, 0.0);
        session.health = 10.0;
        session.food_level = 0;
        session.food_saturation = 0.0;
        let (food, _) = tick_player_vitals(&mut session, 80, false);
        assert_eq!(food, crate::player::FoodTick::Starve);
        assert!((session.health - 9.0).abs() < f32::EPSILON);
    }

    #[test]
    fn vitals_air_drains_underwater_and_refills() {
        let mut session = PlayerSession::new("Air".to_string(), [0xC3; 16], 0.0, 64.0, 0.0);
        session.air = 300;
        let mut last_air = crate::player::AirTick::Nothing;
        for tick in 1u64..=320 {
            let (_, air) = tick_player_vitals(&mut session, tick, true);
            last_air = air;
        }
        assert_eq!(last_air, crate::player::AirTick::Drowning);
        assert_eq!(session.air, 0);

        let (_, air) = tick_player_vitals(&mut session, 321, false);
        assert_eq!(air, crate::player::AirTick::Refilled);
        assert!(session.air > 0, "air should refill out of water");
    }

    #[test]
    fn vitals_death_message_prefers_drowning_then_starvation() {
        assert_eq!(
            vitals_death_message(
                crate::player::FoodTick::Nothing,
                crate::player::AirTick::Drowning
            ),
            Some("Player drowned")
        );
        assert_eq!(
            vitals_death_message(
                crate::player::FoodTick::Starve,
                crate::player::AirTick::Nothing
            ),
            Some("Player starved to death")
        );
        assert_eq!(
            vitals_death_message(
                crate::player::FoodTick::Starve,
                crate::player::AirTick::Drowning
            ),
            Some("Player drowned")
        );
        assert_eq!(
            vitals_death_message(
                crate::player::FoodTick::Nothing,
                crate::player::AirTick::Nothing
            ),
            None
        );
    }

    #[test]
    fn bow_power_curve_matches_vanilla() {
        assert!((bow_power_for_ticks(0) - 0.0).abs() < 1e-12);
        assert!((bow_power_for_ticks(10) - (5.0 / 12.0)).abs() < 1e-12);
        assert!((bow_power_for_ticks(20) - 1.0).abs() < 1e-12);
        assert!((bow_power_for_ticks(40) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bow_full_draw_reaches_full_speed() {
        let mut session = PlayerSession::new("Bow".to_string(), [0xB4; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        session.selected_slot = 0;
        session.yaw = 90.0;
        session.pitch = 0.0;
        let (kind, _, consume) = use_item_projectile(&session).expect("projectile");
        assert_eq!(kind, crate::projectile::ProjectileKind::Arrow);
        assert!(!consume);
        let direction = look_vector(&session);
        let power = bow_power_for_ticks(20);
        let velocity = [
            direction[0] * 3.0 * power,
            direction[1] * 3.0 * power,
            direction[2] * 3.0 * power,
        ];
        let magnitude =
            (velocity[0] * velocity[0] + velocity[1] * velocity[1] + velocity[2] * velocity[2])
                .sqrt();
        assert!((magnitude - 3.0).abs() < 1e-6);
    }

    #[test]
    fn bow_partial_draw_scales_velocity() {
        let mut session = PlayerSession::new("Bow".to_string(), [0xB5; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:bow".to_string(), 1);
        session.selected_slot = 0;
        session.yaw = 90.0;
        session.pitch = 0.0;
        let (kind, _, _) = use_item_projectile(&session).expect("projectile");
        assert_eq!(kind, crate::projectile::ProjectileKind::Arrow);
        let direction = look_vector(&session);
        let power = bow_power_for_ticks(10);
        let velocity = [
            direction[0] * 3.0 * power,
            direction[1] * 3.0 * power,
            direction[2] * 3.0 * power,
        ];
        assert!((velocity[0] + 3.0 * (5.0 / 12.0)).abs() < 1e-6);
        assert!(velocity[1].abs() < 1e-6);
        assert!(velocity[2].abs() < 1e-6);
    }

    #[test]
    fn teleport_coords_parse_three_numbers() {
        let mut parts = "1 64 -3.5".split_whitespace();
        assert_eq!(parse_teleport_coords(&mut parts), Some((1.0, 64.0, -3.5)));
    }

    #[test]
    fn teleport_coords_reject_missing_or_invalid() {
        let mut parts = "1 64".split_whitespace();
        assert_eq!(parse_teleport_coords(&mut parts), None);

        let mut parts = "1 x 3".split_whitespace();
        assert_eq!(parse_teleport_coords(&mut parts), None);

        let mut parts = "NaN 64 0".split_whitespace();
        assert_eq!(parse_teleport_coords(&mut parts), None);
    }

    #[test]
    fn parse_give_accepts_bare_and_namespaced_names() {
        let mut parts = "diamond".split_whitespace();
        assert_eq!(
            parse_give(&mut parts),
            Some(("minecraft:diamond".to_string(), 1))
        );

        let mut parts = "minecraft:apple 16".split_whitespace();
        assert_eq!(
            parse_give(&mut parts),
            Some(("minecraft:apple".to_string(), 16))
        );
    }

    #[test]
    fn parse_give_rejects_unknown_item_and_bad_count() {
        let mut parts = "not_an_item".split_whitespace();
        assert_eq!(parse_give(&mut parts), None);

        let mut parts = "diamond 0".split_whitespace();
        assert_eq!(parse_give(&mut parts), None);

        let mut parts = "diamond 65".split_whitespace();
        assert_eq!(parse_give(&mut parts), None);

        let mut parts = "diamond x".split_whitespace();
        assert_eq!(parse_give(&mut parts), None);
    }

    #[test]
    fn give_stack_applies_prototype_defaults() {
        let mut stack = crate::inventory::ItemStack::new("minecraft:diamond_sword".to_string(), 1);
        crate::item_prototypes::apply_defaults(&mut stack);
        assert!(stack.max_damage() > 0);
        assert_eq!(stack.max_stack_size(), 1);
    }

    #[test]
    fn enchant_selected_item_adds_component() {
        let mut session = PlayerSession::new("Enchant".to_string(), [0xD0; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] =
            crate::inventory::ItemStack::new("minecraft:diamond_sword".to_string(), 1);
        crate::item_prototypes::apply_defaults(&mut session.inventory[0]);
        session.selected_slot = 0;

        assert!(apply_enchantment_to_selected(&mut session, "sharpness", 3).is_ok());
        let enchantments = match session.inventory[0].get_component("minecraft:enchantments") {
            Some(crate::item_components::ComponentValue::Enchantments(map)) => map,
            other => panic!("expected enchantments component, got {other:?}"),
        };
        assert_eq!(enchantments.get("minecraft:sharpness"), Some(&3));
    }

    #[test]
    fn enchant_rejects_unknown_and_out_of_range() {
        let mut session = PlayerSession::new("Enchant".to_string(), [0xD1; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.inventory[0] =
            crate::inventory::ItemStack::new("minecraft:diamond_sword".to_string(), 1);
        session.selected_slot = 0;

        assert!(apply_enchantment_to_selected(&mut session, "not_an_enchant", 1).is_err());
        assert!(apply_enchantment_to_selected(&mut session, "sharpness", 0).is_err());
        assert!(apply_enchantment_to_selected(&mut session, "sharpness", 6).is_err());
    }

    #[test]
    fn enchant_rejects_empty_selected_item() {
        let mut session = PlayerSession::new("Enchant".to_string(), [0xD2; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        session.selected_slot = 0;
        assert!(apply_enchantment_to_selected(&mut session, "sharpness", 1).is_err());
    }

    #[test]
    fn clear_player_items_clears_inventory_armor_and_cursor() {
        let mut session = PlayerSession::new("Clear".to_string(), [0xD3; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        for slot in &mut session.armor {
            *slot = crate::inventory::ItemStack::empty();
        }
        session.cursor = crate::inventory::ItemStack::empty();
        session.inventory[0] = crate::inventory::ItemStack::new("minecraft:diamond".to_string(), 3);
        session.armor[2] =
            crate::inventory::ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
        session.cursor = crate::inventory::ItemStack::new("minecraft:apple".to_string(), 5);

        assert!(clear_player_items(&mut session));
        assert!(session.inventory.iter().all(|stack| stack.is_empty()));
        assert!(session.armor.iter().all(|stack| stack.is_empty()));
        assert!(session.cursor.is_empty());
    }

    #[test]
    fn clear_player_items_returns_false_when_empty() {
        let mut session = PlayerSession::new("Empty".to_string(), [0xD4; 16], 0.0, 64.0, 0.0);
        for stack in &mut session.inventory {
            *stack = crate::inventory::ItemStack::empty();
        }
        for slot in &mut session.armor {
            *slot = crate::inventory::ItemStack::empty();
        }
        session.cursor = crate::inventory::ItemStack::empty();
        assert!(!clear_player_items(&mut session));
    }

    #[test]
    fn seed_command_formats_the_generator_seed() {
        assert_eq!(seed_message(123), "Seed: [123]");
    }

    #[test]
    fn hazards_lava_ticks_every_ten() {
        let mut hazards = PlayerHazards::default();
        assert_eq!(
            hazards.tick(true, false, false),
            Some((
                4.0,
                crate::effects::DamageKind::Fire,
                "Player tried to swim in lava"
            ))
        );
        for _ in 0..9 {
            assert_eq!(hazards.tick(true, false, false), None);
        }
        assert_eq!(
            hazards.tick(true, false, false),
            Some((
                4.0,
                crate::effects::DamageKind::Fire,
                "Player tried to swim in lava"
            ))
        );
    }

    #[test]
    fn hazards_fire_ticks_every_twenty() {
        let mut hazards = PlayerHazards::default();
        assert_eq!(
            hazards.tick(false, true, false),
            Some((
                1.0,
                crate::effects::DamageKind::Fire,
                "Player burned to death"
            ))
        );
        for _ in 0..19 {
            assert_eq!(hazards.tick(false, true, false), None);
        }
        assert_eq!(
            hazards.tick(false, true, false),
            Some((
                1.0,
                crate::effects::DamageKind::Fire,
                "Player burned to death"
            ))
        );
    }

    #[test]
    fn hazards_cactus_ticks_every_ten() {
        let mut hazards = PlayerHazards::default();
        assert_eq!(
            hazards.tick(false, false, true),
            Some((
                1.0,
                crate::effects::DamageKind::Physical,
                "Player was pricked to death"
            ))
        );
        for _ in 0..9 {
            assert_eq!(hazards.tick(false, false, true), None);
        }
        assert_eq!(
            hazards.tick(false, false, true),
            Some((
                1.0,
                crate::effects::DamageKind::Physical,
                "Player was pricked to death"
            ))
        );
    }

    #[test]
    fn hazards_no_flags_return_none() {
        let mut hazards = PlayerHazards::default();
        for _ in 0..25 {
            assert_eq!(hazards.tick(false, false, false), None);
        }
    }

    #[test]
    fn hazards_precedence_lava_over_fire_and_cactus() {
        let mut hazards = PlayerHazards::default();
        assert_eq!(
            hazards.tick(true, true, true),
            Some((
                4.0,
                crate::effects::DamageKind::Fire,
                "Player tried to swim in lava"
            ))
        );
    }

    #[test]
    fn sweeping_requires_full_charge_sword_and_no_sprint() {
        let sword = crate::inventory::ItemStack::new("minecraft:iron_sword", 1);
        let stone = crate::inventory::ItemStack::new("minecraft:stone", 1);
        assert!(!is_sweeping_attack(0.9, false, false, Some(&sword)));
        assert!(is_sweeping_attack(0.91, false, false, Some(&sword)));
        assert!(!is_sweeping_attack(0.91, true, false, Some(&sword)));
        assert!(!is_sweeping_attack(0.91, false, true, Some(&sword)));
        assert!(!is_sweeping_attack(0.91, false, false, Some(&stone)));
        assert!(!is_sweeping_attack(0.91, false, false, None));
    }

    #[test]
    fn kill_command_parser_accepts_self_only() {
        let mut parts = "".split_whitespace();
        assert!(kill_targets_self(&mut parts, "Steve"));

        let mut parts = "Steve".split_whitespace();
        assert!(kill_targets_self(&mut parts, "steve"));

        let mut parts = "Alex".split_whitespace();
        assert!(!kill_targets_self(&mut parts, "Steve"));
    }

    #[test]
    fn lethal_void_damage_marks_player_dead() {
        let entity_id = 9_001_042;
        let mut session = PlayerSession::new("KillCmd".into(), [0x4b; 16], 0.5, 63.0, 0.5);
        crate::entity_metadata::living_entities().entry_player(entity_id);
        let mut rng = folia_random::LegacyRandomSource::new(7);
        let result = damage_player_with_rng(
            &mut session,
            entity_id,
            f32::MAX,
            &mut rng,
            crate::effects::DamageKind::Void,
        );
        assert!(result.died);
        assert_eq!(session.health, 0.0);
        crate::entity_metadata::living_entities().remove(entity_id);
    }

    #[test]
    fn effect_give_parses_defaults_and_explicit_values() {
        let mut parts = "speed".split_whitespace();
        assert_eq!(
            parse_effect_give(&mut parts),
            Some((crate::effects::StatusEffectKind::Speed, 600, 0))
        );

        let mut parts = "minecraft:regeneration 5 2".split_whitespace();
        assert_eq!(
            parse_effect_give(&mut parts),
            Some((crate::effects::StatusEffectKind::Regeneration, 100, 2))
        );
    }

    #[test]
    fn effect_give_rejects_unknown_and_bad_values() {
        let mut parts = "not_an_effect".split_whitespace();
        assert_eq!(parse_effect_give(&mut parts), None);

        let mut parts = "speed 0".split_whitespace();
        assert_eq!(parse_effect_give(&mut parts), None);

        let mut parts = "speed 1000001".split_whitespace();
        assert_eq!(parse_effect_give(&mut parts), None);

        let mut parts = "speed 30 256".split_whitespace();
        assert_eq!(parse_effect_give(&mut parts), None);
    }

    #[test]
    fn effect_give_applies_to_the_player_store() {
        let entity_id = 9_001_001;
        let result = {
            let mut store = crate::entity_metadata::living_entities();
            let player = store.entry_player(entity_id);
            player.apply_effect(crate::effects::StatusEffect::new(
                crate::effects::StatusEffectKind::Speed,
                100,
                1,
            ))
        };
        assert!(!result.packets.is_empty());
        let mut reader = crate::protocol::PacketReader::new(&result.packets[0]);
        assert_eq!(
            reader.read_varint().unwrap(),
            crate::effects::CLIENTBOUND_UPDATE_MOB_EFFECT
        );
        let mut store = crate::entity_metadata::living_entities();
        let player = store.entry_player(entity_id);
        assert!(player.effects.has(crate::effects::StatusEffectKind::Speed));
        let _ = store.remove(entity_id);
    }

    #[test]
    fn parse_summon_accepts_bare_and_coords() {
        let mut parts = "cow".split_whitespace();
        assert_eq!(
            parse_summon(&mut parts),
            Some(("minecraft:cow".to_string(), None))
        );

        let mut parts = "minecraft:zombie 1 64 -3".split_whitespace();
        assert_eq!(
            parse_summon(&mut parts),
            Some(("minecraft:zombie".to_string(), Some((1.0, 64.0, -3.0))))
        );
    }

    #[test]
    fn parse_summon_rejects_unknown_and_partial_coords() {
        let mut parts = "not_a_mob".split_whitespace();
        assert_eq!(parse_summon(&mut parts), None);

        let mut parts = "cow 1 2".split_whitespace();
        assert_eq!(parse_summon(&mut parts), None);

        let mut parts = "cow 1".split_whitespace();
        assert_eq!(parse_summon(&mut parts), None);
    }

    #[test]
    fn spawn_snapshot_has_surface_and_blocks() {
        use crate::entity_ai::WorldView;
        let root = std::env::temp_dir().join(format!(
            "folia-rs-spawn-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let chunk = crate::chunk_packet::flat_chunk(0, 0);
        let mut storage = crate::world::WorldStorage::new(root.join("region"));
        storage
            .save_chunk(&chunk, crate::anvil::Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        let snapshot = spawn_snapshot(&mut storage, crate::spawn::ChunkPos::new(0, 0))
            .expect("snapshot for flat chunk");
        let surface = snapshot.surface_y(0, 0).expect("surface for chunk column");
        assert!((snapshot.min_y()..snapshot.max_y()).contains(&surface));
        let has_block = (snapshot.min_y()..snapshot.max_y()).any(|y| {
            snapshot
                .block(crate::math::BlockPos::new(0, y, 0))
                .is_some()
        });
        assert!(has_block, "expected copied blocks in snapshot band");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn spawn_cycle_respects_the_mob_cap() {
        let root = std::env::temp_dir().join(format!(
            "folia-rs-spawn-cap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let chunk = crate::chunk_packet::flat_chunk(0, 0);
        let mut storage = crate::world::WorldStorage::new(root.join("region"));
        storage
            .save_chunk(&chunk, crate::anvil::Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        let mut tracker =
            crate::entity::SessionEntityTracker::deterministic((0.5, 63.0, 0.5), 63, 7);
        // Pad tracker to cap with summoned mobs.
        for i in 0..9 {
            tracker.spawn_mob("minecraft:cow", (10.0 + i as f64, 63.0, 0.5), i as i64);
        }
        assert_eq!(tracker.len(), 12);
        let mut spawner =
            crate::spawn::NaturalSpawner::new(289, crate::spawn::SpawnSettings::default());
        let mut rng = folia_random::LegacyRandomSource::new(1);
        let packets = run_session_spawn_cycle(
            &mut spawner,
            &mut storage,
            &mut tracker,
            crate::spawn::ChunkPos::new(0, 0),
            (0.5, 63.0, 0.5),
            &mut rng,
            12,
        );
        assert!(packets.is_empty());
        assert_eq!(tracker.len(), 12);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn spawn_cycle_can_add_a_mob() {
        let root = std::env::temp_dir().join(format!(
            "folia-rs-spawn-add-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let chunk = crate::chunk_packet::flat_chunk(0, 0);
        let mut storage = crate::world::WorldStorage::new(root.join("region"));
        storage
            .save_chunk(&chunk, crate::anvil::Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        let mut added = false;
        let mut saw_packets = false;
        for seed in 0i64..64 {
            let mut spawner =
                crate::spawn::NaturalSpawner::new(289, crate::spawn::SpawnSettings::default());
            let mut tracker =
                crate::entity::SessionEntityTracker::deterministic((8.5, 63.0, 8.5), 63, seed);
            let before = tracker.len();
            let mut rng = folia_random::LegacyRandomSource::new(seed);
            let packets = run_session_spawn_cycle(
                &mut spawner,
                &mut storage,
                &mut tracker,
                crate::spawn::ChunkPos::new(0, 0),
                (8.5, 63.0, 8.5),
                &mut rng,
                12,
            );
            if tracker.len() > before {
                added = true;
                if !packets.is_empty() && packet_id(&packets[0]).ok() == Some(1) {
                    saw_packets = true;
                    break;
                }
            }
        }
        assert!(
            added,
            "expected at least one seed to add a naturally spawned mob on a flat chunk"
        );
        assert!(saw_packets, "expected add-entity packets for a spawned mob");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn spawn_egg_entity_maps_supported_mobs() {
        assert_eq!(
            spawn_egg_entity("minecraft:cow_spawn_egg"),
            Some("minecraft:cow")
        );
        assert_eq!(
            spawn_egg_entity("minecraft:pig_spawn_egg"),
            Some("minecraft:pig")
        );
        assert_eq!(
            spawn_egg_entity("minecraft:zombie_spawn_egg"),
            Some("minecraft:zombie")
        );
        assert_eq!(
            spawn_egg_entity("minecraft:sheep_spawn_egg"),
            Some("minecraft:sheep")
        );
        assert_eq!(spawn_egg_entity("minecraft:stone"), None);
        assert_eq!(spawn_egg_entity("minecraft:not_a_mob_spawn_egg"), None);
    }

    #[test]
    fn spawn_egg_entity_rejects_malformed() {
        assert_eq!(spawn_egg_entity("cow_spawn_egg"), Some("minecraft:cow"));
        assert_eq!(spawn_egg_entity("_spawn_egg"), None);
        assert_eq!(spawn_egg_entity("spawn_egg"), None);
        assert_eq!(spawn_egg_entity("minecraft:cow_spawn_egg_extra"), None);
        assert_eq!(spawn_egg_entity("other:cow_spawn_egg"), None);
    }

    #[test]
    fn spawn_egg_enter_packets_include_add_and_metadata() {
        let mut tracker =
            crate::entity::SessionEntityTracker::deterministic((0.5, 63.0, 0.5), 63, 7);
        let id = tracker
            .spawn_mob("minecraft:cow", (4.5, 63.0, 8.5), 0x5A17)
            .expect("cow should spawn");
        let packets = tracker.enter_packets_for(id);
        assert!(
            packets
                .iter()
                .any(|packet| packet_id(packet).ok() == Some(1)),
            "expected an Add Entity packet"
        );
        assert!(
            packets
                .iter()
                .any(|packet| packet_id(packet).ok() == Some(97)),
            "expected an entity metadata packet (id 97)"
        );
    }
}
