//! Cave decoration features: dripstone clusters and pointed dripstone.
//!
//! Ports `PointedDripstoneFeature`, `DripstoneClusterFeature`, and
//! `LargeDripstoneFeature`, plus the shared `Column` scan and
//! `DripstoneUtils` helpers from 1.21.11.

use super::FeatureLevel;
use super::placement::{Heightmap, IntProvider};
use super::support::{
    FloatProvider, float_field, int_field, parse_int_provider_supported, pointed_dripstone, state,
};
use crate::chunk::BlockState;
use crate::tags;
use folia_math::{BlockPos, Direction, Vec3};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

fn object<'a>(config: &'a Value, kind: &str) -> io::Result<&'a Map<String, Value>> {
    config
        .as_object()
        .ok_or_else(|| super::invalid(format!("{kind} config must be an object")))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str, kind: &str) -> io::Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| super::invalid(format!("{kind} is missing `{key}`")))
}

fn block_name(state: &BlockState) -> &str {
    &state.name
}

fn is_lava(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level.get_block(pos).map(block_name) == Some("minecraft:lava")
}

fn is_water_at(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| crate::block_behaviour::fluid(state) == Some("minecraft:water"))
}

fn is_empty_or_water(state: &BlockState) -> bool {
    super::is_air_name(&state.name) || state.name == "minecraft:water"
}

fn is_neither_empty_nor_water(state: &BlockState) -> bool {
    !super::is_air_name(&state.name) && state.name != "minecraft:water"
}

fn is_empty_or_water_or_lava(state: &BlockState) -> bool {
    super::is_air_name(&state.name)
        || state.name == "minecraft:water"
        || state.name == "minecraft:lava"
}

fn is_dripstone_base(name: &str) -> bool {
    name == "minecraft:dripstone_block"
        || tags::block_has("minecraft:dripstone_replaceable_blocks", name)
}

fn is_dripstone_base_or_lava(name: &str) -> bool {
    is_dripstone_base(name) || name == "minecraft:lava"
}

/// `Column`, reduced to the two optional edges the dripstone features use.
#[derive(Debug, Clone, Copy)]
struct Column {
    floor: Option<i32>,
    ceiling: Option<i32>,
}

impl Column {
    fn height(self) -> Option<i32> {
        match (self.floor, self.ceiling) {
            (Some(floor), Some(ceiling)) => Some(ceiling - floor - 1),
            _ => None,
        }
    }

    fn with_floor(self, floor: Option<i32>) -> Self {
        Self {
            floor,
            ceiling: self.ceiling,
        }
    }
}

fn column_scan(
    level: &dyn FeatureLevel,
    origin: BlockPos,
    max_distance: i32,
    column_predicate: &dyn Fn(&BlockState) -> bool,
    tip_predicate: &dyn Fn(&BlockState) -> bool,
) -> Option<Column> {
    let origin_state = level.get_block(origin)?;
    if !column_predicate(origin_state) {
        return None;
    }
    let ceiling = scan_direction(
        level,
        max_distance,
        column_predicate,
        tip_predicate,
        origin,
        Direction::Up,
    );
    let floor = scan_direction(
        level,
        max_distance,
        column_predicate,
        tip_predicate,
        origin,
        Direction::Down,
    );
    Some(match (floor, ceiling) {
        (Some(floor), Some(ceiling)) => Column {
            floor: Some(floor),
            ceiling: Some(ceiling),
        },
        (Some(floor), None) => Column {
            floor: Some(floor),
            ceiling: None,
        },
        (None, Some(ceiling)) => Column {
            floor: None,
            ceiling: Some(ceiling),
        },
        (None, None) => Column {
            floor: None,
            ceiling: None,
        },
    })
}

fn scan_direction(
    level: &dyn FeatureLevel,
    max_distance: i32,
    column_predicate: &dyn Fn(&BlockState) -> bool,
    tip_predicate: &dyn Fn(&BlockState) -> bool,
    origin: BlockPos,
    direction: Direction,
) -> Option<i32> {
    let mut pos = origin;
    let mut distance = 1;
    while distance < max_distance && level.get_block(pos).is_some_and(column_predicate) {
        pos = pos.relative(direction);
        distance += 1;
    }
    level
        .get_block(pos)
        .filter(|state| tip_predicate(state))
        .map(|_| pos.y())
}

