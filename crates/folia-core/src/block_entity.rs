//! Typed block-entity storage and chunk NBT round-tripping.
//!
//! The modern chunk format stores block entities in the top-level
//! `block_entities` list (older writes used `BlockEntities`).  This module
//! decodes that list into a position-keyed [`BlockEntityMap`], models the
//! chest, furnace, sign, and spawner types the server needs today, and writes
//! the map back.
//!
//! Two compatibility rules drive the design:
//!
//! * A block-entity type this port does not model is preserved as an
//!   [`UnknownBlockEntity`] containing the complete original compound.  It is
//!   written back byte-for-byte instead of being deleted.
//! * Every typed entity keeps the fields it did not decode in `extras`, so a
//!   save does not drop plugin data (`PublicBukkitValues`), custom names,
//!   Paper loot data, or fields added by a newer version.
//!
//! Persistence is owned here.  [`crate::crafting::FurnaceState`] is used only
//! as a behaviour model: [`FurnaceBlockEntity::tick`] copies state in, advances
//! it, and copies the result back, but only vanilla's persisted fields are
//! ever written to NBT.

use crate::crafting::{FURNACE_FUEL_SLOT, FURNACE_INPUT_SLOT, FURNACE_RESULT_SLOT};
use crate::crafting::{FurnaceKind, FurnaceState};
use crate::item_components::{ItemStack, item_stack_from_nbt, item_stack_to_nbt};
use crate::loot::RecipeCatalog;
use crate::nbt::Tag;
use std::collections::BTreeMap;
use std::io;

/// The 1.21.11 chunk key for the block-entity list.
pub const BLOCK_ENTITIES_KEY: &str = "block_entities";
/// Pre-1.20.5 chunks used this key; it is still read for compatibility.
pub const LEGACY_BLOCK_ENTITIES_KEY: &str = "BlockEntities";

pub const CHEST_SLOTS: usize = 27;
pub const FURNACE_SLOTS: usize = 3;

/// Absolute block position of a block entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockEntityPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockEntityPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn chunk_x(self) -> i32 {
        self.x.div_euclid(16)
    }

    pub fn chunk_z(self) -> i32 {
        self.z.div_euclid(16)
    }

    pub fn local_x(self) -> usize {
        self.x.rem_euclid(16) as usize
    }

    pub fn local_z(self) -> usize {
        self.z.rem_euclid(16) as usize
    }
}

/// Block entities of one chunk, ordered by position for deterministic saves.
pub type BlockEntityMap = BTreeMap<BlockEntityPos, BlockEntity>;

/// One persisted block entity.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockEntity {
    Chest(ChestBlockEntity),
    Hopper(HopperBlockEntity),
    Furnace(FurnaceBlockEntity),
    /// Boxed because the four-line front/back text arrays make this variant
    /// much larger than the others.
    Sign(Box<SignBlockEntity>),
    Spawner(SpawnerBlockEntity),
    /// A type this port does not model.  The whole original compound is kept.
    Unknown(UnknownBlockEntity),
}

impl BlockEntity {
    /// The vanilla block-entity type id (`minecraft:chest`, ...).
    pub fn id(&self) -> &str {
        match self {
            Self::Chest(chest) => &chest.block_id,
            Self::Hopper(hopper) => &hopper.block_id,
            Self::Furnace(furnace) => furnace.block_id(),
            Self::Sign(sign) => &sign.block_id,
            Self::Spawner(spawner) => &spawner.block_id,
            Self::Unknown(unknown) => &unknown.id,
        }
    }

    pub fn pos(&self) -> BlockEntityPos {
        match self {
            Self::Chest(chest) => chest.pos,
            Self::Hopper(hopper) => hopper.pos,
            Self::Furnace(furnace) => furnace.pos,
            Self::Sign(sign) => sign.pos,
            Self::Spawner(spawner) => spawner.pos,
            Self::Unknown(unknown) => unknown.pos,
        }
    }

    /// Decode one entry of the chunk's `block_entities` list.
    pub fn from_nbt(tag: &Tag) -> io::Result<Self> {
        let fields = tag
            .as_compound()
            .ok_or_else(|| invalid("block entity must be a compound tag"))?;
        let id = normalize_key(&required_string(fields, "id", "block entity")?);
        let pos = BlockEntityPos {
            x: required_int(fields, "x", "block entity")?,
            y: required_int(fields, "y", "block entity")?,
            z: required_int(fields, "z", "block entity")?,
        };
        let extras = fields.to_vec();

        Ok(match id.as_str() {
            "minecraft:chest" | "minecraft:trapped_chest" => {
                Self::Chest(ChestBlockEntity::from_fields(pos, id, extras)?)
            }
            "minecraft:hopper" => Self::Hopper(HopperBlockEntity::from_fields(pos, extras)?),
            "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => {
                Self::Furnace(FurnaceBlockEntity::from_fields(pos, id, extras)?)
            }
            "minecraft:sign" | "minecraft:hanging_sign" => {
                Self::Sign(Box::new(SignBlockEntity::from_fields(pos, id, extras)?))
            }
            "minecraft:spawner" => Self::Spawner(SpawnerBlockEntity::from_fields(pos, extras)?),
            _ => Self::Unknown(UnknownBlockEntity {
                pos,
                id,
                data: tag.clone(),
            }),
        })
    }

    /// Encode this entity back to its chunk-list compound.
    pub fn to_nbt(&self) -> io::Result<Tag> {
        match self {
            Self::Chest(chest) => chest.to_nbt(),
            Self::Hopper(hopper) => hopper.to_nbt(),
            Self::Furnace(furnace) => furnace.to_nbt(),
            Self::Sign(sign) => sign.to_nbt(),
            Self::Spawner(spawner) => spawner.to_nbt(),
            Self::Unknown(unknown) => Ok(unknown.data.clone()),
        }
    }
}

