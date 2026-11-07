//! Block update flags, neighbor updates, scheduled ticks, and random ticks.
//!
//! The reference implementation is Folia 1.21.11:
//!
//! * `net/minecraft/world/level/block/Block.UPDATE_*` and
//!   `BlockBehaviour.UPDATE_SHAPE_ORDER`
//! * `net/minecraft/world/level/redstone/NeighborUpdater`
//! * `net/minecraft/world/ticks/{ScheduledTick,TickPriority,LevelTicks,LevelChunkTicks}`
//! * `ServerLevel.tickChunk` / `LevelChunk.tickBlocks` (random ticks)
//! * `net/minecraft/world/level/block/{FallingBlock,VegetationBlock,GrassBlock}`
//!
//! This is deliberately a small vertical slice. Blocks that need the full
//! `P1.5` behaviour table (doors, leaf distance updates, crop growth, …) keep
//! their vanilla state in the world but their neighbor/random tick is an
//! explicit no-op. Water/lava flow lives in [`crate::fluid`]; wire, lever,
//! button, repeater, comparator, observer, redstone-torch, pressure-plate, and
//! tripwire behaviour lives in [`crate::redstone`]; pistons and moving blocks
//! live in [`crate::piston`].
//! [`unsupported_random_tick_reason`] and [`unsupported_neighbor_reason`] name
//! the remaining gaps and the tick state records every unsupported block a tick
//! actually reached, so a missing behaviour is visible instead of being
//! silently swallowed.

use crate::block_update::{BlockChange, perform_block_change};
use crate::chunk::{BlockState, Chunk, is_air_state};
use crate::math::{BlockPos, Direction};
use crate::random::{RandomSource, XoroshiroRandomSource};
use crate::tags;
use crate::world::WorldStorage;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;

/// Vanilla `Block.UPDATE_NEIGHBORS`.
pub const UPDATE_NEIGHBORS: u32 = 1;
/// Vanilla `Block.UPDATE_CLIENTS` (send a block update to clients).
pub const UPDATE_CLIENTS: u32 = 2;
/// Vanilla `Block.UPDATE_INVISIBLE`.
pub const UPDATE_INVISIBLE: u32 = 4;
/// Vanilla `Block.UPDATE_IMMEDIATE`.
pub const UPDATE_IMMEDIATE: u32 = 8;
/// Vanilla `Block.UPDATE_KNOWN_SHAPE` (suppress neighbour shape updates).
pub const UPDATE_KNOWN_SHAPE: u32 = 16;
/// Vanilla `Block.UPDATE_SUPPRESS_DROPS`.
pub const UPDATE_SUPPRESS_DROPS: u32 = 32;
/// Vanilla `Block.UPDATE_MOVE_BY_PISTON`.
pub const UPDATE_MOVE_BY_PISTON: u32 = 64;
/// Vanilla `Block.UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE`.
pub const UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE: u32 = 128;
/// Vanilla `Block.UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`.
pub const UPDATE_SKIP_BLOCK_ENTITY_SIDE_EFFECTS: u32 = 256;
/// Vanilla `Block.UPDATE_SKIP_ON_PLACE`.
pub const UPDATE_SKIP_ON_PLACE: u32 = 512;
/// Vanilla `Block.UPDATE_NONE`.
pub const UPDATE_NONE: u32 = 260;
/// Vanilla `Block.UPDATE_ALL`.
pub const UPDATE_ALL: u32 = 3;
/// Vanilla `Block.UPDATE_ALL_IMMEDIATE`.
pub const UPDATE_ALL_IMMEDIATE: u32 = 11;
/// Vanilla `Block.UPDATE_SKIP_ALL_SIDEEFFECTS`.
pub const UPDATE_SKIP_ALL_SIDE_EFFECTS: u32 = 816;

/// The vanilla `Block.UPDATE_*` flag set plus one Rust-side convenience bit.
///
/// Vanilla has no positive "do a shape update" bit: shape updates run unless
/// `UPDATE_KNOWN_SHAPE` (16) is set. [`UpdateFlags::SHAPE`] is a private-to-Rust
/// bit (1024) that makes a request explicit, so merging two queued requests can
/// never lose a requested shape update. [`normalize`] sets `SHAPE` whenever the
/// caller did not ask to suppress shape updates.
pub struct UpdateFlags;

impl UpdateFlags {
    /// Notify neighbours (`Block.UPDATE_NEIGHBORS`, 1).
    pub const NEIGHBORS: u32 = UPDATE_NEIGHBORS;
    /// Send the block change to clients (`Block.UPDATE_CLIENTS`, 2).
    pub const BLOCK_UPDATE: u32 = UPDATE_CLIENTS;
    /// Alias for [`Self::BLOCK_UPDATE`].
    pub const CLIENTS: u32 = UPDATE_CLIENTS;
    /// Do not send the change to clients (`Block.UPDATE_INVISIBLE`, 4).
    pub const INVISIBLE: u32 = UPDATE_INVISIBLE;
    /// Process the update immediately (`Block.UPDATE_IMMEDIATE`, 8).
    pub const IMMEDIATE: u32 = UPDATE_IMMEDIATE;
    /// The neighbour already knows its shape, so skip shape updates
    /// (`Block.UPDATE_KNOWN_SHAPE`, 16).
    pub const KNOWN_SHAPE: u32 = UPDATE_KNOWN_SHAPE;
    /// Alias for [`Self::KNOWN_SHAPE`]: suppress neighbour shape updates.
    pub const SUPPRESS_NEIGHBORS: u32 = UPDATE_KNOWN_SHAPE;
    /// Suppress drops (`Block.UPDATE_SUPPRESS_DROPS`, 32).
    pub const SUPPRESS_DROPS: u32 = UPDATE_SUPPRESS_DROPS;
    /// The change is caused by a piston (`Block.UPDATE_MOVE_BY_PISTON`, 64).
    pub const MOVE_BY_PISTON: u32 = UPDATE_MOVE_BY_PISTON;
    /// Skip the redstone-wire shape hook (`Block.UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE`, 128).
    pub const SKIP_SHAPE_UPDATE_ON_WIRE: u32 = UPDATE_SKIP_SHAPE_UPDATE_ON_WIRE;
    /// Skip block-entity side effects (`Block.UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`, 256).
    pub const SKIP_BLOCK_ENTITY_SIDE_EFFECTS: u32 = UPDATE_SKIP_BLOCK_ENTITY_SIDE_EFFECTS;
    /// Skip `onPlace` (`Block.UPDATE_SKIP_ON_PLACE`, 512).
    pub const SKIP_ON_PLACE: u32 = UPDATE_SKIP_ON_PLACE;
    /// Vanilla `Block.UPDATE_NONE`.
    pub const NONE: u32 = UPDATE_NONE;
    /// Vanilla `Block.UPDATE_ALL` (neighbours + clients).
    pub const ALL: u32 = UPDATE_ALL;
    /// Vanilla `Block.UPDATE_ALL_IMMEDIATE`.
    pub const ALL_IMMEDIATE: u32 = UPDATE_ALL_IMMEDIATE;
    /// Vanilla `Block.UPDATE_SKIP_ALL_SIDEEFFECTS`.
    pub const SKIP_ALL_SIDE_EFFECTS: u32 = UPDATE_SKIP_ALL_SIDE_EFFECTS;
    /// Rust-side explicit shape-update request (not a vanilla bit).
    pub const SHAPE: u32 = 1 << 10;

    /// Returns whether these flags request neighbour `neighborChanged` calls.
    pub const fn has_neighbors(flags: u32) -> bool {
        flags & Self::NEIGHBORS != 0
    }

    /// Returns whether these flags request a neighbour shape update.
    ///
    /// The queued form always carries [`Self::SHAPE`], so a merged request can
    /// never be suppressed by a later `KNOWN_SHAPE` request.
    pub const fn has_shape(flags: u32) -> bool {
        flags & Self::SHAPE != 0
    }

    /// Adds [`Self::SHAPE`] when the caller did not suppress shape updates.
    pub const fn normalize(flags: u32) -> u32 {
        if flags & Self::SHAPE != 0 || flags & Self::KNOWN_SHAPE == 0 {
            flags | Self::SHAPE
        } else {
            flags
        }
    }
}

/// Vanilla `TickPriority`, ordered from most urgent (-3) to least urgent (3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TickPriority {
    ExtremelyHigh,
    VeryHigh,
    High,
    #[default]
    Normal,
    Low,
    VeryLow,
    ExtremelyLow,
}

impl TickPriority {
    /// The wire/saved numeric value (`-3..=3`).
    pub const fn value(self) -> i32 {
        match self {
            Self::ExtremelyHigh => -3,
            Self::VeryHigh => -2,
            Self::High => -1,
            Self::Normal => 0,
            Self::Low => 1,
            Self::VeryLow => 2,
            Self::ExtremelyLow => 3,
        }
    }

