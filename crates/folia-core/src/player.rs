use crate::inventory::{HOTBAR_SLOTS, INVENTORY_SLOTS, ItemStack, default_inventory};
use crate::nbt::{self, Tag};
use crate::protocol::PacketReader;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SERVERBOUND_MOVE_POS: i32 = 29;
pub const SERVERBOUND_MOVE_POS_ROT: i32 = 30;
pub const SERVERBOUND_MOVE_ROT: i32 = 31;
pub const SERVERBOUND_MOVE_STATUS: i32 = 32;

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
    pub selected_slot: i32,
    pub inventory: Vec<ItemStack>,
    pub cursor: ItemStack,
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
            selected_slot: 0,
            inventory: default_inventory(),
            cursor: ItemStack::empty(),
        }
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

    /// Parse a serverbound play movement packet. Returns true when the packet
    /// was a movement packet and the session was updated.
    pub fn apply_movement(&mut self, packet: &[u8]) -> io::Result<bool> {
        let mut reader = PacketReader::new(packet);
        let id = reader.read_varint()?;
        match id {
            SERVERBOUND_MOVE_POS => {
                self.x = reader.read_f64()?;
                self.y = reader.read_f64()?;
                self.z = reader.read_f64()?;
                let flags = reader.read_u8()?;
                self.on_ground = flags & 1 != 0;
                Ok(true)
            }
            SERVERBOUND_MOVE_POS_ROT => {
                self.x = reader.read_f64()?;
                self.y = reader.read_f64()?;
                self.z = reader.read_f64()?;
                self.yaw = reader.read_f32()?;
                self.pitch = reader.read_f32()?;
                let flags = reader.read_u8()?;
                self.on_ground = flags & 1 != 0;
                Ok(true)
            }
            SERVERBOUND_MOVE_ROT => {
                self.yaw = reader.read_f32()?;
                self.pitch = reader.read_f32()?;
                let flags = reader.read_u8()?;
                self.on_ground = flags & 1 != 0;
                Ok(true)
            }
            SERVERBOUND_MOVE_STATUS => {
                let flags = reader.read_u8()?;
                self.on_ground = flags & 1 != 0;
                Ok(true)
            }
            _ => Ok(false),
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

        let tag = Tag::Compound(vec![
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
        ]);
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

        Ok(Some(Self {
            name,
            uuid,
            x: coordinate(0)?,
            y: coordinate(1)?,
            z: coordinate(2)?,
            yaw: angle(0)?,
            pitch: angle(1)?,
            on_ground: root
                .get("OnGround")
                .and_then(Tag::as_byte)
                .is_some_and(|value| value != 0),
            selected_slot,
            inventory,
            cursor,
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
        session.selected_slot = 2;
        session.inventory[0] = ItemStack::new("minecraft:dirt", 3);
        session.cursor = ItemStack::new("minecraft:stone", 2);
        let path = session.save(&directory).unwrap();
        assert!(path.exists());

        let loaded = PlayerSession::load(&directory, [0x11; 16])
            .unwrap()
            .expect("playerdata");
        assert_eq!(loaded, session);
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
        let mut session = PlayerSession::new("Alex".into(), [0x33; 16], 0.0, 0.0, 0.0);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_POS).unwrap();
        writer.write_f64(1.0).unwrap();
        writer.write_f64(2.0).unwrap();
        writer.write_f64(3.0).unwrap();
        writer.write_u8(1).unwrap();
        assert!(session.apply_movement(&writer.into_inner()).unwrap());
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
        assert!(session.apply_movement(&writer.into_inner()).unwrap());
        assert_eq!((session.yaw, session.pitch), (45.0, -10.0));
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_ROT).unwrap();
        writer.write_f32(90.0).unwrap();
        writer.write_f32(0.0).unwrap();
        writer.write_u8(1).unwrap();
        assert!(session.apply_movement(&writer.into_inner()).unwrap());
        assert_eq!(session.yaw, 90.0);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_STATUS).unwrap();
        writer.write_u8(0).unwrap();
        assert!(session.apply_movement(&writer.into_inner()).unwrap());
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(99).unwrap();
        assert!(!session.apply_movement(&writer.into_inner()).unwrap());
    }
}
