use folia_core::anvil::Compression;
use folia_core::block_update::set_block;
use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use folia_core::light::{LightScene, LightSectionPos};
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: vec![],
    }
}

fn stone() -> BlockState {
    BlockState {
        name: "minecraft:stone".into(),
        properties: vec![],
    }
}

fn glass() -> BlockState {
    BlockState {
        name: "minecraft:glass".into(),
        properties: vec![],
    }
}

fn water() -> BlockState {
    BlockState {
        name: "minecraft:water".into(),
        properties: vec![],
    }
}

fn torch() -> BlockState {
    BlockState {
        name: "minecraft:torch".into(),
        properties: vec![],
    }
}

/// Default redstone torch (`lit=true`) emits 7, so its radius of seven steps
/// stays inside one 16³ section when placed in the middle. Opacity is zero, so
/// it leaves sky light alone.
fn redstone_torch() -> BlockState {
    BlockState {
        name: "minecraft:redstone_torch".into(),
        properties: vec![],
    }
}

fn small_air_chunk(x: i32, z: i32) -> Chunk {
    // Four sections (y=0..63) keep the reference recompute cheap; every
    // assertion below stays inside this range.
    chunk_with_sections(x, z, 0..=3)
}

fn chunk_with_sections(x: i32, z: i32, sections: impl IntoIterator<Item = i8>) -> Chunk {
    let sections = sections
        .into_iter()
        .map(|y| ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![air()],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: folia_core::light::SectionLight::default(),
        })
        .collect();
    Chunk { x, z, sections }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-light-inc-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Snapshot every light nibble in a chunk so idempotence comparisons do not
/// depend on which optional layers happen to be materialized.
fn light_snapshot(chunk: &Chunk) -> Vec<(i32, u8, u8)> {
    let mut snapshot = Vec::new();
    for section in &chunk.sections {
        let base_y = i32::from(section.y) * 16;
        for y in base_y..base_y + 16 {
            for z in 0..16usize {
                for x in 0..16usize {
                    let sky = chunk.sky_light(x, y, z).unwrap_or(0);
                    let block = chunk.block_light(x, y, z).unwrap_or(0);
                    snapshot.push((y, sky, block));
                }
            }
        }
    }
    snapshot
}

#[test]
fn torch_removal_clears_its_radius_and_keeps_neighbour_light() {
    let mut chunk = small_air_chunk(0, 0);
    {
        let mut scene = LightScene::new(&mut chunk, &mut []);
        scene.recompute_all();
        scene.clear_dirty();

        assert!(scene.set_block_state(1, 8, 1, torch()));
        assert!(scene.set_block_state(14, 8, 14, torch()));
        assert_eq!(scene.block_level(1, 8, 1), 14);
        assert_eq!(scene.block_level(2, 8, 1), 13);
        assert_eq!(scene.block_level(14, 8, 14), 14);

        assert!(scene.set_block_state(1, 8, 1, air()));
        // A's former radius is outside B's radius and must be fully dark.
        assert_eq!(scene.block_level(1, 8, 1), 0);
        assert_eq!(scene.block_level(2, 8, 1), 0);
        assert_eq!(scene.block_level(1, 8, 2), 0);
        assert_eq!(scene.block_level(3, 8, 2), 0);
        assert_eq!(scene.block_level(1, 15, 1), 0);
        // B's own light was not touched by A's decrease.
        assert_eq!(scene.block_level(14, 8, 14), 14);
        assert_eq!(scene.block_level(13, 8, 14), 13);
        assert_eq!(scene.block_level(14, 8, 13), 13);
        assert_eq!(scene.block_level(14, 9, 14), 13);

        // Replacing a torch with an opaque block runs the same decrease path.
        assert!(scene.set_block_state(14, 8, 14, stone()));
        assert_eq!(scene.block_level(14, 8, 14), 0);
        assert_eq!(scene.block_level(13, 8, 14), 0);
        assert_eq!(scene.block_level(14, 8, 13), 0);
    }
}