/// A chest (or trapped chest) with its 27 item slots and loot-table fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ChestBlockEntity {
    pub pos: BlockEntityPos,
    /// `minecraft:chest` or `minecraft:trapped_chest`.
    pub block_id: String,
    pub items: Vec<ItemStack>,
    pub loot_table: Option<String>,
    pub loot_table_seed: i64,
    extras: Vec<(String, Tag)>,
}

impl ChestBlockEntity {
    pub fn new(pos: BlockEntityPos) -> Self {
        Self::with_id(pos, "minecraft:chest")
    }

    pub fn with_id(pos: BlockEntityPos, block_id: impl Into<String>) -> Self {
        Self {
            pos,
            block_id: normalize_key(&block_id.into()),
            items: vec![ItemStack::empty(); CHEST_SLOTS],
            loot_table: None,
            loot_table_seed: 0,
            extras: Vec::new(),
        }
    }

    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.items.get(index)
    }

    /// Replace one slot. Out-of-range slots are rejected without changing
    /// anything, mirroring `Container.setItem` bounds behaviour.
    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> bool {
        let Some(slot) = self.items.get_mut(index) else {
            return false;
        };
        *slot = stack;
        true
    }

    pub fn set_loot_table(&mut self, table: Option<String>, seed: i64) {
        self.loot_table = table.map(|table| normalize_key(&table));
        self.loot_table_seed = seed;
    }

    /// Rolls the stored loot table once and fills empty slots, then clears the
    /// loot fields. Returns `false` when there is no table or the id is unknown.
    ///
    /// Vanilla distributes stacks into random empty slots; this first-fit order
    /// is deterministic and documented as an approximation.
    pub fn fill_loot(
        &mut self,
        catalog: &crate::loot::LootCatalog,
        random: &mut dyn folia_random::RandomSource,
    ) -> bool {
        let Some(table_id) = self.loot_table.clone() else {
            return false;
        };
        let Some(table) = catalog.table(&table_id) else {
            return false;
        };
        let context = crate::loot::LootContext::new();
        let roll = crate::loot::roll_table(catalog, table, random, &context);
        for stack in roll.stacks {
            let Some(slot) = self.items.iter_mut().find(|slot| slot.is_empty()) else {
                break;
            };
            *slot = stack;
        }
        self.loot_table = None;
        self.loot_table_seed = 0;
        true
    }

    pub fn extra_fields(&self) -> &[(String, Tag)] {
        &self.extras
    }

    fn from_fields(
        pos: BlockEntityPos,
        block_id: String,
        extras: Vec<(String, Tag)>,
    ) -> io::Result<Self> {
        let items = decode_items(&extras, CHEST_SLOTS)?;
        let loot_table = optional_string(&extras, "LootTable")?.map(|table| normalize_key(&table));
        let loot_table_seed = optional_long(&extras, "LootTableSeed", 0)?;
        Ok(Self {
            pos,
            block_id,
            items,
            loot_table,
            loot_table_seed,
            extras,
        })
    }

    fn to_nbt(&self) -> io::Result<Tag> {
        let mut fields = self.extras.clone();
        write_identity(&mut fields, &self.block_id, self.pos);
        set_field(&mut fields, "Items", encode_items(&self.items));
        match &self.loot_table {
            Some(table) => set_field(&mut fields, "LootTable", Tag::String(table.clone())),
            None => remove_field(&mut fields, "LootTable"),
        }
        if self.loot_table_seed == 0 {
            remove_field(&mut fields, "LootTableSeed");
        } else {
            set_field(
                &mut fields,
                "LootTableSeed",
                Tag::Long(self.loot_table_seed),
            );
        }
        Ok(Tag::Compound(fields))
    }
}

/// Number of slots in a hopper (`HopperBlockEntity.CONTAINER_SIZE`).
pub const HOPPER_SLOTS: usize = 5;

/// A hopper with its five slots and transfer cooldown.
#[derive(Debug, Clone, PartialEq)]
pub struct HopperBlockEntity {
    pub pos: BlockEntityPos,
    pub block_id: String,
    pub items: Vec<ItemStack>,
    pub transfer_cooldown: i32,
    extras: Vec<(String, Tag)>,
}

impl HopperBlockEntity {
    pub fn new(pos: BlockEntityPos) -> Self {
        Self {
            pos,
            block_id: "minecraft:hopper".to_owned(),
            items: vec![ItemStack::empty(); HOPPER_SLOTS],
            transfer_cooldown: 0,
            extras: Vec::new(),
        }
    }

    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.items.get(index)
    }

    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> bool {
        let Some(slot) = self.items.get_mut(index) else {
            return false;
        };
        *slot = stack;
        true
    }

    /// Advances the cooldown one tick and returns whether a transfer is ready.
    pub fn tick(&mut self) -> bool {
        if self.transfer_cooldown > 0 {
            self.transfer_cooldown -= 1;
        }
        self.transfer_cooldown == 0
    }

    fn from_fields(pos: BlockEntityPos, extras: Vec<(String, Tag)>) -> io::Result<Self> {
        Ok(Self {
            pos,
            block_id: "minecraft:hopper".to_owned(),
            items: decode_items(&extras, HOPPER_SLOTS)?,
            transfer_cooldown: optional_int(&extras, "TransferCooldown", 0)?,
            extras,
        })
    }

    fn to_nbt(&self) -> io::Result<Tag> {
        let mut fields = self.extras.clone();
        write_identity(&mut fields, &self.block_id, self.pos);
        set_field(&mut fields, "Items", encode_items(&self.items));
        if self.transfer_cooldown == 0 {
            remove_field(&mut fields, "TransferCooldown");
        } else {
            set_field(
                &mut fields,
                "TransferCooldown",
                Tag::Int(self.transfer_cooldown),
            );
        }
        Ok(Tag::Compound(fields))
    }
}

