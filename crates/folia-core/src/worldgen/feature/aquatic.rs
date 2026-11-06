//! Aquatic plants, coral, bamboo, and water-floor features.
//!
//! Ports the water-bound half of the P2.5 batch:
//!
//! - `minecraft:seagrass` (`SeagrassFeature`)
//! - `minecraft:kelp` (`KelpFeature`)
//! - `minecraft:sea_pickle` (`SeaPickleFeature`)
//! - `minecraft:coral_tree`, `minecraft:coral_claw`,
//!   `minecraft:coral_mushroom` (`CoralFeature` subclasses)
//! - `minecraft:bamboo` (`BambooFeature`)
//! - `minecraft:underwater_magma` (`UnderwaterMagmaFeature`)
//!
//! Every feature follows the Java RNG draw order. Blocks that vanilla tests
//! through `BlockState.canSurvive` use the same full-cube approximation the
//! rest of the port uses until P1.5 block metadata lands; the helpers here are
//! shared with the vine and mushroom modules so all three agree.

use super::placement::{Heightmap, IntProvider, parse_block_state, parse_int_provider};
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::registry;
use crate::tags;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

/// `Direction.Plane.HORIZONTAL.faces()`: north, east, south, west.
pub(crate) const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

/// `Direction.Plane.HORIZONTAL.getRandomDirection(random)`.
pub(crate) fn random_horizontal(random: &mut dyn RandomSource) -> Direction {
    HORIZONTAL[random.next_int_bound(HORIZONTAL.len() as i32) as usize]
}

/// `Direction.getClockWise()` for a horizontal direction.
pub(crate) fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        _ => direction,
    }
}

/// `Direction.getCounterClockWise()` for a horizontal direction.
pub(crate) fn counter_clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::West,
        Direction::West => Direction::South,
        Direction::South => Direction::East,
        Direction::East => Direction::North,
        _ => direction,
    }
}

/// `Util.shuffle`: Fisher-Yates from the end, one `nextInt` per swap.
pub(crate) fn shuffle_copy<T: Copy>(values: &[T], random: &mut dyn RandomSource) -> Vec<T> {
    let mut values = values.to_vec();
    for index in (2..=values.len()).rev() {
        let swap = random.next_int_bound(index as i32) as usize;
        values.swap(index - 1, swap);
    }
    values
}

/// `Mth.nextInt(RandomSource, int, int)`.
///
/// Java returns `minimum` without drawing when `minimum >= maximum`.
pub(crate) fn mth_next_int(random: &mut dyn RandomSource, minimum: i32, maximum: i32) -> i32 {
    if minimum >= maximum {
        minimum
    } else {
        random.next_int_bound(maximum - minimum + 1) + minimum
    }
}

/// `LevelReader.isEmptyBlock`.
///
/// In-region absent sections read as air; a position outside the write region
/// returns `None` and is treated as not empty.
pub(crate) fn is_empty_block(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| is_air_name(&state.name))
}

/// `BlockState.is(Blocks.WATER)`.
pub(crate) fn is_water_state(state: &BlockState) -> bool {
    state.name == "minecraft:water"
}

/// `LevelReader.getFluidState(pos).is(FluidTags.WATER)` for a source block.
pub(crate) fn is_water_at(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level.get_block(pos).is_some_and(is_water_state)
}

/// Approximation of `Block.isShapeFullBlock(state.getFaceOcclusionShape(...))`.
///
/// Air, fluids, and every tagged replaceable plant do not occlude a face; solid
/// cubes do. Leaves and saplings are excluded because their collision shape is
/// empty even though their support shape is a full cube.
pub(crate) fn face_sturdy(name: &str) -> bool {
    !is_air_name(name)
        && name != "minecraft:water"
        && name != "minecraft:lava"
        && !name.ends_with("_leaves")
        && !name.ends_with("_sapling")
        && !name.ends_with("_flower")
        && !tags::block_has("minecraft:replaceable", name)
        && !tags::block_has("minecraft:replaceable_by_trees", name)
        && !matches!(
            name,
            "minecraft:short_grass"
                | "minecraft:tall_grass"
                | "minecraft:fern"
                | "minecraft:large_fern"
                | "minecraft:vine"
                | "minecraft:glow_lichen"
        )
}

