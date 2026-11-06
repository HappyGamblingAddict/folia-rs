//! Focused tests for recursive `JigsawPlacement` and `PoolElementStructurePiece`.
//!
//! The shipped template pools are exercised directly elsewhere. These tests
//! build tiny synthetic village-like templates so the connector walk, RNG
//! order, bounds, failure paths, waterlogging and structure-void handling can
//! be pinned without the out-of-tree NBT reference directory.

use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{FeatureLevel, Heightmap};
use folia_core::worldgen::structure::StructureError;
use folia_core::worldgen::structure::jigsaw::{JigsawConfig, JigsawPlacement};
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::PoolCatalog;
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::start::{StructureGenerationContext, TerrainAdjustment};
use folia_core::worldgen::structure::template::StructureTemplate;
use std::collections::HashMap;

const POOLS_JSON: &str = r#"{
  "version": "1.21.11",
  "data_version": 4788,
  "counts": {
    "pools": 2,
    "entries": 3,
    "elements": 3,
    "templates": 3,
    "by_element_type": { "minecraft:single_pool_element": 3 },
    "by_projection": { "rigid": 3 }
  },
  "template_pools": {
    "minecraft:test/town_centers": {
      "fallback": "minecraft:empty",
      "elements": [
        { "element": { "element_type": "minecraft:single_pool_element", "location": "minecraft:test/center", "processors": [], "projection": "rigid" }, "weight": 1 }
      ]
    },
    "minecraft:test/houses": {
      "fallback": "minecraft:empty",
      "elements": [
        { "element": { "element_type": "minecraft:single_pool_element", "location": "minecraft:test/house_small", "processors": [], "projection": "rigid" }, "weight": 1 },
        { "element": { "element_type": "minecraft:single_pool_element", "location": "minecraft:test/house_big", "processors": [], "projection": "rigid" }, "weight": 1 }
      ]
    }
  }
}"#;

