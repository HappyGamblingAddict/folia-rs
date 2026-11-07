//! Ground-cover vegetation feature tests.

use folia_core::chunk::{BlockState, Chunk};
use folia_core::random::LegacyRandomSource;
use folia_core::tick::ScheduledTick;
use folia_core::worldgen::feature::{Decoration, FeatureData, WorldGenRegion};
use folia_math::BlockPos;
use serde_json::{Value, json};

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

fn data_with(configured: Value) -> FeatureData {
    FeatureData::parse(
        &json!({
            "configured": configured,
            "placed": {},
            "biomes": {},
            "tags": {}
        })
        .to_string(),
    )
    .expect("synthetic feature data must parse")
}

fn place(data: &FeatureData, id: &str, chunk: &mut Chunk, seed: i64, origin: BlockPos) -> bool {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
    let mut random = LegacyRandomSource::new(seed);
    feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable")
}

/// Places a feature and drains the ticks it queued.
fn place_tracked(
    data: &FeatureData,
    id: &str,
    top: i32,
    top_block: &str,
    seed: i64,
    origin: BlockPos,
) -> (Chunk, Vec<ScheduledTick>) {
    let feature = data.configured(id).expect("configured feature exists");
    let mut chunk = flat_chunk(top, top_block);
    let ticks = {
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        let mut random = LegacyRandomSource::new(seed);
        feature
            .place(&mut level, &mut random, origin)
            .expect("feature should be executable");
        level.scheduled_ticks_mut().tick(1)
    };
    (chunk, ticks)
}

fn simple_block_provider(name: &str) -> Value {
    json!({
        "type": "minecraft:simple_state_provider",
        "state": {"Name": name}
    })
}

#[test]
fn shipped_ground_cover_features_parse() {
    let data = FeatureData::vanilla();
    for id in [
        "minecraft:patch_grass",
        "minecraft:flower_plain",
        "minecraft:single_piece_of_grass",
        "minecraft:pile_hay",
        "minecraft:cave_vine",
        "minecraft:glow_lichen",
        "minecraft:moss_patch",
        "minecraft:pale_moss_patch",
    ] {
        let feature = data
            .configured(id)
            .unwrap_or_else(|| panic!("{id} is in the dump"));
        assert!(feature.is_supported(), "{id} should be executable");
    }
    assert!(
        data.placed("minecraft:patch_grass_plain").is_some(),
        "the shipped patch_grass_plain placement loads"
    );
    assert_eq!(
        data.configured("minecraft:flower_plain")
            .expect("flower_plain configured feature")
            .vanilla_type(),
        "minecraft:flower"
    );
    assert_eq!(
        data.configured("minecraft:pale_moss_patch")
            .expect("pale moss patch")
            .vanilla_type(),
        "minecraft:vegetation_patch"
    );
}

