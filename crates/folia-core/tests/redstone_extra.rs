//! Focused tests for lamps, powered openables, daylight signal, and note pitch.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::redstone::{self, daylight_signal, neighbor_power, note_frequency};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-redstone-extra-{}-{}-{safe}",
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

fn state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn tick_until_settled(storage: &mut WorldStorage) {
    for tick_number in 1..=4 {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

#[test]
fn redstone_lamp_lights_and_unlights_with_neighbour_power() {
    let mut storage = flat_storage("lamp");
    let lamp_pos = BlockPos::new(0, -60, 0);
    place(&mut storage, lamp_pos, state("minecraft:redstone_lamp"));
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("false")
    );

    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:redstone_block"),
    );
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("true")
    );
    assert!(neighbor_power(&storage, lamp_pos) > 0);

    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:air"),
    );
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, lamp_pos), "lit"),
        Some("false")
    );
}

#[test]
fn doors_trapdoors_and_fence_gates_open_while_powered() {
    for name in [
        "minecraft:oak_door",
        "minecraft:oak_trapdoor",
        "minecraft:oak_fence_gate",
    ] {
        let mut storage = flat_storage(name);
        let pos = BlockPos::new(0, -60, 0);
        place(&mut storage, pos, state(name));
        place(
            &mut storage,
            BlockPos::new(1, -60, 0),
            state("minecraft:redstone_block"),
        );
        tick_until_settled(&mut storage);
        let open = block(&storage, pos);
        assert_eq!(redstone::property(&open, "open"), Some("true"), "{name}");
        assert_eq!(redstone::property(&open, "powered"), Some("true"), "{name}");

        place(
            &mut storage,
            BlockPos::new(1, -60, 0),
            state("minecraft:air"),
        );
        tick_until_settled(&mut storage);
        let closed = block(&storage, pos);
        assert_eq!(redstone::property(&closed, "open"), Some("false"), "{name}");
    }
}

#[test]
fn daylight_signal_matches_the_vanilla_scale() {
    assert_eq!(daylight_signal(0, false), 0);
    assert_eq!(daylight_signal(15, false), 15);
    assert_eq!(daylight_signal(7, false), 7);
    assert_eq!(daylight_signal(0, true), 15);
    assert_eq!(daylight_signal(15, true), 0);
    assert_eq!(daylight_signal(7, true), 8);
}

#[test]
fn note_frequency_uses_the_vanilla_octave_curve() {
    assert!((note_frequency(0) - 0.5).abs() < 1e-5);
    assert!((note_frequency(12) - 1.0).abs() < 1e-5);
    assert!((note_frequency(24) - 2.0).abs() < 1e-5);
}

#[test]
fn daylight_detector_samples_sky_light_and_inverts() {
    let mut storage = flat_storage("daylight");
    let detector = BlockPos::new(0, -61, 0);
    place(&mut storage, detector, state("minecraft:daylight_detector"));
    for tick_number in 1..=25 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(
        redstone::property(&block(&storage, detector), "power"),
        Some("15")
    );

    // Cover the detector; the next scheduled sample must drop to zero.
    place(
        &mut storage,
        BlockPos::new(0, -60, 0),
        state("minecraft:stone"),
    );
    for tick_number in 26..=50 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(
        redstone::property(&block(&storage, detector), "power"),
        Some("0")
    );

    let inverted = BlockPos::new(2, -61, 0);
    let mut inverted_state = state("minecraft:daylight_detector");
    inverted_state = redstone::with_property(&inverted_state, "inverted", "true");
    place(&mut storage, inverted, inverted_state);
    place(
        &mut storage,
        BlockPos::new(2, -60, 0),
        state("minecraft:stone"),
    );
    for tick_number in 51..=75 {
        tick::tick_world(&mut storage, tick_number).unwrap();
    }
    assert_eq!(
        redstone::property(&block(&storage, inverted), "power"),
        Some("15")
    );
}

