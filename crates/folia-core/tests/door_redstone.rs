//! Focused P3.10 tests for redstone-driven two-block door updates.
//!
//! A signal on either half opens and powers both halves, and losing that
//! signal closes and un-powers both halves, without partial or duplicate
//! state changes.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::{BlockPos, Direction};
use folia_core::redstone::{self, property};
use folia_core::registry::{self, default_block_state};
use folia_core::tick::{self, BehaviorTable};
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DOOR: &str = "minecraft:oak_door";

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-door-redstone-{}-{}-{safe}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn flat_storage(name: &str) -> WorldStorage {
    let root = temp_dir(name);
    let chunk: Chunk = flat_chunk(0, 0);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
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

fn settle(storage: &mut WorldStorage) {
    for tick_number in 1..=4 {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

fn redstone_block() -> BlockState {
    default_block_state("minecraft:redstone_block").expect("redstone block state")
}

fn air() -> BlockState {
    default_block_state("minecraft:air").expect("air state")
}

fn door_state(half: &str, hinge: &str, facing: &str, open: bool, powered: bool) -> BlockState {
    registry::state_with_properties(
        DOOR,
        &[
            ("facing", facing),
            ("half", half),
            ("hinge", hinge),
            ("open", if open { "true" } else { "false" }),
            ("powered", if powered { "true" } else { "false" }),
        ],
    )
    .expect("valid oak door state")
}

fn place_door(storage: &mut WorldStorage, lower: BlockPos, facing: &str, hinge: &str) {
    place(
        storage,
        lower,
        door_state("lower", hinge, facing, false, false),
    );
    place(
        storage,
        lower.above(),
        door_state("upper", hinge, facing, false, false),
    );
}

fn assert_half(
    storage: &WorldStorage,
    pos: BlockPos,
    half: &str,
    hinge: &str,
    facing: &str,
    open: bool,
    powered: bool,
) {
    let state = block(storage, pos);
    assert_eq!(state.name, DOOR, "half at {pos:?}");
    assert_eq!(property(&state, "half"), Some(half), "half at {pos:?}");
    assert_eq!(property(&state, "hinge"), Some(hinge), "hinge at {pos:?}");
    assert_eq!(
        property(&state, "facing"),
        Some(facing),
        "facing at {pos:?}"
    );
    assert_eq!(
        property(&state, "open"),
        Some(if open { "true" } else { "false" }),
        "open at {pos:?}"
    );
    assert_eq!(
        property(&state, "powered"),
        Some(if powered { "true" } else { "false" }),
        "powered at {pos:?}"
    );
}

fn assert_door(
    storage: &WorldStorage,
    lower: BlockPos,
    hinge: &str,
    facing: &str,
    open: bool,
    powered: bool,
) {
    assert_half(storage, lower, "lower", hinge, facing, open, powered);
    assert_half(
        storage,
        lower.above(),
        "upper",
        hinge,
        facing,
        open,
        powered,
    );
}

fn behavior_addr(behavior: &dyn tick::BlockBehavior) -> *const () {
    behavior as *const dyn tick::BlockBehavior as *const ()
}

#[test]
fn door_behavior_is_registered_in_the_production_table() {
    let state = door_state("lower", "left", "north", false, false);
    let door = behavior_addr(&redstone::DOOR_BEHAVIOR);
    let table = BehaviorTable::with_growth();
    assert_eq!(
        behavior_addr(table.get(&state)),
        door,
        "with_growth resolves the door behavior"
    );
    assert_eq!(
        behavior_addr(tick::behavior_for(&state)),
        door,
        "the built-in table resolves the door behavior"
    );
    for name in [
        "minecraft:oak_trapdoor",
        "minecraft:oak_fence_gate",
        "minecraft:stone",
    ] {
        assert_ne!(
            behavior_addr(table.get(&default_block_state(name).expect("block state"))),
            door,
            "{name} must not use the door behavior"
        );
    }
}

#[test]
fn single_block_door_still_opens_and_closes() {
    let mut storage = flat_storage("single-door");
    let pos = BlockPos::new(0, -60, 0);
    place(&mut storage, pos, default_block_state(DOOR).expect("door"));
    assert_eq!(property(&block(&storage, pos), "open"), Some("false"));

    place(
        &mut storage,
        pos.relative(Direction::East),
        redstone_block(),
    );
    settle(&mut storage);
    assert_eq!(property(&block(&storage, pos), "open"), Some("true"));
    assert_eq!(property(&block(&storage, pos), "powered"), Some("true"));

    place(&mut storage, pos.relative(Direction::East), air());
    settle(&mut storage);
    assert_eq!(property(&block(&storage, pos), "open"), Some("false"));
    assert_eq!(property(&block(&storage, pos), "powered"), Some("false"));
}

#[test]
fn signal_beside_lower_half_opens_both_halves() {
    let mut storage = flat_storage("door-lower-signal");
    let lower = BlockPos::new(0, -60, 0);
    place_door(&mut storage, lower, "west", "right");
    assert_door(&storage, lower, "right", "west", false, false);

    place(
        &mut storage,
        lower.relative(Direction::East),
        redstone_block(),
    );
    settle(&mut storage);
    assert_door(&storage, lower, "right", "west", true, true);
}

#[test]
fn signal_beside_upper_half_opens_both_halves() {
    let mut storage = flat_storage("door-upper-signal");
    let lower = BlockPos::new(2, -60, 2);
    place_door(&mut storage, lower, "north", "left");
    place(
        &mut storage,
        lower.above().relative(Direction::East),
        redstone_block(),
    );
    settle(&mut storage);
    assert_door(&storage, lower, "left", "north", true, true);
}

#[test]
fn signal_above_upper_half_opens_both_halves() {
    let mut storage = flat_storage("door-above-signal");
    let lower = BlockPos::new(4, -60, 4);
    place_door(&mut storage, lower, "south", "right");
    place(&mut storage, lower.above().above(), redstone_block());
    settle(&mut storage);
    assert_door(&storage, lower, "right", "south", true, true);
}

#[test]
fn falling_signal_closes_both_halves() {
    let mut storage = flat_storage("door-falling-signal");
    let lower = BlockPos::new(2, -60, 2);
    place_door(&mut storage, lower, "east", "left");
    let source = lower.relative(Direction::West);

    place(&mut storage, source, redstone_block());
    settle(&mut storage);
    assert_door(&storage, lower, "left", "east", true, true);

    place(&mut storage, source, air());
    settle(&mut storage);
    assert_door(&storage, lower, "left", "east", false, false);
}

#[test]
fn door_updates_are_deterministic_and_do_not_repeat() {
    let mut first = flat_storage("door-determinism-a");
    let mut second = flat_storage("door-determinism-b");
    let lower = BlockPos::new(2, -60, 2);
    for storage in [&mut first, &mut second] {
        place_door(storage, lower, "north", "right");
        place(
            storage,
            lower.above().relative(Direction::West),
            redstone_block(),
        );
        settle(storage);
    }
    for pos in [lower, lower.above()] {
        assert_eq!(block(&first, pos), block(&second, pos));
    }

    let report = tick::tick_world(&mut first, 20).unwrap();
    assert_eq!(report.block_changes, 0, "settled door produces no changes");
    for pos in [lower, lower.above()] {
        assert_eq!(block(&first, pos), block(&second, pos));
    }

    for storage in [&mut first, &mut second] {
        place(storage, lower.above().relative(Direction::West), air());
        settle(storage);
    }
    for pos in [lower, lower.above()] {
        assert_eq!(block(&first, pos), block(&second, pos));
    }
    assert_door(&first, lower, "right", "north", false, false);
}
