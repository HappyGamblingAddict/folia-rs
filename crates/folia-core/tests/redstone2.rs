//! Focused end-to-end tests for the second redstone slice: repeaters,
//! comparators, observers, torches, pressure plates, tripwire, and wire shape
//! updates. These drive the real `tick_world` loop over persisted chunks.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::{BlockPos, Direction};
use folia_core::redstone::{self, PlateOccupancy, SignalReader};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-redstone2-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
    storage
}

fn flat_storage(name: &str) -> WorldStorage {
    storage_with_chunk(name, flat_chunk(0, 0))
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

fn default_state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn run_ticks(storage: &mut WorldStorage, from: u64, to: u64) {
    for tick_number in from..=to {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

fn advance(storage: &mut WorldStorage, now: &mut u64, ticks: u64) {
    for _ in 0..ticks {
        *now += 1;
        tick::tick_world(storage, *now).unwrap();
    }
}

fn air() -> BlockState {
    default_state("minecraft:air")
}

fn stone() -> BlockState {
    default_state("minecraft:stone")
}

fn redstone_block() -> BlockState {
    default_state("minecraft:redstone_block")
}

fn torch_lit(storage: &WorldStorage, pos: BlockPos) -> bool {
    redstone::torch_lit(&block(storage, pos))
}

struct FakeContainer(i32);

impl redstone::ContainerSignalSource for FakeContainer {
    fn analog_output_signal(&self) -> i32 {
        self.0
    }
}

#[test]
fn repeater_honours_delay_and_output_direction() {
    for delay in 1..=4 {
        let mut storage = flat_storage(&format!("repeater-{delay}"));
        let repeater_pos = BlockPos::new(5, -60, 5);
        let input_pos = BlockPos::new(4, -60, 5);
        let output_pos = BlockPos::new(6, -60, 5);

        place(&mut storage, input_pos, default_state("minecraft:lever"));
        let repeater = redstone::with_facing(&default_state("minecraft:repeater"), Direction::West);
        let repeater = redstone::with_delay(&repeater, delay);
        place(&mut storage, repeater_pos, repeater);

        assert!(redstone::toggle_lever(&mut storage, input_pos).unwrap());
        let delay_ticks = (delay * 2) as u64;
        if delay_ticks > 1 {
            run_ticks(&mut storage, 1, delay_ticks - 1);
        }
        assert!(
            !redstone::is_powered(&block(&storage, repeater_pos)),
            "delay {delay} must not power before {delay_ticks} ticks"
        );
        run_ticks(&mut storage, delay_ticks, delay_ticks);
        assert!(
            redstone::is_powered(&block(&storage, repeater_pos)),
            "delay {delay} powers on tick {delay_ticks}"
        );

        let reader = SignalReader::world(&storage);
        // `facing` is the input side, so the output is emitted opposite it.
        assert_eq!(reader.signal(repeater_pos, Direction::West), 15);
        assert_eq!(reader.direct_signal(repeater_pos, Direction::West), 15);
        assert_eq!(reader.signal(repeater_pos, Direction::East), 0);
        assert_eq!(reader.signal(repeater_pos, Direction::North), 0);
        assert_eq!(reader.best_neighbor_signal(output_pos), 15);
        assert_eq!(reader.best_neighbor_signal(input_pos), 0);

        assert!(redstone::toggle_lever(&mut storage, input_pos).unwrap());
        run_ticks(&mut storage, delay_ticks + 1, delay_ticks * 2);
        assert!(
            !redstone::is_powered(&block(&storage, repeater_pos)),
            "delay {delay} also applies when turning off"
        );
    }
}

#[test]
fn locked_repeater_ignores_its_input() {
    let mut storage = flat_storage("repeater-locked");
    let repeater_pos = BlockPos::new(5, -60, 5);
    let lever_pos = BlockPos::new(5, -60, 4);
    let lock_pos = BlockPos::new(4, -60, 5);

    // A faces north: input north, output south, side inputs east/west.
    place(
        &mut storage,
        repeater_pos,
        default_state("minecraft:repeater"),
    );
    place(&mut storage, lever_pos, default_state("minecraft:lever"));
    // The locking repeater is powered permanently by the redstone block behind.
    let lock_input = BlockPos::new(3, -60, 5);
    place(&mut storage, lock_input, redstone_block());
    let lock = redstone::with_facing(&default_state("minecraft:repeater"), Direction::West);
    let lock = redstone::with_powered(&lock, true);
    place(&mut storage, lock_pos, lock);

    assert!(
        redstone::is_locked(&block(&storage, repeater_pos)),
        "a powered diode on the side locks the repeater"
    );

    assert!(redstone::toggle_lever(&mut storage, lever_pos).unwrap());
    run_ticks(&mut storage, 1, 10);
    assert!(
        !redstone::is_powered(&block(&storage, repeater_pos)),
        "a locked repeater ignores its powered input"
    );
    assert!(
        redstone::is_powered(&block(&storage, lock_pos)),
        "the locking repeater keeps its own input"
    );

    // Removing the locking diode unlocks the repeater. The pending input then
    // turns it on after the normal two-tick delay.
    place(&mut storage, lock_pos, air());
    assert!(!redstone::is_locked(&block(&storage, repeater_pos)));
    run_ticks(&mut storage, 11, 12);
    assert!(
        redstone::is_powered(&block(&storage, repeater_pos)),
        "the unlocked repeater accepts the pending input"
    );
}

/// Builds a lever-fed wire line and returns the position of its last segment.
fn powered_wire_line(storage: &mut WorldStorage, length: i32) -> (BlockPos, BlockPos) {
    let lever = BlockPos::new(0, -60, 3);
    place(storage, lever, default_state("minecraft:lever"));
    for x in 1..=length {
        place(storage, BlockPos::new(x, -60, 3), redstone::wire_state(0));
    }
    assert!(redstone::toggle_lever(storage, lever).unwrap());
    (lever, BlockPos::new(length, -60, 3))
}

#[test]
fn comparator_compares_and_subtracts_a_fake_container() {
    let mut storage = flat_storage("comparator-modes");
    let (_, last_wire) = powered_wire_line(&mut storage, 12);
    // A twelve-segment line leaves power 4 at the end.
    assert_eq!(redstone::wire_power(&block(&storage, last_wire)), 4);

    let comparator_pos = BlockPos::new(12, -60, 4);
    let container_pos = BlockPos::new(11, -60, 4);
    storage
        .tick_state_mut()
        .containers
        .insert(container_pos, Box::new(FakeContainer(9)));

    let comparator = redstone::with_facing(&default_state("minecraft:comparator"), Direction::West);
    place(&mut storage, comparator_pos, comparator);

    run_ticks(&mut storage, 1, 2);
    let reader = SignalReader::world(&storage);
    assert!(redstone::is_powered(&block(&storage, comparator_pos)));
    assert_eq!(reader.comparator_output(comparator_pos), 9);
    assert_eq!(reader.signal(comparator_pos, Direction::West), 9);
    assert_eq!(
        reader.best_neighbor_signal(BlockPos::new(13, -60, 4)),
        9,
        "the comparator strongly powers its output side"
    );

    // Cycle to subtract mode (rear 9 - side 4 = 5).
    assert!(redstone::use_block(&mut storage, comparator_pos).unwrap());
    let subtract = block(&storage, comparator_pos);
    assert_eq!(redstone::property(&subtract, "mode"), Some("subtract"));
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.comparator_output(comparator_pos), 5);
    assert_eq!(reader.signal(comparator_pos, Direction::West), 5);
    assert!(redstone::is_powered(&subtract));

    // Cycle back to compare mode.
    assert!(redstone::use_block(&mut storage, comparator_pos).unwrap());
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.comparator_output(comparator_pos), 9);
    assert_eq!(reader.signal(comparator_pos, Direction::West), 9);
}

#[test]
fn comparator_reevaluates_when_the_container_changes() {
    let mut storage = flat_storage("comparator-container-change");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let container_pos = BlockPos::new(4, -60, 5);
    storage
        .tick_state_mut()
        .containers
        .insert(container_pos, Box::new(FakeContainer(6)));
    let comparator = redstone::with_facing(&default_state("minecraft:comparator"), Direction::West);
    place(&mut storage, comparator_pos, comparator);

    run_ticks(&mut storage, 1, 2);
    assert!(redstone::is_powered(&block(&storage, comparator_pos)));
    assert_eq!(
        SignalReader::world(&storage).comparator_output(comparator_pos),
        6
    );

    storage
        .tick_state_mut()
        .containers
        .insert(container_pos, Box::new(FakeContainer(11)));
    redstone::notify_container_changed(&mut storage, container_pos);
    run_ticks(&mut storage, 3, 4);
    assert_eq!(
        SignalReader::world(&storage).comparator_output(comparator_pos),
        11
    );

    storage
        .tick_state_mut()
        .containers
        .insert(container_pos, Box::new(FakeContainer(0)));
    redstone::notify_container_changed(&mut storage, container_pos);
    run_ticks(&mut storage, 5, 6);
    assert!(!redstone::is_powered(&block(&storage, comparator_pos)));
    assert_eq!(
        SignalReader::world(&storage).comparator_output(comparator_pos),
        0
    );
}

#[test]
fn observer_emits_one_two_tick_pulse() {
    let mut storage = flat_storage("observer");
    let observer_pos = BlockPos::new(5, -60, 5);
    let front = BlockPos::new(5, -60, 4);
    let back = BlockPos::new(5, -60, 6);
    let observer = redstone::with_facing(&default_state("minecraft:observer"), Direction::North);
    place(&mut storage, observer_pos, observer);
    assert!(!redstone::is_powered(&block(&storage, observer_pos)));

    place(&mut storage, front, stone());
    run_ticks(&mut storage, 1, 1);
    assert!(
        !redstone::is_powered(&block(&storage, observer_pos)),
        "the observer waits its two-tick delay"
    );
    run_ticks(&mut storage, 2, 2);
    assert!(redstone::is_powered(&block(&storage, observer_pos)));
    let reader = SignalReader::world(&storage);
    assert_eq!(reader.signal(observer_pos, Direction::North), 15);
    assert_eq!(reader.best_neighbor_signal(back), 15);
    assert_eq!(reader.best_neighbor_signal(front), 0);

    run_ticks(&mut storage, 3, 4);
    assert!(
        !redstone::is_powered(&block(&storage, observer_pos)),
        "the pulse ends two ticks later"
    );
    run_ticks(&mut storage, 5, 12);
    assert!(
        !redstone::is_powered(&block(&storage, observer_pos)),
        "the observer does not retrigger itself"
    );
    assert!(storage.tick_state().scheduled.is_empty());
}

#[test]
fn torch_inverts_and_burns_out() {
    let mut storage = flat_storage("torch");
    let torch_pos = BlockPos::new(5, -60, 5);
    let support_pos = torch_pos.below();
    place(
        &mut storage,
        torch_pos,
        default_state("minecraft:redstone_torch"),
    );
    assert!(torch_lit(&storage, torch_pos));

    place(&mut storage, support_pos, redstone_block());
    run_ticks(&mut storage, 1, 2);
    assert!(
        !torch_lit(&storage, torch_pos),
        "a powered support turns the torch off"
    );

    place(&mut storage, support_pos, stone());
    run_ticks(&mut storage, 3, 4);
    assert!(
        torch_lit(&storage, torch_pos),
        "removing the signal turns the torch back on"
    );

    // Eight off toggles inside 60 ticks burn the torch out for 160 ticks.
    let mut now = 4;
    let mut off_toggles = 1;
    let burnout_tick;
    loop {
        place(&mut storage, support_pos, redstone_block());
        advance(&mut storage, &mut now, 2);
        off_toggles += 1;
        if off_toggles == redstone::REDSTONE_TORCH_MAX_TOGGLES as i32 {
            burnout_tick = now;
            break;
        }
        place(&mut storage, support_pos, stone());
        advance(&mut storage, &mut now, 2);
        assert!(
            torch_lit(&storage, torch_pos),
            "the torch relights before burn-out"
        );
    }
    assert!(!torch_lit(&storage, torch_pos));

    place(&mut storage, support_pos, stone());
    advance(&mut storage, &mut now, 60);
    assert!(
        !torch_lit(&storage, torch_pos),
        "a burnt-out torch stays off inside its 160-tick restart delay"
    );
    assert_eq!(now, burnout_tick + 60);

    while now < burnout_tick + redstone::REDSTONE_TORCH_RESTART_DELAY {
        advance(&mut storage, &mut now, 1);
    }
    assert!(
        torch_lit(&storage, torch_pos),
        "the torch restarts after 160 ticks"
    );
}

#[test]
fn wall_torch_requires_its_attachment() {
    let mut storage = flat_storage("wall-torch");
    let torch_pos = BlockPos::new(7, -60, 5);
    let support_pos = BlockPos::new(7, -60, 6);
    let torch = redstone::with_facing(
        &default_state("minecraft:redstone_wall_torch"),
        Direction::North,
    );
    place(&mut storage, support_pos, stone());
    place(&mut storage, torch_pos, torch);
    assert_eq!(
        block(&storage, torch_pos).name,
        "minecraft:redstone_wall_torch"
    );

    place(&mut storage, support_pos, air());
    run_ticks(&mut storage, 1, 2);
    assert_eq!(
        block(&storage, torch_pos).name,
        "minecraft:air",
        "removing the wall removes the torch"
    );
}

#[test]
fn pressure_plates_press_and_release_on_schedule() {
    let mut storage = flat_storage("pressure-plates");
    let stone_pos = BlockPos::new(5, -60, 5);
    let wood_pos = BlockPos::new(6, -60, 5);
    let light_pos = BlockPos::new(7, -60, 5);
    place(
        &mut storage,
        stone_pos,
        default_state("minecraft:stone_pressure_plate"),
    );
    place(
        &mut storage,
        wood_pos,
        default_state("minecraft:oak_pressure_plate"),
    );
    place(
        &mut storage,
        light_pos,
        default_state("minecraft:light_weighted_pressure_plate"),
    );

    // Stone plates only count living entities; wooden plates count everything.
    assert!(
        !redstone::set_pressure_plate_occupancy(
            &mut storage,
            stone_pos,
            PlateOccupancy {
                entities: 1,
                living: 0
            }
        )
        .unwrap()
    );
    assert!(!redstone::is_powered(&block(&storage, stone_pos)));
    assert!(
        redstone::set_pressure_plate_occupancy(
            &mut storage,
            wood_pos,
            PlateOccupancy {
                entities: 1,
                living: 0
            }
        )
        .unwrap()
    );
    assert!(redstone::is_powered(&block(&storage, wood_pos)));

    assert!(
        redstone::set_pressure_plate_occupancy(
            &mut storage,
            stone_pos,
            PlateOccupancy {
                entities: 1,
                living: 1
            }
        )
        .unwrap()
    );
    assert!(redstone::is_powered(&block(&storage, stone_pos)));
    assert_eq!(
        SignalReader::world(&storage).best_neighbor_signal(stone_pos.above()),
        15
    );

    // Weighted plates expose a 0..15 POWER property.
    assert!(
        redstone::set_pressure_plate_occupancy(
            &mut storage,
            light_pos,
            PlateOccupancy {
                entities: 5,
                living: 5
            }
        )
        .unwrap()
    );
    assert_eq!(
        redstone::pressure_plate_state_signal(&block(&storage, light_pos)),
        5
    );

    // Release the stone plate only on its next 20-tick recheck.
    storage.tick_state_mut().occupancy.insert(
        stone_pos,
        PlateOccupancy {
            entities: 0,
            living: 0,
        },
    );
    run_ticks(&mut storage, 1, 19);
    assert!(redstone::is_powered(&block(&storage, stone_pos)));
    run_ticks(&mut storage, 20, 20);
    assert!(!redstone::is_powered(&block(&storage, stone_pos)));

    // Weighted plates recheck after ten ticks.
    storage.tick_state_mut().occupancy.insert(
        light_pos,
        PlateOccupancy {
            entities: 0,
            living: 0,
        },
    );
    run_ticks(&mut storage, 21, 29);
    assert_eq!(
        redstone::pressure_plate_state_signal(&block(&storage, light_pos)),
        5
    );
    run_ticks(&mut storage, 30, 30);
    assert_eq!(
        redstone::pressure_plate_state_signal(&block(&storage, light_pos)),
        0
    );
}

#[test]
fn tripwire_connects_presses_and_releases() {
    let mut storage = flat_storage("tripwire");
    let first = BlockPos::new(5, -60, 5);
    let second = BlockPos::new(5, -60, 6);
    let tripwire = redstone::with_attached(&default_state("minecraft:tripwire"), true);
    place(&mut storage, first, tripwire.clone());
    place(&mut storage, second, tripwire);

    let first_state = block(&storage, first);
    let second_state = block(&storage, second);
    assert_eq!(redstone::property(&first_state, "south"), Some("true"));
    assert_eq!(redstone::property(&second_state, "north"), Some("true"));

    assert!(
        redstone::set_tripwire_occupancy(
            &mut storage,
            first,
            PlateOccupancy {
                entities: 1,
                living: 1
            }
        )
        .unwrap()
    );
    assert!(redstone::is_powered(&block(&storage, first)));

    storage.tick_state_mut().occupancy.insert(
        first,
        PlateOccupancy {
            entities: 0,
            living: 0,
        },
    );
    run_ticks(&mut storage, 1, 9);
    assert!(redstone::is_powered(&block(&storage, first)));
    run_ticks(&mut storage, 10, 10);
    assert!(!redstone::is_powered(&block(&storage, first)));
}

#[test]
fn wire_power_and_update_order_are_deterministic() {
    fn build(name: &str) -> WorldStorage {
        let mut storage = flat_storage(name);
        let lever = BlockPos::new(2, -60, 2);
        place(&mut storage, lever, default_state("minecraft:lever"));
        for x in 3..=8 {
            place(
                &mut storage,
                BlockPos::new(x, -60, 2),
                redstone::wire_state(0),
            );
        }
        assert!(redstone::toggle_lever(&mut storage, lever).unwrap());
        storage
    }

    let first = build("wire-order-a");
    let second = build("wire-order-b");
    assert_eq!(
        first.tick_state().last_neighbor_updates,
        second.tick_state().last_neighbor_updates
    );
    for x in 3..=8 {
        let pos = BlockPos::new(x, -60, 2);
        assert_eq!(block(&first, pos), block(&second, pos));
    }

    // The line carries 15, 14, ..., 10 and the last wire keeps an open stub.
    let reader = SignalReader::world(&first);
    for (offset, power) in (3..=8).zip([15, 14, 13, 12, 11, 10]) {
        assert_eq!(
            redstone::wire_power(&block(&first, BlockPos::new(offset, -60, 2))),
            power
        );
        assert!(reader.signal(BlockPos::new(offset, -60, 2), Direction::Up) == power);
    }
    let last = block(&first, BlockPos::new(8, -60, 2));
    assert_eq!(redstone::property(&last, "east"), Some("side"));
    assert_eq!(redstone::property(&last, "north"), Some("none"));
    let first_wire = block(&first, BlockPos::new(3, -60, 2));
    assert_eq!(redstone::property(&first_wire, "west"), Some("side"));
}
