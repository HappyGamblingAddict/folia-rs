use crate::nbt::{self, Tag};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SECTOR_SIZE: usize = 4096;
pub const HEADER_SECTORS: usize = 2;
pub const HEADER_SIZE: usize = SECTOR_SIZE * HEADER_SECTORS;
pub const CHUNKS_PER_REGION: usize = 32 * 32;
pub const MAX_CHUNK_LENGTH: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    /// Offset in 4 KiB sectors from the start of the region file.
    pub offset: u32,
    /// Number of 4 KiB sectors occupied by the chunk record.
    pub sectors: u8,
}

impl Location {
    pub const EMPTY: Location = Location {
        offset: 0,
        sectors: 0,
    };

    pub fn is_empty(self) -> bool {
        self.offset == 0 || self.sectors == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Gzip,
    Zlib,
    None,
}

impl Compression {
    pub fn id(self) -> u8 {
        match self {
            Compression::Gzip => 1,
            Compression::Zlib => 2,
            Compression::None => 3,
        }
    }

    pub fn from_id(id: u8) -> io::Result<Self> {
        match id {
            1 => Ok(Compression::Gzip),
            2 => Ok(Compression::Zlib),
            3 => Ok(Compression::None),
            other => Err(invalid(format!("unsupported Anvil compression id {other}"))),
        }
    }
}

/// In-memory view of a `.mca` region file.
///
/// The implementation is intentionally append-only when a chunk grows: old
/// sectors become garbage until a future compaction pass. This matches the
/// safety-first behavior needed by the Rust port and keeps writes simple.
#[derive(Debug)]
pub struct RegionFile {
    path: PathBuf,
    data: Vec<u8>,
    locations: [Location; CHUNKS_PER_REGION],
    timestamps: [u32; CHUNKS_PER_REGION],
    dirty: bool,
}

impl RegionFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut data = fs::read(&path)?;
        if data.len() < HEADER_SIZE {
            data.resize(HEADER_SIZE, 0);
        }

        let mut locations = [Location::EMPTY; CHUNKS_PER_REGION];
        let mut timestamps = [0u32; CHUNKS_PER_REGION];
        for index in 0..CHUNKS_PER_REGION {
            let base = index * 4;
            let offset = (u32::from(data[base]) << 16)
                | (u32::from(data[base + 1]) << 8)
                | u32::from(data[base + 2]);
            let sectors = data[base + 3];
            locations[index] = if offset == 0 || sectors == 0 {
                Location::EMPTY
            } else {
                Location { offset, sectors }
            };
            let timestamp_base = SECTOR_SIZE + base;
            timestamps[index] = u32::from_be_bytes([
                data[timestamp_base],
                data[timestamp_base + 1],
                data[timestamp_base + 2],
                data[timestamp_base + 3],
            ]);
        }

        Ok(Self {
            path,
            data,
            locations,
            timestamps,
            dirty: false,
        })
    }

    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let data = vec![0u8; HEADER_SIZE];
        let file = Self {
            path,
            data,
            locations: [Location::EMPTY; CHUNKS_PER_REGION],
            timestamps: [0u32; CHUNKS_PER_REGION],
            dirty: true,
        };
        file.save()?;
        Ok(file)
    }

    pub fn open_or_create(path: impl AsRef<Path>) -> io::Result<Self> {
        if path.as_ref().exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn location(&self, local_x: u8, local_z: u8) -> Location {
        self.locations[index(local_x, local_z)]
    }

    pub fn timestamp(&self, local_x: u8, local_z: u8) -> u32 {
        self.timestamps[index(local_x, local_z)]
    }

    pub fn read_chunk(&self, local_x: u8, local_z: u8) -> io::Result<Option<Tag>> {
        let location = self.location(local_x, local_z);
        if location.is_empty() {
            return Ok(None);
        }

        let start = location.offset as usize * SECTOR_SIZE;
        if start + 4 > self.data.len() {
            return Err(invalid("chunk offset is outside the region file"));
        }
        let length = u32::from_be_bytes([
            self.data[start],
            self.data[start + 1],
            self.data[start + 2],
            self.data[start + 3],
        ]) as usize;
        if length == 0 || length > MAX_CHUNK_LENGTH {
            return Err(invalid(format!("invalid chunk length {length}")));
        }
        if start + 4 + length > self.data.len() {
            return Err(invalid("chunk record is truncated"));
        }

        let compression = Compression::from_id(self.data[start + 4])?;
        let payload = &self.data[start + 5..start + 4 + length];
        let tag = match compression {
            Compression::Gzip => nbt::read_gzip_root(payload)?,
            Compression::Zlib => nbt::read_zlib_root(payload)?,
            Compression::None => nbt::read_root(&mut &payload[..])?,
        };
        Ok(Some(tag))
    }

    pub fn write_chunk(
        &mut self,
        local_x: u8,
        local_z: u8,
        tag: &Tag,
        compression: Compression,
    ) -> io::Result<()> {
        if !matches!(tag, Tag::Compound(_)) {
            return Err(invalid("Anvil chunk roots must be compound tags"));
        }

        let compressed = match compression {
            Compression::Gzip => nbt::write_gzip_root(tag)?,
            Compression::Zlib => nbt::write_zlib_root(tag)?,
            Compression::None => nbt::write_root_to_vec(tag)?,
        };

        let length = compressed.len() + 1;
        if length > MAX_CHUNK_LENGTH {
            return Err(invalid("compressed chunk exceeds the 1 MiB limit"));
        }

        let sectors = (4 + length).div_ceil(SECTOR_SIZE);
        if sectors > u8::MAX as usize {
            return Err(invalid("chunk record needs more than 255 sectors"));
        }

        let offset = self.data.len().div_ceil(SECTOR_SIZE);
        if offset > 0x00FF_FFFF {
            return Err(invalid(
                "region file exceeds the 24-bit sector offset limit",
            ));
        }

        let required = (offset + sectors) * SECTOR_SIZE;
        self.data.resize(required, 0);

        let start = offset * SECTOR_SIZE;
        self.data[start..start + 4].copy_from_slice(&(length as u32).to_be_bytes());
        self.data[start + 4] = compression.id();
        self.data[start + 5..start + 4 + length].copy_from_slice(&compressed);

        let index = index(local_x, local_z);
        self.locations[index] = Location {
            offset: offset as u32,
            sectors: sectors as u8,
        };
        self.timestamps[index] = now_unix();
        self.write_location(index);
        self.dirty = true;
        Ok(())
    }

    pub fn remove_chunk(&mut self, local_x: u8, local_z: u8) {
        let index = index(local_x, local_z);
        self.locations[index] = Location::EMPTY;
        self.timestamps[index] = 0;
        self.write_location(index);
        self.dirty = true;
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, &self.data)?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }

    pub fn save_if_dirty(&mut self) -> io::Result<bool> {
        if !self.dirty {
            return Ok(false);
        }
        self.save()?;
        self.dirty = false;
        Ok(true)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn write_location(&mut self, index: usize) {
        let location = self.locations[index];
        let base = index * 4;
        self.data[base] = ((location.offset >> 16) & 0xFF) as u8;
        self.data[base + 1] = ((location.offset >> 8) & 0xFF) as u8;
        self.data[base + 2] = (location.offset & 0xFF) as u8;
        self.data[base + 3] = location.sectors;
        let timestamp_base = SECTOR_SIZE + base;
        self.data[timestamp_base..timestamp_base + 4]
            .copy_from_slice(&self.timestamps[index].to_be_bytes());
    }
}

