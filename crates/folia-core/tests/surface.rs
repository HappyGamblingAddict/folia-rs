//! Surface-rule fidelity: `BiomeManager` zoom, flat bedrock, and frozen-ocean
//! temperature decisions.

use folia_core::chunk::{BlockState, Chunk};
use folia_core::worldgen::biome::BiomeSampler;
use folia_core::worldgen::biome_zoom::BiomeZoomer;
use folia_core::worldgen::density::WorldgenData;
use folia_core::worldgen::surface::{
    self, SurfaceOptions, build_surface_with_seed, height_adjusted_temperature,
    parse_rules_with_options, should_melt_frozen_ocean_iceberg, system::SurfaceSystem,
};
use folia_math::BlockPos;
use serde_json::json;

const SEED: i64 = 0;
const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn stone_column_chunk(chunk_x: i32, chunk_z: i32, bottom: i32, top: i32) -> Chunk {
    let mut chunk = Chunk {
        x: chunk_x,
        z: chunk_z,
        sections: Vec::new(),
    };
    let stone = block("minecraft:stone");
    for x in 0..16usize {
        for z in 0..16usize {
            for y in bottom..=top {
                chunk.set_block_state_ref(x, y, z, &stone);
            }
        }
    }
    chunk
}

fn fixture() -> (WorldgenData, SurfaceSystem, BiomeSampler) {
    let data = WorldgenData::overworld(SEED).unwrap();
    let system = SurfaceSystem::new(
        &data,
        MIN_Y,
        HEIGHT,
        data.sea_level(),
        block("minecraft:stone"),
    )
    .unwrap();
    let biomes = BiomeSampler::from_data(&data).unwrap();
    (data, system, biomes)
}

fn flat_bedrock_rule(data: &WorldgenData, generate_flat_bedrock: bool) -> surface::RuleSource {
    let value = json!({
        "type": "minecraft:condition",
        "if_true": {
            "type": "paper:optionally_flat_bedrock_condition_source",
            "random_name": "minecraft:bedrock_floor",
            "true_at_and_below": {"above_bottom": 0},
            "false_at_and_above": {"above_bottom": 5},
            "is_roof": false
        },
        "then_run": {
            "type": "minecraft:block",
            "result_state": {"Name": "minecraft:bedrock"}
        }
    });
    parse_rules_with_options(
        &value,
        data.seed(),
        MIN_Y,
        HEIGHT,
        SurfaceOptions {
            generate_flat_bedrock,
        },
    )
    .unwrap()
}

#[test]
fn biome_zoomer_selects_the_same_biome_the_surface_rule_uses() {
    let (data, system, biomes) = fixture();
    let router = data.router().unwrap();
    let preliminary = router.function("preliminary_surface_level").unwrap();
    let zoomer = BiomeZoomer::new(SEED);

    // Seed 0 / chunk (0, 0) straddles forest and river at the surface. The
    // surface pass samples the biome at the terrain surface, so use y = 70
    // rather than an underground quart.
    const SURFACE_Y: i32 = 70;
    const CHUNK_X: i32 = 0;
    const CHUNK_Z: i32 = 0;

    let rule_value = json!({
        "type": "minecraft:condition",
        "if_true": {
            "type": "minecraft:biome",
            "biome_is": ["minecraft:forest"]
        },
        "then_run": {
            "type": "minecraft:block",
            "result_state": {"Name": "minecraft:diamond_block"}
        }
    });
    let rules = parse_rules_with_options(
        &rule_value,
        data.seed(),
        MIN_Y,
        HEIGHT,
        SurfaceOptions::default(),
    )
    .unwrap();
    let mut chunk = stone_column_chunk(CHUNK_X, CHUNK_Z, 60, SURFACE_Y);
    build_surface_with_seed(
        &system,
        &rules,
        preliminary.as_ref(),
        &biomes,
        &[],
        &mut chunk,
        Some(SEED),
    );

    let mut forest = 0;
    let mut other = 0;
    let mut zoom_changes = 0;
    for x in 0..16usize {
        for z in 0..16usize {
            let world_x = CHUNK_X * 16 + x as i32;
            let world_z = CHUNK_Z * 16 + z as i32;
            let expected = zoomer.biome_at(&biomes, BlockPos::new(world_x, SURFACE_Y, world_z));
            if expected != biomes.sample_block(world_x, SURFACE_Y, world_z) {
                zoom_changes += 1;
            }
            let state = chunk.block_state(x, SURFACE_Y, z).expect("surface block");
            if expected == "minecraft:forest" {
                assert_eq!(state.name, "minecraft:diamond_block", "at {x},{z}");
                forest += 1;
            } else {
                assert_eq!(state.name, "minecraft:stone", "at {x},{z}");
                other += 1;
            }
        }
    }
    assert!(
        zoom_changes > 0,
        "the fixture must exercise columns where the fuzzy zoomer differs from QuartPos"
    );
    assert!(forest > 0 && other > 0, "forest={forest} other={other}");
}

