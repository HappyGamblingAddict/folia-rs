//! Aquatic, vine, and mushroom feature tests.

use folia_core::chunk::{BlockState, Chunk};
use folia_core::random::LegacyRandomSource;
use folia_core::worldgen::feature::{FeatureData, WorldGenRegion};
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

fn fill_chunk(chunk: &mut Chunk, from: i32, to: i32, name: &str) {
    let state = block(name);
    for x in 0..16 {
        for z in 0..16 {
            for y in from..to {
                chunk.set_block_state_ref(x, y, z, &state);
            }
        }
    }
}

fn flat_chunk(top: i32, top_block: &str) -> Chunk {
    let mut chunk = empty_chunk();
    fill_chunk(&mut chunk, MIN_Y, top, "minecraft:stone");
    fill_chunk(&mut chunk, top, top + 1, top_block);
    chunk
}

fn water_chunk(floor: i32, surface: i32) -> Chunk {
    let mut chunk = flat_chunk(floor, "minecraft:sand");
    fill_chunk(&mut chunk, floor + 1, surface + 1, "minecraft:water");
    chunk
}

fn set(chunk: &mut Chunk, x: i32, y: i32, z: i32, name: &str) {
    chunk.set_block_state(x as usize, y, z as usize, block(name));
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

fn count_prefix(chunk: &Chunk, prefix: &str) -> usize {
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
        .filter(|state| state.name.starts_with(prefix))
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

fn data_with_one(id: &str, entry: Value) -> FeatureData {
    let mut configured = serde_json::Map::new();
    configured.insert(id.to_owned(), entry);
    data_with(Value::Object(configured))
}

fn count_containing(chunk: &Chunk, needle: &str) -> usize {
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
        .filter(|state| state.name.contains(needle))
        .count()
}

fn place(data: &FeatureData, id: &str, chunk: &mut Chunk, seed: i64, origin: BlockPos) -> bool {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
    let mut random = LegacyRandomSource::new(seed);
    feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable")
}

fn place_twice(data: &FeatureData, id: &str, chunk: &Chunk, seed: i64, origin: BlockPos) -> Chunk {
    let mut first = chunk.clone();
    let mut second = chunk.clone();
    assert!(place(data, id, &mut first, seed, origin));
    assert!(place(data, id, &mut second, seed, origin));
    assert_eq!(first, second, "{id} must replay deterministically");
    first
}

#[test]
fn shipped_aquatic_vine_and_mushroom_configs_parse() {
    let data = FeatureData::vanilla();
    for (id, vanilla_type) in [
        ("minecraft:seagrass_short", "minecraft:seagrass"),
        ("minecraft:seagrass_tall", "minecraft:seagrass"),
        ("minecraft:kelp", "minecraft:kelp"),
        ("minecraft:sea_pickle", "minecraft:sea_pickle"),
        ("minecraft:vines", "minecraft:vines"),
        ("minecraft:twisting_vines", "minecraft:twisting_vines"),
        (
            "minecraft:twisting_vines_bonemeal",
            "minecraft:twisting_vines",
        ),
        ("minecraft:weeping_vines", "minecraft:weeping_vines"),
        (
            "minecraft:huge_brown_mushroom",
            "minecraft:huge_brown_mushroom",
        ),
        ("minecraft:huge_red_mushroom", "minecraft:huge_red_mushroom"),
        ("minecraft:warped_fungus", "minecraft:huge_fungus"),
        ("minecraft:crimson_fungus_planted", "minecraft:huge_fungus"),
        ("minecraft:glowstone_extra", "minecraft:glowstone_blob"),
        ("minecraft:forest_rock", "minecraft:forest_rock"),
        (
            "minecraft:nether_sprouts",
            "minecraft:nether_forest_vegetation",
        ),
        (
            "minecraft:warped_forest_vegetation",
            "minecraft:nether_forest_vegetation",
        ),
        ("minecraft:underwater_magma", "minecraft:underwater_magma"),
        ("minecraft:bamboo_no_podzol", "minecraft:bamboo"),
        ("minecraft:bamboo_some_podzol", "minecraft:bamboo"),
        (
            "minecraft:mushroom_island_vegetation",
            "minecraft:random_boolean_selector",
        ),
    ] {
        let feature = data
            .configured(id)
            .unwrap_or_else(|| panic!("{id} is in the dump"));
        assert!(feature.is_supported(), "{id} should be executable");
        assert_eq!(feature.vanilla_type(), vanilla_type, "{id} type");
    }
    // Unshipped coral and fill-layer types still have to dispatch when a
    // datapack provides them.
    for (id, vanilla_type) in [
        ("test:coral_tree", "minecraft:coral_tree"),
        ("test:coral_claw", "minecraft:coral_claw"),
        ("test:coral_mushroom", "minecraft:coral_mushroom"),
        ("test:fill_layer", "minecraft:fill_layer"),
    ] {
        let config = if vanilla_type == "minecraft:fill_layer" {
            json!({"height": 1, "state": {"Name": "minecraft:stone"}})
        } else {
            json!({})
        };
        let data = data_with_one(id, json!({"type": vanilla_type, "config": config}));
        let feature = data.configured(id).expect("synthetic feature parses");
        assert!(feature.is_supported(), "{id} should be executable");
        assert_eq!(feature.vanilla_type(), vanilla_type);
    }
    // The vegetation-gap slice added `simple_random_selector` and scheduled
    // simple blocks, so the lush-caves clay selector is now executable.
    assert!(
        data.configured("minecraft:lush_caves_clay")
            .expect("lush_caves_clay")
            .is_supported()
    );
}

#[test]
fn seagrass_places_short_and_tall_plants() {
    let short_data = data_with_one(
        "test:seagrass_short",
        json!({"type": "minecraft:seagrass", "config": {"probability": 0.0}}),
    );
    let tall_data = data_with_one(
        "test:seagrass_tall",
        json!({"type": "minecraft:seagrass", "config": {"probability": 1.0}}),
    );
    let chunk = water_chunk(50, 63);
    let short = place_twice(
        &short_data,
        "test:seagrass_short",
        &chunk,
        3,
        BlockPos::new(8, 51, 8),
    );
    assert_eq!(
        count_blocks(&short, "minecraft:seagrass"),
        1,
        "probability 0 always places one short plant"
    );
    assert_eq!(count_blocks(&short, "minecraft:tall_seagrass"), 0);

    let tall = place_twice(
        &tall_data,
        "test:seagrass_tall",
        &chunk,
        9,
        BlockPos::new(8, 51, 8),
    );
    let tall_lower = count_blocks(&tall, "minecraft:tall_seagrass");
    assert_eq!(
        tall_lower, 2,
        "tall seagrass is placed as a lower/upper pair"
    );
    // Every tall seagrass has a matching upper half above it.
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MIN_Y + HEIGHT {
                let Some(state) = tall.block_state(x, y, z) else {
                    continue;
                };
                if state.name != "minecraft:tall_seagrass" {
                    continue;
                }
                let half = state
                    .properties
                    .iter()
                    .find(|(key, _)| key == "half")
                    .map(|(_, value)| value.as_str());
                if half == Some("lower") {
                    let above = tall.block_state(x, y + 1, z).expect("upper half exists");
                    assert_eq!(above.name, "minecraft:tall_seagrass");
                    assert!(
                        above
                            .properties
                            .iter()
                            .any(|(key, value)| key == "half" && value == "upper")
                    );
                }
            }
        }
    }
}

