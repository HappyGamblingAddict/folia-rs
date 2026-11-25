use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::{PoolCatalog, PoolElement, ProcessorListRef};
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructurePlacement,
};
use folia_core::worldgen::structure::start::{
    StructureDefinition, StructureGenerationContext, StructureGenerator, StructureKind,
    TerrainAdjustment,
};
use std::collections::HashMap;

const SEED: i64 = 12_345;

#[derive(Default)]
struct MockLevel {
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new() -> Self {
        Self::default()
    }
}

impl FeatureLevel for MockLevel {
    fn min_y(&self) -> i32 {
        -64
    }

    fn height(&self) -> i32 {
        384
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !(-64..320).contains(&pos.y()) {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        Some(0)
    }
}

struct StaticBiomes(&'static str);

impl StructureBiomeLookup for StaticBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        self.0
    }
}

struct StartHeightBiomes;

impl StructureBiomeLookup for StartHeightBiomes {
    fn biome_at(&self, _x: i32, y: i32, _z: i32) -> &str {
        if y == -27 {
            "minecraft:deep_dark"
        } else {
            "minecraft:plains"
        }
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

fn generator() -> StructureGenerator {
    StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("ancient city structure generator")
        .with_enabled_structures(["minecraft:ancient_city"])
}

fn definition() -> StructureDefinition {
    generator()
        .definitions()
        .get("minecraft:ancient_city")
        .expect("ancient city definition")
        .clone()
}

fn check_element_templates(
    element: &PoolElement,
    catalog: &StructureTemplateCatalog,
    processors: &StructureProcessorCatalog,
    missing: &mut Vec<String>,
) {
    match element {
        PoolElement::Single {
            template,
            processors: list,
            ..
        }
        | PoolElement::LegacySingle {
            template,
            processors: list,
            ..
        } => {
            if !catalog.contains(template) {
                missing.push(template.clone());
            }
            if let ProcessorListRef::Named(name) = list {
                assert!(
                    processors.list(name).is_some(),
                    "missing processor list {name}"
                );
            }
        }
        PoolElement::List { elements, .. } => {
            for child in elements {
                check_element_templates(child, catalog, processors, missing);
            }
        }
        PoolElement::Empty | PoolElement::Feature { .. } => {}
    }
}

#[test]
fn ancient_city_definition_uses_the_shipped_jigsaw_settings() {
    let definition = definition();
    assert_eq!(definition.name, "minecraft:ancient_city");
    assert_eq!(definition.step(), Decoration::UndergroundStructures);
    assert_eq!(
        definition.terrain_adaptation,
        TerrainAdjustment::Encapsulate
    );
    let StructureKind::Jigsaw {
        start_pool,
        start_jigsaw_name,
        size,
        start_height,
        use_expansion_hack,
        project_start_to_heightmap,
        max_distance_horizontal,
        max_distance_vertical,
        dimension_padding_bottom,
        dimension_padding_top,
        ..
    } = &definition.kind
    else {
        panic!("ancient city must use the jigsaw structure kind");
    };
    assert_eq!(start_pool, "minecraft:ancient_city/city_center");
    assert_eq!(start_jigsaw_name.as_deref(), Some("minecraft:city_anchor"));
    assert_eq!(*size, 1);
    assert_eq!(*start_height, -27);
    assert!(!*use_expansion_hack);
    assert_eq!(*project_start_to_heightmap, None);
    assert_eq!(
        (*max_distance_horizontal, *max_distance_vertical),
        (80, 4064)
    );
    assert_eq!((*dimension_padding_bottom, *dimension_padding_top), (1, 1));
}

#[test]
fn ancient_city_catalogs_resolve_processors_and_record_the_missing_template() {
    let catalog = StructureTemplateCatalog::shipped();
    let pools = PoolCatalog::shipped();
    let processors = StructureProcessorCatalog::shipped();
    let ancient_templates = catalog
        .merged_ids()
        .filter(|id| id.starts_with("minecraft:ancient_city/"))
        .count();
    assert_eq!(ancient_templates, 58);
    for name in [
        "minecraft:ancient_city_start_degradation",
        "minecraft:ancient_city_generic_degradation",
        "minecraft:ancient_city_walls_degradation",
    ] {
        assert!(processors.list(name).is_some(), "missing {name}");
    }
    let mut missing = Vec::new();
    for (name, pool) in pools.pools() {
        if !name.starts_with("minecraft:ancient_city/") {
            continue;
        }
        for entry in pool.entries() {
            check_element_templates(&entry.element, catalog, processors, &mut missing);
        }
    }
    missing.sort();
    missing.dedup();
    assert_eq!(
        missing,
        vec!["minecraft:ancient_city/walls/intact_horizontal_wall_stairs_5".to_owned()]
    );
}

#[test]
fn ancient_city_selection_uses_start_height_biomes_and_overworld_only() {
    let structures = generator();
    let set = structures
        .sets()
        .get("ancient_cities")
        .expect("ancient cities set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("ancient cities must use random spread");
    };
    assert_eq!((placement.spacing, placement.separation), (24, 8));
    assert_eq!(placement.salt, 20_083_232);
    let chunk = placement.get_potential_structure_chunk(SEED, 0, 0);
    assert_eq!(chunk, ChunkPos::new(11, 15));
    assert!(placement.is_placement_chunk(SEED, chunk.x, chunk.z));

    let selected = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            SEED,
            StructureDimension::Overworld,
            63,
            &StartHeightBiomes,
        )
        .expect("ancient city selection")
        .expect("deep-dark ancient city selection");
    assert_eq!(selected.structure, "minecraft:ancient_city");
    assert_eq!(selected.set, "minecraft:ancient_cities");
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes("minecraft:plains"),
            )
            .expect("plains rejection")
            .is_none()
    );
    for dimension in [StructureDimension::Nether, StructureDimension::End] {
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    SEED,
                    dimension,
                    63,
                    &StaticBiomes("minecraft:deep_dark"),
                )
                .expect("dimension rejection")
                .is_none()
        );
    }
}