    /// Ports `TickPriority.byValue`, including the out-of-range clamp.
    pub const fn by_value(value: i32) -> Self {
        if value < -3 {
            Self::ExtremelyHigh
        } else if value > 3 {
            Self::ExtremelyLow
        } else {
            match value {
                -3 => Self::ExtremelyHigh,
                -2 => Self::VeryHigh,
                -1 => Self::High,
                1 => Self::Low,
                2 => Self::VeryLow,
                3 => Self::ExtremelyLow,
                _ => Self::Normal,
            }
        }
    }

    /// Java's `Enum.compareTo` order.
    pub fn cmp_value(self, other: Self) -> Ordering {
        (self.value() as i8).cmp(&(other.value() as i8))
    }
}

/// A scheduled block tick, mirroring `ScheduledTick<Block>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledTick {
    /// Absolute block position.
    pub pos: BlockPos,
    /// Block name (`minecraft:sand`). The tick only runs while the block at
    /// `pos` still has this name, like vanilla's `blockState.is(block)` check.
    pub block: String,
    /// Game tick at which this tick becomes due.
    pub trigger_tick: u64,
    /// Vanilla priority, compared before the sub-tick order.
    pub priority: TickPriority,
    /// Insertion order within one `(trigger_tick, priority)` bucket.
    pub sub_tick_order: i64,
}

impl ScheduledTick {
    /// Java `ScheduledTick.INTRA_TICK_DRAIN_ORDER`: priority first, then the
    /// sub-tick order.
    pub fn intra_tick_cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp_value(other.priority)
            .then_with(|| self.sub_tick_order.cmp(&other.sub_tick_order))
    }

    /// Java `ScheduledTick.DRAIN_ORDER`: trigger tick, priority, sub-tick.
    pub fn drain_cmp(&self, other: &Self) -> Ordering {
        self.trigger_tick
            .cmp(&other.trigger_tick)
            .then_with(|| self.intra_tick_cmp(other))
    }
}

/// Deterministic queue for `ScheduledTick`s.
///
/// Ordering matches `LevelTicks`: current tick bucket, then priority, then the
/// sub-tick order assigned at insertion. A `(position, block)` pair can only be
/// queued once, matching `LevelChunkTicks.ticksPerPosition`.
#[derive(Debug, Default)]
pub struct ScheduledTickQueue {
    pending: BTreeMap<(u64, TickPriority, i64), ScheduledTick>,
    unique: HashMap<(BlockPos, String), (u64, TickPriority, i64)>,
    next_sub_tick_order: i64,
}

impl ScheduledTickQueue {
    /// Creates an empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedules a tick. Returns false when the same `(position, block)` is
    /// already queued, matching `LevelChunkTicks.schedule`.
    pub fn schedule(&mut self, tick: ScheduledTick) -> bool {
        let key = (tick.pos, tick.block.clone());
        if self.unique.contains_key(&key) {
            return false;
        }
        let bucket = (tick.trigger_tick, tick.priority, tick.sub_tick_order);
        self.unique.insert(key, bucket);
        self.pending.insert(bucket, tick);
        true
    }

    /// Schedules a block tick `delay` ticks from `current_tick`.
    ///
    /// `delay = 0` is due immediately (on the next `tick` call that is at or
    /// past `current_tick`), matching vanilla's collection-before-run order.
    pub fn schedule_after(
        &mut self,
        current_tick: u64,
        pos: BlockPos,
        block: impl Into<String>,
        delay: u64,
        priority: TickPriority,
    ) -> bool {
        let sub_tick_order = self.next_sub_tick_order;
        self.next_sub_tick_order = self.next_sub_tick_order.wrapping_add(1);
        self.schedule(ScheduledTick {
            pos,
            block: block.into(),
            trigger_tick: current_tick.saturating_add(delay),
            priority,
            sub_tick_order,
        })
    }

    /// Returns whether `(pos, block)` is currently queued.
    pub fn has_scheduled_tick(&self, pos: BlockPos, block: &str) -> bool {
        self.unique.contains_key(&(pos, block.to_owned()))
    }

    /// Number of queued ticks.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// True when no tick is queued.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Cancels every tick at `pos`, like `LevelTicks.clearArea` on one block.
    /// Returns the number removed.
    pub fn cancel(&mut self, pos: BlockPos) -> usize {
        let keys: Vec<(BlockPos, String)> = self
            .unique
            .keys()
            .filter(|(candidate, _)| *candidate == pos)
            .cloned()
            .collect();
        let mut removed = 0;
        for key in keys {
            if let Some(bucket) = self.unique.remove(&key) {
                self.pending.remove(&bucket);
                removed += 1;
            }
        }
        removed
    }

    /// Cancels one `(pos, block)` tick.
    pub fn cancel_block(&mut self, pos: BlockPos, block: &str) -> bool {
        let key = (pos, block.to_owned());
        let Some(bucket) = self.unique.remove(&key) else {
            return false;
        };
        self.pending.remove(&bucket);
        true
    }

    /// Removes every tick accepted by `predicate`, like `LevelTicks.clearArea`.
    pub fn cancel_where(&mut self, mut predicate: impl FnMut(&ScheduledTick) -> bool) -> usize {
        let remove: Vec<(u64, TickPriority, i64)> = self
            .pending
            .iter()
            .filter(|(_, tick)| predicate(tick))
            .map(|(bucket, _)| *bucket)
            .collect();
        for bucket in &remove {
            if let Some(tick) = self.pending.remove(bucket) {
                let key = (tick.pos, tick.block);
                self.unique.remove(&key);
            }
        }
        remove.len()
    }

    /// Collects and removes ticks due at `current_tick` in deterministic
    /// `LevelTicks` order.
    pub fn tick(&mut self, current_tick: u64) -> Vec<ScheduledTick> {
        self.collect_due(current_tick)
    }

    /// Collects due ticks and invokes `callback` on each.
    ///
    /// Cancellation is supported between ticks. A callback cannot cancel a
    /// sibling tick that was already collected for the same tick; vanilla's
    /// `clearArea` can, and that is recorded as a P3.10 gap in `BLOCKERS.md`.
    pub fn tick_with(
        &mut self,
        current_tick: u64,
        mut callback: impl FnMut(&ScheduledTick),
    ) -> usize {
        let due = self.collect_due(current_tick);
        let ran = due.len();
        for tick in due {
            callback(&tick);
        }
        ran
    }

    fn collect_due(&mut self, current_tick: u64) -> Vec<ScheduledTick> {
        let buckets: Vec<(u64, TickPriority, i64)> = self
            .pending
            .range(..=(current_tick, TickPriority::ExtremelyLow, i64::MAX))
            .map(|(bucket, _)| *bucket)
            .collect();
        let mut due = Vec::with_capacity(buckets.len());
        for bucket in buckets {
            if let Some(tick) = self.pending.remove(&bucket) {
                let key = (tick.pos, tick.block.clone());
                self.unique.remove(&key);
                due.push(tick);
            }
        }
        due
    }
}

/// One queued neighbour update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborUpdate {
    /// Position whose block receives the update.
    pub pos: BlockPos,
    /// Position of the block that changed.
    pub source: BlockPos,
    /// Merged update flags.
    pub flags: u32,
}

/// A deterministic, chunk-grouped neighbour-update queue.
///
/// Entries keep the insertion order of their first request inside a chunk, and
/// chunks drain in ascending `(chunk_x, chunk_z)` order. Re-enqueuing a position
/// merges the flags with bitwise OR (and normalizes the Rust `SHAPE` request),
/// so deduplication never loses a stronger update.
#[derive(Debug, Default)]
pub struct NeighborUpdateQueue {
    chunks: BTreeMap<(i32, i32), Vec<NeighborUpdate>>,
    pending: HashMap<BlockPos, (u32, (i32, i32), usize)>,
}

impl NeighborUpdateQueue {
    /// Creates an empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues `pos` with `source` and `flags`, merging repeated positions.
    pub fn enqueue(&mut self, source: BlockPos, pos: BlockPos, flags: u32) {
        let flags = UpdateFlags::normalize(flags);
        if let Some(&(current, chunk, index)) = self.pending.get(&pos) {
            let merged = current | flags;
            self.pending.insert(pos, (merged, chunk, index));
            if let Some(update) = self
                .chunks
                .get_mut(&chunk)
                .and_then(|updates| updates.get_mut(index))
            {
                update.flags = merged;
            }
            return;
        }

        let chunk = (pos.x().div_euclid(16), pos.z().div_euclid(16));
        let updates = self.chunks.entry(chunk).or_default();
        let index = updates.len();
        updates.push(NeighborUpdate { pos, source, flags });
        self.pending.insert(pos, (flags, chunk, index));
    }

