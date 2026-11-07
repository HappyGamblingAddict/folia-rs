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
use crate::crypto::{self, CryptoError, RsaKeyPair, constant_time_eq, server_hash};
use crate::dimension::{self, PortalKind, PortalRequest, SessionDimensionState};
use crate::entity::{SessionEntityTracker, WorldCollider};
use crate::inventory;
use crate::math::BlockPos;
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
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex, OnceLock};
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
    )?;
    let chunk_count = streamed.streamed;
    let counts = streamed.counts;
    for packet in tracked_entities.retain_in_chunks(&sent_chunks) {
        write_packet(&mut stream, &packet)?;
    }

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
    let mut dig = DigState::default();
    let mut menus = inventory::SessionMenus::new();
    let mut remote_chat: Option<RemoteChatSession> = None;
    let mut last_seen = LastSeenValidator::new(LAST_SEEN_MESSAGES_WINDOW);
    let mut next_chat_index = 0i32;
    let (_console_guard, console_commands) =
        SessionCommandGuard::register(&session.name, session.uuid);

    loop {
        if shutdown_requested() {
            tracked_entities.clear();
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
            let tick = {
                let mut storage = crate::world::lock(current_storage);
                let mut collider = WorldCollider::new(&mut storage);
                tracked_entities.tick((session.x, session.y, session.z), &mut collider)
            };
            for packet in &tick.packets {
                if let Err(error) = write_packet(&mut stream, packet) {
                    if is_disconnect(&error) {
                        tracked_entities.clear();
                        finish_disconnected(&session, current_storage, &start.name, &error);
                        return Ok(());
                    }
                    return Err(error);
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
                        )?;
                        for packet in tracked_entities.retain_in_chunks(&sent_chunks) {
                            write_packet(&mut stream, &packet)?;
                        }
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
                for packet in tracked_entities.retain_in_chunks(&sent_chunks) {
                    write_packet(&mut stream, &packet)?;
                }
            }
        } else if apply_void_damage(&mut stream, &mut session, &mut last_void_damage)? {
            awaiting_respawn = true;
            death_at = Some(Instant::now());
            if let Err(error) =
                dimension::save_playerdata_preserving_dimension(&session, playerdata_dir())
            {
                eprintln!("[join] could not save void death: {error}");
            }
            println!("[join] {} died in the void", start.name);
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
                            for packet in tracked_entities.retain_in_chunks(&sent_chunks) {
                                write_packet(&mut stream, &packet)?;
                            }
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
                        if let Err(error) = handle_command(&mut stream, &mut session, &packet) {
                            eprintln!("[join] command failed: {error}");
                        }
                    }
                    SERVERBOUND_PLAYER_ACTION | SERVERBOUND_USE_ITEM_ON => {
                        if id == SERVERBOUND_USE_ITEM_ON {
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
                                // Player inventory menu (container id 0) keeps
                                // the pre-menu behaviour byte-for-byte.
                                inventory::apply_click(
                                    &mut session.inventory,
                                    &mut session.cursor,
                                    click,
                                );
                                send_container_content(&mut stream, &session)?;
                                if let Err(error) = dimension::save_playerdata_preserving_dimension(
                                    &session,
                                    playerdata_dir(),
                                ) {
                                    eprintln!("[join] could not save inventory: {error}");
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
                    _ => {
                        let outcome = {
                            let storage = &mut *crate::world::lock(current_storage);
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
                            for packet in tracked_entities.retain_in_chunks(&sent_chunks) {
                                write_packet(&mut stream, &packet)?;
                            }
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

fn handle_command(
    stream: &mut LoginStream,
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

/// Apply damage through the living-entity model so the client receives the
/// same metadata health update and `ClientboundSetHealth` body as vanilla.
pub fn damage_player(session: &mut PlayerSession, entity_id: i32, amount: f32) -> PlayerDamage {
    let mut store = crate::entity_metadata::living_entities();
    let player = store.entry_player(entity_id);
    player.food = session.food_level.clamp(0, 20);
    player.saturation = session.food_saturation.max(0.0);
    if (player.health - session.health).abs() > f32::EPSILON {
        player.set_health(session.health);
        player.metadata.pack_dirty();
    }
    let result = player.hurt(amount);
    session.health = player.health;
    PlayerDamage {
        damaged: result.damaged,
        died: result.died,
        packets: result.packets,
    }
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
) -> io::Result<bool> {
    if fall_damage_health(session.health, damage, session.game_mode).is_none() {
        return Ok(false);
    }
    let result = damage_player(session, player_entity_id(), damage);
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
    last_damage: &mut Instant,
) -> io::Result<bool> {
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
    let result = damage_player(session, entity_id, VOID_DAMAGE);
    if !result.damaged {
        return Ok(false);
    }
    for packet in &result.packets {
        write_packet(stream, packet)?;
    }
    if result.died {
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
    for packet in tracked.retain_in_chunks(sent_chunks) {
        write_packet(stream, &packet)?;
    }
    println!(
        "[join] {} switched {:?} -> {:?} at ({:.1}, {:.1}, {:.1}); streamed {} chunks",
        session.name, trigger.from, trigger.to, session.x, session.y, session.z, batch.streamed
    );
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
    generator: Option<&crate::worldgen::generator::WorldGenerator>,
    dimension: crate::world::Dimension,
    seed: i64,
    pending: &[(usize, i32, i32)],
    loaded: &mut [Option<(Chunk, ChunkSource)>],
    batch: &mut ChunkBatch,
) -> io::Result<()> {
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

    let next = AtomicUsize::new(0);
    let next = &next;
    let workers = generation_workers(pending.len());
    std::thread::scope(|scope| -> io::Result<()> {
        // The receiver is dropped as soon as this closure returns, even on an
        // early shutdown return, so workers blocked on a full queue see the
        // disconnect and can exit before `thread::scope` joins them.
        let (sender, receiver) = std::sync::mpsc::sync_channel(GENERATION_QUEUE);
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
                    let result = match overworld_generator {
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
    // cold batch uses more than one core. Non-overworld dimensions always
    // generate because the configured overworld generator does not apply.
    let generated = dimension != crate::world::Dimension::Overworld
        || crate::worldgen::generator::configured().is_some();
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
                            finish_break(storage, session, pos, &state, held.as_ref())?;
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
                            finish_break(storage, session, pos, &state, held.as_ref())?;
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
                                    finish_break(storage, session, pos, &state, held.as_ref())?;
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
                                        finish_break(storage, session, pos, &state, held.as_ref())?;
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
/// Vanilla spawns `ItemEntity`s. This port has no item entities yet, so drops
/// are inserted into the player's inventory; anything that does not fit is
/// logged instead of silently vanishing.
fn finish_break(
    storage: &mut WorldStorage,
    session: &mut PlayerSession,
    pos: BlockPos,
    state: &BlockState,
    held: Option<&crate::inventory::ItemStack>,
) -> io::Result<()> {
    let creative = session.game_mode == 1;
    let drops = if creative {
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
        return Ok(());
    };
    storage.remove_block_entity(BlockEntityPos::new(pos.x(), pos.y(), pos.z()))?;
    crate::block_update::broadcast_change(&change);
    if creative {
        return Ok(());
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

    for drop in drops.items {
        let leftover = inventory::add_stack(&mut session.inventory, drop);
        if !leftover.is_empty() {
            eprintln!(
                "[join] {}: inventory full, could not collect {} x{} (item entities are not modelled)",
                session.name, leftover.item, leftover.count
            );
        }
    }
    // `drops.experience` is returned for a future experience-orb path; orbs do
    // not exist in this port yet, so no XP is granted here.
    Ok(())
}

/// Continue a vanilla `hasDelayedDestroy` target when the client stopped
/// before its local progress reached the server's 0.7 threshold.
fn tick_delayed_destroy(
    storage: &Mutex<WorldStorage>,
    stream: &mut LoginStream,
    session: &mut PlayerSession,
    menus: &mut inventory::SessionMenus,
    dig: &mut DigState,
    play_started: Instant,
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
        finish_break(storage, session, pos, &state, held.as_ref())?;
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
        &inventory::container_content_packet(&session.inventory, &session.cursor),
    )
}

fn handle_set_carried(
    stream: &mut LoginStream,
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
        if let Err(error) =
            dimension::save_playerdata_preserving_dimension(session, playerdata_dir())
        {
            eprintln!("[join] could not save inventory: {error}");
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
}
