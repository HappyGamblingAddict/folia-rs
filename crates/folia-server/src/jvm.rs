use anyhow::{Context, Result, bail};
use folia_core::chunk::{self, BlockState};
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::nbt::Tag;
use folia_core::player::PlayerSession;
use folia_core::world::{self, WorldStorage};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jdouble, jint, jlong, jstring};
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM, NativeMethod};
use std::collections::HashMap;
use std::ffi::c_void;
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

struct BridgeState {
    world_root: PathBuf,
    playerdata_dir: PathBuf,
    players: HashMap<[u8; 16], PlayerSession>,
}

impl BridgeState {
    fn new(root: &Path) -> Self {
        let properties = folia_core::config::ServerProperties::read(root.join("server.properties"))
            .unwrap_or_else(|_| folia_core::config::ServerProperties::default());
        let level_name =
            std::env::var("FOLIA_LEVEL_NAME").unwrap_or_else(|_| properties.level_name.clone());
        Self {
            world_root: root.join(&level_name),
            playerdata_dir: root.join("run").join("playerdata"),
            players: HashMap::new(),
        }
    }

    fn spawn(&self) -> ((f64, f64, f64), i32) {
        let level = folia_core::level::read(self.world_root.join("level.dat")).ok();
        let (x, y, z) = level
            .as_ref()
            .and_then(|level| level.spawn)
            .unwrap_or((0, 64, 0));
        let game_mode = level
            .as_ref()
            .map_or(0, |level| level.game_type.clamp(0, 3));
        (
            (f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5),
            game_mode,
        )
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
    let mut encoded = format!(
        "{}\t{}\t{}\t{}\t{}",
        stack.item,
        stack.count,
        damage,
        hex_encode(&display_name),
        u8::from(unbreakable)
    );
    for line in lore {
        encoded.push('\t');
        encoded.push_str(&hex_encode(&line));
    }
    encoded
}

fn decode_stack(encoded: &str) -> ItemStack {
    if encoded.is_empty() {
        return ItemStack::empty();
    }
    let mut fields = encoded.split('\t');
    let item = fields.next().unwrap_or_default();
    if item.is_empty() {
        return ItemStack::empty();
    }
    let count = fields
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let damage = fields
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    let display_name = fields.next().and_then(hex_decode).unwrap_or_default();
    let unbreakable = fields.next().is_some_and(|value| value == "1");
    let lore: Vec<Tag> = fields
        .filter_map(hex_decode)
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
    let accepted = with_bridge(|state| {
        let dir = state.playerdata_dir.clone();
        let session = state.player(uuid);
        session.teleport(x, y, z, false);
        session.yaw = yaw;
        session.pitch = pitch;
        session.physics.yaw = yaw;
        session.physics.pitch = pitch;
        println!(
            "[bridge] {} teleported to ({x:.2}, {y:.2}, {z:.2}); live session sync is pending",
            session.name
        );
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
    println!("[bridge] -> {name}: {message}");
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
        assert!(encode_stack(&ItemStack::empty()).is_empty());
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
