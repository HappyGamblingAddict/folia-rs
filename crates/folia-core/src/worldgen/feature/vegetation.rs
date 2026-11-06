//! Ground-cover and vegetation features.
//!
//! Ports the vanilla feature types that make up the ground-cover batch:
//!
//! - `minecraft:random_patch` and `minecraft:flower` (`RandomPatchFeature`)
//! - `minecraft:simple_block` (`SimpleBlockFeature`)
//! - `minecraft:block_pile` (`BlockPileFeature`)
//! - `minecraft:block_column` (`BlockColumnFeature`)
//! - `minecraft:multiface_growth` (`MultifaceGrowthFeature`)
//! - `minecraft:vegetation_patch` and `minecraft:waterlogged_vegetation_patch`
//!
//! The state-provider implementation is independent from the tree module's
//! private provider so the tree path keeps its historical `simple_state_provider`
//! RNG draw. Vanilla's `SimpleStateProvider` does not draw at all; vegetation
//! follows that rule.

use super::placement::{IntProvider, parse_block_state, parse_int_provider};
use super::{
    ConfiguredFeature, FeatureLevel, UnsupportedFeature, is_air_name,
    parse_inline_configured_feature, resource_location,
};
use crate::chunk::BlockState;
use crate::registry;
use crate::tags::{self, TagRegistryKind};
use crate::worldgen::noise::NormalNoise;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io;

/// A parsed vanilla `BlockStateProvider` used by the vegetation features.
#[derive(Debug, Clone)]
pub(crate) enum VegetationStateProvider {
    Simple(BlockState),
    Weighted {
        entries: Vec<(i32, BlockState)>,
        total_weight: i32,
    },
    RandomizedInt {
        source: Box<VegetationStateProvider>,
        property: String,
        values: IntProvider,
    },
    Noise(NoiseProvider),
    NoiseThreshold(NoiseThresholdProvider),
    DualNoise(DualNoiseProvider),
    Rotated(BlockState),
}

#[derive(Debug, Clone)]
pub(crate) struct NoiseSettings {
    seed: i64,
    scale: f32,
    noise: NormalNoise,
}

#[derive(Debug, Clone)]
pub(crate) struct NoiseProvider {
    settings: NoiseSettings,
    states: Vec<BlockState>,
}

#[derive(Debug, Clone)]
pub(crate) struct NoiseThresholdProvider {
    settings: NoiseSettings,
    threshold: f32,
    high_chance: f32,
    default_state: BlockState,
    low_states: Vec<BlockState>,
    high_states: Vec<BlockState>,
}

#[derive(Debug, Clone)]
pub(crate) struct DualNoiseProvider {
    settings: NoiseSettings,
    variety_min: i32,
    variety_max: i32,
    slow_settings: NoiseSettings,
    slow_scale: f32,
    states: Vec<BlockState>,
}

impl VegetationStateProvider {
    pub(crate) fn parse(value: &Value) -> io::Result<Option<Self>> {
        let provider = value
            .as_object()
            .ok_or_else(|| invalid("block state provider must be an object"))?;
        let kind = provider
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("block state provider is missing `type`"))?;
        match kind {
            "minecraft:simple_state_provider" => {
                let state = provider
                    .get("state")
                    .ok_or_else(|| invalid("simple state provider is missing `state`"))?;
                Ok(Some(Self::Simple(parse_block_state(state)?)))
            }
            "minecraft:weighted_state_provider" => {
                let entries = provider
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
                    let data = entry.get("data").ok_or_else(|| {
                        invalid("weighted state provider entry is missing `data`")
                    })?;
                    parsed.push((weight as i32, parse_block_state(data)?));
                }
                Ok(Some(Self::Weighted {
                    entries: parsed,
                    total_weight: total_weight as i32,
                }))
            }
            "minecraft:randomized_int_state_provider" => {
                let source = provider
                    .get("source")
                    .ok_or_else(|| invalid("randomized int state provider is missing `source`"))?;
                let Some(source) = Self::parse(source)? else {
                    return Ok(None);
                };
                let property = provider
                    .get("property")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("randomized int state provider is missing `property`"))?
                    .to_owned();
                let values = parse_int_provider(provider.get("values").ok_or_else(|| {
                    invalid("randomized int state provider is missing `values`")
                })?)?;
                if !values.is_supported() {
                    return Ok(None);
                }
                Ok(Some(Self::RandomizedInt {
                    source: Box::new(source),
                    property,
                    values,
                }))
            }
            "minecraft:noise_provider" => {
                let settings = parse_noise_settings(provider)?;
                let states = parse_state_list(provider, "states")?;
                Ok(Some(Self::Noise(NoiseProvider { settings, states })))
            }
            "minecraft:noise_threshold_provider" => {
                let settings = parse_noise_settings(provider)?;
                let threshold = float_field(provider, "threshold")?;
                let high_chance = float_field(provider, "high_chance")?;
                let default_state =
                    parse_block_state(provider.get("default_state").ok_or_else(|| {
                        invalid("noise threshold provider is missing `default_state`")
                    })?)?;
                let low_states = parse_state_list(provider, "low_states")?;
                let high_states = parse_state_list(provider, "high_states")?;
                Ok(Some(Self::NoiseThreshold(NoiseThresholdProvider {
                    settings,
                    threshold,
                    high_chance,
                    default_state,
                    low_states,
                    high_states,
                })))
            }
            "minecraft:dual_noise_provider" => {
                let settings = parse_noise_settings(provider)?;
                let variety = provider
                    .get("variety")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("dual noise provider is missing `variety`"))?;
                if variety.len() != 2 {
                    return Err(invalid(
                        "dual noise provider `variety` must have two values",
                    ));
                }
                let variety_min = variety[0]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| invalid("dual noise provider `variety` minimum is invalid"))?;
                let variety_max = variety[1]
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| invalid("dual noise provider `variety` maximum is invalid"))?;
                if variety_min > variety_max {
                    return Err(invalid(
                        "dual noise provider variety minimum exceeds maximum",
                    ));
                }
                let slow_noise = provider
                    .get("slow_noise")
                    .ok_or_else(|| invalid("dual noise provider is missing `slow_noise`"))?;
                let slow_scale = float_field(provider, "slow_scale")?;
                let slow_settings = parse_noise_parameters(settings.seed, slow_noise, slow_scale)?;
                let states = parse_state_list(provider, "states")?;
                Ok(Some(Self::DualNoise(DualNoiseProvider {
                    settings,
                    variety_min,
                    variety_max,
                    slow_settings,
                    slow_scale,
                    states,
                })))
            }
            "minecraft:rotated_block_provider" => {
                let state = provider
                    .get("state")
                    .ok_or_else(|| invalid("rotated block provider is missing `state`"))?;
                let parsed = parse_block_state(state)?;
                // Vanilla keeps only the block and samples its default state.
                let base = registry::default_block_state(&parsed.name).unwrap_or(parsed);
                Ok(Some(Self::Rotated(base)))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn is_supported(&self) -> bool {
        match self {
            Self::Simple(_)
            | Self::Weighted { .. }
            | Self::Noise(_)
            | Self::NoiseThreshold(_)
            | Self::DualNoise(_)
            | Self::Rotated(_) => true,
            Self::RandomizedInt { source, values, .. } => {
                source.is_supported() && values.is_supported()
            }
        }
    }

    /// `BlockStateProvider.getState(random, pos)`.
    pub(crate) fn sample(
        &self,
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
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let mut state = source.sample(random, pos)?;
                if !state.properties.iter().any(|(name, _)| name == property) {
                    return Some(state);
                }
                let value = values.sample(random).ok()?;
                if let Some((_, slot)) = state
                    .properties
                    .iter_mut()
                    .find(|(name, _)| name == property)
                {
                    *slot = value.to_string();
                }
                Some(state)
            }
            Self::Noise(provider) => {
                let delta = provider.settings.noise.get_value(
                    f64::from(pos.x()) * f64::from(provider.settings.scale),
                    f64::from(pos.y()) * f64::from(provider.settings.scale),
                    f64::from(pos.z()) * f64::from(provider.settings.scale),
                );
                Some(provider.states[state_index(&provider.states, delta)].clone())
            }
            Self::NoiseThreshold(provider) => {
                let delta = provider.settings.noise.get_value(
                    f64::from(pos.x()) * f64::from(provider.settings.scale),
                    f64::from(pos.y()) * f64::from(provider.settings.scale),
                    f64::from(pos.z()) * f64::from(provider.settings.scale),
                );
                if delta < f64::from(provider.threshold) {
                    let index = random.next_int_bound(provider.low_states.len() as i32) as usize;
                    Some(provider.low_states[index].clone())
                } else if random.next_float() < provider.high_chance {
                    let index = random.next_int_bound(provider.high_states.len() as i32) as usize;
                    Some(provider.high_states[index].clone())
                } else {
                    Some(provider.default_state.clone())
                }
            }
            Self::DualNoise(provider) => {
                let slow = provider.slow_noise_value(pos);
                let variety = clamped_map(
                    slow,
                    -1.0,
                    1.0,
                    f64::from(provider.variety_min),
                    f64::from(provider.variety_max + 1),
                ) as i32;
                let mut candidates = Vec::with_capacity(variety.max(0) as usize);
                for index in 0..variety {
                    let offset = index * 54545;
                    let z_offset = index * 34234;
                    let sample = pos.offset(offset, 0, z_offset);
                    let delta = provider.slow_noise_value(sample);
                    candidates.push(provider.states[state_index(&provider.states, delta)].clone());
                }
                let delta = provider.settings.noise.get_value(
                    f64::from(pos.x()) * f64::from(provider.settings.scale),
                    f64::from(pos.y()) * f64::from(provider.settings.scale),
                    f64::from(pos.z()) * f64::from(provider.settings.scale),
                );
                Some(candidates[state_index(&candidates, delta)].clone())
            }
            Self::Rotated(state) => {
                let axis = random.next_int_bound(3);
                let axis_name = match axis {
                    0 => "x",
                    1 => "y",
                    _ => "z",
                };
                let mut state = state.clone();
                if let Some((_, value)) =
                    state.properties.iter_mut().find(|(name, _)| name == "axis")
                {
                    *value = axis_name.to_owned();
                }
                Some(state)
            }
        }
    }
}

