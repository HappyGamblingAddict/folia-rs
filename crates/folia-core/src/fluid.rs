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
//! default `waterSourceConversion` game rule), waterlogged placement, the
//! vanilla 5/30-tick water/lava delays, and the interaction rules below.
//!
//! # Fluid interactions
//!
//! * Lava/water contact follows `LiquidBlock.shouldSpreadLiquid` and
//!   `LavaFluid.spreadTo`: a lava source next to water (above or on a
//!   horizontal face) becomes obsidian, flowing lava becomes cobblestone, and
//!   lava flowing down into water becomes stone. Water below a lava source is
//!   deliberately not one of `LiquidBlock`'s five checked faces.
//! * Lava random ticks ignite flammable neighbours using the ported
//!   `FireBlock.bootStrap` odds. [`FireBehavior`] ages fire, burns out
//!   flammable neighbours, and re-places spreading fire; [`interaction_table`]
//!   registers it for callers that run the tick loop with an explicit table.
//! * Water source blocks over soul sand or magma create bubble columns
//!   (`drag=false`/up for soul sand, `drag=true`/down for magma) and the column
//!   propagates through the water source column above. The per-tick velocity
//!   update for entities is exposed through [`bubble_column_velocity`].
//! * Flowing fluid replaces (destroys) replaceable plants, torches, and fire.
//!   `beforeDestroyingBlock` loot drops are not modelled because this port has
//!   no item-entity/drop pipeline yet.
//!
//! Not implemented: fluid destruction loot drops, the exact `CollisionShape`
//! merged-face occlusion test (a full-cube/occlusion approximation is used),
//! fire weather/difficulty/player-radius gating, and TNT priming by fire.

use crate::block_behaviour;
use crate::chunk::{BlockState, is_air_state};
use crate::math::{BlockPos, Direction};
use crate::random::RandomSource;
use crate::registry;
use crate::tags;
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

/// `LiquidBlock.POSSIBLE_FLOW_DIRECTIONS`; each entry is reversed before the
/// water check, so Java inspects up, north, south, west, east in that order.
const LAVA_CONTACT_DIRECTIONS: [Direction; 5] = [
    Direction::Down,
    Direction::South,
    Direction::North,
    Direction::East,
    Direction::West,
];

/// `BubbleColumnBlock.CHECK_PERIOD`.
const BUBBLE_COLUMN_CHECK_DELAY: u64 = 5;

/// Paper's default `fireTickDelay`; Java adds `random.nextInt(10)`.
const FIRE_TICK_DELAY_BASE: u64 = 1;

/// No difficulty is stored in `WorldStorage` yet. `server.properties` defaults
/// to easy, so fire spread uses id 1 until a per-world difficulty is threaded
/// through the tick path.
const DEFAULT_DIFFICULTY_ID: i32 = 1;

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

/// Whether the state is one of the two `LiquidBlock` blocks this slice stores.
pub fn is_liquid_block(state: &BlockState) -> bool {
    matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
}

/// Whether `FlowingFluid.spreadTo` would break this state before placing the
/// fluid. Vanilla calls `beforeDestroyingBlock` for every non-air block that is
/// not a `LiquidBlockContainer`; this port uses the `waterlogged` property to
/// identify containers and has no item-entity drop pipeline yet.
pub fn destroyed_by_fluid(state: &BlockState) -> bool {
    !is_air_state(state)
        && !is_liquid_block(state)
        && property(state, "waterlogged").is_none()
        && can_hold_any_fluid(state)
}

/// Whether `state` is a bubble column block.
pub fn is_bubble_column(state: &BlockState) -> bool {
    state.name == "minecraft:bubble_column"
}

/// The `drag` property of a bubble column: `true` pulls down (magma), `false`
/// pushes up (soul sand). Returns `None` for non-column states.
pub fn bubble_column_drag(state: &BlockState) -> Option<bool> {
    if !is_bubble_column(state) {
        return None;
    }
    // `BubbleColumnBlock` defaults `drag` to true.
    Some(property(state, "drag") != Some("false"))
}

/// A `minecraft:bubble_column` state with the requested drag direction.
pub fn bubble_column_state(drag_down: bool) -> BlockState {
    let mut state = block_state("minecraft:bubble_column");
    set_property(&mut state, "drag", if drag_down { "true" } else { "false" });
    state
}

/// Where the entity is relative to a bubble column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleColumnPlacement {
    /// `Entity.onInsideBubbleColumn`: the entity is inside the column block.
    Inside,
    /// `Entity.onAboveBubbleColumn`: the entity is standing on the column's
    /// surface, so it receives the stronger launch/cap.
    AboveSurface,
}

/// The vertical velocity (`Entity.getDeltaMovement().y`) after one bubble
/// column tick.
///
/// This ports `Entity.handleOnInsideBubbleColumn` and
/// `Entity.handleOnAboveBubbleColumn` exactly:
///
/// | placement | drag down | velocity |
/// |---|---|---|
/// | inside | yes | `max(-0.3, y - 0.03)` |
/// | inside | no | `min(0.7, y + 0.06)` |
/// | above | yes | `max(-0.9, y - 0.03)` |
/// | above | no | `min(1.8, y + 0.1)` |
pub fn bubble_column_velocity(
    drag_down: bool,
    placement: BubbleColumnPlacement,
    vertical_velocity: f64,
) -> f64 {
    match (drag_down, placement) {
        (true, BubbleColumnPlacement::Inside) => (-0.3f64).max(vertical_velocity - 0.03),
        (false, BubbleColumnPlacement::Inside) => 0.7f64.min(vertical_velocity + 0.06),
        (true, BubbleColumnPlacement::AboveSurface) => (-0.9f64).max(vertical_velocity - 0.03),
        (false, BubbleColumnPlacement::AboveSurface) => 1.8f64.min(vertical_velocity + 0.1),
    }
}

