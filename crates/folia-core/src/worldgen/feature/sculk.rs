//! Sculk patch feature port.
//!
//! Ports `SculkPatchFeature`, `SculkSpreader`, `SculkBlock`,
//! `SculkVeinBlock`, and the subset of `MultifaceSpreader` that sculk uses.
//! The spreader is deterministic in Java's RNG order: cursor updates run in
//! insertion order, direction shuffles consume one draw per swap, and the
//! charge/growth draws follow the decompiled classes.

use super::placement::IntProvider;
use super::support::{parse_int_provider_supported, set_property, state, state_with};
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::Value;
use std::io;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn water_source(state: &BlockState) -> bool {
    state.name == "minecraft:water"
        && state
            .properties
            .iter()
            .find(|(name, _)| name == "level")
            .is_none_or(|(_, value)| value == "0")
}

fn fluid(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:water" | "minecraft:lava" | "minecraft:flowing_water" | "minecraft:flowing_lava"
    )
}

fn full_collision(state: &BlockState) -> bool {
    let boxes = crate::block_shapes::collision_boxes_for_state(state);
    boxes.len() == 1
        && boxes[0].min_x == 0.0
        && boxes[0].min_y == 0.0
        && boxes[0].min_z == 0.0
        && boxes[0].max_x == 1.0
        && boxes[0].max_y == 1.0
        && boxes[0].max_z == 1.0
}

fn sturdy(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(crate::block_behaviour::is_solid)
}

fn block_name(level: &dyn FeatureLevel, pos: BlockPos) -> Option<&str> {
    level.get_block(pos).map(|state| state.name.as_str())
}

fn empty(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| is_air_name(&state.name))
}

fn sculk_vein_state(
    north: bool,
    east: bool,
    south: bool,
    west: bool,
    up: bool,
    down: bool,
    waterlogged: bool,
) -> BlockState {
    state_with(
        "minecraft:sculk_vein",
        &[
            ("north", if north { "true" } else { "false" }),
            ("east", if east { "true" } else { "false" }),
            ("south", if south { "true" } else { "false" }),
            ("west", if west { "true" } else { "false" }),
            ("up", if up { "true" } else { "false" }),
            ("down", if down { "true" } else { "false" }),
            ("waterlogged", if waterlogged { "true" } else { "false" }),
        ],
    )
}

fn face_property(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

fn has_face(state: &BlockState, direction: Direction) -> bool {
    state.name == "minecraft:sculk_vein"
        && state
            .properties
            .iter()
            .find(|(name, _)| name == face_property(direction))
            .is_some_and(|(_, value)| value == "true")
}

fn available_faces(state: &BlockState) -> Vec<Direction> {
    Direction::ALL
        .into_iter()
        .filter(|direction| has_face(state, *direction))
        .collect()
}

fn can_attach_to(level: &dyn FeatureLevel, pos: BlockPos, direction: Direction) -> bool {
    sturdy(level, pos.relative(direction))
}

fn regrow(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    state: &BlockState,
    directions: &[Direction],
) -> bool {
    let mut placed = sculk_vein_state(false, false, false, false, false, false, false);
    let mut any = false;
    for direction in directions {
        if can_attach_to(level, pos, *direction) {
            let _ = set_property(&mut placed, face_property(*direction), "true");
            any = true;
        }
    }
    if !any {
        return false;
    }
    let waterlogged = fluid(state)
        && state
            .properties
            .iter()
            .find(|(name, _)| name == "waterlogged")
            .is_none_or(|(_, value)| value == "true");
    if waterlogged {
        let _ = set_property(&mut placed, "waterlogged", "true");
    }
    level.set_block(pos, placed);
    true
}

fn shuffled<T: Copy>(values: &[T], random: &mut dyn RandomSource) -> Vec<T> {
    let mut values = values.to_vec();
    for index in (1..values.len()).rev() {
        let pick = random.next_int_bound((index + 1) as i32) as usize;
        values.swap(index, pick);
    }
    values
}

fn distance_squared(a: BlockPos, b: BlockPos) -> i64 {
    let dx = i64::from(a.x()) - i64::from(b.x());
    let dy = i64::from(a.y()) - i64::from(b.y());
    let dz = i64::from(a.z()) - i64::from(b.z());
    dx * dx + dy * dy + dz * dz
}

fn closer_than(a: BlockPos, b: BlockPos, distance: i32) -> bool {
    distance_squared(a, b) < i64::from(distance) * i64::from(distance)
}

fn chessboard_distance(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x())
        .abs()
        .max((a.y() - b.y()).abs())
        .max((a.z() - b.z()).abs())
}

