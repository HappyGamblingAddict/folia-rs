//! Item stacks and the 1.21.11 data-component model.
//!
//! Vanilla stores a stack as an item id, a count, and a *patch* over the
//! item's prototype components. This module models the patch explicitly:
//! components added by the stack are kept in insertion order and components
//! removed from the prototype are tracked separately.
//!
//! Unknown components are preserved opaquely. NBT round-trips keep their exact
//! tag, so loading a saved inventory and writing it back never drops component
//! data this port does not understand yet. Network encoding is stricter: a
//! component whose stream codec is not ported is omitted from the payload and
//! reported by [`NetworkEncodingReport`] instead of writing malformed bytes.
//!
//! ## Component coverage
//!
//! * Fully ported (NBT + network): `custom_name`, `item_name`, `lore`,
//!   `damage`, `max_damage`, `max_stack_size`, `enchantments`,
//!   `stored_enchantments`, `unbreakable`, `custom_data`, `container`, `food`,
//!   `repair_cost`, `rarity`.
//! * NBT-only (network omitted): `attribute_modifiers`, `potion_contents`,
//!   `tool`, plus every component this port does not know.
//! * Unknown NBT components are stored as [`ComponentValue::Opaque`] and are
//!   re-encoded byte-for-byte at the NBT level.

use crate::nbt::{self, Tag};
use crate::protocol::{PacketReader, PacketWriter};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::OnceLock;

/// Vanilla's fallback stack size when a stack has no `max_stack_size`
/// component. Nearly every item ships `max_stack_size: 64`; per-item prototype
/// defaults are not part of this slice, so 64 keeps the existing inventory
/// behaviour and can be overridden by an explicit component.
pub const DEFAULT_MAX_STACK_SIZE: i32 = 64;

/// Component registry ids derived from the registration order in
/// `net.minecraft.core.component.DataComponents` (1.21.11). The ids are
/// protocol registry ids used by `DataComponentType.STREAM_CODEC`.
pub const KNOWN_COMPONENT_TYPES: &[(&str, i32)] = &[
    ("minecraft:custom_data", 0),
    ("minecraft:max_stack_size", 1),
    ("minecraft:max_damage", 2),
    ("minecraft:damage", 3),
    ("minecraft:unbreakable", 4),
    ("minecraft:custom_name", 6),
    ("minecraft:item_name", 9),
    ("minecraft:lore", 11),
    ("minecraft:rarity", 12),
    ("minecraft:enchantments", 13),
    ("minecraft:attribute_modifiers", 16),
    ("minecraft:repair_cost", 19),
    ("minecraft:food", 23),
    ("minecraft:tool", 28),
    ("minecraft:stored_enchantments", 41),
    ("minecraft:potion_contents", 49),
    ("minecraft:container", 73),
];

pub fn normalize_key(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

pub fn component_id(name: &str) -> Option<i32> {
    let name = normalize_key(name);
    KNOWN_COMPONENT_TYPES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, id)| *id)
}

pub fn component_name(id: i32) -> Option<&'static str> {
    KNOWN_COMPONENT_TYPES
        .iter()
        .find(|(_, known)| *known == id)
        .map(|(name, _)| *name)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentType {
    name: String,
    id: Option<i32>,
}

