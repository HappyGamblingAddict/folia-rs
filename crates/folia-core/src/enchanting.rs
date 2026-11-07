//! Enchanting-table offers and application for 1.21.11.
//!
//! This module ports the behavior of `EnchantmentMenu`, `EnchantmentHelper`,
//! and `Enchantment` for the shipped vanilla enchantments:
//!
//! * a compact definition table (registry id, tags, slots, weight, cost curve,
//!   anvil cost, exclusive set, treasure flag) derived from the generated
//!   `data/minecraft/enchantment/*.json` documents;
//! * deterministic three-slot offer generation from an enchantment seed and
//!   the bookshelf power, including the vanilla cost formula and the
//!   `in_enchanting_table` pool;
//! * click validation (cost, experience levels, lapis lazuli), experience
//!   level debit, and lapis consumption;
//! * merging the rolled enchantments into `minecraft:enchantments` (or
//!   `minecraft:stored_enchantments` for books) while capping levels at the
//!   enchantment maximum and rejecting incompatible combinations.
//!
//! Anvil combine/repair is intentionally not ported yet; [`combine_in_anvil`]
//! returns an explicit [`AnvilError::Unsupported`] instead of silently doing
//! nothing. The same is true for rename handling, repair-cost accumulation,
//! and the Paper `PrepareItemEnchantEvent` hooks.
//!
//! # RNG order
//!
//! Offers follow the two-phase order of `EnchantmentMenu.slotsChanged`:
//!
//! 1. one `LegacyRandomSource` seeded with the enchantment seed draws all
//!    three costs in slot order;
//! 2. each slot that has a cost re-seeds to `seed + slot` to roll the
//!    enchantment list, and then draws the hinted enchantment from that same
//!    stream.
//!
//! `getEnchantmentList` re-rolls on click with the same seed, so a click applies
//! exactly the list the offer was generated from.

use crate::item_components::{ComponentValue, ItemStack, enchantment_id, normalize_key};
use crate::item_prototypes::prototype;
use crate::tags::{self, TagRegistryKind};
use folia_random::{LegacyRandomSource, RandomSource};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

/// Number of enchantment offers shown by the enchanting table.
pub const OFFER_COUNT: usize = 3;

/// Vanilla caps bookshelf power at 15 for the cost formula.
pub const MAX_BOOKSHELF_POWER: i32 = 15;

/// `Enchantment.MAX_LEVEL` and the `ItemEnchantments.Mutable` clamp.
pub const MAX_ENCHANTMENT_LEVEL: i32 = 255;

/// The lowest valid offer cost (`EnchantmentMenu`: `cost >= slot + 1`).
pub const MIN_OFFER_COST: i32 = 1;

/// The highest cost the vanilla formula can produce at power 15.
pub const MAX_OFFER_COST: i32 = 30;

/// A coarse classification for the shipped enchantments. 1.21.11 removed the
/// old `EnchantmentCategory` enum; this table keeps a comparable grouping for
/// callers that need to reason about families of offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnchantmentCategory {
    Armor,
    Weapon,
    Spear,
    Bow,
    Crossbow,
    Trident,
    Mace,
    Mining,
    Fishing,
    Universal,
    Curse,
}

impl EnchantmentCategory {
    pub fn name(self) -> &'static str {
        match self {
            Self::Armor => "armor",
            Self::Weapon => "weapon",
            Self::Spear => "spear",
            Self::Bow => "bow",
            Self::Crossbow => "crossbow",
            Self::Trident => "trident",
            Self::Mace => "mace",
            Self::Mining => "mining",
            Self::Fishing => "fishing",
            Self::Universal => "universal",
            Self::Curse => "curse",
        }
    }
}

/// One `Enchantment.Cost` curve: `base + per_level_above_first * (level - 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub base: i32,
    pub per_level_above_first: i32,
}

impl Cost {
    pub const fn new(base: i32, per_level_above_first: i32) -> Self {
        Self {
            base,
            per_level_above_first,
        }
    }

    /// `Enchantment.Cost.calculate(int level)`.
    pub const fn at(self, level: i32) -> i32 {
        self.base + self.per_level_above_first * (level - 1)
    }
}

/// The definition of one shipped enchantment, derived from the 1.21.11
/// enchantment data files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentDefinition {
    /// Registry id from `assets/registries/enchantment.tsv`.
    pub id: i32,
    /// Namespaced enchantment id.
    pub name: &'static str,
    pub max_level: i32,
    pub category: EnchantmentCategory,
    /// `supported_items` holder set (always a vanilla item tag here).
    pub supported_items: &'static str,
    /// Optional `primary_items` holder set. When absent, every supported item
    /// is a primary item.
    pub primary_items: Option<&'static str>,
    /// `weight` used by `WeightedRandom`.
    pub weight: i32,
    pub min_cost: Cost,
    pub max_cost: Cost,
    pub anvil_cost: i32,
    /// `EquipmentSlotGroup` names the enchantment applies to.
    pub slots: &'static [&'static str],
    /// Optional vanilla `exclusive_set` tag.
    pub exclusive_set: Option<&'static str>,
    /// Membership in the `minecraft:treasure` enchantment tag.
    pub treasure: bool,
}