fn manhattan_distance(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x()).abs() + (a.y() - b.y()).abs() + (a.z() - b.z()).abs()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Default,
    Sculk,
    Vein,
}

impl Behavior {
    fn of(state: &BlockState) -> Self {
        match state.name.as_str() {
            "minecraft:sculk" => Self::Sculk,
            "minecraft:sculk_vein" => Self::Vein,
            _ => Self::Default,
        }
    }

    fn can_change_block_state_on_spread(self) -> bool {
        !matches!(self, Self::Sculk)
    }

    fn update_decay_delay(self, current: i32) -> i32 {
        match self {
            Self::Default => (current - 1).max(0),
            Self::Sculk | Self::Vein => 1,
        }
    }

    fn sculk_spread_delay(self) -> i32 {
        1
    }
}

#[derive(Clone, Copy)]
enum SpreadType {
    SamePosition,
    SamePlane,
    WrapAround,
}

const DEFAULT_SPREAD_ORDER: [SpreadType; 3] = [
    SpreadType::SamePosition,
    SpreadType::SamePlane,
    SpreadType::WrapAround,
];

#[derive(Clone, Copy)]
struct SpreadPos {
    pos: BlockPos,
    face: Direction,
}

fn spread_pos(
    spread_type: SpreadType,
    pos: BlockPos,
    face: Direction,
    spread: Direction,
) -> SpreadPos {
    match spread_type {
        SpreadType::SamePosition => SpreadPos { pos, face },
        SpreadType::SamePlane => SpreadPos {
            pos: pos.relative(face),
            face: spread,
        },
        SpreadType::WrapAround => SpreadPos {
            pos: pos.relative(face).relative(spread),
            face: face.opposite(),
        },
    }
}

fn valid_placement(
    level: &dyn FeatureLevel,
    state: &BlockState,
    pos: BlockPos,
    face: Direction,
) -> bool {
    if state.name == "minecraft:sculk_vein" && has_face(state, face) {
        return false;
    }
    can_attach_to(level, pos, face)
}

fn state_can_be_replaced(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    spread: SpreadPos,
    state: &BlockState,
) -> bool {
    let neighbor = block_name(level, spread.pos.relative(spread.face));
    if neighbor.is_some_and(|name| {
        matches!(
            name,
            "minecraft:sculk" | "minecraft:sculk_catalyst" | "minecraft:moving_piston"
        )
    }) {
        return false;
    }
    if manhattan_distance(pos, spread.pos) == 2 {
        let support = pos.relative(spread.face.opposite());
        if sturdy(level, support) {
            return false;
        }
    }
    !fluid(state)
        && !crate::tags::block_has("minecraft:fire", &state.name)
        && (crate::block_behaviour::replaceable(state)
            || is_air_name(&state.name)
            || state.name == "minecraft:sculk_vein"
            || (state.name == "minecraft:water" && water_source(state)))
}

fn spread_into(level: &dyn FeatureLevel, pos: BlockPos, spread: SpreadPos) -> bool {
    let state = match level.get_block(spread.pos) {
        Some(state) => state,
        None => return false,
    };
    state_can_be_replaced(level, pos, spread, state)
        && valid_placement(level, state, spread.pos, spread.face)
}

fn place_vein(level: &mut dyn FeatureLevel, spread: SpreadPos, current: &BlockState) -> bool {
    if !valid_placement(level, current, spread.pos, spread.face) {
        return false;
    }
    let mut placed = if current.name == "minecraft:sculk_vein" {
        current.clone()
    } else {
        sculk_vein_state(
            false,
            false,
            false,
            false,
            false,
            false,
            water_source(current),
        )
    };
    let _ = set_property(&mut placed, face_property(spread.face), "true");
    level.set_block(spread.pos, placed);
    true
}

