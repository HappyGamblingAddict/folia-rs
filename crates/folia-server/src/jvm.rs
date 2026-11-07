use anyhow::{Context, Result, bail};
use folia_core::chunk::{self, BlockState};
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::nbt::Tag;
use folia_core::player::PlayerSession;
use folia_core::world::{self, WorldStorage};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jdouble, jfloat, jint, jlong, jstring};
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM, NativeMethod};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{LazyLock, Mutex};

const INVENTORY_SLOTS: usize = 36;
const HOTBAR_SLOTS: i32 = 9;

struct Task {
    id: i32,
    plugin: String,
    runnable: GlobalRef,
    delay: i64,
    period: i64,
    cancelled: bool,
}

// SAFETY: GlobalRef is a JVM global reference. It remains valid across threads;
// all calls that use it happen on the JVM-attached server thread.
unsafe impl Send for Task {}

static NEXT_TASK_ID: AtomicI32 = AtomicI32::new(1);
static NEXT_ATTACHMENT_ID: AtomicI32 = AtomicI32::new(1);
static TASKS: LazyLock<Mutex<Vec<Task>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static BRIDGE: LazyLock<Mutex<Option<BridgeState>>> = LazyLock::new(|| Mutex::new(None));

extern "system" fn native_ping(mut env: JNIEnv, _class: JClass, input: JString) -> jstring {
    let message: String = match env.get_string(&input) {
        Ok(value) => value.into(),
        Err(_) => "<invalid jstring>".to_owned(),
    };
    println!("[rust] nativePing received: {message}");
    let response = format!("Rust core received: {message}");
    match env.new_string(response) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

extern "system" fn native_schedule_task(
    mut env: JNIEnv,
    _class: JClass,
    plugin: JString,
    runnable: JObject,
    delay: jlong,
    period: jlong,
) -> jint {
    let plugin_name: String = match env.get_string(&plugin) {
        Ok(value) => value.into(),
        Err(_) => "<unknown plugin>".to_owned(),
    };
    let runnable = match env.new_global_ref(&runnable) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[rust] could not pin plugin task: {error}");
            return -1;
        }
    };
    let id = NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst);
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tasks.push(Task {
        id,
        plugin: plugin_name.clone(),
        runnable,
        delay: delay.max(0),
        period: period.max(0),
        cancelled: false,
    });
    println!("[rust] scheduled task #{id} for {plugin_name} (delay={delay}, period={period})");
    id
}

extern "system" fn native_cancel_task(_env: JNIEnv, _class: JClass, id: jint) {
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for task in tasks.iter_mut() {
        if task.id == id {
            task.cancelled = true;
        }
    }
    tasks.retain(|task| !task.cancelled);
}

extern "system" fn native_cancel_tasks(mut env: JNIEnv, _class: JClass, plugin: JString) {
    let plugin_name: String = match env.get_string(&plugin) {
        Ok(value) => value.into(),
        Err(_) => return,
    };
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for task in tasks.iter_mut() {
        if task.plugin == plugin_name {
            task.cancelled = true;
        }
    }
    tasks.retain(|task| !task.cancelled);
}

fn tick_tasks(env: &mut JNIEnv) {
    let mut due = Vec::new();
    {
        let mut tasks = TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for task in tasks.iter_mut() {
            if task.cancelled {
                continue;
            }
            if task.delay > 0 {
                task.delay -= 1;
                continue;
            }
            due.push((task.id, task.runnable.clone()));
            if task.period > 0 {
                task.delay = task.period;
            } else {
                task.cancelled = true;
            }
        }
        tasks.retain(|task| !task.cancelled);
    }

    for (id, runnable) in due {
        if let Err(error) = env.call_method(runnable.as_obj(), "run", "()V", &[]) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            eprintln!("[rust] scheduled task #{id} threw: {error}");
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge state
// ---------------------------------------------------------------------------

/// The bridge-owned subset of world state exposed through the Bukkit `World`
/// API. It is persisted in `run/world_state.txt` instead of `level.dat` so a
/// plugin edit survives until the native server writes the level again.
#[derive(Debug, Clone, PartialEq)]
struct WorldState {
    full_time: i64,
    spawn: (f64, f64, f64),
    storm: bool,
    thundering: bool,
    weather_duration: i32,
    thunder_duration: i32,
    clear_weather_duration: i32,
}

impl Default for WorldState {
    fn default() -> Self {
        Self {
            full_time: 0,
            spawn: (0.5, 64.0, 0.5),
            storm: false,
            thundering: false,
            weather_duration: 0,
            thunder_duration: 0,
            clear_weather_duration: 0,
        }
    }
}

impl WorldState {
    fn load(path: &Path) -> Self {
        let mut state = Self::default();
        let Ok(contents) = fs::read_to_string(path) else {
            return state;
        };
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "time" => {
                    if let Ok(time) = value.parse() {
                        state.full_time = time;
                    }
                }
                "storm" => state.storm = value == "1" || value.eq_ignore_ascii_case("true"),
                "thundering" => {
                    state.thundering = value == "1" || value.eq_ignore_ascii_case("true");
                }
                "weather_duration" => {
                    if let Ok(duration) = value.parse() {
                        state.weather_duration = duration;
                    }
                }
                "thunder_duration" => {
                    if let Ok(duration) = value.parse() {
                        state.thunder_duration = duration;
                    }
                }
                "clear_weather_duration" => {
                    if let Ok(duration) = value.parse() {
                        state.clear_weather_duration = duration;
                    }
                }
                "spawn_x" => {
                    if let Ok(x) = value.parse() {
                        state.spawn.0 = x;
                    }
                }
                "spawn_y" => {
                    if let Ok(y) = value.parse() {
                        state.spawn.1 = y;
                    }
                }
                "spawn_z" => {
                    if let Ok(z) = value.parse() {
                        state.spawn.2 = z;
                    }
                }
                _ => {}
            }
        }
        state
    }

    fn load_or_level(path: &Path, level_path: &Path) -> Self {
        if path.exists() {
            return Self::load(path);
        }
        let mut state = Self::default();
        if let Ok(level) = folia_core::level::read(level_path) {
            state.full_time = level.day_time;
            if let Some((x, y, z)) = level.spawn {
                state.spawn = (f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
            }
        }
        state
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = format!(
            "time={}\nspawn_x={}\nspawn_y={}\nspawn_z={}\n\
             storm={}\nthundering={}\nweather_duration={}\n\
             thunder_duration={}\nclear_weather_duration={}\n",
            self.full_time,
            self.spawn.0,
            self.spawn.1,
            self.spawn.2,
            u8::from(self.storm),
            u8::from(self.thundering),
            self.weather_duration,
            self.thunder_duration,
            self.clear_weather_duration,
        );
        fs::write(path, contents)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PermissionAttachmentState {
    id: i32,
    plugin: String,
    permissions: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PermissionState {
    attachments: Vec<PermissionAttachmentState>,
}

struct BridgeState {
    world_root: PathBuf,
    data_root: PathBuf,
    playerdata_dir: PathBuf,
    players: HashMap<[u8; 16], PlayerSession>,
    permissions: HashMap<[u8; 16], PermissionState>,
    world_state: WorldState,
}

impl BridgeState {
    fn new(root: &Path) -> Self {
        let properties = folia_core::config::ServerProperties::read(root.join("server.properties"))
            .unwrap_or_else(|_| folia_core::config::ServerProperties::default());
        let level_name =
            std::env::var("FOLIA_LEVEL_NAME").unwrap_or_else(|_| properties.level_name.clone());
        let world_root = root.join(&level_name);
        let data_root = root.join("run");
        let world_state = WorldState::load_or_level(
            &data_root.join("world_state.txt"),
            &world_root.join("level.dat"),
        );
        Self {
            world_root,
            data_root,
            playerdata_dir: root.join("run").join("playerdata"),
            players: HashMap::new(),
            permissions: HashMap::new(),
            world_state,
        }
    }

    fn spawn(&self) -> ((f64, f64, f64), i32) {
        let level = folia_core::level::read(self.world_root.join("level.dat")).ok();
        let game_mode = level
            .as_ref()
            .map_or(0, |level| level.game_type.clamp(0, 3));
        (self.world_state.spawn, game_mode)
    }

    fn world_state_path(&self) -> PathBuf {
        self.data_root.join("world_state.txt")
    }

    fn save_world_state(&self) {
        if let Err(error) = self.world_state.save(&self.world_state_path()) {
            eprintln!("[bridge] could not persist world state: {error}");
        }
    }

    fn plugin_config_path(&self, plugin: &str) -> PathBuf {
        self.data_root
            .join(safe_plugin_dir(plugin))
            .join("config.yml")
    }

    fn read_plugin_config(&self, plugin: &str) -> String {
        fs::read_to_string(self.plugin_config_path(plugin)).unwrap_or_default()
    }

    fn write_plugin_config(&self, plugin: &str, contents: &str) -> bool {
        let path = self.plugin_config_path(plugin);
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!("[bridge] could not create plugin config directory: {error}");
            return false;
        }
        match fs::write(&path, contents) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("[bridge] could not write {}: {error}", path.display());
                false
            }
        }
    }

    fn permission_state(&mut self, uuid: [u8; 16]) -> &mut PermissionState {
        self.permissions.entry(uuid).or_default()
    }

    fn add_permission_attachment(&mut self, uuid: [u8; 16], plugin: &str) -> i32 {
        let id = NEXT_ATTACHMENT_ID.fetch_add(1, Ordering::SeqCst);
        self.permission_state(uuid)
            .attachments
            .push(PermissionAttachmentState {
                id,
                plugin: plugin.to_owned(),
                permissions: BTreeMap::new(),
            });
        id
    }

    fn remove_permission_attachment(&mut self, uuid: [u8; 16], id: i32) -> bool {
        let Some(state) = self.permissions.get_mut(&uuid) else {
            return false;
        };
        let before = state.attachments.len();
        state.attachments.retain(|attachment| attachment.id != id);
        before != state.attachments.len()
    }

    fn set_permission(
        &mut self,
        uuid: [u8; 16],
        attachment: i32,
        permission: &str,
        value: bool,
    ) -> bool {
        let permission = normalize_permission(permission);
        let Some(state) = self.permissions.get_mut(&uuid) else {
            return false;
        };
        let Some(entry) = state
            .attachments
            .iter_mut()
            .find(|entry| entry.id == attachment)
        else {
            return false;
        };
        entry.permissions.insert(permission, value);
        true
    }

    fn unset_permission(&mut self, uuid: [u8; 16], attachment: i32, permission: &str) -> bool {
        let permission = normalize_permission(permission);
        let Some(state) = self.permissions.get_mut(&uuid) else {
            return false;
        };
        let Some(entry) = state
            .attachments
            .iter_mut()
            .find(|entry| entry.id == attachment)
        else {
            return false;
        };
        entry.permissions.remove(&permission).is_some()
    }

    fn effective_permissions(&mut self, uuid: [u8; 16]) -> BTreeMap<String, bool> {
        let mut effective = BTreeMap::new();
        if let Some(state) = self.permissions.get(&uuid) {
            for attachment in &state.attachments {
                for (permission, value) in &attachment.permissions {
                    effective.insert(permission.clone(), *value);
                }
            }
        }
        effective
    }

    fn permission_is_set(&mut self, uuid: [u8; 16], permission: &str) -> bool {
        self.effective_permissions(uuid)
            .contains_key(&normalize_permission(permission))
    }

    fn permission_has(&mut self, uuid: [u8; 16], permission: &str, is_op: bool) -> bool {
        self.effective_permissions(uuid)
            .get(&normalize_permission(permission))
            .copied()
            .unwrap_or(is_op)
    }

    fn player_is_op(&mut self, uuid: [u8; 16]) -> bool {
        let name = self.player(uuid).name.clone();
        folia_core::join::is_operator(&name)
    }

    fn player_set_op(&mut self, uuid: [u8; 16], value: bool) -> bool {
        let name = self.player(uuid).name.clone();
        let command = if value {
            format!("op {name}")
        } else {
            format!("deop {name}")
        };
        for line in folia_core::join::execute_console_command(&command) {
            println!("[bridge] {line}");
        }
        folia_core::join::is_operator(&name) == value
    }

    fn ensure_player(&mut self, name: Option<&str>, uuid: [u8; 16]) -> &mut PlayerSession {
        if !self.players.contains_key(&uuid) {
            let loaded = match PlayerSession::load(&self.playerdata_dir, uuid) {
                Ok(loaded) => loaded,
                Err(error) => {
                    eprintln!(
                        "[bridge] could not read playerdata for {}: {error}",
                        uuid_string(uuid)
                    );
                    None
                }
            };
            let session = loaded.unwrap_or_else(|| {
                let ((x, y, z), game_mode) = self.spawn();
                let mut session =
                    PlayerSession::new(name.unwrap_or("Player").to_owned(), uuid, x, y, z);
                session.game_mode = game_mode;
                session
            });
            self.players.insert(uuid, session);
        }
        let session = self
            .players
            .get_mut(&uuid)
            .expect("player was inserted above");
        if let Some(name) = name
            && !name.is_empty()
        {
            session.name = name.to_owned();
        }
        session
    }

    fn player(&mut self, uuid: [u8; 16]) -> &mut PlayerSession {
        self.ensure_player(None, uuid)
    }
}

fn with_bridge<R>(f: impl FnOnce(&mut BridgeState) -> R) -> Option<R> {
    let mut guard = BRIDGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.as_mut().map(f)
}

fn normalize_permission(permission: &str) -> String {
    permission.trim().to_ascii_lowercase()
}

/// Plugin names are used as `run/<name>/config.yml` directory names. Keep the
/// exact name (PluginHost uses `description.getName()` too) but refuse path
/// traversal and empty names.
fn safe_plugin_dir(name: &str) -> String {
    let trimmed = name.trim();
    let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    if base.is_empty() || base == "." || base == ".." {
        "plugin".to_owned()
    } else {
        base.to_owned()
    }
}

/// `Level.isBrightOutside` for the overworld timeline: sky darkening stays
/// below 4 while the sky-light multiplier is above 11/15. The 1.21.11 timeline
/// crosses that value at ticks 12523 (dusk) and 23478 (dawn).
fn is_day_time(full_time: i64) -> bool {
    let time = full_time.rem_euclid(24_000);
    !(12_523..23_478).contains(&time)
}

// ---------------------------------------------------------------------------
// World access
// ---------------------------------------------------------------------------

fn storage_for(state: &BridgeState) -> &'static Mutex<WorldStorage> {
    world::shared_overworld(state.world_root.clone())
}