#[test]
fn random_patch_places_inline_simple_block_deterministically() {
    let data = data_with(json!({
        "test:patch": {
            "type": "minecraft:random_patch",
            "config": {
                "tries": 8,
                "xz_spread": 0,
                "y_spread": 0,
                "feature": {
                    "feature": {
                        "type": "minecraft:simple_block",
                        "config": {"to_place": simple_block_provider("minecraft:short_grass")}
                    },
                    "placement": [{
                        "type": "minecraft:block_predicate_filter",
                        "predicate": {"type": "minecraft:matching_blocks", "blocks": "minecraft:air"}
                    }]
                }
            }
        }
    }));
    let mut first = flat_chunk(63, "minecraft:grass_block");
    let mut second = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        &data,
        "test:patch",
        &mut first,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert!(place(
        &data,
        "test:patch",
        &mut second,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(first, second, "random patch must replay deterministically");
    assert_eq!(count_blocks(&first, "minecraft:short_grass"), 1);
}

#[test]
fn random_patch_reports_failure_when_the_inner_feature_places_nothing() {
    let data = data_with(json!({
        "test:patch": {
            "type": "minecraft:random_patch",
            "config": {
                "tries": 4,
                "xz_spread": 0,
                "y_spread": 0,
                "feature": {
                    "feature": {
                        "type": "minecraft:simple_block",
                        "config": {"to_place": simple_block_provider("minecraft:short_grass")}
                    },
                    "placement": [{
                        "type": "minecraft:block_predicate_filter",
                        "predicate": {"type": "minecraft:matching_blocks", "blocks": "minecraft:air"}
                    }]
                }
            }
        }
    }));
    // Stone is not a valid plant base, and the predicate is satisfied first
    // only when the position is air. Placing the patch into solid stone makes
    // every attempt fail without writing anything.
    let mut chunk = flat_chunk(63, "minecraft:stone");
    for x in 0..16 {
        for z in 0..16 {
            for y in 64..70 {
                chunk.set_block_state(x, y, z, block("minecraft:stone"));
            }
        }
    }
    assert!(!place(
        &data,
        "test:patch",
        &mut chunk,
        11,
        BlockPos::new(8, 64, 8)
    ));
}

#[test]
fn flower_plain_places_shipped_noise_threshold_states() {
    let data = FeatureData::vanilla();
    let mut chunk = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        data,
        "minecraft:flower_plain",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    let flowers = [
        "minecraft:dandelion",
        "minecraft:poppy",
        "minecraft:azure_bluet",
        "minecraft:oxeye_daisy",
        "minecraft:cornflower",
        "minecraft:orange_tulip",
        "minecraft:red_tulip",
        "minecraft:pink_tulip",
        "minecraft:white_tulip",
    ];
    let placed: usize = flowers.iter().map(|name| count_blocks(&chunk, name)).sum();
    assert!(placed > 0, "flower_plain should place flowers");
}

#[test]
fn simple_block_checks_survival_and_double_plants() {
    let data = data_with(json!({
        "test:grass": {
            "type": "minecraft:simple_block",
            "config": {"to_place": simple_block_provider("minecraft:short_grass")}
        },
        "test:tall_grass": {
            "type": "minecraft:simple_block",
            "config": {"to_place": {
                "type": "minecraft:simple_state_provider",
                "state": {"Name": "minecraft:tall_grass", "Properties": {"half": "lower"}}
            }}
        },
        "test:lily": {
            "type": "minecraft:simple_block",
            "config": {"to_place": simple_block_provider("minecraft:lily_pad")}
        }
    }));
    let mut good = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        &data,
        "test:grass",
        &mut good,
        1,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&good, "minecraft:short_grass"), 1);

    let mut bad = flat_chunk(63, "minecraft:stone");
    assert!(!place(
        &data,
        "test:grass",
        &mut bad,
        1,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&bad, "minecraft:short_grass"), 0);

    let mut tall = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        &data,
        "test:tall_grass",
        &mut tall,
        1,
        BlockPos::new(8, 64, 8)
    ));
    let lower = tall.block_state(8, 64, 8).unwrap();
    let upper = tall.block_state(8, 65, 8).unwrap();
    assert_eq!(lower.name, "minecraft:tall_grass");
    assert_eq!(
        lower.properties,
        vec![("half".to_owned(), "lower".to_owned())]
    );
    assert_eq!(upper.name, "minecraft:tall_grass");
    assert_eq!(
        upper.properties,
        vec![("half".to_owned(), "upper".to_owned())]
    );

    let mut water = flat_chunk(63, "minecraft:water");
    assert!(place(
        &data,
        "test:lily",
        &mut water,
        1,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&water, "minecraft:lily_pad"), 1);
}