impl ComponentType {
    pub fn named(name: &str) -> Self {
        let name = normalize_key(name);
        let id = component_id(&name);
        Self { name, id }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> Option<i32> {
        self.id
    }

    pub fn from_id(id: i32) -> Option<Self> {
        component_name(id).map(Self::named)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

impl Rarity {
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
            Rarity::Epic => "epic",
        }
    }

    pub fn id(self) -> i32 {
        match self {
            Rarity::Common => 0,
            Rarity::Uncommon => 1,
            Rarity::Rare => 2,
            Rarity::Epic => 3,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "common" => Some(Rarity::Common),
            "uncommon" => Some(Rarity::Uncommon),
            "rare" => Some(Rarity::Rare),
            "epic" => Some(Rarity::Epic),
            _ => None,
        }
    }

    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Rarity::Common),
            1 => Some(Rarity::Uncommon),
            2 => Some(Rarity::Rare),
            3 => Some(Rarity::Epic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeOperation {
    AddValue,
    AddMultipliedBase,
    AddMultipliedTotal,
}

impl AttributeOperation {
    fn name(self) -> &'static str {
        match self {
            AttributeOperation::AddValue => "add_value",
            AttributeOperation::AddMultipliedBase => "add_multiplied_base",
            AttributeOperation::AddMultipliedTotal => "add_multiplied_total",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "add_value" => Some(AttributeOperation::AddValue),
            "add_multiplied_base" => Some(AttributeOperation::AddMultipliedBase),
            "add_multiplied_total" => Some(AttributeOperation::AddMultipliedTotal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSlotGroup {
    Any,
    Mainhand,
    Offhand,
    Hand,
    Feet,
    Legs,
    Chest,
    Head,
    Armor,
    Body,
    Saddle,
}

impl EquipmentSlotGroup {
    fn name(self) -> &'static str {
        match self {
            EquipmentSlotGroup::Any => "any",
            EquipmentSlotGroup::Mainhand => "mainhand",
            EquipmentSlotGroup::Offhand => "offhand",
            EquipmentSlotGroup::Hand => "hand",
            EquipmentSlotGroup::Feet => "feet",
            EquipmentSlotGroup::Legs => "legs",
            EquipmentSlotGroup::Chest => "chest",
            EquipmentSlotGroup::Head => "head",
            EquipmentSlotGroup::Armor => "armor",
            EquipmentSlotGroup::Body => "body",
            EquipmentSlotGroup::Saddle => "saddle",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "any" => Some(EquipmentSlotGroup::Any),
            "mainhand" => Some(EquipmentSlotGroup::Mainhand),
            "offhand" => Some(EquipmentSlotGroup::Offhand),
            "hand" => Some(EquipmentSlotGroup::Hand),
            "feet" => Some(EquipmentSlotGroup::Feet),
            "legs" => Some(EquipmentSlotGroup::Legs),
            "chest" => Some(EquipmentSlotGroup::Chest),
            "head" => Some(EquipmentSlotGroup::Head),
            "armor" => Some(EquipmentSlotGroup::Armor),
            "body" => Some(EquipmentSlotGroup::Body),
            "saddle" => Some(EquipmentSlotGroup::Saddle),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeDisplay {
    Default,
    Hidden,
    Override(Tag),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeModifierEntry {
    pub attribute: String,
    pub id: String,
    pub amount: f64,
    pub operation: AttributeOperation,
    pub slot: EquipmentSlotGroup,
    pub display: AttributeDisplay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockSet {
    Tag(String),
    Blocks(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRule {
    pub blocks: BlockSet,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    pub default_mining_speed: f32,
    pub damage_per_block: i32,
    pub can_destroy_blocks_in_creative: bool,
}

impl Tool {
    pub fn new(rules: Vec<ToolRule>) -> Self {
        Self {
            rules,
            default_mining_speed: 1.0,
            damage_per_block: 1,
            can_destroy_blocks_in_creative: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FoodProperties {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MobEffectInstance {
    pub id: String,
    pub amplifier: i32,
    pub duration: i32,
    pub ambient: bool,
    pub show_particles: bool,
    pub show_icon: bool,
    pub hidden_effect: Option<Box<MobEffectInstance>>,
}

impl MobEffectInstance {
    pub fn new(id: impl Into<String>, duration: i32, amplifier: i32) -> Self {
        let id: String = id.into();
        Self {
            id: normalize_key(&id),
            amplifier,
            duration,
            ambient: false,
            show_particles: true,
            show_icon: true,
            hidden_effect: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PotionContents {
    pub potion: Option<String>,
    pub custom_color: Option<i32>,
    pub custom_effects: Vec<MobEffectInstance>,
    pub custom_name: Option<String>,
}

impl PotionContents {
    pub fn new(potion: impl Into<String>) -> Self {
        Self {
            potion: Some(normalize_key(&potion.into())),
            custom_color: None,
            custom_effects: Vec::new(),
            custom_name: None,
        }
    }
}

/// Vanilla `ItemContainerContents`: a list of stacks where the slot index is
/// implicit. Trailing empty slots are not stored. The vanilla codec accepts at
/// most 256 slots; this model keeps the caller's list intact rather than
/// dropping data, and the NBT/network decoders enforce the 256-slot bound.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemContainer {
    pub items: Vec<ItemStack>,
}

impl ItemContainer {
    pub fn new(items: Vec<ItemStack>) -> Self {
        let mut items = items;
        while items.last().is_some_and(ItemStack::is_empty) {
            items.pop();
        }
        Self { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.iter().all(ItemStack::is_empty)
    }

    pub fn non_empty(&self) -> impl Iterator<Item = (usize, &ItemStack)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentValue {
    CustomName(Tag),
    ItemName(Tag),
    Lore(Vec<Tag>),
    Damage(i32),
    MaxDamage(i32),
    MaxStackSize(i32),
    Enchantments(BTreeMap<String, i32>),
    StoredEnchantments(BTreeMap<String, i32>),
    AttributeModifiers(Vec<AttributeModifierEntry>),
    Unbreakable,
    CustomData(Tag),
    Container(ItemContainer),
    PotionContents(PotionContents),
    Food(FoodProperties),
    Tool(Tool),
    RepairCost(i32),
    Rarity(Rarity),
    /// A component this port does not model. The tag is preserved exactly.
    Opaque(Tag),
}

impl ComponentValue {
    pub fn component_name(&self) -> Option<&'static str> {
        match self {
            ComponentValue::CustomName(_) => Some("minecraft:custom_name"),
            ComponentValue::ItemName(_) => Some("minecraft:item_name"),
            ComponentValue::Lore(_) => Some("minecraft:lore"),
            ComponentValue::Damage(_) => Some("minecraft:damage"),
            ComponentValue::MaxDamage(_) => Some("minecraft:max_damage"),
            ComponentValue::MaxStackSize(_) => Some("minecraft:max_stack_size"),
            ComponentValue::Enchantments(_) => Some("minecraft:enchantments"),
            ComponentValue::StoredEnchantments(_) => Some("minecraft:stored_enchantments"),
            ComponentValue::AttributeModifiers(_) => Some("minecraft:attribute_modifiers"),
            ComponentValue::Unbreakable => Some("minecraft:unbreakable"),
            ComponentValue::CustomData(_) => Some("minecraft:custom_data"),
            ComponentValue::Container(_) => Some("minecraft:container"),
            ComponentValue::PotionContents(_) => Some("minecraft:potion_contents"),
            ComponentValue::Food(_) => Some("minecraft:food"),
            ComponentValue::Tool(_) => Some("minecraft:tool"),
            ComponentValue::RepairCost(_) => Some("minecraft:repair_cost"),
            ComponentValue::Rarity(_) => Some("minecraft:rarity"),
            ComponentValue::Opaque(_) => None,
        }
    }
}

/// Insertion-ordered component patch matching vanilla's
/// `PatchedDataComponentMap` patch representation. Added values and removal
/// markers share one list so the NBT/network order of the original patch is
/// preserved.
#[derive(Debug, Clone, Default)]
pub struct ComponentMap {
    entries: Vec<(ComponentType, Option<ComponentValue>)>,
}

impl PartialEq for ComponentMap {
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self.entries.iter().all(|(component, value)| {
                other.entries.iter().any(|(other_component, other_value)| {
                    component == other_component && value == other_value
                })
            })
    }
}

impl ComponentMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, component: &ComponentType) -> Option<&ComponentValue> {
        self.entries
            .iter()
            .find(|(key, _)| key == component)
            .and_then(|(_, value)| value.as_ref())
    }

    pub fn get_named(&self, name: &str) -> Option<&ComponentValue> {
        let name = normalize_key(name);
        self.entries
            .iter()
            .find(|(key, _)| key.name() == name)
            .and_then(|(_, value)| value.as_ref())
    }

    pub fn set(&mut self, value: ComponentValue) -> Option<ComponentValue> {
        let name = value.component_name()?;
        self.set_named(name, value).ok().flatten()
    }

    pub fn set_named(
        &mut self,
        name: &str,
        value: ComponentValue,
    ) -> io::Result<Option<ComponentValue>> {
        let component = ComponentType::named(name);
        if let Some(expected) = value.component_name()
            && expected != component.name()
        {
            return Err(invalid(format!(
                "component {value:?} cannot be stored as {}",
                component.name()
            )));
        }
        if let Some((_, current)) = self.entries.iter_mut().find(|(key, _)| key == &component) {
            return Ok(current.replace(value));
        }
        self.entries.push((component, Some(value)));
        Ok(None)
    }

    /// Insert an opaque tag for a component name. Used by the NBT decoder for
    /// components this port does not model.
    pub fn insert_opaque(&mut self, name: &str, tag: Tag) {
        let component = ComponentType::named(name);
        if let Some((_, current)) = self.entries.iter_mut().find(|(key, _)| key == &component) {
            *current = Some(ComponentValue::Opaque(tag));
            return;
        }
        self.entries
            .push((component, Some(ComponentValue::Opaque(tag))));
    }

    pub fn remove_named(&mut self, name: &str) -> Option<ComponentValue> {
        let component = ComponentType::named(name);
        if let Some(index) = self
            .entries
            .iter()
            .position(|(key, value)| key == &component && value.is_some())
        {
            return self.entries.remove(index).1;
        }
        self.mark_removed(name);
        None
    }

    pub fn mark_removed(&mut self, name: &str) {
        let component = ComponentType::named(name);
        if let Some((_, value)) = self.entries.iter_mut().find(|(key, _)| key == &component) {
            *value = None;
            return;
        }
        self.entries.push((component, None));
    }

    pub fn is_removed(&self, name: &str) -> bool {
        let name = normalize_key(name);
        self.entries
            .iter()
            .any(|(component, value)| component.name() == name && value.is_none())
    }

    pub fn iter_added(&self) -> impl Iterator<Item = (&ComponentType, &ComponentValue)> {
        self.entries
            .iter()
            .filter_map(|(component, value)| value.as_ref().map(|value| (component, value)))
    }

    pub fn iter_removed(&self) -> impl Iterator<Item = &ComponentType> {
        self.entries
            .iter()
            .filter_map(|(component, value)| value.is_none().then_some(component))
    }

    /// Iterate every patch entry in insertion order, including removals.
    pub fn iter_entries(&self) -> impl Iterator<Item = (&ComponentType, Option<&ComponentValue>)> {
        self.entries
            .iter()
            .map(|(component, value)| (component, value.as_ref()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStack {
    pub item: String,
    pub count: i32,
    pub components: ComponentMap,
}

impl ItemStack {
    pub fn new(item: impl Into<String>, count: i32) -> Self {
        Self {
            item: normalize_key(&item.into()),
            count,
            components: ComponentMap::new(),
        }
    }

    pub fn empty() -> Self {
        Self {
            item: String::new(),
            count: 0,
            components: ComponentMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count <= 0 || self.item.is_empty() || self.item == "minecraft:air"
    }

    pub fn get_component(&self, name: &str) -> Option<&ComponentValue> {
        self.components.get_named(name)
    }

    pub fn set_component(&mut self, value: ComponentValue) -> Option<ComponentValue> {
        self.components.set(value)
    }

    pub fn set_component_named(
        &mut self,
        name: &str,
        value: ComponentValue,
    ) -> io::Result<Option<ComponentValue>> {
        self.components.set_named(name, value)
    }

    pub fn remove_component(&mut self, name: &str) -> Option<ComponentValue> {
        self.components.remove_named(name)
    }

    pub fn mark_component_removed(&mut self, name: &str) {
        self.components.mark_removed(name);
    }

    pub fn max_stack_size(&self) -> i32 {
        match self.get_component("minecraft:max_stack_size") {
            Some(ComponentValue::MaxStackSize(size)) if *size > 0 => *size,
            _ => {
                // Vanilla's fallback for a damageable stack is 1; items that
                // stack to 64 carry `max_stack_size: 64` in their prototype.
                // Prototype data is not part of this slice, so an explicit
                // max_damage or damage component is the damageability signal.
                if self.get_component("minecraft:max_damage").is_some()
                    || self.get_component("minecraft:damage").is_some()
                {
                    1
                } else {
                    DEFAULT_MAX_STACK_SIZE
                }
            }
        }
    }

    pub fn same_item_and_components(&self, other: &Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.item == other.item
            && self.components == other.components
    }

    pub fn copy_with_count(&self, count: i32) -> Self {
        Self {
            item: self.item.clone(),
            count,
            components: self.components.clone(),
        }
    }

    /// Split `amount` from the front of the stack, preserving components.
    pub fn split(&mut self, amount: i32) -> Self {
        if self.is_empty() || amount <= 0 {
            return Self::empty();
        }
        let taken = amount.min(self.count);
        let split = self.copy_with_count(taken);
        self.count -= taken;
        if self.count <= 0 {
            *self = Self::empty();
        }
        split
    }

    /// Decrement the count while keeping the component patch intact.
    pub fn shrink(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        self.count -= amount;
        if self.count <= 0 {
            *self = Self::empty();
        }
    }

    pub fn damage(&self) -> i32 {
        match self.get_component("minecraft:damage") {
            Some(ComponentValue::Damage(value)) => *value,
            _ => 0,
        }
    }

    pub fn max_damage(&self) -> i32 {
        match self.get_component("minecraft:max_damage") {
            Some(ComponentValue::MaxDamage(value)) => *value,
            _ => 0,
        }
    }

    pub fn is_damaged(&self) -> bool {
        self.get_component("minecraft:max_damage").is_some()
            && self.get_component("minecraft:unbreakable").is_none()
            && self.get_component("minecraft:damage").is_some()
            && self.damage() > 0
    }

    pub fn is_enchanted(&self) -> bool {
        match self.get_component("minecraft:enchantments") {
            Some(ComponentValue::Enchantments(map)) => !map.is_empty(),
            _ => false,
        }
    }

    /// Level of one `minecraft:enchantments` entry, or 0 when absent.
    pub fn enchantment_level(&self, name: &str) -> i32 {
        match self.get_component("minecraft:enchantments") {
            Some(ComponentValue::Enchantments(map)) => {
                map.get(&normalize_key(name)).copied().unwrap_or(0).max(0)
            }
            _ => 0,
        }
    }
}

pub fn plain_text(text: impl Into<String>) -> Tag {
    Tag::String(text.into())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn int_tag(tag: &Tag, what: &str) -> io::Result<i32> {
    tag.as_int()
        .ok_or_else(|| invalid(format!("{what} must be an NBT int, got {:?}", tag.id())))
}

fn float_tag(tag: &Tag, what: &str) -> io::Result<f32> {
    tag.as_float()
        .ok_or_else(|| invalid(format!("{what} must be an NBT float, got {:?}", tag.id())))
}

fn string_tag<'a>(tag: &'a Tag, what: &str) -> io::Result<&'a str> {
    tag.as_str()
        .ok_or_else(|| invalid(format!("{what} must be an NBT string, got {:?}", tag.id())))
}

fn bool_tag(tag: &Tag, what: &str) -> io::Result<bool> {
    tag.as_byte()
        .map(|value| value != 0)
        .ok_or_else(|| invalid(format!("{what} must be an NBT byte, got {:?}", tag.id())))
}

fn compound_tag<'a>(tag: &'a Tag, what: &str) -> io::Result<&'a [(String, Tag)]> {
    tag.as_compound().ok_or_else(|| {
        invalid(format!(
            "{what} must be an NBT compound, got {:?}",
            tag.id()
        ))
    })
}

fn list_tag<'a>(tag: &'a Tag, what: &str) -> io::Result<&'a [Tag]> {
    tag.as_list()
        .ok_or_else(|| invalid(format!("{what} must be an NBT list, got {:?}", tag.id())))
}

fn field<'a>(entries: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    entries
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn optional_int(entries: &[(String, Tag)], name: &str) -> io::Result<Option<i32>> {
    field(entries, name)
        .map(|tag| int_tag(tag, name))
        .transpose()
}

fn optional_string(entries: &[(String, Tag)], name: &str) -> io::Result<Option<String>> {
    field(entries, name)
        .map(|tag| string_tag(tag, name).map(str::to_owned))
        .transpose()
}

fn optional_bool(entries: &[(String, Tag)], name: &str, default: bool) -> io::Result<bool> {
    field(entries, name)
        .map(|tag| bool_tag(tag, name))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn optional_float(entries: &[(String, Tag)], name: &str, default: f32) -> io::Result<f32> {
    field(entries, name)
        .map(|tag| float_tag(tag, name))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn component_tag(tag: &Tag, what: &str) -> io::Result<Tag> {
    match tag {
        Tag::String(_) | Tag::Compound(_) | Tag::List(_) => Ok(tag.clone()),
        _ => Err(invalid(format!(
            "{what} must be a component string, compound, or list"
        ))),
    }
}

fn is_component_wrapper(tag: &Tag) -> bool {
    matches!(tag, Tag::Compound(entries) if entries.len() == 1 && entries[0].0.is_empty())
}

fn unwrap_component(tag: &Tag) -> Tag {
    if let Tag::Compound(entries) = tag
        && entries.len() == 1
        && entries[0].0.is_empty()
    {
        return entries[0].1.clone();
    }
    tag.clone()
}

/// Encode a component list the way vanilla `ListTag` does: homogeneous lists
/// use their natural element type, while mixed lists use compound elements and
/// wrap non-compound entries in `{"": <tag>}`.
fn component_list(items: &[Tag]) -> Tag {
    let mut kind = None;
    let mut mixed = false;
    for item in items {
        match kind {
            None => kind = Some(item.id()),
            Some(id) if id != item.id() => {
                mixed = true;
                break;
            }
            Some(_) => {}
        }
    }
    if !mixed {
        return Tag::List(items.to_vec());
    }
    Tag::List(
        items
            .iter()
            .map(|item| match item {
                Tag::Compound(_) if !is_component_wrapper(item) => item.clone(),
                _ => Tag::Compound(vec![(String::new(), item.clone())]),
            })
            .collect(),
    )
}

fn unwrap_component_list(items: &[Tag]) -> Vec<Tag> {
    items.iter().map(unwrap_component).collect()
}

/// Convert a component value to its vanilla NBT representation.
pub fn component_to_nbt(value: &ComponentValue) -> Tag {
    match value {
        ComponentValue::CustomName(tag)
        | ComponentValue::ItemName(tag)
        | ComponentValue::CustomData(tag)
        | ComponentValue::Opaque(tag) => tag.clone(),
        ComponentValue::Lore(lines) => component_list(lines),
        ComponentValue::Damage(value)
        | ComponentValue::MaxDamage(value)
        | ComponentValue::MaxStackSize(value)
        | ComponentValue::RepairCost(value) => Tag::Int(*value),
        ComponentValue::Enchantments(map) | ComponentValue::StoredEnchantments(map) => {
            Tag::Compound(
                map.iter()
                    .map(|(enchantment, level)| (enchantment.clone(), Tag::Int(*level)))
                    .collect(),
            )
        }
        ComponentValue::AttributeModifiers(entries) => {
            Tag::List(entries.iter().map(attribute_modifier_to_nbt).collect())
        }
        ComponentValue::Unbreakable => Tag::Compound(Vec::new()),
        ComponentValue::Container(container) => Tag::List(
            container
                .non_empty()
                .map(|(slot, stack)| {
                    Tag::Compound(vec![
                        ("slot".into(), Tag::Int(slot as i32)),
                        ("item".into(), item_stack_to_nbt(stack)),
                    ])
                })
                .collect(),
        ),
        ComponentValue::PotionContents(potion) => potion_contents_to_nbt(potion),
        ComponentValue::Food(food) => {
            let mut entries = vec![
                ("nutrition".into(), Tag::Int(food.nutrition)),
                ("saturation".into(), Tag::Float(food.saturation)),
            ];
            if food.can_always_eat {
                entries.push(("can_always_eat".into(), Tag::Byte(1)));
            }
            Tag::Compound(entries)
        }
        ComponentValue::Tool(tool) => tool_to_nbt(tool),
        ComponentValue::Rarity(rarity) => Tag::String(rarity.name().into()),
    }
}

fn attribute_modifier_to_nbt(entry: &AttributeModifierEntry) -> Tag {
    let mut fields = vec![
        ("type".into(), Tag::String(entry.attribute.clone())),
        ("id".into(), Tag::String(entry.id.clone())),
        ("amount".into(), Tag::Double(entry.amount)),
        (
            "operation".into(),
            Tag::String(entry.operation.name().into()),
        ),
    ];
    if entry.slot != EquipmentSlotGroup::Any {
        fields.push(("slot".into(), Tag::String(entry.slot.name().into())));
    }
    let display = match &entry.display {
        AttributeDisplay::Default => Some(Tag::Compound(vec![(
            "type".into(),
            Tag::String("default".into()),
        )])),
        AttributeDisplay::Hidden => Some(Tag::Compound(vec![(
            "type".into(),
            Tag::String("hidden".into()),
        )])),
        AttributeDisplay::Override(tag) => Some(Tag::Compound(vec![
            ("type".into(), Tag::String("override".into())),
            ("value".into(), tag.clone()),
        ])),
    };
    if let Some(display) = display
        && !matches!(&entry.display, AttributeDisplay::Default)
    {
        fields.push(("display".into(), display));
    }
    Tag::Compound(fields)
}

fn potion_contents_to_nbt(potion: &PotionContents) -> Tag {
    let mut fields = Vec::new();
    if let Some(potion) = &potion.potion {
        fields.push(("potion".into(), Tag::String(potion.clone())));
    }
    if let Some(color) = potion.custom_color {
        fields.push(("custom_color".into(), Tag::Int(color)));
    }
    if !potion.custom_effects.is_empty() {
        fields.push((
            "custom_effects".into(),
            Tag::List(potion.custom_effects.iter().map(effect_to_nbt).collect()),
        ));
    }
    if let Some(name) = &potion.custom_name {
        fields.push(("custom_name".into(), Tag::String(name.clone())));
    }
    Tag::Compound(fields)
}

fn effect_to_nbt(effect: &MobEffectInstance) -> Tag {
    let mut fields = vec![
        ("id".into(), Tag::String(effect.id.clone())),
        ("amplifier".into(), Tag::Int(effect.amplifier)),
        ("duration".into(), Tag::Int(effect.duration)),
    ];
    if effect.ambient {
        fields.push(("ambient".into(), Tag::Byte(1)));
    }
    if !effect.show_particles {
        fields.push(("show_particles".into(), Tag::Byte(0)));
    }
    if effect.show_icon != effect.show_particles {
        fields.push(("show_icon".into(), Tag::Byte(effect.show_icon as i8)));
    }
    if let Some(hidden) = &effect.hidden_effect {
        fields.push(("hidden_effect".into(), effect_to_nbt(hidden)));
    }
    Tag::Compound(fields)
}

fn tool_to_nbt(tool: &Tool) -> Tag {
    let mut fields = vec![(
        "rules".into(),
        Tag::List(tool.rules.iter().map(tool_rule_to_nbt).collect()),
    )];
    if (tool.default_mining_speed - 1.0).abs() > f32::EPSILON {
        fields.push((
            "default_mining_speed".into(),
            Tag::Float(tool.default_mining_speed),
        ));
    }
    if tool.damage_per_block != 1 {
        fields.push(("damage_per_block".into(), Tag::Int(tool.damage_per_block)));
    }
    if !tool.can_destroy_blocks_in_creative {
        fields.push(("can_destroy_blocks_in_creative".into(), Tag::Byte(0)));
    }
    Tag::Compound(fields)
}

fn tool_rule_to_nbt(rule: &ToolRule) -> Tag {
    let blocks = match &rule.blocks {
        BlockSet::Tag(tag) => Tag::String(format!("#{tag}")),
        BlockSet::Blocks(blocks) => Tag::List(blocks.iter().cloned().map(Tag::String).collect()),
    };
    let mut fields = vec![("blocks".into(), blocks)];
    if let Some(speed) = rule.speed {
        fields.push(("speed".into(), Tag::Float(speed)));
    }
    if let Some(correct) = rule.correct_for_drops {
        fields.push(("correct_for_drops".into(), Tag::Byte(correct as i8)));
    }
    Tag::Compound(fields)
}

/// Convert an [`ItemStack`] to the vanilla `ItemStack.MAP_CODEC` NBT shape:
/// `{id, count, components}`. An empty stack is an empty compound.
pub fn item_stack_to_nbt(stack: &ItemStack) -> Tag {
    if stack.is_empty() {
        return Tag::Compound(Vec::new());
    }
    let mut fields = vec![
        ("id".into(), Tag::String(stack.item.clone())),
        ("count".into(), Tag::Int(stack.count)),
    ];
    if !stack.components.is_empty() {
        let mut components = Vec::new();
        for (component, value) in stack.components.iter_entries() {
            match value {
                Some(value) => {
                    components.push((component.name().to_owned(), component_to_nbt(value)));
                }
                None => {
                    components.push((format!("!{}", component.name()), Tag::Compound(Vec::new())))
                }
            }
        }
        fields.push(("components".into(), Tag::Compound(components)));
    }
    Tag::Compound(fields)
}

/// Decode an [`ItemStack`] from `{id, count, components}` NBT. Unknown
/// components and malformed known-component payloads are preserved as
/// [`ComponentValue::Opaque`] rather than dropped.
pub fn item_stack_from_nbt(tag: &Tag) -> io::Result<ItemStack> {
    let entries = compound_tag(tag, "item stack")?;
    let Some(id_tag) = field(entries, "id") else {
        return Ok(ItemStack::empty());
    };
    let id = normalize_key(string_tag(id_tag, "item stack id")?);
    let count = match field(entries, "count").or_else(|| field(entries, "Count")) {
        Some(tag) => int_tag(tag, "item stack count")?,
        None => 1,
    };
    if count <= 0 || id == "minecraft:air" {
        return Ok(ItemStack::empty());
    }

    let mut stack = ItemStack {
        item: id,
        count,
        components: ComponentMap::new(),
    };
    if let Some(components) = field(entries, "components") {
        let components = compound_tag(components, "item stack components")?;
        for (name, value) in components {
            if let Some(removed) = name.strip_prefix('!') {
                stack.mark_component_removed(removed);
                continue;
            }
            let component = ComponentType::named(name);
            match component_from_nbt(&component, value) {
                Ok(value) => {
                    stack.components.insert_decoded(component, value);
                }
                Err(_) => stack.components.insert_opaque(name, value.clone()),
            }
        }
    }
    Ok(stack)
}

/// Decode one component payload from NBT.
pub fn component_from_nbt(component: &ComponentType, tag: &Tag) -> io::Result<ComponentValue> {
    let name = component.name();
    let value = match name {
        "minecraft:custom_name" => ComponentValue::CustomName(component_tag(tag, "custom_name")?),
        "minecraft:item_name" => ComponentValue::ItemName(component_tag(tag, "item_name")?),
        "minecraft:lore" => {
            let lines = list_tag(tag, "lore")?;
            ComponentValue::Lore(
                unwrap_component_list(lines)
                    .into_iter()
                    .map(|line| component_tag(&line, "lore line"))
                    .collect::<io::Result<Vec<_>>>()?,
            )
        }
        "minecraft:damage" => ComponentValue::Damage(int_tag(tag, "damage")?),
        "minecraft:max_damage" => ComponentValue::MaxDamage(int_tag(tag, "max_damage")?),
        "minecraft:max_stack_size" => ComponentValue::MaxStackSize(int_tag(tag, "max_stack_size")?),
        "minecraft:repair_cost" => ComponentValue::RepairCost(int_tag(tag, "repair_cost")?),
        "minecraft:rarity" => {
            let name = string_tag(tag, "rarity")?;
            ComponentValue::Rarity(
                Rarity::from_name(name).ok_or_else(|| invalid(format!("unknown rarity {name}")))?,
            )
        }
        "minecraft:unbreakable" => {
            compound_tag(tag, "unbreakable")?;
            ComponentValue::Unbreakable
        }
        "minecraft:custom_data" => {
            compound_tag(tag, "custom_data")?;
            ComponentValue::CustomData(tag.clone())
        }
        "minecraft:enchantments" | "minecraft:stored_enchantments" => {
            let entries = compound_tag(tag, name)?;
            let mut map = BTreeMap::new();
            for (enchantment, level) in entries {
                let level = int_tag(level, "enchantment level")?;
                if !(1..=255).contains(&level) {
                    return Err(invalid(format!("enchantment level {level} out of range")));
                }
                map.insert(normalize_key(enchantment), level);
            }
            if name == "minecraft:enchantments" {
                ComponentValue::Enchantments(map)
            } else {
                ComponentValue::StoredEnchantments(map)
            }
        }
        "minecraft:attribute_modifiers" => {
            let entries = list_tag(tag, "attribute_modifiers")?;
            ComponentValue::AttributeModifiers(
                entries
                    .iter()
                    .map(attribute_modifier_from_nbt)
                    .collect::<io::Result<Vec<_>>>()?,
            )
        }
        "minecraft:container" => {
            let slots = list_tag(tag, "container")?;
            let mut items: Vec<ItemStack> = Vec::new();
            for slot in slots {
                let fields = compound_tag(slot, "container slot")?;
                let index = int_tag(
                    field(fields, "slot")
                        .ok_or_else(|| invalid("container slot is missing slot"))?,
                    "container slot index",
                )?;
                if !(0..=255).contains(&index) {
                    return Err(invalid(format!("container slot {index} out of range")));
                }
                let item = field(fields, "item")
                    .ok_or_else(|| invalid("container slot is missing item"))?;
                let stack = item_stack_from_nbt(item)?;
                if stack.is_empty() {
                    continue;
                }
                let index = index as usize;
                if items.len() <= index {
                    items.resize(index + 1, ItemStack::empty());
                }
                items[index] = stack;
            }
            ComponentValue::Container(ItemContainer::new(items))
        }
        "minecraft:potion_contents" => {
            ComponentValue::PotionContents(potion_contents_from_nbt(tag)?)
        }
        "minecraft:food" => {
            let fields = compound_tag(tag, "food")?;
            let nutrition = int_tag(
                field(fields, "nutrition").ok_or_else(|| invalid("food is missing nutrition"))?,
                "food nutrition",
            )?;
            let saturation = float_tag(
                field(fields, "saturation").ok_or_else(|| invalid("food is missing saturation"))?,
                "food saturation",
            )?;
            ComponentValue::Food(FoodProperties {
                nutrition,
                saturation,
                can_always_eat: optional_bool(fields, "can_always_eat", false)?,
            })
        }
        "minecraft:tool" => ComponentValue::Tool(tool_from_nbt(tag)?),
        _ => return Ok(ComponentValue::Opaque(tag.clone())),
    };
    Ok(value)
}

impl ComponentMap {
    fn insert_decoded(&mut self, component: ComponentType, value: ComponentValue) {
        if let Some((_, current)) = self.entries.iter_mut().find(|(key, _)| key == &component) {
            *current = Some(value);
            return;
        }
        self.entries.push((component, Some(value)));
    }
}

fn attribute_modifier_from_nbt(tag: &Tag) -> io::Result<AttributeModifierEntry> {
    let fields = compound_tag(tag, "attribute modifier")?;
    let attribute = normalize_key(string_tag(
        field(fields, "type").ok_or_else(|| invalid("attribute modifier is missing type"))?,
        "attribute modifier type",
    )?);
    let id = string_tag(
        field(fields, "id").ok_or_else(|| invalid("attribute modifier is missing id"))?,
        "attribute modifier id",
    )?
    .to_owned();
    let amount = field(fields, "amount")
        .ok_or_else(|| invalid("attribute modifier is missing amount"))?
        .as_double()
        .ok_or_else(|| invalid("attribute modifier amount must be a double"))?;
    let operation = AttributeOperation::from_name(string_tag(
        field(fields, "operation")
            .ok_or_else(|| invalid("attribute modifier is missing operation"))?,
        "attribute modifier operation",
    )?)
    .ok_or_else(|| invalid("unknown attribute modifier operation"))?;
    let slot = match optional_string(fields, "slot")? {
        Some(slot) => EquipmentSlotGroup::from_name(&slot)
            .ok_or_else(|| invalid(format!("unknown equipment slot group {slot}")))?,
        None => EquipmentSlotGroup::Any,
    };
    let display = match field(fields, "display") {
        Some(display) => {
            let display_fields = compound_tag(display, "attribute modifier display")?;
            match string_tag(
                field(display_fields, "type")
                    .ok_or_else(|| invalid("attribute modifier display is missing type"))?,
                "attribute modifier display type",
            )? {
                "default" => AttributeDisplay::Default,
                "hidden" => AttributeDisplay::Hidden,
                "override" => AttributeDisplay::Override(component_tag(
                    field(display_fields, "value")
                        .ok_or_else(|| invalid("override display is missing value"))?,
                    "attribute modifier override",
                )?),
                other => return Err(invalid(format!("unknown attribute display {other}"))),
            }
        }
        None => AttributeDisplay::Default,
    };
    Ok(AttributeModifierEntry {
        attribute,
        id,
        amount,
        operation,
        slot,
        display,
    })
}

fn potion_contents_from_nbt(tag: &Tag) -> io::Result<PotionContents> {
    if let Tag::String(potion) = tag {
        return Ok(PotionContents::new(potion.clone()));
    }
    let fields = compound_tag(tag, "potion_contents")?;
    let effects = match field(fields, "custom_effects") {
        Some(tag) => list_tag(tag, "potion custom_effects")?
            .iter()
            .map(effect_from_nbt)
            .collect::<io::Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    Ok(PotionContents {
        potion: optional_string(fields, "potion")?.map(|potion| normalize_key(&potion)),
        custom_color: optional_int(fields, "custom_color")?,
        custom_effects: effects,
        custom_name: optional_string(fields, "custom_name")?,
    })
}

fn effect_from_nbt(tag: &Tag) -> io::Result<MobEffectInstance> {
    let fields = compound_tag(tag, "mob effect")?;
    let id = normalize_key(string_tag(
        field(fields, "id").ok_or_else(|| invalid("mob effect is missing id"))?,
        "mob effect id",
    )?);
    let amplifier = optional_int(fields, "amplifier")?.unwrap_or(0);
    let duration = optional_int(fields, "duration")?.unwrap_or(0);
    let ambient = optional_bool(fields, "ambient", false)?;
    let show_particles = optional_bool(fields, "show_particles", true)?;
    let show_icon = optional_bool(fields, "show_icon", show_particles)?;
    let hidden_effect = field(fields, "hidden_effect")
        .map(effect_from_nbt)
        .transpose()?
        .map(Box::new);
    Ok(MobEffectInstance {
        id,
        amplifier,
        duration,
        ambient,
        show_particles,
        show_icon,
        hidden_effect,
    })
}

fn tool_from_nbt(tag: &Tag) -> io::Result<Tool> {
    let fields = compound_tag(tag, "tool")?;
    let rules = list_tag(
        field(fields, "rules").ok_or_else(|| invalid("tool is missing rules"))?,
        "tool rules",
    )?
    .iter()
    .map(tool_rule_from_nbt)
    .collect::<io::Result<Vec<_>>>()?;
    Ok(Tool {
        rules,
        default_mining_speed: optional_float(fields, "default_mining_speed", 1.0)?,
        damage_per_block: optional_int(fields, "damage_per_block")?.unwrap_or(1),
        can_destroy_blocks_in_creative: optional_bool(
            fields,
            "can_destroy_blocks_in_creative",
            true,
        )?,
    })
}

fn tool_rule_from_nbt(tag: &Tag) -> io::Result<ToolRule> {
    let fields = compound_tag(tag, "tool rule")?;
    let blocks = field(fields, "blocks").ok_or_else(|| invalid("tool rule is missing blocks"))?;
    let blocks = match blocks {
        Tag::String(tag) => BlockSet::Tag(
            tag.strip_prefix('#')
                .map(normalize_key)
                .ok_or_else(|| invalid("tool rule tag must start with '#'"))?,
        ),
        Tag::List(blocks) => BlockSet::Blocks(
            blocks
                .iter()
                .map(|block| string_tag(block, "tool rule block").map(normalize_key))
                .collect::<io::Result<Vec<_>>>()?,
        ),
        _ => return Err(invalid("tool rule blocks must be a tag string or list")),
    };
    let speed = field(fields, "speed")
        .map(|tag| {
            tag.as_float()
                .ok_or_else(|| invalid("tool rule speed must be a float"))
        })
        .transpose()?;
    let correct_for_drops = field(fields, "correct_for_drops")
        .map(|tag| bool_tag(tag, "correct_for_drops"))
        .transpose()?;
    Ok(ToolRule {
        blocks,
        speed,
        correct_for_drops,
    })
}

fn enchantment_ids() -> &'static HashMap<&'static str, i32> {
    static IDS: OnceLock<HashMap<&'static str, i32>> = OnceLock::new();
    IDS.get_or_init(|| {
        let mut map = HashMap::new();
        for line in include_str!("../assets/registries/enchantment.tsv").lines() {
            let Some((id, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            map.insert(name, id);
        }
        map
    })
}

/// The protocol registry id for a bare or namespaced enchantment name.
pub fn enchantment_id(name: &str) -> Option<i32> {
    enchantment_ids().get(normalize_key(name).as_str()).copied()
}

/// The namespaced enchantment name for a protocol registry id.
pub fn enchantment_name(id: i32) -> Option<&'static str> {
    enchantment_ids()
        .iter()
        .find(|(_, known)| **known == id)
        .map(|(name, _)| *name)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NetworkEncodingReport {
    /// Added components omitted because their stream codec is not ported.
    pub omitted_added: Vec<String>,
    /// Removed components omitted because their registry id is unknown.
    pub omitted_removed: Vec<String>,
}

impl NetworkEncodingReport {
    pub fn is_complete(&self) -> bool {
        self.omitted_added.is_empty() && self.omitted_removed.is_empty()
    }

    fn merge(&mut self, other: NetworkEncodingReport) {
        self.omitted_added.extend(other.omitted_added);
        self.omitted_removed.extend(other.omitted_removed);
    }
}

fn write_nbt_tag(writer: &mut PacketWriter, tag: &Tag) -> io::Result<()> {
    let mut bytes = Vec::new();
    nbt::write_any_tag(&mut bytes, tag)?;
    writer.write_bytes(&bytes);
    Ok(())
}

/// Encode an item stack using the 1.21.11 play-protocol format: count, item
/// registry id, added component count, added payloads, removed component count.
///
/// Components whose stream codec is not ported are omitted and returned in the
/// report. Callers that require completeness must check
/// [`NetworkEncodingReport::is_complete`].
pub fn write_item_stack_report(
    writer: &mut PacketWriter,
    stack: &ItemStack,
) -> io::Result<NetworkEncodingReport> {
    let mut report = NetworkEncodingReport::default();
    if stack.is_empty() {
        writer.write_varint(0)?;
        return Ok(report);
    }
    let Some(item_id) = crate::registry::item_id(&stack.item) else {
        writer.write_varint(0)?;
        return Ok(report);
    };

    let mut payloads: Vec<(i32, Vec<u8>)> = Vec::new();
    for (component, value) in stack.components.iter_added() {
        let Some(component_id) = component.id() else {
            report.omitted_added.push(component.name().to_owned());
            continue;
        };
        match encode_component_payload(value, &mut report)? {
            Some(payload) => payloads.push((component_id, payload)),
            None => report.omitted_added.push(component.name().to_owned()),
        }
    }

    let mut removed = Vec::new();
    for component in stack.components.iter_removed() {
        match component.id() {
            Some(id) => removed.push(id),
            None => report.omitted_removed.push(component.name().to_owned()),
        }
    }

    writer.write_varint(stack.count)?;
    writer.write_varint(item_id)?;
    writer.write_varint(payloads.len() as i32)?;
    for (component_id, payload) in payloads {
        writer.write_varint(component_id)?;
        writer.write_bytes(&payload);
    }
    writer.write_varint(removed.len() as i32)?;
    for id in removed {
        writer.write_varint(id)?;
    }
    Ok(report)
}

pub fn write_item_stack(writer: &mut PacketWriter, stack: &ItemStack) {
    match write_item_stack_report(writer, stack) {
        Ok(report) if !report.is_complete() => {
            eprintln!(
                "[item] omitted components without a network codec: added={:?} removed={:?}",
                report.omitted_added, report.omitted_removed
            );
        }
        Ok(_) => {}
        Err(error) => {
            // The legacy writer has no error channel; an I/O error here means
            // the in-memory packet buffer could not be extended, which is fatal
            // for the connection. Callers that need the report use the
            // `_report` variant.
            eprintln!("[item] failed to encode item stack: {error}");
        }
    }
}

fn encode_component_payload(
    value: &ComponentValue,
    report: &mut NetworkEncodingReport,
) -> io::Result<Option<Vec<u8>>> {
    let mut writer = PacketWriter::new();
    match value {
        ComponentValue::CustomName(tag)
        | ComponentValue::ItemName(tag)
        | ComponentValue::CustomData(tag) => write_nbt_tag(&mut writer, tag)?,
        ComponentValue::Lore(lines) => {
            writer.write_varint(lines.len() as i32)?;
            for line in lines {
                write_nbt_tag(&mut writer, line)?;
            }
        }
        ComponentValue::Damage(value)
        | ComponentValue::MaxDamage(value)
        | ComponentValue::MaxStackSize(value)
        | ComponentValue::RepairCost(value) => writer.write_varint(*value)?,
        ComponentValue::Unbreakable => {}
        ComponentValue::Enchantments(map) | ComponentValue::StoredEnchantments(map) => {
            writer.write_varint(map.len() as i32)?;
            for (enchantment, level) in map {
                let Some(id) = enchantment_id(enchantment) else {
                    return Ok(None);
                };
                writer.write_varint(id)?;
                writer.write_varint(*level)?;
            }
        }
        ComponentValue::Container(container) => {
            writer.write_varint(container.items.len() as i32)?;
            for stack in &container.items {
                report.merge(write_item_stack_report(&mut writer, stack)?);
            }
        }
        ComponentValue::Food(food) => {
            writer.write_varint(food.nutrition)?;
            writer.write_f32(food.saturation)?;
            writer.write_u8(food.can_always_eat as u8)?;
        }
        ComponentValue::Rarity(rarity) => writer.write_varint(rarity.id())?,
        ComponentValue::AttributeModifiers(_)
        | ComponentValue::PotionContents(_)
        | ComponentValue::Tool(_)
        | ComponentValue::Opaque(_) => return Ok(None),
    }
    Ok(Some(writer.into_inner()))
}

fn read_nbt_tag(reader: &mut PacketReader<'_>) -> io::Result<Tag> {
    reader.read_nbt_any()
}

fn read_component_payload(
    component: &ComponentType,
    reader: &mut PacketReader<'_>,
) -> Option<ComponentValue> {
    let value = match component.name() {
        "minecraft:custom_name" => {
            let tag = read_nbt_tag(reader).ok()?;
            ComponentValue::CustomName(component_tag(&tag, "custom_name").ok()?)
        }
        "minecraft:item_name" => {
            let tag = read_nbt_tag(reader).ok()?;
            ComponentValue::ItemName(component_tag(&tag, "item_name").ok()?)
        }
        "minecraft:lore" => {
            let count = reader.read_varint().ok()?;
            if !(0..=256).contains(&count) {
                return None;
            }
            let mut lines = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let tag = read_nbt_tag(reader).ok()?;
                lines.push(component_tag(&tag, "lore line").ok()?);
            }
            ComponentValue::Lore(lines)
        }
        "minecraft:damage" => ComponentValue::Damage(reader.read_varint().ok()?),
        "minecraft:max_damage" => ComponentValue::MaxDamage(reader.read_varint().ok()?),
        "minecraft:max_stack_size" => ComponentValue::MaxStackSize(reader.read_varint().ok()?),
        "minecraft:repair_cost" => ComponentValue::RepairCost(reader.read_varint().ok()?),
        "minecraft:unbreakable" => ComponentValue::Unbreakable,
        "minecraft:custom_data" => {
            let tag = read_nbt_tag(reader).ok()?;
            compound_tag(&tag, "custom_data").ok()?;
            ComponentValue::CustomData(tag)
        }
        "minecraft:enchantments" | "minecraft:stored_enchantments" => {
            let count = reader.read_varint().ok()?;
            if !(0..=1024).contains(&count) {
                return None;
            }
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let id = reader.read_varint().ok()?;
                let level = reader.read_varint().ok()?;
                let name = enchantment_name(id)?.to_owned();
                map.insert(name, level);
            }
            if component.name() == "minecraft:enchantments" {
                ComponentValue::Enchantments(map)
            } else {
                ComponentValue::StoredEnchantments(map)
            }
        }
        "minecraft:container" => {
            let count = reader.read_varint().ok()?;
            if !(0..=256).contains(&count) {
                return None;
            }
            let mut items = Vec::with_capacity(count as usize);
            for _ in 0..count {
                items.push(read_item_stack(reader)?);
            }
            ComponentValue::Container(ItemContainer::new(items))
        }
        "minecraft:food" => ComponentValue::Food(FoodProperties {
            nutrition: reader.read_varint().ok()?,
            saturation: reader.read_f32().ok()?,
            can_always_eat: reader.read_u8().ok()? != 0,
        }),
        "minecraft:rarity" => ComponentValue::Rarity(Rarity::from_id(reader.read_varint().ok()?)?),
        _ => return None,
    };
    Some(value)
}

/// Decode the 1.21.11 play-protocol item-stack shape.
///
/// Returns `None` for an unknown component id or a component whose stream
/// codec is not ported: the payload length cannot be inferred without its
/// codec, so continuing would desynchronise the packet stream.
pub fn read_item_stack(reader: &mut PacketReader<'_>) -> Option<ItemStack> {
    let count = reader.read_varint().ok()?;
    if count <= 0 {
        return Some(ItemStack::empty());
    }
    let item_id = reader.read_varint().ok()?;
    let item = crate::registry::item_name(item_id)?;
    let mut stack = ItemStack::new(item, count);

    let added = reader.read_varint().ok()?;
    if !(0..=1024).contains(&added) {
        return None;
    }
    for _ in 0..added {
        let component_id = reader.read_varint().ok()?;
        let component = ComponentType::from_id(component_id)?;
        let value = read_component_payload(&component, reader)?;
        stack.components.insert_decoded(component, value);
    }

    let removed = reader.read_varint().ok()?;
    if !(0..=1024).contains(&removed) {
        return None;
    }
    for _ in 0..removed {
        let component_id = reader.read_varint().ok()?;
        let component = ComponentType::from_id(component_id)?;
        stack.components.mark_removed(component.name());
    }
    Some(stack)
}

// Enchanting lives in its own source file but is declared here so the slice
// does not need to touch `lib.rs` (which is shared with other concurrent
// workers). It can be promoted to `pub mod enchanting;` in `lib.rs` when the
// branches merge; the path attribute keeps the file at `src/enchanting.rs`.
#[path = "enchanting.rs"]
pub mod enchanting;

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(tag: &Tag) -> Vec<u8> {
        nbt::write_root_to_vec(tag).unwrap()
    }

    fn dec(bytes: &[u8]) -> Tag {
        let mut reader = bytes;
        nbt::read_root(&mut reader).unwrap()
    }

    fn sample_stack() -> ItemStack {
        let mut stack = ItemStack::new("minecraft:diamond_sword", 1);
        stack.set_component(ComponentValue::CustomName(Tag::String("Excalibur".into())));
        stack.set_component(ComponentValue::ItemName(Tag::String("Sword".into())));
        stack.set_component(ComponentValue::Lore(vec![
            Tag::String("A legendary blade".into()),
            Tag::Compound(vec![
                ("text".into(), Tag::String("Second line".into())),
                ("color".into(), Tag::String("gold".into())),
            ]),
        ]));
        stack.set_component(ComponentValue::Damage(12));
        stack.set_component(ComponentValue::MaxDamage(1561));
        stack.set_component(ComponentValue::RepairCost(3));
        stack.set_component(ComponentValue::Rarity(Rarity::Epic));
        stack.set_component(ComponentValue::Unbreakable);
        stack.set_component(ComponentValue::Enchantments(BTreeMap::from([
            ("minecraft:sharpness".to_owned(), 5),
            ("minecraft:unbreaking".to_owned(), 3),
        ])));
        stack.set_component(ComponentValue::StoredEnchantments(BTreeMap::from([(
            "minecraft:mending".to_owned(),
            1,
        )])));
        stack.set_component(ComponentValue::CustomData(Tag::Compound(vec![(
            "plugin".into(),
            Tag::String("test".into()),
        )])));
        stack
    }

    #[test]
    fn nbt_round_trips_known_components() {
        let stack = sample_stack();
        let tag = item_stack_to_nbt(&stack);
        let bytes = enc(&tag);
        let decoded = item_stack_from_nbt(&dec(&bytes)).unwrap();
        assert_eq!(decoded, stack);
        assert_eq!(enc(&item_stack_to_nbt(&decoded)), bytes);
    }

    #[test]
    fn unknown_components_survive_byte_for_byte() {
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:paper".into())),
            ("count".into(), Tag::Int(2)),
            (
                "components".into(),
                Tag::Compound(vec![(
                    "example:custom".into(),
                    Tag::Compound(vec![("value".into(), Tag::Long(9))]),
                )]),
            ),
        ]);
        let stack = item_stack_from_nbt(&tag).unwrap();
        assert!(matches!(
            stack.get_component("example:custom"),
            Some(ComponentValue::Opaque(_))
        ));
        assert_eq!(enc(&item_stack_to_nbt(&stack)), enc(&tag));
    }

    #[test]
    fn removed_components_survive() {
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:stone".into())),
            ("count".into(), Tag::Int(1)),
            (
                "components".into(),
                Tag::Compound(vec![(
                    "!minecraft:damage".into(),
                    Tag::Compound(Vec::new()),
                )]),
            ),
        ]);
        let stack = item_stack_from_nbt(&tag).unwrap();
        assert!(stack.components.is_removed("minecraft:damage"));
        assert!(stack.get_component("minecraft:damage").is_none());
        assert_eq!(enc(&item_stack_to_nbt(&stack)), enc(&tag));
    }

    #[test]
    fn item_stack_network_round_trip_subset() {
        let stack = sample_stack();
        let mut writer = PacketWriter::new();
        let report = write_item_stack_report(&mut writer, &stack).unwrap();
        // Sample contains only network-supported components.
        assert!(report.is_complete(), "{report:?}");
        let bytes = writer.into_inner();
        let mut reader = PacketReader::new(&bytes);
        let decoded = read_item_stack(&mut reader).unwrap();
        assert_eq!(decoded, stack);
    }

    #[test]
    fn unsupported_network_components_are_reported_not_malformed() {
        let mut stack = ItemStack::new("minecraft:stone", 1);
        stack.set_component(ComponentValue::Tool(Tool::new(vec![])));
        let mut writer = PacketWriter::new();
        let report = write_item_stack_report(&mut writer, &stack).unwrap();
        assert_eq!(report.omitted_added, vec!["minecraft:tool"]);
        let bytes = writer.into_inner();
        // count, item id, added=0, removed=0
        assert_eq!(bytes, vec![1, 1, 0, 0]);
        let mut reader = PacketReader::new(&bytes);
        assert_eq!(
            read_item_stack(&mut reader).unwrap(),
            ItemStack::new("minecraft:stone", 1)
        );
    }

    #[test]
    fn stone_network_body_is_unchanged() {
        let mut writer = PacketWriter::new();
        write_item_stack(&mut writer, &ItemStack::new("minecraft:stone", 1));
        assert_eq!(writer.into_inner(), vec![1, 1, 0, 0]);
    }

    #[test]
    fn container_component_round_trips() {
        let mut inner = ItemStack::new("minecraft:golden_apple", 3);
        inner.set_component(ComponentValue::CustomName(Tag::String("Snack".into())));
        let mut stack = ItemStack::new("minecraft:shulker_box", 1);
        stack.set_component(ComponentValue::Container(ItemContainer::new(vec![
            ItemStack::empty(),
            inner,
            ItemStack::new("minecraft:diamond", 2),
        ])));
        let tag = item_stack_to_nbt(&stack);
        let decoded = item_stack_from_nbt(&tag).unwrap();
        assert_eq!(decoded, stack);
        assert_eq!(enc(&item_stack_to_nbt(&decoded)), enc(&tag));

        let mut writer = PacketWriter::new();
        let report = write_item_stack_report(&mut writer, &stack).unwrap();
        assert!(report.is_complete());
        let bytes = writer.into_inner();
        let mut reader = PacketReader::new(&bytes);
        assert_eq!(read_item_stack(&mut reader).unwrap(), stack);
    }

    #[test]
    fn split_and_shrink_preserve_components() {
        let mut stack = ItemStack::new("minecraft:potion", 5);
        stack.set_component(ComponentValue::CustomName(Tag::String("Healing".into())));
        let split = stack.split(2);
        assert_eq!(split.count, 2);
        assert_eq!(
            split.get_component("minecraft:custom_name"),
            stack.get_component("minecraft:custom_name")
        );
        assert_eq!(stack.count, 3);
        stack.shrink(1);
        assert_eq!(stack.count, 2);
        assert!(stack.get_component("minecraft:custom_name").is_some());
        stack.shrink(10);
        assert!(stack.is_empty());
        assert!(stack.components.is_empty(), "empty stack drops its patch");
    }
}