fn spread_from_face(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    source: &BlockState,
    spread_direction: Direction,
    face: Direction,
    spread_types: &[SpreadType],
) -> bool {
    if face.axis() == spread_direction.axis() {
        return false;
    }
    let other_valid = source.name != "minecraft:sculk_vein";
    let can_spread = other_valid || (has_face(source, spread_direction) && !has_face(source, face));
    if !can_spread {
        return false;
    }
    for spread_type in spread_types {
        let spread = spread_pos(*spread_type, pos, face, spread_direction);
        if spread_into(level, pos, spread) {
            let current = level
                .get_block(spread.pos)
                .cloned()
                .unwrap_or_else(|| state("minecraft:air"));
            return place_vein(level, spread, &current);
        }
    }
    false
}

fn spread_all_custom(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    state: &BlockState,
    spread_types: &[SpreadType],
) -> bool {
    let mut any = false;
    for spread_direction in Direction::ALL {
        if state.name != "minecraft:sculk_vein" || has_face(state, spread_direction) {
            for face in Direction::ALL {
                if spread_from_face(level, pos, state, spread_direction, face, spread_types) {
                    any = true;
                }
            }
        }
    }
    any
}

fn same_space_spread(level: &mut dyn FeatureLevel, pos: BlockPos, state: &BlockState) -> bool {
    spread_all_custom(level, pos, state, &[SpreadType::SamePosition])
}

fn vein_spread_all(level: &mut dyn FeatureLevel, pos: BlockPos, state: &BlockState) -> bool {
    spread_all_custom(level, pos, state, &DEFAULT_SPREAD_ORDER)
}

fn attempt_spread_vein(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    state: &BlockState,
    directions: Option<&[Direction]>,
) -> bool {
    match directions {
        None => same_space_spread(level, pos, state),
        Some(directions) => {
            if directions.is_empty() {
                vein_spread_all(level, pos, state)
            } else {
                (is_air_name(&state.name) || water_source(state))
                    && regrow(level, pos, state, directions)
            }
        }
    }
}

fn has_substrate_access(level: &dyn FeatureLevel, state: &BlockState, pos: BlockPos) -> bool {
    if state.name != "minecraft:sculk_vein" {
        return false;
    }
    Direction::ALL.into_iter().any(|direction| {
        has_face(state, direction)
            && block_name(level, pos.relative(direction))
                .is_some_and(|name| crate::tags::block_has("minecraft:sculk_replaceable", name))
    })
}

fn on_discharged_vein(level: &mut dyn FeatureLevel, source: &BlockState, pos: BlockPos) {
    if source.name != "minecraft:sculk_vein" {
        return;
    }
    let mut current = source.clone();
    for direction in Direction::ALL {
        if has_face(&current, direction)
            && block_name(level, pos.relative(direction)) == Some("minecraft:sculk")
        {
            let _ = set_property(&mut current, face_property(direction), "false");
        }
    }
    if !Direction::ALL
        .into_iter()
        .any(|direction| has_face(&current, direction))
    {
        current = if level.get_block(pos).is_some_and(fluid) {
            state("minecraft:water")
        } else {
            state("minecraft:air")
        };
    }
    level.set_block(pos, current);
}

fn can_place_growth(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    let above = match level.get_block(pos.above()) {
        Some(state) => state,
        None => return false,
    };
    if !(is_air_name(&above.name) || (above.name == "minecraft:water" && water_source(above))) {
        return false;
    }
    let mut count = 0;
    for x in -4..=4 {
        for y in 0..=2 {
            for z in -4..=4 {
                if block_name(level, pos.offset(x, y, z)).is_some_and(|name| {
                    matches!(name, "minecraft:sculk_sensor" | "minecraft:sculk_shrieker")
                }) {
                    count += 1;
                }
                if count > 2 {
                    return false;
                }
            }
        }
    }
    true
}

fn decay_penalty(no_growth_radius: i32, cursor: BlockPos, root: BlockPos, charge: i32) -> i32 {
    let distance = (distance_squared(cursor, root) as f64).sqrt();
    let radius = f64::from(no_growth_radius);
    let squared = (distance - radius) * (distance - radius);
    let squared1 = f64::from(24 - no_growth_radius).powi(2);
    let min = if squared1 <= 0.0 {
        1.0
    } else {
        (squared / squared1).min(1.0)
    };
    ((charge as f32) * (min as f32) * 0.5).max(1.0) as i32
}

