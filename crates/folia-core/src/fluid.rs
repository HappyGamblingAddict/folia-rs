//! Water/lava fluid states and scheduled liquid flow.
//!
//! Reference: Folia 1.21.11 `FlowingFluid`, `WaterFluid`, `LavaFluid`, and
//! `LiquidBlock`.
//!
//! The Rust block model stores a liquid as `minecraft:water` / `minecraft:lava`
//! with the vanilla `level` property (`0` source, `1..=7` flowing, `8+`
//! falling); waterlogged blocks carry `waterlogged=true` and read as a water
//! source. [`FluidState`] is the amount/falling model the algorithms use.
//!
//! Implemented: source detection, drop-off, falling priority, the slope search
//! that steers flow toward holes, source conversion (water only, matching the
//! default `waterSourceConversion` game rule), waterlogged placement, and the
//! vanilla 5/30-tick water/lava delays.
//!
//! Not implemented: block destruction drops, lava fire spread, water/lava
//! quenching to stone/obsidian/cobblestone, and the exact `CollisionShape`
//! merged-face occlusion test (a full-cube/occlusion approximation is used).

use crate::block_behaviour;
use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::tick::{BlockBehavior, TickWorld};
use std::io;

/// Vanilla `Direction.Plane.HORIZONTAL` order.
const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

/// Vanilla `EnumMap<Direction, …>` iteration order for the horizontal subset.
const SPREAD_ORDER: [Direction; 4] = [
    Direction::North,
    Direction::South,
    Direction::West,
    Direction::East,
];

/// The two flowing fluid families this slice models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FluidKind {
    Water,
    Lava,
}

impl FluidKind {
    /// The block name a non-waterlogged fluid is stored as.
    pub const fn block_name(self) -> &'static str {
        match self {
            Self::Water => "minecraft:water",
            Self::Lava => "minecraft:lava",
        }
    }

    /// `FlowingFluid.getTickDelay` in the overworld (lava is not "fast lava").
    pub const fn tick_delay(self) -> u64 {
        match self {
            Self::Water => 5,
            Self::Lava => 30,
        }
    }

    /// `FlowingFluid.getDropOff`.
    pub const fn drop_off(self) -> u8 {
        match self {
            Self::Water => 1,
            Self::Lava => 2,
        }
    }

    /// `FlowingFluid.getSlopeFindDistance`.
    pub const fn slope_find_distance(self) -> i32 {
        match self {
            Self::Water => 4,
            Self::Lava => 2,
        }
    }

    /// Vanilla default game rules: water converts, lava does not.
    pub const fn can_convert_to_source(self) -> bool {
        matches!(self, Self::Water)
    }
}

/// A fluid amount/falling pair.
///
/// Amount is `1..=8`; a source is amount 8 with `falling == false`. The legacy
/// block level is derived with `FlowingFluid.getLegacyLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidState {
    pub kind: FluidKind,
    pub amount: u8,
    pub falling: bool,
}

impl FluidState {
    /// A source fluid state (`getSource(false)`).
    pub const fn source(kind: FluidKind) -> Self {
        Self {
            kind,
            amount: 8,
            falling: false,
        }
    }

    /// `getFlowing(level, falling)`, returning `None` for an empty amount.
    fn flowing_of(kind: FluidKind, amount: u8, falling: bool) -> Option<Self> {
        if amount == 0 {
            None
        } else {
            Some(Self {
                kind,
                amount: amount.min(8),
                falling,
            })
        }
    }

    /// `FluidState.isSource`.
    pub const fn is_source(self) -> bool {
        self.amount >= 8 && !self.falling
    }

    /// `FluidState.getOwnHeight` (`amount / 9`).
    pub fn own_height(self) -> f32 {
        f32::from(self.amount.min(9)) / 9.0
    }

    /// `FlowingFluid.getLegacyLevel`.
    pub fn legacy_level(self) -> u8 {
        if self.is_source() {
            0
        } else if self.falling {
            8
        } else {
            8u8.saturating_sub(self.amount)
        }
    }

    /// The `minecraft:water`/`minecraft:lava` block state carrying this fluid.
    pub fn to_block(self) -> BlockState {
        BlockState {
            name: self.kind.block_name().to_owned(),
            properties: vec![("level".to_owned(), self.legacy_level().to_string())],
        }
    }
}

