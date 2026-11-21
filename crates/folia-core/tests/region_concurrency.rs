//! Deterministic concurrency stress suite for region scheduling and ownership.
//!
//! These tests exercise the public `folia_core::region` API with threads,
//! barriers, channels/atomics and `Arc<Mutex<_>>` only — no sleeps and no new
//! dependencies — so they are safe to run in CI. They cover:
//!
//! * concurrent scheduling keeping every task exactly once,
//! * a legitimate cross-region state handoff with no `OwnershipError`,
//! * repeated out-of-region `OwnedWorld` access being rejected under stress,
//! * stable per-region execution order when threads race to schedule, and
//! * stable `RegionMap` grouping under shuffled, repeated rebuilds.
//!
//! API limitation (P10.9): `RegionScheduler` is single-threaded at the
//! scheduler level and every schedule API takes `&mut self`, so a running task
//! cannot re-enter the scheduler to enqueue work in another region. The
//! cross-region transfer test therefore models Folia's queue handoff the way
//! the host loop does it: the source region task publishes the transfer, and
//! the host schedules the destination region task between ticks.

use folia_core::region::{
    BlockPos, ChunkPos, OwnedWorld, RegionKey, RegionMap, RegionOwned, RegionTickDriver,
    RegionWorldSlice, TaskId, enter_region,
};
use folia_core::world::{Dimension, WorldStorage};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn key(x: i32, z: i32) -> RegionKey {
    RegionKey::new(x, z)
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-region-concurrency-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage() -> WorldStorage {
    WorldStorage::new(Dimension::Overworld.region_dir(temp_dir("stress")))
}

/// Deterministic in-place shuffle (xorshift64) so the suite needs no `rand`
/// dependency and never relies on wall-clock timing.
fn shuffled(mut values: Vec<ChunkPos>, seed: u64) -> Vec<ChunkPos> {
    let mut state = seed | 1;
    for index in (1..values.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let swap = (state as usize) % (index + 1);
        values.swap(index, swap);
    }
    values
}

// ---------------------------------------------------------------------------
// 1. Concurrent scheduling keeps every task, exactly once
// ---------------------------------------------------------------------------

#[test]
fn concurrent_scheduling_preserves_all_tasks() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 100;
    const TOTAL: usize = THREADS * PER_THREAD;

    let driver = Arc::new(Mutex::new(RegionTickDriver::vanilla()));
    let executed: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::with_capacity(TOTAL)));
    let start = Arc::new(Barrier::new(THREADS));

    let mut handles = Vec::with_capacity(THREADS);
    for thread_index in 0..THREADS {
        let driver = Arc::clone(&driver);
        let executed = Arc::clone(&executed);
        let start = Arc::clone(&start);
        handles.push(std::thread::spawn(move || {
            start.wait();
            for task_index in 0..PER_THREAD {
                let label = thread_index * PER_THREAD + task_index;
                let region = (thread_index + task_index) % 8;
                let chunk = ChunkPos::new((region * 8 + task_index % 8) as i32, 0);
                let executed = Arc::clone(&executed);
                driver.lock().unwrap().schedule_at(chunk, 0, 0, move || {
                    executed.lock().unwrap().push(label);
                });
            }
        }));
    }
    for handle in handles {
        handle.join().expect("scheduling thread must not panic");
    }

    assert_eq!(driver.lock().unwrap().task_count(), TOTAL);

    let mut world = storage();
    let mut ticked = 0;
    let mut ticks = 0u64;
    for _ in 0..8 {
        ticks += 1;
        let report = driver
            .lock()
            .unwrap()
            .tick(&mut world, ticks)
            .expect("no ownership errors");
        ticked += report.tasks;
        if executed.lock().unwrap().len() == TOTAL {
            break;
        }
    }

    let executed = executed.lock().unwrap();
    assert_eq!(ticked, TOTAL, "every scheduled task must run once");
    assert_eq!(executed.len(), TOTAL);
    let unique: HashSet<usize> = executed.iter().copied().collect();
    assert_eq!(unique.len(), TOTAL, "no task may run twice");
    assert_eq!(
        unique,
        (0..TOTAL).collect::<HashSet<_>>(),
        "all thread/task labels must be present"
    );
    drop(executed);
    assert_eq!(driver.lock().unwrap().task_count(), 0);
}

// ---------------------------------------------------------------------------
// 2. Cross-region task transfer reports no ownership error
// ---------------------------------------------------------------------------

