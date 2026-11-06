//! Region ownership: per-region queues/world slices, scheduler semantics,
//! cross-region rejection, and deterministic ordering.
//!
//! These tests pin the Folia region model in `folia_core::region`:
//! a task only runs when its owning region ticks, a task that touches
//! another region's state fails with `OwnershipError`, and chunk ->
//! region math (including negative coordinates) matches vanilla's
//! 8x8 chunk regions / 4x4 section regions.

use folia_core::region::{
    BlockPos, ChunkPos, OwnedWorld, OwnershipError, Region, RegionKey, RegionMap, RegionOwned,
    RegionScheduler, RegionWorldSlice, SectionPos, ThreadingDetector, WorldSliceToken,
    chunk_region_key, current_region, enter_region, region_id, region_key_from_id,
    section_region_key,
};
use std::sync::{Arc, Mutex};

fn key(x: i32, z: i32) -> RegionKey {
    RegionKey::new(x, z)
}

// ---------------------------------------------------------------------------
// Region id math, including negatives
// ---------------------------------------------------------------------------

#[test]
fn chunk_region_math_handles_negative_coordinates() {
    // Grid 8: floor division, so -1 and -8 share region -1, -9 is -2.
    let cases = [
        (ChunkPos::new(0, 0), key(0, 0)),
        (ChunkPos::new(7, 7), key(0, 0)),
        (ChunkPos::new(8, 0), key(1, 0)),
        (ChunkPos::new(-1, -1), key(-1, -1)),
        (ChunkPos::new(-8, -8), key(-1, -1)),
        (ChunkPos::new(-9, 0), key(-2, 0)),
        (ChunkPos::new(-16, 15), key(-2, 1)),
        (ChunkPos::new(-17, -17), key(-3, -3)),
    ];
    for (chunk, expected) in cases {
        assert_eq!(chunk.region_key(8), expected, "chunk {chunk:?}");
        assert_eq!(chunk_region_key(chunk, 8), expected, "chunk {chunk:?}");
        assert_eq!(RegionMap::vanilla().region_for_chunk(chunk), expected);
    }
    // Grid 4 (as used by some schedulers) follows the same rule.
    assert_eq!(ChunkPos::new(-1, 3).region_key(4), key(-1, 0));
    assert_eq!(ChunkPos::new(-4, 4).region_key(4), key(-1, 1));
    assert_eq!(ChunkPos::new(-5, 0).region_key(4), key(-2, 0));
}

#[test]
fn section_region_math_uses_vanilla_4x4_regions() {
    let cases = [
        (SectionPos::new(0, 0), key(0, 0)),
        (SectionPos::new(3, 3), key(0, 0)),
        (SectionPos::new(4, 0), key(1, 0)),
        (SectionPos::new(-1, -1), key(-1, -1)),
        (SectionPos::new(-4, -4), key(-1, -1)),
        (SectionPos::new(-5, 7), key(-2, 1)),
    ];
    for (section, expected) in cases {
        assert_eq!(section.region_key(), expected, "section {section:?}");
        assert_eq!(
            section_region_key(section.x, section.z),
            expected,
            "section {section:?}"
        );
        assert_eq!(
            RegionMap::vanilla().region_for_section(section),
            expected,
            "section {section:?}"
        );
        assert_eq!(
            RegionMap::vanilla().section_region_for_chunk(section.chunk_pos()),
            expected,
            "section {section:?}"
        );
    }
}

#[test]
fn block_region_math_uses_floor_division() {
    let map = RegionMap::vanilla();
    // Block -1 is in chunk -1, which is in region -1 (grid 8).
    assert_eq!(map.region_for_block(-1, -1), key(-1, -1));
    assert_eq!(map.region_for_block(0, 0), key(0, 0));
    assert_eq!(map.region_for_block(127, 127), key(0, 0));
    assert_eq!(map.region_for_block(128, 0), key(1, 0));
    assert_eq!(map.region_for_block(-128, -128), key(-1, -1));
    assert_eq!(map.region_for_block(-129, 0), key(-2, 0));
    assert_eq!(
        map.region_for_block_pos(BlockPos::new(-1, 64, -1)),
        key(-1, -1)
    );
    assert_eq!(BlockPos::new(-1, 64, -1).chunk_pos(), ChunkPos::new(-1, -1));
    assert_eq!(
        BlockPos::new(-1, 64, -1).section_pos(),
        SectionPos::new(-1, -1)
    );
}