fn block_name_in(storage: &mut WorldStorage, x: i32, y: i32, z: i32) -> Option<String> {
    let chunk_x = x.div_euclid(16);
    let chunk_z = z.div_euclid(16);
    let chunk = storage.load_chunk(chunk_x, chunk_z).ok().flatten()?;
    chunk
        .block_state(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize)
        .map(|state| state.name.clone())
}

fn highest_block_y_in(storage: &mut WorldStorage, x: i32, z: i32) -> Option<i32> {
    let chunk_x = x.div_euclid(16);
    let chunk_z = z.div_euclid(16);
    let chunk = storage.load_chunk(chunk_x, chunk_z).ok().flatten()?;
    let surface = chunk::surface_y(&chunk, x.rem_euclid(16) as usize, z.rem_euclid(16) as usize);
    Some((surface - 1).max(-64))
}

fn biome_in(storage: &mut WorldStorage, x: i32, y: i32, z: i32) -> Option<String> {
    let chunk_x = x.div_euclid(16);
    let chunk_z = z.div_euclid(16);
    let chunk = storage.load_chunk(chunk_x, chunk_z).ok().flatten()?;
    chunk
        .biome(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize)
        .cloned()
}

fn light_in(storage: &mut WorldStorage, x: i32, y: i32, z: i32, sky: bool) -> Option<u8> {
    let chunk_x = x.div_euclid(16);
    let chunk_z = z.div_euclid(16);
    let mut chunk = storage.load_chunk(chunk_x, chunk_z).ok().flatten()?;
    if !chunk.has_light() {
        // Stored flat leftovers predate the light engine; compute a read-only
        // view instead of reporting a misleading zero.
        chunk.recompute_light();
    }
    let local_x = x.rem_euclid(16) as usize;
    let local_z = z.rem_euclid(16) as usize;
    if sky {
        chunk.sky_light(local_x, y, local_z)
    } else {
        chunk.block_light(local_x, y, local_z)
    }
}

fn block_state_for_material(material: &str) -> BlockState {
    let material = material.trim();
    let key = if material.contains(':') {
        material.to_owned()
    } else {
        format!("minecraft:{material}")
    };
    folia_core::registry::default_block_state(&key).unwrap_or(BlockState {
        name: key,
        properties: Vec::new(),
    })
}

fn set_block_in(storage: &mut WorldStorage, x: i32, y: i32, z: i32, material: &str) -> bool {
    match folia_core::block_update::set_block(storage, x, y, z, block_state_for_material(material))
    {
        Ok(Some(change)) => {
            folia_core::block_update::broadcast_change(&change);
            true
        }
        Ok(None) => false,
        Err(error) => {
            eprintln!("[bridge] block edit at ({x},{y},{z}) failed: {error}");
            false
        }
    }
}

fn world_block_name(state: &BridgeState, x: i32, y: i32, z: i32) -> Option<String> {
    let storage = storage_for(state);
    let mut guard = world::lock(storage);
    block_name_in(&mut guard, x, y, z)
}

fn world_highest_block_y(state: &BridgeState, x: i32, z: i32) -> Option<i32> {
    let storage = storage_for(state);
    let mut guard = world::lock(storage);
    highest_block_y_in(&mut guard, x, z)
}

fn world_biome(state: &BridgeState, x: i32, y: i32, z: i32) -> Option<String> {
    let storage = storage_for(state);
    let mut guard = world::lock(storage);
    biome_in(&mut guard, x, y, z)
}

fn world_light(state: &BridgeState, x: i32, y: i32, z: i32, sky: bool) -> Option<u8> {
    let storage = storage_for(state);
    let mut guard = world::lock(storage);
    light_in(&mut guard, x, y, z, sky)
}

