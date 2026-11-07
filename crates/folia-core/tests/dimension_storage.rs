//! Dimension storage, portal math, and frame detection.

use folia_core::anvil::Compression;
use folia_core::dimension::{
    DimensionStorages, PortalBlockView, generator_dimension, is_nether_portal, portal_destination,
    scaled_coordinates, storage_dimension,
};
use folia_core::world::Dimension;
use folia_core::worldgen::generator::{self, Dimension as GeneratorDimension};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("folia-dimension-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn dimensions_use_vanilla_region_directories() {
    let root = PathBuf::from("world");
    assert_eq!(Dimension::Overworld.region_dir(&root), root.join("region"));
    assert_eq!(
        Dimension::Nether.region_dir(&root),
        root.join("dimensions/minecraft/the_nether/region")
    );
    assert_eq!(
        Dimension::End.region_dir(&root),
        root.join("dimensions/minecraft/the_end/region")
    );
}

#[test]
fn dimension_dispatch_types_round_trip() {
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        assert_eq!(storage_dimension(generator_dimension(dimension)), dimension);
    }
    assert_eq!(
        storage_dimension(GeneratorDimension::Nether),
        Dimension::Nether
    );
}

#[test]
fn generated_nether_chunks_are_persisted_per_dimension() {
    let root = scratch("nether");
    let mut storages = DimensionStorages::new(&root);
    let chunk = storages
        .ensure_chunk(Dimension::Nether, 0, 0, 12345, Compression::Zlib)
        .expect("nether chunk generates");

    let direct = generator::generate_dimension(GeneratorDimension::Nether, 0, 0, 12345)
        .expect("direct nether chunk");
    assert_eq!(chunk, direct);

    let path = Dimension::Nether.region_dir(&root).join("r.0.0.mca");
    assert!(path.exists(), "nether region file was not written");

    let loaded = storages
        .load_chunk(Dimension::Nether, 0, 0)
        .expect("nether load")
        .expect("stored nether chunk");
    assert_eq!(loaded, direct);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn overworld_and_end_stores_do_not_cross_talk() {
    let root = scratch("isolation");
    let mut storages = DimensionStorages::new(&root);
    let overworld = storages
        .ensure_chunk(Dimension::Overworld, 0, 0, 7, Compression::Zlib)
        .expect("overworld chunk");
    let end = storages
        .ensure_chunk(Dimension::End, 0, 0, 7, Compression::Zlib)
        .expect("end chunk");

    assert_ne!(
        overworld, end,
        "the two dimensions generated the same chunk"
    );
    assert!(
        Dimension::Overworld
            .region_dir(&root)
            .join("r.0.0.mca")
            .exists()
    );
    assert!(Dimension::End.region_dir(&root).join("r.0.0.mca").exists());

    let loaded_end = storages
        .load_chunk(Dimension::End, 0, 0)
        .expect("end load")
        .expect("stored end chunk");
    assert_eq!(loaded_end, end);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn portal_scaling_matches_vanilla_and_handles_negatives() {
    assert_eq!(
        scaled_coordinates(Dimension::Overworld, Dimension::Nether, 80.0, 0.0),
        (10.0, 0.0)
    );
    assert_eq!(
        scaled_coordinates(Dimension::Overworld, Dimension::Nether, -9.0, -17.0),
        (-2.0, -3.0)
    );
    assert_eq!(
        scaled_coordinates(Dimension::Nether, Dimension::Overworld, -2.0, 3.0),
        (-16.0, 24.0)
    );
    assert_eq!(
        scaled_coordinates(Dimension::Nether, Dimension::End, 4.0, 5.0),
        (4.0, 5.0)
    );
}

#[test]
fn portal_destinations_use_the_end_platform() {
    assert_eq!(
        portal_destination(Dimension::Overworld, Dimension::End, 9.0, 70.0, -9.0),
        folia_core::dimension::END_PLATFORM_POSITION
    );
    let nether = portal_destination(Dimension::Overworld, Dimension::Nether, 80.0, 64.0, -8.0);
    assert_eq!(nether, [10.0, 64.0, -1.0]);
}

struct FakeView(HashMap<(i32, i32, i32), &'static str>);

impl PortalBlockView for FakeView {
    fn block_name(&self, x: i32, y: i32, z: i32) -> Option<String> {
        self.0.get(&(x, y, z)).map(|name| (*name).to_owned())
    }
}

fn x_axis_portal() -> FakeView {
    let mut view = HashMap::new();
    for y in 64..=66 {
        for x in 0..=1 {
            view.insert((x, y, 0), "minecraft:air");
        }
    }
    for y in 63..=67 {
        view.insert((-1, y, 0), "minecraft:obsidian");
        view.insert((2, y, 0), "minecraft:obsidian");
    }
    for x in -1..=2 {
        view.insert((x, 63, 0), "minecraft:obsidian");
        view.insert((x, 67, 0), "minecraft:obsidian");
    }
    FakeView(view)
}

#[test]
fn detects_a_complete_nether_portal_frame() {
    let view = x_axis_portal();
    assert!(is_nether_portal(&view, 0, 64, 0));
    assert!(
        !is_nether_portal(&view, 0, 65, 0),
        "not the interior origin"
    );
}

#[test]
fn rejects_a_frame_with_a_missing_corner() {
    let mut view = x_axis_portal();
    view.0.remove(&(2, 67, 0));
    assert!(!is_nether_portal(&view, 0, 64, 0));
}

#[test]
fn detects_a_z_axis_frame() {
    let mut view = HashMap::new();
    for y in 64..=66 {
        for z in 0..=1 {
            view.insert((0, y, z), "minecraft:air");
        }
    }
    for y in 63..=67 {
        view.insert((0, y, -1), "minecraft:obsidian");
        view.insert((0, y, 2), "minecraft:obsidian");
    }
    for z in -1..=2 {
        view.insert((0, 63, z), "minecraft:obsidian");
        view.insert((0, 67, z), "minecraft:obsidian");
    }
    let view = FakeView(view);
    assert!(is_nether_portal(&view, 0, 64, 0));
}
