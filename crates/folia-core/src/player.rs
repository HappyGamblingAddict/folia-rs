use crate::inventory::{HOTBAR_SLOTS, INVENTORY_SLOTS, ItemStack, default_inventory};
use crate::nbt::{self, Tag};
use crate::player_physics::{
    ClientMove, MovementCorrection, PhysicsWorld, PlayerPhysics, Reconciliation,
};
use crate::protocol::PacketReader;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Playerdata fields this server parses and rewrites. Everything else in the
/// file is kept verbatim in [`PlayerSession::extra`] so a save never drops
/// fields it does not model (health, food, XP, ender chest, ...).
pub const KNOWN_PLAYERDATA_FIELDS: [&str; 20] = [
    "DataVersion",
    "Name",
    "UUID",
    "Pos",
    "Rotation",
    "OnGround",
    "Dimension",
    "SelectedItemSlot",
    "Inventory",
    "Cursor",
    "Health",
    "foodLevel",
    "foodSaturationLevel",
    "foodExhaustionLevel",
    "XpLevel",
    "XpP",
    "XpTotal",
    "Score",
    "Air",
    "playerGameType",
];

pub const SERVERBOUND_MOVE_POS: i32 = 29;
pub const SERVERBOUND_MOVE_POS_ROT: i32 = 30;
pub const SERVERBOUND_MOVE_ROT: i32 = 31;
pub const SERVERBOUND_MOVE_STATUS: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MovementUpdate {
    Position {
        x: f64,
        y: f64,
        z: f64,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    Rotation {
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    Status {
        on_ground: bool,
    },
}

fn parse_movement(packet: &[u8]) -> io::Result<Option<MovementUpdate>> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    let update = match id {
        SERVERBOUND_MOVE_POS => MovementUpdate::Position {
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: None,
            pitch: None,
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_POS_ROT => MovementUpdate::Position {
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: Some(reader.read_f32()?),
            pitch: Some(reader.read_f32()?),
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_ROT => MovementUpdate::Rotation {
            yaw: reader.read_f32()?,
            pitch: reader.read_f32()?,
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_STATUS => MovementUpdate::Status {
            on_ground: reader.read_u8()? & 1 != 0,
        },
        _ => return Ok(None),
    };
    Ok(Some(update))
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSession {
    pub name: String,
    pub uuid: [u8; 16],
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Vanilla game mode id (0 survival, 1 creative, 2 adventure, 3 spectator).
    /// `-1` means the file had no `playerGameType`; the join path fills in the
    /// server default before the session is used.
    pub game_mode: i32,
    pub selected_slot: i32,
    pub inventory: Vec<ItemStack>,
    pub cursor: ItemStack,
    /// Current health points; vanilla's survival default is 20.
    pub health: f32,
    /// `foodLevel`, 0-20.
    pub food_level: i32,
    /// `foodSaturationLevel`.
    pub food_saturation: f32,
    /// `foodExhaustionLevel`.
    pub food_exhaustion: f32,
    /// `XpLevel`.
    pub xp_level: i32,
    /// `XpP`, progress toward the next level (0.0-1.0).
    pub xp_progress: f32,
    /// `XpTotal`.
    pub xp_total: i32,
    /// `Score` (the death/scoreboard counter stored in playerdata).
    pub score: i32,
    /// `Air`, remaining breath ticks; vanilla's default is 300.
    pub air: i16,
    /// Server-authoritative movement state. `x`/`y`/`z` mirror its position.
    pub physics: PlayerPhysics,
    /// Playerdata fields this server does not model, preserved across saves.
    pub extra: Vec<(String, Tag)>,
}

/// Result of applying one movement packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementOutcome {
    pub was_movement: bool,
    pub position_changed: bool,
    pub correction: Option<MovementCorrection>,
    pub fall_damage: f32,
}

impl MovementOutcome {
    fn not_movement() -> Self {
        Self {
            was_movement: false,
            position_changed: false,
            correction: None,
            fall_damage: 0.0,
        }
    }

    fn from_reconciliation(reconciliation: Reconciliation) -> Self {
        Self {
            was_movement: true,
            position_changed: reconciliation.moved,
            correction: reconciliation.correction,
            fall_damage: reconciliation.fall_damage,
        }
    }
}

impl PlayerSession {
    pub fn new(name: String, uuid: [u8; 16], x: f64, y: f64, z: f64) -> Self {
        Self {
            name,
            uuid,
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            game_mode: 0,
            selected_slot: 0,
            inventory: default_inventory(),
            cursor: ItemStack::empty(),
            health: 20.0,
            food_level: 20,
            food_saturation: 5.0,
            food_exhaustion: 0.0,
            xp_level: 0,
            xp_progress: 0.0,
            xp_total: 0,
            score: 0,
            air: 300,
            physics: PlayerPhysics::new(x, y, z),
            extra: Vec::new(),
        }
    }

    /// Rebuild the simulation state from the session's position. Used after
    /// login settling, teleports, respawn, and playerdata loads so the first
    /// movement packet is measured from the accepted position.
    pub fn reset_physics(&mut self) {
        self.physics.teleport(self.x, self.y, self.z);
        self.physics.yaw = self.yaw;
        self.physics.pitch = self.pitch;
        self.physics.on_ground = self.on_ground;
    }

    /// Move the session and its simulation together, discarding velocity and
    /// fall distance like a vanilla teleport.
    pub fn teleport(&mut self, x: f64, y: f64, z: f64, on_ground: bool) {
        self.x = x;
        self.y = y;
        self.z = z;
        self.on_ground = on_ground;
        self.physics.teleport(x, y, z);
        self.physics.on_ground = on_ground;
    }

    pub fn selected_item(&self) -> Option<&ItemStack> {
        self.inventory
            .get(self.selected_slot.max(0) as usize)
            .filter(|stack| !stack.is_empty())
    }

    pub fn consume_selected(&mut self) -> bool {
        let Some(stack) = self.inventory.get_mut(self.selected_slot.max(0) as usize) else {
            return false;
        };
        if stack.is_empty() {
            return false;
        }
        stack.count -= 1;
        if stack.count <= 0 {
            *stack = ItemStack::empty();
        }
        true
    }

    /// Parse a serverbound play movement packet and reconcile its claimed
    /// position with the simulated player. Returns whether the packet was a
    /// movement packet, whether the simulated position changed, and any
    /// correction the caller must send back to the client.
    pub fn apply_movement(
        &mut self,
        packet: &[u8],
        world: &mut impl PhysicsWorld,
    ) -> io::Result<MovementOutcome> {
        let Some(update) = parse_movement(packet)? else {
            return Ok(MovementOutcome::not_movement());
        };
        match update {
            MovementUpdate::Position {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground,
            } => {
                let client = ClientMove {
                    x,
                    y,
                    z,
                    on_ground,
                    yaw,
                    pitch,
                };
                let reconciliation = self.physics.reconcile(world, &client);
                self.x = self.physics.x;
                self.y = self.physics.y;
                self.z = self.physics.z;
                self.yaw = self.physics.yaw;
                self.pitch = self.physics.pitch;
                self.on_ground = reconciliation.on_ground;
                Ok(MovementOutcome::from_reconciliation(reconciliation))
            }
            MovementUpdate::Rotation {
                yaw,
                pitch,
                on_ground,
            } => {
                self.yaw = yaw;
                self.pitch = pitch;
                self.on_ground = on_ground;
                self.physics.yaw = yaw;
                self.physics.pitch = pitch;
                self.physics.on_ground = on_ground;
                Ok(MovementOutcome {
                    was_movement: true,
                    position_changed: false,
                    correction: None,
                    fall_damage: 0.0,
                })
            }
            MovementUpdate::Status { on_ground } => {
                self.on_ground = on_ground;
                self.physics.on_ground = on_ground;
                Ok(MovementOutcome {
                    was_movement: true,
                    position_changed: false,
                    correction: None,
                    fall_damage: 0.0,
                })
            }
        }
    }

    pub fn save(&self, directory: impl AsRef<Path>) -> io::Result<PathBuf> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory)?;
        let path = Self::path(directory, &self.uuid);
        let inventory_entries = self
            .inventory
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(slot, stack)| {
                Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(slot as i32)),
                    ("id".into(), Tag::String(stack.item.clone())),
                    ("Count".into(), Tag::Int(stack.count)),
                ])
            })
            .collect();
        let cursor = if self.cursor.is_empty() {
            Tag::Compound(Vec::new())
        } else {
            Tag::Compound(vec![
                ("id".into(), Tag::String(self.cursor.item.clone())),
                ("Count".into(), Tag::Int(self.cursor.count)),
            ])
        };

        let mut entries = vec![
            ("DataVersion".into(), Tag::Int(crate::level::DATA_VERSION)),
            ("playerGameType".into(), Tag::Int(self.game_mode.max(0))),
            ("Health".into(), Tag::Float(self.health)),
            ("foodLevel".into(), Tag::Int(self.food_level)),
            (
                "foodSaturationLevel".into(),
                Tag::Float(self.food_saturation),
            ),
            (
                "foodExhaustionLevel".into(),
                Tag::Float(self.food_exhaustion),
            ),
            ("XpLevel".into(), Tag::Int(self.xp_level)),
            ("XpP".into(), Tag::Float(self.xp_progress)),
            ("XpTotal".into(), Tag::Int(self.xp_total)),
            ("Score".into(), Tag::Int(self.score)),
            ("Air".into(), Tag::Short(self.air)),
            ("Name".into(), Tag::String(self.name.clone())),
            ("UUID".into(), Tag::String(uuid_string(&self.uuid))),
            (
                "Pos".into(),
                Tag::List(vec![
                    Tag::Double(self.x),
                    Tag::Double(self.y),
                    Tag::Double(self.z),
                ]),
            ),
            (
                "Rotation".into(),
                Tag::List(vec![Tag::Float(self.yaw), Tag::Float(self.pitch)]),
            ),
            ("OnGround".into(), Tag::Byte(self.on_ground as i8)),
            (
                "Dimension".into(),
                Tag::String("minecraft:overworld".into()),
            ),
            ("SelectedItemSlot".into(), Tag::Int(self.selected_slot)),
            ("Inventory".into(), Tag::List(inventory_entries)),
            ("Cursor".into(), cursor),
        ];
        entries.extend(
            self.extra
                .iter()
                .filter(|(key, _)| !KNOWN_PLAYERDATA_FIELDS.contains(&key.as_str()))
                .cloned(),
        );
        let tag = Tag::Compound(entries);
        let bytes = nbt::write_gzip_root(&tag)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &path)?;
        Ok(path)
    }

    pub fn load(directory: impl AsRef<Path>, uuid: [u8; 16]) -> io::Result<Option<Self>> {
        let path = Self::path(directory.as_ref(), &uuid);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        let root = nbt::read_gzip_root(&bytes)?;

        let name = root
            .get("Name")
            .and_then(Tag::as_str)
            .unwrap_or("Player")
            .to_owned();
        let position = root
            .get("Pos")
            .and_then(Tag::as_list)
            .ok_or_else(|| invalid("playerdata is missing Pos"))?;
        let rotation = root
            .get("Rotation")
            .and_then(Tag::as_list)
            .ok_or_else(|| invalid("playerdata is missing Rotation"))?;

        let coordinate = |index: usize| {
            position
                .get(index)
                .and_then(Tag::as_double)
                .ok_or_else(|| invalid("playerdata has invalid Pos"))
        };
        let angle = |index: usize| {
            rotation
                .get(index)
                .and_then(Tag::as_float)
                .ok_or_else(|| invalid("playerdata has invalid Rotation"))
        };

        let selected_slot = root
            .get("SelectedItemSlot")
            .and_then(Tag::as_int)
            .unwrap_or(0)
            .clamp(0, (HOTBAR_SLOTS - 1) as i32);
        let mut inventory = default_inventory();
        if let Some(entries) = root.get("Inventory").and_then(Tag::as_list) {
            inventory = vec![ItemStack::empty(); INVENTORY_SLOTS];
            for entry in entries {
                let Some(slot) = entry.get("Slot").and_then(Tag::as_int) else {
                    continue;
                };
                if !(0..INVENTORY_SLOTS as i32).contains(&slot) {
                    continue;
                }
                let Some(item) = entry.get("id").and_then(Tag::as_str) else {
                    continue;
                };
                let count = entry.get("Count").and_then(Tag::as_int).unwrap_or(0);
                inventory[slot as usize] = ItemStack::new(item, count);
            }
        }
        let cursor = root
            .get("Cursor")
            .and_then(|tag| {
                let item = tag.get("id").and_then(Tag::as_str)?;
                let count = tag.get("Count").and_then(Tag::as_int).unwrap_or(0);
                Some(ItemStack::new(item, count))
            })
            .unwrap_or_else(ItemStack::empty);

        let health = root.get("Health").and_then(Tag::as_float).unwrap_or(20.0);
        let food_level = root.get("foodLevel").and_then(Tag::as_int).unwrap_or(20);
        let food_saturation = root
            .get("foodSaturationLevel")
            .and_then(Tag::as_float)
            .unwrap_or(5.0);
        let food_exhaustion = root
            .get("foodExhaustionLevel")
            .and_then(Tag::as_float)
            .unwrap_or(0.0);
        let xp_level = root.get("XpLevel").and_then(Tag::as_int).unwrap_or(0);
        let xp_progress = root.get("XpP").and_then(Tag::as_float).unwrap_or(0.0);
        let xp_total = root.get("XpTotal").and_then(Tag::as_int).unwrap_or(0);
        let score = root.get("Score").and_then(Tag::as_int).unwrap_or(0);
        let air = root
            .get("Air")
            .and_then(|tag| match tag {
                Tag::Short(value) => Some(*value),
                Tag::Int(value) => i16::try_from(*value).ok(),
                _ => None,
            })
            .unwrap_or(300);
        let game_mode = root
            .get("playerGameType")
            .and_then(Tag::as_int)
            .unwrap_or(-1)
            .clamp(-1, 3);
        let x = coordinate(0)?;
        let y = coordinate(1)?;
        let z = coordinate(2)?;
        let yaw = angle(0)?;
        let pitch = angle(1)?;
        let on_ground = root
            .get("OnGround")
            .and_then(Tag::as_byte)
            .is_some_and(|value| value != 0);
        let mut physics = PlayerPhysics::new(x, y, z);
        physics.yaw = yaw;
        physics.pitch = pitch;
        physics.on_ground = on_ground;

        let extra = root
            .as_compound()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(key, _)| !KNOWN_PLAYERDATA_FIELDS.contains(&key.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(Self {
            name,
            uuid,
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            game_mode,
            selected_slot,
            inventory,
            cursor,
            health,
            food_level,
            food_saturation,
            food_exhaustion,
            xp_level,
            xp_progress,
            xp_total,
            score,
            air,
            physics,
            extra,
        }))
    }

    pub fn path(directory: &Path, uuid: &[u8; 16]) -> PathBuf {
        directory.join(format!("{}.dat", uuid_string(uuid)))
    }
}