#[test]
fn cross_region_task_transfer_reports_no_ownership_error() {
    let source = key(0, 0);
    let destination = key(1, 0);
    let mut driver = RegionTickDriver::vanilla();
    let mut world = storage();

    // An i32 owned by the source region. The source task removes it (taking it
    // out of the Option) only after validating access with the owning region.
    let owned: Arc<Mutex<Option<RegionOwned<i32>>>> =
        Arc::new(Mutex::new(Some(RegionOwned::new(source, 41))));
    let transferred: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));

    {
        let owned = Arc::clone(&owned);
        let transferred = Arc::clone(&transferred);
        driver.schedule_owned_for_region(source, 0, 0, move |_owned_world| {
            let value = owned
                .lock()
                .unwrap()
                .take()
                .expect("source owns the value at the start");
            let checked = value.with_caller(source, |value| *value)?;
            assert!(value.with_caller(destination, |value| *value).is_err());
            *transferred.lock().unwrap() = Some(checked);
            Ok(())
        });
    }

    let first = driver.tick(&mut world, 1).expect("source task is valid");
    assert_eq!(first.tasks, 1);
    assert!(
        owned.lock().unwrap().is_none(),
        "value left the source region"
    );
    assert_eq!(*transferred.lock().unwrap(), Some(41));
    assert!(driver.scheduler().last_ownership_error().is_none());

    // The host posts equivalent work to the destination region's queue
    // (Folia's cross-region handoff; see the module-level limitation note).
    let installed: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));
    {
        let transferred = Arc::clone(&transferred);
        let installed = Arc::clone(&installed);
        driver.schedule_owned_for_region(destination, 0, 0, move |owned_world| {
            let value = transferred
                .lock()
                .unwrap()
                .expect("source published the transfer");
            let reowned = RegionOwned::new(destination, value + 1);
            let checked = reowned.with_caller(destination, |value| *value)?;
            assert!(reowned.with_caller(source, |value| *value).is_err());
            let seen = owned_world.attempt_access(&reowned)?;
            assert_eq!(*seen, checked);
            *installed.lock().unwrap() = Some(*seen);
            Ok(())
        });
    }

    let second = driver
        .tick(&mut world, 2)
        .expect("destination task is valid");
    assert_eq!(second.tasks, 1);
    assert_eq!(*installed.lock().unwrap(), Some(42));
    assert_eq!(driver.scheduler().last_ownership_error(), None);
}

// ---------------------------------------------------------------------------
// 3. OwnedWorld rejects out-of-region access under stress
// ---------------------------------------------------------------------------

#[test]
fn owned_world_rejects_cross_region_access_under_stress() {
    const THREADS: usize = 8;
    const ATTEMPTS_PER_THREAD: usize = 512;
    const TOTAL: usize = THREADS * ATTEMPTS_PER_THREAD;

    let owner = key(0, 0);
    let foreign_region = key(1, 0);

    let mut slice = RegionWorldSlice::new(owner);
    assert_eq!(slice.set_block(BlockPos::new(1, 0, 1), 7), None);
    let owned_world = OwnedWorld::new(owner, &mut slice).expect("slice belongs to owner");
    let world = Arc::new(Mutex::new(owned_world));

    // A value owned by a different region; failed access must not mutate it.
    let foreign: Arc<Mutex<RegionOwned<i32>>> =
        Arc::new(Mutex::new(RegionOwned::new(foreign_region, 99)));

    let attempts = Arc::new(AtomicUsize::new(0));
    let rejected = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(THREADS));

    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            let world = &world;
            let foreign = &foreign;
            let attempts = &attempts;
            let rejected = &rejected;
            let start = &start;
            scope.spawn(move || {
                let _guard = enter_region(owner);
                start.wait();
                for _ in 0..ATTEMPTS_PER_THREAD {
                    attempts.fetch_add(1, Ordering::Relaxed);

                    let mut world = world.lock().unwrap();
                    let mut foreign = foreign.lock().unwrap();

                    let mut failures = 0;
                    if world.touch(foreign_region).is_err() {
                        failures += 1;
                    }
                    if world.require_region(foreign_region).is_err() {
                        failures += 1;
                    }
                    if world.check_caller(foreign_region).is_err() {
                        failures += 1;
                    }
                    if world.attempt_access_mut(&mut *foreign).is_err() {
                        failures += 1;
                    }
                    // The thread ticks `owner`, so a `foreign_region`-owned
                    // value must also reject the thread-local ownership check.
                    if foreign.get().is_err() {
                        failures += 1;
                    }

                    assert_eq!(failures, 5, "every cross-region access must be rejected");
                    rejected.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });

    assert_eq!(attempts.load(Ordering::Relaxed), TOTAL);
    assert_eq!(rejected.load(Ordering::Relaxed), TOTAL);

    // No failed attempt mutated either the owned world or the foreign value.
    {
        let world = world.lock().unwrap();
        assert_eq!(world.touch(owner), Ok(()));
        assert_eq!(world.slice().len(), 1);
        assert_eq!(world.get_block(BlockPos::new(1, 0, 1)), Some(7));
    }
    assert_eq!(
        foreign.lock().unwrap().with_caller(foreign_region, |v| *v),
        Ok(99)
    );
}

