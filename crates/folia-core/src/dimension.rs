//! Dimension storage (Nether/End region files) and portal transfer.
//!
//! Region paths already live on [`crate::world::Dimension`]; this module owns a
//! lazily opened [`WorldStorage`] per dimension, a generation-on-miss helper
//! built on [`crate::worldgen::generator::generate_dimension`], and the
//! coordinate math / frame detection used by portal travel.

use crate::anvil::Compression;
use crate::chunk::Chunk;
use crate::level::{self, LevelData};
use crate::nbt::Tag;
use crate::player::PlayerSession;
use crate::world::{Dimension, WorldStorage};
use crate::worldgen::generator::{self, Dimension as GeneratorDimension};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Vanilla Nether portal coordinate scale.
pub const NETHER_PORTAL_SCALE: f64 = 8.0;

/// End portal arrival point (`ServerLevel.makeObsidianPlatform`).
pub const END_PLATFORM_POSITION: [f64; 3] = [100.5, 50.0, 0.5];

/// Vanilla default Nether portal transition delay
/// (`players_nether_portal_default_delay`, 80 ticks).
pub const NETHER_PORTAL_DELAY_TICKS: u64 = 80;

/// Vanilla dimension-changing delay (`Entity.getDimensionChangingDelay`, 300
/// ticks). Also used as the per-session anti-double-trigger cooldown.
pub const PORTAL_COOLDOWN_TICKS: u64 = 300;

/// One game tick at 20 ticks per second.
pub const TICK_MILLIS: u64 = 50;

/// The dimension of a generator enum value as a storage enum value.
pub fn storage_dimension(dimension: GeneratorDimension) -> Dimension {
    match dimension {
        GeneratorDimension::Overworld => Dimension::Overworld,
        GeneratorDimension::Nether => Dimension::Nether,
        GeneratorDimension::End => Dimension::End,
    }
}

/// The generator dimension for a storage dimension.
pub fn generator_dimension(dimension: Dimension) -> GeneratorDimension {
    match dimension {
        Dimension::Overworld => GeneratorDimension::Overworld,
        Dimension::Nether => GeneratorDimension::Nether,
        Dimension::End => GeneratorDimension::End,
    }
}

/// One cached [`WorldStorage`] per dimension under a shared world root.
pub struct DimensionStorages {
    root: PathBuf,
    storages: HashMap<Dimension, WorldStorage>,
}

