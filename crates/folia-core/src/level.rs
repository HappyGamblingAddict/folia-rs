//! `level.dat` parsing and creation.
//!
//! Mirrors the parts of Minecraft's `PrimaryLevelData`, `LevelVersion` and
//! `WorldGenSettings` that the Rust port needs. Parsing is deliberately
//! tolerant: absent or mistyped fields fall back to vanilla's defaults rather
//! than failing the whole world load.

use crate::game_rules::GameRules;
use crate::nbt::{self, Tag};
use std::fs;
use std::io;
use std::path::Path;

/// `GameType.CREATIVE` id, used by vanilla as the default for `allowCommands`.
const CREATIVE_GAME_TYPE: i32 = 1;

/// `DataVersion` of the target version (Minecraft 1.21.11).
pub const DATA_VERSION: i32 = 4788;

/// Target Minecraft version, written into `Version.Name`.
pub const TARGET_VERSION: &str = "1.21.11";

/// Marker written into `Data` when this server creates a level. A level that
/// has it belongs to this server, so unstamped chunks beside it are leftovers
/// rather than another server's terrain.
pub const OWN_LEVEL_MARKER: &str = "folia:written";

/// The subset of a world's `level.dat` used by the Rust port.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LevelData {
    /// `DataVersion` (Minecraft's world data version).
    pub data_version: i32,
    /// `LevelName`.
    pub level_name: String,
    /// `GameType` id (`0` survival, `1` creative, `2` adventure, `3` spectator).
    pub game_type: i32,
    /// World spawn: `spawn.pos`, or legacy `SpawnX`/`SpawnY`/`SpawnZ`.
    pub spawn: Option<(i32, i32, i32)>,
    /// `DayTime`, falling back to `Time` when absent.
    pub day_time: i64,
    /// `LastPlayed` (epoch milliseconds).
    pub last_played: i64,
    /// `Version.Name`, when a `Version` compound is present.
    pub version_name: Option<String>,
    /// `Version.Id`, when present.
    pub version_id: Option<i32>,
    /// `WorldGenSettings.seed`, when present.
    pub seed: Option<i64>,
    /// `hardcore`.
    pub hardcore: bool,
    /// `allowCommands`; defaults to `true` for creative worlds, like vanilla.
    pub allow_commands: bool,
    /// The modeled rules from `Data.GameRules`.
    pub game_rules: GameRules,
    /// Whether this server created the level (see [`OWN_LEVEL_MARKER`]).
    pub own_level: bool,
}

/// Parse a gzip-compressed `level.dat` image.
pub fn parse(bytes: &[u8]) -> io::Result<LevelData> {
    let root = nbt::read_gzip_root(bytes)?;
    parse_root(&root)
}

/// Read and parse a `level.dat` file from disk.
pub fn read(path: impl AsRef<Path>) -> io::Result<LevelData> {
    let bytes = fs::read(path)?;
    parse(&bytes)
}

/// Parse an already-decoded NBT root.
///
/// The root must be a compound containing a `Data` compound; every other
/// field is optional and uses vanilla defaults when missing.
pub fn parse_root(root: &Tag) -> io::Result<LevelData> {
    let root = root
        .as_compound()
        .ok_or_else(|| invalid("level.dat root is not a compound"))?;
    let data = field(root, "Data")
        .and_then(Tag::as_compound)
        .ok_or_else(|| invalid("level.dat is missing the Data compound"))?;

    let data_version = field(data, "DataVersion")
        .and_then(Tag::as_int)
        .unwrap_or(0);
    let level_name = field(data, "LevelName")
        .and_then(Tag::as_str)
        .unwrap_or_default()
        .to_owned();
    let game_type = field(data, "GameType").and_then(Tag::as_int).unwrap_or(0);
    let time = field(data, "Time").and_then(Tag::as_long).unwrap_or(0);
    let day_time = field(data, "DayTime")
        .and_then(Tag::as_long)
        .unwrap_or(time);
    let last_played = field(data, "LastPlayed")
        .and_then(Tag::as_long)
        .unwrap_or(0);
    let spawn = parse_spawn(data);
    let (version_name, version_id) = parse_version(data);
    let seed = field(data, "WorldGenSettings")
        .and_then(|settings| settings.get("seed"))
        .and_then(Tag::as_long);
    let hardcore = field(data, "hardcore").is_some_and(is_truthy);
    let allow_commands = field(data, "allowCommands")
        .map(is_truthy)
        .unwrap_or(game_type == CREATIVE_GAME_TYPE);
    let own_level = field(data, OWN_LEVEL_MARKER).is_some_and(is_truthy);
    let game_rules = field(data, "GameRules")
        .map(GameRules::from_nbt)
        .unwrap_or_default();

    Ok(LevelData {
        data_version,
        level_name,
        game_type,
        spawn,
        day_time,
        last_played,
        version_name,
        version_id,
        seed,
        hardcore,
        allow_commands,
        game_rules,
        own_level,
    })
}

