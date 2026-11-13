//! Typed vanilla 1.21.11 loot tables and recipes.
//!
//! `scripts/extract_loot_recipes.ps1` normalizes the decompiled data tree into
//! two compact, deterministic catalogs embedded below:
//!
//! * [`LootCatalog`] (`assets/loot_tables.json`, 1,324 tables)
//! * [`RecipeCatalog`] (`assets/recipes.json`, 1,470 recipes)
//!
//! The parser keeps the vanilla data model intact: pools, number providers,
//! weighted entries, functions, and conditions are typed. Functions and
//! conditions this port does not execute are preserved in
//! [`LootFunctionKind::Opaque`] / [`LootCondition::Opaque`] and reported by
//! [`LootRoll::report`] instead of silently doing nothing.
//!
//! The roll engine follows Java's `LootPool`/`LootTable` draw order:
//! entry weights use the integer luck formula, a one-entry pool does not draw
//! a roll, and entry -> pool -> table functions apply in that order.

use crate::item_components::{
    ComponentType, ComponentValue, ItemStack, PotionContents, component_from_nbt,
};
use crate::nbt::Tag;
use crate::tags::{self, TagRegistryKind};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::OnceLock;

const LOOT_TABLES_JSON: &str = include_str!("../assets/loot_tables.json");
const RECIPES_JSON: &str = include_str!("../assets/recipes.json");

const TARGET_VERSION: &str = "1.21.11";
const TARGET_DATA_VERSION: i32 = 4788;
const MAX_NESTED_TABLES: usize = 16;

/// A parse failure caused by malformed catalog data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootError {
    message: String,
}

impl LootError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LootError {}

/// Header shared by the two generated catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIndex {
    pub version: String,
    pub data_version: i32,
    pub total: usize,
    pub by_type: BTreeMap<String, usize>,
}

impl AssetIndex {
    fn from_json(root: &Map<String, Value>) -> Result<Self, LootError> {
        let version = string_field(root, "version")?.to_owned();
        let data_version = i32_field(root, "data_version")?;
        if version != TARGET_VERSION {
            return Err(LootError::new(format!(
                "asset version {version} does not match target {TARGET_VERSION}"
            )));
        }
        if data_version != TARGET_DATA_VERSION {
            return Err(LootError::new(format!(
                "asset data version {data_version} does not match target {TARGET_DATA_VERSION}"
            )));
        }

        let counts = object_field(root, "counts")?;
        let total = usize_field(counts, "total")?;
        let by_type_value = counts
            .get("by_type")
            .ok_or_else(|| LootError::new("counts is missing `by_type`"))?;
        let by_type_object = as_object(by_type_value)?;
        let mut by_type = BTreeMap::new();
        for (kind, count) in by_type_object {
            by_type.insert(kind.clone(), usize_value(count)?);
        }
        let sum: usize = by_type.values().sum();
        if sum != total {
            return Err(LootError::new(format!(
                "counts.total {total} does not match by_type sum {sum}"
            )));
        }
        Ok(Self {
            version,
            data_version,
            total,
            by_type,
        })
    }
}

// ---------------------------------------------------------------------------
// Number providers and level-based values
// ---------------------------------------------------------------------------

/// A vanilla `NumberProvider` (`constant`, `uniform`, `binomial`,
/// `enchantment_level`, `score`, `storage`).
#[derive(Debug, Clone, PartialEq)]
pub enum NumberProvider {
    Constant(f32),
    Uniform {
        min: Box<NumberProvider>,
        max: Box<NumberProvider>,
    },
    Binomial {
        n: Box<NumberProvider>,
        p: Box<NumberProvider>,
    },
    EnchantmentLevel {
        amount: LevelBasedValue,
    },
    /// Scoreboard provider; preserved but not evaluated until a scoreboard
    /// context exists.
    Score {
        target: Value,
        score: String,
        scale: f32,
    },
    /// Command-storage provider; preserved but not evaluated until a storage
    /// context exists.
    Storage {
        storage: String,
        path: String,
        scale: f32,
    },
    Opaque(Value),
}

impl NumberProvider {
    /// Parses the number-or-object shape used by vanilla's `NumberProviders`.
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        let Some(object) = value.as_object() else {
            return Ok(NumberProvider::Constant(number_value(value)? as f32));
        };
        let kind = string_field(object, "type")?;
        match kind {
            "minecraft:constant" => Ok(NumberProvider::Constant(
                number_field(object, "value")? as f32
            )),
            "minecraft:uniform" => Ok(NumberProvider::Uniform {
                min: Box::new(NumberProvider::from_json(required(object, "min")?)?),
                max: Box::new(NumberProvider::from_json(required(object, "max")?)?),
            }),
            "minecraft:binomial" => Ok(NumberProvider::Binomial {
                n: Box::new(NumberProvider::from_json(required(object, "n")?)?),
                p: Box::new(NumberProvider::from_json(required(object, "p")?)?),
            }),
            "minecraft:enchantment_level" => Ok(NumberProvider::EnchantmentLevel {
                amount: LevelBasedValue::from_json(required(object, "amount")?)?,
            }),
            "minecraft:score" => Ok(NumberProvider::Score {
                target: required(object, "target")?.clone(),
                score: string_field(object, "score")?.to_owned(),
                scale: optional_number_field(object, "scale")?.unwrap_or(1.0) as f32,
            }),
            "minecraft:storage" => Ok(NumberProvider::Storage {
                storage: string_field(object, "storage")?.to_owned(),
                path: string_field(object, "path")?.to_owned(),
                scale: optional_number_field(object, "scale")?.unwrap_or(1.0) as f32,
            }),
            _ => Ok(NumberProvider::Opaque(value.clone())),
        }
    }

    /// A stable kind name for reports.
    pub fn kind(&self) -> &str {
        match self {
            NumberProvider::Constant(_) => "minecraft:constant",
            NumberProvider::Uniform { .. } => "minecraft:uniform",
            NumberProvider::Binomial { .. } => "minecraft:binomial",
            NumberProvider::EnchantmentLevel { .. } => "minecraft:enchantment_level",
            NumberProvider::Score { .. } => "minecraft:score",
            NumberProvider::Storage { .. } => "minecraft:storage",
            NumberProvider::Opaque(_) => "unknown",
        }
    }

    /// Whether every nested provider can be evaluated with the current
    /// context model.
    pub fn is_supported(&self) -> bool {
        match self {
            NumberProvider::Constant(_) | NumberProvider::EnchantmentLevel { .. } => true,
            NumberProvider::Uniform { min, max } => min.is_supported() && max.is_supported(),
            NumberProvider::Binomial { n, p } => n.is_supported() && p.is_supported(),
            NumberProvider::Score { .. }
            | NumberProvider::Storage { .. }
            | NumberProvider::Opaque(_) => false,
        }
    }

    /// Samples the provider as a float. Java's `NumberProvider.getFloat`.
    pub fn sample_float(&self, context: &LootContext<'_>, random: &mut dyn RandomSource) -> f32 {
        match self {
            NumberProvider::Constant(value) => *value,
            NumberProvider::Uniform { min, max } => {
                let minimum = min.sample_float(context, random);
                let maximum = max.sample_float(context, random);
                mth_next_float(random, minimum, maximum)
            }
            NumberProvider::Binomial { .. } => self.sample_int(context, random) as f32,
            NumberProvider::EnchantmentLevel { amount } => {
                amount.calculate(context.enchantment_level)
            }
            NumberProvider::Score { .. }
            | NumberProvider::Storage { .. }
            | NumberProvider::Opaque(_) => 0.0,
        }
    }

    /// Samples the provider as an integer. Java's `NumberProvider.getInt`
    /// rounds the float value, except that uniform/binomial providers override
    /// it with discrete sampling.
    pub fn sample_int(&self, context: &LootContext<'_>, random: &mut dyn RandomSource) -> i32 {
        match self {
            NumberProvider::Uniform { min, max } => {
                let minimum = min.sample_int(context, random);
                let maximum = max.sample_int(context, random);
                mth_next_int(random, minimum, maximum)
            }
            NumberProvider::Binomial { n, p } => {
                let rounds = n.sample_int(context, random);
                let probability = p.sample_float(context, random);
                let mut count = 0;
                for _ in 0..rounds {
                    if random.next_float() < probability {
                        count += 1;
                    }
                }
                count
            }
            _ => java_round(self.sample_float(context, random)),
        }
    }
}

/// Vanilla `LevelBasedValue`; all shipped variants are evaluated exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum LevelBasedValue {
    Constant(f32),
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    Clamped {
        value: Box<LevelBasedValue>,
        min: f32,
        max: f32,
    },
    Fraction {
        numerator: Box<LevelBasedValue>,
        denominator: Box<LevelBasedValue>,
    },
    LevelsSquared {
        added: f32,
    },
    Exponent {
        base: Box<LevelBasedValue>,
        power: Box<LevelBasedValue>,
    },
    Lookup {
        values: Vec<f32>,
        fallback: Box<LevelBasedValue>,
    },
    Opaque(Value),
}

