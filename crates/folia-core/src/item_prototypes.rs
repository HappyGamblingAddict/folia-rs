//! Per-item prototype components for Minecraft/Folia 1.21.11.
//!
//! Vanilla items are created with a `DataComponentMap` of prototype defaults:
//! stack size, durability, tool rules, food, equippable slot, enchantability,
//! repair ingredient, fire resistance, and rarity. A stack is then a patch over
//! those defaults. [`crate::item_components`] models the patch, but until this
//! module existed it had no way to know the defaults, so it guessed stack size
//! from the presence of a damage component.
//!
//! `scripts/extract_item_prototypes.ps1` boots the local 1.21.11 server jar,
//! reads each registry item's `components()` map, and writes
//! `assets/item_prototypes.json`. The catalog stores only fields that differ
//! from the defaults, so a plain block item is `{}`:
//!
//! * stack size 64 (vanilla `DataComponents.COMMON_ITEM_COMPONENTS`)
//! * no durability, no tool, no food, no equippable slot
//! * no enchantability, no repair ingredient, not fire resistant
//! * rarity `common`
//!
//! Malformed entries are rejected with [`std::io::ErrorKind::InvalidData`]
//! rather than silently falling back to a guessed value. The extractor emits
//! ordinal-sorted keys with no insignificant whitespace, so regenerating the
//! same jar produces byte-identical output.

use crate::item_components::{
    BlockSet, ComponentMap, ComponentValue, DEFAULT_MAX_STACK_SIZE, FoodProperties, ItemStack,
    Rarity, Tool, ToolRule, normalize_key,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::OnceLock;

/// The vanilla data version this catalog was extracted from.
pub const ITEM_PROTOTYPE_DATA_VERSION: i32 = 4788;

/// The vanilla version string this catalog was extracted from.
pub const ITEM_PROTOTYPE_VERSION: &str = "1.21.11";

/// Vanilla `Item.ABSOLUTE_MAX_STACK_SIZE`.
pub const ABSOLUTE_MAX_STACK_SIZE: i32 = 99;

/// Vanilla `Consumable.DEFAULT_CONSUME_SECONDS`.
pub const DEFAULT_EAT_SECONDS: f32 = 1.6;

/// The generated catalog embedded into the binary.
pub const EMBEDDED_ITEM_PROTOTYPES: &str = include_str!("../assets/item_prototypes.json");

/// A block/item holder set: either a tag or an explicit sorted id list.
///
/// Vanilla uses `HolderSet` for tool rules and repair ingredients. The tag form
/// is kept as a tag so callers can resolve it through [`crate::tags`] instead of
/// expanding it to thousands of ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngredientSet {
    Tag(String),
    Items(Vec<String>),
}

impl IngredientSet {
    pub fn tag(&self) -> Option<&str> {
        match self {
            IngredientSet::Tag(tag) => Some(tag),
            IngredientSet::Items(_) => None,
        }
    }

    pub fn items(&self) -> &[String] {
        match self {
            IngredientSet::Tag(_) => &[],
            IngredientSet::Items(items) => items,
        }
    }
}

/// One `Tool.Rule`: blocks, an optional mining-speed override, and an optional
/// correct-for-drops override. Rules are evaluated in order by vanilla.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRulePrototype {
    pub blocks: IngredientSet,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

impl ToolRulePrototype {
    fn to_component(&self) -> ToolRule {
        ToolRule {
            blocks: match &self.blocks {
                IngredientSet::Tag(tag) => BlockSet::Tag(tag.clone()),
                IngredientSet::Items(items) => BlockSet::Blocks(items.clone()),
            },
            speed: self.speed,
            correct_for_drops: self.correct_for_drops,
        }
    }
}

/// The `minecraft:tool` prototype plus the attack attributes vanilla derives
/// from the tool material.
///
/// `tier` is not a component in 1.21.11; the extractor derives it from the
/// item's repair tag (`*_tool_materials`) or its `incorrect_for_*_tool` rule so
/// callers can still answer "what tier is this tool" without parsing names.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPrototype {
    pub tier: Option<String>,
    pub default_mining_speed: f32,
    pub damage_per_block: i32,
    pub can_destroy_blocks_in_creative: bool,
    pub rules: Vec<ToolRulePrototype>,
    pub attack_damage: Option<f32>,
    pub attack_speed: Option<f32>,
}

