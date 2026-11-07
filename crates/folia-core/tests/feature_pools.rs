//! Focused tests for `feature_pool_element` expansion and placement.
//!
//! A feature element is a real `PoolElementStructurePiece` in vanilla: it has
//! the synthetic downward `minecraft:bottom` jigsaw, a one-block bounding box,
//! and executes its `PlacedFeature` at the piece origin when the start is
//! placed. These tests pin both halves: expansion must not skip the element
//! (RNG order and piece counts), and placement must write the feature blocks
//! through the existing `FeatureLevel` machinery.

use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap, WorldGenRegion};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureGenerator;
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::jigsaw::{JigsawConfig, JigsawPlacement};
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::{PoolCatalog, PoolElement};
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{StructureGenerationContext, TerrainAdjustment};
use folia_core::worldgen::structure::template::StructureTemplate;
use std::collections::HashMap;

const VILLAGE_SEED: i64 = 12_345;
/// A real seed-12345 `village_plains` chunk in the generated Overworld. Its
/// recursive layout also selects feature-pool pieces inside this same chunk,
/// so the generator path can place them without cross-chunk height sampling.
const VILLAGE_START_CHUNK: (i32, i32) = (6, 123);
const VILLAGE_CHUNK: (i32, i32) = (6, 123);

struct MockLevel {
    min_y: i32,
    height: i32,
    floor_y: i32,
    ground: BlockState,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            floor_y: 64,
            ground: BlockState {
                name: "minecraft:dirt".to_owned(),
                properties: Vec::new(),
            },
            blocks: HashMap::new(),
        }
    }

    fn count(&self, name: &str) -> usize {
        self.blocks
            .values()
            .filter(|state| state.name == name)
            .count()
    }
}

impl FeatureLevel for MockLevel {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        if let Some(state) = self.blocks.get(&pos) {
            return Some(state);
        }
        (pos.y() < self.floor_y).then_some(&self.ground)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if pos.y() < self.min_y || pos.y() >= self.min_y + self.height {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        Some(self.floor_y)
    }
}

fn palette_entry(name: &str, properties: &[(&str, &str)]) -> Tag {
    let mut entries = vec![("Name".to_owned(), Tag::String(name.to_owned()))];
    if !properties.is_empty() {
        entries.push((
            "Properties".to_owned(),
            Tag::Compound(
                properties
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), Tag::String((*value).to_owned())))
                    .collect(),
            ),
        ));
    }
    Tag::Compound(entries)
}

fn block(pos: BlockPos, state: i32, nbt: Option<Tag>) -> Tag {
    let mut entries = vec![
        (
            "pos".to_owned(),
            Tag::List(vec![
                Tag::Int(pos.x()),
                Tag::Int(pos.y()),
                Tag::Int(pos.z()),
            ]),
        ),
        ("state".to_owned(), Tag::Int(state)),
    ];
    if let Some(nbt) = nbt {
        entries.push(("nbt".to_owned(), nbt));
    }
    Tag::Compound(entries)
}

fn template(size: [i32; 3], palette: Vec<Tag>, blocks: Vec<Tag>) -> StructureTemplate {
    let root = Tag::Compound(vec![
        ("DataVersion".to_owned(), Tag::Int(4788)),
        (
            "size".to_owned(),
            Tag::List(vec![
                Tag::Int(size[0]),
                Tag::Int(size[1]),
                Tag::Int(size[2]),
            ]),
        ),
        ("palette".to_owned(), Tag::List(palette)),
        ("blocks".to_owned(), Tag::List(blocks)),
    ]);
    StructureTemplate::from_nbt(&root).expect("synthetic template parses")
}

const SYNTHETIC_POOLS: &str = r#"{
  "version": "1.21.11",
  "data_version": 4788,
  "counts": {
    "pools": 2,
    "entries": 2,
    "elements": 2,
    "templates": 2,
    "by_element_type": {
      "minecraft:single_pool_element": 1,
      "minecraft:feature_pool_element": 1
    },
    "by_projection": { "rigid": 2 }
  },
  "template_pools": {
    "minecraft:test/town_centers": {
      "fallback": "minecraft:empty",
      "elements": [
        { "element": { "element_type": "minecraft:single_pool_element", "location": "minecraft:test/center", "processors": [], "projection": "rigid" }, "weight": 1 }
      ]
    },
    "minecraft:test/features": {
      "fallback": "minecraft:empty",
      "elements": [
        { "element": { "element_type": "minecraft:feature_pool_element", "feature": "minecraft:pile_hay", "projection": "rigid" }, "weight": 1 }
      ]
    }
  }
}"#;