/// Moves exactly one item from `source` into `target`, merging into a
/// compatible stack first and then using an empty slot. Returns whether an
/// item moved. This is the slot-transfer core of `HopperBlockEntity`.
pub fn move_one_item(source: &mut [ItemStack], target: &mut [ItemStack]) -> bool {
    for source_stack in source.iter_mut() {
        if source_stack.is_empty() {
            continue;
        }
        let max_stack = crate::item_prototypes::prototype_or_default(&source_stack.item)
            .max_stack_size
            .max(1);
        let item = source_stack.item.clone();
        let components = source_stack.components.clone();
        let mut moved = false;
        for target_stack in target.iter_mut() {
            if target_stack.item == item
                && target_stack.components == components
                && target_stack.count < max_stack
            {
                target_stack.count += 1;
                moved = true;
                break;
            }
        }
        if !moved && let Some(empty) = target.iter_mut().find(|slot| slot.is_empty()) {
            *empty = ItemStack {
                item: item.clone(),
                count: 1,
                components: components.clone(),
            };
            moved = true;
        }
        if moved {
            source_stack.count -= 1;
            if source_stack.count <= 0 {
                *source_stack = ItemStack::empty();
            }
            return true;
        }
    }
    false
}

/// A furnace, blast furnace, or smoker.
///
/// NBT stores the four timers as shorts and the used-recipe map as
/// `RecipesUsed`.  `experience` is a runtime convenience: vanilla derives XP
/// from `RecipesUsed` when a player extracts, so it is never persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct FurnaceBlockEntity {
    pub pos: BlockEntityPos,
    pub kind: FurnaceKind,
    pub items: Vec<ItemStack>,
    pub cooking_time_spent: i32,
    pub cooking_total_time: i32,
    pub lit_time_remaining: i32,
    pub lit_total_time: i32,
    pub recipes_used: BTreeMap<String, i32>,
    pub cook_speed_multiplier: f64,
    /// Runtime-only accumulated experience. Not written to NBT.
    pub experience: f32,
    extras: Vec<(String, Tag)>,
}

impl FurnaceBlockEntity {
    pub fn new(pos: BlockEntityPos, kind: FurnaceKind) -> Self {
        Self {
            pos,
            kind,
            items: vec![ItemStack::empty(); FURNACE_SLOTS],
            cooking_time_spent: 0,
            cooking_total_time: 0,
            lit_time_remaining: 0,
            lit_total_time: 0,
            recipes_used: BTreeMap::new(),
            cook_speed_multiplier: 1.0,
            experience: 0.0,
            extras: Vec::new(),
        }
    }

    pub fn block_id(&self) -> &'static str {
        furnace_id(self.kind)
    }

    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.items.get(index)
    }

    /// Replace one slot. Changing the input resets cooking progress, matching
    /// `AbstractFurnaceBlockEntity.setItem(0, ...)`.
    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> bool {
        let Some(slot) = self.items.get_mut(index) else {
            return false;
        };
        *slot = stack;
        if index == FURNACE_INPUT_SLOT {
            self.cooking_time_spent = 0;
            self.cooking_total_time = 0;
        }
        true
    }

    /// Build the crafting behaviour model from persisted state.
    pub fn furnace_state(&self) -> FurnaceState {
        let mut state = FurnaceState::new(self.kind);
        state.input = self.items[FURNACE_INPUT_SLOT].clone();
        state.fuel = self.items[FURNACE_FUEL_SLOT].clone();
        state.output = self.items[FURNACE_RESULT_SLOT].clone();
        state.lit_time_remaining = self.lit_time_remaining;
        state.lit_total_time = self.lit_total_time;
        state.cooking_timer = self.cooking_time_spent;
        state.cooking_total_time = self.cooking_total_time;
        state.experience = self.experience;
        state.recipes_used = self.recipes_used.clone();
        state
    }

    /// Copy a behaviour-model state back into the persisted fields.
    pub fn apply_furnace_state(&mut self, state: &FurnaceState) {
        self.kind = state.kind;
        self.items[FURNACE_INPUT_SLOT] = state.input.clone();
        self.items[FURNACE_FUEL_SLOT] = state.fuel.clone();
        self.items[FURNACE_RESULT_SLOT] = state.output.clone();
        self.lit_time_remaining = state.lit_time_remaining;
        self.lit_total_time = state.lit_total_time;
        self.cooking_time_spent = state.cooking_timer;
        self.cooking_total_time = state.cooking_total_time;
        self.experience = state.experience;
        self.recipes_used = state.recipes_used.clone();
    }

    /// Advance one tick through [`FurnaceState`].
    pub fn tick(&mut self, catalog: &RecipeCatalog) {
        let mut state = self.furnace_state();
        state.tick(catalog);
        self.apply_furnace_state(&state);
    }

    pub fn extra_fields(&self) -> &[(String, Tag)] {
        &self.extras
    }

    fn from_fields(
        pos: BlockEntityPos,
        block_id: String,
        extras: Vec<(String, Tag)>,
    ) -> io::Result<Self> {
        let kind = furnace_kind(&block_id)
            .ok_or_else(|| invalid(format!("unknown furnace block entity id `{block_id}`")))?;
        Ok(Self {
            pos,
            kind,
            items: decode_items(&extras, FURNACE_SLOTS)?,
            cooking_time_spent: optional_int(&extras, "cooking_time_spent", 0)?,
            cooking_total_time: optional_int(&extras, "cooking_total_time", 0)?,
            lit_time_remaining: optional_int(&extras, "lit_time_remaining", 0)?,
            lit_total_time: optional_int(&extras, "lit_total_time", 0)?,
            recipes_used: decode_recipes_used(&extras)?,
            cook_speed_multiplier: optional_double(&extras, "Paper.CookSpeedMultiplier", 1.0)?,
            experience: 0.0,
            extras,
        })
    }

    fn to_nbt(&self) -> io::Result<Tag> {
        let mut fields = self.extras.clone();
        write_identity(&mut fields, self.block_id(), self.pos);
        set_field(&mut fields, "Items", encode_items(&self.items));
        set_field(
            &mut fields,
            "cooking_time_spent",
            Tag::Short(self.cooking_time_spent as i16),
        );
        set_field(
            &mut fields,
            "cooking_total_time",
            Tag::Short(self.cooking_total_time as i16),
        );
        set_field(
            &mut fields,
            "lit_time_remaining",
            Tag::Short(self.lit_time_remaining as i16),
        );
        set_field(
            &mut fields,
            "lit_total_time",
            Tag::Short(self.lit_total_time as i16),
        );
        set_field(
            &mut fields,
            "RecipesUsed",
            encode_recipes_used(&self.recipes_used),
        );
        set_field(
            &mut fields,
            "Paper.CookSpeedMultiplier",
            Tag::Double(self.cook_speed_multiplier),
        );
        Ok(Tag::Compound(fields))
    }
}