    /// Enqueues all six neighbours of `source` in vanilla `UPDATE_ORDER`
    /// (`WEST, EAST, DOWN, UP, NORTH, SOUTH`).
    pub fn enqueue_neighbors(&mut self, source: BlockPos, flags: u32) {
        for direction in NEIGHBOR_UPDATE_ORDER {
            self.enqueue(source, source.relative(direction), flags);
        }
    }

    /// Number of distinct queued positions.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// True when no update is queued.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Removes and returns the next update in deterministic chunk and
    /// insertion order.
    pub fn pop(&mut self) -> Option<NeighborUpdate> {
        let chunk = *self.chunks.keys().next()?;
        let updates = self.chunks.get_mut(&chunk)?;
        let update = updates.remove(0);
        self.pending.remove(&update.pos);
        for (index, remaining) in updates.iter().enumerate() {
            if let Some(entry) = self.pending.get_mut(&remaining.pos) {
                entry.2 = index;
            }
        }
        if updates.is_empty() {
            self.chunks.remove(&chunk);
        }
        Some(update)
    }

    /// Removes and returns every queued update in deterministic chunk and
    /// insertion order.
    pub fn drain(&mut self) -> Vec<NeighborUpdate> {
        let chunks = std::mem::take(&mut self.chunks);
        self.pending.clear();
        let mut drained = Vec::new();
        for (_, updates) in chunks {
            drained.extend(updates);
        }
        drained
    }

    /// Removes and returns updates for one chunk only. Used by a future
    /// region-scoped tick loop; the world tick drains every chunk.
    pub fn drain_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Vec<NeighborUpdate> {
        let Some(updates) = self.chunks.remove(&(chunk_x, chunk_z)) else {
            return Vec::new();
        };
        for update in &updates {
            self.pending.remove(&update.pos);
        }
        updates
    }
}

/// Vanilla `NeighborUpdater.UPDATE_ORDER`.
pub const NEIGHBOR_UPDATE_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::Down,
    Direction::Up,
    Direction::North,
    Direction::South,
];

/// Vanilla `BlockBehaviour.UPDATE_SHAPE_ORDER`.
pub const UPDATE_SHAPE_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::North,
    Direction::South,
    Direction::Down,
    Direction::Up,
];

/// A random tick sample selected by [`RandomTickQueue`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandomTickSample {
    /// Absolute block position.
    pub pos: BlockPos,
}

/// Vanilla-style random tick sampler.
///
/// For every 16x16x16 section, `randomTickSpeed` positions are drawn with
/// three `nextInt(16)` calls, like the original `LevelChunk.tickBlocks` loop
/// (Folia's `optimiseRandomTick` samples the same positions from a per-section
/// list). The per-chunk RNG is derived from the world seed, chunk coordinates,
/// and game tick, so the same world and tick always select the same positions
/// regardless of hash-map iteration order.
#[derive(Clone, Debug)]
pub struct RandomTickQueue {
    /// World seed, replaced by `WorldStorage::set_tick_seed`.
    pub seed: i64,
    /// Vanilla `randomTickSpeed` (default 3).
    pub random_tick_speed: u32,
}

impl Default for RandomTickQueue {
    fn default() -> Self {
        Self {
            seed: 0,
            random_tick_speed: 3,
        }
    }
}

impl RandomTickQueue {
    /// Creates a sampler with vanilla speed 3.
    pub fn new(seed: i64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    /// Overrides the vanilla random tick speed.
    pub fn with_speed(mut self, speed: u32) -> Self {
        self.random_tick_speed = speed;
        self
    }

    /// Returns the sampled positions for a chunk in deterministic order.
    pub fn sample_positions(&self, chunk: &Chunk, tick: u64) -> Vec<BlockPos> {
        let sections: Vec<i8> = chunk.sections.iter().map(|section| section.y).collect();
        self.sample_positions_in_sections(chunk.x, chunk.z, &sections, tick)
    }

    /// Samples a chunk from its section-coordinate list instead of cloning the
    /// chunk. `sections` is sorted internally, so the result matches
    /// [`Self::sample_positions`].
    pub fn sample_positions_in_sections(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        sections: &[i8],
        tick: u64,
    ) -> Vec<BlockPos> {
        let mut positions = Vec::with_capacity(self.random_tick_speed as usize * sections.len());
        let mut sorted_sections = sections.to_vec();
        sorted_sections.sort_unstable();
        let mut rng = XoroshiroRandomSource::new(sample_seed(self.seed, chunk_x, chunk_z, tick));
        for section_y in sorted_sections {
            let base_y = i32::from(section_y) * 16;
            for _ in 0..self.random_tick_speed {
                let x = rng.next_int_bound(16);
                let y = rng.next_int_bound(16);
                let z = rng.next_int_bound(16);
                positions.push(BlockPos::new(
                    chunk_x * 16 + x,
                    base_y + y,
                    chunk_z * 16 + z,
                ));
            }
        }
        positions
    }

    /// Samples a chunk and invokes `callback` for every selected block accepted
    /// by `predicate`. Returns the number of callbacks that reported a change.
    pub fn tick_chunk(
        &self,
        chunk: &Chunk,
        tick: u64,
        predicate: impl Fn(&BlockState) -> bool,
        mut callback: impl FnMut(BlockPos) -> io::Result<bool>,
    ) -> io::Result<usize> {
        let mut applied = 0;
        let mut error = None;
        self.for_each_sample(chunk, tick, |pos, state| {
            if error.is_some() {
                return;
            }
            let Some(state) = state else {
                return;
            };
            if !predicate(state) {
                return;
            }
            match callback(pos) {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(err) => error = Some(err),
            }
        });
        match error {
            Some(error) => Err(error),
            None => Ok(applied),
        }
    }

    fn for_each_sample(
        &self,
        chunk: &Chunk,
        tick: u64,
        mut visit: impl FnMut(BlockPos, Option<&BlockState>),
    ) {
        let mut rng = XoroshiroRandomSource::new(sample_seed(self.seed, chunk.x, chunk.z, tick));
        let mut sections: Vec<_> = chunk.sections.iter().collect();
        sections.sort_by_key(|section| section.y);
        for section in sections {
            let base_y = i32::from(section.y) * 16;
            for _ in 0..self.random_tick_speed {
                let x = rng.next_int_bound(16) as usize;
                let y = rng.next_int_bound(16) as usize;
                let z = rng.next_int_bound(16) as usize;
                let index = (y * 16 + z) * 16 + x;
                let state = section.block_states.get(index);
                visit(
                    BlockPos::new(
                        chunk.x * 16 + x as i32,
                        base_y + y as i32,
                        chunk.z * 16 + z as i32,
                    ),
                    state,
                );
            }
        }
    }
}

/// A block-entity-like container that can drive a comparator.
///
/// Chests, furnaces, hoppers, barrels, and the other vanilla containers will
/// implement this and register themselves in [`TickState::containers`] at their
/// block position. The redstone module then never needs to know which inventory
/// block is present, and inventory changes can call
/// [`crate::redstone::notify_container_changed`] to make the comparator
/// re-evaluate on its normal two-tick schedule.
pub trait ContainerSignalSource: Send + Sync {
    /// Vanilla `Container.getAnalogOutputSignal` (`0..=15`).
    fn analog_output_signal(&self) -> i32;
}

/// Position-keyed container signal sources for comparator inputs.
#[derive(Default)]
pub struct ContainerSignals {
    sources: HashMap<BlockPos, Box<dyn ContainerSignalSource>>,
}

impl ContainerSignals {
    /// Registers (or replaces) the container at `pos`.
    pub fn insert(&mut self, pos: BlockPos, source: Box<dyn ContainerSignalSource>) {
        self.sources.insert(pos, source);
    }

    /// Removes the container at `pos`, returning whether one was present.
    pub fn remove(&mut self, pos: BlockPos) -> bool {
        self.sources.remove(&pos).is_some()
    }

    /// The analog output at `pos`, clamped to the comparator input range.
    pub fn signal(&self, pos: BlockPos) -> Option<i32> {
        self.sources
            .get(&pos)
            .map(|source| source.analog_output_signal().clamp(0, 15))
    }

    /// Number of registered containers.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// True when no container is registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Drops every registered container.
    pub fn clear(&mut self) {
        self.sources.clear();
    }
}

impl std::fmt::Debug for ContainerSignals {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerSignals")
            .field("len", &self.sources.len())
            .finish()
    }
}

/// One neighbour update that was handed to a block behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessedNeighborUpdate {
    /// Position whose block was updated.
    pub pos: BlockPos,
    /// Position of the block that changed.
    pub source: BlockPos,
    /// Flags that were in effect.
    pub flags: u32,
}