fn synthetic_templates() -> HashMap<String, StructureTemplate> {
    let mut templates = HashMap::new();
    // One upward connector whose `final_state` is stone, so the hay pile has a
    // sturdy block below it at y=64.
    let nbt = Tag::Compound(vec![
        (
            "name".to_owned(),
            Tag::String("minecraft:center".to_owned()),
        ),
        (
            "target".to_owned(),
            Tag::String("minecraft:bottom".to_owned()),
        ),
        (
            "pool".to_owned(),
            Tag::String("minecraft:test/features".to_owned()),
        ),
        (
            "final_state".to_owned(),
            Tag::String("minecraft:stone".to_owned()),
        ),
        ("joint".to_owned(), Tag::String("rollable".to_owned())),
    ]);
    templates.insert(
        "minecraft:test/center".to_owned(),
        template(
            [1, 1, 1],
            vec![palette_entry(
                "minecraft:jigsaw",
                &[("orientation", "up_north")],
            )],
            vec![block(BlockPos::new(0, 0, 0), 0, Some(nbt))],
        ),
    );
    templates
}

fn synthetic_start(
    seed: i64,
) -> (
    folia_core::worldgen::structure::jigsaw::JigsawStart,
    HashMap<String, StructureTemplate>,
) {
    let pools = PoolCatalog::from_json(SYNTHETIC_POOLS).expect("synthetic pools parse");
    let templates = synthetic_templates();
    let level = MockLevel::new(0, 384);
    let context = StructureGenerationContext::new(seed, ChunkPos::new(0, 0), 0, &templates)
        .with_level(&level);
    let mut config = JigsawConfig::new("minecraft:test/town_centers", 3, 0);
    config.project_start_to_heightmap = Some(Heightmap::WorldSurfaceWg);
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        config,
    );
    let start = placement
        .generate("minecraft:test", &context, TerrainAdjustment::None)
        .expect("synthetic jigsaw expansion")
        .expect("valid synthetic start");
    (start, templates)
}

#[test]
fn synthetic_feature_pool_element_places_its_feature_at_the_piece_origin() {
    let (start, templates) = synthetic_start(12_345);
    assert_eq!(
        start.pieces().len(),
        2,
        "feature element must become a piece"
    );
    let feature_piece = start
        .pieces()
        .iter()
        .find(|piece| matches!(piece.element, PoolElement::Feature { .. }))
        .expect("feature element piece");
    assert_eq!(feature_piece.position.y(), 64);
    assert!(start.skipped_feature_elements().is_empty());

    let mut level = MockLevel::new(0, 384);
    let placement = start
        .place_in_chunk(
            &mut level,
            BoundingBox::new(0, 0, 0, 15, 383, 15),
            &templates,
            StructureProcessorCatalog::shipped(),
            12_345,
        )
        .expect("synthetic feature placement");
    assert_eq!(placement.feature_elements, 1);
    assert_eq!(placement.feature_placements, 1);
    assert!(placement.feature_written > 0, "hay pile wrote no blocks");
    assert!(
        level.count("minecraft:hay_block") > 0,
        "feature blocks: {:?}",
        placement.feature_blocks
    );
    assert!(
        placement
            .feature_blocks
            .iter()
            .all(|pos| level.blocks.contains_key(pos)),
        "every recorded feature block must be in the level"
    );
    assert!(
        placement
            .written_blocks
            .iter()
            .any(|pos| placement.feature_blocks.contains(pos)),
        "feature blocks must join the piece/start written-block set"
    );

    let again = start
        .place_in_chunk(
            &mut MockLevel::new(0, 384),
            BoundingBox::new(0, 0, 0, 15, 383, 15),
            &templates,
            StructureProcessorCatalog::shipped(),
            12_345,
        )
        .expect("synthetic feature placement is deterministic");
    assert_eq!(placement, again);
}

