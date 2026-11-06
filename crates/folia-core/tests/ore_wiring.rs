//! Focused tests for ore, scattered-ore, and ore-vein generator wiring.
//!
//! The parity numbers are pinned against the Java fixture in
//! `assets/java-chunk-0--1.txt` (seed 12345, chunk (0, -1)). Exact-name
//! agreement was 92.01% (5653/6144) before the `underground_ores` step was
//! wired and is 95.80% (5886/6144) with ores, scattered ore, and the
//! `OreVeinifier` filler enabled. The tests below lock in the enabled default
//! and the main behavioral effects rather than a brittle single block.

use folia_core::worldgen::feature::Decoration;
use folia_core::worldgen::generator::WorldGenerator;
use std::collections::HashMap;

const JAVA_CHUNK: &str = include_str!("../assets/java-chunk-0--1.txt");
const CHUNK_X: i32 = 0;
const CHUNK_Z: i32 = -1;
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;
const SIDE: i32 = 16;

const BASE_STEPS: [Decoration; 3] = [
    Decoration::UndergroundDecoration,
    Decoration::VegetalDecoration,
    Decoration::TopLayerModification,
];

type JavaBlocks = HashMap<(i32, i32, i32), &'static str>;

fn java_fixture() -> (Vec<(i32, i32)>, JavaBlocks) {
    let mut columns = Vec::new();
    let mut blocks = HashMap::new();
    for raw in JAVA_CHUNK.lines() {
        let line = raw.trim_end();
        let mut parts = line.split('\t');
        if parts.next() != Some("column") {
            continue;
        }
        let x: i32 = parts.next().unwrap().parse().unwrap();
        let z: i32 = parts.next().unwrap().parse().unwrap();
        columns.push((x, z));
        for entry in parts {
            if entry.is_empty() {
                continue;
            }
            let (y, block) = entry.split_once('=').unwrap();
            blocks.insert((x, z, y.parse().unwrap()), block);
        }
    }
    (columns, blocks)
}

fn block_class(name: &str) -> u8 {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
        "minecraft:water" => 2,
        "minecraft:lava" => 3,
        _ => 1,
    }
}

fn ore_like(name: &str) -> bool {
    name.contains("_ore") || name.starts_with("minecraft:raw_")
}

fn count_named(chunk: &folia_core::chunk::Chunk, name: &str) -> usize {
    let mut count = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MIN_Y + 384 {
                if chunk
                    .block_state(x, y, z)
                    .is_some_and(|state| state.name == name)
                {
                    count += 1;
                }
            }
        }
    }
    count
}

#[test]
fn underground_ores_is_wired_by_default() {
    assert!(
        WorldGenerator::DEFAULT_DECORATION_STEPS.contains(&Decoration::UndergroundOres),
        "ore wiring is the documented default"
    );

    let generator = WorldGenerator::overworld(12345).unwrap();
    let chunk = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();

    let ores: usize = (0..16)
        .flat_map(|x| (0..16).flat_map(move |z| (MIN_Y..MIN_Y + 384).map(move |y| (x, y, z))))
        .filter(|&(x, y, z)| {
            chunk
                .block_state(x, y, z)
                .is_some_and(|state| ore_like(&state.name))
        })
        .count();
    assert!(
        ores > 0,
        "the default generation must dispatch ore features"
    );

    // `ore_clay` is only dispatched when the feature index uses the biome
    // parameter order; with the registry order its seed was wrong.
    assert!(
        count_named(&chunk, "minecraft:clay") > 0,
        "ore_clay should place clay in a lush-caves chunk"
    );
}

#[test]
fn ore_vein_filler_writes_granite_and_tuff() {
    let generator = WorldGenerator::overworld(12345).unwrap();
    let chunk = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();
    assert!(
        count_named(&chunk, "minecraft:granite") > 0,
        "copper veins fill with granite"
    );
    assert!(
        count_named(&chunk, "minecraft:tuff") > 0,
        "iron veins fill with tuff"
    );
}

#[test]
fn ore_generation_is_deterministic() {
    let generator = WorldGenerator::overworld(12345).unwrap();
    let first = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();
    let second = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();
    assert_eq!(first, second);
}

#[test]
fn ore_wiring_improves_the_java_parity_fixture() {
    let generator = WorldGenerator::overworld(12345).unwrap();
    let with_ores = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();
    let without_ores = generator
        .generate_chunk_with_steps(CHUNK_X, CHUNK_Z, &BASE_STEPS)
        .unwrap();
    let (columns, java) = java_fixture();
    assert_eq!(columns.len(), 16);

    let mut exact_with = 0u64;
    let mut exact_without = 0u64;
    let mut class_matches = 0u64;
    let mut total = 0u64;
    let mut java_ores = 0u64;
    let mut matched_ores = 0u64;
    for &(world_x, world_z) in &columns {
        let local_x = world_x.rem_euclid(SIDE) as usize;
        let local_z = world_z.rem_euclid(SIDE) as usize;
        for y in MIN_Y..MAX_Y {
            let java_name = java
                .get(&(world_x, world_z, y))
                .copied()
                .unwrap_or("minecraft:air");
            let rust_name = with_ores
                .block_state(local_x, y, local_z)
                .map(|state| state.name.as_str())
                .unwrap_or("minecraft:air");
            if rust_name == java_name {
                exact_with += 1;
            }
            if without_ores
                .block_state(local_x, y, local_z)
                .map(|state| state.name.as_str())
                .unwrap_or("minecraft:air")
                == java_name
            {
                exact_without += 1;
            }
            if block_class(rust_name) == block_class(java_name) {
                class_matches += 1;
            }
            if ore_like(java_name) {
                java_ores += 1;
                if rust_name == java_name {
                    matched_ores += 1;
                }
            }
            total += 1;
        }
    }

    let exact = exact_with as f64 / total as f64;
    let class = class_matches as f64 / total as f64;
    println!(
        "ore-wired parity: exact {exact:.4} ({exact_with}/{total}), class {class:.4} \
         ({class_matches}/{total}), java ores {java_ores}, matched {matched_ores}; \
         without ore step {exact_without}/{total}"
    );

    // The pre-wiring fixture floor is 92.01%; keep a margin over it so a
    // regression cannot silently re-disable or mis-seed ores.
    assert!(
        exact > 0.955,
        "ore-wired exact-name agreement fell to {exact:.4}"
    );
    assert!(class > 0.98, "block-class agreement fell to {class:.4}");
    assert!(
        exact_with > exact_without,
        "the default ore step must keep its parity gain"
    );
    assert!(
        matched_ores >= 24,
        "only {matched_ores} of {java_ores} Java ore blocks matched"
    );
}