#[test]
fn glass_and_opaque_placement_update_the_sky_column() {
    let mut chunk = small_air_chunk(0, 0);
    let mut scene = LightScene::new(&mut chunk, &mut []);
    scene.recompute_all();
    scene.clear_dirty();

    assert_eq!(scene.sky_level(8, 7, 8), 15);
    assert_eq!(scene.sky_level(8, 8, 8), 15);

    // Glass has zero light attenuation, so it is invisible to the sky engine.
    assert!(scene.set_block_state(8, 8, 8, glass()));
    assert_eq!(scene.sky_level(8, 8, 8), 15);
    assert_eq!(scene.sky_level(8, 7, 8), 15);
    assert!(scene.dirty().is_empty(), "glass does not change sky light");

    // An opaque block removes the direct-sky run for the whole column.
    assert!(scene.set_block_state(8, 8, 8, stone()));
    assert_eq!(scene.sky_level(8, 8, 8), 0);
    assert_eq!(
        scene.sky_level(8, 7, 8),
        14,
        "one step below the block is lit from the open neighbouring columns"
    );
    assert_eq!(scene.sky_level(8, 9, 8), 15);

    // Removing the block recomputes the column source and refills downward.
    assert!(scene.set_block_state(8, 8, 8, air()));
    assert_eq!(scene.sky_level(8, 8, 8), 15);
    assert_eq!(scene.sky_level(8, 7, 8), 15);
}

#[test]
fn edit_at_chunk_border_lights_the_neighbour() {
    let mut center = small_air_chunk(0, 0);
    let east = small_air_chunk(1, 0);
    let mut neighbours = [east];
    {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.recompute_all();
        scene.clear_dirty();

        assert!(scene.set_block_state(15, 8, 8, torch()));
        assert_eq!(scene.block_level(15, 8, 8), 14);
        assert_eq!(scene.block_level(14, 8, 8), 13);
        assert_eq!(
            scene.block_level(16, 8, 8),
            13,
            "world x=16 is local x=0 in the east chunk"
        );

        let dirty: Vec<LightSectionPos> = scene.dirty().iter().copied().collect();
        assert!(
            dirty.contains(&LightSectionPos::new(0, 0, 0)),
            "source section is dirty, got {dirty:?}"
        );
        assert!(
            dirty.contains(&LightSectionPos::new(1, 0, 0)),
            "neighbour section is dirty, got {dirty:?}"
        );
    }
    assert_eq!(neighbours[0].block_light(0, 8, 8), Some(13));
    assert_eq!(neighbours[0].block_light(1, 8, 8), Some(12));

    // Removing the border torch must also reach into the neighbour.
    let mut scene = LightScene::new(&mut center, &mut neighbours);
    scene.clear_dirty();
    assert!(scene.set_block_state(15, 8, 8, air()));
    assert_eq!(scene.block_level(16, 8, 8), 0);
}

#[test]
fn single_section_edit_reports_exact_dirty_sections() {
    let mut chunk = small_air_chunk(0, 0);
    let mut scene = LightScene::new(&mut chunk, &mut []);
    scene.recompute_all();
    scene.clear_dirty();

    // Section (0, 0, 0) spans y=0..15. A radius-7 source at (8, 8, 8) cannot
    // reach any other section.
    assert!(scene.set_block_state(8, 8, 8, redstone_torch()));
    assert_eq!(scene.block_level(8, 8, 8), 7);
    assert_eq!(scene.block_level(1, 8, 8), 0);
    assert_eq!(scene.block_level(2, 8, 8), 1);
    assert_eq!(scene.block_level(8, 15, 8), 0);
    assert_eq!(scene.block_level(8, 14, 8), 1);

    let dirty: Vec<LightSectionPos> = scene.dirty().iter().copied().collect();
    assert_eq!(dirty, vec![LightSectionPos::new(0, 0, 0)]);
}

