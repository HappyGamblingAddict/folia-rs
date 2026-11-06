//! Huge mushrooms, huge fungi, glowstone, block blobs, fill layers, nether
//! forest vegetation, and the boolean selector used by mushroom islands.
//!
//! Ports `HugeBrownMushroomFeature`, `HugeRedMushroomFeature`,
//! `HugeFungusFeature`, `GlowstoneFeature`, `BlockBlobFeature`,
//! `FillLayerFeature`, `NetherForestVegetationFeature`, and
//! `RandomBooleanSelectorFeature`.

use super::aquatic::{
    can_be_replaced, config_object, default_state, int_field, is_empty_block, mth_next_int,
    nether_plant_can_survive, parse_state_field, set_property,
};
use super::vegetation::VegetationStateProvider;
use super::vines::place_weeping_vines_column;
use super::{ConfiguredFeature, FeatureLevel, UnsupportedFeature};
use crate::chunk::BlockState;
use crate::tags;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io;

/// `HugeBrownMushroomFeature` and `HugeRedMushroomFeature`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HugeMushroomKind {
    Brown,
    Red,
}

/// `AbstractHugeMushroomFeature`.
#[derive(Debug, Clone)]
pub struct HugeMushroomFeature {
    kind: HugeMushroomKind,
    cap_provider: VegetationStateProvider,
    stem_provider: VegetationStateProvider,
    foliage_radius: i32,
}