impl ToolPrototype {
    pub fn to_component(&self) -> Tool {
        Tool {
            rules: self
                .rules
                .iter()
                .map(ToolRulePrototype::to_component)
                .collect(),
            default_mining_speed: self.default_mining_speed,
            damage_per_block: self.damage_per_block,
            can_destroy_blocks_in_creative: self.can_destroy_blocks_in_creative,
        }
    }
}

/// The `minecraft:food` prototype plus its `minecraft:consumable` eat time.
#[derive(Debug, Clone, PartialEq)]
pub struct FoodPrototype {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
    pub eat_seconds: f32,
}

impl FoodPrototype {
    /// Vanilla `Consumable.consumeTicks`: `(int) (consumeSeconds * 20.0F)`.
    pub fn eat_ticks(&self) -> i32 {
        (self.eat_seconds * 20.0) as i32
    }
}

/// The prototype defaults for one item id.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPrototype {
    pub max_stack_size: i32,
    pub max_damage: Option<i32>,
    pub tool: Option<ToolPrototype>,
    pub food: Option<FoodPrototype>,
    pub equippable_slot: Option<String>,
    pub enchantability: Option<i32>,
    pub repairable: Option<IngredientSet>,
    pub fire_resistant: bool,
    pub rarity: Rarity,
}

impl Default for ItemPrototype {
    fn default() -> Self {
        Self {
            max_stack_size: DEFAULT_MAX_STACK_SIZE,
            max_damage: None,
            tool: None,
            food: None,
            equippable_slot: None,
            enchantability: None,
            repairable: None,
            fire_resistant: false,
            rarity: Rarity::Common,
        }
    }
}

impl ItemPrototype {
    pub fn is_damageable(&self) -> bool {
        self.max_damage.is_some()
    }

    pub fn eat_ticks(&self) -> Option<i32> {
        self.food.as_ref().map(FoodPrototype::eat_ticks)
    }

    /// Build the component patch this prototype adds over the common item
    /// components. Components that [`crate::item_components`] does not model
    /// (equippable, enchantability, repairable, fire resistance) stay on the
    /// prototype and are intentionally not invented as opaque tags.
    pub fn component_patch(&self) -> ComponentMap {
        let mut patch = ComponentMap::new();
        if self.max_stack_size != DEFAULT_MAX_STACK_SIZE {
            add_component(
                &mut patch,
                ComponentValue::MaxStackSize(self.max_stack_size),
            );
        }
        if let Some(max_damage) = self.max_damage {
            add_component(&mut patch, ComponentValue::MaxDamage(max_damage));
            add_component(&mut patch, ComponentValue::Damage(0));
        }
        if let Some(food) = &self.food {
            add_component(
                &mut patch,
                ComponentValue::Food(FoodProperties {
                    nutrition: food.nutrition,
                    saturation: food.saturation,
                    can_always_eat: food.can_always_eat,
                }),
            );
        }
        if let Some(tool) = &self.tool {
            add_component(&mut patch, ComponentValue::Tool(tool.to_component()));
        }
        if self.rarity != Rarity::Common {
            add_component(&mut patch, ComponentValue::Rarity(self.rarity));
        }
        patch
    }
}

fn add_component(patch: &mut ComponentMap, value: ComponentValue) {
    let _ = patch.set(value);
}

/// The catalog's recomputable counts index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrototypeCounts {
    pub total: usize,
    pub by_rarity: BTreeMap<String, usize>,
    pub damageable: usize,
    pub enchantable: usize,
    pub equippable: usize,
    pub fire_resistant: usize,
    pub food: usize,
    pub repairable: usize,
    pub tool: usize,
}