impl LevelBasedValue {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        if !value.is_object() {
            return Ok(LevelBasedValue::Constant(number_value(value)? as f32));
        }
        let object = as_object(value)?;
        let kind = string_field(object, "type")?;
        match kind {
            "minecraft:clamped" => Ok(LevelBasedValue::Clamped {
                value: Box::new(LevelBasedValue::from_json(required(object, "value")?)?),
                min: number_field(object, "min")? as f32,
                max: number_field(object, "max")? as f32,
            }),
            "minecraft:fraction" => Ok(LevelBasedValue::Fraction {
                numerator: Box::new(LevelBasedValue::from_json(required(object, "numerator")?)?),
                denominator: Box::new(LevelBasedValue::from_json(required(
                    object,
                    "denominator",
                )?)?),
            }),
            "minecraft:levels_squared" => Ok(LevelBasedValue::LevelsSquared {
                added: number_field(object, "added")? as f32,
            }),
            "minecraft:linear" => Ok(LevelBasedValue::Linear {
                base: number_field(object, "base")? as f32,
                per_level_above_first: number_field(object, "per_level_above_first")? as f32,
            }),
            "minecraft:exponent" => Ok(LevelBasedValue::Exponent {
                base: Box::new(LevelBasedValue::from_json(required(object, "base")?)?),
                power: Box::new(LevelBasedValue::from_json(required(object, "power")?)?),
            }),
            "minecraft:lookup" => {
                let values = required(object, "values")?
                    .as_array()
                    .ok_or_else(|| LootError::new("lookup values must be an array"))?
                    .iter()
                    .map(number_value)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|value| value as f32)
                    .collect();
                Ok(LevelBasedValue::Lookup {
                    values,
                    fallback: Box::new(LevelBasedValue::from_json(required(object, "fallback")?)?),
                })
            }
            _ => Ok(LevelBasedValue::Opaque(value.clone())),
        }
    }

    pub fn calculate(&self, level: i32) -> f32 {
        match self {
            LevelBasedValue::Constant(value) => *value,
            LevelBasedValue::Linear {
                base,
                per_level_above_first,
            } => base + per_level_above_first * (level - 1) as f32,
            LevelBasedValue::Clamped { value, min, max } => {
                value.calculate(level).clamp(*min, *max)
            }
            LevelBasedValue::Fraction {
                numerator,
                denominator,
            } => {
                let divisor = denominator.calculate(level);
                if divisor == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / divisor
                }
            }
            LevelBasedValue::LevelsSquared { added } => (level * level) as f32 + added,
            LevelBasedValue::Exponent { base, power } => {
                base.calculate(level).powf(power.calculate(level))
            }
            LevelBasedValue::Lookup { values, fallback } => usize::try_from(level)
                .ok()
                .and_then(|index| values.get(index).copied())
                .unwrap_or_else(|| fallback.calculate(level)),
            LevelBasedValue::Opaque(_) => 0.0,
        }
    }
}

/// Vanilla `IntRange` (`limit_count`). Missing bounds are unbounded.
#[derive(Debug, Clone, PartialEq)]
pub struct IntRange {
    pub min: Option<NumberProvider>,
    pub max: Option<NumberProvider>,
}

impl IntRange {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        if !value.is_object() {
            let constant = NumberProvider::Constant(number_value(value)? as f32);
            return Ok(Self {
                min: Some(constant.clone()),
                max: Some(constant),
            });
        }
        let object = as_object(value)?;
        Ok(Self {
            min: object
                .get("min")
                .map(NumberProvider::from_json)
                .transpose()?,
            max: object
                .get("max")
                .map(NumberProvider::from_json)
                .transpose()?,
        })
    }

    pub fn clamp(
        &self,
        value: i32,
        context: &LootContext<'_>,
        random: &mut dyn RandomSource,
    ) -> i32 {
        let mut clamped = value;
        if let Some(min) = &self.min {
            clamped = clamped.max(min.sample_int(context, random));
        }
        if let Some(max) = &self.max {
            clamped = clamped.min(max.sample_int(context, random));
        }
        clamped
    }
}

// ---------------------------------------------------------------------------
// Loot tables, pools, and entries
// ---------------------------------------------------------------------------

/// Loot-context parameter set (`LootContextParamSets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LootTableKind {
    Generic,
    Block,
    Chest,
    Entity,
    Fishing,
    Gift,
    Barter,
    Archaeology,
    Shearing,
    Equipment,
    EntityInteract,
    BlockInteract,
    Unknown(String),
}

impl LootTableKind {
    pub fn from_id(id: &str) -> Self {
        match normalized(id).as_str() {
            "minecraft:generic" => Self::Generic,
            "minecraft:block" => Self::Block,
            "minecraft:chest" => Self::Chest,
            "minecraft:entity" => Self::Entity,
            "minecraft:fishing" => Self::Fishing,
            "minecraft:gift" => Self::Gift,
            "minecraft:barter" => Self::Barter,
            "minecraft:archaeology" => Self::Archaeology,
            "minecraft:shearing" => Self::Shearing,
            "minecraft:equipment" => Self::Equipment,
            "minecraft:entity_interact" => Self::EntityInteract,
            "minecraft:block_interact" => Self::BlockInteract,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Generic => "minecraft:generic",
            Self::Block => "minecraft:block",
            Self::Chest => "minecraft:chest",
            Self::Entity => "minecraft:entity",
            Self::Fishing => "minecraft:fishing",
            Self::Gift => "minecraft:gift",
            Self::Barter => "minecraft:barter",
            Self::Archaeology => "minecraft:archaeology",
            Self::Shearing => "minecraft:shearing",
            Self::Equipment => "minecraft:equipment",
            Self::EntityInteract => "minecraft:entity_interact",
            Self::BlockInteract => "minecraft:block_interact",
            Self::Unknown(id) => id,
        }
    }
}

/// A parsed loot table.
#[derive(Debug, Clone, PartialEq)]
pub struct LootTable {
    /// Catalog id, or `None` for an inline nested table.
    pub id: Option<String>,
    pub kind: LootTableKind,
    pub random_sequence: Option<String>,
    pub pools: Vec<LootPool>,
    pub functions: Vec<LootFunction>,
}

impl LootTable {
    pub fn from_json(id: Option<&str>, value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let kind = LootTableKind::from_id(
            object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("minecraft:generic"),
        );
        let random_sequence = object
            .get("random_sequence")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let pools = optional_array_field(object, "pools")?
            .iter()
            .map(LootPool::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let functions = optional_array_field(object, "functions")?
            .iter()
            .map(LootFunction::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id: id.map(str::to_owned),
            kind,
            random_sequence,
            pools,
            functions,
        })
    }
}

/// A loot pool with its rolls, entries, conditions, and functions.
#[derive(Debug, Clone, PartialEq)]
pub struct LootPool {
    pub rolls: NumberProvider,
    pub bonus_rolls: NumberProvider,
    pub entries: Vec<LootEntry>,
    pub conditions: Vec<LootCondition>,
    pub functions: Vec<LootFunction>,
}

impl LootPool {
    fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let rolls = NumberProvider::from_json(required(object, "rolls")?)?;
        let bonus_rolls = match object.get("bonus_rolls") {
            Some(value) => NumberProvider::from_json(value)?,
            None => NumberProvider::Constant(0.0),
        };
        let entries = optional_array_field(object, "entries")?
            .iter()
            .map(LootEntry::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let conditions = optional_array_field(object, "conditions")?
            .iter()
            .map(LootCondition::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let functions = optional_array_field(object, "functions")?
            .iter()
            .map(LootFunction::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            rolls,
            bonus_rolls,
            entries,
            conditions,
            functions,
        })
    }
}

/// A weighted pool entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LootEntry {
    pub weight: i32,
    pub quality: i32,
    pub conditions: Vec<LootCondition>,
    pub functions: Vec<LootFunction>,
    pub kind: LootEntryKind,
}

