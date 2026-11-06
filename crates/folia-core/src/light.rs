//! Light storage and the sky/block light engines.
//!
//! Vanilla stores one 2048-byte nibble array per 16x16x16 section for each
//! light layer (`DataLayer`). This module ports that storage format, the
//! per-column sky-light source scan from `ChunkSkyLightSources`, the
//! breadth-first increase propagation from `SkyLightEngine`/`BlockLightEngine`,
//! and the decrease queues that run when a source is removed or an opaque block
//! is placed.
//!
//! [`LightScene`] is the incremental engine. It owns the edited chunk plus any
//! loaded neighbours, so light can cross chunk borders in both directions.
//! Each edit runs vanilla's two-phase schedule: every decrease first (zeroing
//! the region that depended on the old source) and then every increase (pulling
//! light back in from the unaffected border). Only section coordinates whose
//! nibbles actually changed end up in [`DirtySections`], which callers use to
//! send section-scoped `ClientboundLightUpdate` payloads.
//!
//! Opacity and emission come from the generated per-state
//! [`crate::block_behaviour`] table. Unknown states resolve the same way
//! [`crate::block_behaviour::light_block`] does: fully opaque and non-emitting,
//! which is conservative because it can never fabricate light. Block shapes
//! (and therefore Java's face-occlusion checks) are still full cubes; the
//! engine uses the per-state light-block value, exactly like the whole-chunk
//! path it replaces.

use crate::chunk::{BlockState, Chunk};
use crate::chunk_packet::{LIGHT_SECTION_COUNT, OVERWORLD_MIN_SECTION};
use crate::protocol::PacketWriter;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::io;

/// Side length of a light section.
pub const SECTION_SIZE: usize = 16;
/// Number of cells in one light section.
pub const SECTION_CELLS: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;
/// Vanilla's packed nibble-array size for one section.
pub const DATA_LAYER_BYTES: usize = SECTION_CELLS / 2;
/// Highest representable light level.
pub const MAX_LIGHT: u8 = 15;

const SECTION_BLOCKS: i32 = SECTION_SIZE as i32;

/// Java `Direction.values()` order: DOWN, UP, NORTH, SOUTH, WEST, EAST.
const PROPAGATION_DIRECTIONS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

const DIRECTION_ALL: u8 = 0b11_1111;
/// Bit for UP, the direction sky source entries skip.
const DIRECTION_UP: u8 = 1 << 1;

/// A 4-bit-per-cell light array, laid out exactly like vanilla `DataLayer`:
/// `index = y << 8 | z << 4 | x`, low nibble for even indices, high for odd.
#[derive(Clone, PartialEq, Eq)]
pub struct DataLayer {
    data: Box<[u8; DATA_LAYER_BYTES]>,
}

impl DataLayer {
    pub fn new() -> Self {
        Self::filled(0)
    }

    pub fn filled(level: u8) -> Self {
        let level = level & MAX_LIGHT;
        let packed = level | (level << 4);
        Self {
            data: Box::new([packed; DATA_LAYER_BYTES]),
        }
    }

    /// Validate and take ownership of a vanilla `SkyLight`/`BlockLight`
    /// byte array. Anything other than 2048 bytes is corrupt chunk data.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != DATA_LAYER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "light layer should be {DATA_LAYER_BYTES} bytes, found {}",
                    bytes.len()
                ),
            ));
        }
        let mut data = Box::new([0u8; DATA_LAYER_BYTES]);
        data.copy_from_slice(bytes);
        Ok(Self { data })
    }

    pub fn as_bytes(&self) -> &[u8; DATA_LAYER_BYTES] {
        &self.data
    }

    pub fn into_bytes(self) -> [u8; DATA_LAYER_BYTES] {
        *self.data
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        let index = layer_index(x, y, z);
        let byte = self.data[index >> 1];
        if index & 1 == 0 {
            byte & 0x0F
        } else {
            byte >> 4
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, level: u8) {
        let index = layer_index(x, y, z);
        let level = level & MAX_LIGHT;
        let shift = (index & 1) * 4;
        let mask = 0x0F << shift;
        let byte = &mut self.data[index >> 1];
        *byte = (*byte & !mask) | (level << shift);
    }

    pub fn fill(&mut self, level: u8) {
        let level = level & MAX_LIGHT;
        self.data.fill(level | (level << 4));
    }

    pub fn is_empty(&self) -> bool {
        self.data.iter().all(|&byte| byte == 0)
    }

    pub fn is_filled_with(&self, level: u8) -> bool {
        let level = level & MAX_LIGHT;
        self.data.iter().all(|&byte| byte == level | (level << 4))
    }

    /// Stable content hash used by the packet fallback to decide whether a
    /// stored layer is the same one vanilla's height approximation produces.
    pub fn content_eq(&self, bytes: &[u8; DATA_LAYER_BYTES]) -> bool {
        self.data.as_ref() == bytes
    }
}

impl Default for DataLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DataLayer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DataLayer")
            .field("empty", &self.is_empty())
            .finish()
    }
}

/// The two optional light layers stored in one chunk section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionLight {
    pub sky: Option<DataLayer>,
    pub block: Option<DataLayer>,
}

impl SectionLight {
    pub fn is_empty(&self) -> bool {
        self.sky.is_none() && self.block.is_none()
    }

    pub fn sky_get(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        self.sky.as_ref().map(|layer| layer.get(x, y, z))
    }

    pub fn block_get(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        self.block.as_ref().map(|layer| layer.get(x, y, z))
    }
}

/// Light arrays keyed by section Y, the shape the whole-chunk engine computes
/// before it is applied back into a chunk's sections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LightStorage {
    sections: BTreeMap<i8, SectionLight>,
}