impl PrototypeCounts {
    fn from_items(items: &HashMap<String, ItemPrototype>) -> Self {
        let mut by_rarity = BTreeMap::new();
        for rarity in [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::Epic] {
            by_rarity.insert(rarity.name().to_owned(), 0usize);
        }

        let mut counts = Self {
            total: items.len(),
            by_rarity,
            damageable: 0,
            enchantable: 0,
            equippable: 0,
            fire_resistant: 0,
            food: 0,
            repairable: 0,
            tool: 0,
        };
        for prototype in items.values() {
            if let Some(count) = counts.by_rarity.get_mut(prototype.rarity.name()) {
                *count += 1;
            }
            counts.damageable += usize::from(prototype.max_damage.is_some());
            counts.enchantable += usize::from(prototype.enchantability.is_some());
            counts.equippable += usize::from(prototype.equippable_slot.is_some());
            counts.fire_resistant += usize::from(prototype.fire_resistant);
            counts.food += usize::from(prototype.food.is_some());
            counts.repairable += usize::from(prototype.repairable.is_some());
            counts.tool += usize::from(prototype.tool.is_some());
        }
        counts
    }
}

/// The parsed item prototype catalog.
#[derive(Debug, Clone)]
pub struct ItemCatalog {
    items: HashMap<String, ItemPrototype>,
    counts: PrototypeCounts,
}

impl ItemCatalog {
    /// Parse and validate a catalog. Unknown fields, invalid ranges, and a
    /// counts index that disagrees with the entries are errors.
    pub fn from_json(text: &str) -> io::Result<Self> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| invalid(format!("item prototype catalog is not JSON: {error}")))?;
        let root = object(&value, "catalog")?;
        reject_unknown(
            root,
            &["counts", "data_version", "items", "version"],
            "catalog",
        )?;

        let version = string(required(root, "version", "catalog")?, "catalog.version")?;
        if version != ITEM_PROTOTYPE_VERSION {
            return Err(invalid(format!(
                "catalog version is '{version}', expected '{ITEM_PROTOTYPE_VERSION}'"
            )));
        }
        let data_version = integer(
            required(root, "data_version", "catalog")?,
            "catalog.data_version",
        )?;
        if data_version != i64::from(ITEM_PROTOTYPE_DATA_VERSION) {
            return Err(invalid(format!(
                "catalog data version is {data_version}, expected {ITEM_PROTOTYPE_DATA_VERSION}"
            )));
        }

        let items_value = required(root, "items", "catalog")?;
        let items_object = object(items_value, "catalog.items")?;
        if items_object.is_empty() {
            return Err(invalid("catalog.items must not be empty"));
        }
        let mut items = HashMap::with_capacity(items_object.len());
        for (id, entry) in items_object {
            if !id.contains(':') {
                return Err(invalid(format!("item id '{id}' is not namespaced")));
            }
            let prototype = parse_prototype(entry, id)?;
            items.insert(id.clone(), prototype);
        }

        let counts = parse_counts(required(root, "counts", "catalog")?, &items)?;
        Ok(Self { items, counts })
    }

    /// The embedded catalog, parsed once on first use.
    pub fn embedded() -> &'static Self {
        static CATALOG: OnceLock<ItemCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            Self::from_json(EMBEDDED_ITEM_PROTOTYPES)
                .unwrap_or_else(|error| panic!("assets/item_prototypes.json is invalid: {error}"))
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn counts(&self) -> &PrototypeCounts {
        &self.counts
    }

    pub fn get(&self, item: &str) -> Option<&ItemPrototype> {
        let key = normalize_key(item);
        self.items.get(&key)
    }

    pub fn contains(&self, item: &str) -> bool {
        let key = normalize_key(item);
        self.items.contains_key(&key)
    }

    pub fn get_by_id(&self, id: i32) -> Option<&ItemPrototype> {
        crate::registry::item_name(id).and_then(|name| self.get(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ItemPrototype)> {
        self.items
            .iter()
            .map(|(name, prototype)| (name.as_str(), prototype))
    }
}

/// The embedded catalog.
pub fn catalog() -> &'static ItemCatalog {
    ItemCatalog::embedded()
}

/// Look up one item's prototype, or `None` for an unknown/modded id.
pub fn prototype(item: &str) -> Option<&'static ItemPrototype> {
    catalog().get(item)
}

