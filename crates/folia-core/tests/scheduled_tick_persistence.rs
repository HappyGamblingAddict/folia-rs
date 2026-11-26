//! Focused P3.10 tests for persisting scheduled block ticks with chunks.
//!
//! Vanilla 1.21 stores block ticks in a chunk's `block_ticks` list of
//! `SavedTick` compounds. The Rust port writes the same `i`/`x`/`y`/`z`/`t`/`p`
//! fields plus an extra `o` (sub-tick order) so a reload keeps FIFO ordering.

use folia_core::anvil::{self, Compression, RegionFile};
use folia_core::chunk::encode_chunk;
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::tick::{ScheduledTick, TickPriority};
use folia_core::world::{Dimension, RegionPos, WorldStorage};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-scheduled-ticks-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn tick(x: i32, z: i32, trigger: u64, priority: TickPriority, order: i64) -> ScheduledTick {
    ScheduledTick {
        pos: BlockPos::new(x, 64, z),
        block: "minecraft:sand".into(),
        trigger_tick: trigger,
        priority,
        sub_tick_order: order,
        unknown: Vec::new(),
    }
}

fn write_raw_chunk(region_dir: &std::path::Path, x: i32, z: i32, root: &Tag) {
    let position = RegionPos::from_chunk(x, z);
    let (local_x, local_z) = position.local_chunk(x, z);
    let mut region = RegionFile::open_or_create(region_dir.join(position.file_name())).unwrap();
    region
        .write_chunk(local_x, local_z, root, Compression::Zlib)
        .unwrap();
    region.save().unwrap();
}

fn stored_root(region_dir: &std::path::Path, x: i32, z: i32) -> Tag {
    let position = RegionPos::from_chunk(x, z);
    let (local_x, local_z) = position.local_chunk(x, z);
    let region = RegionFile::open(region_dir.join(position.file_name())).unwrap();
    region.read_chunk(local_x, local_z).unwrap().unwrap()
}

#[test]
fn nbt_round_trip_keeps_position_delay_priority_and_sub_order() {
    let scheduled = tick(5, 9, 105, TickPriority::High, 7);
    let tag = anvil::encode_scheduled_tick(&scheduled, 100);

    assert_eq!(tag.get("i"), Some(&Tag::String("minecraft:sand".into())));
    assert_eq!(tag.get("x"), Some(&Tag::Int(5)));
    assert_eq!(tag.get("y"), Some(&Tag::Int(64)));
    assert_eq!(tag.get("z"), Some(&Tag::Int(9)));
    assert_eq!(tag.get("t"), Some(&Tag::Int(5)));
    assert_eq!(tag.get("p"), Some(&Tag::Int(-1)));
    assert_eq!(tag.get("o"), Some(&Tag::Long(7)));

    let decoded = anvil::decode_scheduled_tick(&tag, 0, 0).unwrap();
    assert_eq!(decoded.pos, scheduled.pos);
    assert_eq!(decoded.block, scheduled.block);
    assert_eq!(decoded.trigger_tick, 5);
    assert_eq!(decoded.priority, TickPriority::High);
    assert_eq!(decoded.sub_tick_order, 7);
    assert!(decoded.unknown.is_empty());
}

#[test]
fn overdue_ticks_are_stored_with_a_negative_delay() {
    let scheduled = tick(0, 0, 96, TickPriority::Normal, 0);
    let tag = anvil::encode_scheduled_tick(&scheduled, 100);
    assert_eq!(tag.get("t"), Some(&Tag::Int(-4)));

    let decoded = anvil::decode_scheduled_tick(&tag, 10, 0).unwrap();
    assert_eq!(decoded.trigger_tick, 6);
}

#[test]
fn unknown_tick_fields_survive_a_round_trip() {
    let scheduled = tick(1, 2, 30, TickPriority::VeryLow, 3);
    let mut tag = anvil::encode_scheduled_tick(&scheduled, 0);
    if let Tag::Compound(fields) = &mut tag {
        fields.push(("mod:Extra".into(), Tag::String("keep".into())));
        fields.push(("DataVersion".into(), Tag::Int(4788)));
    }
    let decoded = anvil::decode_scheduled_tick(&tag, 0, 0).unwrap();
    assert_eq!(decoded.unknown.len(), 2);
    let again = anvil::encode_scheduled_tick(&decoded, 0);
    assert_eq!(again.get("mod:Extra"), Some(&Tag::String("keep".into())));
    assert_eq!(again.get("DataVersion"), Some(&Tag::Int(4788)));
}

