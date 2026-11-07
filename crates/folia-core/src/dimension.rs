//! Dimension storage (Nether/End region files) and portal transfer.
//!
//! Region paths already live on [`crate::world::Dimension`]; this module owns a
//! lazily opened [`WorldStorage`] per dimension, a generation-on-miss helper
//! built on [`crate::worldgen::generator::generate_dimension`], and the
//! coordinate math / frame detection used by portal travel.

use crate::anvil::Compression;
use crate::chunk::Chunk;
use crate::world::{Dimension, WorldStorage};
use crate::worldgen::generator::{self, Dimension as GeneratorDimension};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Vanilla Nether portal coordinate scale.
pub const NETHER_PORTAL_SCALE: f64 = 8.0;

/// End portal arrival point (`ServerLevel.makeObsidianPlatform`).
pub const END_PLATFORM_POSITION: [f64; 3] = [100.5, 50.0, 0.5];

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
/// the obsidian platform; the Nether pair scales X/Z and keeps Y.
pub fn portal_destination(from: Dimension, to: Dimension, x: f64, y: f64, z: f64) -> [f64; 3] {
    match to {
        Dimension::End => END_PLATFORM_POSITION,
        _ => {
            let (x, z) = scaled_coordinates(from, to, x, z);
            [x, y, z]
        }
    }
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