/// Look up the prototype for a registry id.
pub fn prototype_by_id(id: i32) -> Option<&'static ItemPrototype> {
    catalog().get_by_id(id)
}

/// A prototype with the explicit "no overrides" fallback: stack 64, no
/// durability, common rarity, and no tool/food/equipment data.
pub fn prototype_or_default(item: &str) -> &'static ItemPrototype {
    static DEFAULT: ItemPrototype = ItemPrototype {
        max_stack_size: DEFAULT_MAX_STACK_SIZE,
        max_damage: None,
        tool: None,
        food: None,
        equippable_slot: None,
        enchantability: None,
        repairable: None,
        fire_resistant: false,
        rarity: Rarity::Common,
    };
    prototype(item).unwrap_or(&DEFAULT)
}

/// The component patch for an item's prototype defaults.
///
/// `max_stack_size: 64` is omitted because it is the shared fallback in
/// [`crate::item_components`]; every other modeled default is emitted.
pub fn defaults_for(item: &str) -> ComponentMap {
    prototype_or_default(item).component_patch()
}

/// Apply an item's prototype defaults to a stack without replacing explicit
/// components or honoring an explicit `!component` removal. Returns how many
/// components were added.
pub fn apply_defaults(stack: &mut ItemStack) -> usize {
    let patch = defaults_for(&stack.item);
    let mut applied = 0;
    for (component, value) in patch.iter_added() {
        if stack.components.get(component).is_some()
            || stack.components.is_removed(component.name())
        {
            continue;
        }
        let value = value.clone();
        if stack.set_component_named(component.name(), value).is_ok() {
            applied += 1;
        }
    }
    applied
}

fn parse_prototype(value: &Value, id: &str) -> io::Result<ItemPrototype> {
    let context = format!("items.{id}");
    let object = object(value, &context)?;
    reject_unknown(
        object,
        &[
            "enchantability",
            "equippable",
            "fire_resistant",
            "food",
            "max_damage",
            "max_stack_size",
            "rarity",
            "repairable",
            "tool",
        ],
        &context,
    )?;

    let max_stack_size = match object.get("max_stack_size") {
        Some(value) => integer(value, &format!("{context}.max_stack_size"))?,
        None => i64::from(DEFAULT_MAX_STACK_SIZE),
    };
    let max_stack_size = i32::try_from(max_stack_size)
        .map_err(|_| invalid(format!("{context}.max_stack_size is out of range")))?;
    if !(1..=ABSOLUTE_MAX_STACK_SIZE).contains(&max_stack_size) {
        return Err(invalid(format!(
            "{context}.max_stack_size {max_stack_size} is outside 1..={ABSOLUTE_MAX_STACK_SIZE}"
        )));
    }

    let max_damage = match object.get("max_damage") {
        Some(value) => {
            let damage = integer(value, &format!("{context}.max_damage"))?;
            if damage <= 0 {
                return Err(invalid(format!("{context}.max_damage must be positive")));
            }
            Some(
                i32::try_from(damage)
                    .map_err(|_| invalid(format!("{context}.max_damage is out of range")))?,
            )
        }
        None => None,
    };
    if max_damage.is_some() && max_stack_size != 1 {
        return Err(invalid(format!(
            "{context} is damageable but max_stack_size is {max_stack_size}, not 1"
        )));
    }

    let tool = object
        .get("tool")
        .map(|value| parse_tool(value, &format!("{context}.tool")))
        .transpose()?;
    let food = object
        .get("food")
        .map(|value| parse_food(value, &format!("{context}.food")))
        .transpose()?;

    let equippable_slot = match object.get("equippable") {
        Some(value) => {
            let slot = string(value, &format!("{context}.equippable"))?;
            if !EQUIPMENT_SLOTS.contains(&slot) {
                return Err(invalid(format!("{context}.equippable '{slot}' is unknown")));
            }
            Some(slot.to_owned())
        }
        None => None,
    };

    let enchantability = match object.get("enchantability") {
        Some(value) => {
            let enchantability = integer(value, &format!("{context}.enchantability"))?;
            if enchantability <= 0 {
                return Err(invalid(format!(
                    "{context}.enchantability must be positive"
                )));
            }
            Some(
                i32::try_from(enchantability)
                    .map_err(|_| invalid(format!("{context}.enchantability is out of range")))?,
            )
        }
        None => None,
    };

    let repairable = object
        .get("repairable")
        .map(|value| parse_ingredient_set(value, &format!("{context}.repairable")))
        .transpose()?;

    let fire_resistant = match object.get("fire_resistant") {
        Some(value) => boolean(value, &format!("{context}.fire_resistant"))?,
        None => false,
    };

    let rarity = match object.get("rarity") {
        Some(value) => {
            let name = string(value, &format!("{context}.rarity"))?;
            Rarity::from_name(name)
                .ok_or_else(|| invalid(format!("{context}.rarity '{name}' is unknown")))?
        }
        None => Rarity::Common,
    };

    Ok(ItemPrototype {
        max_stack_size,
        max_damage,
        tool,
        food,
        equippable_slot,
        enchantability,
        repairable,
        fire_resistant,
        rarity,
    })
}