#[test]
fn fixed_seed_ancient_city_expands_city_center_and_child_pieces() {
    let structures = generator();
    let definition = structures
        .definitions()
        .get("minecraft:ancient_city")
        .expect("ancient city definition");
    let catalog = structures.template_catalog().expect("template catalog");
    let level = MockLevel::new();
    let context =
        StructureGenerationContext::new(SEED, ChunkPos::new(0, 0), 0, catalog).with_level(&level);
    let start = definition
        .generate_start(&context)
        .expect("ancient city start");
    assert!(start.is_valid());
    let pieces = start.jigsaw_pieces();
    assert_eq!(pieces.len(), 5);
    assert_eq!(
        pieces
            .iter()
            .map(|piece| piece.element.template_location().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec![
            "minecraft:ancient_city/city_center/city_center_2",
            "minecraft:ancient_city/city_center/walls/left",
            "minecraft:ancient_city/city_center/walls/top",
            "minecraft:ancient_city/city_center/walls/right",
            "minecraft:ancient_city/city_center/walls/bottom_1",
        ]
    );
    assert_eq!(pieces[0].position, BlockPos::new(20, -52, -13));
    assert_eq!(
        pieces[0].bounding_box,
        BoundingBox::new(-20, -52, -13, 20, -22, 4)
    );
    assert_eq!(pieces[0].gen_depth, 0);
    assert!(pieces.iter().all(|piece| piece.gen_depth <= 1));
    assert!(pieces.iter().any(|piece| {
        piece.element.template_location().is_some_and(|name| {
            name == "minecraft:ancient_city/city_center/city_center_1"
                || name == "minecraft:ancient_city/city_center/city_center_2"
                || name == "minecraft:ancient_city/city_center/city_center_3"
        })
    }));
    assert!(pieces.iter().any(|piece| {
        piece.element.template_location().is_some_and(|name| {
            name.starts_with("minecraft:ancient_city/city_center/walls/")
                || name.starts_with("minecraft:ancient_city/structures/")
                || name.starts_with("minecraft:ancient_city/walls/")
        })
    }));
    let raw = start.pieces_bounding_box().expect("piece bounds");
    let adapted = start.bounding_box().expect("adapted bounds");
    assert_eq!(raw, BoundingBox::new(-34, -52, -27, 34, -22, 18));
    assert_eq!(adapted, BoundingBox::new(-46, -64, -39, 46, -10, 30));
    assert_eq!(adapted, raw.inflated_by(12));
    assert!(raw.min_y() >= -63);
    assert!(raw.max_y() <= 319);

    let again = definition.generate_start(&context).expect("second start");
    assert_eq!(start, again);
}

#[test]
fn ancient_city_places_deepslate_through_the_underground_structure_step() {
    let structures = generator();
    let set = structures
        .sets()
        .get("ancient_cities")
        .expect("ancient cities set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("ancient cities must use random spread");
    };
    let chunk = placement.get_potential_structure_chunk(SEED, 0, 0);
    let mut level = MockLevel::new();
    let placement = structures
        .place_for_chunk(
            &mut level,
            chunk,
            SEED,
            StructureDimension::Overworld,
            63,
            &StaticBiomes("minecraft:deep_dark"),
            0,
            Decoration::UndergroundStructures,
        )
        .expect("ancient city placement");
    assert_eq!(placement.generated_starts, 1);
    assert!(placement.placed_pieces > 0);
    assert!(placement.written > 0);
    assert!(level.blocks.values().any(|state| {
        matches!(
            state.name.as_str(),
            "minecraft:deepslate_bricks"
                | "minecraft:deepslate_tiles"
                | "minecraft:cracked_deepslate_bricks"
                | "minecraft:cracked_deepslate_tiles"
        )
    }));
    assert!(
        level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw")
    );
    assert!(level.blocks.keys().all(|pos| pos.y() < 0));
}

#[test]
fn generated_deep_dark_chunk_contains_ancient_city_blocks() {
    let structures = generator();
    let generator = WorldGenerator::overworld(SEED)
        .expect("overworld generator")
        .with_structure_generator(structures);
    let biomes = GenBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structure generator");
    let (x, z) = (-71, -47);
    let selection = structures
        .structure_at_chunk(x, z, SEED, StructureDimension::Overworld, 63, &biomes)
        .expect("ancient city selection")
        .expect("generated deep-dark ancient city");
    assert_eq!(selection.structure, "minecraft:ancient_city");
    assert_eq!(selection.set, "minecraft:ancient_cities");
    assert_eq!(
        generator.biome_at((x << 4) + 8, -27, (z << 4) + 8),
        "minecraft:deep_dark"
    );

    let plain = WorldGenerator::overworld(SEED)
        .expect("plain generator")
        .generate_chunk_with_steps(x, z, &[])
        .expect("plain chunk");
    let structured = generator.generate_chunk(x, z).expect("structured chunk");
    let mut deepslate = 0;
    let mut sculk = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("structured block")
                    .name
                    .as_str()
                {
                    "minecraft:deepslate_bricks"
                    | "minecraft:deepslate_tiles"
                    | "minecraft:cracked_deepslate_bricks"
                    | "minecraft:cracked_deepslate_tiles" => deepslate += 1,
                    "minecraft:sculk" | "minecraft:sculk_vein" => sculk += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(deepslate, 1350, "ancient city deepslate in chunk {x},{z}");
    assert_eq!(sculk, 17, "ancient city sculk in chunk {x},{z}");
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
            plain
                .block_state(local_x, y, local_z)
                .expect("plain block")
                .name
                != "minecraft:deepslate_bricks"
        })
    })));
}
