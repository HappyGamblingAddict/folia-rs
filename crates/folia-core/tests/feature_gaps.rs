//! Configured-feature gap coverage for the terrain, selector, and cave batch.

use folia_core::chunk::{BlockState, Chunk};
use folia_core::random::LegacyRandomSource;
use folia_core::tick::ScheduledTick;
use folia_core::worldgen::feature::{FeatureData, WorldGenRegion};
use folia_math::BlockPos;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn empty_chunk() -> Chunk {
    Chunk {
        x: 0,
        z: 0,
        sections: Vec::new(),
    }
}

fn flat_chunk(top: i32, top_block: &str) -> Chunk {
    let mut chunk = empty_chunk();
    let stone = block("minecraft:stone");
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..top {
                chunk.set_block_state_ref(x, y, z, &stone);
            }
            chunk.set_block_state(x, top, z, block(top_block));
        }
    }
    chunk
}

fn fill(chunk: &mut Chunk, from_y: i32, to_y: i32, name: &str) {
    let state = block(name);
    for x in 0..16 {
        for z in 0..16 {
            for y in from_y..to_y {
                chunk.set_block_state_ref(x, y, z, &state);
            }
        }
    }
}

fn place(data: &FeatureData, id: &str, chunk: &mut Chunk, seed: i64, origin: BlockPos) -> bool {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
    let mut random = LegacyRandomSource::new(seed);
    feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable")
}

fn place_tracked(
    data: &FeatureData,
    id: &str,
    chunk: &mut Chunk,
    seed: i64,
    origin: BlockPos,
) -> (bool, Vec<ScheduledTick>) {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
    let mut random = LegacyRandomSource::new(seed);
    let placed = feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable");
    (placed, level.scheduled_ticks_mut().tick(1))
}

fn count_blocks(chunk: &Chunk, name: &str) -> usize {
    chunk
        .sections
        .iter()
        .flat_map(|section| {
            section
                .block_states
                .indices
                .iter()
                .map(move |index| &section.block_states.palette[*index as usize])
        })
        .filter(|state| state.name == name)
        .count()
}

fn data_with(configured: Value, placed: Value) -> FeatureData {
    FeatureData::parse(
        &json!({
            "configured": configured,
            "placed": placed,
            "biomes": {},
            "tags": {}
        })
        .to_string(),
    )
    .expect("synthetic feature data must parse")
}

fn assert_places_for_some_seed(
    data: &FeatureData,
    id: &str,
    make_chunk: impl Fn() -> Chunk,
    origin: BlockPos,
    block_name: &str,
) -> Chunk {
    for seed in 0..32 {
        let mut chunk = make_chunk();
        place(data, id, &mut chunk, seed, origin);
        if count_blocks(&chunk, block_name) > 0 {
            let mut replay = make_chunk();
            place(data, id, &mut replay, seed, origin);
            assert_eq!(chunk, replay, "{id} must replay deterministically");
            return chunk;
        }
    }
    panic!("{id} never placed {block_name}");
}

