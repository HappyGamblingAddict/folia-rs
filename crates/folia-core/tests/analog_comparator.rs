//! P3.10 analog comparator inputs: item frames, lecterns, and daylight
//! detectors. Every test drives the real scheduled-refresh queue.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::item_components::{ComponentValue, ItemStack, item_stack_to_nbt};
use folia_core::math::{BlockPos, Direction};
use folia_core::nbt::Tag;
use folia_core::redstone::{
    self, AnalogSource, SignalReader, item_frame_analog_output, lectern_analog_output,
    lectern_page_count,
};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-analog-{}-{}-{name}",
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

fn state(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| panic!("missing block state {name}"))
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn place(storage: &mut WorldStorage, pos: BlockPos, block: BlockState) {
    block_update::set_block(storage, pos.x(), pos.y(), pos.z(), block)
        .unwrap()
        .expect("placement changed the block");
}

fn comparator(facing: Direction) -> BlockState {
    redstone::with_facing(&state("minecraft:comparator"), facing)
}

fn output(storage: &WorldStorage, pos: BlockPos) -> i32 {
    SignalReader::world(storage).comparator_output(pos)
}

fn run_ticks(storage: &mut WorldStorage, from: u64, to: u64) {
    for tick_number in from..=to {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

fn framed_item() -> ItemStack {
    ItemStack::new("minecraft:stone", 1)
}

fn written_book(pages: usize) -> ItemStack {
    let mut book = ItemStack::new("minecraft:written_book", 1);
    let content = Tag::Compound(vec![(
        "pages".to_owned(),
        Tag::List(
            (0..pages)
                .map(|index| Tag::String(format!("page {index}")))
                .collect(),
        ),
    )]);
    book.set_component_named(
        "minecraft:written_book_content",
        ComponentValue::Opaque(content),
    )
    .unwrap();
    book
}

fn lectern_nbt(book: &ItemStack, page: i32) -> Tag {
    Tag::Compound(vec![
        ("Book".to_owned(), item_stack_to_nbt(book)),
        ("Page".to_owned(), Tag::Int(page)),
    ])
}

#[test]
fn item_frame_rotation_maps_to_vanilla_one_through_eight() {
    let item = framed_item();
    assert_eq!(item_frame_analog_output(&ItemStack::empty(), 3), 0);
    for rotation in 0..8 {
        assert_eq!(item_frame_analog_output(&item, rotation), rotation + 1);
    }
    assert_eq!(item_frame_analog_output(&item, 8), 1);
    assert_eq!(item_frame_analog_output(&item, -1), 8);
}

#[test]
fn item_frame_nbt_carries_item_rotation_and_facing() {
    let tag = Tag::Compound(vec![
        ("Item".to_owned(), item_stack_to_nbt(&framed_item())),
        ("ItemRotation".to_owned(), Tag::Byte(6)),
        ("Facing".to_owned(), Tag::String("west".to_owned())),
    ]);
    let source = AnalogSource::from_item_frame_nbt(&tag).expect("item frame decodes");
    assert_eq!(source.analog_output_signal(Some(Direction::West)), 7);
    assert_eq!(source.analog_output_signal(Some(Direction::East)), 0);
    assert_eq!(source.analog_output_signal(None), 7);

    let empty = Tag::Compound(vec![("Facing".to_owned(), Tag::String("west".to_owned()))]);
    let source = AnalogSource::from_item_frame_nbt(&empty).expect("empty frame decodes");
    assert_eq!(source.analog_output_signal(Some(Direction::West)), 0);
    assert!(AnalogSource::from_item_frame_nbt(&Tag::Compound(Vec::new())).is_none());
}

#[test]
fn item_frame_drives_comparator_through_conductor_and_refreshes_in_two_ticks() {
    let mut storage = flat_storage("frame-conductor");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let conductor_pos = BlockPos::new(4, -60, 5);
    let frame_pos = BlockPos::new(3, -60, 5);
    place(&mut storage, conductor_pos, stone());
    place(&mut storage, comparator_pos, comparator(Direction::West));

    assert!(redstone::set_item_frame_source_from_nbt(
        &mut storage,
        frame_pos,
        &Tag::Compound(vec![
            ("Item".to_owned(), item_stack_to_nbt(&framed_item())),
            ("ItemRotation".to_owned(), Tag::Byte(2)),
            ("Facing".to_owned(), Tag::String("west".to_owned())),
        ]),
    ));
    run_ticks(&mut storage, 1, 2);
    assert_eq!(output(&storage, comparator_pos), 3);
    assert!(redstone::is_powered(
        &storage
            .block_state_at(comparator_pos.x(), comparator_pos.y(), comparator_pos.z())
            .unwrap()
    ));

    // Rotation 6 must not take effect until the scheduled two-tick refresh.
    redstone::set_item_frame_source(&mut storage, frame_pos, Direction::West, framed_item(), 6);
    run_ticks(&mut storage, 3, 3);
    assert_eq!(output(&storage, comparator_pos), 3);
    run_ticks(&mut storage, 4, 4);
    assert_eq!(output(&storage, comparator_pos), 7);

    // A face pointing away from the comparator supplies no signal.
    redstone::set_item_frame_source(&mut storage, frame_pos, Direction::East, framed_item(), 6);
    run_ticks(&mut storage, 5, 6);
    assert_eq!(output(&storage, comparator_pos), 0);
    assert!(!redstone::is_powered(
        &storage
            .block_state_at(comparator_pos.x(), comparator_pos.y(), comparator_pos.z())
            .unwrap()
    ));

    assert!(redstone::remove_analog_source(&mut storage, frame_pos));
    run_ticks(&mut storage, 7, 8);
    assert_eq!(output(&storage, comparator_pos), 0);
    assert!(!redstone::remove_analog_source(&mut storage, frame_pos));
}

#[test]
fn item_frame_is_ignored_without_a_conductor_between_it_and_the_comparator() {
    let mut storage = flat_storage("frame-no-conductor");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let frame_pos = BlockPos::new(4, -60, 5);
    place(&mut storage, comparator_pos, comparator(Direction::West));
    redstone::set_item_frame_source(&mut storage, frame_pos, Direction::West, framed_item(), 7);
    run_ticks(&mut storage, 1, 4);
    assert_eq!(output(&storage, comparator_pos), 0);
}

#[test]
fn lectern_page_strength_matches_the_vanilla_float_formula() {
    let book = written_book(2);
    assert_eq!(lectern_page_count(&book), 2);
    assert_eq!(lectern_page_count(&ItemStack::new("minecraft:book", 1)), 0);

    assert_eq!(lectern_analog_output(false, 0, 2), 0);
    assert_eq!(lectern_analog_output(true, 0, 1), 15);
    assert_eq!(lectern_analog_output(true, 0, 0), 15);
    assert_eq!(lectern_analog_output(true, 0, 2), 1);
    assert_eq!(lectern_analog_output(true, 1, 2), 15);
    assert_eq!(lectern_analog_output(true, 9, 15), 10);
    assert_eq!(lectern_analog_output(true, 14, 15), 15);

    let tag = lectern_nbt(&book, 1);
    let source = AnalogSource::from_lectern_nbt(&tag).expect("lectern decodes");
    assert!(!source.is_item_frame());
    assert_eq!(source.analog_output_signal(None), 15);
    assert!(AnalogSource::from_lectern_nbt(&Tag::Compound(Vec::new())).is_some());
    assert_eq!(
        AnalogSource::from_lectern_nbt(&Tag::Compound(Vec::new()))
            .unwrap()
            .analog_output_signal(None),
        0
    );
}

#[test]
fn lectern_drives_comparator_directly_and_updates_on_page_turn() {
    let mut storage = flat_storage("lectern-direct");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let lectern_pos = BlockPos::new(4, -60, 5);
    let mut lectern = state("minecraft:lectern");
    lectern = redstone::with_property(&lectern, "has_book", "true");
    place(&mut storage, lectern_pos, lectern);
    place(&mut storage, comparator_pos, comparator(Direction::West));

    let book = written_book(2);
    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        lectern_pos,
        &lectern_nbt(&book, 0),
    ));
    run_ticks(&mut storage, 1, 2);
    assert_eq!(output(&storage, comparator_pos), 1);

    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        lectern_pos,
        &lectern_nbt(&book, 1),
    ));
    run_ticks(&mut storage, 3, 3);
    assert_eq!(output(&storage, comparator_pos), 1);
    run_ticks(&mut storage, 4, 4);
    assert_eq!(output(&storage, comparator_pos), 15);

    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        lectern_pos,
        &Tag::Compound(Vec::new()),
    ));
    run_ticks(&mut storage, 5, 6);
    assert_eq!(output(&storage, comparator_pos), 0);
    assert!(!redstone::is_powered(
        &storage
            .block_state_at(comparator_pos.x(), comparator_pos.y(), comparator_pos.z())
            .unwrap()
    ));
}

