//! Chunk tickets and reference-counted player loaders.
//!
//! This is the authority for *why* a column is kept loaded. Every ticket has
//! an owner (a player session, the spawn area, or an explicit forced load) and
//! a load level. A column is loaded while at least one ticket holds it; when
//! the last ticket leaves, the column enters a deterministic pending-unload
//! queue so a short gap between two players (or a border crossing) does not
//! immediately discard it.
//!
//! The module deliberately has no sockets, generators, or `WorldStorage`
//! dependency. It models the `ChunkMap`/`DistanceManager` ticket bookkeeping so
//! it can be unit-tested in isolation; the join streamer owns the actual chunk
//! packets and asks this manager for the shared view footprints. Keys include
//! the dimension, because two players in different dimensions must never
//! reference-count the same coordinates as the same loaded chunk.

use crate::region::ChunkPos;
use crate::world::Dimension;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// How many ticks a column stays in the pending-unload queue after losing its
/// last ticket. Vanilla's `CHUNK_UNLOAD_DELAY` is 300 ticks (15 seconds at
/// 20 TPS); keeping the same window lets a player walking back re-acquire the
/// chunk before the server discards it.
pub const UNLOAD_DELAY_TICKS: u64 = 300;

/// A ticketed column: dimension plus chunk coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TicketChunk {
    pub dimension: Dimension,
    pub pos: ChunkPos,
}

impl TicketChunk {
    pub fn new(dimension: Dimension, x: i32, z: i32) -> Self {
        Self {
            dimension,
            pos: ChunkPos::new(x, z),
        }
    }

    /// Shortcut for the common overworld case.
    pub fn overworld(x: i32, z: i32) -> Self {
        Self::new(Dimension::Overworld, x, z)
    }
}

/// Load levels ordered from most to least loaded: a lower value means a more
/// loaded chunk.
///
/// Only the shape of vanilla's `ChunkLevel` ladder is modelled here, not its
/// exact ticket-to-level conversion. `ENTITY_TICKING` is the closest level a
/// player holds; `BLOCK_TICKING` is what persistent spawn/forced tickets get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TicketLevel(u32);

impl TicketLevel {
    /// Fully generated and ticking.
    pub const FULL: Self = Self(0);
    /// Entities and blocks tick (player tickets).
    pub const ENTITY_TICKING: Self = Self(32);
    /// Blocks tick, entities do not (persistent tickets).
    pub const BLOCK_TICKING: Self = Self(33);
    /// Not loaded.
    pub const UNLOADED: Self = Self(34);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// True when this level keeps a chunk ticking (blocks or entities).
    pub const fn is_ticking(self) -> bool {
        self.0 <= Self::BLOCK_TICKING.0
    }
}

/// Why a column is held loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TicketKind {
    /// Spawn chunks, added once around the level spawn and never expired.
    Spawn,
    /// An explicit forced load (`/forceload` or a plugin API call).
    Forced,
    /// A player's view footprint, added and removed as the player moves.
    Player,
}

impl TicketKind {
    /// Default level used when a ticket is added without an explicit level.
    pub const fn default_level(self) -> TicketLevel {
        match self {
            Self::Spawn | Self::Forced => TicketLevel::BLOCK_TICKING,
            Self::Player => TicketLevel::ENTITY_TICKING,
        }
    }
}

/// Identity of one ticket holder. Player sessions use a unique `id`; spawn and
/// forced loads use the reserved ids below so they never collide with players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TicketId {
    pub owner: u64,
    pub kind: TicketKind,
}

impl TicketId {
    pub const fn new(owner: u64, kind: TicketKind) -> Self {
        Self { owner, kind }
    }

    pub const fn player(owner: u64) -> Self {
        Self::new(owner, TicketKind::Player)
    }
}

/// Reserved owner id for the persistent spawn footprint.
pub const SPAWN_OWNER: u64 = u64::MAX;
/// Reserved owner id for explicit forced loads.
pub const FORCED_OWNER: u64 = u64::MAX - 1;

static NEXT_PLAYER_OWNER: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh owner id for one player session.
pub fn allocate_player_owner() -> u64 {
    NEXT_PLAYER_OWNER.fetch_add(1, Ordering::Relaxed)
}

/// The sorted, deduplicated square footprint of columns around `center`.
///
/// `radius` is clamped to zero, so a radius of zero is the single center
/// column. The returned set is deterministic and never contains duplicates,
/// which is what lets two overlapping players share a column instead of each
/// tracking a separate copy of it.
pub fn footprint(dimension: Dimension, center: ChunkPos, radius: i32) -> BTreeSet<TicketChunk> {
    let radius = radius.max(0);
    let mut positions = BTreeSet::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            positions.insert(TicketChunk::new(dimension, center.x + dx, center.z + dz));
        }
    }
    positions
}

/// The columns a footprint change affected, in deterministic order.
///
/// `added` lists columns that became *newly loaded* because of this player —
/// columns another player already holds are deliberately excluded, so the
/// streamer can generate each column once and share it. `removed` lists the
/// columns this player stopped holding, which its client must forget even if
/// another player keeps them loaded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FootprintChange {
    pub added: Vec<TicketChunk>,
    pub removed: Vec<TicketChunk>,
}

