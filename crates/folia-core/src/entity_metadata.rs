//! Entity metadata, living-entity health/damage, attributes, and equipment.
//!
//! The wire format follows Folia 1.21.11:
//!
//! - `ClientboundSetEntityDataPacket`: entity id, then repeated
//!   `(index: u8, serializer: VarInt, value)` entries terminated by `255`.
//! - `ClientboundSetEquipmentPacket`: entity id, then repeated
//!   `(slot | 0x80): u8, item stack` entries. The high bit means "another slot
//!   follows"; the slot is `EquipmentSlot.ordinal()`, not `EquipmentSlot.id()`.
//! - `ClientboundEntityEventPacket`: entity id as a plain `i32`, then the
//!   event byte. Death uses event `3`; the post-death poof is event `60`.
//! - `ClientboundSetHealthPacket`: health `f32`, food VarInt, saturation `f32`.
//!
//! Metadata indices and serializer ids are pinned to `Entity`,
//! `LivingEntity`, and `EntityDataSerializers`. Only the serializers needed by
//! the current model are implemented; an unknown serializer is an explicit
//! error rather than a guessed payload length.

use crate::item_components::{ItemStack, read_item_stack, write_item_stack};
use crate::nbt::Tag;
use crate::protocol::{PacketReader, PacketWriter};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io;
use std::sync::{LazyLock, Mutex, MutexGuard};

/// `GameProtocols.CLIENTBOUND_TEMPLATE` registration order in 1.21.11.
pub const CLIENTBOUND_ENTITY_EVENT: i32 = 34;
pub const CLIENTBOUND_SET_ENTITY_DATA: i32 = 97;
pub const CLIENTBOUND_SET_EQUIPMENT: i32 = 100;
pub const CLIENTBOUND_SET_HEALTH: i32 = 102;

/// `ClientboundSetEntityDataPacket.EOF_MARKER`.
pub const ENTITY_DATA_EOF: u8 = 255;

/// `LivingEntity.handleEntityEvent` ids used by this slice.
pub const ENTITY_EVENT_DEATH: u8 = 3;
pub const ENTITY_EVENT_DEATH_POOF: u8 = 60;

/// `Entity.DATA_*` indices. `LivingEntity` extends Entity's list.
pub const DATA_SHARED_FLAGS: u8 = 0;
pub const DATA_AIR_SUPPLY: u8 = 1;
pub const DATA_CUSTOM_NAME: u8 = 2;
pub const DATA_CUSTOM_NAME_VISIBLE: u8 = 3;
pub const DATA_SILENT: u8 = 4;
pub const DATA_NO_GRAVITY: u8 = 5;
pub const DATA_POSE: u8 = 6;
pub const DATA_TICKS_FROZEN: u8 = 7;
pub const DATA_LIVING_ENTITY_FLAGS: u8 = 8;
pub const DATA_HEALTH: u8 = 9;

/// `Entity` shared-flag bit positions (`Entity.setSharedFlag`).
pub const FLAG_ON_FIRE: u8 = 0;
pub const FLAG_SHIFT_KEY_DOWN: u8 = 1;
pub const FLAG_SPRINTING: u8 = 3;
pub const FLAG_SWIMMING: u8 = 4;
pub const FLAG_INVISIBLE: u8 = 5;
pub const FLAG_GLOWING: u8 = 6;
pub const FLAG_FALL_FLYING: u8 = 7;

/// `LivingEntity` flag masks (`LivingEntity.setLivingEntityFlag`).
pub const LIVING_FLAG_USING_ITEM: u8 = 1;
pub const LIVING_FLAG_OFF_HAND: u8 = 2;
pub const LIVING_FLAG_SPIN_ATTACK: u8 = 4;

/// `Entity.getMaxAirSupply()` default.
pub const DEFAULT_AIR_SUPPLY: i32 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataError {
    /// The index already carries a value with a different serializer, which
    /// would desynchronise the client because the accessor type is fixed.
    SerializerMismatch {
        index: u8,
        expected: EntityDataSerializer,
        actual: EntityDataSerializer,
    },
    /// A modification targeted an entity id that is not in the store.
    UnknownEntity(i32),
    /// A dirty mark targeted an index the entity does not define.
    UnknownIndex(u8),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SerializerMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "metadata index {index} expects {expected:?}, got {actual:?}"
            ),
            Self::UnknownEntity(id) => write!(formatter, "unknown entity id {id}"),
            Self::UnknownIndex(index) => write!(formatter, "unknown metadata index {index}"),
        }
    }
}

impl std::error::Error for MetadataError {}

/// `EntityDataSerializers` registration order. Values are the ids written in
/// the metadata packet's serializer field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum EntityDataSerializer {
    Byte = 0,
    Int = 1,
    Long = 2,
    Float = 3,
    String = 4,
    Component = 5,
    OptionalComponent = 6,
    ItemStack = 7,
    Boolean = 8,
    Pose = 20,
}

impl EntityDataSerializer {
    pub const fn id(self) -> i32 {
        self as i32
    }

    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::Byte,
            1 => Self::Int,
            2 => Self::Long,
            3 => Self::Float,
            4 => Self::String,
            5 => Self::Component,
            6 => Self::OptionalComponent,
            7 => Self::ItemStack,
            8 => Self::Boolean,
            20 => Self::Pose,
            _ => return None,
        })
    }
}

/// `Pose` ids from `Pose.java`; the serializer writes the id as a VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    Standing,
    FallFlying,
    Sleeping,
    Swimming,
    SpinAttack,
    Crouching,
    LongJumping,
    Dying,
    Croaking,
    UsingTongue,
    Sitting,
    Roaring,
    Sniffing,
    Emerging,
    Digging,
    Sliding,
    Shooting,
    Inhaling,
}

