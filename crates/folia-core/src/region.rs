//! Per-region world/queue ownership in the spirit of Folia.
//!
//! Folia's [`RegionisedWorld`](https://docs.papermc.io/folia/reference/tick-regions)
//! partitions chunks into regions and ticks each region on its own thread
//! (`TickThread`). A region owns its task queue and its slice of world
//! state; touching another region's state from the wrong tick thread is an
//! error (`ThreadingDetector`), not a silent data race.
//!
//! This module makes that model real for the task scheduler:
//!
//! * [`Region`] owns a task queue, a [`RegionWorldSlice`] (the region's
//!   mutable world-slice token and sparse block cells), and a monotonic
//!   tick counter. Tasks only run when their owning region ticks.
//! * [`RegionOwned<T>`] and [`OwnedWorld`] reject cross-region access with
//!   [`OwnershipError`]. The check is both explicit (caller-supplied
//!   [`RegionKey`]) and runtime (a per-thread current-region map, like
//!   `TickThread`).
//! * [`RegionMap`] maps chunks/sections/blocks to regions with vanilla's
//!   8x8 chunk regions (and 4x4 section regions), using floor division so
//!   negative coordinates map deterministically.
//!
//! The legacy scheduler API ([`RegionScheduler::schedule_at`],
//! [`RegionScheduler::tick`], …) is preserved; the new owned-task API
//! ([`RegionScheduler::schedule_owned_at`],
//! [`RegionScheduler::tick_region_checked`], …) layers ownership on top.
//! [`RegionTickDriver`] wires the model into the live server loop: it groups
//! cached chunks by region and runs each region's tasks plus a region-scoped
//! [`crate::world::WorldStorage::tick_region`] pass. `WorldStorage` itself and
//! entity/player state are still global, so regions are not yet ticked in
//! parallel at the world level (TODO P10.3-P10.7).

use crate::world::WorldStorage;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;

pub type TaskId = u64;

/// Deterministic, bijective pack of a [`RegionKey`]: the high 32 bits are
/// `x` and the low 32 bits are `z`, both reinterpreted unsigned. Equal
/// regions always get equal ids, including for negative coordinates.
pub type RegionId = u64;

/// Vanilla region size in chunks: 8x8 chunks per regionized region.
pub const CHUNK_REGION_SIZE: i32 = 8;
/// Vanilla section region size: 4x4 sections per section region.
pub const SECTION_REGION_SIZE: i32 = 4;
/// Default scheduler grid size, in chunks.
pub const DEFAULT_REGION_GRID_SIZE: u32 = 8;

/// Sentinel key for the global (non-regionized) queue. It can never be a
/// real chunk region because real region keys are bounded by chunk
/// coordinates divided by a grid size of at most 1024.
pub const GLOBAL_REGION_KEY: RegionKey = RegionKey {
    x: i32::MIN,
    z: i32::MIN,
};

fn assert_grid_size(grid_size: u32) {
    assert!(
        (1..=1024).contains(&grid_size),
        "region grid size must be between 1 and 1024"
    );
}

/// Pack a region key into its deterministic [`RegionId`].
pub fn region_id(key: RegionKey) -> RegionId {
    ((key.x as u32 as u64) << 32) | (key.z as u32 as u64)
}

/// Unpack a [`RegionId`] back into its [`RegionKey`].
pub fn region_key_from_id(id: RegionId) -> RegionKey {
    RegionKey {
        x: (id >> 32) as u32 as i32,
        z: id as u32 as i32,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn region_key(self, grid_size: u32) -> RegionKey {
        chunk_region_key(self, grid_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionPos {
    pub x: i32,
    pub z: i32,
}

impl SectionPos {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Section region key using the vanilla 4x4 section region size and
    /// floor division, so negative sections map deterministically.
    pub fn region_key(self) -> RegionKey {
        section_region_key(self.x, self.z)
    }

    pub fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.x, self.z)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, z, y }
    }

    pub fn chunk_pos(self) -> ChunkPos {
        ChunkPos::new(self.x.div_euclid(16), self.z.div_euclid(16))
    }

    pub fn section_pos(self) -> SectionPos {
        SectionPos::new(self.x.div_euclid(16), self.z.div_euclid(16))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionKey {
    pub x: i32,
    pub z: i32,
}

impl RegionKey {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

/// Region key for a chunk at `grid_size` chunks per side, using floor
/// division (`div_euclid`) so negative chunks map deterministically:
/// chunk `-1` is in region `-1`, chunk `-8` is in region `-1`, and
/// chunk `-9` is in region `-2` for grid size 8.
pub fn chunk_region_key(chunk: ChunkPos, grid_size: u32) -> RegionKey {
    assert_grid_size(grid_size);
    let grid = grid_size as i32;
    RegionKey {
        x: chunk.x.div_euclid(grid),
        z: chunk.z.div_euclid(grid),
    }
}

/// Section region key for a section at the vanilla 4x4 section region
/// size, using floor division for the same negative-coordinate rule.
pub fn section_region_key(section_x: i32, section_z: i32) -> RegionKey {
    RegionKey {
        x: section_x.div_euclid(SECTION_REGION_SIZE),
        z: section_z.div_euclid(SECTION_REGION_SIZE),
    }
}

/// Explicit error raised when code touches a region's state from another
/// region (or from a thread that is not ticking the owning region).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipError {
    /// The region that owns the state being touched.
    pub expected: RegionKey,
    /// The region that attempted the access, if any region is active.
    pub actual: Option<RegionKey>,
    /// Human-readable explanation.
    pub message: String,
}

impl OwnershipError {
    pub fn new(expected: RegionKey, actual: Option<RegionKey>) -> Self {
        Self::with_message(
            expected,
            actual,
            format!(
                "region {expected:?} state accessed by {}",
                match actual {
                    Some(actual) => format!("region {actual:?}"),
                    None => "a thread with no active region".to_string(),
                }
            ),
        )
    }

    pub fn with_message(
        expected: RegionKey,
        actual: Option<RegionKey>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            expected,
            actual,
            message: message.into(),
        }
    }

    pub fn cross_region(expected: RegionKey, actual: RegionKey) -> Self {
        Self::with_message(
            expected,
            Some(actual),
            format!("cross-region access: region {actual:?} touched state owned by {expected:?}"),
        )
    }

    pub fn no_active_region(expected: RegionKey) -> Self {
        Self::with_message(
            expected,
            None,
            format!("region {expected:?} state accessed with no active region on this thread"),
        )
    }

    pub fn wrong_thread(expected: RegionKey, actual: Option<RegionKey>) -> Self {
        Self::with_message(
            expected,
            actual,
            format!(
                "region {expected:?} state accessed from the wrong thread (active region: {actual:?})"
            ),
        )
    }
}

impl fmt::Display for OwnershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for OwnershipError {}

// ---------------------------------------------------------------------------
// Per-thread current-region map (the `TickThread` analogue)
// ---------------------------------------------------------------------------

thread_local! {
    static CURRENT_REGION: RefCell<Option<RegionKey>> = const { RefCell::new(None) };
}

static THREAD_REGIONS: OnceLock<Mutex<HashMap<ThreadId, RegionKey>>> = OnceLock::new();

fn thread_regions() -> &'static Mutex<HashMap<ThreadId, RegionKey>> {
    THREAD_REGIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record_thread_region(key: Option<RegionKey>) {
    let map = thread_regions();
    if let Ok(mut guard) = map.lock() {
        match key {
            Some(key) => {
                guard.insert(std::thread::current().id(), key);
            }
            None => {
                guard.remove(&std::thread::current().id());
            }
        }
    }
}

/// The region currently ticking on this thread, if any.
pub fn current_region() -> Option<RegionKey> {
    CURRENT_REGION
        .try_with(|current| *current.borrow())
        .unwrap_or_else(|_| current_region_for_thread(std::thread::current().id()))
}

/// The region recorded for `thread`, if any. This is the per-thread id map
/// exposed for later integration (for example, logging which thread owns
/// a region); the fast path for the current thread is [`current_region`].
pub fn current_region_for_thread(thread: ThreadId) -> Option<RegionKey> {
    thread_regions()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&thread).copied())
}

