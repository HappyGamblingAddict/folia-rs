//! Differential tests for the `NoiseChunk` 4x8x4 interpolation of the ore-vein
//! density functions and for the 16-column Java parity fixture.
//!
//! Java wraps `vein_toggle`, and the two noise arguments inside `vein_ridged`,
//! in `DensityFunctions.Marker.Type.Interpolated`. `NoiseChunk` evaluates the
//! wrapped argument on a 4x8x4 grid and trilinearly interpolates per block.
//! `density::DensityFunction::compute_interpolated` reproduces that path while
//! `compute` keeps Java's direct `DensityFunction.compute` behaviour.

use folia_core::worldgen::density::{DensityFunction, WorldgenData};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::ore_vein::{OreVeinifier, VeinSampling};
use std::collections::HashMap;

const JAVA_GRID: &str = include_str!("../assets/java-density-grid.txt");
const JAVA_CHUNK: &str = include_str!("../assets/java-chunk-0--1.txt");
const CELL_WIDTH: i32 = 4;
const CELL_HEIGHT: i32 = 8;
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;
const SIDE: i32 = 16;

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

fn lerp2(delta1: f64, delta2: f64, start1: f64, end1: f64, start2: f64, end2: f64) -> f64 {
    lerp(
        delta2,
        lerp(delta1, start1, end1),
        lerp(delta1, start2, end2),
    )
}

/// `Mth.lerp3` over the eight direct corner values of a 4x8x4 cell.
fn trilinear(argument: &DensityFunction, x: i32, y: i32, z: i32) -> f64 {
    let x0 = x.div_euclid(CELL_WIDTH) * CELL_WIDTH;
    let y0 = MIN_Y + (y - MIN_Y).div_euclid(CELL_HEIGHT) * CELL_HEIGHT;
    let z0 = z.div_euclid(CELL_WIDTH) * CELL_WIDTH;
    let x1 = x0 + CELL_WIDTH;
    let y1 = y0 + CELL_HEIGHT;
    let z1 = z0 + CELL_WIDTH;
    let values = [
        argument.compute(x0, y0, z0),
        argument.compute(x1, y0, z0),
        argument.compute(x0, y1, z0),
        argument.compute(x1, y1, z0),
        argument.compute(x0, y0, z1),
        argument.compute(x1, y0, z1),
        argument.compute(x0, y1, z1),
        argument.compute(x1, y1, z1),
    ];
    let delta_x = f64::from(x - x0) / f64::from(CELL_WIDTH);
    let delta_y = f64::from(y - y0) / f64::from(CELL_HEIGHT);
    let delta_z = f64::from(z - z0) / f64::from(CELL_WIDTH);
    lerp(
        delta_z,
        lerp2(delta_x, delta_y, values[0], values[1], values[2], values[3]),
        lerp2(delta_x, delta_y, values[4], values[5], values[6], values[7]),
    )
}

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

#[test]
fn vein_direct_values_match_the_java_router_dump() {
    // Pinned from the real 1.21.11 server's `worldgen-dump.txt`
    // (`DensityFunction.compute` on `router.veinToggle/veinRidged/veinGap`).
    let data = WorldgenData::overworld(12345).unwrap();
    let router = data.router().unwrap();
    let cases: [(&str, (i32, i32, i32), f64); 30] = [
        ("vein_toggle", (0, 0, 0), -0.460_668_059_155_418_2),
        ("vein_toggle", (0, -64, 0), 0.0),
        ("vein_toggle", (0, -60, 0), -0.312_803_632_595_837_43),
        ("vein_toggle", (0, -30, 0), -0.493_389_565_518_618_96),
        ("vein_toggle", (0, 63, 0), 0.0),
        ("vein_toggle", (0, 120, 0), 0.0),
        ("vein_toggle", (123, 64, -456), 0.0),
        ("vein_toggle", (-1000, 80, 2000), 0.0),
        ("vein_toggle", (15, -16, 31), -0.635_450_087_692_762),
        ("vein_toggle", (16, 0, -16), -0.475_983_958_867_301_44),
        ("vein_ridged", (0, 0, 0), 0.074_361_367_615_381_48),
        ("vein_ridged", (0, -64, 0), -0.079_999_998_211_860_66),
        ("vein_ridged", (0, -60, 0), 0.789_633_156_612_353_4),
        ("vein_ridged", (0, -30, 0), 0.384_538_354_553_908_8),
        ("vein_ridged", (0, 63, 0), -0.079_999_998_211_860_66),
        ("vein_ridged", (0, 120, 0), -0.079_999_998_211_860_66),
        ("vein_ridged", (123, 64, -456), -0.079_999_998_211_860_66),
        ("vein_ridged", (-1000, 80, 2000), -0.079_999_998_211_860_66),
        ("vein_ridged", (15, -16, 31), 0.030_218_428_409_164_264),
        ("vein_ridged", (16, 0, -16), 0.274_917_743_816_718_4),
        ("vein_gap", (0, 0, 0), 0.474_128_116_822_707_17),
        ("vein_gap", (0, -64, 0), -0.518_045_975_438_790_8),
        ("vein_gap", (0, -60, 0), -0.404_972_258_760_870_95),
        ("vein_gap", (0, -30, 0), -0.285_611_507_524_332_2),
        ("vein_gap", (0, 63, 0), 0.316_078_311_963_488_8),
        ("vein_gap", (0, 120, 0), 0.173_981_461_065_655_46),
        ("vein_gap", (123, 64, -456), 0.233_440_717_834_501_85),
        ("vein_gap", (-1000, 80, 2000), 0.750_534_924_548_341_6),
        ("vein_gap", (15, -16, 31), -0.178_688_867_214_056_86),
        ("vein_gap", (16, 0, -16), 0.794_841_758_604_733),
    ];
    for (name, (x, y, z), expected) in cases {
        let actual = router.compute(name, x, y, z).unwrap();
        assert!(
            (actual - expected).abs() < 1.0e-9,
            "{name} ({x},{y},{z}): {actual} != {expected}"
        );
    }
}