impl Pose {
    pub const fn id(self) -> i32 {
        match self {
            Self::Standing => 0,
            Self::FallFlying => 1,
            Self::Sleeping => 2,
            Self::Swimming => 3,
            Self::SpinAttack => 4,
            Self::Crouching => 5,
            Self::LongJumping => 6,
            Self::Dying => 7,
            Self::Croaking => 8,
            Self::UsingTongue => 9,
            Self::Sitting => 10,
            Self::Roaring => 11,
            Self::Sniffing => 12,
            Self::Emerging => 13,
            Self::Digging => 14,
            Self::Sliding => 15,
            Self::Shooting => 16,
            Self::Inhaling => 17,
        }
    }

    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::Standing,
            1 => Self::FallFlying,
            2 => Self::Sleeping,
            3 => Self::Swimming,
            4 => Self::SpinAttack,
            5 => Self::Crouching,
            6 => Self::LongJumping,
            7 => Self::Dying,
            8 => Self::Croaking,
            9 => Self::UsingTongue,
            10 => Self::Sitting,
            11 => Self::Roaring,
            12 => Self::Sniffing,
            13 => Self::Emerging,
            14 => Self::Digging,
            15 => Self::Sliding,
            16 => Self::Shooting,
            17 => Self::Inhaling,
            _ => return None,
        })
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Standing => "standing",
            Self::FallFlying => "fall_flying",
            Self::Sleeping => "sleeping",
            Self::Swimming => "swimming",
            Self::SpinAttack => "spin_attack",
            Self::Crouching => "crouching",
            Self::LongJumping => "long_jumping",
            Self::Dying => "dying",
            Self::Croaking => "croaking",
            Self::UsingTongue => "using_tongue",
            Self::Sitting => "sitting",
            Self::Roaring => "roaring",
            Self::Sniffing => "sniffing",
            Self::Emerging => "emerging",
            Self::Digging => "digging",
            Self::Sliding => "sliding",
            Self::Shooting => "shooting",
            Self::Inhaling => "inhaling",
        }
    }
}

/// One typed synched-data value. The variant determines the serializer id, so a
/// serializer and its payload cannot drift apart.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Byte(i8),
    Int(i32),
    Long(i64),
    Float(f32),
    String(String),
    /// A component is sent as a network NBT tag (type byte + payload).
    Component(Tag),
    OptionalComponent(Option<Tag>),
    ItemStack(ItemStack),
    Boolean(bool),
    Pose(Pose),
}

impl MetadataValue {
    pub const fn serializer(&self) -> EntityDataSerializer {
        match self {
            Self::Byte(_) => EntityDataSerializer::Byte,
            Self::Int(_) => EntityDataSerializer::Int,
            Self::Long(_) => EntityDataSerializer::Long,
            Self::Float(_) => EntityDataSerializer::Float,
            Self::String(_) => EntityDataSerializer::String,
            Self::Component(_) => EntityDataSerializer::Component,
            Self::OptionalComponent(_) => EntityDataSerializer::OptionalComponent,
            Self::ItemStack(_) => EntityDataSerializer::ItemStack,
            Self::Boolean(_) => EntityDataSerializer::Boolean,
            Self::Pose(_) => EntityDataSerializer::Pose,
        }
    }

    pub fn write(&self, writer: &mut PacketWriter) -> io::Result<()> {
        match self {
            Self::Byte(value) => writer.write_u8(*value as u8),
            Self::Int(value) => writer.write_varint(*value),
            Self::Long(value) => writer.write_varlong(*value),
            Self::Float(value) => writer.write_f32(*value),
            Self::String(value) => writer.write_string(value),
            Self::Component(tag) => write_nbt_tag(writer, tag),
            Self::OptionalComponent(tag) => match tag {
                Some(tag) => {
                    writer.write_u8(1)?;
                    write_nbt_tag(writer, tag)
                }
                None => writer.write_u8(0),
            },
            Self::ItemStack(stack) => {
                write_item_stack(writer, stack);
                Ok(())
            }
            Self::Boolean(value) => writer.write_u8(u8::from(*value)),
            Self::Pose(pose) => writer.write_varint(pose.id()),
        }
    }

    pub fn read(
        serializer: EntityDataSerializer,
        reader: &mut PacketReader<'_>,
    ) -> io::Result<Self> {
        Ok(match serializer {
            EntityDataSerializer::Byte => Self::Byte(reader.read_u8()? as i8),
            EntityDataSerializer::Int => Self::Int(reader.read_varint()?),
            EntityDataSerializer::Long => Self::Long(reader.read_varlong()?),
            EntityDataSerializer::Float => Self::Float(reader.read_f32()?),
            EntityDataSerializer::String => Self::String(reader.read_string()?),
            EntityDataSerializer::Component => Self::Component(reader.read_nbt_any()?),
            EntityDataSerializer::OptionalComponent => {
                if reader.read_bool()? {
                    Self::OptionalComponent(Some(reader.read_nbt_any()?))
                } else {
                    Self::OptionalComponent(None)
                }
            }
            EntityDataSerializer::ItemStack => {
                let stack = read_item_stack(reader).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unsupported item stack in entity metadata",
                    )
                })?;
                Self::ItemStack(stack)
            }
            EntityDataSerializer::Boolean => Self::Boolean(reader.read_bool()?),
            EntityDataSerializer::Pose => {
                let id = reader.read_varint()?;
                Self::Pose(Pose::from_id(id).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("unknown pose id {id}"))
                })?)
            }
        })
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_pose(&self) -> Option<Pose> {
        match self {
            Self::Pose(value) => Some(*value),
            _ => None,
        }
    }
}

fn write_nbt_tag(writer: &mut PacketWriter, tag: &Tag) -> io::Result<()> {
    let mut bytes = Vec::new();
    crate::nbt::write_any_tag(&mut bytes, tag)?;
    writer.write_bytes(&bytes);
    Ok(())
}