/// Set the region ticking on this thread, returning the previous value.
/// Prefer [`enter_region`], which restores the previous value on drop.
pub fn set_current_region(key: Option<RegionKey>) -> Option<RegionKey> {
    let previous = CURRENT_REGION
        .try_with(|current| std::mem::replace(&mut *current.borrow_mut(), key))
        .unwrap_or_else(|_| current_region_for_thread(std::thread::current().id()));
    record_thread_region(key);
    previous
}

/// RAII guard that marks `key` as the region ticking on this thread and
/// restores the previous region when dropped (including on panic).
pub struct CurrentRegionGuard {
    previous: Option<RegionKey>,
}

impl CurrentRegionGuard {
    pub fn region(&self) -> Option<RegionKey> {
        self.previous
    }
}

impl Drop for CurrentRegionGuard {
    fn drop(&mut self) {
        set_current_region(self.previous);
    }
}

/// Mark `key` as ticking on this thread until the returned guard drops.
pub fn enter_region(key: RegionKey) -> CurrentRegionGuard {
    CurrentRegionGuard {
        previous: set_current_region(Some(key)),
    }
}

fn check_current_region(expected: RegionKey) -> Result<(), OwnershipError> {
    match current_region() {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(OwnershipError::cross_region(expected, actual)),
        None => Err(OwnershipError::no_active_region(expected)),
    }
}

/// Captures the ticking thread and region, and verifies later that the
/// same thread is still ticking the same region. This mirrors Folia's
/// `ThreadingDetector`: access from a different thread fails even when
/// the region matches, and access from the right thread with the wrong
/// active region fails as a cross-region access.
#[derive(Debug, Clone)]
pub struct ThreadingDetector {
    region: RegionKey,
    thread: ThreadId,
}

impl ThreadingDetector {
    pub fn capture(region: RegionKey) -> Self {
        Self {
            region,
            thread: std::thread::current().id(),
        }
    }

    pub fn region(&self) -> RegionKey {
        self.region
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread
    }

    pub fn verify(&self) -> Result<(), OwnershipError> {
        self.verify_for(std::thread::current().id(), current_region())
    }

    pub fn verify_for(
        &self,
        thread: ThreadId,
        active: Option<RegionKey>,
    ) -> Result<(), OwnershipError> {
        if thread != self.thread {
            return Err(OwnershipError::wrong_thread(self.region, active));
        }
        match active {
            Some(actual) if actual == self.region => Ok(()),
            Some(actual) => Err(OwnershipError::cross_region(self.region, actual)),
            None => Err(OwnershipError::no_active_region(self.region)),
        }
    }
}

// ---------------------------------------------------------------------------
// World slice token, owned world, and region-owned values
// ---------------------------------------------------------------------------

/// Proof that a caller belongs to a region's world slice. A token is
/// minted by the owning [`Region`] (or its [`RegionWorldSlice`]) and can
/// be checked against a caller-supplied [`RegionKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldSliceToken {
    region: RegionKey,
}

impl WorldSliceToken {
    pub fn new(region: RegionKey) -> Self {
        Self { region }
    }

    pub fn region(&self) -> RegionKey {
        self.region
    }

    pub fn matches(&self, caller: RegionKey) -> bool {
        caller == self.region
    }

    pub fn check(&self, caller: RegionKey) -> Result<(), OwnershipError> {
        if caller == self.region {
            Ok(())
        } else {
            Err(OwnershipError::cross_region(self.region, caller))
        }
    }
}

/// A region's owned mutable world slice: a sparse map of block cells
/// guarded by the region's [`WorldSliceToken`]. In later integration this
/// is where per-region chunk/entity/block-entity state lives; for now it
/// is the token + cells the scheduler hands to owned tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionWorldSlice {
    region: RegionKey,
    cells: BTreeMap<BlockPos, u16>,
}

impl RegionWorldSlice {
    pub fn new(region: RegionKey) -> Self {
        Self {
            region,
            cells: BTreeMap::new(),
        }
    }

    pub fn region(&self) -> RegionKey {
        self.region
    }

    pub fn token(&self) -> WorldSliceToken {
        WorldSliceToken::new(self.region)
    }

    pub fn get_block(&self, pos: BlockPos) -> Option<u16> {
        self.cells.get(&pos).copied()
    }

    pub fn set_block(&mut self, pos: BlockPos, state: u16) -> Option<u16> {
        self.cells.insert(pos, state)
    }

    pub fn remove_block(&mut self, pos: BlockPos) -> Option<u16> {
        self.cells.remove(&pos)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn blocks(&self) -> &BTreeMap<BlockPos, u16> {
        &self.cells
    }
}

/// A value owned by exactly one region. Access is checked against the
/// ticking (current) region on this thread; use
/// [`RegionOwned::with_caller`] for explicit, context-free checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionOwned<T> {
    owner: RegionKey,
    value: T,
}

impl<T> RegionOwned<T> {
    pub fn new(owner: RegionKey, value: T) -> Self {
        Self { owner, value }
    }

    pub fn owner(&self) -> RegionKey {
        self.owner
    }

    pub fn check(&self, caller: RegionKey) -> Result<(), OwnershipError> {
        if caller == self.owner {
            Ok(())
        } else {
            Err(OwnershipError::cross_region(self.owner, caller))
        }
    }

    /// Access the value if this thread is ticking the owning region.
    pub fn get(&self) -> Result<&T, OwnershipError> {
        check_current_region(self.owner)?;
        Ok(&self.value)
    }

    /// Mutably access the value if this thread is ticking the owning region.
    pub fn get_mut(&mut self) -> Result<&mut T, OwnershipError> {
        check_current_region(self.owner)?;
        Ok(&mut self.value)
    }

    /// Access the value when `caller` is given explicitly. This is the
    /// context-free form of the check, useful outside a tick (for example
    /// in tests and in handoff code that names the source region).
    pub fn with_caller<R>(
        &self,
        caller: RegionKey,
        f: impl FnOnce(&T) -> R,
    ) -> Result<R, OwnershipError> {
        self.check(caller)?;
        Ok(f(&self.value))
    }