impl DualNoiseProvider {
    fn slow_noise_value(&self, pos: BlockPos) -> f64 {
        self.slow_settings.noise.get_value(
            f64::from(pos.x()) * f64::from(self.slow_scale),
            f64::from(pos.y()) * f64::from(self.slow_scale),
            f64::from(pos.z()) * f64::from(self.slow_scale),
        )
    }
}

fn state_index(states: &[BlockState], delta: f64) -> usize {
    let mapped = ((1.0 + delta) / 2.0).clamp(0.0, 0.999_9);
    ((mapped * states.len() as f64) as usize).min(states.len().saturating_sub(1))
}

fn clamped_map(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let delta = ((value - from_min) / (from_max - from_min)).clamp(0.0, 1.0);
    to_min + delta * (to_max - to_min)
}

fn parse_noise_settings(object: &Map<String, Value>) -> io::Result<NoiseSettings> {
    let seed = object
        .get("seed")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("noise provider is missing an i64 `seed`"))?;
    let noise = object
        .get("noise")
        .ok_or_else(|| invalid("noise provider is missing `noise`"))?;
    let scale = float_field(object, "scale")?;
    parse_noise_parameters(seed, noise, scale)
}

fn parse_noise_parameters(seed: i64, value: &Value, scale: f32) -> io::Result<NoiseSettings> {
    if scale <= 0.0 {
        return Err(invalid("noise provider scale must be positive"));
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("noise parameters must be an object"))?;
    let first_octave = object
        .get("firstOctave")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| invalid("noise parameters are missing an i32 `firstOctave`"))?;
    let amplitudes = object
        .get("amplitudes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("noise parameters are missing `amplitudes`"))?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| invalid("noise amplitude must be a number"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    // Vanilla seeds provider noise through
    // `WorldgenRandom(new LegacyRandomSource(seed))`, so use the legacy
    // positional factory rather than the xoroshiro noise factory.
    let noise = NormalNoise::create_legacy_seed(seed, first_octave, &amplitudes);
    Ok(NoiseSettings { seed, scale, noise })
}

fn float_field(object: &Map<String, Value>, key: &str) -> io::Result<f32> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .ok_or_else(|| invalid(format!("noise provider is missing a numeric `{key}`")))
}

fn parse_state_list(object: &Map<String, Value>, key: &str) -> io::Result<Vec<BlockState>> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("state provider is missing `{key}`")))?;
    if values.is_empty() {
        return Err(invalid(format!("state provider `{key}` must not be empty")));
    }
    values.iter().map(parse_block_state).collect()
}

/// `minecraft:simple_block`.
#[derive(Debug, Clone)]
pub struct SimpleBlockFeature {
    to_place: VegetationStateProvider,
}

impl SimpleBlockFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Some(state) = self.to_place.sample(random, origin) else {
            return false;
        };
        if !can_survive(&state, level, origin) {
            return false;
        }
        if let Some(upper) = double_plant_upper(&state) {
            if !level
                .get_block(origin.relative(Direction::Up))
                .is_none_or(|above| is_air_name(&above.name))
            {
                return false;
            }
            if !level.set_block(origin, state) {
                return false;
            }
            return level.set_block(origin.relative(Direction::Up), upper);
        }
        if state.name == "minecraft:pale_moss_carpet" {
            place_mossy_carpet(level, origin, random);
            return true;
        }
        level.set_block(origin, state)
    }
}

/// `minecraft:block_pile`.
#[derive(Debug, Clone)]
pub struct BlockPileFeature {
    state_provider: VegetationStateProvider,
}

impl BlockPileFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if origin.y() < level.min_y() + 5 {
            return false;
        }
        let radius_x = 2 + random.next_int_bound(2);
        let radius_z = 2 + random.next_int_bound(2);
        for x in origin.x() - radius_x..=origin.x() + radius_x {
            for y in origin.y()..=origin.y() + 1 {
                for z in origin.z() - radius_z..=origin.z() + radius_z {
                    let dx = origin.x() - x;
                    let dz = origin.z() - z;
                    let pos = BlockPos::new(x, y, z);
                    // Java evaluates the left draw before the right draw.
                    let first = random.next_float() * 10.0;
                    let second = random.next_float() * 6.0;
                    let inside = (dx * dx + dz * dz) as f32 <= first - second;
                    let place = inside || random.next_float() < 0.031;
                    if place {
                        self.try_place(level, pos, random);
                    }
                }
            }
        }
        true
    }

    fn try_place(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        random: &mut dyn RandomSource,
    ) {
        let empty = level
            .get_block(pos)
            .is_none_or(|state| is_air_name(&state.name));
        if empty
            && may_place_on(level, pos, random)
            && let Some(state) = self.state_provider.sample(random, pos)
        {
            level.set_block(pos, state);
        }
    }
}

fn may_place_on(level: &dyn FeatureLevel, pos: BlockPos, random: &mut dyn RandomSource) -> bool {
    let Some(below) = level.get_block(pos.below()) else {
        return false;
    };
    if below.name == "minecraft:dirt_path" {
        return random.next_boolean();
    }
    is_face_sturdy(&below.name)
}

/// `minecraft:block_column`.
#[derive(Debug, Clone)]
pub struct BlockColumnFeature {
    layers: Vec<BlockColumnLayer>,
    direction: Direction,
    allowed_placement: VegetationPredicate,
    prioritize_tip: bool,
}

#[derive(Debug, Clone)]
struct BlockColumnLayer {
    height: IntProvider,
    provider: VegetationStateProvider,
}

impl BlockColumnFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut heights = Vec::with_capacity(self.layers.len());
        let mut total = 0;
        for layer in &self.layers {
            let height = layer
                .height
                .sample(random)
                .expect("block column heights are validated while loading");
            heights.push(height);
            total += height;
        }
        if total == 0 {
            return false;
        }
        let mut cursor = origin;
        let mut probe = cursor.relative(self.direction);
        for index in 0..total {
            if !self.allowed_placement.test(level, probe) {
                truncate(&mut heights, total, index, self.prioritize_tip);
                break;
            }
            probe = probe.relative(self.direction);
        }
        for (layer, height) in self.layers.iter().zip(heights) {
            for _ in 0..height {
                if let Some(state) = layer.provider.sample(random, cursor) {
                    level.set_block(cursor, state);
                }
                cursor = cursor.relative(self.direction);
            }
        }
        true
    }
}

