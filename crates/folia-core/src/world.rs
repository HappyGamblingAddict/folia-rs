use crate::anvil::{Compression, RegionFile};
use crate::chunk::{Chunk, decode_chunk, encode_chunk};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Region coordinates are chunk coordinates divided by 32, rounded down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RegionPos {
    pub x: i32,
    pub z: i32,
}

impl RegionPos {
    pub fn from_chunk(chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            x: chunk_x.div_euclid(32),
            z: chunk_z.div_euclid(32),
        }
    }

    pub fn local_chunk(self, chunk_x: i32, chunk_z: i32) -> (u8, u8) {
        (chunk_x.rem_euclid(32) as u8, chunk_z.rem_euclid(32) as u8)
    }

    pub fn file_name(self) -> String {
        format!("r.{}.{}.mca", self.x, self.z)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    pub fn region_dir(self, world_root: impl AsRef<Path>) -> PathBuf {
        let root = world_root.as_ref();
        match self {
            Dimension::Overworld => root.join("region"),
            Dimension::Nether => root
                .join("dimensions")
                .join("minecraft")
                .join("the_nether")
                .join("region"),
            Dimension::End => root
                .join("dimensions")
                .join("minecraft")
                .join("the_end")
                .join("region"),
        }
    }
}

/// Cached Anvil storage for one dimension.
///
/// Region files stay open in memory until `flush` or `unload_region`. This
/// mirrors the server-side region cache while keeping the Rust API explicit.
pub struct WorldStorage {
    region_dir: PathBuf,
    regions: HashMap<RegionPos, RegionFile>,
}

impl WorldStorage {
    pub fn new(region_dir: impl Into<PathBuf>) -> Self {
        Self {
            region_dir: region_dir.into(),
            regions: HashMap::new(),
        }
    }

    pub fn for_dimension(world_root: impl AsRef<Path>, dimension: Dimension) -> Self {
        Self::new(dimension.region_dir(world_root))
    }

    pub fn region_dir(&self) -> &Path {
        &self.region_dir
    }

    pub fn region_path(&self, position: RegionPos) -> PathBuf {
        self.region_dir.join(position.file_name())
    }

    pub fn loaded_regions(&self) -> usize {
        self.regions.len()
    }

    pub fn load_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<Option<Chunk>> {
        let position = RegionPos::from_chunk(chunk_x, chunk_z);
        let (local_x, local_z) = position.local_chunk(chunk_x, chunk_z);
        let region = self.region_mut(position)?;

        let Some(tag) = region.read_chunk(local_x, local_z)? else {
            return Ok(None);
        };
        let chunk = decode_chunk(&tag)?;
        if chunk.x != chunk_x || chunk.z != chunk_z {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "chunk coordinates mismatch: requested ({chunk_x},{chunk_z}), stored ({},{})",
                    chunk.x, chunk.z
                ),
            ));
        }
        Ok(Some(chunk))
    }

    pub fn save_chunk(&mut self, chunk: &Chunk, compression: Compression) -> io::Result<()> {
        let position = RegionPos::from_chunk(chunk.x, chunk.z);
        let (local_x, local_z) = position.local_chunk(chunk.x, chunk.z);
        let tag = encode_chunk(chunk)?;
        let region = self.region_mut(position)?;
        region.write_chunk(local_x, local_z, &tag, compression)
    }

    pub fn unload_region(&mut self, position: RegionPos) -> io::Result<()> {
        if let Some(mut region) = self.regions.remove(&position) {
            region.save_if_dirty()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        for region in self.regions.values_mut() {
            region.save_if_dirty()?;
        }
        Ok(())
    }

    fn region_mut(&mut self, position: RegionPos) -> io::Result<&mut RegionFile> {
        if !self.regions.contains_key(&position) {
            let path = self.region_path(position);
            let region = RegionFile::open_or_create(path)?;
            self.regions.insert(position, region);
        }
        Ok(self
            .regions
            .get_mut(&position)
            .expect("region was just inserted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockState, ChunkSection, PalettedContainer};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: vec![],
        };
        let mut indices = vec![1u16; 4096];
        indices[0] = 0;

        Chunk {
            x: chunk_x,
            z: chunk_z,
            sections: vec![ChunkSection {
                y: 4,
                block_states: PalettedContainer {
                    palette: vec![stone, air],
                    indices,
                },
                biomes: None,
            }],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-world-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn region_positions_and_file_names() {
        assert_eq!(RegionPos::from_chunk(0, 0), RegionPos { x: 0, z: 0 });
        assert_eq!(RegionPos::from_chunk(31, 31), RegionPos { x: 0, z: 0 });
        assert_eq!(RegionPos::from_chunk(32, 32), RegionPos { x: 1, z: 1 });
        assert_eq!(RegionPos::from_chunk(-1, -1), RegionPos { x: -1, z: -1 });
        assert_eq!(RegionPos::from_chunk(-1, -1).local_chunk(-1, -1), (31, 31));
        assert_eq!(RegionPos::from_chunk(32, 32).file_name(), "r.1.1.mca");
        assert_eq!(RegionPos::from_chunk(-33, 5).file_name(), "r.-2.0.mca");
    }

    #[test]
    fn dimension_region_paths_match_vanilla_layout() {
        let root = Path::new("world");
        assert_eq!(
            Dimension::Overworld.region_dir(root),
            Path::new("world/region")
        );
        assert_eq!(
            Dimension::Nether.region_dir(root),
            Path::new("world/dimensions/minecraft/the_nether/region")
        );
        assert_eq!(
            Dimension::End.region_dir(root),
            Path::new("world/dimensions/minecraft/the_end/region")
        );
    }

    #[test]
    fn saves_and_loads_chunks_across_regions() {
        let root = temp_dir("cross-region");
        let region_dir = Dimension::Overworld.region_dir(&root);
        let positions = [(0, 0), (31, 31), (32, 32), (-1, -1), (-33, 5)];

        {
            let mut storage = WorldStorage::new(&region_dir);
            for (chunk_x, chunk_z) in positions {
                storage
                    .save_chunk(&sample_chunk(chunk_x, chunk_z), Compression::Zlib)
                    .unwrap();
            }
            storage.flush().unwrap();
            assert_eq!(storage.loaded_regions(), 4);
        }

        {
            let mut storage = WorldStorage::new(&region_dir);
            for (chunk_x, chunk_z) in positions {
                let chunk = storage.load_chunk(chunk_x, chunk_z).unwrap().unwrap();
                assert_eq!(chunk.x, chunk_x);
                assert_eq!(chunk.z, chunk_z);
                assert_eq!(chunk.block_state(0, 64, 0).unwrap().name, "minecraft:stone");
                assert_eq!(chunk.block_state(1, 64, 0).unwrap().name, "minecraft:air");
            }
            assert!(storage.load_chunk(100, 100).unwrap().is_none());
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unload_region_flushes_dirty_data() {
        let root = temp_dir("unload");
        let region_dir = Dimension::Overworld.region_dir(&root);
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&sample_chunk(5, 5), Compression::Gzip)
            .unwrap();
        assert_eq!(storage.loaded_regions(), 1);
        storage.unload_region(RegionPos::from_chunk(5, 5)).unwrap();
        assert_eq!(storage.loaded_regions(), 0);

        let mut reopened = WorldStorage::new(&region_dir);
        assert!(reopened.load_chunk(5, 5).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