/// One side of a sign (`front_text` or `back_text`).
///
/// Line components are stored as raw NBT so styles, click events, and
/// translation components survive even though this port only provides plain
/// text helpers.
#[derive(Debug, Clone, PartialEq)]
pub struct SignText {
    pub messages: [Tag; 4],
    pub filtered_messages: Option<[Tag; 4]>,
    pub color: String,
    pub has_glowing_text: bool,
}

impl SignText {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plain(lines: [&str; 4]) -> Self {
        Self {
            messages: [
                Tag::String(lines[0].to_owned()),
                Tag::String(lines[1].to_owned()),
                Tag::String(lines[2].to_owned()),
                Tag::String(lines[3].to_owned()),
            ],
            filtered_messages: None,
            color: "black".into(),
            has_glowing_text: false,
        }
    }

    /// Plain text of a line. Compound components using `{"text": "..."}` are
    /// also understood; other component shapes return `None`.
    pub fn line(&self, index: usize) -> Option<&str> {
        let tag = self.messages.get(index)?;
        match tag {
            Tag::String(text) => Some(text),
            Tag::Compound(_) => tag.get("text").and_then(Tag::as_str),
            _ => None,
        }
    }

    pub fn set_line(&mut self, index: usize, text: impl Into<String>) -> bool {
        let Some(line) = self.messages.get_mut(index) else {
            return false;
        };
        *line = Tag::String(text.into());
        true
    }
}

