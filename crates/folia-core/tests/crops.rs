//! Crop age growth and light gating.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::crops;
use folia_core::math::BlockPos;
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::time::{SystemTime, UNIX_EPOCH};

fn storage(name: &str) -> WorldStorage {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-crops-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.set_tick_seed(7);
    storage.tick_state_mut().random.random_tick_speed = 4_096;
    storage
}

fn place(storage: &mut WorldStorage, pos: BlockPos, state: BlockState) {
    block_update::set_block(storage, pos.x(), pos.y(), pos.z(), state)
        .unwrap()
        .expect("placement changed the block");
}

fn block(storage: &WorldStorage, pos: BlockPos) -> BlockState {
    storage
        .block_state_at(pos.x(), pos.y(), pos.z())
        .expect("block is in a cached chunk")
}

fn state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

#[test]
fn bone_meal_style_growth_clamps_at_the_crop_maximum() {
    let wheat = state("minecraft:wheat");
    assert_eq!(crops::age(&wheat), 0);
    assert_eq!(crops::age(&crops::grow(&wheat, 3)), 3);
    assert_eq!(crops::age(&crops::grow(&wheat, 200)), 7);

    let beetroot = state("minecraft:beetroots");
    assert_eq!(crops::age(&crops::grow(&beetroot, 200)), 3);
}

#[test]
fn crop_identity_covers_the_vanilla_crops() {
    for name in [
        "minecraft:wheat",
        "minecraft:carrots",
        "minecraft:potatoes",
        "minecraft:beetroots",
    ] {
        assert!(crops::is_crop(&state(name)), "{name}");
    }
    assert!(!crops::is_crop(&state("minecraft:oak_sapling")));
}

#[test]
fn wheat_grows_on_a_random_tick_under_sky_light() {
    let mut storage = storage("grow");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, state("minecraft:farmland"));
    place(
        &mut storage,
        BlockPos::new(0, -59, 0),
        state("minecraft:wheat"),
    );
    for tick_number in 1..=20 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    let crop = block(&storage, BlockPos::new(0, -59, 0));
    assert!(
        crops::age(&crop) > 0,
        "wheat did not advance under full sky light"
    );
}

#[test]
fn covered_crops_stay_at_age_zero() {
    let mut storage = storage("dark");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, state("minecraft:farmland"));
    place(
        &mut storage,
        BlockPos::new(0, -59, 0),
        state("minecraft:wheat"),
    );
    place(
        &mut storage,
        BlockPos::new(0, -58, 0),
        state("minecraft:stone"),
    );
    for tick_number in 1..=10 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(crops::age(&block(&storage, BlockPos::new(0, -59, 0))), 0);
}