impl DimensionStorages {
    pub fn new(world_root: impl Into<PathBuf>) -> Self {
        Self {
            root: world_root.into(),
            storages: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn storage(&mut self, dimension: Dimension) -> &mut WorldStorage {
        let root = self.root.clone();
        self.storages
            .entry(dimension)
            .or_insert_with(|| WorldStorage::for_dimension(root, dimension))
    }

    pub fn load_chunk(
        &mut self,
        dimension: Dimension,
        chunk_x: i32,
        chunk_z: i32,
    ) -> io::Result<Option<Chunk>> {
        self.storage(dimension).load_chunk(chunk_x, chunk_z)
    }

    pub fn save_chunk(
        &mut self,
        dimension: Dimension,
        chunk: &Chunk,
        compression: Compression,
    ) -> io::Result<()> {
        self.storage(dimension).save_chunk(chunk, compression)
    }

    /// Loads a stored chunk or generates and persists it in the target
    /// dimension. This mirrors the join path's overworld generation-on-miss
    /// behavior without wiring portals into sessions yet.
    pub fn ensure_chunk(
        &mut self,
        dimension: Dimension,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
        compression: Compression,
    ) -> io::Result<Chunk> {
        if let Some(chunk) = self.load_chunk(dimension, chunk_x, chunk_z)? {
            return Ok(chunk);
        }
        let chunk =
            generator::generate_dimension(generator_dimension(dimension), chunk_x, chunk_z, seed)?;
        self.save_chunk(dimension, &chunk, compression)?;
        Ok(chunk)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        for storage in self.storages.values_mut() {
            storage.flush()?;
        }
        Ok(())
    }
}

/// Reads block names for portal frame detection without requiring a live
/// server. `WorldStorage` already provides a blanket implementation below.
pub trait PortalBlockView {
    fn block_name(&self, x: i32, y: i32, z: i32) -> Option<String>;
}

impl PortalBlockView for WorldStorage {
    fn block_name(&self, x: i32, y: i32, z: i32) -> Option<String> {
        self.block_state_at(x, y, z).map(|state| state.name)
    }
}

/// Applies the vanilla Nether coordinate scale. Only Overworld <-> Nether
/// changes coordinates; the End keeps the incoming position.
pub fn scaled_coordinates(from: Dimension, to: Dimension, x: f64, z: f64) -> (f64, f64) {
    match (from, to) {
        (Dimension::Overworld, Dimension::Nether) => (
            (x / NETHER_PORTAL_SCALE).floor(),
            (z / NETHER_PORTAL_SCALE).floor(),
        ),
        (Dimension::Nether, Dimension::Overworld) => {
            (x * NETHER_PORTAL_SCALE, z * NETHER_PORTAL_SCALE)
        }
        _ => (x, z),
    }
}

/// Destination position for a dimension transition. The End always lands on
/// the obsidian platform; the Nether pair scales X/Z and keeps Y inside the
/// target build range.
pub fn portal_destination(from: Dimension, to: Dimension, x: f64, y: f64, z: f64) -> [f64; 3] {
    match to {
        Dimension::End => END_PLATFORM_POSITION,
        _ => {
            let (x, z) = scaled_coordinates(from, to, x, z);
            let generator = generator_dimension(to);
            let min = f64::from(generator.min_y() + 1);
            let max = f64::from(generator.min_y() + generator.height() - 2);
            [x, y.clamp(min, max), z]
        }
    }
}

/// The kind of block that initiated a portal transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalKind {
    Nether,
    End,
}

impl PortalKind {
    /// Returns the block name that identifies this portal kind.
    pub const fn block_name(self) -> &'static str {
        match self {
            PortalKind::Nether => "minecraft:nether_portal",
            PortalKind::End => "minecraft:end_portal",
        }
    }

    /// Target dimension. Nether portals always go to the Nether unless already
    /// there; End portals go to the End unless already there, in which case
    /// they use the overworld respawn point.
    pub const fn destination(self, from: Dimension) -> Dimension {
        match (self, from) {
            (PortalKind::Nether, Dimension::Nether) => Dimension::Overworld,
            (PortalKind::Nether, _) => Dimension::Nether,
            (PortalKind::End, Dimension::End) => Dimension::Overworld,
            (PortalKind::End, _) => Dimension::End,
        }
    }

    /// Vanilla transition delay in ticks. The End portal transfers on the next
    /// tick (0 delay); the Nether portal uses the 80-tick player default.
    pub const fn transition_ticks(self) -> u64 {
        match self {
            PortalKind::Nether => NETHER_PORTAL_DELAY_TICKS,
            PortalKind::End => 0,
        }
    }
}

/// Recognizes the two portal block names the live server transfers through.
pub fn portal_kind_for_block(name: &str) -> Option<PortalKind> {
    match name {
        "minecraft:nether_portal" => Some(PortalKind::Nether),
        "minecraft:end_portal" => Some(PortalKind::End),
        _ => None,
    }
}

/// A planned dimension transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortalTrigger {
    pub kind: PortalKind,
    pub from: Dimension,
    pub to: Dimension,
    /// Source position (the player's feet) used for the coordinate math.
    pub source: [f64; 3],
    /// Destination position for the target dimension.
    pub destination: [f64; 3],
}

/// Computes a trigger for a portal use without any session state.
pub fn portal_trigger(
    kind: PortalKind,
    from: Dimension,
    x: f64,
    y: f64,
    z: f64,
) -> PortalTrigger {
    let to = kind.destination(from);
    PortalTrigger {
        kind,
        from,
        to,
        source: [x, y, z],
        destination: portal_destination(from, to, x, y, z),
    }
}

/// Why a portal request was accepted or rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalRequest {
    /// The transition was scheduled.
    Started,
    /// A transition is already waiting for its delay to elapse.
    AlreadyPending,
    /// The session is still inside the post-transfer cooldown window.
    CoolingDown,
}