/// Per-storage tick state: queues, the current game tick, and diagnostics.
#[derive(Debug, Default)]
pub struct TickState {
    /// The tick currently being processed, or the last tick an edit used for
    /// scheduling.
    pub current_tick: u64,
    /// Pending neighbour updates.
    pub neighbors: NeighborUpdateQueue,
    /// Pending scheduled block ticks.
    pub scheduled: ScheduledTickQueue,
    /// Random-tick sampler settings.
    pub random: RandomTickQueue,
    /// Block-entity containers that expose a comparator signal.
    pub containers: ContainerSignals,
    /// Comparator output strength learned by the last scheduled refresh.
    ///
    /// Vanilla stores this in `ComparatorBlockEntity`. The port keeps it on the
    /// tick state until block entities are serialized; the value is cleared when
    /// the comparator block is replaced.
    pub comparator_outputs: HashMap<BlockPos, i32>,
    /// Occupancy counts supplied by the entity layer for pressure plates and
    /// tripwire. The block behaviours never count entities themselves.
    pub occupancy: HashMap<BlockPos, crate::redstone::PlateOccupancy>,
    /// Redstone-torch off toggles used for vanilla's 8-in-60-tick burn-out.
    pub torch_toggles: Vec<crate::redstone::TorchToggle>,
    /// Moving-piston payloads keyed by their moving-block position.
    pub pistons: crate::piston::PistonState,
    /// Neighbour updates processed during the current top-level edit/tick.
    pub last_neighbor_updates: Vec<ProcessedNeighborUpdate>,
    /// Blocks whose random tick was selected but is explicitly unsupported.
    pub unsupported_random_ticks: BTreeSet<String>,
    /// Blocks whose neighbour update was received but is explicitly
    /// unsupported.
    pub unsupported_neighbor_blocks: BTreeSet<String>,
    /// True when a chained neighbour update exceeded vanilla's limit.
    pub overflowed: bool,
}

/// Result counts from [`tick_world`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub tick: u64,
    pub scheduled_ticks: usize,
    pub random_ticks: usize,
    pub neighbor_updates: usize,
    pub block_changes: usize,
    pub overflowed: bool,
}

/// Maximum chained neighbour updates before the rest are skipped, matching
/// vanilla's default `maxChainedNeighborUpdates`.
pub const MAX_CHAINED_NEIGHBOR_UPDATES: usize = 512;

/// A minimal block behaviour hook table.
///
/// Blocks that need full `P1.5` metadata are not registered here. The default
/// implementations are deliberate no-ops; [`unsupported_random_tick_reason`]
/// and [`unsupported_neighbor_reason`] make the gaps explicit. A caller can add
/// a hook for a future block with [`BehaviorTable::with`].
pub trait BlockBehavior: Send + Sync {
    fn is_randomly_ticking(&self, _state: &BlockState) -> bool {
        false
    }

    fn random_tick(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
    ) -> io::Result<()> {
        Ok(())
    }

    fn scheduled_tick(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
    ) -> io::Result<()> {
        Ok(())
    }

    fn neighbor_changed(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        Ok(())
    }

    /// Returns a replacement state, or `None` when the shape is unchanged.
    fn update_shape(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _direction: Direction,
        _neighbor: &BlockState,
        _state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        Ok(None)
    }

    fn on_place(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
        _old: Option<&BlockState>,
    ) -> io::Result<()> {
        Ok(())
    }
}

/// Lookup table for [`BlockBehavior`]s, with per-block overrides.
#[derive(Default)]
pub struct BehaviorTable {
    overrides: HashMap<String, &'static dyn BlockBehavior>,
}

impl BehaviorTable {
    /// The built-in hard-coded behaviours only.
    pub fn vanilla() -> Self {
        Self::default()
    }

    /// Returns a table with one block-name override. This is the callback
    /// interface for blocks that will be ported later.
    pub fn with(mut self, block: impl Into<String>, behavior: &'static dyn BlockBehavior) -> Self {
        self.overrides.insert(block.into(), behavior);
        self
    }

    /// Resolves the behaviour for `state`.
    pub fn get(&self, state: &BlockState) -> &'static dyn BlockBehavior {
        self.overrides
            .get(state.name.as_str())
            .copied()
            .unwrap_or_else(|| behavior_for(state))
    }
}

/// Mutable world view handed to a block behaviour.
pub struct TickWorld<'a> {
    storage: &'a mut WorldStorage,
    tick: u64,
    changed: bool,
}

impl<'a> TickWorld<'a> {
    fn new(storage: &'a mut WorldStorage, tick: u64) -> Self {
        Self {
            storage,
            tick,
            changed: false,
        }
    }

    /// The game tick this behaviour runs on.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Returns the current state at `pos`, if its chunk is loaded.
    pub fn block_state(&mut self, pos: BlockPos) -> io::Result<Option<BlockState>> {
        Ok(self.storage.block_state_at(pos.x(), pos.y(), pos.z()))
    }

    /// Read-only storage view for signal/fluid queries.
    pub fn storage(&self) -> &WorldStorage {
        self.storage
    }

