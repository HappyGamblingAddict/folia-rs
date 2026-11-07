//! Region tick loop: per-region task queues plus per-region world ticking.
//!
//! These tests pin the `RegionTickDriver` integration: loaded chunks are
//! grouped into vanilla 8x8 regions, each region ticks its own tasks,
//! scheduled/random ticks, block entities, hoppers, and neighbour updates in
//! sorted key order, and an ownership failure is reported instead of panicking.

use folia_core::anvil::Compression;
use folia_core::block_entity::{BlockEntity, BlockEntityPos, ChestBlockEntity, HopperBlockEntity};
use folia_core::chunk::BlockState;
use folia_core::chunk_packet::flat_chunk;
use folia_core::item_components::ItemStack;
use folia_core::math::BlockPos;
use folia_core::region::{ChunkPos, OwnershipError, RegionKey, RegionTickDriver, RegionTickError};
use folia_core::tick::{BehaviorTable, TickWorld, UpdateFlags};
use folia_core::world::{Dimension, WorldStorage, flush_shared, lock, shared_overworld};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn key(x: i32, z: i32) -> RegionKey {
    RegionKey::new(x, z)
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-region-tick-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage(name: &str) -> (WorldStorage, PathBuf) {
    let root = temp_dir(name);
    let world = WorldStorage::new(Dimension::Overworld.region_dir(&root));
    (world, root)
}

fn load_flat(storage: &mut WorldStorage, chunk_x: i32, chunk_z: i32) {
    storage
        .save_chunk(&flat_chunk(chunk_x, chunk_z), Compression::Zlib)
        .unwrap();
}

fn insert_hopper_pair(
    storage: &mut WorldStorage,
    x: i32,
    z: i32,
) -> (BlockEntityPos, BlockEntityPos) {
    let hopper_pos = BlockEntityPos::new(x, 64, z);
    let chest_pos = BlockEntityPos::new(x, 63, z);
    let mut hopper = HopperBlockEntity::new(hopper_pos);
    assert!(hopper.set_slot(0, ItemStack::new("minecraft:iron_ingot", 5)));
    storage
        .insert_block_entity(BlockEntity::Hopper(hopper))
        .unwrap();
    storage
        .insert_block_entity(BlockEntity::Chest(ChestBlockEntity::new(chest_pos)))
        .unwrap();
    (hopper_pos, chest_pos)
}

fn hopper_state(storage: &mut WorldStorage, pos: BlockEntityPos) -> (i32, i32) {
    let Some(BlockEntity::Hopper(hopper)) = storage.get_block_entity(pos).unwrap() else {
        panic!("hopper missing at {pos:?}");
    };
    (hopper.slot(0).unwrap().count, hopper.transfer_cooldown)
}

fn chest_state(storage: &mut WorldStorage, pos: BlockEntityPos) -> (String, i32) {
    let Some(BlockEntity::Chest(chest)) = storage.get_block_entity(pos).unwrap() else {
        panic!("chest missing at {pos:?}");
    };
    let stack = chest.slot(0).unwrap();
    (stack.item.clone(), stack.count)
}

#[test]
fn driver_ticks_two_regions_in_sorted_key_order() {
    let (mut world, root) = storage("two-regions");
    load_flat(&mut world, 0, 0); // region (0, 0)
    load_flat(&mut world, 8, 0); // region (1, 0)

    let mut driver = RegionTickDriver::vanilla();
    let log = Arc::new(Mutex::new(Vec::new()));
    for region in [key(1, 0), key(0, 0)] {
        let log = Arc::clone(&log);
        driver.schedule_for_region(region, 0, 0, move || {
            log.lock().unwrap().push(region);
        });
    }

    let report = driver.tick(&mut world, 1).unwrap();
    assert_eq!(report.regions, 2);
    assert_eq!(report.world_regions, 2);
    assert_eq!(report.tasks, 2);
    assert_eq!(*log.lock().unwrap(), [key(0, 0), key(1, 0)]);
    assert_eq!(driver.region_ticks(key(0, 0)), Some(1));
    assert_eq!(driver.region_ticks(key(1, 0)), Some(1));

    // A task-only region still ticks its queue, but runs no world tick.
    driver.schedule_for_region(key(5, 5), 0, 0, || {});
    let report = driver.tick(&mut world, 2).unwrap();
    assert_eq!(report.regions, 3);
    assert_eq!(report.world_regions, 2);
    assert_eq!(driver.region_ticks(key(5, 5)), Some(1));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn cross_region_task_fails_without_panicking_in_serial_and_parallel_modes() {
    let (mut world, root) = storage("cross-region-task");

    let mut serial = RegionTickDriver::vanilla();
    serial.schedule_owned_at(ChunkPos::new(0, 0), 0, 0, |region_world| {
        region_world.touch(key(1, 0))?;
        Ok(())
    });
    let error = serial
        .tick_region(&mut world, key(0, 0), 1)
        .expect_err("a cross-region owned task must fail");
    match error {
        RegionTickError::Ownership(error) => {
            assert_eq!(error, OwnershipError::cross_region(key(0, 0), key(1, 0)))
        }
        other => panic!("expected an ownership error, got {other:?}"),
    }
    assert_eq!(serial.region_ticks(key(0, 0)), Some(1));

    // Parallel mode: region (0, 0)'s task fails while region (1, 0)'s task
    // still runs. The error is returned, never turned into a panic.
    let mut parallel = RegionTickDriver::vanilla();
    parallel.schedule_owned_at(ChunkPos::new(0, 0), 0, 0, |region_world| {
        region_world.touch(key(1, 0))?;
        Ok(())
    });
    let executed = Arc::new(AtomicUsize::new(0));
    {
        let executed = Arc::clone(&executed);
        parallel.schedule_for_region(key(1, 0), 0, 0, move || {
            executed.fetch_add(1, Ordering::SeqCst);
        });
    }

    let error = parallel
        .tick_parallel_for_test(&mut world, 2, 2)
        .expect_err("the parallel tick must surface the ownership error");
    assert!(matches!(
        error,
        RegionTickError::Ownership(OwnershipError {
            expected,
            actual: Some(actual),
            ..
        }) if expected == key(0, 0) && actual == key(1, 0)
    ));
    assert_eq!(executed.load(Ordering::SeqCst), 1);
    assert_eq!(parallel.region_ticks(key(1, 0)), Some(1));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn block_entities_and_hoppers_only_tick_for_their_owning_region() {
    let (mut world, root) = storage("owner-block-entities");
    load_flat(&mut world, 0, 0);
    load_flat(&mut world, 8, 0);
    let (left_hopper, left_chest) = insert_hopper_pair(&mut world, 0, 0);
    let (right_hopper, right_chest) = insert_hopper_pair(&mut world, 128, 0);

    let mut driver = RegionTickDriver::vanilla();
    driver.tick_region(&mut world, key(0, 0), 1).unwrap();

    assert_eq!(hopper_state(&mut world, left_hopper), (4, 8));
    assert_eq!(
        chest_state(&mut world, left_chest),
        ("minecraft:iron_ingot".to_owned(), 1)
    );
    // The other region's hopper/chest must be untouched by this tick.
    assert_eq!(hopper_state(&mut world, right_hopper), (5, 0));
    assert_eq!(chest_state(&mut world, right_chest), (String::new(), 0));

    driver.tick_region(&mut world, key(1, 0), 2).unwrap();
    assert_eq!(hopper_state(&mut world, right_hopper), (4, 8));
    assert_eq!(
        chest_state(&mut world, right_chest),
        ("minecraft:iron_ingot".to_owned(), 1)
    );
    // Region (0, 0) was not ticked, so its cooldown did not advance.
    assert_eq!(hopper_state(&mut world, left_hopper), (4, 8));

    std::fs::remove_dir_all(root).ok();
}

static SCHEDULED_HITS: AtomicUsize = AtomicUsize::new(0);
static RANDOM_HITS: AtomicUsize = AtomicUsize::new(0);

struct CountingBehavior;

impl folia_core::tick::BlockBehavior for CountingBehavior {
    fn is_randomly_ticking(&self, _state: &BlockState) -> bool {
        true
    }

    fn random_tick(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
    ) -> std::io::Result<()> {
        RANDOM_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn scheduled_tick(
        &self,
        _world: &mut TickWorld<'_>,
        _pos: BlockPos,
        _state: &BlockState,
    ) -> std::io::Result<()> {
        SCHEDULED_HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

static COUNTING: CountingBehavior = CountingBehavior;

fn stone() -> BlockState {
    BlockState {
        name: "minecraft:stone".into(),
        properties: Vec::new(),
    }
}

#[test]
fn scheduled_random_and_neighbor_work_is_scoped_to_the_region_chunks() {
    let (mut world, root) = storage("scoped-world-work");
    let map = RegionTickDriver::vanilla().region_map();
    for (chunk_x, chunk_z) in [(0, 0), (8, 0)] {
        let mut chunk = flat_chunk(chunk_x, chunk_z);
        assert!(chunk.set_block_state(0, -60, 0, stone()));
        world.save_chunk(&chunk, Compression::Zlib).unwrap();
    }

    let left = BlockPos::new(0, -60, 0);
    let right = BlockPos::new(128, -60, 0);
    world.tick_state_mut().scheduled.schedule_after(
        0,
        left,
        "minecraft:stone",
        1,
        Default::default(),
    );
    world.tick_state_mut().scheduled.schedule_after(
        0,
        right,
        "minecraft:stone",
        1,
        Default::default(),
    );
    world
        .tick_state_mut()
        .neighbors
        .enqueue(left.above(), left, UpdateFlags::ALL);
    world
        .tick_state_mut()
        .neighbors
        .enqueue(right.above(), right, UpdateFlags::ALL);
    world.tick_state_mut().random.random_tick_speed = 32;

    let table = BehaviorTable::vanilla()
        .with("minecraft:stone", &COUNTING)
        .with("minecraft:air", &COUNTING);

    world
        .tick_region_with_table(&map, key(0, 0), 1, &table)
        .unwrap();

    assert_eq!(
        SCHEDULED_HITS.load(Ordering::SeqCst),
        1,
        "left scheduled tick"
    );
    assert!(RANDOM_HITS.load(Ordering::SeqCst) > 0, "left random ticks");
    assert_eq!(
        world.tick_state().neighbors.len(),
        1,
        "the right region's neighbour update is queued for its own tick"
    );
    assert!(
        world
            .tick_state()
            .scheduled
            .has_scheduled_tick(right, "minecraft:stone"),
        "the right region's scheduled tick is still queued"
    );
    assert_eq!(world.tick_state().last_neighbor_updates.len(), 1);
    assert_eq!(world.tick_state().last_neighbor_updates[0].pos, left);

    let left_random = RANDOM_HITS.load(Ordering::SeqCst);
    world
        .tick_region_with_table(&map, key(1, 0), 1, &table)
        .unwrap();
    assert_eq!(
        SCHEDULED_HITS.load(Ordering::SeqCst),
        2,
        "right scheduled tick"
    );
    assert!(
        RANDOM_HITS.load(Ordering::SeqCst) > left_random,
        "right random ticks run on their own region tick"
    );
    assert!(world.tick_state().neighbors.is_empty());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn shutdown_flush_persists_terrain_after_a_region_tick() {
    let root = temp_dir("shutdown-flush");
    let shared = shared_overworld(&root);
    {
        let mut world = lock(shared);
        world
            .save_chunk(&flat_chunk(3, 4), Compression::Zlib)
            .unwrap();
        let mut driver = RegionTickDriver::vanilla();
        driver.tick(&mut world, 1).unwrap();
    }
    flush_shared().unwrap();

    let mut reopened = WorldStorage::for_dimension(&root, Dimension::Overworld);
    let chunk = reopened
        .load_chunk(3, 4)
        .unwrap()
        .expect("region tick + shutdown flush wrote the cached chunk");
    assert_eq!((chunk.x, chunk.z), (3, 4));

    std::fs::remove_dir_all(root).ok();
}