impl LootEntry {
    fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let weight = object
            .get("weight")
            .map(i32_value)
            .transpose()?
            .unwrap_or(1);
        let quality = object
            .get("quality")
            .map(i32_value)
            .transpose()?
            .unwrap_or(0);
        let conditions = optional_array_field(object, "conditions")?
            .iter()
            .map(LootCondition::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let functions = optional_array_field(object, "functions")?
            .iter()
            .map(LootFunction::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let kind = LootEntryKind::from_json(object)?;
        Ok(Self {
            weight,
            quality,
            conditions,
            functions,
            kind,
        })
    }
}

/// The entry variants used by 1.21.11.
#[derive(Debug, Clone, PartialEq)]
pub enum LootEntryKind {
    Item {
        name: String,
    },
    Tag {
        name: String,
        expand: bool,
    },
    Empty,
    Alternatives(Vec<LootEntry>),
    Sequence(Vec<LootEntry>),
    Group(Vec<LootEntry>),
    Dynamic {
        name: String,
    },
    NestedLootTable {
        table: Option<String>,
        inline: Option<Box<LootTable>>,
    },
    Opaque {
        kind: String,
        value: Value,
    },
}

impl LootEntryKind {
    fn from_json(object: &Map<String, Value>) -> Result<Self, LootError> {
        let kind = string_field(object, "type")?;
        match kind {
            "minecraft:item" => Ok(Self::Item {
                name: string_field(object, "name")?.to_owned(),
            }),
            "minecraft:tag" => Ok(Self::Tag {
                name: string_field(object, "name")?.to_owned(),
                expand: object
                    .get("expand")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }),
            "minecraft:empty" => Ok(Self::Empty),
            "minecraft:alternatives" => Ok(Self::Alternatives(parse_entries(object)?)),
            "minecraft:sequence" => Ok(Self::Sequence(parse_entries(object)?)),
            "minecraft:group" => Ok(Self::Group(parse_entries(object)?)),
            "minecraft:dynamic" => Ok(Self::Dynamic {
                name: string_field(object, "name")?.to_owned(),
            }),
            "minecraft:loot_table" => {
                let value = required(object, "value")?;
                if value.is_string() {
                    Ok(Self::NestedLootTable {
                        table: Some(string_value(value)?.to_owned()),
                        inline: None,
                    })
                } else {
                    Ok(Self::NestedLootTable {
                        table: None,
                        inline: Some(Box::new(LootTable::from_json(None, value)?)),
                    })
                }
            }
            _ => Ok(Self::Opaque {
                kind: kind.to_owned(),
                value: Value::Object(object.clone()),
            }),
        }
    }
}

fn parse_entries(object: &Map<String, Value>) -> Result<Vec<LootEntry>, LootError> {
    object
        .get("children")
        .and_then(Value::as_array)
        .map(|children| children.iter().map(LootEntry::from_json).collect())
        .unwrap_or_else(|| Ok(Vec::new()))
}

// ---------------------------------------------------------------------------
// Conditions
// ---------------------------------------------------------------------------

/// A vanilla loot condition. Predicates this port cannot evaluate are kept as
/// `Opaque` and fail closed while being recorded in [`LootReport`].
#[derive(Debug, Clone, PartialEq)]
pub enum LootCondition {
    SurvivesExplosion,
    RandomChance(NumberProvider),
    RandomChanceWithEnchantedBonus {
        unenchanted_chance: f32,
        enchanted_chance: LevelBasedValue,
        enchantment: String,
    },
    AnyOf(Vec<LootCondition>),
    AllOf(Vec<LootCondition>),
    Inverted(Box<LootCondition>),
    KilledByPlayer,
    TableBonus {
        enchantment: String,
        chances: Vec<f32>,
    },
    MatchTool {
        predicate: Option<ItemPredicate>,
    },
    BlockStateProperty {
        block: String,
        properties: BTreeMap<String, String>,
    },
    EntityProperties {
        entity: String,
        predicate: Value,
    },
    LocationCheck {
        predicate: Value,
        offset: Option<Value>,
    },
    DamageSourceProperties {
        predicate: Value,
    },
    Opaque {
        kind: String,
        value: Value,
    },
}

impl LootCondition {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let kind = string_field(object, "condition")?;
        match kind {
            "minecraft:survives_explosion" => Ok(Self::SurvivesExplosion),
            "minecraft:random_chance" => Ok(Self::RandomChance(NumberProvider::from_json(
                required(object, "chance")?,
            )?)),
            "minecraft:random_chance_with_enchanted_bonus" => {
                Ok(Self::RandomChanceWithEnchantedBonus {
                    unenchanted_chance: number_field(object, "unenchanted_chance")? as f32,
                    enchanted_chance: LevelBasedValue::from_json(required(
                        object,
                        "enchanted_chance",
                    )?)?,
                    enchantment: string_field(object, "enchantment")?.to_owned(),
                })
            }
            "minecraft:any_of" => Ok(Self::AnyOf(parse_conditions(object, "terms")?)),
            "minecraft:all_of" => Ok(Self::AllOf(parse_conditions(object, "terms")?)),
            "minecraft:inverted" => Ok(Self::Inverted(Box::new(LootCondition::from_json(
                required(object, "term")?,
            )?))),
            "minecraft:killed_by_player" => Ok(Self::KilledByPlayer),
            "minecraft:table_bonus" => Ok(Self::TableBonus {
                enchantment: string_field(object, "enchantment")?.to_owned(),
                chances: required(object, "chances")?
                    .as_array()
                    .ok_or_else(|| LootError::new("table_bonus chances must be an array"))?
                    .iter()
                    .map(number_value)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|chance| chance as f32)
                    .collect(),
            }),
            "minecraft:match_tool" => Ok(Self::MatchTool {
                predicate: object
                    .get("predicate")
                    .map(ItemPredicate::from_json)
                    .transpose()?,
            }),
            "minecraft:block_state_property" => {
                let block = string_field(object, "block")?.to_owned();
                match object.get("properties") {
                    Some(properties) => {
                        let properties_object = as_object(properties)?;
                        let mut parsed = BTreeMap::new();
                        for (name, value) in properties_object {
                            let Some(value) = value.as_str() else {
                                return Ok(Self::Opaque {
                                    kind: kind.to_owned(),
                                    value: Value::Object(object.clone()),
                                });
                            };
                            parsed.insert(name.clone(), value.to_owned());
                        }
                        Ok(Self::BlockStateProperty {
                            block,
                            properties: parsed,
                        })
                    }
                    None => Ok(Self::BlockStateProperty {
                        block,
                        properties: BTreeMap::new(),
                    }),
                }
            }
            "minecraft:entity_properties" => Ok(Self::EntityProperties {
                entity: string_field(object, "entity")?.to_owned(),
                predicate: object.get("predicate").cloned().unwrap_or(Value::Null),
            }),
            "minecraft:location_check" => Ok(Self::LocationCheck {
                predicate: required(object, "predicate")?.clone(),
                offset: object.get("offset").cloned(),
            }),
            "minecraft:damage_source_properties" => Ok(Self::DamageSourceProperties {
                predicate: required(object, "predicate")?.clone(),
            }),
            _ => Ok(Self::Opaque {
                kind: kind.to_owned(),
                value: Value::Object(object.clone()),
            }),
        }
    }

    /// Evaluates the condition. Unsupported predicates return `false` and are
    /// recorded, so a caller never mistakes a skipped condition for a match.
    pub fn test(
        &self,
        context: &LootContext<'_>,
        random: &mut dyn RandomSource,
        report: &mut LootReport,
    ) -> bool {
        match self {
            LootCondition::SurvivesExplosion => match context.explosion_radius {
                Some(radius) => random.next_float() < 1.0 / radius,
                None => true,
            },
            LootCondition::RandomChance(chance) => {
                random.next_float() < chance.sample_float(context, random)
            }
            LootCondition::RandomChanceWithEnchantedBonus {
                unenchanted_chance,
                enchanted_chance,
                enchantment: _,
            } => {
                let chance = if context.looting_level > 0 {
                    enchanted_chance.calculate(context.looting_level)
                } else {
                    *unenchanted_chance
                };
                random.next_float() < chance
            }
            LootCondition::AnyOf(terms) => {
                terms.iter().any(|term| term.test(context, random, report))
            }
            LootCondition::AllOf(terms) => {
                terms.iter().all(|term| term.test(context, random, report))
            }
            LootCondition::Inverted(term) => !term.test(context, random, report),
            LootCondition::KilledByPlayer => context.killed_by_player,
            LootCondition::TableBonus {
                enchantment,
                chances,
            } => {
                let level = context.effective_enchantment_level(enchantment);
                let chance = chances
                    .get(usize::try_from(level).unwrap_or(usize::MAX))
                    .or_else(|| chances.last())
                    .copied()
                    .unwrap_or(0.0);
                random.next_float() < chance
            }
            LootCondition::MatchTool { predicate } => {
                let matches_stack = context.tool.is_some_and(|tool| {
                    predicate
                        .as_ref()
                        .is_none_or(|predicate| predicate.matches(tool))
                });
                if matches_stack {
                    true
                } else {
                    match context.tool_context {
                        Some(tool_context) => predicate
                            .as_ref()
                            .is_none_or(|predicate| predicate.matches_tool_context(tool_context)),
                        None => false,
                    }
                }
            }
            LootCondition::BlockStateProperty { block, properties } => {
                let Some(state) = context.block_state else {
                    return false;
                };
                normalized(state.block) == normalized(block)
                    && properties
                        .iter()
                        .all(|(name, value)| state.properties.get(name) == Some(value))
            }
            LootCondition::EntityProperties { .. }
            | LootCondition::LocationCheck { .. }
            | LootCondition::DamageSourceProperties { .. } => {
                report.unsupported_conditions.insert(self.kind().to_owned());
                false
            }
            LootCondition::Opaque { kind, .. } => {
                report.unsupported_conditions.insert(kind.clone());
                false
            }
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            LootCondition::SurvivesExplosion => "minecraft:survives_explosion",
            LootCondition::RandomChance(_) => "minecraft:random_chance",
            LootCondition::RandomChanceWithEnchantedBonus { .. } => {
                "minecraft:random_chance_with_enchanted_bonus"
            }
            LootCondition::AnyOf(_) => "minecraft:any_of",
            LootCondition::AllOf(_) => "minecraft:all_of",
            LootCondition::Inverted(_) => "minecraft:inverted",
            LootCondition::KilledByPlayer => "minecraft:killed_by_player",
            LootCondition::TableBonus { .. } => "minecraft:table_bonus",
            LootCondition::MatchTool { .. } => "minecraft:match_tool",
            LootCondition::BlockStateProperty { .. } => "minecraft:block_state_property",
            LootCondition::EntityProperties { .. } => "minecraft:entity_properties",
            LootCondition::LocationCheck { .. } => "minecraft:location_check",
            LootCondition::DamageSourceProperties { .. } => "minecraft:damage_source_properties",
            LootCondition::Opaque { kind, .. } => kind,
        }
    }
}

fn parse_conditions(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Vec<LootCondition>, LootError> {
    optional_array_field(object, key)?
        .iter()
        .map(LootCondition::from_json)
        .collect()
}

/// `ItemPredicate` subset used by `match_tool`: item/tag sets, stack counts,
/// and enchantment predicates. Unknown component matchers fail closed.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPredicate {
    pub items: Option<Vec<String>>,
    pub count_min: Option<i32>,
    pub count_max: Option<i32>,
    pub enchantments: Vec<EnchantmentPredicate>,
    pub opaque_components: Vec<Value>,
}

