//! End-to-end structure wiring tests against a generated Overworld chunk.
//!
//! The igloo NBT fixtures are shared with `structure_starts.rs` so the shipped
//! templates stay byte-for-byte pinned in one place. No entities are spawned
//! and no chest block entities are created yet; those outputs are explicit
//! placement results rather than world state.

use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap, WorldGenRegion};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureGenerator;
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::ruined_portal::RuinedPortalPlacement;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{IglooStructure, StructureGenerationContext};
use folia_core::worldgen::structure::template::{Rotation, StructureTemplate};
use folia_random::{LegacyRandomSource, RandomSource};
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

struct FixedHeightSource;

impl FeatureLevel for FixedHeightSource {
    fn min_y(&self) -> i32 {
        -64
    }

    fn height(&self) -> i32 {
        384
    }

    fn get_block(&self, _pos: BlockPos) -> Option<&BlockState> {
        None
    }

    fn set_block(&mut self, _pos: BlockPos, _state: BlockState) -> bool {
        false
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        Some(64)
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

fn find_igloo_lab_chunk(generator: &WorldGenerator, seed: i64) -> (i32, i32, String) {
    let biomes = GenBiomes { generator };
    let structures = generator.structures().expect("structures");
    for x in -128..=128 {
        for z in -128..=128 {
            if let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
                && selection.structure == "minecraft:igloo"
            {
                let definition = structures
                    .definitions()
                    .get("minecraft:igloo")
                    .expect("igloo definition");
                let context = StructureGenerationContext::new(
                    seed,
                    ChunkPos::new(x, z),
                    0,
                    structures.templates(),
                );
                let start = definition.generate_start(&context).expect("igloo start");
                if start.pieces().iter().any(|piece| {
                    piece.as_template().is_some_and(|template| {
                        template.template_name == IglooStructure::BOTTOM_TEMPLATE
                    })
                }) {
                    let biome = generator.biome_at((x << 4) + 8, 63, (z << 4) + 8);
                    return (x, z, biome.to_owned());
                }
            }
        }
    }
    panic!("no snowy-biome igloo laboratory near origin");
}

fn igloo_chest_marker_position(
    structures: &StructureGenerator,
    seed: i64,
    chunk: ChunkPos,
) -> BlockPos {
    let definition = structures
        .definitions()
        .get("minecraft:igloo")
        .expect("igloo definition");
    let context = StructureGenerationContext::new(seed, chunk, 0, structures.templates());
    let start = definition.generate_start(&context).expect("igloo start");
    let bottom = start
        .pieces()
        .iter()
        .find_map(|piece| {
            let template = piece.as_template()?;
            (template.template_name == IglooStructure::BOTTOM_TEMPLATE).then_some(template)
        })
        .expect("igloo laboratory template");
    let marker = bottom
        .template
        .blocks()
        .iter()
        .find(|block| {
            block
                .nbt
                .as_ref()
                .and_then(|nbt| nbt.get("metadata"))
                .and_then(folia_core::nbt::Tag::as_str)
                == Some("chest")
        })
        .expect("igloo chest marker");
    let offset = folia_core::worldgen::structure::template::transform(
        marker.pos,
        bottom.place_settings.mirror(),
        bottom.place_settings.rotation(),
        bottom.place_settings.rotation_pivot(),
    );
    bottom.template_position.offset_by(offset)
}

fn find_buried_treasure_chunk(generator: &WorldGenerator, seed: i64) -> (i32, i32, String) {
    let biomes = GenBiomes { generator };
    let structures = generator.structures().expect("structures");
    for x in -64..=64 {
        for z in -64..=64 {
            if let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
                && selection.structure == "minecraft:buried_treasure"
            {
                let biome = generator.biome_at((x << 4) + 8, 63, (z << 4) + 8);
                return (x, z, biome.to_owned());
            }
        }
    }
    panic!("no beach buried-treasure chunk near origin");
}

#[test]
fn generated_chunk_contains_buried_treasure_from_underground_structure_step() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure manager")
        .with_enabled_structures(["minecraft:buried_treasure"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let (x, z, biome) = find_buried_treasure_chunk(&generator, seed);
    assert!(matches!(
        biome.as_str(),
        "minecraft:beach" | "minecraft:snowy_beach"
    ));

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut chests = Vec::new();
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                if structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    == "minecraft:chest"
                {
                    chests.push((local_x, y, local_z));
                }
            }
        }
    }
    assert_eq!(chests.len(), 1, "buried treasure at chunk {x},{z}");
    assert_eq!((chests[0].0, chests[0].2), (9, 9));
    assert!(chests[0].1 < 90);
    assert!(
        structured
            .block_state(9, chests[0].1 + 1, 9)
            .expect("block above chest")
            .name
            != "minecraft:air"
    );
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        plain
            .block_state(local_x, chests[0].1, local_z)
            .expect("plain block")
            .name
            != "minecraft:chest"
    })));
}