#[test]
fn region_id_is_a_deterministic_bijection() {
    let keys = [
        key(0, 0),
        key(-1, -1),
        key(-2, 3),
        key(3, -2),
        key(i32::MIN, i32::MAX),
        key(i32::MAX, i32::MIN),
    ];
    for k in keys {
        assert_eq!(region_key_from_id(region_id(k)), k, "key {k:?}");
        assert_eq!(region_id(k), region_id(k), "key {k:?}");
    }
    // Distinct keys get distinct ids.
    let mut ids: Vec<u64> = keys.iter().map(|k| region_id(*k)).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), keys.len());
    assert_ne!(region_id(key(1, 0)), region_id(key(0, 1)));
    assert_eq!(
        RegionMap::vanilla().region_key_from_id(region_id(key(-7, 9))),
        key(-7, 9)
    );
}

#[test]
fn region_map_bounds_and_grouping_are_deterministic() {
    let map = RegionMap::vanilla();
    assert_eq!(
        map.chunk_bounds(key(0, 0)),
        Some((ChunkPos::new(0, 0), ChunkPos::new(7, 7)))
    );
    assert_eq!(
        map.chunk_bounds(key(-1, -1)),
        Some((ChunkPos::new(-8, -8), ChunkPos::new(-1, -1)))
    );
    assert_eq!(
        map.section_bounds(key(-1, 0)),
        Some((SectionPos::new(-4, 0), SectionPos::new(-1, 3)))
    );
    assert!(map.contains_chunk(key(-1, -1), ChunkPos::new(-3, -5)));
    assert!(!map.contains_chunk(key(0, 0), ChunkPos::new(-3, -5)));
    assert!(map.contains_section(key(-1, 0), SectionPos::new(-1, 3)));
    // Bounds that would overflow chunk coordinates report None.
    assert_eq!(map.chunk_bounds(key(i32::MAX, 0)), None);

    // Grouping sorts keys and chunks, regardless of input order.
    let chunks = [
        ChunkPos::new(8, 0),
        ChunkPos::new(0, 0),
        ChunkPos::new(-2, -2),
        ChunkPos::new(1, 1),
        ChunkPos::new(9, 9),
    ];
    let grouped = map.group_chunks(chunks);
    let keys: Vec<RegionKey> = grouped.keys().copied().collect();
    assert_eq!(keys, [key(-1, -1), key(0, 0), key(1, 0), key(1, 1)]);
    assert_eq!(
        grouped[&key(0, 0)],
        [ChunkPos::new(0, 0), ChunkPos::new(1, 1)]
    );
    assert_eq!(grouped[&key(-1, -1)], [ChunkPos::new(-2, -2)]);
}

// ---------------------------------------------------------------------------
// Tasks only run on their owning region's tick
// ---------------------------------------------------------------------------

#[test]
fn task_only_runs_when_its_owner_ticks() {
    let mut scheduler = RegionScheduler::new(8);
    let log = Arc::new(Mutex::new(Vec::new()));
    {
        let log = Arc::clone(&log);
        scheduler.schedule_at(ChunkPos::new(1, 1), 0, 0, move || {
            log.lock().unwrap().push("left");
        });
    }
    {
        let log = Arc::clone(&log);
        scheduler.schedule_at(ChunkPos::new(9, 9), 0, 0, move || {
            log.lock().unwrap().push("right");
        });
    }

    assert_eq!(scheduler.tick_region(key(0, 0)), 1);
    assert_eq!(*log.lock().unwrap(), ["left"]);
    // Ticking the other region does not re-run the first task.
    assert_eq!(scheduler.tick_region(key(0, 0)), 0);
    assert_eq!(*log.lock().unwrap(), ["left"]);
    assert_eq!(scheduler.tick_region(key(1, 1)), 1);
    assert_eq!(*log.lock().unwrap(), ["left", "right"]);
    // A region that was never scheduled has no queue to tick.
    assert_eq!(scheduler.tick_region(key(5, 5)), 0);
}