#[derive(Default)]
struct MockLevel {
    min_y: i32,
    height: i32,
    fixed_height: Option<i32>,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            fixed_height: Some(64),
            blocks: HashMap::new(),
        }
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
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if pos.y() < self.min_y || pos.y() >= self.min_y + self.height {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        self.fixed_height
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

fn jigsaw_nbt(name: &str, target: &str, pool: &str) -> Tag {
    Tag::Compound(vec![
        ("name".to_owned(), Tag::String(name.to_owned())),
        ("target".to_owned(), Tag::String(target.to_owned())),
        ("pool".to_owned(), Tag::String(pool.to_owned())),
        (
            "final_state".to_owned(),
            Tag::String("minecraft:air".to_owned()),
        ),
        ("joint".to_owned(), Tag::String("rollable".to_owned())),
    ])
}

fn template(size: [i32; 3], palette: Vec<Tag>, blocks: Vec<Tag>) -> StructureTemplate {
    let root = Tag::Compound(vec![
        ("DataVersion".to_owned(), Tag::Int(4671)),
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

fn village_templates() -> HashMap<String, StructureTemplate> {
    let mut templates = HashMap::new();

    // Center: 7x5x7 stone floor. The north-facing connector sits on the north
    // edge and targets a child named `house`.
    let mut center_blocks = Vec::new();
    for x in 0..7 {
        for z in 0..7 {
            center_blocks.push(block(BlockPos::new(x, 0, z), 0, None));
        }
    }
    center_blocks.push(block(
        BlockPos::new(3, 0, 0),
        1,
        Some(jigsaw_nbt("center", "house", "minecraft:test/houses")),
    ));
    templates.insert(
        "minecraft:test/center".to_owned(),
        template(
            [7, 5, 7],
            vec![
                palette_entry("minecraft:stone", &[]),
                palette_entry("minecraft:jigsaw", &[("orientation", "north_up")]),
            ],
            center_blocks,
        ),
    );

    // Small house: south-facing connector on the south edge.
    let mut small_blocks = vec![block(BlockPos::new(0, 0, 0), 0, None)];
    small_blocks.push(block(
        BlockPos::new(2, 0, 4),
        1,
        Some(jigsaw_nbt("house", "end", "minecraft:empty")),
    ));
    templates.insert(
        "minecraft:test/house_small".to_owned(),
        template(
            [5, 3, 5],
            vec![
                palette_entry("minecraft:stone", &[]),
                palette_entry("minecraft:jigsaw", &[("orientation", "south_up")]),
            ],
            small_blocks,
        ),
    );

    let mut big_blocks = vec![block(BlockPos::new(0, 0, 0), 0, None)];
    big_blocks.push(block(
        BlockPos::new(3, 0, 6),
        1,
        Some(jigsaw_nbt("house", "end", "minecraft:empty")),
    ));
    templates.insert(
        "minecraft:test/house_big".to_owned(),
        template(
            [7, 5, 7],
            vec![
                palette_entry("minecraft:stone", &[]),
                palette_entry("minecraft:jigsaw", &[("orientation", "south_up")]),
            ],
            big_blocks,
        ),
    );

    templates
}

fn config(start_pool: &str) -> JigsawConfig {
    let mut config = JigsawConfig::new(start_pool, 3, 0);
    config.project_start_to_heightmap = Some(Heightmap::WorldSurfaceWg);
    config.max_distance_horizontal = 80;
    config.max_distance_vertical = 4064;
    config
}

fn generate(seed: i64) -> folia_core::worldgen::structure::jigsaw::JigsawStart {
    let pools = PoolCatalog::from_json(POOLS_JSON).expect("synthetic pools parse");
    let templates = village_templates();
    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(64);
    let context = StructureGenerationContext::new(seed, ChunkPos::new(0, 0), 0, &templates)
        .with_level(&level);
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        config("minecraft:test/town_centers"),
    );
    placement
        .generate(
            "minecraft:village_plains",
            &context,
            TerrainAdjustment::BeardThin,
        )
        .expect("jigsaw expansion succeeds")
        .expect("start is not empty")
}

#[test]
fn recursive_village_has_multiple_pieces_and_stable_bounds() {
    let start = generate(12_345);
    assert_eq!(start.structure_id(), "minecraft:village_plains");
    assert!(start.is_valid());
    assert!(
        start.pieces().len() > 1,
        "recursive expansion produced only the start piece: {:?}",
        start.pieces().len()
    );
    let bounds = start.pieces_bounding_box().expect("bounds");
    assert_eq!(bounds.min_y(), 63);
    assert!(bounds.max_y() >= 64);

    let again = generate(12_345);
    assert_eq!(start, again, "same seed must produce the same start");

    let other = generate(54_321);
    assert_ne!(
        (start.pieces().len(), start.pieces_bounding_box()),
        (other.pieces().len(), other.pieces_bounding_box()),
        "a different seed should produce a different piece layout or size"
    );
}

#[test]
fn placement_writes_blocks_into_a_chunk() {
    let start = generate(12_345);
    let templates = village_templates();
    let processors = StructureProcessorCatalog::shipped();
    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(64);
    let chunk_box = BoundingBox::new(0, 0, 0, 15, 383, 15);
    let placement = start
        .place_in_chunk(&mut level, chunk_box, &templates, processors, 12_345)
        .expect("jigsaw start places");
    assert!(placement.placed_pieces >= 1);
    assert!(placement.written > 0);
    assert!(
        level
            .blocks
            .values()
            .any(|state| state.name == "minecraft:stone"),
        "expected the center template's stone floor in the chunk"
    );
    assert!(
        level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw"),
        "normal generation must replace jigsaw blocks"
    );
}

#[test]
fn start_jigsaw_name_and_missing_pool_fail_explicitly() {
    let pools = PoolCatalog::from_json(POOLS_JSON).expect("synthetic pools parse");
    let templates = village_templates();
    let mut level = MockLevel::new(0, 384);
    let context =
        StructureGenerationContext::new(1, ChunkPos::new(0, 0), 0, &templates).with_level(&level);
    let mut placement_config = config("minecraft:test/town_centers");
    placement_config.start_jigsaw_name = Some("missing".to_owned());
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        placement_config,
    );
    let error = placement
        .generate(
            "minecraft:village_plains",
            &context,
            TerrainAdjustment::BeardThin,
        )
        .expect_err("missing start jigsaw must be explicit");
    assert!(matches!(
        error,
        StructureError::StartJigsawNotFound { ref name, .. } if name == "missing"
    ));

    // A connector pointing at a pool that is not in the catalog must fail
    // rather than returning a valid single-piece start.
    let mut broken_templates = village_templates();
    let palette = vec![
        palette_entry("minecraft:stone", &[]),
        palette_entry("minecraft:jigsaw", &[("orientation", "north_up")]),
    ];
    let broken = template(
        [7, 5, 7],
        palette,
        vec![block(
            BlockPos::new(3, 0, 0),
            1,
            Some(jigsaw_nbt("center", "house", "minecraft:missing_pool")),
        )],
    );
    broken_templates.insert("minecraft:test/center".to_owned(), broken);
    let context = StructureGenerationContext::new(1, ChunkPos::new(0, 0), 0, &broken_templates)
        .with_level(&level);
    let placement = JigsawPlacement::new(
        &pools,
        &broken_templates,
        StructureProcessorCatalog::shipped(),
        config("minecraft:test/town_centers"),
    );
    let error = placement
        .generate(
            "minecraft:village_plains",
            &context,
            TerrainAdjustment::BeardThin,
        )
        .expect_err("missing pool must be explicit");
    assert!(matches!(
        error,
        StructureError::UnknownPool { ref pool } if pool == "minecraft:missing_pool"
    ));
    let _ = &mut level;
}

fn single_element_template(
    _name: &str,
    state: BlockState,
) -> (PoolCatalog, HashMap<String, StructureTemplate>) {
    let json = r#"{
          "version":"1.21.11","data_version":4788,
          "counts":{"pools":1,"entries":1,"elements":1,"templates":1,
            "by_element_type":{"minecraft:single_pool_element":1},
            "by_projection":{"rigid":1}},
          "template_pools":{"minecraft:test/single":{"fallback":"minecraft:empty","elements":[
            {"element":{"element_type":"minecraft:single_pool_element","location":"minecraft:test/single","processors":[],"projection":"rigid"},"weight":1}
          ]}}
        }"#;
    let properties: Vec<Tag> = state
        .properties
        .iter()
        .map(|(key, value)| palette_entry(&state.name, &[(key.as_str(), value.as_str())]))
        .collect();
    let palette = if properties.is_empty() {
        vec![palette_entry(&state.name, &[])]
    } else {
        properties
    };
    let template = template(
        [1, 1, 1],
        palette,
        vec![block(BlockPos::new(0, 0, 0), 0, None)],
    );
    let mut templates = HashMap::new();
    templates.insert("minecraft:test/single".to_owned(), template);
    (
        PoolCatalog::from_json(json).expect("single pool parses"),
        templates,
    )
}

#[test]
fn waterlogging_and_structure_void_are_explicit() {
    // Apply waterlogging: a waterloggable block written into water becomes
    // waterlogged.
    let fence = BlockState {
        name: "minecraft:oak_fence".to_owned(),
        properties: vec![("waterlogged".to_owned(), "false".to_owned())],
    };
    let (pools, templates) = single_element_template("fence", fence.clone());
    let mut level = MockLevel::new(0, 384);
    level.blocks.insert(
        BlockPos::new(0, 63, 0),
        BlockState {
            name: "minecraft:water".to_owned(),
            properties: vec![],
        },
    );
    let context =
        StructureGenerationContext::new(1, ChunkPos::new(0, 0), 0, &templates).with_level(&level);
    let mut config = JigsawConfig::new("minecraft:test/single", 0, 0);
    config.project_start_to_heightmap = Some(Heightmap::WorldSurfaceWg);
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        config,
    );
    let start = placement
        .generate("minecraft:test", &context, TerrainAdjustment::None)
        .expect("single element generates")
        .expect("start");
    start
        .place_in_chunk(
            &mut level,
            BoundingBox::new(0, 0, 0, 15, 383, 15),
            &templates,
            StructureProcessorCatalog::shipped(),
            1,
        )
        .expect("places");
    let placed = level
        .get_block(BlockPos::new(0, 63, 0))
        .expect("fence block");
    assert_eq!(placed.name, "minecraft:oak_fence");
    assert!(
        placed
            .properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true"),
        "expected waterlogging to be applied: {placed:?}"
    );

    // Ignore waterlogging: the same template keeps waterlogged=false.
    let (pools, templates) = single_element_template("fence", fence);
    let mut level = MockLevel::new(0, 384);
    level.blocks.insert(
        BlockPos::new(0, 63, 0),
        BlockState {
            name: "minecraft:water".to_owned(),
            properties: vec![],
        },
    );
    let context =
        StructureGenerationContext::new(1, ChunkPos::new(0, 0), 0, &templates).with_level(&level);
    let mut config = JigsawConfig::new("minecraft:test/single", 0, 0);
    config.project_start_to_heightmap = Some(Heightmap::WorldSurfaceWg);
    config.liquid_settings =
        folia_core::worldgen::structure::pool::LiquidSettings::IgnoreWaterlogging;
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        config,
    );
    let start = placement
        .generate("minecraft:test", &context, TerrainAdjustment::None)
        .expect("single element generates")
        .expect("start");
    start
        .place_in_chunk(
            &mut level,
            BoundingBox::new(0, 0, 0, 15, 383, 15),
            &templates,
            StructureProcessorCatalog::shipped(),
            1,
        )
        .expect("places");
    let placed = level
        .get_block(BlockPos::new(0, 63, 0))
        .expect("fence block");
    assert!(
        placed
            .properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "false"),
        "ignore_waterlogging must keep the template property: {placed:?}"
    );

    // Jigsaw final_state=structure_void drops the block entirely.
    let jigsaw = template(
        [1, 1, 1],
        vec![palette_entry(
            "minecraft:jigsaw",
            &[("orientation", "north_up")],
        )],
        vec![block(
            BlockPos::new(0, 0, 0),
            0,
            Some(Tag::Compound(vec![
                (
                    "name".to_owned(),
                    Tag::String("minecraft:center".to_owned()),
                ),
                (
                    "target".to_owned(),
                    Tag::String("minecraft:empty".to_owned()),
                ),
                ("pool".to_owned(), Tag::String("minecraft:empty".to_owned())),
                (
                    "final_state".to_owned(),
                    Tag::String("minecraft:structure_void".to_owned()),
                ),
            ])),
        )],
    );
    let json = r#"{
      "version":"1.21.11","data_version":4788,
      "counts":{"pools":1,"entries":1,"elements":1,"templates":1,
        "by_element_type":{"minecraft:single_pool_element":1},
        "by_projection":{"rigid":1}},
      "template_pools":{"minecraft:test/single":{"fallback":"minecraft:empty","elements":[
        {"element":{"element_type":"minecraft:single_pool_element","location":"minecraft:test/single","processors":[],"projection":"rigid"},"weight":1}
      ]}}
    }"#;
    let pools = PoolCatalog::from_json(json).expect("void pool parses");
    let mut templates = HashMap::new();
    templates.insert("minecraft:test/single".to_owned(), jigsaw);
    let mut level = MockLevel::new(0, 384);
    let context =
        StructureGenerationContext::new(1, ChunkPos::new(0, 0), 0, &templates).with_level(&level);
    let mut config = JigsawConfig::new("minecraft:test/single", 0, 0);
    config.project_start_to_heightmap = Some(Heightmap::WorldSurfaceWg);
    let placement = JigsawPlacement::new(
        &pools,
        &templates,
        StructureProcessorCatalog::shipped(),
        config,
    );
    let start = placement
        .generate("minecraft:test", &context, TerrainAdjustment::None)
        .expect("start")
        .expect("valid start");
    let result = start
        .place_in_chunk(
            &mut level,
            BoundingBox::new(0, 0, 0, 15, 383, 15),
            &templates,
            StructureProcessorCatalog::shipped(),
            1,
        )
        .expect("places");
    assert!(result.written == 0);
    assert!(
        level.get_block(BlockPos::new(0, 63, 0)).is_none(),
        "structure_void replacement must not write a block"
    );
}

