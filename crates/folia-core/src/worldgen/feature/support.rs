//! Shared helpers for the newer feature ports.
//!
//! The vegetation batch grew its own state-provider, predicate, and
//! world-adapter helpers. The terrain/cave features need the same building
//! blocks, plus rule-based state providers and float providers, so they live
//! here instead of being copied per module.

use super::placement::{
    BlockPredicate, IntProvider, PlacementModifierKind, PlacementWorld, parse_int_provider,
    parse_modifier,
};
use super::{FeatureLevel, Heightmap, is_air_name};
use crate::chunk::BlockState;
use folia_math::BlockPos;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

/// `BlockState` with no properties.
pub(crate) fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

/// `BlockState` with the given property pairs.
pub(crate) fn state_with(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

pub(crate) fn set_property(state: &mut BlockState, name: &str, value: &str) -> bool {
    if let Some((_, current)) = state.properties.iter_mut().find(|(key, _)| key == name) {
        *current = value.to_owned();
        true
    } else {
        false
    }
}

/// A `LevelReader` view over a [`FeatureLevel`] for predicates and state
/// providers. The biome is deliberately not invented: features that need a
/// biome pass their own sampler (none of the types here do).
pub(crate) struct LevelPlacementWorld<'a> {
    level: &'a dyn FeatureLevel,
}

impl<'a> LevelPlacementWorld<'a> {
    pub(crate) fn new(level: &'a dyn FeatureLevel) -> Self {
        Self { level }
    }
}

impl PlacementWorld for LevelPlacementWorld<'_> {
    fn biome_at(&self, _pos: BlockPos) -> &str {
        ""
    }

    fn biome_has_placed_feature(&self, _biome: &str, _placed_feature: &str) -> bool {
        false
    }

    fn heightmap(&self, kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        self.level.heightmap(kind, x, z)
    }

    fn block_at(&self, pos: BlockPos) -> Option<&BlockState> {
        self.level.get_block(pos)
    }
}

/// Parses a `BlockPredicate` by reusing the placement decoder.
pub(crate) fn parse_block_predicate(value: &Value) -> io::Result<Option<BlockPredicate>> {
    let modifier = parse_modifier(&serde_json::json!({
        "type": "minecraft:block_predicate_filter",
        "predicate": value,
    }))?;
    let PlacementModifierKind::BlockPredicate(predicate) = modifier.kind() else {
        unreachable!("synthetic block_predicate_filter must decode as a predicate")
    };
    Ok(predicate.is_supported().then(|| predicate.clone()))
}

/// Tests a predicate against a feature level at `pos`.
pub(crate) fn test_predicate(
    predicate: &BlockPredicate,
    level: &dyn FeatureLevel,
    pos: BlockPos,
) -> bool {
    let world = LevelPlacementWorld::new(level);
    predicate.test_at(&world, pos, level.min_y(), level.height())
}

/// The block-state providers used by the shipped new features.
#[derive(Debug, Clone)]
pub(crate) enum StateProvider {
    Simple(BlockState),
    Weighted {
        entries: Vec<(i32, BlockState)>,
        total_weight: i32,
    },
    RuleBased {
        fallback: Box<StateProvider>,
        rules: Vec<(BlockPredicate, StateProvider)>,
    },
    Rotated(BlockState),
    Unsupported,
}

