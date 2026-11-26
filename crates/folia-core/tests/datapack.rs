//! Integration coverage for initial worldgen datapack loading.

use folia_core::chunk::Chunk;
use folia_core::random::LegacyRandomSource;
use folia_core::worldgen::biome::BiomeSampler;
use folia_core::worldgen::datapack::DatapackOverlay;
use folia_core::worldgen::feature::{Decoration, FeatureData, FeatureLevel, WorldGenRegion};
use folia_core::worldgen::generator::WorldGenerator;
use folia_math::BlockPos;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "folia-datapack-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_json(root: &Path, registry: &str, id: &str, value: Value) -> PathBuf {
    let (namespace, path) = id.split_once(':').expect("namespaced id");
    let file = root
        .join("datapacks")
        .join("testpack")
        .join("data")
        .join(namespace)
        .join("worldgen")
        .join(registry)
        .join(format!("{path}.json"));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, value.to_string()).unwrap();
    file
}

fn gold_configured() -> Value {
    json!({
        "type": "minecraft:simple_block",
        "config": {"to_place": {
            "type": "minecraft:simple_state_provider",
            "state": {"Name": "minecraft:gold_block"}
        }}
    })
}

fn gold_placed() -> Value {
    json!({
        "feature": "testmod:gold_block",
        "placement": [
            {"type": "minecraft:heightmap", "heightmap": "OCEAN_FLOOR"},
            {"type": "minecraft:biome"}
        ]
    })
}

fn gold_ore_configured() -> Value {
    json!({
        "type": "minecraft:ore",
        "config": {
            "size": 8,
            "discard_chance_on_air_exposure": 0.0,
            "targets": [{
                "state": {"Name": "minecraft:gold_block"},
                "target": {
                    "predicate_type": "minecraft:tag_match",
                    "tag": "minecraft:base_stone_overworld"
                }
            }]
        }
    })
}

fn gold_ore_placed() -> Value {
    json!({
        "feature": "testmod:gold_vein",
        "placement": [
            {"type": "minecraft:count", "count": 24},
            {"type": "minecraft:in_square"},
            {"type": "minecraft:height_range", "height": {
                "type": "minecraft:uniform",
                "max_inclusive": {"absolute": 64},
                "min_inclusive": {"absolute": 0}
            }}
        ]
    })
}

fn climate_json(values: [i64; 7]) -> Value {
    let pair = |value: i64| json!([value, value]);
    json!({
        "temperature": pair(values[0]),
        "humidity": pair(values[1]),
        "continentalness": pair(values[2]),
        "erosion": pair(values[3]),
        "depth": pair(values[4]),
        "weirdness": pair(values[5]),
        "offset": values[6]
    })
}