/// Reference-counted ticket authority for one server process.
///
/// A column appears in `tickets` only while it has at least one ticket, so the
/// map size is the number of loaded columns. `pending_unloads` holds columns
/// whose last ticket was removed; re-adding any ticket cancels the pending
/// unload. `materialized` records columns the server has loaded or generated
/// at least once, so a second player overlapping the first reuses the cached
/// chunk instead of generating a duplicate copy.
#[derive(Debug)]
pub struct ChunkTicketManager {
    tickets: BTreeMap<TicketChunk, BTreeMap<TicketId, TicketLevel>>,
    pending_unloads: BTreeMap<TicketChunk, u64>,
    materialized: BTreeSet<TicketChunk>,
    tick: u64,
}

impl Default for ChunkTicketManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkTicketManager {
    pub fn new() -> Self {
        Self {
            tickets: BTreeMap::new(),
            pending_unloads: BTreeMap::new(),
            materialized: BTreeSet::new(),
            tick: 0,
        }
    }

    /// Add one ticket. Returns true when this call loaded a previously
    /// unticketed column (including cancelling a pending unload).
    pub fn add_ticket(&mut self, chunk: TicketChunk, id: TicketId, level: TicketLevel) -> bool {
        let holders = self.tickets.entry(chunk).or_default();
        let was_empty = holders.is_empty();
        holders.insert(id, level);
        if was_empty {
            self.pending_unloads.remove(&chunk);
        }
        was_empty
    }

    /// Add a ticket using its kind's default level.
    pub fn add_for(&mut self, chunk: TicketChunk, id: TicketId) -> bool {
        let level = id.kind.default_level();
        self.add_ticket(chunk, id, level)
    }

    /// Remove one ticket. Returns true when this call removed the last holder
    /// and queued the column for unload.
    pub fn remove_ticket(&mut self, chunk: TicketChunk, id: TicketId) -> bool {
        let became_empty = match self.tickets.get_mut(&chunk) {
            Some(holders) => {
                holders.remove(&id);
                holders.is_empty()
            }
            None => false,
        };
        if became_empty {
            self.tickets.remove(&chunk);
            self.queue_unload(chunk);
        }
        became_empty
    }

    /// Remove every ticket owned by `owner`. Returns the columns whose last
    /// ticket disappeared (and which are therefore pending unload), sorted.
    pub fn remove_owner(&mut self, owner: u64) -> Vec<TicketChunk> {
        let mut emptied = Vec::new();
        let mut positions: Vec<TicketChunk> = self
            .tickets
            .iter()
            .filter_map(|(chunk, holders)| {
                holders.keys().any(|id| id.owner == owner).then_some(*chunk)
            })
            .collect();
        positions.sort_unstable();
        for chunk in positions {
            if let Some(holders) = self.tickets.get_mut(&chunk) {
                holders.retain(|id, _level| id.owner != owner);
                if holders.is_empty() {
                    self.tickets.remove(&chunk);
                    self.queue_unload(chunk);
                    emptied.push(chunk);
                }
            }
        }
        emptied
    }

    /// Replace one player's view footprint with the square around `center`.
    /// Only the difference is touched, so unchanged columns keep their exact
    /// ticket (and any other player's shared ticket) untouched.
    pub fn set_player_footprint(
        &mut self,
        owner: u64,
        dimension: Dimension,
        center: ChunkPos,
        radius: i32,
    ) -> FootprintChange {
        let id = TicketId::player(owner);
        let desired = footprint(dimension, center, radius);
        let current: BTreeSet<TicketChunk> = self
            .tickets
            .iter()
            .filter_map(|(chunk, holders)| holders.contains_key(&id).then_some(*chunk))
            .collect();

        let newly_held: Vec<TicketChunk> = desired.difference(&current).copied().collect();
        let added: Vec<TicketChunk> = newly_held
            .iter()
            .copied()
            .filter(|chunk| !self.is_loaded(*chunk))
            .collect();
        let removed: Vec<TicketChunk> = current.difference(&desired).copied().collect();
        for chunk in newly_held {
            self.add_for(chunk, id);
        }
        for chunk in &removed {
            self.remove_ticket(*chunk, id);
        }
        FootprintChange { added, removed }
    }

    /// Number of tickets holding a column.
    pub fn refcount(&self, chunk: TicketChunk) -> usize {
        self.tickets.get(&chunk).map_or(0, BTreeMap::len)
    }

    /// Whether the column has at least one ticket.
    pub fn is_loaded(&self, chunk: TicketChunk) -> bool {
        self.tickets.contains_key(&chunk)
    }

    /// The most loaded level among the column's tickets, when loaded.
    pub fn effective_level(&self, chunk: TicketChunk) -> Option<TicketLevel> {
        self.tickets
            .get(&chunk)
            .and_then(|holders| holders.values().copied().min())
    }