/// The per-tick change in vertical velocity, `bubble_column_velocity(...) -
/// vertical_velocity`.
pub fn bubble_column_velocity_delta(
    drag_down: bool,
    placement: BubbleColumnPlacement,
    vertical_velocity: f64,
) -> f64 {
    bubble_column_velocity(drag_down, placement, vertical_velocity) - vertical_velocity
}

fn can_exist_in_bubble_column(state: &BlockState) -> bool {
    is_bubble_column(state)
        || (state.name == "minecraft:water"
            && fluid_state(state).is_some_and(|fluid| fluid.is_source() && fluid.amount >= 8))
}

fn column_state_for(below: &BlockState) -> BlockState {
    if is_bubble_column(below) {
        below.clone()
    } else if below.name == "minecraft:soul_sand" {
        bubble_column_state(false)
    } else if below.name == "minecraft:magma_block" {
        bubble_column_state(true)
    } else {
        block_state("minecraft:water")
    }
}

/// Ports `BubbleColumnBlock.updateColumn`: turn `pos` and every water source
/// (or bubble column) above it into the column state derived from the block
/// below `pos`, stopping at the first block that cannot hold a column.
fn update_bubble_column(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<()> {
    let Some(fluid) = world.block_state(pos)? else {
        return Ok(());
    };
    if !can_exist_in_bubble_column(&fluid) {
        return Ok(());
    }
    let Some(below) = world.block_state(pos.below())? else {
        return Ok(());
    };
    let column = column_state_for(&below);
    set_bubble_column(world, pos, &column)
}

fn set_bubble_column(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    column: &BlockState,
) -> io::Result<()> {
    world.set_block(pos, column.clone())?;
    let mut above = pos.above();
    while let Some(current) = world.block_state(above)? {
        if !can_exist_in_bubble_column(&current) {
            break;
        }
        if !world.set_block(above, column.clone())? {
            return Ok(());
        }
        above = above.above();
    }
    Ok(())
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

/// `LiquidBlock.shouldSpreadLiquid`: whether a lava block must convert because
/// water touches one of `POSSIBLE_FLOW_DIRECTIONS` (up, north, south, west,
/// east in Java's reversed iteration order).
///
/// A source becomes obsidian and flowing lava becomes cobblestone. A soul-soil
/// lava block next to blue ice becomes basalt. Water below lava is not checked
/// here; that contact is handled by [`spread_to`] as `LavaFluid.spreadTo`
/// (lava flowing down into a water `LiquidBlock` becomes stone).
fn lava_water_replacement(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    fluid: FluidState,
) -> io::Result<Option<BlockState>> {
    if fluid.kind != FluidKind::Lava {
        return Ok(None);
    }
    let soul_soil = world
        .block_state(pos.below())?
        .is_some_and(|below| below.name == "minecraft:soul_soil");
    for direction in LAVA_CONTACT_DIRECTIONS {
        let neighbor_pos = pos.relative(direction.opposite());
        let Some(neighbor) = world.block_state(neighbor_pos)? else {
            continue;
        };
        if fluid_state(&neighbor).is_some_and(|neighbor| neighbor.kind == FluidKind::Water) {
            let replacement = if fluid.is_source() {
                block_state("minecraft:obsidian")
            } else {
                block_state("minecraft:cobblestone")
            };
            return Ok(Some(replacement));
        }
        if soul_soil && neighbor.name == "minecraft:blue_ice" {
            return Ok(Some(block_state("minecraft:basalt")));
        }
    }
    Ok(None)
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

/// `FlowingFluid.spreadTo`, including waterlogging the target block and
/// `LavaFluid.spreadTo`'s downward water quench.
fn spread_to(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    direction: Direction,
    fluid: FluidState,
) -> io::Result<()> {
    // `LavaFluid.spreadTo`: lava flowing down into a water `LiquidBlock`
    // becomes stone. Lava cannot flow horizontally into water because
    // `LavaFluid.canBeReplacedWith` only allows water to replace lava, not the
    // reverse; the horizontal contact is handled by `lava_water_replacement`
    // on the lava block itself.
    if fluid.kind == FluidKind::Lava
        && direction == Direction::Down
        && is_liquid_block(state)
        && fluid_state(state).is_some_and(|existing| existing.kind == FluidKind::Water)
    {
        world.set_block(pos, block_state("minecraft:stone"))?;
        return Ok(());
    }

    if property(state, "waterlogged").is_some() {
        if fluid.kind == FluidKind::Water {
            let mut waterlogged = state.clone();
            set_property(&mut waterlogged, "waterlogged", "true");
            world.set_block(pos, waterlogged)?;
        }
        return Ok(());
    }

    if destroyed_by_fluid(state) {
        before_destroying_block(state);
    }
    world.set_block(pos, fluid.to_block())?;
    Ok(())
}

/// Vanilla `FlowingFluid.beforeDestroyingBlock` drops the replaced block's loot.
/// Item entities and the loot pipeline are outside this slice, so the fluid
/// replacement still removes the block, but no drop is spawned.
fn before_destroying_block(_state: &BlockState) {}

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
        spread_to(world, spread_pos, &spread_state, direction, *new_fluid)?;
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
            spread_to(world, below_pos, &below_state, Direction::Down, new_liquid)?;
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
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        fluid_state(state).is_some_and(|fluid| fluid.kind == FluidKind::Lava)
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if fluid_state(state).is_some_and(|fluid| fluid.kind == FluidKind::Lava) {
            lava_random_tick(world, pos)?;
        }
        Ok(())
    }

    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if is_bubble_column(state) {
            // `BubbleColumnBlock` has no `onPlace`; keeping a self-check queued
            // makes the column settle even when it is created by a neighbour.
            world.schedule(pos, state.name.clone(), BUBBLE_COLUMN_CHECK_DELAY);
            return Ok(());
        }
        let Some(fluid) = fluid_state(state) else {
            return Ok(());
        };
        // A waterlogged non-liquid block exposes `Fluids.WATER` but does not
        // run `FlowingFluid.tick` (`LiquidBlock` owns that behaviour).
        if state.name != fluid.kind.block_name() {
            return Ok(());
        }
        if old.is_some_and(|old| old.name == state.name && old.properties == state.properties) {
            return Ok(());
        }
        if let Some(replacement) = lava_water_replacement(world, pos, fluid)? {
            world.set_block(pos, replacement)?;
            return Ok(());
        }
        world.schedule(pos, state.name.clone(), fluid.kind.tick_delay());
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if is_bubble_column(state) {
            return update_bubble_column(world, pos);
        }
        let Some(mut fluid) = fluid_state(state) else {
            return Ok(());
        };
        if state.name != fluid.kind.block_name() {
            return Ok(());
        }

        // `SoulSandBlock`/`MagmaBlock` schedule their own 20-tick column check
        // in Java. Those blocks are not registered with the tick table yet, so
        // the water source above them performs the equivalent column update
        // when its normal fluid tick runs.
        if fluid.kind == FluidKind::Water && fluid.is_source() {
            let below = world.block_state(pos.below())?;
            if below.is_some_and(|below| {
                matches!(
                    below.name.as_str(),
                    "minecraft:soul_sand" | "minecraft:magma_block"
                )
            }) {
                return update_bubble_column(world, pos);
            }
        }

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
        if is_bubble_column(state) {
            world.schedule(pos, state.name.clone(), BUBBLE_COLUMN_CHECK_DELAY);
            return Ok(());
        }
        let Some(fluid) = fluid_state(state) else {
            return Ok(());
        };
        if state.name != fluid.kind.block_name() {
            return Ok(());
        }
        if let Some(replacement) = lava_water_replacement(world, pos, fluid)? {
            world.set_block(pos, replacement)?;
            return Ok(());
        }
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
        if is_bubble_column(state) {
            world.schedule(pos, state.name.clone(), BUBBLE_COLUMN_CHECK_DELAY);
            return Ok(None);
        }
        let Some(own) = fluid_state(state) else {
            return Ok(None);
        };
        if state.name != own.kind.block_name() {
            return Ok(None);
        }
        // The behaviour was resolved before `neighbor_changed` ran; a lava
        // block may already have quenched into stone/obsidian/cobblestone.
        let Some(current) = world.block_state(pos)? else {
            return Ok(None);
        };
        if current.name != state.name || current.properties != state.properties {
            return Ok(None);
        }
        let neighbor_source = fluid_state(neighbor).is_some_and(FluidState::is_source);
        if own.is_source() || neighbor_source {
            world.schedule(pos, state.name.clone(), own.kind.tick_delay());
        }
        Ok(None)
    }
}