#[test]
fn generated_chunk_contains_witch_hut_planks_stilts_and_cauldron() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure manager")
        .with_enabled_structures(["minecraft:swamp_hut"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let (x, z) = (-149, -87);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let selection = generator
        .structures()
        .expect("structures")
        .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
        .expect("selection")
        .expect("witch hut placement chunk");
    assert_eq!(selection.structure, "minecraft:swamp_hut");
    assert_eq!(selection.set, "minecraft:swamp_huts");
    assert_eq!(
        generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
        "minecraft:swamp"
    );

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut cauldrons = Vec::new();
    let mut planks = 0;
    let mut logs = 0;
    let mut stairs = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:cauldron" => cauldrons.push((local_x, y, local_z)),
                    "minecraft:spruce_planks" => planks += 1,
                    "minecraft:spruce_log" => logs += 1,
                    "minecraft:spruce_stairs" => stairs += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(cauldrons.len(), 1, "witch hut at chunk {x},{z}");
    assert_eq!((cauldrons[0].0, cauldrons[0].2), (2, 2));
    assert!(planks >= 25);
    assert!(logs >= 12);
    assert!(stairs >= 15);
    let floor_y = cauldrons[0].1 - 1;
    let support_logs = (generator.min_y()..floor_y)
        .filter(|&y| {
            structured
                .block_state(0, y, 0)
                .expect("support column")
                .name
                == "minecraft:spruce_log"
        })
        .count();
    assert!(support_logs >= 1, "the hut floor must sit on spruce stilts");
    assert_eq!(
        structured
            .block_state(2, floor_y, 2)
            .expect("hut floor")
            .name,
        "minecraft:spruce_planks"
    );
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        plain
            .block_state(local_x, floor_y, local_z)
            .expect("plain block")
            .name
            != "minecraft:cauldron"
    })));
}

#[test]
fn generated_jungle_chunk_contains_temple_chests_dispensers_and_stairs() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure manager")
        .with_enabled_structures(["minecraft:jungle_pyramid"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let (x, z) = (-96, 37);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let selection = generator
        .structures()
        .expect("structures")
        .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
        .expect("selection")
        .expect("jungle pyramid placement chunk");
    assert_eq!(selection.structure, "minecraft:jungle_pyramid");
    assert_eq!(selection.set, "minecraft:jungle_temples");
    assert_eq!(
        generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
        "minecraft:jungle"
    );

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut chests = Vec::new();
    let mut dispensers = Vec::new();
    let mut mossy = 0;
    let mut stairs = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:chest" => chests.push((local_x, y, local_z)),
                    "minecraft:dispenser" => dispensers.push((local_x, y, local_z)),
                    "minecraft:mossy_cobblestone" => mossy += 1,
                    "minecraft:cobblestone_stairs" => stairs += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(chests.len(), 2, "jungle pyramid at chunk {x},{z}");
    assert_eq!(dispensers.len(), 2, "jungle pyramid at chunk {x},{z}");
    assert!(mossy > 0);
    assert!(stairs >= 8);
    assert!(chests.iter().all(|(_, y, _)| *y > 0));
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        plain
            .block_state(local_x, chests[0].1, local_z)
            .expect("plain block")
            .name
            != "minecraft:chest"
    })));
}

