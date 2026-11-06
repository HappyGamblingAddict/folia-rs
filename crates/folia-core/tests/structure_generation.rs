//! End-to-end structure wiring tests against a generated Overworld chunk.
//!
//! The igloo NBT fixtures are shared with `structure_starts.rs` so the shipped
//! templates stay byte-for-byte pinned in one place. No entities are spawned
//! and no chest block entities are created yet; those outputs are explicit
//! placement results rather than world state.

use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, WorldGenRegion};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureGenerator;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::IglooStructure;
use folia_core::worldgen::structure::template::StructureTemplate;
use std::collections::HashMap;

const STARTS_SOURCE: &str = include_str!("structure_starts.rs");

fn embedded_hex(name: &str) -> String {
    let marker = format!("const {name}: &str = \"");
    let start = STARTS_SOURCE
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {name} in structure_starts.rs"))
        + marker.len();
    let end = STARTS_SOURCE[start..]
        .find('"')
        .expect("unterminated test constant")
        + start;
    STARTS_SOURCE[start..end].to_owned()
}

fn hex(source: &str) -> Vec<u8> {
    assert!(source.len().is_multiple_of(2));
    (0..source.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&source[index..index + 2], 16).expect("valid hex"))
        .collect()
}

fn igloo_templates() -> HashMap<String, StructureTemplate> {
    let mut templates = HashMap::new();
    for (name, constant) in [
        (IglooStructure::TOP_TEMPLATE, "IGLOO_TOP_NBT"),
        (IglooStructure::MIDDLE_TEMPLATE, "IGLOO_MIDDLE_NBT"),
        (IglooStructure::BOTTOM_TEMPLATE, "IGLOO_BOTTOM_NBT"),
    ] {
        templates.insert(
            name.to_owned(),
            StructureTemplate::from_gzip(&hex(&embedded_hex(constant)))
                .expect("igloo template parses"),
        );
    }
    templates
}

struct StaticBiomes(&'static str);

impl StructureBiomeLookup for StaticBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        self.0
    }
}

struct GenBiomes<'a> {
    generator: &'a WorldGenerator,
}

impl StructureBiomeLookup for GenBiomes<'_> {
    fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.generator.biome_at(x, y, z)
    }
}

fn structured_generator(seed: i64) -> WorldGenerator {
    let structures = StructureGenerator::shipped(igloo_templates())
        .expect("structure manager")
        .with_enabled_structures(["minecraft:igloo"]);
    WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures)
}

fn find_igloo_chunk(generator: &WorldGenerator, seed: i64) -> (i32, i32, String) {
    let biomes = GenBiomes { generator };
    let structures = generator.structures().expect("structures");
    for x in -64..=64 {
        for z in -64..=64 {
            if let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
                && selection.structure == "minecraft:igloo"
            {
                let biome = generator.biome_at((x << 4) + 8, 63, (z << 4) + 8);
                return (x, z, biome.to_owned());
            }
        }
    }
    panic!("no snow-biome igloo chunk near origin");
}

#[test]
fn generated_chunk_contains_igloo_blocks_at_pinned_positions() {
    let seed = 12_345;
    let generator = structured_generator(seed);
    let (x, z, biome) = find_igloo_chunk(&generator, seed);
    assert_eq!(biome, "minecraft:snowy_taiga");

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk(x, z)
        .expect("plain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");

    let mut differences = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                if plain.block_state(local_x, y, local_z)
                    != structured.block_state(local_x, y, local_z)
                {
                    differences += 1;
                }
            }
        }
    }
    assert!(
        differences > 0,
        "structure pass did not change chunk {x},{z}"
    );

    // The igloo top template places its furniture/lab markers at these
    // positions for seed 12345 and chunk (36, 41). They are stable because the
    // structure RNG is `setLargeFeatureSeed(seed, chunkX, chunkZ)`.
    assert_eq!(
        structured
            .block_state(1, 86, 4)
            .expect("pinned redstone torch")
            .name,
        "minecraft:redstone_torch"
    );
    assert_eq!(
        structured
            .block_state(1, 86, 5)
            .expect("pinned crafting table")
            .name,
        "minecraft:crafting_table"
    );
    assert_eq!(
        structured
            .block_state(2, 86, 3)
            .expect("pinned white carpet")
            .name,
        "minecraft:white_carpet"
    );
}