fn plains_generator() -> StructureGenerator {
    StructureGenerator::shipped_catalog().expect("catalog-backed structure generator")
}

struct PlainsBiomes;

impl StructureBiomeLookup for PlainsBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        "minecraft:plains"
    }
}

#[test]
fn shipped_village_feature_pool_elements_are_pieces_and_execute() {
    let catalog = StructureTemplateCatalog::shipped();
    let generator = plains_generator();
    let level = MockLevel::new(0, 384);
    let definition = generator
        .definitions()
        .get("minecraft:village_plains")
        .expect("village_plains definition");
    let context = StructureGenerationContext::new(
        VILLAGE_SEED,
        ChunkPos::new(VILLAGE_START_CHUNK.0, VILLAGE_START_CHUNK.1),
        0,
        catalog,
    )
    .with_level(&level);
    let start = definition
        .generate_start(&context)
        .expect("shipped village start");
    assert!(start.is_valid());
    let jigsaw = start.jigsaw().expect("jigsaw payload");
    assert!(
        jigsaw.skipped_feature_elements().is_empty(),
        "feature elements must not be recorded as skipped: {:?}",
        jigsaw.skipped_feature_elements()
    );
    let feature_elements = start
        .jigsaw_pieces()
        .iter()
        .filter(|piece| matches!(piece.element, PoolElement::Feature { .. }))
        .count();
    assert!(
        feature_elements > 0,
        "the fixed village layout must select at least one feature-pool element"
    );
    let plain_templates = start
        .jigsaw_pieces()
        .iter()
        .filter(|piece| {
            matches!(
                piece.element,
                PoolElement::Single { .. } | PoolElement::LegacySingle { .. }
            )
        })
        .count();
    assert!(plain_templates > 0);

    // Place through the real generator path with a dirt ground plane so tree
    // and pile features have support. Any unportable feature fails with its
    // exact id instead of being skipped.
    let biomes = PlainsBiomes;
    let mut first_level = MockLevel::new(0, 384);
    let first = generator
        .place_for_chunk(
            &mut first_level,
            ChunkPos::new(VILLAGE_CHUNK.0, VILLAGE_CHUNK.1),
            VILLAGE_SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
            8,
            Decoration::SurfaceStructures,
        )
        .expect("shipped village feature placement");
    assert!(first.generated_starts > 0);
    assert!(
        first.feature_elements > 0,
        "shipped village must execute selected feature elements"
    );
    assert!(
        first.feature_written > 0,
        "shipped village feature elements must write blocks; placements={} blocks={:?}",
        first.feature_placements,
        first.feature_blocks
    );
    let mut feature_blocks = 0;
    for name in [
        "minecraft:hay_block",
        "minecraft:oak_log",
        "minecraft:oak_leaves",
        "minecraft:dandelion",
        "minecraft:poppy",
        "minecraft:grass_block",
    ] {
        feature_blocks += first_level.count(name);
    }
    assert!(
        feature_blocks > 0,
        "expected at least one shipped decor/tree block in the placed level"
    );
    assert!(
        first
            .written_blocks
            .iter()
            .any(|pos| first.feature_blocks.contains(pos)),
        "feature blocks must be part of the start written-block set"
    );

    let mut second_level = MockLevel::new(0, 384);
    let second = generator
        .place_for_chunk(
            &mut second_level,
            ChunkPos::new(VILLAGE_CHUNK.0, VILLAGE_CHUNK.1),
            VILLAGE_SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
            8,
            Decoration::SurfaceStructures,
        )
        .expect("second shipped village feature placement");
    assert_eq!(first, second, "feature placement must be deterministic");
    assert_eq!(first_level.blocks, second_level.blocks);
}