#[test]
fn generated_snowy_chunk_contains_igloo_surface_and_laboratory() {
    let seed = 12_345;
    let generator = structured_generator(seed);
    let (x, z, biome) = find_igloo_lab_chunk(&generator, seed);
    assert_eq!((x, z), (20, 72));
    assert_eq!(biome, "minecraft:snowy_taiga");

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk(x, z)
        .expect("plain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");

    let mut differences = 0;
    let mut torches = Vec::new();
    let mut crafting_tables = Vec::new();
    let mut white_carpets = Vec::new();
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                if plain.block_state(local_x, y, local_z)
                    != structured.block_state(local_x, y, local_z)
                {
                    differences += 1;
                }
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:redstone_torch" => torches.push((local_x, y, local_z)),
                    "minecraft:crafting_table" => crafting_tables.push((local_x, y, local_z)),
                    "minecraft:white_carpet" => white_carpets.push((local_x, y, local_z)),
                    _ => {}
                }
            }
        }
    }
    assert!(
        differences > 0,
        "structure pass did not change chunk {x},{z}"
    );
    assert_eq!(torches, vec![(1, 75, 4)]);
    assert_eq!(crafting_tables, vec![(1, 75, 5)]);
    assert_eq!(
        white_carpets,
        vec![
            (2, 75, 3),
            (2, 75, 4),
            (2, 75, 5),
            (3, 75, 3),
            (3, 75, 4),
            (3, 75, 5),
            (4, 75, 3),
            (4, 75, 4),
            (4, 75, 5),
        ]
    );

    let marker = igloo_chest_marker_position(
        generator.structures().expect("structures"),
        seed,
        ChunkPos::new(x, z),
    );
    let lab_chunk = ChunkPos::new(marker.x() >> 4, marker.z() >> 4);
    assert!(matches!(
        generator.biome_at((lab_chunk.x << 4) + 8, 63, (lab_chunk.z << 4) + 8),
        "minecraft:snowy_plains" | "minecraft:snowy_slopes" | "minecraft:snowy_taiga"
    ));
    let plain_lab = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk(lab_chunk.x, lab_chunk.z)
        .expect("plain laboratory chunk");
    let structured_lab = generator
        .generate_chunk(lab_chunk.x, lab_chunk.z)
        .expect("structured laboratory chunk");
    let mut chests = Vec::new();
    let mut brewing_stands = 0;
    let mut water_cauldrons = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured_lab
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:chest" => chests.push((local_x, y, local_z)),
                    "minecraft:brewing_stand" => brewing_stands += 1,
                    "minecraft:water_cauldron" => water_cauldrons += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(chests.len(), 1, "igloo chest in chunk {lab_chunk:?}");
    assert_eq!(
        (chests[0].0, chests[0].2),
        ((marker.x() & 15) as usize, (marker.z() & 15) as usize)
    );
    assert!(chests[0].1 < 63);
    assert_eq!(brewing_stands, 1);
    assert_eq!(water_cauldrons, 1);
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
            plain_lab
                .block_state(local_x, y, local_z)
                .expect("plain laboratory block")
                .name
                != "minecraft:chest"
        })
    })));
}