/// A scheduled portal transition and the instant it becomes ready.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingPortal {
    pub trigger: PortalTrigger,
    pub ready_at: Instant,
}

/// Per-session dimension state: current dimension, a pending portal delay, and
/// the post-transfer cooldown that prevents a portal from immediately firing
/// again (the "double-trigger loop").
#[derive(Debug, Clone)]
pub struct SessionDimensionState {
    current: Dimension,
    pending: Option<PendingPortal>,
    cooldown_until: Option<Instant>,
}

fn ticks_duration(ticks: u64) -> Duration {
    Duration::from_millis(ticks.saturating_mul(TICK_MILLIS))
}

impl SessionDimensionState {
    pub fn new(current: Dimension) -> Self {
        Self {
            current,
            pending: None,
            cooldown_until: None,
        }
    }

    pub fn current(&self) -> Dimension {
        self.current
    }

    pub fn pending(&self) -> Option<PendingPortal> {
        self.pending
    }

    pub fn is_cooling_down(&self, now: Instant) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }

    pub fn cooldown_remaining(&self, now: Instant) -> Option<Duration> {
        self.cooldown_until
            .filter(|until| now < *until)
            .map(|until| until - now)
    }

    /// Schedules a portal transition using the portal kind's vanilla delay.
    pub fn request(
        &mut self,
        kind: PortalKind,
        source: [f64; 3],
        now: Instant,
    ) -> PortalRequest {
        self.request_with_delay(kind, source, now, ticks_duration(kind.transition_ticks()))
    }

    /// Schedules a portal transition with an explicit delay. A second request
    /// while one is pending is rejected instead of resetting the delay.
    pub fn request_with_delay(
        &mut self,
        kind: PortalKind,
        source: [f64; 3],
        now: Instant,
        delay: Duration,
    ) -> PortalRequest {
        if self.pending.is_some() {
            return PortalRequest::AlreadyPending;
        }
        if self.is_cooling_down(now) {
            return PortalRequest::CoolingDown;
        }
        let trigger = portal_trigger(kind, self.current, source[0], source[1], source[2]);
        self.pending = Some(PendingPortal {
            trigger,
            ready_at: now + delay,
        });
        PortalRequest::Started
    }

    /// Returns the pending trigger once its delay has elapsed.
    pub fn ready(&self, now: Instant) -> Option<PortalTrigger> {
        self.pending
            .filter(|pending| now >= pending.ready_at)
            .map(|pending| pending.trigger)
    }

    /// Commits the pending transition, updates the current dimension, and
    /// starts the anti-double-trigger cooldown.
    pub fn complete(&mut self, now: Instant) -> Option<PortalTrigger> {
        let pending = self.pending.take()?;
        self.current = pending.trigger.to;
        self.cooldown_until = Some(now + ticks_duration(PORTAL_COOLDOWN_TICKS));
        Some(pending.trigger)
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// Directly relocates the session (used by death respawn and by callers
    /// that need to override the portal destination) and starts the cooldown.
    pub fn relocate(&mut self, dimension: Dimension, now: Instant) {
        self.current = dimension;
        self.pending = None;
        self.cooldown_until = Some(now + ticks_duration(PORTAL_COOLDOWN_TICKS));
    }

    /// Overrides the pending destination, e.g. an End portal in the End that
    /// must use the overworld respawn point instead of the coordinate math.
    pub fn override_pending_destination(&mut self, destination: [f64; 3]) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        pending.trigger.destination = destination;
        true
    }
}

/// Reads the player's persisted dimension. Missing or unreadable playerdata
/// defaults to the overworld.
pub fn playerdata_dimension(directory: impl AsRef<Path>, uuid: &[u8; 16]) -> Dimension {
    let path = PlayerSession::path(directory.as_ref(), uuid);
    let Ok(bytes) = std::fs::read(path) else {
        return Dimension::Overworld;
    };
    let Ok(root) = crate::nbt::read_gzip_root(&bytes) else {
        return Dimension::Overworld;
    };
    match root.get("Dimension").and_then(Tag::as_str) {
        Some(name) => Dimension::from_resource_name(name),
        None => Dimension::Overworld,
    }
}