fn parse_spawn(data: &[(String, Tag)]) -> Option<(i32, i32, i32)> {
    if let Some(pos) = field(data, "spawn")
        .and_then(|spawn| spawn.get("pos"))
        .and_then(integer_triple)
    {
        return Some(pos);
    }

    // Pre-1.21.9 worlds store the spawn as three top-level ints.
    let x = field(data, "SpawnX").and_then(Tag::as_int)?;
    let y = field(data, "SpawnY").and_then(Tag::as_int)?;
    let z = field(data, "SpawnZ").and_then(Tag::as_int)?;
    Some((x, y, z))
}

fn parse_version(data: &[(String, Tag)]) -> (Option<String>, Option<i32>) {
    let Some(version) = field(data, "Version").and_then(Tag::as_compound) else {
        return (None, None);
    };
    let name = field(version, "Name")
        .and_then(Tag::as_str)
        .map(str::to_owned);
    let id = field(version, "Id").and_then(Tag::as_int);
    (name, id)
}

fn field<'a>(entries: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    entries
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// Decode the three integer components of a position tag.
fn integer_triple(tag: &Tag) -> Option<(i32, i32, i32)> {
    match tag {
        Tag::IntArray(values) if values.len() >= 3 => Some((values[0], values[1], values[2])),
        Tag::List(values) if values.len() >= 3 => Some((
            values[0].as_int()?,
            values[1].as_int()?,
            values[2].as_int()?,
        )),
        _ => None,
    }
}

/// Build the `level.dat` root for a fresh survival overworld at `seed` with the
/// given block spawn point.
///
/// Only the fields the port and vanilla's world loader need are written; the
/// generator is the standard multi-noise overworld, so a world created here has
/// the same terrain as a default world for that seed.
pub fn new_world_root(level_name: &str, seed: i64, spawn: (i32, i32, i32)) -> Tag {
    let (spawn_x, spawn_y, spawn_z) = spawn;
    let data = Tag::Compound(vec![
        ("DataVersion".into(), Tag::Int(DATA_VERSION)),
        (
            "Version".into(),
            Tag::Compound(vec![
                ("Id".into(), Tag::Int(DATA_VERSION)),
                ("Name".into(), Tag::String(TARGET_VERSION.into())),
                ("Series".into(), Tag::String("main".into())),
                ("Snapshot".into(), Tag::Byte(0)),
            ]),
        ),
        ("LevelName".into(), Tag::String(level_name.to_owned())),
        ("GameType".into(), Tag::Int(0)),
        ("Difficulty".into(), Tag::Byte(2)),
        ("DifficultyLocked".into(), Tag::Byte(0)),
        ("hardcore".into(), Tag::Byte(0)),
        ("allowCommands".into(), Tag::Byte(0)),
        ("GameRules".into(), GameRules::default().to_nbt()),
        ("initialized".into(), Tag::Byte(1)),
        ("Time".into(), Tag::Long(0)),
        ("DayTime".into(), Tag::Long(0)),
        ("LastPlayed".into(), Tag::Long(now_millis())),
        (
            "spawn".into(),
            Tag::Compound(vec![
                ("pos".into(), Tag::IntArray(vec![spawn_x, spawn_y, spawn_z])),
                ("angle".into(), Tag::Float(0.0)),
                (
                    "dimension".into(),
                    Tag::String("minecraft:overworld".into()),
                ),
            ]),
        ),
        (
            "WorldGenSettings".into(),
            Tag::Compound(vec![
                ("seed".into(), Tag::Long(seed)),
                ("generate_features".into(), Tag::Byte(1)),
                ("bonus_chest".into(), Tag::Byte(0)),
                (
                    "dimensions".into(),
                    Tag::Compound(vec![(
                        "minecraft:overworld".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String("minecraft:overworld".into())),
                            (
                                "generator".into(),
                                Tag::Compound(vec![
                                    ("type".into(), Tag::String("minecraft:noise".into())),
                                    (
                                        "biome_source".into(),
                                        Tag::Compound(vec![
                                            (
                                                "type".into(),
                                                Tag::String("minecraft:multi_noise".into()),
                                            ),
                                            (
                                                "preset".into(),
                                                Tag::String("minecraft:overworld".into()),
                                            ),
                                        ]),
                                    ),
                                    ("settings".into(), Tag::String("minecraft:overworld".into())),
                                ]),
                            ),
                        ]),
                    )]),
                ),
            ]),
        ),
        (OWN_LEVEL_MARKER.into(), Tag::Byte(1)),
    ]);
    Tag::Compound(vec![("Data".into(), data)])
}