#[test]
fn run_for_region_schedules_by_key_not_chunk() {
    let mut scheduler = RegionScheduler::vanilla();
    assert_eq!(scheduler.grid_size(), 8);
    let count = Arc::new(Mutex::new(0));
    {
        let count = Arc::clone(&count);
        scheduler.run_for_region(key(-1, 2), 0, 0, move || {
            *count.lock().unwrap() += 1;
        });
    }
    assert_eq!(scheduler.tick_region(key(0, 0)), 0);
    assert_eq!(*count.lock().unwrap(), 0);
    assert_eq!(scheduler.tick_region(key(-1, 2)), 1);
    assert_eq!(*count.lock().unwrap(), 1);
    assert_eq!(scheduler.region_ticks(key(-1, 2)), Some(1));
}

#[test]
fn region_owns_its_world_slice_and_tick_counter() {
    let mut region = Region::new(key(2, -3));
    assert_eq!(region.key(), key(2, -3));
    assert_eq!(region.ticks(), 0);
    assert_eq!(region.world_token(), WorldSliceToken::new(key(2, -3)));
    assert_eq!(region.world().region(), key(2, -3));

    region.world_mut().set_block(BlockPos::new(1, 2, 3), 7);
    assert_eq!(region.world().get_block(BlockPos::new(1, 2, 3)), Some(7));
    assert_eq!(region.world().len(), 1);

    let ran = Arc::new(Mutex::new(Vec::new()));
    {
        let ran = Arc::clone(&ran);
        region.schedule_owned(0, 0, move |world| {
            world.set_block(BlockPos::new(4, 5, 6), 9);
            ran.lock().unwrap().push(world.owner());
            Ok(())
        });
    }
    let outcome = region.tick();
    assert!(outcome.is_ok());
    assert_eq!(outcome.executed, 1);
    assert_eq!(region.ticks(), 1);
    assert_eq!(region.world().get_block(BlockPos::new(4, 5, 6)), Some(9));
    assert_eq!(*ran.lock().unwrap(), [key(2, -3)]);
}

// ---------------------------------------------------------------------------
// Ownership errors on cross-region access
// ---------------------------------------------------------------------------

#[test]
fn owned_task_touching_another_region_fails_with_ownership_error() {
    let mut scheduler = RegionScheduler::new(8);
    let other = key(1, 1);
    scheduler.schedule_owned_at(ChunkPos::new(0, 0), 0, 0, move |world| {
        world.touch(other)?;
        Ok(())
    });

    let error = scheduler
        .tick_region_checked(key(0, 0))
        .expect_err("cross-region touch must fail");
    assert_eq!(error, OwnershipError::cross_region(key(0, 0), other));
    assert_eq!(
        scheduler.last_ownership_error(),
        Some(OwnershipError::cross_region(key(0, 0), other))
    );
    // The tick counter still advanced; the one-shot task was dropped.
    assert_eq!(scheduler.region_ticks(key(0, 0)), Some(1));
    assert_eq!(scheduler.task_count(), 0);
}

#[test]
fn owned_task_accessing_another_regions_owned_value_fails() {
    let mut scheduler = RegionScheduler::new(8);
    let foreign = RegionOwned::new(key(1, 0), 42);
    scheduler.schedule_owned_at(ChunkPos::new(0, 0), 0, 0, move |world| {
        let value = world.attempt_access(&foreign)?;
        let _ = value;
        Ok(())
    });

    let error = scheduler
        .tick_region_checked(key(0, 0))
        .expect_err("accessing another region's value must fail");
    assert_eq!(error.expected, key(0, 0));
    assert_eq!(error.actual, Some(key(1, 0)));
}

