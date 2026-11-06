use folia_core::chunk::BlockState;
use folia_core::math::{Axis, BlockPos};
use folia_core::nbt::Tag;
use folia_core::worldgen::feature::{FeatureLevel, Heightmap};
use folia_core::worldgen::structure::pool::{
    PoolElement, ProcessorListRef, Projection, StructureTemplatePool,
};
use folia_core::worldgen::structure::processor::{
    BlockEntityModifier, PositionTest, ProcessorRule, StructureProcessorCatalog,
};
use folia_core::worldgen::structure::template::{
    Rotation, StructurePlaceSettings, StructureTemplate,
};
use folia_core::worldgen::structure::{
    BoundingBox, PoolCatalog, PoolError, ProcessorApplyContext, StructureProcessor,
    apply_processors, template_block_infos,
};
use folia_random::LegacyRandomSource;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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
            fixed_height: None,
            blocks: HashMap::new(),
        }
    }

    fn with_block(mut self, pos: BlockPos, state: BlockState) -> Self {
        self.blocks.insert(pos, state);
        self
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

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn compound(entries: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn int_list(values: [i32; 3]) -> Tag {
    Tag::List(values.into_iter().map(Tag::Int).collect())
}

fn make_template(
    size: [i32; 3],
    palette: Vec<BlockState>,
    blocks: Vec<(BlockPos, usize, Option<Tag>)>,
) -> StructureTemplate {
    let palette_tag = Tag::List(
        palette
            .into_iter()
            .map(|state| {
                compound(vec![
                    ("Name", Tag::String(state.name)),
                    (
                        "Properties",
                        Tag::Compound(
                            state
                                .properties
                                .into_iter()
                                .map(|(key, value)| (key, Tag::String(value)))
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    let blocks_tag = Tag::List(
        blocks
            .into_iter()
            .map(|(pos, state_index, nbt)| {
                let mut entries = vec![
                    ("pos", int_list([pos.x(), pos.y(), pos.z()])),
                    ("state", Tag::Int(state_index as i32)),
                ];
                if let Some(nbt) = nbt {
                    entries.push(("nbt", nbt));
                }
                compound(entries)
            })
            .collect(),
    );
    let root = compound(vec![
        ("DataVersion", Tag::Int(4_788)),
        ("size", int_list(size)),
        ("palette", palette_tag),
        ("blocks", blocks_tag),
        ("entities", Tag::List(Vec::new())),
    ]);
    StructureTemplate::from_nbt(&root).expect("synthetic template parses")
}

fn one_block_template(block: BlockState) -> StructureTemplate {
    make_template([1, 1, 1], vec![block], vec![(BlockPos::ZERO, 0, None)])
}

fn parse_processor(value: Value) -> StructureProcessor {
    StructureProcessor::from_json(&value).expect("processor parses")
}

fn apply_one(
    processor: &StructureProcessor,
    template: &StructureTemplate,
    origin: BlockPos,
    level: &MockLevel,
) -> Vec<folia_core::worldgen::structure::ProcessedBlock> {
    apply_one_chain(std::slice::from_ref(processor), template, origin, level)
}

fn apply_one_chain(
    processors: &[StructureProcessor],
    template: &StructureTemplate,
    origin: BlockPos,
    level: &MockLevel,
) -> Vec<folia_core::worldgen::structure::ProcessedBlock> {
    let settings = StructurePlaceSettings::new();
    let raw_blocks = template_block_infos(template, &settings, origin);
    let context = ProcessorApplyContext::new(0, origin, origin);
    apply_processors(processors, level, &context, &raw_blocks)
}

fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn asset_text(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn canonical_json(value: &Value) -> String {
    let mut output = String::new();
    write_canonical(value, &mut output);
    output
}

fn write_canonical(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => {
            output.push_str(&serde_json::to_string(value).expect("string encodes"));
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(value, output);
            }
            output.push(']');
        }
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_unstable();
            output.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).expect("key encodes"));
                output.push(':');
                write_canonical(&object[key], output);
            }
            output.push('}');
        }
    }
}

#[test]
fn shipped_catalogs_have_expected_counts() {
    let processors = StructureProcessorCatalog::shipped();
    assert_eq!(processors.index().total, 40);
    assert_eq!(processors.index().processors, 52);
    assert_eq!(processors.len(), 40);
    assert_eq!(processors.index().by_type.get("minecraft:rule"), Some(&35));
    assert_eq!(
        processors.index().by_type.get("minecraft:block_rot"),
        Some(&6)
    );
    assert_eq!(
        processors.index().by_type.get("minecraft:protected_blocks"),
        Some(&7)
    );
    assert_eq!(processors.index().by_type.get("minecraft:capped"), Some(&4));

    let fossil_rot = processors.list("fossil_rot").expect("fossil_rot list");
    assert_eq!(fossil_rot.name(), "minecraft:fossil_rot");
    assert_eq!(fossil_rot.processors().len(), 2);
    assert_eq!(
        fossil_rot.processors()[0].type_name(),
        "minecraft:block_rot"
    );
    assert_eq!(
        fossil_rot.processors()[1].type_name(),
        "minecraft:protected_blocks"
    );

    let pools = PoolCatalog::shipped();
    assert_eq!(pools.index().pools, 188);
    assert_eq!(pools.index().entries, 1_198);
    assert_eq!(pools.index().elements, 1_204);
    assert_eq!(pools.index().templates, 4_880);
    assert_eq!(
        pools
            .index()
            .by_element_type
            .get("minecraft:legacy_single_pool_element"),
        Some(&603)
    );
    assert_eq!(
        pools
            .index()
            .by_element_type
            .get("minecraft:single_pool_element"),
        Some(&531)
    );
    assert_eq!(
        pools
            .index()
            .by_element_type
            .get("minecraft:feature_pool_element"),
        Some(&36)
    );
    assert_eq!(
        pools
            .index()
            .by_element_type
            .get("minecraft:list_pool_element"),
        Some(&3)
    );
    assert_eq!(
        pools
            .index()
            .by_element_type
            .get("minecraft:empty_pool_element"),
        Some(&31)
    );
    assert_eq!(pools.index().by_projection.get("rigid"), Some(&990));
    assert_eq!(
        pools.index().by_projection.get("terrain_matching"),
        Some(&183)
    );

    let town_centers = pools
        .pool("minecraft:village/plains/town_centers")
        .expect("town centers");
    assert_eq!(town_centers.size(), 204);
    assert_eq!(town_centers.entries().len(), 8);
    assert_eq!(town_centers.fallback(), "minecraft:empty");
    let PoolElement::LegacySingle {
        template,
        processors,
        projection,
        ..
    } = &town_centers.entries()[0].element
    else {
        panic!("town center entry must be legacy single");
    };
    assert_eq!(
        template,
        "minecraft:village/plains/town_centers/plains_fountain_01"
    );
    assert_eq!(
        processors,
        &ProcessorListRef::Named("minecraft:mossify_20_percent".to_owned())
    );
    assert_eq!(*projection, Projection::Rigid);
    assert!(matches!(
        &town_centers.entries()[2].element.processors(),
        Some(ProcessorListRef::Inline(list)) if list.processors().is_empty()
    ));

    let streets = pools
        .pool("minecraft:village/plains/streets")
        .expect("streets");
    assert_eq!(streets.entries().len(), 16);
    assert_eq!(streets.size(), 49);
    assert_eq!(streets.fallback(), "minecraft:village/plains/terminators");
    assert!(
        streets
            .entries()
            .iter()
            .all(|entry| entry.element.projection() == Projection::TerrainMatching)
    );

    let empty = pools.pool("minecraft:empty").expect("empty pool");
    assert_eq!(empty.size(), 0);
    assert!(
        empty
            .weighted_element(&mut LegacyRandomSource::new(0))
            .is_none()
    );

    let decor = pools
        .pool("minecraft:village/plains/decor")
        .expect("plains decor");
    assert!(decor.entries().iter().any(|entry| {
        matches!(
            entry.element.feature(),
            Some("minecraft:oak") | Some("minecraft:flower_plain")
        )
    }));

    let structures = pools
        .pool("minecraft:ancient_city/structures")
        .expect("ancient city structures");
    assert!(
        structures
            .entries()
            .iter()
            .any(|entry| matches!(&entry.element, PoolElement::List { .. }))
    );
}

#[test]
fn catalogs_are_canonical_and_deterministic() {
    for name in ["structure_processors.json", "template_pools.json"] {
        let text = asset_text(name);
        assert!(text.ends_with('\n'), "{name} must end with a newline");
        assert!(!text.contains("\r"), "{name} must use LF newlines");
        let value: Value = serde_json::from_str(&text).expect("catalog is valid JSON");
        assert_eq!(
            canonical_json(&value) + "\n",
            text,
            "{name} is not compact/sorted"
        );
    }
}

#[test]
fn block_ignore_drops_configured_blocks() {
    let template = make_template(
        [2, 1, 1],
        vec![state("minecraft:air", &[]), state("minecraft:stone", &[])],
        vec![(BlockPos::ZERO, 0, None), (BlockPos::new(1, 0, 0), 1, None)],
    );
    let processor = parse_processor(json!({
        "processor_type": "minecraft:block_ignore",
        "blocks": [{"Name": "minecraft:air"}]
    }));
    let output = apply_one(
        &processor,
        &template,
        BlockPos::ZERO,
        &MockLevel::new(0, 384),
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:stone");
    assert_eq!(output[0].pos, BlockPos::new(1, 0, 0));
}

#[test]
fn block_rot_uses_integrity_and_rottable_blocks() {
    let template = one_block_template(state("minecraft:deepslate_bricks", &[]));
    let level = MockLevel::new(0, 384);

    let kept = parse_processor(json!({
        "processor_type": "minecraft:block_rot",
        "integrity": 1.0,
        "rottable_blocks": "#minecraft:ancient_city_replaceable"
    }));
    assert_eq!(apply_one(&kept, &template, BlockPos::ZERO, &level).len(), 1);

    let rotted = parse_processor(json!({
        "processor_type": "minecraft:block_rot",
        "integrity": 0.0,
        "rottable_blocks": "#minecraft:ancient_city_replaceable"
    }));
    assert!(
        apply_one(&rotted, &template, BlockPos::ZERO, &level).is_empty(),
        "integrity 0.0 drops a rottable block at this fixed position"
    );

    let non_rottable = one_block_template(state("minecraft:stone", &[]));
    assert_eq!(
        apply_one(&rotted, &non_rottable, BlockPos::ZERO, &level).len(),
        1,
        "non-rottable blocks survive integrity 0.0"
    );
}

#[test]
fn gravity_snaps_blocks_to_the_requested_heightmap() {
    let template = make_template(
        [1, 3, 1],
        vec![state("minecraft:stone", &[])],
        vec![(BlockPos::new(0, 2, 0), 0, None)],
    );
    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(64);
    let processor = parse_processor(json!({
        "processor_type": "minecraft:gravity",
        "heightmap": "WORLD_SURFACE_WG",
        "offset": -1
    }));
    let output = apply_one(&processor, &template, BlockPos::new(100, 64, 200), &level);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].pos, BlockPos::new(100, 65, 200));
}

#[test]
fn jigsaw_replacement_parses_final_state_and_structure_void() {
    let nbt = compound(vec![(
        "final_state",
        Tag::String("minecraft:oak_stairs[facing=east,half=bottom]".to_owned()),
    )]);
    let template = make_template(
        [1, 1, 1],
        vec![state("minecraft:jigsaw", &[])],
        vec![(BlockPos::ZERO, 0, Some(nbt))],
    );
    let processor = parse_processor(json!({"processor_type": "minecraft:jigsaw_replacement"}));
    let output = apply_one(
        &processor,
        &template,
        BlockPos::ZERO,
        &MockLevel::new(0, 384),
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:oak_stairs");
    assert_eq!(property(&output[0].state, "facing"), Some("east"));
    assert_eq!(property(&output[0].state, "half"), Some("bottom"));
    assert!(output[0].nbt.is_none());

    let void_template = make_template(
        [1, 1, 1],
        vec![state("minecraft:jigsaw", &[])],
        vec![(
            BlockPos::ZERO,
            0,
            Some(compound(vec![(
                "final_state",
                Tag::String("minecraft:structure_void".to_owned()),
            )])),
        )],
    );
    assert!(
        apply_one(
            &processor,
            &void_template,
            BlockPos::ZERO,
            &MockLevel::new(0, 384)
        )
        .is_empty()
    );
}

#[test]
fn protected_blocks_keeps_cannot_replace_blocks() {
    let template = one_block_template(state("minecraft:stone", &[]));
    let protected = parse_processor(json!({
        "processor_type": "minecraft:protected_blocks",
        "value": "#minecraft:features_cannot_replace"
    }));

    let level = MockLevel::new(0, 384).with_block(BlockPos::ZERO, state("minecraft:bedrock", &[]));
    assert!(apply_one(&protected, &template, BlockPos::ZERO, &level).is_empty());

    let level = MockLevel::new(0, 384).with_block(BlockPos::ZERO, state("minecraft:stone", &[]));
    assert_eq!(
        apply_one(&protected, &template, BlockPos::ZERO, &level).len(),
        1
    );
}

#[test]
fn block_age_matches_java_draw_order() {
    let template = one_block_template(state("minecraft:stone_bricks", &[]));
    let level = MockLevel::new(0, 384);
    let origin = BlockPos::new(100, 64, 200);

    let normal = parse_processor(json!({
        "processor_type": "minecraft:block_age",
        "mossiness": 0.0
    }));
    let output = apply_one(&normal, &template, origin, &level);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:stone_brick_stairs");
    assert_eq!(property(&output[0].state, "facing"), Some("east"));
    assert_eq!(property(&output[0].state, "half"), Some("bottom"));

    let mossy = parse_processor(json!({
        "processor_type": "minecraft:block_age",
        "mossiness": 1.0
    }));
    let output = apply_one(&mossy, &template, origin, &level);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:mossy_stone_brick_stairs");
    assert_eq!(property(&output[0].state, "facing"), Some("south"));
    assert_eq!(property(&output[0].state, "half"), Some("top"));

    let obsidian = one_block_template(state("minecraft:obsidian", &[]));
    let output = apply_one(
        &parse_processor(json!({
            "processor_type": "minecraft:block_age",
            "mossiness": 0.0
        })),
        &obsidian,
        BlockPos::new(2, 1, 3),
        &level,
    );
    assert_eq!(output[0].state.name, "minecraft:crying_obsidian");
}

#[test]
fn blackstone_replacement_copies_facing_half_and_type() {
    let stairs = state(
        "minecraft:cobblestone_stairs",
        &[("facing", "west"), ("half", "top")],
    );
    let template = one_block_template(stairs);
    let processor = parse_processor(json!({"processor_type": "minecraft:blackstone_replace"}));
    let output = apply_one(
        &processor,
        &template,
        BlockPos::ZERO,
        &MockLevel::new(0, 384),
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:blackstone_stairs");
    assert_eq!(property(&output[0].state, "facing"), Some("west"));
    assert_eq!(property(&output[0].state, "half"), Some("top"));

    let bars = one_block_template(state("minecraft:iron_bars", &[]));
    let output = apply_one(&processor, &bars, BlockPos::ZERO, &MockLevel::new(0, 384));
    assert_eq!(output[0].state.name, "minecraft:iron_chain");
}

#[test]
fn lava_submerged_replaces_non_full_blocks_only() {
    let processor = parse_processor(json!({"processor_type": "minecraft:lava_submerged_block"}));
    let level = MockLevel::new(0, 384).with_block(BlockPos::ZERO, state("minecraft:lava", &[]));

    let air = one_block_template(state("minecraft:air", &[]));
    let output = apply_one(&processor, &air, BlockPos::ZERO, &level);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].state.name, "minecraft:lava");

    let stone = one_block_template(state("minecraft:stone", &[]));
    let output = apply_one(&processor, &stone, BlockPos::ZERO, &level);
    assert_eq!(output[0].state.name, "minecraft:stone");
}

#[test]
fn rule_processor_is_ordered_and_changes_block_entities() {
    let template = one_block_template(state("minecraft:stone", &[]));
    let level = MockLevel::new(0, 384);
    let ordered = parse_processor(json!({
        "processor_type": "minecraft:rule",
        "rules": [
            {
                "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
                "location_predicate": {"predicate_type": "minecraft:always_true"},
                "output_state": {"Name": "minecraft:gold_block"}
            },
            {
                "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
                "location_predicate": {"predicate_type": "minecraft:always_true"},
                "output_state": {"Name": "minecraft:diamond_block"}
            }
        ]
    }));
    let output = apply_one(&ordered, &template, BlockPos::ZERO, &level);
    assert_eq!(output[0].state.name, "minecraft:gold_block");

    let with_nbt = make_template(
        [1, 1, 1],
        vec![state("minecraft:stone", &[])],
        vec![(
            BlockPos::ZERO,
            0,
            Some(compound(vec![("Existing", Tag::Int(7))])),
        )],
    );
    let append_static = parse_processor(json!({
        "processor_type": "minecraft:rule",
        "rules": [{
            "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
            "location_predicate": {"predicate_type": "minecraft:always_true"},
            "output_state": {"Name": "minecraft:chest"},
            "block_entity_modifier": {
                "type": "minecraft:append_static",
                "data": {"CustomName": "loot", "Count": 3}
            }
        }]
    }));
    let output = apply_one(&append_static, &with_nbt, BlockPos::ZERO, &level);
    let nbt = output[0].nbt.as_ref().expect("merged block-entity nbt");
    assert_eq!(nbt.get("Existing"), Some(&Tag::Int(7)));
    assert_eq!(nbt.get("CustomName"), Some(&Tag::String("loot".to_owned())));
    assert_eq!(nbt.get("Count"), Some(&Tag::Int(3)));

    let append_loot = parse_processor(json!({
        "processor_type": "minecraft:rule",
        "rules": [{
            "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
            "location_predicate": {"predicate_type": "minecraft:always_true"},
            "output_state": {"Name": "minecraft:chest"},
            "block_entity_modifier": {
                "type": "minecraft:append_loot",
                "loot_table": "minecraft:chests/simple_dungeon"
            }
        }]
    }));
    let output = apply_one(&append_loot, &template, BlockPos::ZERO, &level);
    let nbt = output[0].nbt.as_ref().expect("loot nbt");
    assert_eq!(
        nbt.get("LootTable"),
        Some(&Tag::String("minecraft:chests/simple_dungeon".to_owned()))
    );
    assert_eq!(
        nbt.get("LootTableSeed"),
        Some(&Tag::Long(-4_962_768_465_676_381_896))
    );

    let clear = parse_processor(json!({
        "processor_type": "minecraft:rule",
        "rules": [{
            "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
            "location_predicate": {"predicate_type": "minecraft:always_true"},
            "output_state": {"Name": "minecraft:chest"},
            "block_entity_modifier": {"type": "minecraft:clear"}
        }]
    }));
    let output = apply_one(&clear, &with_nbt, BlockPos::ZERO, &level);
    assert_eq!(output[0].nbt, Some(Tag::Compound(Vec::new())));
}

#[test]
fn position_predicates_are_evaluated_in_rule_order() {
    let always = PositionTest::from_json(&json!({
        "predicate_type": "minecraft:linear_pos",
        "min_chance": 1.0,
        "max_chance": 1.0,
        "min_dist": 0,
        "max_dist": 10
    }))
    .expect("linear pos parses");
    let mut random = LegacyRandomSource::new(0);
    assert!(always.test(
        BlockPos::ZERO,
        BlockPos::new(5, 0, 0),
        BlockPos::ZERO,
        &mut random
    ));

    let never = PositionTest::from_json(&json!({
        "predicate_type": "minecraft:axis_aligned_linear_pos",
        "axis": "x",
        "min_chance": 0.0,
        "max_chance": 0.0,
        "min_dist": 0,
        "max_dist": 10
    }))
    .expect("axis aligned pos parses");
    let mut random = LegacyRandomSource::new(0);
    assert!(!never.test(
        BlockPos::ZERO,
        BlockPos::new(5, 0, 0),
        BlockPos::ZERO,
        &mut random
    ));

    let rule = ProcessorRule::from_json(&json!({
        "input_predicate": {"predicate_type": "minecraft:always_true"},
        "location_predicate": {"predicate_type": "minecraft:always_true"},
        "position_predicate": {
            "predicate_type": "minecraft:axis_aligned_linear_pos",
            "axis": "x",
            "min_chance": 1.0,
            "max_chance": 1.0,
            "min_dist": 0,
            "max_dist": 10
        },
        "output_state": {"Name": "minecraft:gold_block"}
    }))
    .expect("rule parses");
    assert_eq!(rule.position_predicate, always_position());

    let processor = StructureProcessor::Rule { rules: vec![rule] };
    let template = one_block_template(state("minecraft:stone", &[]));
    let output = apply_one(
        &processor,
        &template,
        BlockPos::new(3, 0, 0),
        &MockLevel::new(0, 384),
    );
    assert_eq!(output[0].state.name, "minecraft:gold_block");
}

fn always_position() -> PositionTest {
    PositionTest::AxisAlignedLinear {
        min_chance: 1.0,
        max_chance: 1.0,
        min_dist: 0,
        max_dist: 10,
        axis: Axis::X,
    }
}

#[test]
fn capped_processor_limits_finalized_changes() {
    let template = make_template(
        [4, 1, 1],
        vec![state("minecraft:stone", &[])],
        (0..4).map(|x| (BlockPos::new(x, 0, 0), 0, None)).collect(),
    );
    let processor = parse_processor(json!({
        "processor_type": "minecraft:capped",
        "limit": 2,
        "delegate": {
            "processor_type": "minecraft:rule",
            "rules": [{
                "input_predicate": {"predicate_type": "minecraft:block_match", "block": "minecraft:stone"},
                "location_predicate": {"predicate_type": "minecraft:always_true"},
                "output_state": {"Name": "minecraft:gold_block"}
            }]
        }
    }));
    let level = MockLevel::new(0, 384);
    let first = apply_one(&processor, &template, BlockPos::ZERO, &level);
    let changed: Vec<_> = first
        .iter()
        .filter(|block| block.state.name == "minecraft:gold_block")
        .map(|block| block.pos)
        .collect();
    assert_eq!(changed.len(), 2);
    let second = apply_one(&processor, &template, BlockPos::ZERO, &level);
    assert_eq!(first, second, "capped selection is deterministic");
}

#[test]
fn unknown_processor_and_element_types_fail_explicitly() {
    let processor_error = StructureProcessor::from_json(&json!({
        "processor_type": "minecraft:bogus_processor"
    }))
    .expect_err("unknown processor type must fail");
    assert!(
        processor_error
            .to_string()
            .contains("minecraft:bogus_processor"),
        "{processor_error}"
    );

    let element_error = PoolElement::from_json(&json!({
        "element_type": "minecraft:bogus_element",
        "projection": "rigid"
    }))
    .expect_err("unknown element type must fail");
    assert!(
        element_error
            .to_string()
            .contains("minecraft:bogus_element"),
        "{element_error}"
    );

    let predicate_error = folia_core::worldgen::structure::RuleTest::from_json(&json!({
        "predicate_type": "minecraft:bogus_predicate"
    }))
    .expect_err("unknown rule test must fail");
    assert!(
        predicate_error
            .to_string()
            .contains("minecraft:bogus_predicate"),
        "{predicate_error}"
    );

    let modifier_error = BlockEntityModifier::from_json(&json!({
        "type": "minecraft:bogus_modifier"
    }))
    .expect_err("unknown block entity modifier must fail");
    assert!(
        modifier_error
            .to_string()
            .contains("minecraft:bogus_modifier"),
        "{modifier_error}"
    );
}

#[test]
fn weighted_element_selection_matches_java_for_fixed_seed() {
    let pools = PoolCatalog::shipped();
    let pool = pools
        .pool("minecraft:village/plains/town_centers")
        .expect("town centers");
    let mut random = LegacyRandomSource::new(12_345);
    let sequence: Vec<_> = (0..10)
        .map(|_| {
            pool.weighted_element(&mut random)
                .expect("town centers are not empty")
                .template_location()
                .expect("town centers are templates")
                .to_owned()
        })
        .collect();
    assert_eq!(
        sequence,
        vec![
            "minecraft:village/plains/town_centers/plains_fountain_01".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_2".to_owned(),
            "minecraft:village/plains/zombie/town_centers/plains_meeting_point_1".to_owned(),
            "minecraft:village/plains/town_centers/plains_fountain_01".to_owned(),
            "minecraft:village/plains/town_centers/plains_fountain_01".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_3".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_1".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_2".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_3".to_owned(),
            "minecraft:village/plains/town_centers/plains_meeting_point_1".to_owned(),
        ]
    );

    let mut random = LegacyRandomSource::new(12_345);
    assert_eq!(
        pool.weighted_element(&mut random)
            .and_then(PoolElement::template_location),
        Some("minecraft:village/plains/town_centers/plains_fountain_01")
    );
}

fn test_template(size: [i32; 3], block: BlockPos) -> StructureTemplate {
    make_template(
        size,
        vec![state("minecraft:stone", &[])],
        vec![(block, 0, None)],
    )
}

fn test_pool_catalog() -> PoolCatalog {
    let json = json!({
        "counts": {
            "by_element_type": {"minecraft:single_pool_element": 2},
            "by_projection": {"rigid": 1, "terrain_matching": 1},
            "elements": 2,
            "entries": 2,
            "pools": 2,
            "templates": 2
        },
        "data_version": 4788,
        "template_pools": {
            "minecraft:test/rigid": {
                "elements": [{
                    "element": {
                        "element_type": "minecraft:single_pool_element",
                        "location": "minecraft:test/rigid",
                        "processors": {"processors": []},
                        "projection": "rigid"
                    },
                    "weight": 1
                }],
                "fallback": "minecraft:empty"
            },
            "minecraft:test/terrain": {
                "elements": [{
                    "element": {
                        "element_type": "minecraft:single_pool_element",
                        "location": "minecraft:test/terrain",
                        "processors": {"processors": []},
                        "projection": "terrain_matching"
                    },
                    "weight": 1
                }],
                "fallback": "minecraft:empty"
            }
        },
        "version": "1.21.11"
    });
    let text = serde_json::to_string(&json).expect("catalog encodes");
    PoolCatalog::from_json(&text).expect("synthetic pool catalog parses")
}

#[test]
fn start_pool_expansion_places_rigid_and_terrain_elements() {
    let catalog = test_pool_catalog();
    let mut templates = HashMap::new();
    templates.insert(
        "minecraft:test/rigid".to_owned(),
        test_template([1, 3, 1], BlockPos::new(0, 2, 0)),
    );
    templates.insert(
        "minecraft:test/terrain".to_owned(),
        test_template([1, 3, 1], BlockPos::new(0, 2, 0)),
    );

    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(80);
    let processors = StructureProcessorCatalog::shipped();
    let chunk_box = BoundingBox::new(0, 0, 0, 31, 255, 31);
    let position = BlockPos::new(10, 100, 10);

    let mut random = LegacyRandomSource::new(4_096);
    let rigid = catalog
        .expand_start(&templates, "minecraft:test/rigid", position, &mut random)
        .expect("rigid expansion")
        .expect("rigid element");
    assert_eq!(rigid.rotation, Rotation::None);
    assert_eq!(
        rigid.bounding_box,
        Some(BoundingBox::new(10, 100, 10, 10, 102, 10))
    );
    assert!(rigid.piece.is_some());
    let placement = rigid
        .place(
            &mut level, &templates, processors, chunk_box, position, 12_345, false,
        )
        .expect("rigid placement");
    assert_eq!(placement.written, 1);
    assert_eq!(placement.rejected, 0);
    assert_eq!(
        level
            .get_block(BlockPos::new(10, 102, 10))
            .map(|state| state.name.as_str()),
        Some("minecraft:stone")
    );

    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(80);
    let mut random = LegacyRandomSource::new(4_096);
    let terrain = catalog
        .expand_start(&templates, "minecraft:test/terrain", position, &mut random)
        .expect("terrain expansion")
        .expect("terrain element");
    assert_eq!(terrain.rotation, Rotation::None);
    let placement = terrain
        .place(
            &mut level, &templates, processors, chunk_box, position, 12_345, false,
        )
        .expect("terrain placement");
    assert_eq!(placement.written, 1);
    assert_eq!(
        level
            .get_block(BlockPos::new(10, 81, 10))
            .map(|state| state.name.as_str()),
        Some("minecraft:stone")
    );
    assert!(level.get_block(BlockPos::new(10, 102, 10)).is_none());
}

#[test]
fn feature_pool_elements_fail_explicitly_on_placement() {
    let json = json!({
        "counts": {
            "by_element_type": {"minecraft:feature_pool_element": 1},
            "by_projection": {"rigid": 1},
            "elements": 1,
            "entries": 1,
            "pools": 1,
            "templates": 1
        },
        "data_version": 4788,
        "template_pools": {
            "minecraft:test/feature": {
                "elements": [{
                    "element": {
                        "element_type": "minecraft:feature_pool_element",
                        "feature": "minecraft:oak",
                        "projection": "rigid"
                    },
                    "weight": 1
                }],
                "fallback": "minecraft:empty"
            }
        },
        "version": "1.21.11"
    });
    let catalog = PoolCatalog::from_json(&serde_json::to_string(&json).expect("encodes"))
        .expect("feature catalog parses");
    let templates: HashMap<String, StructureTemplate> = HashMap::new();
    let mut random = LegacyRandomSource::new(0);
    let expansion = catalog
        .expand_start(
            &templates,
            "minecraft:test/feature",
            BlockPos::new(1, 2, 3),
            &mut random,
        )
        .expect("feature expansion resolves")
        .expect("feature element");
    let error = expansion
        .place(
            &mut MockLevel::new(0, 384),
            &templates,
            StructureProcessorCatalog::shipped(),
            BoundingBox::new(0, 0, 0, 15, 255, 15),
            BlockPos::new(1, 2, 3),
            0,
            false,
        )
        .expect_err("feature placement is not implemented");
    assert!(matches!(
        error,
        PoolError::UnsupportedFeatureElement { ref feature, .. } if feature == "minecraft:oak"
    ));
}

#[test]
fn pool_expansion_fails_explicitly_for_missing_pools_templates_and_processor_lists() {
    let catalog = test_pool_catalog();
    let templates: HashMap<String, StructureTemplate> = HashMap::new();
    let mut random = LegacyRandomSource::new(0);
    let error = catalog
        .expand_start(
            &templates,
            "minecraft:test/missing",
            BlockPos::ZERO,
            &mut random,
        )
        .expect_err("missing pool must fail");
    assert!(matches!(error, PoolError::UnknownPool { .. }));

    let mut random = LegacyRandomSource::new(0);
    let error = catalog
        .expand_start(
            &templates,
            "minecraft:test/rigid",
            BlockPos::ZERO,
            &mut random,
        )
        .expect_err("missing template must fail");
    assert!(matches!(error, PoolError::UnknownTemplate { .. }));

    let mut templates = HashMap::new();
    templates.insert(
        "minecraft:test/rigid".to_owned(),
        test_template([1, 1, 1], BlockPos::ZERO),
    );
    let pool = StructureTemplatePool::from_value(
        "minecraft:test/processors",
        &json!({
            "elements": [{
                "element": {
                    "element_type": "minecraft:single_pool_element",
                    "location": "minecraft:test/rigid",
                    "processors": "minecraft:missing_processor_list",
                    "projection": "rigid"
                },
                "weight": 1
            }],
            "fallback": "minecraft:empty"
        }),
    )
    .expect("pool parses");
    assert!(matches!(
        pool.entries()[0].element.processors(),
        Some(ProcessorListRef::Named(name)) if name == "minecraft:missing_processor_list"
    ));
    // The missing reference is only resolved at placement time; use the real
    // catalog API with a one-pool catalog so the error path is exercised.
    let json = json!({
        "counts": {
            "by_element_type": {"minecraft:single_pool_element": 1},
            "by_projection": {"rigid": 1},
            "elements": 1,
            "entries": 1,
            "pools": 1,
            "templates": 1
        },
        "data_version": 4788,
        "template_pools": {"minecraft:test/processors": {
            "elements": [{
                "element": {
                    "element_type": "minecraft:single_pool_element",
                    "location": "minecraft:test/rigid",
                    "processors": "minecraft:missing_processor_list",
                    "projection": "rigid"
                },
                "weight": 1
            }],
            "fallback": "minecraft:empty"
        }},
        "version": "1.21.11"
    });
    let catalog = PoolCatalog::from_json(&serde_json::to_string(&json).expect("encodes"))
        .expect("catalog parses");
    let mut random = LegacyRandomSource::new(0);
    let expansion = catalog
        .expand_start(
            &templates,
            "minecraft:test/processors",
            BlockPos::ZERO,
            &mut random,
        )
        .expect("expansion resolves")
        .expect("element");
    let error = expansion
        .place(
            &mut MockLevel::new(0, 384),
            &templates,
            StructureProcessorCatalog::shipped(),
            BoundingBox::new(-8, 0, -8, 8, 384, 8),
            BlockPos::ZERO,
            0,
            false,
        )
        .expect_err("missing processor list must fail");
    assert!(matches!(error, PoolError::UnknownProcessorList { .. }));
}

#[test]
fn pool_element_variants_parse_projection_and_weights() {
    let pool = StructureTemplatePool::from_value(
        "minecraft:test/variants",
        &json!({
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:empty_pool_element"
                    },
                    "weight": 2
                },
                {
                    "element": {
                        "element_type": "minecraft:legacy_single_pool_element",
                        "location": "minecraft:test/legacy",
                        "processors": {"processors": []},
                        "projection": "rigid",
                        "override_liquid_settings": "ignore_waterlogging"
                    },
                    "weight": 3
                },
                {
                    "element": {
                        "element_type": "minecraft:list_pool_element",
                        "elements": [{
                            "element_type": "minecraft:single_pool_element",
                            "location": "minecraft:test/child",
                            "processors": {"processors": []},
                            "projection": "rigid"
                        }],
                        "projection": "terrain_matching"
                    },
                    "weight": 1
                }
            ],
            "fallback": "minecraft:test/fallback"
        }),
    )
    .expect("variant pool parses");

    assert_eq!(pool.size(), 6);
    assert_eq!(pool.fallback(), "minecraft:test/fallback");
    assert_eq!(
        pool.entries()[0].element.projection(),
        Projection::TerrainMatching
    );
    assert_eq!(pool.entries()[1].element.projection(), Projection::Rigid);
    assert_eq!(
        pool.entries()[2].element.projection(),
        Projection::TerrainMatching
    );
    let list = pool.entries()[2].element.children().expect("list children");
    assert_eq!(list[0].projection(), Projection::TerrainMatching);
    assert!(matches!(
        &pool.entries()[1].element,
        PoolElement::LegacySingle {
            liquid_settings: Some(
                folia_core::worldgen::structure::LiquidSettings::IgnoreWaterlogging
            ),
            ..
        }
    ));
}