/// Reads a fluid state from a block state, including waterlogged blocks.
pub fn fluid_state(state: &BlockState) -> Option<FluidState> {
    let kind = kind_of(state)?;
    let level = if state.name == kind.block_name() {
        property(state, "level")
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0)
    } else {
        // Waterlogged blocks expose a full water source (`Fluids.WATER`).
        0
    };
    Some(if level == 0 {
        FluidState::source(kind)
    } else if level <= 7 {
        FluidState::flowing_of(kind, 8 - level, false).expect("level 1..=7 is non-empty")
    } else {
        FluidState::flowing_of(kind, 8, true).expect("falling is non-empty")
    })
}

/// Returns the fluid family carried by a state, if any.
pub fn kind_of(state: &BlockState) -> Option<FluidKind> {
    match state.name.as_str() {
        "minecraft:water" => Some(FluidKind::Water),
        "minecraft:lava" => Some(FluidKind::Lava),
        _ => match block_behaviour::fluid(state)? {
            "minecraft:water" | "minecraft:flowing_water" => Some(FluidKind::Water),
            "minecraft:lava" | "minecraft:flowing_lava" => Some(FluidKind::Lava),
            _ => None,
        },
    }
}

/// Whether the state is a waterlogged non-liquid block.
pub fn is_waterlogged(state: &BlockState) -> bool {
    property(state, "waterlogged") == Some("true")
}

/// `FlowingFluid.canHoldAnyFluid`: a liquid container, or a non-motion block
/// that is not one of the vanilla "holds no fluid" shapes.
fn can_hold_any_fluid(state: &BlockState) -> bool {
    if property(state, "waterlogged").is_some() {
        return true;
    }
    if block_behaviour::blocks_motion(state) {
        return false;
    }
    let name = state.name.as_str();
    if name.ends_with("_door")
        || name.ends_with("_sign")
        || name.ends_with("_wall_sign")
        || name.ends_with("_hanging_sign")
        || name.ends_with("_wall_hanging_sign")
    {
        return false;
    }
    !matches!(
        name,
        "minecraft:ladder"
            | "minecraft:sugar_cane"
            | "minecraft:bubble_column"
            | "minecraft:nether_portal"
            | "minecraft:end_portal"
            | "minecraft:end_gateway"
            | "minecraft:structure_void"
    )
}

/// `FlowingFluid.canHoldSpecificFluid` for the shipped liquid containers.
fn can_hold_specific_fluid(state: &BlockState, kind: FluidKind) -> bool {
    match property(state, "waterlogged") {
        Some(value) => kind == FluidKind::Water && value != "true",
        None => true,
    }
}

/// `FlowingFluid.canHoldFluid`.
fn can_hold_fluid(state: &BlockState, kind: FluidKind) -> bool {
    can_hold_any_fluid(state) && can_hold_specific_fluid(state, kind)
}

/// `FlowingFluid.canPassThroughWall`.
///
/// Vanilla compares merged collision-shape faces. This slice uses the dumped
/// full-cube metadata: two collision-less states pass, any fully occluding
/// state blocks, and two partial collision states are conservatively blocked.
fn can_pass_through_wall(from: &BlockState, to: &BlockState) -> bool {
    let from_collision = block_behaviour::has_collision(from);
    let to_collision = block_behaviour::has_collision(to);
    if !from_collision && !to_collision {
        return true;
    }
    if block_behaviour::occludes(from) || block_behaviour::occludes(to) {
        return false;
    }
    !(block_behaviour::blocks_motion(from) && block_behaviour::blocks_motion(to))
}

fn is_source_of_kind(fluid: Option<FluidState>, kind: FluidKind) -> bool {
    fluid.is_some_and(|fluid| fluid.kind == kind && fluid.is_source())
}

fn can_maybe_pass_through(
    kind: FluidKind,
    from: &BlockState,
    to: &BlockState,
    to_fluid: Option<FluidState>,
) -> bool {
    !is_source_of_kind(to_fluid, kind) && can_hold_any_fluid(to) && can_pass_through_wall(from, to)
}

/// `FlowingFluid.isWaterHole`.
fn is_water_hole(below_state: &BlockState, from: &BlockState, kind: FluidKind) -> bool {
    if !can_pass_through_wall(from, below_state) {
        return false;
    }
    match fluid_state(below_state) {
        Some(fluid) if fluid.kind == kind => true,
        _ => can_hold_fluid(below_state, kind),
    }
}