#[test]
fn owned_task_can_access_its_own_world_and_values() {
    let mut scheduler = RegionScheduler::new(8);
    let own = RegionOwned::new(key(0, 0), 7);
    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = Arc::clone(&seen);
        scheduler.schedule_owned_at(ChunkPos::new(0, 0), 0, 0, move |world| {
            world.touch(key(0, 0))?;
            world.set_block(BlockPos::new(0, 0, 0), 3);
            assert_eq!(world.get_block(BlockPos::new(0, 0, 0)), Some(3));
            let value = *world.attempt_access(&own)?;
            seen.lock().unwrap().push(value);
            // The ticking thread is the owning region, so the
            // thread-checked accessor agrees.
            seen.lock().unwrap().push(*own.get()?);
            Ok(())
        });
    }
    assert_eq!(scheduler.tick_region_checked(key(0, 0)), Ok(1));
    assert_eq!(*seen.lock().unwrap(), [7, 7]);
    assert_eq!(
        scheduler
            .region_world_mut(key(0, 0))
            .and_then(|slice| slice.get_block(BlockPos::new(0, 0, 0))),
        Some(3)
    );
}

#[test]
fn region_owned_rejects_wrong_thread_region_and_no_region() {
    let owned = RegionOwned::new(key(0, 0), 99);
    // No active region on this thread.
    assert!(owned.get().is_err());
    assert_eq!(owned.get().unwrap_err().expected, key(0, 0));
    // Explicit caller checks.
    assert_eq!(owned.with_caller(key(0, 0), |v| *v), Ok(99));
    assert!(owned.with_caller(key(1, 0), |v| *v).is_err());
    assert_eq!(
        owned.with_caller(key(1, 0), |v| *v).unwrap_err(),
        OwnershipError::cross_region(key(0, 0), key(1, 0))
    );

    // Inside the owning region's tick the accessor succeeds; inside
    // another region's "tick" it fails.
    {
        let _guard = enter_region(key(0, 0));
        assert_eq!(current_region(), Some(key(0, 0)));
        assert_eq!(owned.get(), Ok(&99));
    }
    {
        let _guard = enter_region(key(1, 0));
        assert_eq!(
            owned.get().unwrap_err(),
            OwnershipError::cross_region(key(0, 0), key(1, 0))
        );
    }
    assert_eq!(current_region(), None);
}

#[test]
fn threading_detector_rejects_another_thread() {
    let detector = ThreadingDetector::capture(key(0, 0));
    {
        let _guard = enter_region(key(0, 0));
        assert_eq!(detector.verify(), Ok(()));
    }

    let result = std::thread::scope(|scope| {
        scope
            .spawn(|| detector.verify())
            .join()
            .expect("scoped thread must not panic")
    });
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().expected, key(0, 0));
}

#[test]
fn owned_world_and_token_reject_mismatched_slices() {
    let mut slice = RegionWorldSlice::new(key(1, 0));
    let error = OwnedWorld::new(key(0, 0), &mut slice).unwrap_err();
    assert_eq!(error, OwnershipError::cross_region(key(0, 0), key(1, 0)));

    let token = WorldSliceToken::new(key(0, 0));
    assert!(token.matches(key(0, 0)));
    assert_eq!(token.check(key(0, 0)), Ok(()));
    assert_eq!(
        token.check(key(0, 1)),
        Err(OwnershipError::cross_region(key(0, 0), key(0, 1)))
    );
}

// ---------------------------------------------------------------------------
// Delay, period, cancellation (existing semantics preserved)
// ---------------------------------------------------------------------------