/// A metadata entry ready for the packet body.
#[derive(Debug, Clone, PartialEq)]
pub struct DataValue {
    pub index: u8,
    pub value: MetadataValue,
}

impl DataValue {
    pub const fn new(index: u8, value: MetadataValue) -> Self {
        Self { index, value }
    }

    pub const fn serializer(&self) -> EntityDataSerializer {
        self.value.serializer()
    }

    fn write(&self, writer: &mut PacketWriter) -> io::Result<()> {
        writer.write_u8(self.index)?;
        writer.write_varint(self.value.serializer().id())?;
        self.value.write(writer)
    }

    fn read(index: u8, reader: &mut PacketReader<'_>) -> io::Result<Self> {
        let serializer_id = reader.read_varint()?;
        let serializer = EntityDataSerializer::from_id(serializer_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported entity data serializer {serializer_id}"),
            )
        })?;
        Ok(Self {
            index,
            value: MetadataValue::read(serializer, reader)?,
        })
    }
}

/// Per-entity synched-data map with vanilla dirty semantics: a value becomes
/// dirty only when it changes (or is explicitly marked dirty), and
/// [`EntityMetadata::pack_dirty`] clears the dirty flags after collecting the
/// entries in ascending index order.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityMetadata {
    entity_id: i32,
    items: BTreeMap<u8, MetadataValue>,
    dirty: BTreeSet<u8>,
}

impl EntityMetadata {
    /// Base `Entity` synched data (indices 0-7).
    pub fn new(entity_id: i32) -> Self {
        let mut metadata = Self {
            entity_id,
            items: BTreeMap::new(),
            dirty: BTreeSet::new(),
        };
        metadata.insert_default(DATA_SHARED_FLAGS, MetadataValue::Byte(0));
        metadata.insert_default(DATA_AIR_SUPPLY, MetadataValue::Int(DEFAULT_AIR_SUPPLY));
        metadata.insert_default(DATA_CUSTOM_NAME, MetadataValue::OptionalComponent(None));
        metadata.insert_default(DATA_CUSTOM_NAME_VISIBLE, MetadataValue::Boolean(false));
        metadata.insert_default(DATA_SILENT, MetadataValue::Boolean(false));
        metadata.insert_default(DATA_NO_GRAVITY, MetadataValue::Boolean(false));
        metadata.insert_default(DATA_POSE, MetadataValue::Pose(Pose::Standing));
        metadata.insert_default(DATA_TICKS_FROZEN, MetadataValue::Int(0));
        metadata
    }

    /// Base `Entity` plus `LivingEntity` synched data (indices 0-9).
    pub fn new_living(entity_id: i32) -> Self {
        Self::new_living_with_health(entity_id, 20.0)
    }

    pub fn new_living_with_health(entity_id: i32, health: f32) -> Self {
        let mut metadata = Self::new(entity_id);
        metadata.insert_default(DATA_LIVING_ENTITY_FLAGS, MetadataValue::Byte(0));
        metadata.insert_default(DATA_HEALTH, MetadataValue::Float(health));
        metadata
    }

    fn insert_default(&mut self, index: u8, value: MetadataValue) {
        self.items.insert(index, value);
    }

    pub const fn entity_id(&self) -> i32 {
        self.entity_id
    }

    pub fn get(&self, index: u8) -> Option<&MetadataValue> {
        self.items.get(&index)
    }

    pub fn get_health(&self) -> Option<f32> {
        self.get(DATA_HEALTH).and_then(MetadataValue::as_f32)
    }

    pub fn get_pose(&self) -> Option<Pose> {
        self.get(DATA_POSE).and_then(MetadataValue::as_pose)
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub fn dirty_indices(&self) -> impl Iterator<Item = u8> + '_ {
        self.dirty.iter().copied()
    }

    /// Mirrors `SynchedEntityData.set`: no serializer change, no dirty flag when
    /// the value is equal.
    pub fn set(&mut self, index: u8, value: MetadataValue) -> Result<bool, MetadataError> {
        if let Some(existing) = self.items.get(&index)
            && existing.serializer() != value.serializer()
        {
            return Err(MetadataError::SerializerMismatch {
                index,
                expected: existing.serializer(),
                actual: value.serializer(),
            });
        }
        if self.items.get(&index) == Some(&value) {
            return Ok(false);
        }
        self.items.insert(index, value);
        self.dirty.insert(index);
        Ok(true)
    }

    /// Mirrors `SynchedEntityData.markDirty`.
    pub fn mark_dirty(&mut self, index: u8) -> Result<(), MetadataError> {
        if !self.items.contains_key(&index) {
            return Err(MetadataError::UnknownIndex(index));
        }
        self.dirty.insert(index);
        Ok(())
    }

    pub fn set_health(&mut self, health: f32) -> Result<bool, MetadataError> {
        self.set(DATA_HEALTH, MetadataValue::Float(health))
    }

    pub fn set_pose(&mut self, pose: Pose) -> Result<bool, MetadataError> {
        self.set(DATA_POSE, MetadataValue::Pose(pose))
    }

    pub fn set_custom_name(&mut self, name: Option<Tag>) -> Result<bool, MetadataError> {
        self.set(DATA_CUSTOM_NAME, MetadataValue::OptionalComponent(name))
    }

    pub fn set_custom_name_visible(&mut self, visible: bool) -> Result<bool, MetadataError> {
        self.set(DATA_CUSTOM_NAME_VISIBLE, MetadataValue::Boolean(visible))
    }

    /// Set one of `Entity`'s shared-flag bits by bit position.
    pub fn set_shared_flag(&mut self, flag: u8, enabled: bool) -> Result<bool, MetadataError> {
        let current = self.shared_flags();
        let mask = 1u8 << (flag & 7);
        let updated = if enabled {
            current | mask
        } else {
            current & !mask
        };
        self.set(DATA_SHARED_FLAGS, MetadataValue::Byte(updated as i8))
    }