/// `BlockBehaviour.BlockStateBase.canBeReplaced()` without a place context.
pub(crate) fn can_be_replaced(state: &BlockState) -> bool {
    is_air_name(&state.name)
        || state.name == "minecraft:water"
        || state.name == "minecraft:lava"
        || tags::block_has("minecraft:replaceable", &state.name)
}

/// Sets a property when the state declares it, like `BlockState.setValue`.
pub(crate) fn set_property(state: &mut BlockState, name: &str, value: &str) -> bool {
    if let Some((_, slot)) = state.properties.iter_mut().find(|(key, _)| key == name) {
        *slot = value.to_owned();
        true
    } else {
        false
    }
}

/// Default state for a vanilla block, or a property-less fallback.
pub(crate) fn default_state(name: &str) -> BlockState {
    registry::default_block_state(name).unwrap_or_else(|| BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    })
}

/// `VegetationBlock.canSurvive` for seagrass: a sturdy, non-magma block below.
pub(crate) fn seagrass_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos.below())
        .is_some_and(|below| face_sturdy(&below.name) && below.name != "minecraft:magma_block")
}

/// `GrowingPlantBlock.canSurvive` for kelp, whose growth direction is up.
fn kelp_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    let Some(below) = level.get_block(pos.below()) else {
        return false;
    };
    if below.name == "minecraft:magma_block" {
        return false;
    }
    matches!(
        below.name.as_str(),
        "minecraft:kelp" | "minecraft:kelp_plant"
    ) || face_sturdy(&below.name)
}

/// `VegetationBlock.canSurvive` for sea pickles: any sturdy block below.
fn sea_pickle_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos.below())
        .is_some_and(|below| face_sturdy(&below.name))
}

/// `blockState.canSurvive(level, pos)` for the nether plants this slice places.
pub(crate) fn nether_plant_can_survive(
    name: &str,
    level: &dyn FeatureLevel,
    pos: BlockPos,
) -> bool {
    let Some(below) = level.get_block(pos.below()) else {
        return false;
    };
    match name {
        "minecraft:crimson_roots"
        | "minecraft:warped_roots"
        | "minecraft:nether_sprouts"
        | "minecraft:crimson_fungus"
        | "minecraft:warped_fungus" => {
            tags::block_has("minecraft:nylium", &below.name)
                || below.name == "minecraft:soul_soil"
                || tags::block_has("minecraft:dirt", &below.name)
                || below.name == "minecraft:farmland"
        }
        _ => false,
    }
}

/// `minecraft:seagrass`.
#[derive(Debug, Clone)]
pub struct SeagrassFeature {
    probability: f32,
}

impl SeagrassFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut placed = false;
        let dx = random.next_int_bound(8) - random.next_int_bound(8);
        let dz = random.next_int_bound(8) - random.next_int_bound(8);
        let Some(height) = level.heightmap(Heightmap::OceanFloor, origin.x() + dx, origin.z() + dz)
        else {
            return false;
        };
        let pos = BlockPos::new(origin.x() + dx, height, origin.z() + dz);
        if !is_water_at(level, pos) {
            return false;
        }
        let tall = random.next_double() < f64::from(self.probability);
        if tall {
            let lower = default_state("minecraft:tall_seagrass");
            let mut upper = lower.clone();
            set_property(&mut upper, "half", "upper");
            if seagrass_lower_can_survive(level, pos) {
                let above = pos.above();
                if is_water_at(level, above) {
                    level.set_block(pos, lower);
                    level.set_block(above, upper);
                }
                // Vanilla sets the success flag after `canSurvive`, even when
                // the upper half was not water and nothing was written.
                placed = true;
            }
        } else {
            let state = default_state("minecraft:seagrass");
            if seagrass_can_survive(level, pos) {
                level.set_block(pos, state);
                placed = true;
            }
        }
        placed
    }
}