fn source_neighbor_count(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    kind: FluidKind,
) -> io::Result<i32> {
    let mut count = 0;
    for direction in HORIZONTAL {
        let Some(state) = world.block_state(pos.relative(direction))? else {
            continue;
        };
        if is_source_of_kind(fluid_state(&state), kind) {
            count += 1;
        }
    }
    Ok(count)
}

/// `FlowingFluid.getNewLiquid`.
fn get_new_liquid(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    kind: FluidKind,
) -> io::Result<Option<FluidState>> {
    let mut max_amount = 0u8;
    let mut sources = 0;
    for direction in HORIZONTAL {
        let Some(neighbor_state) = world.block_state(pos.relative(direction))? else {
            continue;
        };
        let Some(neighbor_fluid) = fluid_state(&neighbor_state) else {
            continue;
        };
        if neighbor_fluid.kind == kind && can_pass_through_wall(state, &neighbor_state) {
            if neighbor_fluid.is_source() {
                sources += 1;
            }
            max_amount = max_amount.max(neighbor_fluid.amount);
        }
    }

    if sources >= 2 && kind.can_convert_to_source() {
        let below = world.block_state(pos.below())?;
        let below_solid = below.as_ref().is_some_and(block_behaviour::is_solid);
        let below_source = below
            .as_ref()
            .and_then(fluid_state)
            .is_some_and(|fluid| fluid.kind == kind && fluid.is_source());
        if below_solid || below_source {
            return Ok(Some(FluidState::source(kind)));
        }
    }

    if let Some(above_state) = world.block_state(pos.above())?
        && let Some(above_fluid) = fluid_state(&above_state)
        && above_fluid.kind == kind
        && can_pass_through_wall(state, &above_state)
    {
        return Ok(FluidState::flowing_of(kind, 8, true));
    }

    let amount = max_amount.saturating_sub(kind.drop_off());
    Ok(FluidState::flowing_of(kind, amount, false))
}

/// `FlowingFluid.getSlopeDistance`: depth-first search for a water hole within
/// `slopeFindDistance` horizontal steps.
fn slope_distance(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    depth: i32,
    from: Direction,
    state: &BlockState,
    kind: FluidKind,
) -> io::Result<i32> {
    let mut best = 1000;
    for direction in HORIZONTAL {
        if direction == from {
            continue;
        }
        let spread_pos = pos.relative(direction);
        let Some(spread_state) = world.block_state(spread_pos)? else {
            continue;
        };
        let spread_fluid = fluid_state(&spread_state);
        if !can_maybe_pass_through(kind, state, &spread_state, spread_fluid)
            || !can_hold_specific_fluid(&spread_state, kind)
        {
            continue;
        }
        if is_water_hole(&spread_state, state, kind) {
            return Ok(depth);
        }
        if depth < kind.slope_find_distance() {
            let distance = slope_distance(
                world,
                spread_pos,
                depth + 1,
                direction.opposite(),
                &spread_state,
                kind,
            )?;
            best = best.min(distance);
        }
    }
    Ok(best)
}

/// `FluidState.canBeReplacedWith` for the shipped fluid types.
fn can_be_replaced_with(
    existing: Option<FluidState>,
    new_kind: FluidKind,
    direction: Direction,
) -> bool {
    match existing {
        None => true,
        Some(fluid) if fluid.kind == new_kind => false,
        Some(fluid) if fluid.kind == FluidKind::Water => {
            direction == Direction::Down && new_kind != FluidKind::Water
        }
        Some(fluid) => fluid.own_height() >= 0.444_444_45 && new_kind == FluidKind::Water,
    }
}

/// `FlowingFluid.getSpread`.
fn get_spread(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    kind: FluidKind,
) -> io::Result<Vec<(Direction, FluidState)>> {
    let mut best = 1000;
    let mut spread = Vec::with_capacity(4);
    for direction in HORIZONTAL {
        let spread_pos = pos.relative(direction);
        let Some(spread_state) = world.block_state(spread_pos)? else {
            continue;
        };
        let spread_fluid = fluid_state(&spread_state);
        if !can_maybe_pass_through(kind, state, &spread_state, spread_fluid) {
            continue;
        }
        let Some(new_liquid) = get_new_liquid(world, spread_pos, &spread_state, kind)? else {
            continue;
        };
        if !can_hold_specific_fluid(&spread_state, new_liquid.kind) {
            continue;
        }
        let distance = if is_water_hole(&spread_state, state, kind) {
            0
        } else {
            slope_distance(
                world,
                spread_pos,
                1,
                direction.opposite(),
                &spread_state,
                kind,
            )?
        };
        if distance < best {
            spread.clear();
        }
        if distance <= best {
            if can_be_replaced_with(spread_fluid, new_liquid.kind, direction) {
                spread.push((direction, new_liquid));
            }
            best = distance;
        }
    }
    Ok(spread)
}