impl HugeMushroomFeature {
    pub(crate) fn vanilla_type(&self) -> &'static str {
        match self.kind {
            HugeMushroomKind::Brown => "minecraft:huge_brown_mushroom",
            HugeMushroomKind::Red => "minecraft:huge_red_mushroom",
        }
    }

    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut tree_height = random.next_int_bound(3) + 4;
        if random.next_int_bound(12) == 0 {
            tree_height *= 2;
        }
        if !self.is_valid_position(level, origin, tree_height) {
            return false;
        }
        self.make_cap(level, random, origin, tree_height);
        self.place_trunk(level, random, origin, tree_height);
        true
    }

    fn is_valid_position(
        &self,
        level: &dyn FeatureLevel,
        origin: BlockPos,
        max_height: i32,
    ) -> bool {
        let max_y = level.min_y() + level.height() - 1;
        if origin.y() < level.min_y() + 1 || origin.y() + max_height + 1 > max_y {
            return false;
        }
        let Some(below) = level.get_block(origin.below()) else {
            return false;
        };
        if !tags::block_has("minecraft:dirt", &below.name)
            && !tags::block_has("minecraft:mushroom_grow_block", &below.name)
        {
            return false;
        }
        for y in 0..=max_height {
            let radius = self.validity_radius(y);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let Some(state) = level.get_block(origin.offset(x, y, z)) else {
                        return false;
                    };
                    if !super::is_air_name(&state.name)
                        && !tags::block_has("minecraft:leaves", &state.name)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// `getTreeRadiusForHeight(-1, -1, foliageRadius, y)`.
    ///
    /// The vanilla validity check passes `height = -1`, so the red mushroom's
    /// `y == height`/`y >= height - 3` branches can never match and only the
    /// centre column is checked. The brown mushroom ignores those arguments.
    fn validity_radius(&self, y: i32) -> i32 {
        match self.kind {
            HugeMushroomKind::Brown => {
                if y <= 3 {
                    0
                } else {
                    self.foliage_radius
                }
            }
            // The red feature's branches all compare against `height`, which
            // the caller passes as -1; neither branch can match a loop index.
            HugeMushroomKind::Red => 0,
        }
    }

    fn make_cap(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        tree_height: i32,
    ) {
        match self.kind {
            HugeMushroomKind::Brown => self.make_brown_cap(level, random, origin, tree_height),
            HugeMushroomKind::Red => self.make_red_cap(level, random, origin, tree_height),
        }
    }

    fn make_brown_cap(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        tree_height: i32,
    ) {
        let radius = self.foliage_radius;
        for x in -radius..=radius {
            for z in -radius..=radius {
                let west = x == -radius;
                let east = x == radius;
                let north = z == -radius;
                let south = z == radius;
                let x_edge = west || east;
                let z_edge = north || south;
                if x_edge && z_edge {
                    continue;
                }
                let pos = origin.offset(x, tree_height, z);
                let Some(mut state) = self.cap_provider.sample(random, origin) else {
                    continue;
                };
                set_property(
                    &mut state,
                    "west",
                    &(west || z_edge && x == 1 - radius).to_string(),
                );
                set_property(
                    &mut state,
                    "east",
                    &(east || z_edge && x == radius - 1).to_string(),
                );
                set_property(
                    &mut state,
                    "north",
                    &(north || x_edge && z == 1 - radius).to_string(),
                );
                set_property(
                    &mut state,
                    "south",
                    &(south || x_edge && z == radius - 1).to_string(),
                );
                place_mushroom_block(level, pos, state);
            }
        }
    }

    fn make_red_cap(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        tree_height: i32,
    ) {
        let radius = self.foliage_radius;
        for y in tree_height - 3..=tree_height {
            let layer_radius = if y < tree_height { radius } else { radius - 1 };
            let inner = radius - 2;
            for x in -layer_radius..=layer_radius {
                for z in -layer_radius..=layer_radius {
                    let north = x == -layer_radius;
                    let south = x == layer_radius;
                    let west = z == -layer_radius;
                    let east = z == layer_radius;
                    let x_edge = north || south;
                    let z_edge = west || east;
                    if y < tree_height && x_edge == z_edge {
                        continue;
                    }
                    let pos = origin.offset(x, y, z);
                    let Some(mut state) = self.cap_provider.sample(random, origin) else {
                        continue;
                    };
                    set_property(&mut state, "up", &(y >= tree_height - 1).to_string());
                    set_property(&mut state, "west", &(x < -inner).to_string());
                    set_property(&mut state, "east", &(x > inner).to_string());
                    set_property(&mut state, "north", &(z < -inner).to_string());
                    set_property(&mut state, "south", &(z > inner).to_string());
                    place_mushroom_block(level, pos, state);
                }
            }
        }
    }

    fn place_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        max_height: i32,
    ) {
        for y in 0..max_height {
            let state = self.stem_provider.sample(random, origin);
            if let Some(state) = state {
                place_mushroom_block(level, origin.above_n(y), state);
            }
        }
    }
}

/// `AbstractHugeMushroomFeature.placeMushroomBlock`.
fn place_mushroom_block(level: &mut dyn FeatureLevel, pos: BlockPos, state: BlockState) {
    if level.get_block(pos).is_some_and(|current| {
        super::is_air_name(&current.name)
            || tags::block_has("minecraft:replaceable_by_mushrooms", &current.name)
    }) {
        level.set_block(pos, state);
    }
}

/// `minecraft:huge_fungus` with the configuration defined for 1.21.11.
#[derive(Debug, Clone)]
pub struct HugeFungusFeature {
    valid_base_state: BlockState,
    stem_state: BlockState,
    hat_state: BlockState,
    decor_state: BlockState,
    replaceable_blocks: Vec<String>,
    planted: bool,
}

