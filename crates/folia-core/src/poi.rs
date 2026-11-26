//! Per-chunk points of interest (POI) storage and persistence.
//!
//! A POI is one block position tagged with a POI type (a bed, a job site, a
//! beehive, a meeting point, …) plus an optional number of free tickets. This
//! module models the data a spawner needs to reason about village/bee behaviour
//! and keeps it in a deterministic, self-contained file format.
//!
//! # Format difference from vanilla
//!
//! Vanilla stores POIs in `poi/r.X.Z.mca` using the Anvil region layout with
//! per-section, compressed, length-prefixed records and numeric POI-type
//! registry ids. This port deliberately uses a different local format:
//! `poi/r.X.Z.poi`, a plain big-endian binary stream introduced by a `FPOI`
//! magic and a version byte. It is byte-stable and easy to validate, but it is
//! **not** vanilla Anvil POI byte-compatible: a vanilla server will ignore
//! these files and this server ignores vanilla `.mca` POI files. POI types are
//! stored as resource-location strings rather than registry numbers, so an
//! unknown/modded type survives a save/reload without a type-registry lookup.

use crate::world::RegionPos;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Sub-directory of the world root that holds per-region POI files.
pub const POI_DIR: &str = "poi";
/// Extension of a local POI region file (`r.X.Z.poi`).
pub const POI_EXTENSION: &str = "poi";
/// Magic prefix of a local POI region file.
pub const POI_MAGIC: [u8; 4] = *b"FPOI";
/// Current local POI file version.
pub const POI_FORMAT_VERSION: u8 = 1;

/// Upper bound on a POI type string, so a corrupt length cannot allocate
/// unbounded memory while decoding.
const MAX_KIND_LENGTH: usize = 1024;
/// A region holds at most 32×32 chunks.
const MAX_CHUNKS_PER_REGION: usize = 32 * 32;

/// Block position of one POI, in world coordinates (`x`, `y`, `z`).
pub type PoiPos = [i32; 3];
/// Records of one chunk, keyed by block position so iteration is deterministic.
pub type PoiChunk = BTreeMap<PoiPos, PoiRecord>;
/// All POIs of one region, keyed by chunk coordinates.
pub type PoiRegion = BTreeMap<(i32, i32), PoiChunk>;

/// One point of interest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PoiRecord {
    /// Absolute block position of the POI.
    pub position: PoiPos,
    /// POI type as a resource location, e.g. `minecraft:home`. Kept as a string
    /// so types unknown to this build are never dropped.
    pub kind: String,
    /// Number of free tickets, when known. `None` denotes an unspecified count
    /// (for example a record reconstructed without vanilla ticket state).
    pub free_tickets: Option<u32>,
}

impl PoiRecord {
    /// A record with no ticket information.
    pub fn new(position: PoiPos, kind: impl Into<String>) -> Self {
        Self {
            position,
            kind: kind.into(),
            free_tickets: None,
        }
    }

    /// Attach a free-ticket count.
    pub fn with_tickets(mut self, free_tickets: u32) -> Self {
        self.free_tickets = Some(free_tickets);
        self
    }

    /// Chunk coordinates that contain this record's block position.
    pub fn chunk(&self) -> (i32, i32) {
        (
            self.position[0].div_euclid(16),
            self.position[2].div_euclid(16),
        )
    }
}

/// Derive the POI directory from a world's region directory. Both live under
/// the same world root, so `<world>/region` maps to `<world>/poi`.
pub fn poi_dir_from_region_dir(region_dir: &Path) -> PathBuf {
    region_dir
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(POI_DIR)
}

/// In-memory POI store, keyed by chunk, with per-region persistence.
///
/// A region is loaded once on first access; edits mark that region dirty and a
/// later [`flush`](Self::flush) or [`unload_region`](Self::unload_region)
/// rewrites its file exactly once. All collection types are ordered, so file
/// bytes depend only on the records, never on insertion order.
#[derive(Debug)]
pub struct PoiStorage {
    dir: PathBuf,
    chunks: PoiRegion,
    loaded_regions: BTreeSet<RegionPos>,
    dirty_regions: BTreeSet<RegionPos>,
    writes: u64,
}