#[test]
fn lectern_drives_comparator_through_conductor() {
    let mut storage = flat_storage("lectern-conductor");
    let comparator_pos = BlockPos::new(10, -60, 5);
    let conductor_pos = BlockPos::new(9, -60, 5);
    let lectern_pos = BlockPos::new(8, -60, 5);
    place(&mut storage, conductor_pos, stone());
    let mut lectern = state("minecraft:lectern");
    lectern = redstone::with_property(&lectern, "has_book", "true");
    place(&mut storage, lectern_pos, lectern);
    place(&mut storage, comparator_pos, comparator(Direction::West));

    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        lectern_pos,
        &lectern_nbt(&written_book(2), 1),
    ));
    run_ticks(&mut storage, 1, 2);
    assert_eq!(output(&storage, comparator_pos), 15);
}

#[test]
fn compare_and_subtract_modes_apply_to_analog_page_strength() {
    let mut storage = flat_storage("lectern-modes");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let lectern_pos = BlockPos::new(4, -60, 5);
    let side_pos = BlockPos::new(5, -60, 6);
    let mut lectern = state("minecraft:lectern");
    lectern = redstone::with_property(&lectern, "has_book", "true");
    place(&mut storage, lectern_pos, lectern);
    place(&mut storage, side_pos, state("minecraft:redstone_block"));
    place(&mut storage, comparator_pos, comparator(Direction::West));
    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        lectern_pos,
        &lectern_nbt(&written_book(2), 1),
    ));
    run_ticks(&mut storage, 1, 2);

    let block = storage
        .block_state_at(comparator_pos.x(), comparator_pos.y(), comparator_pos.z())
        .unwrap();
    assert_eq!(redstone::property(&block, "mode"), Some("compare"));
    assert!(redstone::is_powered(&block));
    assert_eq!(output(&storage, comparator_pos), 15);

    assert!(redstone::use_block(&mut storage, comparator_pos).unwrap());
    let block = storage
        .block_state_at(comparator_pos.x(), comparator_pos.y(), comparator_pos.z())
        .unwrap();
    assert_eq!(redstone::property(&block, "mode"), Some("subtract"));
    assert_eq!(output(&storage, comparator_pos), 15 - 15);
    assert!(!redstone::is_powered(&block));
}