    pub fn with_caller_mut<R>(
        &mut self,
        caller: RegionKey,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, OwnershipError> {
        self.check(caller)?;
        Ok(f(&mut self.value))
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

/// Mutable access to a [`RegionWorldSlice`] that is statically tied to its
/// owning region. It can only be constructed for the slice's own region;
/// wrapping another region's slice fails with [`OwnershipError`].
pub struct OwnedWorld<'a> {
    owner: RegionKey,
    slice: &'a mut RegionWorldSlice,
}

impl<'a> OwnedWorld<'a> {
    pub fn new(owner: RegionKey, slice: &'a mut RegionWorldSlice) -> Result<Self, OwnershipError> {
        if slice.region() != owner {
            return Err(OwnershipError::cross_region(owner, slice.region()));
        }
        Ok(Self { owner, slice })
    }

    pub fn owner(&self) -> RegionKey {
        self.owner
    }

    pub fn token(&self) -> WorldSliceToken {
        WorldSliceToken::new(self.owner)
    }

    pub fn slice(&self) -> &RegionWorldSlice {
        self.slice
    }

    pub fn slice_mut(&mut self) -> &mut RegionWorldSlice {
        self.slice
    }

    pub fn check_caller(&self, caller: RegionKey) -> Result<(), OwnershipError> {
        self.token().check(caller)
    }

    /// Attempt to touch `region`'s state from this world. Succeeds only
    /// when `region` is this world's owner; otherwise this is the explicit
    /// cross-region error Folia raises for out-of-region access.
    pub fn touch(&self, region: RegionKey) -> Result<(), OwnershipError> {
        if region == self.owner {
            Ok(())
        } else {
            Err(OwnershipError::cross_region(self.owner, region))
        }
    }

    /// Alias for [`OwnedWorld::touch`].
    pub fn require_region(&self, region: RegionKey) -> Result<(), OwnershipError> {
        self.touch(region)
    }

    pub fn get_block(&self, pos: BlockPos) -> Option<u16> {
        self.slice.get_block(pos)
    }

    pub fn set_block(&mut self, pos: BlockPos, state: u16) -> Option<u16> {
        self.slice.set_block(pos, state)
    }

    /// Access a value owned by this world's region. A value owned by any
    /// other region is rejected with [`OwnershipError`].
    pub fn attempt_access<'b, T>(
        &self,
        owned: &'b RegionOwned<T>,
    ) -> Result<&'b T, OwnershipError> {
        self.touch(owned.owner())?;
        Ok(&owned.value)
    }

    pub fn attempt_access_mut<'b, T>(
        &mut self,
        owned: &'b mut RegionOwned<T>,
    ) -> Result<&'b mut T, OwnershipError> {
        self.touch(owned.owner())?;
        Ok(&mut owned.value)
    }
}

impl fmt::Debug for OwnedWorld<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedWorld")
            .field("owner", &self.owner)
            .field("slice", self.slice)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// RegionMap: chunks/sections/blocks -> regions
// ---------------------------------------------------------------------------

/// Maps chunks (and sections/blocks) to regions. The default is the
/// vanilla mapping: 8x8 chunk regions and 4x4 section regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionMap {
    grid_size: u32,
}

impl RegionMap {
    pub fn new(grid_size: u32) -> Self {
        assert_grid_size(grid_size);
        Self { grid_size }
    }

    /// The vanilla mapping: 8x8 chunk regions.
    pub fn vanilla() -> Self {
        Self::new(DEFAULT_REGION_GRID_SIZE)
    }

    pub fn grid_size(&self) -> u32 {
        self.grid_size
    }

    pub fn region_for_chunk(&self, chunk: ChunkPos) -> RegionKey {
        chunk_region_key(chunk, self.grid_size)
    }

    pub fn region_id_for_chunk(&self, chunk: ChunkPos) -> RegionId {
        region_id(self.region_for_chunk(chunk))
    }

    pub fn region_for_section(&self, section: SectionPos) -> RegionKey {
        section.region_key()
    }

    /// Section region (4x4) for a chunk position.
    pub fn section_region_for_chunk(&self, chunk: ChunkPos) -> RegionKey {
        section_region_key(chunk.x, chunk.z)
    }

    pub fn region_for_block(&self, block_x: i32, block_z: i32) -> RegionKey {
        self.region_for_chunk(ChunkPos::new(
            block_x.div_euclid(16),
            block_z.div_euclid(16),
        ))
    }

    pub fn region_for_block_pos(&self, pos: BlockPos) -> RegionKey {
        self.region_for_chunk(pos.chunk_pos())
    }

    pub fn region_id(&self, key: RegionKey) -> RegionId {
        region_id(key)
    }

    pub fn region_key_from_id(&self, id: RegionId) -> RegionKey {
        region_key_from_id(id)
    }

    /// Inclusive chunk bounds of a region, or `None` when the bounds
    /// would overflow chunk coordinates.
    pub fn chunk_bounds(&self, key: RegionKey) -> Option<(ChunkPos, ChunkPos)> {
        let grid = i64::from(self.grid_size);
        let min_x = i64::from(key.x) * grid;
        let min_z = i64::from(key.z) * grid;
        let max_x = min_x + grid - 1;
        let max_z = min_z + grid - 1;
        if !(i32::MIN as i64..=i32::MAX as i64).contains(&min_x)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&max_x)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&min_z)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&max_z)
        {
            return None;
        }
        Some((
            ChunkPos::new(min_x as i32, min_z as i32),
            ChunkPos::new(max_x as i32, max_z as i32),
        ))
    }

    /// Inclusive section bounds of a section region (4x4 sections).
    pub fn section_bounds(&self, key: RegionKey) -> Option<(SectionPos, SectionPos)> {
        let size = i64::from(SECTION_REGION_SIZE);
        let min_x = i64::from(key.x) * size;
        let min_z = i64::from(key.z) * size;
        let max_x = min_x + size - 1;
        let max_z = min_z + size - 1;
        if !(i32::MIN as i64..=i32::MAX as i64).contains(&min_x)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&max_x)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&min_z)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&max_z)
        {
            return None;
        }
        Some((
            SectionPos::new(min_x as i32, min_z as i32),
            SectionPos::new(max_x as i32, max_z as i32),
        ))
    }

    pub fn contains_chunk(&self, key: RegionKey, chunk: ChunkPos) -> bool {
        self.region_for_chunk(chunk) == key
    }

    pub fn contains_section(&self, key: RegionKey, section: SectionPos) -> bool {
        self.region_for_section(section) == key
    }

    /// Group chunks by region for migration into per-region ownership.
    /// Keys are sorted (BTreeMap) and chunks within each region are
    /// sorted by `(x, z)`, so the result is deterministic for any input
    /// order.
    pub fn group_chunks(
        &self,
        chunks: impl IntoIterator<Item = ChunkPos>,
    ) -> BTreeMap<RegionKey, Vec<ChunkPos>> {
        let mut grouped: BTreeMap<RegionKey, Vec<ChunkPos>> = BTreeMap::new();
        for chunk in chunks {
            grouped
                .entry(self.region_for_chunk(chunk))
                .or_default()
                .push(chunk);
        }
        for list in grouped.values_mut() {
            list.sort_by_key(|chunk| (chunk.x, chunk.z));
        }
        grouped
    }
}