/// `FireBlock.bootStrap` flammability table: `(block, ignite odds, burn odds)`.
///
/// The dumped per-state behaviour table does not carry `ignitedByLava`, so the
/// same table doubles as the `ignitedByLava` predicate for lava ignition. This
/// is the exact 1.21.11 `FireBlock` registration list.
const FIRE_ODDS: &[(&str, u16, u16)] = &[
    ("minecraft:acacia_fence", 5, 20),
    ("minecraft:acacia_fence_gate", 5, 20),
    ("minecraft:acacia_leaves", 30, 60),
    ("minecraft:acacia_log", 5, 5),
    ("minecraft:acacia_planks", 5, 20),
    ("minecraft:acacia_shelf", 30, 20),
    ("minecraft:acacia_slab", 5, 20),
    ("minecraft:acacia_stairs", 5, 20),
    ("minecraft:acacia_wood", 5, 5),
    ("minecraft:allium", 60, 100),
    ("minecraft:azalea", 30, 60),
    ("minecraft:azalea_leaves", 30, 60),
    ("minecraft:azure_bluet", 60, 100),
    ("minecraft:bamboo", 60, 60),
    ("minecraft:bamboo_block", 5, 5),
    ("minecraft:bamboo_fence", 5, 20),
    ("minecraft:bamboo_fence_gate", 5, 20),
    ("minecraft:bamboo_mosaic", 5, 20),
    ("minecraft:bamboo_mosaic_slab", 5, 20),
    ("minecraft:bamboo_mosaic_stairs", 5, 20),
    ("minecraft:bamboo_planks", 5, 20),
    ("minecraft:bamboo_shelf", 30, 20),
    ("minecraft:bamboo_slab", 5, 20),
    ("minecraft:bamboo_stairs", 5, 20),
    ("minecraft:bee_nest", 30, 20),
    ("minecraft:beehive", 5, 20),
    ("minecraft:big_dripleaf", 60, 100),
    ("minecraft:big_dripleaf_stem", 60, 100),
    ("minecraft:birch_fence", 5, 20),
    ("minecraft:birch_fence_gate", 5, 20),
    ("minecraft:birch_leaves", 30, 60),
    ("minecraft:birch_log", 5, 5),
    ("minecraft:birch_planks", 5, 20),
    ("minecraft:birch_shelf", 30, 20),
    ("minecraft:birch_slab", 5, 20),
    ("minecraft:birch_stairs", 5, 20),
    ("minecraft:birch_wood", 5, 5),
    ("minecraft:black_carpet", 60, 20),
    ("minecraft:black_wool", 30, 60),
    ("minecraft:blue_carpet", 60, 20),
    ("minecraft:blue_orchid", 60, 100),
    ("minecraft:blue_wool", 30, 60),
    ("minecraft:bookshelf", 30, 20),
    ("minecraft:brown_carpet", 60, 20),
    ("minecraft:brown_wool", 30, 60),
    ("minecraft:bush", 60, 100),
    ("minecraft:cactus_flower", 60, 100),
    ("minecraft:cave_vines", 15, 60),
    ("minecraft:cave_vines_plant", 15, 60),
    ("minecraft:cherry_fence", 5, 20),
    ("minecraft:cherry_fence_gate", 5, 20),
    ("minecraft:cherry_leaves", 30, 60),
    ("minecraft:cherry_log", 5, 5),
    ("minecraft:cherry_planks", 5, 20),
    ("minecraft:cherry_shelf", 30, 20),
    ("minecraft:cherry_slab", 5, 20),
    ("minecraft:cherry_stairs", 5, 20),
    ("minecraft:cherry_wood", 5, 5),
    ("minecraft:closed_eyeblossom", 60, 100),
    ("minecraft:coal_block", 5, 5),
    ("minecraft:composter", 5, 20),
    ("minecraft:cornflower", 60, 100),
    ("minecraft:cyan_carpet", 60, 20),
    ("minecraft:cyan_wool", 30, 60),
    ("minecraft:dandelion", 60, 100),
    ("minecraft:dark_oak_fence", 5, 20),
    ("minecraft:dark_oak_fence_gate", 5, 20),
    ("minecraft:dark_oak_leaves", 30, 60),
    ("minecraft:dark_oak_log", 5, 5),
    ("minecraft:dark_oak_planks", 5, 20),
    ("minecraft:dark_oak_shelf", 30, 20),
    ("minecraft:dark_oak_slab", 5, 20),
    ("minecraft:dark_oak_stairs", 5, 20),
    ("minecraft:dark_oak_wood", 5, 5),
    ("minecraft:dead_bush", 60, 100),
    ("minecraft:dried_kelp_block", 30, 60),
    ("minecraft:fern", 60, 100),
    ("minecraft:firefly_bush", 60, 100),
    ("minecraft:flowering_azalea", 30, 60),
    ("minecraft:flowering_azalea_leaves", 30, 60),
    ("minecraft:glow_lichen", 15, 100),
    ("minecraft:gray_carpet", 60, 20),
    ("minecraft:gray_wool", 30, 60),
    ("minecraft:green_carpet", 60, 20),
    ("minecraft:green_wool", 30, 60),
    ("minecraft:hanging_roots", 30, 60),
    ("minecraft:hay_block", 60, 20),
    ("minecraft:jungle_fence", 5, 20),
    ("minecraft:jungle_fence_gate", 5, 20),
    ("minecraft:jungle_leaves", 30, 60),
    ("minecraft:jungle_log", 5, 5),
    ("minecraft:jungle_planks", 5, 20),
    ("minecraft:jungle_shelf", 30, 20),
    ("minecraft:jungle_slab", 5, 20),
    ("minecraft:jungle_stairs", 5, 20),
    ("minecraft:jungle_wood", 5, 5),
    ("minecraft:large_fern", 60, 100),
    ("minecraft:leaf_litter", 60, 100),
    ("minecraft:lectern", 30, 20),
    ("minecraft:light_blue_carpet", 60, 20),
    ("minecraft:light_blue_wool", 30, 60),
    ("minecraft:light_gray_carpet", 60, 20),
    ("minecraft:light_gray_wool", 30, 60),
    ("minecraft:lilac", 60, 100),
    ("minecraft:lily_of_the_valley", 60, 100),
    ("minecraft:lime_carpet", 60, 20),
    ("minecraft:lime_wool", 30, 60),
    ("minecraft:magenta_carpet", 60, 20),
    ("minecraft:magenta_wool", 30, 60),
    ("minecraft:mangrove_fence", 5, 20),
    ("minecraft:mangrove_fence_gate", 5, 20),
    ("minecraft:mangrove_leaves", 30, 60),
    ("minecraft:mangrove_log", 5, 5),
    ("minecraft:mangrove_planks", 5, 20),
    ("minecraft:mangrove_roots", 5, 20),
    ("minecraft:mangrove_shelf", 30, 20),
    ("minecraft:mangrove_slab", 5, 20),
    ("minecraft:mangrove_stairs", 5, 20),
    ("minecraft:mangrove_wood", 5, 5),
    ("minecraft:oak_fence", 5, 20),
    ("minecraft:oak_fence_gate", 5, 20),
    ("minecraft:oak_leaves", 30, 60),
    ("minecraft:oak_log", 5, 5),
    ("minecraft:oak_planks", 5, 20),
    ("minecraft:oak_shelf", 30, 20),
    ("minecraft:oak_slab", 5, 20),
    ("minecraft:oak_stairs", 5, 20),
    ("minecraft:oak_wood", 5, 5),
    ("minecraft:open_eyeblossom", 60, 100),
    ("minecraft:orange_carpet", 60, 20),
    ("minecraft:orange_tulip", 60, 100),
    ("minecraft:orange_wool", 30, 60),
    ("minecraft:oxeye_daisy", 60, 100),
    ("minecraft:pale_hanging_moss", 5, 100),
    ("minecraft:pale_moss_block", 5, 100),
    ("minecraft:pale_moss_carpet", 5, 100),
    ("minecraft:pale_oak_fence", 5, 20),
    ("minecraft:pale_oak_fence_gate", 5, 20),
    ("minecraft:pale_oak_leaves", 30, 60),
    ("minecraft:pale_oak_log", 5, 5),
    ("minecraft:pale_oak_planks", 5, 20),
    ("minecraft:pale_oak_shelf", 30, 20),
    ("minecraft:pale_oak_slab", 5, 20),
    ("minecraft:pale_oak_stairs", 5, 20),
    ("minecraft:pale_oak_wood", 5, 5),
    ("minecraft:peony", 60, 100),
    ("minecraft:pink_carpet", 60, 20),
    ("minecraft:pink_petals", 60, 100),
    ("minecraft:pink_tulip", 60, 100),
    ("minecraft:pink_wool", 30, 60),
    ("minecraft:pitcher_plant", 60, 100),
    ("minecraft:poppy", 60, 100),
    ("minecraft:purple_carpet", 60, 20),
    ("minecraft:purple_wool", 30, 60),
    ("minecraft:red_carpet", 60, 20),
    ("minecraft:red_tulip", 60, 100),
    ("minecraft:red_wool", 30, 60),
    ("minecraft:rose_bush", 60, 100),
    ("minecraft:scaffolding", 60, 60),
    ("minecraft:short_dry_grass", 60, 100),
    ("minecraft:short_grass", 60, 100),
    ("minecraft:small_dripleaf", 60, 100),
    ("minecraft:spore_blossom", 60, 100),
    ("minecraft:spruce_fence", 5, 20),
    ("minecraft:spruce_fence_gate", 5, 20),
    ("minecraft:spruce_leaves", 30, 60),
    ("minecraft:spruce_log", 5, 5),
    ("minecraft:spruce_planks", 5, 20),
    ("minecraft:spruce_shelf", 30, 20),
    ("minecraft:spruce_slab", 5, 20),
    ("minecraft:spruce_stairs", 5, 20),
    ("minecraft:spruce_wood", 5, 5),
    ("minecraft:stripped_acacia_log", 5, 5),
    ("minecraft:stripped_acacia_wood", 5, 5),
    ("minecraft:stripped_bamboo_block", 5, 5),
    ("minecraft:stripped_birch_log", 5, 5),
    ("minecraft:stripped_birch_wood", 5, 5),
    ("minecraft:stripped_cherry_log", 5, 5),
    ("minecraft:stripped_cherry_wood", 5, 5),
    ("minecraft:stripped_dark_oak_log", 5, 5),
    ("minecraft:stripped_dark_oak_wood", 5, 5),
    ("minecraft:stripped_jungle_log", 5, 5),
    ("minecraft:stripped_jungle_wood", 5, 5),
    ("minecraft:stripped_mangrove_log", 5, 5),
    ("minecraft:stripped_mangrove_wood", 5, 5),
    ("minecraft:stripped_oak_log", 5, 5),
    ("minecraft:stripped_oak_wood", 5, 5),
    ("minecraft:stripped_pale_oak_log", 5, 5),
    ("minecraft:stripped_pale_oak_wood", 5, 5),
    ("minecraft:stripped_spruce_log", 5, 5),
    ("minecraft:stripped_spruce_wood", 5, 5),
    ("minecraft:sunflower", 60, 100),
    ("minecraft:sweet_berry_bush", 60, 100),
    ("minecraft:tall_dry_grass", 60, 100),
    ("minecraft:tall_grass", 60, 100),
    ("minecraft:target", 15, 20),
    ("minecraft:tnt", 15, 100),
    ("minecraft:torchflower", 60, 100),
    ("minecraft:vine", 15, 100),
    ("minecraft:white_carpet", 60, 20),
    ("minecraft:white_tulip", 60, 100),
    ("minecraft:white_wool", 30, 60),
    ("minecraft:wildflowers", 60, 100),
    ("minecraft:wither_rose", 60, 100),
    ("minecraft:yellow_carpet", 60, 20),
    ("minecraft:yellow_wool", 30, 60),
];