fn truncate(heights: &mut [i32], total: i32, current: i32, prioritize_tip: bool) {
    let mut remaining = total - current;
    if prioritize_tip {
        for height in heights.iter_mut() {
            if remaining <= 0 {
                break;
            }
            let remove = (*height).min(remaining);
            *height -= remove;
            remaining -= remove;
        }
    } else {
        for height in heights.iter_mut().rev() {
            if remaining <= 0 {
                break;
            }
            let remove = (*height).min(remaining);
            *height -= remove;
            remaining -= remove;
        }
    }
}

/// `minecraft:multiface_growth`.
#[derive(Debug, Clone)]
pub struct MultifaceGrowthFeature {
    block: BlockState,
    search_range: i32,
    can_place_on_floor: bool,
    can_place_on_ceiling: bool,
    can_place_on_wall: bool,
    chance_of_spreading: f32,
    can_be_placed_on: Vec<BlockMatcher>,
}

impl MultifaceGrowthFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Some(current) = level.get_block(origin).cloned() else {
            return false;
        };
        if !is_air_or_water(&current) {
            return false;
        }
        let directions = self.valid_directions();
        let shuffled = shuffle_copy(&directions, random);
        if self.place_growth_if_possible(level, origin, &current, &shuffled, random) {
            return true;
        }
        for direction in &shuffled {
            let except = shuffle_copy(
                &directions
                    .iter()
                    .copied()
                    .filter(|candidate| candidate != &direction.opposite())
                    .collect::<Vec<_>>(),
                random,
            );
            for _ in 0..self.search_range {
                let pos = origin.relative(*direction);
                let Some(state) = level.get_block(pos).cloned() else {
                    continue;
                };
                if !is_air_or_water(&state) && state.name != self.block.name {
                    break;
                }
                if self.place_growth_if_possible(level, pos, &state, &except, random) {
                    return true;
                }
            }
        }
        false
    }

    fn valid_directions(&self) -> Vec<Direction> {
        let mut directions = Vec::new();
        if self.can_place_on_ceiling {
            directions.push(Direction::Up);
        }
        if self.can_place_on_floor {
            directions.push(Direction::Down);
        }
        if self.can_place_on_wall {
            directions.extend([
                Direction::North,
                Direction::East,
                Direction::South,
                Direction::West,
            ]);
        }
        directions
    }

    fn place_growth_if_possible(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        current: &BlockState,
        directions: &[Direction],
        random: &mut dyn RandomSource,
    ) -> bool {
        for direction in directions {
            let neighbor = pos.relative(*direction);
            let Some(neighbor_state) = level.get_block(neighbor) else {
                continue;
            };
            if !self
                .can_be_placed_on
                .iter()
                .any(|matcher| matcher.matches(&neighbor_state.name))
            {
                continue;
            }
            let Some(state) = multiface_placement_state(
                self.block.name.as_str(),
                current,
                level,
                pos,
                *direction,
            ) else {
                return false;
            };
            if !level.set_block(pos, state.clone()) {
                return false;
            }
            if random.next_float() < self.chance_of_spreading {
                spread_from_face(level, pos, &state, *direction, random);
            }
            return true;
        }
        false
    }
}

fn multiface_placement_state(
    place_block: &str,
    current: &BlockState,
    level: &dyn FeatureLevel,
    pos: BlockPos,
    face: Direction,
) -> Option<BlockState> {
    if has_multiface_face(current, face) {
        return None;
    }
    let neighbor = pos.relative(face);
    let neighbor_state = level.get_block(neighbor)?;
    if !is_face_sturdy(&neighbor_state.name) {
        return None;
    }
    let mut state = if current.name == place_block {
        current.clone()
    } else if current.name == "minecraft:water" {
        let mut state = registry::default_block_state(place_block).unwrap_or_else(|| BlockState {
            name: place_block.to_owned(),
            properties: Vec::new(),
        });
        set_property(&mut state, "waterlogged", "true");
        state
    } else {
        registry::default_block_state(place_block).unwrap_or_else(|| BlockState {
            name: place_block.to_owned(),
            properties: Vec::new(),
        })
    };
    set_property(&mut state, face_name(face), "true");
    Some(state)
}

fn has_multiface_face(state: &BlockState, face: Direction) -> bool {
    state
        .properties
        .iter()
        .any(|(name, value)| name == face_name(face) && value == "true")
}