fn dripstone_block_if_possible(level: &mut dyn FeatureLevel, pos: BlockPos) -> bool {
    let replaceable = level.get_block(pos).is_some_and(|state| {
        tags::block_has("minecraft:dripstone_replaceable_blocks", &state.name)
    });
    if replaceable {
        level.set_block(pos, state("minecraft:dripstone_block"));
        true
    } else {
        false
    }
}

fn grow_pointed_dripstone(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    direction: Direction,
    height: i32,
    merge_tip: bool,
) {
    let base = level
        .get_block(pos.relative(direction.opposite()))
        .is_some_and(|state| is_dripstone_base(&state.name));
    if !base || height <= 0 {
        return;
    }
    let direction_name = if direction == Direction::Down {
        "down"
    } else {
        "up"
    };
    let mut cursor = pos;
    let mut place = |thickness: &str, merge: bool| {
        let waterlogged = is_water_at(level, cursor);
        level.set_block(
            cursor,
            pointed_dripstone(direction_name, thickness, waterlogged),
        );
        cursor = cursor.relative(direction);
        let _ = merge;
    };
    if height >= 3 {
        place("base", false);
        for _ in 0..height - 3 {
            place("middle", false);
        }
    }
    if height >= 2 {
        place("frustum", false);
    }
    if height >= 1 {
        place(if merge_tip { "tip_merge" } else { "tip" }, true);
    }
}

fn clamped_map(value: f32, from_min: f32, from_max: f32, to_min: f32, to_max: f32) -> f32 {
    if from_min == from_max {
        return to_min;
    }
    let clamped = value.clamp(from_min, from_max);
    to_min + (clamped - from_min) / (from_max - from_min) * (to_max - to_min)
}

fn random_between_biased(
    random: &mut dyn RandomSource,
    min: f32,
    max: f32,
    mean: f32,
    deviation: f32,
) -> f32 {
    (random.next_gaussian() as f32 * deviation + mean).clamp(min, max)
}

fn dripstone_height(radius: f64, max_radius: f64, scale: f64, min_radius: f64) -> f64 {
    let radius = radius.max(min_radius);
    let base = radius / max_radius * 0.384;
    let a = 0.75 * base.powf(1.333_333_333_333_333_3);
    let b = base.powf(0.666_666_666_666_666_6);
    let c = 0.333_333_333_333_333_3 * base.ln();
    let value = (scale * (a - b - c)).max(0.0);
    value / 0.384 * max_radius
}

fn circle_mostly_embedded_in_stone(level: &dyn FeatureLevel, pos: BlockPos, radius: i32) -> bool {
    if level.get_block(pos).is_none_or(is_empty_or_water_or_lava) {
        return false;
    }
    let step = 6.0 / radius as f32;
    let mut angle = 0.0f32;
    while angle < std::f32::consts::PI * 2.0 {
        let x = (angle.cos() * radius as f32) as i32;
        let z = (angle.sin() * radius as f32) as i32;
        if level
            .get_block(pos.offset(x, 0, z))
            .is_none_or(is_empty_or_water_or_lava)
        {
            return false;
        }
        angle += step;
    }
    true
}

#[derive(Debug, Clone)]
pub struct PointedDripstoneFeature {
    chance_of_directional_spread: f32,
    chance_of_spread_radius2: f32,
    chance_of_spread_radius3: f32,
    chance_of_taller_dripstone: f32,
}

impl PointedDripstoneFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let tip_direction = self.tip_direction(level, origin, random);
        let Some(tip_direction) = tip_direction else {
            return false;
        };
        let patch_origin = origin.relative(tip_direction.opposite());
        self.create_patch_of_dripstone_blocks(level, random, patch_origin);
        let taller = random.next_float() < self.chance_of_taller_dripstone
            && level
                .get_block(origin.relative(tip_direction))
                .is_none_or(is_empty_or_water);
        let height = if taller { 2 } else { 1 };
        grow_pointed_dripstone(level, origin, tip_direction, height, false);
        true
    }

    fn tip_direction(
        &self,
        level: &dyn FeatureLevel,
        pos: BlockPos,
        random: &mut dyn RandomSource,
    ) -> Option<Direction> {
        let above = level
            .get_block(pos.above())
            .is_some_and(|state| is_dripstone_base(&state.name));
        let below = level
            .get_block(pos.below())
            .is_some_and(|state| is_dripstone_base(&state.name));
        match (above, below) {
            (true, true) => Some(if random.next_boolean() {
                Direction::Down
            } else {
                Direction::Up
            }),
            (true, false) => Some(Direction::Down),
            (false, true) => Some(Direction::Up),
            (false, false) => None,
        }
    }

    fn create_patch_of_dripstone_blocks(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) {
        dripstone_block_if_possible(level, pos);
        for direction in [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ] {
            if random.next_float() > self.chance_of_directional_spread {
                continue;
            }
            let side = pos.relative(direction);
            dripstone_block_if_possible(level, side);
            if random.next_float() > self.chance_of_spread_radius2 {
                continue;
            }
            let second = side.relative(random_direction(random));
            dripstone_block_if_possible(level, second);
            if random.next_float() > self.chance_of_spread_radius3 {
                continue;
            }
            let third = second.relative(random_direction(random));
            dripstone_block_if_possible(level, third);
        }
    }
}

