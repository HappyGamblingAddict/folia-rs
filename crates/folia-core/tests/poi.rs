//! Focused tests for per-chunk POI storage and its local region persistence.

use folia_core::anvil::Compression;
use folia_core::block_update::set_block;
use folia_core::chunk::{BlockState, encode_chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::nbt::Tag;
use folia_core::poi::{PoiRecord, PoiStorage, decode_poi_file, poi_dir_from_region_dir};
use folia_core::world::{Dimension, RegionPos, WorldStorage};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-poi-tests-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn record(x: i32, y: i32, z: i32, kind: &str, tickets: Option<u32>) -> PoiRecord {
    let mut record = PoiRecord::new([x, y, z], kind);
    record.free_tickets = tickets;
    record
}

fn region(x: i32, z: i32) -> RegionPos {
    RegionPos { x, z }
}

#[test]
fn add_remove_and_query_are_keyed_by_chunk_and_position_ordered() {
    let mut storage = PoiStorage::new(temp_dir("query"));
    storage
        .insert(0, 0, record(8, 70, 9, "minecraft:home", Some(1)))
        .unwrap();
    storage
        .insert(0, 0, record(1, 64, 2, "minecraft:farmer", None))
        .unwrap();
    storage
        .insert(1, 0, record(20, 64, 2, "minecraft:meeting", Some(2)))
        .unwrap();

    let chunk = storage.chunk_records(0, 0).unwrap();
    assert_eq!(
        chunk.iter().map(|entry| entry.position).collect::<Vec<_>>(),
        vec![[1, 64, 2], [8, 70, 9]],
        "queries are ordered by block position"
    );
    assert_eq!(chunk[0].kind, "minecraft:farmer");
    assert_eq!(chunk[1].free_tickets, Some(1));
    assert_eq!(storage.chunk_records(1, 0).unwrap().len(), 1);
    assert!(storage.chunk_records(5, 5).unwrap().is_empty());

    assert_eq!(
        storage
            .at(0, 0, [8, 70, 9])
            .unwrap()
            .map(|entry| entry.kind),
        Some("minecraft:home".to_owned())
    );

    let replaced = storage
        .insert(0, 0, record(1, 64, 2, "minecraft:librarian", Some(4)))
        .unwrap();
    assert_eq!(replaced.unwrap().kind, "minecraft:farmer");
    assert_eq!(
        storage.chunk_records(0, 0).unwrap()[0].kind,
        "minecraft:librarian"
    );

    assert_eq!(
        storage.remove(0, 0, [1, 64, 2]).unwrap().unwrap().kind,
        "minecraft:librarian"
    );
    assert!(storage.remove(0, 0, [1, 64, 2]).unwrap().is_none());
    assert_eq!(storage.chunk_records(0, 0).unwrap().len(), 1);

    let region_records = storage.region_records(region(0, 0)).unwrap();
    assert_eq!(
        region_records
            .iter()
            .map(|entry| entry.position)
            .collect::<Vec<_>>(),
        vec![[8, 70, 9], [20, 64, 2]],
        "a region query walks chunks in coordinate order"
    );
    assert_eq!(storage.clear_chunk(0, 0).unwrap(), 1);
    assert!(storage.chunk_records(0, 0).unwrap().is_empty());
}

#[test]
fn pois_are_written_to_one_local_file_per_region() {
    let root = temp_dir("per-region");
    let dir = root.join("poi");
    let mut storage = PoiStorage::new(&dir);
    storage
        .insert(0, 0, record(1, 64, 2, "minecraft:home", Some(1)))
        .unwrap();
    storage
        .insert(31, 31, record(497, 65, 498, "minecraft:meeting", None))
        .unwrap();
    storage
        .insert(32, 0, record(513, 64, 2, "minecraft:farmer", Some(2)))
        .unwrap();
    assert_eq!(storage.dirty_regions(), vec![region(0, 0), region(1, 0)]);
    assert_eq!(storage.flush().unwrap(), 2);

    let first = dir.join("r.0.0.poi");
    let second = dir.join("r.1.0.poi");
    assert!(first.exists() && second.exists());
    assert_eq!(
        decode_poi_file(&fs::read(&first).unwrap(), region(0, 0))
            .unwrap()
            .values()
            .map(|entries| entries.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(
        decode_poi_file(&fs::read(&second).unwrap(), region(1, 0))
            .unwrap()
            .values()
            .map(|entries| entries.len())
            .sum::<usize>(),
        1
    );

    let reopened = PoiStorage::new(&dir);
    assert_eq!(reopened.dirty_regions(), Vec::new());
    let mut reopened = reopened;
    assert_eq!(reopened.chunk_records(0, 0).unwrap().len(), 1);
    assert_eq!(reopened.chunk_records(31, 31).unwrap().len(), 1);
    assert_eq!(reopened.chunk_records(32, 0).unwrap().len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_edits_batch_into_one_write_per_region() {
    let root = temp_dir("batching");
    let dir = root.join("poi");
    let mut storage = PoiStorage::new(&dir);
    storage
        .insert(0, 0, record(1, 64, 1, "minecraft:home", None))
        .unwrap();
    storage
        .insert(0, 0, record(2, 64, 1, "minecraft:home", None))
        .unwrap();
    storage
        .insert(1, 0, record(20, 64, 1, "minecraft:home", None))
        .unwrap();

    assert_eq!(storage.dirty_regions(), vec![region(0, 0)]);
    assert_eq!(storage.flush().unwrap(), 1);
    assert_eq!(storage.write_count(), 1);
    assert!(storage.dirty_regions().is_empty());
    assert_eq!(storage.flush().unwrap(), 0, "a clean flush writes nothing");
    assert_eq!(storage.write_count(), 1);

    storage
        .insert(0, 0, record(3, 64, 1, "minecraft:home", None))
        .unwrap();
    assert_eq!(storage.flush().unwrap(), 1);
    assert_eq!(storage.write_count(), 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_poi_file_is_skipped_and_repaired_by_a_later_flush() {
    let root = temp_dir("corrupt");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let poi_dir = poi_dir_from_region_dir(&region_dir);
    fs::create_dir_all(&poi_dir).unwrap();
    fs::write(poi_dir.join("r.0.0.poi"), b"not a POI file at all").unwrap();

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        let mut reopened = WorldStorage::new(&region_dir);
        assert!(
            reopened.load_chunk(0, 0).unwrap().is_some(),
            "a damaged POI file does not block the chunk load"
        );
        assert!(
            reopened.pois(0, 0).unwrap().is_empty(),
            "the corrupt file is treated as an empty region"
        );
        reopened
            .insert_poi(record(4, 64, 5, "minecraft:bed", Some(1)))
            .unwrap();
        assert_eq!(reopened.dirty_poi_regions(), vec![region(0, 0)]);
        reopened.flush().unwrap();
    }

    let mut final_storage = WorldStorage::new(&region_dir);
    final_storage.load_chunk(0, 0).unwrap();
    assert_eq!(
        final_storage.pois(0, 0).unwrap(),
        vec![record(4, 64, 5, "minecraft:bed", Some(1))],
        "the repaired file contains only the real edit"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn world_save_reload_keeps_tickets_and_unknown_poi_types() {
    let root = temp_dir("save-reload");
    let region_dir = Dimension::Overworld.region_dir(&root);

    {
        let mut storage = WorldStorage::new(&region_dir);
        assert_eq!(storage.poi_storage().dir().to_path_buf(), root.join("poi"));
        storage
            .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();
        storage
            .insert_poi(record(1, 64, 2, "minecraft:librarian", Some(3)))
            .unwrap();
        storage
            .insert_poi(record(9, 65, 3, "somemod:custom_poi", None))
            .unwrap();
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    assert_eq!(
        reopened.poi_at(0, 0, [1, 64, 2]).unwrap(),
        Some(record(1, 64, 2, "minecraft:librarian", Some(3)))
    );
    assert_eq!(
        reopened.pois(0, 0).unwrap(),
        vec![
            record(1, 64, 2, "minecraft:librarian", Some(3)),
            record(9, 65, 3, "somemod:custom_poi", None),
        ]
    );

    reopened
        .remove_poi(0, 0, [1, 64, 2])
        .unwrap()
        .expect("record existed");
    reopened.flush().unwrap();
    let mut again = WorldStorage::new(&region_dir);
    again.load_chunk(0, 0).unwrap();
    assert_eq!(
        again.pois(0, 0).unwrap(),
        vec![record(9, 65, 3, "somemod:custom_poi", None)]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn world_flush_and_unload_persist_poi_edits() {
    let root = temp_dir("world-flush");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();

    storage
        .insert_poi(record(2, 64, 3, "minecraft:home", Some(1)))
        .unwrap();
    storage
        .insert_poi(record(513, 64, 3, "minecraft:home", Some(1)))
        .unwrap();
    assert_eq!(
        storage.dirty_poi_regions(),
        vec![region(0, 0), region(1, 0)]
    );
    storage.flush().unwrap();
    assert!(storage.dirty_poi_regions().is_empty());
    assert!(root.join("poi").join("r.1.0.poi").exists());

    storage
        .insert_poi(record(3, 65, 3, "minecraft:farmer", None))
        .unwrap();
    storage.unload_region(region(0, 0)).unwrap();
    assert!(storage.dirty_poi_regions().is_empty());
    assert!(
        !storage
            .poi_storage()
            .loaded_regions()
            .contains(&region(0, 0))
    );

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap();
    reopened.load_chunk(32, 0).unwrap();
    assert_eq!(reopened.pois(0, 0).unwrap().len(), 2);
    assert_eq!(reopened.pois(32, 0).unwrap().len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn regenerating_a_chunk_drops_stale_pois() {
    let root = temp_dir("regenerate");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();
    storage
        .insert_poi(record(1, 64, 1, "minecraft:home", None))
        .unwrap();
    storage.flush().unwrap();

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap();
    assert_eq!(reopened.pois(0, 0).unwrap().len(), 1);
    reopened
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    reopened.flush().unwrap();

    let mut final_storage = WorldStorage::new(&region_dir);
    final_storage.load_chunk(0, 0).unwrap();
    assert!(
        final_storage.pois(0, 0).unwrap().is_empty(),
        "new terrain does not inherit POIs from the old column"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn editing_a_chunk_keeps_its_pois_and_unknown_chunk_fields() {
    let root = temp_dir("update");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();
    storage
        .insert_poi(record(1, 64, 1, "minecraft:home", Some(1)))
        .unwrap();
    storage.flush().unwrap();

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    let change = set_block(
        &mut reopened,
        0,
        -50,
        0,
        BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        },
    )
    .unwrap();
    assert!(change.is_some());
    reopened.flush().unwrap();

    let mut final_storage = WorldStorage::new(&region_dir);
    final_storage.load_chunk(0, 0).unwrap();
    assert_eq!(final_storage.pois(0, 0).unwrap().len(), 1);
    let raw_region = folia_core::anvil::RegionFile::open(
        Dimension::Overworld.region_dir(&root).join("r.0.0.mca"),
    )
    .unwrap();
    let root_tag = raw_region.read_chunk(0, 0).unwrap().unwrap();
    assert!(matches!(root_tag, Tag::Compound(_)));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn poi_files_are_byte_deterministic_across_insertion_orders() {
    let first_root = temp_dir("bytes-a");
    let second_root = temp_dir("bytes-b");
    let records = [
        record(1, 64, 2, "minecraft:home", Some(1)),
        record(1, 64, 1, "minecraft:meeting", None),
        record(20, 65, 3, "minecraft:librarian", Some(2)),
    ];

    for (root, order) in [(&first_root, 0), (&second_root, 1)] {
        let mut storage = WorldStorage::new(Dimension::Overworld.region_dir(root));
        storage
            .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();
        let indices: Vec<usize> = if order == 0 {
            vec![0, 1, 2]
        } else {
            vec![2, 1, 0]
        };
        for index in indices {
            storage.insert_poi(records[index].clone()).unwrap();
        }
        storage.flush().unwrap();
    }

    let first = fs::read(first_root.join("poi").join("r.0.0.poi")).unwrap();
    let second = fs::read(second_root.join("poi").join("r.0.0.poi")).unwrap();
    assert_eq!(first, second, "the same POI set has one canonical encoding");
    assert!(first.starts_with(b"FPOI"));

    fs::remove_dir_all(first_root).unwrap();
    fs::remove_dir_all(second_root).unwrap();
}

#[test]
fn malformed_record_lengths_are_rejected_without_touching_the_region_file() {
    let root = temp_dir("bad-records");
    let dir = root.join("poi");
    let region = region(0, 0);
    let mut storage = PoiStorage::new(&dir);
    storage
        .insert(0, 0, record(1, 64, 1, "minecraft:home", None))
        .unwrap();
    storage.flush().unwrap();

    let mut bytes = fs::read(dir.join("r.0.0.poi")).unwrap();
    bytes[33] = 0xff;
    bytes[34] = 0xff;
    fs::write(dir.join("r.0.0.poi"), bytes).unwrap();

    let mut reopened = PoiStorage::new(&dir);
    assert!(
        reopened.chunk_records(0, 0).unwrap().is_empty(),
        "a malformed record length is skipped, not partially applied"
    );
    assert!(!reopened.is_region_dirty(region));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn plain_chunk_encode_still_has_no_poi_section() {
    let chunk = flat_chunk(0, 0);
    let tag = encode_chunk(&chunk).unwrap();
    assert!(
        tag.get("poi").is_none(),
        "POIs stay in their own file, not in a chunk section"
    );
}