/// `FlowingFluid.spreadTo`, including waterlogging the target block.
fn spread_to(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    fluid: FluidState,
) -> io::Result<()> {
    if property(state, "waterlogged").is_some() {
        if fluid.kind == FluidKind::Water {
            let mut waterlogged = state.clone();
            set_property(&mut waterlogged, "waterlogged", "true");
            world.set_block(pos, waterlogged)?;
        }
        return Ok(());
    }
    world.set_block(pos, fluid.to_block())?;
    Ok(())
}

/// `FlowingFluid.spreadToSides`.
fn spread_to_sides(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    fluid: FluidState,
) -> io::Result<()> {
    let mut amount = i32::from(fluid.amount) - i32::from(fluid.kind.drop_off());
    if fluid.falling {
        amount = 7;
    }
    if amount <= 0 {
        return Ok(());
    }
    let spread = get_spread(world, pos, state, fluid.kind)?;
    for direction in SPREAD_ORDER {
        let Some((_, new_fluid)) = spread.iter().find(|(side, _)| *side == direction) else {
            continue;
        };
        let spread_pos = pos.relative(direction);
        let Some(spread_state) = world.block_state(spread_pos)? else {
            continue;
        };
        spread_to(world, spread_pos, &spread_state, *new_fluid)?;
    }
    Ok(())
}

/// `FlowingFluid.spread`: down wins, then sideways.
fn spread(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    fluid: FluidState,
) -> io::Result<()> {
    let below_pos = pos.below();
    if let Some(below_state) = world.block_state(below_pos)? {
        let below_fluid = fluid_state(&below_state);
        if can_maybe_pass_through(fluid.kind, state, &below_state, below_fluid)
            && let Some(new_liquid) = get_new_liquid(world, below_pos, &below_state, fluid.kind)?
            && can_be_replaced_with(below_fluid, new_liquid.kind, Direction::Down)
            && can_hold_specific_fluid(&below_state, new_liquid.kind)
        {
            spread_to(world, below_pos, &below_state, new_liquid)?;
            if source_neighbor_count(world, pos, fluid.kind)? >= 3 {
                spread_to_sides(world, pos, state, fluid)?;
            }
            return Ok(());
        }

        if fluid.is_source() || !is_water_hole(&below_state, state, fluid.kind) {
            spread_to_sides(world, pos, state, fluid)?;
        }
        return Ok(());
    }
    Ok(())
}

fn empty_replacement(state: &BlockState) -> BlockState {
    if property(state, "waterlogged").is_some() {
        let mut emptied = state.clone();
        set_property(&mut emptied, "waterlogged", "false");
        emptied
    } else {
        air()
    }
}

/// Behaviour of every liquid and waterlogged block. Registered from
/// [`crate::tick::behavior_for`].
pub struct FluidBehavior;

/// The shared fluid behaviour.
pub static FLUID: FluidBehavior = FluidBehavior;

impl BlockBehavior for FluidBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if old.is_some_and(|old| old.name == state.name && old.properties == state.properties) {
            return Ok(());
        }
        let Some(fluid) = fluid_state(state) else {
            return Ok(());
        };
        world.schedule(pos, state.name.clone(), fluid.kind.tick_delay());
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        let Some(mut fluid) = fluid_state(state) else {
            return Ok(());
        };
        let mut current = state.clone();
        if !fluid.is_source() {
            let new_liquid = get_new_liquid(world, pos, &current, fluid.kind)?;
            match new_liquid {
                None => {
                    let replacement = empty_replacement(&current);
                    world.set_block(pos, replacement)?;
                    return Ok(());
                }
                Some(new_liquid) if new_liquid != fluid => {
                    let new_state = new_liquid.to_block();
                    world.set_block(pos, new_state.clone())?;
                    world.schedule(pos, new_state.name.clone(), new_liquid.kind.tick_delay());
                    fluid = new_liquid;
                    current = new_state;
                }
                Some(_) => {}
            }
        }
        spread(world, pos, &current, fluid)
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        let Some(fluid) = fluid_state(state) else {
            return Ok(());
        };
        world.schedule(pos, state.name.clone(), fluid.kind.tick_delay());
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        _direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        let Some(own) = fluid_state(state) else {
            return Ok(None);
        };
        let neighbor_source = fluid_state(neighbor).is_some_and(FluidState::is_source);
        if own.is_source() || neighbor_source {
            world.schedule(pos, state.name.clone(), own.kind.tick_delay());
        }
        Ok(None)
    }
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    }
}