// ---------------------------------------------------------------------------
// Tasks, Region, and RegionScheduler
// ---------------------------------------------------------------------------

type LegacyTaskCallback = Box<dyn FnMut() + Send>;
type OwnedTaskCallback =
    Box<dyn for<'a> FnMut(&mut OwnedWorld<'a>) -> Result<(), OwnershipError> + Send>;

enum TaskCallback {
    Legacy(LegacyTaskCallback),
    Owned(OwnedTaskCallback),
}

struct Task {
    id: TaskId,
    delay: u32,
    period: u32,
    callback: TaskCallback,
    cancelled: bool,
}

/// Outcome of one region tick: how many tasks executed and, when an owned
/// task raised an [`OwnershipError`], the first such error. On error the
/// failing task is rescheduled/removed exactly as if it had run, the rest
/// of the region's queue is left untouched, and the region's tick counter
/// has still advanced.
#[derive(Debug, Clone, Default)]
pub struct TickOutcome {
    pub executed: usize,
    pub error: Option<OwnershipError>,
}

impl TickOutcome {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// A single regionized unit of world: its own task queue, its own world
/// slice (token + cells), and a monotonic tick counter.
///
/// Ownership bookkeeping lives here: a tick always runs with this region
/// marked as the current region on the ticking thread, so
/// [`RegionOwned`] and [`ThreadingDetector`] checks inside tasks see the
/// owning region.
pub struct Region {
    key: RegionKey,
    tasks: Vec<Task>,
    ticks: u64,
    world: RegionWorldSlice,
    next_id: TaskId,
    last_error: Option<OwnershipError>,
}

impl Region {
    pub fn new(key: RegionKey) -> Self {
        Self {
            key,
            tasks: Vec::new(),
            ticks: 0,
            world: RegionWorldSlice::new(key),
            next_id: 1,
            last_error: None,
        }
    }

    pub fn key(&self) -> RegionKey {
        self.key
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn tick_count(&self) -> u64 {
        self.ticks
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn world(&self) -> &RegionWorldSlice {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut RegionWorldSlice {
        &mut self.world
    }

    pub fn world_token(&self) -> WorldSliceToken {
        self.world.token()
    }

    /// Mutable access to this region's world as an [`OwnedWorld`]. The
    /// world slice is the region's own, so this cannot fail.
    pub fn owned_world(&mut self) -> OwnedWorld<'_> {
        OwnedWorld::new(self.key, &mut self.world)
            .expect("region world slice must be owned by the region")
    }

    pub fn last_error(&self) -> Option<OwnershipError> {
        self.last_error.clone()
    }

    pub fn schedule(
        &mut self,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        self.insert_task(id, delay, period, TaskCallback::Legacy(Box::new(callback)));
        id
    }

    pub fn schedule_owned(
        &mut self,
        delay: u32,
        period: u32,
        callback: impl FnMut(&mut OwnedWorld<'_>) -> Result<(), OwnershipError> + Send + 'static,
    ) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        self.insert_task(id, delay, period, TaskCallback::Owned(Box::new(callback)));
        id
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        let mut found = false;
        for task in &mut self.tasks {
            if task.id == id {
                task.cancelled = true;
                found = true;
            }
        }
        found
    }

    pub fn cancel_all(&mut self) {
        for task in &mut self.tasks {
            task.cancelled = true;
        }
    }

    fn insert_task(&mut self, id: TaskId, delay: u32, period: u32, callback: TaskCallback) {
        self.tasks.push(Task {
            id,
            delay,
            period,
            callback,
            cancelled: false,
        });
    }

    /// Tick this region once. The tick counter advances first (monotonic),
    /// then due tasks run in insertion order with this region marked as
    /// the current region on the ticking thread.
    pub fn tick(&mut self) -> TickOutcome {
        tick_region_inner(self)
    }

    pub fn tick_checked(&mut self) -> Result<usize, OwnershipError> {
        let outcome = self.tick();
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.executed),
        }
    }
}

fn tick_region_inner(region: &mut Region) -> TickOutcome {
    region.ticks = region.ticks.saturating_add(1);
    let key = region.key;
    let _current = enter_region(key);
    let mut executed = 0;
    let mut first_error: Option<OwnershipError> = None;

    let Region { tasks, world, .. } = &mut *region;

    let mut index = 0;
    while index < tasks.len() {
        if tasks[index].cancelled {
            tasks.remove(index);
            continue;
        }
        if tasks[index].delay > 0 {
            tasks[index].delay -= 1;
            index += 1;
            continue;
        }

        let period = tasks[index].period;
        let result: Result<(), OwnershipError> = match &mut tasks[index].callback {
            TaskCallback::Legacy(callback) => {
                callback();
                Ok(())
            }
            TaskCallback::Owned(callback) => match OwnedWorld::new(key, &mut *world) {
                Ok(mut owned) => callback(&mut owned),
                Err(error) => Err(error),
            },
        };

        // The callback borrow has ended; reschedule or drop the task
        // exactly as for a successful run, then stop on the first error.
        if period > 0 {
            tasks[index].delay = period.saturating_sub(1);
            index += 1;
        } else {
            tasks.remove(index);
        }

        match result {
            Ok(()) => {
                executed += 1;
            }
            Err(error) => {
                first_error = Some(error);
                break;
            }
        }
    }

    region.last_error = first_error.clone();
    TickOutcome {
        executed,
        error: first_error,
    }
}

/// A small regionized task scheduler in the spirit of Folia.
///
/// Chunks are partitioned into square regions. Tasks scheduled at a chunk
/// position only run when that region is ticked, which is the core
/// isolation property Folia uses to run regions independently. This
/// implementation is deliberately single-threaded at the scheduler level;
/// the scheduler owns the queues and the host decides how regions are
/// dispatched to worker threads.
pub struct RegionScheduler {
    grid_size: u32,
    next_id: TaskId,
    global: Region,
    regions: BTreeMap<RegionKey, Region>,
    last_error: Option<OwnershipError>,
}

impl RegionScheduler {
    pub fn new(grid_size: u32) -> Self {
        assert_grid_size(grid_size);
        Self {
            grid_size,
            next_id: 1,
            global: Region::new(GLOBAL_REGION_KEY),
            regions: BTreeMap::new(),
            last_error: None,
        }
    }

    /// The vanilla scheduler: 8x8 chunk regions.
    pub fn vanilla() -> Self {
        Self::new(DEFAULT_REGION_GRID_SIZE)
    }

    pub fn from_region_map(map: RegionMap) -> Self {
        Self::new(map.grid_size())
    }

    pub fn grid_size(&self) -> u32 {
        self.grid_size
    }