#[test]
fn vanilla_field_names_without_sub_order_are_accepted() {
    // A real 1.21 `SavedTick` has no `o` field; its FIFO order is derived from
    // the list position. Two entries exercise the `-len + index` fallback.
    let root = Tag::Compound(vec![(
        "block_ticks".into(),
        Tag::List(vec![
            Tag::Compound(vec![
                ("i".into(), Tag::String("minecraft:sand".into())),
                ("x".into(), Tag::Int(1)),
                ("y".into(), Tag::Int(64)),
                ("z".into(), Tag::Int(2)),
                ("t".into(), Tag::Int(7)),
                ("p".into(), Tag::Int(-2)),
            ]),
            Tag::Compound(vec![
                ("i".into(), Tag::String("minecraft:gravel".into())),
                ("x".into(), Tag::Int(3)),
                ("y".into(), Tag::Int(65)),
                ("z".into(), Tag::Int(4)),
                ("t".into(), Tag::Int(9)),
                ("p".into(), Tag::Int(1)),
            ]),
        ]),
    )]);
    let decoded = anvil::decode_scheduled_ticks(&root, 20).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].trigger_tick, 27);
    assert_eq!(decoded[0].priority, TickPriority::VeryHigh);
    assert_eq!(decoded[0].sub_tick_order, -2);
    assert_eq!(decoded[1].trigger_tick, 29);
    assert_eq!(decoded[1].sub_tick_order, -1);
}

#[test]
fn legacy_tile_ticks_are_read() {
    let root = Tag::Compound(vec![(
        "TileTicks".into(),
        Tag::List(vec![Tag::Compound(vec![
            ("i".into(), Tag::String("minecraft:water".into())),
            ("x".into(), Tag::Int(8)),
            ("y".into(), Tag::Int(60)),
            ("z".into(), Tag::Int(9)),
            ("t".into(), Tag::Int(5)),
            ("p".into(), Tag::Int(0)),
        ])]),
    )]);
    let decoded = anvil::decode_scheduled_ticks(&root, 0).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].block, "minecraft:water");
    assert_eq!(decoded[0].trigger_tick, 5);
}