    pub fn shared_flags(&self) -> u8 {
        match self.get(DATA_SHARED_FLAGS) {
            Some(MetadataValue::Byte(value)) => *value as u8,
            _ => 0,
        }
    }

    /// Set one of `LivingEntity`'s flag masks (`LIVING_FLAG_*`).
    pub fn set_living_flag(&mut self, mask: u8, enabled: bool) -> Result<bool, MetadataError> {
        let current = match self.get(DATA_LIVING_ENTITY_FLAGS) {
            Some(MetadataValue::Byte(value)) => *value as u8,
            _ => 0,
        };
        let updated = if enabled {
            current | mask
        } else {
            current & !mask
        };
        self.set(DATA_LIVING_ENTITY_FLAGS, MetadataValue::Byte(updated as i8))
    }

    /// All values, in index order, without clearing dirty flags.
    pub fn pack_all(&self) -> Vec<DataValue> {
        self.items
            .iter()
            .map(|(index, value)| DataValue::new(*index, value.clone()))
            .collect()
    }

    /// Dirty values in index order. Returns `None` when nothing changed, like
    /// `SynchedEntityData.packDirty`.
    pub fn pack_dirty(&mut self) -> Option<Vec<DataValue>> {
        if self.dirty.is_empty() {
            return None;
        }
        let indices = std::mem::take(&mut self.dirty);
        Some(
            indices
                .into_iter()
                .filter_map(|index| {
                    self.items
                        .get(&index)
                        .map(|value| DataValue::new(index, value.clone()))
                })
                .collect(),
        )
    }

    pub fn all_packet(&self) -> Vec<u8> {
        set_entity_data_packet(self.entity_id, &self.pack_all())
    }

    pub fn dirty_packet(&mut self) -> Option<Vec<u8>> {
        let values = self.pack_dirty()?;
        Some(set_entity_data_packet(self.entity_id, &values))
    }
}

/// Typed metadata store keyed by network entity id.
#[derive(Debug, Default)]
pub struct MetadataStore {
    entities: HashMap<i32, EntityMetadata>,
}

impl MetadataStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn insert(&mut self, metadata: EntityMetadata) -> Option<EntityMetadata> {
        self.entities.insert(metadata.entity_id(), metadata)
    }

    pub fn remove(&mut self, entity_id: i32) -> Option<EntityMetadata> {
        self.entities.remove(&entity_id)
    }

    pub fn get(&self, entity_id: i32) -> Option<&EntityMetadata> {
        self.entities.get(&entity_id)
    }

    pub fn get_mut(&mut self, entity_id: i32) -> Option<&mut EntityMetadata> {
        self.entities.get_mut(&entity_id)
    }

    pub fn entry_living(&mut self, entity_id: i32) -> &mut EntityMetadata {
        self.entities
            .entry(entity_id)
            .or_insert_with(|| EntityMetadata::new_living(entity_id))
    }

    pub fn set(
        &mut self,
        entity_id: i32,
        index: u8,
        value: MetadataValue,
    ) -> Result<bool, MetadataError> {
        self.get_mut(entity_id)
            .ok_or(MetadataError::UnknownEntity(entity_id))?
            .set(index, value)
    }

    /// Pack and clear one entity's dirty values as a
    /// `ClientboundSetEntityDataPacket`.
    pub fn dirty_packet(&mut self, entity_id: i32) -> Option<Vec<u8>> {
        self.get_mut(entity_id)?.dirty_packet()
    }

    pub fn all_packet(&self, entity_id: i32) -> Option<Vec<u8>> {
        self.get(entity_id).map(EntityMetadata::all_packet)
    }
}

static METADATA_STORE: LazyLock<Mutex<MetadataStore>> =
    LazyLock::new(|| Mutex::new(MetadataStore::new()));

/// Process-wide metadata store. Region-owned stores can be created with
/// [`MetadataStore::new`] and used instead; this one mirrors the existing
/// process-wide entity map until P5.2 moves entities into regions.
pub fn metadata_store() -> MutexGuard<'static, MetadataStore> {
    METADATA_STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EquipmentSlot {
    MainHand,
    OffHand,
    Feet,
    Legs,
    Chest,
    Head,
    Body,
    Saddle,
}

impl EquipmentSlot {
    /// Wire order is `EquipmentSlot.ordinal()` in
    /// `ClientboundSetEquipmentPacket`, not `EquipmentSlot.getId()`.
    pub const ALL: [Self; 8] = [
        Self::MainHand,
        Self::OffHand,
        Self::Feet,
        Self::Legs,
        Self::Chest,
        Self::Head,
        Self::Body,
        Self::Saddle,
    ];

    pub const fn ordinal(self) -> u8 {
        match self {
            Self::MainHand => 0,
            Self::OffHand => 1,
            Self::Feet => 2,
            Self::Legs => 3,
            Self::Chest => 4,
            Self::Head => 5,
            Self::Body => 6,
            Self::Saddle => 7,
        }
    }

    pub fn from_ordinal(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::MainHand,
            1 => Self::OffHand,
            2 => Self::Feet,
            3 => Self::Legs,
            4 => Self::Chest,
            5 => Self::Head,
            6 => Self::Body,
            7 => Self::Saddle,
            _ => return None,
        })
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::MainHand => "mainhand",
            Self::OffHand => "offhand",
            Self::Feet => "feet",
            Self::Legs => "legs",
            Self::Chest => "chest",
            Self::Head => "head",
            Self::Body => "body",
            Self::Saddle => "saddle",
        }
    }

    pub const fn is_armor(self) -> bool {
        matches!(
            self,
            Self::Feet | Self::Legs | Self::Chest | Self::Head | Self::Body
        )
    }

    pub const fn is_hand(self) -> bool {
        matches!(self, Self::MainHand | Self::OffHand)
    }
}