impl HugeFungusFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let base_matches = level
            .get_block(origin.below())
            .is_some_and(|below| below.name == self.valid_base_state.name);
        if !base_matches {
            return false;
        }
        let mut height = mth_next_int(random, 4, 13);
        if random.next_int_bound(12) == 0 {
            height *= 2;
        }
        if !self.planted && origin.y() + height + 1 >= level.height() {
            return false;
        }
        let huge = !self.planted && random.next_float() < 0.06;
        level.set_block(origin, default_state("minecraft:air"));
        self.place_stem(level, random, origin, height, huge);
        self.place_hat(level, random, origin, height, huge);
        true
    }

    fn is_replaceable(&self, level: &dyn FeatureLevel, pos: BlockPos, check_config: bool) -> bool {
        level.get_block(pos).is_some_and(|state| {
            can_be_replaced(state)
                || (check_config
                    && self
                        .replaceable_blocks
                        .iter()
                        .any(|name| name == &state.name))
        })
    }

    fn place_stem(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        height: i32,
        huge: bool,
    ) {
        let radius = i32::from(huge);
        for x in -radius..=radius {
            for z in -radius..=radius {
                let corner = huge && x.abs() == radius && z.abs() == radius;
                for y in 0..height {
                    let pos = origin.offset(x, y, z);
                    if !self.is_replaceable(level, pos, true) {
                        continue;
                    }
                    if self.planted {
                        level.set_block(pos, self.stem_state.clone());
                    } else if corner {
                        if random.next_float() < 0.1 {
                            level.set_block(pos, self.stem_state.clone());
                        }
                    } else {
                        level.set_block(pos, self.stem_state.clone());
                    }
                }
            }
        }
    }

    fn place_hat(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        height: i32,
        huge: bool,
    ) {
        let weeping = self.hat_state.name == "minecraft:nether_wart_block";
        let minimum = (random.next_int_bound(1 + height / 3) + 5).min(height);
        let start = height - minimum;
        for y in start..=height {
            let mut radius = if y < height - random.next_int_bound(3) {
                2
            } else {
                1
            };
            if minimum > 8 && y < start + 4 {
                radius = 3;
            }
            if huge {
                radius += 1;
            }
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let x_edge = x == -radius || x == radius;
                    let z_edge = z == -radius || z == radius;
                    let inner = !x_edge && !z_edge && y != height;
                    let corner = x_edge && z_edge;
                    let below_start = y < start + 3;
                    let pos = origin.offset(x, y, z);
                    if !self.is_replaceable(level, pos, false) {
                        continue;
                    }
                    if below_start {
                        if !inner {
                            self.place_hat_drop_block(level, random, pos, weeping);
                        }
                    } else if inner {
                        self.place_hat_block(
                            level,
                            random,
                            pos,
                            0.1,
                            0.2,
                            if weeping { 0.1 } else { 0.0 },
                        );
                    } else if corner {
                        self.place_hat_block(
                            level,
                            random,
                            pos,
                            0.01,
                            0.7,
                            if weeping { 0.083 } else { 0.0 },
                        );
                    } else {
                        self.place_hat_block(
                            level,
                            random,
                            pos,
                            5.0e-4,
                            0.98,
                            if weeping { 0.07 } else { 0.0 },
                        );
                    }
                }
            }
        }
    }

    fn place_hat_block(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        decoration_chance: f32,
        hat_chance: f32,
        weeping_chance: f32,
    ) {
        if random.next_float() < decoration_chance {
            level.set_block(pos, self.decor_state.clone());
        } else if random.next_float() < hat_chance {
            level.set_block(pos, self.hat_state.clone());
            if random.next_float() < weeping_chance {
                try_place_weeping_vines(level, random, pos);
            }
        }
    }

    fn place_hat_drop_block(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        weeping: bool,
    ) {
        let below_is_hat = level
            .get_block(pos.below())
            .is_some_and(|below| below.name == self.hat_state.name);
        if below_is_hat {
            level.set_block(pos, self.hat_state.clone());
        } else if random.next_float() < 0.15 {
            level.set_block(pos, self.hat_state.clone());
            if weeping && random.next_int_bound(11) == 0 {
                try_place_weeping_vines(level, random, pos);
            }
        }
    }
}

fn try_place_weeping_vines(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    pos: BlockPos,
) {
    let below = pos.below();
    if !is_empty_block(level, below) {
        return;
    }
    let mut length = mth_next_int(random, 1, 5);
    if random.next_int_bound(7) == 0 {
        length *= 2;
    }
    place_weeping_vines_column(level, random, below, length, 23, 25);
}