fn fire_odds(state: &BlockState) -> (u16, u16) {
    if property(state, "waterlogged") == Some("true") {
        return (0, 0);
    }
    FIRE_ODDS
        .iter()
        .find(|(name, _, _)| *name == state.name)
        .map_or((0, 0), |(_, ignite, burn)| (*ignite, *burn))
}

/// `BlockBehaviour.BlockStateBase.ignitedByLava` approximation. The dumped
/// per-state table has no `ignitedByLava` column, so the exact `FireBlock`
/// registration list is used instead.
pub fn ignited_by_lava(state: &BlockState) -> bool {
    fire_odds(state).0 > 0
}

fn can_burn(state: &BlockState) -> bool {
    fire_odds(state).0 > 0
}

fn fire_age_of(state: &BlockState) -> u8 {
    property(state, "age")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
}

/// The `age` property of a fire block, or `None` for non-fire states.
pub fn fire_age(state: &BlockState) -> Option<u8> {
    (state.name == "minecraft:fire").then(|| fire_age_of(state))
}

fn fire_side_property(direction: Direction) -> Option<&'static str> {
    match direction {
        Direction::Down => None,
        Direction::Up => Some("up"),
        Direction::North => Some("north"),
        Direction::South => Some("south"),
        Direction::West => Some("west"),
        Direction::East => Some("east"),
    }
}