fn world_set_block(state: &BridgeState, x: i32, y: i32, z: i32, material: &str) -> bool {
    let storage = storage_for(state);
    let mut guard = world::lock(storage);
    set_block_in(&mut guard, x, y, z, material)
}

fn world_full_time(state: &BridgeState) -> i64 {
    state.world_state.full_time
}

fn world_set_full_time(state: &mut BridgeState, time: i64) {
    state.world_state.full_time = time;
    state.save_world_state();
    let sent = folia_core::join::broadcast_world_time(time, time, true);
    println!("[bridge] world time {time} sent to {sent} live session(s)");
}

fn world_set_storm(state: &mut BridgeState, storm: bool) {
    let changed = state.world_state.storm != storm;
    state.world_state.storm = storm;
    state.world_state.weather_duration = 0;
    state.world_state.clear_weather_duration = 0;
    state.save_world_state();
    if changed {
        folia_core::join::broadcast_game_event(if storm { 1 } else { 2 }, 0.0);
        folia_core::join::broadcast_game_event(7, if storm { 1.0 } else { 0.0 });
    }
}

fn world_set_thundering(state: &mut BridgeState, thundering: bool) {
    let changed = state.world_state.thundering != thundering;
    state.world_state.thundering = thundering;
    state.world_state.thunder_duration = 0;
    state.world_state.clear_weather_duration = 0;
    state.save_world_state();
    if changed {
        folia_core::join::broadcast_game_event(8, if thundering { 1.0 } else { 0.0 });
    }
}

fn world_set_spawn(state: &mut BridgeState, x: f64, y: f64, z: f64) -> bool {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return false;
    }
    state.world_state.spawn = (x, y, z);
    state.save_world_state();
    true
}

// ---------------------------------------------------------------------------
// Item/inventory codec
// ---------------------------------------------------------------------------

fn hex_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut index = 0;
    while index < value.len() {
        let byte = u8::from_str_radix(value.get(index..index + 2)?, 16).ok()?;
        bytes.push(byte);
        index += 2;
    }
    String::from_utf8(bytes).ok()
}

fn plain_text(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(text) => Some(text.clone()),
        Tag::Compound(entries) => entries
            .iter()
            .find(|(key, _)| key == "text")
            .and_then(|(_, value)| value.as_str())
            .map(str::to_owned),
        _ => None,
    }
}

fn encode_stack(stack: &ItemStack) -> String {
    if stack.is_empty() {
        return String::new();
    }
    let damage = match stack.get_component("minecraft:damage") {
        Some(ComponentValue::Damage(value)) => *value,
        _ => 0,
    };
    let display_name = match stack.get_component("minecraft:custom_name") {
        Some(ComponentValue::CustomName(tag)) => plain_text(tag).unwrap_or_default(),
        _ => String::new(),
    };
    let unbreakable = matches!(
        stack.get_component("minecraft:unbreakable"),
        Some(ComponentValue::Unbreakable)
    );
    let lore = match stack.get_component("minecraft:lore") {
        Some(ComponentValue::Lore(lines)) => {
            lines.iter().filter_map(plain_text).collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    let enchantments = match stack.get_component("minecraft:enchantments") {
        Some(ComponentValue::Enchantments(levels)) => levels.clone(),
        _ => BTreeMap::new(),
    };
    let mut encoded = format!(
        "2\t{}\t{}\t{}\t{}\t{}\t{}",
        stack.item,
        stack.count,
        damage,
        hex_encode(&display_name),
        u8::from(unbreakable),
        lore.len()
    );
    for line in lore {
        encoded.push('\t');
        encoded.push_str(&hex_encode(&line));
    }
    encoded.push('\t');
    encoded.push_str(&enchantments.len().to_string());
    for (key, level) in enchantments {
        encoded.push('\t');
        encoded.push_str(&hex_encode(&key));
        encoded.push('=');
        encoded.push_str(&level.to_string());
    }
    encoded
}

fn decode_stack(encoded: &str) -> ItemStack {
    if encoded.is_empty() {
        return ItemStack::empty();
    }
    let fields: Vec<&str> = encoded.split('\t').collect();
    if fields.first().is_some_and(|field| *field == "2") {
        return decode_stack_v2(&fields);
    }
    decode_stack_v1(&fields)
}

fn decode_stack_v1(fields: &[&str]) -> ItemStack {
    let item = fields.first().copied().unwrap_or_default();
    if item.is_empty() {
        return ItemStack::empty();
    }
    let count = fields
        .get(1)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let damage = fields
        .get(2)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let display_name = fields
        .get(3)
        .and_then(|value| hex_decode(value))
        .unwrap_or_default();
    let unbreakable = fields.get(4).is_some_and(|value| *value == "1");
    let lore: Vec<Tag> = fields
        .iter()
        .skip(5)
        .filter_map(|value| hex_decode(value))
        .filter(|line| !line.is_empty())
        .map(Tag::String)
        .collect();

    let mut stack = ItemStack::new(item, count);
    if damage > 0 {
        stack.set_component(ComponentValue::Damage(damage));
    }
    if !display_name.is_empty() {
        stack.set_component(ComponentValue::CustomName(Tag::String(display_name)));
    }
    if unbreakable {
        stack.set_component(ComponentValue::Unbreakable);
    }
    if !lore.is_empty() {
        stack.set_component(ComponentValue::Lore(lore));
    }
    stack
}

fn decode_stack_v2(fields: &[&str]) -> ItemStack {
    let item = fields.get(1).copied().unwrap_or_default();
    if item.is_empty() {
        return ItemStack::empty();
    }
    let count = fields
        .get(2)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let damage = fields
        .get(3)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let display_name = fields
        .get(4)
        .and_then(|value| hex_decode(value))
        .unwrap_or_default();
    let unbreakable = fields.get(5).is_some_and(|value| *value == "1");
    let lore_count = fields
        .get(6)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let lore_start: usize = 7;
    let lore_end = lore_start.saturating_add(lore_count).min(fields.len());
    let lore: Vec<Tag> = fields[lore_start..lore_end]
        .iter()
        .filter_map(|value| hex_decode(value))
        .map(Tag::String)
        .collect();
    let enchantment_count = fields
        .get(lore_end)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut enchantments = BTreeMap::new();
    for value in fields
        .iter()
        .skip(lore_end.saturating_add(1))
        .take(enchantment_count)
    {
        let Some((key, level)) = value.split_once('=') else {
            continue;
        };
        let Some(key) = hex_decode(key) else {
            continue;
        };
        let Ok(level) = level.parse::<i32>() else {
            continue;
        };
        enchantments.insert(key, level);
    }

    let mut stack = ItemStack::new(item, count);
    if damage > 0 {
        stack.set_component(ComponentValue::Damage(damage));
    }
    if !display_name.is_empty() {
        stack.set_component(ComponentValue::CustomName(Tag::String(display_name)));
    }
    if unbreakable {
        stack.set_component(ComponentValue::Unbreakable);
    }
    if !lore.is_empty() {
        stack.set_component(ComponentValue::Lore(lore));
    }
    if !enchantments.is_empty() {
        stack.set_component(ComponentValue::Enchantments(enchantments));
    }
    stack
}

fn encode_inventory(inventory: &[ItemStack]) -> String {
    let mut encoded = String::new();
    for (slot, stack) in inventory.iter().enumerate().take(INVENTORY_SLOTS) {
        if stack.is_empty() {
            continue;
        }
        encoded.push_str(&format!("{slot}\t{}\n", encode_stack(stack)));
    }
    encoded
}

fn decode_inventory(encoded: &str) -> Vec<ItemStack> {
    let mut inventory = vec![ItemStack::empty(); INVENTORY_SLOTS];
    for line in encoded.split('\n') {
        if line.is_empty() {
            continue;
        }
        let Some((slot, stack)) = line.split_once('\t') else {
            continue;
        };
        let Ok(slot) = slot.parse::<usize>() else {
            continue;
        };
        if slot < inventory.len() {
            inventory[slot] = decode_stack(stack);
        }
    }
    inventory
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

fn string_arg(env: &mut JNIEnv, value: &JString) -> String {
    env.get_string(value)
        .map(Into::into)
        .unwrap_or_else(|_| String::new())
}

fn java_string(env: &mut JNIEnv, value: &str) -> jstring {
    env.new_string(value)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn parse_uuid(value: &str) -> Option<[u8; 16]> {
    let hex: String = value
        .chars()
        .filter(|character| *character != '-')
        .collect();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

fn uuid_string(uuid: [u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---------------------------------------------------------------------------
// World native methods
// ---------------------------------------------------------------------------

extern "system" fn native_world_block(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    y: jint,
    z: jint,
) -> jstring {
    let _world = string_arg(&mut env, &world_name);
    let name = with_bridge(|state| world_block_name(state, x, y, z))
        .flatten()
        .unwrap_or_else(|| "minecraft:air".to_owned());
    java_string(&mut env, &name)
}

extern "system" fn native_world_set_block(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    y: jint,
    z: jint,
    material: JString,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    let material = string_arg(&mut env, &material);
    u8::from(with_bridge(|state| world_set_block(state, x, y, z, &material)).unwrap_or(false))
}

extern "system" fn native_world_highest_block_y(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    z: jint,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_highest_block_y(state, x, z))
        .flatten()
        .unwrap_or(-64)
}

extern "system" fn native_world_biome(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    y: jint,
    z: jint,
) -> jstring {
    let _world = string_arg(&mut env, &world_name);
    let biome = with_bridge(|state| world_biome(state, x, y, z))
        .flatten()
        .unwrap_or_else(|| "minecraft:plains".to_owned());
    java_string(&mut env, &biome)
}

extern "system" fn native_world_block_light(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    y: jint,
    z: jint,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_light(state, x, y, z, false))
        .flatten()
        .map_or(0, jint::from)
}

extern "system" fn native_world_sky_light(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jint,
    y: jint,
    z: jint,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_light(state, x, y, z, true))
        .flatten()
        .map_or(0, jint::from)
}

extern "system" fn native_world_spawn(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jstring {
    let _world = string_arg(&mut env, &world_name);
    let spawn = with_bridge(|state| {
        let ((x, y, z), _) = state.spawn();
        format!("{x}\t{y}\t{z}")
    })
    .unwrap_or_else(|| "0.5\t64\t0.5".to_owned());
    java_string(&mut env, &spawn)
}

extern "system" fn native_world_full_time(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jlong {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_full_time(&*state)).unwrap_or(0)
}

extern "system" fn native_world_set_full_time(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    time: jlong,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_set_full_time(state, time));
}

extern "system" fn native_world_is_day_time(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    u8::from(with_bridge(|state| is_day_time(state.world_state.full_time)).unwrap_or(true))
}

extern "system" fn native_world_has_storm(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    u8::from(with_bridge(|state| state.world_state.storm).unwrap_or(false))
}

extern "system" fn native_world_set_storm(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    storm: jboolean,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_set_storm(state, storm != 0));
}

extern "system" fn native_world_weather_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| state.world_state.weather_duration).unwrap_or(0)
}

extern "system" fn native_world_set_weather_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    duration: jint,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| {
        state.world_state.weather_duration = duration.max(0);
        state.save_world_state();
    });
}