    pub fn region_map(&self) -> RegionMap {
        RegionMap::new(self.grid_size)
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// Region keys with a queue, in sorted order.
    pub fn region_keys(&self) -> Vec<RegionKey> {
        self.regions.keys().copied().collect()
    }

    /// Creates an empty region queue if `key` does not have one yet. Used by
    /// [`RegionTickDriver`] so every loaded chunk region gets ticked even when
    /// it has no plugin/block tasks.
    pub fn ensure_region(&mut self, key: RegionKey) -> &mut Region {
        self.region_entry(key)
    }

    pub fn task_count(&self) -> usize {
        self.global.task_count() + self.regions.values().map(Region::task_count).sum::<usize>()
    }

    pub fn region_ticks(&self, key: RegionKey) -> Option<u64> {
        self.regions.get(&key).map(Region::ticks)
    }

    pub fn global_ticks(&self) -> u64 {
        self.global.ticks()
    }

    pub fn get_region(&self, key: RegionKey) -> Option<&Region> {
        self.regions.get(&key)
    }

    pub fn get_region_mut(&mut self, key: RegionKey) -> Option<&mut Region> {
        self.regions.get_mut(&key)
    }

    pub fn region_world_mut(&mut self, key: RegionKey) -> Option<&mut RegionWorldSlice> {
        self.regions.get_mut(&key).map(Region::world_mut)
    }

    pub fn global_world_mut(&mut self) -> &mut RegionWorldSlice {
        self.global.world_mut()
    }

    /// The first ownership error from the most recent tick, if any.
    pub fn last_ownership_error(&self) -> Option<OwnershipError> {
        self.last_error.clone()
    }

    fn allocate_id(&mut self) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn region_entry(&mut self, key: RegionKey) -> &mut Region {
        self.regions.entry(key).or_insert_with(|| Region::new(key))
    }

    pub fn schedule_at(
        &mut self,
        chunk: ChunkPos,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        let key = chunk.region_key(self.grid_size);
        let id = self.allocate_id();
        self.region_entry(key).insert_task(
            id,
            delay,
            period,
            TaskCallback::Legacy(Box::new(callback)),
        );
        id
    }

    /// Schedule a task that runs only when `key`'s region ticks.
    pub fn run_for_region(
        &mut self,
        key: RegionKey,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        let id = self.allocate_id();
        self.region_entry(key).insert_task(
            id,
            delay,
            period,
            TaskCallback::Legacy(Box::new(callback)),
        );
        id
    }

    /// Alias for [`RegionScheduler::run_for_region`].
    pub fn schedule_for_region(
        &mut self,
        key: RegionKey,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        self.run_for_region(key, delay, period, callback)
    }

    /// Schedule an owned task at a chunk. The task receives this region's
    /// [`OwnedWorld`] and may fail with [`OwnershipError`] when it touches
    /// another region's state.
    pub fn schedule_owned_at(
        &mut self,
        chunk: ChunkPos,
        delay: u32,
        period: u32,
        callback: impl FnMut(&mut OwnedWorld<'_>) -> Result<(), OwnershipError> + Send + 'static,
    ) -> TaskId {
        let key = chunk.region_key(self.grid_size);
        let id = self.allocate_id();
        self.region_entry(key).insert_task(
            id,
            delay,
            period,
            TaskCallback::Owned(Box::new(callback)),
        );
        id
    }

    /// Schedule an owned task that runs only when `key`'s region ticks.
    pub fn schedule_owned_for_region(
        &mut self,
        key: RegionKey,
        delay: u32,
        period: u32,
        callback: impl FnMut(&mut OwnedWorld<'_>) -> Result<(), OwnershipError> + Send + 'static,
    ) -> TaskId {
        let id = self.allocate_id();
        self.region_entry(key).insert_task(
            id,
            delay,
            period,
            TaskCallback::Owned(Box::new(callback)),
        );
        id
    }

    pub fn schedule_global(
        &mut self,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        let id = self.allocate_id();
        self.global
            .insert_task(id, delay, period, TaskCallback::Legacy(Box::new(callback)));
        id
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        let mut found = self.global.cancel(id);
        for region in self.regions.values_mut() {
            found = region.cancel(id) || found;
        }
        found
    }

    /// Alias for [`RegionScheduler::cancel`].
    pub fn cancel_task(&mut self, id: TaskId) -> bool {
        self.cancel(id)
    }

    pub fn cancel_region(&mut self, key: RegionKey) {
        if let Some(region) = self.regions.get_mut(&key) {
            region.cancel_all();
        }
    }

    /// Runs the global queue and every region once, in sorted region
    /// order (BTreeMap order), so multi-region execution order is
    /// deterministic.
    pub fn tick(&mut self) -> usize {
        self.tick_outcome().executed
    }

    pub fn tick_outcome(&mut self) -> TickOutcome {
        let mut outcome = tick_region_inner(&mut self.global);
        let keys: Vec<RegionKey> = self.regions.keys().copied().collect();
        for key in keys {
            if let Some(region) = self.regions.get_mut(&key) {
                let region_outcome = tick_region_inner(region);
                outcome.executed += region_outcome.executed;
                if outcome.error.is_none() {
                    outcome.error = region_outcome.error;
                }
            }
        }
        self.last_error = outcome.error.clone();
        outcome
    }

    pub fn tick_checked(&mut self) -> Result<usize, OwnershipError> {
        let outcome = self.tick_outcome();
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.executed),
        }
    }

    /// Runs the global queue once, then dispatches regions across worker
    /// threads. Each region has exclusive access to its own task queue,
    /// which is the isolation property Folia relies on to parallelize
    /// regions.
    pub fn tick_parallel(&mut self, workers: usize) -> usize {
        self.tick_parallel_outcome(workers).executed
    }

    pub fn tick_parallel_checked(&mut self, workers: usize) -> Result<usize, OwnershipError> {
        let outcome = self.tick_parallel_outcome(workers);
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.executed),
        }
    }

    fn tick_parallel_outcome(&mut self, workers: usize) -> TickOutcome {
        let workers = workers.max(1);
        if workers == 1 || self.regions.is_empty() {
            return self.tick_outcome();
        }

        let global = tick_region_inner(&mut self.global);
        let mut entries: Vec<(RegionKey, Region)> =
            std::mem::take(&mut self.regions).into_iter().collect();
        let chunk_size = entries.len().div_ceil(workers);
        let executed = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for chunk in entries.chunks_mut(chunk_size) {
                let executed = &executed;
                scope.spawn(move || {
                    let mut local = 0;
                    for (_, region) in chunk.iter_mut() {
                        local += tick_region_inner(region).executed;
                    }
                    executed.fetch_add(local, Ordering::Relaxed);
                });
            }
        });

        // Entries are still in sorted key order, so the first error found
        // is the one from the smallest region key: deterministic even
        // though the regions ticked in parallel.
        let first_error = entries.iter().find_map(|(_, region)| region.last_error());
        self.regions = entries.into_iter().collect();
        let outcome = TickOutcome {
            executed: global.executed + executed.load(Ordering::Relaxed),
            error: global.error.or(first_error),
        };
        self.last_error = outcome.error.clone();
        outcome
    }

    /// Runs only the queue belonging to one region.
    pub fn tick_region(&mut self, key: RegionKey) -> usize {
        self.tick_region_outcome(key).executed
    }

    pub fn tick_region_outcome(&mut self, key: RegionKey) -> TickOutcome {
        let outcome = match self.regions.get_mut(&key) {
            Some(region) => tick_region_inner(region),
            None => TickOutcome::default(),
        };
        self.last_error = outcome.error.clone();
        outcome
    }

    pub fn tick_region_checked(&mut self, key: RegionKey) -> Result<usize, OwnershipError> {
        let outcome = self.tick_region_outcome(key);
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.executed),
        }
    }

    pub fn tick_global(&mut self) -> usize {
        self.tick_global_outcome().executed
    }

    pub fn tick_global_outcome(&mut self) -> TickOutcome {
        let outcome = tick_region_inner(&mut self.global);
        self.last_error = outcome.error.clone();
        outcome
    }
}