fn face_name(face: Direction) -> &'static str {
    match face {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

fn spread_from_face(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    state: &BlockState,
    spread_direction: Direction,
    random: &mut dyn RandomSource,
) {
    let faces = shuffle_copy(&Direction::ALL, random);
    for face in faces {
        if face.axis() == spread_direction.axis() {
            continue;
        }
        if !has_multiface_face(state, spread_direction) || has_multiface_face(state, face) {
            continue;
        }
        let candidates = [
            (pos, face),
            (pos.relative(face), spread_direction),
            (
                pos.relative(face).relative(spread_direction),
                face.opposite(),
            ),
        ];
        for (target, target_face) in candidates {
            let Some(target_state) = level.get_block(target) else {
                continue;
            };
            if !spreadable(target_state) {
                continue;
            }
            let Some(placement) =
                multiface_placement_state(&state.name, target_state, level, target, target_face)
            else {
                continue;
            };
            level.set_block(target, placement);
            return;
        }
    }
}

fn spreadable(state: &BlockState) -> bool {
    is_air_name(&state.name) || state.name == "minecraft:water"
}

fn is_air_or_water(state: &BlockState) -> bool {
    is_air_name(&state.name) || state.name == "minecraft:water"
}

/// `minecraft:vegetation_patch` and `minecraft:waterlogged_vegetation_patch`.
#[derive(Debug, Clone)]
pub struct VegetationPatchFeature {
    vanilla_type: String,
    replaceable: String,
    ground_state: VegetationStateProvider,
    vegetation_feature: NestedPlacedFeature,
    surface: CaveSurface,
    depth: IntProvider,
    extra_bottom_block_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    xz_radius: IntProvider,
    extra_edge_column_chance: f32,
    waterlogged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaveSurface {
    Floor,
    Ceiling,
}

impl CaveSurface {
    fn direction(self) -> Direction {
        match self {
            Self::Floor => Direction::Down,
            Self::Ceiling => Direction::Up,
        }
    }
}

impl VegetationPatchFeature {
    pub(crate) fn vanilla_type(&self) -> &str {
        &self.vanilla_type
    }

    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        let x_radius = self
            .xz_radius
            .sample(random)
            .expect("vegetation patch radius is validated while loading")
            + 1;
        let z_radius = self
            .xz_radius
            .sample(random)
            .expect("vegetation patch radius is validated while loading")
            + 1;
        let mut positions = self.place_ground_patch(level, random, origin, x_radius, z_radius);
        if self.waterlogged {
            let mut enclosed = Vec::with_capacity(positions.len());
            for pos in &positions {
                if !is_exposed(level, *pos) {
                    enclosed.push(*pos);
                }
            }
            enclosed = java_hash_order(enclosed);
            for pos in &enclosed {
                level.set_block(
                    *pos,
                    BlockState {
                        name: "minecraft:water".to_owned(),
                        properties: Vec::new(),
                    },
                );
            }
            positions = enclosed;
        }
        self.distribute_vegetation(level, random, &positions)?;
        Ok(!positions.is_empty())
    }

    fn place_ground_patch(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        x_radius: i32,
        z_radius: i32,
    ) -> Vec<BlockPos> {
        let direction = self.surface.direction();
        let opposite = direction.opposite();
        let mut positions = Vec::new();
        for x in -x_radius..=x_radius {
            let edge_x = x == -x_radius || x == x_radius;
            for z in -z_radius..=z_radius {
                let edge_z = z == -z_radius || z == z_radius;
                let edge = edge_x || edge_z;
                let corner = edge_x && edge_z;
                let side = edge && !corner;
                if corner {
                    continue;
                }
                if side {
                    if self.extra_edge_column_chance == 0.0 {
                        continue;
                    }
                    if random.next_float() > self.extra_edge_column_chance {
                        continue;
                    }
                }
                let mut cursor = origin.offset(x, 0, z);
                let mut moved = 0;
                while level
                    .get_block(cursor)
                    .is_none_or(|state| is_air_name(&state.name))
                    && moved < self.vertical_range
                {
                    cursor = cursor.relative(direction);
                    moved += 1;
                }
                let mut moved = 0;
                while level
                    .get_block(cursor)
                    .is_some_and(|state| !is_air_name(&state.name))
                    && moved < self.vertical_range
                {
                    cursor = cursor.relative(opposite);
                    moved += 1;
                }
                let ground = cursor.relative(direction);
                let Some(ground_state) = level.get_block(ground) else {
                    continue;
                };
                let above_is_air = level
                    .get_block(cursor)
                    .is_none_or(|state| is_air_name(&state.name));
                if above_is_air && is_face_sturdy(&ground_state.name) {
                    let mut depth = self
                        .depth
                        .sample(random)
                        .expect("vegetation patch depth is validated while loading");
                    if self.extra_bottom_block_chance > 0.0
                        && random.next_float() < self.extra_bottom_block_chance
                    {
                        depth += 1;
                    }
                    if self.place_ground(level, random, ground, depth) {
                        positions.push(ground);
                    }
                }
            }
        }
        java_hash_order(positions)
    }

    fn place_ground(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        mut pos: BlockPos,
        max_distance: i32,
    ) -> bool {
        for index in 0..max_distance {
            let Some(state) = self.ground_state.sample(random, pos) else {
                return false;
            };
            let Some(current) = level.get_block(pos) else {
                return false;
            };
            if state.name == current.name {
                continue;
            }
            if !is_replaceable(&self.replaceable, current) {
                return index != 0;
            }
            if !level.set_block(pos, state) {
                return false;
            }
            pos = pos.relative(self.surface.direction());
        }
        true
    }

    fn distribute_vegetation(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        positions: &[BlockPos],
    ) -> Result<(), UnsupportedFeature> {
        for pos in positions {
            if self.vegetation_chance > 0.0 && random.next_float() < self.vegetation_chance {
                self.place_vegetation(level, random, *pos)?;
            }
        }
        Ok(())
    }

    fn place_vegetation(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        if self.waterlogged {
            let below = pos.below();
            if self.place_vegetation_at(level, random, below)? {
                if let Some(mut state) = level.get_block(pos).cloned()
                    && let Some((_, value)) = state
                        .properties
                        .iter_mut()
                        .find(|(name, _)| name == "waterlogged")
                    && *value == "false"
                {
                    *value = "true".to_owned();
                    level.set_block(pos, state);
                }
                return Ok(true);
            }
            Ok(false)
        } else {
            self.place_vegetation_at(level, random, pos)
        }
    }

    fn place_vegetation_at(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        self.vegetation_feature.place_all(
            level,
            random,
            pos.relative(self.surface.direction().opposite()),
        )
    }
}

#[derive(Debug, Clone)]
struct NestedPlacedFeature {
    feature: Box<ConfiguredFeature>,
    modifiers: Vec<InlinePlacementModifier>,
}

impl NestedPlacedFeature {
    fn is_supported(&self) -> bool {
        self.feature.is_supported()
            && self
                .modifiers
                .iter()
                .all(InlinePlacementModifier::is_supported)
    }

    fn place_all(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        let positions = self.placement_positions(origin, level);
        let mut any = false;
        for pos in positions {
            if self.feature.place(level, random, pos)? {
                any = true;
            }
        }
        Ok(any)
    }

    fn placement_positions(&self, origin: BlockPos, level: &dyn FeatureLevel) -> Vec<BlockPos> {
        self.apply_modifiers(0, origin, level)
    }

    fn apply_modifiers(
        &self,
        index: usize,
        pos: BlockPos,
        level: &dyn FeatureLevel,
    ) -> Vec<BlockPos> {
        let Some(modifier) = self.modifiers.get(index) else {
            return vec![pos];
        };
        let mut positions = Vec::new();
        for next in modifier.apply(pos, level) {
            positions.extend(self.apply_modifiers(index + 1, next, level));
        }
        positions
    }
}

#[derive(Debug, Clone)]
enum InlinePlacementModifier {
    BlockPredicate(VegetationPredicate),
    Unsupported { vanilla_type: String },
}

impl InlinePlacementModifier {
    fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("inline placement modifier must be an object"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("inline placement modifier is missing `type`"))?;
        match kind {
            "minecraft:block_predicate_filter" => {
                Ok(Self::BlockPredicate(VegetationPredicate::parse(
                    object
                        .get("predicate")
                        .ok_or_else(|| invalid("block predicate filter is missing `predicate`"))?,
                )?))
            }
            _ => Ok(Self::Unsupported {
                vanilla_type: resource_location(kind),
            }),
        }
    }

    fn is_supported(&self) -> bool {
        match self {
            Self::BlockPredicate(predicate) => predicate.is_supported(),
            Self::Unsupported { vanilla_type } => {
                debug_assert!(!vanilla_type.is_empty());
                false
            }
        }
    }

    fn apply(&self, pos: BlockPos, level: &dyn FeatureLevel) -> Vec<BlockPos> {
        match self {
            Self::BlockPredicate(predicate) => {
                if predicate.test(level, pos) {
                    vec![pos]
                } else {
                    Vec::new()
                }
            }
            Self::Unsupported { .. } => Vec::new(),
        }
    }
}

/// The `BlockPredicate` subset used by nested vegetation placements and
/// `block_column.allowed_placement`.
#[derive(Debug, Clone)]
enum VegetationPredicate {
    MatchingBlocks {
        blocks: Vec<BlockMatcher>,
        offset: (i32, i32, i32),
    },
    MatchingFluids {
        fluids: Vec<String>,
        offset: (i32, i32, i32),
    },
    WouldSurvive {
        state: BlockState,
        offset: (i32, i32, i32),
    },
    Replaceable {
        offset: (i32, i32, i32),
    },
    Solid {
        offset: (i32, i32, i32),
    },
    HasSturdyFace {
        offset: (i32, i32, i32),
        direction: Direction,
    },
    AllOf(Vec<VegetationPredicate>),
    AnyOf(Vec<VegetationPredicate>),
    Not(Box<VegetationPredicate>),
    Unsupported {
        vanilla_type: String,
    },
}