/// The eight vanilla equipment slots. Empty stacks are encoded as count 0 by
/// `ItemStack.OPTIONAL_STREAM_CODEC`.
#[derive(Debug, Clone, PartialEq)]
pub struct Equipment {
    slots: [ItemStack; 8],
}

impl Default for Equipment {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::empty()),
        }
    }
}

impl Equipment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: EquipmentSlot) -> &ItemStack {
        &self.slots[slot.ordinal() as usize]
    }

    pub fn get_mut(&mut self, slot: EquipmentSlot) -> &mut ItemStack {
        &mut self.slots[slot.ordinal() as usize]
    }

    /// Replace a slot and return the previous stack.
    pub fn set(&mut self, slot: EquipmentSlot, stack: ItemStack) -> ItemStack {
        std::mem::replace(&mut self.slots[slot.ordinal() as usize], stack)
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(ItemStack::is_empty)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EquipmentSlot, &ItemStack)> {
        EquipmentSlot::ALL
            .into_iter()
            .map(|slot| (slot, self.get(slot)))
    }
}

/// Minimal synced attributes. Values use vanilla base values and ranged-attribute
/// limits where the 1.21.11 table is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Attribute {
    MaxHealth,
    KnockbackResistance,
    MovementSpeed,
    Armor,
    ArmorToughness,
    AttackDamage,
    AttackSpeed,
    FollowRange,
}

impl Attribute {
    pub const ALL: [Self; 8] = [
        Self::MaxHealth,
        Self::KnockbackResistance,
        Self::MovementSpeed,
        Self::Armor,
        Self::ArmorToughness,
        Self::AttackDamage,
        Self::AttackSpeed,
        Self::FollowRange,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::MaxHealth => "minecraft:max_health",
            Self::KnockbackResistance => "minecraft:knockback_resistance",
            Self::MovementSpeed => "minecraft:movement_speed",
            Self::Armor => "minecraft:armor",
            Self::ArmorToughness => "minecraft:armor_toughness",
            Self::AttackDamage => "minecraft:attack_damage",
            Self::AttackSpeed => "minecraft:attack_speed",
            Self::FollowRange => "minecraft:follow_range",
        }
    }

    pub const fn default_value(self) -> f64 {
        match self {
            Self::MaxHealth => 20.0,
            Self::KnockbackResistance => 0.0,
            Self::MovementSpeed => 0.7,
            Self::Armor => 0.0,
            Self::ArmorToughness => 0.0,
            Self::AttackDamage => 2.0,
            Self::AttackSpeed => 4.0,
            Self::FollowRange => 32.0,
        }
    }

    pub const fn range(self) -> (f64, f64) {
        match self {
            Self::MaxHealth => (1.0, 2048.0),
            Self::KnockbackResistance => (0.0, 1.0),
            Self::MovementSpeed => (0.0, 1024.0),
            Self::Armor => (0.0, 30.0),
            Self::ArmorToughness => (0.0, 20.0),
            Self::AttackDamage => (0.0, 2048.0),
            Self::AttackSpeed => (0.0, 1024.0),
            Self::FollowRange => (0.0, 2048.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeMap {
    values: BTreeMap<Attribute, f64>,
}

impl Default for AttributeMap {
    fn default() -> Self {
        Self::living()
    }
}

impl AttributeMap {
    /// `LivingEntity.createLivingAttributes` defaults before a mob overrides
    /// them.
    pub fn living() -> Self {
        let mut values = BTreeMap::new();
        for attribute in Attribute::ALL {
            values.insert(attribute, attribute.default_value());
        }
        Self { values }
    }

    /// `Player.createAttributes` overrides attack damage and movement speed.
    pub fn player() -> Self {
        let mut attributes = Self::living();
        attributes.values.insert(Attribute::AttackDamage, 1.0);
        attributes.values.insert(Attribute::MovementSpeed, 0.1);
        attributes
    }

    pub fn get(&self, attribute: Attribute) -> f64 {
        self.values
            .get(&attribute)
            .copied()
            .unwrap_or_else(|| attribute.default_value())
    }

    /// Set a base value, clamped to the attribute's vanilla range. Returns true
    /// when the value changed.
    pub fn set(&mut self, attribute: Attribute, value: f64) -> bool {
        let (minimum, maximum) = attribute.range();
        let value = if value.is_nan() {
            attribute.default_value()
        } else {
            value.clamp(minimum, maximum)
        };
        if self.get(attribute) == value {
            return false;
        }
        self.values.insert(attribute, value);
        true
    }

    pub fn max_health(&self) -> f64 {
        self.get(Attribute::MaxHealth)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Attribute, f64)> + '_ {
        self.values
            .iter()
            .map(|(attribute, value)| (*attribute, *value))
    }
}

/// Vanilla `ClientboundSetEntityDataPacket` body (without the frame length).
pub fn set_entity_data_packet(entity_id: i32, values: &[DataValue]) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_SET_ENTITY_DATA)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(entity_id)
        .expect("writing to Vec cannot fail");
    pack_metadata_values(&mut writer, values);
    writer
        .write_u8(ENTITY_DATA_EOF)
        .expect("writing to Vec cannot fail");
    writer.into_inner()
}

fn pack_metadata_values(writer: &mut PacketWriter, values: &[DataValue]) {
    for value in values {
        value.write(writer).expect("writing to Vec cannot fail");
    }
}

