//! Redstone components and power queries.
//!
//! Reference: Folia 1.21.11 `SignalGetter`, `RedStoneWireBlock`,
//! `DefaultRedstoneWireEvaluator`, `LeverBlock`, `ButtonBlock`, `DiodeBlock`,
//! `RepeaterBlock`, `ComparatorBlock`, `ObserverBlock`, `RedstoneTorchBlock`,
//! `RedstoneWallTorchBlock`, `BasePressurePlateBlock`, `PressurePlateBlock`,
//! `WeightedPressurePlateBlock`, and `TripWireBlock`.
//!
//! Implemented:
//!
//! * strong (`getDirectSignal`) and weak (`getSignal`) power queries over the
//!   loaded world, including wire's side/connection rule, diodes, observers,
//!   torches, pressure plates, levers, and buttons;
//! * wire target strength and its connection-shape properties
//!   (`north/east/south/west`), both flowing through [`crate::tick`]'s neighbour
//!   queue;
//! * repeaters: facing, delay 1-4 (2/4/6/8 ticks), input side detection, strong
//!   output power, scheduled toggle, side locking, and delay cycling on use;
//! * comparators: compare/subtract mode, rear and side inputs, container signal
//!   strength through [`ContainerSignalSource`], strong output power, and
//!   scheduled re-evaluation;
//! * observers: front-neighbour change detection, a two-tick pulse, and
//!   scheduled ticks;
//! * redstone torches: lit/unlit inversion, floor/wall attachment, and vanilla's
//!   8-toggles-in-60-ticks burn-out with a 160-tick restart delay;
//! * pressure plates and tripwire: an entity-occupancy input API, pressed state,
//!   and scheduled release; wooden, stone, and weighted pressure plates.
//!
//! Not implemented: pistons, tripwire-hook signal chains, target blocks,
//! redstone lamps, analog block outputs other than registered containers (item
//! frames, daylight detectors, lecterns, …), and NBT persistence for comparator
//! outputs or scheduled ticks. Those stay explicit gaps in `BLOCKERS.md`.

use crate::block_behaviour;
use crate::block_update;
use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::tick::{BlockBehavior, TickPriority, TickWorld, UpdateFlags};
use crate::world::WorldStorage;
use std::io;

pub use crate::tick::ContainerSignalSource;

/// `minecraft:redstone_wire`.
pub const REDSTONE_WIRE: &str = "minecraft:redstone_wire";
/// `minecraft:redstone_block`.
pub const REDSTONE_BLOCK: &str = "minecraft:redstone_block";
/// `minecraft:lever`.
pub const LEVER: &str = "minecraft:lever";
/// `minecraft:repeater`.
pub const REPEATER: &str = "minecraft:repeater";
/// `minecraft:comparator`.
pub const COMPARATOR: &str = "minecraft:comparator";
/// `minecraft:observer`.
pub const OBSERVER: &str = "minecraft:observer";
/// `minecraft:redstone_torch`.
pub const REDSTONE_TORCH: &str = "minecraft:redstone_torch";
/// `minecraft:redstone_wall_torch`.
pub const REDSTONE_WALL_TORCH: &str = "minecraft:redstone_wall_torch";
/// `minecraft:tripwire`.
pub const TRIPWIRE: &str = "minecraft:tripwire";
/// `minecraft:tripwire_hook`.
pub const TRIPWIRE_HOOK: &str = "minecraft:tripwire_hook";

/// Vanilla `RedstoneTorchBlock.RECENT_TOGGLE_TIMER`.
pub const REDSTONE_TORCH_TOGGLE_WINDOW: u64 = 60;
/// Vanilla `RedstoneTorchBlock.MAX_RECENT_TOGGLES`.
pub const REDSTONE_TORCH_MAX_TOGGLES: usize = 8;
/// Vanilla `RedstoneTorchBlock.RESTART_DELAY`.
pub const REDSTONE_TORCH_RESTART_DELAY: u64 = 160;
/// Vanilla observer pulse length (`ObserverBlock` schedules 2 ticks twice).
pub const OBSERVER_PULSE_TICKS: u64 = 2;
/// Vanilla `TripWireBlock.RECHECK_PERIOD`.
pub const TRIPWIRE_RECHECK_TICKS: u64 = 10;

/// Vanilla `Direction.Plane.HORIZONTAL` order.
const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

/// Entity occupancy supplied by the entity layer for plates and tripwire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlateOccupancy {
    /// Non-spectator, non-trigger-ignoring entities inside the touch box.
    pub entities: u32,
    /// The living subset, used by stone pressure plates.
    pub living: u32,
}

/// One redstone-torch off toggle, used for burn-out detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TorchToggle {
    pub pos: BlockPos,
    pub tick: u64,
}

/// Pressure-plate entity sensitivity (`BlockSetType.PressurePlateSensitivity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateSensitivity {
    /// Wooden, golden, iron, cherry, crimson, warped, and bamboo plates.
    Everything,
    /// Stone and polished-blackstone plates.
    Mobs,
}

/// A read-only `SignalGetter` over the decoded-chunk cache.
///
/// `should_signal == false` reproduces `RedStoneWireBlock.getBlockSignal`'s
/// temporary `shouldSignal = false` flag: wire then reports zero weak power, so
/// a wire does not power itself while it is being evaluated.
#[derive(Clone, Copy)]
pub struct SignalReader<'a> {
    storage: &'a WorldStorage,
    should_signal: bool,
}

impl<'a> SignalReader<'a> {
    /// Creates a reader. Normal world queries use `should_signal = true`.
    pub fn new(storage: &'a WorldStorage, should_signal: bool) -> Self {
        Self {
            storage,
            should_signal,
        }
    }

    /// A normal reader (`shouldSignal = true`).
    pub fn world(storage: &'a WorldStorage) -> Self {
        Self::new(storage, true)
    }

    fn block_state(&self, pos: BlockPos) -> Option<BlockState> {
        self.storage.block_state_at(pos.x(), pos.y(), pos.z())
    }

    /// `SignalGetter.getSignal`: weak power at `pos` on `side`.
    ///
    /// `side` is the direction from the receiving block to `pos`, matching
    /// vanilla's `getSignal(pos.relative(direction), direction)` calls.
    pub fn signal(&self, pos: BlockPos, side: Direction) -> i32 {
        let Some(state) = self.block_state(pos) else {
            return 0;
        };
        let signal = self.state_signal(pos, &state, side);
        if is_redstone_conductor(&state) {
            signal.max(self.direct_signal_to(pos))
        } else {
            signal
        }
    }

    /// `SignalGetter.getDirectSignal`: strong power at `pos` on `side`.
    pub fn direct_signal(&self, pos: BlockPos, side: Direction) -> i32 {
        let Some(state) = self.block_state(pos) else {
            return 0;
        };
        self.state_direct_signal(pos, &state, side)
    }

    /// `SignalGetter.getDirectSignalTo`.
    pub fn direct_signal_to(&self, pos: BlockPos) -> i32 {
        let mut best = 0;
        for direction in Direction::ALL {
            best = best.max(self.direct_signal(pos.relative(direction), direction));
            if best >= 15 {
                return best;
            }
        }
        best
    }

    /// `SignalGetter.getControlInputSignal`.
    pub fn control_input_signal(
        &self,
        pos: BlockPos,
        direction: Direction,
        diodes_only: bool,
    ) -> i32 {
        let Some(state) = self.block_state(pos) else {
            return 0;
        };
        if diodes_only {
            if is_diode(&state) {
                return self.direct_signal(pos, direction);
            }
            return 0;
        }
        if state.name == REDSTONE_BLOCK {
            return 15;
        }
        if is_wire(&state) {
            return wire_power(&state);
        }
        if is_signal_source(&state) {
            return self.direct_signal(pos, direction);
        }
        0
    }

