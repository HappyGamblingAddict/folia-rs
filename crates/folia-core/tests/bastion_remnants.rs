use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::nether::{HEIGHT, MIN_Y, NetherGenerator};
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::{
    LiquidSettings, PoolCatalog, PoolElement, ProcessorListRef,
};
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructurePlacement,
};
use folia_core::worldgen::structure::start::{
    BiomeSet, StructureDefinition, StructureGenerationContext, StructureGenerator, StructureKind,
    StructureStartHeight, TerrainAdjustment,
};
use std::collections::{BTreeSet, HashMap};

const SEED: i64 = 12_345;
const START_CHUNK: ChunkPos = ChunkPos { x: 12, z: 20 };
const FORTRESS_CHUNK: ChunkPos = ChunkPos { x: 6, z: -32 };

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
        MIN_Y
    }

    fn height(&self) -> i32 {
        HEIGHT
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !(MIN_Y..MIN_Y + HEIGHT).contains(&pos.y()) {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        Some(MIN_Y)
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
        if y == 33 {
            "minecraft:nether_wastes"
        } else {
            "minecraft:basalt_deltas"
        }
    }
}

fn generator() -> StructureGenerator {
    StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("bastion structure generator")
        .with_enabled_structures(["minecraft:fortress", "minecraft:bastion_remnant"])
}

fn bastion_generator() -> StructureGenerator {
    StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("bastion structure generator")
        .with_enabled_structures(["minecraft:bastion_remnant"])
}