// ---------------------------------------------------------------------------
// RegionTickDriver: per-region world + task ticks
// ---------------------------------------------------------------------------

/// Aggregated result of one [`RegionTickDriver`] tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegionTickReport {
    /// Game tick that was simulated.
    pub tick: u64,
    /// Number of region queues ticked (loaded regions plus task-only regions).
    pub regions: usize,
    /// Number of those regions that had loaded chunks and ran a world tick.
    pub world_regions: usize,
    /// Tasks executed across the global and region queues.
    pub tasks: usize,
    /// Due scheduled block ticks that ran.
    pub scheduled_ticks: usize,
    /// Random block ticks that changed a block.
    pub random_ticks: usize,
    /// Neighbour updates processed for the region's own chunks.
    pub neighbor_updates: usize,
    /// Block changes applied by the tick.
    pub block_changes: usize,
    /// True when a region exceeded the chained neighbour-update limit.
    pub overflowed: bool,
}

/// Error raised by a region tick. World I/O and ownership failures are kept
/// distinct so callers can log a bad chunk separately from a cross-region
/// access.
#[derive(Debug)]
pub enum RegionTickError {
    /// A task touched another region's state (or ran without an active region).
    Ownership(OwnershipError),
    /// The world tick failed while reading/writing terrain.
    World(io::Error),
}

impl RegionTickError {
    pub fn ownership(&self) -> Option<&OwnershipError> {
        match self {
            Self::Ownership(error) => Some(error),
            Self::World(_) => None,
        }
    }
}

impl fmt::Display for RegionTickError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ownership(error) => write!(f, "{error}"),
            Self::World(error) => write!(f, "world tick failed: {error}"),
        }
    }
}

impl Error for RegionTickError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ownership(error) => Some(error),
            Self::World(error) => Some(error),
        }
    }
}

impl From<OwnershipError> for RegionTickError {
    fn from(error: OwnershipError) -> Self {
        Self::Ownership(error)
    }
}

impl From<io::Error> for RegionTickError {
    fn from(error: io::Error) -> Self {
        Self::World(error)
    }
}

/// Result of [`RegionTickDriver::tick_outcome`]: the aggregate report plus the
/// first error. Regions keep ticking after a failed task so one bad region
/// cannot silently freeze the others.
#[derive(Debug)]
pub struct RegionTickOutcome {
    pub report: RegionTickReport,
    pub error: Option<RegionTickError>,
}

impl RegionTickOutcome {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Drives the server tick loop through per-region ownership.
///
/// Loaded chunks are grouped with [`RegionMap`]'s vanilla 8x8 chunk regions and
/// ticked in sorted [`RegionKey`] order. For each region the driver:
///
/// 1. runs that region's task queue with the region marked current (so owned
///    tasks and [`RegionOwned`] values are checked), then
/// 2. runs [`WorldStorage::tick_region`] for exactly that region's chunks
///    (scheduled/random ticks, block entities, hoppers, neighbour drains).
///
/// The global queue always runs first, matching [`RegionScheduler::tick`].
/// A task or world error is reported once all regions have ticked, so other
/// regions continue; it is never turned into a panic.
pub struct RegionTickDriver {
    map: RegionMap,
    scheduler: RegionScheduler,
}

impl RegionTickDriver {
    pub fn new(grid_size: u32) -> Self {
        Self {
            map: RegionMap::new(grid_size),
            scheduler: RegionScheduler::new(grid_size),
        }
    }

    /// The vanilla driver: 8x8 chunk regions.
    pub fn vanilla() -> Self {
        Self::new(DEFAULT_REGION_GRID_SIZE)
    }

    pub fn from_region_map(map: RegionMap) -> Self {
        Self {
            map,
            scheduler: RegionScheduler::from_region_map(map),
        }
    }

    pub fn region_map(&self) -> RegionMap {
        self.map
    }

    pub fn scheduler(&self) -> &RegionScheduler {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut RegionScheduler {
        &mut self.scheduler
    }

    pub fn region_count(&self) -> usize {
        self.scheduler.region_count()
    }

    pub fn task_count(&self) -> usize {
        self.scheduler.task_count()
    }

    pub fn region_ticks(&self, key: RegionKey) -> Option<u64> {
        self.scheduler.region_ticks(key)
    }

    pub fn schedule_at(
        &mut self,
        chunk: ChunkPos,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        self.scheduler.schedule_at(chunk, delay, period, callback)
    }

    pub fn schedule_for_region(
        &mut self,
        key: RegionKey,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        self.scheduler
            .schedule_for_region(key, delay, period, callback)
    }

    pub fn schedule_owned_at(
        &mut self,
        chunk: ChunkPos,
        delay: u32,
        period: u32,
        callback: impl FnMut(&mut OwnedWorld<'_>) -> Result<(), OwnershipError> + Send + 'static,
    ) -> TaskId {
        self.scheduler
            .schedule_owned_at(chunk, delay, period, callback)
    }

    pub fn schedule_owned_for_region(
        &mut self,
        key: RegionKey,
        delay: u32,
        period: u32,
        callback: impl FnMut(&mut OwnedWorld<'_>) -> Result<(), OwnershipError> + Send + 'static,
    ) -> TaskId {
        self.scheduler
            .schedule_owned_for_region(key, delay, period, callback)
    }

    pub fn schedule_global(
        &mut self,
        delay: u32,
        period: u32,
        callback: impl FnMut() + Send + 'static,
    ) -> TaskId {
        self.scheduler.schedule_global(delay, period, callback)
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        self.scheduler.cancel(id)
    }

    /// Loaded chunks grouped by region, in sorted region and chunk order.
    pub fn grouped_chunks(&self, storage: &WorldStorage) -> BTreeMap<RegionKey, Vec<ChunkPos>> {
        self.map.group_chunks(
            storage
                .loaded_chunk_coords()
                .into_iter()
                .map(|(x, z)| ChunkPos::new(x, z)),
        )
    }

    /// Ticks the global queue and every loaded/task region in sorted key order.
    /// Returns the first error after every region has had its tick.
    pub fn tick(
        &mut self,
        storage: &mut WorldStorage,
        tick: u64,
    ) -> Result<RegionTickReport, RegionTickError> {
        let outcome = self.tick_outcome(storage, tick);
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.report),
        }
    }

    /// [`tick`](Self::tick) that returns the partial report together with the
    /// first error instead of discarding the report.
    pub fn tick_outcome(&mut self, storage: &mut WorldStorage, tick: u64) -> RegionTickOutcome {
        let grouped = self.grouped_chunks(storage);
        let keys = self.region_keys(&grouped);
        let mut report = RegionTickReport {
            tick,
            regions: keys.len(),
            ..RegionTickReport::default()
        };
        let mut first_error = None;

        let global = self.scheduler.tick_global_outcome();
        report.tasks += global.executed;
        if first_error.is_none() {
            first_error = global.error.map(RegionTickError::Ownership);
        }

        for key in keys {
            let has_world = grouped.contains_key(&key);
            let outcome = self.tick_one(storage, key, tick, has_world);
            merge_report(&mut report, &outcome.report);
            if first_error.is_none() {
                first_error = outcome.error;
            }
        }

        self.scheduler.last_error = first_error
            .as_ref()
            .and_then(RegionTickError::ownership)
            .cloned();
        RegionTickOutcome {
            report,
            error: first_error,
        }
    }

    /// Ticks one region's task queue and (when it has loaded chunks) its world
    /// slice. Global tasks are not run.
    pub fn tick_region(
        &mut self,
        storage: &mut WorldStorage,
        key: RegionKey,
        tick: u64,
    ) -> Result<RegionTickReport, RegionTickError> {
        let outcome = self.tick_region_outcome(storage, key, tick);
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.report),
        }
    }