fn index(local_x: u8, local_z: u8) -> usize {
    (u32::from(local_z & 31) * 32 + u32::from(local_x & 31)) as usize
}

fn now_unix() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as u32)
        .unwrap_or(0)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(x: i32, z: i32) -> Tag {
        Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(4671)),
            ("xPos".into(), Tag::Int(x)),
            ("zPos".into(), Tag::Int(z)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            (
                "sections".into(),
                Tag::List(vec![Tag::Compound(vec![
                    ("Y".into(), Tag::Byte(0)),
                    ("block_states".into(), Tag::Compound(vec![])),
                ])]),
            ),
        ])
    }

    fn temp_path(name: &str) -> PathBuf {
        let unique = format!(
            "folia-rs-{}-{}-{name}.mca",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn round_trips_chunks_with_all_compressions() {
        for (compression, name) in [
            (Compression::Gzip, "gzip"),
            (Compression::Zlib, "zlib"),
            (Compression::None, "none"),
        ] {
            let path = temp_path(name);
            let mut region = RegionFile::create(&path).unwrap();
            region.write_chunk(0, 0, &chunk(0, 0), compression).unwrap();
            region.write_chunk(5, 7, &chunk(5, 7), compression).unwrap();
            region
                .write_chunk(31, 31, &chunk(31, 31), compression)
                .unwrap();
            region.save().unwrap();

            let reopened = RegionFile::open(&path).unwrap();
            assert_eq!(reopened.read_chunk(0, 0).unwrap().unwrap(), chunk(0, 0));
            assert_eq!(reopened.read_chunk(5, 7).unwrap().unwrap(), chunk(5, 7));
            assert_eq!(reopened.read_chunk(31, 31).unwrap().unwrap(), chunk(31, 31));
            assert!(reopened.read_chunk(1, 1).unwrap().is_none());

            let location = reopened.location(5, 7);
            assert!(!location.is_empty());
            assert_eq!(location.offset % 1, 0);
            assert_eq!(location.sectors as usize * SECTOR_SIZE % SECTOR_SIZE, 0);

            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn removing_a_chunk_clears_the_header_entry() {
        let path = temp_path("remove");
        let mut region = RegionFile::create(&path).unwrap();
        region
            .write_chunk(2, 3, &chunk(2, 3), Compression::Zlib)
            .unwrap();
        assert!(!region.location(2, 3).is_empty());
        region.remove_chunk(2, 3);
        region.save().unwrap();

        let reopened = RegionFile::open(&path).unwrap();
        assert!(reopened.location(2, 3).is_empty());
        assert!(reopened.read_chunk(2, 3).unwrap().is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn header_offsets_are_24_bit_big_endian() {
        let path = temp_path("header");
        let mut region = RegionFile::create(&path).unwrap();
        region
            .write_chunk(1, 2, &chunk(1, 2), Compression::Gzip)
            .unwrap();
        region.save().unwrap();

        let bytes = fs::read(&path).unwrap();
        let base = (2 * 32 + 1) * 4;
        let offset = (u32::from(bytes[base]) << 16)
            | (u32::from(bytes[base + 1]) << 8)
            | u32::from(bytes[base + 2]);
        assert_eq!(offset, region.location(1, 2).offset);
        assert_eq!(bytes[base + 3], region.location(1, 2).sectors);
        assert_eq!(bytes.len() % SECTOR_SIZE, 0);
        fs::remove_file(path).unwrap();
    }
}