#[test]
fn kelp_grows_a_head_and_body_column() {
    let data = FeatureData::vanilla();
    let chunk = water_chunk(40, 63);
    let kelp = place_twice(data, "minecraft:kelp", &chunk, 5, BlockPos::new(8, 41, 8));
    let heads = count_blocks(&kelp, "minecraft:kelp");
    let plants = count_blocks(&kelp, "minecraft:kelp_plant");
    assert!(heads > 0, "kelp places a head");
    assert!(plants > 0, "deep water places kelp plants below the head");
}

#[test]
fn sea_pickles_place_in_groups() {
    let data = FeatureData::vanilla();
    let chunk = water_chunk(50, 63);
    let pickles = place_twice(
        data,
        "minecraft:sea_pickle",
        &chunk,
        11,
        BlockPos::new(8, 51, 8),
    );
    assert!(count_blocks(&pickles, "minecraft:sea_pickle") > 0);
}

#[test]
fn coral_tree_claw_and_mushroom_place_in_water() {
    let chunk = water_chunk(50, 70);
    for id in [
        "minecraft:coral_tree",
        "minecraft:coral_claw",
        "minecraft:coral_mushroom",
    ] {
        // These types are not in the shipped dump, so drive them through the
        // same inline configured-feature path datapacks use.
        let synthetic = data_with_one(id, json!({"type": id, "config": {}}));
        let placed = place_twice(&synthetic, id, &chunk, 13, BlockPos::new(8, 51, 8));
        assert!(
            count_containing(&placed, "coral") > 0,
            "{id} should add coral blocks"
        );
        let mut wall_fans = 0;
        for section in &placed.sections {
            for state in &section.block_states.palette {
                if state.name.ends_with("_coral_wall_fan") {
                    wall_fans += 1;
                    let facing = state
                        .properties
                        .iter()
                        .find(|(key, _)| key == "facing")
                        .map(|(_, value)| value.as_str());
                    assert!(
                        matches!(facing, Some("north" | "east" | "south" | "west")),
                        "{id} wall fan must carry a horizontal facing, got {facing:?}"
                    );
                }
            }
        }
        assert!(wall_fans > 0, "{id} should place at least one wall fan");
    }
}