/// Saves playerdata and rewrites its `Dimension` field atomically.
pub fn save_playerdata_with_dimension(
    session: &PlayerSession,
    directory: impl AsRef<Path>,
    dimension: Dimension,
) -> io::Result<PathBuf> {
    let directory = directory.as_ref();
    let path = session.save(directory)?;
    let bytes = std::fs::read(&path)?;
    let mut root = crate::nbt::read_gzip_root(&bytes)?;
    let Tag::Compound(entries) = &mut root else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "playerdata root is not a compound",
        ));
    };
    let value = Tag::String(dimension.resource_name().to_owned());
    match entries.iter_mut().find(|(key, _)| key == "Dimension") {
        Some((_, slot)) => *slot = value,
        None => entries.push(("Dimension".into(), value)),
    }
    let bytes = crate::nbt::write_gzip_root(&root)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, &path)?;
    Ok(path)
}

/// Saves playerdata while keeping the dimension already stored on disk. Used
/// by every ordinary save path so a nether/end session never gets rewritten as
/// overworld by an autosave.
pub fn save_playerdata_preserving_dimension(
    session: &PlayerSession,
    directory: impl AsRef<Path>,
) -> io::Result<PathBuf> {
    let directory = directory.as_ref();
    let dimension = playerdata_dimension(directory, &session.uuid);
    save_playerdata_with_dimension(session, directory, dimension)
}

/// Game time and weather are overworld-level state shared by every dimension.
/// Secondary dimensions never get their own `level.dat`.
pub fn shared_level_data(world_root: impl AsRef<Path>) -> io::Result<LevelData> {
    level::read(world_root.as_ref().join("level.dat"))
}

/// Records elapsed ticks in the shared overworld `level.dat`. Refuses secondary
/// dimensions so a switch can never create or overwrite a per-dimension copy.
pub fn record_shared_session(
    world_root: impl AsRef<Path>,
    dimension: Dimension,
    elapsed_ticks: u64,
) -> io::Result<LevelData> {
    if dimension != Dimension::Overworld {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secondary dimensions share the overworld level.dat",
        ));
    }
    level::record_session(world_root.as_ref().join("level.dat"), elapsed_ticks, None)
}

fn is_name(view: &dyn PortalBlockView, x: i32, y: i32, z: i32, expected: &str) -> bool {
    view.block_name(x, y, z)
        .is_some_and(|name| name == expected)
}

fn frame_at(view: &dyn PortalBlockView, x: i32, y: i32, z: i32, axis_x: bool) -> bool {
    // Interior is 2 wide, 3 tall. The frame is one block outside it.
    for level in 0..3 {
        for side in 0..2 {
            let (ix, iz) = if axis_x { (x + side, z) } else { (x, z + side) };
            if !is_name(view, ix, y + level, iz, "minecraft:air")
                && !is_name(view, ix, y + level, iz, "minecraft:nether_portal")
            {
                return false;
            }
        }
    }
    for level in -1..=3 {
        for side in -1..=2 {
            let (fx, fz) = if axis_x { (x + side, z) } else { (x, z + side) };
            if (level == -1 || level == 3 || side == -1 || side == 2)
                && !is_name(view, fx, y + level, fz, "minecraft:obsidian")
            {
                return false;
            }
        }
    }
    true
}

/// Detects a vanilla 2x3 Nether portal interior at `(x, y, z)` along either
/// horizontal axis.
pub fn is_nether_portal(view: &dyn PortalBlockView, x: i32, y: i32, z: i32) -> bool {
    frame_at(view, x, y, z, true) || frame_at(view, x, y, z, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_scale_rounds_down_toward_negative_infinity() {
        assert_eq!(
            scaled_coordinates(Dimension::Overworld, Dimension::Nether, 80.0, -9.0),
            (10.0, -2.0)
        );
        assert_eq!(
            scaled_coordinates(Dimension::Nether, Dimension::Overworld, 10.0, -2.0),
            (80.0, -16.0)
        );
    }

    #[test]
    fn end_transfer_uses_the_platform() {
        assert_eq!(
            portal_destination(Dimension::Overworld, Dimension::End, 1000.0, 70.0, -1000.0),
            END_PLATFORM_POSITION
        );
    }
}