/// `TallSeagrassBlock.canSurvive` for the lower half.
fn seagrass_lower_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    is_water_at(level, pos) && seagrass_can_survive(level, pos)
}

/// `minecraft:kelp`.
#[derive(Debug, Clone, Copy, Default)]
pub struct KelpFeature;

impl KelpFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Some(height) = level.heightmap(Heightmap::OceanFloor, origin.x(), origin.z()) else {
            return false;
        };
        let mut pos = BlockPos::new(origin.x(), height, origin.z());
        if !is_water_at(level, pos) {
            return false;
        }
        let mut placed = 0;
        let length = 1 + random.next_int_bound(10);
        for index in 0..=length {
            if is_water_at(level, pos)
                && is_water_at(level, pos.above())
                && kelp_can_survive(level, pos)
            {
                if index == length {
                    let mut head = default_state("minecraft:kelp");
                    set_property(
                        &mut head,
                        "age",
                        &(random.next_int_bound(4) + 20).to_string(),
                    );
                    level.set_block(pos, head);
                    placed += 1;
                } else {
                    level.set_block(pos, default_state("minecraft:kelp_plant"));
                }
            } else if index > 0 {
                let below = pos.below();
                if kelp_can_survive(level, below)
                    && level
                        .get_block(below.below())
                        .is_none_or(|state| state.name != "minecraft:kelp")
                {
                    let mut head = default_state("minecraft:kelp");
                    set_property(
                        &mut head,
                        "age",
                        &(random.next_int_bound(4) + 20).to_string(),
                    );
                    level.set_block(below, head);
                    placed += 1;
                }
                break;
            }
            pos = pos.above();
        }
        placed > 0
    }
}

/// `minecraft:sea_pickle`.
#[derive(Debug, Clone)]
pub struct SeaPickleFeature {
    count: IntProvider,
}

impl SeaPickleFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Ok(count) = self.count.sample(random) else {
            return false;
        };
        let mut placed = 0;
        for _ in 0..count {
            let dx = random.next_int_bound(8) - random.next_int_bound(8);
            let dz = random.next_int_bound(8) - random.next_int_bound(8);
            let Some(height) =
                level.heightmap(Heightmap::OceanFloor, origin.x() + dx, origin.z() + dz)
            else {
                continue;
            };
            let pos = BlockPos::new(origin.x() + dx, height, origin.z() + dz);
            let mut state = default_state("minecraft:sea_pickle");
            set_property(
                &mut state,
                "pickles",
                &(random.next_int_bound(4) + 1).to_string(),
            );
            if is_water_at(level, pos) && sea_pickle_can_survive(level, pos) {
                level.set_block(pos, state);
                placed += 1;
            }
        }
        placed > 0
    }
}

/// `CoralFeature` shape selected by the configured-feature type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoralShape {
    Tree,
    Claw,
    Mushroom,
}

/// `CoralTreeFeature`, `CoralClawFeature`, and `CoralMushroomFeature`.
#[derive(Debug, Clone)]
pub struct CoralFeature {
    shape: CoralShape,
}

impl CoralFeature {
    pub(crate) fn vanilla_type(&self) -> &'static str {
        match self.shape {
            CoralShape::Tree => "minecraft:coral_tree",
            CoralShape::Claw => "minecraft:coral_claw",
            CoralShape::Mushroom => "minecraft:coral_mushroom",
        }
    }

    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let block = CORAL_BLOCKS[random.next_int_bound(CORAL_BLOCKS.len() as i32) as usize];
        let state = default_state(block);
        match self.shape {
            CoralShape::Tree => place_coral_tree(level, random, origin, state),
            CoralShape::Claw => place_coral_claw(level, random, origin, state),
            CoralShape::Mushroom => place_coral_mushroom(level, random, origin, state),
        }
    }
}

