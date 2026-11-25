use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::jigsaw::{JigsawConfig, JigsawPlacement};
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::{
    PoolAliasBinding, PoolAliasLookup, PoolCatalog, PoolElement, ProcessorListRef,
};
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructurePlacement,
};
use folia_core::worldgen::structure::start::{
    BiomeSet, StructureDefinition, StructureGenerationContext, StructureGenerator, StructureKind,
    StructureStartHeight, TerrainAdjustment,
};
use std::collections::HashMap;

const SEED: i64 = 12_345;
const START_CHUNK: ChunkPos = ChunkPos { x: 0, z: 14 };

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
        if (-40..=-20).contains(&y) {
            "minecraft:dripstone_caves"
        } else {
            "minecraft:plains"
        }
    }
}

struct GeneratorBiomes<'a> {
    generator: &'a WorldGenerator,
}

impl StructureBiomeLookup for GeneratorBiomes<'_> {
    fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.generator.biome_at(x, y, z)
    }
}

fn generator() -> StructureGenerator {
    StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("trial chamber structure generator")
        .with_enabled_structures(["minecraft:trial_chambers"])
}

fn definition() -> StructureDefinition {
    generator()
        .definitions()
        .get("minecraft:trial_chambers")
        .expect("trial chamber definition")
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
fn trial_chamber_definition_uses_the_shipped_jigsaw_settings() {
    let definition = definition();
    assert_eq!(definition.name, "minecraft:trial_chambers");
    assert_eq!(definition.step(), Decoration::UndergroundStructures);
    assert_eq!(
        definition.terrain_adaptation,
        TerrainAdjustment::Encapsulate
    );
    assert_eq!(
        definition.biomes,
        BiomeSet::Tag("#minecraft:has_structure/trial_chambers".to_owned())
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
        liquid_settings,
        pool_aliases,
    } = &definition.kind
    else {
        panic!("trial chambers must use the jigsaw structure kind");
    };
    assert_eq!(start_pool, "minecraft:trial_chambers/chamber/end");
    assert_eq!(start_jigsaw_name.as_deref(), None);
    assert_eq!(*size, 20);
    assert_eq!(
        *start_height,
        StructureStartHeight::Uniform {
            min_inclusive: -40,
            max_inclusive: -20,
        }
    );
    assert!(!*use_expansion_hack);
    assert_eq!(*project_start_to_heightmap, None);
    assert_eq!(
        (*max_distance_horizontal, *max_distance_vertical),
        (116, 4064)
    );
    assert_eq!(
        (*dimension_padding_bottom, *dimension_padding_top),
        (10, 10)
    );
    assert_eq!(
        *liquid_settings,
        folia_core::worldgen::structure::pool::LiquidSettings::IgnoreWaterlogging
    );
    assert_eq!(pool_aliases.len(), 3);
    assert!(matches!(
        &pool_aliases[0],
        PoolAliasBinding::RandomGroup { groups } if groups.len() == 3
    ));
    assert!(matches!(
        &pool_aliases[1],
        PoolAliasBinding::Random { alias, targets }
            if alias == "minecraft:trial_chambers/spawner/contents/melee"
                && targets.len() == 3
    ));
    assert!(matches!(
        &pool_aliases[2],
        PoolAliasBinding::Random { alias, targets }
            if alias == "minecraft:trial_chambers/spawner/contents/small_melee"
                && targets.len() == 4
    ));
}

#[test]
fn trial_chamber_catalogs_resolve_every_pool_template_and_processor() {
    let catalog = StructureTemplateCatalog::shipped();
    let pools = PoolCatalog::shipped();
    let processors = StructureProcessorCatalog::shipped();
    assert_eq!(
        catalog
            .merged_ids()
            .filter(|id| id.starts_with("minecraft:trial_chambers/"))
            .count(),
        191
    );
    assert_eq!(
        pools
            .pools()
            .filter(|(name, _)| name.starts_with("minecraft:trial_chambers/"))
            .count(),
        47
    );
    assert!(
        processors
            .list("minecraft:trial_chambers_copper_bulb_degradation")
            .is_some()
    );
    let start_pool = pools
        .pool("minecraft:trial_chambers/chamber/end")
        .expect("trial chamber start pool");
    assert_eq!(start_pool.size(), 2);
    assert_eq!(
        start_pool
            .entries()
            .iter()
            .map(|entry| entry.element.template_location().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec![
            "minecraft:trial_chambers/corridor/end_1",
            "minecraft:trial_chambers/corridor/end_2",
        ]
    );
    let mut missing = Vec::new();
    for (name, pool) in pools.pools() {
        if !name.starts_with("minecraft:trial_chambers/") {
            continue;
        }
        for entry in pool.entries() {
            check_element_templates(&entry.element, catalog, processors, &mut missing);
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "missing trial templates: {missing:?}");
}

#[test]
fn trial_chamber_selection_samples_start_height_and_is_overworld_only() {
    let structures = generator();
    let set = structures
        .sets()
        .get("trial_chambers")
        .expect("trial chambers set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("trial chambers must use random spread");
    };
    assert_eq!((placement.spacing, placement.separation), (34, 12));
    assert_eq!(placement.salt, 94_251_327);
    assert_eq!(
        placement.get_potential_structure_chunk(SEED, 0, 0),
        START_CHUNK
    );
    assert!(placement.is_placement_chunk(SEED, START_CHUNK.x, START_CHUNK.z));

    let selected = structures
        .structure_at_chunk(
            START_CHUNK.x,
            START_CHUNK.z,
            SEED,
            StructureDimension::Overworld,
            63,
            &StartHeightBiomes,
        )
        .expect("trial chamber selection")
        .expect("start-height trial chamber selection");
    assert_eq!(selected.structure, "minecraft:trial_chambers");
    assert_eq!(selected.set, "minecraft:trial_chambers");
    assert!(
        structures
            .structure_at_chunk(
                START_CHUNK.x,
                START_CHUNK.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes("minecraft:deep_dark"),
            )
            .expect("deep-dark rejection")
            .is_none()
    );
    for dimension in [StructureDimension::Nether, StructureDimension::End] {
        assert!(
            structures
                .structure_at_chunk(
                    START_CHUNK.x,
                    START_CHUNK.z,
                    SEED,
                    dimension,
                    63,
                    &StartHeightBiomes,
                )
                .expect("dimension rejection")
                .is_none()
        );
    }
    assert!(
        StructureGenerator::shipped_catalog()
            .expect("shipped catalog generator")
            .structure_at_chunk(
                START_CHUNK.x,
                START_CHUNK.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &StartHeightBiomes,
            )
            .expect("default catalog selection")
            .is_some_and(|selection| selection.structure == "minecraft:trial_chambers")
    );
}

#[test]
fn fixed_seed_trial_chamber_expands_corridor_end_and_child_pieces() {
    let structures = generator();
    let definition = structures
        .definitions()
        .get("minecraft:trial_chambers")
        .expect("trial chamber definition");
    let catalog = structures.template_catalog().expect("template catalog");
    let level = MockLevel::new();
    let context = StructureGenerationContext::new(SEED, START_CHUNK, 0, catalog).with_level(&level);
    let start = definition
        .generate_start(&context)
        .expect("trial chamber start");
    assert!(start.is_valid());
    let pieces = start.jigsaw_pieces();
    assert_eq!(pieces.len(), 206);
    assert_eq!(
        pieces[0].element.template_location(),
        Some("minecraft:trial_chambers/corridor/end_1")
    );
    assert_eq!(pieces[0].position, BlockPos::new(0, -38, 224));
    assert_eq!(pieces[0].gen_depth, 0);
    assert!(pieces.iter().all(|piece| piece.gen_depth <= 20));
    assert!(pieces.iter().any(|piece| {
        piece
            .element
            .template_location()
            .is_some_and(|name| name.starts_with("minecraft:trial_chambers/chamber/"))
    }));
    assert!(pieces.iter().any(|piece| {
        piece
            .element
            .template_location()
            .is_some_and(|name| name.starts_with("minecraft:trial_chambers/corridor/"))
    }));
    assert!(pieces.iter().any(|piece| {
        piece.pool.starts_with("minecraft:trial_chambers/spawner/")
            && !piece.pool.contains("/contents/")
    }));
    let raw = start.pieces_bounding_box().expect("piece bounds");
    let adapted = start.bounding_box().expect("adapted bounds");
    assert_eq!(raw, BoundingBox::new(-30, -51, 145, 95, -5, 271));
    assert_eq!(adapted, BoundingBox::new(-42, -63, 133, 107, 7, 283));
    assert_eq!(adapted, raw.inflated_by(12));
    assert!(raw.min_y() >= -54);
    assert!(raw.max_y() <= 309);
    assert_eq!(
        pieces[..12]
            .iter()
            .map(|piece| piece.element.template_location().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec![
            "minecraft:trial_chambers/corridor/end_1",
            "minecraft:trial_chambers/reward/ominous_vault",
            "minecraft:trial_chambers/corridor/second_plate",
            "minecraft:trial_chambers/chamber/slanted",
            "minecraft:trial_chambers/decor/disposal",
            "minecraft:trial_chambers/chamber/slanted/center",
            "minecraft:trial_chambers/decor/disposal",
            "minecraft:trial_chambers/chamber/entrance_cap",
            "minecraft:trial_chambers/intersection/intersection_2",
            "minecraft:trial_chambers/corridor/straight_7",
            "minecraft:trial_chambers/corridor/straight_3",
            "minecraft:trial_chambers/corridor/straight_5",
        ]
    );
    assert_eq!(
        start,
        definition.generate_start(&context).expect("second start")
    );

    let StructureKind::Jigsaw {
        start_height,
        pool_aliases,
        ..
    } = &definition.kind
    else {
        unreachable!();
    };
    assert_eq!(start_height.sample_at_chunk(SEED, START_CHUNK), -37);
    let lookup = PoolAliasLookup::create(
        pool_aliases,
        BlockPos::new(START_CHUNK.x << 4, -37, START_CHUNK.z << 4),
        SEED,
    )
    .expect("trial pool aliases resolve");
    let ranged = lookup.lookup("minecraft:trial_chambers/spawner/contents/ranged");
    let slow_ranged = lookup.lookup("minecraft:trial_chambers/spawner/contents/slow_ranged");
    assert_eq!(
        ranged,
        "minecraft:trial_chambers/spawner/ranged/poison_skeleton"
    );
    assert_eq!(
        slow_ranged,
        "minecraft:trial_chambers/spawner/slow_ranged/poison_skeleton"
    );
    assert_eq!(
        lookup.lookup("minecraft:trial_chambers/spawner/contents/melee"),
        "minecraft:trial_chambers/spawner/melee/spider"
    );
    assert_eq!(
        lookup.lookup("minecraft:trial_chambers/spawner/contents/small_melee"),
        "minecraft:trial_chambers/spawner/small_melee/silverfish"
    );
    assert!(
        ranged.ends_with("/skeleton")
            || ranged.ends_with("/stray")
            || ranged.ends_with("/poison_skeleton")
    );
    assert_eq!(
        ranged.strip_prefix("minecraft:trial_chambers/spawner/ranged/"),
        slow_ranged.strip_prefix("minecraft:trial_chambers/spawner/slow_ranged/")
    );
}

#[test]
fn trial_chamber_aliases_rewrite_connector_pools_during_expansion() {
    let definition = definition();
    let StructureKind::Jigsaw { pool_aliases, .. } = &definition.kind else {
        unreachable!();
    };
    let catalog = StructureTemplateCatalog::shipped();
    let level = MockLevel::new();
    let context =
        StructureGenerationContext::new(SEED, ChunkPos::new(0, 0), 0, catalog).with_level(&level);
    let mut config = JigsawConfig::new(
        "minecraft:trial_chambers/spawner/ranged",
        1,
        StructureStartHeight::Absolute(-30),
    );
    config.pool_aliases = pool_aliases.clone();
    let start = JigsawPlacement::new(
        PoolCatalog::shipped(),
        catalog,
        StructureProcessorCatalog::shipped(),
        config,
    )
    .generate(
        "minecraft:trial_chambers",
        &context,
        TerrainAdjustment::None,
    )
    .expect("aliased spawner expansion")
    .expect("spawner start");
    let lookup = PoolAliasLookup::create(pool_aliases, BlockPos::new(0, -30, 0), SEED)
        .expect("spawner aliases");
    let expected = lookup.lookup("minecraft:trial_chambers/spawner/contents/ranged");
    assert!(start.pieces().len() >= 2);
    assert!(start.pieces().iter().any(|piece| piece.pool == expected));
    assert!(expected.starts_with("minecraft:trial_chambers/spawner/ranged/"));
}

#[test]
fn trial_chamber_places_through_the_underground_structure_step_only() {
    let structures = generator();
    let mut level = MockLevel::new();
    let placement = structures
        .place_for_chunk(
            &mut level,
            START_CHUNK,
            SEED,
            StructureDimension::Overworld,
            63,
            &StartHeightBiomes,
            0,
            Decoration::UndergroundStructures,
        )
        .expect("trial chamber placement");
    assert_eq!(placement.generated_starts, 1);
    assert!(placement.placed_pieces > 0);
    assert!(placement.written > 0);
    assert!(level.blocks.values().any(|state| {
        matches!(
            state.name.as_str(),
            "minecraft:tuff_bricks"
                | "minecraft:chiseled_tuff_bricks"
                | "minecraft:polished_tuff"
                | "minecraft:waxed_oxidized_cut_copper"
                | "minecraft:waxed_oxidized_copper"
        )
    }));
    assert!(
        level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw")
    );
    assert!(level.blocks.keys().all(|pos| pos.y() < 0));

    let mut surface_level = MockLevel::new();
    let surface = structures
        .place_for_chunk(
            &mut surface_level,
            START_CHUNK,
            SEED,
            StructureDimension::Overworld,
            63,
            &StartHeightBiomes,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("surface structure pass");
    assert_eq!(surface.generated_starts, 0);
    assert_eq!(surface.written, 0);
    assert!(surface_level.blocks.is_empty());
}

#[test]
fn generated_chunk_contains_trial_chamber_blocks() {
    let structures = generator();
    let generator = WorldGenerator::overworld(SEED)
        .expect("overworld generator")
        .with_structure_generator(structures.clone());
    let biomes = GeneratorBiomes {
        generator: &generator,
    };
    let mut selected_chunk = None;
    for region_x in -2..=2 {
        for region_z in -2..=2 {
            let set = structures
                .sets()
                .get("trial_chambers")
                .expect("trial chambers set");
            let StructurePlacement::RandomSpread(placement) = &set.placement else {
                unreachable!();
            };
            let chunk = placement.get_potential_structure_chunk(SEED, region_x, region_z);
            if structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    SEED,
                    StructureDimension::Overworld,
                    63,
                    &biomes,
                )
                .expect("generated trial selection")
                .is_some_and(|selection| selection.structure == "minecraft:trial_chambers")
            {
                selected_chunk = Some(chunk);
                break;
            }
        }
        if selected_chunk.is_some() {
            break;
        }
    }
    let chunk = selected_chunk.expect("a generated trial chamber candidate");
    assert_eq!(chunk, ChunkPos::new(-30, -19));
    let plain = WorldGenerator::overworld(SEED)
        .expect("plain generator")
        .generate_chunk_with_steps(chunk.x, chunk.z, &[])
        .expect("plain chunk");
    let structured = generator
        .generate_chunk(chunk.x, chunk.z)
        .expect("structured chunk");
    let mut tuff_bricks = 0;
    let mut chiseled_tuff = 0;
    let mut polished_tuff = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("structured block")
                    .name
                    .as_str()
                {
                    "minecraft:tuff_bricks" => tuff_bricks += 1,
                    "minecraft:chiseled_tuff_bricks" => chiseled_tuff += 1,
                    "minecraft:polished_tuff" => polished_tuff += 1,
                    "minecraft:jigsaw" => panic!("jigsaw block survived placement"),
                    _ => {}
                }
                assert_ne!(
                    plain
                        .block_state(local_x, y, local_z)
                        .expect("plain block")
                        .name,
                    "minecraft:tuff_bricks"
                );
            }
        }
    }
    assert_eq!(tuff_bricks, 320, "trial chamber tuff in chunk {chunk:?}");
    assert_eq!(chiseled_tuff, 0);
    assert_eq!(polished_tuff, 0);
}