    /// `SignalGetter.hasSignal`.
    pub fn has_signal(&self, pos: BlockPos, direction: Direction) -> bool {
        self.signal(pos, direction) > 0
    }

    /// `SignalGetter.hasNeighborSignal`.
    pub fn has_neighbor_signal(&self, pos: BlockPos) -> bool {
        Direction::ALL
            .into_iter()
            .any(|direction| self.signal(pos.relative(direction), direction) > 0)
    }

    /// `SignalGetter.getBestNeighborSignal`.
    pub fn best_neighbor_signal(&self, pos: BlockPos) -> i32 {
        let mut best = 0;
        for direction in Direction::ALL {
            let signal = self.signal(pos.relative(direction), direction);
            if signal >= 15 {
                return 15;
            }
            best = best.max(signal);
        }
        best
    }

    /// `BlockState.hasAnalogOutputSignal`/`getAnalogOutputSignal` for the
    /// registered container sources.
    ///
    /// `Some(0)` means the block is a container but currently empty, which the
    /// comparator must distinguish from "no analog output".
    pub fn analog_output_signal(&self, pos: BlockPos) -> Option<i32> {
        self.storage.tick_state().containers.signal(pos)
    }

    /// The stored `ComparatorBlockEntity.outputSignal` at `pos`.
    pub fn comparator_output(&self, pos: BlockPos) -> i32 {
        self.storage
            .tick_state()
            .comparator_outputs
            .get(&pos)
            .copied()
            .unwrap_or(0)
            .clamp(0, 15)
    }

    fn state_signal(&self, pos: BlockPos, state: &BlockState, side: Direction) -> i32 {
        if is_wire(state) {
            if self.should_signal {
                wire_signal(state, side)
            } else {
                0
            }
        } else if is_pressure_plate(state) {
            pressure_plate_state_signal(state)
        } else if is_redstone_torch(state) {
            if is_wall_redstone_torch(state) {
                if torch_lit(state) && facing(state) != side {
                    15
                } else {
                    0
                }
            } else if torch_lit(state) && side != Direction::Up {
                15
            } else {
                0
            }
        } else if is_diode(state) {
            if is_powered(state) && facing(state) == side {
                self.diode_output_signal(pos, state)
            } else {
                0
            }
        } else if is_observer(state) {
            if is_powered(state) && facing(state) == side {
                15
            } else {
                0
            }
        } else if is_power_source(state) {
            if is_powered(state) { 15 } else { 0 }
        } else if state.name == REDSTONE_BLOCK {
            15
        } else {
            0
        }
    }

    fn state_direct_signal(&self, pos: BlockPos, state: &BlockState, side: Direction) -> i32 {
        if is_wire(state) {
            // `RedStoneWireBlock.getDirectSignal` delegates to `getSignal`.
            return if self.should_signal {
                wire_signal(state, side)
            } else {
                0
            };
        }
        if is_pressure_plate(state) {
            // `BasePressurePlateBlock.getDirectSignal`: strong power downward.
            return if side == Direction::Up {
                pressure_plate_state_signal(state)
            } else {
                0
            };
        }
        if is_redstone_torch(state) {
            // `RedstoneTorchBlock.getDirectSignal`: strong power upward only.
            return if torch_lit(state) && side == Direction::Down {
                15
            } else {
                0
            };
        }
        if is_diode(state) {
            // `DiodeBlock.getDirectSignal` delegates to `getSignal`.
            return if is_powered(state) && facing(state) == side {
                self.diode_output_signal(pos, state)
            } else {
                0
            };
        }
        if is_observer(state) {
            return if is_powered(state) && facing(state) == side {
                15
            } else {
                0
            };
        }
        if is_power_source(state) && is_powered(state) && connected_direction(state) == side {
            15
        } else {
            0
        }
    }

    fn diode_output_signal(&self, pos: BlockPos, state: &BlockState) -> i32 {
        if is_repeater(state) {
            15
        } else if is_comparator(state) {
            self.comparator_output(pos)
        } else {
            0
        }
    }
}

/// `BlockState.isRedstoneConductor` approximation from dumped solidity.
pub fn is_redstone_conductor(state: &BlockState) -> bool {
    block_behaviour::is_solid(state)
}

/// A weak-power query for one position (`getBestNeighborSignal`).
pub fn power_at(storage: &WorldStorage, pos: BlockPos) -> i32 {
    SignalReader::world(storage).best_neighbor_signal(pos)
}

/// Whether a block at `pos` receives any weak neighbour signal.
pub fn has_neighbor_signal(storage: &WorldStorage, pos: BlockPos) -> bool {
    SignalReader::world(storage).has_neighbor_signal(pos)
}

/// `RedStoneWireBlock.POWER` (0..=15).
pub fn wire_power(state: &BlockState) -> i32 {
    if !is_wire(state) {
        return 0;
    }
    property(state, "power")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0)
        .clamp(0, 15)
}

/// Clones a wire state with a new `power` while keeping its connection props.
pub fn wire_with_power(state: &BlockState, power: i32) -> BlockState {
    let mut wire = state.clone();
    set_property(&mut wire, "power", &power.clamp(0, 15).to_string());
    wire
}

/// A connected wire state with all four horizontal sides set to `side`.
pub fn wire_state(power: i32) -> BlockState {
    let mut wire = BlockState {
        name: REDSTONE_WIRE.to_owned(),
        properties: vec![
            ("east".to_owned(), "side".to_owned()),
            ("north".to_owned(), "side".to_owned()),
            ("power".to_owned(), power.clamp(0, 15).to_string()),
            ("south".to_owned(), "side".to_owned()),
            ("west".to_owned(), "side".to_owned()),
        ],
    };
    wire.properties.sort();
    wire
}

/// Wire's `getSignal`: upward always emits, horizontal sides need a connection.
fn wire_signal(state: &BlockState, side: Direction) -> i32 {
    let power = wire_power(state);
    if power == 0 || side == Direction::Down {
        return 0;
    }
    if side == Direction::Up {
        return power;
    }
    if wire_side(state, side.opposite()) == "none" {
        0
    } else {
        power
    }
}

fn wire_side(state: &BlockState, direction: Direction) -> &str {
    let key = match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        _ => return "none",
    };
    property(state, key).unwrap_or("none")
}

fn set_wire_side(state: &mut BlockState, direction: Direction, value: &str) {
    let key = match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        _ => return,
    };
    set_property(state, key, value);
}

fn wire_connected(state: &BlockState, direction: Direction) -> bool {
    wire_side(state, direction) != "none"
}

fn is_wire_dot(state: &BlockState) -> bool {
    HORIZONTAL
        .into_iter()
        .all(|direction| !wire_connected(state, direction))
}

/// `RedStoneWireBlock.getConnectingSide` with the full-cube sturdy
/// approximation used elsewhere in the port.
fn wire_connecting_side(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    direction: Direction,
    non_normal_cube_above: bool,
) -> &'static str {
    let neighbor_pos = pos.relative(direction);
    let Some(neighbor) = reader.block_state(neighbor_pos) else {
        return "none";
    };
    if non_normal_cube_above && can_survive_on(&neighbor) {
        let above_connects = reader
            .block_state(neighbor_pos.above())
            .is_some_and(|above| should_connect_to(&above, None));
        if above_connects {
            if block_behaviour::is_solid(&neighbor) {
                return "up";
            }
            return "side";
        }
    }
    let below_connects = !is_redstone_conductor(&neighbor)
        && reader
            .block_state(neighbor_pos.below())
            .is_some_and(|below| should_connect_to(&below, None));
    if should_connect_to(&neighbor, Some(direction)) || below_connects {
        "side"
    } else {
        "none"
    }
}

