use folia_core::anvil::Compression;
use folia_core::block_update::set_block;
use folia_core::chunk::{BlockState, encode_chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::world::{Dimension, RegionPos, WorldStorage};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-dirty-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn edits_survive_a_batched_flush_and_reload() {
    let root = temp_dir("flush-reload");
    let region_dir = Dimension::Overworld.region_dir(&root);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
            .unwrap();
        assert!(storage.is_chunk_dirty(0, 0), "generated chunk is staged");
        storage.flush().unwrap();
        assert_eq!(storage.dirty_chunk_count(), 0);

        let change = set_block(&mut storage, 0, -50, 0, state("minecraft:stone"))
            .unwrap()
            .expect("block changed");
        assert_eq!(change.state_id, 1);
        assert!(
            storage.is_chunk_dirty(0, 0),
            "a block edit is staged without an immediate region write"
        );
        assert_eq!(storage.dirty_chunk_count(), 1);

        storage.flush().unwrap();
        assert_eq!(storage.dirty_chunk_count(), 0);
    }

    let mut reopened = WorldStorage::new(&region_dir);
    let chunk = reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    assert_eq!(
        chunk.block_state(0, -50, 0).unwrap().name,
        "minecraft:stone"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_edits_write_the_chunk_once_per_batch() {
    let root = temp_dir("single-write");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.flush().unwrap();
    let baseline = storage.chunk_write_count();

    set_block(&mut storage, 0, -50, 0, state("minecraft:stone")).unwrap();
    set_block(&mut storage, 0, -50, 0, state("minecraft:dirt")).unwrap();
    set_block(&mut storage, 1, -50, 0, state("minecraft:stone")).unwrap();

    assert_eq!(
        storage.dirty_chunk_count(),
        1,
        "three edits collapse to one queued chunk"
    );
    storage.flush().unwrap();
    assert_eq!(
        storage.chunk_write_count(),
        baseline + 1,
        "one chunk record is applied per batch"
    );
    assert_eq!(storage.dirty_chunk_count(), 0);

    let mut reopened = WorldStorage::new(&region_dir);
    let chunk = reopened.load_chunk(0, 0).unwrap().unwrap();
    assert_eq!(
        chunk.block_state(0, -50, 0).unwrap().name,
        "minecraft:dirt",
        "the last edit wins"
    );
    assert_eq!(
        chunk.block_state(1, -50, 0).unwrap().name,
        "minecraft:stone"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_batch_groups_dirty_chunks_by_region() {
    let root = temp_dir("by-region");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);

    for (chunk_x, chunk_z) in [(0, 0), (1, 0), (32, 0), (33, 0)] {
        storage
            .save_chunk(&flat_chunk(chunk_x, chunk_z), Compression::Zlib)
            .unwrap();
    }
    assert_eq!(storage.dirty_chunk_count(), 4);
    assert_eq!(
        storage.dirty_chunk_coords(),
        vec![(0, 0), (1, 0), (32, 0), (33, 0)],
        "flush order is deterministic"
    );
    let baseline = storage.chunk_write_count();
    storage.flush().unwrap();
    assert_eq!(storage.chunk_write_count(), baseline + 4);
    assert_eq!(storage.dirty_chunk_count(), 0);
    assert_eq!(storage.loaded_regions(), 2);

    let mut reopened = WorldStorage::new(&region_dir);
    for (chunk_x, chunk_z) in [(0, 0), (1, 0), (32, 0), (33, 0)] {
        assert!(
            reopened.load_chunk(chunk_x, chunk_z).unwrap().is_some(),
            "chunk ({chunk_x},{chunk_z}) reached disk"
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_pending_edit_survives_cache_eviction() {
    let root = temp_dir("eviction");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    set_block(&mut storage, 0, -50, 0, state("minecraft:stone")).unwrap();

    // Push the edited column out of the 256-entry decode cache before flush.
    for chunk_x in 1..260 {
        storage
            .save_chunk(&flat_chunk(chunk_x, 0), Compression::Zlib)
            .unwrap();
    }
    let reloaded = storage.load_chunk(0, 0).unwrap().unwrap();
    assert_eq!(
        reloaded.block_state(0, -50, 0).unwrap().name,
        "minecraft:stone",
        "the queued root wins over stale disk data"
    );

    storage.flush().unwrap();
    let mut reopened = WorldStorage::new(&region_dir);
    assert_eq!(
        reopened
            .load_chunk(0, 0)
            .unwrap()
            .unwrap()
            .block_state(0, -50, 0)
            .unwrap()
            .name,
        "minecraft:stone"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unloading_a_region_flushes_its_queued_edits() {
    let root = temp_dir("unload-queued");
    let region_dir = Dimension::Overworld.region_dir(&root);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&flat_chunk(5, 5), Compression::Zlib)
            .unwrap();
        assert!(storage.is_chunk_dirty(5, 5));
        storage.unload_region(RegionPos::from_chunk(5, 5)).unwrap();
        assert!(!storage.is_chunk_dirty(5, 5), "unload clears the batch");
    }

    let mut reopened = WorldStorage::new(&region_dir);
    assert!(reopened.load_chunk(5, 5).unwrap().is_some());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mark_and_clear_dirty_control_the_batch() {
    let root = temp_dir("mark-clear");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    let tag = encode_chunk(&flat_chunk(7, 7)).unwrap();

    storage.mark_dirty(7, 7, tag, Compression::Zlib);
    assert!(storage.is_chunk_dirty(7, 7));
    assert_eq!(storage.dirty_chunk_count(), 1);

    storage.clear_dirty();
    assert!(!storage.is_chunk_dirty(7, 7));
    storage.flush().unwrap();
    assert_eq!(
        storage.chunk_write_count(),
        0,
        "cleared writes are not applied"
    );
    assert!(storage.load_chunk(7, 7).unwrap().is_none());

    fs::remove_dir_all(root).unwrap();
}