fn parse_tool(value: &Value, context: &str) -> io::Result<ToolPrototype> {
    let object = object(value, context)?;
    reject_unknown(
        object,
        &[
            "attack_damage",
            "attack_speed",
            "can_destroy_blocks_in_creative",
            "damage_per_block",
            "default_mining_speed",
            "rules",
            "tier",
        ],
        context,
    )?;

    let tier = match object.get("tier") {
        Some(value) => {
            let tier = string(value, &format!("{context}.tier"))?;
            if tier.is_empty() {
                return Err(invalid(format!("{context}.tier must not be empty")));
            }
            Some(tier.to_owned())
        }
        None => None,
    };

    let default_mining_speed = optional_number(object, "default_mining_speed", 1.0, context)?;
    if default_mining_speed < 0.0 {
        return Err(invalid(format!(
            "{context}.default_mining_speed must not be negative"
        )));
    }
    let damage_per_block = match object.get("damage_per_block") {
        Some(value) => {
            let damage = integer(value, &format!("{context}.damage_per_block"))?;
            if damage < 0 {
                return Err(invalid(format!(
                    "{context}.damage_per_block must not be negative"
                )));
            }
            i32::try_from(damage)
                .map_err(|_| invalid(format!("{context}.damage_per_block is out of range")))?
        }
        None => 1,
    };
    let can_destroy_blocks_in_creative = match object.get("can_destroy_blocks_in_creative") {
        Some(value) => boolean(value, &format!("{context}.can_destroy_blocks_in_creative"))?,
        None => true,
    };

    let rules_value = required(object, "rules", context)?;
    let rules = array(rules_value, &format!("{context}.rules"))?
        .iter()
        .enumerate()
        .map(|(index, rule)| parse_tool_rule(rule, &format!("{context}.rules[{index}]")))
        .collect::<io::Result<Vec<_>>>()?;

    let attack_damage = optional_optional_number(object, "attack_damage", context)?;
    let attack_speed = optional_optional_number(object, "attack_speed", context)?;

    Ok(ToolPrototype {
        tier,
        default_mining_speed,
        damage_per_block,
        can_destroy_blocks_in_creative,
        rules,
        attack_damage,
        attack_speed,
    })
}

fn parse_tool_rule(value: &Value, context: &str) -> io::Result<ToolRulePrototype> {
    let object = object(value, context)?;
    reject_unknown(object, &["blocks", "correct_for_drops", "speed"], context)?;
    let blocks = parse_ingredient_set(
        required(object, "blocks", context)?,
        &format!("{context}.blocks"),
    )?;
    let speed = match object.get("speed") {
        Some(value) => {
            let speed = number(value, &format!("{context}.speed"))?;
            if speed <= 0.0 {
                return Err(invalid(format!("{context}.speed must be positive")));
            }
            Some(speed)
        }
        None => None,
    };
    let correct_for_drops = match object.get("correct_for_drops") {
        Some(value) => Some(boolean(value, &format!("{context}.correct_for_drops"))?),
        None => None,
    };
    Ok(ToolRulePrototype {
        blocks,
        speed,
        correct_for_drops,
    })
}

