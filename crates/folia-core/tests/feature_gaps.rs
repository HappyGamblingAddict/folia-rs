//! Configured-feature gap coverage for the terrain, selector, and cave batch.

use folia_core::chunk::{BlockState, Chunk, PalettedContainer};
use folia_core::random::LegacyRandomSource;
use folia_core::tick::ScheduledTick;
use folia_core::worldgen::feature::{FeatureData, FeatureEntity, WorldGenRegion};
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

/// Runs a feature at an explicit level seed and vertical range, returning the
/// entity descriptors it queued.
#[allow(clippy::too_many_arguments)]
fn place_seeded_in(
    data: &FeatureData,
    id: &str,
    chunk: &mut Chunk,
    seed: i64,
    origin: BlockPos,
    level_seed: i64,
    min_y: i32,
    height: i32,
) -> (bool, Vec<FeatureEntity>) {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, min_y, height).with_seed(level_seed);
    let mut random = LegacyRandomSource::new(seed);
    let placed = feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable");
    (placed, level.take_entities())
}

fn set_biome(chunk: &mut Chunk, biome: &str) {
    // A generated chunk has a section for every vertical layer. Test chunks
    // are sparse, so create the missing layers before replacing the palettes.
    for y in (MIN_Y..MIN_Y + HEIGHT).step_by(16) {
        if chunk.block_state(0, y, 0).is_none() {
            chunk.set_block_state(0, y, 0, block("minecraft:air"));
        }
    }
    for section in &mut chunk.sections {
        section.biomes = Some(PalettedContainer {
            palette: vec![biome.to_owned()],
            indices: vec![0; 64],
        });
    }
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
    assert_eq!(data.supported_feature_count(), 224);
    assert!(
        data.unsupported_type_counts().is_empty(),
        "all shipped configured features should be supported"
    );
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

fn place_capture(
    data: &FeatureData,
    id: &str,
    chunk: &mut Chunk,
    seed: i64,
    origin: BlockPos,
) -> (bool, Vec<folia_core::worldgen::feature::FeatureBlockEntity>) {
    let feature = data.configured(id).expect("configured feature exists");
    let mut level = WorldGenRegion::new(chunk, MIN_Y, HEIGHT);
    let mut random = LegacyRandomSource::new(seed);
    let placed = feature
        .place(&mut level, &mut random, origin)
        .expect("feature should be executable");
    (placed, level.take_block_entities())
}

fn nbt_string<'a>(nbt: &'a folia_core::nbt::Tag, key: &str) -> Option<&'a str> {
    nbt.as_compound()?
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.as_str())
}

fn nbt_int_array(nbt: &folia_core::nbt::Tag, key: &str) -> Option<Vec<i32>> {
    nbt.as_compound()?
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.as_int_array().map(|values| values.to_vec()))
}

fn cave_chunk() -> Chunk {
    let mut chunk = flat_chunk(80, "minecraft:stone");
    let air = block("minecraft:air");
    for x in 5..=11 {
        for z in 5..=11 {
            for y in 51..=54 {
                chunk.set_block_state(x, y, z, air.clone());
            }
        }
    }
    for x in 3..=4 {
        for y in 51..=52 {
            chunk.set_block_state(x, y, 8, air.clone());
        }
    }
    chunk
}