#[test]
fn template_structure_wiring_places_blocks_in_a_generated_chunk() {
    let seed = 12_345;
    let generator = structured_generator(seed);
    let (x, z, _) = find_igloo_chunk(&generator, seed);
    let structures = generator.structures().expect("structures");

    // Generate terrain/surface/carvers without decoration, then run the
    // structure pass directly. This isolates the wiring from later features
    // and pins both the placement count and a concrete block.
    let mut chunk = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let biomes = GenBiomes {
        generator: &generator,
    };
    let mut region = WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());
    let placement = structures
        .place_for_chunk(
            &mut region,
            ChunkPos::new(x, z),
            seed,
            StructureDimension::Overworld,
            63,
            &biomes,
            8,
            Decoration::SurfaceStructures,
        )
        .expect("structure pass");
    assert!(placement.generated_starts > 0);
    assert!(placement.placed_pieces > 0);
    assert!(placement.written > 0);
    assert_eq!(
        chunk.block_state(1, 86, 4).expect("torch").name,
        "minecraft:redstone_torch"
    );
    assert_eq!(
        chunk.block_state(1, 86, 5).expect("table").name,
        "minecraft:crafting_table"
    );
}

#[test]
fn structure_biome_filter_rejects_a_mismatched_set() {
    let registry = StructureSetRegistry::shipped().expect("sets");
    let desert = StaticBiomes("minecraft:desert");
    let snowy = StaticBiomes("minecraft:snowy_taiga");
    let (chunk_x, chunk_z) = (36, 41);

    let rejected = registry
        .structure_at_chunk_filtered_matching(
            chunk_x,
            chunk_z,
            12_345,
            StructureDimension::Overworld,
            63,
            &desert,
            |structure| structure == "minecraft:igloo",
        )
        .expect("selection");
    assert!(rejected.is_none(), "igloo set must reject a desert biome");

    let accepted = registry
        .structure_at_chunk_filtered_matching(
            chunk_x,
            chunk_z,
            12_345,
            StructureDimension::Overworld,
            63,
            &snowy,
            |structure| structure == "minecraft:igloo",
        )
        .expect("selection")
        .expect("snowy igloo chunk");
    assert_eq!(accepted.structure, "minecraft:igloo");
    assert_eq!(accepted.set, "minecraft:igloos");
}

#[test]
fn structure_biome_filter_rejects_nether_and_end_sets_in_the_overworld() {
    let registry = StructureSetRegistry::from_json_documents([
        (
            "nether_complexes",
            r#"{ "placement": { "type": "minecraft:random_spread", "salt": 1, "spacing": 1, "separation": 0 }, "structures": [ { "structure": "minecraft:fortress", "weight": 1 } ] }"#,
        ),
        (
            "end_cities",
            r#"{ "placement": { "type": "minecraft:random_spread", "salt": 2, "spacing": 1, "separation": 0 }, "structures": [ { "structure": "minecraft:end_city", "weight": 1 } ] }"#,
        ),
    ])
    .expect("synthetic dimension sets");
    let biomes = StaticBiomes("minecraft:nether_wastes");
    for dimension in [StructureDimension::Overworld, StructureDimension::Nether] {
        let selection = registry
            .structure_at_chunk_filtered_matching(0, 0, 12_345, dimension, 63, &biomes, |_| true)
            .expect("selection");
        match dimension {
            StructureDimension::Overworld => assert!(
                selection.is_none(),
                "Overworld must not select Nether/End sets"
            ),
            StructureDimension::Nether => {
                assert_eq!(
                    selection.expect("nether selection").structure,
                    "minecraft:fortress"
                );
            }
            StructureDimension::End => {}
        }
    }
}