/// `minecraft:glowstone_blob`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlowstoneFeature;

impl GlowstoneFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if !is_empty_block(level, origin) {
            return false;
        }
        let supported_ceiling = level.get_block(origin.above()).is_some_and(|above| {
            matches!(
                above.name.as_str(),
                "minecraft:netherrack" | "minecraft:basalt" | "minecraft:blackstone"
            )
        });
        if !supported_ceiling {
            return false;
        }
        level.set_block(origin, default_state("minecraft:glowstone"));
        for _ in 0..1500 {
            let dx = random.next_int_bound(8) - random.next_int_bound(8);
            let dy = -random.next_int_bound(12);
            let dz = random.next_int_bound(8) - random.next_int_bound(8);
            let pos = origin.offset(dx, dy, dz);
            if !is_empty_block(level, pos) {
                continue;
            }
            let mut neighbours = 0;
            for direction in Direction::ALL {
                if level
                    .get_block(pos.relative(direction))
                    .is_some_and(|state| state.name == "minecraft:glowstone")
                {
                    neighbours += 1;
                }
                if neighbours > 1 {
                    break;
                }
            }
            if neighbours == 1 {
                level.set_block(pos, default_state("minecraft:glowstone"));
            }
        }
        true
    }
}

/// `minecraft:forest_rock` (`BlockBlobFeature`).
#[derive(Debug, Clone)]
pub struct BlockBlobFeature {
    state: BlockState,
}

impl BlockBlobFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut pos = origin;
        while pos.y() > level.min_y() + 3 {
            if !is_empty_block(level, pos.below()) {
                let below = level.get_block(pos.below()).expect("checked above");
                if tags::block_has("minecraft:dirt", &below.name)
                    || tags::block_has("minecraft:base_stone_overworld", &below.name)
                {
                    break;
                }
            }
            pos = pos.below();
        }
        if pos.y() <= level.min_y() + 3 {
            return false;
        }
        for _ in 0..3 {
            let rx = random.next_int_bound(2);
            let ry = random.next_int_bound(2);
            let rz = random.next_int_bound(2);
            let limit = (rx + ry + rz) as f32 * 0.333 + 0.5;
            let min = pos.offset(-rx, -ry, -rz);
            let max = pos.offset(rx, ry, rz);
            for candidate in BlockPos::between_closed(min, max) {
                let dx = f64::from(candidate.x() - pos.x());
                let dy = f64::from(candidate.y() - pos.y());
                let dz = f64::from(candidate.z() - pos.z());
                if dx * dx + dy * dy + dz * dz <= f64::from(limit * limit) {
                    level.set_block(candidate, self.state.clone());
                }
            }
            pos = pos.offset(
                -1 + random.next_int_bound(2),
                -random.next_int_bound(2),
                -1 + random.next_int_bound(2),
            );
        }
        true
    }
}

/// `minecraft:fill_layer`.
#[derive(Debug, Clone)]
pub struct FillLayerFeature {
    height: i32,
    state: BlockState,
}

impl FillLayerFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let y = level.min_y() + self.height;
        for x in 0..16 {
            for z in 0..16 {
                let pos = BlockPos::new(origin.x() + x, y, origin.z() + z);
                if is_empty_block(level, pos) {
                    level.set_block(pos, self.state.clone());
                }
            }
        }
        true
    }
}

/// `minecraft:nether_forest_vegetation`.
#[derive(Debug, Clone)]
pub struct NetherForestVegetationFeature {
    state_provider: VegetationStateProvider,
    spread_width: i32,
    spread_height: i32,
}