extern "system" fn native_world_is_thundering(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    u8::from(with_bridge(|state| state.world_state.thundering).unwrap_or(false))
}

extern "system" fn native_world_set_thundering(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    thundering: jboolean,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| world_set_thundering(state, thundering != 0));
}

extern "system" fn native_world_thunder_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| state.world_state.thunder_duration).unwrap_or(0)
}

extern "system" fn native_world_set_thunder_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    duration: jint,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| {
        state.world_state.thunder_duration = duration.max(0);
        state.save_world_state();
    });
}

extern "system" fn native_world_clear_weather_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
) -> jint {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| state.world_state.clear_weather_duration).unwrap_or(0)
}

extern "system" fn native_world_set_clear_weather_duration(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    duration: jint,
) {
    let _world = string_arg(&mut env, &world_name);
    with_bridge(|state| {
        state.world_state.clear_weather_duration = duration.max(0);
        state.save_world_state();
    });
}

extern "system" fn native_world_set_spawn(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    x: jdouble,
    y: jdouble,
    z: jdouble,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    u8::from(with_bridge(|state| world_set_spawn(state, x, y, z)).unwrap_or(false))
}

extern "system" fn native_world_play_sound(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    sound: JString,
    category: jint,
    x: jdouble,
    y: jdouble,
    z: jdouble,
    volume: jfloat,
    pitch: jfloat,
    seed: jlong,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    let sound = string_arg(&mut env, &sound);
    u8::from(folia_core::join::broadcast_sound(&sound, category, x, y, z, volume, pitch, seed) > 0)
}

extern "system" fn native_world_spawn_particle(
    mut env: JNIEnv,
    _class: JClass,
    world_name: JString,
    particle: JString,
    x: jdouble,
    y: jdouble,
    z: jdouble,
    offset_x: jfloat,
    offset_y: jfloat,
    offset_z: jfloat,
    speed: jfloat,
    count: jint,
    force: jboolean,
) -> jboolean {
    let _world = string_arg(&mut env, &world_name);
    let particle = string_arg(&mut env, &particle);
    u8::from(
        folia_core::join::broadcast_particle(
            &particle,
            x,
            y,
            z,
            offset_x,
            offset_y,
            offset_z,
            speed,
            count,
            force != 0,
        ) > 0,
    )
}

// ---------------------------------------------------------------------------
// Config native methods
// ---------------------------------------------------------------------------

extern "system" fn native_config_read(mut env: JNIEnv, _class: JClass, plugin: JString) -> jstring {
    let plugin = string_arg(&mut env, &plugin);
    let contents = with_bridge(|state| state.read_plugin_config(&plugin)).unwrap_or_default();
    java_string(&mut env, &contents)
}

extern "system" fn native_config_write(
    mut env: JNIEnv,
    _class: JClass,
    plugin: JString,
    contents: JString,
) -> jboolean {
    let plugin = string_arg(&mut env, &plugin);
    let contents = string_arg(&mut env, &contents);
    u8::from(with_bridge(|state| state.write_plugin_config(&plugin, &contents)).unwrap_or(false))
}

// ---------------------------------------------------------------------------
// Player native methods
// ---------------------------------------------------------------------------

fn player_uuid(env: &mut JNIEnv, value: &JString) -> Option<[u8; 16]> {
    parse_uuid(&string_arg(env, value))
}

extern "system" fn native_player_exists(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let exists = with_bridge(|state| {
        state.players.contains_key(&uuid)
            || PlayerSession::path(&state.playerdata_dir, &uuid).exists()
    })
    .unwrap_or(false);
    u8::from(exists)
}

extern "system" fn native_player_identify(
    mut env: JNIEnv,
    _class: JClass,
    name: JString,
    uuid: JString,
) {
    let name = string_arg(&mut env, &name);
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    with_bridge(|state| {
        state.ensure_player(Some(&name), uuid);
    });
}

extern "system" fn native_player_health(mut env: JNIEnv, _class: JClass, uuid: JString) -> jdouble {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 20.0;
    };
    with_bridge(|state| f64::from(state.player(uuid).health)).unwrap_or(20.0)
}

extern "system" fn native_player_set_health(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    health: jdouble,
) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.health = (health as f32).clamp(0.0, 20.0);
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save health for {}: {error}",
                session.name
            );
        }
    });
}

extern "system" fn native_player_food(mut env: JNIEnv, _class: JClass, uuid: JString) -> jint {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 20;
    };
    with_bridge(|state| state.player(uuid).food_level).unwrap_or(20)
}

extern "system" fn native_player_set_food(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    food: jint,
) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.food_level = food.clamp(0, 20);
        if let Err(error) = session.save(&dir) {
            eprintln!("[bridge] could not save food for {}: {error}", session.name);
        }
    });
}

extern "system" fn native_player_saturation(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jdouble {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 5.0;
    };
    with_bridge(|state| f64::from(state.player(uuid).food_saturation)).unwrap_or(5.0)
}

extern "system" fn native_player_set_saturation(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    saturation: jdouble,
) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.food_saturation = (saturation as f32).clamp(0.0, 20.0);
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save saturation for {}: {error}",
                session.name
            );
        }
    });
}

extern "system" fn native_player_location(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jstring {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return java_string(&mut env, "0.5\t64\t0.5\t0\t0");
    };
    let location = with_bridge(|state| {
        let session = state.player(uuid);
        format!(
            "{}\t{}\t{}\t{}\t{}",
            session.x, session.y, session.z, session.yaw, session.pitch
        )
    })
    .unwrap_or_else(|| "0.5\t64\t0.5\t0\t0".to_owned());
    java_string(&mut env, &location)
}

extern "system" fn native_player_teleport(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    x: jdouble,
    y: jdouble,
    z: jdouble,
    yaw: jni::sys::jfloat,
    pitch: jni::sys::jfloat,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    if !x.is_finite() || !y.is_finite() || !z.is_finite() || !yaw.is_finite() || !pitch.is_finite()
    {
        return 0;
    }
    let accepted = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.teleport(x, y, z, false);
        session.yaw = yaw;
        session.pitch = pitch;
        session.physics.yaw = yaw;
        session.physics.pitch = pitch;
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save teleport for {}: {error}",
                session.name
            );
            return false;
        }
        true
    })
    .unwrap_or(false);
    if accepted {
        let live = folia_core::join::teleport_player(uuid, x, y, z, yaw, pitch);
        println!(
            "[bridge] teleported {} to ({x:.2}, {y:.2}, {z:.2}); live session {}",
            uuid_string(uuid),
            if live { "queued" } else { "not connected" }
        );
    }
    u8::from(accepted)
}

extern "system" fn native_player_game_mode(mut env: JNIEnv, _class: JClass, uuid: JString) -> jint {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    with_bridge(|state| state.player(uuid).game_mode.clamp(0, 3)).unwrap_or(0)
}

