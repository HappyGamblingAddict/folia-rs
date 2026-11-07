use crate::anvil::{Compression, RegionFile};
use crate::block_entity::{
    BlockEntity, BlockEntityMap, BlockEntityPos, BlockEntityTicker, decode_block_entities,
    store_block_entities,
};
use crate::chunk::{BlockState, Chunk, decode_chunk, encode_chunk};
use crate::nbt::Tag;
use crate::tick::TickState;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// NBT key stamped on every chunk this server writes.
///
/// It separates terrain this server produced (and therefore trusts) from
/// region files left behind by an earlier flat-world run that has no
/// `level.dat`; vanilla ignores unknown chunk fields.
pub const WRITTEN_MARKER: &str = "folia:written";

/// How many decoded chunks one storage keeps before evicting the oldest.
const CHUNK_CACHE_LIMIT: usize = 256;

static SHARED_OVERWORLD: OnceLock<Mutex<WorldStorage>> = OnceLock::new();

/// The process-wide overworld storage.
///
/// A region file is rewritten as a whole file when it is saved, so two
/// independent `WorldStorage` values over the same directory silently drop each
/// other's chunk writes. Every subsystem that touches overworld terrain (the
/// join streamer, block edits, entity physics) must go through this one
/// instance. The first directory wins, which matches a server that serves a
/// single level.
pub fn shared_overworld(world_root: impl Into<PathBuf>) -> &'static Mutex<WorldStorage> {
    SHARED_OVERWORLD.get_or_init(|| {
        Mutex::new(WorldStorage::for_dimension(
            world_root.into(),
            Dimension::Overworld,
        ))
    })
}

/// Lock the shared storage, recovering from a poisoned mutex.
pub fn lock(storage: &Mutex<WorldStorage>) -> MutexGuard<'_, WorldStorage> {
    storage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Flush the process-wide overworld storage if a session ever opened it.
///
/// Terrain saved during a session stays in the region cache until a flush;
/// shutdown calls this so an orderly stop does not leave cached chunks behind.
pub fn flush_shared() -> io::Result<()> {
    let Some(storage) = SHARED_OVERWORLD.get() else {
        return Ok(());
    };
    lock(storage).flush()
}

/// Cached Anvil storage for one dimension.
///
/// Region files stay open in memory until `flush` or `unload_region`. This
/// mirrors the server-side region cache while keeping the Rust API explicit.
pub struct WorldStorage {
    region_dir: PathBuf,
    regions: HashMap<RegionPos, RegionFile>,
    /// Decoded chunks, so a second reader does not re-decompress and re-decode.
    chunks: HashMap<(i32, i32), Chunk>,
    /// Block entities belonging to cached/loaded chunks, keyed by chunk.
    block_entities: HashMap<(i32, i32), BlockEntityMap>,
    cache_order: VecDeque<(i32, i32)>,
    cache_hits: u64,
    cache_misses: u64,
    /// Neighbour/scheduled/random tick queues for this world.
    tick: TickState,
}