/// Write a new `level.dat` at `path`, replacing any previous file.
///
/// The image is written to a temporary file and renamed, so an interrupted
/// write cannot leave a half-written level behind.
pub fn write_new_world(
    path: impl AsRef<Path>,
    level_name: &str,
    seed: i64,
    spawn: (i32, i32, i32),
) -> io::Result<()> {
    write_root(path.as_ref(), &new_world_root(level_name, seed, spawn))
}

/// Persist the parts of the level state this run changed.
///
/// `elapsed_ticks` is added to `Time` and `DayTime` (this server only advances
/// game time while its loop runs), `LastPlayed` is stamped with the current
/// time, and `spawn.pos` is replaced when a spawn is given. Every other field
/// in the file, including ones this port does not model, is preserved: the
/// whole root is rewritten from what was read.
pub fn record_session(
    path: impl AsRef<Path>,
    elapsed_ticks: u64,
    spawn: Option<(i32, i32, i32)>,
) -> io::Result<LevelData> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let mut root = nbt::read_gzip_root(&bytes)?;
    let data = data_mut(&mut root)?;

    let ticks = i64::try_from(elapsed_ticks).unwrap_or(i64::MAX);
    let time = data
        .iter()
        .find(|(key, _)| key == "Time")
        .and_then(|(_, tag)| tag.as_long())
        .unwrap_or(0)
        .saturating_add(ticks);
    set_long(data, "Time", time);
    if let Some((_, tag)) = data.iter_mut().find(|(key, _)| key == "DayTime")
        && let Tag::Long(day_time) = tag
    {
        *day_time = day_time.saturating_add(ticks);
    }
    set_long(data, "LastPlayed", now_millis());
    if let Some((x, y, z)) = spawn
        && let Some((_, Tag::Compound(spawn_data))) =
            data.iter_mut().find(|(key, _)| key == "spawn")
        && let Some((_, tag)) = spawn_data.iter_mut().find(|(key, _)| key == "pos")
        && let Tag::IntArray(pos) = tag
        && pos.len() == 3
    {
        pos[0] = x;
        pos[1] = y;
        pos[2] = z;
    }

    let level = parse_root(&root)?;
    write_root(path, &root)?;
    Ok(level)
}

/// Runtime state a running server writes back to `level.dat` without waiting
/// for a clean shutdown.
#[derive(Debug, Clone, Default)]
pub struct RuntimeChange {
    /// Replace `spawn.pos` with this block position.
    pub spawn: Option<(i32, i32, i32)>,
    /// Merge the modeled rules into `Data.GameRules`.
    pub game_rules: Option<GameRules>,
}

/// Atomically apply runtime changes to `level.dat` and return the new state.
///
/// The whole root is read back from disk and rewritten, so fields this port
/// does not model (including game rules it does not implement) survive, and
/// the previous file is kept as `level.dat_old`. The temp-file rename in
/// [`write_root`] means a crash cannot leave a half-written level behind.
pub fn persist_runtime_state(
    path: impl AsRef<Path>,
    change: RuntimeChange,
) -> io::Result<LevelData> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let mut root = nbt::read_gzip_root(&bytes)?;
    {
        let data = data_mut(&mut root)?;
        if let Some(rules) = &change.game_rules {
            apply_game_rules(data, rules);
        }
        if let Some(spawn) = change.spawn {
            apply_spawn(data, spawn);
        }
    }
    let level = parse_root(&root)?;
    write_root(path, &root)?;
    Ok(level)
}