#[test]
fn scheduled_ticks_survive_a_cold_save_reload() {
    let root = temp_dir("save-reload");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.tick_state_mut().scheduled.schedule_after(
            0,
            BlockPos::new(1, 64, 2),
            "minecraft:sand",
            7,
            TickPriority::Normal,
        );
        storage.tick_state_mut().scheduled.schedule_after(
            0,
            BlockPos::new(3, 65, 4),
            "minecraft:gravel",
            5,
            TickPriority::Low,
        );
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        assert!(storage.is_chunk_dirty(0, 0));
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    let restored = reopened.scheduled_ticks_in_chunk(0, 0);
    assert_eq!(restored.len(), 2, "both ticks come back from disk");
    assert_eq!(restored[0].pos, BlockPos::new(3, 65, 4));
    assert_eq!(restored[0].block, "minecraft:gravel");
    assert_eq!(restored[0].priority, TickPriority::Low);
    assert_eq!(restored[1].pos, BlockPos::new(1, 64, 2));
    assert_eq!(restored[1].trigger_tick, 7);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ticks_stay_with_their_chunk_across_multiple_chunks() {
    let root = temp_dir("multiple-chunks");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let coords = [(0, 0), (1, 0), (0, 1)];

    {
        let mut storage = WorldStorage::new(&region_dir);
        for &(cx, cz) in &coords {
            storage
                .save_chunk(&flat_chunk(cx, cz), Compression::Zlib)
                .unwrap();
        }
        for (index, &(cx, cz)) in coords.iter().enumerate() {
            storage.tick_state_mut().scheduled.schedule_after(
                0,
                BlockPos::new(cx * 16 + index as i32 + 1, 64, cz * 16 + 2),
                "minecraft:sand",
                index as u64 + 3,
                TickPriority::Normal,
            );
        }
        for &(cx, cz) in &coords {
            storage
                .save_chunk_with_entities(&flat_chunk(cx, cz), Compression::Zlib)
                .unwrap();
        }
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    for &(cx, cz) in &coords {
        reopened
            .load_chunk(cx, cz)
            .unwrap()
            .expect("chunk survives");
        let restored = reopened.scheduled_ticks_in_chunk(cx, cz);
        assert_eq!(
            restored.len(),
            1,
            "chunk ({cx},{cz}) keeps only its own tick"
        );
        assert_eq!(
            restored[0].pos.x(),
            cx * 16 + 1 + coords.iter().position(|&p| p == (cx, cz)).unwrap() as i32
        );
        assert_eq!(restored[0].pos.z(), cz * 16 + 2);
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn drain_order_is_identical_after_reload() {
    let root = temp_dir("ordering");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    let mut before = WorldStorage::new(&region_dir);
    before.save_chunk(&chunk, Compression::Zlib).unwrap();
    let queue = &mut before.tick_state_mut().scheduled;
    queue.schedule_after(
        0,
        BlockPos::new(1, 64, 1),
        "minecraft:a",
        5,
        TickPriority::Normal,
    );
    queue.schedule_after(
        0,
        BlockPos::new(2, 64, 2),
        "minecraft:b",
        5,
        TickPriority::ExtremelyHigh,
    );
    queue.schedule_after(
        0,
        BlockPos::new(3, 64, 3),
        "minecraft:c",
        2,
        TickPriority::Normal,
    );
    queue.schedule_after(
        0,
        BlockPos::new(4, 64, 4),
        "minecraft:d",
        5,
        TickPriority::Normal,
    );
    let expected: Vec<ScheduledTick> = before.scheduled_ticks_in_chunk(0, 0);
    assert_eq!(
        expected
            .iter()
            .map(|t| t.block.as_str())
            .collect::<Vec<_>>(),
        ["minecraft:c", "minecraft:b", "minecraft:a", "minecraft:d",]
    );

    before
        .save_chunk_with_entities(&chunk, Compression::Zlib)
        .unwrap();
    before.flush().unwrap();
    drop(before);

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    let drained = reopened.tick_state_mut().scheduled.tick(u64::MAX);
    let actual: Vec<ScheduledTick> = drained
        .into_iter()
        .filter(|t| t.pos.x().div_euclid(16) == 0 && t.pos.z().div_euclid(16) == 0)
        .collect();
    assert_eq!(actual, expected);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_tick_booked_after_reload_sorts_behind_restored_ticks() {
    let root = temp_dir("sub-order");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.tick_state_mut().scheduled.schedule_after(
            0,
            BlockPos::new(1, 64, 1),
            "minecraft:sand",
            5,
            TickPriority::Normal,
        );
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    reopened.tick_state_mut().scheduled.schedule_after(
        0,
        BlockPos::new(2, 64, 2),
        "minecraft:gravel",
        5,
        TickPriority::Normal,
    );
    let restored = reopened.scheduled_ticks_in_chunk(0, 0);
    let order: Vec<&str> = restored.iter().map(|t| t.block.as_str()).collect();
    assert_eq!(order, ["minecraft:sand", "minecraft:gravel"]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ticks_survive_unknown_top_level_fields_and_an_update() {
    let root = temp_dir("preserve");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut stored = encode_chunk(&chunk).unwrap();
        if let Tag::Compound(fields) = &mut stored {
            fields.push(("mod:extra".into(), Tag::String("keep me".into())));
            fields.push((
                "structures".into(),
                folia_core::snbt::parse("{Starts: {}, References: {}}").unwrap(),
            ));
        }
        write_raw_chunk(&region_dir, 0, 0, &stored);
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.load_chunk(0, 0).unwrap().expect("stored chunk");
        storage.tick_state_mut().scheduled.schedule_after(
            0,
            BlockPos::new(6, 64, 7),
            "minecraft:sand",
            11,
            TickPriority::High,
        );
        storage
            .update_chunk(&chunk, Compression::Zlib, &[(0, 64, 0)])
            .unwrap();
        storage.flush().unwrap();
    }

    let raw = stored_root(&region_dir, 0, 0);
    assert_eq!(raw.get("mod:extra"), Some(&Tag::String("keep me".into())));
    assert!(raw.get("structures").is_some());
    let list = raw.get("block_ticks").and_then(Tag::as_list).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0].get("i"),
        Some(&Tag::String("minecraft:sand".into()))
    );
    assert_eq!(list[0].get("t"), Some(&Tag::Int(11)));
    assert_eq!(list[0].get("p"), Some(&Tag::Int(-1)));
    assert_eq!(raw.get("TileTicks"), None);

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    let restored = reopened.scheduled_ticks_in_chunk(0, 0);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].sub_tick_order, 0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ticks_outside_the_loaded_chunk_are_discarded() {
    let root = temp_dir("filter");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut stored = encode_chunk(&flat_chunk(0, 0)).unwrap();
    if let Tag::Compound(fields) = &mut stored {
        fields.push((
            "block_ticks".into(),
            Tag::List(vec![
                anvil::encode_scheduled_tick(&tick(1, 2, 5, TickPriority::Normal, 0), 0),
                anvil::encode_scheduled_tick(&tick(20, 2, 6, TickPriority::Normal, 1), 0),
            ]),
        ));
    }
    write_raw_chunk(&region_dir, 0, 0, &stored);

    let mut storage = WorldStorage::new(&region_dir);
    storage.load_chunk(0, 0).unwrap().expect("stored chunk");
    let restored = storage.scheduled_ticks_in_chunk(0, 0);
    assert_eq!(restored.len(), 1, "the out-of-chunk tick is filtered");
    assert_eq!(restored[0].pos.x(), 1);
    assert_eq!(storage.tick_state().scheduled.len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dirty_ticks_are_written_when_the_region_unloads() {
    let root = temp_dir("region-unload");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.flush().unwrap();
        storage.tick_state_mut().scheduled.schedule_after(
            0,
            BlockPos::new(2, 64, 3),
            "minecraft:sand",
            9,
            TickPriority::Low,
        );
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        assert!(storage.is_chunk_dirty(0, 0));
        storage.unload_region(RegionPos::from_chunk(0, 0)).unwrap();
        assert_eq!(storage.dirty_chunk_count(), 0);
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    let restored = reopened.scheduled_ticks_in_chunk(0, 0);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].trigger_tick, 9);
    assert_eq!(restored[0].priority, TickPriority::Low);

    fs::remove_dir_all(root).unwrap();
}