/// Decode a complete `ClientboundSetEntityDataPacket` payload (packet id
/// included) back into typed values.
pub fn read_set_entity_data(packet: &[u8]) -> io::Result<(i32, Vec<DataValue>)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_SET_ENTITY_DATA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected set_entity_data id {CLIENTBOUND_SET_ENTITY_DATA}, got {packet_id}"),
        ));
    }
    let entity_id = reader.read_varint()?;
    let mut values = Vec::new();
    loop {
        let index = reader.read_u8()?;
        if index == ENTITY_DATA_EOF {
            break;
        }
        values.push(DataValue::read(index, &mut reader)?);
    }
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after entity data terminator",
        ));
    }
    Ok((entity_id, values))
}

/// Full equipment update: all eight slots with the continuation bit set on
/// every entry except the last.
pub fn set_equipment_packet(entity_id: i32, equipment: &Equipment) -> Vec<u8> {
    let slots: Vec<(EquipmentSlot, ItemStack)> = equipment_slots(equipment);
    set_equipment_subset_packet(entity_id, &slots)
}

fn equipment_slots(equipment: &Equipment) -> Vec<(EquipmentSlot, ItemStack)> {
    EquipmentSlot::ALL
        .into_iter()
        .map(|slot| (slot, equipment.get(slot).clone()))
        .collect()
}

/// Equipment update for a subset of slots, preserving `ClientboundSetEquipment
/// Packet`'s continuation-bit encoding. `slots` must not be empty.
pub fn set_equipment_subset_packet(
    entity_id: i32,
    slots: &[(EquipmentSlot, ItemStack)],
) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_SET_EQUIPMENT)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(entity_id)
        .expect("writing to Vec cannot fail");
    for (index, (slot, stack)) in slots.iter().enumerate() {
        let continuation = index + 1 != slots.len();
        let slot = slot.ordinal() | if continuation { 0x80 } else { 0 };
        writer.write_u8(slot).expect("writing to Vec cannot fail");
        write_item_stack(&mut writer, stack);
    }
    writer.into_inner()
}

pub fn read_set_equipment(packet: &[u8]) -> io::Result<(i32, Vec<(EquipmentSlot, ItemStack)>)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_SET_EQUIPMENT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected set_equipment id {CLIENTBOUND_SET_EQUIPMENT}, got {packet_id}"),
        ));
    }
    let entity_id = reader.read_varint()?;
    let mut slots = Vec::new();
    loop {
        let packed = reader.read_u8()?;
        let slot = EquipmentSlot::from_ordinal(packed & 0x7f).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown equipment slot {}", packed & 0x7f),
            )
        })?;
        let stack = read_item_stack(&mut reader).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported item stack in equipment packet",
            )
        })?;
        slots.push((slot, stack));
        if packed & 0x80 == 0 {
            break;
        }
    }
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after equipment slots",
        ));
    }
    Ok((entity_id, slots))
}

pub fn entity_event_packet(entity_id: i32, event: u8) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_ENTITY_EVENT)
        .expect("writing to Vec cannot fail");
    // ClientboundEntityEventPacket writes a plain int, not a VarInt.
    writer
        .write_i32(entity_id)
        .expect("writing to Vec cannot fail");
    writer.write_u8(event).expect("writing to Vec cannot fail");
    writer.into_inner()
}

pub fn read_entity_event(packet: &[u8]) -> io::Result<(i32, u8)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_ENTITY_EVENT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected entity_event id {CLIENTBOUND_ENTITY_EVENT}, got {packet_id}"),
        ));
    }
    let entity_id = reader.read_i32()?;
    let event = reader.read_u8()?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after entity event",
        ));
    }
    Ok((entity_id, event))
}

pub fn set_health_packet(health: f32, food: i32, saturation: f32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_SET_HEALTH)
        .expect("writing to Vec cannot fail");
    writer
        .write_f32(health)
        .expect("writing to Vec cannot fail");
    writer
        .write_varint(food)
        .expect("writing to Vec cannot fail");
    writer
        .write_f32(saturation)
        .expect("writing to Vec cannot fail");
    writer.into_inner()
}

pub fn read_set_health(packet: &[u8]) -> io::Result<(f32, i32, f32)> {
    let mut reader = PacketReader::new(packet);
    let packet_id = reader.read_varint()?;
    if packet_id != CLIENTBOUND_SET_HEALTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected set_health id {CLIENTBOUND_SET_HEALTH}, got {packet_id}"),
        ));
    }
    let health = reader.read_f32()?;
    let food = reader.read_varint()?;
    let saturation = reader.read_f32()?;
    if !reader.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trailing bytes after set_health",
        ));
    }
    Ok((health, food, saturation))
}

/// Result of applying one damage source. Packets are ready to broadcast in
/// vanilla order: entity event first for a lethal hit, then synched-data and
/// `ClientboundSetHealth`.
#[derive(Debug, Clone, PartialEq)]
pub struct HurtResult {
    pub damaged: bool,
    pub amount: f32,
    pub died: bool,
    pub packets: Vec<Vec<u8>>,
}

impl HurtResult {
    fn rejected() -> Self {
        Self {
            damaged: false,
            amount: 0.0,
            died: false,
            packets: Vec::new(),
        }
    }
}

/// `LivingEntity` health/damage state plus its synched metadata, attributes,
/// and equipment.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingEntity {
    pub entity_id: i32,
    pub health: f32,
    pub invulnerable_time: i32,
    pub invulnerable_duration: i32,
    pub last_hurt: f32,
    pub hurt_time: i32,
    pub hurt_duration: i32,
    pub death_time: i32,
    pub dead: bool,
    pub death_broadcasted: bool,
    pub removed: bool,
    pub is_player: bool,
    pub food: i32,
    pub saturation: f32,
    pub attributes: AttributeMap,
    pub equipment: Equipment,
    pub metadata: EntityMetadata,
}

impl LivingEntity {
    pub fn new(entity_id: i32) -> Self {
        Self::with_attributes(entity_id, false, AttributeMap::living())
    }

