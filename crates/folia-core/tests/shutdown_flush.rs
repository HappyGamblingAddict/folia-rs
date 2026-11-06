use folia_core::anvil::Compression;
use folia_core::chunk_packet::flat_chunk;
use folia_core::world::{Dimension, WorldStorage, lock, shared_overworld};
use std::time::{SystemTime, UNIX_EPOCH};

/// Terrain saved through the shared storage is still in the region cache until
/// a flush. `flush_shared` is what shutdown uses to write it before exit.
#[test]
fn flush_shared_writes_cached_chunks_for_the_next_process() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-shutdown-flush-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();

    let shared = shared_overworld(&root);
    {
        let mut storage = lock(shared);
        storage
            .save_chunk(&flat_chunk(3, 4), Compression::Zlib)
            .unwrap();
    }
    folia_core::world::flush_shared().unwrap();

    let mut reopened = WorldStorage::for_dimension(&root, Dimension::Overworld);
    let chunk = reopened
        .load_chunk(3, 4)
        .unwrap()
        .expect("flush_shared wrote the cached chunk");
    assert_eq!((chunk.x, chunk.z), (3, 4));

    std::fs::remove_dir_all(root).ok();
}