fn random_direction(random: &mut dyn RandomSource) -> Direction {
    Direction::ALL[random.next_int_bound(6) as usize]
}

pub(crate) fn parse_pointed_dripstone_config(
    config: &Value,
) -> io::Result<Option<PointedDripstoneFeature>> {
    let object = object(config, "pointed dripstone")?;
    Ok(Some(PointedDripstoneFeature {
        chance_of_directional_spread: float_field(object, "chance_of_directional_spread")?,
        chance_of_spread_radius2: float_field(object, "chance_of_spread_radius2")?,
        chance_of_spread_radius3: float_field(object, "chance_of_spread_radius3")?,
        chance_of_taller_dripstone: float_field(object, "chance_of_taller_dripstone")?,
    }))
}

#[derive(Debug, Clone)]
pub struct DripstoneClusterFeature {
    chance_of_dripstone_column_at_max_distance_from_center: f32,
    density: FloatProvider,
    dripstone_block_layer_thickness: IntProvider,
    floor_to_ceiling_search_range: i32,
    height: IntProvider,
    height_deviation: f32,
    max_distance_from_center_affecting_height_bias: i32,
    max_distance_from_edge_affecting_chance_of_dripstone_column: i32,
    max_stalagmite_stalactite_height_diff: i32,
    radius: IntProvider,
    wetness: FloatProvider,
}