/// Java `data/minecraft/tags/block/coral_blocks.json` order. The extracted tag
/// table is sorted for lookup, so the feature keeps the data order here.
const CORAL_BLOCKS: [&str; 5] = [
    "minecraft:tube_coral_block",
    "minecraft:brain_coral_block",
    "minecraft:bubble_coral_block",
    "minecraft:fire_coral_block",
    "minecraft:horn_coral_block",
];

/// `minecraft:corals` after expanding `#minecraft:coral_plants`, in tag order.
const CORALS: [&str; 10] = [
    "minecraft:tube_coral",
    "minecraft:brain_coral",
    "minecraft:bubble_coral",
    "minecraft:fire_coral",
    "minecraft:horn_coral",
    "minecraft:tube_coral_fan",
    "minecraft:brain_coral_fan",
    "minecraft:bubble_coral_fan",
    "minecraft:fire_coral_fan",
    "minecraft:horn_coral_fan",
];

/// `minecraft:wall_corals` in tag order.
const WALL_CORALS: [&str; 5] = [
    "minecraft:tube_coral_wall_fan",
    "minecraft:brain_coral_wall_fan",
    "minecraft:bubble_coral_wall_fan",
    "minecraft:fire_coral_wall_fan",
    "minecraft:horn_coral_wall_fan",
];

/// `CoralFeature.placeCoralBlock`.
fn place_coral_block(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    pos: BlockPos,
    state: &BlockState,
) -> bool {
    let above = pos.above();
    let current_is_valid = level.get_block(pos).is_some_and(|current| {
        is_water_state(current) || tags::block_has("minecraft:corals", &current.name)
    });
    if !(current_is_valid && is_water_at(level, above)) {
        return false;
    }
    level.set_block(pos, state.clone());
    if random.next_float() < 0.25 {
        let coral = CORALS[random.next_int_bound(CORALS.len() as i32) as usize];
        level.set_block(above, default_state(coral));
    } else if random.next_float() < 0.05 {
        let mut pickle = default_state("minecraft:sea_pickle");
        set_property(
            &mut pickle,
            "pickles",
            &(random.next_int_bound(4) + 1).to_string(),
        );
        level.set_block(above, pickle);
    }
    for direction in HORIZONTAL {
        if random.next_float() < 0.2 {
            let neighbor = pos.relative(direction);
            if is_water_at(level, neighbor) {
                let coral = WALL_CORALS[random.next_int_bound(WALL_CORALS.len() as i32) as usize];
                let mut fan = default_state(coral);
                set_property(&mut fan, "facing", direction_name(direction));
                level.set_block(neighbor, fan);
            }
        }
    }
    true
}

fn place_coral_tree(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
    state: BlockState,
) -> bool {
    let mut cursor = origin;
    let height = random.next_int_bound(3) + 1;
    for _ in 0..height {
        if !place_coral_block(level, random, cursor, &state) {
            return true;
        }
        cursor = cursor.above();
    }
    let top = cursor;
    let branches = random.next_int_bound(3) + 2;
    let directions = shuffle_copy(&HORIZONTAL, random);
    for direction in directions.into_iter().take(branches as usize) {
        let mut branch = top.relative(direction);
        let length = random.next_int_bound(5) + 2;
        let mut run = 0;
        for step in 0..length {
            if !place_coral_block(level, random, branch, &state) {
                break;
            }
            run += 1;
            branch = branch.above();
            if step == 0 || (run >= 2 && random.next_float() < 0.25) {
                branch = branch.relative(direction);
                run = 0;
            }
        }
    }
    true
}