#[test]
fn neighbours_can_supply_power_directly() {
    let mut storage = flat_storage("power");
    let pos = BlockPos::new(0, -60, 0);
    place(
        &mut storage,
        BlockPos::new(1, -60, 0),
        state("minecraft:redstone_block"),
    );
    assert_eq!(neighbor_power(&storage, pos), 15);
}

use folia_core::math::Direction;
use folia_core::redstone::{TripwireHookChain, scan_tripwire_hook_chain};
use std::collections::HashMap;

fn tripwire_block(powered: bool) -> BlockState {
    BlockState {
        name: "minecraft:tripwire".to_owned(),
        properties: vec![(
            "powered".to_owned(),
            if powered { "true" } else { "false" }.to_owned(),
        )],
    }
}

fn hook_block() -> BlockState {
    BlockState {
        name: "minecraft:tripwire_hook".to_owned(),
        properties: vec![],
    }
}

fn stone_block() -> BlockState {
    BlockState {
        name: "minecraft:stone".to_owned(),
        properties: vec![],
    }
}

fn scan_map(map: &HashMap<BlockPos, BlockState>) -> TripwireHookChain {
    let map = map.clone();
    scan_tripwire_hook_chain(BlockPos::new(0, 0, 0), Direction::East, |pos| {
        map.get(&pos).cloned()
    })
}

#[test]
fn tripwire_hook_chain_attaches_across_three_wires() {
    let mut map = HashMap::new();
    for i in 1..=3 {
        map.insert(BlockPos::new(i, 0, 0), tripwire_block(false));
    }
    map.insert(BlockPos::new(4, 0, 0), hook_block());
    let chain = scan_map(&map);
    assert!(chain.attached);
    assert_eq!(chain.wires.len(), 3);
    assert!(!chain.powered);
}

#[test]
fn tripwire_hook_chain_reports_powered_wire() {
    let mut map = HashMap::new();
    map.insert(BlockPos::new(1, 0, 0), tripwire_block(false));
    map.insert(BlockPos::new(2, 0, 0), tripwire_block(true));
    map.insert(BlockPos::new(3, 0, 0), tripwire_block(false));
    map.insert(BlockPos::new(4, 0, 0), hook_block());
    let chain = scan_map(&map);
    assert!(chain.attached);
    assert!(chain.powered);
}

#[test]
fn tripwire_hook_chain_stops_at_obstruction() {
    let mut map = HashMap::new();
    map.insert(BlockPos::new(1, 0, 0), tripwire_block(false));
    map.insert(BlockPos::new(2, 0, 0), stone_block());
    let chain = scan_map(&map);
    assert!(!chain.attached);
    assert_eq!(chain.wires.len(), 1);
}

#[test]
fn tripwire_hook_chain_requires_a_second_hook() {
    let mut map = HashMap::new();
    for i in 1..=3 {
        map.insert(BlockPos::new(i, 0, 0), tripwire_block(false));
    }
    let chain = scan_map(&map);
    assert!(!chain.attached);
}

#[test]
fn tripwire_hook_chain_rejects_direct_hook_without_wire() {
    let mut map = HashMap::new();
    map.insert(BlockPos::new(1, 0, 0), hook_block());
    let chain = scan_map(&map);
    assert!(!chain.attached);
    assert!(chain.wires.is_empty());
}

#[test]
fn tripwire_hook_chain_is_bounded_to_42_blocks() {
    let mut map = HashMap::new();
    for i in 1..=42 {
        map.insert(BlockPos::new(i, 0, 0), tripwire_block(false));
    }
    map.insert(BlockPos::new(43, 0, 0), hook_block());
    let chain = scan_map(&map);
    assert!(chain.attached);
    assert_eq!(chain.wires.len(), 42);

    let mut map = HashMap::new();
    for i in 1..=43 {
        map.insert(BlockPos::new(i, 0, 0), tripwire_block(false));
    }
    map.insert(BlockPos::new(44, 0, 0), hook_block());
    let chain = scan_map(&map);
    assert!(!chain.attached);
}