/// Persist the current in-memory rule set to `level.dat`.
pub fn persist_game_rules(path: impl AsRef<Path>, rules: &GameRules) -> io::Result<LevelData> {
    persist_runtime_state(
        path,
        RuntimeChange {
            game_rules: Some(rules.clone()),
            ..RuntimeChange::default()
        },
    )
}

/// Persist a changed world spawn to `level.dat`.
pub fn set_spawn(path: impl AsRef<Path>, spawn: (i32, i32, i32)) -> io::Result<LevelData> {
    persist_runtime_state(
        path,
        RuntimeChange {
            spawn: Some(spawn),
            ..RuntimeChange::default()
        },
    )
}

fn data_mut(root: &mut Tag) -> io::Result<&mut Vec<(String, Tag)>> {
    let Tag::Compound(entries) = root else {
        return Err(invalid("level.dat root is not a compound"));
    };
    entries
        .iter_mut()
        .find(|(key, _)| key == "Data")
        .and_then(|(_, tag)| match tag {
            Tag::Compound(data) => Some(data),
            _ => None,
        })
        .ok_or_else(|| invalid("level.dat is missing the Data compound"))
}

/// Merge only the modeled rules, leaving rules this port does not implement
/// untouched inside the `GameRules` compound.
fn apply_game_rules(data: &mut Vec<(String, Tag)>, rules: &GameRules) {
    let encoded = rules.to_nbt();
    let Some(new_entries) = encoded.as_compound() else {
        return;
    };
    let existing = data
        .iter()
        .position(|(key, _)| key == "GameRules")
        .map(|index| std::mem::replace(&mut data[index].1, Tag::End))
        .unwrap_or(Tag::Compound(Vec::new()));
    let mut merged = match existing {
        Tag::Compound(entries) => entries,
        _ => Vec::new(),
    };
    for (name, value) in new_entries {
        set_entry(&mut merged, name, value.clone());
    }
    set_entry(data, "GameRules", Tag::Compound(merged));
}

/// Replace `spawn.pos` in place, creating the spawn compound when a level does
/// not have one (legacy worlds only carry `SpawnX`/`SpawnY`/`SpawnZ`).
fn apply_spawn(data: &mut Vec<(String, Tag)>, (x, y, z): (i32, i32, i32)) {
    let index = data
        .iter()
        .position(|(key, _)| key == "spawn")
        .unwrap_or_else(|| {
            data.push(("spawn".into(), Tag::End));
            data.len() - 1
        });
    let existing = std::mem::replace(&mut data[index].1, Tag::End);
    let mut spawn = match existing {
        Tag::Compound(entries) => entries,
        _ => Vec::new(),
    };
    set_entry(&mut spawn, "pos", Tag::IntArray(vec![x, y, z]));
    if !spawn.iter().any(|(key, _)| key == "dimension") {
        set_entry(
            &mut spawn,
            "dimension",
            Tag::String("minecraft:overworld".into()),
        );
    }
    if !spawn.iter().any(|(key, _)| key == "angle") {
        set_entry(&mut spawn, "angle", Tag::Float(0.0));
    }
    data[index].1 = Tag::Compound(spawn);
}

fn set_entry(entries: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    match entries.iter_mut().find(|(key, _)| key == name) {
        Some((_, tag)) => *tag = value,
        None => entries.push((name.to_owned(), value)),
    }
}

fn set_long(entries: &mut Vec<(String, Tag)>, name: &str, value: i64) {
    set_entry(entries, name, Tag::Long(value));
}