/// `RedStoneWireBlock.getConnectionState`: recompute every side and add the
/// dot/cross stubs vanilla uses when a wire has no straight connection.
fn wire_connection_state(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> BlockState {
    let was_dot = is_wire_dot(state);
    let mut shaped = state.clone();
    for direction in HORIZONTAL {
        set_wire_side(&mut shaped, direction, "none");
    }
    let non_normal_cube_above = !reader
        .block_state(pos.above())
        .is_some_and(|above| is_redstone_conductor(&above));
    for direction in HORIZONTAL {
        let side = wire_connecting_side(reader, pos, direction, non_normal_cube_above);
        set_wire_side(&mut shaped, direction, side);
    }
    if was_dot && is_wire_dot(&shaped) {
        return shaped;
    }

    let north = wire_connected(&shaped, Direction::North);
    let south = wire_connected(&shaped, Direction::South);
    let east = wire_connected(&shaped, Direction::East);
    let west = wire_connected(&shaped, Direction::West);
    let no_north_south = !north && !south;
    let no_east_west = !east && !west;
    if !west && no_north_south {
        set_wire_side(&mut shaped, Direction::West, "side");
    }
    if !east && no_north_south {
        set_wire_side(&mut shaped, Direction::East, "side");
    }
    if !north && no_east_west {
        set_wire_side(&mut shaped, Direction::North, "side");
    }
    if !south && no_east_west {
        set_wire_side(&mut shaped, Direction::South, "side");
    }
    shaped
}

/// `RedStoneWireBlock.canSurviveOn`: a sturdy face or a hopper.
fn can_survive_on(state: &BlockState) -> bool {
    block_behaviour::is_solid(state) || state.name == "minecraft:hopper"
}

/// `RedStoneWireBlock.shouldConnectTo`.
fn should_connect_to(state: &BlockState, direction: Option<Direction>) -> bool {
    if is_wire(state) {
        true
    } else if is_repeater(state) {
        let facing = facing(state);
        direction.is_some_and(|direction| direction == facing || direction == facing.opposite())
    } else if is_observer(state) {
        direction.is_some_and(|direction| direction == facing(state))
    } else {
        is_signal_source(state) && direction.is_some()
    }
}

/// `RedstoneWireEvaluator.getIncomingWireSignal`.
pub fn incoming_wire_signal(storage: &WorldStorage, pos: BlockPos) -> i32 {
    let reader = SignalReader::new(storage, false);
    let mut best = 0;
    for direction in HORIZONTAL {
        let neighbor_pos = pos.relative(direction);
        let Some(neighbor_state) = reader.block_state(neighbor_pos) else {
            continue;
        };
        best = best.max(wire_power(&neighbor_state));
        if is_redstone_conductor(&neighbor_state)
            && !reader
                .block_state(neighbor_pos.above())
                .is_some_and(|above| is_redstone_conductor(&above))
        {
            if let Some(above) = reader.block_state(neighbor_pos.above()) {
                best = best.max(wire_power(&above));
            }
        } else if !is_redstone_conductor(&neighbor_state)
            && let Some(below) = reader.block_state(neighbor_pos.below())
        {
            best = best.max(wire_power(&below));
        }
    }
    best.saturating_sub(1).max(0)
}

/// `DefaultRedstoneWireEvaluator.calculateTargetStrength`.
pub fn target_wire_power(storage: &WorldStorage, pos: BlockPos) -> i32 {
    let block_signal = SignalReader::new(storage, false).best_neighbor_signal(pos);
    if block_signal == 15 {
        block_signal
    } else {
        block_signal.max(incoming_wire_signal(storage, pos))
    }
}

/// Returns whether the block is redstone wire.
pub fn is_wire(state: &BlockState) -> bool {
    state.name == REDSTONE_WIRE
}

/// Returns whether the block is a lever.
pub fn is_lever(state: &BlockState) -> bool {
    state.name == LEVER
}

/// Returns whether the block is any vanilla button.
pub fn is_button(state: &BlockState) -> bool {
    state.name.ends_with("_button")
}

/// Returns whether the block is a repeater.
pub fn is_repeater(state: &BlockState) -> bool {
    state.name == REPEATER
}

/// Returns whether the block is a comparator.
pub fn is_comparator(state: &BlockState) -> bool {
    state.name == COMPARATOR
}

/// Returns whether the block is an observer.
pub fn is_observer(state: &BlockState) -> bool {
    state.name == OBSERVER
}

/// Vanilla `DiodeBlock.isDiode`.
pub fn is_diode(state: &BlockState) -> bool {
    is_repeater(state) || is_comparator(state)
}

/// Returns whether the block is a redstone torch (floor or wall).
pub fn is_redstone_torch(state: &BlockState) -> bool {
    matches!(state.name.as_str(), REDSTONE_TORCH | REDSTONE_WALL_TORCH)
}

fn is_wall_redstone_torch(state: &BlockState) -> bool {
    state.name == REDSTONE_WALL_TORCH
}

/// Returns whether the block is a pressure plate.
pub fn is_pressure_plate(state: &BlockState) -> bool {
    state.name.ends_with("_pressure_plate")
}

/// Returns whether the block is a weighted pressure plate.
pub fn is_weighted_pressure_plate(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:light_weighted_pressure_plate" | "minecraft:heavy_weighted_pressure_plate"
    )
}

/// Returns whether the block is tripwire.
pub fn is_tripwire(state: &BlockState) -> bool {
    state.name == TRIPWIRE
}

/// Returns whether the block is a tripwire hook.
pub fn is_tripwire_hook(state: &BlockState) -> bool {
    state.name == TRIPWIRE_HOOK
}

/// Vanilla `ButtonBlock.getSignal` source check.
fn is_power_source(state: &BlockState) -> bool {
    is_lever(state) || is_button(state)
}

/// `BlockState.isSignalSource` for the ported sources.
pub fn is_signal_source(state: &BlockState) -> bool {
    is_power_source(state)
        || is_wire(state)
        || is_redstone_torch(state)
        || is_diode(state)
        || is_observer(state)
        || is_pressure_plate(state)
        || state.name == REDSTONE_BLOCK
}

/// `BlockState.getValue(LIT)`.
pub fn torch_lit(state: &BlockState) -> bool {
    property(state, "lit") != Some("false")
}

/// `BlockState.getValue(POWERED)`.
pub fn is_powered(state: &BlockState) -> bool {
    property(state, "powered") == Some("true")
}

/// `BlockState.getValue(LOCKED)`.
pub fn is_locked(state: &BlockState) -> bool {
    property(state, "locked") == Some("true")
}

/// `HorizontalDirectionalBlock.FACING`.
pub fn facing(state: &BlockState) -> Direction {
    match property(state, "facing").unwrap_or("north") {
        "north" => Direction::North,
        "south" => Direction::South,
        "west" => Direction::West,
        "east" => Direction::East,
        other => match other {
            "up" => Direction::Up,
            "down" => Direction::Down,
            _ => Direction::North,
        },
    }
}

fn horizontal_clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        other => other,
    }
}