impl PoiStorage {
    /// Create a store rooted at `dir`. No I/O happens until a region is read or
    /// flushed; a missing directory is created on the first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            chunks: BTreeMap::new(),
            loaded_regions: BTreeSet::new(),
            dirty_regions: BTreeSet::new(),
            writes: 0,
        }
    }

    /// Directory holding this store's region files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of one region's POI file.
    pub fn path(&self, region: RegionPos) -> PathBuf {
        self.dir
            .join(format!("r.{}.{}.{}", region.x, region.z, POI_EXTENSION))
    }

    /// Number of POI region files written by [`flush`](Self::flush) and
    /// [`unload_region`](Self::unload_region) so far.
    pub fn write_count(&self) -> u64 {
        self.writes
    }

    /// Regions with edits not yet written to disk, in flush order.
    pub fn dirty_regions(&self) -> Vec<RegionPos> {
        self.dirty_regions.iter().copied().collect()
    }

    /// True when `region` has unflushed edits.
    pub fn is_region_dirty(&self, region: RegionPos) -> bool {
        self.dirty_regions.contains(&region)
    }

    /// Regions whose files have been read (or created empty) in memory.
    pub fn loaded_regions(&self) -> Vec<RegionPos> {
        self.loaded_regions.iter().copied().collect()
    }

    /// Load a region's records from disk on first access.
    ///
    /// A missing file means an empty region. A corrupt file is logged and
    /// treated as empty, so one damaged POI file cannot stop a chunk load; it is
    /// only replaced on disk once a caller makes a real edit and flushes.
    /// Filesystem errors other than "not found" reach the caller.
    pub fn load_region(&mut self, region: RegionPos) -> io::Result<()> {
        if self.loaded_regions.contains(&region) {
            return Ok(());
        }
        let path = self.path(region);
        let bytes = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(bytes) = bytes {
            match decode_poi_file(&bytes, region) {
                Ok(records) => {
                    for (chunk, entries) in records {
                        self.chunks.entry(chunk).or_default().extend(entries);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::InvalidData
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::InvalidInput
                    ) =>
                {
                    eprintln!(
                        "[poi] ignoring corrupt POI file {}: {error}",
                        path.display()
                    );
                }
                Err(error) => return Err(error),
            }
        }
        self.loaded_regions.insert(region);
        Ok(())
    }

    /// One record at a block position in a chunk, loading its region if needed.
    pub fn at(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        position: PoiPos,
    ) -> io::Result<Option<PoiRecord>> {
        self.load_region(RegionPos::from_chunk(chunk_x, chunk_z))?;
        Ok(self
            .chunks
            .get(&(chunk_x, chunk_z))
            .and_then(|entries| entries.get(&position))
            .cloned())
    }

    /// Every record of a chunk in deterministic position order.
    pub fn chunk_records(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<Vec<PoiRecord>> {
        self.load_region(RegionPos::from_chunk(chunk_x, chunk_z))?;
        Ok(self
            .chunks
            .get(&(chunk_x, chunk_z))
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default())
    }

    /// Every record of a region in deterministic (chunk, position) order.
    pub fn region_records(&mut self, region: RegionPos) -> io::Result<Vec<PoiRecord>> {
        self.load_region(region)?;
        Ok(self
            .chunks
            .iter()
            .filter(|((x, z), _)| RegionPos::from_chunk(*x, *z) == region)
            .flat_map(|(_, entries)| entries.values().cloned())
            .collect())
    }

    /// Insert or replace a record by block position, returning the old value.
    /// The record's position must lie in the supplied chunk.
    pub fn insert(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        record: PoiRecord,
    ) -> io::Result<Option<PoiRecord>> {
        if record.kind.is_empty() || record.kind.len() > MAX_KIND_LENGTH {
            return Err(invalid(format!(
                "POI type must be 1..={MAX_KIND_LENGTH} bytes, got {}",
                record.kind.len()
            )));
        }
        if record.chunk() != (chunk_x, chunk_z) {
            return Err(invalid("POI position is outside the target chunk"));
        }
        let region = RegionPos::from_chunk(chunk_x, chunk_z);
        self.load_region(region)?;
        let previous = self
            .chunks
            .entry((chunk_x, chunk_z))
            .or_default()
            .insert(record.position, record);
        self.dirty_regions.insert(region);
        Ok(previous)
    }

    /// Remove the record at a block position, returning it when present.
    pub fn remove(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        position: PoiPos,
    ) -> io::Result<Option<PoiRecord>> {
        let region = RegionPos::from_chunk(chunk_x, chunk_z);
        self.load_region(region)?;
        let removed = self
            .chunks
            .get_mut(&(chunk_x, chunk_z))
            .and_then(|entries| entries.remove(&position));
        if removed.is_some() {
            self.dirty_regions.insert(region);
        }
        Ok(removed)
    }

    /// Drop every record of a chunk, returning how many were removed.
    pub fn clear_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> io::Result<usize> {
        let region = RegionPos::from_chunk(chunk_x, chunk_z);
        self.load_region(region)?;
        match self.chunks.remove(&(chunk_x, chunk_z)) {
            Some(entries) => {
                self.dirty_regions.insert(region);
                Ok(entries.len())
            }
            None => Ok(0),
        }
    }

    /// Write every dirty region file once, in region-coordinate order.
    /// Returns the number of files written. A failed write keeps that region
    /// (and every later one) dirty for the next flush.
    pub fn flush(&mut self) -> io::Result<usize> {
        let dirty = std::mem::take(&mut self.dirty_regions);
        let mut regions = dirty.into_iter();
        let mut written = 0;
        let mut result = Ok(());
        for region in regions.by_ref() {
            let bytes = encode_poi_file(region, &self.region(region));
            match write_poi_file(&self.path(region), &bytes) {
                Ok(()) => {
                    self.writes += 1;
                    written += 1;
                }
                Err(error) => {
                    self.dirty_regions.insert(region);
                    result = Err(error);
                    break;
                }
            }
        }
        for region in regions {
            self.dirty_regions.insert(region);
        }
        result?;
        Ok(written)
    }

    /// Write a region if dirty, then forget its in-memory records so a later
    /// access reloads from disk. Used when a region's chunks are unloaded.
    pub fn unload_region(&mut self, region: RegionPos) -> io::Result<()> {
        if self.dirty_regions.contains(&region) {
            let bytes = encode_poi_file(region, &self.region(region));
            write_poi_file(&self.path(region), &bytes)?;
            self.writes += 1;
        }
        self.chunks
            .retain(|(x, z), _| RegionPos::from_chunk(*x, *z) != region);
        self.loaded_regions.remove(&region);
        self.dirty_regions.remove(&region);
        Ok(())
    }

    fn region(&self, region: RegionPos) -> PoiRegion {
        self.chunks
            .iter()
            .filter(|((x, z), _)| RegionPos::from_chunk(*x, *z) == region)
            .map(|(&chunk, entries)| (chunk, entries.clone()))
            .collect()
    }
}

/// Encode a region's records into the deterministic local binary format.
///
/// Layout (all integers big-endian):
/// `magic[4]`, `version:u8`, `chunk_count:u32`, then for each chunk in
/// increasing `(x, z)` order: `x:i32`, `z:i32`, `record_count:u32`, then for
/// each record in increasing `(x, y, z)` order: `x:i32`, `y:i32`, `z:i32`,
/// `kind_len:u16`, `kind:bytes`, `tickets_present:u8`, and `tickets:u32` when
/// present.
pub fn encode_poi_file(region: RegionPos, chunks: &PoiRegion) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&POI_MAGIC);
    out.push(POI_FORMAT_VERSION);
    let relevant: Vec<_> = chunks
        .iter()
        .filter(|((x, z), entries)| RegionPos::from_chunk(*x, *z) == region && !entries.is_empty())
        .collect();
    out.extend_from_slice(&(relevant.len() as u32).to_be_bytes());
    for (&(chunk_x, chunk_z), entries) in relevant {
        out.extend_from_slice(&chunk_x.to_be_bytes());
        out.extend_from_slice(&chunk_z.to_be_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (position, record) in entries {
            out.extend_from_slice(&position[0].to_be_bytes());
            out.extend_from_slice(&position[1].to_be_bytes());
            out.extend_from_slice(&position[2].to_be_bytes());
            out.extend_from_slice(&(record.kind.len() as u16).to_be_bytes());
            out.extend_from_slice(record.kind.as_bytes());
            match record.free_tickets {
                None => out.push(0),
                Some(tickets) => {
                    out.push(1);
                    out.extend_from_slice(&tickets.to_be_bytes());
                }
            }
        }
    }
    out
}