impl DripstoneClusterFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if level
            .get_block(origin)
            .is_none_or(|state| !is_empty_or_water(state))
        {
            return false;
        }
        let height = self.height.sample(random).expect("int provider is ported");
        let wetness = self.wetness.sample(random);
        let density = self.density.sample(random);
        let radius_x = self.radius.sample(random).expect("int provider is ported");
        let radius_z = self.radius.sample(random).expect("int provider is ported");
        for x in -radius_x..=radius_x {
            for z in -radius_z..=radius_z {
                let min = (radius_x - x.abs()).min(radius_z - z.abs());
                let chance = f64::from(clamped_map(
                    min as f32,
                    0.0,
                    self.max_distance_from_edge_affecting_chance_of_dripstone_column as f32,
                    self.chance_of_dripstone_column_at_max_distance_from_center,
                    1.0,
                ));
                self.place_column(
                    level,
                    random,
                    origin.offset(x, 0, z),
                    x,
                    z,
                    wetness,
                    chance,
                    height,
                    density,
                );
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn place_column(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        x: i32,
        z: i32,
        wetness: f32,
        chance: f64,
        height: i32,
        density: f32,
    ) {
        let Some(column) = column_scan(
            level,
            pos,
            self.floor_to_ceiling_search_range,
            &is_empty_or_water,
            &is_neither_empty_nor_water,
        ) else {
            return;
        };
        if column.floor.is_none() && column.ceiling.is_none() {
            return;
        }
        let original = column;
        let pool = random.next_float() < wetness;
        let column = match column.floor {
            Some(floor) if pool && self.can_place_pool(level, pos.at_y(floor)) => {
                level.set_block(pos.at_y(floor), state("minecraft:water"));
                original.with_floor(Some(floor - 1))
            }
            _ => original,
        };
        let floor = column.floor;
        let ceiling = column.ceiling;
        let stalactite_chance = random.next_double() < chance;
        let mut stalactite_height = 0;
        if let Some(ceiling) =
            ceiling.filter(|ceiling| stalactite_chance && !is_lava(level, pos.at_y(*ceiling)))
        {
            let thickness = self
                .dripstone_block_layer_thickness
                .sample(random)
                .expect("int provider is ported");
            replace_blocks_with_dripstone_blocks(
                level,
                pos.at_y(ceiling),
                thickness,
                Direction::Up,
            );
            let available = floor.map_or(height, |floor| height.min(ceiling - floor));
            stalactite_height = self.get_dripstone_height(random, x, z, density, available);
        }
        let stalagmite_chance = random.next_double() < chance;
        let mut stalagmite_height = 0;
        if let Some(floor) =
            floor.filter(|floor| stalagmite_chance && !is_lava(level, pos.at_y(*floor)))
        {
            let thickness = self
                .dripstone_block_layer_thickness
                .sample(random)
                .expect("int provider is ported");
            replace_blocks_with_dripstone_blocks(
                level,
                pos.at_y(floor),
                thickness,
                Direction::Down,
            );
            stalagmite_height = if ceiling.is_some() {
                (stalactite_height
                    + random.next_int_between_inclusive(
                        -self.max_stalagmite_stalactite_height_diff,
                        self.max_stalagmite_stalactite_height_diff,
                    ))
                .max(0)
            } else {
                self.get_dripstone_height(random, x, z, density, height)
            };
        }
        let (ceiling_height, floor_height) = match (ceiling, floor) {
            (Some(ceiling), Some(floor))
                if ceiling - stalactite_height <= floor + stalagmite_height =>
            {
                let max = (ceiling - stalactite_height).max(floor + 1);
                let min = (floor + stalagmite_height).min(ceiling - 1);
                let split = random.next_int_between_inclusive(max, min + 1);
                (ceiling - split, split - 1 - floor)
            }
            _ => (stalactite_height, stalagmite_height),
        };
        let merge_tip = random.next_boolean()
            && ceiling_height > 0
            && floor_height > 0
            && column.height() == Some(ceiling_height + floor_height);
        if let Some(ceiling) = ceiling {
            grow_pointed_dripstone(
                level,
                pos.at_y(ceiling - 1),
                Direction::Down,
                ceiling_height,
                merge_tip,
            );
        }
        if let Some(floor) = floor {
            grow_pointed_dripstone(
                level,
                pos.at_y(floor + 1),
                Direction::Up,
                floor_height,
                merge_tip,
            );
        }
    }

    fn get_dripstone_height(
        &self,
        random: &mut dyn RandomSource,
        x: i32,
        z: i32,
        chance: f32,
        height: i32,
    ) -> i32 {
        if random.next_float() > chance {
            return 0;
        }
        let distance = (x.abs() + z.abs()) as f32;
        let mean = clamped_map(
            distance,
            0.0,
            self.max_distance_from_center_affecting_height_bias as f32,
            height as f32 / 2.0,
            0.0,
        );
        random_between_biased(random, 0.0, height as f32, mean, self.height_deviation) as i32
    }

    fn can_place_pool(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        let Some(current) = level.get_block(pos) else {
            return false;
        };
        if matches!(
            current.name.as_str(),
            "minecraft:water" | "minecraft:dripstone_block" | "minecraft:pointed_dripstone"
        ) {
            return false;
        }
        if level
            .get_block(pos.above())
            .is_some_and(|state| crate::block_behaviour::fluid(state) == Some("minecraft:water"))
        {
            return false;
        }
        for direction in [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ] {
            if !can_be_adjacent_to_water(level, pos.relative(direction)) {
                return false;
            }
        }
        can_be_adjacent_to_water(level, pos.below())
    }
}

fn can_be_adjacent_to_water(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level.get_block(pos).is_some_and(|state| {
        tags::block_has("minecraft:base_stone_overworld", &state.name)
            || crate::block_behaviour::fluid(state) == Some("minecraft:water")
    })
}

fn replace_blocks_with_dripstone_blocks(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    thickness: i32,
    direction: Direction,
) {
    let mut cursor = pos;
    for _ in 0..thickness {
        if !dripstone_block_if_possible(level, cursor) {
            return;
        }
        cursor = cursor.relative(direction);
    }
}

pub(crate) fn parse_dripstone_cluster_config(
    config: &Value,
) -> io::Result<Option<DripstoneClusterFeature>> {
    let object = object(config, "dripstone cluster")?;
    Ok(Some(DripstoneClusterFeature {
        chance_of_dripstone_column_at_max_distance_from_center: float_field(
            object,
            "chance_of_dripstone_column_at_max_distance_from_center",
        )?,
        density: FloatProvider::parse(required(object, "density", "dripstone cluster")?)?,
        dripstone_block_layer_thickness: parse_int_provider_supported(required(
            object,
            "dripstone_block_layer_thickness",
            "dripstone cluster",
        )?)?,
        floor_to_ceiling_search_range: int_field(object, "floor_to_ceiling_search_range")?,
        height: parse_int_provider_supported(required(object, "height", "dripstone cluster")?)?,
        height_deviation: float_field(object, "height_deviation")?,
        max_distance_from_center_affecting_height_bias: int_field(
            object,
            "max_distance_from_center_affecting_height_bias",
        )?,
        max_distance_from_edge_affecting_chance_of_dripstone_column: int_field(
            object,
            "max_distance_from_edge_affecting_chance_of_dripstone_column",
        )?,
        max_stalagmite_stalactite_height_diff: int_field(
            object,
            "max_stalagmite_stalactite_height_diff",
        )?,
        radius: parse_int_provider_supported(required(object, "radius", "dripstone cluster")?)?,
        wetness: FloatProvider::parse(required(object, "wetness", "dripstone cluster")?)?,
    }))
}

trait AtY {
    fn at_y(self, y: i32) -> BlockPos;
}

impl AtY for BlockPos {
    fn at_y(self, y: i32) -> BlockPos {
        BlockPos::new(self.x(), y, self.z())
    }
}

#[derive(Debug, Clone)]
struct LargeDripstone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    fn height(&self) -> i32 {
        self.height_at_radius(0.0)
    }

    fn height_at_radius(&self, radius: f32) -> i32 {
        dripstone_height(
            f64::from(radius),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }

    fn suitable_for_wind(&self, min_radius: i32, min_bluntness: f32) -> bool {
        self.radius >= min_radius && self.bluntness >= f64::from(min_bluntness)
    }

    fn move_back_until_embedded(&mut self, level: &dyn FeatureLevel, wind: &WindOffsetter) -> bool {
        while self.radius > 1 {
            let mut candidate = self.root;
            let distance = 10.min(self.height());
            for _ in 0..distance {
                if is_lava(level, candidate) {
                    return false;
                }
                if circle_mostly_embedded_in_stone(level, wind.offset(candidate), self.radius) {
                    self.root = candidate;
                    return true;
                }
                candidate = candidate.relative(if self.pointing_up {
                    Direction::Down
                } else {
                    Direction::Up
                });
            }
            self.radius /= 2;
        }
        false
    }

    fn place_blocks(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        wind: &WindOffsetter,
    ) {
        for x in -self.radius..=self.radius {
            for z in -self.radius..=self.radius {
                let distance = ((x * x + z * z) as f32).sqrt();
                if distance > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at_radius(distance);
                if height <= 0 {
                    continue;
                }
                if random.next_float() < 0.2 {
                    height = (height as f32 * (random.next_float() * (1.0 - 0.8) + 0.8)) as i32;
                }
                let mut cursor = self.root.offset(x, 0, z);
                let mut placed = false;
                let limit = if self.pointing_up {
                    level
                        .heightmap(Heightmap::WorldSurfaceWg, cursor.x(), cursor.z())
                        .unwrap_or(i32::MAX)
                } else {
                    i32::MAX
                };
                let mut step = 0;
                while step < height && cursor.y() < limit {
                    let pos = wind.offset(cursor);
                    let empty = level.get_block(pos).is_none_or(is_empty_or_water_or_lava);
                    if empty {
                        placed = true;
                        level.set_block(pos, state("minecraft:dripstone_block"));
                    } else if placed
                        && level.get_block(pos).is_some_and(|state| {
                            tags::block_has("minecraft:base_stone_overworld", &state.name)
                        })
                    {
                        break;
                    }
                    cursor = cursor.relative(if self.pointing_up {
                        Direction::Up
                    } else {
                        Direction::Down
                    });
                    step += 1;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WindOffsetter {
    origin_y: i32,
    wind: Option<Vec3>,
}

impl WindOffsetter {
    fn new(origin_y: i32, random: &mut dyn RandomSource, magnitude: &FloatProvider) -> Self {
        let strength = magnitude.sample(random);
        let angle = random.next_float() * std::f32::consts::PI;
        Self {
            origin_y,
            wind: Some(Vec3::new(
                f64::from(angle.cos() * strength),
                0.0,
                f64::from(angle.sin() * strength),
            )),
        }
    }

    fn none() -> Self {
        Self {
            origin_y: 0,
            wind: None,
        }
    }

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some(wind) = self.wind else {
            return pos;
        };
        let scale = f64::from(self.origin_y - pos.y());
        let offset = wind.scale(scale);
        pos.offset(offset.x.floor() as i32, 0, offset.z.floor() as i32)
    }
}

#[derive(Debug, Clone)]
pub struct LargeDripstoneFeature {
    column_radius: IntProvider,
    floor_to_ceiling_search_range: i32,
    height_scale: FloatProvider,
    max_column_radius_to_cave_height_ratio: f32,
    min_bluntness_for_wind: f32,
    min_radius_for_wind: i32,
    stalactite_bluntness: FloatProvider,
    stalagmite_bluntness: FloatProvider,
    wind_speed: FloatProvider,
}

impl LargeDripstoneFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if level
            .get_block(origin)
            .is_none_or(|state| !is_empty_or_water(state))
        {
            return false;
        }
        let tip_predicate = |state: &BlockState| is_dripstone_base_or_lava(&state.name);
        let Some(column) = column_scan(
            level,
            origin,
            self.floor_to_ceiling_search_range,
            &is_empty_or_water,
            &tip_predicate,
        ) else {
            return false;
        };
        let (Some(floor), Some(ceiling)) = (column.floor, column.ceiling) else {
            return false;
        };
        let height = column.height().unwrap_or(0);
        if height < 4 {
            return false;
        }
        let ratio = (height as f32 * self.max_column_radius_to_cave_height_ratio) as i32;
        let (min_radius, max_radius) = int_provider_bounds(&self.column_radius);
        let upper = ratio.clamp(min_radius, max_radius);
        let radius = random.next_int_between_inclusive(min_radius, upper);
        let mut stalactite = LargeDripstone {
            root: origin.at_y(ceiling - 1),
            pointing_up: false,
            radius,
            bluntness: f64::from(self.stalactite_bluntness.sample(random)),
            scale: f64::from(self.height_scale.sample(random)),
        };
        let mut stalagmite = LargeDripstone {
            root: origin.at_y(floor + 1),
            pointing_up: true,
            radius,
            bluntness: f64::from(self.stalagmite_bluntness.sample(random)),
            scale: f64::from(self.height_scale.sample(random)),
        };
        let wind = if stalactite
            .suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
            && stalagmite.suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
        {
            WindOffsetter::new(origin.y(), random, &self.wind_speed)
        } else {
            WindOffsetter::none()
        };
        let stalactite_ok = stalactite.move_back_until_embedded(level, &wind);
        let stalagmite_ok = stalagmite.move_back_until_embedded(level, &wind);
        if stalactite_ok {
            stalactite.place_blocks(level, random, &wind);
        }
        if stalagmite_ok {
            stalagmite.place_blocks(level, random, &wind);
        }
        true
    }
}

fn int_provider_bounds(provider: &IntProvider) -> (i32, i32) {
    match provider {
        IntProvider::Constant(value) => (*value, *value),
        IntProvider::Uniform {
            min_inclusive,
            max_inclusive,
        }
        | IntProvider::BiasedToBottom {
            min_inclusive,
            max_inclusive,
        }
        | IntProvider::Clamped {
            min_inclusive,
            max_inclusive,
            ..
        }
        | IntProvider::ClampedNormal {
            min_inclusive,
            max_inclusive,
            ..
        } => (*min_inclusive, *max_inclusive),
        _ => {
            let sampled = provider
                .sample(&mut folia_random::LegacyRandomSource::new(0))
                .unwrap_or(0);
            (sampled, sampled)
        }
    }
}

pub(crate) fn parse_large_dripstone_config(
    config: &Value,
) -> io::Result<Option<LargeDripstoneFeature>> {
    let object = object(config, "large dripstone")?;
    Ok(Some(LargeDripstoneFeature {
        column_radius: parse_int_provider_supported(required(
            object,
            "column_radius",
            "large dripstone",
        )?)?,
        floor_to_ceiling_search_range: int_field(object, "floor_to_ceiling_search_range")?,
        height_scale: FloatProvider::parse(required(object, "height_scale", "large dripstone")?)?,
        max_column_radius_to_cave_height_ratio: float_field(
            object,
            "max_column_radius_to_cave_height_ratio",
        )?,
        min_bluntness_for_wind: float_field(object, "min_bluntness_for_wind")?,
        min_radius_for_wind: int_field(object, "min_radius_for_wind")?,
        stalactite_bluntness: FloatProvider::parse(required(
            object,
            "stalactite_bluntness",
            "large dripstone",
        )?)?,
        stalagmite_bluntness: FloatProvider::parse(required(
            object,
            "stalagmite_bluntness",
            "large dripstone",
        )?)?,
        wind_speed: FloatProvider::parse(required(object, "wind_speed", "large dripstone")?)?,
    }))
}