impl Default for SignText {
    fn default() -> Self {
        Self {
            messages: [
                Tag::String(String::new()),
                Tag::String(String::new()),
                Tag::String(String::new()),
                Tag::String(String::new()),
            ],
            filtered_messages: None,
            color: "black".into(),
            has_glowing_text: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignBlockEntity {
    pub pos: BlockEntityPos,
    /// `minecraft:sign` or `minecraft:hanging_sign`.
    pub block_id: String,
    pub front_text: SignText,
    pub back_text: SignText,
    pub is_waxed: bool,
    extras: Vec<(String, Tag)>,
}

impl SignBlockEntity {
    pub fn new(pos: BlockEntityPos) -> Self {
        Self {
            pos,
            block_id: "minecraft:sign".into(),
            front_text: SignText::default(),
            back_text: SignText::default(),
            is_waxed: false,
            extras: Vec::new(),
        }
    }

    pub fn extra_fields(&self) -> &[(String, Tag)] {
        &self.extras
    }

    fn from_fields(
        pos: BlockEntityPos,
        block_id: String,
        extras: Vec<(String, Tag)>,
    ) -> io::Result<Self> {
        Ok(Self {
            pos,
            block_id,
            front_text: decode_sign_text(&extras, "front_text")?,
            back_text: decode_sign_text(&extras, "back_text")?,
            is_waxed: optional_bool(&extras, "is_waxed", false)?,
            extras,
        })
    }

    fn to_nbt(&self) -> io::Result<Tag> {
        let mut fields = self.extras.clone();
        write_identity(&mut fields, &self.block_id, self.pos);
        set_field(
            &mut fields,
            "front_text",
            encode_sign_text(&self.front_text),
        );
        set_field(&mut fields, "back_text", encode_sign_text(&self.back_text));
        set_field(
            &mut fields,
            "is_waxed",
            Tag::Byte(if self.is_waxed { 1 } else { 0 }),
        );
        Ok(Tag::Compound(fields))
    }
}

/// A monster spawner.  The full `SpawnData` compound and the
/// `SpawnPotentials` list are kept verbatim; the typed fields cover the values
/// the ticker and plugins inspect.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnerBlockEntity {
    pub pos: BlockEntityPos,
    pub block_id: String,
    pub entity_id: Option<String>,
    pub delay: i32,
    pub min_spawn_delay: i32,
    pub max_spawn_delay: i32,
    pub spawn_count: i32,
    pub max_nearby_entities: i32,
    pub required_player_range: i32,
    pub spawn_range: i32,
    /// Complete `SpawnData` compound, including the nested `entity` tag.
    pub spawn_data: Option<Tag>,
    /// Complete `SpawnPotentials` list.
    pub spawn_potentials: Tag,
    extras: Vec<(String, Tag)>,
}

impl SpawnerBlockEntity {
    pub fn new(pos: BlockEntityPos) -> Self {
        Self {
            pos,
            block_id: "minecraft:spawner".into(),
            entity_id: None,
            delay: 20,
            min_spawn_delay: 200,
            max_spawn_delay: 800,
            spawn_count: 4,
            max_nearby_entities: 6,
            required_player_range: 16,
            spawn_range: 4,
            spawn_data: None,
            spawn_potentials: Tag::List(Vec::new()),
            extras: Vec::new(),
        }
    }

    /// Set the entity id inside `SpawnData.entity.id`, creating the compound
    /// structure when needed.
    pub fn set_entity_id(&mut self, entity_id: impl Into<String>) {
        let entity_id = normalize_key(&entity_id.into());
        self.entity_id = Some(entity_id.clone());
        let data = self
            .spawn_data
            .get_or_insert_with(|| Tag::Compound(Vec::new()));
        set_nested_entity_id(data, entity_id);
    }

    /// Simple ticker: consume the spawn delay. Full spawner spawning needs the
    /// entity layer and is intentionally out of scope here.
    pub fn tick(&mut self) {
        if self.delay > 0 {
            self.delay -= 1;
        }
    }

    pub fn extra_fields(&self) -> &[(String, Tag)] {
        &self.extras
    }

    fn from_fields(pos: BlockEntityPos, extras: Vec<(String, Tag)>) -> io::Result<Self> {
        let spawn_data = match field(&extras, "SpawnData") {
            Some(tag) => {
                if tag.as_compound().is_none() {
                    return Err(invalid("spawner SpawnData must be a compound tag"));
                }
                Some(tag.clone())
            }
            None => None,
        };
        let entity_id = spawn_data
            .as_ref()
            .and_then(|data| data.get("entity"))
            .and_then(|entity| entity.get("id"))
            .and_then(Tag::as_str)
            .map(normalize_key);
        let spawn_potentials = match field(&extras, "SpawnPotentials") {
            Some(tag) => {
                if tag.as_list().is_none() {
                    return Err(invalid("spawner SpawnPotentials must be a list tag"));
                }
                tag.clone()
            }
            None => Tag::List(Vec::new()),
        };
        let delay = match field(&extras, "Paper.Delay") {
            Some(tag) => numeric_int(tag, "Paper.Delay")?,
            None => optional_int(&extras, "Delay", 20)?,
        };
        let min_spawn_delay = match field(&extras, "Paper.MinSpawnDelay") {
            Some(tag) => numeric_int(tag, "Paper.MinSpawnDelay")?,
            None => optional_int(&extras, "MinSpawnDelay", 200)?,
        };
        let max_spawn_delay = match field(&extras, "Paper.MaxSpawnDelay") {
            Some(tag) => numeric_int(tag, "Paper.MaxSpawnDelay")?,
            None => optional_int(&extras, "MaxSpawnDelay", 800)?,
        };

        Ok(Self {
            pos,
            block_id: "minecraft:spawner".into(),
            entity_id,
            delay,
            min_spawn_delay,
            max_spawn_delay,
            spawn_count: optional_int(&extras, "SpawnCount", 4)?,
            max_nearby_entities: optional_int(&extras, "MaxNearbyEntities", 6)?,
            required_player_range: optional_int(&extras, "RequiredPlayerRange", 16)?,
            spawn_range: optional_int(&extras, "SpawnRange", 4)?,
            spawn_data,
            spawn_potentials,
            extras,
        })
    }

    fn to_nbt(&self) -> io::Result<Tag> {
        let mut fields = self.extras.clone();
        write_identity(&mut fields, &self.block_id, self.pos);

        let mut spawn_data = self.spawn_data.clone();
        if let Some(entity_id) = &self.entity_id {
            let data = spawn_data.get_or_insert_with(|| Tag::Compound(Vec::new()));
            set_nested_entity_id(data, entity_id.clone());
        }
        match spawn_data {
            Some(data) => set_field(&mut fields, "SpawnData", data),
            None => remove_field(&mut fields, "SpawnData"),
        }

        set_field(
            &mut fields,
            "SpawnPotentials",
            self.spawn_potentials.clone(),
        );
        write_spawner_delay(&mut fields, "Delay", "Paper.Delay", self.delay, 32767);
        write_spawner_delay(
            &mut fields,
            "MinSpawnDelay",
            "Paper.MinSpawnDelay",
            self.min_spawn_delay,
            32767,
        );
        write_spawner_delay(
            &mut fields,
            "MaxSpawnDelay",
            "Paper.MaxSpawnDelay",
            self.max_spawn_delay,
            32767,
        );
        set_field(
            &mut fields,
            "SpawnCount",
            Tag::Short(self.spawn_count as i16),
        );
        set_field(
            &mut fields,
            "MaxNearbyEntities",
            Tag::Short(self.max_nearby_entities as i16),
        );
        set_field(
            &mut fields,
            "RequiredPlayerRange",
            Tag::Short(self.required_player_range as i16),
        );
        set_field(
            &mut fields,
            "SpawnRange",
            Tag::Short(self.spawn_range as i16),
        );
        Ok(Tag::Compound(fields))
    }
}

/// Opaque storage for a block-entity type this port does not model.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownBlockEntity {
    pub pos: BlockEntityPos,
    pub id: String,
    /// The complete original compound, including `id`, coordinates, and any
    /// fields this port does not understand.
    pub data: Tag,
}

fn set_nested_entity_id(data: &mut Tag, entity_id: String) {
    if data.as_compound().is_none() {
        *data = Tag::Compound(Vec::new());
    }
    let mut fields = data
        .as_compound()
        .expect("data was normalized to a compound")
        .to_vec();
    set_nested_entity_id_tag(&mut fields, &entity_id);
    *data = Tag::Compound(fields);
}

fn set_nested_entity_id_tag(fields: &mut Vec<(String, Tag)>, entity_id: &str) {
    let entity_index = fields.iter().position(|(key, _)| key == "entity");
    let entity = match entity_index {
        Some(index) => &mut fields[index].1,
        None => {
            fields.push(("entity".into(), Tag::Compound(Vec::new())));
            &mut fields.last_mut().expect("entity was just pushed").1
        }
    };
    if entity.as_compound().is_none() {
        *entity = Tag::Compound(Vec::new());
    }
    let mut entity_fields = entity
        .as_compound()
        .expect("entity was normalized to a compound")
        .to_vec();
    set_field(&mut entity_fields, "id", Tag::String(entity_id.to_owned()));
    *entity = Tag::Compound(entity_fields);
}

/// A ticker runs block-entity behaviour explicitly.  The server tick loop does
/// not call this yet; `WorldStorage::tick_block_entities` is the opt-in hook.
pub trait BlockEntityTicker {
    fn tick(&mut self, entity: &mut BlockEntity);
}

/// Vanilla-shaped simple ticker: furnaces advance through `FurnaceState` when
/// a recipe catalog is supplied, spawners count down their delay, and
/// chests/signs are no-ops.
#[derive(Default)]
pub struct SimpleBlockEntityTicker<'a> {
    pub recipes: Option<&'a RecipeCatalog>,
}