#[test]
fn desert_pyramid_generates_and_places_chest_markers() {
    use folia_core::worldgen::structure::start::DesertPyramidStart;

    let templates = HashMap::new();
    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(64);
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(0, 0), 0, &templates)
        .with_level(&level);
    let start = DesertPyramidStart::generate(
        "minecraft:desert_pyramid",
        &context,
        TerrainAdjustment::None,
    )
    .expect("desert pyramid start");
    assert_eq!(start.structure_id(), "minecraft:desert_pyramid");
    assert!(start.is_valid());
    assert_eq!(start.chunk_pos(), ChunkPos::new(0, 0));

    let placement = start
        .place_in_chunk(&mut level, ChunkPos::new(0, 0))
        .expect("desert pyramid places");
    assert_eq!(placement.placed_pieces, 1);
    assert!(placement.written > 0);
    assert!(
        level
            .blocks
            .values()
            .any(|state| state.name == "minecraft:sandstone"),
        "desert pyramid body must place sandstone"
    );
    let chest = placement
        .block_entities
        .iter()
        .find(|block| block.state.name == "minecraft:chest");
    let chest = chest.expect("desert pyramid chest marker");
    assert!(chest.nbt.as_ref().is_some_and(|nbt| {
        nbt.get("LootTable").and_then(Tag::as_str) == Some("minecraft:chests/desert_pyramid")
    }));
}