#[test]
fn block_pile_places_on_sturdy_ground_and_rejects_low_origins() {
    let data = data_with(json!({
        "test:pile": {
            "type": "minecraft:block_pile",
            "config": {"state_provider": simple_block_provider("minecraft:snow")}
        }
    }));
    let mut chunk = flat_chunk(63, "minecraft:stone");
    assert!(place(
        &data,
        "test:pile",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert!(count_blocks(&chunk, "minecraft:snow") > 0);

    let mut low = flat_chunk(63, "minecraft:stone");
    assert!(!place(
        &data,
        "test:pile",
        &mut low,
        7,
        BlockPos::new(8, MIN_Y + 4, 8)
    ));
    assert_eq!(count_blocks(&low, "minecraft:snow"), 0);
}

#[test]
fn block_column_places_layers_and_truncates_at_an_obstruction() {
    let data = data_with(json!({
        "test:column": {
            "type": "minecraft:block_column",
            "config": {
                "direction": "up",
                "allowed_placement": {"type": "minecraft:matching_blocks", "blocks": "minecraft:air"},
                "prioritize_tip": false,
                "layers": [
                    {"height": 2, "provider": simple_block_provider("minecraft:cactus")},
                    {"height": 1, "provider": simple_block_provider("minecraft:cactus_flower")}
                ]
            }
        }
    }));
    let mut chunk = flat_chunk(63, "minecraft:sand");
    assert!(place(
        &data,
        "test:column",
        &mut chunk,
        3,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&chunk, "minecraft:cactus"), 2);
    assert_eq!(count_blocks(&chunk, "minecraft:cactus_flower"), 1);

    let mut obstructed = flat_chunk(63, "minecraft:sand");
    obstructed.set_block_state(8, 66, 8, block("minecraft:stone"));
    assert!(place(
        &data,
        "test:column",
        &mut obstructed,
        3,
        BlockPos::new(8, 64, 8)
    ));
    // The validation probe starts one block above the origin, so the stone at
    // y=66 truncates the tail of the column (the flower layer first, then one
    // cactus layer).
    assert_eq!(count_blocks(&obstructed, "minecraft:cactus"), 1);
    assert_eq!(count_blocks(&obstructed, "minecraft:cactus_flower"), 0);
}

#[test]
fn multiface_growth_attaches_to_an_allowed_face() {
    let data = data_with(json!({
        "test:lichen": {
            "type": "minecraft:multiface_growth",
            "config": {
                "block": "minecraft:glow_lichen",
                "search_range": 4,
                "can_place_on_floor": false,
                "can_place_on_ceiling": true,
                "can_place_on_wall": true,
                "chance_of_spreading": 0.0,
                "can_be_placed_on": ["minecraft:stone"]
            }
        }
    }));
    let mut chunk = empty_chunk();
    chunk.set_block_state(9, 64, 8, block("minecraft:stone"));
    assert!(place(
        &data,
        "test:lichen",
        &mut chunk,
        5,
        BlockPos::new(8, 64, 8)
    ));
    let placed = chunk.block_state(8, 64, 8).expect("growth was placed");
    assert_eq!(placed.name, "minecraft:glow_lichen");
    assert!(
        placed
            .properties
            .iter()
            .any(|(name, value)| name == "east" && value == "true"),
        "glow lichen should attach to the stone on its east face: {placed:?}"
    );

    let mut empty = empty_chunk();
    assert!(!place(
        &data,
        "test:lichen",
        &mut empty,
        5,
        BlockPos::new(8, 64, 8)
    ));
}

#[test]
fn vegetation_patch_replaces_ground_and_places_vegetation() {
    let data = data_with(json!({
        "test:vegetation": {
            "type": "minecraft:simple_block",
            "config": {"to_place": simple_block_provider("minecraft:short_grass")}
        },
        "test:patch": {
            "type": "minecraft:vegetation_patch",
            "config": {
                "replaceable": "#minecraft:moss_replaceable",
                "ground_state": simple_block_provider("minecraft:moss_block"),
                "vegetation_feature": {"feature": "test:vegetation", "placement": []},
                "surface": "floor",
                "depth": 1,
                "extra_bottom_block_chance": 0.0,
                "vertical_range": 5,
                "vegetation_chance": 1.0,
                "xz_radius": 1,
                "extra_edge_column_chance": 0.0
            }
        }
    }));
    let mut chunk = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        &data,
        "test:patch",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&chunk, "minecraft:moss_block"), 9);
    assert_eq!(count_blocks(&chunk, "minecraft:short_grass"), 9);
}

#[test]
fn waterlogged_vegetation_patch_fills_enclosed_ground_with_water() {
    let data = data_with(json!({
        "test:patch": {
            "type": "minecraft:waterlogged_vegetation_patch",
            "config": {
                "replaceable": "#minecraft:moss_replaceable",
                "ground_state": simple_block_provider("minecraft:moss_block"),
                "vegetation_feature": {"feature": "test:noop", "placement": []},
                "surface": "floor",
                "depth": 1,
                "extra_bottom_block_chance": 0.0,
                "vertical_range": 5,
                "vegetation_chance": 0.0,
                "xz_radius": 1,
                "extra_edge_column_chance": 0.0
            }
        },
        "test:noop": {"type": "minecraft:no_op"}
    }));
    assert_eq!(
        data.configured("test:patch")
            .expect("waterlogged patch")
            .vanilla_type(),
        "minecraft:waterlogged_vegetation_patch"
    );
    let mut chunk = flat_chunk(63, "minecraft:stone");
    assert!(place(
        &data,
        "test:patch",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&chunk, "minecraft:water"), 9);
    assert_eq!(count_blocks(&chunk, "minecraft:moss_block"), 0);
}