    pub fn new_player(entity_id: i32) -> Self {
        Self::with_attributes(entity_id, true, AttributeMap::player())
    }

    fn with_attributes(entity_id: i32, is_player: bool, attributes: AttributeMap) -> Self {
        let health = attributes.max_health() as f32;
        Self {
            entity_id,
            health,
            invulnerable_time: 0,
            invulnerable_duration: 20,
            last_hurt: 0.0,
            hurt_time: 0,
            hurt_duration: 0,
            death_time: 0,
            dead: false,
            death_broadcasted: false,
            removed: false,
            is_player,
            food: 20,
            saturation: 5.0,
            attributes,
            equipment: Equipment::new(),
            metadata: EntityMetadata::new_living_with_health(entity_id, health),
        }
    }

    pub fn max_health(&self) -> f32 {
        self.attributes.max_health() as f32
    }

    pub fn is_alive(&self) -> bool {
        !self.dead && self.health > 0.0
    }

    pub fn is_dead_or_dying(&self) -> bool {
        self.health <= 0.0
    }

    /// `LivingEntity.setHealth`: NaN becomes max health, then the value is
    /// clamped to `[0, max_health]`. The synched `DATA_HEALTH` value is updated.
    pub fn set_health(&mut self, health: f32) -> bool {
        let maximum = self.max_health();
        let health = if health.is_nan() {
            maximum
        } else {
            health.clamp(0.0, maximum)
        };
        if self.health == health {
            return false;
        }
        self.health = health;
        let _ = self.metadata.set_health(health);
        true
    }

    pub fn set_attribute(&mut self, attribute: Attribute, value: f64) -> bool {
        let changed = self.attributes.set(attribute, value);
        if changed && attribute == Attribute::MaxHealth && self.health > self.max_health() {
            self.set_health(self.max_health());
        }
        changed
    }

    /// `LivingEntity.hurtServer` subset: dead/removed entities ignore damage,
    /// negative damage is zero, and the 20-tick invulnerability window ignores
    /// hits that do not exceed the previous hit.
    pub fn hurt(&mut self, amount: f32) -> HurtResult {
        self.hurt_with(amount, false)
    }

    /// [`Self::hurt`] with an explicit invulnerability bypass for damage types
    /// such as the void, matching `DamageSource.is(DamageTypeTags.BYPASSES_INVULNERABILITY)`.
    pub fn hurt_with(&mut self, amount: f32, bypass_invulnerability: bool) -> HurtResult {
        if self.removed || self.dead || self.health <= 0.0 {
            return HurtResult::rejected();
        }

        let amount = if amount.is_nan() || amount.is_infinite() {
            f32::MAX
        } else {
            amount.max(0.0)
        };

        let in_invulnerable_window = self.invulnerable_time > self.invulnerable_duration / 2;
        if !bypass_invulnerability && in_invulnerable_window && amount <= self.last_hurt {
            return HurtResult::rejected();
        }

        self.last_hurt = amount;
        if !bypass_invulnerability && !in_invulnerable_window {
            self.invulnerable_time = self.invulnerable_duration;
            self.hurt_duration = 10;
            self.hurt_time = self.hurt_duration;
        }

        let health = (self.health - amount).max(0.0);
        self.set_health(health);

        let died = health <= 0.0 && !self.dead;
        let mut packets = Vec::new();
        if died {
            self.dead = true;
            self.death_time = 0;
            self.death_broadcasted = false;
            let _ = self.metadata.set_pose(Pose::Dying);
            packets.push(entity_event_packet(self.entity_id, ENTITY_EVENT_DEATH));
        }
        if let Some(packet) = self.metadata.dirty_packet() {
            packets.push(packet);
        }
        if self.is_player {
            packets.push(set_health_packet(
                self.health,
                self.food.clamp(0, 20),
                self.saturation.max(0.0),
            ));
        }

        HurtResult {
            damaged: true,
            amount,
            died,
            packets,
        }
    }

    /// One `LivingEntity.tick` subset: hurt/invulnerability countdown and the
    /// death animation. Vanilla does not tick `invulnerableTime` down for a
    /// server player, so a player flag preserves that behavior.
    pub fn tick(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
        if self.invulnerable_time > 0 && !self.is_player {
            self.invulnerable_time -= 1;
        }

        if self.is_dead_or_dying() {
            self.death_time += 1;
            if self.death_time >= 20 && !self.death_broadcasted {
                self.death_broadcasted = true;
                packets.push(entity_event_packet(self.entity_id, ENTITY_EVENT_DEATH_POOF));
                if !self.is_player {
                    self.removed = true;
                }
            }
        } else {
            self.death_broadcasted = false;
        }
        packets
    }

    pub fn all_metadata_packet(&self) -> Vec<u8> {
        self.metadata.all_packet()
    }

    pub fn dirty_metadata_packet(&mut self) -> Option<Vec<u8>> {
        self.metadata.dirty_packet()
    }

    pub fn equipment_packet(&self) -> Vec<u8> {
        set_equipment_packet(self.entity_id, &self.equipment)
    }

    pub fn set_equipment(&mut self, slot: EquipmentSlot, stack: ItemStack) -> bool {
        let changed = self.equipment.get(slot) != &stack;
        self.equipment.set(slot, stack);
        changed
    }
}

/// Living entities keyed by network id, with health/damage helpers.
#[derive(Debug, Default)]
pub struct LivingEntityStore {
    entities: HashMap<i32, LivingEntity>,
}