fn place_coral_claw(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
    state: BlockState,
) -> bool {
    if !place_coral_block(level, random, origin, &state) {
        return false;
    }
    let main = random_horizontal(random);
    let branches = random.next_int_bound(2) + 2;
    let all = [main, clockwise(main), counter_clockwise(main)];
    let directions = shuffle_copy(&all, random);
    for direction in directions.into_iter().take(branches as usize) {
        let mut cursor = origin;
        let straight = random.next_int_bound(2) + 1;
        cursor = cursor.relative(direction);
        let (growth, length) = if direction == main {
            (main, random.next_int_bound(3) + 2)
        } else {
            cursor = cursor.above();
            let growth = [direction, Direction::Up][random.next_int_bound(2) as usize];
            (growth, random.next_int_bound(3) + 3)
        };
        let mut step = 0;
        while step < straight && place_coral_block(level, random, cursor, &state) {
            cursor = cursor.relative(growth);
            step += 1;
        }
        cursor = cursor.relative(growth.opposite());
        cursor = cursor.above();
        for _ in 0..length {
            cursor = cursor.relative(main);
            if !place_coral_block(level, random, cursor, &state) {
                break;
            }
            if random.next_float() < 0.25 {
                cursor = cursor.above();
            }
        }
    }
    true
}

fn place_coral_mushroom(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
    state: BlockState,
) -> bool {
    let size_x = random.next_int_bound(3) + 3;
    let size_y = random.next_int_bound(3) + 3;
    let size_z = random.next_int_bound(3) + 3;
    let drop = random.next_int_bound(3) + 1;
    for x in 0..=size_x {
        for y in 0..=size_y {
            for z in 0..=size_z {
                let pos = BlockPos::new(x + origin.x(), y + origin.y(), z + origin.z())
                    .relative_n(Direction::Down, drop);
                // Java's shape condition: none of (x,y), (z,y), (x,z) may be
                // a pair of edges.
                let x_edge = x == 0 || x == size_x;
                let y_edge = y == 0 || y == size_y;
                let z_edge = z == 0 || z == size_z;
                let corner_ok = !((z_edge || x_edge) && y_edge || (x_edge && z_edge));
                let shell = x_edge || y_edge || z_edge;
                if corner_ok && shell && random.next_float() >= 0.1 {
                    place_coral_block(level, random, pos, &state);
                }
            }
        }
    }
    true
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

/// `minecraft:bamboo`.
#[derive(Debug, Clone)]
pub struct BambooFeature {
    probability: f32,
}

impl BambooFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if !is_empty_block(level, origin) {
            return false;
        }
        if bamboo_can_survive(level, origin) {
            let height = random.next_int_bound(12) + 5;
            if random.next_float() < self.probability {
                let radius = random.next_int_bound(4) + 1;
                for x in origin.x() - radius..=origin.x() + radius {
                    for z in origin.z() - radius..=origin.z() + radius {
                        let dx = x - origin.x();
                        let dz = z - origin.z();
                        if dx * dx + dz * dz > radius * radius {
                            continue;
                        }
                        let Some(surface) = level.heightmap(Heightmap::WorldSurface, x, z) else {
                            continue;
                        };
                        let ground = BlockPos::new(x, surface - 1, z);
                        if level
                            .get_block(ground)
                            .is_some_and(|state| tags::block_has("minecraft:dirt", &state.name))
                        {
                            level.set_block(ground, default_state("minecraft:podzol"));
                        }
                    }
                }
            }
            let mut cursor = origin;
            for _ in 0..height {
                if !is_empty_block(level, cursor) {
                    break;
                }
                level.set_block(cursor, bamboo_state("1", "none", "0"));
                cursor = cursor.above();
            }
            if cursor.y() - origin.y() >= 3 {
                level.set_block(cursor, bamboo_state("1", "large", "1"));
                level.set_block(cursor.below(), bamboo_state("1", "large", "0"));
                level.set_block(cursor.below_n(2), bamboo_state("1", "small", "0"));
            }
        }
        true
    }
}

fn bamboo_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos.below())
        .is_some_and(|below| tags::block_has("minecraft:bamboo_plantable_on", &below.name))
}