#[test]
fn repeated_identical_edits_are_idempotent() {
    let mut chunk = small_air_chunk(0, 0);
    let baseline = {
        let mut scene = LightScene::new(&mut chunk, &mut []);
        scene.recompute_all();
        scene.clear_dirty();
        assert!(scene.set_block_state(8, 8, 8, torch()));
        let dirty: Vec<LightSectionPos> = scene.dirty().iter().copied().collect();
        assert!(!dirty.is_empty());
        drop(scene);
        light_snapshot(&chunk)
    };

    let mut scene = LightScene::new(&mut chunk, &mut []);
    scene.clear_dirty();
    assert!(
        !scene.set_block_state(8, 8, 8, torch()),
        "setting the same state is a no-op"
    );
    assert!(
        scene.dirty().is_empty(),
        "a no-op edit must not report dirty sections"
    );
    assert_eq!(scene.block_level(8, 8, 8), 14);
    assert_eq!(scene.block_level(7, 8, 8), 13);

    assert!(scene.set_block_state(8, 8, 8, air()));
    assert_eq!(scene.block_level(8, 8, 8), 0);
    assert!(scene.set_block_state(8, 8, 8, torch()));
    drop(scene);

    assert_eq!(
        light_snapshot(&chunk),
        baseline,
        "remove + replace must restore the same nibbles"
    );
}

#[test]
fn incremental_matches_bulk_recompute_on_small_scene() {
    let mut center = small_air_chunk(0, 0);
    let mut east = small_air_chunk(1, 0);

    // A partial roof in the centre and a border wall in the neighbour give the
    // edits something non-trivial to re-light.
    for x in 0..16usize {
        for z in 0..16usize {
            if (x + z) % 5 != 0 {
                assert!(center.set_block_state(x, 30, z, stone()));
            }
        }
    }
    for z in 0..16usize {
        assert!(east.set_block_state(0, 31, z, stone()));
        assert!(east.set_block_state(5, 29, z, glass()));
    }

    let mut neighbours = [east];
    {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.recompute_all();
        scene.clear_dirty();

        assert!(scene.set_block_state(4, 30, 4, air()));
        assert!(scene.set_block_state(3, 31, 3, torch()));
        assert!(scene.set_block_state(8, 32, 8, glass()));
        assert!(scene.set_block_state(3, 31, 3, air()));
        assert!(scene.set_block_state(15, 40, 8, stone()));
        // A small secondary roof exercises the sky column decrease path in
        // columns that were fully open to the sky.
        for x in 5..8 {
            for z in 5..8 {
                assert!(scene.set_block_state(x, 40, z, stone()));
            }
        }
        // Non-zero attenuation (water 1) changes a column source without
        // fully blocking it; replacing it with opaque stone must recalc again.
        assert!(scene.set_block_state(10, 32, 10, water()));
        assert!(scene.set_block_state(10, 32, 10, stone()));
        assert!(scene.set_block_state(10, 32, 10, air()));
        assert!(scene.set_block_state(18, 32, 4, torch()));
        assert!(scene.set_block_state(0, 40, 8, torch()));
    }

    let incremental_center = center.clone();
    let incremental_east = neighbours[0].clone();

    let mut bulk_center = incremental_center.clone();
    let mut bulk_neighbours = [incremental_east.clone()];
    {
        let mut scene = LightScene::new(&mut bulk_center, &mut bulk_neighbours);
        scene.recompute_all();
    }

    assert_chunk_light_eq(&incremental_center, &bulk_center);
    assert_chunk_light_eq(&incremental_east, &bulk_neighbours[0]);
}