#[test]
fn delay_period_and_cancellation_follow_the_owner_queue() {
    let mut scheduler = RegionScheduler::new(8);
    let count = Arc::new(Mutex::new(0));
    let id = {
        let count = Arc::clone(&count);
        scheduler.run_for_region(key(0, 0), 1, 2, move || {
            *count.lock().unwrap() += 1;
        })
    };
    // Ticking a different region neither decrements the delay nor runs it.
    scheduler.tick_region(key(1, 1));
    assert_eq!(*count.lock().unwrap(), 0);

    scheduler.tick_region(key(0, 0)); // delay 1 -> 0
    assert_eq!(*count.lock().unwrap(), 0);
    scheduler.tick_region(key(0, 0)); // run
    assert_eq!(*count.lock().unwrap(), 1);
    scheduler.tick_region(key(0, 0)); // delay 1 -> 0
    assert_eq!(*count.lock().unwrap(), 1);
    scheduler.tick_region(key(0, 0)); // run, exactly `period` ticks later
    assert_eq!(*count.lock().unwrap(), 2);

    assert!(scheduler.cancel(id));
    assert!(!scheduler.cancel(id + 1000));
    scheduler.tick_region(key(0, 0));
    assert_eq!(*count.lock().unwrap(), 2);
    assert_eq!(scheduler.task_count(), 0);
}

#[test]
fn cancel_region_only_cancels_that_region() {
    let mut scheduler = RegionScheduler::new(8);
    let left = Arc::new(Mutex::new(0));
    let right = Arc::new(Mutex::new(0));
    {
        let left = Arc::clone(&left);
        scheduler.schedule_at(ChunkPos::new(0, 0), 0, 1, move || {
            *left.lock().unwrap() += 1;
        });
    }
    {
        let right = Arc::clone(&right);
        scheduler.schedule_at(ChunkPos::new(8, 0), 0, 1, move || {
            *right.lock().unwrap() += 1;
        });
    }
    scheduler.cancel_region(key(0, 0));
    assert_eq!(scheduler.tick_region(key(0, 0)), 0);
    assert_eq!(scheduler.tick_region(key(1, 0)), 1);
    assert_eq!(*left.lock().unwrap(), 0);
    assert_eq!(*right.lock().unwrap(), 1);
    assert_eq!(scheduler.task_count(), 1);
}

// ---------------------------------------------------------------------------
// Deterministic ordering across regions
// ---------------------------------------------------------------------------

#[test]
fn tick_all_runs_regions_in_sorted_key_order_deterministically() {
    // Schedule in non-sorted region order; `tick()` must still execute
    // in sorted (BTreeMap) region order.
    let mut scheduler = RegionScheduler::new(8);
    let log = Arc::new(Mutex::new(Vec::new()));
    for region_key in [key(2, 0), key(-1, 0), key(0, 1), key(0, -1)] {
        let log = Arc::clone(&log);
        scheduler.run_for_region(region_key, 0, 0, move || {
            log.lock().unwrap().push(region_key);
        });
    }
    assert_eq!(scheduler.tick(), 4);
    let expected = [key(-1, 0), key(0, -1), key(0, 1), key(2, 0)];
    assert_eq!(*log.lock().unwrap(), expected);

    // Repeating the same schedule on a fresh scheduler gives the same
    // order.
    let mut again = RegionScheduler::new(8);
    let log2 = Arc::new(Mutex::new(Vec::new()));
    for region_key in [key(0, -1), key(2, 0), key(-1, 0), key(0, 1)] {
        let log = Arc::clone(&log2);
        again.run_for_region(region_key, 0, 0, move || {
            log.lock().unwrap().push(region_key);
        });
    }
    assert_eq!(again.tick(), 4);
    assert_eq!(*log2.lock().unwrap(), expected);
}

#[test]
fn within_a_region_tasks_run_in_insertion_order() {
    let mut scheduler = RegionScheduler::new(8);
    let log = Arc::new(Mutex::new(Vec::new()));
    for token in [3, 1, 2] {
        let log = Arc::clone(&log);
        scheduler.schedule_at(ChunkPos::new(token, 7), 0, 0, move || {
            log.lock().unwrap().push(token);
        });
    }
    assert_eq!(scheduler.tick_region(key(0, 0)), 3);
    assert_eq!(*log.lock().unwrap(), [3, 1, 2]);
}