    pub fn tick_region_outcome(
        &mut self,
        storage: &mut WorldStorage,
        key: RegionKey,
        tick: u64,
    ) -> RegionTickOutcome {
        let has_world = self
            .grouped_chunks(storage)
            .get(&key)
            .is_some_and(|chunks| !chunks.is_empty());
        let outcome = self.tick_one(storage, key, tick, has_world);
        self.scheduler.last_error = outcome
            .error
            .as_ref()
            .and_then(RegionTickError::ownership)
            .cloned();
        outcome
    }

    /// Test-only parallel tick mode.
    ///
    /// Region task queues are dispatched across `workers` scoped threads, so
    /// each region still ticks with its own current-region marker and any
    /// cross-region task yields [`OwnershipError`] instead of a panic. The
    /// world pass remains sequential because [`WorldStorage`] is still one
    /// global store (P10.3); it runs afterwards in sorted key order, so the
    /// observable world order is deterministic. This exists to exercise the
    /// threaded ownership path until per-region world state lands.
    #[doc(hidden)]
    pub fn tick_parallel_for_test(
        &mut self,
        storage: &mut WorldStorage,
        tick: u64,
        workers: usize,
    ) -> Result<RegionTickReport, RegionTickError> {
        let outcome = self.tick_parallel_outcome_for_test(storage, tick, workers);
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.report),
        }
    }

    #[doc(hidden)]
    pub fn tick_parallel_outcome_for_test(
        &mut self,
        storage: &mut WorldStorage,
        tick: u64,
        workers: usize,
    ) -> RegionTickOutcome {
        let grouped = self.grouped_chunks(storage);
        let keys = self.region_keys(&grouped);
        let mut report = RegionTickReport {
            tick,
            regions: keys.len(),
            ..RegionTickReport::default()
        };
        let mut first_error = None;

        let global = self.scheduler.tick_global_outcome();
        report.tasks += global.executed;
        if first_error.is_none() {
            first_error = global.error.map(RegionTickError::Ownership);
        }

        for key in &keys {
            self.scheduler.ensure_region(*key);
        }

        let mut entries: Vec<(RegionKey, Region)> = std::mem::take(&mut self.scheduler.regions)
            .into_iter()
            .collect();
        let workers = workers.max(1);
        let executed = AtomicUsize::new(0);
        if workers == 1 || entries.len() <= 1 {
            let mut local = 0;
            for (_, region) in entries.iter_mut() {
                local += tick_region_inner(region).executed;
            }
            executed.store(local, Ordering::Relaxed);
        } else {
            let chunk_size = entries.len().div_ceil(workers);
            std::thread::scope(|scope| {
                for chunk in entries.chunks_mut(chunk_size) {
                    let executed = &executed;
                    scope.spawn(move || {
                        let mut local = 0;
                        for (_, region) in chunk.iter_mut() {
                            local += tick_region_inner(region).executed;
                        }
                        executed.fetch_add(local, Ordering::Relaxed);
                    });
                }
            });
        }
        report.tasks += executed.load(Ordering::Relaxed);

        // Reports are read back in sorted key order, so the first error is
        // deterministic even though regions ticked in parallel.
        if first_error.is_none() {
            first_error = entries
                .iter()
                .find_map(|(_, region)| region.last_error())
                .map(RegionTickError::Ownership);
        }
        self.scheduler.regions = entries.into_iter().collect();

        for key in keys {
            if !grouped.contains_key(&key) {
                continue;
            }
            match storage.tick_region(&self.map, key, tick) {
                Ok(world) => {
                    report.world_regions += 1;
                    merge_world_report(&mut report, &world);
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(RegionTickError::World(error));
                    }
                }
            }
        }

        self.scheduler.last_error = first_error
            .as_ref()
            .and_then(RegionTickError::ownership)
            .cloned();
        RegionTickOutcome {
            report,
            error: first_error,
        }
    }

    fn region_keys(&mut self, grouped: &BTreeMap<RegionKey, Vec<ChunkPos>>) -> Vec<RegionKey> {
        let mut keys: BTreeSet<RegionKey> = grouped.keys().copied().collect();
        keys.extend(self.scheduler.region_keys());
        keys.into_iter().collect()
    }

    fn tick_one(
        &mut self,
        storage: &mut WorldStorage,
        key: RegionKey,
        tick: u64,
        has_world: bool,
    ) -> RegionTickOutcome {
        self.scheduler.ensure_region(key);
        let task_outcome = self.scheduler.tick_region_outcome(key);
        let mut report = RegionTickReport {
            tick,
            regions: 1,
            tasks: task_outcome.executed,
            ..RegionTickReport::default()
        };
        let mut error = task_outcome.error.map(RegionTickError::Ownership);

        if has_world {
            match storage.tick_region(&self.map, key, tick) {
                Ok(world) => {
                    report.world_regions = 1;
                    merge_world_report(&mut report, &world);
                }
                Err(world_error) => {
                    if error.is_none() {
                        error = Some(RegionTickError::World(world_error));
                    }
                }
            }
        }

        RegionTickOutcome { report, error }
    }
}

fn merge_report(target: &mut RegionTickReport, source: &RegionTickReport) {
    target.tasks += source.tasks;
    target.world_regions += source.world_regions;
    target.scheduled_ticks += source.scheduled_ticks;
    target.random_ticks += source.random_ticks;
    target.neighbor_updates += source.neighbor_updates;
    target.block_changes += source.block_changes;
    target.overflowed |= source.overflowed;
}