fn horizontal_counter_clockwise(direction: Direction) -> Direction {
    horizontal_clockwise(direction).opposite()
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`.
fn connected_direction(state: &BlockState) -> Direction {
    match property(state, "face").unwrap_or("wall") {
        "ceiling" => Direction::Down,
        "floor" => Direction::Up,
        _ => facing(state),
    }
}

/// Clones a lever/button state with `powered` set.
pub fn with_powered(state: &BlockState, powered: bool) -> BlockState {
    with_property(state, "powered", if powered { "true" } else { "false" })
}

/// Clones a repeater state with `locked` set.
pub fn with_locked(state: &BlockState, locked: bool) -> BlockState {
    with_property(state, "locked", if locked { "true" } else { "false" })
}

/// Clones a redstone torch state with `lit` set.
pub fn with_lit(state: &BlockState, lit: bool) -> BlockState {
    with_property(state, "lit", if lit { "true" } else { "false" })
}

/// Clones a horizontal directional block state with `facing` set.
pub fn with_facing(state: &BlockState, direction: Direction) -> BlockState {
    let value = match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        Direction::Up => "up",
        Direction::Down => "down",
    };
    with_property(state, "facing", value)
}

/// Clones a repeater state with `delay` clamped to vanilla's 1..=4.
pub fn with_delay(state: &BlockState, delay: i32) -> BlockState {
    with_property(state, "delay", &delay.clamp(1, 4).to_string())
}

/// Clones a comparator state with `mode` set to `compare` or `subtract`.
pub fn with_mode(state: &BlockState, mode: &str) -> BlockState {
    with_property(
        state,
        "mode",
        if mode == "subtract" {
            "subtract"
        } else {
            "compare"
        },
    )
}

/// Clones a tripwire state with `attached` set.
pub fn with_attached(state: &BlockState, attached: bool) -> BlockState {
    with_property(state, "attached", if attached { "true" } else { "false" })
}

/// Clones a block state with one property replaced (or added).
pub fn with_property(state: &BlockState, key: &str, value: &str) -> BlockState {
    let mut updated = state.clone();
    set_property(&mut updated, key, value);
    updated
}

/// `ButtonBlock` press duration, from the `ButtonBlock` registration.
pub fn button_delay(state: &BlockState) -> u64 {
    match state.name.as_str() {
        "minecraft:stone_button" | "minecraft:polished_blackstone_button" => 20,
        _ => 30,
    }
}

/// Repeater `getDelay`: `delay * 2` ticks (2, 4, 6, or 8).
pub fn repeater_delay_ticks(state: &BlockState) -> u64 {
    let delay = property(state, "delay")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(1)
        .clamp(1, 4);
    (delay * 2) as u64
}

/// `DiodeBlock.getInputSignal` for repeaters.
pub fn diode_input_signal(reader: &SignalReader<'_>, pos: BlockPos, state: &BlockState) -> i32 {
    let direction = facing(state);
    let input_pos = pos.relative(direction);
    let signal = reader.signal(input_pos, direction);
    if signal >= 15 {
        return signal;
    }
    let wire = reader
        .block_state(input_pos)
        .map(|state| wire_power(&state))
        .unwrap_or(0);
    signal.max(wire)
}

/// `DiodeBlock.getAlternateSignal` from the two sides of `facing`.
pub fn alternate_signal(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
    diodes_only: bool,
) -> i32 {
    let direction = facing(state);
    let clockwise = horizontal_clockwise(direction);
    let counter_clockwise = horizontal_counter_clockwise(direction);
    reader
        .control_input_signal(pos.relative(clockwise), clockwise, diodes_only)
        .max(reader.control_input_signal(
            pos.relative(counter_clockwise),
            counter_clockwise,
            diodes_only,
        ))
}

/// `RepeaterBlock.isLocked`: a powered diode on either side (diodes only).
pub fn repeater_is_locked(reader: &SignalReader<'_>, pos: BlockPos, state: &BlockState) -> bool {
    alternate_signal(reader, pos, state, true) > 0
}

/// `DiodeBlock.shouldTurnOn` for a repeater.
pub fn repeater_should_turn_on(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> bool {
    diode_input_signal(reader, pos, state) > 0
}

/// `ComparatorBlock.getInputSignal`: rear weak power, a container directly
/// behind the input, or a container one block further through a conductor.
pub fn comparator_input_signal(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> i32 {
    let mut signal = diode_input_signal(reader, pos, state);
    let direction = facing(state);
    let front = pos.relative(direction);
    if let Some(analog) = reader.analog_output_signal(front) {
        signal = analog;
    } else if signal < 15
        && reader
            .block_state(front)
            .is_some_and(|front| is_redstone_conductor(&front))
    {
        let beyond = front.relative(direction);
        if let Some(analog) = reader.analog_output_signal(beyond) {
            signal = analog;
        }
    }
    signal.clamp(0, 15)
}

/// `ComparatorBlock.shouldTurnOn`.
pub fn comparator_should_turn_on(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> bool {
    let input = comparator_input_signal(reader, pos, state);
    if input == 0 {
        return false;
    }
    let alternate = alternate_signal(reader, pos, state, false);
    input > alternate || (input == alternate && !comparator_is_subtract(state))
}

/// `ComparatorBlock.calculateOutputSignal`.
pub fn comparator_output_signal(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> i32 {
    let input = comparator_input_signal(reader, pos, state);
    if input == 0 {
        return 0;
    }
    let alternate = alternate_signal(reader, pos, state, false);
    if alternate > input {
        0
    } else if comparator_is_subtract(state) {
        input - alternate
    } else {
        input
    }
}

fn comparator_is_subtract(state: &BlockState) -> bool {
    property(state, "mode") == Some("subtract")
}

/// `RepeaterBlock.getDelay`. Kept for parity naming.
pub fn repeater_delay(state: &BlockState) -> u64 {
    repeater_delay_ticks(state)
}

/// `ComparatorBlock.getDelay`.
pub fn comparator_delay() -> u64 {
    2
}

/// `DiodeBlock.shouldPrioritize`.
pub fn should_prioritize(reader: &SignalReader<'_>, pos: BlockPos, state: &BlockState) -> bool {
    let opposite = facing(state).opposite();
    reader
        .block_state(pos.relative(opposite))
        .is_some_and(|behind| is_diode(&behind) && facing(&behind) != opposite)
}

/// `PressurePlateBlock.getSignalForState`/`WeightedPressurePlateBlock`.
pub fn pressure_plate_state_signal(state: &BlockState) -> i32 {
    if is_weighted_pressure_plate(state) {
        property(state, "power")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0)
            .clamp(0, 15)
    } else if is_powered(state) {
        15
    } else {
        0
    }
}

/// `BlockSetType.pressurePlateSensitivity`.
pub fn pressure_plate_sensitivity(state: &BlockState) -> PlateSensitivity {
    match state.name.as_str() {
        "minecraft:stone_pressure_plate" | "minecraft:polished_blackstone_pressure_plate" => {
            PlateSensitivity::Mobs
        }
        _ => PlateSensitivity::Everything,
    }
}

/// `BasePressurePlateBlock.getPressedTime`.
pub fn pressure_plate_pressed_time(state: &BlockState) -> u64 {
    if is_weighted_pressure_plate(state) {
        10
    } else {
        20
    }
}

/// `WeightedPressurePlateBlock.maxWeight`, or 0 for an unweighted plate.
pub fn pressure_plate_max_weight(state: &BlockState) -> u32 {
    match state.name.as_str() {
        "minecraft:light_weighted_pressure_plate" => 15,
        "minecraft:heavy_weighted_pressure_plate" => 150,
        _ => 0,
    }
}

/// `BasePressurePlateBlock.getSignalStrength` from entity occupancy.
pub fn pressure_plate_signal(state: &BlockState, occupancy: PlateOccupancy) -> i32 {
    let count = match pressure_plate_sensitivity(state) {
        PlateSensitivity::Everything => occupancy.entities,
        PlateSensitivity::Mobs => occupancy.living,
    };
    if count == 0 {
        return 0;
    }
    if is_weighted_pressure_plate(state) {
        let max_weight = pressure_plate_max_weight(state).max(1);
        let fraction = count.min(max_weight) as f32 / max_weight as f32;
        (fraction * 15.0).ceil() as i32
    } else {
        15
    }
}

/// Clones a plate state with the given signal strength.
pub fn pressure_plate_with_signal(state: &BlockState, strength: i32) -> BlockState {
    if is_weighted_pressure_plate(state) {
        with_property(state, "power", &strength.clamp(0, 15).to_string())
    } else {
        with_powered(state, strength > 0)
    }
}

/// `RedstoneTorchBlock.hasNeighborSignal` for a floor torch.
pub fn torch_has_neighbor_signal(
    reader: &SignalReader<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> bool {
    if is_wall_redstone_torch(state) {
        let opposite = facing(state).opposite();
        reader.signal(pos.relative(opposite), opposite) > 0
    } else {
        reader.signal(pos.below(), Direction::Down) > 0
    }
}

/// Flips a lever and updates its neighbours through the tick queue.
pub fn toggle_lever(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if !is_lever(&state) {
        return Ok(false);
    }
    let replacement = with_powered(&state, !is_powered(&state));
    block_update::set_block(storage, pos.x(), pos.y(), pos.z(), replacement)
        .map(|change| change.is_some())
}

/// Presses a button: sets `powered`, schedules the vanilla release tick, and
/// updates neighbours through the tick queue.
pub fn press_button(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if !is_button(&state) || is_powered(&state) {
        return Ok(false);
    }
    let delay = button_delay(&state);
    let name = state.name.clone();
    let tick = storage.tick_state().current_tick;
    let changed = block_update::set_block(
        storage,
        pos.x(),
        pos.y(),
        pos.z(),
        with_powered(&state, true),
    )?
    .is_some();
    if changed {
        storage.tick_state_mut().scheduled.schedule_after(
            tick,
            pos,
            name,
            delay,
            TickPriority::Normal,
        );
    }
    Ok(changed)
}

/// Releases a pressed button without scheduling another tick.
pub fn release_button(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if !is_button(&state) || !is_powered(&state) {
        return Ok(false);
    }
    block_update::set_block(
        storage,
        pos.x(),
        pos.y(),
        pos.z(),
        with_powered(&state, false),
    )
    .map(|change| change.is_some())
}

/// Records entity occupancy for a pressure plate.
///
/// This is the hook the entity layer calls when an entity enters or leaves the
/// plate's touch box. Vanilla only checks a plate that is currently unpowered;
/// a pressed plate re-checks its occupancy on the scheduled tick.
pub fn set_pressure_plate_occupancy(
    storage: &mut WorldStorage,
    pos: BlockPos,
    occupancy: PlateOccupancy,
) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if !is_pressure_plate(&state) {
        return Ok(false);
    }
    storage.tick_state_mut().occupancy.insert(pos, occupancy);
    if pressure_plate_state_signal(&state) > 0 {
        return Ok(false);
    }
    check_pressure_plate(storage, pos, &state, occupancy)
}

/// Records entity occupancy for tripwire.
///
/// Vanilla only checks an unpowered tripwire; a pressed tripwire re-checks on
/// its ten-tick scheduled tick.
pub fn set_tripwire_occupancy(
    storage: &mut WorldStorage,
    pos: BlockPos,
    occupancy: PlateOccupancy,
) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if !is_tripwire(&state) {
        return Ok(false);
    }
    storage.tick_state_mut().occupancy.insert(pos, occupancy);
    if is_powered(&state) {
        return Ok(false);
    }
    check_tripwire(storage, pos, &state, occupancy)
}

fn check_pressure_plate(
    storage: &mut WorldStorage,
    pos: BlockPos,
    state: &BlockState,
    occupancy: PlateOccupancy,
) -> io::Result<bool> {
    let strength = pressure_plate_signal(state, occupancy);
    let current = pressure_plate_state_signal(state);
    let changed = if current != strength {
        block_update::set_block(
            storage,
            pos.x(),
            pos.y(),
            pos.z(),
            pressure_plate_with_signal(state, strength),
        )?
        .is_some()
    } else {
        false
    };
    if strength > 0 {
        let tick = storage.tick_state().current_tick;
        storage.tick_state_mut().scheduled.schedule_after(
            tick,
            pos,
            state.name.clone(),
            pressure_plate_pressed_time(state),
            TickPriority::Normal,
        );
    }
    Ok(changed)
}

fn check_tripwire(
    storage: &mut WorldStorage,
    pos: BlockPos,
    state: &BlockState,
    occupancy: PlateOccupancy,
) -> io::Result<bool> {
    let occupied = occupancy.entities > 0;
    let powered = is_powered(state);
    let changed = if occupied != powered {
        block_update::set_block(
            storage,
            pos.x(),
            pos.y(),
            pos.z(),
            with_powered(state, occupied),
        )?
        .is_some()
    } else {
        false
    };
    if occupied {
        let tick = storage.tick_state().current_tick;
        storage.tick_state_mut().scheduled.schedule_after(
            tick,
            pos,
            state.name.clone(),
            TRIPWIRE_RECHECK_TICKS,
            TickPriority::Normal,
        );
    }
    Ok(changed)
}

/// Notifies comparators around a container that its analog output changed.
///
/// Vanilla `Level.updateNeighbourForOutputSignal` reaches the comparator through
/// `neighborChanged`, which schedules its normal two-tick re-evaluation.
pub fn notify_container_changed(storage: &mut WorldStorage, pos: BlockPos) {
    let tick = storage.tick_state().current_tick;
    for direction in Direction::ALL {
        let neighbor = pos.relative(direction);
        let Some(state) = storage.block_state_at(neighbor.x(), neighbor.y(), neighbor.z()) else {
            continue;
        };
        if is_comparator(&state) {
            storage.tick_state_mut().scheduled.schedule_after(
                tick,
                neighbor,
                state.name,
                comparator_delay(),
                TickPriority::Normal,
            );
        }
    }
}

/// Dispatches `useWithoutItem` for the ported interactive blocks.
pub fn use_block(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if is_lever(&state) {
        toggle_lever(storage, pos)
    } else if is_button(&state) {
        press_button(storage, pos)
    } else if is_repeater(&state) {
        let delay = property(&state, "delay")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(1);
        let next = with_delay(&state, delay % 4 + 1);
        block_update::set_block(storage, pos.x(), pos.y(), pos.z(), next)
            .map(|change| change.is_some())
    } else if is_comparator(&state) {
        let next = if comparator_is_subtract(&state) {
            with_mode(&state, "compare")
        } else {
            with_mode(&state, "subtract")
        };
        let changed =
            block_update::set_block(storage, pos.x(), pos.y(), pos.z(), next.clone())?.is_some();
        let current = storage
            .block_state_at(pos.x(), pos.y(), pos.z())
            .unwrap_or(next);
        refresh_comparator_storage(storage, pos, &current)?;
        Ok(changed)
    } else {
        Ok(false)
    }
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    }
}

/// Behaviour for `minecraft:redstone_wire`.
pub struct WireBehavior;

/// The shared wire behaviour.
pub static WIRE: WireBehavior = WireBehavior;

fn recompute_wire(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    refresh_connections: bool,
    placed: bool,
) -> io::Result<()> {
    let shaped = if refresh_connections {
        // `RedStoneWireBlock.getStateForPlacement` starts from `crossState` and
        // fills the missing connections, so a freshly placed lone wire becomes
        // a cross rather than a dot.
        let base = if placed && is_wire_dot(state) {
            wire_state(wire_power(state))
        } else {
            state.clone()
        };
        wire_connection_state(&SignalReader::world(world.storage()), pos, &base)
    } else {
        state.clone()
    };
    let target = target_wire_power(world.storage(), pos);
    let next = wire_with_power(&shaped, target);
    if next != *state {
        world.set_block(pos, next)?;
    }
    Ok(())
}

impl BlockBehavior for WireBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if is_wire(state) {
            let is_new_block = old.is_none_or(|old| old.name != state.name);
            recompute_wire(world, pos, state, is_new_block, is_new_block)?;
        }
        Ok(())
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if is_wire(state) {
            recompute_wire(world, pos, state, true, false)?;
        }
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        let shaped = wire_connection_state(&SignalReader::world(world.storage()), pos, state);
        Ok((shaped != *state).then_some(shaped))
    }
}

/// Behaviour for `minecraft:lever`.
pub struct LeverBehavior;

/// The shared lever behaviour.
pub static LEVER_BEHAVIOR: LeverBehavior = LeverBehavior;

impl BlockBehavior for LeverBehavior {}

/// Behaviour for every `minecraft:*_button`.
pub struct ButtonBehavior;

/// The shared button behaviour.
pub static BUTTON_BEHAVIOR: ButtonBehavior = ButtonBehavior;

impl BlockBehavior for ButtonBehavior {
    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if is_button(state) && is_powered(state) {
            world.set_block(pos, with_powered(state, false))?;
        }
        Ok(())
    }
}

/// Behaviour for `minecraft:repeater`.
pub struct RepeaterBehavior;

/// The shared repeater behaviour.
pub static REPEATER_BEHAVIOR: RepeaterBehavior = RepeaterBehavior;

impl BlockBehavior for RepeaterBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if !is_repeater(state) || old.is_some_and(|old| old.name == state.name) {
            return Ok(());
        }
        let (locked, should_turn_on) = {
            let reader = SignalReader::world(world.storage());
            (
                repeater_is_locked(&reader, pos, state),
                repeater_should_turn_on(&reader, pos, state),
            )
        };
        if locked != is_locked(state) {
            world.set_block(pos, with_locked(state, locked))?;
        }
        // `DiodeBlock.setPlacedBy` schedules a one-tick refresh when the input
        // is already powered.
        if should_turn_on {
            world.schedule(pos, state.name.clone(), 1);
        }
        Ok(())
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if !is_repeater(state) {
            return Ok(());
        }
        if world
            .storage()
            .tick_state()
            .scheduled
            .has_scheduled_tick(pos, state.name.as_str())
        {
            return Ok(());
        }
        let (locked, powered, should_turn_on, prioritize) = {
            let reader = SignalReader::world(world.storage());
            (
                repeater_is_locked(&reader, pos, state),
                is_powered(state),
                repeater_should_turn_on(&reader, pos, state),
                should_prioritize(&reader, pos, state),
            )
        };
        if !locked && powered != should_turn_on {
            let priority = if prioritize {
                TickPriority::ExtremelyHigh
            } else if powered {
                TickPriority::VeryHigh
            } else {
                TickPriority::High
            };
            world.schedule_with_priority(
                pos,
                state.name.clone(),
                repeater_delay_ticks(state),
                priority,
            );
        }
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_repeater(state) {
            return Ok(());
        }
        let (locked, should_turn_on) = {
            let reader = SignalReader::world(world.storage());
            (
                repeater_is_locked(&reader, pos, state),
                repeater_should_turn_on(&reader, pos, state),
            )
        };
        if locked {
            return Ok(());
        }
        if is_powered(state) && !should_turn_on {
            world.set_block(pos, with_powered(state, false))?;
        } else if !is_powered(state) {
            world.set_block(pos, with_powered(state, true))?;
            if !should_turn_on {
                world.schedule_with_priority(
                    pos,
                    state.name.clone(),
                    repeater_delay_ticks(state),
                    TickPriority::VeryHigh,
                );
            }
        }
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if !is_repeater(state) {
            return Ok(None);
        }
        if direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        if direction.axis() != facing(state).axis() {
            let locked = {
                let reader = SignalReader::world(world.storage());
                repeater_is_locked(&reader, pos, state)
            };
            if locked != is_locked(state) {
                return Ok(Some(with_locked(state, locked)));
            }
        }
        Ok(None)
    }
}

/// Behaviour for `minecraft:comparator`.
pub struct ComparatorBehavior;

/// The shared comparator behaviour.
pub static COMPARATOR_BEHAVIOR: ComparatorBehavior = ComparatorBehavior;

impl BlockBehavior for ComparatorBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if !is_comparator(state) || old.is_some_and(|old| old.name == state.name) {
            return Ok(());
        }
        if comparator_should_turn_on(&SignalReader::world(world.storage()), pos, state) {
            // `DiodeBlock.setPlacedBy` schedules one tick, not `getDelay`.
            world.schedule(pos, state.name.clone(), 1);
        }
        Ok(())
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if !is_comparator(state)
            || world
                .storage()
                .tick_state()
                .scheduled
                .has_scheduled_tick(pos, state.name.as_str())
        {
            return Ok(());
        }
        let (output, should_turn_on, prioritize) = {
            let reader = SignalReader::world(world.storage());
            (
                comparator_output_signal(&reader, pos, state),
                comparator_should_turn_on(&reader, pos, state),
                should_prioritize(&reader, pos, state),
            )
        };
        let stored = SignalReader::world(world.storage()).comparator_output(pos);
        if output != stored || is_powered(state) != should_turn_on {
            let priority = if prioritize {
                TickPriority::High
            } else {
                TickPriority::Normal
            };
            world.schedule_with_priority(pos, state.name.clone(), comparator_delay(), priority);
        }
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_comparator(state) {
            return Ok(());
        }
        refresh_comparator_tick(world, pos, state)
    }

    fn update_shape(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if is_comparator(state) && direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        Ok(None)
    }
}

fn refresh_comparator_tick(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<()> {
    let (output, should_turn_on) = {
        let reader = SignalReader::world(world.storage());
        (
            comparator_output_signal(&reader, pos, state),
            comparator_should_turn_on(&reader, pos, state),
        )
    };
    let previous = world.comparator_output(pos);
    world.set_comparator_output(pos, output);
    if previous != output || !comparator_is_subtract(state) {
        let powered = is_powered(state);
        if powered && !should_turn_on {
            world.set_block(pos, with_powered(state, false))?;
        } else if !powered && should_turn_on {
            world.set_block(pos, with_powered(state, true))?;
        }
        // `ComparatorBlock.refreshOutputState` always calls
        // `updateNeighborsInFront`; the output may change while POWERED stays
        // true, so the block state alone cannot carry the notification.
        world.notify_neighbors(pos);
    }
    Ok(())
}

fn refresh_comparator_storage(
    storage: &mut WorldStorage,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<bool> {
    let (output, should_turn_on) = {
        let reader = SignalReader::world(storage);
        (
            comparator_output_signal(&reader, pos, state),
            comparator_should_turn_on(&reader, pos, state),
        )
    };
    let previous = storage.tick_state().comparator_outputs.get(&pos).copied();
    storage
        .tick_state_mut()
        .comparator_outputs
        .insert(pos, output.clamp(0, 15));

    let mut changed = false;
    if previous != Some(output) || !comparator_is_subtract(state) {
        let powered = is_powered(state);
        if powered && !should_turn_on {
            changed |= block_update::set_block(
                storage,
                pos.x(),
                pos.y(),
                pos.z(),
                with_powered(state, false),
            )?
            .is_some();
        } else if !powered && should_turn_on {
            changed |= block_update::set_block(
                storage,
                pos.x(),
                pos.y(),
                pos.z(),
                with_powered(state, true),
            )?
            .is_some();
        }
        storage
            .tick_state_mut()
            .neighbors
            .enqueue_neighbors(pos, UpdateFlags::ALL | UpdateFlags::SHAPE);
        crate::tick::process_pending_updates(storage)?;
    }
    Ok(changed)
}

/// Behaviour for `minecraft:observer`.
pub struct ObserverBehavior;

/// The shared observer behaviour.
pub static OBSERVER_BEHAVIOR: ObserverBehavior = ObserverBehavior;

impl BlockBehavior for ObserverBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if !is_observer(state) || old.is_some_and(|old| old.name == state.name) {
            return Ok(());
        }
        if is_powered(state)
            && !world
                .storage()
                .tick_state()
                .scheduled
                .has_scheduled_tick(pos, state.name.as_str())
        {
            world.set_block(pos, with_powered(state, false))?;
        }
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: Direction,
        _neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if is_observer(state) && direction == facing(state) && !is_powered(state) {
            // `ObserverBlock.startSignal`: the scheduled queue already
            // deduplicates `(position, block)`, matching
            // `hasScheduledTick(pos, this)`.
            world.schedule(pos, state.name.clone(), OBSERVER_PULSE_TICKS);
        }
        Ok(None)
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_observer(state) {
            return Ok(());
        }
        if is_powered(state) {
            world.set_block(pos, with_powered(state, false))?;
        } else {
            world.set_block(pos, with_powered(state, true))?;
            world.schedule(pos, state.name.clone(), OBSERVER_PULSE_TICKS);
        }
        Ok(())
    }
}

/// Behaviour for `minecraft:redstone_torch` and `minecraft:redstone_wall_torch`.
pub struct RedstoneTorchBehavior;

/// The shared redstone-torch behaviour.
pub static REDSTONE_TORCH_BEHAVIOR: RedstoneTorchBehavior = RedstoneTorchBehavior;

impl BlockBehavior for RedstoneTorchBehavior {
    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if !is_redstone_torch(state) {
            return Ok(());
        }
        let has_signal =
            torch_has_neighbor_signal(&SignalReader::world(world.storage()), pos, state);
        if torch_lit(state) == has_signal
            && !world
                .storage()
                .tick_state()
                .scheduled
                .has_scheduled_tick(pos, state.name.as_str())
        {
            world.schedule(pos, state.name.clone(), 2);
        }
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_redstone_torch(state) {
            return Ok(());
        }
        world.prune_torch_toggles();
        let has_signal =
            torch_has_neighbor_signal(&SignalReader::world(world.storage()), pos, state);
        if torch_lit(state) {
            if has_signal {
                world.set_block(pos, with_lit(state, false))?;
                if world.record_torch_toggle(pos) {
                    world.schedule_with_priority(
                        pos,
                        state.name.clone(),
                        REDSTONE_TORCH_RESTART_DELAY,
                        TickPriority::Normal,
                    );
                }
            }
        } else if !has_signal && world.torch_toggle_count(pos) < REDSTONE_TORCH_MAX_TOGGLES {
            world.set_block(pos, with_lit(state, true))?;
        }
        Ok(())
    }

    fn update_shape(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if !is_redstone_torch(state) {
            return Ok(None);
        }
        if is_wall_redstone_torch(state) {
            if direction == facing(state).opposite() && !block_behaviour::is_solid(neighbor) {
                return Ok(Some(air()));
            }
        } else if direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        Ok(None)
    }
}

/// Behaviour for every `minecraft:*_pressure_plate`.
pub struct PressurePlateBehavior;

/// The shared pressure-plate behaviour.
pub static PRESSURE_PLATE_BEHAVIOR: PressurePlateBehavior = PressurePlateBehavior;

impl BlockBehavior for PressurePlateBehavior {
    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_pressure_plate(state) {
            return Ok(());
        }
        // `BasePressurePlateBlock.tick` only re-checks a currently powered
        // plate; a stale tick must not re-press an unpowered one.
        if pressure_plate_state_signal(state) == 0 {
            return Ok(());
        }
        let occupancy = world.occupancy(pos);
        let strength = pressure_plate_signal(state, occupancy);
        let current = pressure_plate_state_signal(state);
        if current != strength {
            world.set_block(pos, pressure_plate_with_signal(state, strength))?;
        }
        if strength > 0 {
            world.schedule(pos, state.name.clone(), pressure_plate_pressed_time(state));
        }
        Ok(())
    }

    fn update_shape(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if is_pressure_plate(state) && direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        Ok(None)
    }
}

/// Behaviour for `minecraft:tripwire`.
pub struct TripwireBehavior;

/// The shared tripwire behaviour.
pub static TRIPWIRE_BEHAVIOR: TripwireBehavior = TripwireBehavior;

fn tripwire_connection_property(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        _ => "north",
    }
}

/// `TripWireBlock.shouldConnectTo`.
pub fn tripwire_should_connect_to(neighbor: &BlockState, direction: Direction) -> bool {
    if is_tripwire_hook(neighbor) {
        facing(neighbor) == direction.opposite()
    } else {
        is_tripwire(neighbor)
    }
}

impl BlockBehavior for TripwireBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if !is_tripwire(state) || old.is_some_and(|old| old.name == state.name) {
            return Ok(());
        }
        let mut shaped = state.clone();
        for direction in HORIZONTAL {
            let connected = world
                .block_state(pos.relative(direction))?
                .is_some_and(|neighbor| tripwire_should_connect_to(&neighbor, direction));
            set_property(
                &mut shaped,
                tripwire_connection_property(direction),
                if connected { "true" } else { "false" },
            );
        }
        if shaped != *state {
            world.set_block(pos, shaped)?;
        }
        Ok(())
    }

    fn update_shape(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if !is_tripwire(state) || !direction.is_horizontal() {
            return Ok(None);
        }
        let connected = tripwire_should_connect_to(neighbor, direction);
        let key = tripwire_connection_property(direction);
        if property(state, key) == Some(if connected { "true" } else { "false" }) {
            return Ok(None);
        }
        Ok(Some(with_property(
            state,
            key,
            if connected { "true" } else { "false" },
        )))
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if !is_tripwire(state) {
            return Ok(());
        }
        // `TripWireBlock.tick` only re-checks a currently powered wire.
        if !is_powered(state) {
            return Ok(());
        }
        let occupied = world.occupancy(pos).entities > 0;
        if is_powered(state) != occupied {
            world.set_block(pos, with_powered(state, occupied))?;
        }
        if occupied {
            world.schedule(pos, state.name.clone(), TRIPWIRE_RECHECK_TICKS);
        }
        Ok(())
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
    use crate::anvil::Compression;
    use crate::chunk::Chunk;
    use crate::chunk_packet::flat_chunk;
    use crate::registry::default_block_state;
    use crate::tick;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-redstone-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
        let root = temp_dir(name);
        let mut storage = WorldStorage::new(root.join("region"));
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.flush().unwrap();
        storage
    }

    fn flat_storage(name: &str) -> WorldStorage {
        storage_with_chunk(name, flat_chunk(0, 0))
    }

    fn place(storage: &mut WorldStorage, pos: BlockPos, state: BlockState) {
        block_update::set_block(storage, pos.x(), pos.y(), pos.z(), state)
            .unwrap()
            .expect("block change");
    }

    fn block(storage: &WorldStorage, pos: BlockPos) -> BlockState {
        storage
            .block_state_at(pos.x(), pos.y(), pos.z())
            .expect("block is cached")
    }

    fn flat_wire(power: i32) -> BlockState {
        wire_state(power)
    }

    #[test]
    fn weak_and_strong_queries_distinguish_lever_sides() {
        let mut storage = flat_storage("signal-lever");
        let pos = BlockPos::new(4, -60, 4);
        let lever = default_block_state(LEVER).unwrap();
        place(&mut storage, pos, with_powered(&lever, true));

        let reader = SignalReader::world(&storage);
        assert_eq!(reader.signal(pos, Direction::Up), 15);
        assert_eq!(reader.signal(pos, Direction::West), 15);
        // Default wall lever faces north; only its attached face is strong.
        assert_eq!(reader.direct_signal(pos, Direction::North), 15);
        assert_eq!(reader.direct_signal(pos, Direction::South), 0);
        assert_eq!(
            reader.control_input_signal(pos, Direction::North, false),
            15
        );
        assert_eq!(reader.control_input_signal(pos, Direction::Up, false), 0);

        place(&mut storage, pos, with_powered(&lever, false));
        assert_eq!(power_at(&storage, pos), 0);
        assert!(!has_neighbor_signal(&storage, pos.above()));
    }

    #[test]
    fn redstone_block_is_weak_power_only() {
        let mut storage = flat_storage("signal-block");
        let pos = BlockPos::new(2, -60, 2);
        place(
            &mut storage,
            pos,
            default_block_state(REDSTONE_BLOCK).unwrap(),
        );
        let reader = SignalReader::world(&storage);
        assert_eq!(reader.signal(pos, Direction::Up), 15);
        assert_eq!(reader.direct_signal(pos, Direction::Up), 0);
        assert_eq!(reader.control_input_signal(pos, Direction::Up, false), 15);
        assert_eq!(reader.best_neighbor_signal(pos.above()), 15);
    }

    #[test]
    fn lever_powers_wire_to_fifteen_and_it_decays() {
        let mut storage = flat_storage("wire-chain");
        let lever_pos = BlockPos::new(0, -60, 0);
        place(&mut storage, lever_pos, default_block_state(LEVER).unwrap());
        for x in 1..=5 {
            place(&mut storage, BlockPos::new(x, -60, 0), flat_wire(0));
        }

        assert!(toggle_lever(&mut storage, lever_pos).unwrap());
        assert!(is_powered(&block(&storage, lever_pos)));
        let expected = [15, 14, 13, 12, 11];
        for (x, power) in expected.into_iter().enumerate() {
            assert_eq!(
                wire_power(&block(&storage, BlockPos::new(x as i32 + 1, -60, 0))),
                power,
                "wire at x={}",
                x + 1
            );
        }

        assert!(toggle_lever(&mut storage, lever_pos).unwrap());
        assert!(!is_powered(&block(&storage, lever_pos)));
        for x in 1..=5 {
            assert_eq!(wire_power(&block(&storage, BlockPos::new(x, -60, 0))), 0);
        }
    }

    #[test]
    fn button_press_schedules_and_releases_after_twenty_ticks() {
        let mut storage = flat_storage("button");
        let pos = BlockPos::new(3, -60, 3);
        place(
            &mut storage,
            pos,
            default_block_state("minecraft:stone_button").unwrap(),
        );

        assert!(press_button(&mut storage, pos).unwrap());
        assert!(is_powered(&block(&storage, pos)));
        assert!(
            storage
                .tick_state()
                .scheduled
                .has_scheduled_tick(pos, "minecraft:stone_button")
        );
        assert_eq!(
            SignalReader::world(&storage).best_neighbor_signal(pos.above()),
            15
        );

        for tick_number in 1..=19 {
            tick::tick_world(&mut storage, tick_number).unwrap();
        }
        assert!(
            is_powered(&block(&storage, pos)),
            "stone buttons stay pressed for 20 ticks"
        );
        tick::tick_world(&mut storage, 20).unwrap();
        assert!(!is_powered(&block(&storage, pos)));
        assert_eq!(
            SignalReader::world(&storage).best_neighbor_signal(pos.above()),
            0
        );
        assert!(storage.tick_state().scheduled.is_empty());
    }

    #[test]
    fn use_block_dispatches_levers_and_buttons() {
        let mut storage = flat_storage("use-block");
        let lever = BlockPos::new(1, -60, 1);
        let button = BlockPos::new(2, -60, 2);
        place(&mut storage, lever, default_block_state(LEVER).unwrap());
        place(
            &mut storage,
            button,
            default_block_state("minecraft:oak_button").unwrap(),
        );

        assert!(use_block(&mut storage, lever).unwrap());
        assert!(is_powered(&block(&storage, lever)));
        assert!(use_block(&mut storage, button).unwrap());
        assert!(is_powered(&block(&storage, button)));
        assert_eq!(button_delay(&block(&storage, button)), 30);
    }

    #[test]
    fn wire_helpers_round_trip_power_and_connections() {
        let wire = wire_state(9);
        assert_eq!(wire_power(&wire), 9);
        assert_eq!(wire_signal(&wire, Direction::Up), 9);
        assert_eq!(wire_signal(&wire, Direction::North), 9);
        assert_eq!(wire_signal(&wire, Direction::Down), 0);
        assert_eq!(
            wire_side(&wire_with_power(&wire, 15), Direction::East),
            "side"
        );
        assert_eq!(wire_power(&wire_with_power(&wire, 99)), 15);
    }

    #[test]
    fn repeater_delay_property_maps_to_vanilla_ticks() {
        let repeater = default_block_state(REPEATER).unwrap();
        for (delay, ticks) in [(1, 2), (2, 4), (3, 6), (4, 8)] {
            let state = with_delay(&repeater, delay);
            assert_eq!(property(&state, "delay"), Some(delay.to_string().as_str()));
            assert_eq!(repeater_delay_ticks(&state), ticks);
        }
        assert_eq!(with_delay(&repeater, 99), with_delay(&repeater, 4));
        assert_eq!(with_delay(&repeater, 0), with_delay(&repeater, 1));
    }

    #[test]
    fn pressure_plate_signals_match_vanilla_variants() {
        let wooden = default_block_state("minecraft:oak_pressure_plate").unwrap();
        let stone = default_block_state("minecraft:stone_pressure_plate").unwrap();
        let light = default_block_state("minecraft:light_weighted_pressure_plate").unwrap();
        let heavy = default_block_state("minecraft:heavy_weighted_pressure_plate").unwrap();

        assert_eq!(
            pressure_plate_sensitivity(&wooden),
            PlateSensitivity::Everything
        );
        assert_eq!(pressure_plate_sensitivity(&stone), PlateSensitivity::Mobs);
        assert_eq!(
            pressure_plate_signal(
                &wooden,
                PlateOccupancy {
                    entities: 1,
                    living: 0
                }
            ),
            15
        );
        assert_eq!(
            pressure_plate_signal(
                &stone,
                PlateOccupancy {
                    entities: 1,
                    living: 0
                }
            ),
            0
        );
        assert_eq!(
            pressure_plate_signal(
                &stone,
                PlateOccupancy {
                    entities: 1,
                    living: 1
                }
            ),
            15
        );
        assert_eq!(
            pressure_plate_signal(
                &light,
                PlateOccupancy {
                    entities: 5,
                    living: 5
                }
            ),
            5
        );
        assert_eq!(
            pressure_plate_signal(
                &heavy,
                PlateOccupancy {
                    entities: 30,
                    living: 30
                }
            ),
            3
        );
        assert_eq!(pressure_plate_pressed_time(&wooden), 20);
        assert_eq!(pressure_plate_pressed_time(&light), 10);
        assert_eq!(pressure_plate_max_weight(&heavy), 150);
    }

    #[test]
    fn torch_lit_and_signal_rules_follow_vanilla_sides() {
        let floor = default_block_state(REDSTONE_TORCH).unwrap();
        assert!(torch_lit(&floor));
        assert_eq!(
            SignalReader::world(&flat_storage("torch-rules"))
                .signal(BlockPos::new(0, 0, 0), Direction::Up),
            0
        );
        assert_eq!(
            with_lit(&floor, false),
            with_property(&floor, "lit", "false")
        );
        assert_eq!(REDSTONE_TORCH_MAX_TOGGLES, 8);
        assert_eq!(REDSTONE_TORCH_TOGGLE_WINDOW, 60);
        assert_eq!(REDSTONE_TORCH_RESTART_DELAY, 160);
    }

    #[test]
    fn comparator_mode_cycle_and_output_helpers_are_consistent() {
        let comparator = default_block_state(COMPARATOR).unwrap();
        let subtract = with_mode(&comparator, "subtract");
        assert_eq!(property(&subtract, "mode"), Some("subtract"));
        assert!(comparator_is_subtract(&subtract));
        assert!(!comparator_is_subtract(&comparator));
        assert_eq!(comparator_delay(), 2);
        assert_eq!(
            facing(&default_block_state(OBSERVER).unwrap()),
            Direction::South
        );
        assert_eq!(
            with_facing(&comparator, Direction::West),
            with_property(&comparator, "facing", "west")
        );
    }
}