impl ItemPredicate {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let items = match object.get("items") {
            Some(items) if items.is_string() => Some(vec![string_value(items)?.to_owned()]),
            Some(items) => Some(
                items
                    .as_array()
                    .ok_or_else(|| {
                        LootError::new("item predicate `items` must be a string or array")
                    })?
                    .iter()
                    .map(|item| string_value(item).map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        let (count_min, count_max) = match object.get("count") {
            Some(count) => {
                let bounds = as_object(count)?;
                (
                    bounds.get("min").map(i32_value).transpose()?,
                    bounds.get("max").map(i32_value).transpose()?,
                )
            }
            None => (None, None),
        };
        let mut enchantments = Vec::new();
        let mut opaque_components = Vec::new();
        if let Some(predicates) = object.get("predicates") {
            let predicates = as_object(predicates)?;
            for (component, predicate) in predicates {
                if component == "minecraft:enchantments" {
                    let entries = predicate
                        .as_array()
                        .ok_or_else(|| LootError::new("enchantment predicate must be an array"))?;
                    for entry in entries {
                        enchantments.push(EnchantmentPredicate::from_json(entry)?);
                    }
                } else {
                    opaque_components.push(predicate.clone());
                }
            }
        }
        if let Some(components) = object.get("components") {
            opaque_components.push(components.clone());
        }
        Ok(Self {
            items,
            count_min,
            count_max,
            enchantments,
            opaque_components,
        })
    }

    pub fn matches(&self, stack: &ItemStack) -> bool {
        if !self.opaque_components.is_empty() {
            return false;
        }
        if let Some(items) = &self.items
            && !items
                .iter()
                .any(|item| item_selector_matches(item, &stack.item))
        {
            return false;
        }
        if let Some(min) = self.count_min
            && stack.count < min
        {
            return false;
        }
        if let Some(max) = self.count_max
            && stack.count > max
        {
            return false;
        }
        self.enchantments.iter().all(|predicate| {
            if let Some(tag) = predicate.enchantment.strip_prefix('#') {
                return enchantment_map(stack).is_some_and(|map| {
                    map.iter().any(|(enchantment, level)| {
                        tags::has(TagRegistryKind::Enchantment, tag, enchantment)
                            && *level >= predicate.min
                            && *level <= predicate.max
                    })
                });
            }
            let level = enchantment_map(stack)
                .and_then(|map| map.get(&normalized(&predicate.enchantment)))
                .copied()
                .unwrap_or(0);
            level >= predicate.min && level <= predicate.max
        })
    }

    /// Matches an explicit [`ToolContext`] when no `ItemStack` is available.
    /// Item/count predicates cannot be answered from enchantments alone and
    /// fail closed; enchantment predicates are evaluated against the supplied
    /// Silk Touch/Fortune levels.
    pub fn matches_tool_context(&self, context: ToolContext) -> bool {
        if !self.opaque_components.is_empty()
            || self.items.is_some()
            || self.count_min.is_some()
            || self.count_max.is_some()
        {
            return false;
        }
        self.enchantments.iter().all(|predicate| {
            if let Some(tag) = predicate.enchantment.strip_prefix('#') {
                return tags::members(TagRegistryKind::Enchantment, tag)
                    .iter()
                    .any(|enchantment| predicate.accepts(context.enchantment_level(enchantment)));
            }
            predicate.accepts(context.enchantment_level(&predicate.enchantment))
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnchantmentPredicate {
    pub enchantment: String,
    pub min: i32,
    pub max: i32,
}

impl EnchantmentPredicate {
    fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let enchantment = string_field(object, "enchantments")?.to_owned();
        let (min, max) = match object.get("levels") {
            Some(levels) if levels.is_object() => {
                let levels = as_object(levels)?;
                (
                    levels.get("min").map(i32_value).transpose()?.unwrap_or(1),
                    levels
                        .get("max")
                        .map(i32_value)
                        .transpose()?
                        .unwrap_or(i32::MAX),
                )
            }
            Some(level) => {
                let level = i32_value(level)?;
                (level, level)
            }
            None => (1, i32::MAX),
        };
        Ok(Self {
            enchantment,
            min,
            max,
        })
    }

    fn accepts(&self, level: i32) -> bool {
        level >= self.min && level <= self.max
    }
}

fn item_selector_matches(selector: &str, item: &str) -> bool {
    if let Some(tag) = selector.strip_prefix('#') {
        tags::item_has(tag, item)
    } else {
        normalized(selector) == normalized(item)
    }
}

fn enchantment_map(stack: &ItemStack) -> Option<&BTreeMap<String, i32>> {
    match stack.get_component("minecraft:enchantments") {
        Some(ComponentValue::Enchantments(map)) => Some(map),
        _ => None,
    }
}

fn tool_enchantment_level(tool: &ItemStack, enchantment: &str) -> i32 {
    enchantment_map(tool)
        .and_then(|map| map.get(&normalized(enchantment)))
        .copied()
        .unwrap_or(0)
}

/// Enchantment inputs for a loot roll.
///
/// Vanilla reads Silk Touch and Fortune from the held stack's
/// `minecraft:enchantments` component. Callers that model enchantments outside
/// `item_components` (a future enchantment/attribute registry, or a plugin
/// supplied tool) can pass the two levels explicitly. Explicit values take
/// precedence over the stack component.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolContext {
    pub silk_touch: bool,
    pub fortune: i32,
}

impl ToolContext {
    pub fn new(silk_touch: bool, fortune: i32) -> Self {
        Self {
            silk_touch,
            fortune: fortune.max(0),
        }
    }

    /// Reads the two loot-relevant enchantments from a held stack.
    pub fn from_stack(tool: Option<&ItemStack>) -> Self {
        let Some(tool) = tool else {
            return Self::default();
        };
        Self {
            silk_touch: tool_enchantment_level(tool, "minecraft:silk_touch") > 0,
            fortune: tool_enchantment_level(tool, "minecraft:fortune").max(0),
        }
    }

    pub fn with_silk_touch(mut self, silk_touch: bool) -> Self {
        self.silk_touch = silk_touch;
        self
    }

    pub fn with_fortune(mut self, fortune: i32) -> Self {
        self.fortune = fortune.max(0);
        self
    }

    /// Level of a loot-relevant enchantment. Only Silk Touch and Fortune have
    /// gameplay meaning for block tables, so unknown enchantments are level 0.
    pub fn enchantment_level(&self, enchantment: &str) -> i32 {
        match normalized(enchantment).as_str() {
            "minecraft:silk_touch" => i32::from(self.silk_touch),
            "minecraft:fortune" => self.fortune,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// A loot function together with its optional conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct LootFunction {
    pub conditions: Vec<LootCondition>,
    pub kind: LootFunctionKind,
}

impl LootFunction {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let conditions = optional_array_field(object, "conditions")?
            .iter()
            .map(LootCondition::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            conditions,
            kind: LootFunctionKind::from_json(object)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameTarget {
    CustomName,
    ItemName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StewEffect {
    pub effect: String,
    pub duration: NumberProvider,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BonusFormula {
    OreDrops,
    BinomialWithBonusCount { extra: i32, probability: f32 },
    UniformBonusCount { bonus_multiplier: i32 },
    Opaque(Value),
}

impl BonusFormula {
    fn from_json(object: &Map<String, Value>) -> Result<Self, LootError> {
        let formula = string_field(object, "formula")?;
        let parameters = object.get("parameters").cloned().unwrap_or(Value::Null);
        match formula {
            "minecraft:ore_drops" => Ok(Self::OreDrops),
            "minecraft:binomial_with_bonus_count" => {
                let parameters = as_object(&parameters)?;
                Ok(Self::BinomialWithBonusCount {
                    extra: i32_field(parameters, "extra")?,
                    probability: number_field(parameters, "probability")? as f32,
                })
            }
            "minecraft:uniform_bonus_count" => {
                let parameters = as_object(&parameters)?;
                Ok(Self::UniformBonusCount {
                    bonus_multiplier: i32_field(parameters, "bonusMultiplier")?,
                })
            }
            _ => Ok(Self::Opaque(parameters)),
        }
    }

    pub fn calculate(&self, random: &mut dyn RandomSource, count: i32, level: i32) -> i32 {
        match self {
            BonusFormula::OreDrops => {
                if level > 0 {
                    let extra = random.next_int_bound(level + 2) - 1;
                    count * (extra.max(0) + 1)
                } else {
                    count
                }
            }
            BonusFormula::BinomialWithBonusCount { extra, probability } => {
                let mut count = count;
                for _ in 0..level + extra {
                    if random.next_float() < *probability {
                        count += 1;
                    }
                }
                count
            }
            BonusFormula::UniformBonusCount { bonus_multiplier } => {
                count + random.next_int_bound(bonus_multiplier * level + 1)
            }
            BonusFormula::Opaque(_) => count,
        }
    }
}

/// A typed loot function. All 1.21.11 shipped function types have a variant;
/// ones that need registries or world state are reported when executed.
#[derive(Debug, Clone, PartialEq)]
pub enum LootFunctionKind {
    SetCount {
        count: NumberProvider,
        add: bool,
    },
    SetDamage {
        damage: NumberProvider,
        add: bool,
    },
    EnchantRandomly {
        options: Vec<String>,
        only_compatible: bool,
    },
    EnchantWithLevels {
        levels: NumberProvider,
        options: Vec<String>,
    },
    ApplyBonus {
        enchantment: String,
        formula: BonusFormula,
    },
    ExplosionDecay,
    LimitCount {
        limit: IntRange,
    },
    SetComponents {
        components: BTreeMap<String, Value>,
    },
    SetEnchantments {
        enchantments: BTreeMap<String, NumberProvider>,
        add: bool,
    },
    SetName {
        name: Value,
        target: NameTarget,
    },
    SetPotion {
        id: String,
    },
    SetOminousBottleAmplifier {
        amplifier: NumberProvider,
    },
    SetInstrument {
        options: String,
    },
    SetStewEffect {
        effects: Vec<StewEffect>,
    },
    FurnaceSmelt,
    EnchantedCountIncrease {
        enchantment: String,
        count: NumberProvider,
        limit: i32,
    },
    CopyComponents {
        source: Value,
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
    },
    CopyState {
        block: String,
        properties: Vec<String>,
    },
    ExplorationMap {
        value: Value,
    },
    Opaque {
        kind: String,
        value: Value,
    },
}

impl LootFunctionKind {
    fn from_json(object: &Map<String, Value>) -> Result<Self, LootError> {
        let kind = string_field(object, "function")?;
        match kind {
            "minecraft:set_count" => Ok(Self::SetCount {
                count: NumberProvider::from_json(required(object, "count")?)?,
                add: object.get("add").and_then(Value::as_bool).unwrap_or(false),
            }),
            "minecraft:set_damage" => Ok(Self::SetDamage {
                damage: NumberProvider::from_json(required(object, "damage")?)?,
                add: object.get("add").and_then(Value::as_bool).unwrap_or(false),
            }),
            "minecraft:enchant_randomly" => Ok(Self::EnchantRandomly {
                options: parse_string_list(object.get("options"))?,
                only_compatible: object
                    .get("only_compatible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            }),
            "minecraft:enchant_with_levels" => Ok(Self::EnchantWithLevels {
                levels: NumberProvider::from_json(required(object, "levels")?)?,
                options: parse_string_list(object.get("options"))?,
            }),
            "minecraft:apply_bonus" => Ok(Self::ApplyBonus {
                enchantment: string_field(object, "enchantment")?.to_owned(),
                formula: BonusFormula::from_json(object)?,
            }),
            "minecraft:explosion_decay" => Ok(Self::ExplosionDecay),
            "minecraft:limit_count" => Ok(Self::LimitCount {
                limit: IntRange::from_json(required(object, "limit")?)?,
            }),
            "minecraft:set_components" => Ok(Self::SetComponents {
                components: object_field(object, "components")?
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            }),
            "minecraft:set_enchantments" => {
                let enchantments = object_field(object, "enchantments")?
                    .iter()
                    .map(|(name, value)| {
                        NumberProvider::from_json(value).map(|provider| (name.clone(), provider))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;
                Ok(Self::SetEnchantments {
                    enchantments,
                    add: object.get("add").and_then(Value::as_bool).unwrap_or(false),
                })
            }
            "minecraft:set_name" => Ok(Self::SetName {
                name: object.get("name").cloned().unwrap_or(Value::Null),
                target: match object.get("target").and_then(Value::as_str) {
                    Some("item_name") => NameTarget::ItemName,
                    _ => NameTarget::CustomName,
                },
            }),
            "minecraft:set_potion" => Ok(Self::SetPotion {
                id: string_field(object, "id")?.to_owned(),
            }),
            "minecraft:set_ominous_bottle_amplifier" => Ok(Self::SetOminousBottleAmplifier {
                amplifier: NumberProvider::from_json(required(object, "amplifier")?)?,
            }),
            "minecraft:set_instrument" => Ok(Self::SetInstrument {
                options: string_field(object, "options")?.to_owned(),
            }),
            "minecraft:set_stew_effect" => Ok(Self::SetStewEffect {
                effects: optional_array_field(object, "effects")?
                    .iter()
                    .map(|effect| {
                        let effect = as_object(effect)?;
                        Ok(StewEffect {
                            effect: string_field(effect, "type")?.to_owned(),
                            duration: NumberProvider::from_json(required(effect, "duration")?)?,
                        })
                    })
                    .collect::<Result<Vec<_>, LootError>>()?,
            }),
            "minecraft:furnace_smelt" => Ok(Self::FurnaceSmelt),
            "minecraft:enchanted_count_increase" => Ok(Self::EnchantedCountIncrease {
                enchantment: string_field(object, "enchantment")?.to_owned(),
                count: NumberProvider::from_json(required(object, "count")?)?,
                limit: object.get("limit").map(i32_value).transpose()?.unwrap_or(0),
            }),
            "minecraft:copy_components" => Ok(Self::CopyComponents {
                source: required(object, "source")?.clone(),
                include: optional_string_list(object.get("include"))?,
                exclude: optional_string_list(object.get("exclude"))?,
            }),
            "minecraft:copy_state" => Ok(Self::CopyState {
                block: string_field(object, "block")?.to_owned(),
                properties: parse_string_list(object.get("properties"))?,
            }),
            "minecraft:exploration_map" => Ok(Self::ExplorationMap {
                value: Value::Object(object.clone()),
            }),
            _ => Ok(Self::Opaque {
                kind: kind.to_owned(),
                value: Value::Object(object.clone()),
            }),
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            LootFunctionKind::SetCount { .. } => "minecraft:set_count",
            LootFunctionKind::SetDamage { .. } => "minecraft:set_damage",
            LootFunctionKind::EnchantRandomly { .. } => "minecraft:enchant_randomly",
            LootFunctionKind::EnchantWithLevels { .. } => "minecraft:enchant_with_levels",
            LootFunctionKind::ApplyBonus { .. } => "minecraft:apply_bonus",
            LootFunctionKind::ExplosionDecay => "minecraft:explosion_decay",
            LootFunctionKind::LimitCount { .. } => "minecraft:limit_count",
            LootFunctionKind::SetComponents { .. } => "minecraft:set_components",
            LootFunctionKind::SetEnchantments { .. } => "minecraft:set_enchantments",
            LootFunctionKind::SetName { .. } => "minecraft:set_name",
            LootFunctionKind::SetPotion { .. } => "minecraft:set_potion",
            LootFunctionKind::SetOminousBottleAmplifier { .. } => {
                "minecraft:set_ominous_bottle_amplifier"
            }
            LootFunctionKind::SetInstrument { .. } => "minecraft:set_instrument",
            LootFunctionKind::SetStewEffect { .. } => "minecraft:set_stew_effect",
            LootFunctionKind::FurnaceSmelt => "minecraft:furnace_smelt",
            LootFunctionKind::EnchantedCountIncrease { .. } => "minecraft:enchanted_count_increase",
            LootFunctionKind::CopyComponents { .. } => "minecraft:copy_components",
            LootFunctionKind::CopyState { .. } => "minecraft:copy_state",
            LootFunctionKind::ExplorationMap { .. } => "minecraft:exploration_map",
            LootFunctionKind::Opaque { kind, .. } => kind,
        }
    }

    /// Executes the function. Unsupported functions are recorded and leave the
    /// stack unchanged.
    pub fn apply(
        &self,
        stack: &mut ItemStack,
        context: &LootContext<'_>,
        random: &mut dyn RandomSource,
        report: &mut LootReport,
    ) {
        match self {
            LootFunctionKind::SetCount { count, add } => {
                let value = count.sample_int(context, random);
                stack.count = if *add { stack.count + value } else { value };
            }
            LootFunctionKind::SetDamage { damage, add } => {
                let max_damage = stack.max_damage();
                if max_damage > 0 {
                    let existing = if *add {
                        1.0 - stack.damage() as f32 / max_damage as f32
                    } else {
                        0.0
                    };
                    let ratio =
                        1.0 - (damage.sample_float(context, random) + existing).clamp(0.0, 1.0);
                    stack.set_component(ComponentValue::Damage(
                        (ratio * max_damage as f32).floor() as i32,
                    ));
                } else {
                    report
                        .unsupported_functions
                        .insert("minecraft:set_damage (non-damageable item)".to_owned());
                }
            }
            LootFunctionKind::EnchantRandomly {
                options,
                only_compatible: _,
            } => {
                let candidates = expand_options(options);
                if candidates.is_empty() {
                    report
                        .unsupported_functions
                        .insert("minecraft:enchant_randomly (no candidate registry)".to_owned());
                    return;
                }
                let index = random.next_int_bound(candidates.len() as i32) as usize;
                let enchantment = candidates[index].clone();
                let mut map = enchantment_map(stack).cloned().unwrap_or_default();
                map.insert(normalized(&enchantment), 1);
                stack.set_component(ComponentValue::Enchantments(map));
                report
                    .approximations
                    .insert("minecraft:enchant_randomly uses a fixed level 1".to_owned());
            }
            LootFunctionKind::EnchantWithLevels { .. } => {
                report.unsupported_functions.insert(
                    "minecraft:enchant_with_levels (needs enchantment registry)".to_owned(),
                );
            }
            LootFunctionKind::ApplyBonus {
                enchantment,
                formula,
            } => {
                // Java applies the formula whenever a tool parameter exists,
                // even at enchantment level 0; `uniform_bonus_count` still
                // consumes its `nextInt(1)` draw.
                if context.tool.is_some() || context.tool_context.is_some() {
                    let level = context.effective_enchantment_level(enchantment);
                    stack.count = formula.calculate(random, stack.count, level);
                }
            }
            LootFunctionKind::ExplosionDecay => {
                if let Some(radius) = context.explosion_radius {
                    let chance = 1.0 / radius;
                    let mut kept = 0;
                    for _ in 0..stack.count {
                        if random.next_float() <= chance {
                            kept += 1;
                        }
                    }
                    stack.count = kept;
                }
            }
            LootFunctionKind::LimitCount { limit } => {
                stack.count = limit.clamp(stack.count, context, random);
            }
            LootFunctionKind::SetComponents { components } => {
                for (name, value) in components {
                    apply_component_json(stack, name, value);
                }
            }
            LootFunctionKind::SetEnchantments { enchantments, add } => {
                let mut map = enchantment_map(stack).cloned().unwrap_or_default();
                for (name, provider) in enchantments {
                    let level = provider.sample_int(context, random).clamp(0, 255);
                    let key = normalized(name);
                    let value = if *add {
                        map.get(&key).copied().unwrap_or(0) + level
                    } else {
                        level
                    };
                    if value <= 0 {
                        map.remove(&key);
                    } else {
                        map.insert(key, value.clamp(0, 255));
                    }
                }
                if map.is_empty() {
                    stack.remove_component("minecraft:enchantments");
                } else {
                    stack.set_component(ComponentValue::Enchantments(map));
                }
            }
            LootFunctionKind::SetName { name, target } => {
                if !name.is_null() {
                    let tag = json_to_tag(name);
                    stack.set_component(match target {
                        NameTarget::CustomName => ComponentValue::CustomName(tag),
                        NameTarget::ItemName => ComponentValue::ItemName(tag),
                    });
                }
            }
            LootFunctionKind::SetPotion { id } => {
                stack.set_component(ComponentValue::PotionContents(PotionContents::new(id)));
            }
            LootFunctionKind::SetOminousBottleAmplifier { amplifier } => {
                let value = amplifier.sample_int(context, random).clamp(0, 4);
                stack
                    .components
                    .insert_opaque("minecraft:ominous_bottle_amplifier", Tag::Int(value));
            }
            LootFunctionKind::SetInstrument { .. } => {
                report
                    .unsupported_functions
                    .insert("minecraft:set_instrument (needs instrument registry)".to_owned());
            }
            LootFunctionKind::SetStewEffect { .. } => {
                report
                    .unsupported_functions
                    .insert("minecraft:set_stew_effect (needs mob-effect data)".to_owned());
            }
            LootFunctionKind::FurnaceSmelt => {
                if let Some(result) = RecipeCatalog::global().smelting_result(&stack.item) {
                    stack.item = result.item.clone();
                    stack.components = result.components.clone();
                }
            }
            LootFunctionKind::EnchantedCountIncrease {
                enchantment: _,
                count,
                limit,
            } => {
                if context.looting_level > 0 {
                    let increase = (context.looting_level as f32
                        * count.sample_float(context, random))
                    .round() as i32;
                    stack.count += increase;
                    if *limit > 0 {
                        stack.count = stack.count.min(*limit);
                    }
                }
            }
            LootFunctionKind::CopyComponents { .. } => {
                report.unsupported_functions.insert(
                    "minecraft:copy_components (needs context component source)".to_owned(),
                );
            }
            LootFunctionKind::CopyState { .. } => {
                report
                    .unsupported_functions
                    .insert("minecraft:copy_state (needs block-state context)".to_owned());
            }
            LootFunctionKind::ExplorationMap { .. } => {
                report
                    .unsupported_functions
                    .insert("minecraft:exploration_map (needs structure registry)".to_owned());
            }
            LootFunctionKind::Opaque { kind, .. } => {
                report.unsupported_functions.insert(kind.clone());
            }
        }
    }
}

fn expand_options(options: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for option in options {
        if let Some(tag) = option.strip_prefix('#') {
            out.extend(
                tags::members(TagRegistryKind::Enchantment, tag)
                    .iter()
                    .cloned(),
            );
        } else {
            out.push(option.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Context and roll engine
// ---------------------------------------------------------------------------

/// Block state available to `block_state_property`.
#[derive(Debug, Clone, Copy)]
pub struct LootBlockState<'a> {
    pub block: &'a str,
    pub properties: &'a BTreeMap<String, String>,
}

/// Loot context parameters this port can answer. World-backed predicates that
/// are not modelled are reported when a table reaches them.
#[derive(Debug, Clone, Default)]
pub struct LootContext<'a> {
    pub luck: f32,
    pub explosion_radius: Option<f32>,
    pub tool: Option<&'a ItemStack>,
    /// Explicit Silk Touch/Fortune levels. When `None`, they are read from
    /// `tool`'s `minecraft:enchantments` component.
    pub tool_context: Option<ToolContext>,
    pub block_state: Option<LootBlockState<'a>>,
    pub killed_by_player: bool,
    /// Enchantment level of the attacking entity (`looting`).
    pub looting_level: i32,
    /// Enchantment level parameter (`enchantment_level` provider).
    pub enchantment_level: i32,
}

impl<'a> LootContext<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_luck(mut self, luck: f32) -> Self {
        self.luck = luck;
        self
    }

    pub fn with_explosion_radius(mut self, radius: f32) -> Self {
        self.explosion_radius = Some(radius);
        self
    }

    pub fn with_tool(mut self, tool: &'a ItemStack) -> Self {
        self.tool = Some(tool);
        self
    }

    /// Supplies Silk Touch/Fortune explicitly instead of reading them from the
    /// held stack's component patch.
    pub fn with_tool_context(mut self, tool_context: ToolContext) -> Self {
        self.tool_context = Some(tool_context);
        self
    }

    /// Effective enchantment level: explicit [`ToolContext`] first, then the
    /// held stack's component patch.
    pub fn effective_enchantment_level(&self, enchantment: &str) -> i32 {
        match self.tool_context {
            Some(tool_context) => tool_context.enchantment_level(enchantment),
            None => self
                .tool
                .map(|tool| tool_enchantment_level(tool, enchantment))
                .unwrap_or(0),
        }
    }

    pub fn with_block_state(
        mut self,
        block: &'a str,
        properties: &'a BTreeMap<String, String>,
    ) -> Self {
        self.block_state = Some(LootBlockState { block, properties });
        self
    }

    pub fn with_killed_by_player(mut self) -> Self {
        self.killed_by_player = true;
        self
    }

    pub fn with_looting_level(mut self, level: i32) -> Self {
        self.looting_level = level;
        self
    }

    pub fn with_enchantment_level(mut self, level: i32) -> Self {
        self.enchantment_level = level;
        self
    }
}

/// Diagnostics from one roll. Unsupported/opaque payloads are not silently
/// skipped; they are named here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootReport {
    pub unsupported_conditions: BTreeSet<String>,
    pub unsupported_functions: BTreeSet<String>,
    pub unsupported_entries: BTreeSet<String>,
    pub unsupported_number_providers: BTreeSet<String>,
    pub approximations: BTreeSet<String>,
    pub recursion_guard: bool,
}

impl LootReport {
    pub fn is_empty(&self) -> bool {
        self.unsupported_conditions.is_empty()
            && self.unsupported_functions.is_empty()
            && self.unsupported_entries.is_empty()
            && self.unsupported_number_providers.is_empty()
            && self.approximations.is_empty()
            && !self.recursion_guard
    }
}

/// The output of one table roll.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootRoll {
    pub stacks: Vec<ItemStack>,
    pub report: LootReport,
}

/// Rolls a table with a caller-supplied catalog, random source, and context.
///
/// Nested tables resolve through `catalog`; `LootCatalog::roll` is the common
/// entry point.
pub fn roll_table(
    catalog: &LootCatalog,
    table: &LootTable,
    random: &mut dyn RandomSource,
    context: &LootContext<'_>,
) -> LootRoll {
    let mut roller = Roller {
        catalog,
        context,
        random,
        report: LootReport::default(),
        depth: 0,
    };
    let stacks = roller.roll_table(table);
    LootRoll {
        stacks,
        report: roller.report,
    }
}

struct Candidate<'a> {
    weight: i32,
    source: CandidateSource<'a>,
    functions: &'a [LootFunction],
}

enum CandidateSource<'a> {
    Item(&'a str),
    Tag(&'a str),
    Empty,
    Nested {
        table: Option<&'a LootTable>,
        reference: Option<&'a str>,
    },
    Dynamic(&'a str),
}

struct Roller<'a> {
    catalog: &'a LootCatalog,
    context: &'a LootContext<'a>,
    random: &'a mut dyn RandomSource,
    report: LootReport,
    depth: usize,
}

impl<'a> Roller<'a> {
    fn roll_table(&mut self, table: &'a LootTable) -> Vec<ItemStack> {
        if self.depth >= MAX_NESTED_TABLES {
            self.report.recursion_guard = true;
            return Vec::new();
        }
        self.depth += 1;
        let mut stacks = Vec::new();
        for pool in &table.pools {
            self.roll_pool(pool, &mut stacks);
        }
        for function in &table.functions {
            self.apply_function(function, &mut stacks);
        }
        self.depth -= 1;
        stacks
    }

    fn roll_pool(&mut self, pool: &'a LootPool, out: &mut Vec<ItemStack>) {
        if !self.conditions_pass(&pool.conditions) {
            return;
        }
        let rolls = self.number_int(&pool.rolls);
        let bonus = self.number_float(&pool.bonus_rolls);
        let total_rolls = rolls + (bonus * self.context.luck).floor() as i32;
        for _ in 0..total_rolls {
            let mut candidates = Vec::new();
            for entry in &pool.entries {
                self.expand_entry(entry, &mut candidates);
            }
            let total_weight: i32 = candidates
                .iter()
                .filter(|candidate| candidate.weight > 0)
                .map(|candidate| candidate.weight)
                .sum();
            if total_weight <= 0 {
                continue;
            }
            let selected = if candidates
                .iter()
                .filter(|candidate| candidate.weight > 0)
                .count()
                == 1
            {
                candidates
                    .iter()
                    .find(|candidate| candidate.weight > 0)
                    .expect("one positive candidate")
            } else {
                let mut roll = self.random.next_int_bound(total_weight);
                let mut selected = None;
                for candidate in &candidates {
                    if candidate.weight <= 0 {
                        continue;
                    }
                    roll -= candidate.weight;
                    if roll < 0 {
                        selected = Some(candidate);
                        break;
                    }
                }
                let Some(selected) = selected else {
                    continue;
                };
                selected
            };

            let mut produced = self.emit_candidate(selected);
            for function in &pool.functions {
                self.apply_function(function, &mut produced);
            }
            out.extend(produced);
        }
    }

    fn expand_entry(&mut self, entry: &'a LootEntry, out: &mut Vec<Candidate<'a>>) -> bool {
        if !self.conditions_pass(&entry.conditions) {
            return false;
        }
        let weight = entry.weight as f32 + entry.quality as f32 * self.context.luck;
        let weight = weight.floor().max(0.0) as i32;
        match &entry.kind {
            LootEntryKind::Item { name } => {
                out.push(Candidate {
                    weight,
                    source: CandidateSource::Item(name),
                    functions: &entry.functions,
                });
                true
            }
            LootEntryKind::Tag { name, expand } => {
                if *expand {
                    let members = tags::members(TagRegistryKind::Item, name);
                    if members.is_empty() {
                        return false;
                    }
                    for member in members {
                        out.push(Candidate {
                            weight,
                            source: CandidateSource::Item(member),
                            functions: &entry.functions,
                        });
                    }
                    true
                } else {
                    out.push(Candidate {
                        weight,
                        source: CandidateSource::Tag(name),
                        functions: &entry.functions,
                    });
                    true
                }
            }
            LootEntryKind::Empty => {
                out.push(Candidate {
                    weight,
                    source: CandidateSource::Empty,
                    functions: &entry.functions,
                });
                true
            }
            LootEntryKind::Alternatives(children) => {
                for child in children {
                    if self.expand_entry(child, out) {
                        return true;
                    }
                }
                false
            }
            LootEntryKind::Sequence(children) => {
                for child in children {
                    if !self.expand_entry(child, out) {
                        return false;
                    }
                }
                true
            }
            LootEntryKind::Group(children) => {
                for child in children {
                    self.expand_entry(child, out);
                }
                true
            }
            LootEntryKind::NestedLootTable { table, inline } => {
                out.push(Candidate {
                    weight,
                    source: CandidateSource::Nested {
                        table: inline.as_deref(),
                        reference: table.as_deref(),
                    },
                    functions: &entry.functions,
                });
                true
            }
            LootEntryKind::Dynamic { name } => {
                out.push(Candidate {
                    weight,
                    source: CandidateSource::Dynamic(name),
                    functions: &entry.functions,
                });
                true
            }
            LootEntryKind::Opaque { kind, .. } => {
                self.report.unsupported_entries.insert(kind.clone());
                false
            }
        }
    }

    fn emit_candidate(&mut self, candidate: &Candidate<'a>) -> Vec<ItemStack> {
        let mut produced = match &candidate.source {
            CandidateSource::Item(name) => vec![ItemStack::new(*name, 1)],
            CandidateSource::Tag(name) => tags::members(TagRegistryKind::Item, name)
                .iter()
                .map(|member| ItemStack::new(member, 1))
                .collect(),
            CandidateSource::Empty => Vec::new(),
            CandidateSource::Nested { table, reference } => {
                let nested = match (table, reference) {
                    (Some(table), _) => Some(*table),
                    (None, Some(reference)) => self.catalog.table(reference),
                    (None, None) => None,
                };
                if let Some(nested) = nested {
                    self.roll_table(nested)
                } else {
                    if let Some(reference) = reference {
                        self.report
                            .unsupported_entries
                            .insert(format!("missing loot table {reference}"));
                    }
                    Vec::new()
                }
            }
            CandidateSource::Dynamic(name) => {
                self.report
                    .unsupported_entries
                    .insert(format!("minecraft:dynamic {name}"));
                Vec::new()
            }
        };
        for function in candidate.functions {
            self.apply_function(function, &mut produced);
        }
        produced
    }

    fn apply_function(&mut self, function: &LootFunction, stacks: &mut [ItemStack]) {
        if !self.conditions_pass(&function.conditions) {
            return;
        }
        for stack in stacks {
            function
                .kind
                .apply(stack, self.context, self.random, &mut self.report);
        }
    }

    fn conditions_pass(&mut self, conditions: &[LootCondition]) -> bool {
        conditions
            .iter()
            .all(|condition| condition.test(self.context, self.random, &mut self.report))
    }

    fn number_int(&mut self, provider: &NumberProvider) -> i32 {
        if !provider.is_supported() {
            self.report
                .unsupported_number_providers
                .insert(provider.kind().to_owned());
        }
        provider.sample_int(self.context, self.random)
    }

    fn number_float(&mut self, provider: &NumberProvider) -> f32 {
        if !provider.is_supported() {
            self.report
                .unsupported_number_providers
                .insert(provider.kind().to_owned());
        }
        provider.sample_float(self.context, self.random)
    }
}

// ---------------------------------------------------------------------------
// Loot catalog
// ---------------------------------------------------------------------------

/// The embedded loot-table catalog.
#[derive(Debug, Clone)]
pub struct LootCatalog {
    index: AssetIndex,
    tables: BTreeMap<String, LootTable>,
}

static LOOT_CATALOG: OnceLock<LootCatalog> = OnceLock::new();

impl LootCatalog {
    /// Loads the embedded 1.21.11 catalog once.
    pub fn global() -> &'static Self {
        LOOT_CATALOG.get_or_init(|| {
            Self::from_json(LOOT_TABLES_JSON)
                .unwrap_or_else(|error| panic!("embedded loot_tables.json is invalid: {error}"))
        })
    }

    /// Parses a catalog in the generated asset format.
    pub fn from_json(text: &str) -> Result<Self, LootError> {
        let value: Value =
            serde_json::from_str(text).map_err(|error| LootError::new(error.to_string()))?;
        let root = as_object(&value)?;
        let index = AssetIndex::from_json(root)?;
        let tables_value = required(root, "tables")?;
        let tables_object = as_object(tables_value)?;
        let mut tables = BTreeMap::new();
        for (id, table) in tables_object {
            tables.insert(id.clone(), LootTable::from_json(Some(id), table)?);
        }
        if tables.len() != index.total {
            return Err(LootError::new(format!(
                "catalog has {} tables but index reports {}",
                tables.len(),
                index.total
            )));
        }
        Ok(Self { index, tables })
    }

    pub fn index(&self) -> &AssetIndex {
        &self.index
    }

    pub fn table(&self, id: &str) -> Option<&LootTable> {
        self.tables.get(&normalized(id))
    }

    pub fn tables(&self) -> impl Iterator<Item = (&str, &LootTable)> {
        self.tables.iter().map(|(id, table)| (id.as_str(), table))
    }

    /// Rolls one table. Returns `None` when the id is not in the catalog.
    pub fn roll(
        &self,
        id: &str,
        random: &mut dyn RandomSource,
        context: &LootContext<'_>,
    ) -> Option<LootRoll> {
        self.table(id)
            .map(|table| roll_table(self, table, random, context))
    }

    /// Resolves a block id to its shipped block loot table.
    ///
    /// Vanilla stores block tables under `minecraft:blocks/<block path>`, so
    /// `minecraft:stone` resolves to `minecraft:blocks/stone`. The exact table
    /// id is also accepted (`minecraft:blocks/stone`), which lets callers pass
    /// a table id they already resolved. Returns `None` when no table ships.
    pub fn block_table_id(&self, block: &str) -> Option<&str> {
        let block_id = normalized(block);
        if let Some(path) = block_id.strip_prefix("minecraft:") {
            let candidate = format!("minecraft:blocks/{path}");
            if let Some((id, _)) = self.tables.get_key_value(&candidate) {
                return Some(id);
            }
        }
        if let Some((id, _)) = self.tables.get_key_value(&block_id) {
            return Some(id);
        }
        None
    }

    /// Rolls the shipped block table for a block id. Returns `None` when the
    /// block has no shipped table.
    pub fn roll_block(
        &self,
        block: &str,
        random: &mut dyn RandomSource,
        context: &LootContext<'_>,
    ) -> Option<LootRoll> {
        let id = self.block_table_id(block)?;
        self.roll(id, random, context)
    }

    /// Resolves an entity id to its shipped entity loot table.
    ///
    /// Vanilla stores entity tables under `minecraft:entities/<entity path>`,
    /// so `minecraft:cow` resolves to `minecraft:entities/cow`. The exact table
    /// id is also accepted (`minecraft:entities/cow`). Returns `None` when the
    /// table is absent.
    pub fn entity_table_id(&self, entity: &str) -> Option<&str> {
        let entity_id = normalized(entity);
        if let Some(path) = entity_id.strip_prefix("minecraft:") {
            let candidate = format!("minecraft:entities/{path}");
            if let Some((id, _)) = self.tables.get_key_value(&candidate) {
                return Some(id);
            }
        }
        if let Some((id, _)) = self.tables.get_key_value(&entity_id) {
            return Some(id);
        }
        None
    }

    /// Rolls the shipped entity table for an entity id. Returns `None` when the
    /// entity has no shipped table.
    pub fn roll_entity(
        &self,
        entity: &str,
        random: &mut dyn RandomSource,
        context: &LootContext<'_>,
    ) -> Option<LootRoll> {
        let id = self.entity_table_id(entity)?;
        self.roll(id, random, context)
    }
}

// ---------------------------------------------------------------------------
// Recipes
// ---------------------------------------------------------------------------

/// A recipe ingredient: a concrete item, an item tag, or a list of
/// alternatives (the JSON array form).
#[derive(Debug, Clone, PartialEq)]
pub enum Ingredient {
    Item(String),
    Tag(String),
    Alternatives(Vec<Ingredient>),
    Opaque(Value),
}

impl Ingredient {
    pub fn from_json(value: &Value) -> Result<Self, LootError> {
        if let Some(text) = value.as_str() {
            return Ok(match text.strip_prefix('#') {
                Some(tag) => Ingredient::Tag(normalized(tag)),
                None => Ingredient::Item(normalized(text)),
            });
        }
        if let Some(array) = value.as_array() {
            return Ok(Ingredient::Alternatives(
                array
                    .iter()
                    .map(Ingredient::from_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ));
        }
        Ok(Ingredient::Opaque(value.clone()))
    }

    /// Tests an item id against this ingredient.
    pub fn matches(&self, item: &str) -> bool {
        match self {
            Ingredient::Item(expected) => normalized(expected) == normalized(item),
            Ingredient::Tag(tag) => tags::item_has(tag, item),
            Ingredient::Alternatives(alternatives) => alternatives
                .iter()
                .any(|alternative| alternative.matches(item)),
            Ingredient::Opaque(_) => false,
        }
    }

    pub fn is_opaque(&self) -> bool {
        matches!(self, Ingredient::Opaque(_))
    }
}

/// Vanilla recipe categories used by the serializer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeCategory {
    CraftingMisc,
    CraftingBuilding,
    CraftingEquipment,
    CraftingRedstone,
    Cooking,
    Stonecutting,
    Smithing,
    Unknown,
}

impl RecipeCategory {
    pub fn from_id(id: &str) -> Self {
        match id {
            "misc" => Self::CraftingMisc,
            "building" => Self::CraftingBuilding,
            "equipment" => Self::CraftingEquipment,
            "redstone" => Self::CraftingRedstone,
            "food" => Self::Cooking,
            "blocks" => Self::Stonecutting,
            "smithing" => Self::Smithing,
            _ => Self::Unknown,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::CraftingMisc => "misc",
            Self::CraftingBuilding => "building",
            Self::CraftingEquipment => "equipment",
            Self::CraftingRedstone => "redstone",
            Self::Cooking => "food",
            Self::Stonecutting => "blocks",
            Self::Smithing => "smithing",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookingKind {
    Smelting,
    Blasting,
    Smoking,
    Campfire,
}

impl CookingKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Smelting => "minecraft:smelting",
            Self::Blasting => "minecraft:blasting",
            Self::Smoking => "minecraft:smoking",
            Self::Campfire => "minecraft:campfire_cooking",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapedRecipe {
    pub pattern: Vec<String>,
    pub key: BTreeMap<char, Ingredient>,
    pub result: ItemStack,
    pub group: Option<String>,
    pub show_notification: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapelessRecipe {
    pub ingredients: Vec<Ingredient>,
    pub result: ItemStack,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CookingRecipe {
    pub cooking: CookingKind,
    pub ingredient: Ingredient,
    pub result: ItemStack,
    pub experience: f32,
    pub cooking_time: i32,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StonecuttingRecipe {
    pub ingredient: Ingredient,
    pub result: ItemStack,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmithingTransformRecipe {
    pub template: Ingredient,
    pub base: Ingredient,
    pub addition: Ingredient,
    pub result: ItemStack,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmithingTrimRecipe {
    pub template: Ingredient,
    pub base: Ingredient,
    pub addition: Ingredient,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransmuteRecipe {
    pub input: Ingredient,
    pub material: Ingredient,
    pub result: ItemStack,
    pub group: Option<String>,
}

/// A parsed recipe. Special crafting recipes with no data payload stay as
/// explicit `Special` variants; unknown recipe types keep the full JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeKind {
    Shaped(ShapedRecipe),
    Shapeless(ShapelessRecipe),
    Cooking(CookingRecipe),
    Stonecutting(StonecuttingRecipe),
    SmithingTransform(SmithingTransformRecipe),
    SmithingTrim(SmithingTrimRecipe),
    Transmute(TransmuteRecipe),
    DecoratedPot,
    Special { kind: String },
    Opaque { kind: String, value: Value },
}

/// A recipe with the fields common to every serializer.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub category: RecipeCategory,
    pub group: Option<String>,
    pub kind: RecipeKind,
}

impl Recipe {
    pub fn from_json(id: &str, value: &Value) -> Result<Self, LootError> {
        let object = as_object(value)?;
        let kind_id = string_field(object, "type")?;
        let category = object
            .get("category")
            .and_then(Value::as_str)
            .map(RecipeCategory::from_id)
            .unwrap_or(RecipeCategory::Unknown);
        let group = object
            .get("group")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let kind = match kind_id {
            "minecraft:crafting_shaped" => RecipeKind::Shaped(ShapedRecipe {
                pattern: parse_string_list(object.get("pattern"))?,
                key: parse_key(required(object, "key")?)?,
                result: parse_result(required(object, "result")?)?,
                group: group.clone(),
                show_notification: object
                    .get("show_notification")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            }),
            "minecraft:crafting_shapeless" => RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: required(object, "ingredients")?
                    .as_array()
                    .ok_or_else(|| LootError::new("shapeless ingredients must be an array"))?
                    .iter()
                    .map(Ingredient::from_json)
                    .collect::<Result<Vec<_>, _>>()?,
                result: parse_result(required(object, "result")?)?,
                group: group.clone(),
            }),
            "minecraft:smelting"
            | "minecraft:blasting"
            | "minecraft:smoking"
            | "minecraft:campfire_cooking" => {
                let cooking = match kind_id {
                    "minecraft:smelting" => CookingKind::Smelting,
                    "minecraft:blasting" => CookingKind::Blasting,
                    "minecraft:smoking" => CookingKind::Smoking,
                    _ => CookingKind::Campfire,
                };
                RecipeKind::Cooking(CookingRecipe {
                    cooking,
                    ingredient: Ingredient::from_json(required(object, "ingredient")?)?,
                    result: parse_result(required(object, "result")?)?,
                    experience: number_field(object, "experience")? as f32,
                    cooking_time: i32_field(object, "cookingtime")?,
                    group: group.clone(),
                })
            }
            "minecraft:stonecutting" => RecipeKind::Stonecutting(StonecuttingRecipe {
                ingredient: Ingredient::from_json(required(object, "ingredient")?)?,
                result: parse_result(required(object, "result")?)?,
                group: group.clone(),
            }),
            "minecraft:smithing_transform" => {
                RecipeKind::SmithingTransform(SmithingTransformRecipe {
                    template: Ingredient::from_json(required(object, "template")?)?,
                    base: Ingredient::from_json(required(object, "base")?)?,
                    addition: Ingredient::from_json(required(object, "addition")?)?,
                    result: parse_result(required(object, "result")?)?,
                })
            }
            "minecraft:smithing_trim" => RecipeKind::SmithingTrim(SmithingTrimRecipe {
                template: Ingredient::from_json(required(object, "template")?)?,
                base: Ingredient::from_json(required(object, "base")?)?,
                addition: Ingredient::from_json(required(object, "addition")?)?,
                pattern: object
                    .get("pattern")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            "minecraft:crafting_transmute" => RecipeKind::Transmute(TransmuteRecipe {
                input: Ingredient::from_json(required(object, "input")?)?,
                material: Ingredient::from_json(required(object, "material")?)?,
                result: parse_result(required(object, "result")?)?,
                group: group.clone(),
            }),
            "minecraft:crafting_decorated_pot" => RecipeKind::DecoratedPot,
            other if other.starts_with("minecraft:crafting_special_") => RecipeKind::Special {
                kind: other.to_owned(),
            },
            _ => RecipeKind::Opaque {
                kind: kind_id.to_owned(),
                value: Value::Object(object.clone()),
            },
        };
        Ok(Self {
            id: normalized(id),
            category,
            group,
            kind,
        })
    }

    pub fn result(&self) -> Option<&ItemStack> {
        match &self.kind {
            RecipeKind::Shaped(recipe) => Some(&recipe.result),
            RecipeKind::Shapeless(recipe) => Some(&recipe.result),
            RecipeKind::Cooking(recipe) => Some(&recipe.result),
            RecipeKind::Stonecutting(recipe) => Some(&recipe.result),
            RecipeKind::SmithingTransform(recipe) => Some(&recipe.result),
            RecipeKind::Transmute(recipe) => Some(&recipe.result),
            RecipeKind::SmithingTrim(_)
            | RecipeKind::DecoratedPot
            | RecipeKind::Special { .. }
            | RecipeKind::Opaque { .. } => None,
        }
    }
}

fn parse_key(value: &Value) -> Result<BTreeMap<char, Ingredient>, LootError> {
    let object = as_object(value)?;
    let mut key = BTreeMap::new();
    for (symbol, ingredient) in object {
        let mut characters = symbol.chars();
        let Some(character) = characters.next() else {
            return Err(LootError::new("shaped recipe key has an empty symbol"));
        };
        if characters.next().is_some() {
            return Err(LootError::new(format!(
                "shaped recipe key symbol {symbol:?} is not one character"
            )));
        }
        key.insert(character, Ingredient::from_json(ingredient)?);
    }
    Ok(key)
}

fn parse_result(value: &Value) -> Result<ItemStack, LootError> {
    let object = as_object(value)?;
    let id = string_field(object, "id")?;
    let count = object.get("count").map(i32_value).transpose()?.unwrap_or(1);
    let mut stack = ItemStack::new(id, count);
    if let Some(components) = object.get("components") {
        for (name, value) in as_object(components)? {
            apply_component_json(&mut stack, name, value);
        }
    }
    Ok(stack)
}

/// The embedded recipe catalog.
#[derive(Debug, Clone)]
pub struct RecipeCatalog {
    index: AssetIndex,
    recipes: BTreeMap<String, Recipe>,
}

static RECIPE_CATALOG: OnceLock<RecipeCatalog> = OnceLock::new();

impl RecipeCatalog {
    pub fn global() -> &'static Self {
        RECIPE_CATALOG.get_or_init(|| {
            Self::from_json(RECIPES_JSON)
                .unwrap_or_else(|error| panic!("embedded recipes.json is invalid: {error}"))
        })
    }

    pub fn from_json(text: &str) -> Result<Self, LootError> {
        let value: Value =
            serde_json::from_str(text).map_err(|error| LootError::new(error.to_string()))?;
        let root = as_object(&value)?;
        let index = AssetIndex::from_json(root)?;
        let recipes_value = required(root, "recipes")?;
        let recipes_object = as_object(recipes_value)?;
        let mut recipes = BTreeMap::new();
        for (id, recipe) in recipes_object {
            recipes.insert(id.clone(), Recipe::from_json(id, recipe)?);
        }
        if recipes.len() != index.total {
            return Err(LootError::new(format!(
                "catalog has {} recipes but index reports {}",
                recipes.len(),
                index.total
            )));
        }
        Ok(Self { index, recipes })
    }

    pub fn index(&self) -> &AssetIndex {
        &self.index
    }

    pub fn recipe(&self, id: &str) -> Option<&Recipe> {
        self.recipes.get(&normalized(id))
    }

    pub fn recipes(&self) -> impl Iterator<Item = (&str, &Recipe)> {
        self.recipes
            .iter()
            .map(|(id, recipe)| (id.as_str(), recipe))
    }

    /// Returns the first smelting recipe matching an item, used by the
    /// `furnace_smelt` loot function.
    pub fn smelting_result(&self, item: &str) -> Option<&ItemStack> {
        self.recipes.values().find_map(|recipe| match &recipe.kind {
            RecipeKind::Cooking(cooking)
                if cooking.cooking == CookingKind::Smelting && cooking.ingredient.matches(item) =>
            {
                Some(&cooking.result)
            }
            _ => None,
        })
    }
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn as_object(value: &Value) -> Result<&Map<String, Value>, LootError> {
    value
        .as_object()
        .ok_or_else(|| LootError::new("expected a JSON object"))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value, LootError> {
    object
        .get(key)
        .ok_or_else(|| LootError::new(format!("missing required field `{key}`")))
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, LootError> {
    as_object(required(object, key)?)
}

fn object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, LootError> {
    required_object(object, key)
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, LootError> {
    string_value(required(object, key)?)
}

fn string_value(value: &Value) -> Result<&str, LootError> {
    value
        .as_str()
        .ok_or_else(|| LootError::new("expected a JSON string"))
}

fn number_field(object: &Map<String, Value>, key: &str) -> Result<f64, LootError> {
    number_value(required(object, key)?)
}

fn optional_number_field(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, LootError> {
    object.get(key).map(number_value).transpose()
}

fn number_value(value: &Value) -> Result<f64, LootError> {
    value
        .as_f64()
        .ok_or_else(|| LootError::new("expected a JSON number"))
}

fn i32_field(object: &Map<String, Value>, key: &str) -> Result<i32, LootError> {
    i32_value(required(object, key)?)
}

fn i32_value(value: &Value) -> Result<i32, LootError> {
    let number = number_value(value)?;
    if number.fract() != 0.0 || number < i32::MIN as f64 || number > i32::MAX as f64 {
        return Err(LootError::new("expected a 32-bit integer"));
    }
    Ok(number as i32)
}

fn usize_field(object: &Map<String, Value>, key: &str) -> Result<usize, LootError> {
    usize_value(required(object, key)?)
}

fn usize_value(value: &Value) -> Result<usize, LootError> {
    let number = i32_value(value)?;
    usize::try_from(number).map_err(|_| LootError::new("expected a non-negative integer"))
}

fn optional_array_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a [Value], LootError> {
    match object.get(key) {
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| LootError::new(format!("`{key}` must be an array"))),
        None => Ok(&[]),
    }
}

fn parse_string_list(value: Option<&Value>) -> Result<Vec<String>, LootError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) if value.is_string() => Ok(vec![string_value(value)?.to_owned()]),
        Some(value) => value
            .as_array()
            .ok_or_else(|| LootError::new("expected a string or string array"))?
            .iter()
            .map(|entry| string_value(entry).map(str::to_owned))
            .collect(),
    }
}

fn optional_string_list(value: Option<&Value>) -> Result<Option<Vec<String>>, LootError> {
    match value {
        Some(value) => parse_string_list(Some(value)).map(Some),
        None => Ok(None),
    }
}

/// Converts a JSON component/text payload into the NBT shape the item
/// component codecs expect.
pub fn json_to_tag(value: &Value) -> Tag {
    match value {
        Value::Null => Tag::End,
        Value::Bool(value) => Tag::Byte(i8::from(*value)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Tag::Int(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
            } else {
                Tag::Double(number.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(value) => Tag::String(value.clone()),
        Value::Array(values) => Tag::List(values.iter().map(json_to_tag).collect()),
        Value::Object(values) => Tag::Compound(
            values
                .iter()
                .map(|(key, value)| (key.clone(), json_to_tag(value)))
                .collect(),
        ),
    }
}

fn apply_component_json(stack: &mut ItemStack, name: &str, value: &Value) {
    if value.is_null() {
        stack.mark_component_removed(name);
        return;
    }
    let component = ComponentType::named(name);
    let tag = json_to_tag(value);
    match component_from_nbt(&component, &tag) {
        Ok(value) => {
            stack.set_component(value);
        }
        Err(_) => stack.components.insert_opaque(name, tag),
    }
}

fn normalized(value: &str) -> String {
    if value.contains(':') {
        value.to_owned()
    } else {
        format!("minecraft:{value}")
    }
}

/// Java `Math.round(float)` is `floor(value + 0.5)`.
fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

/// Vanilla `Mth.nextInt`: when `minimum >= maximum` it returns `minimum`
/// without consuming RNG.
fn mth_next_int(random: &mut dyn RandomSource, minimum: i32, maximum: i32) -> i32 {
    if minimum >= maximum {
        minimum
    } else {
        random.next_int_between_inclusive(minimum, maximum)
    }
}

/// Vanilla `Mth.nextFloat`: when `minimum >= maximum` it returns `minimum`
/// without consuming RNG.
fn mth_next_float(random: &mut dyn RandomSource, minimum: f32, maximum: f32) -> f32 {
    if minimum >= maximum {
        minimum
    } else {
        random.next_float() * (maximum - minimum) + minimum
    }
}