impl EnchantmentDefinition {
    /// `Enchantment.getMinCost(int level)`.
    pub const fn min_cost_at(&self, level: i32) -> i32 {
        self.min_cost.at(level)
    }

    /// `Enchantment.getMaxCost(int level)`.
    pub const fn max_cost_at(&self, level: i32) -> i32 {
        self.max_cost.at(level)
    }

    /// `Enchantment.isSupportedItem(ItemStack)`.
    pub fn is_supported_item(&self, item: &str) -> bool {
        tags::item_has(self.supported_items, item)
    }

    /// `Enchantment.isPrimaryItem(ItemStack)`. Books bypass this check in
    /// [`available_enchantment_results`], matching `EnchantmentHelper`.
    pub fn is_primary_item(&self, item: &str) -> bool {
        self.is_supported_item(item)
            && self
                .primary_items
                .is_none_or(|tag| tags::item_has(tag, item))
    }

    /// The highest level in `1..=max_level` whose cost window contains
    /// `level`, matching `getAvailableEnchantmentResults`' descending scan.
    pub fn level_for_cost(&self, level: i32) -> Option<i32> {
        (1..=self.max_level).rev().find(|candidate| {
            level >= self.min_cost_at(*candidate) && level <= self.max_cost_at(*candidate)
        })
    }

    /// Whether this enchantment can be written to a stack. Enchanted books
    /// accept every enchantment in the table.
    pub fn can_apply_to(&self, stack: &ItemStack) -> bool {
        is_book_item(&stack.item) || self.is_supported_item(&stack.item)
    }
}

/// A rolled enchantment and its level (`EnchantmentInstance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentInstance {
    pub enchantment: &'static EnchantmentDefinition,
    pub level: i32,
}

impl EnchantmentInstance {
    pub const fn new(enchantment: &'static EnchantmentDefinition, level: i32) -> Self {
        Self { enchantment, level }
    }

    /// `EnchantmentInstance.weight()`.
    pub const fn weight(&self) -> i32 {
        self.enchantment.weight
    }
}

/// One of the three enchanting-table buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentOffer {
    /// Zero-based slot, matching the vanilla button id (`0..3`).
    pub slot: usize,
    /// Experience cost used for the click requirement and display.
    pub cost: i32,
    /// The enchantment shown as the clue. Vanilla can leave this empty when the
    /// rolled list is empty even though the cost is positive.
    pub hint: Option<EnchantmentInstance>,
}

/// The three offers produced by `EnchantmentMenu.slotsChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferSet {
    offers: [Option<EnchantmentOffer>; OFFER_COUNT],
}

impl OfferSet {
    pub const fn empty() -> Self {
        Self {
            offers: [None; OFFER_COUNT],
        }
    }