impl<'a> SimpleBlockEntityTicker<'a> {
    pub fn new() -> Self {
        Self { recipes: None }
    }

    pub fn with_recipes(recipes: &'a RecipeCatalog) -> Self {
        Self {
            recipes: Some(recipes),
        }
    }
}

impl BlockEntityTicker for SimpleBlockEntityTicker<'_> {
    fn tick(&mut self, entity: &mut BlockEntity) {
        match entity {
            BlockEntity::Furnace(furnace) => {
                if let Some(recipes) = self.recipes {
                    furnace.tick(recipes);
                }
            }
            BlockEntity::Spawner(spawner) => spawner.tick(),
            BlockEntity::Hopper(hopper) => {
                hopper.tick();
            }
            BlockEntity::Chest(_) | BlockEntity::Sign(_) | BlockEntity::Unknown(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Chunk NBT
// ---------------------------------------------------------------------------

/// Decode the chunk's block-entity list.  Both the modern `block_entities` key
/// and the legacy `BlockEntities` key are read; a modern entry wins when both
/// contain the same position.
pub fn decode_block_entities(root: &Tag) -> io::Result<BlockEntityMap> {
    let mut entities = BlockEntityMap::new();
    if let Some(tag) = root.get(BLOCK_ENTITIES_KEY) {
        decode_block_entity_list(tag, &mut entities, false)?;
    }
    if let Some(tag) = root.get(LEGACY_BLOCK_ENTITIES_KEY) {
        decode_block_entity_list(tag, &mut entities, true)?;
    }
    Ok(entities)
}

fn decode_block_entity_list(
    tag: &Tag,
    entities: &mut BlockEntityMap,
    only_missing: bool,
) -> io::Result<()> {
    let list = tag
        .as_list()
        .ok_or_else(|| invalid("chunk block_entities must be a list tag"))?;
    for (index, entry) in list.iter().enumerate() {
        let entity = BlockEntity::from_nbt(entry)
            .map_err(|error| invalid(format!("block_entities[{index}]: {error}")))?;
        let position = entity.pos();
        if only_missing {
            entities.entry(position).or_insert(entity);
        } else {
            entities.insert(position, entity);
        }
    }
    Ok(())
}

/// Encode a position-keyed map as a `block_entities` list tag.
pub fn encode_block_entities(entities: &BlockEntityMap) -> io::Result<Tag> {
    let mut list = Vec::with_capacity(entities.len());
    for entity in entities.values() {
        list.push(entity.to_nbt()?);
    }
    Ok(Tag::List(list))
}

/// Replace the chunk's block-entity list with `entities`.
///
/// The legacy key is removed so a chunk cannot carry two competing lists.  An
/// empty map still writes an empty list, matching `SerializableChunkData`.
pub fn store_block_entities(root: &mut Tag, entities: &BlockEntityMap) -> io::Result<()> {
    let Tag::Compound(fields) = root else {
        return Err(invalid("chunk root must be a compound tag"));
    };
    fields.retain(|(key, _)| key != BLOCK_ENTITIES_KEY && key != LEGACY_BLOCK_ENTITIES_KEY);
    let list = encode_block_entities(entities)?;
    fields.push((BLOCK_ENTITIES_KEY.into(), list));
    Ok(())
}

// ---------------------------------------------------------------------------
// NBT helpers
// ---------------------------------------------------------------------------

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn normalize_key(key: &str) -> String {
    if key.contains(':') {
        key.to_owned()
    } else {
        format!("minecraft:{key}")
    }
}

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn set_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    match fields.iter().position(|(key, _)| key == name) {
        Some(index) => fields[index].1 = value,
        None => fields.push((name.to_owned(), value)),
    }
}

fn remove_field(fields: &mut Vec<(String, Tag)>, name: &str) {
    fields.retain(|(key, _)| key != name);
}

fn write_identity(fields: &mut Vec<(String, Tag)>, id: &str, pos: BlockEntityPos) {
    set_field(fields, "id", Tag::String(id.to_owned()));
    set_field(fields, "x", Tag::Int(pos.x));
    set_field(fields, "y", Tag::Int(pos.y));
    set_field(fields, "z", Tag::Int(pos.z));
}

fn numeric_i64(tag: &Tag) -> Option<i64> {
    match tag {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        Tag::Float(value) => Some(*value as i64),
        Tag::Double(value) => Some(*value as i64),
        _ => None,
    }
}

fn numeric_f64(tag: &Tag) -> Option<f64> {
    match tag {
        Tag::Byte(value) => Some(f64::from(*value)),
        Tag::Short(value) => Some(f64::from(*value)),
        Tag::Int(value) => Some(f64::from(*value)),
        Tag::Long(value) => Some(*value as f64),
        Tag::Float(value) => Some(f64::from(*value)),
        Tag::Double(value) => Some(*value),
        _ => None,
    }
}

fn numeric_int(tag: &Tag, field_name: &str) -> io::Result<i32> {
    numeric_i64(tag)
        .map(|value| value as i32)
        .ok_or_else(|| invalid(format!("{field_name} must be a numeric tag")))
}

fn required_int(fields: &[(String, Tag)], name: &str, owner: &str) -> io::Result<i32> {
    let tag = field(fields, name)
        .ok_or_else(|| invalid(format!("{owner} is missing integer field {name}")))?;
    numeric_int(tag, name)
}

fn optional_int(fields: &[(String, Tag)], name: &str, default: i32) -> io::Result<i32> {
    match field(fields, name) {
        Some(tag) => numeric_int(tag, name),
        None => Ok(default),
    }
}

fn optional_long(fields: &[(String, Tag)], name: &str, default: i64) -> io::Result<i64> {
    match field(fields, name) {
        Some(tag) => {
            numeric_i64(tag).ok_or_else(|| invalid(format!("{name} must be a numeric tag")))
        }
        None => Ok(default),
    }
}

fn optional_double(fields: &[(String, Tag)], name: &str, default: f64) -> io::Result<f64> {
    match field(fields, name) {
        Some(tag) => {
            numeric_f64(tag).ok_or_else(|| invalid(format!("{name} must be a numeric tag")))
        }
        None => Ok(default),
    }
}

fn optional_bool(fields: &[(String, Tag)], name: &str, default: bool) -> io::Result<bool> {
    match field(fields, name) {
        Some(tag) => numeric_i64(tag)
            .map(|value| value != 0)
            .ok_or_else(|| invalid(format!("{name} must be a numeric tag"))),
        None => Ok(default),
    }
}

fn required_string(fields: &[(String, Tag)], name: &str, owner: &str) -> io::Result<String> {
    field(fields, name)
        .and_then(Tag::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("{owner} is missing string field {name}")))
}

fn optional_string(fields: &[(String, Tag)], name: &str) -> io::Result<Option<String>> {
    match field(fields, name) {
        Some(tag) => tag
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| invalid(format!("{name} must be a string tag"))),
        None => Ok(None),
    }
}