    /// Applies a block change and returns whether it changed anything.
    ///
    /// Neighbour updates are queued and processed by the enclosing tick; a
    /// behaviour never re-enters the queue itself.
    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> io::Result<bool> {
        self.storage.tick_state_mut().current_tick = self.tick;
        let change = perform_block_change(
            self.storage,
            pos,
            state,
            UpdateFlags::ALL | UpdateFlags::SHAPE,
            false,
            true,
        )?;
        if change.is_some() {
            self.changed = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Schedules a block tick `delay` ticks from this tick.
    pub fn schedule(&mut self, pos: BlockPos, block: impl Into<String>, delay: u64) {
        self.storage.tick_state_mut().scheduled.schedule_after(
            self.tick,
            pos,
            block,
            delay,
            TickPriority::Normal,
        );
    }

    /// Schedules a block tick with an explicit priority.
    pub fn schedule_with_priority(
        &mut self,
        pos: BlockPos,
        block: impl Into<String>,
        delay: u64,
        priority: TickPriority,
    ) {
        self.storage
            .tick_state_mut()
            .scheduled
            .schedule_after(self.tick, pos, block, delay, priority);
    }

    /// Queues the six neighbours of `pos` without changing the block.
    ///
    /// Vanilla diodes call `updateNeighborsInFront` after refreshing their
    /// output, even when the block state itself did not change.
    pub fn notify_neighbors(&mut self, pos: BlockPos) {
        self.storage
            .tick_state_mut()
            .neighbors
            .enqueue_neighbors(pos, UpdateFlags::ALL | UpdateFlags::SHAPE);
    }

    /// Queues one neighbour update at `pos` from `source`.
    pub fn queue_neighbor_update(&mut self, source: BlockPos, pos: BlockPos, flags: u32) {
        self.storage
            .tick_state_mut()
            .neighbors
            .enqueue(source, pos, flags);
    }

    /// The moving-piston payload at `pos`, if any.
    pub fn piston_payload(&self, pos: BlockPos) -> Option<crate::piston::MovingPiston> {
        self.storage.tick_state().pistons.get(pos).cloned()
    }

    /// Stores a moving-piston payload at `pos`.
    pub fn set_piston_payload(&mut self, pos: BlockPos, payload: crate::piston::MovingPiston) {
        self.storage.tick_state_mut().pistons.insert(pos, payload);
    }

    /// Removes and returns the moving-piston payload at `pos`.
    pub fn remove_piston_payload(&mut self, pos: BlockPos) -> Option<crate::piston::MovingPiston> {
        self.storage.tick_state_mut().pistons.remove(pos)
    }

    /// Moving-piston payload positions in deterministic order.
    pub fn piston_positions(&self) -> Vec<BlockPos> {
        self.storage.tick_state().pistons.positions().collect()
    }

    /// The comparator output stored for `pos` (`ComparatorBlockEntity`).
    pub fn comparator_output(&self, pos: BlockPos) -> i32 {
        self.storage
            .tick_state()
            .comparator_outputs
            .get(&pos)
            .copied()
            .unwrap_or(0)
    }

    /// Stores a comparator output strength (`0..=15`).
    pub fn set_comparator_output(&mut self, pos: BlockPos, output: i32) {
        self.storage
            .tick_state_mut()
            .comparator_outputs
            .insert(pos, output.clamp(0, 15));
    }

    /// Occupancy counts recorded by the entity layer for `pos`.
    pub fn occupancy(&self, pos: BlockPos) -> crate::redstone::PlateOccupancy {
        self.storage
            .tick_state()
            .occupancy
            .get(&pos)
            .copied()
            .unwrap_or_default()
    }

    /// Replaces the occupancy counts for `pos`.
    pub fn set_occupancy(&mut self, pos: BlockPos, occupancy: crate::redstone::PlateOccupancy) {
        self.storage
            .tick_state_mut()
            .occupancy
            .insert(pos, occupancy);
    }

    /// Drops torch toggles older than the vanilla 60-tick window.
    pub fn prune_torch_toggles(&mut self) {
        let tick = self.tick;
        self.storage
            .tick_state_mut()
            .torch_toggles
            .retain(|toggle| {
                tick.saturating_sub(toggle.tick) <= crate::redstone::REDSTONE_TORCH_TOGGLE_WINDOW
            });
    }

    /// Records one redstone-torch off toggle and reports whether the torch has
    /// now toggled at least eight times inside the 60-tick window.
    pub fn record_torch_toggle(&mut self, pos: BlockPos) -> bool {
        self.prune_torch_toggles();
        let tick = self.tick;
        let toggles = &mut self.storage.tick_state_mut().torch_toggles;
        toggles.push(crate::redstone::TorchToggle { pos, tick });
        toggles.iter().filter(|toggle| toggle.pos == pos).count()
            >= crate::redstone::REDSTONE_TORCH_MAX_TOGGLES
    }

    /// Number of recorded off toggles for `pos` inside the current window.
    pub fn torch_toggle_count(&self, pos: BlockPos) -> usize {
        self.storage
            .tick_state()
            .torch_toggles
            .iter()
            .filter(|toggle| toggle.pos == pos)
            .count()
    }

    /// A per-position deterministic random source.
    pub fn random(&self, pos: BlockPos) -> XoroshiroRandomSource {
        let seed = self.storage.tick_state().random.seed;
        XoroshiroRandomSource::new(sample_seed(seed, pos.x(), pos.z(), self.tick))
    }

    /// World seed used by random ticks.
    pub fn seed(&self) -> i64 {
        self.storage.tick_state().random.seed
    }

    /// True when this behaviour changed at least one block.
    pub fn changed(&self) -> bool {
        self.changed
    }
}

struct NoopBehavior;
static NOOP: NoopBehavior = NoopBehavior;

impl BlockBehavior for NoopBehavior {}

struct FallingBehavior;
static FALLING: FallingBehavior = FallingBehavior;

struct PlantBehavior;
static PLANT: PlantBehavior = PlantBehavior;

struct GrassBehavior;
static GRASS: GrassBehavior = GrassBehavior;

/// Returns the built-in behaviour for a block state.
pub fn behavior_for(state: &BlockState) -> &'static dyn BlockBehavior {
    if crate::fluid::fluid_state(state).is_some() {
        &crate::fluid::FLUID
    } else if crate::redstone::is_wire(state) {
        &crate::redstone::WIRE
    } else if crate::redstone::is_lever(state) {
        &crate::redstone::LEVER_BEHAVIOR
    } else if crate::redstone::is_button(state) {
        &crate::redstone::BUTTON_BEHAVIOR
    } else if crate::redstone::is_repeater(state) {
        &crate::redstone::REPEATER_BEHAVIOR
    } else if crate::redstone::is_comparator(state) {
        &crate::redstone::COMPARATOR_BEHAVIOR
    } else if crate::redstone::is_observer(state) {
        &crate::redstone::OBSERVER_BEHAVIOR
    } else if crate::redstone::is_redstone_torch(state) {
        &crate::redstone::REDSTONE_TORCH_BEHAVIOR
    } else if crate::redstone::is_pressure_plate(state) {
        &crate::redstone::PRESSURE_PLATE_BEHAVIOR
    } else if crate::redstone::is_tripwire(state) {
        &crate::redstone::TRIPWIRE_BEHAVIOR
    } else if crate::leaves::is_leaves(state) {
        &crate::leaves::LEAVES_BEHAVIOR
    } else if crate::crops::is_crop(state) {
        &crate::crops::CROP_BEHAVIOR
    } else if crate::redstone::is_redstone_lamp(state) {
        &crate::redstone::REDSTONE_LAMP_BEHAVIOR
    } else if crate::redstone::is_daylight_detector(state) {
        &crate::redstone::DAYLIGHT_DETECTOR_BEHAVIOR
    } else if crate::redstone::is_powered_openable(state) {
        &crate::redstone::POWERED_OPENABLE_BEHAVIOR
    } else if crate::piston::is_piston(state) {
        &crate::piston::PISTON_BEHAVIOR
    } else if crate::piston::is_piston_head(state) {
        &crate::piston::PISTON_HEAD_BEHAVIOR
    } else if crate::piston::is_moving_piston(state) {
        &crate::piston::MOVING_PISTON_BEHAVIOR
    } else if state.name == "minecraft:fire" {
        &crate::fluid::FIRE
    } else if is_falling_block(state.name.as_str()) {
        &FALLING
    } else if plant_soil(state).is_some() {
        &PLANT
    } else if state.name == "minecraft:grass_block" {
        &GRASS
    } else {
        &NOOP
    }
}

/// Explains why a block's vanilla random tick is not implemented, or `None`
/// when no randomised behaviour is known for it.
pub fn unsupported_random_tick_reason(state: &BlockState) -> Option<&'static str> {
    let name = state.name.as_str();
    if name == "minecraft:lava" {
        Some("lava random ticks spread fire, which needs ignition and flammability metadata")
    } else if tags::block_has("leaves", name) {
        Some("leaf decay needs distance/shape updates during generation and placement")
    } else if tags::block_has("crops", name) {
        Some("crop growth needs age, light, and growth-speed metadata")
    } else if tags::block_has("saplings", name) {
        Some("sapling growth needs tree feature placement")
    } else {
        None
    }
}

/// Explains why a block's vanilla neighbour reaction is not implemented, or
/// `None` when the block has no known dynamic neighbour behaviour.
pub fn unsupported_neighbor_reason(state: &BlockState) -> Option<&'static str> {
    let name = state.name.as_str();
    if tags::block_has("leaves", name) {
        Some("leaf distance updates need P1.5 state/shape metadata")
    } else if name == "minecraft:tripwire_hook" {
        Some("tripwire-hook signal chains and hook shape updates are not ported")
    } else if is_known_unsupported_neighbor(name) {
        Some("doors, rails, and non-redstone torch support are not ported")
    } else {
        None
    }
}

fn is_known_unsupported_neighbor(name: &str) -> bool {
    matches!(
        name,
        "minecraft:dispenser"
            | "minecraft:dropper"
            | "minecraft:hopper"
            | "minecraft:rail"
            | "minecraft:powered_rail"
            | "minecraft:detector_rail"
            | "minecraft:activator_rail"
            | "minecraft:torch"
            | "minecraft:wall_torch"
            | "minecraft:soul_torch"
            | "minecraft:soul_wall_torch"
    )
}

/// Vanilla `FallingBlock.isFree`: air, fire, liquid, or replaceable.
fn is_free(state: &BlockState) -> bool {
    is_air_state(state)
        || tags::block_has("fire", state.name.as_str())
        || matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
        || tags::block_has("replaceable", state.name.as_str())
}

fn is_falling_block(name: &str) -> bool {
    matches!(
        name,
        "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:gravel"
            | "minecraft:suspicious_sand"
            | "minecraft:suspicious_gravel"
            | "minecraft:dragon_egg"
            | "minecraft:anvil"
            | "minecraft:chipped_anvil"
            | "minecraft:damaged_anvil"
    ) || name.ends_with("_concrete_powder")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlantSoil {
    /// `VegetationBlock.mayPlaceOn`: `BlockTags.DIRT` or farmland.
    DirtOrFarmland,
    /// `CropBlock.mayPlaceOn`: farmland only.
    Farmland,
}

fn plant_soil(state: &BlockState) -> Option<PlantSoil> {
    let name = state.name.as_str();
    if tags::block_has("crops", name) {
        Some(PlantSoil::Farmland)
    } else if tags::block_has("small_flowers", name)
        || tags::block_has("saplings", name)
        || matches!(
            name,
            "minecraft:short_grass"
                | "minecraft:fern"
                | "minecraft:bush"
                | "minecraft:sweet_berry_bush"
        )
    {
        Some(PlantSoil::DirtOrFarmland)
    } else if tags::block_has("flowers", name) && has_property(state, "half") {
        // Double plants (sunflower, lilac, rose bush, peony, pitcher plant, …)
        // use `DoublePlantBlock`'s lower/upper rules.
        Some(PlantSoil::DirtOrFarmland)
    } else {
        None
    }
}

fn is_farmland(state: &BlockState) -> bool {
    state.name == "minecraft:farmland"
}

fn is_dirt(state: &BlockState) -> bool {
    tags::block_has("dirt", state.name.as_str())
}

fn has_property(state: &BlockState, key: &str) -> bool {
    state.properties.iter().any(|(name, _)| name == key)
}

fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    }
}

fn dirt() -> BlockState {
    BlockState {
        name: "minecraft:dirt".into(),
        properties: Vec::new(),
    }
}