impl VegetationPredicate {
    fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("block predicate must be an object"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("block predicate is missing `type`"))?;
        let offset = parse_offset(object)?;
        match kind {
            "minecraft:matching_blocks" => Ok(Self::MatchingBlocks {
                blocks: parse_block_matchers(
                    object
                        .get("blocks")
                        .ok_or_else(|| invalid("matching_blocks is missing `blocks`"))?,
                )?,
                offset,
            }),
            "minecraft:matching_fluids" => Ok(Self::MatchingFluids {
                fluids: parse_string_list(
                    object
                        .get("fluids")
                        .ok_or_else(|| invalid("matching_fluids is missing `fluids`"))?,
                )?,
                offset,
            }),
            "minecraft:would_survive" => Ok(Self::WouldSurvive {
                state: parse_block_state(
                    object
                        .get("state")
                        .ok_or_else(|| invalid("would_survive is missing `state`"))?,
                )?,
                offset,
            }),
            "minecraft:replaceable" => Ok(Self::Replaceable { offset }),
            "minecraft:solid" => Ok(Self::Solid { offset }),
            "minecraft:has_sturdy_face" => Ok(Self::HasSturdyFace {
                offset,
                direction: parse_direction(
                    object
                        .get("direction")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("has_sturdy_face is missing `direction`"))?,
                )?,
            }),
            "minecraft:all_of" | "minecraft:any_of" => {
                let predicates = object
                    .get("predicates")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid(format!("{kind} is missing `predicates`")))?
                    .iter()
                    .map(Self::parse)
                    .collect::<io::Result<Vec<_>>>()?;
                if kind == "minecraft:all_of" {
                    Ok(Self::AllOf(predicates))
                } else {
                    Ok(Self::AnyOf(predicates))
                }
            }
            "minecraft:not" => Ok(Self::Not(Box::new(Self::parse(
                object
                    .get("predicate")
                    .ok_or_else(|| invalid("not is missing `predicate`"))?,
            )?))),
            _ => Ok(Self::Unsupported {
                vanilla_type: resource_location(kind),
            }),
        }
    }

    fn is_supported(&self) -> bool {
        match self {
            Self::AllOf(predicates) | Self::AnyOf(predicates) => {
                predicates.iter().all(Self::is_supported)
            }
            Self::Not(predicate) => predicate.is_supported(),
            Self::Unsupported { vanilla_type } => {
                debug_assert!(!vanilla_type.is_empty());
                false
            }
            _ => true,
        }
    }

    fn test(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        match self {
            Self::MatchingBlocks { blocks, offset } => {
                let pos = apply_offset(pos, *offset);
                match level.get_block(pos) {
                    Some(state) => blocks.iter().any(|matcher| matcher.matches(&state.name)),
                    // An absent section is vanilla air.
                    None => blocks
                        .iter()
                        .any(|matcher| matcher.matches("minecraft:air")),
                }
            }
            Self::MatchingFluids { fluids, offset } => {
                let pos = apply_offset(pos, *offset);
                let fluid = level.get_block(pos).map_or("minecraft:empty", fluid_name);
                fluids.iter().any(|name| {
                    if let Some(tag) = name.strip_prefix('#') {
                        tags::has(TagRegistryKind::Fluid, tag, fluid)
                    } else {
                        name == fluid
                    }
                })
            }
            Self::WouldSurvive { state, offset } => {
                can_survive(state, level, apply_offset(pos, *offset))
            }
            Self::Replaceable { offset } => match level.get_block(apply_offset(pos, *offset)) {
                Some(state) => is_replaceable_state(state),
                None => true,
            },
            Self::Solid { offset } => level
                .get_block(apply_offset(pos, *offset))
                .is_some_and(|state| is_solid_block(&state.name)),
            Self::HasSturdyFace { offset, direction } => {
                let axis = direction.axis();
                debug_assert!(axis.is_vertical() || axis.is_horizontal());
                let pos = apply_offset(pos, *offset);
                level
                    .get_block(pos)
                    .is_some_and(|state| is_face_sturdy(&state.name))
            }
            Self::AllOf(predicates) => predicates
                .iter()
                .all(|predicate| predicate.test(level, pos)),
            Self::AnyOf(predicates) => predicates
                .iter()
                .any(|predicate| predicate.test(level, pos)),
            Self::Not(predicate) => !predicate.test(level, pos),
            Self::Unsupported { .. } => false,
        }
    }
}

#[derive(Debug, Clone)]
enum BlockMatcher {
    Name(String),
    Tag(String),
}

impl BlockMatcher {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Name(expected) => expected == name,
            Self::Tag(tag) => tags::block_has(tag, name),
        }
    }
}

fn parse_block_matchers(value: &Value) -> io::Result<Vec<BlockMatcher>> {
    parse_string_list(value)?
        .into_iter()
        .map(|name| {
            if let Some(tag) = name.strip_prefix('#') {
                Ok(BlockMatcher::Tag(tag.to_owned()))
            } else {
                Ok(BlockMatcher::Name(name))
            }
        })
        .collect()
}

fn parse_string_list(value: &Value) -> io::Result<Vec<String>> {
    match value {
        Value::String(name) => Ok(vec![resource_location(name)]),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(resource_location)
                    .ok_or_else(|| invalid("expected a string or list of strings"))
            })
            .collect(),
        _ => Err(invalid("expected a string or list of strings")),
    }
}

fn parse_offset(object: &Map<String, Value>) -> io::Result<(i32, i32, i32)> {
    let Some(value) = object.get("offset") else {
        return Ok((0, 0, 0));
    };
    let values = value
        .as_array()
        .ok_or_else(|| invalid("predicate `offset` must be an array"))?;
    if values.len() != 3 {
        return Err(invalid("predicate `offset` must have three values"));
    }
    Ok((
        values[0]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| invalid("predicate offset x is invalid"))?,
        values[1]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| invalid("predicate offset y is invalid"))?,
        values[2]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| invalid("predicate offset z is invalid"))?,
    ))
}

fn apply_offset(pos: BlockPos, offset: (i32, i32, i32)) -> BlockPos {
    pos.offset(offset.0, offset.1, offset.2)
}

fn parse_direction(name: &str) -> io::Result<Direction> {
    match name {
        "down" => Ok(Direction::Down),
        "up" => Ok(Direction::Up),
        "north" => Ok(Direction::North),
        "south" => Ok(Direction::South),
        "west" => Ok(Direction::West),
        "east" => Ok(Direction::East),
        _ => Err(invalid(format!("unknown direction `{name}`"))),
    }
}

/// `minecraft:random_patch` and `minecraft:flower`.
#[derive(Debug, Clone)]
pub struct RandomPatchFeature {
    vanilla_type: String,
    tries: i32,
    xz_spread: i32,
    y_spread: i32,
    feature: NestedPlacedFeature,
}

impl RandomPatchFeature {
    pub(crate) fn vanilla_type(&self) -> &str {
        &self.vanilla_type
    }

    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        let xz_bound = self.xz_spread + 1;
        let y_bound = self.y_spread + 1;
        let mut placed = 0;
        for _ in 0..self.tries {
            let dx = random.next_int_bound(xz_bound) - random.next_int_bound(xz_bound);
            let dy = random.next_int_bound(y_bound) - random.next_int_bound(y_bound);
            let dz = random.next_int_bound(xz_bound) - random.next_int_bound(xz_bound);
            let pos = origin.offset(dx, dy, dz);
            if self.feature.place_all(level, random, pos)? {
                placed += 1;
            }
        }
        Ok(placed > 0)
    }
}

pub(crate) fn parse_simple_block_config(config: &Value) -> io::Result<Option<SimpleBlockFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("simple block config must be an object"))?;
    if object
        .get("schedule_tick")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        // Scheduling a tick on a freshly generated block needs the chunk tick
        // queue, which this port does not expose at the generation boundary.
        return Ok(None);
    }
    let Some(to_place) = VegetationStateProvider::parse(
        object
            .get("to_place")
            .ok_or_else(|| invalid("simple block is missing `to_place`"))?,
    )?
    else {
        return Ok(None);
    };
    if !to_place.is_supported() {
        return Ok(None);
    }
    Ok(Some(SimpleBlockFeature { to_place }))
}

pub(crate) fn parse_block_pile_config(config: &Value) -> io::Result<Option<BlockPileFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("block pile config must be an object"))?;
    let Some(state_provider) = VegetationStateProvider::parse(
        object
            .get("state_provider")
            .ok_or_else(|| invalid("block pile is missing `state_provider`"))?,
    )?
    else {
        return Ok(None);
    };
    if !state_provider.is_supported() {
        return Ok(None);
    }
    Ok(Some(BlockPileFeature { state_provider }))
}

pub(crate) fn parse_block_column_config(config: &Value) -> io::Result<Option<BlockColumnFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("block column config must be an object"))?;
    let direction = parse_direction(
        object
            .get("direction")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("block column is missing `direction`"))?,
    )?;
    let allowed_placement = VegetationPredicate::parse(
        object
            .get("allowed_placement")
            .ok_or_else(|| invalid("block column is missing `allowed_placement`"))?,
    )?;
    if !allowed_placement.is_supported() {
        return Ok(None);
    }
    let prioritize_tip = object
        .get("prioritize_tip")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("block column is missing `prioritize_tip`"))?;
    let layers = object
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("block column is missing `layers`"))?;
    if layers.is_empty() {
        return Err(invalid("block column has no layers"));
    }
    let mut parsed_layers = Vec::with_capacity(layers.len());
    for layer in layers {
        let layer = layer
            .as_object()
            .ok_or_else(|| invalid("block column layer must be an object"))?;
        let height = parse_int_provider(
            layer
                .get("height")
                .ok_or_else(|| invalid("block column layer is missing `height`"))?,
        )?;
        if !height.is_supported() || !int_provider_non_negative(&height) {
            return Ok(None);
        }
        let Some(provider) = VegetationStateProvider::parse(
            layer
                .get("provider")
                .ok_or_else(|| invalid("block column layer is missing `provider`"))?,
        )?
        else {
            return Ok(None);
        };
        if !provider.is_supported() {
            return Ok(None);
        }
        parsed_layers.push(BlockColumnLayer { height, provider });
    }
    Ok(Some(BlockColumnFeature {
        layers: parsed_layers,
        direction,
        allowed_placement,
        prioritize_tip,
    }))
}