fn growth_state(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    random: &mut dyn RandomSource,
    worldgen: bool,
) -> BlockState {
    let sheet = if random.next_int_bound(11) == 0 {
        state_with(
            "minecraft:sculk_shrieker",
            &[
                ("can_summon", if worldgen { "true" } else { "false" }),
                ("shrieking", "false"),
                ("waterlogged", "false"),
            ],
        )
    } else {
        state_with(
            "minecraft:sculk_sensor",
            &[("powered", "false"), ("waterlogged", "false")],
        )
    };
    let is_fluid = level.get_block(pos).is_some_and(fluid);
    if is_fluid {
        let mut sheet = sheet;
        let _ = set_property(&mut sheet, "waterlogged", "true");
        sheet
    } else {
        sheet
    }
}

fn non_corner_neighbours() -> Vec<BlockPos> {
    let mut neighbours = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    neighbours.push(BlockPos::new(x, y, z));
                }
            }
        }
    }
    neighbours
}

fn unobstructed(level: &dyn FeatureLevel, pos: BlockPos, direction: Direction) -> bool {
    !sturdy(level, pos.relative(direction))
}

fn movement_unobstructed(level: &dyn FeatureLevel, from: BlockPos, to: BlockPos) -> bool {
    if manhattan_distance(from, to) == 1 {
        return true;
    }
    let delta = BlockPos::new(to.x() - from.x(), to.y() - from.y(), to.z() - from.z());
    let x = if delta.x() < 0 {
        Direction::West
    } else {
        Direction::East
    };
    let y = if delta.y() < 0 {
        Direction::Down
    } else {
        Direction::Up
    };
    let z = if delta.z() < 0 {
        Direction::North
    } else {
        Direction::South
    };
    if delta.x() == 0 {
        unobstructed(level, from, y) || unobstructed(level, from, z)
    } else if delta.y() == 0 {
        unobstructed(level, from, x) || unobstructed(level, from, z)
    } else {
        unobstructed(level, from, x) || unobstructed(level, from, y)
    }
}

fn valid_movement_pos(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    random: &mut dyn RandomSource,
) -> Option<BlockPos> {
    let mut chosen = pos;
    for offset in shuffled(&non_corner_neighbours(), random) {
        let candidate = pos.offset(offset.x(), offset.y(), offset.z());
        let state = match level.get_block(candidate) {
            Some(state) => state,
            None => continue,
        };
        if matches!(
            state.name.as_str(),
            "minecraft:sculk" | "minecraft:sculk_vein"
        ) && movement_unobstructed(level, pos, candidate)
        {
            chosen = candidate;
            if has_substrate_access(level, state, candidate) {
                break;
            }
        }
    }
    (chosen != pos).then_some(chosen)
}

struct Cursor {
    pos: BlockPos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    facings: Option<Vec<Direction>>,
}

impl Cursor {
    fn new(pos: BlockPos, charge: i32) -> Self {
        Self {
            pos,
            charge,
            update_delay: 0,
            decay_delay: 1,
            facings: None,
        }
    }
}

struct Spreader {
    cursors: Vec<Cursor>,
    replaceable: &'static str,
    growth_spawn_cost: i32,
    no_growth_radius: i32,
    charge_decay_rate: i32,
    additional_decay_rate: i32,
    worldgen: bool,
}

impl Spreader {
    fn new_worldgen() -> Self {
        Self {
            cursors: Vec::new(),
            replaceable: "minecraft:sculk_replaceable_world_gen",
            growth_spawn_cost: 50,
            no_growth_radius: 1,
            charge_decay_rate: 5,
            additional_decay_rate: 10,
            worldgen: true,
        }
    }

    fn add_cursors(&mut self, pos: BlockPos, mut charge: i32) {
        while charge > 0 {
            let amount = charge.min(1000);
            if self.cursors.len() < 32 {
                self.cursors.push(Cursor::new(pos, amount));
            }
            charge -= amount;
        }
    }

    fn clear(&mut self) {
        self.cursors.clear();
    }

    fn update_cursors(
        &mut self,
        level: &mut dyn FeatureLevel,
        root: BlockPos,
        random: &mut dyn RandomSource,
        should_convert: bool,
    ) {
        let mut next = Vec::with_capacity(self.cursors.len());
        for mut cursor in std::mem::take(&mut self.cursors) {
            if chessboard_distance(cursor.pos, root) > 1024 {
                continue;
            }
            self.update_cursor(&mut cursor, level, root, random, should_convert);
            if cursor.charge > 0 {
                next.push(cursor);
            }
        }
        self.cursors = next;
    }