fn grass_block(snowy: bool) -> BlockState {
    BlockState {
        name: "minecraft:grass_block".into(),
        properties: vec![("snowy".into(), if snowy { "true" } else { "false" }.into())],
    }
}

fn is_snowy_setting(state: &BlockState) -> bool {
    state.name == "minecraft:snow" && property(state, "layers") == Some("1")
}

fn can_be_grass(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    let Some(above) = world.block_state(pos.above())? else {
        return Ok(false);
    };
    if is_snowy_setting(&above) {
        return Ok(true);
    }
    if above.name == "minecraft:water" {
        return Ok(false);
    }
    // Without the P1.5 opacity table, light-blocking is approximated from
    // vanilla tags: air and replaceable plants let light through; any other
    // block is treated as blocking.
    Ok(is_air_state(&above) || tags::block_has("replaceable", above.name.as_str()))
}

fn can_propagate(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    if !can_be_grass(world, pos)? {
        return Ok(false);
    }
    let Some(above) = world.block_state(pos.above())? else {
        return Ok(false);
    };
    Ok(above.name != "minecraft:water")
}

/// Approximate `LevelReader.getMaxLocalRawBrightness` for the supported slice.
///
/// Stored/computed light wins. When a chunk has no light layers (generated
/// chunks are streamed before the first relight), a column is considered fully
/// sky-lit from `surface_y` upwards.
fn light_at(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<u8> {
    let chunk_x = pos.x().div_euclid(16);
    let chunk_z = pos.z().div_euclid(16);
    let local_x = pos.x().rem_euclid(16) as usize;
    let local_z = pos.z().rem_euclid(16) as usize;
    let Some(chunk) = world.storage.load_chunk(chunk_x, chunk_z)? else {
        return Ok(0);
    };
    if chunk.has_light() {
        let sky = chunk.sky_light(local_x, pos.y(), local_z).unwrap_or(0);
        let block = chunk.block_light(local_x, pos.y(), local_z).unwrap_or(0);
        return Ok(sky.max(block));
    }
    let surface = crate::chunk::surface_y(&chunk, local_x, local_z);
    Ok(if pos.y() >= surface { 15 } else { 0 })
}

impl BlockBehavior for FallingBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if old.is_some_and(|old| old.name == state.name) {
            return Ok(());
        }
        // `FallingBlock.onPlace` schedules its own delay (2 ticks).
        world.schedule(pos, state.name.clone(), 2);
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        _direction: Direction,
        _neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        // `FallingBlock.updateShape` schedules on every neighbour update.
        world.schedule(pos, state.name.clone(), 2);
        Ok(None)
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if pos.y() <= crate::chunk_packet::OVERWORLD_MIN_Y {
            return Ok(());
        }
        let Some(below) = world.block_state(pos.below())? else {
            return Ok(());
        };
        if !is_free(&below) {
            return Ok(());
        }
        // This slice moves one block per scheduled tick instead of spawning a
        // `FallingBlockEntity` with gravity. The result is deterministic and
        // lands on the same support, but the timing differs from vanilla's
        // entity fall. See DECISIONS.md (P3.10).
        world.set_block(pos, air())?;
        world.set_block(pos.below(), state.clone())?;
        Ok(())
    }
}

impl BlockBehavior for PlantBehavior {
    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: Direction,
        neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        let Some(soil) = plant_soil(state) else {
            return Ok(None);
        };
        let half = property(state, "half");
        if half == Some("upper") {
            // `DoublePlantBlock.canSurvive`: the upper half only needs the
            // matching lower half below it; horizontal neighbour changes do not
            // remove it.
            let below = world.block_state(pos.below())?;
            let survives = below.is_some_and(|below| {
                below.name == state.name && property(&below, "half") == Some("lower")
            });
            return Ok(if survives { None } else { Some(air()) });
        }

        // Lower half / single plant: support must be soil below.
        let Some(below) = world.block_state(pos.below())? else {
            return Ok(Some(air()));
        };
        let supported = match soil {
            PlantSoil::Farmland => is_farmland(&below),
            PlantSoil::DirtOrFarmland => is_dirt(&below) || is_farmland(&below),
        };
        if !supported {
            return Ok(Some(air()));
        }

        if half == Some("lower") && direction == Direction::Up {
            // `DoublePlantBlock.updateShape`: a lower half whose upper half
            // disappeared is removed.
            let upper_is_ours =
                neighbor.name == state.name && property(neighbor, "half") == Some("upper");
            if !upper_is_ours {
                return Ok(Some(air()));
            }
        }
        Ok(None)
    }
}

impl BlockBehavior for GrassBehavior {
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        state.name == "minecraft:grass_block"
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if state.name != "minecraft:grass_block" {
            return Ok(());
        }
        if !can_be_grass(world, pos)? {
            world.set_block(pos, dirt())?;
            return Ok(());
        }
        // `SpreadingSnowyDirtBlock` requires brightness 9 at the block above.
        if light_at(world, pos.above())? < 9 {
            return Ok(());
        }

        let mut random = world.random(pos);
        for _ in 0..4 {
            let target = pos.offset(
                random.next_int_bound(3) - 1,
                random.next_int_bound(5) - 3,
                random.next_int_bound(3) - 1,
            );
            if target == pos {
                continue;
            }
            let Some(target_state) = world.block_state(target)? else {
                continue;
            };
            if target_state.name != "minecraft:dirt" {
                continue;
            }
            if !can_propagate(world, target)? {
                continue;
            }
            let snowy = world
                .block_state(target.above())?
                .is_some_and(|above| is_snowy_setting(&above));
            world.set_block(target, grass_block(snowy))?;
        }
        Ok(())
    }
}

/// Runs one random tick at an explicit position. Used by [`tick_world`] and by
/// focused tests that need to exercise a behaviour without waiting for the
/// 1-in-4096 random position draw.
pub fn random_tick_at(storage: &mut WorldStorage, tick: u64, pos: BlockPos) -> io::Result<bool> {
    let Some(state) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
        return Ok(false);
    };
    let behavior = behavior_for(&state);
    if !behavior.is_randomly_ticking(&state) {
        if let Some(_reason) = unsupported_random_tick_reason(&state) {
            storage
                .tick_state_mut()
                .unsupported_random_ticks
                .insert(state.name.clone());
        }
        return Ok(false);
    }
    let changed = {
        let mut world = TickWorld::new(storage, tick);
        behavior.random_tick(&mut world, pos, &state)?;
        world.changed
    };
    process_pending_updates(storage)?;
    Ok(changed)
}

/// Applies one queued block change, queues its neighbours, and (optionally)
/// drains the neighbour queue.
pub(crate) fn on_block_change(
    storage: &mut WorldStorage,
    pos: BlockPos,
    old: Option<&BlockState>,
    new_state: &BlockState,
    flags: u32,
    process: bool,
) -> io::Result<()> {
    let tick = storage.tick_state().current_tick;
    if !crate::redstone::is_comparator(new_state) {
        storage.tick_state_mut().comparator_outputs.remove(&pos);
    }
    if !crate::redstone::is_pressure_plate(new_state) && !crate::redstone::is_tripwire(new_state) {
        storage.tick_state_mut().occupancy.remove(&pos);
    }
    if old.is_some_and(|old| old.name != new_state.name) {
        storage.tick_state_mut().containers.remove(pos);
    }
    let behavior = behavior_for(new_state);
    {
        let mut world = TickWorld::new(storage, tick);
        behavior.on_place(&mut world, pos, new_state, old)?;
    }
    if UpdateFlags::has_neighbors(flags) || UpdateFlags::has_shape(flags) {
        storage
            .tick_state_mut()
            .neighbors
            .enqueue_neighbors(pos, flags);
    }
    if process {
        process_pending_updates(storage)?;
    }
    Ok(())
}