fn parse_food(value: &Value, context: &str) -> io::Result<FoodPrototype> {
    let object = object(value, context)?;
    reject_unknown(
        object,
        &["can_always_eat", "eat_seconds", "nutrition", "saturation"],
        context,
    )?;

    let nutrition = integer(
        required(object, "nutrition", context)?,
        &format!("{context}.nutrition"),
    )?;
    if nutrition < 0 {
        return Err(invalid(format!("{context}.nutrition must not be negative")));
    }
    let nutrition = i32::try_from(nutrition)
        .map_err(|_| invalid(format!("{context}.nutrition is out of range")))?;

    let saturation = number(
        required(object, "saturation", context)?,
        &format!("{context}.saturation"),
    )?;
    if saturation < 0.0 {
        return Err(invalid(format!(
            "{context}.saturation must not be negative"
        )));
    }

    let can_always_eat = match object.get("can_always_eat") {
        Some(value) => boolean(value, &format!("{context}.can_always_eat"))?,
        None => false,
    };
    let eat_seconds = optional_number(object, "eat_seconds", DEFAULT_EAT_SECONDS, context)?;
    if eat_seconds < 0.0 {
        return Err(invalid(format!(
            "{context}.eat_seconds must not be negative"
        )));
    }

    Ok(FoodPrototype {
        nutrition,
        saturation,
        can_always_eat,
        eat_seconds,
    })
}

fn parse_ingredient_set(value: &Value, context: &str) -> io::Result<IngredientSet> {
    let object = object(value, context)?;
    reject_unknown(object, &["items", "tag"], context)?;
    match (object.get("tag"), object.get("items")) {
        (Some(tag), None) => {
            let tag = string(tag, &format!("{context}.tag"))?;
            if tag.is_empty() || tag.starts_with('#') || !tag.contains(':') {
                return Err(invalid(format!(
                    "{context}.tag '{tag}' must be a namespaced tag without '#'"
                )));
            }
            Ok(IngredientSet::Tag(tag.to_owned()))
        }
        (None, Some(items)) => {
            let items = array(items, &format!("{context}.items"))?;
            if items.is_empty() {
                return Err(invalid(format!("{context}.items must not be empty")));
            }
            let mut parsed = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let item = string(item, &format!("{context}.items[{index}]"))?;
                if !item.contains(':') {
                    return Err(invalid(format!(
                        "{context}.items[{index}] '{item}' is not namespaced"
                    )));
                }
                parsed.push(item.to_owned());
            }
            Ok(IngredientSet::Items(parsed))
        }
        _ => Err(invalid(format!(
            "{context} must have exactly one of 'tag' or 'items'"
        ))),
    }
}

fn parse_counts(
    value: &Value,
    items: &HashMap<String, ItemPrototype>,
) -> io::Result<PrototypeCounts> {
    let counts_object = object(value, "catalog.counts")?;
    reject_unknown(
        counts_object,
        &[
            "by_rarity",
            "damageable",
            "enchantable",
            "equippable",
            "fire_resistant",
            "food",
            "repairable",
            "tool",
            "total",
        ],
        "catalog.counts",
    )?;

    let total = usize_value(
        required(counts_object, "total", "catalog.counts")?,
        "catalog.counts.total",
    )?;
    let by_rarity_object = object(
        required(counts_object, "by_rarity", "catalog.counts")?,
        "catalog.counts.by_rarity",
    )?;
    let mut by_rarity = BTreeMap::new();
    for (rarity, count) in by_rarity_object {
        if Rarity::from_name(rarity).is_none() {
            return Err(invalid(format!(
                "catalog.counts.by_rarity has unknown rarity '{rarity}'"
            )));
        }
        by_rarity.insert(
            String::from(rarity.as_str()),
            usize_value(count, &format!("catalog.counts.by_rarity.{rarity}"))?,
        );
    }

    let counts = PrototypeCounts {
        total,
        by_rarity,
        damageable: usize_field(counts_object, "damageable")?,
        enchantable: usize_field(counts_object, "enchantable")?,
        equippable: usize_field(counts_object, "equippable")?,
        fire_resistant: usize_field(counts_object, "fire_resistant")?,
        food: usize_field(counts_object, "food")?,
        repairable: usize_field(counts_object, "repairable")?,
        tool: usize_field(counts_object, "tool")?,
    };
    let computed = PrototypeCounts::from_items(items);
    if counts != computed {
        return Err(invalid(format!(
            "catalog.counts disagrees with items: stored {counts:?}, computed {computed:?}"
        )));
    }
    Ok(counts)
}