fn gold_biome(climate: Option<Value>, step: Decoration, feature: &str) -> Value {
    let mut steps: Vec<Value> = (0..11).map(|_| json!([])).collect();
    steps[step.index()] = json!([feature]);
    let mut biome = json!({"steps": steps});
    if let Some(climate) = climate {
        biome
            .as_object_mut()
            .unwrap()
            .insert("climate".to_owned(), climate);
    }
    biome
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

#[test]
fn custom_biome_and_features_merge_without_losing_vanilla_data() {
    let temp = TempRoot::new("merge");
    write_json(
        temp.path(),
        "configured_feature",
        "testmod:gold_block",
        gold_configured(),
    );
    write_json(
        temp.path(),
        "placed_feature",
        "testmod:gold_block",
        gold_placed(),
    );
    write_json(
        temp.path(),
        "biome",
        "testmod:gold_biome",
        gold_biome(None, Decoration::VegetalDecoration, "testmod:gold_block"),
    );

    let overlay = DatapackOverlay::load_root(temp.path()).unwrap();
    assert_eq!(overlay.packs(), ["testpack"]);
    assert_eq!(overlay.configured_count(), 1);
    assert_eq!(overlay.placed_count(), 1);
    assert_eq!(overlay.biome_count(), 1);

    let vanilla = FeatureData::vanilla();
    let merged = vanilla.merge(&overlay).unwrap();
    assert_eq!(merged.configured_count(), vanilla.configured_count() + 1);
    assert_eq!(merged.placed_count(), vanilla.placed_count() + 1);
    assert_eq!(merged.biome_count(), vanilla.biome_count() + 1);
    assert_eq!(merged.tag_count(), vanilla.tag_count());

    let configured = merged.configured("testmod:gold_block").unwrap();
    assert!(configured.is_supported());
    assert_eq!(configured.vanilla_type(), "minecraft:simple_block");
    let placed = merged.placed("testmod:gold_block").unwrap();
    assert_eq!(placed.feature(), "testmod:gold_block");
    assert_eq!(placed.placement_modifier_count(), 2);

    let steps = merged.biome_features("testmod:gold_biome").unwrap();
    assert!(
        steps[Decoration::VegetalDecoration.index()].contains(&"testmod:gold_block".to_owned())
    );
    assert_eq!(
        merged.biome_features("minecraft:plains"),
        vanilla.biome_features("minecraft:plains"),
        "vanilla biome features must survive the merge"
    );
    assert!(
        merged.step_feature_order()[Decoration::VegetalDecoration.index()]
            .contains(&"testmod:gold_block".to_owned())
    );
}

#[test]
fn appending_to_a_vanilla_biome_keeps_all_vanilla_features() {
    let temp = TempRoot::new("append");
    let mut steps: Vec<Value> = (0..11).map(|_| json!([])).collect();
    steps[Decoration::TopLayerModification.index()] = json!(["testmod:snow"]);
    write_json(
        temp.path(),
        "configured_feature",
        "testmod:snow",
        gold_configured(),
    );
    write_json(
        temp.path(),
        "placed_feature",
        "testmod:snow",
        json!({
            "feature": "testmod:snow",
            "placement": [{"type": "minecraft:biome"}]
        }),
    );
    write_json(
        temp.path(),
        "biome",
        "minecraft:plains",
        json!({"steps": steps}),
    );
    let overlay = DatapackOverlay::load_root(temp.path()).unwrap();
    let merged = FeatureData::vanilla().merge(&overlay).unwrap();
    let vanilla_steps = FeatureData::vanilla()
        .biome_features("minecraft:plains")
        .unwrap();
    let merged_steps = merged.biome_features("minecraft:plains").unwrap();
    let step = Decoration::TopLayerModification.index();
    assert_eq!(
        &merged_steps[step][..vanilla_steps[step].len()],
        &vanilla_steps[step][..]
    );
    assert_eq!(merged_steps[step].last().unwrap().as_str(), "testmod:snow");
}

#[test]
fn malformed_json_is_rejected_with_the_file_path() {
    let temp = TempRoot::new("malformed");
    let file = write_json(
        temp.path(),
        "configured_feature",
        "testmod:broken",
        json!({"type": 7}),
    );
    let error = DatapackOverlay::load_root(temp.path()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(file.file_name().unwrap().to_string_lossy().as_ref()),
        "error should name the file: {message}"
    );
    assert!(message.contains("type"), "{message}");
}

#[test]
fn unknown_cross_references_and_id_collisions_are_rejected() {
    let temp = TempRoot::new("refs");
    write_json(
        temp.path(),
        "placed_feature",
        "testmod:orphan",
        json!({"feature": "testmod:missing", "placement": []}),
    );
    let overlay = DatapackOverlay::load_root(temp.path()).unwrap();
    let error = FeatureData::vanilla()
        .merge(&overlay)
        .unwrap_err()
        .to_string();
    assert!(error.contains("testmod:orphan"), "{error}");
    assert!(error.contains("testmod:missing"), "{error}");

    let temp = TempRoot::new("collision");
    write_json(
        temp.path(),
        "configured_feature",
        "minecraft:ore_dirt",
        gold_configured(),
    );
    let overlay = DatapackOverlay::load_root(temp.path()).unwrap();
    let error = FeatureData::vanilla()
        .merge(&overlay)
        .unwrap_err()
        .to_string();
    assert!(error.contains("collides"), "{error}");
    assert!(error.contains("ore_dirt"), "{error}");
}

#[test]
fn custom_biome_climate_is_sampled_and_its_feature_generates_blocks() {
    let temp = TempRoot::new("generation");
    let seed = 12345;
    let base = BiomeSampler::overworld(seed).unwrap();
    // This quart is pinned by the Java biome oracle fixture.
    let (quart_x, quart_y, quart_z) = (0, -16, -4);
    assert_eq!(
        base.sample_quart(quart_x, quart_y, quart_z),
        "minecraft:lush_caves"
    );
    let values = base.climate_at(quart_x, quart_y, quart_z);

    write_json(
        temp.path(),
        "configured_feature",
        "testmod:gold_vein",
        gold_ore_configured(),
    );
    write_json(
        temp.path(),
        "placed_feature",
        "testmod:gold_vein",
        gold_ore_placed(),
    );
    write_json(
        temp.path(),
        "biome",
        "testmod:gold_biome",
        gold_biome(
            Some(climate_json(values)),
            Decoration::UndergroundOres,
            "testmod:gold_vein",
        ),
    );

    let overlay = DatapackOverlay::load_root(temp.path()).unwrap();
    assert!(overlay.climates().contains_key("testmod:gold_biome"));
    let data = FeatureData::vanilla().merge(&overlay).unwrap();
    let custom: Vec<_> = overlay
        .climates()
        .iter()
        .map(|(id, climate)| (id.clone(), *climate))
        .collect();
    let biomes = base.clone().with_custom_biomes(custom).unwrap();
    assert_eq!(
        biomes.sample_quart(quart_x, quart_y, quart_z),
        "testmod:gold_biome",
        "an exact custom climate match must win the parameter search"
    );
    let other = (0..24)
        .flat_map(|qx| (0..24).map(move |qz| (qx, qz)))
        .find(|&(qx, qz)| base.climate_at(qx, quart_y, qz) != values)
        .map(|(qx, qz)| (qx, quart_y, qz))
        .expect("a vanilla quart must differ from the custom climate");
    assert_eq!(
        biomes.sample_quart(other.0, other.1, other.2),
        base.sample_quart(other.0, other.1, other.2),
        "vanilla samples outside the custom climate must be unchanged"
    );

    let generator = WorldGenerator::overworld(seed)
        .unwrap()
        .with_feature_data(data)
        .with_biome_sampler(biomes);
    assert!(
        generator
            .feature_data()
            .configured("testmod:gold_vein")
            .is_some()
    );
    assert_eq!(
        generator.biome_at(0, quart_y * 4, quart_z * 4),
        "testmod:gold_biome"
    );

    let chunk = generator.generate_chunk(0, -1).unwrap();
    assert!(
        count_blocks(&chunk, "minecraft:gold_block") > 0,
        "the custom placed feature must run during world generation"
    );
}

#[test]
fn custom_feature_places_blocks_through_a_datapack_placed_feature() {
    let temp = TempRoot::new("place");
    write_json(
        temp.path(),
        "configured_feature",
        "testmod:gold_block",
        gold_configured(),
    );
    write_json(
        temp.path(),
        "placed_feature",
        "testmod:gold_block",
        gold_placed(),
    );
    write_json(
        temp.path(),
        "biome",
        "testmod:gold_biome",
        gold_biome(None, Decoration::VegetalDecoration, "testmod:gold_block"),
    );
    let data = FeatureData::vanilla()
        .merge(&DatapackOverlay::load_root(temp.path()).unwrap())
        .unwrap();

    let placed = data.placed("testmod:gold_block").unwrap();
    struct World;
    impl folia_core::worldgen::feature::PlacementWorld for World {
        fn biome_at(&self, _pos: BlockPos) -> &str {
            "testmod:gold_biome"
        }

        fn biome_has_placed_feature(&self, biome: &str, feature: &str) -> bool {
            biome == "testmod:gold_biome" && feature == "testmod:gold_block"
        }

        fn heightmap(
            &self,
            _kind: folia_core::worldgen::feature::Heightmap,
            _x: i32,
            _z: i32,
        ) -> Option<i32> {
            Some(63)
        }
    }
    let positions = placed
        .placement_positions(
            BlockPos::new(8, 63, 8),
            &mut LegacyRandomSource::new(7),
            &World,
            MIN_Y,
            HEIGHT,
        )
        .unwrap();
    assert_eq!(
        positions.len(),
        1,
        "heightmap + biome modifiers yield one position"
    );

    let mut chunk = Chunk {
        light_present: false,
        x: 0,
        z: 0,
        sections: Vec::new(),
    };
    let dirt = folia_core::chunk::BlockState {
        name: "minecraft:dirt".to_owned(),
        properties: Vec::new(),
    };
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..63 {
                chunk.set_block_state_ref(x, y, z, &dirt);
            }
        }
    }
    let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
    let configured = data.configured("testmod:gold_block").unwrap();
    assert!(
        configured
            .place(&mut level, &mut LegacyRandomSource::new(7), positions[0])
            .unwrap()
    );
    assert!(
        level
            .get_block(positions[0])
            .is_some_and(|state| state.name == "minecraft:gold_block")
    );
}