/// Decode a local POI file, validating every field and the region boundary.
pub fn decode_poi_file(bytes: &[u8], region: RegionPos) -> io::Result<PoiRegion> {
    let mut reader = PoiReader::new(bytes);
    if reader.take(4)? != POI_MAGIC {
        return Err(invalid("bad POI file magic"));
    }
    let version = reader.u8()?;
    if version != POI_FORMAT_VERSION {
        return Err(invalid(format!("unsupported POI file version {version}")));
    }
    let chunk_count = reader.u32()? as usize;
    if chunk_count > MAX_CHUNKS_PER_REGION {
        return Err(invalid(format!("too many POI chunks: {chunk_count}")));
    }
    let mut chunks = BTreeMap::new();
    let mut previous_chunk = None;
    for _ in 0..chunk_count {
        let chunk_x = reader.i32()?;
        let chunk_z = reader.i32()?;
        if RegionPos::from_chunk(chunk_x, chunk_z) != region {
            return Err(invalid("POI chunk is outside the file's region"));
        }
        let key = (chunk_x, chunk_z);
        if previous_chunk.is_some_and(|previous| key <= previous) {
            return Err(invalid("POI chunks are not in strictly increasing order"));
        }
        previous_chunk = Some(key);
        let record_count = reader.u32()? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..record_count {
            let position = [reader.i32()?, reader.i32()?, reader.i32()?];
            if (position[0].div_euclid(16), position[2].div_euclid(16)) != key {
                return Err(invalid("POI record is outside its chunk"));
            }
            let kind_len = reader.u16()? as usize;
            if kind_len == 0 || kind_len > MAX_KIND_LENGTH {
                return Err(invalid(format!("invalid POI type length {kind_len}")));
            }
            let kind = String::from_utf8(reader.take(kind_len)?.to_vec())
                .map_err(|error| invalid(format!("invalid POI type string: {error}")))?;
            let free_tickets = match reader.u8()? {
                0 => None,
                1 => Some(reader.u32()?),
                other => return Err(invalid(format!("unknown POI ticket flag {other}"))),
            };
            let record = PoiRecord {
                position,
                kind,
                free_tickets,
            };
            if entries.insert(position, record).is_some() {
                return Err(invalid("duplicate POI position in a chunk"));
            }
        }
        chunks.insert(key, entries);
    }
    if reader.remaining() != 0 {
        return Err(invalid("trailing bytes in POI file"));
    }
    Ok(chunks)
}