fn merge_world_report(target: &mut RegionTickReport, world: &crate::tick::TickReport) {
    target.scheduled_ticks += world.scheduled_ticks;
    target.random_ticks += world.random_ticks;
    target.neighbor_updates += world.neighbor_updates;
    target.block_changes += world.block_changes;
    target.overflowed |= world.overflowed;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn chunk_to_region_mapping_handles_negative_coordinates() {
        let scheduler = RegionScheduler::new(8);
        assert_eq!(
            ChunkPos::new(-1, -1).region_key(scheduler.grid_size()),
            RegionKey { x: -1, z: -1 }
        );
        assert_eq!(
            ChunkPos::new(0, 0).region_key(scheduler.grid_size()),
            RegionKey { x: 0, z: 0 }
        );
        assert_eq!(
            ChunkPos::new(7, 7).region_key(scheduler.grid_size()),
            RegionKey { x: 0, z: 0 }
        );
        assert_eq!(
            ChunkPos::new(8, 8).region_key(scheduler.grid_size()),
            RegionKey { x: 1, z: 1 }
        );
    }

    #[test]
    fn region_id_round_trips_negative_keys() {
        for key in [
            RegionKey::new(0, 0),
            RegionKey::new(-1, -1),
            RegionKey::new(-2, 3),
            RegionKey::new(i32::MIN, i32::MAX),
        ] {
            assert_eq!(region_key_from_id(region_id(key)), key);
        }
        assert_ne!(
            region_id(RegionKey::new(1, 0)),
            region_id(RegionKey::new(0, 1))
        );
    }

    #[test]
    fn region_owned_rejects_access_outside_its_tick() {
        let owned = RegionOwned::new(RegionKey::new(0, 0), 42);
        assert!(owned.get().is_err());
        assert_eq!(owned.with_caller(RegionKey::new(0, 0), |v| *v), Ok(42));
        assert_eq!(
            owned.with_caller(RegionKey::new(1, 0), |v| *v),
            Err(OwnershipError::cross_region(
                RegionKey::new(0, 0),
                RegionKey::new(1, 0)
            ))
        );
    }

    #[test]
    fn owned_world_rejects_another_regions_slice() {
        let mut slice = RegionWorldSlice::new(RegionKey::new(1, 0));
        let error = OwnedWorld::new(RegionKey::new(0, 0), &mut slice).unwrap_err();
        assert_eq!(error.expected, RegionKey::new(0, 0));
        assert_eq!(error.actual, Some(RegionKey::new(1, 0)));
    }

    #[test]
    fn tasks_only_run_when_their_region_ticks() {
        let mut scheduler = RegionScheduler::new(8);
        let left = Arc::new(Mutex::new(0));
        let right = Arc::new(Mutex::new(0));

        {
            let left = Arc::clone(&left);
            scheduler.schedule_at(ChunkPos::new(1, 1), 0, 0, move || {
                *left.lock().unwrap() += 1;
            });
        }
        {
            let right = Arc::clone(&right);
            scheduler.schedule_at(ChunkPos::new(9, 9), 0, 0, move || {
                *right.lock().unwrap() += 1;
            });
        }

        assert_eq!(scheduler.region_count(), 2);
        assert_eq!(scheduler.tick_region(RegionKey { x: 0, z: 0 }), 1);
        assert_eq!(*left.lock().unwrap(), 1);
        assert_eq!(*right.lock().unwrap(), 0);

        assert_eq!(scheduler.tick_region(RegionKey { x: 1, z: 1 }), 1);
        assert_eq!(*left.lock().unwrap(), 1);
        assert_eq!(*right.lock().unwrap(), 1);
    }

    #[test]
    fn delay_and_period_behave_like_bukkit_ticks() {
        let mut scheduler = RegionScheduler::new(4);
        let count = Arc::new(Mutex::new(0));
        {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(0, 0), 1, 2, move || {
                *count.lock().unwrap() += 1;
            });
        }
        let key = RegionKey { x: 0, z: 0 };

        scheduler.tick_region(key); // delay 1 -> 0
        assert_eq!(*count.lock().unwrap(), 0);
        scheduler.tick_region(key); // run, then delay = period - 1 = 1
        assert_eq!(*count.lock().unwrap(), 1);
        scheduler.tick_region(key); // delay 1 -> 0
        assert_eq!(*count.lock().unwrap(), 1);
        scheduler.tick_region(key); // run again, exactly `period` ticks later
        assert_eq!(*count.lock().unwrap(), 2);
    }

    #[test]
    fn cancellation_removes_tasks() {
        let mut scheduler = RegionScheduler::new(8);
        let count = Arc::new(Mutex::new(0));
        let id = {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(0, 0), 0, 1, move || {
                *count.lock().unwrap() += 1;
            })
        };
        assert!(scheduler.cancel(id));
        scheduler.tick();
        assert_eq!(*count.lock().unwrap(), 0);
        assert_eq!(scheduler.task_count(), 0);
    }

    #[test]
    fn global_queue_ticks_with_every_scheduler_tick() {
        let mut scheduler = RegionScheduler::new(8);
        let count = Arc::new(Mutex::new(0));
        {
            let count = Arc::clone(&count);
            scheduler.schedule_global(0, 0, move || {
                *count.lock().unwrap() += 1;
            });
        }
        assert_eq!(scheduler.tick(), 1);
        assert_eq!(*count.lock().unwrap(), 1);
        assert_eq!(scheduler.global_ticks(), 1);
    }

    #[test]
    fn region_tick_counters_are_independent() {
        let mut scheduler = RegionScheduler::new(8);
        scheduler.schedule_at(ChunkPos::new(0, 0), 0, 0, || {});
        scheduler.schedule_at(ChunkPos::new(9, 0), 0, 0, || {});

        scheduler.tick_region(RegionKey { x: 0, z: 0 });
        scheduler.tick_region(RegionKey { x: 0, z: 0 });
        scheduler.tick_region(RegionKey { x: 1, z: 0 });

        assert_eq!(scheduler.region_ticks(RegionKey { x: 0, z: 0 }), Some(2));
        assert_eq!(scheduler.region_ticks(RegionKey { x: 1, z: 0 }), Some(1));
    }

    #[test]
    fn parallel_tick_runs_every_region_exactly_once() {
        let mut scheduler = RegionScheduler::new(4);
        let count = Arc::new(AtomicUsize::new(0));
        for index in 0..16 {
            let count = Arc::clone(&count);
            scheduler.schedule_at(ChunkPos::new(index * 4, 0), 0, 0, move || {
                count.fetch_add(1, Ordering::Relaxed);
            });
        }

        let executed = scheduler.tick_parallel(4);
        assert_eq!(executed, 16);
        assert_eq!(count.load(Ordering::Relaxed), 16);
        assert_eq!(scheduler.region_count(), 16);
        assert_eq!(scheduler.task_count(), 0);
    }
}