#[test]
fn interpolation_matches_direct_at_all_java_density_grid_corners() {
    let data = WorldgenData::overworld(12345).unwrap();
    let router = data.router().unwrap();
    let final_density = router.function("final_density").unwrap();
    let vein_functions = ["vein_toggle", "vein_ridged", "vein_gap"];

    let mut checked = 0;
    for line in JAVA_GRID.lines() {
        let mut parts = line.split('\t');
        let first = match parts.next() {
            Some(value) => value,
            None => continue,
        };
        if first == "seed" || first.is_empty() {
            continue;
        }
        let x: i32 = first.parse().unwrap();
        let y: i32 = parts.next().unwrap().parse().unwrap();
        let z: i32 = parts.next().unwrap().parse().unwrap();
        let expected: f64 = parts.next().unwrap().parse().unwrap();

        let direct = final_density.compute(x, y, z);
        let interpolated = final_density.compute_interpolated(x, y, z);
        assert!(
            (direct - expected).abs() < 1.0e-9,
            "final_density direct ({x},{y},{z}): {direct} != {expected}"
        );
        assert!(
            (interpolated - direct).abs() < 1.0e-12,
            "final_density interpolation moved a grid corner ({x},{y},{z}): \
             {interpolated} != {direct}"
        );

        for name in vein_functions {
            let function = router.function(name).unwrap();
            let direct = function.compute(x, y, z);
            let interpolated = function.compute_interpolated(x, y, z);
            assert!(
                (interpolated - direct).abs() < 1.0e-12,
                "{name} interpolation moved a grid corner ({x},{y},{z}): \
                 {interpolated} != {direct}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 5 * 5 * 49);
}

#[test]
#[allow(clippy::excessive_precision)] // pinned 17-digit oracle values
fn vein_toggle_interpolates_the_wrapped_argument_trilinearly() {
    let data = WorldgenData::overworld(12345).unwrap();
    let router = data.router().unwrap();
    let toggle = router.function("vein_toggle").unwrap();
    let ridged = router.function("vein_ridged").unwrap();
    let gap = router.function("vein_gap").unwrap();
    let DensityFunction::Interpolated { argument, .. } = toggle.as_ref() else {
        panic!("vein_toggle must be wrapped in an interpolated marker");
    };

    // Points inside a cell and on the cell/section boundaries. The pinned
    // numbers are the expected `NoiseChunk` values produced from the
    // Java-verified direct corner values by `Mth.lerp3`.
    let cases: [(i32, i32, i32, f64, f64, f64, f64); 8] = [
        (
            1,
            -61,
            -15,
            -0.121_041_041_347_453_97,
            0.0,
            0.188_107_171_158_565_62,
            -0.079_999_998_211_860_66,
        ),
        (
            2,
            -57,
            -13,
            -0.287_677_330_615_200_79,
            -0.321_984_478_322_447_7,
            0.559_051_803_087_712_96,
            0.682_139_531_976_958_76,
        ),
        (
            3,
            -49,
            -9,
            -0.384_934_526_222_708_52,
            -0.385_280_457_885_376_57,
            0.284_403_858_163_998_32,
            0.288_684_294_623_707_82,
        ),
        (
            7,
            -33,
            -25,
            -0.400_215_423_223_152_62,
            -0.400_659_540_344_983_63,
            0.326_924_227_537_891_8,
            0.349_445_797_471_725_16,
        ),
        (
            11,
            -17,
            -3,
            -0.541_407_915_728_252_2,
            -0.542_316_048_882_022_23,
            0.297_377_880_000_621_82,
            0.324_224_824_875_282_37,
        ),
        (
            15,
            41,
            -1,
            -0.246_546_798_663_588_86,
            -0.246_647_068_993_176_45,
            0.352_936_192_066_904_74,
            0.380_633_987_189_650_8,
        ),
        (
            4,
            -56,
            -12,
            -0.335_224_624_108_542_98,
            -0.335_224_624_108_542_98,
            0.647_550_226_132_174_39,
            0.647_550_226_132_174_39,
        ),
        (
            0,
            -64,
            -16,
            0.0,
            0.0,
            -0.079_999_998_211_860_66,
            -0.079_999_998_211_860_66,
        ),
    ];
    for (x, y, z, toggle_interpolated, toggle_direct, ridged_interpolated, ridged_direct) in cases {
        let expected = trilinear(argument, x, y, z);
        let actual = toggle.compute_interpolated(x, y, z);
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "vein_toggle ({x},{y},{z}): {actual} != trilinear {expected}"
        );
        assert!(
            (actual - toggle_interpolated).abs() < 1.0e-12,
            "vein_toggle interpolated ({x},{y},{z}): {actual} != {toggle_interpolated}"
        );
        // Outside a `NoiseChunk`, Java returns the unwrapped argument.
        let direct = toggle.compute(x, y, z);
        assert!(
            (direct - argument.compute(x, y, z)).abs() < 1.0e-15,
            "direct vein_toggle ({x},{y},{z}) must equal its argument"
        );
        assert!(
            (direct - toggle_direct).abs() < 1.0e-12,
            "vein_toggle direct ({x},{y},{z}): {direct} != {toggle_direct}"
        );
        assert!(
            (ridged.compute_interpolated(x, y, z) - ridged_interpolated).abs() < 1.0e-12,
            "vein_ridged interpolated ({x},{y},{z}) mismatch"
        );
        assert!(
            (ridged.compute(x, y, z) - ridged_direct).abs() < 1.0e-12,
            "vein_ridged direct ({x},{y},{z}) mismatch"
        );
        // `vein_gap` is not marked interpolated, so both modes are identical.
        assert_eq!(
            gap.compute_interpolated(x, y, z).to_bits(),
            gap.compute(x, y, z).to_bits(),
            "vein_gap must stay per-block"
        );
    }

    // The y range_choice starts at -60, but the interpolator still bleeds the
    // -56 corner into y=-61..-63 exactly like Java's NoiseChunk.
    assert!(
        toggle.compute_interpolated(1, -61, -15) < 0.0,
        "interpolation must see the in-range corner below y=-60"
    );
}

#[test]
fn vein_edge_blocks_change_with_noise_chunk_interpolation() {
    let data = WorldgenData::overworld(12345).unwrap();
    let router = data.router().unwrap();
    let veins = OreVeinifier::new(&router, data.seed()).unwrap();

    // Chunk (0, -1): world x 0..16, world z -16..0, the full vein height range.
    let mut changed = 0usize;
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut replaced = 0usize;
    let mut direct_blocks = 0usize;
    let mut interpolated_blocks = 0usize;
    let mut examples = Vec::new();
    for y in -60..=50 {
        for z in -16..0 {
            for x in 0..16 {
                let direct = veins.block_at_with(x, y, z, VeinSampling::Direct);
                let interpolated = veins.block_at_with(x, y, z, VeinSampling::Interpolated);
                if direct.is_some() {
                    direct_blocks += 1;
                }
                if interpolated.is_some() {
                    interpolated_blocks += 1;
                }
                if direct != interpolated {
                    changed += 1;
                    match (&direct, &interpolated) {
                        (None, Some(_)) => added += 1,
                        (Some(_), None) => removed += 1,
                        _ => replaced += 1,
                    }
                    if examples.len() < 8 {
                        examples.push((x, y, z, direct, interpolated));
                    }
                }
            }
        }
    }
    println!(
        "vein interpolation delta over chunk (0,-1): {changed} positions changed \
         ({added} added, {removed} removed, {replaced} replaced); \
         direct={direct_blocks}, interpolated={interpolated_blocks}"
    );
    for (x, y, z, direct, interpolated) in &examples {
        println!(
            "  ({x},{y},{z}): direct={:?} interpolated={:?}",
            direct.as_ref().map(|state| state.name.as_str()),
            interpolated.as_ref().map(|state| state.name.as_str())
        );
    }
    assert!(
        changed > 0,
        "interpolation must move at least one vein edge block"
    );
    assert!(
        changed < direct_blocks.max(interpolated_blocks).max(1) / 2,
        "interpolation should only move vein edges, not most of the vein"
    );
}

#[test]
fn sixteen_column_parity_floor_after_interpolation() {
    let generator = WorldGenerator::overworld(12345).unwrap();
    let chunk = generator.generate_chunk(0, -1).unwrap();
    let (columns, java) = java_fixture();
    assert_eq!(columns.len(), 16);

    let mut exact = 0u64;
    let mut class_matches = 0u64;
    let mut total = 0u64;
    let mut java_ores = 0u64;
    let mut matched_ores = 0u64;
    let mut exact_confusions: HashMap<(&str, &str), u64> = HashMap::new();
    let mut ore_mismatches = Vec::new();
    for &(world_x, world_z) in &columns {
        let local_x = world_x.rem_euclid(SIDE) as usize;
        let local_z = world_z.rem_euclid(SIDE) as usize;
        for y in MIN_Y..MAX_Y {
            let java_name = java
                .get(&(world_x, world_z, y))
                .copied()
                .unwrap_or("minecraft:air");
            let rust_name = chunk
                .block_state(local_x, y, local_z)
                .map(|state| state.name.as_str())
                .unwrap_or("minecraft:air");
            if rust_name == java_name {
                exact += 1;
            } else {
                *exact_confusions.entry((rust_name, java_name)).or_insert(0) += 1;
            }
            if block_class(rust_name) == block_class(java_name) {
                class_matches += 1;
            }
            if ore_like(java_name) {
                java_ores += 1;
                if rust_name == java_name {
                    matched_ores += 1;
                } else if ore_mismatches.len() < 8 {
                    ore_mismatches.push((world_x, y, world_z, rust_name, java_name));
                }
            }
            total += 1;
        }
    }

    let mut exact_confusions: Vec<_> = exact_confusions.into_iter().collect();
    exact_confusions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("top exact-name mismatches (rust -> java):");
    for ((rust_name, java_name), count) in exact_confusions.iter().take(12) {
        println!("  {rust_name} -> {java_name}: {count}");
    }
    for (x, y, z, rust_name, java_name) in &ore_mismatches {
        println!("  ore mismatch ({x},{y},{z}): {rust_name} -> {java_name}");
    }

    println!(
        "parity after NoiseChunk interpolation: exact {exact}/{total} ({:.2}%), \
         class {class_matches}/{total} ({:.2}%), ores {matched_ores}/{java_ores}",
        exact as f64 / total as f64 * 100.0,
        class_matches as f64 / total as f64 * 100.0
    );

    assert_eq!(total, 16 * (MAX_Y - MIN_Y) as u64);
    // Pre-interpolation baseline was 5886/6144 exact and 6067/6144 class.
    assert!(
        exact >= 5891,
        "exact-name agreement regressed below the interpolated baseline: {exact}/{total}"
    );
    assert!(
        class_matches >= 6067,
        "block-class agreement regressed below the baseline: {class_matches}/{total}"
    );
    assert!(
        matched_ores >= 28,
        "only {matched_ores} of {java_ores} Java ore blocks matched"
    );
}

#[test]
fn vein_interpolation_is_deterministic_and_cache_safe() {
    let data = WorldgenData::overworld(12345).unwrap();
    let router = data.router().unwrap();
    let veins = OreVeinifier::new(&router, data.seed()).unwrap();
    let other = OreVeinifier::new(&router, data.seed()).unwrap();
    let positions = [
        (0, -60, -16),
        (3, -57, -13),
        (4, -56, -12),
        (7, -49, -9),
        (15, -33, -1),
        (16, -17, 0),
    ];
    let first: Vec<_> = positions
        .iter()
        .map(|&(x, y, z)| veins.block_at(x, y, z))
        .collect();
    // Interleave the direct sampler; the interpolated cache must not be
    // poisoned by the diagnostic path.
    let _: Vec<_> = positions
        .iter()
        .map(|&(x, y, z)| veins.block_at_with(x, y, z, VeinSampling::Direct))
        .collect();
    let second: Vec<_> = positions
        .iter()
        .map(|&(x, y, z)| veins.block_at(x, y, z))
        .collect();
    let third: Vec<_> = positions
        .iter()
        .map(|&(x, y, z)| other.block_at(x, y, z))
        .collect();
    assert_eq!(
        first, second,
        "repeated interpolated sampling must be stable"
    );
    assert_eq!(
        first, third,
        "a fresh veinifier must produce the same blocks"
    );
}