/// Finds a property value on a block state.
pub fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn set_property(state: &mut BlockState, key: &str, value: &str) {
    if let Some((_, current)) = state.properties.iter_mut().find(|(name, _)| name == key) {
        *current = value.to_owned();
    } else {
        state.properties.push((key.to_owned(), value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    #[test]
    fn legacy_levels_match_liquid_block_state_cache() {
        let source = fluid_state(&state("minecraft:water", &[("level", "0")])).unwrap();
        assert!(source.is_source());
        assert_eq!(source.legacy_level(), 0);

        let flowing = fluid_state(&state("minecraft:water", &[("level", "1")])).unwrap();
        assert!(!flowing.is_source());
        assert_eq!(flowing.amount, 7);
        assert_eq!(flowing.legacy_level(), 1);

        let falling = fluid_state(&state("minecraft:lava", &[("level", "8")])).unwrap();
        assert!(!falling.is_source());
        assert!(falling.falling);
        assert_eq!(falling.amount, 8);
        assert_eq!(falling.legacy_level(), 8);

        assert_eq!(
            FluidState::source(FluidKind::Lava).to_block().name,
            "minecraft:lava"
        );
    }

    #[test]
    fn waterlogged_blocks_are_water_sources() {
        let waterlogged = state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "true")],
        );
        let fluid = fluid_state(&waterlogged).unwrap();
        assert_eq!(fluid.kind, FluidKind::Water);
        assert!(fluid.is_source());

        let dry = state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        assert!(fluid_state(&dry).is_none());
        assert!(can_hold_any_fluid(&dry));
    }

    #[test]
    fn drop_off_delay_and_slope_search_match_vanilla() {
        assert_eq!(FluidKind::Water.tick_delay(), 5);
        assert_eq!(FluidKind::Lava.tick_delay(), 30);
        assert_eq!(FluidKind::Water.drop_off(), 1);
        assert_eq!(FluidKind::Lava.drop_off(), 2);
        assert_eq!(FluidKind::Water.slope_find_distance(), 4);
        assert_eq!(FluidKind::Lava.slope_find_distance(), 2);
        assert!(FluidKind::Water.can_convert_to_source());
        assert!(!FluidKind::Lava.can_convert_to_source());
    }

    #[test]
    fn replacement_rules_follow_water_and_lava() {
        assert!(can_be_replaced_with(
            None,
            FluidKind::Water,
            Direction::North
        ));
        assert!(!can_be_replaced_with(
            Some(FluidState::source(FluidKind::Water)),
            FluidKind::Water,
            Direction::North
        ));
        assert!(can_be_replaced_with(
            Some(FluidState::source(FluidKind::Water)),
            FluidKind::Lava,
            Direction::Down
        ));
        assert!(!can_be_replaced_with(
            Some(FluidState::source(FluidKind::Water)),
            FluidKind::Lava,
            Direction::North
        ));
        assert!(can_be_replaced_with(
            Some(FluidState::source(FluidKind::Lava)),
            FluidKind::Water,
            Direction::North
        ));
    }

    #[test]
    fn containers_and_solid_faces_are_classified() {
        assert!(can_hold_any_fluid(&state("minecraft:air", &[])));
        assert!(can_hold_any_fluid(&state(
            "minecraft:water",
            &[("level", "0")]
        )));
        assert!(!can_hold_any_fluid(&state("minecraft:stone", &[])));
        assert!(!can_hold_any_fluid(&state("minecraft:ladder", &[])));
        assert!(can_pass_through_wall(
            &state("minecraft:water", &[("level", "0")]),
            &state("minecraft:air", &[])
        ));
        assert!(!can_pass_through_wall(
            &state("minecraft:water", &[("level", "0")]),
            &state("minecraft:stone", &[])
        ));
    }
}