/// Drains and applies every pending neighbour update.
///
/// Returns the block changes that were applied. Each change's packet is
/// broadcast by the caller (`perform_block_change` does this for changes the
/// queue itself applies).
pub(crate) fn process_pending_updates(storage: &mut WorldStorage) -> io::Result<Vec<BlockChange>> {
    let mut changes = Vec::new();
    let mut processed = 0usize;
    loop {
        let Some(update) = storage.tick_state_mut().neighbors.pop() else {
            break;
        };
        if processed >= MAX_CHAINED_NEIGHBOR_UPDATES {
            storage.tick_state_mut().overflowed = true;
            eprintln!(
                "[tick] too many chained neighbour updates; skipping the rest at {:?}",
                update.pos
            );
            break;
        }
        processed += 1;
        let tick = storage.tick_state().current_tick;
        storage
            .tick_state_mut()
            .last_neighbor_updates
            .push(ProcessedNeighborUpdate {
                pos: update.pos,
                source: update.source,
                flags: update.flags,
            });

        let Some(state) = storage.block_state_at(update.pos.x(), update.pos.y(), update.pos.z())
        else {
            continue;
        };
        if let Some(_reason) = unsupported_neighbor_reason(&state) {
            storage
                .tick_state_mut()
                .unsupported_neighbor_blocks
                .insert(state.name.clone());
        }
        let behavior = behavior_for(&state);
        let direction = direction_between(update.pos, update.source);

        // `notifyAndUpdatePhysics` calls updateNeighborsAt before
        // updateNeighbourShapes; keep that order for merged queue entries.
        if UpdateFlags::has_neighbors(update.flags) {
            let mut world = TickWorld::new(storage, tick);
            behavior.neighbor_changed(&mut world, update.pos, &state, update.source)?;
        }

        if UpdateFlags::has_shape(update.flags) {
            let neighbor = storage
                .block_state_at(update.source.x(), update.source.y(), update.source.z())
                .unwrap_or_else(air);
            let replacement = {
                let mut world = TickWorld::new(storage, tick);
                behavior.update_shape(&mut world, update.pos, direction, &neighbor, &state)?
            };
            if let Some(replacement) = replacement
                && replacement != state
                && let Some(change) = perform_block_change(
                    storage,
                    update.pos,
                    replacement,
                    UpdateFlags::ALL | UpdateFlags::SHAPE,
                    false,
                    true,
                )?
            {
                changes.push(change);
            }
        }
    }
    Ok(changes)
}

fn direction_between(from: BlockPos, to: BlockPos) -> Direction {
    match (to.x() - from.x(), to.y() - from.y(), to.z() - from.z()) {
        (-1, 0, 0) => Direction::West,
        (1, 0, 0) => Direction::East,
        (0, -1, 0) => Direction::Down,
        (0, 1, 0) => Direction::Up,
        (0, 0, -1) => Direction::North,
        (0, 0, 1) => Direction::South,
        _ => Direction::Up,
    }
}

/// Runs one game tick of world block simulation.
///
/// Ordering follows `ServerLevel.tick`: pending neighbour updates, scheduled
/// block ticks, then random ticks for every cached chunk. Neighbour updates
/// produced by a scheduled or random tick are drained after that callback,
/// before the next callback runs.
pub fn tick_world(storage: &mut WorldStorage, tick: u64) -> io::Result<TickReport> {
    tick_world_with_table(storage, tick, &BehaviorTable::vanilla())
}

/// [`tick_world`] with an explicit behaviour table (the callback interface for
/// future block ports and tests).
pub fn tick_world_with_table(
    storage: &mut WorldStorage,
    tick: u64,
    table: &BehaviorTable,
) -> io::Result<TickReport> {
    storage.tick_state_mut().current_tick = tick;
    storage.tick_state_mut().last_neighbor_updates.clear();
    storage.tick_state_mut().overflowed = false;

    let mut report = TickReport {
        tick,
        ..TickReport::default()
    };

    let pending = process_pending_updates(storage)?;
    report.block_changes += pending.len();

    let due = storage.tick_state_mut().scheduled.tick(tick);
    for scheduled in due {
        let Some(state) =
            storage.block_state_at(scheduled.pos.x(), scheduled.pos.y(), scheduled.pos.z())
        else {
            continue;
        };
        if state.name != scheduled.block {
            continue;
        }
        let behavior = table.get(&state);
        let changed = {
            let mut world = TickWorld::new(storage, tick);
            behavior.scheduled_tick(&mut world, scheduled.pos, &state)?;
            world.changed
        };
        let drained = process_pending_updates(storage)?;
        report.scheduled_ticks += 1;
        report.block_changes += usize::from(changed) + drained.len();
    }

    let queue = storage.tick_state().random.clone();
    let chunks = storage.loaded_chunk_coords();
    for (chunk_x, chunk_z) in chunks {
        let Some(sections) = storage.cached_section_ys(chunk_x, chunk_z) else {
            continue;
        };
        let positions = queue.sample_positions_in_sections(chunk_x, chunk_z, &sections, tick);
        let mut applied = 0usize;
        for pos in positions {
            let Some(current) = storage.block_state_at(pos.x(), pos.y(), pos.z()) else {
                continue;
            };
            let behavior = table.get(&current);
            if !behavior.is_randomly_ticking(&current) {
                if let Some(_reason) = unsupported_random_tick_reason(&current) {
                    storage
                        .tick_state_mut()
                        .unsupported_random_ticks
                        .insert(current.name.clone());
                }
                continue;
            }
            let changed = {
                let mut world = TickWorld::new(storage, tick);
                behavior.random_tick(&mut world, pos, &current)?;
                world.changed
            };
            process_pending_updates(storage)?;
            if changed {
                applied += 1;
            }
        }
        report.random_ticks += applied;
    }

    {
        // Furnaces/spawners tick through their persisted block entities. The
        // catalog is the shipped 1,470-recipe static, so this does not allocate
        // a catalog per tick.
        let mut ticker = crate::block_entity::SimpleBlockEntityTicker::with_recipes(
            crate::loot::RecipeCatalog::global(),
        );
        storage.tick_block_entities(&mut ticker);
    }

    let drained = process_pending_updates(storage)?;
    report.block_changes += drained.len();
    report.neighbor_updates = storage.tick_state().last_neighbor_updates.len();
    report.overflowed = storage.tick_state().overflowed;
    Ok(report)
}