fn bamboo_state(age: &str, leaves: &str, stage: &str) -> BlockState {
    let mut state = default_state("minecraft:bamboo");
    set_property(&mut state, "age", age);
    set_property(&mut state, "leaves", leaves);
    set_property(&mut state, "stage", stage);
    state
}

/// `minecraft:underwater_magma`.
#[derive(Debug, Clone)]
pub struct UnderwaterMagmaFeature {
    floor_search_range: i32,
    placement_radius_around_floor: i32,
    placement_probability_per_valid_position: f32,
}

impl UnderwaterMagmaFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Some(floor_y) = self.floor_y(level, origin) else {
            return false;
        };
        let center = origin.with_y(floor_y);
        let radius = self.placement_radius_around_floor;
        let min = center.offset(-radius, -radius, -radius);
        let max = center.offset(radius, radius, radius);
        let mut placed = 0;
        for pos in BlockPos::between_closed(min, max) {
            if random.next_float() < self.placement_probability_per_valid_position
                && self.is_valid_placement(level, pos)
            {
                level.set_block(pos, default_state("minecraft:magma_block"));
                placed += 1;
            }
        }
        placed > 0
    }

    /// `Column.scan` with a water column predicate and a non-water tip
    /// predicate, returning only the floor.
    fn floor_y(&self, level: &dyn FeatureLevel, origin: BlockPos) -> Option<i32> {
        if !is_water_at(level, origin) {
            return None;
        }
        let mut y = origin.y();
        let mut step = 1;
        while step < self.floor_search_range
            && is_water_at(level, BlockPos::new(origin.x(), y, origin.z()))
        {
            y -= 1;
            step += 1;
        }
        if is_water_at(level, BlockPos::new(origin.x(), y, origin.z())) {
            None
        } else {
            Some(y)
        }
    }

    fn is_valid_placement(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        let Some(current) = level.get_block(pos) else {
            return false;
        };
        if is_water_state(current) || is_air_name(&current.name) {
            return false;
        }
        if visible_from_outside(level, pos.below(), Direction::Up) {
            return false;
        }
        HORIZONTAL.iter().all(|direction| {
            !visible_from_outside(level, pos.relative(*direction), direction.opposite())
        })
    }
}

/// `BlockState.getFaceOcclusionShape` emptiness/fullness test.
fn visible_from_outside(level: &dyn FeatureLevel, pos: BlockPos, _face: Direction) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| !face_sturdy(&state.name))
}

pub(crate) fn parse_seagrass_config(config: &Value) -> io::Result<Option<SeagrassFeature>> {
    let probability = probability_field(config, "seagrass")?;
    Ok(Some(SeagrassFeature { probability }))
}

pub(crate) fn parse_kelp_config(config: &Value) -> io::Result<Option<KelpFeature>> {
    config_object(config, "kelp")?;
    Ok(Some(KelpFeature))
}

pub(crate) fn parse_sea_pickle_config(config: &Value) -> io::Result<Option<SeaPickleFeature>> {
    let object = config_object(config, "sea pickle")?;
    let count = parse_int_provider(
        object
            .get("count")
            .ok_or_else(|| invalid("sea pickle is missing `count`"))?,
    )?;
    if !count.is_supported() || !non_negative_count(&count) {
        return Ok(None);
    }
    Ok(Some(SeaPickleFeature { count }))
}

pub(crate) fn parse_coral_config(
    config: &Value,
    vanilla_type: &str,
) -> io::Result<Option<CoralFeature>> {
    config_object(config, "coral")?;
    let shape = match vanilla_type {
        "minecraft:coral_tree" => CoralShape::Tree,
        "minecraft:coral_claw" => CoralShape::Claw,
        "minecraft:coral_mushroom" => CoralShape::Mushroom,
        _ => return Ok(None),
    };
    Ok(Some(CoralFeature { shape }))
}

pub(crate) fn parse_bamboo_config(config: &Value) -> io::Result<Option<BambooFeature>> {
    let probability = probability_field(config, "bamboo")?;
    Ok(Some(BambooFeature { probability }))
}

