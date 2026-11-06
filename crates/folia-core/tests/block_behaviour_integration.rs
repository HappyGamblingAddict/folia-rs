//! End-to-end checks that lighting and player physics consume the generated
//! per-state block-behaviour table instead of the old hand-written names/tags.

use folia_core::anvil::Compression;
use folia_core::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use folia_core::light::SectionLight;
use folia_core::player_physics::{DEFAULT_BLOCK_FRICTION, PhysicsWorld, StoragePhysicsWorld};
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Section used by the light tests; world Y `64..=79`.
const SECTION_Y: i8 = 4;
/// A Y inside [`SECTION_Y`] with room above and below.
const TEST_Y: i32 = 70;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn air_chunk(x: i32, z: i32) -> Chunk {
    Chunk {
        x,
        z,
        sections: vec![ChunkSection {
            y: SECTION_Y,
            block_states: PalettedContainer {
                palette: vec![state("minecraft:air")],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        }],
    }
}

fn fill_layer(chunk: &mut Chunk, y: i32, name: &str) {
    let block = state(name);
    for x in 0..16 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, y, z, block.clone()));
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-behaviour-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn block_light_uses_the_generated_emission_table() {
    let mut torch = air_chunk(0, 0);
    assert!(torch.set_block_state(8, TEST_Y, 8, state("minecraft:torch")));
    torch.recompute_light();
    assert_eq!(torch.block_light(8, TEST_Y, 8), Some(14), "torch source");
    assert_eq!(torch.block_light(7, TEST_Y, 8), Some(13), "one block away");
    assert_eq!(
        torch.block_light(8, TEST_Y, 15),
        Some(7),
        "seven blocks away"
    );

    for name in ["minecraft:glowstone", "minecraft:lava"] {
        let mut chunk = air_chunk(0, 0);
        assert!(chunk.set_block_state(8, TEST_Y, 8, state(name)));
        chunk.recompute_light();
        assert_eq!(
            chunk.block_light(8, TEST_Y, 8),
            Some(15),
            "{name} should emit 15"
        );
    }
}

#[test]
fn glass_is_transparent_while_water_attenuates_sky_light() {
    let mut glass = air_chunk(0, 0);
    fill_layer(&mut glass, TEST_Y, "minecraft:glass");
    glass.recompute_light();
    assert_eq!(
        glass.sky_light(8, TEST_Y - 1, 8),
        Some(15),
        "glass transmits sky light unchanged"
    );
    assert_eq!(
        glass.sky_light(8, 64, 8),
        Some(15),
        "the column below glass stays fully lit"
    );

    let mut water = air_chunk(0, 0);
    fill_layer(&mut water, TEST_Y, "minecraft:water");
    water.recompute_light();
    assert_eq!(water.sky_light(8, TEST_Y, 8), Some(14));
    assert_eq!(water.sky_light(8, TEST_Y - 1, 8), Some(13));
    assert_eq!(water.sky_light(8, TEST_Y - 2, 8), Some(12));
}

#[test]
fn sky_light_reaches_under_a_stone_overhang_through_a_glass_shaft() {
    let mut chunk = air_chunk(0, 0);
    fill_layer(&mut chunk, TEST_Y, "minecraft:stone");
    assert!(chunk.set_block_state(8, TEST_Y, 8, state("minecraft:glass")));
    chunk.recompute_light();

    assert_eq!(
        chunk.sky_light(8, TEST_Y - 1, 8),
        Some(15),
        "the glass shaft keeps full sky light"
    );
    assert_eq!(
        chunk.sky_light(7, TEST_Y - 1, 8),
        Some(14),
        "one step under the roof edge is 14"
    );
    assert_eq!(
        chunk.sky_light(0, TEST_Y - 1, 8),
        Some(7),
        "light falls by one per block along the roof"
    );
    assert_eq!(
        chunk.sky_light(0, TEST_Y, 8),
        Some(0),
        "the stone roof blocks sky light"
    );
    assert_eq!(
        chunk.sky_light(0, TEST_Y + 1, 8),
        Some(15),
        "sky above the roof is still full"
    );
}

fn physics_chunk() -> Chunk {
    let mut chunk = air_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 0, z, state("minecraft:stone")));
        }
    }
    for x in 0..4 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 0, z, state("minecraft:ice")));
        }
    }
    for x in 4..8 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 0, z, state("minecraft:slime_block")));
        }
    }
    for x in 8..12 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 0, z, state("minecraft:blue_ice")));
        }
    }

    // Passable states: fluid and replaceable plants.
    assert!(chunk.set_block_state(1, 1, 1, state("minecraft:water")));
    assert!(chunk.set_block_state(2, 1, 2, state("minecraft:short_grass")));
    assert!(chunk.set_block_state(3, 1, 3, state("minecraft:poppy")));

    // Collidable states.
    assert!(chunk.set_block_state(5, 1, 5, state("minecraft:stone")));
    assert!(chunk.set_block_state(6, 1, 6, state("minecraft:oak_leaves")));

    // Movement modifiers.
    assert!(chunk.set_block_state(9, 0, 9, state("minecraft:soul_sand")));
    assert!(chunk.set_block_state(10, 0, 10, state("minecraft:honey_block")));
    chunk
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1.0e-6
}

#[test]
fn player_physics_queries_the_generated_block_behaviour() {
    let root = temp_dir("physics");
    let region_dir = root.join("region");
    {
        let mut storage = WorldStorage::new(&region_dir);
        storage
            .save_chunk(&physics_chunk(), Compression::Zlib)
            .expect("save physics chunk");
        let mut world = StoragePhysicsWorld::new(&mut storage);

        assert!(
            close(world.friction(1, 0, 1), 0.98),
            "ice friction {}",
            world.friction(1, 0, 1)
        );
        assert!(
            close(world.friction(5, 0, 5), 0.8),
            "slime friction {}",
            world.friction(5, 0, 5)
        );
        assert!(
            close(world.friction(11, 0, 5), 0.989),
            "blue ice friction {}",
            world.friction(11, 0, 5)
        );
        assert!(
            close(world.friction(14, 0, 5), DEFAULT_BLOCK_FRICTION),
            "stone friction {}",
            world.friction(14, 0, 5)
        );
        assert!(
            close(world.speed_factor(9, 0, 9), 0.4),
            "soul sand speed factor {}",
            world.speed_factor(9, 0, 9)
        );
        assert!(
            close(world.jump_factor(10, 0, 10), 0.5),
            "honey jump factor {}",
            world.jump_factor(10, 0, 10)
        );

        assert!(
            world.collision_box(1, 1, 1).is_none(),
            "water has no collision"
        );
        assert!(
            world.collision_box(2, 1, 2).is_none(),
            "short grass has no collision"
        );
        assert!(
            world.collision_box(3, 1, 3).is_none(),
            "a poppy has no collision"
        );
        assert!(
            world.collision_box(5, 1, 5).is_some(),
            "stone has a collision box"
        );
        assert!(
            world.collision_box(6, 1, 6).is_some(),
            "leaves have a collision box"
        );
        assert!(
            world.collision_box(11, 0, 5).is_some(),
            "blue ice has a collision box"
        );

        assert!(!world.blocks_motion(1, 1, 1), "water does not block motion");
        assert!(world.blocks_motion(1, 0, 1), "ice blocks motion");
        assert!(!world.is_solid(2, 1, 2), "short grass is not solid");
        assert!(world.is_solid(5, 1, 5), "stone is solid");
        assert!(world.is_solid(6, 1, 6), "leaves are solid");
    }
    std::fs::remove_dir_all(root).unwrap();
}