// ---------------------------------------------------------------------------
// 4. Task ordering is stable under concurrent scheduling
// ---------------------------------------------------------------------------

#[test]
fn task_ordering_is_stable_under_concurrent_schedule() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 25;
    const TOTAL: usize = THREADS * PER_THREAD;
    const REGIONS: usize = 4;

    let driver = Arc::new(Mutex::new(RegionTickDriver::vanilla()));
    let executed: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::with_capacity(TOTAL)));
    let id_of: Arc<Mutex<HashMap<usize, TaskId>>> = Arc::new(Mutex::new(HashMap::new()));
    let region_of: Arc<Mutex<HashMap<usize, RegionKey>>> = Arc::new(Mutex::new(HashMap::new()));
    let next_label = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(THREADS));

    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let driver = Arc::clone(&driver);
        let executed = Arc::clone(&executed);
        let id_of = Arc::clone(&id_of);
        let region_of = Arc::clone(&region_of);
        let next_label = Arc::clone(&next_label);
        let start = Arc::clone(&start);
        handles.push(std::thread::spawn(move || {
            start.wait();
            for _ in 0..PER_THREAD {
                let label = next_label.fetch_add(1, Ordering::Relaxed);
                let region = key((label % REGIONS) as i32, 0);
                let executed = Arc::clone(&executed);
                let id = driver
                    .lock()
                    .unwrap()
                    .schedule_for_region(region, 0, 0, move || {
                        executed.lock().unwrap().push(label);
                    });
                id_of.lock().unwrap().insert(label, id);
                region_of.lock().unwrap().insert(label, region);
            }
        }));
    }
    for handle in handles {
        handle.join().expect("ordering thread must not panic");
    }

    let mut world = storage();
    let mut ticks = 0u64;
    for _ in 0..8 {
        ticks += 1;
        driver
            .lock()
            .unwrap()
            .tick(&mut world, ticks)
            .expect("no ownership errors");
        if executed.lock().unwrap().len() == TOTAL {
            break;
        }
    }

    let executed = executed.lock().unwrap();
    let id_of = id_of.lock().unwrap();
    let region_of = region_of.lock().unwrap();

    assert_eq!(executed.len(), TOTAL);
    let unique: HashSet<usize> = executed.iter().copied().collect();
    assert_eq!(unique.len(), TOTAL, "no task ran twice");

    let mut by_region: HashMap<RegionKey, Vec<TaskId>> = HashMap::new();
    for &label in executed.iter() {
        let region = region_of[&label];
        by_region.entry(region).or_default().push(id_of[&label]);
    }
    assert_eq!(by_region.len(), REGIONS);
    for (region, ids) in &by_region {
        assert_eq!(ids.len(), TOTAL / REGIONS);
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "region {region:?} executed out of TaskId order: {ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. RegionMap grouping is stable under repeated, shuffled rebuilds
// ---------------------------------------------------------------------------

#[test]
fn region_map_grouping_is_stable_under_repeated_rebuild() {
    const THREADS: usize = 8;
    const REBUILDS: u32 = 25;

    let map = RegionMap::vanilla();
    let chunks: Arc<Vec<ChunkPos>> = Arc::new(
        (-12..=12)
            .flat_map(|x| (-12..=12).map(move |z| ChunkPos::new(x, z)))
            .collect(),
    );
    let baseline = map.group_chunks(chunks.iter().copied());

    // Sequential shuffled rebuilds.
    for iteration in 0..REBUILDS {
        let shuffled = shuffled((*chunks).clone(), u64::from(iteration) + 1);
        assert_eq!(
            map.group_chunks(shuffled),
            baseline,
            "sequential rebuild {iteration} changed the grouping"
        );
    }

    // Concurrent shuffled rebuilds on independent RegionMap copies.
    let handles: Vec<_> = (0..THREADS)
        .map(|thread_index| {
            let chunks = Arc::clone(&chunks);
            let baseline = baseline.clone();
            std::thread::spawn(move || {
                for iteration in 0..REBUILDS {
                    let seed = (thread_index as u64 + 1) * 1_000_003 + u64::from(iteration);
                    let shuffled = shuffled((*chunks).clone(), seed);
                    assert_eq!(
                        map.group_chunks(shuffled),
                        baseline,
                        "thread {thread_index} rebuild {iteration} changed the grouping"
                    );
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("rebuild thread must not panic");
    }

    // The grouping itself is order-independent by construction.
    assert_eq!(map.group_chunks(chunks.iter().rev().copied()), baseline);
    assert!(
        baseline
            .values()
            .all(|group| { group.windows(2).all(|pair| pair[0] < pair[1]) })
    );
    // Sanity: negative chunks land in negative regions deterministically.
    assert_eq!(map.region_for_chunk(ChunkPos::new(-1, -9)), key(-1, -2));
}