    fn update_cursor(
        &mut self,
        cursor: &mut Cursor,
        level: &mut dyn FeatureLevel,
        root: BlockPos,
        random: &mut dyn RandomSource,
        should_convert: bool,
    ) {
        if cursor.charge <= 0 {
            return;
        }
        if cursor.update_delay > 0 {
            cursor.update_delay -= 1;
            return;
        }

        let mut current = level
            .get_block(cursor.pos)
            .cloned()
            .unwrap_or_else(|| state("minecraft:air"));
        let mut behavior = Behavior::of(&current);
        if should_convert
            && attempt_spread_vein(level, cursor.pos, &current, cursor.facings.as_deref())
            && behavior.can_change_block_state_on_spread()
        {
            current = level
                .get_block(cursor.pos)
                .cloned()
                .unwrap_or_else(|| state("minecraft:air"));
            behavior = Behavior::of(&current);
        }

        cursor.charge = self.use_charge(cursor, level, root, random, should_convert);
        if cursor.charge <= 0 {
            on_discharged_vein(level, &current, cursor.pos);
            return;
        }

        if let Some(position) = valid_movement_pos(level, cursor.pos, random) {
            on_discharged_vein(level, &current, cursor.pos);
            cursor.pos = position;
            if self.worldgen && !closer_than(cursor.pos, root, 15) {
                cursor.charge = 0;
                return;
            }
            current = level
                .get_block(cursor.pos)
                .cloned()
                .unwrap_or_else(|| state("minecraft:air"));
        }

        if current.name == "minecraft:sculk_vein" {
            cursor.facings = Some(available_faces(&current));
        }
        cursor.decay_delay = behavior.update_decay_delay(cursor.decay_delay);
        cursor.update_delay = behavior.sculk_spread_delay();
    }

    fn use_charge(
        &mut self,
        cursor: &mut Cursor,
        level: &mut dyn FeatureLevel,
        root: BlockPos,
        random: &mut dyn RandomSource,
        should_convert: bool,
    ) -> i32 {
        let current = level
            .get_block(cursor.pos)
            .cloned()
            .unwrap_or_else(|| state("minecraft:air"));
        match Behavior::of(&current) {
            Behavior::Default => {
                if cursor.decay_delay > 0 {
                    cursor.charge
                } else {
                    0
                }
            }
            Behavior::Sculk => {
                let charge = cursor.charge;
                if charge == 0 || random.next_int_bound(self.charge_decay_rate) != 0 {
                    return charge;
                }
                let position = cursor.pos;
                let near = closer_than(position, root, self.no_growth_radius);
                if !near && can_place_growth(level, position) {
                    if random.next_int_bound(self.growth_spawn_cost) < charge {
                        let above = position.above();
                        let growth = growth_state(level, above, random, self.worldgen);
                        level.set_block(above, growth);
                    }
                    return (charge - self.growth_spawn_cost).max(0);
                }
                if random.next_int_bound(self.additional_decay_rate) != 0 {
                    charge
                } else {
                    charge
                        - if near {
                            1
                        } else {
                            decay_penalty(self.no_growth_radius, position, root, charge)
                        }
                }
            }
            Behavior::Vein => {
                if should_convert && self.attempt_place_sculk(cursor.pos, level, random) {
                    return cursor.charge - 1;
                }
                if random.next_int_bound(self.charge_decay_rate) == 0 {
                    (cursor.charge as f32 * 0.5) as i32
                } else {
                    cursor.charge
                }
            }
        }
    }

    fn attempt_place_sculk(
        &self,
        position: BlockPos,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
    ) -> bool {
        let current = level
            .get_block(position)
            .cloned()
            .unwrap_or_else(|| state("minecraft:air"));
        for direction in shuffled(&Direction::ALL, random) {
            if !has_face(&current, direction) {
                continue;
            }
            let target = position.relative(direction);
            if !block_name(level, target)
                .is_some_and(|name| crate::tags::block_has(self.replaceable, name))
            {
                continue;
            }
            let sculk = state("minecraft:sculk");
            level.set_block(target, sculk.clone());
            vein_spread_all(level, target, &sculk);
            let opposite = direction.opposite();
            for side in Direction::ALL {
                if side == opposite {
                    continue;
                }
                let neighbor = target.relative(side);
                if block_name(level, neighbor) == Some("minecraft:sculk_vein") {
                    let neighbor_state = level
                        .get_block(neighbor)
                        .cloned()
                        .unwrap_or_else(|| state("minecraft:air"));
                    on_discharged_vein(level, &neighbor_state, neighbor);
                }
            }
            return true;
        }
        false
    }
}

