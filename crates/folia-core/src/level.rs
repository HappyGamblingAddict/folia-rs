//! `level.dat` parsing.
//!
//! Mirrors the parts of Minecraft's `PrimaryLevelData`, `LevelVersion` and
//! `WorldGenSettings` that the Rust port needs. Parsing is deliberately
//! tolerant: absent or mistyped fields fall back to vanilla's defaults rather
//! than failing the whole world load.

use crate::nbt::{self, Tag};
use std::fs;
use std::io;
use std::path::Path;

/// `GameType.CREATIVE` id, used by vanilla as the default for `allowCommands`.
const CREATIVE_GAME_TYPE: i32 = 1;

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