impl LightStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, section_y: i8) -> Option<&SectionLight> {
        self.sections.get(&section_y)
    }

    pub fn insert(&mut self, section_y: i8, light: SectionLight) {
        self.sections.insert(section_y, light);
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (i8, &SectionLight)> {
        self.sections.iter().map(|(&y, light)| (y, light))
    }

    /// Capture the layers currently materialized in `chunk`. Sections with no
    /// layers are omitted; [`Self::apply_to`] restores them as default.
    pub fn from_chunk(chunk: &Chunk) -> Self {
        let mut sections = BTreeMap::new();
        for section in &chunk.sections {
            if !section.light.is_empty() {
                sections.insert(section.y, section.light.clone());
            }
        }
        Self { sections }
    }

    /// Apply this storage back into a chunk. Sections without an entry are
    /// reset to the default (no layers).
    pub fn apply_to(&self, chunk: &mut Chunk) {
        for section in &mut chunk.sections {
            section.light = self.sections.get(&section.y).cloned().unwrap_or_default();
        }
    }
}

fn layer_index(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
    (y << 8) | (z << 4) | x
}

/// The coordinates of one 16x16x16 light section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LightSectionPos {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub section_y: i8,
}

impl LightSectionPos {
    pub fn new(chunk_x: i32, chunk_z: i32, section_y: i8) -> Self {
        Self {
            chunk_x,
            chunk_z,
            section_y,
        }
    }

    /// Section containing an absolute block position.
    pub fn from_block(x: i32, y: i32, z: i32) -> Self {
        Self {
            chunk_x: x.div_euclid(SECTION_SIZE as i32),
            chunk_z: z.div_euclid(SECTION_SIZE as i32),
            section_y: y.div_euclid(SECTION_BLOCKS) as i8,
        }
    }
}

/// Deterministically ordered set of light sections whose nibbles changed.
///
/// Iteration is sorted by `(chunk_x, chunk_z, section_y)`, so repeated edits
/// produce the same packet order regardless of hash-map state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtySections {
    sections: BTreeSet<LightSectionPos>,
}

impl DirtySections {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, pos: LightSectionPos) -> bool {
        self.sections.insert(pos)
    }

    pub fn remove(&mut self, pos: &LightSectionPos) -> bool {
        self.sections.remove(pos)
    }

    pub fn contains(&self, pos: &LightSectionPos) -> bool {
        self.sections.contains(pos)
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &LightSectionPos> {
        self.sections.iter()
    }

    pub fn for_chunk(&self, chunk_x: i32, chunk_z: i32) -> impl Iterator<Item = &LightSectionPos> {
        self.sections
            .iter()
            .filter(move |pos| pos.chunk_x == chunk_x && pos.chunk_z == chunk_z)
    }

    pub fn merge(&mut self, other: &DirtySections) {
        self.sections.extend(other.sections.iter().copied());
    }

    pub fn clear(&mut self) {
        self.sections.clear();
    }
}

/// Recompute both light layers for one chunk without loading neighbours.
///
/// Chunk-border light is handled as well as a single chunk allows: the engine
/// seeds the one-block border from the chunk itself and propagates inward, but
/// cannot import light from a neighbour that is not supplied. Use
/// [`recompute_chunk_light_with_neighbours`] when the loaded neighbours are
/// available, or [`LightScene`] for incremental updates.
pub fn recompute_chunk_light(chunk: &mut Chunk) {
    recompute_chunk_light_with_neighbours(chunk, &[]);
}

/// Recompute both layers, importing border light from any supplied chunks.
///
/// Neighbours are matched by their `x`/`z` fields, so callers can pass any
/// subset of the eight surrounding columns. Missing neighbours are treated as
/// air for propagation but are not seeded with sky sources; that avoids
/// inventing a bright border on an unloaded edge.
pub fn recompute_chunk_light_with_neighbours(chunk: &mut Chunk, neighbours: &[Chunk]) {
    if chunk.sections.is_empty() {
        return;
    }
    let storage = compute_chunk_light(chunk, neighbours);
    storage.apply_to(chunk);
}

/// Compute light for `chunk` and return the section-keyed result without
/// mutating the chunk. Exposed so callers can compare or persist light before
/// applying it.
pub fn compute_chunk_light(chunk: &Chunk, neighbours: &[Chunk]) -> LightStorage {
    let refs: Vec<&Chunk> = neighbours.iter().collect();
    compute_chunk_light_with_refs(chunk, &refs)
}

/// Borrowed-neighbour variant of [`compute_chunk_light`], used by
/// [`LightScene`] when it has split mutable access to its chunks.
pub fn compute_chunk_light_with_refs(chunk: &Chunk, neighbours: &[&Chunk]) -> LightStorage {
    let Some((min_section, max_section)) = section_bounds(chunk) else {
        return LightStorage::new();
    };
    let volume = Volume::new(chunk, neighbours, min_section, max_section);

    let (sky, sky_sources) = compute_sky(&volume);
    let block = compute_block(&volume);

    let mut storage = LightStorage::new();
    let top_sky = sky_top_section(&volume, &sky_sources);
    let top_block = block_top_section(&volume);
    let top = top_sky.max(top_block);
    let bottom = min_section;
    let last = (top + 1).min(max_section);

    for section_y in bottom..=last {
        storage.insert(
            section_y,
            SectionLight {
                sky: Some(extract_layer(&volume, section_y, &sky)),
                block: Some(extract_layer(&volume, section_y, &block)),
            },
        );
    }
    storage
}

fn section_bounds(chunk: &Chunk) -> Option<(i8, i8)> {
    chunk
        .sections
        .iter()
        .map(|section| section.y)
        .min()
        .zip(chunk.sections.iter().map(|section| section.y).max())
}

/// A padded single-block margin view over the centre chunk and any loaded
/// neighbours. Local coordinates use the centre chunk's origin; `-1..=16` is
/// the padded horizontal range.
struct Volume<'a> {
    chunks: HashMap<(i32, i32), &'a Chunk>,
    center: (i32, i32),
    min_section: i8,
    max_section: i8,
    min_y: i32,
    max_y: i32,
    x_size: usize,
    z_size: usize,
    y_size: usize,
}

const MARGIN: i32 = 1;