fn usize_field(object: &Map<String, Value>, key: &str) -> io::Result<usize> {
    usize_value(
        required(object, key, "catalog.counts")?,
        &format!("catalog.counts.{key}"),
    )
}

const EQUIPMENT_SLOTS: [&str; 8] = [
    "mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle",
];

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn object<'a>(value: &'a Value, context: &str) -> io::Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{context} must be an object")))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> io::Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| invalid(format!("{context} is missing '{key}'")))
}

fn string<'a>(value: &'a Value, context: &str) -> io::Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{context} must be a string")))
}

fn integer(value: &Value, context: &str) -> io::Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| invalid(format!("{context} must be an integer")))
}

fn usize_value(value: &Value, context: &str) -> io::Result<usize> {
    let value = value
        .as_u64()
        .ok_or_else(|| invalid(format!("{context} must be a non-negative integer")))?;
    usize::try_from(value).map_err(|_| invalid(format!("{context} is out of range")))
}

fn number(value: &Value, context: &str) -> io::Result<f32> {
    let value = value
        .as_f64()
        .ok_or_else(|| invalid(format!("{context} must be a number")))?;
    let value = value as f32;
    if !value.is_finite() {
        return Err(invalid(format!("{context} must be finite")));
    }
    Ok(value)
}

fn optional_number(
    object: &Map<String, Value>,
    key: &str,
    default: f32,
    context: &str,
) -> io::Result<f32> {
    match object.get(key) {
        Some(value) => number(value, &format!("{context}.{key}")),
        None => Ok(default),
    }
}

fn optional_optional_number(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> io::Result<Option<f32>> {
    match object.get(key) {
        Some(value) => number(value, &format!("{context}.{key}")).map(Some),
        None => Ok(None),
    }
}

fn boolean(value: &Value, context: &str) -> io::Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| invalid(format!("{context} must be a boolean")))
}

fn array<'a>(value: &'a Value, context: &str) -> io::Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| invalid(format!("{context} must be an array")))
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str], context: &str) -> io::Result<()> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(format!("{context} has unknown field '{key}'")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_no_override_default_is_stack_64_without_durability() {
        let prototype = ItemPrototype::default();
        assert_eq!(prototype.max_stack_size, DEFAULT_MAX_STACK_SIZE);
        assert_eq!(prototype.max_damage, None);
        assert_eq!(prototype.rarity, Rarity::Common);
        assert!(prototype.component_patch().is_empty());
    }

    #[test]
    fn eat_ticks_truncates_like_vanilla_consumable() {
        let food = FoodPrototype {
            nutrition: 4,
            saturation: 2.4,
            can_always_eat: false,
            eat_seconds: 1.6,
        };
        assert_eq!(food.eat_ticks(), 32);
        let instant = FoodPrototype {
            eat_seconds: 0.0,
            ..food
        };
        assert_eq!(instant.eat_ticks(), 0);
    }

    #[test]
    fn patch_omits_the_shared_stack_size_but_keeps_durability() {
        let prototype = ItemPrototype {
            max_stack_size: 1,
            max_damage: Some(1561),
            ..ItemPrototype::default()
        };
        let patch = prototype.component_patch();
        assert!(patch.get_named("minecraft:max_stack_size").is_some());
        assert_eq!(
            patch.get_named("minecraft:max_damage"),
            Some(&ComponentValue::MaxDamage(1561))
        );
        assert_eq!(
            patch.get_named("minecraft:damage"),
            Some(&ComponentValue::Damage(0))
        );
    }
}