#[test]
fn igloo_template_wiring_uses_the_height_source_for_the_laboratory_chest() {
    let seed = 12_345;
    let generator = structured_generator(seed);
    let (x, z, _) = find_igloo_lab_chunk(&generator, seed);
    let structures = generator.structures().expect("structures");
    let marker = igloo_chest_marker_position(structures, seed, ChunkPos::new(x, z));
    let lab_chunk = ChunkPos::new(marker.x() >> 4, marker.z() >> 4);
    let marker_y = marker.y() + 64 - IglooStructure::GENERATION_HEIGHT - 1;
    let marker_pos = BlockPos::new(marker.x(), marker_y, marker.z());
    let expected_seed =
        LegacyRandomSource::new((i64::from(lab_chunk.x) << 32) ^ i64::from(lab_chunk.z))
            .next_long();

    // Generate terrain/surface/carvers without decoration, then run the
    // structure pass directly. This isolates the wiring from later features
    // and pins both the placement count and a concrete block.
    let mut chunk = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(lab_chunk.x, lab_chunk.z, &[])
        .expect("terrain chunk");
    let biomes = GenBiomes {
        generator: &generator,
    };
    let mut region = WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut region,
            &FixedHeightSource,
            lab_chunk,
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
    assert_eq!(placement.data_markers.len(), 1);
    assert_eq!(placement.data_markers[0].name, "chest");
    assert_eq!(placement.data_markers[0].pos, marker_pos);
    let chest = placement
        .block_entities
        .iter()
        .find(|block| block.pos == marker_pos.below())
        .expect("igloo loot chest");
    assert!(chest.nbt.as_ref().is_some_and(|nbt| {
        nbt.get("id").and_then(folia_core::nbt::Tag::as_str) == Some("minecraft:chest")
            && nbt.get("LootTable").and_then(folia_core::nbt::Tag::as_str)
                == Some("minecraft:chests/igloo_chest")
            && nbt
                .get("LootTableSeed")
                .and_then(folia_core::nbt::Tag::as_long)
                == Some(expected_seed)
    }));
    assert_eq!(
        chunk
            .block_state(
                (marker_pos.x() & 15) as usize,
                marker_pos.y(),
                (marker_pos.z() & 15) as usize,
            )
            .expect("cleared chest marker")
            .name,
        "minecraft:air"
    );
    assert_eq!(
        chunk
            .block_state(
                (marker_pos.x() & 15) as usize,
                marker_pos.y() - 1,
                (marker_pos.z() & 15) as usize,
            )
            .expect("laboratory chest")
            .name,
        "minecraft:chest"
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
fn generated_ocean_chunk_contains_shipwreck_wood_and_chest() {
    let seed = 12_345;
    let structures = StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("shipwreck generator")
        .with_enabled_structures(["minecraft:shipwreck"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structures");
    let mut found = None;
    for x in -96..=96 {
        for z in -96..=96 {
            let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
            else {
                continue;
            };
            if selection.structure != "minecraft:shipwreck"
                || !matches!(
                    generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
                    "minecraft:cold_ocean"
                        | "minecraft:deep_cold_ocean"
                        | "minecraft:deep_frozen_ocean"
                        | "minecraft:deep_lukewarm_ocean"
                        | "minecraft:deep_ocean"
                        | "minecraft:frozen_ocean"
                        | "minecraft:lukewarm_ocean"
                        | "minecraft:ocean"
                        | "minecraft:warm_ocean"
                )
            {
                continue;
            }
            let definition = structures
                .definitions()
                .get("minecraft:shipwreck")
                .expect("shipwreck definition");
            let context = StructureGenerationContext::new(
                seed,
                ChunkPos::new(x, z),
                0,
                structures.template_catalog().expect("template catalog"),
            );
            let start = definition
                .generate_start(&context)
                .expect("shipwreck start");
            if start
                .shipwreck()
                .expect("shipwreck piece")
                .template()
                .place_settings
                .rotation()
                == Rotation::None
            {
                found = Some((x, z));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (x, z) = found.expect("unrotated shipwreck in a generated ocean biome");
    let selection = structures
        .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
        .expect("selection")
        .expect("shipwreck placement chunk");
    assert_eq!(selection.structure, "minecraft:shipwreck");
    assert_eq!(selection.set, "minecraft:shipwrecks");
    assert!(matches!(
        generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
        "minecraft:cold_ocean"
            | "minecraft:deep_cold_ocean"
            | "minecraft:deep_frozen_ocean"
            | "minecraft:deep_lukewarm_ocean"
            | "minecraft:deep_ocean"
            | "minecraft:frozen_ocean"
            | "minecraft:lukewarm_ocean"
            | "minecraft:ocean"
            | "minecraft:warm_ocean"
    ));

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut planks = 0;
    let mut stairs = 0;
    let mut chests = Vec::new();
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:oak_planks" => planks += 1,
                    "minecraft:spruce_stairs" => stairs += 1,
                    "minecraft:chest" => chests.push((local_x, y, local_z)),
                    _ => {}
                }
            }
        }
    }
    assert!(planks > 20, "shipwreck planks in chunk {x},{z}");
    assert!(stairs > 5, "shipwreck stairs in chunk {x},{z}");
    assert_eq!(chests.len(), 1, "shipwreck chest in chunk {x},{z}");
    assert!(chests[0].1 < 63, "shipwreck must sit on the ocean floor");
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        plain
            .block_state(local_x, chests[0].1, local_z)
            .expect("plain block")
            .name
            != "minecraft:chest"
    })));
}