#[test]
fn flat_bedrock_collapses_the_random_gradient_to_one_layer() {
    let (data, system, biomes) = fixture();
    let router = data.router().unwrap();
    let preliminary = router.function("preliminary_surface_level").unwrap();

    let mut random_bedrock = stone_column_chunk(0, 0, MIN_Y, 0);
    let random_rules = flat_bedrock_rule(&data, false);
    build_surface_with_seed(
        &system,
        &random_rules,
        preliminary.as_ref(),
        &biomes,
        &[],
        &mut random_bedrock,
        None,
    );
    let mut above_bottom = 0;
    for x in 0..16usize {
        for z in 0..16usize {
            assert_eq!(
                random_bedrock.block_state(x, MIN_Y, z).unwrap().name,
                "minecraft:bedrock"
            );
            for y in (MIN_Y + 1)..=(MIN_Y + 4) {
                if random_bedrock.block_state(x, y, z).unwrap().name == "minecraft:bedrock" {
                    above_bottom += 1;
                }
            }
        }
    }
    assert!(
        above_bottom > 0,
        "the random gradient must place some bedrock above the bottom layer"
    );

    let mut flat = stone_column_chunk(0, 0, MIN_Y, 0);
    let flat_rules = flat_bedrock_rule(&data, true);
    build_surface_with_seed(
        &system,
        &flat_rules,
        preliminary.as_ref(),
        &biomes,
        &[],
        &mut flat,
        None,
    );
    for x in 0..16usize {
        for z in 0..16usize {
            assert_eq!(
                flat.block_state(x, MIN_Y, z).unwrap().name,
                "minecraft:bedrock"
            );
            for y in (MIN_Y + 1)..=(MIN_Y + 4) {
                assert_eq!(
                    flat.block_state(x, y, z).unwrap().name,
                    "minecraft:stone",
                    "flat bedrock must not extend above the bottom layer at {x},{y},{z}"
                );
            }
        }
    }
}

#[test]
fn frozen_ocean_temperature_and_melt_are_pinned_at_fixed_coordinates() {
    // Frosted branches of `Biome.FROZEN_TEMPERATURE_NOISE`: (0, 0) takes the
    // 0.2 branch, (-100, 0) keeps the base 0.0. These were read from the
    // ported `PerlinSimplexNoise` tables, not fitted to the test.
    assert_eq!(
        height_adjusted_temperature("minecraft:frozen_ocean", 0, 63, 0, 63),
        Some(0.2)
    );
    assert_eq!(
        should_melt_frozen_ocean_iceberg("minecraft:frozen_ocean", 0, 0, 63),
        Some(true)
    );
    assert_eq!(
        height_adjusted_temperature("minecraft:frozen_ocean", -100, 63, 0, 63),
        Some(0.0)
    );
    assert_eq!(
        should_melt_frozen_ocean_iceberg("minecraft:frozen_ocean", -100, 0, 63),
        Some(false)
    );

    // The deep ocean's base temperature is 0.5. At (0, 0) the `frozen`
    // modifier returns 0.2, overriding the base like Java does; both remain
    // above the 0.1 iceberg-melt threshold.
    assert_eq!(
        height_adjusted_temperature("minecraft:deep_frozen_ocean", 0, 63, 0, 63),
        Some(0.2)
    );
    assert_eq!(
        height_adjusted_temperature("minecraft:deep_frozen_ocean", -100, 63, 0, 63),
        Some(0.5)
    );
    assert_eq!(
        should_melt_frozen_ocean_iceberg("minecraft:deep_frozen_ocean", 0, 0, 63),
        Some(true)
    );
    assert_eq!(
        should_melt_frozen_ocean_iceberg("minecraft:plains", 0, 0, 63),
        None
    );

    // `getHeightAdjustedTemperature` only cools above sea level + 17. At
    // y = 63 the adjusted value equals the base; at y = 200 the TEMPERATURE
    // noise plus the 0.05/40 height gradient pushes 0.2 below the snow
    // threshold.
    assert_eq!(
        height_adjusted_temperature("minecraft:frozen_ocean", 0, 63, 0, 63),
        Some(0.2)
    );
    let high = height_adjusted_temperature("minecraft:frozen_ocean", 0, 200, 0, 63).unwrap();
    assert!(
        high < 0.15,
        "high-altitude frozen ocean should freeze: {high}"
    );
    assert_eq!(
        height_adjusted_temperature("minecraft:frozen_peaks", 0, 63, 0, 63),
        Some(-0.7)
    );
    assert!(height_adjusted_temperature("minecraft:frozen_peaks", 0, 200, 0, 63).unwrap() < -0.7);
}