    pub fn slot(&self, slot: usize) -> Option<&EnchantmentOffer> {
        self.offers.get(slot).and_then(Option::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = &EnchantmentOffer> {
        self.offers.iter().flatten()
    }

    pub fn costs(&self) -> [i32; OFFER_COUNT] {
        std::array::from_fn(|slot| self.offers[slot].map_or(0, |offer| offer.cost))
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The shipped 1.21.11 enchantments, sorted by registry id. Values are derived
/// from `data/minecraft/enchantment/*.json` (DataVersion 4788).
pub static DEFINITIONS: &[EnchantmentDefinition] = &[
    EnchantmentDefinition {
        id: 0,
        name: "minecraft:aqua_affinity",
        max_level: 1,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/head_armor",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(1, 0),
        max_cost: Cost::new(41, 0),
        anvil_cost: 4,
        slots: &["head"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 1,
        name: "minecraft:bane_of_arthropods",
        max_level: 5,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/weapon",
        primary_items: Some("minecraft:enchantable/melee_weapon"),
        weight: 5,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(25, 8),
        anvil_cost: 2,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 2,
        name: "minecraft:binding_curse",
        max_level: 1,
        category: EnchantmentCategory::Curse,
        supported_items: "minecraft:enchantable/equippable",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(25, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 8,
        slots: &["armor"],
        exclusive_set: None,
        treasure: true,
    },
    EnchantmentDefinition {
        id: 3,
        name: "minecraft:blast_protection",
        max_level: 4,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/armor",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(13, 8),
        anvil_cost: 4,
        slots: &["armor"],
        exclusive_set: Some("minecraft:exclusive_set/armor"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 4,
        name: "minecraft:breach",
        max_level: 4,
        category: EnchantmentCategory::Mace,
        supported_items: "minecraft:enchantable/mace",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 5,
        name: "minecraft:channeling",
        max_level: 1,
        category: EnchantmentCategory::Trident,
        supported_items: "minecraft:enchantable/trident",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(25, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 8,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 6,
        name: "minecraft:density",
        max_level: 5,
        category: EnchantmentCategory::Mace,
        supported_items: "minecraft:enchantable/mace",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(25, 8),
        anvil_cost: 2,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 7,
        name: "minecraft:depth_strider",
        max_level: 3,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/foot_armor",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(10, 10),
        max_cost: Cost::new(25, 10),
        anvil_cost: 4,
        slots: &["feet"],
        exclusive_set: Some("minecraft:exclusive_set/boots"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 8,
        name: "minecraft:efficiency",
        max_level: 5,
        category: EnchantmentCategory::Mining,
        supported_items: "minecraft:enchantable/mining",
        primary_items: None,
        weight: 10,
        min_cost: Cost::new(1, 10),
        max_cost: Cost::new(51, 10),
        anvil_cost: 1,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 9,
        name: "minecraft:feather_falling",
        max_level: 4,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/foot_armor",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(5, 6),
        max_cost: Cost::new(11, 6),
        anvil_cost: 2,
        slots: &["armor"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 10,
        name: "minecraft:fire_aspect",
        max_level: 2,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/fire_aspect",
        primary_items: Some("minecraft:enchantable/melee_weapon"),
        weight: 2,
        min_cost: Cost::new(10, 20),
        max_cost: Cost::new(60, 20),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 11,
        name: "minecraft:fire_protection",
        max_level: 4,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/armor",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(10, 8),
        max_cost: Cost::new(18, 8),
        anvil_cost: 2,
        slots: &["armor"],
        exclusive_set: Some("minecraft:exclusive_set/armor"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 12,
        name: "minecraft:flame",
        max_level: 1,
        category: EnchantmentCategory::Bow,
        supported_items: "minecraft:enchantable/bow",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(20, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 13,
        name: "minecraft:fortune",
        max_level: 3,
        category: EnchantmentCategory::Mining,
        supported_items: "minecraft:enchantable/mining_loot",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/mining"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 14,
        name: "minecraft:frost_walker",
        max_level: 2,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/foot_armor",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(10, 10),
        max_cost: Cost::new(25, 10),
        anvil_cost: 4,
        slots: &["feet"],
        exclusive_set: Some("minecraft:exclusive_set/boots"),
        treasure: true,
    },
    EnchantmentDefinition {
        id: 15,
        name: "minecraft:impaling",
        max_level: 5,
        category: EnchantmentCategory::Trident,
        supported_items: "minecraft:enchantable/trident",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(1, 8),
        max_cost: Cost::new(21, 8),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 16,
        name: "minecraft:infinity",
        max_level: 1,
        category: EnchantmentCategory::Bow,
        supported_items: "minecraft:enchantable/bow",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(20, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 8,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/bow"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 17,
        name: "minecraft:knockback",
        max_level: 2,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/melee_weapon",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(5, 20),
        max_cost: Cost::new(55, 20),
        anvil_cost: 2,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 18,
        name: "minecraft:looting",
        max_level: 3,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/melee_weapon",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 19,
        name: "minecraft:loyalty",
        max_level: 3,
        category: EnchantmentCategory::Trident,
        supported_items: "minecraft:enchantable/trident",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(12, 7),
        max_cost: Cost::new(50, 0),
        anvil_cost: 2,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 20,
        name: "minecraft:luck_of_the_sea",
        max_level: 3,
        category: EnchantmentCategory::Fishing,
        supported_items: "minecraft:enchantable/fishing",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 21,
        name: "minecraft:lunge",
        max_level: 3,
        category: EnchantmentCategory::Spear,
        supported_items: "minecraft:enchantable/lunge",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(25, 8),
        anvil_cost: 2,
        slots: &["hand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 22,
        name: "minecraft:lure",
        max_level: 3,
        category: EnchantmentCategory::Fishing,
        supported_items: "minecraft:enchantable/fishing",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 23,
        name: "minecraft:mending",
        max_level: 1,
        category: EnchantmentCategory::Universal,
        supported_items: "minecraft:enchantable/durability",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(25, 25),
        max_cost: Cost::new(75, 25),
        anvil_cost: 4,
        slots: &["any"],
        exclusive_set: None,
        treasure: true,
    },
    EnchantmentDefinition {
        id: 24,
        name: "minecraft:multishot",
        max_level: 1,
        category: EnchantmentCategory::Crossbow,
        supported_items: "minecraft:enchantable/crossbow",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(20, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/crossbow"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 25,
        name: "minecraft:piercing",
        max_level: 4,
        category: EnchantmentCategory::Crossbow,
        supported_items: "minecraft:enchantable/crossbow",
        primary_items: None,
        weight: 10,
        min_cost: Cost::new(1, 10),
        max_cost: Cost::new(50, 0),
        anvil_cost: 1,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/crossbow"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 26,
        name: "minecraft:power",
        max_level: 5,
        category: EnchantmentCategory::Bow,
        supported_items: "minecraft:enchantable/bow",
        primary_items: None,
        weight: 10,
        min_cost: Cost::new(1, 10),
        max_cost: Cost::new(16, 10),
        anvil_cost: 1,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 27,
        name: "minecraft:projectile_protection",
        max_level: 4,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/armor",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(3, 6),
        max_cost: Cost::new(9, 6),
        anvil_cost: 2,
        slots: &["armor"],
        exclusive_set: Some("minecraft:exclusive_set/armor"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 28,
        name: "minecraft:protection",
        max_level: 4,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/armor",
        primary_items: None,
        weight: 10,
        min_cost: Cost::new(1, 11),
        max_cost: Cost::new(12, 11),
        anvil_cost: 1,
        slots: &["armor"],
        exclusive_set: Some("minecraft:exclusive_set/armor"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 29,
        name: "minecraft:punch",
        max_level: 2,
        category: EnchantmentCategory::Bow,
        supported_items: "minecraft:enchantable/bow",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(12, 20),
        max_cost: Cost::new(37, 20),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 30,
        name: "minecraft:quick_charge",
        max_level: 3,
        category: EnchantmentCategory::Crossbow,
        supported_items: "minecraft:enchantable/crossbow",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(12, 20),
        max_cost: Cost::new(50, 0),
        anvil_cost: 2,
        slots: &["mainhand", "offhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 31,
        name: "minecraft:respiration",
        max_level: 3,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/head_armor",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(10, 10),
        max_cost: Cost::new(40, 10),
        anvil_cost: 4,
        slots: &["head"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 32,
        name: "minecraft:riptide",
        max_level: 3,
        category: EnchantmentCategory::Trident,
        supported_items: "minecraft:enchantable/trident",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(17, 7),
        max_cost: Cost::new(50, 0),
        anvil_cost: 4,
        slots: &["hand"],
        exclusive_set: Some("minecraft:exclusive_set/riptide"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 33,
        name: "minecraft:sharpness",
        max_level: 5,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/sharp_weapon",
        primary_items: Some("minecraft:enchantable/melee_weapon"),
        weight: 10,
        min_cost: Cost::new(1, 11),
        max_cost: Cost::new(21, 11),
        anvil_cost: 1,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 34,
        name: "minecraft:silk_touch",
        max_level: 1,
        category: EnchantmentCategory::Mining,
        supported_items: "minecraft:enchantable/mining_loot",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(15, 0),
        max_cost: Cost::new(65, 0),
        anvil_cost: 8,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/mining"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 35,
        name: "minecraft:smite",
        max_level: 5,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/weapon",
        primary_items: Some("minecraft:enchantable/melee_weapon"),
        weight: 5,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(25, 8),
        anvil_cost: 2,
        slots: &["mainhand"],
        exclusive_set: Some("minecraft:exclusive_set/damage"),
        treasure: false,
    },
    EnchantmentDefinition {
        id: 36,
        name: "minecraft:soul_speed",
        max_level: 3,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/foot_armor",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(10, 10),
        max_cost: Cost::new(25, 10),
        anvil_cost: 8,
        slots: &["feet"],
        exclusive_set: None,
        treasure: true,
    },
    EnchantmentDefinition {
        id: 37,
        name: "minecraft:sweeping_edge",
        max_level: 3,
        category: EnchantmentCategory::Weapon,
        supported_items: "minecraft:enchantable/sweeping",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(5, 9),
        max_cost: Cost::new(20, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 38,
        name: "minecraft:swift_sneak",
        max_level: 3,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/leg_armor",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(25, 25),
        max_cost: Cost::new(75, 25),
        anvil_cost: 8,
        slots: &["legs"],
        exclusive_set: None,
        treasure: true,
    },
    EnchantmentDefinition {
        id: 39,
        name: "minecraft:thorns",
        max_level: 3,
        category: EnchantmentCategory::Armor,
        supported_items: "minecraft:enchantable/armor",
        primary_items: Some("minecraft:enchantable/chest_armor"),
        weight: 1,
        min_cost: Cost::new(10, 20),
        max_cost: Cost::new(60, 20),
        anvil_cost: 8,
        slots: &["any"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 40,
        name: "minecraft:unbreaking",
        max_level: 3,
        category: EnchantmentCategory::Universal,
        supported_items: "minecraft:enchantable/durability",
        primary_items: None,
        weight: 5,
        min_cost: Cost::new(5, 8),
        max_cost: Cost::new(55, 8),
        anvil_cost: 2,
        slots: &["any"],
        exclusive_set: None,
        treasure: false,
    },
    EnchantmentDefinition {
        id: 41,
        name: "minecraft:vanishing_curse",
        max_level: 1,
        category: EnchantmentCategory::Curse,
        supported_items: "minecraft:enchantable/vanishing",
        primary_items: None,
        weight: 1,
        min_cost: Cost::new(25, 0),
        max_cost: Cost::new(50, 0),
        anvil_cost: 8,
        slots: &["any"],
        exclusive_set: None,
        treasure: true,
    },
    EnchantmentDefinition {
        id: 42,
        name: "minecraft:wind_burst",
        max_level: 3,
        category: EnchantmentCategory::Mace,
        supported_items: "minecraft:enchantable/mace",
        primary_items: None,
        weight: 2,
        min_cost: Cost::new(15, 9),
        max_cost: Cost::new(65, 9),
        anvil_cost: 4,
        slots: &["mainhand"],
        exclusive_set: None,
        treasure: true,
    },
];

fn definition_index() -> &'static HashMap<&'static str, usize> {
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    INDEX.get_or_init(|| {
        DEFINITIONS
            .iter()
            .enumerate()
            .map(|(index, definition)| (definition.name, index))
            .collect()
    })
}

/// Look up a definition by bare or namespaced id.
pub fn definition(name: &str) -> Option<&'static EnchantmentDefinition> {
    let name = normalize_key(name);
    definition_index()
        .get(name.as_str())
        .map(|index| &DEFINITIONS[*index])
}

/// Look up a definition by enchantment registry id.
pub fn definition_by_id(id: i32) -> Option<&'static EnchantmentDefinition> {
    DEFINITIONS.iter().find(|definition| definition.id == id)
}

fn enchanting_table_pool() -> &'static [&'static EnchantmentDefinition] {
    static POOL: OnceLock<Vec<&'static EnchantmentDefinition>> = OnceLock::new();
    POOL.get_or_init(|| {
        DEFINITIONS
            .iter()
            .filter(|definition| {
                tags::has(
                    TagRegistryKind::Enchantment,
                    "minecraft:in_enchanting_table",
                    definition.name,
                )
            })
            .collect()
    })
}

/// Whether an enchantment is in `minecraft:in_enchanting_table`.
pub fn is_in_enchanting_table(name: &str) -> bool {
    tags::has(
        TagRegistryKind::Enchantment,
        "minecraft:in_enchanting_table",
        name,
    )
}

/// `Enchantment.areCompatible`: different enchantments that are not in each
/// other's exclusive set.
pub fn are_compatible(first: &EnchantmentDefinition, second: &EnchantmentDefinition) -> bool {
    if first.name == second.name {
        return false;
    }
    let first_excludes_second = first
        .exclusive_set
        .is_some_and(|tag| tags::has(TagRegistryKind::Enchantment, tag, second.name));
    let second_excludes_first = second
        .exclusive_set
        .is_some_and(|tag| tags::has(TagRegistryKind::Enchantment, tag, first.name));
    !first_excludes_second && !second_excludes_first
}

/// `EnchantmentHelper.getEnchantmentCost`. Returns `0` for a stack without the
/// `minecraft:enchantable` prototype value.
pub fn enchantment_cost<R: RandomSource + ?Sized>(
    random: &mut R,
    slot: usize,
    power: i32,
    stack: &ItemStack,
) -> i32 {
    if stack.is_empty() {
        return 0;
    }
    if prototype(&stack.item)
        .and_then(|prototype| prototype.enchantability)
        .is_none()
    {
        return 0;
    }
    let power = power.clamp(0, MAX_BOOKSHELF_POWER);
    let base = random.next_int_bound(8) + 1 + (power >> 1) + random.next_int_bound(power + 1);
    match slot {
        0 => (base / 3).max(1),
        1 => base * 2 / 3 + 1,
        _ => base.max(power * 2),
    }
}

/// `EnchantmentHelper.getAvailableEnchantmentResults`: every level in the
/// requested cost window, scanning each enchantment from max level down.
///
/// Books accept every enchantment in `minecraft:in_enchanting_table`; other
/// stacks must match `isPrimaryItem`.
pub fn available_enchantment_results(level: i32, stack: &ItemStack) -> Vec<EnchantmentInstance> {
    let is_book = stack.item == "minecraft:book";
    let mut results = Vec::new();
    for definition in enchanting_table_pool() {
        if !is_book && !definition.is_primary_item(&stack.item) {
            continue;
        }
        if let Some(level) = definition.level_for_cost(level) {
            results.push(EnchantmentInstance::new(definition, level));
        }
    }
    results
}

/// `WeightedRandom.getRandomItem` over enchantment weights.
fn weighted_pick<R: RandomSource + ?Sized>(
    random: &mut R,
    elements: &[EnchantmentInstance],
) -> Option<EnchantmentInstance> {
    let total: i32 = elements.iter().map(EnchantmentInstance::weight).sum();
    if total <= 0 {
        return None;
    }
    let mut index = random.next_int_bound(total);
    for element in elements {
        index -= element.weight();
        if index < 0 {
            return Some(*element);
        }
    }
    None
}

/// `EnchantmentHelper.selectEnchantment`.
pub fn select_enchantment<R: RandomSource + ?Sized>(
    random: &mut R,
    stack: &ItemStack,
    level: i32,
) -> Vec<EnchantmentInstance> {
    let Some(enchantability) =
        prototype(&stack.item).and_then(|prototype| prototype.enchantability)
    else {
        return Vec::new();
    };

    let mut level = level;
    level += 1
        + random.next_int_bound(enchantability / 4 + 1)
        + random.next_int_bound(enchantability / 4 + 1);
    let spread = (random.next_float() + random.next_float() - 1.0) * 0.15;
    level = (level as f32 + level as f32 * spread)
        .round()
        .clamp(1.0, i32::MAX as f32) as i32;

    let mut available = available_enchantment_results(level, stack);
    let mut selected = Vec::new();
    if available.is_empty() {
        return selected;
    }

    if let Some(instance) = weighted_pick(random, &available) {
        selected.push(instance);
    }
    while random.next_int_bound(50) <= level {
        if let Some(last) = selected.last() {
            available.retain(|candidate| are_compatible(last.enchantment, candidate.enchantment));
        }
        if available.is_empty() {
            break;
        }
        if let Some(instance) = weighted_pick(random, &available) {
            selected.push(instance);
        }
        level /= 2;
    }
    selected
}

fn roll_list<R: RandomSource + ?Sized>(
    random: &mut R,
    stack: &ItemStack,
    cost: i32,
) -> Vec<EnchantmentInstance> {
    let mut list = select_enchantment(random, stack, cost);
    if stack.item == "minecraft:book" && list.len() > 1 {
        let index = random.next_int_bound(list.len() as i32) as usize;
        list.remove(index);
    }
    list
}

/// `EnchantmentMenu.getEnchantmentList`: roll the list for one slot and remove
/// one random entry for a book. The seed is the table's enchantment seed.
pub fn enchantment_list(
    stack: &ItemStack,
    slot: usize,
    cost: i32,
    seed: i32,
) -> Vec<EnchantmentInstance> {
    let mut random = LegacyRandomSource::new(i64::from(seed.wrapping_add(slot as i32)));
    roll_list(&mut random, stack, cost)
}

/// `EnchantmentMenu.slotsChanged`: three offers for a stack at a bookshelf
/// power and enchantment seed.
pub fn generate_offers(stack: &ItemStack, power: i32, seed: i32) -> OfferSet {
    let mut costs = [0i32; OFFER_COUNT];
    if !stack.is_empty() {
        let mut random = LegacyRandomSource::new(i64::from(seed));
        for (slot, cost) in costs.iter_mut().enumerate() {
            *cost = enchantment_cost(&mut random, slot, power, stack);
            if *cost < slot as i32 + 1 {
                *cost = 0;
            }
        }
    }

    let mut offers = [None; OFFER_COUNT];
    for (slot, cost) in costs.iter().enumerate() {
        if *cost <= 0 {
            continue;
        }
        let mut random = LegacyRandomSource::new(i64::from(seed.wrapping_add(slot as i32)));
        let list = roll_list(&mut random, stack, *cost);
        let hint = if list.is_empty() {
            None
        } else {
            let index = random.next_int_bound(list.len() as i32) as usize;
            Some(list[index])
        };
        offers[slot] = Some(EnchantmentOffer {
            slot,
            cost: *cost,
            hint,
        });
    }
    OfferSet { offers }
}

/// A live enchanting-table interaction: the seed, bookshelf power, and the
/// last generated offers.
#[derive(Debug, Clone)]
pub struct EnchantingTable {
    seed: i32,
    power: i32,
    offers: OfferSet,
}

impl EnchantingTable {
    /// Create a table state without offers. Call [`Self::refresh`] with the
    /// stack in the input slot.
    pub fn new(seed: i32, power: i32) -> Self {
        Self {
            seed,
            power,
            offers: OfferSet::empty(),
        }
    }

    /// Open a table state and generate the offers for `stack`.
    pub fn open(stack: &ItemStack, power: i32, seed: i32) -> Self {
        Self {
            seed,
            power,
            offers: generate_offers(stack, power, seed),
        }
    }

    pub fn seed(&self) -> i32 {
        self.seed
    }

    pub fn power(&self) -> i32 {
        self.power
    }

    pub fn offers(&self) -> &OfferSet {
        &self.offers
    }

    /// Regenerate the offers after the input stack or bookshelf power changed.
    pub fn refresh(&mut self, stack: &ItemStack) {
        self.offers = generate_offers(stack, self.power, self.seed);
    }

    pub fn set_power(&mut self, power: i32, stack: &ItemStack) {
        self.power = power;
        self.refresh(stack);
    }

    /// Apply one clicked offer.
    ///
    /// Follows `EnchantmentMenu.clickMenuButton`:
    /// * the lapis slot must hold at least `slot + 1` lapis lazuli unless the
    ///   player has infinite materials;
    /// * the offer cost must be positive and the player must have at least
    ///   both `slot + 1` levels and the offer cost (again unless creative);
    /// * the applied list is re-rolled from `seed + slot` and `cost`, so it is
    ///   identical to the offer's list;
    /// * `slot + 1` experience levels are debited (clamped at zero, even in
    ///   creative, matching `Player.onEnchantmentPerformed`) and `slot + 1`
    ///   lapis lazuli are consumed unless creative;
    /// * books become enchanted books and receive `stored_enchantments`, other
    ///   items receive `enchantments`.
    ///
    /// `next_seed` becomes the table seed after a successful click, mirroring
    /// `Player.onEnchantmentPerformed` updating `enchantmentSeed`; the new
    /// offers are generated immediately from the mutated stack.
    pub fn enchant(
        &mut self,
        slot: usize,
        stack: &mut ItemStack,
        lapis: &mut ItemStack,
        xp_level: &mut i32,
        creative: bool,
        next_seed: i32,
    ) -> Result<EnchantOutcome, EnchantError> {
        if slot >= OFFER_COUNT {
            return Err(EnchantError::InvalidSlot { slot });
        }
        let offer = *self
            .offers
            .slot(slot)
            .ok_or(EnchantError::NoOffer { slot })?;
        if offer.cost <= 0 {
            return Err(EnchantError::NoOffer { slot });
        }
        if stack.is_empty() {
            return Err(EnchantError::EmptyStack);
        }
        let required_levels = slot as i32 + 1;
        if (lapis.item != "minecraft:lapis_lazuli" || lapis.count < required_levels) && !creative {
            return Err(EnchantError::NotEnoughLapis {
                required: required_levels,
                available: if lapis.item == "minecraft:lapis_lazuli" {
                    lapis.count
                } else {
                    0
                },
            });
        }
        if !creative && (*xp_level < required_levels || *xp_level < offer.cost) {
            return Err(EnchantError::NotEnoughExperience {
                required_levels,
                required_cost: offer.cost,
                available: *xp_level,
            });
        }

        let applied = enchantment_list(stack, slot, offer.cost, self.seed);
        if applied.is_empty() {
            return Err(EnchantError::NoEnchantments);
        }
        let application = apply_enchantments(stack, &applied)?;

        let xp_before = *xp_level;
        if !creative {
            lapis.shrink(required_levels);
        }
        *xp_level = (*xp_level - required_levels).max(0);
        let levels_spent = xp_before - *xp_level;

        self.seed = next_seed;
        self.offers = generate_offers(stack, self.power, next_seed);

        Ok(EnchantOutcome {
            application,
            levels_spent,
            lapis_spent: if creative { 0 } else { required_levels },
            new_seed: next_seed,
        })
    }
}

/// The result of merging rolled enchantments into an item stack.
#[derive(Debug, Clone, PartialEq)]
pub struct EnchantmentApplication {
    /// The component that was written (`minecraft:enchantments` or
    /// `minecraft:stored_enchantments`).
    pub component: &'static str,
    /// The final enchantment map.
    pub enchantments: BTreeMap<String, i32>,
    /// The rolled instances that were applied.
    pub applied: Vec<EnchantmentInstance>,
    /// Whether a plain book was transmuted into an enchanted book.
    pub became_enchanted_book: bool,
}

/// A successful table click.
#[derive(Debug, Clone, PartialEq)]
pub struct EnchantOutcome {
    pub application: EnchantmentApplication,
    /// Experience levels actually removed from the counter (vanilla also
    /// debits them in creative, clamped at zero).
    pub levels_spent: i32,
    /// Lapis lazuli consumed (creative clicks consume none).
    pub lapis_spent: i32,
    /// The seed installed for the next offer set.
    pub new_seed: i32,
}

/// Errors from offer application and enchantment merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnchantError {
    InvalidSlot {
        slot: usize,
    },
    NoOffer {
        slot: usize,
    },
    EmptyStack,
    NotEnoughLapis {
        required: i32,
        available: i32,
    },
    NotEnoughExperience {
        required_levels: i32,
        required_cost: i32,
        available: i32,
    },
    NoEnchantments,
    NotApplicable {
        enchantment: &'static str,
        item: String,
    },
    InvalidLevel {
        enchantment: &'static str,
        level: i32,
    },
    Incompatible {
        first: &'static str,
        second: String,
    },
    MalformedComponent {
        component: &'static str,
    },
}

impl fmt::Display for EnchantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot { slot } => write!(formatter, "invalid enchanting slot {slot}"),
            Self::NoOffer { slot } => write!(formatter, "enchanting slot {slot} has no offer"),
            Self::EmptyStack => write!(formatter, "the input stack is empty"),
            Self::NotEnoughLapis {
                required,
                available,
            } => write!(
                formatter,
                "not enough lapis lazuli: need {required}, have {available}"
            ),
            Self::NotEnoughExperience {
                required_levels,
                required_cost,
                available,
            } => write!(
                formatter,
                "not enough experience: need {required_levels} levels and cost {required_cost}, have {available}"
            ),
            Self::NoEnchantments => write!(formatter, "the offer rolled no enchantments"),
            Self::NotApplicable { enchantment, item } => {
                write!(formatter, "{enchantment} cannot be applied to {item}")
            }
            Self::InvalidLevel { enchantment, level } => {
                write!(formatter, "{enchantment} has invalid level {level}")
            }
            Self::Incompatible { first, second } => {
                write!(formatter, "{first} is incompatible with {second}")
            }
            Self::MalformedComponent { component } => {
                write!(formatter, "{component} is not an enchantment component")
            }
        }
    }
}

impl std::error::Error for EnchantError {}

fn is_book_item(item: &str) -> bool {
    item == "minecraft:book" || item == "minecraft:enchanted_book"
}

fn existing_level(map: &BTreeMap<String, i32>, name: &str) -> Option<(String, i32)> {
    map.iter()
        .find(|(key, _)| normalize_key(key) == name)
        .map(|(key, level)| (key.clone(), *level))
}

/// Merge rolled enchantments into a stack's enchantment component.
///
/// The merge keeps every other component untouched, upgrades an existing
/// enchantment to the higher level, caps the result at the enchantment's
/// `max_level`, and rejects a rolled enchantment that conflicts with an
/// existing or another rolled enchantment's exclusive set.
///
/// A plain `minecraft:book` is transmuted to `minecraft:enchanted_book` and
/// receives `minecraft:stored_enchantments`; an existing enchanted book keeps
/// that component; every other item receives `minecraft:enchantments`.
pub fn apply_enchantments(
    stack: &mut ItemStack,
    instances: &[EnchantmentInstance],
) -> Result<EnchantmentApplication, EnchantError> {
    if stack.is_empty() {
        return Err(EnchantError::EmptyStack);
    }
    if instances.is_empty() {
        return Err(EnchantError::NoEnchantments);
    }

    let book = is_book_item(&stack.item);
    let component = if book {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    };

    for instance in instances {
        if instance.level <= 0 {
            return Err(EnchantError::InvalidLevel {
                enchantment: instance.enchantment.name,
                level: instance.level,
            });
        }
        if !instance.enchantment.can_apply_to(stack) {
            return Err(EnchantError::NotApplicable {
                enchantment: instance.enchantment.name,
                item: stack.item.clone(),
            });
        }
    }

    let mut map = match stack.get_component(component) {
        Some(ComponentValue::Enchantments(map)) | Some(ComponentValue::StoredEnchantments(map)) => {
            map.clone()
        }
        Some(_) => return Err(EnchantError::MalformedComponent { component }),
        None => BTreeMap::new(),
    };

    for (index, instance) in instances.iter().enumerate() {
        for other in &instances[index + 1..] {
            if instance.enchantment.name != other.enchantment.name
                && !are_compatible(instance.enchantment, other.enchantment)
            {
                return Err(EnchantError::Incompatible {
                    first: instance.enchantment.name,
                    second: other.enchantment.name.to_owned(),
                });
            }
        }
        for existing_name in map.keys() {
            if normalize_key(existing_name) == instance.enchantment.name {
                continue;
            }
            let Some(existing) = definition(existing_name) else {
                // Unknown (modded) enchantments are preserved but cannot be
                // checked against the vanilla exclusive sets.
                continue;
            };
            if !are_compatible(instance.enchantment, existing) {
                return Err(EnchantError::Incompatible {
                    first: instance.enchantment.name,
                    second: existing.name.to_owned(),
                });
            }
        }
    }

    for instance in instances {
        let name = instance.enchantment.name;
        let current = existing_level(&map, name).map_or(0, |(_, level)| level);
        let capped = current.max(instance.level.min(instance.enchantment.max_level));
        if let Some((key, _)) = existing_level(&map, name)
            && key != name
        {
            map.remove(&key);
        }
        map.insert(name.to_owned(), capped.min(MAX_ENCHANTMENT_LEVEL));
    }

    let became_enchanted_book = stack.item == "minecraft:book";
    if became_enchanted_book {
        stack.item = "minecraft:enchanted_book".to_owned();
    }
    let value = if book {
        ComponentValue::StoredEnchantments(map.clone())
    } else {
        ComponentValue::Enchantments(map.clone())
    };
    stack
        .set_component_named(component, value)
        .map_err(|_| EnchantError::MalformedComponent { component })?;

    Ok(EnchantmentApplication {
        component,
        enchantments: map,
        applied: instances.to_vec(),
        became_enchanted_book,
    })
}

/// Anvil combine/repair outcome (not produced yet).
#[derive(Debug, Clone, PartialEq)]
pub struct AnvilResult {
    pub result: ItemStack,
    pub cost: i32,
}

/// Anvil errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnvilError {
    /// The subset of `AnvilMenu.createResult` needed for combine/repair has not
    /// been ported. Renaming, repair-cost accumulation, and item durability
    /// merging are all intentionally included in this unsupported path.
    Unsupported(&'static str),
}

impl fmt::Display for AnvilError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(reason) => {
                write!(formatter, "anvil combine/repair is unsupported: {reason}")
            }
        }
    }
}

impl std::error::Error for AnvilError {}

/// Explicitly unsupported anvil combine/repair entry point.
///
/// Returning an error here keeps the slice honest: callers cannot mistake an
/// empty stack or an unmodified input for a successful combine.
pub fn combine_in_anvil(_left: &ItemStack, _right: &ItemStack) -> Result<AnvilResult, AnvilError> {
    Err(AnvilError::Unsupported(
        "anvil combine/repair, renaming, and repair-cost accumulation are not ported",
    ))
}

/// Verify that the embedded definition table matches the shipped enchantment
/// registry. Used by tests and by future asset validation.
pub fn validate_registry_ids() -> Result<(), String> {
    for definition in DEFINITIONS {
        match enchantment_id(definition.name) {
            Some(id) if id == definition.id => {}
            Some(id) => {
                return Err(format!(
                    "{} has registry id {id}, table says {}",
                    definition.name, definition.id
                ));
            }
            None => {
                return Err(format!(
                    "{} is missing from assets/registries/enchantment.tsv",
                    definition.name
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_table_matches_the_registry_dump() {
        validate_registry_ids().unwrap();
        assert_eq!(DEFINITIONS.len(), 43);
        for (index, definition) in DEFINITIONS.iter().enumerate() {
            assert_eq!(definition.id, index as i32);
        }
        assert_eq!(definition("sharpness").unwrap().name, "minecraft:sharpness");
        assert_eq!(definition_by_id(33).unwrap().name, "minecraft:sharpness");
        assert!(definition("minecraft:not_real").is_none());
    }

    #[test]
    fn enchanting_table_pool_excludes_treasure_and_curses() {
        let pool = enchanting_table_pool();
        assert_eq!(pool.len(), 36);
        assert!(!pool.iter().any(|definition| definition.treasure));
        assert!(
            !pool
                .iter()
                .any(|definition| definition.name == "minecraft:mending")
        );
        assert!(
            !pool
                .iter()
                .any(|definition| definition.name == "minecraft:vanishing_curse")
        );
    }

    #[test]
    fn cost_curves_and_level_windows_match_the_data() {
        let sharpness = definition("minecraft:sharpness").unwrap();
        assert_eq!(sharpness.min_cost_at(1), 1);
        assert_eq!(sharpness.min_cost_at(5), 45);
        assert_eq!(sharpness.max_cost_at(5), 65);
        assert_eq!(sharpness.level_for_cost(1), Some(1));
        assert_eq!(sharpness.level_for_cost(65), Some(5));
        assert_eq!(sharpness.level_for_cost(66), None);
    }
}