#[test]
fn generated_ocean_chunk_contains_cold_ruin_materials_and_loot_chest() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("ocean ruin generator")
        .with_enabled_structures(["minecraft:ocean_ruin_cold"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structures");
    let mut found = None;
    for x in -96..=96 {
        for z in -96..=96 {
            let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
            else {
                continue;
            };
            if selection.structure == "minecraft:ocean_ruin_cold"
                && matches!(
                    generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
                    "minecraft:cold_ocean"
                        | "minecraft:deep_cold_ocean"
                        | "minecraft:deep_frozen_ocean"
                        | "minecraft:deep_ocean"
                        | "minecraft:frozen_ocean"
                        | "minecraft:ocean"
                )
            {
                found = Some((x, z));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (x, z) = found.expect("cold ocean ruin in a generated ocean biome");
    let selection = structures
        .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
        .expect("selection")
        .expect("ocean ruin placement chunk");
    assert_eq!(selection.structure, "minecraft:ocean_ruin_cold");
    assert_eq!(selection.set, "minecraft:ocean_ruins");

    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("terrain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut stone_bricks = 0;
    let mut cobblestone = 0;
    let mut andesite = 0;
    let mut prismarine = 0;
    let mut chests = Vec::new();
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("block")
                    .name
                    .as_str()
                {
                    "minecraft:stone_bricks" => stone_bricks += 1,
                    "minecraft:cobblestone" => cobblestone += 1,
                    "minecraft:andesite" => andesite += 1,
                    "minecraft:prismarine" => prismarine += 1,
                    "minecraft:chest" => chests.push((local_x, y, local_z)),
                    _ => {}
                }
            }
        }
    }
    assert!(stone_bricks > 0, "stone bricks in ruin chunk {x},{z}");
    assert!(cobblestone > 0, "cobblestone in ruin chunk {x},{z}");
    assert!(andesite > 0, "andesite in ruin chunk {x},{z}");
    assert!(prismarine > 0, "prismarine in ruin chunk {x},{z}");
    assert!(
        (1..=2).contains(&chests.len()),
        "ruin chests in chunk {x},{z}"
    );
    assert!(chests.iter().all(|(_, y, _)| *y < 63));
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
            plain
                .block_state(local_x, y, local_z)
                .expect("plain block")
                .name
                != "minecraft:chest"
        })
    })));
}