pub(crate) fn parse_underwater_magma_config(
    config: &Value,
) -> io::Result<Option<UnderwaterMagmaFeature>> {
    let object = config_object(config, "underwater magma")?;
    let floor_search_range = int_field(object, "floor_search_range", 0, 512)?;
    let placement_radius_around_floor = int_field(object, "placement_radius_around_floor", 0, 64)?;
    let placement_probability_per_valid_position =
        float_field(object, "placement_probability_per_valid_position", 0.0, 1.0)?;
    Ok(Some(UnderwaterMagmaFeature {
        floor_search_range,
        placement_radius_around_floor,
        placement_probability_per_valid_position,
    }))
}

fn probability_field(config: &Value, name: &str) -> io::Result<f32> {
    let object = config_object(config, name)?;
    float_field(object, "probability", 0.0, 1.0)
}

pub(crate) fn config_object<'a>(
    config: &'a Value,
    name: &str,
) -> io::Result<&'a Map<String, Value>> {
    config
        .as_object()
        .ok_or_else(|| invalid(format!("{name} config must be an object")))
}

pub(crate) fn int_field(
    object: &Map<String, Value>,
    key: &str,
    min: i32,
    max: i32,
) -> io::Result<i32> {
    let value = object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("config is missing integer `{key}`")))?;
    let value =
        i32::try_from(value).map_err(|_| invalid(format!("`{key}` does not fit in i32")))?;
    if !(min..=max).contains(&value) {
        return Err(invalid(format!("`{key}` must be between {min} and {max}")));
    }
    Ok(value)
}

pub(crate) fn float_field(
    object: &Map<String, Value>,
    key: &str,
    min: f32,
    max: f32,
) -> io::Result<f32> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| invalid(format!("config is missing numeric `{key}`")))?
        as f32;
    if !(min..=max).contains(&value) {
        return Err(invalid(format!("`{key}` must be between {min} and {max}")));
    }
    Ok(value)
}

pub(crate) fn non_negative_count(provider: &IntProvider) -> bool {
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
        IntProvider::Clamped { source, .. } => non_negative_count(source),
        IntProvider::WeightedList(entries) => {
            entries.iter().all(|entry| non_negative_count(&entry.data))
        }
        IntProvider::ClampedNormal {
            min_inclusive,
            max_inclusive,
            ..
        } => *min_inclusive >= 0 && *max_inclusive >= 0,
        IntProvider::Unsupported { .. } => false,
    }
}

pub(crate) fn parse_state_field(object: &Map<String, Value>, key: &str) -> io::Result<BlockState> {
    parse_block_state(
        object
            .get(key)
            .ok_or_else(|| invalid(format!("config is missing `{key}`")))?,
    )
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coral_tag_orders_match_the_shipped_tag_membership() {
        for block in CORAL_BLOCKS {
            assert!(tags::block_has("minecraft:coral_blocks", block));
        }
        for coral in CORALS {
            assert!(tags::block_has("minecraft:corals", coral));
        }
        for coral in WALL_CORALS {
            assert!(tags::block_has("minecraft:wall_corals", coral));
        }
        assert_eq!(CORAL_BLOCKS.len(), 5);
        assert_eq!(CORALS.len(), 10);
        assert_eq!(WALL_CORALS.len(), 5);
        assert_eq!(
            tags::members(
                crate::tags::TagRegistryKind::Block,
                "minecraft:coral_blocks"
            )
            .len(),
            CORAL_BLOCKS.len()
        );
    }

    #[test]
    fn java_horizontal_order_and_rotation() {
        assert_eq!(
            HORIZONTAL,
            [
                Direction::North,
                Direction::East,
                Direction::South,
                Direction::West
            ]
        );
        for direction in HORIZONTAL {
            assert_eq!(counter_clockwise(clockwise(direction)), direction);
        }
    }
}