#[test]
fn unsupported_feature_inventory_is_explicit() {
    let data = FeatureData::vanilla();
    assert_eq!(data.supported_feature_count(), 207);
    let expected: BTreeMap<&str, usize> = [
        ("minecraft:bonus_chest", 1),
        ("minecraft:chorus_plant", 1),
        ("minecraft:desert_well", 1),
        ("minecraft:end_gateway", 2),
        ("minecraft:end_island", 1),
        ("minecraft:end_platform", 1),
        ("minecraft:end_spike", 1),
        ("minecraft:fossil", 2),
        ("minecraft:freeze_top_layer", 1),
        ("minecraft:geode", 1),
        ("minecraft:monster_room", 1),
        ("minecraft:root_system", 1),
        ("minecraft:sculk_patch", 2),
        ("minecraft:void_start_platform", 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(data.unsupported_type_counts(), expected);
}

#[test]
fn two_registry_random_selector_resolves_a_placed_branch() {
    let data = data_with(
        json!({
            "test:block": {
                "type": "minecraft:simple_block",
                "config": {"to_place": {
                    "type": "minecraft:simple_state_provider",
                    "state": {"Name": "minecraft:short_grass"}
                }}
            },
            "test:selector": {
                "type": "minecraft:random_selector",
                "config": {
                    "default": "test:block_checked",
                    "features": []
                }
            }
        }),
        json!({
            "test:block": {"feature": "test:block", "placement": []},
            "test:block_checked": {
                "feature": "test:block",
                "placement": [{
                    "type": "minecraft:random_offset",
                    "xz_spread": 1,
                    "y_spread": 0
                }]
            },
            "test:selector": {"feature": "test:selector", "placement": []}
        }),
    );
    let selector = data.configured("test:selector").expect("selector");
    assert!(selector.is_supported());
    let mut chunk = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        &data,
        "test:selector",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(
        count_blocks(&chunk, "minecraft:short_grass"),
        1,
        "the placed branch's random_offset must run before its configured feature"
    );
    assert_eq!(
        chunk.block_state(9, 64, 9).map(|state| state.name.as_str()),
        Some("minecraft:short_grass"),
        "the branch offset is x+1, z+1"
    );
}

#[test]
fn shipped_pale_garden_selector_executes_placed_tree_branch() {
    let data = FeatureData::vanilla();
    assert!(
        data.configured("minecraft:pale_garden_vegetation")
            .expect("pale garden selector")
            .is_supported()
    );
    assert_places_for_some_seed(
        data,
        "minecraft:pale_garden_vegetation",
        || flat_chunk(63, "minecraft:grass_block"),
        BlockPos::new(8, 64, 8),
        "minecraft:pale_oak_log",
    );
}

#[test]
fn shipped_disk_and_spring_features_place() {
    let data = FeatureData::vanilla();
    assert_places_for_some_seed(
        data,
        "minecraft:disk_sand",
        || flat_chunk(63, "minecraft:grass_block"),
        BlockPos::new(8, 64, 8),
        "minecraft:sand",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:disk_gravel",
        || flat_chunk(63, "minecraft:dirt"),
        BlockPos::new(8, 64, 8),
        "minecraft:gravel",
    );

    let mut spring = flat_chunk(70, "minecraft:stone");
    spring.set_block_state(8, 64, 8, block("minecraft:air"));
    spring.set_block_state(9, 64, 8, block("minecraft:air"));
    let (placed, ticks) = place_tracked(
        data,
        "minecraft:spring_water",
        &mut spring,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    assert_eq!(
        spring
            .block_state(8, 64, 8)
            .map(|state| state.name.as_str()),
        Some("minecraft:water")
    );
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].block, "minecraft:water");
}

#[test]
fn shipped_ice_features_place() {
    let data = FeatureData::vanilla();
    assert_places_for_some_seed(
        data,
        "minecraft:iceberg_blue",
        || flat_chunk(62, "minecraft:water"),
        BlockPos::new(8, 63, 8),
        "minecraft:blue_ice",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:ice_patch",
        || flat_chunk(63, "minecraft:grass_block"),
        BlockPos::new(8, 64, 8),
        "minecraft:packed_ice",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:ice_spike",
        || flat_chunk(63, "minecraft:snow_block"),
        BlockPos::new(8, 64, 8),
        "minecraft:packed_ice",
    );

    let mut blue = empty_chunk();
    blue.set_block_state(8, 62, 8, block("minecraft:water"));
    blue.set_block_state(9, 62, 8, block("minecraft:packed_ice"));
    assert!(place(
        data,
        "minecraft:blue_ice",
        &mut blue,
        7,
        BlockPos::new(8, 62, 8)
    ));
    assert_eq!(
        blue.block_state(8, 62, 8).map(|state| state.name.as_str()),
        Some("minecraft:blue_ice")
    );
}

#[test]
fn shipped_lake_and_nether_features_place() {
    let data = FeatureData::vanilla();
    assert_places_for_some_seed(
        data,
        "minecraft:lake_lava",
        || flat_chunk(63, "minecraft:stone"),
        BlockPos::new(0, 63, 0),
        "minecraft:lava",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:blackstone_blobs",
        || flat_chunk(63, "minecraft:netherrack"),
        BlockPos::new(8, 64, 8),
        "minecraft:blackstone",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:basalt_blobs",
        || flat_chunk(63, "minecraft:netherrack"),
        BlockPos::new(8, 64, 8),
        "minecraft:basalt",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:small_basalt_columns",
        || flat_chunk(63, "minecraft:netherrack"),
        BlockPos::new(8, 64, 8),
        "minecraft:basalt",
    );
}

#[test]
fn shipped_basalt_pillar_and_delta_place() {
    let data = FeatureData::vanilla();

    let mut pillar = empty_chunk();
    fill(&mut pillar, 40, 41, "minecraft:netherrack");
    fill(&mut pillar, 64, 65, "minecraft:netherrack");
    assert!(place(
        data,
        "minecraft:basalt_pillar",
        &mut pillar,
        7,
        BlockPos::new(8, 63, 8)
    ));
    assert!(count_blocks(&pillar, "minecraft:basalt") > 0);

    let mut delta = flat_chunk(64, "minecraft:netherrack");
    for x in 6..=10 {
        for z in 6..=10 {
            delta.set_block_state(x, 64, z, block("minecraft:air"));
        }
    }
    assert_places_for_some_seed(
        data,
        "minecraft:delta",
        || delta.clone(),
        BlockPos::new(8, 64, 8),
        "minecraft:magma_block",
    );
}

#[test]
fn shipped_dripstone_features_place() {
    let data = FeatureData::vanilla();
    let cavity = || {
        let mut chunk = empty_chunk();
        fill(&mut chunk, MIN_Y, 40, "minecraft:stone");
        fill(&mut chunk, 60, 80, "minecraft:stone");
        chunk
    };
    assert_places_for_some_seed(
        data,
        "minecraft:dripstone_cluster",
        cavity,
        BlockPos::new(8, 50, 8),
        "minecraft:pointed_dripstone",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:large_dripstone",
        cavity,
        BlockPos::new(8, 50, 8),
        "minecraft:dripstone_block",
    );
    assert_places_for_some_seed(
        data,
        "minecraft:pointed_dripstone",
        || {
            let mut chunk = flat_chunk(63, "minecraft:stone");
            chunk.set_block_state(8, 62, 8, block("minecraft:air"));
            chunk
        },
        BlockPos::new(8, 62, 8),
        "minecraft:pointed_dripstone",
    );
}

#[test]
fn generator_decorations_are_deterministic() {
    let generator = folia_core::worldgen::generator::WorldGenerator::overworld(0).unwrap();
    let first = generator.generate_chunk(0, 0).unwrap();
    let second = generator.generate_chunk(0, 0).unwrap();
    assert_eq!(first, second);
}