pub(crate) fn parse_multiface_growth_config(
    config: &Value,
) -> io::Result<Option<MultifaceGrowthFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("multiface growth config must be an object"))?;
    let block_name = resource_location(
        object
            .get("block")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("multiface growth is missing `block`"))?,
    );
    let block = registry::default_block_state(&block_name).unwrap_or(BlockState {
        name: block_name,
        properties: Vec::new(),
    });
    let search_range = object
        .get("search_range")
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .unwrap_or(10);
    if search_range <= 0 {
        return Err(invalid("multiface growth search range must be positive"));
    }
    let can_place_on_floor = object
        .get("can_place_on_floor")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let can_place_on_ceiling = object
        .get("can_place_on_ceiling")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let can_place_on_wall = object
        .get("can_place_on_wall")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let chance_of_spreading = object
        .get("chance_of_spreading")
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(0.5);
    let can_be_placed_on = parse_block_matchers(
        object
            .get("can_be_placed_on")
            .ok_or_else(|| invalid("multiface growth is missing `can_be_placed_on`"))?,
    )?;
    Ok(Some(MultifaceGrowthFeature {
        block,
        search_range,
        can_place_on_floor,
        can_place_on_ceiling,
        can_place_on_wall,
        chance_of_spreading,
        can_be_placed_on,
    }))
}

pub(crate) fn parse_random_patch_config(
    config: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
    id: &str,
    vanilla_type: &str,
) -> io::Result<Option<RandomPatchFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("random patch config must be an object"))?;
    let tries = object
        .get("tries")
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .unwrap_or(128);
    let xz_spread = object
        .get("xz_spread")
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .unwrap_or(7);
    let y_spread = object
        .get("y_spread")
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .unwrap_or(3);
    if tries <= 0 || xz_spread < 0 || y_spread < 0 {
        return Err(invalid("random patch tries/spread values are invalid"));
    }
    let Some(feature) = parse_nested_placed_feature(
        object
            .get("feature")
            .ok_or_else(|| invalid("random patch is missing `feature`"))?,
        configured,
        id,
    )?
    else {
        return Ok(None);
    };
    if !feature.is_supported() {
        return Ok(None);
    }
    Ok(Some(RandomPatchFeature {
        vanilla_type: vanilla_type.to_owned(),
        tries,
        xz_spread,
        y_spread,
        feature,
    }))
}

pub(crate) fn parse_vegetation_patch_config(
    config: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
    id: &str,
    waterlogged: bool,
) -> io::Result<Option<VegetationPatchFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("vegetation patch config must be an object"))?;
    let replaceable = object
        .get("replaceable")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("vegetation patch is missing `replaceable`"))?;
    let replaceable = replaceable
        .strip_prefix('#')
        .ok_or_else(|| invalid("vegetation patch `replaceable` must be a tag"))?
        .to_owned();
    let Some(ground_state) = VegetationStateProvider::parse(
        object
            .get("ground_state")
            .ok_or_else(|| invalid("vegetation patch is missing `ground_state`"))?,
    )?
    else {
        return Ok(None);
    };
    if !ground_state.is_supported() {
        return Ok(None);
    }
    let surface = match object
        .get("surface")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("vegetation patch is missing `surface`"))?
    {
        "floor" => CaveSurface::Floor,
        "ceiling" => CaveSurface::Ceiling,
        other => {
            return Err(invalid(format!(
                "unknown vegetation patch surface `{other}`"
            )));
        }
    };
    let depth = parse_int_provider(
        object
            .get("depth")
            .ok_or_else(|| invalid("vegetation patch is missing `depth`"))?,
    )?;
    if !depth.is_supported() || !int_provider_non_negative(&depth) {
        return Ok(None);
    }
    let xz_radius = parse_int_provider(
        object
            .get("xz_radius")
            .ok_or_else(|| invalid("vegetation patch is missing `xz_radius`"))?,
    )?;
    if !xz_radius.is_supported() || !int_provider_non_negative(&xz_radius) {
        return Ok(None);
    }
    let vertical_range = object
        .get("vertical_range")
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .ok_or_else(|| invalid("vegetation patch is missing `vertical_range`"))?;
    if vertical_range <= 0 {
        return Err(invalid("vegetation patch vertical range must be positive"));
    }
    let extra_bottom_block_chance = float_field(object, "extra_bottom_block_chance")?;
    let vegetation_chance = float_field(object, "vegetation_chance")?;
    let extra_edge_column_chance = float_field(object, "extra_edge_column_chance")?;
    let Some(vegetation_feature) = parse_nested_placed_feature(
        object
            .get("vegetation_feature")
            .ok_or_else(|| invalid("vegetation patch is missing `vegetation_feature`"))?,
        configured,
        id,
    )?
    else {
        return Ok(None);
    };
    if !vegetation_feature.is_supported() {
        return Ok(None);
    }
    Ok(Some(VegetationPatchFeature {
        vanilla_type: if waterlogged {
            "minecraft:waterlogged_vegetation_patch".to_owned()
        } else {
            "minecraft:vegetation_patch".to_owned()
        },
        replaceable,
        ground_state,
        vegetation_feature,
        surface,
        depth,
        extra_bottom_block_chance,
        vertical_range,
        vegetation_chance,
        xz_radius,
        extra_edge_column_chance,
        waterlogged,
    }))
}

fn parse_nested_placed_feature(
    value: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
    id: &str,
) -> io::Result<Option<NestedPlacedFeature>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("nested placed feature must be an object"))?;
    let feature_value = object
        .get("feature")
        .ok_or_else(|| invalid("nested placed feature is missing `feature`"))?;
    let feature = if feature_value.is_string() {
        let reference = resource_location(
            feature_value
                .as_str()
                .expect("checked string feature reference"),
        );
        configured
            .get(&reference)
            .filter(|feature| feature.is_supported())
            .map(|feature| Box::new(feature.clone()))
    } else {
        parse_inline_configured_feature(id, feature_value, configured)?
    };
    let Some(feature) = feature else {
        return Ok(None);
    };
    let placement = object
        .get("placement")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("nested placed feature is missing `placement`"))?;
    let mut modifiers = Vec::with_capacity(placement.len());
    for modifier in placement {
        let modifier = InlinePlacementModifier::parse(modifier)?;
        if !modifier.is_supported() {
            return Ok(None);
        }
        modifiers.push(modifier);
    }
    Ok(Some(NestedPlacedFeature { feature, modifiers }))
}

fn int_provider_non_negative(provider: &IntProvider) -> bool {
    match provider {
        IntProvider::Constant(value) => *value >= 0,
        IntProvider::Uniform {
            min_inclusive,
            max_inclusive,
        } => *min_inclusive >= 0 && *max_inclusive >= 0,
        IntProvider::BiasedToBottom {
            min_inclusive,
            max_inclusive,
        } => *min_inclusive >= 0 && *max_inclusive >= 0,
        IntProvider::Clamped {
            source,
            min_inclusive,
            max_inclusive,
        } => *min_inclusive >= 0 && *max_inclusive >= 0 && int_provider_non_negative(source),
        IntProvider::WeightedList(entries) => entries
            .iter()
            .all(|entry| int_provider_non_negative(&entry.data)),
        IntProvider::ClampedNormal {
            min_inclusive,
            max_inclusive,
            ..
        } => *min_inclusive >= 0 && *max_inclusive >= 0,
        IntProvider::Unsupported { .. } => false,
    }
}