impl WorldStorage {
    pub fn new(region_dir: impl Into<PathBuf>) -> Self {
        Self {
            region_dir: region_dir.into(),
            regions: HashMap::new(),
            chunks: HashMap::new(),
            block_entities: HashMap::new(),
            cache_order: VecDeque::new(),
            cache_hits: 0,
            cache_misses: 0,
            tick: TickState::default(),
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

    /// Read-only view of this world's tick queues and diagnostics.
    pub fn tick_state(&self) -> &TickState {
        &self.tick
    }

    /// Mutable view of this world's tick queues and diagnostics.
    pub fn tick_state_mut(&mut self) -> &mut TickState {
        &mut self.tick
    }

    /// Sets the seed used to derive per-chunk random-tick sampling.
    pub fn set_tick_seed(&mut self, seed: i64) {
        self.tick.random.seed = seed;
    }

    /// Chunk coordinates currently held in the decoded-chunk cache, sorted so a
    /// tick pass has a deterministic order.
    pub fn loaded_chunk_coords(&self) -> Vec<(i32, i32)> {
        let mut coords: Vec<(i32, i32)> = self.chunks.keys().copied().collect();
        coords.sort_unstable();
        coords
    }

    /// Section Y coordinates of a cached chunk, sorted for deterministic random
    /// tick sampling. Returns `None` when the chunk is not cached.
    pub fn cached_section_ys(&self, chunk_x: i32, chunk_z: i32) -> Option<Vec<i8>> {
        let chunk = self.chunks.get(&(chunk_x, chunk_z))?;
        let mut ys: Vec<i8> = chunk.sections.iter().map(|section| section.y).collect();
        ys.sort_unstable();
        Some(ys)
    }

    /// Reads a block state from the decoded-chunk cache without loading a
    /// region record. Returns `None` when the chunk is not cached.
    pub fn block_state_at(&self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        chunk
            .block_state(x.rem_euclid(16) as usize, y, z.rem_euclid(16) as usize)
            .cloned()
    }

    /// Sky-light level at a block position when its chunk and light layer are
    /// cached. Returns `None` for an unloaded chunk or a section without light.
    pub fn sky_light_at(&self, x: i32, y: i32, z: i32) -> Option<u8> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let section = chunk.section(y.div_euclid(16) as i8)?;
        let layer = section.light.sky.as_ref()?;
        Some(layer.get(
            x.rem_euclid(16) as usize,
            y.rem_euclid(16) as usize,
            z.rem_euclid(16) as usize,
        ))
    }

    /// All block entities of a chunk. Returns `None` when the chunk does not
    /// exist (neither cached nor stored). An existing chunk with no block
    /// entities returns `Some` with an empty map.
    pub fn block_entities(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
    ) -> io::Result<Option<&BlockEntityMap>> {
        if !self.ensure_block_entities(chunk_x, chunk_z)? {
            return Ok(None);
        }
        Ok(self.block_entities.get(&(chunk_x, chunk_z)))
    }

    /// Owned copy of a chunk's block entities, loading them from disk when the
    /// chunk is not cached yet.
    pub fn load_block_entities(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
    ) -> io::Result<Option<BlockEntityMap>> {
        if !self.ensure_block_entities(chunk_x, chunk_z)? {
            return Ok(None);
        }
        Ok(self.block_entities.get(&(chunk_x, chunk_z)).cloned())
    }

    /// Look up one block entity by its absolute position.
    pub fn block_entity(&mut self, position: BlockEntityPos) -> io::Result<Option<&BlockEntity>> {
        if !self.ensure_block_entities(position.chunk_x(), position.chunk_z())? {
            return Ok(None);
        }
        Ok(self
            .block_entities
            .get(&(position.chunk_x(), position.chunk_z()))
            .and_then(|entities| entities.get(&position)))
    }

    /// Alias for [`block_entity`](Self::block_entity), matching the
    /// get/insert/remove naming used by container callers.
    pub fn get_block_entity(
        &mut self,
        position: BlockEntityPos,
    ) -> io::Result<Option<&BlockEntity>> {
        self.block_entity(position)
    }

    /// Insert or replace a block entity, returning the previous value at the
    /// same position.
    pub fn insert_block_entity(&mut self, entity: BlockEntity) -> io::Result<Option<BlockEntity>> {
        let position = entity.pos();
        let key = (position.chunk_x(), position.chunk_z());
        self.ensure_block_entities(key.0, key.1)?;
        Ok(self
            .block_entities
            .entry(key)
            .or_default()
            .insert(position, entity))
    }

    /// Remove one block entity by position. Returns the removed value.
    pub fn remove_block_entity(
        &mut self,
        position: BlockEntityPos,
    ) -> io::Result<Option<BlockEntity>> {
        if !self.ensure_block_entities(position.chunk_x(), position.chunk_z())? {
            return Ok(None);
        }
        Ok(self
            .block_entities
            .get_mut(&(position.chunk_x(), position.chunk_z()))
            .and_then(|entities| entities.remove(&position)))
    }

    /// Tick every cached block entity through an explicit ticker. The server
    /// tick loop does not call this yet; callers opt in.
    pub fn tick_block_entities(&mut self, ticker: &mut dyn BlockEntityTicker) {
        for entities in self.block_entities.values_mut() {
            for entity in entities.values_mut() {
                ticker.tick(entity);
            }
        }
    }

    fn ensure_block_entities(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<bool> {
        let key = (chunk_x, chunk_z);
        if self.block_entities.contains_key(&key) {
            return Ok(true);
        }
        if let Some(entities) = self.read_stored_block_entities(chunk_x, chunk_z)? {
            self.block_entities.insert(key, entities);
            return Ok(true);
        }
        if self.chunks.contains_key(&key) {
            self.block_entities.insert(key, BlockEntityMap::new());
            return Ok(true);
        }
        Ok(false)
    }

    fn read_stored_block_entities(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
    ) -> io::Result<Option<BlockEntityMap>> {
        let position = RegionPos::from_chunk(chunk_x, chunk_z);
        let (local_x, local_z) = position.local_chunk(chunk_x, chunk_z);
        let region = self.region_mut(position)?;
        match read_stored_chunk(region, local_x, local_z, chunk_x, chunk_z)? {
            Some(tag) => Ok(Some(decode_block_entities(&tag)?)),
            None => Ok(None),
        }
    }

    pub fn load_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<Option<Chunk>> {
        if let Some(cached) = self.chunks.get(&(chunk_x, chunk_z)) {
            self.cache_hits += 1;
            return Ok(Some(cached.clone()));
        }
        self.cache_misses += 1;
        let position = RegionPos::from_chunk(chunk_x, chunk_z);
        let (local_x, local_z) = position.local_chunk(chunk_x, chunk_z);
        let region = self.region_mut(position)?;

        let Some(tag) = read_stored_chunk(region, local_x, local_z, chunk_x, chunk_z)? else {
            return Ok(None);
        };
        let chunk = decode_chunk(&tag)?;
        let entities = decode_block_entities(&tag)?;
        if chunk.x != chunk_x || chunk.z != chunk_z {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "chunk coordinates mismatch: requested ({chunk_x},{chunk_z}), stored ({},{})",
                    chunk.x, chunk.z
                ),
            ));
        }
        self.cache_chunk(chunk.clone());
        self.block_entities
            .entry((chunk_x, chunk_z))
            .or_insert(entities);
        Ok(Some(chunk))
    }

    pub fn save_chunk(&mut self, chunk: &Chunk, compression: Compression) -> io::Result<()> {
        // A generated column replaces the stored one, so it must not inherit
        // block entities from whatever was there before.
        self.write_chunk_with_entities(chunk, compression, &BlockEntityMap::new())
    }

    /// Save a chunk together with the block-entity map currently cached for
    /// it.  Use this after [`insert_block_entity`](Self::insert_block_entity)
    /// or [`remove_block_entity`](Self::remove_block_entity); ordinary
    /// generated terrain should use [`save_chunk`](Self::save_chunk), which
    /// deliberately writes an empty map.
    pub fn save_chunk_with_block_entities(
        &mut self,
        chunk: &Chunk,
        compression: Compression,
    ) -> io::Result<()> {
        let entities = self
            .block_entities
            .get(&(chunk.x, chunk.z))
            .cloned()
            .unwrap_or_default();
        self.write_chunk_with_entities(chunk, compression, &entities)
    }

    fn write_chunk_with_entities(
        &mut self,
        chunk: &Chunk,
        compression: Compression,
        entities: &BlockEntityMap,
    ) -> io::Result<()> {
        let position = RegionPos::from_chunk(chunk.x, chunk.z);
        let (local_x, local_z) = position.local_chunk(chunk.x, chunk.z);
        let mut tag = encode_chunk(chunk)?;
        store_block_entities(&mut tag, entities)?;
        let tag = stamp_chunk(tag, chunk);
        let region = self.region_mut(position)?;
        region.write_chunk(local_x, local_z, &tag, compression)?;
        self.cache_chunk(chunk.clone());
        self.block_entities
            .insert((chunk.x, chunk.z), entities.clone());
        Ok(())
    }

    /// Save a chunk that was loaded from this level, keeping the top-level
    /// fields this port does not decode (status, heightmaps, entities,
    /// structures, post-processing, ...). Block entities are handled by the
    /// position-keyed map rather than copied from the stored root.
    ///
    /// Generated terrain goes through [`save_chunk`](Self::save_chunk) instead:
    /// it replaces the stored column, so merging fields such as block entities
    /// from whatever was there before would attach stale data to new terrain.
    ///
    /// `edited_blocks` are the world positions whose block changed in this
    /// save; the block entity at each of those positions is removed, because
    /// vanilla removes a block entity when its block is replaced. Positions
    /// this port did not edit keep their stored block entity.
    pub fn update_chunk(
        &mut self,
        chunk: &Chunk,
        compression: Compression,
        edited_blocks: &[(i32, i32, i32)],
    ) -> io::Result<()> {
        let position = RegionPos::from_chunk(chunk.x, chunk.z);
        let (local_x, local_z) = position.local_chunk(chunk.x, chunk.z);
        let existing = {
            let region = self.region_mut(position)?;
            read_stored_chunk(region, local_x, local_z, chunk.x, chunk.z)?
        };
        let key = (chunk.x, chunk.z);
        let mut entities = match self.block_entities.get(&key) {
            Some(entities) => entities.clone(),
            None => match &existing {
                Some(existing) => decode_block_entities(existing)?,
                None => BlockEntityMap::new(),
            },
        };
        for &(x, y, z) in edited_blocks {
            entities.remove(&BlockEntityPos::new(x, y, z));
        }
        let mut tag = encode_chunk(chunk)?;
        if let Some(existing) = &existing {
            preserve_unknown_fields(existing, &mut tag);
        }
        store_block_entities(&mut tag, &entities)?;
        let tag = stamp_chunk(tag, chunk);
        let region = self.region_mut(position)?;
        region.write_chunk(local_x, local_z, &tag, compression)?;
        self.cache_chunk(chunk.clone());
        self.block_entities.insert(key, entities);
        Ok(())
    }

    /// True when the chunk on disk carries [`WRITTEN_MARKER`], meaning this
    /// server produced it rather than an earlier flat placeholder run.
    pub fn is_written_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<bool> {
        let position = RegionPos::from_chunk(chunk_x, chunk_z);
        let (local_x, local_z) = position.local_chunk(chunk_x, chunk_z);
        let region = self.region_mut(position)?;
        Ok(
            read_stored_chunk(region, local_x, local_z, chunk_x, chunk_z)?
                .is_some_and(|tag| tag.get(WRITTEN_MARKER).is_some()),
        )
    }

    pub fn unload_region(&mut self, position: RegionPos) -> io::Result<()> {
        if let Some(mut region) = self.regions.remove(&position) {
            region.save_if_dirty()?;
        }
        self.chunks
            .retain(|(chunk_x, chunk_z), _| RegionPos::from_chunk(*chunk_x, *chunk_z) != position);
        self.block_entities
            .retain(|(chunk_x, chunk_z), _| RegionPos::from_chunk(*chunk_x, *chunk_z) != position);
        self.cache_order
            .retain(|(chunk_x, chunk_z)| RegionPos::from_chunk(*chunk_x, *chunk_z) != position);
        Ok(())
    }

    fn cache_chunk(&mut self, chunk: Chunk) {
        let key = (chunk.x, chunk.z);
        if self.chunks.insert(key, chunk).is_none() {
            self.cache_order.push_back(key);
        }
        while self.chunks.len() > CHUNK_CACHE_LIMIT {
            let Some(oldest) = self.cache_order.pop_front() else {
                break;
            };
            self.chunks.remove(&oldest);
        }
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

/// Read a stored chunk, treating a damaged record as a missing chunk so one
/// corrupt column cannot stop a join or block a later save. Only invalid
/// record data is swallowed; filesystem errors still reach the caller.
fn read_stored_chunk(
    region: &RegionFile,
    local_x: u8,
    local_z: u8,
    chunk_x: i32,
    chunk_z: i32,
) -> io::Result<Option<Tag>> {
    match region.read_chunk(local_x, local_z) {
        Ok(tag) => Ok(tag),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::InvalidInput
            ) =>
        {
            eprintln!(
                "[world] ignoring corrupt chunk ({chunk_x},{chunk_z}) in {}: {error}",
                region.path().display()
            );
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn stamp_chunk(mut tag: Tag, chunk: &Chunk) -> Tag {
    if let Tag::Compound(entries) = &mut tag {
        entries.retain(|(key, _)| key != WRITTEN_MARKER && key != "Heightmaps");
        entries.push((
            "Heightmaps".into(),
            crate::chunk_packet::heightmap_nbt(chunk),
        ));
        entries.push((WRITTEN_MARKER.into(), Tag::Byte(1)));
    }
    tag
}

/// Copy top-level fields the chunk model does not decode from `existing` into
/// `target`. Fields this server writes win, so a stale position or section list
/// can never replace the chunk being saved.
fn preserve_unknown_fields(existing: &Tag, target: &mut Tag) {
    let (Tag::Compound(source), Tag::Compound(destination)) = (existing, target) else {
        return;
    };
    for (key, value) in source {
        if key == "xPos"
            || key == "zPos"
            || key == "sections"
            || key == WRITTEN_MARKER
            || key == crate::block_entity::BLOCK_ENTITIES_KEY
            || key == crate::block_entity::LEGACY_BLOCK_ENTITIES_KEY
        {
            continue;
        }
        if destination.iter().any(|(written, _)| written == key) {
            continue;
        }
        destination.push((key.clone(), value.clone()));
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
                light: crate::light::SectionLight::default(),
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
    fn the_shared_overworld_storage_is_a_single_instance() {
        let root = temp_dir("shared");
        let first = shared_overworld(&root);
        // A second caller (another session, or entity physics) must get the same
        // storage: region saves rewrite whole files, so separate instances would
        // drop each other's chunks.
        let second = shared_overworld(root.join("ignored-later-root"));
        assert!(std::ptr::eq(first, second));
        assert_eq!(
            lock(first).region_dir(),
            Dimension::Overworld.region_dir(&root)
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
    fn updating_a_loaded_chunk_keeps_fields_the_model_does_not_decode() {
        let root = temp_dir("preserve-fields");
        let region_dir = Dimension::Overworld.region_dir(&root);
        let position = RegionPos::from_chunk(0, 0);
        let (local_x, local_z) = position.local_chunk(0, 0);

        let mut stored = encode_chunk(&sample_chunk(0, 0)).unwrap();
        if let Tag::Compound(entries) = &mut stored {
            entries.push(("Status".into(), Tag::String("full".into())));
            entries.push((
                "BlockEntities".into(),
                Tag::List(vec![
                    Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:chest".into())),
                        ("x".into(), Tag::Int(0)),
                        ("y".into(), Tag::Int(64)),
                        ("z".into(), Tag::Int(0)),
                    ]),
                    Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:furnace".into())),
                        ("x".into(), Tag::Int(5)),
                        ("y".into(), Tag::Int(64)),
                        ("z".into(), Tag::Int(5)),
                    ]),
                ]),
            ));
            entries.push((
                "Heightmaps".into(),
                Tag::Compound(vec![(
                    "MOTION_BLOCKING".into(),
                    Tag::LongArray(vec![7; 37]),
                )]),
            ));
            entries.push((WRITTEN_MARKER.into(), Tag::Byte(1)));
        }

        {
            let mut storage = WorldStorage::new(&region_dir);
            storage
                .region_mut(position)
                .unwrap()
                .write_chunk(local_x, local_z, &stored, Compression::Zlib)
                .unwrap();
            storage.flush().unwrap();
        }

        {
            let mut storage = WorldStorage::new(&region_dir);
            let mut chunk = storage.load_chunk(0, 0).unwrap().unwrap();
            assert!(chunk.set_block_state(
                0,
                64,
                0,
                BlockState {
                    name: "minecraft:dirt".into(),
                    properties: vec![],
                }
            ));
            storage
                .update_chunk(&chunk, Compression::Zlib, &[(0, 64, 0)])
                .unwrap();
            storage.flush().unwrap();
        }

        let mut reopened = WorldStorage::new(&region_dir);
        let tag = reopened
            .region_mut(position)
            .unwrap()
            .read_chunk(local_x, local_z)
            .unwrap()
            .unwrap();
        assert!(
            matches!(tag.get("Status"), Some(Tag::String(status)) if status == "minecraft:full"),
            "an edited chunk is saved with full status"
        );
        let block_entities = tag
            .get(crate::block_entity::BLOCK_ENTITIES_KEY)
            .and_then(Tag::as_list)
            .expect("block entities are written under the modern chunk key");
        assert_eq!(
            block_entities.len(),
            1,
            "the edited position loses its block entity"
        );
        assert!(
            tag.get(crate::block_entity::LEGACY_BLOCK_ENTITIES_KEY)
                .is_none(),
            "the legacy key is removed after a rewrite"
        );
        let remaining = block_entities[0].as_compound().unwrap();
        assert_eq!(
            remaining
                .iter()
                .find(|(key, _)| key == "x")
                .and_then(|(_, value)| value.as_int()),
            Some(5),
            "an untouched block entity survives"
        );
        assert!(tag.get("Heightmaps").is_some());
        assert_eq!(tag.get(WRITTEN_MARKER), Some(&Tag::Byte(1)));
        let chunk = reopened.load_chunk(0, 0).unwrap().unwrap();
        assert_eq!(chunk.block_state(0, 64, 0).unwrap().name, "minecraft:dirt");

        // A generated column replaces the stored one and must not inherit its
        // block entities or heightmaps.
        reopened
            .save_chunk(&sample_chunk(0, 0), Compression::Zlib)
            .unwrap();
        reopened.flush().unwrap();
        let replaced = reopened
            .region_mut(position)
            .unwrap()
            .read_chunk(local_x, local_z)
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                replaced.get("Status"),
                Some(Tag::String(status)) if status == "minecraft:full"
            ),
            "a generated column carries full status"
        );
        assert_eq!(
            replaced.get("DataVersion").and_then(Tag::as_int),
            Some(crate::level::DATA_VERSION)
        );
        assert!(replaced.get("BlockEntities").is_none());
        assert_eq!(
            replaced
                .get(crate::block_entity::BLOCK_ENTITIES_KEY)
                .and_then(Tag::as_list)
                .map(<[Tag]>::len),
            Some(0),
            "a generated column replaces the stored block entities"
        );
        let heightmaps = replaced
            .get("Heightmaps")
            .and_then(Tag::as_compound)
            .expect("generated chunk NBT has heightmaps");
        let motion = heightmaps
            .iter()
            .find(|(key, _)| key == "MOTION_BLOCKING")
            .and_then(|(_, value)| value.as_long_array())
            .expect("MOTION_BLOCKING is a long array");
        assert_eq!(motion.len(), 37, "256 entries at 9 bits pack into 37 longs");
        assert_eq!(
            motion[0] as u64 & 0x1FF,
            129,
            "column (0,0) is y=65, stored 129 above the world minimum"
        );

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

    #[test]
    fn a_corrupt_chunk_reads_as_missing_and_its_neighbour_still_loads() {
        let root = temp_dir("corrupt-chunk");
        let region_dir = Dimension::Overworld.region_dir(&root);
        let position = RegionPos::from_chunk(0, 0);
        let (bad_x, bad_z) = position.local_chunk(0, 0);

        {
            let mut storage = WorldStorage::new(&region_dir);
            storage
                .save_chunk(&sample_chunk(0, 0), Compression::Zlib)
                .unwrap();
            storage
                .save_chunk(&sample_chunk(1, 0), Compression::Zlib)
                .unwrap();
            storage.flush().unwrap();
        }

        // Break the compression id of the first record while leaving its
        // header entry valid: the reader sees damage, not an empty slot.
        let storage = WorldStorage::new(&region_dir);
        let path = storage.region_path(position);
        let mut bytes = fs::read(&path).unwrap();
        let base = (u32::from(bad_z) * 32 + u32::from(bad_x)) as usize * 4;
        let offset = (u32::from(bytes[base]) << 16)
            | (u32::from(bytes[base + 1]) << 8)
            | u32::from(bytes[base + 2]);
        let start = offset as usize * 4096;
        bytes[start + 4] = 0xFF;
        fs::write(&path, bytes).unwrap();

        let mut storage = WorldStorage::new(&region_dir);
        assert!(
            storage.load_chunk(0, 0).unwrap().is_none(),
            "a corrupt record is treated as a missing chunk"
        );
        let neighbour = storage
            .load_chunk(1, 0)
            .unwrap()
            .expect("the intact chunk still loads");
        assert_eq!(neighbour.x, 1);
        assert_eq!(
            neighbour.block_state(0, 64, 0).unwrap().name,
            "minecraft:stone"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decoded_chunks_are_cached_until_they_change_or_unload() {
        let root = temp_dir("chunk-cache");
        let region_dir = Dimension::Overworld.region_dir(&root);
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&sample_chunk(3, 4), Compression::Zlib)
            .unwrap();
        assert_eq!(storage.chunks.len(), 1);

        let first = storage.load_chunk(3, 4).unwrap().unwrap();
        let second = storage.load_chunk(3, 4).unwrap().unwrap();
        assert_eq!(storage.cache_hits, 2, "both loads come from the cache");
        assert_eq!(
            first.block_state(0, 64, 0).unwrap().name,
            second.block_state(0, 64, 0).unwrap().name
        );

        let mut edited = first;
        assert!(edited.set_block_state(
            0,
            64,
            0,
            BlockState {
                name: "minecraft:dirt".into(),
                properties: vec![],
            }
        ));
        storage
            .update_chunk(&edited, Compression::Zlib, &[(0, 64, 0)])
            .unwrap();
        assert_eq!(
            storage
                .load_chunk(3, 4)
                .unwrap()
                .unwrap()
                .block_state(0, 64, 0)
                .unwrap()
                .name,
            "minecraft:dirt",
            "an edit replaces the cached copy"
        );

        storage.unload_region(RegionPos::from_chunk(3, 4)).unwrap();
        assert!(storage.chunks.is_empty(), "unloading drops cached chunks");
        fs::remove_dir_all(root).unwrap();
    }
}