impl StateProvider {
    pub(crate) fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("block state provider must be an object"))?;
        let Some(kind) = object.get("type").and_then(Value::as_str) else {
            // `RuleBasedBlockStateProvider` is a record, not a dispatched
            // provider, so `DiskConfiguration` serializes it without `type`.
            if object.contains_key("fallback") && object.contains_key("rules") {
                return Self::parse_rule_based(object);
            }
            return Err(invalid("block state provider is missing `type`"));
        };
        match kind {
            "minecraft:simple_state_provider" => {
                Ok(Self::Simple(parse_state(object.get("state").ok_or_else(
                    || invalid("simple state provider is missing `state`"),
                )?)?))
            }
            "minecraft:weighted_state_provider" => {
                let entries = object
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("weighted state provider is missing `entries`"))?;
                if entries.is_empty() {
                    return Err(invalid("weighted state provider has no entries"));
                }
                let mut parsed = Vec::with_capacity(entries.len());
                let mut total_weight = 0i64;
                for entry in entries {
                    let entry = entry.as_object().ok_or_else(|| {
                        invalid("weighted state provider entry must be an object")
                    })?;
                    let weight = entry.get("weight").and_then(Value::as_i64).ok_or_else(|| {
                        invalid("weighted state provider entry is missing `weight`")
                    })?;
                    if weight <= 0 {
                        return Err(invalid("weighted state provider weights must be positive"));
                    }
                    total_weight += weight;
                    if total_weight > i64::from(i32::MAX) {
                        return Err(invalid("weighted state provider weights exceed i32"));
                    }
                    parsed.push((
                        weight as i32,
                        parse_state(entry.get("data").ok_or_else(|| {
                            invalid("weighted state provider entry is missing `data`")
                        })?)?,
                    ));
                }
                Ok(Self::Weighted {
                    entries: parsed,
                    total_weight: total_weight as i32,
                })
            }
            "minecraft:rule_based_state_provider" => Self::parse_rule_based(object),
            "minecraft:rotated_block_provider" => Ok(Self::Rotated(parse_state(
                object
                    .get("state")
                    .ok_or_else(|| invalid("rotated block provider is missing `state`"))?,
            )?)),
            _ => Ok(Self::Unsupported),
        }
    }

    fn parse_rule_based(object: &Map<String, Value>) -> io::Result<Self> {
        let fallback = object
            .get("fallback")
            .ok_or_else(|| invalid("rule-based state provider is missing `fallback`"))?;
        let parsed_fallback = Self::parse(fallback)?;
        if matches!(parsed_fallback, Self::Unsupported) {
            return Ok(Self::Unsupported);
        }
        let rules = object
            .get("rules")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("rule-based state provider is missing `rules`"))?;
        let mut parsed_rules = Vec::with_capacity(rules.len());
        for rule in rules {
            let rule = rule
                .as_object()
                .ok_or_else(|| invalid("rule-based state provider rule must be an object"))?;
            let Some(predicate) =
                parse_block_predicate(rule.get("if_true").ok_or_else(|| {
                    invalid("rule-based state provider rule is missing `if_true`")
                })?)?
            else {
                return Ok(Self::Unsupported);
            };
            let then = Self::parse(
                rule.get("then")
                    .ok_or_else(|| invalid("rule-based state provider rule is missing `then`"))?,
            )?;
            if matches!(then, Self::Unsupported) {
                return Ok(Self::Unsupported);
            }
            parsed_rules.push((predicate, then));
        }
        Ok(Self::RuleBased {
            fallback: Box::new(parsed_fallback),
            rules: parsed_rules,
        })
    }

    pub(crate) fn is_supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
            && match self {
                Self::RuleBased { fallback, rules } => {
                    fallback.is_supported()
                        && rules.iter().all(|(_, provider)| provider.is_supported())
                }
                _ => true,
            }
    }

    /// `BlockStateProvider.getState(level, random, pos)`.
    pub(crate) fn sample(
        &self,
        level: &dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) -> Option<BlockState> {
        match self {
            Self::Simple(state) => Some(state.clone()),
            Self::Weighted {
                entries,
                total_weight,
            } => {
                let mut roll = random.next_int_bound(*total_weight);
                for (weight, state) in entries {
                    if roll < *weight {
                        return Some(state.clone());
                    }
                    roll -= *weight;
                }
                None
            }
            Self::RuleBased { fallback, rules } => {
                for (predicate, provider) in rules {
                    if test_predicate(predicate, level, pos) {
                        return provider.sample(level, random, pos);
                    }
                }
                fallback.sample(level, random, pos)
            }
            Self::Rotated(state) => Some(rotate_state(state, random)),
            Self::Unsupported => None,
        }
    }
}

/// `RotatedBlockProvider`: picks one of the four horizontal facings.
fn rotate_state(state: &BlockState, random: &mut dyn RandomSource) -> BlockState {
    if !state.properties.iter().any(|(key, _)| key == "facing") {
        return state.clone();
    }
    let mut rotated = state.clone();
    let index = random.next_int_bound(4);
    let facing = ["north", "east", "south", "west"][index as usize];
    let _ = set_property(&mut rotated, "facing", facing);
    rotated
}