#[test]
fn shipped_moss_and_pale_moss_patches_place_ground_cover() {
    let data = FeatureData::vanilla();
    let mut moss = flat_chunk(63, "minecraft:stone");
    assert!(place(
        data,
        "minecraft:moss_patch",
        &mut moss,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert!(count_blocks(&moss, "minecraft:moss_block") > 0);

    let mut pale = flat_chunk(63, "minecraft:stone");
    assert!(place(
        data,
        "minecraft:pale_moss_patch",
        &mut pale,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert!(count_blocks(&pale, "minecraft:pale_moss_block") > 0);
}

#[test]
fn decoration_steps_include_ground_cover_passes() {
    let data = FeatureData::vanilla();
    let order = data.step_feature_order();
    assert!(
        order[Decoration::UndergroundDecoration.index()]
            .contains(&"minecraft:sculk_vein".to_owned())
    );
    assert!(
        order[Decoration::VegetalDecoration.index()]
            .contains(&"minecraft:patch_grass_plain".to_owned())
    );
}

#[test]
fn shipped_feature_support_snapshot() {
    let data = FeatureData::vanilla();
    assert_eq!(
        data.supported_feature_count(),
        218,
        "end/dungeon/well/sculk batch raises configured-feature support"
    );
    let unsupported = data.unsupported_type_counts();
    for kind in [
        "minecraft:random_patch",
        "minecraft:flower",
        "minecraft:no_bonemeal_flower",
        "minecraft:vegetation_patch",
        "minecraft:waterlogged_vegetation_patch",
        "minecraft:random_boolean_selector",
        "minecraft:random_selector",
        "minecraft:simple_random_selector",
        "minecraft:tree",
        "minecraft:disk",
        "minecraft:spring_feature",
        "minecraft:dripstone_cluster",
        "minecraft:large_dripstone",
        "minecraft:pointed_dripstone",
    ] {
        assert!(
            !unsupported.contains_key(kind),
            "{kind} should have no unsupported shipped configs"
        );
    }
    for id in [
        "minecraft:pale_oak",
        "minecraft:pale_oak_creaking",
        "minecraft:pale_forest_flowers",
        "minecraft:flower_pale_garden",
        "minecraft:dripleaf",
        "minecraft:clay_with_dripleaves",
        "minecraft:clay_pool_with_dripleaves",
        "minecraft:lush_caves_clay",
        "minecraft:pale_garden_vegetation",
        "minecraft:warm_ocean_vegetation",
        "minecraft:pointed_dripstone",
    ] {
        assert!(
            data.configured(id)
                .unwrap_or_else(|| panic!("{id} is in the dump"))
                .is_supported(),
            "{id} should be executable"
        );
    }
}

#[test]
fn shipped_pale_forest_trees_place_pale_moss() {
    let data = FeatureData::vanilla();
    let mut found = None;
    for seed in 0..64 {
        let mut chunk = flat_chunk(63, "minecraft:grass_block");
        if !place(
            data,
            "minecraft:pale_oak",
            &mut chunk,
            seed,
            BlockPos::new(8, 64, 8),
        ) {
            continue;
        }
        if count_blocks(&chunk, "minecraft:pale_moss_block") > 0
            || count_blocks(&chunk, "minecraft:pale_moss_carpet") > 0
        {
            found = Some(seed);
            break;
        }
    }
    let seed = found.expect("a pale oak seed places pale moss");

    let mut first = flat_chunk(63, "minecraft:grass_block");
    let mut second = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        data,
        "minecraft:pale_oak",
        &mut first,
        seed,
        BlockPos::new(8, 64, 8)
    ));
    assert!(place(
        data,
        "minecraft:pale_oak",
        &mut second,
        seed,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(first, second, "pale oak placement is deterministic");
    assert!(
        count_blocks(&first, "minecraft:pale_oak_log") > 0,
        "the trunk is placed"
    );
}

#[test]
fn shipped_pale_forest_flowers_place_and_schedule_ticks() {
    let data = FeatureData::vanilla();
    let mut found = None;
    for seed in 0..64 {
        let (chunk, ticks) = place_tracked(
            data,
            "minecraft:pale_forest_flowers",
            63,
            "minecraft:grass_block",
            seed,
            BlockPos::new(8, 64, 8),
        );
        let flowers = count_blocks(&chunk, "minecraft:closed_eyeblossom");
        if flowers > 0 {
            found = Some((seed, chunk, ticks));
            break;
        }
    }
    let (seed, chunk, ticks) = found.expect("a pale forest seed places closed eyeblossoms");
    let flowers = count_blocks(&chunk, "minecraft:closed_eyeblossom");
    assert_eq!(
        ticks.len(),
        flowers,
        "every placed closed eyeblossom schedules one tick"
    );
    assert!(
        ticks
            .iter()
            .all(|tick| tick.block == "minecraft:closed_eyeblossom")
    );
    let (repeat, repeat_ticks) = place_tracked(
        data,
        "minecraft:pale_forest_flowers",
        63,
        "minecraft:grass_block",
        seed,
        BlockPos::new(8, 64, 8),
    );
    assert_eq!(chunk, repeat, "pale forest flowers are deterministic");
    assert_eq!(ticks, repeat_ticks);

    // The single-try garden flower uses the same scheduled-tick path.
    let mut scheduled = false;
    for seed in 0..64 {
        let (chunk, ticks) = place_tracked(
            data,
            "minecraft:flower_pale_garden",
            63,
            "minecraft:grass_block",
            seed,
            BlockPos::new(8, 64, 8),
        );
        if count_blocks(&chunk, "minecraft:closed_eyeblossom") > 0 {
            assert_eq!(ticks.len(), 1);
            assert_eq!(ticks[0].block, "minecraft:closed_eyeblossom");
            scheduled = true;
            break;
        }
    }
    assert!(scheduled, "a flower_pale_garden seed places a flower");
}

#[test]
fn shipped_dripleaf_selector_places_one_branch_deterministically() {
    let data = FeatureData::vanilla();
    assert_eq!(
        data.configured("minecraft:dripleaf")
            .expect("dripleaf")
            .vanilla_type(),
        "minecraft:simple_random_selector"
    );
    let mut found = None;
    for seed in 0..64 {
        let mut chunk = flat_chunk(63, "minecraft:grass_block");
        if place(
            data,
            "minecraft:dripleaf",
            &mut chunk,
            seed,
            BlockPos::new(8, 64, 8),
        ) {
            let drip = chunk
                .sections
                .iter()
                .flat_map(|section| {
                    section
                        .block_states
                        .indices
                        .iter()
                        .map(move |index| &section.block_states.palette[*index as usize])
                })
                .filter(|state| state.name.contains("dripleaf"))
                .count();
            if drip > 0 {
                found = Some(seed);
                break;
            }
        }
    }
    let seed = found.expect("a dripleaf seed places a branch");

    let mut first = flat_chunk(63, "minecraft:grass_block");
    let mut second = flat_chunk(63, "minecraft:grass_block");
    assert!(place(
        data,
        "minecraft:dripleaf",
        &mut first,
        seed,
        BlockPos::new(8, 64, 8)
    ));
    assert!(place(
        data,
        "minecraft:dripleaf",
        &mut second,
        seed,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(first, second, "the selector branch is deterministic");
    assert!(
        first
            .sections
            .iter()
            .flat_map(|section| {
                section
                    .block_states
                    .palette
                    .iter()
                    .filter(|state| state.name.contains("dripleaf"))
            })
            .count()
            > 0
    );
}

#[test]
fn generator_decorations_are_deterministic_with_the_new_steps() {
    let generator = folia_core::worldgen::generator::WorldGenerator::overworld(0).unwrap();
    let first = generator.generate_chunk(0, 0).unwrap();
    let second = generator.generate_chunk(0, 0).unwrap();
    assert_eq!(first, second);
}

#[test]
fn floor_vegetation_patch_ignores_non_replaceable_ground() {
    let data = data_with(json!({
        "test:patch": {
            "type": "minecraft:vegetation_patch",
            "config": {
                "replaceable": "#minecraft:moss_replaceable",
                "ground_state": simple_block_provider("minecraft:moss_block"),
                "vegetation_feature": {"feature": "test:noop", "placement": []},
                "surface": "floor",
                "depth": 1,
                "extra_bottom_block_chance": 0.0,
                "vertical_range": 5,
                "vegetation_chance": 0.0,
                "xz_radius": 0,
                "extra_edge_column_chance": 0.0
            }
        },
        "test:noop": {"type": "minecraft:no_op"}
    }));
    let mut chunk = flat_chunk(63, "minecraft:bedrock");
    assert!(!place(
        &data,
        "test:patch",
        &mut chunk,
        7,
        BlockPos::new(8, 64, 8)
    ));
    assert_eq!(count_blocks(&chunk, "minecraft:moss_block"), 0);
}