fn with_fire_age(state: &BlockState, age: u8) -> BlockState {
    let mut state = state.clone();
    if state.name == "minecraft:fire" {
        set_property(&mut state, "age", &age.to_string());
    }
    state
}

/// `FireBlock.getStateForPlacement(level, pos)`.
fn fire_placement_state(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<BlockState> {
    let below = world.block_state(pos.below())?;
    let burn_below = below.as_ref().is_some_and(can_burn);
    let sturdy_below = below.as_ref().is_some_and(block_behaviour::occludes);
    if burn_below || sturdy_below {
        return Ok(block_state("minecraft:fire"));
    }

    let mut state = block_state("minecraft:fire");
    for direction in Direction::ALL {
        let Some(property_name) = fire_side_property(direction) else {
            continue;
        };
        let flammable = world
            .block_state(pos.relative(direction))?
            .as_ref()
            .is_some_and(can_burn);
        set_property(
            &mut state,
            property_name,
            if flammable { "true" } else { "false" },
        );
    }
    Ok(state)
}

/// `BaseFireBlock.getState(level, pos)`.
fn base_fire_state(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<BlockState> {
    let soul_base = world
        .block_state(pos.below())?
        .is_some_and(|below| tags::block_has("soul_fire_base_blocks", &below.name));
    if soul_base {
        Ok(block_state("minecraft:soul_fire"))
    } else {
        fire_placement_state(world, pos)
    }
}

/// `FireBlock.getStateWithAge(level, pos, age)`.
fn fire_state_with_age(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    age: u8,
) -> io::Result<BlockState> {
    Ok(with_fire_age(&base_fire_state(world, pos)?, age))
}

fn fire_can_survive(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    _state: &BlockState,
) -> io::Result<bool> {
    let Some(below) = world.block_state(pos.below())? else {
        return Ok(false);
    };
    if block_behaviour::occludes(&below) {
        return Ok(true);
    }
    fire_is_valid_location(world, pos)
}

/// `FireBlock.isValidFireLocation`: any of the six neighbours can burn.
fn fire_is_valid_location(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    for direction in Direction::ALL {
        if world
            .block_state(pos.relative(direction))?
            .as_ref()
            .is_some_and(can_burn)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `LavaFluid.hasFlammableNeighbours`.
fn has_flammable_neighbours(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    for direction in Direction::ALL {
        if is_flammable(world, pos.relative(direction))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `LavaFluid.isFlammable`.
fn is_flammable(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    Ok(world
        .block_state(pos)?
        .as_ref()
        .is_some_and(ignited_by_lava))
}

/// `LavaFluid.randomTick`: search up to two blocks upward for an air pocket
/// with a flammable neighbour, or place fire above a flammable floor block.
fn lava_random_tick(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<()> {
    // `ServerLevel.canSpreadFireAround` needs player tracking; this slice has no
    // loaded-player set in `TickWorld`, so the `-1` (unlimited) game rule result
    // is used.
    let mut random = world.random(pos);
    let random_int = random.next_int_bound(3);
    if random_int > 0 {
        let mut fire_pos = pos;
        for _ in 0..random_int {
            fire_pos = fire_pos.offset(
                random.next_int_bound(3) - 1,
                1,
                random.next_int_bound(3) - 1,
            );
            let Some(block_state) = world.block_state(fire_pos)? else {
                return Ok(());
            };
            if is_air_state(&block_state) {
                if has_flammable_neighbours(world, fire_pos)? {
                    let fire = base_fire_state(world, fire_pos)?;
                    world.set_block(fire_pos, fire)?;
                    schedule_fire_tick(world, fire_pos, &mut random);
                    return Ok(());
                }
            } else if block_behaviour::blocks_motion(&block_state) {
                return Ok(());
            }
        }
    } else {
        for _ in 0..3 {
            let floor_pos = pos.offset(
                random.next_int_bound(3) - 1,
                0,
                random.next_int_bound(3) - 1,
            );
            if world.block_state(floor_pos)?.is_none() {
                return Ok(());
            }
            let above_is_air = world
                .block_state(floor_pos.above())?
                .is_some_and(|above| is_air_state(&above));
            if above_is_air && is_flammable(world, floor_pos)? {
                // Java computes `BaseFireBlock.getState(level, blockPos1)` at
                // the flammable support position, then writes it above.
                let fire = base_fire_state(world, floor_pos)?;
                let fire_pos = floor_pos.above();
                world.set_block(fire_pos, fire)?;
                schedule_fire_tick(world, fire_pos, &mut random);
            }
        }
    }
    Ok(())
}

fn schedule_fire_tick(world: &mut TickWorld<'_>, pos: BlockPos, random: &mut impl RandomSource) {
    let delay = FIRE_TICK_DELAY_BASE + random.next_int_bound(10) as u64;
    world.schedule(pos, "minecraft:fire", delay);
}

fn fire_ignite_odds_at(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<u16> {
    let Some(state) = world.block_state(pos)? else {
        return Ok(0);
    };
    if !is_air_state(&state) {
        return Ok(0);
    }
    let mut odds = 0;
    for direction in Direction::ALL {
        if let Some(neighbor) = world.block_state(pos.relative(direction))? {
            odds = odds.max(fire_odds(&neighbor).0);
        }
    }
    Ok(odds)
}

/// `FireBlock.checkBurnOut`.
fn check_burn_out(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    chance: i32,
    age: i32,
    random: &mut impl RandomSource,
) -> io::Result<()> {
    let Some(state) = world.block_state(pos)? else {
        return Ok(());
    };
    let burn_odds = i32::from(fire_odds(&state).1);
    if random.next_int_bound(chance) < burn_odds {
        let replace_with_fire = random.next_int_bound(age + 10) < 5;
        if state.name == "minecraft:tnt" {
            // TNT priming needs a primed-TNT entity; consume the same draws as
            // `checkBurnOut` but leave the block in place rather than faking an
            // explosion.
            return Ok(());
        }
        if replace_with_fire {
            let new_age = ((age + random.next_int_bound(5) / 4).min(15)) as u8;
            let fire = fire_state_with_age(world, pos, new_age)?;
            world.set_block(pos, fire)?;
            schedule_fire_tick(world, pos, random);
        } else {
            world.set_block(pos, air())?;
        }
    }
    Ok(())
}

/// `FireBlock.tick`: age the fire, extinguish unsupported fire, burn
/// flammable neighbours, and spread to nearby air.
///
/// Weather (`isRaining`/`isNearRain`), the increased-burnout environment
/// attribute, and the difficulty-scaled temptation formula use fixed defaults:
/// no rain, normal burnout, and `server.properties`' default easy difficulty.
pub fn fire_tick(world: &mut TickWorld<'_>, pos: BlockPos, state: &BlockState) -> io::Result<()> {
    let mut random = world.random(pos);
    let delay = FIRE_TICK_DELAY_BASE + random.next_int_bound(10) as u64;
    world.schedule(pos, "minecraft:fire", delay);

    if !fire_can_survive(world, pos, state)? {
        world.set_block(pos, air())?;
        return Ok(());
    }

    let below = world.block_state(pos.below())?;
    let infiniburn = below
        .as_ref()
        .is_some_and(|below| tags::block_has("infiniburn_overworld", &below.name));
    let age = i32::from(fire_age_of(state));

    let new_age = (age + random.next_int_bound(3) / 2).min(15);
    if new_age != age {
        world.set_block(pos, with_fire_age(state, new_age as u8))?;
    }

    if !infiniburn {
        if !fire_is_valid_location(world, pos)? {
            if !below.as_ref().is_some_and(block_behaviour::occludes) || age > 3 {
                world.set_block(pos, air())?;
            }
            return Ok(());
        }
        if age == 15 && random.next_int_bound(4) == 0 && !below.as_ref().is_some_and(can_burn) {
            world.set_block(pos, air())?;
            return Ok(());
        }
    }

    for (direction, chance) in [
        (Direction::East, 300),
        (Direction::West, 300),
        (Direction::Down, 250),
        (Direction::Up, 250),
        (Direction::North, 300),
        (Direction::South, 300),
    ] {
        check_burn_out(world, pos.relative(direction), chance, age, &mut random)?;
    }

    for offset_x in -1..=1 {
        for offset_z in -1..=1 {
            for offset_y in -1..=4 {
                if offset_x == 0 && offset_y == 0 && offset_z == 0 {
                    continue;
                }
                let target = pos.offset(offset_x, offset_y, offset_z);
                let odds = fire_ignite_odds_at(world, target)?;
                if odds == 0 {
                    continue;
                }
                let chance = 100
                    + if offset_y > 1 {
                        (offset_y - 1) * 100
                    } else {
                        0
                    };
                let spread = (i32::from(odds) + 40 + DEFAULT_DIFFICULTY_ID * 7) / (age + 30);
                if spread > 0 && random.next_int_bound(chance) <= spread {
                    let target_state = world.block_state(target)?.unwrap_or_else(air);
                    if target_state.name != "minecraft:fire" {
                        let new_age = ((age + random.next_int_bound(5) / 4).min(15)) as u8;
                        let fire = fire_state_with_age(world, target, new_age)?;
                        world.set_block(target, fire)?;
                        schedule_fire_tick(world, target, &mut random);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Behaviour of `minecraft:fire`. `FireBlock` is a scheduled-tick block, not a
/// random-tick block, so only [`Self::scheduled_tick`] drives its lifetime.
pub struct FireBehavior;

/// The shared fire behaviour.
pub static FIRE: FireBehavior = FireBehavior;

impl BlockBehavior for FireBehavior {
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
        let mut random = world.random(pos);
        schedule_fire_tick(world, pos, &mut random);
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if state.name == "minecraft:fire" {
            fire_tick(world, pos, state)
        } else {
            Ok(())
        }
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        _direction: Direction,
        _neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if !fire_can_survive(world, pos, state)? {
            return Ok(Some(air()));
        }
        Ok(Some(fire_state_with_age(world, pos, fire_age_of(state))?))
    }
}

/// A behaviour table that adds [`FIRE`] on top of the built-in fluid/redstone/
/// falling/plant/grass table.
///
/// `tick::behavior_for` cannot be edited by the fluid slice, so production
/// `tick::tick_world` still routes `minecraft:fire` through the no-op
/// behaviour. Lava ignition is live because lava itself is a fluid and uses
/// [`FluidBehavior::random_tick`]; fire aging/spread runs whenever the caller
/// ticks with this table (tests, and a future one-line registration in
/// `tick.rs`).
pub fn interaction_table() -> crate::tick::BehaviorTable {
    crate::tick::BehaviorTable::vanilla().with("minecraft:fire", &FIRE)
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    }
}

fn block_state(name: &str) -> BlockState {
    registry::default_block_state(name).unwrap_or_else(|| BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    })
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

    #[test]
    fn bubble_column_velocity_matches_entity_handlers() {
        use BubbleColumnPlacement::{AboveSurface, Inside};

        assert_eq!(bubble_column_velocity(false, Inside, 0.0), 0.06);
        assert_eq!(bubble_column_velocity(true, Inside, 0.0), -0.03);
        assert_eq!(bubble_column_velocity(false, AboveSurface, 0.0), 0.1);
        assert_eq!(bubble_column_velocity(true, AboveSurface, 0.0), -0.03);

        // Clamps.
        assert_eq!(bubble_column_velocity(false, Inside, 0.69), 0.7);
        assert_eq!(bubble_column_velocity(true, Inside, -0.29), -0.3);
        assert_eq!(bubble_column_velocity(false, AboveSurface, 1.75), 1.8);
        assert_eq!(bubble_column_velocity(true, AboveSurface, -0.9), -0.9);

        assert!((bubble_column_velocity_delta(false, AboveSurface, 0.0) - 0.1).abs() < 1e-12);
        assert!((bubble_column_velocity_delta(true, Inside, 0.0) + 0.03).abs() < 1e-12);
    }

    #[test]
    fn bubble_column_states_carry_the_java_drag_property() {
        let up = bubble_column_state(false);
        assert_eq!(up.name, "minecraft:bubble_column");
        assert_eq!(bubble_column_drag(&up), Some(false));
        assert_eq!(
            fluid_state(&up).map(|fluid| fluid.kind),
            Some(FluidKind::Water)
        );
        assert!(fluid_state(&up).is_some_and(FluidState::is_source));

        let down = bubble_column_state(true);
        assert_eq!(bubble_column_drag(&down), Some(true));
        assert_eq!(bubble_column_drag(&state("minecraft:water", &[])), None);
    }

    #[test]
    fn fluid_destruction_only_targets_replaceable_containerless_blocks() {
        let grass = registry::default_block_state("minecraft:short_grass").expect("short grass");
        assert!(destroyed_by_fluid(&grass));
        assert!(destroyed_by_fluid(&state("minecraft:fire", &[])));
        assert!(!destroyed_by_fluid(&state("minecraft:stone", &[])));
        assert!(!destroyed_by_fluid(&state(
            "minecraft:water",
            &[("level", "0")]
        )));
        assert!(!destroyed_by_fluid(&state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")]
        )));
    }

    #[test]
    fn fire_odds_match_the_bootstrapped_table() {
        assert!(ignited_by_lava(&state("minecraft:oak_planks", &[])));
        assert!(ignited_by_lava(&state("minecraft:short_grass", &[])));
        assert!(ignited_by_lava(&state("minecraft:oak_leaves", &[])));
        assert!(!ignited_by_lava(&state("minecraft:stone", &[])));
        assert!(!ignited_by_lava(&state(
            "minecraft:water",
            &[("level", "0")]
        )));
        assert!(!ignited_by_lava(&state(
            "minecraft:oak_planks",
            &[("waterlogged", "true")]
        )));

        let fire = registry::default_block_state("minecraft:fire").expect("fire");
        assert_eq!(fire_age(&fire), Some(0));
        assert_eq!(fire_age(&state("minecraft:soul_fire", &[])), None);
    }
}