#[test]
fn shipped_end_features_place() {
    let data = FeatureData::vanilla();

    let mut chorus = flat_chunk(63, "minecraft:end_stone");
    let (placed, _) = place_capture(
        data,
        "minecraft:chorus_plant",
        &mut chorus,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    assert!(count_blocks(&chorus, "minecraft:chorus_plant") > 0);
    assert!(count_blocks(&chorus, "minecraft:chorus_flower") > 0);

    let mut island = empty_chunk();
    let (placed, _) = place_capture(
        data,
        "minecraft:end_island",
        &mut island,
        7,
        BlockPos::new(8, 50, 8),
    );
    assert!(placed);
    assert!(count_blocks(&island, "minecraft:end_stone") > 0);

    let mut platform = empty_chunk();
    let (placed, _) = place_capture(
        data,
        "minecraft:end_platform",
        &mut platform,
        7,
        BlockPos::new(8, 49, 8),
    );
    assert!(placed);
    assert_eq!(count_blocks(&platform, "minecraft:obsidian"), 25);

    let mut void = empty_chunk();
    let (placed, _) = place_capture(
        data,
        "minecraft:void_start_platform",
        &mut void,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    assert_eq!(count_blocks(&void, "minecraft:cobblestone"), 1);
    assert_eq!(count_blocks(&void, "minecraft:stone"), 255);
}

#[test]
fn shipped_end_gateway_features_place_and_attach_the_exit() {
    let data = FeatureData::vanilla();

    let mut gateway = empty_chunk();
    let (placed, entities) = place_capture(
        data,
        "minecraft:end_gateway_return",
        &mut gateway,
        7,
        BlockPos::new(8, 50, 8),
    );
    assert!(placed);
    assert_eq!(
        gateway
            .block_state(8, 50, 8)
            .map(|state| state.name.as_str()),
        Some("minecraft:end_gateway")
    );
    assert!(count_blocks(&gateway, "minecraft:bedrock") > 0);
    let entity = entities
        .iter()
        .find(|entity| entity.pos == BlockPos::new(8, 50, 8))
        .expect("gateway block entity");
    assert_eq!(nbt_string(&entity.nbt, "id"), Some("minecraft:end_gateway"));
    assert_eq!(
        nbt_int_array(&entity.nbt, "exit_portal"),
        Some(vec![100, 50, 0])
    );

    let mut delayed = empty_chunk();
    let (placed, entities) = place_capture(
        data,
        "minecraft:end_gateway_delayed",
        &mut delayed,
        7,
        BlockPos::new(8, 50, 8),
    );
    assert!(placed);
    let entity = entities
        .iter()
        .find(|entity| entity.pos == BlockPos::new(8, 50, 8))
        .expect("gateway block entity");
    assert_eq!(nbt_int_array(&entity.nbt, "exit_portal"), None);
}

#[test]
fn shipped_bonus_chest_and_desert_well_place_with_block_entities() {
    let data = FeatureData::vanilla();

    let mut chest_chunk = flat_chunk(63, "minecraft:grass_block");
    let (placed, entities) = place_capture(
        data,
        "minecraft:bonus_chest",
        &mut chest_chunk,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    assert_eq!(count_blocks(&chest_chunk, "minecraft:chest"), 1);
    assert!(count_blocks(&chest_chunk, "minecraft:torch") > 0);
    let entity = entities
        .iter()
        .find(|entity| nbt_string(&entity.nbt, "id") == Some("minecraft:chest"))
        .expect("bonus chest entity");
    assert_eq!(
        nbt_string(&entity.nbt, "LootTable"),
        Some("minecraft:spawn_bonus_chest")
    );

    let mut well_chunk = flat_chunk(63, "minecraft:sand");
    let (placed, entities) = place_capture(
        data,
        "minecraft:desert_well",
        &mut well_chunk,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    assert!(count_blocks(&well_chunk, "minecraft:water") > 0);
    assert!(count_blocks(&well_chunk, "minecraft:sandstone") > 0);
    assert_eq!(count_blocks(&well_chunk, "minecraft:suspicious_sand"), 2);
    assert_eq!(
        entities
            .iter()
            .filter(|entity| {
                nbt_string(&entity.nbt, "id") == Some("minecraft:brushable_block")
                    && nbt_string(&entity.nbt, "LootTable")
                        == Some("minecraft:desert_well_archaeology")
            })
            .count(),
        2
    );
}

#[test]
fn shipped_monster_room_places_a_spawner_with_its_mob() {
    let data = FeatureData::vanilla();
    let (placed, entities) = place_capture(
        data,
        "minecraft:monster_room",
        &mut cave_chunk(),
        7,
        BlockPos::new(8, 51, 8),
    );
    assert!(placed);
    let spawner = entities
        .iter()
        .find(|entity| entity.pos == BlockPos::new(8, 51, 8))
        .expect("monster-room spawner");
    assert_eq!(
        nbt_string(&spawner.nbt, "id"),
        Some("minecraft:mob_spawner")
    );
    let spawn_data = spawner
        .nbt
        .as_compound()
        .and_then(|fields| fields.iter().find(|(name, _)| name == "SpawnData"))
        .map(|(_, value)| value)
        .expect("SpawnData");
    let entity_id = spawn_data
        .as_compound()
        .and_then(|fields| fields.iter().find(|(name, _)| name == "entity"))
        .map(|(_, value)| value)
        .and_then(|value| nbt_string(value, "id"))
        .expect("spawner entity id");
    assert!(matches!(
        entity_id,
        "minecraft:skeleton" | "minecraft:zombie" | "minecraft:spider"
    ));

    let first = cave_chunk();
    let second = cave_chunk();
    let mut first_chunk = first;
    let mut second_chunk = second;
    let (_, first_entities) = place_capture(
        data,
        "minecraft:monster_room",
        &mut first_chunk,
        7,
        BlockPos::new(8, 51, 8),
    );
    let (_, second_entities) = place_capture(
        data,
        "minecraft:monster_room",
        &mut second_chunk,
        7,
        BlockPos::new(8, 51, 8),
    );
    assert_eq!(first_chunk, second_chunk);
    assert_eq!(first_entities, second_entities);
}

#[test]
fn shipped_sculk_patches_place_deterministically() {
    let data = FeatureData::vanilla();
    let mut first = flat_chunk(63, "minecraft:stone");
    let (placed, entities) = place_capture(
        data,
        "minecraft:sculk_patch_deep_dark",
        &mut first,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
    let sculk = count_blocks(&first, "minecraft:sculk");
    let veins = count_blocks(&first, "minecraft:sculk_vein");
    assert!(
        sculk + veins > 0,
        "sculk patch must spread sculk or veins (sculk={sculk}, veins={veins})"
    );
    assert!(
        entities
            .iter()
            .all(|entity| nbt_string(&entity.nbt, "id") == Some("minecraft:sculk_catalyst"))
    );

    let mut replay = flat_chunk(63, "minecraft:stone");
    place(
        data,
        "minecraft:sculk_patch_deep_dark",
        &mut replay,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert_eq!(first, replay);

    let mut city = flat_chunk(63, "minecraft:sculk");
    let (placed, _) = place_capture(
        data,
        "minecraft:sculk_patch_ancient_city",
        &mut city,
        7,
        BlockPos::new(8, 64, 8),
    );
    assert!(placed);
}

#[test]
fn shipped_geode_places_deterministically_from_the_level_seed() {
    let data = FeatureData::vanilla();
    let geode = data
        .configured("minecraft:amethyst_geode")
        .expect("amethyst geode");
    assert!(geode.is_supported(), "geode must be executable");

    let mut first = flat_chunk(64, "minecraft:stone");
    let (placed, _) = place_seeded_in(
        data,
        "minecraft:amethyst_geode",
        &mut first,
        7,
        BlockPos::new(8, 24, 8),
        1234,
        MIN_Y,
        HEIGHT,
    );
    assert!(placed);
    assert!(
        count_blocks(&first, "minecraft:amethyst_block") > 0
            && count_blocks(&first, "minecraft:calcite") > 0
            && count_blocks(&first, "minecraft:smooth_basalt") > 0,
        "geode must write its inner, middle, and outer layers"
    );

    let mut replay = flat_chunk(64, "minecraft:stone");
    place_seeded_in(
        data,
        "minecraft:amethyst_geode",
        &mut replay,
        7,
        BlockPos::new(8, 24, 8),
        1234,
        MIN_Y,
        HEIGHT,
    );
    assert_eq!(
        first, replay,
        "geode must replay from the pinned level seed"
    );
}

#[test]
fn shipped_freeze_top_layer_snows_and_freezes_from_the_chunk_biome() {
    let data = FeatureData::vanilla();
    let feature = data
        .configured("minecraft:freeze_top_layer")
        .expect("freeze_top_layer");
    assert!(
        feature.is_supported(),
        "freeze_top_layer must use FeatureLevel::biome_at"
    );

    let mut snowy = flat_chunk(63, "minecraft:grass_block");
    let grass = BlockState {
        name: "minecraft:grass_block".to_owned(),
        properties: vec![("snowy".to_owned(), "false".to_owned())],
    };
    for x in 0..16 {
        for z in 0..16 {
            snowy.set_block_state(x, 63, z, grass.clone());
        }
    }
    set_biome(&mut snowy, "minecraft:snowy_plains");
    let (placed, _) = place_seeded_in(
        data,
        "minecraft:freeze_top_layer",
        &mut snowy,
        7,
        BlockPos::new(0, 64, 0),
        0,
        MIN_Y,
        HEIGHT,
    );
    assert!(placed);
    assert_eq!(
        count_blocks(&snowy, "minecraft:snow"),
        256,
        "every snowy column gets one snow layer"
    );
    for x in 0..16 {
        for z in 0..16 {
            let grass = snowy.block_state(x, 63, z).expect("grass layer at y=63");
            assert_eq!(
                grass.properties.iter().find(|(key, _)| key == "snowy"),
                Some(&("snowy".to_owned(), "true".to_owned())),
                "snowy dirt must flip its property at ({x}, {z})"
            );
        }
    }

    let mut water = flat_chunk(62, "minecraft:stone");
    water.set_block_state(8, 63, 8, block("minecraft:water"));
    set_biome(&mut water, "minecraft:frozen_ocean");
    let (placed, _) = place_seeded_in(
        data,
        "minecraft:freeze_top_layer",
        &mut water,
        7,
        BlockPos::new(0, 64, 0),
        0,
        MIN_Y,
        HEIGHT,
    );
    assert!(placed);
    assert_eq!(
        count_blocks(&water, "minecraft:ice"),
        1,
        "a lone water block below the surface freezes"
    );

    // Warm biomes must not freeze or snow.
    let mut warm = flat_chunk(63, "minecraft:grass_block");
    set_biome(&mut warm, "minecraft:desert");
    place_seeded_in(
        data,
        "minecraft:freeze_top_layer",
        &mut warm,
        7,
        BlockPos::new(0, 64, 0),
        0,
        MIN_Y,
        HEIGHT,
    );
    assert_eq!(count_blocks(&warm, "minecraft:snow"), 0);
    assert_eq!(count_blocks(&warm, "minecraft:ice"), 0);
}

#[test]
fn shipped_end_spike_uses_the_level_seed_and_queues_its_crystal() {
    let data = FeatureData::vanilla();
    assert!(
        data.configured("minecraft:end_spike")
            .expect("end_spike")
            .is_supported()
    );

    let seed = 0;
    let spike = folia_core::worldgen::end::end_spikes_for_seed(seed)
        .into_iter()
        .next()
        .expect("every seed has ten spikes");
    let chunk_x = spike.chunk_x();
    let chunk_z = spike.chunk_z();

    let mut chunk = Chunk {
        x: chunk_x,
        z: chunk_z,
        sections: Vec::new(),
    };
    let (placed, entities) = place_seeded_in(
        data,
        "minecraft:end_spike",
        &mut chunk,
        7,
        BlockPos::new(chunk_x * 16, 64, chunk_z * 16),
        seed,
        0,
        256,
    );
    assert!(placed);
    assert!(count_blocks(&chunk, "minecraft:obsidian") > 0);
    assert!(count_blocks(&chunk, "minecraft:bedrock") > 0);
    assert!(count_blocks(&chunk, "minecraft:fire") > 0);

    let crystal = entities
        .iter()
        .find(|entity| entity.entity_type == "minecraft:end_crystal")
        .expect("end crystal descriptor");
    assert_eq!(
        crystal.pos,
        (
            f64::from(spike.center_x) + 0.5,
            f64::from(spike.height) + 1.0,
            f64::from(spike.center_z) + 0.5
        )
    );

    let mut replay = Chunk {
        x: chunk_x,
        z: chunk_z,
        sections: Vec::new(),
    };
    place_seeded_in(
        data,
        "minecraft:end_spike",
        &mut replay,
        7,
        BlockPos::new(chunk_x * 16, 64, chunk_z * 16),
        seed,
        0,
        256,
    );
    assert_eq!(chunk, replay, "end_spike must replay from the pinned seed");
}

#[test]
fn shipped_root_system_places_the_embedded_azalea_tree_and_rooted_dirt() {
    let data = FeatureData::vanilla();
    assert!(
        data.configured("minecraft:rooted_azalea_tree")
            .expect("rooted_azalea_tree")
            .is_supported()
    );

    let make_chunk = || {
        let mut chunk = flat_chunk(62, "minecraft:stone");
        // An air cell under a grass shelf: the shipped placement's
        // environment_scan + y-1 offset produces exactly this shape.
        chunk.set_block_state(8, 64, 8, block("minecraft:grass_block"));
        chunk
    };
    let mut first = make_chunk();
    let (placed, _) = place_seeded_in(
        data,
        "minecraft:rooted_azalea_tree",
        &mut first,
        7,
        BlockPos::new(8, 63, 8),
        0,
        MIN_Y,
        HEIGHT,
    );
    assert!(placed);
    assert!(
        count_blocks(&first, "minecraft:oak_log") > 0,
        "the embedded azalea_tree must grow"
    );
    assert!(
        count_blocks(&first, "minecraft:azalea_leaves") > 0,
        "azalea foliage must be placed"
    );
    assert!(
        count_blocks(&first, "minecraft:rooted_dirt") > 0,
        "the support block must become rooted dirt"
    );

    let mut second = make_chunk();
    place_seeded_in(
        data,
        "minecraft:rooted_azalea_tree",
        &mut second,
        7,
        BlockPos::new(8, 63, 8),
        0,
        MIN_Y,
        HEIGHT,
    );
    assert_eq!(first, second, "root system must replay deterministically");
}

#[test]
fn unsupported_features_name_the_missing_capability() {
    let data = FeatureData::vanilla();
    assert!(
        data.configured("minecraft:fossil_coal")
            .expect("configured feature")
            .is_supported(),
        "fossils now use the shipped template catalog"
    );
    assert!(
        data.configured("minecraft:fossil_diamonds")
            .expect("configured feature")
            .is_supported()
    );
}