#[test]
fn generated_deep_ocean_chunk_contains_monument_core_and_sponge_room() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("ocean monument generator")
        .with_enabled_structures(["minecraft:monument"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structures");
    let (x, z) = (-247, -46);
    let selection = structures
        .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
        .expect("selection")
        .expect("monument placement chunk");
    assert_eq!(selection.structure, "minecraft:monument");
    assert_eq!(selection.set, "minecraft:ocean_monuments");
    assert_eq!(
        generator.biome_at((x << 4) + 8, 63, (z << 4) + 8),
        "minecraft:deep_ocean"
    );

    let plain_generator = WorldGenerator::overworld(seed).expect("plain generator");
    let mut plain_chunks = Vec::new();
    let mut structured_chunks = Vec::new();
    for dx in 0..2 {
        for dz in 0..2 {
            plain_chunks.push(
                plain_generator
                    .generate_chunk_with_steps(x + dx, z + dz, &[])
                    .expect("terrain chunk"),
            );
            structured_chunks.push(
                generator
                    .generate_chunk(x + dx, z + dz)
                    .expect("structured chunk"),
            );
        }
    }
    let mut prismarine = 0;
    let mut gold = 0;
    let mut sea_lanterns = 0;
    let mut sponges = 0;
    for chunk in &structured_chunks {
        for local_x in 0..16 {
            for local_z in 0..16 {
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    match chunk
                        .block_state(local_x, y, local_z)
                        .expect("block")
                        .name
                        .as_str()
                    {
                        "minecraft:prismarine"
                        | "minecraft:prismarine_bricks"
                        | "minecraft:dark_prismarine" => prismarine += 1,
                        "minecraft:gold_block" => gold += 1,
                        "minecraft:sea_lantern" => sea_lanterns += 1,
                        "minecraft:sponge" | "minecraft:wet_sponge" => sponges += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    assert!(prismarine > 50, "monument prismarine near chunk {x},{z}");
    assert_eq!(gold, 1, "monument guardian gold near chunk {x},{z}");
    assert_eq!(sea_lanterns, 4, "monument sea lanterns near chunk {x},{z}");
    assert!(sponges > 0, "monument sponge room near chunk {x},{z}");
    assert!(plain_chunks.iter().all(|chunk| {
        (0..16).all(|local_x| {
            (0..16).all(|local_z| {
                (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
                    chunk
                        .block_state(local_x, y, local_z)
                        .expect("plain block")
                        .name
                        != "minecraft:gold_block"
                })
            })
        })
    }));
}

#[test]
fn generated_chunk_contains_standard_ruined_portal_obsidian_and_lava() {
    let seed = 12_345;
    let structures = StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("ruined portal generator")
        .with_enabled_structures(["minecraft:ruined_portal"]);
    let generator = WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structures");
    let definition = structures
        .definitions()
        .get("minecraft:ruined_portal")
        .expect("standard portal definition");
    let catalog = structures.template_catalog().expect("template catalog");
    let mut found = None;
    for x in -64..=64 {
        for z in -64..=64 {
            let Some(selection) = structures
                .structure_at_chunk(x, z, seed, StructureDimension::Overworld, 63, &biomes)
                .expect("selection")
            else {
                continue;
            };
            if selection.structure != "minecraft:ruined_portal" {
                continue;
            }
            let context = StructureGenerationContext::new(seed, ChunkPos::new(x, z), 0, catalog)
                .with_level(&FixedHeightSource);
            let start = definition.generate_start(&context).expect("portal start");
            let portal = start.ruined_portal().expect("portal piece");
            if portal.placement() != RuinedPortalPlacement::OnLandSurface {
                continue;
            }
            let center = portal.bounding_box().center();
            if center.x() >> 4 == x && center.z() >> 4 == z {
                found = Some((x, z));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (x, z) = found.expect("centered standard ruined portal near origin");
    let plain = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk(x, z)
        .expect("plain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut obsidian = 0;
    let mut lava = 0;
    let mut plain_lava = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("structured block")
                    .name
                    .as_str()
                {
                    "minecraft:obsidian" => obsidian += 1,
                    "minecraft:lava" if y > 0 => lava += 1,
                    _ => {}
                }
                if y > 0
                    && plain
                        .block_state(local_x, y, local_z)
                        .expect("plain block")
                        .name
                        == "minecraft:lava"
                {
                    plain_lava += 1;
                }
            }
        }
    }
    assert!(obsidian > 0, "ruined portal obsidian in chunk {x},{z}");
    assert!(lava > 0, "ruined portal lava in chunk {x},{z}");
    assert_eq!(plain_lava, 0);
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
