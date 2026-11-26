//! Focused tests for P3.10 support loss: floor/wall torches and rails drop to
//! air through the same placement predicates used to validate their support.

use folia_core::anvil::Compression;
use folia_core::block_update::set_block;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::math::BlockPos;
use folia_core::tick;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let safe = name.replace(':', "-");
    std::env::temp_dir().join(format!(
        "folia-rs-support-{}-{}-{safe}",
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

fn named_state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn air() -> BlockState {
    named_state("minecraft:air", &[])
}

fn stone() -> BlockState {
    named_state("minecraft:stone", &[])
}

fn bottom_slab() -> BlockState {
    named_state(
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    )
}

fn floor_torch() -> BlockState {
    named_state("minecraft:torch", &[])
}

fn wall_torch(facing: &str) -> BlockState {
    named_state("minecraft:wall_torch", &[("facing", facing)])
}

fn rail(name: &str) -> BlockState {
    named_state(name, &[("shape", "north_south")])
}

fn place(storage: &mut WorldStorage, pos: BlockPos, state: BlockState) {
    set_block(storage, pos.x(), pos.y(), pos.z(), state)
        .unwrap()
        .expect("placement changed the block");
}

fn name_at(storage: &WorldStorage, pos: BlockPos) -> String {
    storage
        .block_state_at(pos.x(), pos.y(), pos.z())
        .expect("block is in a cached chunk")
        .name
}

fn visited(storage: &WorldStorage, pos: BlockPos) -> bool {
    storage
        .tick_state()
        .last_neighbor_updates
        .iter()
        .any(|update| update.pos == pos)
}

/// Wall offsets paired with the torch `facing` that hangs off that wall.
/// A torch facing north is attached to the wall south of it, and so on.
const WALL_ORIENTATIONS: [(&str, i32, i32); 4] = [
    ("north", 0, 1),
    ("south", 0, -1),
    ("west", 1, 0),
    ("east", -1, 0),
];

#[test]
fn floor_torch_breaks_when_the_block_below_is_removed() {
    let mut storage = flat_storage("floor-remove");
    let torch_pos = BlockPos::new(2, -60, 2);
    let support = torch_pos.below();
    place(&mut storage, torch_pos, floor_torch());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:torch");

    place(&mut storage, support, air());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:air");
    assert!(
        visited(&storage, torch_pos),
        "the torch received the shape update from its support"
    );
    assert!(
        storage.tick_state().unsupported_neighbor_blocks.is_empty(),
        "torch support is an implemented neighbour behaviour"
    );
}

#[test]
fn floor_torch_survives_when_support_remains_valid() {
    let mut storage = flat_storage("floor-stays");
    let torch_pos = BlockPos::new(2, -60, 2);
    place(&mut storage, torch_pos, floor_torch());

    place(&mut storage, torch_pos.above(), stone());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:torch");

    // Replacing the flat grass block with another full cube keeps the torch.
    place(&mut storage, torch_pos.below(), stone());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:torch");
}

#[test]
fn floor_torch_uses_the_full_upper_face_support_rule() {
    let mut storage = flat_storage("floor-slab");
    let torch_pos = BlockPos::new(2, -60, 2);
    place(&mut storage, torch_pos.below(), bottom_slab());
    place(&mut storage, torch_pos, floor_torch());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:torch");

    // A bottom slab does not cover the full upper face. Any further neighbour
    // shape update re-checks support through the placement predicate.
    place(&mut storage, torch_pos.above(), stone());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:air");
}

#[test]
fn wall_torch_breaks_when_its_wall_is_removed_from_any_orientation() {
    for (facing, dx, dz) in WALL_ORIENTATIONS {
        let mut storage = flat_storage(&format!("wall-remove-{facing}"));
        let torch_pos = BlockPos::new(2, -60, 2);
        let wall = torch_pos.offset(dx, 0, dz);
        place(&mut storage, wall, stone());
        place(&mut storage, torch_pos, wall_torch(facing));
        assert_eq!(name_at(&storage, torch_pos), "minecraft:wall_torch");

        place(&mut storage, wall, air());
        assert_eq!(
            name_at(&storage, torch_pos),
            "minecraft:air",
            "{facing} torch falls with its wall"
        );
        assert!(visited(&storage, torch_pos), "{facing} torch was updated");
        assert!(
            storage.tick_state().unsupported_neighbor_blocks.is_empty(),
            "{facing} torch support is implemented"
        );
    }
}

#[test]
fn wall_torch_survives_unless_its_own_wall_changes() {
    for (facing, dx, dz) in WALL_ORIENTATIONS {
        let mut storage = flat_storage(&format!("wall-stays-{facing}"));
        let torch_pos = BlockPos::new(2, -60, 2);
        let wall = torch_pos.offset(dx, 0, dz);
        place(&mut storage, wall, stone());
        place(&mut storage, torch_pos, wall_torch(facing));

        place(&mut storage, torch_pos.above(), stone());
        assert_eq!(
            name_at(&storage, torch_pos),
            "minecraft:wall_torch",
            "{facing} torch ignores an unrelated neighbour"
        );

        // A different solid block in the same wall is still support.
        place(
            &mut storage,
            wall,
            named_state("minecraft:cobblestone", &[]),
        );
        assert_eq!(
            name_at(&storage, torch_pos),
            "minecraft:wall_torch",
            "{facing} torch survives a solid wall replacement"
        );
    }
}

#[test]
fn wall_torch_breaks_when_its_wall_becomes_a_partial_face() {
    let mut storage = flat_storage("wall-slab");
    let torch_pos = BlockPos::new(2, -60, 2);
    let wall = torch_pos.offset(0, 0, 1);
    place(&mut storage, wall, stone());
    place(&mut storage, torch_pos, wall_torch("north"));

    // A bottom slab occupying the wall position does not occlude the full face.
    place(&mut storage, wall, bottom_slab());
    assert_eq!(name_at(&storage, torch_pos), "minecraft:air");
}

#[test]
fn rails_break_when_the_block_below_is_removed() {
    for name in folia_core::rail::RAIL_BLOCKS {
        let mut storage = flat_storage(&format!("rail-remove-{name}"));
        let rail_pos = BlockPos::new(2, -60, 2);
        place(&mut storage, rail_pos, rail(name));
        assert_eq!(&name_at(&storage, rail_pos), name);

        place(&mut storage, rail_pos.below(), air());
        assert_eq!(
            name_at(&storage, rail_pos),
            "minecraft:air",
            "{name} falls with its support"
        );
        assert!(visited(&storage, rail_pos), "{name} was updated");
        assert!(
            storage.tick_state().unsupported_neighbor_blocks.is_empty(),
            "rail support is an implemented neighbour behaviour"
        );
    }
}

#[test]
fn rail_uses_the_full_upper_face_support_rule() {
    let mut storage = flat_storage("rail-slab");
    let rail_pos = BlockPos::new(2, -60, 2);
    place(&mut storage, rail_pos.below(), bottom_slab());
    place(&mut storage, rail_pos, rail("minecraft:rail"));
    assert_eq!(name_at(&storage, rail_pos), "minecraft:rail");

    // The slab never covered a full upper face; another shape update lets the
    // shared placement predicate drop the rail.
    place(&mut storage, rail_pos.above(), stone());
    assert_eq!(name_at(&storage, rail_pos), "minecraft:air");
}

#[test]
fn rails_survive_when_supported_and_tick_normally() {
    let mut storage = flat_storage("rail-stays");
    let rail_pos = BlockPos::new(2, -60, 2);
    for x in [rail_pos.x() - 1, rail_pos.x(), rail_pos.x() + 1] {
        place(
            &mut storage,
            BlockPos::new(x, rail_pos.y(), rail_pos.z()),
            rail("minecraft:rail"),
        );
    }

    for tick in 1..=4 {
        tick::tick_world(&mut storage, tick).unwrap();
    }
    assert_eq!(name_at(&storage, rail_pos), "minecraft:rail");
    assert_eq!(
        folia_core::rail::shape_of(
            &storage
                .block_state_at(rail_pos.x(), rail_pos.y(), rail_pos.z())
                .unwrap()
        ),
        Some(folia_core::rail::RailShape::EastWest)
    );
}