fn definition() -> StructureDefinition {
    generator()
        .definitions()
        .get("minecraft:bastion_remnant")
        .expect("bastion definition")
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
fn bastion_definition_uses_the_shipped_jigsaw_settings() {
    let definition = definition();
    assert_eq!(definition.name, "minecraft:bastion_remnant");
    assert_eq!(definition.step(), Decoration::SurfaceStructures);
    assert_eq!(definition.terrain_adaptation, TerrainAdjustment::None);
    assert_eq!(
        definition.biomes,
        BiomeSet::Tag("#minecraft:has_structure/bastion_remnant".to_owned())
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
        panic!("bastions must use the jigsaw structure kind");
    };
    assert_eq!(start_pool, "minecraft:bastion/starts");
    assert_eq!(start_jigsaw_name.as_deref(), None);
    assert_eq!(*size, 6);
    assert_eq!(*start_height, StructureStartHeight::Absolute(33));
    assert!(!*use_expansion_hack);
    assert_eq!(*project_start_to_heightmap, None);
    assert_eq!(
        (*max_distance_horizontal, *max_distance_vertical),
        (80, 4064)
    );
    assert_eq!((*dimension_padding_bottom, *dimension_padding_top), (0, 0));
    assert_eq!(*liquid_settings, LiquidSettings::ApplyWaterlogging);
    assert!(pool_aliases.is_empty());
}

#[test]
fn bastion_catalogs_resolve_every_pool_template_and_processor() {
    let catalog = StructureTemplateCatalog::shipped();
    let pools = PoolCatalog::shipped();
    let processors = StructureProcessorCatalog::shipped();
    assert_eq!(
        catalog
            .merged_ids()
            .filter(|id| id.starts_with("minecraft:bastion/"))
            .count(),
        167
    );
    assert_eq!(
        pools
            .pools()
            .filter(|(name, _)| name.starts_with("minecraft:bastion/"))
            .count(),
        60
    );
    assert!(
        processors
            .list("minecraft:bastion_generic_degradation")
            .is_some()
    );
    let start_pool = pools
        .pool("minecraft:bastion/starts")
        .expect("bastion start pool");
    assert_eq!(start_pool.size(), 4);
    assert_eq!(
        start_pool
            .entries()
            .iter()
            .map(|entry| entry.element.template_location().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec![
            "minecraft:bastion/units/air_base",
            "minecraft:bastion/hoglin_stable/air_base",
            "minecraft:bastion/treasure/big_air_full",
            "minecraft:bastion/bridge/starting_pieces/entrance_base",
        ]
    );
    let mut missing = Vec::new();
    for (name, pool) in pools.pools() {
        if !name.starts_with("minecraft:bastion/") {
            continue;
        }
        for entry in pool.entries() {
            check_element_templates(&entry.element, catalog, processors, &mut missing);
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "missing bastion assets: {missing:?}");
}

#[test]
fn bastion_selection_samples_start_height_and_is_nether_only() {
    let structures = bastion_generator();
    let set = structures
        .sets()
        .get("nether_complexes")
        .expect("nether complexes set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("nether complexes must use random spread");
    };
    assert_eq!((placement.spacing, placement.separation), (27, 4));
    assert_eq!(placement.salt, 30_084_232);
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
            StructureDimension::Nether,
            64,
            &StartHeightBiomes,
        )
        .expect("bastion selection")
        .expect("start-height bastion selection");
    assert_eq!(selected.structure, "minecraft:bastion_remnant");
    assert_eq!(selected.set, "minecraft:nether_complexes");
    assert!(
        structures
            .structure_at_chunk(
                START_CHUNK.x,
                START_CHUNK.z,
                SEED,
                StructureDimension::Nether,
                33,
                &StaticBiomes("minecraft:basalt_deltas"),
            )
            .expect("basalt-delta rejection")
            .is_none()
    );
    for dimension in [StructureDimension::Overworld, StructureDimension::End] {
        assert!(
            structures
                .structure_at_chunk(
                    START_CHUNK.x,
                    START_CHUNK.z,
                    SEED,
                    dimension,
                    33,
                    &StaticBiomes("minecraft:nether_wastes"),
                )
                .expect("dimension rejection")
                .is_none()
        );
    }

    let fallback = generator()
        .structure_at_chunk(
            START_CHUNK.x,
            START_CHUNK.z,
            SEED,
            StructureDimension::Nether,
            33,
            &StaticBiomes("minecraft:basalt_deltas"),
        )
        .expect("fortress fallback")
        .expect("basalt deltas must fall back to a fortress");
    assert_eq!(fallback.structure, "minecraft:fortress");
    assert_eq!(fallback.set, "minecraft:nether_complexes");
    assert!(
        StructureGenerator::shipped_catalog()
            .expect("shipped catalog generator")
            .structure_at_chunk(
                START_CHUNK.x,
                START_CHUNK.z,
                SEED,
                StructureDimension::Nether,
                64,
                &StaticBiomes("minecraft:nether_wastes"),
            )
            .expect("default catalog selection")
            .is_some_and(|selection| selection.structure == "minecraft:bastion_remnant")
    );
}

#[test]
fn fixed_seed_bastion_expands_start_pool_and_child_pieces() {
    let structures = generator();
    let definition = structures
        .definitions()
        .get("minecraft:bastion_remnant")
        .expect("bastion definition");
    let catalog = structures.template_catalog().expect("template catalog");
    let level = MockLevel::new();
    let context = StructureGenerationContext::new(SEED, START_CHUNK, 0, catalog).with_level(&level);
    let start = definition.generate_start(&context).expect("bastion start");
    assert!(start.is_valid());
    let pieces = start.jigsaw_pieces();
    assert_eq!(pieces.len(), 25);
    assert_eq!(pieces[0].pool, "minecraft:bastion/starts");
    assert_eq!(
        pieces[0].element.template_location(),
        Some("minecraft:bastion/treasure/big_air_full")
    );
    assert!(pieces.iter().all(|piece| piece.gen_depth <= 6));
    assert!(pieces.iter().any(|piece| {
        piece
            .element
            .template_location()
            .is_some_and(|name| name.starts_with("minecraft:bastion/treasure/"))
    }));
    assert!(pieces.iter().any(|piece| {
        piece
            .element
            .template_location()
            .is_some_and(|name| name.starts_with("minecraft:bastion/mobs/"))
    }));
    assert_eq!(pieces[0].position, BlockPos::new(192, 32, 320));
    assert_eq!(
        pieces[0].bounding_box,
        BoundingBox::new(192, 32, 283, 229, 79, 320)
    );
    let raw = start.pieces_bounding_box().expect("piece bounds");
    assert_eq!(raw, BoundingBox::new(154, 29, 283, 229, 102, 322));
    assert_eq!(start.bounding_box(), Some(raw));
    assert!(raw.min_y() >= MIN_Y);
    assert!(raw.max_y() < MIN_Y + HEIGHT);
    assert!(
        pieces
            .iter()
            .all(|piece| piece.bounding_box.intersects(raw))
    );
    assert_eq!(
        start,
        definition.generate_start(&context).expect("second start")
    );
}

#[test]
fn every_bastion_start_variant_expands_with_shipped_pools() {
    let structures = generator();
    let definition = structures
        .definitions()
        .get("minecraft:bastion_remnant")
        .expect("bastion definition");
    let catalog = structures.template_catalog().expect("template catalog");
    let level = MockLevel::new();
    let expected = [
        "minecraft:bastion/units/air_base",
        "minecraft:bastion/hoglin_stable/air_base",
        "minecraft:bastion/treasure/big_air_full",
        "minecraft:bastion/bridge/starting_pieces/entrance_base",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    for seed in 0..512 {
        let context = StructureGenerationContext::new(seed, ChunkPos::new(0, 0), 0, catalog)
            .with_level(&level);
        let start = definition
            .generate_start(&context)
            .unwrap_or_else(|error| panic!("bastion seed {seed}: {error}"));
        let pieces = start.jigsaw_pieces();
        assert!(pieces.len() > 1, "bastion seed {seed} did not expand");
        seen.insert(
            pieces[0]
                .element
                .template_location()
                .expect("start template")
                .to_owned(),
        );
        if seen == expected {
            break;
        }
    }
    assert_eq!(seen, expected);
}

#[test]
fn bastion_places_through_the_surface_structure_step() {
    let structures = bastion_generator();
    let mut level = MockLevel::new();
    let placement = structures
        .place_for_chunk(
            &mut level,
            ChunkPos::new(12, 19),
            SEED,
            StructureDimension::Nether,
            64,
            &StaticBiomes("minecraft:nether_wastes"),
            8,
            Decoration::SurfaceStructures,
        )
        .expect("bastion placement");
    assert_eq!(placement.generated_starts, 1);
    assert!(placement.placed_pieces > 0);
    assert!(placement.written > 0);
    assert!(level.blocks.values().any(|state| {
        matches!(
            state.name.as_str(),
            "minecraft:polished_blackstone_bricks"
                | "minecraft:cracked_polished_blackstone_bricks"
                | "minecraft:gilded_blackstone"
        )
    }));
    assert!(
        level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw")
    );
}

#[test]
fn nether_structure_start_uses_the_shipped_bastion_catalog() {
    let generator = NetherGenerator::new(SEED).expect("nether generator");
    let selection = generator
        .structure_at_chunk(START_CHUNK.x, START_CHUNK.z)
        .expect("nether structure selection")
        .expect("pinned bastion selection");
    assert_eq!(selection.structure, "minecraft:bastion_remnant");
    let start = generator
        .structure_start(&selection)
        .expect("bastion start from shipped templates");
    assert!(start.is_valid());
    assert!(
        start
            .jigsaw()
            .is_some_and(|jigsaw| jigsaw.pieces().len() > 6)
    );
}

#[test]
fn generated_nether_chunk_contains_bastion_blocks() {
    let generator = NetherGenerator::new(SEED).expect("nether generator");
    let chunk = generator
        .generate_chunk(12, 19)
        .expect("generated bastion chunk");
    let mut blackstone = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MIN_Y + HEIGHT {
                match chunk
                    .block_state(x, y, z)
                    .expect("nether block")
                    .name
                    .as_str()
                {
                    "minecraft:polished_blackstone_bricks"
                    | "minecraft:cracked_polished_blackstone_bricks"
                    | "minecraft:gilded_blackstone" => blackstone += 1,
                    "minecraft:jigsaw" => panic!("jigsaw block leaked into generated chunk"),
                    _ => {}
                }
            }
        }
    }
    assert!(
        blackstone > 0,
        "generated chunk contains no bastion blackstone"
    );
}

#[test]
fn generated_nether_chunk_contains_fortress_blocks() {
    let generator = NetherGenerator::new(SEED).expect("nether generator");
    let selection = generator
        .structure_at_chunk(FORTRESS_CHUNK.x, FORTRESS_CHUNK.z)
        .expect("nether structure selection")
        .expect("pinned fortress selection");
    assert_eq!(selection.structure, "minecraft:fortress");
    let chunk = generator
        .generate_chunk(FORTRESS_CHUNK.x, FORTRESS_CHUNK.z)
        .expect("generated fortress chunk");
    let mut bricks = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..MIN_Y + HEIGHT {
                if chunk.block_state(x, y, z).expect("nether block").name
                    == "minecraft:nether_bricks"
                {
                    bricks += 1;
                }
            }
        }
    }
    assert!(bricks > 0, "generated chunk contains no fortress bricks");
}