fn write_poi_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("poi.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

struct PoiReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> PoiReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| invalid("POI file length overflow"))?;
        if end > self.bytes.len() {
            return Err(invalid("truncated POI file"));
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(self.u32()? as i32)
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn region(x: i32, z: i32) -> RegionPos {
        RegionPos { x, z }
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-poi-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn encode_decode_round_trips_tickets_and_unknown_types() {
        let mut chunks = PoiRegion::new();
        chunks.entry((0, 0)).or_default().insert(
            [1, 64, 2],
            PoiRecord::new([1, 64, 2], "minecraft:home").with_tickets(3),
        );
        chunks.entry((0, 0)).or_default().insert(
            [15, 70, 15],
            PoiRecord::new([15, 70, 15], "somemod:custom_poi"),
        );
        chunks.entry((31, 0)).or_default().insert(
            [496, 71, 2],
            PoiRecord::new([496, 71, 2], "minecraft:librarian").with_tickets(1),
        );

        let bytes = encode_poi_file(region(0, 0), &chunks);
        let decoded = decode_poi_file(&bytes, region(0, 0)).unwrap();
        assert_eq!(decoded, chunks);
        assert_eq!(
            decoded[&(0, 0)][&[1, 64, 2]].free_tickets,
            Some(3),
            "ticket count survives"
        );
        assert_eq!(
            decoded[&(0, 0)][&[15, 70, 15]].kind,
            "somemod:custom_poi",
            "unknown types are preserved"
        );
        assert!(
            decode_poi_file(&bytes, region(1, 0)).is_err(),
            "records cannot be decoded into the wrong region"
        );
    }

    #[test]
    fn encoding_is_independent_of_insertion_order() {
        let first = PoiRecord::new([1, 64, 2], "minecraft:home").with_tickets(2);
        let second = PoiRecord::new([1, 64, 1], "minecraft:meeting");
        let third = PoiRecord::new([16, 65, 2], "minecraft:farmer");

        let mut a = PoiStorage::new(temp_dir("order-a"));
        let mut b = PoiStorage::new(temp_dir("order-b"));
        for (storage, records) in [
            (&mut a, [&first, &second, &third]),
            (&mut b, [&third, &second, &first]),
        ] {
            for record in records {
                let (x, z) = record.chunk();
                storage.insert(x, z, record.clone()).unwrap();
            }
        }
        let region = region(0, 0);
        assert_eq!(
            encode_poi_file(region, &a.region(region)),
            encode_poi_file(region, &b.region(region)),
            "the same records always produce the same bytes"
        );
    }

    #[test]
    fn decode_rejects_corrupt_files() {
        let good = encode_poi_file(region(0, 0), &PoiRegion::new());

        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert!(decode_poi_file(&bad_magic, region(0, 0)).is_err());

        let mut bad_version = good.clone();
        bad_version[4] = 99;
        assert!(decode_poi_file(&bad_version, region(0, 0)).is_err());

        assert!(decode_poi_file(&good[..good.len() - 1], region(0, 0)).is_err());
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode_poi_file(&trailing, region(0, 0)).is_err());
    }

    #[test]
    fn insert_validates_position_and_type() {
        let mut storage = PoiStorage::new(temp_dir("validate"));
        assert!(
            storage
                .insert(0, 0, PoiRecord::new([16, 64, 0], "minecraft:home"))
                .is_err()
        );
        assert!(
            storage
                .insert(0, 0, PoiRecord::new([0, 64, 0], ""))
                .is_err()
        );
        assert!(
            storage
                .insert(0, 0, PoiRecord::new([0, 64, 0], "x".repeat(2000)))
                .is_err()
        );
    }
}