#[test]
fn vines_attach_to_a_sturdy_face() {
    let data = FeatureData::vanilla();
    let mut chunk = empty_chunk();
    set(&mut chunk, 8, 60, 7, "minecraft:stone");
    let vines = place_twice(data, "minecraft:vines", &chunk, 21, BlockPos::new(8, 60, 8));
    let vine = vines.block_state(8, 60, 8).expect("vine was written");
    assert_eq!(vine.name, "minecraft:vine");
    let north = vine
        .properties
        .iter()
        .find(|(key, _)| key == "north")
        .map(|(_, value)| value.as_str());
    assert_eq!(north, Some("true"), "the north face touched stone");
    let up = vine
        .properties
        .iter()
        .find(|(key, _)| key == "up")
        .map(|(_, value)| value.as_str());
    assert_eq!(up, Some("false"));
}

#[test]
fn twisting_and_weeping_vines_place_heads_and_bodies() {
    let data = FeatureData::vanilla();
    let mut twisting_chunk = empty_chunk();
    fill_chunk(&mut twisting_chunk, 60, 70, "minecraft:netherrack");
    let twisting = place_twice(
        data,
        "minecraft:twisting_vines",
        &twisting_chunk,
        31,
        BlockPos::new(8, 70, 8),
    );
    assert!(
        count_prefix(&twisting, "minecraft:twisting_vines") > 0,
        "twisting vines spread around the origin"
    );

    let mut weeping_chunk = empty_chunk();
    fill_chunk(&mut weeping_chunk, 71, 72, "minecraft:netherrack");
    let weeping = place_twice(
        data,
        "minecraft:weeping_vines",
        &weeping_chunk,
        37,
        BlockPos::new(8, 70, 8),
    );
    assert!(count_blocks(&weeping, "minecraft:nether_wart_block") > 0);
    assert!(
        count_prefix(&weeping, "minecraft:weeping_vines") > 0,
        "weeping vines hang from the wart roof"
    );
}

#[test]
fn huge_mushrooms_place_stem_and_cap() {
    let data = FeatureData::vanilla();
    let chunk = flat_chunk(63, "minecraft:dirt");
    let brown = place_twice(
        data,
        "minecraft:huge_brown_mushroom",
        &chunk,
        41,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&brown, "minecraft:mushroom_stem") >= 4);
    assert!(count_blocks(&brown, "minecraft:brown_mushroom_block") > 0);
    let cap = brown
        .sections
        .iter()
        .flat_map(|section| section.block_states.palette.iter())
        .find(|state| state.name == "minecraft:brown_mushroom_block")
        .expect("brown cap exists");
    assert!(
        cap.properties.iter().any(|(key, _)| key == "west"),
        "brown cap carries its connection properties"
    );

    let red = place_twice(
        data,
        "minecraft:huge_red_mushroom",
        &chunk,
        43,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&red, "minecraft:mushroom_stem") >= 4);
    assert!(count_blocks(&red, "minecraft:red_mushroom_block") > 0);
}