fn shuffle_copy<T: Copy>(values: &[T], random: &mut dyn RandomSource) -> Vec<T> {
    let mut values = values.to_vec();
    for index in (2..=values.len()).rev() {
        let swap = random.next_int_bound(index as i32) as usize;
        values.swap(index - 1, swap);
    }
    values
}

fn can_survive(state: &BlockState, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    let Some(below) = level.get_block(pos.below()) else {
        return false;
    };
    match state.name.as_str() {
        "minecraft:pale_moss_carpet" => {
            let base = property(state, "base") == Some("true");
            if base {
                !is_air_name(&below.name)
            } else {
                below.name == "minecraft:pale_moss_carpet"
                    && property(below, "base") == Some("true")
            }
        }
        "minecraft:moss_carpet" => !is_air_name(&below.name),
        "minecraft:lily_pad" => below.name == "minecraft:water",
        "minecraft:spore_blossom" => {
            level
                .get_block(pos.above())
                .is_some_and(|above| is_face_sturdy(&above.name))
                && !is_water_at(level, pos)
        }
        "minecraft:dead_bush" | "minecraft:short_dry_grass" | "minecraft:tall_dry_grass" => {
            tags::block_has("minecraft:dry_vegetation_may_place_on", &below.name)
        }
        "minecraft:cactus" => {
            let base_ok =
                below.name == "minecraft:cactus" || tags::block_has("minecraft:sand", &below.name);
            if !base_ok {
                return false;
            }
            for face in [
                Direction::North,
                Direction::East,
                Direction::South,
                Direction::West,
            ] {
                if level
                    .get_block(pos.relative(face))
                    .is_some_and(|state| is_solid_block(&state.name))
                {
                    return false;
                }
            }
            level
                .get_block(pos.above())
                .is_none_or(|state| state.name != "minecraft:water")
        }
        "minecraft:sugar_cane" => {
            if below.name == "minecraft:sugar_cane" {
                return true;
            }
            if !(tags::block_has("minecraft:dirt", &below.name)
                || tags::block_has("minecraft:sand", &below.name))
            {
                return false;
            }
            let base = pos.below();
            [
                Direction::North,
                Direction::East,
                Direction::South,
                Direction::West,
            ]
            .into_iter()
            .any(|face| {
                level
                    .get_block(base.relative(face))
                    .is_some_and(|state| state.name == "minecraft:water")
            })
        }
        "minecraft:crimson_roots" | "minecraft:warped_roots" | "minecraft:nether_sprouts" => {
            tags::block_has("minecraft:nylium", &below.name)
                || below.name == "minecraft:soul_soil"
                || may_place_on_dirt(&below.name)
        }
        "minecraft:fire" => is_face_sturdy(&below.name) || below.name == "minecraft:netherrack",
        "minecraft:soul_fire" => below.name == "minecraft:soul_soil",
        _ => may_place_on_dirt(&below.name),
    }
}

fn may_place_on_dirt(name: &str) -> bool {
    tags::block_has("minecraft:dirt", name) || name == "minecraft:farmland"
}

fn is_water_at(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| state.name == "minecraft:water")
}

fn is_replaceable(tag: &str, state: &BlockState) -> bool {
    tags::block_has(tag, &state.name)
}

fn is_replaceable_state(state: &BlockState) -> bool {
    is_air_name(&state.name)
        || state.name == "minecraft:water"
        || state.name == "minecraft:lava"
        || tags::block_has("minecraft:replaceable_by_trees", &state.name)
}

fn is_solid_block(name: &str) -> bool {
    !is_air_name(name)
        && name != "minecraft:water"
        && name != "minecraft:lava"
        && !tags::block_has("minecraft:replaceable_by_trees", name)
}

fn is_face_sturdy(name: &str) -> bool {
    !is_air_name(name)
        && name != "minecraft:water"
        && name != "minecraft:lava"
        && !name.ends_with("_leaves")
        && !tags::block_has("minecraft:replaceable_by_trees", name)
        && !name.ends_with("_sapling")
        && !name.ends_with("_flower")
        && name != "minecraft:short_grass"
        && name != "minecraft:tall_grass"
        && name != "minecraft:fern"
        && name != "minecraft:large_fern"
}

fn fluid_name(state: &BlockState) -> &'static str {
    match state.name.as_str() {
        "minecraft:water" | "minecraft:flowing_water" => "minecraft:water",
        "minecraft:lava" | "minecraft:flowing_lava" => "minecraft:lava",
        _ => {
            if property(state, "waterlogged") == Some("true") {
                "minecraft:water"
            } else {
                "minecraft:empty"
            }
        }
    }
}

fn property<'a>(state: &'a BlockState, name: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn set_property(state: &mut BlockState, name: &str, value: &str) -> bool {
    if let Some((_, slot)) = state.properties.iter_mut().find(|(key, _)| key == name) {
        *slot = value.to_owned();
        true
    } else {
        false
    }
}

fn double_plant_upper(state: &BlockState) -> Option<BlockState> {
    if property(state, "half") != Some("lower") {
        return None;
    }
    let mut upper = state.clone();
    set_property(&mut upper, "half", "upper");
    Some(upper)
}

fn is_exposed(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
        Direction::Down,
    ]
    .into_iter()
    .any(|direction| {
        let neighbor = pos.relative(direction);
        level
            .get_block(neighbor)
            .is_none_or(|state| !is_face_sturdy(&state.name))
    })
}

fn java_hash_order(positions: Vec<BlockPos>) -> Vec<BlockPos> {
    if positions.is_empty() {
        return positions;
    }
    let mut unique = Vec::with_capacity(positions.len());
    let mut seen = std::collections::HashSet::with_capacity(positions.len());
    for pos in positions {
        if seen.insert(pos) {
            unique.push(pos);
        }
    }
    let mut capacity = 16usize;
    while unique.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    let mut ordered: Vec<(usize, usize, BlockPos)> = unique
        .into_iter()
        .enumerate()
        .map(|(index, pos)| {
            let hash = pos
                .y()
                .wrapping_add(pos.z().wrapping_mul(31))
                .wrapping_mul(31)
                .wrapping_add(pos.x());
            let spread = hash ^ ((hash as u32 >> 16) as i32);
            ((spread as u32 as usize) & (capacity - 1), index, pos)
        })
        .collect();
    ordered.sort_by_key(|(bucket, index, _)| (*bucket, *index));
    ordered.into_iter().map(|(_, _, pos)| pos).collect()
}

fn place_mossy_carpet(level: &mut dyn FeatureLevel, pos: BlockPos, random: &mut dyn RandomSource) {
    let Some(default) = registry::default_block_state("minecraft:pale_moss_carpet") else {
        return;
    };
    let updated = mossy_carpet_updated_state(default, level, pos, true);
    level.set_block(pos, updated.clone());
    if let Some(topper) = mossy_carpet_topper(level, pos, random) {
        level.set_block(pos.above(), topper);
        let updated = mossy_carpet_updated_state(updated, level, pos, true);
        level.set_block(pos, updated);
    }
}

fn mossy_carpet_updated_state(
    mut state: BlockState,
    level: &dyn FeatureLevel,
    pos: BlockPos,
    mut tip: bool,
) -> BlockState {
    tip |= property(&state, "base") == Some("true");
    let above = level.get_block(pos.above()).cloned();
    let below = level.get_block(pos.below()).cloned();
    for direction in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ] {
        let name = face_name(direction);
        let can_support = can_attach_to(level, pos, direction);
        let mut side = if can_support {
            if tip {
                "low".to_owned()
            } else {
                property(&state, name).unwrap_or("none").to_owned()
            }
        } else {
            "none".to_owned()
        };
        if side == "low" {
            if let Some(above) = &above
                && above.name == "minecraft:pale_moss_carpet"
                && property(above, name).is_some_and(|value| value != "none")
                && property(above, "base") != Some("true")
            {
                side = "tall".to_owned();
            }
            if property(&state, "base") != Some("true")
                && let Some(below) = &below
                && below.name == "minecraft:pale_moss_carpet"
                && property(below, name) == Some("none")
            {
                side = "none".to_owned();
            }
        }
        set_property(&mut state, name, &side);
    }
    state
}