#[test]
fn replacing_an_analog_source_block_clears_the_registered_source() {
    let mut storage = flat_storage("source-teardown");
    let comparator_pos = BlockPos::new(5, -60, 5);
    let source_pos = BlockPos::new(4, -60, 5);
    let mut lectern = state("minecraft:lectern");
    lectern = redstone::with_property(&lectern, "has_book", "true");
    place(&mut storage, source_pos, lectern);
    place(&mut storage, comparator_pos, comparator(Direction::West));
    assert!(redstone::set_lectern_source_from_nbt(
        &mut storage,
        source_pos,
        &lectern_nbt(&written_book(2), 1),
    ));
    run_ticks(&mut storage, 1, 2);
    assert_eq!(output(&storage, comparator_pos), 15);

    place(&mut storage, source_pos, stone());
    run_ticks(&mut storage, 3, 4);
    assert_eq!(output(&storage, comparator_pos), 0);
    assert!(
        !storage
            .tick_state()
            .analog_sources
            .contains_key(&source_pos)
    );
}

#[test]
fn daylight_detector_power_drives_a_comparator_and_updates_on_sample() {
    let mut storage = flat_storage("daylight-analog");
    let detector_pos = BlockPos::new(0, -61, 0);
    let comparator_pos = BlockPos::new(1, -61, 0);
    let mut detector = state("minecraft:daylight_detector");
    detector = redstone::with_property(&detector, "power", "0");
    place(&mut storage, detector_pos, detector);
    place(&mut storage, comparator_pos, comparator(Direction::West));

    run_ticks(&mut storage, 1, 25);
    assert_eq!(
        redstone::property(
            &storage
                .block_state_at(detector_pos.x(), detector_pos.y(), detector_pos.z())
                .unwrap(),
            "power"
        ),
        Some("15")
    );
    assert_eq!(output(&storage, comparator_pos), 15);

    place(&mut storage, BlockPos::new(0, -60, 0), stone());
    run_ticks(&mut storage, 26, 50);
    assert_eq!(
        redstone::property(
            &storage
                .block_state_at(detector_pos.x(), detector_pos.y(), detector_pos.z())
                .unwrap(),
            "power"
        ),
        Some("0")
    );
    assert_eq!(output(&storage, comparator_pos), 0);
}

#[test]
fn daylight_analog_output_is_read_from_the_power_property() {
    let mut storage = flat_storage("daylight-property");
    let detector_pos = BlockPos::new(0, -61, 0);
    place(&mut storage, BlockPos::new(0, -60, 0), stone());
    let mut detector = state("minecraft:daylight_detector");
    detector = redstone::with_property(&detector, "power", "6");
    place(&mut storage, detector_pos, detector);
    assert_eq!(
        redstone::property(
            &storage
                .block_state_at(detector_pos.x(), detector_pos.y(), detector_pos.z())
                .unwrap(),
            "power"
        ),
        Some("0"),
        "placement samples the covered sky light"
    );
    let reader = SignalReader::world(&storage);
    assert!(reader.has_analog_output_signal(detector_pos));
    assert_eq!(reader.analog_output_signal(detector_pos), Some(0));
    assert_eq!(
        reader.analog_output_signal_from(detector_pos, Some(Direction::East)),
        Some(0)
    );
    assert_eq!(reader.analog_output_signal(BlockPos::new(2, -61, 0)), None);
}
