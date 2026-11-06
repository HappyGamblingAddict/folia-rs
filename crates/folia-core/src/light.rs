//! Light storage and the first real sky/block light engines.
//!
//! Vanilla stores one 2048-byte nibble array per 16x16x16 section for each
//! light layer (`DataLayer`). This module ports that storage format, the
//! per-column sky-light source scan from `ChunkSkyLightSources`, and the
//! breadth-first increase propagation from `SkyLightEngine`/`BlockLightEngine`.
//!
//! Opacity and emission come from the generated per-state
//! [`crate::block_behaviour`] table. Unknown states resolve the same way
//! [`crate::block_behaviour::light_block`] does: fully opaque and non-emitting,
//! which is conservative because it can never fabricate light.
//! TODO(P3.7): port the incremental decrease queues and Java light fixtures.

use crate::chunk::{BlockState, Chunk};
use std::collections::{BTreeMap, HashMap, VecDeque};
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

/// Light arrays keyed by section Y, the shape the engine computes before it is
/// applied back into a chunk's sections.
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

    fn apply_to(&self, chunk: &mut Chunk) {
        for section in &mut chunk.sections {
            section.light = self.sections.get(&section.y).cloned().unwrap_or_default();
        }
    }
}

fn layer_index(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
    (y << 8) | (z << 4) | x
}

/// Recompute both light layers for one chunk without loading neighbours.
///
/// Chunk-border light is handled as well as a single chunk allows: the engine
/// seeds the one-block border from the chunk itself and propagates inward, but
/// cannot import light from a neighbour that is not supplied. Use
/// [`recompute_chunk_light_with_neighbours`] when the loaded neighbours are
/// available.
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
    center: &'a Chunk,
    neighbours: HashMap<(i32, i32), &'a Chunk>,
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
    fn new(center: &'a Chunk, neighbours: &'a [Chunk], min_section: i8, max_section: i8) -> Self {
        let neighbours = neighbours
            .iter()
            .filter(|chunk| chunk.x != center.x || chunk.z != center.z)
            .map(|chunk| ((chunk.x, chunk.z), chunk))
            .collect();
        let min_y = i32::from(min_section) * SECTION_SIZE as i32;
        let max_y = i32::from(max_section) * SECTION_SIZE as i32 + 15;
        Self {
            center,
            neighbours,
            min_section,
            max_section,
            min_y,
            max_y,
            x_size: (SECTION_SIZE as i32 + MARGIN * 2) as usize,
            z_size: (SECTION_SIZE as i32 + MARGIN * 2) as usize,
            y_size: (max_y - min_y + 1) as usize,
        }
    }

    fn chunk_at(&self, local_x: i32, local_z: i32) -> Option<&'a Chunk> {
        let dx = local_x.div_euclid(SECTION_SIZE as i32);
        let dz = local_z.div_euclid(SECTION_SIZE as i32);
        if dx == 0 && dz == 0 {
            Some(self.center)
        } else {
            self.neighbours
                .get(&(self.center.x + dx, self.center.z + dz))
                .copied()
        }
    }

    fn state(&self, local_x: i32, local_z: i32, y: i32) -> Option<&'a BlockState> {
        if y < self.min_y || y > self.max_y {
            return None;
        }
        let chunk = self.chunk_at(local_x, local_z)?;
        let x = local_x.rem_euclid(SECTION_SIZE as i32) as usize;
        let z = local_z.rem_euclid(SECTION_SIZE as i32) as usize;
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
        debug_assert!((-MARGIN..SECTION_SIZE as i32 + MARGIN).contains(&local_x));
        debug_assert!((-MARGIN..SECTION_SIZE as i32 + MARGIN).contains(&local_z));
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
        y.div_euclid(SECTION_SIZE as i32) as i8
    }
}

fn compute_sky(volume: &Volume<'_>) -> (Vec<u8>, Vec<i32>) {
    let mut light = vec![0u8; volume.total_cells()];
    let mut queue = VecDeque::new();
    let mut sources = Vec::with_capacity(volume.x_size * volume.z_size);

    for local_z in -MARGIN..SECTION_SIZE as i32 + MARGIN {
        for local_x in -MARGIN..SECTION_SIZE as i32 + MARGIN {
            if !volume.has_column(local_x, local_z) {
                continue;
            }
            let source_y = volume.sky_source_y(local_x, local_z);
            if local_x >= 0
                && local_x < SECTION_SIZE as i32
                && local_z >= 0
                && local_z < SECTION_SIZE as i32
            {
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

    for local_z in -MARGIN..SECTION_SIZE as i32 + MARGIN {
        for local_x in -MARGIN..SECTION_SIZE as i32 + MARGIN {
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
    const DIRECTIONS: [(i32, i32, i32); 6] = [
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (-1, 0, 0),
        (1, 0, 0),
    ];

    while let Some(index) = queue.pop_front() {
        let level = light[index];
        if level <= 1 {
            continue;
        }
        let (x, z, y) = volume.position(index);
        for (dx, dy, dz) in DIRECTIONS {
            let nx = x + dx;
            let ny = y + dy;
            let nz = z + dz;
            if !(-MARGIN..SECTION_SIZE as i32 + MARGIN).contains(&nx)
                || !(-MARGIN..SECTION_SIZE as i32 + MARGIN).contains(&nz)
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
    for section in &volume.center.sections {
        for y in 0..SECTION_SIZE {
            let world_y = i32::from(section.y) * SECTION_SIZE as i32 + y as i32;
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
    let base_y = i32::from(section_y) * SECTION_SIZE as i32;
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
}