impl<'a> Volume<'a> {
    fn new(center: &'a Chunk, neighbours: &[&'a Chunk], min_section: i8, max_section: i8) -> Self {
        let mut chunks = HashMap::new();
        chunks.insert((center.x, center.z), center);
        for &neighbour in neighbours {
            if neighbour.x == center.x && neighbour.z == center.z {
                continue;
            }
            chunks.insert((neighbour.x, neighbour.z), neighbour);
        }
        let min_y = i32::from(min_section) * SECTION_BLOCKS;
        let max_y = i32::from(max_section) * SECTION_BLOCKS + 15;
        Self {
            chunks,
            center: (center.x, center.z),
            min_section,
            max_section,
            min_y,
            max_y,
            x_size: (SECTION_BLOCKS + MARGIN * 2) as usize,
            z_size: (SECTION_BLOCKS + MARGIN * 2) as usize,
            y_size: (max_y - min_y + 1) as usize,
        }
    }

    fn chunk_at(&self, local_x: i32, local_z: i32) -> Option<&'a Chunk> {
        let dx = local_x.div_euclid(SECTION_BLOCKS);
        let dz = local_z.div_euclid(SECTION_BLOCKS);
        if dx == 0 && dz == 0 {
            self.chunks.get(&self.center).copied()
        } else {
            self.chunks
                .get(&(self.center.0 + dx, self.center.1 + dz))
                .copied()
        }
    }

    fn state(&self, local_x: i32, local_z: i32, y: i32) -> Option<&'a BlockState> {
        if y < self.min_y || y > self.max_y {
            return None;
        }
        let chunk = self.chunk_at(local_x, local_z)?;
        let x = local_x.rem_euclid(SECTION_BLOCKS) as usize;
        let z = local_z.rem_euclid(SECTION_BLOCKS) as usize;
        chunk.block_state(x, y, z)
    }

    fn opacity(&self, local_x: i32, local_z: i32, y: i32) -> u8 {
        if self.chunk_at(local_x, local_z).is_none() {
            // An unloaded neighbour must not act as an air gap that lets light
            // wrap around a roof or cave wall at the loaded chunk's edge.
            return 15;
        }
        self.state(local_x, local_z, y)
            .map_or(0, crate::block_behaviour::light_block)
    }

    fn has_column(&self, local_x: i32, local_z: i32) -> bool {
        self.chunk_at(local_x, local_z).is_some()
    }

    fn index(&self, local_x: i32, local_z: i32, y: i32) -> usize {
        debug_assert!((-MARGIN..SECTION_BLOCKS + MARGIN).contains(&local_x));
        debug_assert!((-MARGIN..SECTION_BLOCKS + MARGIN).contains(&local_z));
        let x = (local_x + MARGIN) as usize;
        let z = (local_z + MARGIN) as usize;
        let y = (y - self.min_y) as usize;
        (y * self.z_size + z) * self.x_size + x
    }

    fn position(&self, index: usize) -> (i32, i32, i32) {
        let x = index % self.x_size;
        let z = (index / self.x_size) % self.z_size;
        let y = index / (self.x_size * self.z_size);
        (x as i32 - MARGIN, z as i32 - MARGIN, y as i32 + self.min_y)
    }

    fn total_cells(&self) -> usize {
        self.x_size * self.z_size * self.y_size
    }

    /// Vanilla `ChunkSkyLightSources`: return the Y of the lowest cell that
    /// still receives direct sky light, i.e. one above the first light-blocking
    /// block in the column. A fully transparent column starts at the bottom of
    /// the loaded volume.
    fn sky_source_y(&self, local_x: i32, local_z: i32) -> i32 {
        for y in (self.min_y..=self.max_y).rev() {
            if self.opacity(local_x, local_z, y) != 0 {
                return y + 1;
            }
        }
        self.min_y
    }

    fn section_from_y(&self, y: i32) -> i8 {
        y.div_euclid(SECTION_BLOCKS) as i8
    }
}

fn compute_sky(volume: &Volume<'_>) -> (Vec<u8>, Vec<i32>) {
    let mut light = vec![0u8; volume.total_cells()];
    let mut queue = VecDeque::new();
    let mut sources = Vec::with_capacity(volume.x_size * volume.z_size);

    for local_z in -MARGIN..SECTION_BLOCKS + MARGIN {
        for local_x in -MARGIN..SECTION_BLOCKS + MARGIN {
            if !volume.has_column(local_x, local_z) {
                continue;
            }
            let source_y = volume.sky_source_y(local_x, local_z);
            if (0..SECTION_BLOCKS).contains(&local_x) && (0..SECTION_BLOCKS).contains(&local_z) {
                sources.push(source_y);
            }
            for y in source_y..=volume.max_y {
                let index = volume.index(local_x, local_z, y);
                light[index] = 15;
                queue.push_back(index);
            }
        }
    }

    propagate(volume, &mut light, queue);
    (light, sources)
}

fn compute_block(volume: &Volume<'_>) -> Vec<u8> {
    let mut light = vec![0u8; volume.total_cells()];
    let mut queue = VecDeque::new();

    for local_z in -MARGIN..SECTION_BLOCKS + MARGIN {
        for local_x in -MARGIN..SECTION_BLOCKS + MARGIN {
            for y in volume.min_y..=volume.max_y {
                let Some(state) = volume.state(local_x, local_z, y) else {
                    continue;
                };
                let emission = crate::block_behaviour::light_emission(state);
                if emission == 0 {
                    continue;
                }
                let index = volume.index(local_x, local_z, y);
                if emission > light[index] {
                    light[index] = emission;
                    queue.push_back(index);
                }
            }
        }
    }

    propagate(volume, &mut light, queue);
    light
}

fn propagate(volume: &Volume<'_>, light: &mut [u8], mut queue: VecDeque<usize>) {
    while let Some(index) = queue.pop_front() {
        let level = light[index];
        if level <= 1 {
            continue;
        }
        let (x, z, y) = volume.position(index);
        for (dx, dy, dz) in PROPAGATION_DIRECTIONS {
            let nx = x + dx;
            let ny = y + dy;
            let nz = z + dz;
            if !(-MARGIN..SECTION_BLOCKS + MARGIN).contains(&nx)
                || !(-MARGIN..SECTION_BLOCKS + MARGIN).contains(&nz)
                || ny < volume.min_y
                || ny > volume.max_y
            {
                continue;
            }
            let opacity = volume.opacity(nx, nz, ny);
            let cost = opacity.max(1);
            if level <= cost {
                continue;
            }
            let candidate = level - cost;
            let neighbour = volume.index(nx, nz, ny);
            if candidate > light[neighbour] {
                light[neighbour] = candidate;
                queue.push_back(neighbour);
            }
        }
    }
}