#[test]
fn huge_fungus_places_stem_hat_and_decor() {
    let data = FeatureData::vanilla();
    let chunk = flat_chunk(63, "minecraft:warped_nylium");
    let fungus = place_twice(
        data,
        "minecraft:warped_fungus",
        &chunk,
        47,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&fungus, "minecraft:warped_stem") >= 4);
    assert!(
        count_blocks(&fungus, "minecraft:warped_wart_block") > 0
            || count_blocks(&fungus, "minecraft:shroomlight") > 0,
        "the hat or its decoration is present"
    );
    let planted = place_twice(
        data,
        "minecraft:crimson_fungus_planted",
        &flat_chunk(63, "minecraft:crimson_nylium"),
        53,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&planted, "minecraft:crimson_stem") >= 4);
}

#[test]
fn glowstone_blobs_replace_air_under_a_nether_ceiling() {
    let data = FeatureData::vanilla();
    let mut chunk = empty_chunk();
    set(&mut chunk, 8, 71, 8, "minecraft:netherrack");
    let glowstone = place_twice(
        data,
        "minecraft:glowstone_extra",
        &chunk,
        59,
        BlockPos::new(8, 70, 8),
    );
    assert!(count_blocks(&glowstone, "minecraft:glowstone") >= 1);
}

#[test]
fn forest_rock_is_a_block_blob_on_dirt_or_stone() {
    let data = FeatureData::vanilla();
    let chunk = flat_chunk(63, "minecraft:grass_block");
    let blob = place_twice(
        data,
        "minecraft:forest_rock",
        &chunk,
        61,
        BlockPos::new(8, 66, 8),
    );
    assert!(count_blocks(&blob, "minecraft:mossy_cobblestone") > 0);
}

#[test]
fn fill_layer_writes_the_configured_layer() {
    let data = data_with(json!({
        "test:fill": {
            "type": "minecraft:fill_layer",
            "config": {"height": 5, "state": {"Name": "minecraft:bedrock"}}
        }
    }));
    let chunk = place_twice(
        &data,
        "test:fill",
        &empty_chunk(),
        67,
        BlockPos::new(0, 0, 0),
    );
    assert_eq!(count_blocks(&chunk, "minecraft:bedrock"), 16 * 16);
}

#[test]
fn nether_forest_vegetation_places_plants_on_nylium() {
    let data = data_with_one(
        "test:nether_vegetation",
        json!({
            "type": "minecraft:nether_forest_vegetation",
            "config": {
                "spread_width": 1,
                "spread_height": 1,
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": {"Name": "minecraft:warped_roots"}
                }
            }
        }),
    );
    let chunk = flat_chunk(63, "minecraft:warped_nylium");
    let vegetation = place_twice(
        &data,
        "test:nether_vegetation",
        &chunk,
        71,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&vegetation, "minecraft:warped_roots") > 0);
}

#[test]
fn underwater_magma_replaces_enclosed_stone() {
    let data = FeatureData::vanilla();
    let chunk = water_chunk(59, 63);
    let magma = place_twice(
        data,
        "minecraft:underwater_magma",
        &chunk,
        73,
        BlockPos::new(8, 60, 8),
    );
    assert!(count_blocks(&magma, "minecraft:magma_block") > 0);
}

#[test]
fn bamboo_places_a_leafy_column() {
    let data = FeatureData::vanilla();
    let chunk = flat_chunk(63, "minecraft:dirt");
    let bamboo = place_twice(
        data,
        "minecraft:bamboo_no_podzol",
        &chunk,
        79,
        BlockPos::new(8, 64, 8),
    );
    assert!(count_blocks(&bamboo, "minecraft:bamboo") >= 5);
    let has_leaves = bamboo
        .sections
        .iter()
        .flat_map(|section| section.block_states.palette.iter())
        .filter(|state| state.name == "minecraft:bamboo")
        .any(|state| {
            state
                .properties
                .iter()
                .any(|(key, value)| key == "leaves" && value != "none")
        });
    assert!(has_leaves);
}

#[test]
fn generator_decoration_with_aquatic_features_is_deterministic() {
    let generator = folia_core::worldgen::generator::WorldGenerator::overworld(12345).unwrap();
    let first = generator.generate_chunk(0, -1).unwrap();
    let second = generator.generate_chunk(0, -1).unwrap();
    assert_eq!(first, second);
    assert!(
        count_blocks(&first, "minecraft:seagrass")
            + count_blocks(&first, "minecraft:tall_seagrass")
            > 0,
        "the ocean fixture chunk should contain generated seagrass"
    );
}