extern "system" fn native_player_set_game_mode(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    game_mode: jint,
) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    let game_mode = game_mode.clamp(0, 3);
    let name = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.game_mode = game_mode;
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save game mode for {}: {error}",
                session.name
            );
        }
        session.name.clone()
    });
    if let Some(name) = name {
        let command = format!("gamemode {game_mode} {name}");
        for line in folia_core::join::execute_console_command(&command) {
            println!("[bridge] {line}");
        }
    }
}

extern "system" fn native_player_send_message(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    message: JString,
) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    let message = string_arg(&mut env, &message);
    let name = with_bridge(|state| state.player(uuid).name.clone()).unwrap_or_default();
    let lines: Vec<&str> = message.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };
    let mut delivered = false;
    for line in lines {
        delivered |= folia_core::join::send_player_chat(uuid, line);
    }
    if !delivered {
        println!("[bridge] -> {name} (offline): {message}");
    }
}

#[allow(clippy::too_many_arguments)]
extern "system" fn native_player_play_sound(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    sound: JString,
    category: jint,
    x: jdouble,
    y: jdouble,
    z: jdouble,
    volume: jfloat,
    pitch: jfloat,
    seed: jlong,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let sound = string_arg(&mut env, &sound);
    u8::from(folia_core::join::send_player_sound(
        uuid, &sound, category, x, y, z, volume, pitch, seed,
    ))
}

#[allow(clippy::too_many_arguments)]
extern "system" fn native_player_spawn_particle(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    particle: JString,
    x: jdouble,
    y: jdouble,
    z: jdouble,
    offset_x: jfloat,
    offset_y: jfloat,
    offset_z: jfloat,
    speed: jfloat,
    count: jint,
    force: jboolean,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let particle = string_arg(&mut env, &particle);
    u8::from(folia_core::join::send_player_particle(
        uuid,
        &particle,
        x,
        y,
        z,
        offset_x,
        offset_y,
        offset_z,
        speed,
        count,
        force != 0,
    ))
}

extern "system" fn native_player_save(mut env: JNIEnv, _class: JClass, uuid: JString) {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return;
    };
    with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save playerdata for {}: {error}",
                session.name
            );
        }
    });
}

extern "system" fn native_player_inventory(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jstring {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return java_string(&mut env, "");
    };
    let encoded = with_bridge(|state| {
        let inventory = state.player(uuid).inventory.clone();
        encode_inventory(&inventory)
    })
    .unwrap_or_default();
    java_string(&mut env, &encoded)
}

extern "system" fn native_player_set_inventory(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    encoded: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let encoded = string_arg(&mut env, &encoded);
    let inventory = decode_inventory(&encoded);
    let saved = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.inventory = inventory;
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save inventory for {}: {error}",
                session.name
            );
            return false;
        }
        true
    })
    .unwrap_or(false);
    u8::from(saved)
}

extern "system" fn native_player_item(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    slot: jint,
) -> jstring {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return java_string(&mut env, "");
    };
    let encoded = with_bridge(|state| {
        let session = state.player(uuid);
        if slot < 0 || slot as usize >= INVENTORY_SLOTS {
            return String::new();
        }
        encode_stack(&session.inventory[slot as usize])
    })
    .unwrap_or_default();
    java_string(&mut env, &encoded)
}

extern "system" fn native_player_set_item(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    slot: jint,
    encoded: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    if slot < 0 || slot as usize >= INVENTORY_SLOTS {
        return 0;
    }
    let stack = decode_stack(&string_arg(&mut env, &encoded));
    let saved = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.inventory[slot as usize] = stack;
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save item slot {slot} for {}: {error}",
                session.name
            );
            return false;
        }
        true
    })
    .unwrap_or(false);
    u8::from(saved)
}

extern "system" fn native_player_selected_slot(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jint {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    with_bridge(|state| state.player(uuid).selected_slot.clamp(0, HOTBAR_SLOTS - 1)).unwrap_or(0)
}

extern "system" fn native_player_set_selected_slot(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    slot: jint,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let saved = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.selected_slot = slot.clamp(0, HOTBAR_SLOTS - 1);
        if let Err(error) = session.save(&dir) {
            eprintln!(
                "[bridge] could not save selected slot for {}: {error}",
                session.name
            );
            return false;
        }
        true
    })
    .unwrap_or(false);
    u8::from(saved)
}

extern "system" fn native_player_is_op(mut env: JNIEnv, _class: JClass, uuid: JString) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    u8::from(with_bridge(|state| state.player_is_op(uuid)).unwrap_or(false))
}

extern "system" fn native_player_set_op(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    value: jboolean,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    u8::from(with_bridge(|state| state.player_set_op(uuid, value != 0)).unwrap_or(false))
}

extern "system" fn native_player_permission_has(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    permission: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let permission = string_arg(&mut env, &permission);
    u8::from(
        with_bridge(|state| {
            let is_op = state.player_is_op(uuid);
            state.permission_has(uuid, &permission, is_op)
        })
        .unwrap_or(false),
    )
}

extern "system" fn native_player_permission_is_set(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    permission: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let permission = string_arg(&mut env, &permission);
    u8::from(with_bridge(|state| state.permission_is_set(uuid, &permission)).unwrap_or(false))
}

extern "system" fn native_player_permission_effective(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
) -> jstring {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return java_string(&mut env, "");
    };
    let encoded = with_bridge(|state| {
        let mut encoded = String::new();
        for (permission, value) in state.effective_permissions(uuid) {
            encoded.push_str(&permission);
            encoded.push('\t');
            encoded.push(if value { '1' } else { '0' });
            encoded.push('\n');
        }
        encoded
    })
    .unwrap_or_default();
    java_string(&mut env, &encoded)
}

extern "system" fn native_player_permission_add_attachment(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    plugin: JString,
) -> jint {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return -1;
    };
    let plugin = string_arg(&mut env, &plugin);
    with_bridge(|state| state.add_permission_attachment(uuid, &plugin)).unwrap_or(-1)
}

extern "system" fn native_player_permission_remove_attachment(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    attachment: jint,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    u8::from(
        with_bridge(|state| state.remove_permission_attachment(uuid, attachment)).unwrap_or(false),
    )
}

extern "system" fn native_player_permission_set(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    attachment: jint,
    permission: JString,
    value: jboolean,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let permission = string_arg(&mut env, &permission);
    u8::from(
        with_bridge(|state| state.set_permission(uuid, attachment, &permission, value != 0))
            .unwrap_or(false),
    )
}

extern "system" fn native_player_permission_unset(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    attachment: jint,
    permission: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let permission = string_arg(&mut env, &permission);
    u8::from(
        with_bridge(|state| state.unset_permission(uuid, attachment, &permission)).unwrap_or(false),
    )
}

extern "system" fn native_player_kick(
    mut env: JNIEnv,
    _class: JClass,
    uuid: JString,
    reason: JString,
) -> jboolean {
    let Some(uuid) = player_uuid(&mut env, &uuid) else {
        return 0;
    };
    let reason = string_arg(&mut env, &reason);
    let pushed = folia_core::join::kick_player(uuid, &reason);
    if pushed {
        println!("[bridge] kick queued for {}: {reason}", uuid_string(uuid));
    } else {
        let name = with_bridge(|state| state.player(uuid).name.clone()).unwrap_or_default();
        println!("[bridge] kick requested for {name} but no live session: {reason}");
    }
    u8::from(pushed)
}

// ---------------------------------------------------------------------------
// Plugin runtime
// ---------------------------------------------------------------------------

/// Optional Java compatibility layer for `.jar` plugins.
///
/// The native server never depends on this type: when `plugins/` contains no
/// jars (or `FOLIA_DISABLE_PLUGINS=1` is set), startup returns `None` and no JVM
/// is created. When plugin jars are present, the embedded JVM hosts the Bukkit
/// API surface and the Rust tick loop drives it.
pub struct PluginRuntime {
    jvm: JavaVM,
    bridge: GlobalRef,
}