impl LivingEntityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn insert(&mut self, entity: LivingEntity) -> Option<LivingEntity> {
        self.entities.insert(entity.entity_id, entity)
    }

    pub fn remove(&mut self, entity_id: i32) -> Option<LivingEntity> {
        self.entities.remove(&entity_id)
    }

    pub fn get(&self, entity_id: i32) -> Option<&LivingEntity> {
        self.entities.get(&entity_id)
    }

    pub fn get_mut(&mut self, entity_id: i32) -> Option<&mut LivingEntity> {
        self.entities.get_mut(&entity_id)
    }

    pub fn entry(&mut self, entity_id: i32) -> &mut LivingEntity {
        self.entities
            .entry(entity_id)
            .or_insert_with(|| LivingEntity::new(entity_id))
    }

    pub fn entry_player(&mut self, entity_id: i32) -> &mut LivingEntity {
        self.entities
            .entry(entity_id)
            .or_insert_with(|| LivingEntity::new_player(entity_id))
    }

    pub fn hurt(&mut self, entity_id: i32, amount: f32) -> Option<HurtResult> {
        self.get_mut(entity_id).map(|entity| entity.hurt(amount))
    }

    pub fn tick(&mut self, entity_id: i32) -> Option<Vec<Vec<u8>>> {
        self.get_mut(entity_id).map(LivingEntity::tick)
    }

    pub fn tick_all(&mut self) -> Vec<Vec<u8>> {
        self.entities
            .values_mut()
            .flat_map(LivingEntity::tick)
            .collect()
    }

    pub fn all_metadata_packet(&self, entity_id: i32) -> Option<Vec<u8>> {
        self.get(entity_id).map(LivingEntity::all_metadata_packet)
    }

    pub fn dirty_metadata_packet(&mut self, entity_id: i32) -> Option<Vec<u8>> {
        self.get_mut(entity_id)
            .and_then(LivingEntity::dirty_metadata_packet)
    }

    pub fn equipment_packet(&self, entity_id: i32) -> Option<Vec<u8>> {
        self.get(entity_id).map(LivingEntity::equipment_packet)
    }

    pub fn set_equipment(
        &mut self,
        entity_id: i32,
        slot: EquipmentSlot,
        stack: ItemStack,
    ) -> Option<bool> {
        self.get_mut(entity_id)
            .map(|entity| entity.set_equipment(slot, stack))
    }
}

static LIVING_ENTITIES: LazyLock<Mutex<LivingEntityStore>> =
    LazyLock::new(|| Mutex::new(LivingEntityStore::new()));

/// Process-wide living-entity store for the existing process-wide entity map.
pub fn living_entities() -> MutexGuard<'static, LivingEntityStore> {
    LIVING_ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn register_living(entity_id: i32) {
    living_entities().entry(entity_id);
}

pub fn register_player(entity_id: i32) {
    living_entities().entry_player(entity_id);
}

pub fn remove_living(entity_id: i32) -> Option<LivingEntity> {
    living_entities().remove(entity_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_ids_match_java() {
        assert_eq!(Pose::Standing.id(), 0);
        assert_eq!(Pose::Dying.id(), 7);
        assert_eq!(Pose::Inhaling.id(), 17);
        assert_eq!(Pose::from_id(5), Some(Pose::Crouching));
        assert_eq!(Pose::from_id(18), None);
        assert_eq!(Pose::Crouching.name(), "crouching");
    }

    #[test]
    fn metadata_dirty_only_tracks_changes() {
        let mut metadata = EntityMetadata::new_living(7);
        assert!(!metadata.is_dirty());
        assert!(metadata.pack_dirty().is_none());

        assert!(metadata.set_health(18.0).unwrap());
        assert_eq!(metadata.dirty_indices().collect::<Vec<_>>(), vec![9]);
        let values = metadata.pack_dirty().unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].index, DATA_HEALTH);
        assert_eq!(values[0].value, MetadataValue::Float(18.0));
        assert!(metadata.pack_dirty().is_none());

        assert!(!metadata.set_health(18.0).unwrap());
        assert!(!metadata.is_dirty());
    }

    #[test]
    fn metadata_rejects_serializer_changes() {
        let mut metadata = EntityMetadata::new_living(7);
        let error = metadata
            .set(DATA_HEALTH, MetadataValue::Byte(1))
            .unwrap_err();
        match error {
            MetadataError::SerializerMismatch {
                index,
                expected,
                actual,
            } => {
                assert_eq!(index, DATA_HEALTH);
                assert_eq!(expected, EntityDataSerializer::Float);
                assert_eq!(actual, EntityDataSerializer::Byte);
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn equipment_slot_wire_order_matches_ordinals() {
        assert_eq!(EquipmentSlot::MainHand.ordinal(), 0);
        assert_eq!(EquipmentSlot::OffHand.ordinal(), 1);
        assert_eq!(EquipmentSlot::Head.ordinal(), 5);
        assert_eq!(EquipmentSlot::Saddle.ordinal(), 7);
        assert!(EquipmentSlot::Head.is_armor());
        assert!(EquipmentSlot::MainHand.is_hand());
    }

    #[test]
    fn attribute_defaults_match_vanilla() {
        let living = AttributeMap::living();
        assert_eq!(living.max_health(), 20.0);
        assert_eq!(living.get(Attribute::AttackDamage), 2.0);
        assert_eq!(living.get(Attribute::MovementSpeed), 0.7);
        let player = AttributeMap::player();
        assert_eq!(player.get(Attribute::AttackDamage), 1.0);
        assert_eq!(player.get(Attribute::MovementSpeed), 0.1);
    }

    #[test]
    fn set_health_clamps_and_syncs_metadata() {
        let mut entity = LivingEntity::new(7);
        assert_eq!(entity.health, 20.0);
        assert!(!entity.set_health(25.0));
        assert_eq!(entity.health, 20.0);
        assert!(!entity.metadata.is_dirty());
        assert!(entity.set_health(-3.0));
        assert_eq!(entity.health, 0.0);
        assert_eq!(entity.metadata.get_health(), Some(0.0));
    }
}