fn sky_top_section(volume: &Volume<'_>, sources: &[i32]) -> i8 {
    let mut top = volume.min_section;
    let mut all_transparent = true;
    for &source_y in sources {
        if source_y <= volume.min_y {
            continue;
        }
        all_transparent = false;
        let first_blocked = source_y - 1;
        top = top.max(volume.section_from_y(first_blocked));
    }
    if all_transparent {
        volume.max_section
    } else {
        top
    }
}

fn block_top_section(volume: &Volume<'_>) -> i8 {
    let mut top = volume.min_section;
    let Some(center) = volume.chunks.get(&volume.center).copied() else {
        return top;
    };
    for section in &center.sections {
        for y in 0..SECTION_SIZE {
            let world_y = i32::from(section.y) * SECTION_BLOCKS + y as i32;
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let Some(state) = volume.state(x as i32, z as i32, world_y) else {
                        continue;
                    };
                    if crate::block_behaviour::light_emission(state) > 0 {
                        top = top.max(section.y);
                    }
                }
            }
        }
    }
    top
}

fn extract_layer(volume: &Volume<'_>, section_y: i8, light: &[u8]) -> DataLayer {
    let mut layer = DataLayer::new();
    let base_y = i32::from(section_y) * SECTION_BLOCKS;
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let index = volume.index(x as i32, z as i32, base_y + y as i32);
                layer.set(x, y, z, light[index]);
            }
        }
    }
    layer
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Layer {
    Sky,
    Block,
}

/// One pending increase. `level` is the level the entry wants the cell to
/// reach; emission entries force it (Java's `increaseLightFromEmission`),
/// plain entries only propagate when the cell still has that level.
#[derive(Debug, Clone, Copy)]
struct IncreaseEntry {
    level: u8,
    mask: u8,
    emission: bool,
}

#[derive(Debug, Clone, Copy)]
struct IncreaseSeed {
    level: u8,
    mask: u8,
    emission: bool,
}

impl IncreaseSeed {
    fn plain(level: u8, mask: u8) -> Self {
        Self {
            level,
            mask,
            emission: false,
        }
    }

    fn emission(level: u8) -> Self {
        Self {
            level,
            mask: DIRECTION_ALL,
            emission: true,
        }
    }
}

#[derive(Default)]
struct IncreaseQueue {
    order: VecDeque<(i32, i32, i32, Layer)>,
    pending: HashMap<(i32, i32, i32, Layer), IncreaseEntry>,
}

impl IncreaseQueue {
    fn push(&mut self, x: i32, y: i32, z: i32, layer: Layer, seed: IncreaseSeed) {
        let key = (x, y, z, layer);
        match self.pending.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                value.level = value.level.max(seed.level);
                value.mask |= seed.mask;
                value.emission |= seed.emission;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(IncreaseEntry {
                    level: seed.level,
                    mask: seed.mask,
                    emission: seed.emission,
                });
                self.order.push_back(key);
            }
        }
    }

    fn pop(&mut self) -> Option<(i32, i32, i32, Layer, IncreaseEntry)> {
        let (x, y, z, layer) = self.order.pop_front()?;
        let entry = self.pending.remove(&(x, y, z, layer))?;
        Some((x, y, z, layer, entry))
    }

    fn clear(&mut self) {
        self.order.clear();
        self.pending.clear();
    }
}

#[derive(Debug, Clone, Copy)]
struct DecreaseEntry {
    x: i32,
    y: i32,
    z: i32,
    layer: Layer,
    from_level: u8,
    mask: u8,
}

#[derive(Default)]
struct DecreaseQueue {
    order: VecDeque<DecreaseEntry>,
    pending: HashSet<(i32, i32, i32, Layer)>,
}

impl DecreaseQueue {
    fn push(&mut self, x: i32, y: i32, z: i32, layer: Layer, from_level: u8, mask: u8) {
        if self.pending.insert((x, y, z, layer)) {
            self.order.push_back(DecreaseEntry {
                x,
                y,
                z,
                layer,
                from_level,
                mask,
            });
        }
    }

    fn pop(&mut self) -> Option<DecreaseEntry> {
        let entry = self.order.pop_front()?;
        self.pending
            .remove(&(entry.x, entry.y, entry.z, entry.layer));
        Some(entry)
    }

    fn clear(&mut self) {
        self.order.clear();
        self.pending.clear();
    }
}

/// Incremental light engine over an edited chunk plus its loaded neighbours.
///
/// The constructor materializes a consistent starting state: chunks whose
/// light was never computed get one whole-chunk pass (a one-time
/// initialization), while already-lit chunks only gain their missing
/// above-surface sky layers. After that, [`Self::set_block_state`] runs only
/// the affected decrease/increase region and records changed sections in
/// [`Self::dirty`].
pub struct LightScene<'a> {
    chunks: Vec<&'a mut Chunk>,
    index: HashMap<(i32, i32), usize>,
    center: usize,
    min_y: i32,
    max_y: i32,
    dirty: DirtySections,
    increase: IncreaseQueue,
    decrease: DecreaseQueue,
}

impl<'a> LightScene<'a> {
    /// Build a scene over `center` and any subset of its neighbours. Duplicate
    /// and same-column entries in `neighbours` are ignored.
    pub fn new(center: &'a mut Chunk, neighbours: &'a mut [Chunk]) -> Self {
        let mut seen = HashSet::new();
        seen.insert((center.x, center.z));
        let mut neighbour_refs: Vec<&'a mut Chunk> = Vec::new();
        for neighbour in neighbours.iter_mut() {
            if seen.insert((neighbour.x, neighbour.z)) {
                neighbour_refs.push(neighbour);
            }
        }
        // Deterministic scene order regardless of how the caller collected
        // neighbours from storage.
        neighbour_refs.sort_by_key(|chunk| (chunk.x, chunk.z));

        let center_coords = (center.x, center.z);
        let mut chunks: Vec<&'a mut Chunk> = Vec::with_capacity(neighbour_refs.len() + 1);
        chunks.push(center);
        chunks.extend(neighbour_refs);
        let center_index = 0;
        let mut index = HashMap::with_capacity(chunks.len());
        for (slot, chunk) in chunks.iter().enumerate() {
            index.insert((chunk.x, chunk.z), slot);
        }

        let mut min_section = i8::MAX;
        let mut max_section = i8::MIN;
        for chunk in &chunks {
            for section in &chunk.sections {
                min_section = min_section.min(section.y);
                max_section = max_section.max(section.y);
            }
        }
        if min_section == i8::MAX {
            min_section = 0;
            max_section = 0;
        }

        let mut scene = Self {
            chunks,
            index,
            center: center_index,
            min_y: i32::from(min_section) * SECTION_BLOCKS,
            max_y: i32::from(max_section) * SECTION_BLOCKS + 15,
            dirty: DirtySections::new(),
            increase: IncreaseQueue::default(),
            decrease: DecreaseQueue::default(),
        };
        debug_assert_eq!(
            scene.index.get(&center_coords),
            Some(&center_index),
            "center chunk must stay at slot zero"
        );
        scene.ensure_initialized();
        scene
    }