impl PluginRuntime {
    pub fn start(root: &Path) -> Result<Option<Self>> {
        if std::env::var("FOLIA_DISABLE_PLUGINS").is_ok_and(|value| value == "1") {
            println!("[rust] plugin layer disabled (FOLIA_DISABLE_PLUGINS=1)");
            return Ok(None);
        }

        let plugins_dir = root.join("plugins");
        std::fs::create_dir_all(&plugins_dir)?;
        if plugin_jars(&plugins_dir).is_empty() {
            println!(
                "[rust] no .jar plugins in {}; running native-only (no JVM)",
                plugins_dir.display()
            );
            return Ok(None);
        }

        if !root
            .join("java")
            .join("bridge")
            .join("classes")
            .join("rs")
            .join("folia")
            .join("bridge")
            .join("Bridge.class")
            .is_file()
        {
            bail!(
                "plugin jars are present but the Bukkit bridge is missing; \
                 run scripts/build-java.ps1 first"
            );
        }

        eprintln!("[rust] locating JDK for the plugin compatibility layer");
        ensure_java_home();
        let classpath = build_classpath(root)?;
        eprintln!("[rust] bootstrapping plugin JVM");

        let args = InitArgsBuilder::new()
            .version(JNIVersion::V8)
            .option("-Dfile.encoding=UTF-8")
            .option(format!("-Djava.class.path={classpath}"))
            .build()
            .context("failed to build JVM arguments")?;
        let jvm = JavaVM::new(args).context(
            "failed to start the plugin JVM; install Java 21 or remove the jars \
             from plugins/ to run native-only",
        )?;

        let bridge = {
            let mut env = jvm
                .attach_current_thread()
                .context("failed to attach to the plugin JVM")?;
            let bridge = env
                .find_class("rs/folia/bridge/Bridge")
                .context("Bukkit bridge class is missing; run scripts/build-java.ps1")?;

            let methods = [
                NativeMethod {
                    name: "nativePing".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_ping as *mut c_void,
                },
                NativeMethod {
                    name: "nativeScheduleTask".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/Runnable;JJ)I".into(),
                    fn_ptr: native_schedule_task as *mut c_void,
                },
                NativeMethod {
                    name: "nativeCancelTask".into(),
                    sig: "(I)V".into(),
                    fn_ptr: native_cancel_task as *mut c_void,
                },
                NativeMethod {
                    name: "nativeCancelTasks".into(),
                    sig: "(Ljava/lang/String;)V".into(),
                    fn_ptr: native_cancel_tasks as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldBlock".into(),
                    sig: "(Ljava/lang/String;III)Ljava/lang/String;".into(),
                    fn_ptr: native_world_block as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetBlock".into(),
                    sig: "(Ljava/lang/String;IIILjava/lang/String;)Z".into(),
                    fn_ptr: native_world_set_block as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldHighestBlockY".into(),
                    sig: "(Ljava/lang/String;II)I".into(),
                    fn_ptr: native_world_highest_block_y as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldBiome".into(),
                    sig: "(Ljava/lang/String;III)Ljava/lang/String;".into(),
                    fn_ptr: native_world_biome as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldBlockLight".into(),
                    sig: "(Ljava/lang/String;III)I".into(),
                    fn_ptr: native_world_block_light as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSkyLight".into(),
                    sig: "(Ljava/lang/String;III)I".into(),
                    fn_ptr: native_world_sky_light as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSpawn".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_world_spawn as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldFullTime".into(),
                    sig: "(Ljava/lang/String;)J".into(),
                    fn_ptr: native_world_full_time as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetFullTime".into(),
                    sig: "(Ljava/lang/String;J)V".into(),
                    fn_ptr: native_world_set_full_time as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldIsDayTime".into(),
                    sig: "(Ljava/lang/String;)Z".into(),
                    fn_ptr: native_world_is_day_time as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldHasStorm".into(),
                    sig: "(Ljava/lang/String;)Z".into(),
                    fn_ptr: native_world_has_storm as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetStorm".into(),
                    sig: "(Ljava/lang/String;Z)V".into(),
                    fn_ptr: native_world_set_storm as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldWeatherDuration".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_world_weather_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetWeatherDuration".into(),
                    sig: "(Ljava/lang/String;I)V".into(),
                    fn_ptr: native_world_set_weather_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldIsThundering".into(),
                    sig: "(Ljava/lang/String;)Z".into(),
                    fn_ptr: native_world_is_thundering as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetThundering".into(),
                    sig: "(Ljava/lang/String;Z)V".into(),
                    fn_ptr: native_world_set_thundering as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldThunderDuration".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_world_thunder_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetThunderDuration".into(),
                    sig: "(Ljava/lang/String;I)V".into(),
                    fn_ptr: native_world_set_thunder_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldClearWeatherDuration".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_world_clear_weather_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetClearWeatherDuration".into(),
                    sig: "(Ljava/lang/String;I)V".into(),
                    fn_ptr: native_world_set_clear_weather_duration as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSetSpawn".into(),
                    sig: "(Ljava/lang/String;DDD)Z".into(),
                    fn_ptr: native_world_set_spawn as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldPlaySound".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;IDDDFFJ)Z".into(),
                    fn_ptr: native_world_play_sound as *mut c_void,
                },
                NativeMethod {
                    name: "nativeWorldSpawnParticle".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;DDDFFFFIZ)Z".into(),
                    fn_ptr: native_world_spawn_particle as *mut c_void,
                },
                NativeMethod {
                    name: "nativeConfigRead".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_config_read as *mut c_void,
                },
                NativeMethod {
                    name: "nativeConfigWrite".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)Z".into(),
                    fn_ptr: native_config_write as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerExists".into(),
                    sig: "(Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_exists as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerIdentify".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
                    fn_ptr: native_player_identify as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerHealth".into(),
                    sig: "(Ljava/lang/String;)D".into(),
                    fn_ptr: native_player_health as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetHealth".into(),
                    sig: "(Ljava/lang/String;D)V".into(),
                    fn_ptr: native_player_set_health as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerFood".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_player_food as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetFood".into(),
                    sig: "(Ljava/lang/String;I)V".into(),
                    fn_ptr: native_player_set_food as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSaturation".into(),
                    sig: "(Ljava/lang/String;)D".into(),
                    fn_ptr: native_player_saturation as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetSaturation".into(),
                    sig: "(Ljava/lang/String;D)V".into(),
                    fn_ptr: native_player_set_saturation as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerLocation".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_player_location as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerTeleport".into(),
                    sig: "(Ljava/lang/String;DDDFF)Z".into(),
                    fn_ptr: native_player_teleport as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerGameMode".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_player_game_mode as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetGameMode".into(),
                    sig: "(Ljava/lang/String;I)V".into(),
                    fn_ptr: native_player_set_game_mode as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSendMessage".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
                    fn_ptr: native_player_send_message as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPlaySound".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;IDDDFFJ)Z".into(),
                    fn_ptr: native_player_play_sound as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSpawnParticle".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;DDDFFFFIZ)Z".into(),
                    fn_ptr: native_player_spawn_particle as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSave".into(),
                    sig: "(Ljava/lang/String;)V".into(),
                    fn_ptr: native_player_save as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerInventory".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_player_inventory as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetInventory".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_set_inventory as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerItem".into(),
                    sig: "(Ljava/lang/String;I)Ljava/lang/String;".into(),
                    fn_ptr: native_player_item as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetItem".into(),
                    sig: "(Ljava/lang/String;ILjava/lang/String;)Z".into(),
                    fn_ptr: native_player_set_item as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSelectedSlot".into(),
                    sig: "(Ljava/lang/String;)I".into(),
                    fn_ptr: native_player_selected_slot as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetSelectedSlot".into(),
                    sig: "(Ljava/lang/String;I)Z".into(),
                    fn_ptr: native_player_set_selected_slot as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerIsOp".into(),
                    sig: "(Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_is_op as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerSetOp".into(),
                    sig: "(Ljava/lang/String;Z)Z".into(),
                    fn_ptr: native_player_set_op as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionHas".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_permission_has as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionIsSet".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_permission_is_set as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionEffective".into(),
                    sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                    fn_ptr: native_player_permission_effective as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionAddAttachment".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)I".into(),
                    fn_ptr: native_player_permission_add_attachment as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionRemoveAttachment".into(),
                    sig: "(Ljava/lang/String;I)Z".into(),
                    fn_ptr: native_player_permission_remove_attachment as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionSet".into(),
                    sig: "(Ljava/lang/String;ILjava/lang/String;Z)Z".into(),
                    fn_ptr: native_player_permission_set as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerPermissionUnset".into(),
                    sig: "(Ljava/lang/String;ILjava/lang/String;)Z".into(),
                    fn_ptr: native_player_permission_unset as *mut c_void,
                },
                NativeMethod {
                    name: "nativePlayerKick".into(),
                    sig: "(Ljava/lang/String;Ljava/lang/String;)Z".into(),
                    fn_ptr: native_player_kick as *mut c_void,
                },
            ];
            env.register_native_methods(&bridge, &methods)
                .context("failed to register native bridge methods")?;

            let data_dir = root.join("run");
            std::fs::create_dir_all(&data_dir)?;
            let plugins_string = env.new_string(plugins_dir.to_string_lossy())?;
            let data_string = env.new_string(data_dir.to_string_lossy())?;
            let plugins_object = JObject::from(plugins_string);
            let data_object = JObject::from(data_string);

            *BRIDGE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(BridgeState::new(root));

            call(
                &mut env,
                &bridge,
                "boot",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[
                    JValue::Object(&plugins_object),
                    JValue::Object(&data_object),
                ],
            )
            .context("plugin host boot failed")?;

            env.new_global_ref(&bridge)
                .context("failed to pin the bridge class")?
        };

        eprintln!("[rust] plugin compatibility layer ready");
        let mut runtime = Self { jvm, bridge };
        if std::env::var("FOLIA_BRIDGE_SELFTEST").is_ok_and(|value| value == "1") {
            runtime.run_event_selftest()?;
        }
        Ok(Some(runtime))
    }

    fn class<'local>(&self, env: &mut JNIEnv<'local>) -> Result<JClass<'local>> {
        Ok(JClass::from(env.new_local_ref(self.bridge.as_obj())?))
    }

    pub fn tick(&mut self) -> Result<()> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        tick_tasks(&mut env);
        let bridge = self.class(&mut env)?;
        call(&mut env, &bridge, "tick", "()V", &[]).context("plugin tick failed")
    }

    pub fn fire_player_join(&mut self, name: &str, uuid: &str) -> Result<()> {
        if let Some(uuid) = parse_uuid(uuid) {
            with_bridge(|state| {
                state.ensure_player(Some(name), uuid);
            });
        }
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        let name = JObject::from(env.new_string(name)?);
        let uuid = JObject::from(env.new_string(uuid)?);
        let bridge = self.class(&mut env)?;
        call(
            &mut env,
            &bridge,
            "firePlayerJoin",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&name), JValue::Object(&uuid)],
        )
        .context("PlayerJoinEvent dispatch failed")
    }

    pub fn fire_block_break(
        &mut self,
        name: &str,
        uuid: &str,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<bool> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        let name = JObject::from(env.new_string(name)?);
        let uuid = JObject::from(env.new_string(uuid)?);
        let bridge = self.class(&mut env)?;
        let value = env
            .call_static_method(
                &bridge,
                "fireBlockBreak",
                "(Ljava/lang/String;Ljava/lang/String;III)Z",
                &[
                    JValue::Object(&name),
                    JValue::Object(&uuid),
                    JValue::Int(x),
                    JValue::Int(y),
                    JValue::Int(z),
                ],
            )
            .context("BlockBreakEvent dispatch failed")?;
        Ok(value.z()?)
    }

    pub fn fire_block_place(
        &mut self,
        name: &str,
        uuid: &str,
        x: i32,
        y: i32,
        z: i32,
        material: &str,
    ) -> Result<bool> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        let name = JObject::from(env.new_string(name)?);
        let uuid = JObject::from(env.new_string(uuid)?);
        let material = JObject::from(env.new_string(material)?);
        let bridge = self.class(&mut env)?;
        let value = env
            .call_static_method(
                &bridge,
                "fireBlockPlace",
                "(Ljava/lang/String;Ljava/lang/String;IIILjava/lang/String;)Z",
                &[
                    JValue::Object(&name),
                    JValue::Object(&uuid),
                    JValue::Int(x),
                    JValue::Int(y),
                    JValue::Int(z),
                    JValue::Object(&material),
                ],
            )
            .context("BlockPlaceEvent dispatch failed")?;
        Ok(value.z()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fire_player_interact(
        &mut self,
        name: &str,
        uuid: &str,
        x: i32,
        y: i32,
        z: i32,
        material: &str,
        action: &str,
        face: &str,
    ) -> Result<bool> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        let name = JObject::from(env.new_string(name)?);
        let uuid = JObject::from(env.new_string(uuid)?);
        let material = JObject::from(env.new_string(material)?);
        let action = JObject::from(env.new_string(action)?);
        let face = JObject::from(env.new_string(face)?);
        let bridge = self.class(&mut env)?;
        let value = env
            .call_static_method(
                &bridge,
                "firePlayerInteract",
                "(Ljava/lang/String;Ljava/lang/String;IIILjava/lang/String;\
                 Ljava/lang/String;Ljava/lang/String;)Z",
                &[
                    JValue::Object(&name),
                    JValue::Object(&uuid),
                    JValue::Int(x),
                    JValue::Int(y),
                    JValue::Int(z),
                    JValue::Object(&material),
                    JValue::Object(&action),
                    JValue::Object(&face),
                ],
            )
            .context("PlayerInteractEvent dispatch failed")?;
        Ok(value.z()?)
    }

    fn run_event_selftest(&mut self) -> Result<()> {
        let name = std::env::var("FOLIA_BRIDGE_SELFTEST_PLAYER")
            .unwrap_or_else(|_| "BridgeProbe".to_owned());
        let uuid = "00000000-0000-0000-0000-0000000000aa";
        let bedrock = with_bridge(|state| world_block_name(state, 0, -64, 0))
            .flatten()
            .unwrap_or_else(|| "minecraft:air".to_owned());
        let solid = bedrock == "minecraft:bedrock";

        let break_cancelled = self.fire_block_break(&name, uuid, 0, -64, 0)?;
        if break_cancelled != solid {
            bail!(
                "bridge self-test: BlockBreakEvent cancellation {break_cancelled} did not \
                 match bedrock={solid}"
            );
        }

        let place_cancelled = self.fire_block_place(&name, uuid, 0, 100, 0, "minecraft:stone")?;
        if place_cancelled {
            bail!("bridge self-test: stone BlockPlaceEvent was unexpectedly cancelled");
        }
        let place_bedrock = self.fire_block_place(&name, uuid, 0, 100, 0, "minecraft:bedrock")?;
        if !place_bedrock {
            bail!("bridge self-test: bedrock BlockPlaceEvent was not cancelled");
        }

        let interact_cancelled = self.fire_player_interact(
            &name,
            uuid,
            0,
            -64,
            0,
            "minecraft:bedrock",
            "LEFT_CLICK_BLOCK",
            "UP",
        )?;
        if interact_cancelled != solid {
            bail!(
                "bridge self-test: PlayerInteractEvent cancellation {interact_cancelled} did \
                 not match bedrock={solid}"
            );
        }

        println!(
            "[bridge] self-test passed: break={break_cancelled}, place-stone={place_cancelled}, \
             place-bedrock={place_bedrock}, interact={interact_cancelled}"
        );
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let mut env = self
            .jvm
            .attach_current_thread()
            .context("failed to attach to the plugin JVM")?;
        let bridge = self.class(&mut env)?;
        call(&mut env, &bridge, "shutdown", "()V", &[]).context("plugin host shutdown failed")?;
        TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        Ok(())
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        // GlobalRefs must be released while the JVM is still alive.
        TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

fn call(env: &mut JNIEnv, class: &JClass, name: &str, sig: &str, args: &[JValue]) -> Result<()> {
    if let Err(error) = env.call_static_method(class, name, sig, args) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err(error.into());
    }
    Ok(())
}