fn write_root(path: &Path, root: &Tag) -> io::Result<()> {
    let bytes = nbt::write_gzip_root(root)?;
    let temp = path.with_extension("dat.tmp");
    fs::write(&temp, bytes)?;
    if path.exists() {
        // Vanilla keeps the previous level file so a bad write can be undone.
        fs::copy(path, path.with_extension("dat_old"))?;
    }
    fs::rename(&temp, path)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// NBT booleans are bytes, but old or third-party writers sometimes use other
/// numeric tags; accept those too.
fn is_truthy(tag: &Tag) -> bool {
    match tag {
        Tag::Byte(value) => *value != 0,
        Tag::Short(value) => *value != 0,
        Tag::Int(value) => *value != 0,
        Tag::Long(value) => *value != 0,
        Tag::String(value) => value.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(data: Vec<(&str, Tag)>) -> Tag {
        Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(
                data.into_iter()
                    .map(|(name, tag)| (name.to_owned(), tag))
                    .collect(),
            ),
        )])
    }

    #[test]
    fn parses_complete_level_dat() {
        let root = root_with(vec![
            ("DataVersion", Tag::Int(4189)),
            ("LevelName", Tag::String("Folia Rust".into())),
            ("GameType", Tag::Int(1)),
            ("hardcore", Tag::Byte(1)),
            ("allowCommands", Tag::Byte(0)),
            (
                "spawn",
                Tag::Compound(vec![
                    ("pos".into(), Tag::IntArray(vec![10, 64, -20])),
                    ("yaw".into(), Tag::Float(90.0)),
                    ("pitch".into(), Tag::Float(0.0)),
                ]),
            ),
            ("DayTime", Tag::Long(123_456)),
            ("LastPlayed", Tag::Long(1_700_000_000_000)),
            (
                "Version",
                Tag::Compound(vec![
                    ("Name".into(), Tag::String("1.21.11".into())),
                    ("Id".into(), Tag::Int(4325)),
                    ("Snapshot".into(), Tag::Byte(0)),
                ]),
            ),
            (
                "WorldGenSettings",
                Tag::Compound(vec![
                    ("seed".into(), Tag::Long(-1_234_567_890_123)),
                    ("generate_features".into(), Tag::Byte(1)),
                ]),
            ),
        ]);

        let bytes = nbt::write_gzip_root(&root).unwrap();
        let level = parse(&bytes).unwrap();

        assert_eq!(level.data_version, 4189);
        assert_eq!(level.level_name, "Folia Rust");
        assert_eq!(level.game_type, 1);
        assert_eq!(level.spawn, Some((10, 64, -20)));
        assert_eq!(level.day_time, 123_456);
        assert_eq!(level.last_played, 1_700_000_000_000);
        assert_eq!(level.version_name.as_deref(), Some("1.21.11"));
        assert_eq!(level.version_id, Some(4325));
        assert_eq!(level.seed, Some(-1_234_567_890_123));
        assert!(level.hardcore);
        assert!(!level.allow_commands);
    }

    #[test]
    fn missing_optional_fields_use_defaults() {
        let root = root_with(vec![
            ("DataVersion", Tag::Int(3955)),
            ("LevelName", Tag::String("Minimal".into())),
        ]);

        let level = parse_root(&root).unwrap();

        assert_eq!(level.data_version, 3955);
        assert_eq!(level.level_name, "Minimal");
        assert_eq!(level.game_type, 0);
        assert_eq!(level.spawn, None);
        assert_eq!(level.day_time, 0);
        assert_eq!(level.last_played, 0);
        assert_eq!(level.version_name, None);
        assert_eq!(level.version_id, None);
        assert_eq!(level.seed, None);
        assert!(!level.hardcore);
        assert!(!level.allow_commands);
    }

    #[test]
    fn extracts_nested_worldgen_seed_and_version_name() {
        let root = root_with(vec![
            (
                "Version",
                Tag::Compound(vec![
                    ("Name".into(), Tag::String("1.20.4".into())),
                    ("Id".into(), Tag::Int(3700)),
                ]),
            ),
            (
                "WorldGenSettings",
                Tag::Compound(vec![("seed".into(), Tag::Long(42))]),
            ),
        ]);

        let level = parse_root(&root).unwrap();

        assert_eq!(level.version_name.as_deref(), Some("1.20.4"));
        assert_eq!(level.version_id, Some(3700));
        assert_eq!(level.seed, Some(42));
    }

    #[test]
    fn falls_back_to_legacy_spawn_and_game_time() {
        let root = root_with(vec![
            ("SpawnX", Tag::Int(-5)),
            ("SpawnY", Tag::Int(70)),
            ("SpawnZ", Tag::Int(12)),
            ("Time", Tag::Long(9_000)),
            ("GameType", Tag::Int(3)),
        ]);

        let level = parse_root(&root).unwrap();

        assert_eq!(level.spawn, Some((-5, 70, 12)));
        assert_eq!(level.day_time, 9_000);
        assert!(!level.allow_commands);
    }

    #[test]
    fn allow_commands_defaults_to_creative_like_vanilla() {
        let root = root_with(vec![("GameType", Tag::Int(CREATIVE_GAME_TYPE))]);

        let level = parse_root(&root).unwrap();

        assert!(level.allow_commands);
    }

    #[test]
    fn written_level_round_trips_and_marks_the_level_as_ours() {
        let root = new_world_root("Rust World", -1_234_567_890_123, (8, 71, -4));
        let level = parse_root(&root).unwrap();

        assert_eq!(level.data_version, DATA_VERSION);
        assert_eq!(level.level_name, "Rust World");
        assert_eq!(level.seed, Some(-1_234_567_890_123));
        assert_eq!(level.spawn, Some((8, 71, -4)));
        assert_eq!(level.version_name.as_deref(), Some(TARGET_VERSION));
        assert_eq!(level.version_id, Some(DATA_VERSION));
        assert_eq!(level.game_type, 0);
        assert!(!level.hardcore);
        assert!(!level.allow_commands);
        assert!(level.own_level, "the marker identifies a level we created");

        let generator = root
            .get("Data")
            .and_then(|data| data.get("WorldGenSettings"))
            .and_then(|settings| settings.get("dimensions"))
            .and_then(|dimensions| dimensions.get("minecraft:overworld"))
            .and_then(|overworld| overworld.get("generator"))
            .expect("overworld generator");
        assert_eq!(
            generator.get("settings").and_then(Tag::as_str),
            Some("minecraft:overworld")
        );
    }

    #[test]
    fn foreign_levels_are_not_marked_as_ours() {
        let root = root_with(vec![("DataVersion", Tag::Int(4189))]);
        assert!(!parse_root(&root).unwrap().own_level);
    }

    #[test]
    fn writes_a_level_file_and_replaces_it_atomically() {
        let directory = std::env::temp_dir().join(format!(
            "folia-rs-level-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("level.dat");

        write_new_world(&path, "First", 7, (0, 64, 0)).unwrap();
        assert_eq!(read(&path).unwrap().level_name, "First");

        write_new_world(&path, "Second", 9, (0, 70, 0)).unwrap();
        let level = read(&path).unwrap();
        assert_eq!(level.level_name, "Second");
        assert_eq!(level.seed, Some(9));
        assert_eq!(level.spawn, Some((0, 70, 0)));
        assert!(
            !directory.join("level.dat.tmp").exists(),
            "the temporary image is renamed into place"
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn recording_a_session_updates_time_and_preserves_unknown_fields() {
        let directory = std::env::temp_dir().join(format!(
            "folia-rs-level-session-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("level.dat");

        let mut root = new_world_root("Session", 5, (0, 70, 0));
        // A field this port does not model, like vanilla's DataPacks or Player.
        if let Tag::Compound(entries) = &mut root
            && let Some((_, Tag::Compound(data))) =
                entries.iter_mut().find(|(key, _)| key == "Data")
        {
            data.push(("DataPacks".into(), Tag::String("keep me".into())));
        }
        write_root(&path, &root).unwrap();

        let level = record_session(&path, 1_200, Some((4, 80, -4))).unwrap();
        assert_eq!(level.day_time, 1_200, "elapsed ticks advance the day time");
        assert_eq!(level.spawn, Some((4, 80, -4)));
        assert!(level.last_played > 0, "LastPlayed is stamped");
        assert_eq!(
            crate::level::read(directory.join("level.dat_old"))
                .unwrap()
                .day_time,
            0,
            "the previous level file is kept as level.dat_old"
        );

        let reread_root = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            reread_root
                .get("Data")
                .and_then(|data| data.get("DataPacks")),
            Some(&Tag::String("keep me".into())),
            "unknown fields survive the rewrite"
        );
        assert_eq!(
            reread_root
                .get("Data")
                .and_then(|data| data.get("Time"))
                .and_then(Tag::as_long),
            Some(1_200)
        );

        let again = record_session(&path, 300, None).unwrap();
        assert_eq!(again.day_time, 1_500, "time accumulates across sessions");
        assert_eq!(again.spawn, Some((4, 80, -4)), "no spawn means no change");
        assert_eq!(
            crate::level::read(directory.join("level.dat_old"))
                .unwrap()
                .day_time,
            1_200,
            "the backup holds the previous session's time"
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn runtime_change_persists_rules_and_spawn_without_a_shutdown() {
        let directory = std::env::temp_dir().join(format!(
            "folia-rs-level-runtime-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("level.dat");
        write_new_world(&path, "Runtime", 11, (0, 64, 0)).unwrap();

        let rules = GameRules {
            keep_inventory: true,
            do_daylight_cycle: false,
            ..GameRules::default()
        };
        let changed = persist_runtime_state(
            &path,
            RuntimeChange {
                spawn: Some((-12, 71, 34)),
                game_rules: Some(rules.clone()),
            },
        )
        .unwrap();

        assert_eq!(changed.spawn, Some((-12, 71, 34)));
        assert_eq!(changed.game_rules, rules);

        let reloaded = read(&path).unwrap();
        assert_eq!(reloaded.spawn, Some((-12, 71, 34)));
        assert_eq!(
            reloaded.game_rules, rules,
            "rules survive a reload from disk"
        );
        assert_eq!(
            read(directory.join("level.dat_old")).unwrap().spawn,
            Some((0, 64, 0)),
            "the previous file is kept as level.dat_old"
        );
        assert!(!directory.join("level.dat.tmp").exists());

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn runtime_change_merges_unmodeled_rules_and_unknown_fields() {
        let directory = std::env::temp_dir().join(format!(
            "folia-rs-level-runtime-merge-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("level.dat");
        let mut root = new_world_root("Merge", 3, (0, 64, 0));
        if let Tag::Compound(entries) = &mut root
            && let Some((_, Tag::Compound(data))) =
                entries.iter_mut().find(|(key, _)| key == "Data")
        {
            set_entry(
                data,
                "GameRules",
                Tag::Compound(vec![
                    ("fallingBlocks".into(), Tag::String("true".into())),
                    ("keepInventory".into(), Tag::String("false".into())),
                ]),
            );
            set_entry(data, "DataPacks", Tag::String("keep me".into()));
        }
        write_root(&path, &root).unwrap();

        let rules = GameRules {
            keep_inventory: true,
            ..GameRules::default()
        };
        persist_game_rules(&path, &rules).unwrap();

        let on_disk = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
        let data = on_disk.get("Data").expect("Data compound");
        assert_eq!(
            data.get("DataPacks"),
            Some(&Tag::String("keep me".into())),
            "unknown fields survive"
        );
        let game_rules = data.get("GameRules").expect("GameRules compound");
        assert_eq!(
            game_rules.get("fallingBlocks"),
            Some(&Tag::String("true".into())),
            "rules this port does not model survive"
        );
        assert_eq!(
            game_rules.get("keepInventory"),
            Some(&Tag::String("true".into())),
            "the modeled rule is replaced with its vanilla string form"
        );
        assert_eq!(read(&path).unwrap().game_rules, rules);

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn set_spawn_creates_a_spawn_compound_for_legacy_levels() {
        let directory = std::env::temp_dir().join(format!(
            "folia-rs-level-legacy-spawn-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("level.dat");
        let mut root = new_world_root("Legacy", 1, (0, 64, 0));
        if let Tag::Compound(entries) = &mut root
            && let Some((_, Tag::Compound(data))) =
                entries.iter_mut().find(|(key, _)| key == "Data")
        {
            data.retain(|(key, _)| key != "spawn");
        }
        write_root(&path, &root).unwrap();

        let changed = set_spawn(&path, (7, 80, -9)).unwrap();
        assert_eq!(changed.spawn, Some((7, 80, -9)));
        let on_disk = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
        let spawn = on_disk
            .get("Data")
            .and_then(|data| data.get("spawn"))
            .expect("spawn compound");
        assert_eq!(spawn.get("pos"), Some(&Tag::IntArray(vec![7, 80, -9])));
        assert_eq!(
            spawn.get("dimension").and_then(Tag::as_str),
            Some("minecraft:overworld")
        );
        assert_eq!(read(&path).unwrap().game_rules, GameRules::default());

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn recording_a_session_without_a_level_file_reports_not_found() {
        let path = std::env::temp_dir().join(format!(
            "folia-rs-level-missing-{}-{}.dat",
            std::process::id(),
            now_millis()
        ));
        let error = record_session(&path, 20, None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn rejects_roots_without_data_compound() {
        let root = Tag::Compound(vec![("DataVersion".into(), Tag::Int(1))]);
        let error = parse_root(&root).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let root = Tag::String("not a compound".into());
        assert_eq!(
            parse_root(&root).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