/// Real terrain (no decorations) plus the shipped village structure pass.
fn place_village_on_generated_terrain(
    generator: &WorldGenerator,
    chunk_x: i32,
    chunk_z: i32,
    seed: i64,
) -> (
    folia_core::chunk::Chunk,
    folia_core::worldgen::structure::start::StructureChunkPlacement,
) {
    let structures = generator.structures().expect("structure manager");
    let mut chunk = WorldGenerator::overworld(seed)
        .expect("plain generator")
        .generate_chunk_with_steps(chunk_x, chunk_z, &[])
        .expect("terrain chunk");
    let biomes = GeneratorBiomes { generator };
    let heights = GeneratorHeights { generator };
    let mut region = WorldGenRegion::new(&mut chunk, generator.min_y(), generator.height());
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut region,
            &heights,
            ChunkPos::new(chunk_x, chunk_z),
            seed,
            StructureDimension::Overworld,
            63,
            &biomes,
            8,
            Decoration::SurfaceStructures,
        )
        .expect("village structure pass");
    (chunk, placement)
}

struct GeneratorBiomes<'a> {
    generator: &'a WorldGenerator,
}

impl StructureBiomeLookup for GeneratorBiomes<'_> {
    fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.generator.biome_at(x, y, z)
    }
}

/// Height-only level using the generator's base-height approximation, matching
/// `WorldGenerator::apply_structures`.
struct GeneratorHeights<'a> {
    generator: &'a WorldGenerator,
}

impl FeatureLevel for GeneratorHeights<'_> {
    fn min_y(&self) -> i32 {
        self.generator.min_y()
    }

    fn height(&self) -> i32 {
        self.generator.height()
    }

    fn get_block(&self, _pos: BlockPos) -> Option<&BlockState> {
        None
    }

    fn set_block(&mut self, _pos: BlockPos, _state: BlockState) -> bool {
        false
    }

    fn heightmap(&self, _kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        Some(self.generator.base_height(x, z))
    }
}

fn structured_generator(seed: i64) -> WorldGenerator {
    let structures = StructureGenerator::shipped_catalog().expect("village structure manager");
    WorldGenerator::overworld(seed)
        .expect("generator")
        .with_structure_generator(structures)
}

#[test]
fn generated_chunk_contains_village_feature_pool_blocks() {
    let seed = 12_345;
    let generator = structured_generator(seed);
    assert!(
        StructureSetRegistry::shipped()
            .expect("sets")
            .structure_at_chunk_filtered_matching(
                VILLAGE_START_CHUNK.0,
                VILLAGE_START_CHUNK.1,
                seed,
                StructureDimension::Overworld,
                63,
                &GeneratorBiomes {
                    generator: &generator
                },
                |structure| structure == "minecraft:village_plains",
            )
            .expect("village selection")
            .is_some(),
        "the pinned village chunk must select village_plains with real biomes"
    );

    let (terrain, placement) =
        place_village_on_generated_terrain(&generator, VILLAGE_CHUNK.0, VILLAGE_CHUNK.1, seed);
    assert!(
        placement.feature_elements > 0,
        "village chunk has no feature-pool elements"
    );
    assert!(
        placement.feature_written > 0,
        "village feature elements wrote no blocks into the generated terrain"
    );
    assert!(
        !placement.feature_blocks.is_empty(),
        "feature-written block set is empty"
    );

    // Full generator path: the recorded feature blocks must survive the later
    // decoration steps (except where a decoration deliberately replaces them).
    let chunk = generator
        .generate_chunk(VILLAGE_CHUNK.0, VILLAGE_CHUNK.1)
        .expect("structured chunk");
    let mut found = 0usize;
    for pos in &placement.feature_blocks {
        let local_x = pos.x().rem_euclid(16) as usize;
        let local_z = pos.z().rem_euclid(16) as usize;
        if let Some(state) = chunk.block_state(local_x, pos.y(), local_z)
            && state.name != "minecraft:air"
        {
            found += 1;
        }
    }
    assert!(
        found > 0,
        "none of the feature-pool blocks reached the generated chunk"
    );
    // The terrain-only reference must differ from the structured chunk.
    let mut differs = false;
    for x in 0..16 {
        for z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                if terrain.block_state(x, y, z) != chunk.block_state(x, y, z) {
                    differs = true;
                    break;
                }
            }
        }
    }
    assert!(differs, "structure pass did not change the generated chunk");
}