impl NetherForestVegetationFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let is_nylium = level
            .get_block(origin.below())
            .is_some_and(|below| tags::block_has("minecraft:nylium", &below.name));
        if !is_nylium {
            return false;
        }
        let y = origin.y();
        let max_y = level.min_y() + level.height() - 1;
        if y < level.min_y() + 1 || y + 1 > max_y {
            return false;
        }
        let mut placed = 0;
        let tries = self.spread_width * self.spread_width;
        for _ in 0..tries {
            let dx =
                random.next_int_bound(self.spread_width) - random.next_int_bound(self.spread_width);
            let dy = random.next_int_bound(self.spread_height)
                - random.next_int_bound(self.spread_height);
            let dz =
                random.next_int_bound(self.spread_width) - random.next_int_bound(self.spread_width);
            let pos = origin.offset(dx, dy, dz);
            let Some(state) = self.state_provider.sample(random, pos) else {
                continue;
            };
            if is_empty_block(level, pos)
                && pos.y() > level.min_y()
                && nether_plant_can_survive(&state.name, level, pos)
            {
                level.set_block(pos, state);
                placed += 1;
            }
        }
        placed > 0
    }
}

/// `minecraft:random_boolean_selector`.
///
/// Both shipped uses nest placed features with an empty placement list (the
/// mushroom-island selector and the lush-caves clay selector), so nested
/// non-empty placement lists are rejected instead of half-executed.
#[derive(Debug, Clone)]
pub struct RandomBooleanSelectorFeature {
    feature_false: Box<ConfiguredFeature>,
    feature_true: Box<ConfiguredFeature>,
}

impl RandomBooleanSelectorFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        if random.next_boolean() {
            self.feature_true.place(level, random, origin)
        } else {
            self.feature_false.place(level, random, origin)
        }
    }
}

pub(crate) fn parse_huge_mushroom_config(
    config: &Value,
    kind: HugeMushroomKind,
) -> io::Result<Option<HugeMushroomFeature>> {
    let object = config_object(config, "huge mushroom")?;
    let Some(cap_provider) = VegetationStateProvider::parse(required(object, "cap_provider")?)?
    else {
        return Ok(None);
    };
    let Some(stem_provider) = VegetationStateProvider::parse(required(object, "stem_provider")?)?
    else {
        return Ok(None);
    };
    if !cap_provider.is_supported() || !stem_provider.is_supported() {
        return Ok(None);
    }
    let foliage_radius = object
        .get("foliage_radius")
        .and_then(Value::as_i64)
        .unwrap_or(2);
    let foliage_radius = i32::try_from(foliage_radius)
        .map_err(|_| super::aquatic::invalid("huge mushroom foliage radius does not fit in i32"))?;
    if foliage_radius < 0 {
        return Err(super::aquatic::invalid(
            "huge mushroom foliage radius must not be negative",
        ));
    }
    Ok(Some(HugeMushroomFeature {
        kind,
        cap_provider,
        stem_provider,
        foliage_radius,
    }))
}

