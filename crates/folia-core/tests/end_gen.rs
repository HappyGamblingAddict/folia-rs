//! Fixed-seed End generation tests.
//!
//! The central island must be solid and covered by air, the obsidian arrival
//! platform must exist at the vanilla `(100, 49, 0)` origin (including its
//! chunk-border half), and generation must be deterministic. Outer islands are
//! covered by the shipped `end_islands` density function.

use folia_core::chunk::Chunk;
use folia_core::worldgen::end::{EndGenerator, PLATFORM_ORIGIN};
use folia_core::worldgen::generator::{Dimension, generate_dimension};

const SEED: i64 = 12345;

const END_BIOMES: [&str; 5] = [
    "minecraft:the_end",
    "minecraft:end_highlands",
    "minecraft:end_midlands",
    "minecraft:small_end_islands",
    "minecraft:end_barrens",
];

fn generate(chunk_x: i32, chunk_z: i32) -> Chunk {
    EndGenerator::new(SEED)
        .unwrap()
        .generate_chunk(chunk_x, chunk_z)
        .unwrap()
}

fn top_solid(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
    for y in (0..128).rev() {
        let state = chunk.block_state(x, y, z).unwrap();
        if state.name != "minecraft:air" {
            return Some(y);
        }
    }
    None
}

#[test]
fn end_central_island_is_solid_with_air_above() {
    let chunk = generate(0, 0);
    let top = top_solid(&chunk, 0, 0).expect("central island has no terrain at (0,0)");
    assert!(
        (30..=100).contains(&top),
        "unexpected central-island top {top}"
    );
    assert_eq!(
        chunk.block_state(0, top, 0).unwrap().name,
        "minecraft:end_stone"
    );
    for y in top + 1..128 {
        assert_eq!(
            chunk.block_state(0, y, 0).unwrap().name,
            "minecraft:air",
            "terrain above the central island at y={y}"
        );
    }
}

#[test]
fn end_idle_sections_above_the_noise_column_are_air() {
    let chunk = generate(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            for y in 128..256 {
                assert_eq!(
                    chunk.block_state(x, y, z).unwrap().name,
                    "minecraft:air",
                    "unexpected block above the End noise column at ({x},{y},{z})"
                );
            }
        }
    }
}

#[test]
fn end_obsidian_platform_is_at_the_vanilla_origin() {
    let (origin_x, origin_y, origin_z) = PLATFORM_ORIGIN;
    let chunk_x = origin_x.div_euclid(16);
    let chunk_z = origin_z.div_euclid(16);
    let chunk = generate(chunk_x, chunk_z);
    let local_x = origin_x.rem_euclid(16) as usize;
    let local_z = origin_z.rem_euclid(16) as usize;
    assert_eq!(
        chunk
            .block_state(local_x, origin_y - 1, local_z)
            .unwrap()
            .name,
        "minecraft:obsidian",
        "missing platform obsidian at {PLATFORM_ORIGIN:?}"
    );
    for y in origin_y..origin_y + 3 {
        assert_eq!(
            chunk.block_state(local_x, y, local_z).unwrap().name,
            "minecraft:air",
            "platform headroom blocked at y={y}"
        );
    }

    // The platform spans z=-2..2 and therefore also writes into the chunk
    // north of the origin chunk.
    let north = generate(chunk_x, chunk_z - 1);
    let north_local_z = (origin_z - 2).rem_euclid(16) as usize;
    assert_eq!(
        north
            .block_state(local_x, origin_y - 1, north_local_z)
            .unwrap()
            .name,
        "minecraft:obsidian",
        "platform did not cross the chunk border"
    );
}

#[test]
fn end_biomes_are_from_the_shipped_source() {
    for (chunk_x, chunk_z) in [(0, 0), (64, 64), (70, -70)] {
        let chunk = generate(chunk_x, chunk_z);
        for x in 0..16 {
            for z in 0..16 {
                for y in [0, 64, 127, 128, 255] {
                    let biome = chunk.biome(x, y, z).unwrap();
                    assert!(
                        END_BIOMES.contains(&biome.as_str()),
                        "invalid End biome {biome} at chunk ({chunk_x},{chunk_z}) ({x},{y},{z})"
                    );
                }
            }
        }
    }
}

#[test]
fn end_generation_is_deterministic() {
    let first = generate(0, 0);
    let second = generate(0, 0);
    assert_eq!(first, second, "End generation must be deterministic");
}

#[test]
fn outer_islands_generate_end_stone() {
    let mut end_stone = 0;
    let mut non_the_end_biome = false;
    for chunk_x in 64..70 {
        for chunk_z in 64..70 {
            let chunk = generate(chunk_x, chunk_z);
            for x in 0..16 {
                for z in 0..16 {
                    for y in 0..128 {
                        if chunk.block_state(x, y, z).unwrap().name == "minecraft:end_stone" {
                            end_stone += 1;
                        }
                    }
                }
            }
            non_the_end_biome |= chunk.biome(0, 64, 0).unwrap() != "minecraft:the_end";
        }
    }
    assert!(end_stone > 0, "outer island ring generated no end stone");
    assert!(
        non_the_end_biome,
        "outer ring still used the central the_end biome"
    );
}

#[test]
fn dimension_entry_point_matches_the_end_generator() {
    let direct = generate(0, 0);
    let through_entry = generate_dimension(Dimension::End, 0, 0, SEED).unwrap();
    assert_eq!(direct, through_entry);
}

#[test]
fn end_dimension_type_matches_the_shipped_document() {
    assert_eq!(Dimension::End.min_y(), 0);
    assert_eq!(Dimension::End.height(), 256);
    assert_eq!(Dimension::End.logical_height(), 256);
    assert_eq!(Dimension::End.coordinate_scale(), 1.0);
    assert!(!Dimension::End.has_ceiling());
    assert!(Dimension::End.has_skylight());
    assert!((Dimension::End.ambient_light() - 0.25).abs() < f32::EPSILON);
}