/// `minecraft:sculk_patch`.
#[derive(Debug, Clone)]
pub struct SculkPatchFeature {
    charge_count: i32,
    amount_per_charge: i32,
    spread_attempts: i32,
    growth_rounds: i32,
    spread_rounds: i32,
    extra_rare_growths: IntProvider,
    catalyst_chance: f32,
}

pub(crate) fn parse_sculk_patch_config(config: &Value) -> io::Result<Option<SculkPatchFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("sculk_patch config must be an object"))?;
    let int = |key: &str| -> io::Result<i32> {
        i32::try_from(
            object
                .get(key)
                .and_then(Value::as_i64)
                .ok_or_else(|| invalid(format!("sculk_patch is missing `{key}`")))?,
        )
        .map_err(|_| invalid(format!("sculk_patch `{key}` does not fit in i32")))
    };
    let extra = object
        .get("extra_rare_growths")
        .ok_or_else(|| invalid("sculk_patch is missing `extra_rare_growths`"))?;
    let extra_rare_growths = parse_int_provider_supported(extra)?;
    Ok(Some(SculkPatchFeature {
        charge_count: int("charge_count")?,
        amount_per_charge: int("amount_per_charge")?,
        spread_attempts: int("spread_attempts")?,
        growth_rounds: int("growth_rounds")?,
        spread_rounds: int("spread_rounds")?,
        extra_rare_growths,
        catalyst_chance: object
            .get("catalyst_chance")
            .and_then(Value::as_f64)
            .ok_or_else(|| invalid("sculk_patch is missing `catalyst_chance`"))?
            as f32,
    }))
}

impl SculkPatchFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let can_spread = level.get_block(origin).is_some_and(|state| {
            matches!(
                state.name.as_str(),
                "minecraft:sculk" | "minecraft:sculk_vein"
            ) || ((is_air_name(&state.name) || water_source(state))
                && Direction::ALL.into_iter().any(|direction| {
                    level
                        .get_block(origin.relative(direction))
                        .is_some_and(full_collision)
                }))
        });
        if !can_spread {
            return false;
        }

        let mut spreader = Spreader::new_worldgen();
        let rounds = self.spread_rounds + self.growth_rounds;
        for round in 0..rounds {
            for _ in 0..self.charge_count {
                spreader.add_cursors(origin, self.amount_per_charge);
            }
            let convert = round < self.spread_rounds;
            for _ in 0..self.spread_attempts {
                spreader.update_cursors(level, origin, random, convert);
            }
            spreader.clear();
        }

        let below = origin.below();
        if random.next_float() <= self.catalyst_chance
            && level.get_block(below).is_some_and(full_collision)
        {
            level.set_block(
                origin,
                state_with("minecraft:sculk_catalyst", &[("bloom", "false")]),
            );
            level.set_block_entity(
                origin,
                crate::nbt::Tag::Compound(vec![
                    (
                        "id".to_owned(),
                        crate::nbt::Tag::String("minecraft:sculk_catalyst".to_owned()),
                    ),
                    ("x".to_owned(), crate::nbt::Tag::Int(origin.x())),
                    ("y".to_owned(), crate::nbt::Tag::Int(origin.y())),
                    ("z".to_owned(), crate::nbt::Tag::Int(origin.z())),
                ]),
            );
        }

        let rare = self
            .extra_rare_growths
            .sample(random)
            .expect("int provider is supported");
        for _ in 0..rare {
            let pos = origin.offset(
                random.next_int_bound(5) - 2,
                0,
                random.next_int_bound(5) - 2,
            );
            if empty(level, pos) && sturdy(level, pos.below()) {
                level.set_block(
                    pos,
                    state_with(
                        "minecraft:sculk_shrieker",
                        &[
                            ("can_summon", "true"),
                            ("shrieking", "false"),
                            ("waterlogged", "false"),
                        ],
                    ),
                );
            }
        }
        true
    }
}