    /// Whether any ticket keeps this column ticking.
    pub fn is_ticking(&self, chunk: TicketChunk) -> bool {
        self.effective_level(chunk)
            .is_some_and(TicketLevel::is_ticking)
    }

    /// All loaded columns, sorted.
    pub fn loaded_chunks(&self) -> Vec<TicketChunk> {
        self.tickets.keys().copied().collect()
    }

    /// Columns held by a player with `owner`, sorted.
    pub fn player_chunks(&self, owner: u64) -> Vec<TicketChunk> {
        let id = TicketId::player(owner);
        self.tickets
            .iter()
            .filter_map(|(chunk, holders)| holders.contains_key(&id).then_some(*chunk))
            .collect()
    }

    /// All persistent (spawn or forced) columns, sorted.
    pub fn persistent_chunks(&self) -> Vec<TicketChunk> {
        self.tickets
            .iter()
            .filter_map(|(chunk, holders)| {
                holders
                    .keys()
                    .any(|id| matches!(id.kind, TicketKind::Spawn | TicketKind::Forced))
                    .then_some(*chunk)
            })
            .collect()
    }

    /// Columns currently waiting to be unloaded, sorted.
    pub fn pending_unloads(&self) -> Vec<TicketChunk> {
        self.pending_unloads.keys().copied().collect()
    }

    /// Add or replace the spawn footprint around `center`. Uses the reserved
    /// spawn owner, so it survives every player disconnect.
    pub fn set_spawn_footprint(&mut self, dimension: Dimension, center: ChunkPos, radius: i32) {
        let id = TicketId::new(SPAWN_OWNER, TicketKind::Spawn);
        let desired = footprint(dimension, center, radius);
        let current: BTreeSet<TicketChunk> = self
            .tickets
            .iter()
            .filter_map(|(chunk, holders)| holders.contains_key(&id).then_some(*chunk))
            .collect();
        for chunk in desired.difference(&current) {
            self.add_for(*chunk, id);
        }
        for chunk in current.difference(&desired) {
            self.remove_ticket(*chunk, id);
        }
    }

    /// Force one column to stay loaded until it is explicitly unforced.
    pub fn force_chunk(&mut self, chunk: TicketChunk) {
        self.add_for(chunk, TicketId::new(FORCED_OWNER, TicketKind::Forced));
    }

    /// Remove a forced load. Returns true when the column was newly pending
    /// unload (no other ticket holds it).
    pub fn unforce_chunk(&mut self, chunk: TicketChunk) -> bool {
        self.remove_ticket(chunk, TicketId::new(FORCED_OWNER, TicketKind::Forced))
    }

    /// Record that the server has materialized a column (loaded it from disk
    /// or generated it). Repeated calls are harmless.
    pub fn mark_materialized(&mut self, chunk: TicketChunk) {
        self.materialized.insert(chunk);
    }

    /// Whether the server already has this column cached or stored.
    pub fn is_materialized(&self, chunk: TicketChunk) -> bool {
        self.materialized.contains(&chunk)
    }

    /// All materialized columns, sorted.
    pub fn materialized_chunks(&self) -> Vec<TicketChunk> {
        self.materialized.iter().copied().collect()
    }

    /// Advance the logical unload clock and drain every due column, sorted.
    /// Callers drive this from the tick loop; it is a plain counter here so the
    /// behaviour is deterministic and testable without wall-clock time.
    pub fn tick(&mut self) -> Vec<TicketChunk> {
        self.tick += 1;
        let mut due = Vec::new();
        let mut remaining = BTreeMap::new();
        for (chunk, deadline) in std::mem::take(&mut self.pending_unloads) {
            if deadline <= self.tick {
                due.push(chunk);
            } else {
                remaining.insert(chunk, deadline);
            }
        }
        self.pending_unloads = remaining;
        due
    }

    fn queue_unload(&mut self, chunk: TicketChunk) {
        self.pending_unloads
            .insert(chunk, self.tick + UNLOAD_DELAY_TICKS);
    }
}

static SHARED: OnceLock<Mutex<ChunkTicketManager>> = OnceLock::new();

/// The process-wide ticket manager. Every session shares it, so overlapping
/// players reference-count the same column instead of owning separate copies.
pub fn shared() -> &'static Mutex<ChunkTicketManager> {
    SHARED.get_or_init(|| Mutex::new(ChunkTicketManager::new()))
}

/// Lock the shared manager, recovering from a poisoned mutex.
pub fn lock() -> MutexGuard<'static, ChunkTicketManager> {
    shared()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

static SPAWN_RADIUS: OnceLock<i32> = OnceLock::new();

/// Configure the spawn-chunk radius from `server.properties`. A value <= 0
/// disables persistent spawn chunks, which is the default.
pub fn configure_spawn_chunk_radius(radius: i32) {
    let _ = SPAWN_RADIUS.set(radius.max(0));
}

pub fn spawn_chunk_radius() -> i32 {
    SPAWN_RADIUS.get().copied().unwrap_or(0)
}
