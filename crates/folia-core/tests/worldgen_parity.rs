//! Full per-block parity harness for generated chunks.
//!
//! The Java fixture lists every non-air block in a real server's chunk, sampled
//! at 16 columns (one every four blocks in x and z). Exact block-name agreement
//! is capped by features that are not ported yet (ore veins, vegetation,
//! structures), so the meaningful terrain metric is the coarse block class:
//! air, solid, water, or lava. Exact-name agreement is still reported to show
//! how close the terrain pass is.

use folia_core::worldgen::generator::WorldGenerator;
use std::collections::HashMap;

const JAVA_CHUNK: &str = include_str!("../assets/java-chunk-0--1.txt");
const CHUNK_X: i32 = 0;
const CHUNK_Z: i32 = -1;
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;
const SIDE: i32 = 16;

fn block_class(name: &str) -> u8 {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
        "minecraft:water" => 2,
        "minecraft:lava" => 3,
        _ => 1,
    }
}

fn class_name(class: u8) -> &'static str {
    match class {
        0 => "air",
        2 => "water",
        3 => "lava",
        _ => "solid",
    }
}

#[test]
fn generated_chunk_matches_java_per_block() {
    let generator = WorldGenerator::overworld(12345).unwrap();
    let chunk = generator.generate_chunk(CHUNK_X, CHUNK_Z).unwrap();

    let mut java: HashMap<(i32, i32, i32), &str> = HashMap::new();
    let mut column_list: Vec<(i32, i32)> = Vec::new();
    let mut fixture_chunk = None;
    for raw in JAVA_CHUNK.lines() {
        let line = raw.trim_end();
        let mut parts = line.split('\t');
        match parts.next() {
            Some("chunk") => {
                let x: i32 = parts.next().unwrap().parse().unwrap();
                let z: i32 = parts.next().unwrap().parse().unwrap();
                fixture_chunk = Some((x, z));
            }
            Some("column") => {
                let x: i32 = parts.next().unwrap().parse().unwrap();
                let z: i32 = parts.next().unwrap().parse().unwrap();
                column_list.push((x, z));
                for entry in parts {
                    if entry.is_empty() {
                        continue;
                    }
                    let (y, block) = entry.split_once('=').unwrap();
                    java.insert((x, z, y.parse().unwrap()), block);
                }
            }
            _ => {}
        }
    }

    assert_eq!(fixture_chunk, Some((CHUNK_X, CHUNK_Z)));
    assert_eq!(
        column_list.len(),
        16,
        "fixture must contain 16 sampled columns"
    );
    assert!(!java.is_empty(), "fixture must list non-air blocks");

    let mut exact = 0u64;
    let mut class_matches = 0u64;
    let mut total = 0u64;
    let mut rust_water = 0u64;
    let mut java_water = 0u64;
    let mut water_mismatches = 0u64;
    let mut confusions: HashMap<(u8, u8), u64> = HashMap::new();

    for &(world_x, world_z) in &column_list {
        let local_x = world_x.rem_euclid(SIDE) as usize;
        let local_z = world_z.rem_euclid(SIDE) as usize;
        for y in MIN_Y..MAX_Y {
            let rust_name = chunk
                .block_state(local_x, y, local_z)
                .map(|state| state.name.as_str())
                .unwrap_or("minecraft:air");
            let java_name = java
                .get(&(world_x, world_z, y))
                .copied()
                .unwrap_or("minecraft:air");

            let rust_class = block_class(rust_name);
            let java_class = block_class(java_name);

            if rust_name == java_name {
                exact += 1;
            }
            if rust_class == java_class {
                class_matches += 1;
            } else {
                *confusions.entry((rust_class, java_class)).or_insert(0) += 1;
            }
            if rust_class == 2 {
                rust_water += 1;
            }
            if java_class == 2 {
                java_water += 1;
            }
            if (rust_class == 2) != (java_class == 2) {
                water_mismatches += 1;
            }
            total += 1;
        }
    }

    let class_agreement = class_matches as f64 / total as f64;
    let exact_agreement = exact as f64 / total as f64;
    let mut confusions: Vec<((u8, u8), u64)> = confusions.into_iter().collect();
    confusions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    println!("compared blocks: {total}");
    println!(
        "block-class agreement: {:.2}% ({class_matches}/{total})",
        class_agreement * 100.0
    );
    println!(
        "exact-name agreement: {:.2}% ({exact}/{total})",
        exact_agreement * 100.0
    );
    println!("water blocks: Rust={rust_water} Java={java_water} mismatches={water_mismatches}");
    println!("top class confusions (rust -> java):");
    for ((rust_class, java_class), count) in confusions.iter().take(8) {
        println!(
            "  {} -> {}: {count}",
            class_name(*rust_class),
            class_name(*java_class)
        );
    }

    assert_eq!(
        total,
        column_list.len() as u64 * (MAX_Y - MIN_Y) as u64,
        "every block in each sampled column must be compared"
    );
    assert!(
        class_agreement > 0.98,
        "block-class agreement too low: {class_agreement:.3}"
    );
    assert!(
        exact_agreement > 0.915,
        "exact block-name agreement too low: {exact_agreement:.3}"
    );
}