fn plugin_jars(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut jars: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        })
        .collect();
    jars.sort();
    jars
}

fn build_classpath(root: &Path) -> Result<String> {
    let mut entries = vec![root.join("java").join("bridge").join("classes")];
    if let Ok(directory) = std::env::var("FOLIA_LIBRARIES") {
        collect_jars(Path::new(&directory), &mut entries)?;
    } else {
        let local_libraries = root.join("libraries");
        let libraries = if local_libraries.is_dir() {
            local_libraries
        } else {
            root.join("..").join("libraries")
        };
        collect_jars(&libraries, &mut entries)?;
    }
    let separator = if cfg!(windows) { ';' } else { ':' };
    Ok(entries
        .iter()
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(&separator.to_string()))
}

fn collect_jars(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jars(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "jar") {
            out.push(path);
        }
    }
    Ok(())
}

fn ensure_java_home() {
    if std::env::var_os("JAVA_HOME").is_some() {
        return;
    }

    let candidates = [
        r"C:\Program Files\Java\jdk-21",
        r"C:\Program Files\Eclipse Adoptium\jdk-21",
        r"C:\Program Files\Microsoft\jdk-21",
    ];

    for candidate in candidates {
        let home = Path::new(candidate);
        let windows_jvm = home.join("bin").join("server").join("jvm.dll");
        let unix_jvm = home.join("lib").join("server").join("libjvm.so");
        if windows_jvm.exists() || unix_jvm.exists() {
            // SAFETY: this runs before any JVM threads are started.
            unsafe { std::env::set_var("JAVA_HOME", home) };
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use folia_core::anvil::Compression;
    use folia_core::world::Dimension;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "folia-bridge-{label}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp directory");
        path
    }

    #[test]
    fn hex_round_trips_unicode() {
        let value = "Grüße, 世界\twith a tab";
        assert_eq!(hex_decode(&hex_encode(value)).as_deref(), Some(value));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn item_stack_round_trips_components() {
        let mut stack = ItemStack::new("minecraft:diamond_sword", 1);
        stack.set_component(ComponentValue::Damage(7));
        stack.set_component(ComponentValue::CustomName(Tag::String(
            "Bridge Blade".into(),
        )));
        stack.set_component(ComponentValue::Lore(vec![
            Tag::String("first line".into()),
            Tag::String("第二行".into()),
        ]));
        stack.set_component(ComponentValue::Unbreakable);
        stack.set_component(ComponentValue::Enchantments(BTreeMap::from([
            ("minecraft:sharpness".into(), 3),
            ("minecraft:unbreaking".into(), 2),
        ])));

        let encoded = encode_stack(&stack);
        let decoded = decode_stack(&encoded);
        assert_eq!(decoded.item, "minecraft:diamond_sword");
        assert_eq!(decoded.count, 1);
        assert_eq!(
            decoded.get_component("minecraft:damage"),
            Some(&ComponentValue::Damage(7))
        );
        assert_eq!(
            decoded.get_component("minecraft:custom_name"),
            Some(&ComponentValue::CustomName(Tag::String(
                "Bridge Blade".into()
            )))
        );
        assert_eq!(
            decoded.get_component("minecraft:lore"),
            Some(&ComponentValue::Lore(vec![
                Tag::String("first line".into()),
                Tag::String("第二行".into())
            ]))
        );
        assert_eq!(
            decoded.get_component("minecraft:unbreakable"),
            Some(&ComponentValue::Unbreakable)
        );
        assert_eq!(
            decoded.get_component("minecraft:enchantments"),
            Some(&ComponentValue::Enchantments(BTreeMap::from([
                ("minecraft:sharpness".into(), 3),
                ("minecraft:unbreaking".into(), 2),
            ])))
        );
        assert!(encode_stack(&ItemStack::empty()).is_empty());
    }

    #[test]
    fn legacy_item_encoding_still_decodes() {
        let encoded = format!(
            "minecraft:stone\t12\t0\t{}\t1\t{}",
            hex_encode(""),
            hex_encode("legacy lore")
        );
        let decoded = decode_stack(&encoded);
        assert_eq!(decoded, {
            let mut expected = ItemStack::new("minecraft:stone", 12);
            expected.set_component(ComponentValue::Unbreakable);
            expected.set_component(ComponentValue::Lore(vec![Tag::String(
                "legacy lore".into(),
            )]));
            expected
        });
    }

    #[test]
    fn inventory_round_trips_slots() {
        let mut inventory = vec![ItemStack::empty(); INVENTORY_SLOTS];
        inventory[0] = ItemStack::new("minecraft:stone", 32);
        inventory[7] = ItemStack::new("minecraft:diamond", 3);
        let encoded = encode_inventory(&inventory);
        let decoded = decode_inventory(&encoded);
        assert_eq!(decoded[0], ItemStack::new("minecraft:stone", 32));
        assert_eq!(decoded[7], ItemStack::new("minecraft:diamond", 3));
        assert!(decoded[8].is_empty());
        assert_eq!(
            decode_inventory("bogus\n99\tminecraft:stone\t1\t0\t\t0"),
            vec![ItemStack::empty(); INVENTORY_SLOTS]
        );
    }

    #[test]
    fn uuid_parser_accepts_both_forms() {
        let hyphenated = "00000000-0000-0000-0000-0000000000aa";
        let compact = "000000000000000000000000000000aa";
        assert_eq!(parse_uuid(hyphenated), parse_uuid(compact));
        assert!(parse_uuid("not-a-uuid").is_none());
        assert_eq!(
            parse_uuid(hyphenated).map(uuid_string).as_deref(),
            Some(hyphenated)
        );
    }

    #[test]
    fn playerdata_round_trips_through_bridge_state() {
        let root = temp_dir("player");
        let mut state = BridgeState::new(&root);
        let uuid = [0x11; 16];
        {
            let dir = state.playerdata_dir.clone();
            let session = state.ensure_player(Some("Alice"), uuid);
            session.health = 17.0;
            session.food_level = 13;
            session.food_saturation = 2.5;
            session.game_mode = 1;
            session.inventory[0] = ItemStack::new("minecraft:diamond", 3);
            session.inventory[5]
                .set_component(ComponentValue::CustomName(Tag::String("Bridge".into())));
            session.teleport(12.5, 70.0, -4.5, false);
            session.save(&dir).expect("save playerdata");
        }

        let mut reloaded = BridgeState::new(&root);
        let session = reloaded.ensure_player(None, uuid);
        assert_eq!(session.name, "Alice");
        assert_eq!(session.health, 17.0);
        assert_eq!(session.food_level, 13);
        assert_eq!(session.food_saturation, 2.5);
        assert_eq!(session.game_mode, 1);
        assert_eq!(session.inventory[0], ItemStack::new("minecraft:diamond", 3));
        assert_eq!(session.x, 12.5);
        assert_eq!(session.y, 70.0);
        assert_eq!(session.z, -4.5);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_config_round_trips_through_the_rust_file() {
        let root = temp_dir("config");
        let state = BridgeState::new(&root);
        assert!(state.read_plugin_config("HelloPlugin").is_empty());
        let contents = "motd: \"Hello from Rust\"\nmax-homes: 3\ndebug: false\n";
        assert!(state.write_plugin_config("HelloPlugin", contents));
        assert_eq!(state.read_plugin_config("HelloPlugin"), contents);
        assert_eq!(
            state.plugin_config_path("HelloPlugin"),
            root.join("run").join("HelloPlugin").join("config.yml")
        );

        assert!(state.write_plugin_config("../escape", "x"));
        assert_eq!(state.read_plugin_config("../escape"), "x");
        assert_eq!(safe_plugin_dir("../escape"), "escape");
        assert_eq!(safe_plugin_dir(".."), "plugin");
        assert_eq!(safe_plugin_dir(""), "plugin");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn permission_attachments_are_rust_owned() {
        let root = temp_dir("permissions");
        let mut state = BridgeState::new(&root);
        let uuid = [0x22; 16];
        state.ensure_player(Some("PermProbe"), uuid);

        assert!(!state.permission_is_set(uuid, "bridge.test"));
        assert!(!state.permission_has(uuid, "bridge.test", false));
        assert!(state.permission_has(uuid, "bridge.test", true));

        let first = state.add_permission_attachment(uuid, "HelloPlugin");
        let second = state.add_permission_attachment(uuid, "OtherPlugin");
        assert!(first > 0 && second > first);

        assert!(state.set_permission(uuid, first, "Bridge.Test", true));
        assert!(state.permission_is_set(uuid, "bridge.test"));
        assert!(state.permission_has(uuid, "bridge.test", false));
        assert_eq!(
            state.effective_permissions(uuid),
            BTreeMap::from([("bridge.test".to_owned(), true)])
        );

        assert!(state.set_permission(uuid, second, "bridge.test", false));
        assert!(!state.permission_has(uuid, "bridge.test", true));
        assert!(state.unset_permission(uuid, second, "bridge.test"));
        assert!(state.permission_has(uuid, "bridge.test", true));

        assert!(state.remove_permission_attachment(uuid, first));
        assert!(!state.remove_permission_attachment(uuid, first));
        assert!(!state.permission_is_set(uuid, "bridge.test"));
        assert!(!state.permission_has(uuid, "bridge.test", false));

        assert!(!state.set_permission(uuid, first, "bridge.test", true));
        assert!(!state.unset_permission(uuid, first, "bridge.test"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn world_state_persists_time_spawn_and_weather() {
        let root = temp_dir("worldstate");
        let mut state = BridgeState::new(&root);
        assert_eq!(state.world_state.spawn, (0.5, 64.0, 0.5));
        world_set_full_time(&mut state, 18_000);
        assert!(world_set_spawn(&mut state, 12.5, 70.0, -4.5));
        world_set_storm(&mut state, true);
        world_set_thundering(&mut state, true);
        state.world_state.weather_duration = 120;
        state.world_state.thunder_duration = 80;
        state.world_state.clear_weather_duration = 40;
        state.save_world_state();

        let reloaded = BridgeState::new(&root);
        assert_eq!(world_full_time(&reloaded), 18_000);
        assert_eq!(reloaded.world_state.spawn, (12.5, 70.0, -4.5));
        assert!(reloaded.world_state.storm);
        assert!(reloaded.world_state.thundering);
        assert_eq!(reloaded.world_state.weather_duration, 120);
        assert_eq!(reloaded.world_state.thunder_duration, 80);
        assert_eq!(reloaded.world_state.clear_weather_duration, 40);
        let mut invalid = BridgeState::new(&root);
        assert!(!world_set_spawn(&mut invalid, f64::NAN, 0.0, 0.0));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn day_time_matches_the_overworld_timeline() {
        assert!(is_day_time(0));
        assert!(is_day_time(6_000));
        assert!(is_day_time(12_522));
        assert!(!is_day_time(12_523));
        assert!(!is_day_time(18_000));
        assert!(!is_day_time(23_477));
        assert!(is_day_time(23_478));
        assert!(is_day_time(-1));
        assert!(is_day_time(24_000));
    }

    #[test]
    fn block_reads_and_writes_use_world_storage() {
        let root = temp_dir("world");
        let mut storage = WorldStorage::for_dimension(root.join("world"), Dimension::Overworld);
        let mut chunk = chunk::Chunk {
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        chunk.set_block_state(
            0,
            64,
            0,
            BlockState {
                name: "minecraft:stone".into(),
                properties: Vec::new(),
            },
        );
        storage
            .save_chunk(&chunk, Compression::Zlib)
            .expect("save chunk");

        assert_eq!(
            block_name_in(&mut storage, 0, 64, 0).as_deref(),
            Some("minecraft:stone")
        );
        assert_eq!(
            block_name_in(&mut storage, 0, 65, 0).as_deref(),
            Some("minecraft:air")
        );
        assert_eq!(highest_block_y_in(&mut storage, 0, 0), Some(64));
        assert_eq!(
            biome_in(&mut storage, 0, 64, 0).as_deref(),
            Some("minecraft:plains")
        );
        assert!(light_in(&mut storage, 0, 65, 0, true).is_some());

        assert!(set_block_in(&mut storage, 0, 64, 0, "minecraft:gold_block"));
        assert_eq!(
            block_name_in(&mut storage, 0, 64, 0).as_deref(),
            Some("minecraft:gold_block")
        );
        assert_eq!(
            block_state_for_material("gold_block").name,
            "minecraft:gold_block"
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