fn furnace_id(kind: FurnaceKind) -> &'static str {
    match kind {
        FurnaceKind::Furnace => "minecraft:furnace",
        FurnaceKind::BlastFurnace => "minecraft:blast_furnace",
        FurnaceKind::Smoker => "minecraft:smoker",
        FurnaceKind::Campfire => "minecraft:campfire",
    }
}

fn furnace_kind(id: &str) -> Option<FurnaceKind> {
    match id {
        "minecraft:furnace" => Some(FurnaceKind::Furnace),
        "minecraft:blast_furnace" => Some(FurnaceKind::BlastFurnace),
        "minecraft:smoker" => Some(FurnaceKind::Smoker),
        _ => None,
    }
}

fn decode_items(fields: &[(String, Tag)], slots: usize) -> io::Result<Vec<ItemStack>> {
    let mut items = vec![ItemStack::empty(); slots];
    let Some(tag) = field(fields, "Items") else {
        return Ok(items);
    };
    let list = tag
        .as_list()
        .ok_or_else(|| invalid("Items must be a list tag"))?;
    for (index, entry) in list.iter().enumerate() {
        let entry_fields = entry
            .as_compound()
            .ok_or_else(|| invalid(format!("Items[{index}] must be a compound tag")))?;
        let slot = optional_int(entry_fields, "Slot", 0)?;
        if slot < 0 || slot as usize >= slots {
            continue;
        }
        let stack = item_stack_from_nbt(entry)
            .map_err(|error| invalid(format!("Items[{index}]: {error}")))?;
        if !stack.is_empty() {
            items[slot as usize] = stack;
        }
    }
    Ok(items)
}

fn encode_items(items: &[ItemStack]) -> Tag {
    let mut list = Vec::new();
    for (slot, stack) in items.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let Tag::Compound(stack_fields) = item_stack_to_nbt(stack) else {
            // `item_stack_to_nbt` always returns a compound; keep the encoder
            // total rather than panicking on a future change.
            continue;
        };
        let mut entry = Vec::with_capacity(stack_fields.len() + 1);
        entry.push(("Slot".into(), Tag::Byte(slot as i8)));
        entry.extend(stack_fields);
        list.push(Tag::Compound(entry));
    }
    Tag::List(list)
}

fn decode_recipes_used(fields: &[(String, Tag)]) -> io::Result<BTreeMap<String, i32>> {
    let Some(tag) = field(fields, "RecipesUsed") else {
        return Ok(BTreeMap::new());
    };
    let entries = tag
        .as_compound()
        .ok_or_else(|| invalid("RecipesUsed must be a compound tag"))?;
    let mut recipes = BTreeMap::new();
    for (recipe, value) in entries {
        recipes.insert(recipe.clone(), numeric_int(value, recipe)?);
    }
    Ok(recipes)
}

fn encode_recipes_used(recipes: &BTreeMap<String, i32>) -> Tag {
    Tag::Compound(
        recipes
            .iter()
            .map(|(recipe, count)| (recipe.clone(), Tag::Int(*count)))
            .collect(),
    )
}

