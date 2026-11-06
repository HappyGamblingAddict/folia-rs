//! Fixed-seed Nether generation tests.
//!
//! These pin the structural contract of `worldgen::nether`: bedrock floor and
//! roof at the build limits, a lava sea below y=32, netherrack terrain in
//! between, the shipped Nether multi-noise biomes, and deterministic output.

use folia_core::chunk::Chunk;
use folia_core::worldgen::generator::{Dimension, generate_dimension};
use folia_core::worldgen::nether::{HEIGHT, NetherGenerator, SEA_LEVEL};

const SEED: i64 = 12345;
const CHUNK_X: i32 = 0;
const CHUNK_Z: i32 = -3;

const NETHER_BIOMES: [&str; 5] = [
    "minecraft:nether_wastes",
    "minecraft:soul_sand_valley",
    "minecraft:crimson_forest",
    "minecraft:warped_forest",
    "minecraft:basalt_deltas",
];

fn generate() -> Chunk {
    NetherGenerator::new(SEED)
        .unwrap()
        .generate_chunk(CHUNK_X, CHUNK_Z)
        .unwrap()
}

#[test]
fn nether_has_bedrock_floor_and_roof() {
    let chunk = generate();
    for x in 0..16 {
        for z in 0..16 {
            assert_eq!(
                chunk.block_state(x, 0, z).unwrap().name,
                "minecraft:bedrock",
                "missing floor bedrock at ({x},0,{z})"
            );
            assert_eq!(
                chunk.block_state(x, 127, z).unwrap().name,
                "minecraft:bedrock",
                "missing roof bedrock at ({x},127,{z})"
            );
        }
    }
}

#[test]
fn nether_lava_sea_is_at_the_vanilla_level() {
    let chunk = generate();
    let mut lava_at_top = 0;
    let mut lava_below = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..SEA_LEVEL {
                let state = chunk.block_state(x, y, z).unwrap();
                if state.name == "minecraft:lava" {
                    if y == SEA_LEVEL - 1 {
                        lava_at_top += 1;
                    }
                    lava_below += 1;
                } else {
                    assert_ne!(
                        state.name, "minecraft:air",
                        "air below the lava sea at ({x},{y},{z})"
                    );
                }
            }
        }
    }
    assert!(lava_at_top > 0, "expected open lava at y={}", SEA_LEVEL - 1);
    assert!(lava_below > lava_at_top);
}

#[test]
fn nether_terrain_is_netherrack_and_stays_below_the_roof() {
    let chunk = generate();
    let mut netherrack = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 1..127 {
                let state = chunk.block_state(x, y, z).unwrap();
                if state.name == "minecraft:netherrack" {
                    netherrack += 1;
                }
            }
            for y in HEIGHT..256 {
                assert_eq!(
                    chunk.block_state(x, y, z).unwrap().name,
                    "minecraft:air",
                    "generated terrain above the 128-high column at ({x},{y},{z})"
                );
            }
        }
    }
    assert!(
        netherrack > 16 * 16 * 16,
        "expected substantial netherrack terrain, found {netherrack}"
    );
}

#[test]
fn nether_biomes_are_from_the_shipped_parameter_list() {
    let chunk = generate();
    let mut sampled = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in [0, 32, 64, 96, 127] {
                let biome = chunk.biome(x, y, z).unwrap();
                assert!(
                    NETHER_BIOMES.contains(&biome.as_str()),
                    "invalid Nether biome {biome} at ({x},{y},{z})"
                );
                sampled += 1;
            }
        }
    }
    assert_eq!(sampled, 16 * 16 * 5);
}

#[test]
fn nether_quartz_and_glowstone_features_place() {
    let generator = NetherGenerator::new(SEED).unwrap();
    let quartz_chunk = generator.generate_chunk(0, -4).unwrap();
    let mut quartz = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..128 {
                if quartz_chunk.block_state(x, y, z).unwrap().name == "minecraft:nether_quartz_ore"
                {
                    quartz += 1;
                }
            }
        }
    }
    assert!(quartz > 0, "ore_quartz_nether placed no quartz");

    let glowstone_chunk = generator.generate_chunk(2, 3).unwrap();
    let mut glowstone = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..128 {
                if glowstone_chunk.block_state(x, y, z).unwrap().name == "minecraft:glowstone" {
                    glowstone += 1;
                }
            }
        }
    }
    assert!(glowstone > 0, "glowstone_extra placed no glowstone");
}

#[test]
fn nether_generation_is_deterministic() {
    let first = generate();
    let second = generate();
    assert_eq!(first, second, "Nether generation must be deterministic");
}

#[test]
fn dimension_entry_point_matches_the_nether_generator() {
    let direct = generate();
    let through_entry = generate_dimension(Dimension::Nether, CHUNK_X, CHUNK_Z, SEED).unwrap();
    assert_eq!(direct, through_entry);
}

#[test]
fn overworld_entry_point_is_byte_identical_to_the_generator() {
    use folia_core::worldgen::generator::WorldGenerator;

    let direct = WorldGenerator::overworld(SEED)
        .unwrap()
        .generate_chunk(0, -1)
        .unwrap();
    let through_entry = generate_dimension(Dimension::Overworld, 0, -1, SEED).unwrap();
    assert_eq!(
        direct, through_entry,
        "the dimension entry point must not change the Overworld path"
    );
}

#[test]
fn nether_dimension_type_matches_the_shipped_document() {
    assert_eq!(Dimension::Nether.min_y(), 0);
    assert_eq!(Dimension::Nether.height(), 256);
    assert_eq!(Dimension::Nether.logical_height(), 128);
    assert_eq!(Dimension::Nether.coordinate_scale(), 8.0);
    assert!(Dimension::Nether.has_ceiling());
    assert!(!Dimension::Nether.has_skylight());
}