fn mossy_carpet_topper(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    random: &mut dyn RandomSource,
) -> Option<BlockState> {
    let above_pos = pos.above();
    let above = level.get_block(above_pos).cloned().unwrap_or(BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    });
    let is_pale = above.name == "minecraft:pale_moss_carpet";
    if is_pale && property(&above, "base") == Some("true") {
        return None;
    }
    if !is_pale && !is_replaceable_state(&above) {
        return None;
    }
    let mut state = registry::default_block_state("minecraft:pale_moss_carpet")?;
    set_property(&mut state, "base", "false");
    let mut updated = mossy_carpet_updated_state(state, level, above_pos, true);
    for direction in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ] {
        let name = face_name(direction);
        if property(&updated, name).is_some_and(|value| value != "none") && !random.next_boolean() {
            set_property(&mut updated, name, "none");
        }
    }
    let has_faces = property(&updated, "base") == Some("true")
        || [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ]
        .into_iter()
        .any(|direction| {
            property(&updated, face_name(direction)).is_some_and(|value| value != "none")
        });
    (has_faces && updated != above).then_some(updated)
}

fn can_attach_to(level: &dyn FeatureLevel, pos: BlockPos, direction: Direction) -> bool {
    if direction == Direction::Up {
        return false;
    }
    level
        .get_block(pos.relative(direction))
        .is_some_and(|state| is_face_sturdy(&state.name))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use folia_random::LegacyRandomSource;
    use serde_json::json;

    fn provider(value: Value) -> VegetationStateProvider {
        VegetationStateProvider::parse(&value)
            .expect("provider parses")
            .expect("provider is supported")
    }

    fn pos() -> BlockPos {
        BlockPos::new(8, 64, -3)
    }

    #[test]
    fn simple_provider_does_not_consume_randomness() {
        let provider = provider(json!({
            "type": "minecraft:simple_state_provider",
            "state": {"Name": "minecraft:stone"}
        }));
        let mut draw = LegacyRandomSource::new(42);
        let untouched = draw.next_int();
        let mut sample = LegacyRandomSource::new(42);
        let state = provider.sample(&mut sample, pos()).expect("state");
        assert_eq!(state.name, "minecraft:stone");
        assert_eq!(sample.next_int(), untouched);
    }

    #[test]
    fn weighted_provider_draws_one_bound_in_declaration_order() {
        let provider = provider(json!({
            "type": "minecraft:weighted_state_provider",
            "entries": [
                {"weight": 1, "data": {"Name": "minecraft:poppy"}},
                {"weight": 3, "data": {"Name": "minecraft:dandelion"}}
            ]
        }));
        let mut sample = LegacyRandomSource::new(7);
        let state = provider.sample(&mut sample, pos()).expect("state");
        let mut expected = LegacyRandomSource::new(7);
        let roll = expected.next_int_bound(4);
        let expected_name = if roll < 1 {
            "minecraft:poppy"
        } else {
            "minecraft:dandelion"
        };
        assert_eq!(state.name, expected_name);
        assert_eq!(sample.next_int(), expected.next_int());
    }

    #[test]
    fn randomized_int_provider_samples_source_then_value() {
        let provider = provider(json!({
            "type": "minecraft:randomized_int_state_provider",
            "property": "age",
            "source": {
                "type": "minecraft:weighted_state_provider",
                "entries": [
                    {"weight": 2, "data": {"Name": "minecraft:oak_log", "Properties": {"axis": "y", "age": "0"}}}
                ]
            },
            "values": {"type": "minecraft:uniform", "min_inclusive": 5, "max_inclusive": 7}
        }));
        let mut sample = LegacyRandomSource::new(9);
        let state = provider.sample(&mut sample, pos()).expect("state");
        let mut expected = LegacyRandomSource::new(9);
        let _source_draw = expected.next_int_bound(2);
        let value = expected.next_int_between_inclusive(5, 7);
        assert_eq!(
            state.properties,
            vec![
                ("age".to_owned(), value.to_string()),
                ("axis".to_owned(), "y".to_owned())
            ]
        );
        assert_eq!(sample.next_int(), expected.next_int());
    }

    #[test]
    fn noise_provider_is_position_based_and_draws_nothing() {
        let provider = provider(json!({
            "type": "minecraft:noise_provider",
            "seed": 2345,
            "scale": 0.02,
            "noise": {"firstOctave": 0, "amplitudes": [1.0]},
            "states": [
                {"Name": "minecraft:short_grass"},
                {"Name": "minecraft:fern"}
            ]
        }));
        let mut sample = LegacyRandomSource::new(3);
        let first = provider.sample(&mut sample, pos()).expect("state");
        let second = provider.sample(&mut sample, pos()).expect("state");
        assert_eq!(first, second);
        let mut untouched = LegacyRandomSource::new(3);
        assert_eq!(sample.next_int(), untouched.next_int());
    }

    #[test]
    fn noise_threshold_provider_uses_one_draw_for_the_low_branch() {
        let provider = provider(json!({
            "type": "minecraft:noise_threshold_provider",
            "seed": 2345,
            "scale": 0.005,
            "noise": {"firstOctave": 0, "amplitudes": [1.0]},
            "threshold": 1.0,
            "high_chance": 0.0,
            "default_state": {"Name": "minecraft:dandelion"},
            "low_states": [
                {"Name": "minecraft:poppy"},
                {"Name": "minecraft:cornflower"}
            ],
            "high_states": [{"Name": "minecraft:oxeye_daisy"}]
        }));
        let mut sample = LegacyRandomSource::new(11);
        let state = provider.sample(&mut sample, pos()).expect("state");
        let mut expected = LegacyRandomSource::new(11);
        let index = expected.next_int_bound(2) as usize;
        let expected_name = ["minecraft:poppy", "minecraft:cornflower"][index];
        assert_eq!(state.name, expected_name);
        assert_eq!(sample.next_int(), expected.next_int());
    }

    #[test]
    fn dual_noise_provider_is_position_based_and_draws_nothing() {
        let provider = provider(json!({
            "type": "minecraft:dual_noise_provider",
            "seed": 2345,
            "scale": 1.0,
            "noise": {"firstOctave": -3, "amplitudes": [1.0]},
            "variety": [1, 3],
            "slow_scale": 1.0,
            "slow_noise": {"firstOctave": -10, "amplitudes": [1.0]},
            "states": [
                {"Name": "minecraft:tall_grass", "Properties": {"half": "lower"}},
                {"Name": "minecraft:allium"},
                {"Name": "minecraft:poppy"}
            ]
        }));
        let mut sample = LegacyRandomSource::new(5);
        let first = provider.sample(&mut sample, pos()).expect("state");
        let second = provider.sample(&mut sample, pos()).expect("state");
        assert_eq!(first, second);
        let mut untouched = LegacyRandomSource::new(5);
        assert_eq!(sample.next_int(), untouched.next_int());
    }

    #[test]
    fn rotated_provider_draws_one_axis_value() {
        let provider = provider(json!({
            "type": "minecraft:rotated_block_provider",
            "state": {"Name": "minecraft:hay_block", "Properties": {"axis": "y"}}
        }));
        let mut sample = LegacyRandomSource::new(13);
        let state = provider.sample(&mut sample, pos()).expect("state");
        let mut expected = LegacyRandomSource::new(13);
        let axis = match expected.next_int_bound(3) {
            0 => "x",
            1 => "y",
            _ => "z",
        };
        assert_eq!(state.properties, vec![("axis".to_owned(), axis.to_owned())]);
        assert_eq!(sample.next_int(), expected.next_int());
    }

    #[test]
    fn java_hash_set_order_uses_spread_buckets() {
        let positions = vec![
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 0, 0),
            BlockPos::new(16, 0, 0),
            BlockPos::new(0, 0, 1),
        ];
        let ordered = java_hash_order(positions.clone());
        assert_eq!(ordered.len(), positions.len());
        for pos in positions {
            assert!(ordered.contains(&pos));
        }
    }
}