fn decode_sign_text(fields: &[(String, Tag)], name: &str) -> io::Result<SignText> {
    let Some(tag) = field(fields, name) else {
        return Ok(SignText::default());
    };
    let text = tag
        .as_compound()
        .ok_or_else(|| invalid(format!("sign {name} must be a compound tag")))?;
    Ok(SignText {
        messages: decode_component_lines(text, "messages")?,
        filtered_messages: match field(text, "filtered_messages") {
            Some(_) => Some(decode_component_lines(text, "filtered_messages")?),
            None => None,
        },
        color: optional_string(text, "color")?.unwrap_or_else(|| "black".into()),
        has_glowing_text: optional_bool(text, "has_glowing_text", false)?,
    })
}

fn decode_component_lines(fields: &[(String, Tag)], name: &str) -> io::Result<[Tag; 4]> {
    let list = field(fields, name)
        .ok_or_else(|| invalid(format!("sign text is missing {name}")))?
        .as_list()
        .ok_or_else(|| invalid(format!("sign text {name} must be a list tag")))?;
    if list.len() != 4 {
        return Err(invalid(format!(
            "sign text {name} must contain exactly 4 lines, found {}",
            list.len()
        )));
    }
    Ok([
        list[0].clone(),
        list[1].clone(),
        list[2].clone(),
        list[3].clone(),
    ])
}

fn encode_sign_text(text: &SignText) -> Tag {
    let mut fields = vec![("messages".into(), Tag::List(text.messages.to_vec()))];
    if let Some(filtered) = &text.filtered_messages {
        fields.push(("filtered_messages".into(), Tag::List(filtered.to_vec())));
    }
    fields.push(("color".into(), Tag::String(text.color.clone())));
    fields.push((
        "has_glowing_text".into(),
        Tag::Byte(if text.has_glowing_text { 1 } else { 0 }),
    ));
    Tag::Compound(fields)
}

fn write_spawner_delay(
    fields: &mut Vec<(String, Tag)>,
    short_name: &str,
    paper_name: &str,
    value: i32,
    max_short: i32,
) {
    set_field(fields, short_name, Tag::Short(value.min(max_short) as i16));
    if value > max_short {
        set_field(fields, paper_name, Tag::Int(value));
    } else {
        remove_field(fields, paper_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos() -> BlockEntityPos {
        BlockEntityPos::new(3, 64, -7)
    }

    fn empty_chunk_tag() -> Tag {
        Tag::Compound(vec![
            ("xPos".into(), Tag::Int(0)),
            ("zPos".into(), Tag::Int(0)),
            ("sections".into(), Tag::List(Vec::new())),
        ])
    }

    #[test]
    fn chest_round_trips_items_and_loot_table() {
        let mut chest = ChestBlockEntity::new(pos());
        let stack = ItemStack::new("minecraft:diamond", 3);
        assert!(chest.set_slot(0, stack.clone()));
        chest.set_loot_table(Some("chests/simple_dungeon".into()), 42);
        let mut entities = BlockEntityMap::new();
        entities.insert(pos(), BlockEntity::Chest(chest));

        let mut root = empty_chunk_tag();
        store_block_entities(&mut root, &entities).unwrap();
        let decoded = decode_block_entities(&root).unwrap();
        let Some(BlockEntity::Chest(chest)) = decoded.get(&pos()) else {
            panic!("chest did not round-trip");
        };
        assert_eq!(chest.items.len(), CHEST_SLOTS);
        assert_eq!(chest.slot(0), Some(&stack));
        assert_eq!(
            chest.loot_table.as_deref(),
            Some("minecraft:chests/simple_dungeon")
        );
        assert_eq!(chest.loot_table_seed, 42);
    }

    #[test]
    fn unknown_entities_keep_their_complete_compound() {
        let original = Tag::Compound(vec![
            ("id".into(), Tag::String("mod:magic_block".into())),
            ("x".into(), Tag::Int(3)),
            ("y".into(), Tag::Int(64)),
            ("z".into(), Tag::Int(-7)),
            (
                "ModData".into(),
                Tag::Compound(vec![
                    ("level".into(), Tag::Int(9)),
                    ("tags".into(), Tag::List(vec![Tag::String("a".into())])),
                ]),
            ),
        ]);
        let mut entities = BlockEntityMap::new();
        entities.insert(
            pos(),
            BlockEntity::Unknown(UnknownBlockEntity {
                pos: pos(),
                id: "mod:magic_block".into(),
                data: original.clone(),
            }),
        );
        let mut root = empty_chunk_tag();
        store_block_entities(&mut root, &entities).unwrap();
        let decoded = decode_block_entities(&root).unwrap();
        let Some(BlockEntity::Unknown(unknown)) = decoded.get(&pos()) else {
            panic!("unknown entity was not preserved");
        };
        assert_eq!(unknown.data, original);
        let mut second = empty_chunk_tag();
        store_block_entities(&mut second, &decoded).unwrap();
        assert_eq!(
            root.get(BLOCK_ENTITIES_KEY),
            second.get(BLOCK_ENTITIES_KEY),
            "an unmodified unknown entity is byte-identical after a save"
        );
    }

    #[test]
    fn malformed_block_entity_lists_fail() {
        let root = Tag::Compound(vec![(
            BLOCK_ENTITIES_KEY.into(),
            Tag::List(vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(1)),
                ("y".into(), Tag::Int(2)),
            ])]),
        )]);
        assert!(decode_block_entities(&root).is_err());

        let root = Tag::Compound(vec![(
            BLOCK_ENTITIES_KEY.into(),
            Tag::List(vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(1)),
                ("y".into(), Tag::Int(2)),
                ("z".into(), Tag::Int(3)),
                ("Items".into(), Tag::String("not a list".into())),
            ])]),
        )]);
        assert!(decode_block_entities(&root).is_err());
    }
}