fn assert_chunk_light_eq(left: &Chunk, right: &Chunk) {
    let low = left
        .sections
        .iter()
        .map(|section| section.y)
        .min()
        .unwrap_or(0);
    let high = left
        .sections
        .iter()
        .map(|section| section.y)
        .max()
        .unwrap_or(0);
    for section_y in low..=high {
        let base_y = i32::from(section_y) * 16;
        for y in base_y..base_y + 16 {
            for z in 0..16usize {
                for x in 0..16usize {
                    assert_eq!(
                        left.sky_light(x, y, z),
                        right.sky_light(x, y, z),
                        "sky light differs at ({x}, {y}, {z})"
                    );
                    assert_eq!(
                        left.block_light(x, y, z),
                        right.block_light(x, y, z),
                        "block light differs at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }
}

#[test]
fn border_edit_persists_neighbour_light_through_storage() {
    let root = temp_dir("border-persist");
    let region_dir = root.join("region");
    let mut storage = WorldStorage::new(&region_dir);

    let mut center = small_air_chunk(0, 0);
    center.recompute_light();
    storage
        .save_chunk(&center, Compression::Zlib)
        .expect("save centre");
    let mut east = small_air_chunk(1, 0);
    east.recompute_light();
    storage
        .save_chunk(&east, Compression::Zlib)
        .expect("save east");

    let change = set_block(&mut storage, 15, 8, 8, torch())
        .expect("set torch")
        .expect("torch changed");
    assert_eq!(change.x, 15);
    assert!(!change.light.is_empty(), "centre light payload is present");
    assert!(
        change
            .dirty_sections
            .contains(&LightSectionPos::new(1, 0, 0)),
        "east border section is marked dirty"
    );
    assert_eq!(change.neighbour_light.len(), 1);
    assert_eq!(change.neighbour_light[0].chunk_x, 1);

    let reloaded_east = storage
        .load_chunk(1, 0)
        .expect("load east")
        .expect("east chunk");
    assert_eq!(
        reloaded_east.block_light(0, 8, 8),
        Some(13),
        "neighbour light reached the stored chunk"
    );
    assert_eq!(reloaded_east.block_light(1, 8, 8), Some(12));

    // The stored NBT carries the neighbour's updated layer too.
    let encoded = encode_chunk(&reloaded_east).expect("encode east");
    let decoded = decode_chunk(&encoded).expect("decode east");
    assert_eq!(decoded.block_light(0, 8, 8), Some(13));

    storage.flush().expect("flush");
    let mut reopened = WorldStorage::new(&region_dir);
    let reopened_east = reopened
        .load_chunk(1, 0)
        .expect("reopen east")
        .expect("east chunk");
    assert_eq!(reopened_east.block_light(0, 8, 8), Some(13));
    let reopened_center = reopened
        .load_chunk(0, 0)
        .expect("reopen centre")
        .expect("centre chunk");
    assert_eq!(reopened_center.block_light(15, 8, 8), Some(14));

    std::fs::remove_dir_all(root).unwrap();
}

/// A scene edit that only touches a neighbour chunk still persists it and
/// returns a payload for that chunk, even though the centre is untouched.
#[test]
fn neighbour_only_edit_marks_that_chunk_dirty() {
    let mut center = small_air_chunk(0, 0);
    let east = small_air_chunk(1, 0);
    let mut neighbours = [east];

    let dirty = {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.recompute_all();
        scene.clear_dirty();
        assert!(scene.set_block_state(17, 8, 8, torch()));
        scene.dirty().clone()
    };

    assert!(
        dirty.contains(&LightSectionPos::new(1, 0, 0)),
        "neighbour source section is dirty"
    );
    assert_eq!(neighbours[0].block_light(1, 8, 8), Some(14));
    assert_eq!(neighbours[0].block_light(0, 8, 8), Some(13));

    // DirtySections is per-section and deterministic; re-running the same edit
    // reports nothing new.
    let mut scene = LightScene::new(&mut center, &mut neighbours);
    scene.clear_dirty();
    assert!(!scene.set_block_state(17, 8, 8, torch()));
    assert!(scene.dirty().is_empty());
}