fn hook_state(facing: &str) -> BlockState {
    BlockState {
        name: "minecraft:tripwire_hook".to_owned(),
        properties: vec![
            ("attached".to_owned(), "false".to_owned()),
            ("facing".to_owned(), facing.to_owned()),
            ("powered".to_owned(), "false".to_owned()),
        ],
    }
}

fn tripwire_state(powered: bool) -> BlockState {
    BlockState {
        name: "minecraft:tripwire".to_owned(),
        properties: vec![
            ("attached".to_owned(), "true".to_owned()),
            ("disarmed".to_owned(), "false".to_owned()),
            ("east".to_owned(), "false".to_owned()),
            ("north".to_owned(), "false".to_owned()),
            (
                "powered".to_owned(),
                if powered { "true" } else { "false" }.to_owned(),
            ),
            ("south".to_owned(), "false".to_owned()),
            ("west".to_owned(), "false".to_owned()),
        ],
    }
}

#[test]
fn tripwire_hook_attaches_and_detaches_with_chain() {
    let mut storage = flat_storage("tripwire-hook-attach");
    let hook = BlockPos::new(0, -60, 0);
    let wire1 = BlockPos::new(1, -60, 0);
    let wire2 = BlockPos::new(2, -60, 0);
    let wire3 = BlockPos::new(3, -60, 0);
    let other = BlockPos::new(4, -60, 0);
    place(&mut storage, hook, hook_state("east"));
    place(&mut storage, wire1, tripwire_state(false));
    place(&mut storage, wire2, tripwire_state(false));
    place(&mut storage, wire3, tripwire_state(false));
    place(&mut storage, other, hook_state("west"));

    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, hook), "attached"),
        Some("true")
    );

    place(&mut storage, wire2, state("minecraft:air"));
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, hook), "attached"),
        Some("false")
    );
}

#[test]
fn tripwire_hook_power_follows_attached_tripwire() {
    let mut storage = flat_storage("tripwire-hook-power");
    let hook = BlockPos::new(0, -60, 0);
    let wire1 = BlockPos::new(1, -60, 0);
    let wire2 = BlockPos::new(2, -60, 0);
    let wire3 = BlockPos::new(3, -60, 0);
    let other = BlockPos::new(4, -60, 0);
    place(&mut storage, hook, hook_state("east"));
    place(&mut storage, wire1, tripwire_state(false));
    place(&mut storage, wire2, tripwire_state(false));
    place(&mut storage, wire3, tripwire_state(false));
    place(&mut storage, other, hook_state("west"));
    tick_until_settled(&mut storage);

    place(&mut storage, wire2, tripwire_state(true));
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, hook), "powered"),
        Some("true")
    );

    place(&mut storage, wire2, tripwire_state(false));
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, hook), "powered"),
        Some("false")
    );
}

#[test]
fn powered_tripwire_hook_emits_signal_behind_it() {
    let mut storage = flat_storage("tripwire-hook-signal");
    let hook = BlockPos::new(0, -60, 0);
    let wire1 = BlockPos::new(1, -60, 0);
    let wire2 = BlockPos::new(2, -60, 0);
    let wire3 = BlockPos::new(3, -60, 0);
    let other = BlockPos::new(4, -60, 0);
    place(&mut storage, hook, hook_state("east"));
    place(&mut storage, wire1, tripwire_state(false));
    place(&mut storage, wire2, tripwire_state(true));
    place(&mut storage, wire3, tripwire_state(false));
    place(&mut storage, other, hook_state("west"));
    tick_until_settled(&mut storage);
    assert_eq!(
        redstone::property(&block(&storage, hook), "powered"),
        Some("true")
    );

    let reader = redstone::SignalReader::world(&storage);
    assert_eq!(
        reader.direct_signal(hook, folia_core::math::Direction::East),
        15
    );
    assert_eq!(
        reader.direct_signal(hook, folia_core::math::Direction::West),
        0
    );
}
