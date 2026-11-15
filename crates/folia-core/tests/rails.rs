//! Focused tests for stage-A rail block shapes.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::rail::{self, RailShape};
use folia_core::registry::default_block_state;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-rails-{}-{}-{safe}",
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

fn rail_state() -> BlockState {
    let mut state = default_block_state("minecraft:rail").expect("rail state");
    state
        .properties
        .push(("shape".to_owned(), "north_south".to_owned()));
    state
}

#[test]
fn rail_shape_names_round_trip() {
    for shape in [
        RailShape::NorthSouth,
        RailShape::EastWest,
        RailShape::AscendingNorth,
        RailShape::AscendingSouth,
        RailShape::AscendingEast,
        RailShape::AscendingWest,
        RailShape::SouthEast,
        RailShape::SouthWest,
        RailShape::NorthWest,
        RailShape::NorthEast,
    ] {
        assert_eq!(RailShape::from_name(shape.name()), Some(shape));
    }
    assert!(RailShape::from_name("not_a_shape").is_none());
}

#[test]
fn is_rail_recognizes_all_four_rail_blocks() {
    for name in rail::RAIL_BLOCKS {
        let state = BlockState {
            name: (*name).to_owned(),
            properties: vec![],
        };
        assert!(rail::is_rail(&state));
    }
    let stone = BlockState {
        name: "minecraft:stone".to_owned(),
        properties: vec![],
    };
    assert!(!rail::is_rail(&stone));
}

#[test]
fn connected_shape_table() {
    use rail::RailShape::*;
    assert_eq!(
        rail::connected_shape(NorthSouth, false, false, false, false),
        NorthSouth
    );
    assert_eq!(
        rail::connected_shape(NorthSouth, false, false, true, true),
        EastWest
    );
    assert_eq!(
        rail::connected_shape(NorthSouth, true, false, true, false),
        NorthEast
    );
    assert_eq!(
        rail::connected_shape(NorthSouth, true, false, false, true),
        NorthWest
    );
    assert_eq!(
        rail::connected_shape(NorthSouth, false, true, true, false),
        SouthEast
    );
    assert_eq!(
        rail::connected_shape(NorthSouth, false, true, false, true),
        SouthWest
    );
    assert_eq!(
        rail::connected_shape(AscendingNorth, true, true, true, true),
        AscendingNorth
    );
    assert_eq!(
        rail::connected_shape(NorthEast, true, false, true, false),
        NorthEast
    );
    assert_eq!(
        rail::connected_shape(NorthEast, true, true, true, false),
        NorthSouth
    );
}

#[test]
fn rail_reconnects_straight_after_placement() {
    let mut storage = flat_storage("straight");
    place(&mut storage, BlockPos::new(1, -60, 1), rail_state());
    place(&mut storage, BlockPos::new(2, -60, 1), rail_state());
    place(&mut storage, BlockPos::new(3, -60, 1), rail_state());

    for tick in 1..=4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    let updated = storage.block_state_at(2, -60, 1).expect("cached");
    assert_eq!(rail::shape_of(&updated), Some(RailShape::EastWest));
}

#[test]
fn rail_reconnects_north_south() {
    let mut storage = flat_storage("ns");
    place(&mut storage, BlockPos::new(2, -60, 0), rail_state());
    place(&mut storage, BlockPos::new(2, -60, 1), rail_state());
    place(&mut storage, BlockPos::new(2, -60, 2), rail_state());

    for tick in 1..=4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    let updated = storage.block_state_at(2, -60, 1).expect("cached");
    assert_eq!(rail::shape_of(&updated), Some(RailShape::NorthSouth));
}

#[test]
fn rail_corner_feeds_a_curve() {
    let mut storage = flat_storage("corner");
    place(&mut storage, BlockPos::new(2, -60, 1), rail_state());
    place(&mut storage, BlockPos::new(3, -60, 1), rail_state());
    place(&mut storage, BlockPos::new(2, -60, 2), rail_state());

    for tick in 1..=4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    let updated = storage.block_state_at(2, -60, 1).expect("cached");
    assert_eq!(rail::shape_of(&updated), Some(RailShape::SouthEast));
}