    pub fn center_coords(&self) -> (i32, i32) {
        let chunk = &*self.chunks[self.center];
        (chunk.x, chunk.z)
    }

    /// Sections changed since the last [`Self::clear_dirty`].
    pub fn dirty(&self) -> &DirtySections {
        &self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    /// Light level currently stored for a block position. Missing chunks or
    /// layers read as zero.
    pub fn sky_level(&self, x: i32, y: i32, z: i32) -> u8 {
        self.layer_get(Layer::Sky, x, y, z)
    }

    pub fn block_level(&self, x: i32, y: i32, z: i32) -> u8 {
        self.layer_get(Layer::Block, x, y, z)
    }

    /// Mutate one block and run the incremental light update.
    ///
    /// Returns false when the block state did not change (the light layers are
    /// left untouched and nothing is marked dirty).
    pub fn set_block_state(&mut self, x: i32, y: i32, z: i32, state: BlockState) -> bool {
        let old_state = self.state(x, y, z).cloned();
        let Some(&chunk_index) = self
            .index
            .get(&(x.div_euclid(SECTION_BLOCKS), z.div_euclid(SECTION_BLOCKS)))
        else {
            return false;
        };
        let local_x = x.rem_euclid(SECTION_BLOCKS) as usize;
        let local_z = z.rem_euclid(SECTION_BLOCKS) as usize;

        // `Chunk::set_block_state` invalidates the whole chunk's light because
        // it cannot know which sections an edit affects. Capture the current
        // layers, replay them, then let the scene compute the delta itself.
        let previous = LightStorage::from_chunk(&*self.chunks[chunk_index]);
        if !self.chunks[chunk_index].set_block_state(local_x, y, local_z, state) {
            return false;
        }
        previous.apply_to(&mut *self.chunks[chunk_index]);
        self.update_block(x, y, z, old_state.as_ref());
        true
    }

    /// Discard all existing light and rebuild every loaded chunk in the scene
    /// from scratch. Every section is marked dirty because a full rebuild can
    /// clear sections that previously held light.
    pub fn recompute_all(&mut self) {
        for chunk in &mut self.chunks {
            chunk.clear_light();
        }
        self.dirty.clear();
        self.increase.clear();
        self.decrease.clear();

        self.seed_sky_sources();
        self.seed_block_sources();
        self.propagate_increases();

        for chunk in &mut self.chunks {
            let (chunk_x, chunk_z) = (chunk.x, chunk.z);
            for section in &mut chunk.sections {
                if section.light.sky.is_none() {
                    section.light.sky = Some(DataLayer::new());
                }
                if section.light.block.is_none() {
                    section.light.block = Some(DataLayer::new());
                }
                self.dirty
                    .insert(LightSectionPos::new(chunk_x, chunk_z, section.y));
            }
        }
    }

    fn ensure_initialized(&mut self) {
        for slot in 0..self.chunks.len() {
            if chunk_has_sky(&*self.chunks[slot]) {
                materialize_sky_layers(&mut *self.chunks[slot]);
                continue;
            }

            // A chunk without light data has never been through an engine
            // (fresh generation or a stored chunk that predates light
            // persistence). Compute one whole-chunk pass so later edits have a
            // consistent base. This is initialization, not per-edit work.
            let storage = {
                let total = self.chunks.len();
                let (left, right) = self.chunks.split_at_mut(slot);
                let (target, rest) = right.split_first_mut().expect("slot index is valid");
                let mut others: Vec<&Chunk> = Vec::with_capacity(total - 1);
                others.extend(left.iter().map(|chunk| &**chunk));
                others.extend(rest.iter().map(|chunk| &**chunk));
                compute_chunk_light_with_refs(target, &others)
            };
            storage.apply_to(&mut *self.chunks[slot]);
            materialize_sky_layers(&mut *self.chunks[slot]);
            let chunk = &*self.chunks[slot];
            for section in &chunk.sections {
                self.dirty
                    .insert(LightSectionPos::new(chunk.x, chunk.z, section.y));
            }
        }
    }

    fn seed_sky_sources(&mut self) {
        let chunk_coords: Vec<(i32, i32)> =
            self.chunks.iter().map(|chunk| (chunk.x, chunk.z)).collect();
        let max_y = self.max_y;
        for (chunk_x, chunk_z) in chunk_coords {
            for local_z in 0..SECTION_BLOCKS {
                for local_x in 0..SECTION_BLOCKS {
                    let x = chunk_x * SECTION_BLOCKS + local_x;
                    let z = chunk_z * SECTION_BLOCKS + local_z;
                    let source = self.column_source_y(x, z);
                    for y in source..=max_y {
                        self.layer_set(Layer::Sky, x, y, z, MAX_LIGHT);
                        // The whole direct-sky run is seeded so light can
                        // leave a shaft sideways at every depth under an
                        // overhang.
                        self.increase.push(
                            x,
                            y,
                            z,
                            Layer::Sky,
                            IncreaseSeed::plain(MAX_LIGHT, DIRECTION_ALL),
                        );
                    }
                }
            }
        }
    }

    fn seed_block_sources(&mut self) {
        let positions: Vec<(i32, i32, i32, u8)> = self
            .chunks
            .iter()
            .flat_map(|chunk| {
                let chunk_x = chunk.x * SECTION_BLOCKS;
                let chunk_z = chunk.z * SECTION_BLOCKS;
                chunk.sections.iter().flat_map(move |section| {
                    let base_y = i32::from(section.y) * SECTION_BLOCKS;
                    (0..SECTION_SIZE).flat_map(move |y| {
                        (0..SECTION_SIZE).flat_map(move |z| {
                            (0..SECTION_SIZE).filter_map(move |x| {
                                let state = section.block_states.get((y * 16 + z) * 16 + x)?;
                                let emission = crate::block_behaviour::light_emission(state);
                                (emission > 0).then_some((
                                    chunk_x + x as i32,
                                    base_y + y as i32,
                                    chunk_z + z as i32,
                                    emission,
                                ))
                            })
                        })
                    })
                })
            })
            .collect();
        for (x, y, z, emission) in positions {
            self.layer_set(Layer::Block, x, y, z, emission);
            self.increase
                .push(x, y, z, Layer::Block, IncreaseSeed::emission(emission));
        }
    }

    fn update_block(&mut self, x: i32, y: i32, z: i32, old_state: Option<&BlockState>) {
        let Some(new_state) = self.state(x, y, z).cloned() else {
            return;
        };
        let new_emission = crate::block_behaviour::light_emission(&new_state);
        self.update_block_light(x, y, z, new_emission);

        let old_opacity = old_state.map_or(0, crate::block_behaviour::light_block);
        let new_opacity = crate::block_behaviour::light_block(&new_state);
        if old_opacity != new_opacity {
            self.update_sky_column(x, y, z, old_opacity);
        }
        self.flush_queues();
    }

    /// Port of `BlockLightEngine.checkNode`'s decrease half plus the increase
    /// seed for the new emission.
    fn update_block_light(&mut self, x: i32, y: i32, z: i32, new_emission: u8) {
        let stored = self.layer_get(Layer::Block, x, y, z);
        if new_emission < stored {
            self.layer_set(Layer::Block, x, y, z, 0);
            self.decrease
                .push(x, y, z, Layer::Block, stored, DIRECTION_ALL);
        } else {
            // Java's PULL_LIGHT_IN_ENTRY: re-pull from any lit neighbour.
            self.decrease.push(x, y, z, Layer::Block, 1, DIRECTION_ALL);
        }
        if new_emission > 0 {
            self.increase
                .push(x, y, z, Layer::Block, IncreaseSeed::emission(new_emission));
        }
    }

    /// Port of `SkyLightEngine`'s column source bookkeeping for one changed
    /// column, followed by the per-cell decrease/pull at the edit position.
    fn update_sky_column(&mut self, x: i32, y: i32, z: i32, old_opacity: u8) {
        let old_source = self.column_source_y_with(x, z, Some((y, old_opacity)));
        let new_source = self.column_source_y(x, z);

        if new_source > old_source {
            let top = new_source - 1;
            for removed_y in (old_source..=top).rev() {
                if self.layer_get(Layer::Sky, x, removed_y, z) != MAX_LIGHT {
                    break;
                }
                self.layer_set(Layer::Sky, x, removed_y, z, 0);
                // The cell directly under the new source sees every direction;
                // deeper cells skip UP because their upper neighbour is part of
                // the same removed run.
                let mask = if removed_y == top {
                    DIRECTION_ALL
                } else {
                    DIRECTION_ALL & !DIRECTION_UP
                };
                self.decrease
                    .push(x, removed_y, z, Layer::Sky, MAX_LIGHT, mask);
            }
        } else if new_source < old_source {
            for added_y in new_source..old_source {
                self.layer_set(Layer::Sky, x, added_y, z, MAX_LIGHT);
                self.increase.push(
                    x,
                    added_y,
                    z,
                    Layer::Sky,
                    IncreaseSeed::plain(MAX_LIGHT, DIRECTION_ALL & !DIRECTION_UP),
                );
            }
        }

        if y >= new_source {
            if self.layer_get(Layer::Sky, x, y, z) != MAX_LIGHT {
                self.layer_set(Layer::Sky, x, y, z, MAX_LIGHT);
                self.increase.push(
                    x,
                    y,
                    z,
                    Layer::Sky,
                    IncreaseSeed::plain(MAX_LIGHT, DIRECTION_ALL & !DIRECTION_UP),
                );
            }
        } else {
            let stored = self.layer_get(Layer::Sky, x, y, z);
            if stored > 0 {
                self.layer_set(Layer::Sky, x, y, z, 0);
                self.decrease
                    .push(x, y, z, Layer::Sky, stored, DIRECTION_ALL);
            } else {
                self.decrease.push(x, y, z, Layer::Sky, 1, DIRECTION_ALL);
            }
        }
    }

    fn propagate_increases(&mut self) {
        while let Some((x, y, z, layer, entry)) = self.increase.pop() {
            let mut level = self.layer_get(layer, x, y, z);
            if entry.emission && level < entry.level {
                self.layer_set(layer, x, y, z, entry.level);
                level = entry.level;
            } else if !entry.emission && level < entry.level {
                // A decrease zeroed this cell after the entry was queued.
                continue;
            }
            if level <= 1 {
                continue;
            }

            for (direction, (dx, dy, dz)) in PROPAGATION_DIRECTIONS.iter().enumerate() {
                if entry.mask & (1 << direction) == 0 {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                let nz = z + dz;
                if !self.has_chunk(nx, nz) {
                    continue;
                }
                let opacity = self.opacity(nx, ny, nz).max(1);
                if level <= opacity {
                    continue;
                }
                let candidate = level - opacity;
                if candidate > self.layer_get(layer, nx, ny, nz) {
                    self.layer_set(layer, nx, ny, nz, candidate);
                    if candidate > 1 {
                        self.increase.push(
                            nx,
                            ny,
                            nz,
                            layer,
                            IncreaseSeed::plain(candidate, DIRECTION_ALL & !(1 << (direction ^ 1))),
                        );
                    }
                }
            }
        }
    }

    fn propagate_decreases(&mut self) {
        while let Some(entry) = self.decrease.pop() {
            let DecreaseEntry {
                x,
                y,
                z,
                layer,
                from_level,
                mask,
            } = entry;
            for (direction, (dx, dy, dz)) in PROPAGATION_DIRECTIONS.iter().enumerate() {
                if mask & (1 << direction) == 0 {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                let nz = z + dz;
                if !self.has_chunk(nx, nz) {
                    continue;
                }
                let neighbour_level = self.layer_get(layer, nx, ny, nz);
                if neighbour_level == 0 {
                    continue;
                }
                if neighbour_level < from_level {
                    let emission = if layer == Layer::Block {
                        self.emission(nx, ny, nz)
                    } else {
                        0
                    };
                    self.layer_set(layer, nx, ny, nz, 0);
                    self.decrease.push(
                        nx,
                        ny,
                        nz,
                        layer,
                        neighbour_level,
                        DIRECTION_ALL & !(1 << (direction ^ 1)),
                    );
                    if emission > 0 {
                        self.increase
                            .push(nx, ny, nz, layer, IncreaseSeed::emission(emission));
                    }
                } else {
                    // This neighbour was not lit by the decreased cell; let it
                    // propagate back toward the hole.
                    self.increase.push(
                        nx,
                        ny,
                        nz,
                        layer,
                        IncreaseSeed::plain(neighbour_level, 1 << (direction ^ 1)),
                    );
                }
            }
        }
    }

    fn flush_queues(&mut self) {
        self.propagate_decreases();
        self.propagate_increases();
    }

    fn column_source_y(&self, x: i32, z: i32) -> i32 {
        self.column_source_y_with(x, z, None)
    }

    fn column_source_y_with(&self, x: i32, z: i32, override_state: Option<(i32, u8)>) -> i32 {
        for y in (self.min_y..=self.max_y).rev() {
            let opacity = match override_state {
                Some((override_y, opacity)) if y == override_y => opacity,
                _ => self.opacity(x, y, z),
            };
            if opacity != 0 {
                return y + 1;
            }
        }
        self.min_y
    }

    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Option<&Chunk> {
        self.index
            .get(&(chunk_x, chunk_z))
            .map(|&slot| &*self.chunks[slot])
    }

    fn has_chunk(&self, x: i32, z: i32) -> bool {
        self.index
            .contains_key(&(x.div_euclid(SECTION_BLOCKS), z.div_euclid(SECTION_BLOCKS)))
    }

    fn state(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        let chunk = self.chunk(x.div_euclid(SECTION_BLOCKS), z.div_euclid(SECTION_BLOCKS))?;
        chunk.block_state(
            x.rem_euclid(SECTION_BLOCKS) as usize,
            y,
            z.rem_euclid(SECTION_BLOCKS) as usize,
        )
    }

    fn opacity(&self, x: i32, y: i32, z: i32) -> u8 {
        let chunk_x = x.div_euclid(SECTION_BLOCKS);
        let chunk_z = z.div_euclid(SECTION_BLOCKS);
        if !self.index.contains_key(&(chunk_x, chunk_z)) {
            return 15;
        }
        self.state(x, y, z)
            .map_or(0, crate::block_behaviour::light_block)
    }

    fn emission(&self, x: i32, y: i32, z: i32) -> u8 {
        self.state(x, y, z)
            .map_or(0, crate::block_behaviour::light_emission)
    }

    fn layer_get(&self, layer: Layer, x: i32, y: i32, z: i32) -> u8 {
        let chunk_x = x.div_euclid(SECTION_BLOCKS);
        let chunk_z = z.div_euclid(SECTION_BLOCKS);
        let Some(chunk) = self.chunk(chunk_x, chunk_z) else {
            return 0;
        };
        let section_y = y.div_euclid(SECTION_BLOCKS) as i8;
        let Some(section) = chunk.section(section_y) else {
            return 0;
        };
        let local_x = x.rem_euclid(SECTION_BLOCKS) as usize;
        let local_y = y.rem_euclid(SECTION_BLOCKS) as usize;
        let local_z = z.rem_euclid(SECTION_BLOCKS) as usize;
        match layer {
            Layer::Sky => section.light.sky_get(local_x, local_y, local_z),
            Layer::Block => section.light.block_get(local_x, local_y, local_z),
        }
        .unwrap_or(0)
    }

    fn layer_set(&mut self, layer: Layer, x: i32, y: i32, z: i32, value: u8) -> bool {
        let chunk_x = x.div_euclid(SECTION_BLOCKS);
        let chunk_z = z.div_euclid(SECTION_BLOCKS);
        let Some(&chunk_index) = self.index.get(&(chunk_x, chunk_z)) else {
            return false;
        };
        let section_y = y.div_euclid(SECTION_BLOCKS) as i8;
        let local_x = x.rem_euclid(SECTION_BLOCKS) as usize;
        let local_y = y.rem_euclid(SECTION_BLOCKS) as usize;
        let local_z = z.rem_euclid(SECTION_BLOCKS) as usize;

        let chunk = &mut *self.chunks[chunk_index];
        let Some(section) = chunk
            .sections
            .iter_mut()
            .find(|section| section.y == section_y)
        else {
            return false;
        };
        let current = match layer {
            Layer::Sky => section.light.sky_get(local_x, local_y, local_z),
            Layer::Block => section.light.block_get(local_x, local_y, local_z),
        }
        .unwrap_or(0);
        if current == value {
            return false;
        }
        let target = match layer {
            Layer::Sky => section.light.sky.get_or_insert_with(DataLayer::new),
            Layer::Block => section.light.block.get_or_insert_with(DataLayer::new),
        };
        target.set(local_x, local_y, local_z, value);
        self.dirty
            .insert(LightSectionPos::new(chunk_x, chunk_z, section_y));
        true
    }
}

fn chunk_has_sky(chunk: &Chunk) -> bool {
    chunk
        .sections
        .iter()
        .any(|section| section.light.sky.is_some())
}

/// Extend a chunk's sky layers to every section. Sections above the highest
/// materialized layer are always direct sky after a whole-chunk compute, so
/// fill them with 15; gaps below it stay at zero. This keeps the incremental
/// source scan independent of which layers a previous compute materialized.
fn materialize_sky_layers(chunk: &mut Chunk) {
    let Some(highest) = chunk
        .sections
        .iter()
        .filter(|section| section.light.sky.is_some())
        .map(|section| section.y)
        .max()
    else {
        return;
    };
    for section in &mut chunk.sections {
        if section.light.sky.is_none() {
            section.light.sky = Some(if section.y > highest {
                DataLayer::filled(MAX_LIGHT)
            } else {
                DataLayer::new()
            });
        }
    }
}

fn light_section_index(section_y: i8) -> Option<usize> {
    let index = i32::from(section_y) - (i32::from(OVERWORLD_MIN_SECTION) - 1);
    (0..LIGHT_SECTION_COUNT as i32)
        .contains(&index)
        .then_some(index as usize)
}

/// Build a `ClientboundLightUpdate` payload (`ClientboundLightUpdatePacketData`
/// wire shape) containing only the dirty sections of `chunk`.
///
/// Sections without any stored layer are skipped, exactly like Java's
/// `prepareSectionData` when `getDataLayerData` returns null. Returns `None`
/// when `dirty` has no section for this chunk.
pub fn light_update_for_sections(chunk: &Chunk, dirty: &DirtySections) -> Option<Vec<u8>> {
    let mut sky_mask = 0u64;
    let mut block_mask = 0u64;
    let mut empty_sky = 0u64;
    let mut empty_block = 0u64;
    let mut sky_updates: Vec<DataLayer> = Vec::new();
    let mut block_updates: Vec<DataLayer> = Vec::new();
    let mut any = false;

    for pos in dirty.for_chunk(chunk.x, chunk.z) {
        let Some(index) = light_section_index(pos.section_y) else {
            continue;
        };
        let Some(section) = chunk.section(pos.section_y) else {
            continue;
        };
        any = true;
        if let Some(sky) = section.light.sky.as_ref() {
            if sky.is_empty() {
                empty_sky |= 1 << index;
            } else {
                sky_mask |= 1 << index;
                sky_updates.push(sky.clone());
            }
        }
        if let Some(block) = section.light.block.as_ref() {
            if block.is_empty() {
                empty_block |= 1 << index;
            } else {
                block_mask |= 1 << index;
                block_updates.push(block.clone());
            }
        }
    }

    if !any {
        return None;
    }

    let mut writer = PacketWriter::new();
    write_bitset(&mut writer, sky_mask);
    write_bitset(&mut writer, block_mask);
    write_bitset(&mut writer, empty_sky);
    write_bitset(&mut writer, empty_block);
    writer.write_varint(sky_updates.len() as i32).unwrap();
    for update in sky_updates {
        writer.write_varint(DATA_LAYER_BYTES as i32).unwrap();
        writer.write_bytes(update.as_bytes());
    }
    writer.write_varint(block_updates.len() as i32).unwrap();
    for update in block_updates {
        writer.write_varint(DATA_LAYER_BYTES as i32).unwrap();
        writer.write_bytes(update.as_bytes());
    }
    Some(writer.into_inner())
}

fn write_bitset(writer: &mut PacketWriter, mask: u64) {
    if mask == 0 {
        writer.write_varint(0).unwrap();
    } else {
        writer.write_varint(1).unwrap();
        writer.write_i64(mask as i64).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_behaviour::{light_block, light_emission};

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: vec![],
        }
    }

    #[test]
    fn data_layer_packs_nibbles_like_vanilla() {
        let mut layer = DataLayer::new();
        layer.set(0, 0, 0, 0x0A);
        layer.set(1, 0, 0, 0x05);
        assert_eq!(layer.as_bytes()[0], 0x5A);
        assert_eq!(layer.get(0, 0, 0), 0x0A);
        assert_eq!(layer.get(1, 0, 0), 0x05);

        layer.set(15, 15, 15, 0x0F);
        assert_eq!(layer.get(15, 15, 15), 0x0F);
        assert!(!layer.is_empty());
        layer.fill(15);
        assert!(layer.is_filled_with(15));
        assert!(DataLayer::filled(15).is_filled_with(15));
    }

    #[test]
    fn data_layer_rejects_wrong_length() {
        assert!(DataLayer::from_bytes(&[0; 2047]).is_err());
        assert!(DataLayer::from_bytes(&[0; 2049]).is_err());
        assert!(DataLayer::from_bytes(&[0; 2048]).is_ok());
    }

    #[test]
    fn generated_block_behaviour_drives_light_values() {
        let air = state("minecraft:air");
        assert_eq!(light_block(&air), 0);
        assert_eq!(light_emission(&air), 0);

        let torch = state("minecraft:torch");
        assert_eq!(light_emission(&torch), 14);
        assert_eq!(light_block(&torch), 0);

        for name in ["minecraft:glowstone", "minecraft:lava"] {
            assert_eq!(light_emission(&state(name)), 15, "{name}");
        }

        let magma = state("minecraft:magma_block");
        assert_eq!(light_emission(&magma), 3, "vanilla magma emits 3, not 15");

        assert_eq!(
            light_block(&state("minecraft:water")),
            1,
            "water attenuates by one"
        );
        assert_eq!(
            light_block(&state("minecraft:oak_leaves")),
            1,
            "leaves attenuate by one"
        );
        assert_eq!(
            light_block(&state("minecraft:glass")),
            0,
            "glass does not attenuate light"
        );
        assert_eq!(
            light_block(&state("minecraft:tinted_glass")),
            15,
            "tinted glass is opaque"
        );

        let unknown = state("minecraft:not_a_real_emitter");
        assert_eq!(light_emission(&unknown), 0);
        assert_eq!(
            light_block(&unknown),
            15,
            "unknown states stay conservative and opaque"
        );
    }

    #[test]
    fn dirty_sections_are_sorted_and_deduplicated() {
        let mut dirty = DirtySections::new();
        dirty.insert(LightSectionPos::new(1, 0, 4));
        dirty.insert(LightSectionPos::new(0, 0, 1));
        dirty.insert(LightSectionPos::new(1, 0, 4));
        let order: Vec<_> = dirty.iter().copied().collect();
        assert_eq!(
            order,
            vec![LightSectionPos::new(0, 0, 1), LightSectionPos::new(1, 0, 4),]
        );
        assert_eq!(dirty.len(), 2);
    }
}