/// Parses a `BlockState` JSON object (`{ "Name": ..., "Properties": ... }`).
pub(crate) fn parse_state(value: &Value) -> io::Result<BlockState> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("block state must be an object"))?;
    let name = object
        .get("Name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("block state is missing `Name`"))?;
    let mut properties: Vec<(String, String)> = Vec::new();
    if let Some(values) = object.get("Properties") {
        let values = values
            .as_object()
            .ok_or_else(|| invalid("block state `Properties` must be an object"))?;
        for (key, value) in values {
            let value = value
                .as_str()
                .ok_or_else(|| invalid("block state property value must be a string"))?;
            properties.push((key.clone(), value.to_owned()));
        }
        // Java writes properties in sorted order; the chunk palette keeps the
        // same order so determinism does not depend on JSON insertion order.
        properties.sort_by(|left, right| left.0.cmp(&right.0));
    }
    Ok(BlockState {
        name: name.to_owned(),
        properties,
    })
}

/// A parsed `FloatProvider`.
#[derive(Debug, Clone)]
pub(crate) enum FloatProvider {
    Constant(f32),
    Uniform {
        min: f32,
        max: f32,
    },
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min: f32,
        max: f32,
    },
    Trapezoid {
        min: f32,
        max: f32,
        plateau: f32,
    },
}

impl FloatProvider {
    pub(crate) fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("float provider must be an object"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("float provider is missing `type`"))?;
        match kind {
            "minecraft:constant" => Ok(Self::Constant(float_field(object, "value")?)),
            "minecraft:uniform" => Ok(Self::Uniform {
                min: float_field(object, "min_inclusive")?,
                max: float_field(object, "max_exclusive")?,
            }),
            "minecraft:clamped_normal" => Ok(Self::ClampedNormal {
                mean: float_field(object, "mean")?,
                deviation: float_field(object, "deviation")?,
                min: float_field(object, "min")?,
                max: float_field(object, "max")?,
            }),
            "minecraft:trapezoid" => Ok(Self::Trapezoid {
                min: float_field(object, "min")?,
                max: float_field(object, "max")?,
                plateau: float_field(object, "plateau")?,
            }),
            _ => Err(invalid(format!(
                "float provider {kind} is not supported by the terrain features"
            ))),
        }
    }

    /// `FloatProvider.sample`.
    pub(crate) fn sample(&self, random: &mut dyn RandomSource) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => *min + random.next_float() * (*max - *min),
            Self::ClampedNormal {
                mean,
                deviation,
                min,
                max,
            } => {
                // `ClampedNormalFloat.sample`: a gaussian draw clamped to the
                // inclusive range.
                let value = mean + random.next_gaussian() as f32 * deviation;
                value.clamp(*min, *max)
            }
            Self::Trapezoid { min, max, plateau } => {
                let range = *max - *min;
                if range <= 0.0 {
                    return *min;
                }
                let plateau = plateau.clamp(0.0, range / 2.0);
                loop {
                    let value = *min + random.next_float() * range;
                    if value >= *min + plateau && value <= *max - plateau {
                        return value;
                    }
                    let edge = if value < *min + plateau {
                        value - *min
                    } else {
                        *max - value
                    };
                    if edge >= 0.0 && random.next_float() < edge / plateau.max(f32::EPSILON) {
                        return value;
                    }
                }
            }
        }
    }
}

pub(crate) fn float_field(object: &Map<String, Value>, key: &str) -> io::Result<f32> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .ok_or_else(|| invalid(format!("float provider is missing `{key}`")))
}

/// Integer field helper shared by the new configs.
pub(crate) fn int_field(object: &Map<String, Value>, key: &str) -> io::Result<i32> {
    i32::try_from(
        object
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid(format!("config is missing `{key}`")))?,
    )
    .map_err(|_| invalid(format!("`{key}` does not fit in i32")))
}

/// Boolean field with a default.
pub(crate) fn optional_bool(object: &Map<String, Value>, key: &str, default: bool) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Parses an integer provider and rejects unsupported distributions.
pub(crate) fn parse_int_provider_supported(value: &Value) -> io::Result<IntProvider> {
    let provider = parse_int_provider(value)?;
    if !provider.is_supported() {
        return Err(invalid("int provider distribution is not ported"));
    }
    Ok(provider)
}

/// Air check through the region.
pub(crate) fn is_air(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| is_air_name(&state.name))
}

/// Full-cube collision test through the state metadata.
pub(crate) fn is_solid(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(crate::block_behaviour::is_solid)
}

/// Builds a `minecraft:pointed_dripstone` state.
pub(crate) fn pointed_dripstone(
    tip_direction: &str,
    thickness: &str,
    waterlogged: bool,
) -> BlockState {
    state_with(
        "minecraft:pointed_dripstone",
        &[
            ("thickness", thickness),
            ("tip_direction", tip_direction),
            ("waterlogged", if waterlogged { "true" } else { "false" }),
        ],
    )
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