pub fn uuid_string(uuid: &[u8; 16]) -> String {
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

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketWriter;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-player-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn uuid_formatting_is_hyphenated() {
        let mut uuid = [0u8; 16];
        uuid[0] = 0x12;
        uuid[15] = 0xef;
        assert_eq!(uuid_string(&uuid), "12000000-0000-0000-0000-0000000000ef");
    }

    #[test]
    fn saves_and_loads_player_position() {
        let directory = temp_dir("save");
        let mut session = PlayerSession::new("Steve".into(), [0x11; 16], 12.5, 64.0, -3.25);
        session.yaw = 90.0;
        session.pitch = -15.0;
        session.on_ground = true;
        session.physics.on_ground = true;
        session.physics.yaw = 90.0;
        session.physics.pitch = -15.0;
        session.selected_slot = 2;
        session.inventory[0] = ItemStack::new("minecraft:dirt", 3);
        session.cursor = ItemStack::new("minecraft:stone", 2);
        session.health = 13.5;
        session.food_level = 11;
        session.food_saturation = 2.25;
        session.food_exhaustion = 1.5;
        session.xp_level = 7;
        session.xp_progress = 0.75;
        session.xp_total = 140;
        session.score = 9;
        session.air = 120;
        session.game_mode = 1;
        let path = session.save(&directory).unwrap();
        assert!(path.exists());
        let saved = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved.get("DataVersion").and_then(Tag::as_int),
            Some(crate::level::DATA_VERSION),
            "playerdata carries the current data version"
        );

        let loaded = PlayerSession::load(&directory, [0x11; 16])
            .unwrap()
            .expect("playerdata");
        assert_eq!(loaded, session);
        assert_eq!(loaded.game_mode, 1, "playerGameType round-trips");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_item_consumes_one_per_placement() {
        let mut session = PlayerSession::new("Alex".into(), [0x44; 16], 0.0, 0.0, 0.0);
        session.selected_slot = 1;
        assert_eq!(session.selected_item().unwrap().count, 64);
        assert!(session.consume_selected());
        assert_eq!(session.selected_item().unwrap().count, 63);
        for _ in 0..63 {
            assert!(session.consume_selected());
        }
        assert!(session.selected_item().is_none());
        assert!(!session.consume_selected());
    }

    #[test]
    fn preserves_unknown_playerdata_fields() {
        let directory = temp_dir("extra");
        fs::create_dir_all(&directory).unwrap();
        let uuid = [0x66; 16];
        let root = Tag::Compound(vec![
            (
                "Pos".into(),
                Tag::List(vec![Tag::Double(1.5), Tag::Double(70.0), Tag::Double(-2.5)]),
            ),
            (
                "Rotation".into(),
                Tag::List(vec![Tag::Float(10.0), Tag::Float(0.0)]),
            ),
            ("Health".into(), Tag::Float(17.5)),
            ("foodLevel".into(), Tag::Int(12)),
            (
                "EnderItems".into(),
                Tag::List(vec![Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:stone".into()),
                )])]),
            ),
        ]);
        fs::write(
            PlayerSession::path(&directory, &uuid),
            nbt::write_gzip_root(&root).unwrap(),
        )
        .unwrap();

        let mut session = PlayerSession::load(&directory, uuid)
            .unwrap()
            .expect("playerdata");
        assert_eq!(session.extra.len(), 1, "only unknown fields are kept");
        assert_eq!(session.health, 17.5, "Health is parsed into the session");
        assert_eq!(
            session.food_level, 12,
            "foodLevel is parsed into the session"
        );
        assert_eq!(
            session.game_mode, -1,
            "a missing playerGameType leaves the server default to the join path"
        );
        session.y = 71.0;
        session.save(&directory).unwrap();

        let reloaded =
            nbt::read_gzip_root(&fs::read(PlayerSession::path(&directory, &uuid)).unwrap())
                .unwrap();
        for key in ["Health", "foodLevel", "EnderItems", "UUID"] {
            assert!(reloaded.get(key).is_some(), "{key} survived the save");
        }
        let position = reloaded.get("Pos").and_then(Tag::as_list).unwrap();
        assert_eq!(position[1].as_double(), Some(71.0));

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn new_playerdata_starts_with_survival_defaults() {
        let directory = temp_dir("defaults");
        let session = PlayerSession::new("New".into(), [0x12; 16], 0.0, 64.0, 0.0);
        let path = session.save(&directory).unwrap();
        let root = nbt::read_gzip_root(&fs::read(path).unwrap()).unwrap();

        assert_eq!(root.get("Health").and_then(Tag::as_float), Some(20.0));
        assert_eq!(root.get("foodLevel").and_then(Tag::as_int), Some(20));
        assert_eq!(
            root.get("foodSaturationLevel").and_then(Tag::as_float),
            Some(5.0)
        );
        assert_eq!(
            root.get("Air").and_then(|tag| match tag {
                Tag::Short(value) => Some(*value),
                _ => None,
            }),
            Some(300)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_playerdata_returns_none() {
        let directory = temp_dir("missing");
        assert!(
            PlayerSession::load(&directory, [0x22; 16])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parses_all_movement_packets() {
        use crate::player_physics::FlatPhysicsWorld;

        let mut session = PlayerSession::new("Alex".into(), [0x33; 16], 0.0, 0.0, 0.0);
        let mut world = FlatPhysicsWorld::new(-100);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_POS).unwrap();
        writer.write_f64(1.0).unwrap();
        writer.write_f64(2.0).unwrap();
        writer.write_f64(3.0).unwrap();
        writer.write_u8(1).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!((session.x, session.y, session.z), (1.0, 2.0, 3.0));
        assert!(session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_POS_ROT).unwrap();
        writer.write_f64(4.0).unwrap();
        writer.write_f64(5.0).unwrap();
        writer.write_f64(6.0).unwrap();
        writer.write_f32(45.0).unwrap();
        writer.write_f32(-10.0).unwrap();
        writer.write_u8(0).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!((session.yaw, session.pitch), (45.0, -10.0));
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_ROT).unwrap();
        writer.write_f32(90.0).unwrap();
        writer.write_f32(0.0).unwrap();
        writer.write_u8(1).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!(session.yaw, 90.0);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_STATUS).unwrap();
        writer.write_u8(0).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(99).unwrap();
        assert!(
            !session
                .apply_movement(&writer.into_inner(), &mut world)
                .unwrap()
                .was_movement
        );
    }
}
