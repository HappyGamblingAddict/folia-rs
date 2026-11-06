//! Redstone power queries and the wire/lever/button vertical slice.
//!
//! Reference: Folia 1.21.11 `SignalGetter`, `RedStoneWireBlock`,
//! `DefaultRedstoneWireEvaluator`, `LeverBlock`, `ButtonBlock`, and
//! `SignalGetter.getDirectSignalTo`/`getBestNeighborSignal`.
//!
//! Implemented:
//!
//! * strong (`getDirectSignal`) and weak (`getSignal`) power queries over the
//!   loaded world, including wire's side/connection rule;
//! * wire target strength: adjacent source power, or the strongest neighbouring
//!   wire minus one, capped at 15 and stopped at 0;
//! * wire updates flow through [`crate::tick`]'s neighbour queue, so identical
//!   worlds produce identical update order;
//! * lever toggling and button press with its vanilla 20/30-tick scheduled
//!   release.
//!
//! Not implemented: repeaters/comparators, torches, observers, pistons,
//! pressure plates, target blocks, redstone lamp reactions, wire connection
//! shape properties (power only), and block-entity power sources. Those stay
//! explicit gaps in `BLOCKERS.md`.

use crate::block_behaviour;
use crate::block_update;
use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::tick::{BlockBehavior, TickPriority, TickWorld};
use crate::world::WorldStorage;
use std::io;

/// `minecraft:redstone_wire`.
pub const REDSTONE_WIRE: &str = "minecraft:redstone_wire";
/// `minecraft:redstone_block`.
pub const REDSTONE_BLOCK: &str = "minecraft:redstone_block";
/// `minecraft:lever`.
pub const LEVER: &str = "minecraft:lever";

/// Vanilla `Direction.Plane.HORIZONTAL` order.
const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];

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
    pub fn signal(&self, pos: BlockPos, side: Direction) -> i32 {
        let Some(state) = self.block_state(pos) else {
            return 0;
        };
        let signal = self.state_signal(&state, side);
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
        self.state_direct_signal(&state, side)
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
            if matches!(
                state.name.as_str(),
                "minecraft:repeater" | "minecraft:comparator"
            ) {
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

    fn state_signal(&self, state: &BlockState, side: Direction) -> i32 {
        if is_wire(state) {
            if self.should_signal {
                wire_signal(state, side)
            } else {
                0
            }
        } else if is_power_source(state) {
            if is_powered(state) { 15 } else { 0 }
        } else if state.name == REDSTONE_BLOCK || is_lit_redstone_torch(state) {
            15
        } else {
            0
        }
    }

    fn state_direct_signal(&self, state: &BlockState, side: Direction) -> i32 {
        if is_wire(state) {
            // `RedStoneWireBlock.getDirectSignal` delegates to `getSignal`.
            return if self.should_signal {
                wire_signal(state, side)
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
///
/// Useful for placement/tests where the connection shape is not modeled yet.
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

/// Vanilla `ButtonBlock.getSignal` source check.
fn is_power_source(state: &BlockState) -> bool {
    is_lever(state) || is_button(state)
}

/// `BlockState.isSignalSource` for the ported sources.
pub fn is_signal_source(state: &BlockState) -> bool {
    is_power_source(state)
        || is_wire(state)
        || state.name == REDSTONE_BLOCK
        || is_lit_redstone_torch(state)
}

fn is_lit_redstone_torch(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch"
    ) && property(state, "lit") != Some("false")
}

/// `BlockState.getValue(POWERED)`.
pub fn is_powered(state: &BlockState) -> bool {
    property(state, "powered") == Some("true")
}

/// Clones a lever/button state with `powered` set.
pub fn with_powered(state: &BlockState, powered: bool) -> BlockState {
    let mut powered_state = state.clone();
    set_property(
        &mut powered_state,
        "powered",
        if powered { "true" } else { "false" },
    );
    powered_state
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`.
fn connected_direction(state: &BlockState) -> Direction {
    match property(state, "face").unwrap_or("wall") {
        "ceiling" => Direction::Down,
        "floor" => Direction::Up,
        _ => match property(state, "facing").unwrap_or("north") {
            "north" => Direction::North,
            "south" => Direction::South,
            "west" => Direction::West,
            "east" => Direction::East,
            _ => Direction::North,
        },
    }
}

/// `ButtonBlock` press duration, from the `ButtonBlock` registration.
pub fn button_delay(state: &BlockState) -> u64 {
    match state.name.as_str() {
        "minecraft:stone_button" | "minecraft:polished_blackstone_button" => 20,
        _ => 30,
    }
}

/// Flips a lever and updates its neighbours through the tick queue.
///
/// Returns `false` when the block is not a lever.
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
///
/// Returns `false` when the block is not a button or is already pressed.
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

/// Dispatches `useWithoutItem` for the ported interactive blocks.
///
/// This is the hook a future `ServerboundUseItemOn` handler should call for
/// levers and buttons.
pub fn use_block(storage: &mut WorldStorage, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    if is_lever(&state) {
        toggle_lever(storage, pos)
    } else if is_button(&state) {
        press_button(storage, pos)
    } else {
        Ok(false)
    }
}

/// Behaviour for `minecraft:redstone_wire`.
pub struct WireBehavior;

/// The shared wire behaviour.
pub static WIRE: WireBehavior = WireBehavior;

fn can_survive_on(state: &BlockState) -> bool {
    block_behaviour::is_solid(state) || state.name == "minecraft:hopper"
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    }
}

fn recompute_wire(world: &mut TickWorld<'_>, pos: BlockPos, state: &BlockState) -> io::Result<()> {
    let target = target_wire_power(world.storage(), pos);
    if target != wire_power(state) {
        world.set_block(pos, wire_with_power(state, target))?;
    }
    Ok(())
}

impl BlockBehavior for WireBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _old: Option<&BlockState>,
    ) -> io::Result<()> {
        if is_wire(state) {
            recompute_wire(world, pos, state)?;
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
            recompute_wire(world, pos, state)?;
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
        if direction == Direction::Down && !can_survive_on(neighbor) {
            return Ok(Some(air()));
        }
        let _ = state;
        Ok(None)
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
}