fn sample_seed(seed: i64, x: i32, z: i32, tick: u64) -> i64 {
    let mut value = seed as u64;
    value ^= mix64(x as i64 as u64).rotate_left(17);
    value ^= mix64(z as i64 as u64).rotate_left(31);
    value ^= mix64(tick).rotate_left(47);
    mix64(value) as i64
}

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkSection, PalettedContainer};
    use crate::light::SectionLight;

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    fn chunk_with_grass() -> Chunk {
        let air = state("minecraft:air");
        Chunk {
            x: 0,
            z: 0,
            sections: vec![ChunkSection {
                y: 4,
                block_states: PalettedContainer {
                    palette: vec![air],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
                    indices: vec![0; 64],
                }),
                light: SectionLight::default(),
            }],
        }
    }

    #[test]
    fn flags_match_vanilla_values() {
        assert_eq!(UpdateFlags::NEIGHBORS, 1);
        assert_eq!(UpdateFlags::BLOCK_UPDATE, 2);
        assert_eq!(UpdateFlags::INVISIBLE, 4);
        assert_eq!(UpdateFlags::IMMEDIATE, 8);
        assert_eq!(UpdateFlags::KNOWN_SHAPE, 16);
        assert_eq!(UpdateFlags::SUPPRESS_NEIGHBORS, 16);
        assert_eq!(UpdateFlags::SUPPRESS_DROPS, 32);
        assert_eq!(UpdateFlags::MOVE_BY_PISTON, 64);
        assert_eq!(UpdateFlags::SKIP_SHAPE_UPDATE_ON_WIRE, 128);
        assert_eq!(UpdateFlags::SKIP_BLOCK_ENTITY_SIDE_EFFECTS, 256);
        assert_eq!(UpdateFlags::SKIP_ON_PLACE, 512);
        assert_eq!(UpdateFlags::NONE, 260);
        assert_eq!(UpdateFlags::ALL, 3);
        assert_eq!(UpdateFlags::ALL_IMMEDIATE, 11);
        assert_eq!(UpdateFlags::SKIP_ALL_SIDE_EFFECTS, 816);
        assert!(UpdateFlags::has_neighbors(UpdateFlags::ALL));
        assert!(UpdateFlags::has_shape(UpdateFlags::normalize(
            UpdateFlags::ALL
        )));
        assert!(!UpdateFlags::has_shape(UpdateFlags::normalize(
            UpdateFlags::KNOWN_SHAPE
        )));
    }

    #[test]
    fn neighbor_queue_dedupes_positions_and_merges_flags() {
        let mut queue = NeighborUpdateQueue::new();
        let source = BlockPos::new(1, 2, 3);
        let target = BlockPos::new(2, 2, 3);
        queue.enqueue(source, target, UpdateFlags::NEIGHBORS);
        queue.enqueue(
            source,
            target,
            UpdateFlags::BLOCK_UPDATE | UpdateFlags::SHAPE,
        );
        queue.enqueue(source, target, UpdateFlags::KNOWN_SHAPE);
        assert_eq!(queue.len(), 1);
        let updates = queue.drain();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].pos, target);
        assert_eq!(updates[0].source, source);
        assert!(UpdateFlags::has_neighbors(updates[0].flags));
        assert!(UpdateFlags::has_shape(updates[0].flags));
        assert!(queue.is_empty());
    }

    #[test]
    fn neighbor_queue_groups_by_chunk_and_preserves_insertion_order() {
        let mut queue = NeighborUpdateQueue::new();
        let source = BlockPos::new(5, 20, 5);
        queue.enqueue_neighbors(source, UpdateFlags::ALL);
        let updates = queue.drain();
        let expected = [
            Direction::West,
            Direction::East,
            Direction::Down,
            Direction::Up,
            Direction::North,
            Direction::South,
        ];
        assert_eq!(updates.len(), 6);
        for (update, direction) in updates.iter().zip(expected) {
            assert_eq!(update.pos, source.relative(direction));
            assert!(UpdateFlags::has_neighbors(update.flags));
            assert!(UpdateFlags::has_shape(update.flags));
        }
    }

    #[test]
    fn scheduled_ticks_order_by_priority_then_sub_tick() {
        let mut queue = ScheduledTickQueue::new();
        let pos = BlockPos::new(0, 0, 0);
        assert!(queue.schedule_after(10, pos, "minecraft:sand", 0, TickPriority::Normal));
        assert!(queue.schedule_after(
            10,
            BlockPos::new(1, 0, 0),
            "minecraft:sand",
            0,
            TickPriority::ExtremelyHigh
        ));
        assert!(queue.schedule_after(
            10,
            BlockPos::new(2, 0, 0),
            "minecraft:sand",
            0,
            TickPriority::Normal
        ));
        let due = queue.tick(10);
        assert_eq!(due.len(), 3);
        assert_eq!(due[0].pos, BlockPos::new(1, 0, 0));
        assert_eq!(due[0].priority, TickPriority::ExtremelyHigh);
        assert_eq!(due[1].pos, pos);
        assert_eq!(due[2].pos, BlockPos::new(2, 0, 0));
        assert!(due[1].sub_tick_order < due[2].sub_tick_order);
    }

    #[test]
    fn scheduled_ticks_respect_delay_and_deduplicate() {
        let mut queue = ScheduledTickQueue::new();
        let pos = BlockPos::new(4, 5, 6);
        assert!(queue.schedule_after(3, pos, "minecraft:sand", 2, TickPriority::Normal));
        assert!(!queue.schedule_after(3, pos, "minecraft:sand", 9, TickPriority::Low));
        assert!(queue.has_scheduled_tick(pos, "minecraft:sand"));
        assert!(queue.tick(4).is_empty());
        assert_eq!(queue.tick(5).len(), 1);
        assert!(queue.is_empty());
    }

    #[test]
    fn scheduled_ticks_cancel_before_a_tick() {
        let mut queue = ScheduledTickQueue::new();
        let first = BlockPos::new(0, 0, 0);
        let second = BlockPos::new(1, 0, 0);
        queue.schedule_after(0, first, "minecraft:sand", 0, TickPriority::Normal);
        queue.schedule_after(0, second, "minecraft:sand", 0, TickPriority::Normal);
        assert_eq!(queue.cancel(second), 1);

        let mut visited = Vec::new();
        let ran = queue.tick_with(0, |tick| visited.push(tick.pos));
        assert_eq!(ran, 1);
        assert_eq!(visited, vec![first]);
        assert!(queue.is_empty());

        queue.schedule_after(0, second, "minecraft:sand", 0, TickPriority::Normal);
        assert!(queue.cancel_block(second, "minecraft:sand"));
        assert!(queue.tick(0).is_empty());
    }

    #[test]
    fn random_tick_selection_is_deterministic_and_in_bounds() {
        let chunk = chunk_with_grass();
        let queue = RandomTickQueue::new(12345);
        let first = queue.sample_positions(&chunk, 7);
        let second = queue.sample_positions(&chunk, 7);
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        for pos in &first {
            assert!((64..80).contains(&pos.y()));
            assert!((0..16).contains(&pos.x()));
            assert!((0..16).contains(&pos.z()));
        }
        assert_ne!(first, queue.sample_positions(&chunk, 8));
    }

    #[test]
    fn random_tick_callback_only_receives_matching_states() {
        let mut chunk = chunk_with_grass();
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    chunk.set_block_state(x, 64 + y, z, state("minecraft:stone"));
                }
            }
        }
        let queue = RandomTickQueue::new(1);
        let mut visited = Vec::new();
        let applied = queue
            .tick_chunk(
                &chunk,
                3,
                |state| state.name == "minecraft:stone",
                |pos| {
                    visited.push(pos);
                    Ok(false)
                },
            )
            .unwrap();
        assert_eq!(applied, 0);
        assert_eq!(visited.len(), 3);
    }

    #[test]
    fn behavior_table_classifies_the_supported_slice() {
        assert!(is_falling_block("minecraft:sand"));
        assert!(is_falling_block("minecraft:white_concrete_powder"));
        assert!(plant_soil(&state("minecraft:oak_sapling")).is_some());
        assert!(plant_soil(&state("minecraft:wheat")).is_some());
        assert!(unsupported_random_tick_reason(&state("minecraft:oak_leaves")).is_some());
        assert!(unsupported_random_tick_reason(&state("minecraft:lava")).is_some());
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:redstone_wire")),
            None
        );
        assert_eq!(unsupported_neighbor_reason(&state("minecraft:lever")), None);
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:repeater")),
            None
        );
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:comparator")),
            None
        );
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:observer")),
            None
        );
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:redstone_torch")),
            None
        );
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:stone_pressure_plate")),
            None
        );
        assert_eq!(
            unsupported_neighbor_reason(&state("minecraft:tripwire")),
            None
        );
        assert!(
            unsupported_neighbor_reason(&state("minecraft:tripwire_hook")).is_some(),
            "the hook signal chain is still an explicit gap"
        );
        assert!(unsupported_neighbor_reason(&state("minecraft:oak_leaves")).is_some());
        assert_eq!(
            unsupported_random_tick_reason(&state("minecraft:stone")),
            None
        );
    }

    #[test]
    fn fluid_and_redstone_behaviours_are_registered() {
        use crate::fluid;
        use crate::redstone;

        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:water")),
            &fluid::FLUID as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:redstone_wire")),
            &redstone::WIRE as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:lever")),
            &redstone::LEVER_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:oak_button")),
            &redstone::BUTTON_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:repeater")),
            &redstone::REPEATER_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:comparator")),
            &redstone::COMPARATOR_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:observer")),
            &redstone::OBSERVER_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:redstone_wall_torch")),
            &redstone::REDSTONE_TORCH_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:stone_pressure_plate")),
            &redstone::PRESSURE_PLATE_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:tripwire")),
            &redstone::TRIPWIRE_BEHAVIOR as &dyn BlockBehavior
        ));
        assert!(std::ptr::addr_eq(
            behavior_for(&state("minecraft:fire")),
            &fluid::FIRE as &dyn BlockBehavior
        ));
    }

    struct FixedContainer(i32);
    impl ContainerSignalSource for FixedContainer {
        fn analog_output_signal(&self) -> i32 {
            self.0
        }
    }

    #[test]
    fn container_signals_are_position_keyed_and_clamped() {
        let mut containers = ContainerSignals::default();
        containers.insert(BlockPos::new(1, 2, 3), Box::new(FixedContainer(9)));
        assert_eq!(containers.signal(BlockPos::new(1, 2, 3)), Some(9));
        containers.insert(BlockPos::new(4, 5, 6), Box::new(FixedContainer(99)));
        assert_eq!(containers.signal(BlockPos::new(4, 5, 6)), Some(15));
        assert_eq!(containers.signal(BlockPos::new(7, 8, 9)), None);
        assert_eq!(containers.len(), 2);
        assert!(containers.remove(BlockPos::new(1, 2, 3)));
        assert!(!containers.remove(BlockPos::new(1, 2, 3)));
        assert_eq!(containers.len(), 1);
        containers.clear();
        assert!(containers.is_empty());
    }

    struct MarkerBehavior;
    static MARKER: MarkerBehavior = MarkerBehavior;
    impl BlockBehavior for MarkerBehavior {}

    #[test]
    fn behavior_table_accepts_future_block_overrides() {
        let table = BehaviorTable::vanilla().with("minecraft:stone", &MARKER);
        let stone = state("minecraft:stone");
        assert!(std::ptr::eq(
            table.get(&stone),
            &MARKER as &dyn BlockBehavior
        ));
        assert!(!std::ptr::eq(
            table.get(&state("minecraft:dirt")),
            &MARKER as &dyn BlockBehavior
        ));
    }

    #[test]
    fn tick_priority_values_clamp_like_vanilla() {
        assert_eq!(TickPriority::by_value(-9), TickPriority::ExtremelyHigh);
        assert_eq!(TickPriority::by_value(-1), TickPriority::High);
        assert_eq!(TickPriority::by_value(0), TickPriority::Normal);
        assert_eq!(TickPriority::by_value(9), TickPriority::ExtremelyLow);
        assert!(TickPriority::ExtremelyHigh < TickPriority::Normal);
        assert!(TickPriority::Normal < TickPriority::ExtremelyLow);
    }
}