pub(crate) fn parse_huge_fungus_config(config: &Value) -> io::Result<Option<HugeFungusFeature>> {
    let object = config_object(config, "huge fungus")?;
    let Some(replaceable_blocks) = parse_matching_blocks(required(object, "replaceable_blocks")?)?
    else {
        return Ok(None);
    };
    Ok(Some(HugeFungusFeature {
        valid_base_state: parse_state_field(object, "valid_base_block")?,
        stem_state: parse_state_field(object, "stem_state")?,
        hat_state: parse_state_field(object, "hat_state")?,
        decor_state: parse_state_field(object, "decor_state")?,
        replaceable_blocks,
        planted: object
            .get("planted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

pub(crate) fn parse_glowstone_config(config: &Value) -> io::Result<Option<GlowstoneFeature>> {
    config_object(config, "glowstone blob")?;
    Ok(Some(GlowstoneFeature))
}

pub(crate) fn parse_block_blob_config(config: &Value) -> io::Result<Option<BlockBlobFeature>> {
    let object = config_object(config, "block blob")?;
    Ok(Some(BlockBlobFeature {
        state: parse_state_field(object, "state")?,
    }))
}

pub(crate) fn parse_fill_layer_config(config: &Value) -> io::Result<Option<FillLayerFeature>> {
    let object = config_object(config, "fill layer")?;
    Ok(Some(FillLayerFeature {
        height: int_field(object, "height", 0, 4064)?,
        state: parse_state_field(object, "state")?,
    }))
}

pub(crate) fn parse_nether_forest_vegetation_config(
    config: &Value,
) -> io::Result<Option<NetherForestVegetationFeature>> {
    let object = config_object(config, "nether forest vegetation")?;
    let Some(state_provider) = VegetationStateProvider::parse(required(object, "state_provider")?)?
    else {
        return Ok(None);
    };
    if !state_provider.is_supported() {
        return Ok(None);
    }
    Ok(Some(NetherForestVegetationFeature {
        state_provider,
        spread_width: int_field(object, "spread_width", 1, i32::MAX)?,
        spread_height: int_field(object, "spread_height", 1, i32::MAX)?,
    }))
}

pub(crate) fn parse_random_boolean_selector_config(
    config: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
) -> io::Result<Option<RandomBooleanSelectorFeature>> {
    let object = config_object(config, "random boolean selector")?;
    let Some(feature_false) = parse_nested_feature(required(object, "feature_false")?, configured)?
    else {
        return Ok(None);
    };
    let Some(feature_true) = parse_nested_feature(required(object, "feature_true")?, configured)?
    else {
        return Ok(None);
    };
    Ok(Some(RandomBooleanSelectorFeature {
        feature_false,
        feature_true,
    }))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str) -> io::Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| super::aquatic::invalid(format!("config is missing `{key}`")))
}

fn parse_matching_blocks(value: &Value) -> io::Result<Option<Vec<String>>> {
    let object = value
        .as_object()
        .ok_or_else(|| super::aquatic::invalid("block predicate must be an object"))?;
    if object.get("type").and_then(Value::as_str) != Some("minecraft:matching_blocks") {
        return Ok(None);
    }
    let blocks = object
        .get("blocks")
        .ok_or_else(|| super::aquatic::invalid("matching_blocks is missing `blocks`"))?;
    let names: Vec<String> = if let Some(name) = blocks.as_str() {
        vec![name.to_owned()]
    } else {
        let list = blocks
            .as_array()
            .ok_or_else(|| super::aquatic::invalid("matching_blocks `blocks` must be a list"))?;
        list.iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| super::aquatic::invalid("block name must be a string"))
            })
            .collect::<io::Result<_>>()?
    };
    Ok(Some(
        names
            .into_iter()
            .map(|name| super::resource_location(&name))
            .collect(),
    ))
}

/// Parses the `{ "feature": ..., "placement": [] }` branch form used by
/// `random_boolean_selector`. Non-empty placement lists stay explicit
/// unsupported.
fn parse_nested_feature(
    value: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
) -> io::Result<Option<Box<ConfiguredFeature>>> {
    if let Some(reference) = value.as_str() {
        return Ok(configured
            .get(&super::resource_location(reference))
            .filter(|feature| feature.is_supported())
            .map(|feature| Box::new(feature.clone())));
    }
    let object = value
        .as_object()
        .ok_or_else(|| super::aquatic::invalid("nested feature must be a string or object"))?;
    let placement = object
        .get("placement")
        .and_then(Value::as_array)
        .ok_or_else(|| super::aquatic::invalid("nested feature is missing `placement`"))?;
    if !placement.is_empty() {
        return Ok(None);
    }
    let feature = object
        .get("feature")
        .ok_or_else(|| super::aquatic::invalid("nested feature is missing `feature`"))?;
    super::parse_inline_configured_feature(
        "minecraft:random_boolean_selector_nested",
        feature,
        configured,
    )
}
