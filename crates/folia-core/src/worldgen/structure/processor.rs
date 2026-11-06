//! Ports of `StructureProcessor`, `StructureProcessorList`, and the shipped
//! processor kinds.
//!
//! The catalog stores every shipped `worldgen/processor_list` document in
//! compact, sorted JSON. Parsing accepts both direct-codec shapes (a bare
//! array or `{ "processors": [...] }`) and all processor types registered in
//! 1.21.11: `block_ignore`, `block_rot`, `gravity`, `jigsaw_replacement`,
//! `rule`, `nop`, `block_age`, `blackstone_replace`,
//! `lava_submerged_block`, `protected_blocks`, and `capped`. Unknown processor,
//! rule-test, position-test, block-entity-modifier, and int-provider types fail
//! explicitly instead of being skipped.
//!
//! Application follows `StructureTemplate.processBlockInfos`: processors see
//! the template block before the placement mirror/rotation is applied, the
//! location predicate sees the existing world state, and the final state is
//! mirrored/rotated when it is written. `capped` runs in the finalize pass over
//! the surviving blocks and consumes Java's positional legacy random source.
//!
//! Not ported here: multi-palette random selection (callers use the first
//! palette, already a documented `template.rs` limitation), liquid
//! waterlogging, shape updates, and entity spawning.

use crate::chunk::{BlockState, is_air_state};
use crate::nbt::Tag;
use crate::registry::default_block_state;
use crate::tags;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::template::{StructurePlaceSettings, StructureTemplate, transform};
use folia_math::{Axis, BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource, get_seed};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

const TARGET_VERSION: &str = "1.21.11";
const TARGET_DATA_VERSION: i32 = 4788;
const SHIPPED_PROCESSORS: &str = include_str!("../../../assets/structure_processors.json");

/// A template block before processors run.
///
/// `local_pos` is Java's `blockInfo.pos()` (relative to the template) and
/// `pos` is `relativeBlockInfo.pos()` (already transformed and offset).
#[derive(Debug, Clone, PartialEq)]
pub struct RawBlockInfo {
    pub local_pos: BlockPos,
    pub pos: BlockPos,
    pub state: BlockState,
    pub nbt: Option<Tag>,
}

/// A block after the processor chain and finalize pass.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedBlock {
    pub pos: BlockPos,
    pub state: BlockState,
    pub nbt: Option<Tag>,
}

/// Values threaded through `StructureTemplate.processBlockInfos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorApplyContext {
    /// Level seed used by `CappedProcessor`'s positional random source.
    pub seed: i64,
    /// Java's `offset` argument (the piece/template origin).
    pub offset: BlockPos,
    /// Java's `pos` argument (the structure generation position).
    pub structure_pos: BlockPos,
}

impl ProcessorApplyContext {
    pub fn new(seed: i64, offset: BlockPos, structure_pos: BlockPos) -> Self {
        Self {
            seed,
            offset,
            structure_pos,
        }
    }
}

/// Header of the generated processor-list catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessorCatalogIndex {
    pub version: String,
    pub data_version: i32,
    pub total: usize,
    pub processors: usize,
    pub by_type: BTreeMap<String, usize>,
}

/// A parsed `StructureProcessorList`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureProcessorList {
    name: String,
    processors: Vec<StructureProcessor>,
}

impl StructureProcessorList {
    pub fn new(name: impl Into<String>, processors: Vec<StructureProcessor>) -> Self {
        Self {
            name: name.into(),
            processors,
        }
    }

    /// Parses a direct-codec processor list (`[...]` or `{"processors":[...]}`).
    pub fn from_value(name: &str, value: &Value) -> Result<Self, ProcessorError> {
        let entries = match value {
            Value::Array(entries) => entries.as_slice(),
            Value::Object(object) => object
                .get("processors")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ProcessorError::InvalidDefinition(format!(
                        "processor list `{name}` is missing the processors array"
                    ))
                })?
                .as_slice(),
            _ => {
                return Err(ProcessorError::InvalidDefinition(format!(
                    "processor list `{name}` must be an array or an object"
                )));
            }
        };

        let processors = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                StructureProcessor::from_json(entry).map_err(|error| {
                    ProcessorError::InvalidDefinition(format!(
                        "processor list `{name}` entry {index}: {error}"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name: name.to_owned(),
            processors,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn processors(&self) -> &[StructureProcessor] {
        &self.processors
    }
}

/// The embedded shipped processor-list catalog.
#[derive(Debug, Clone)]
pub struct StructureProcessorCatalog {
    index: ProcessorCatalogIndex,
    lists: BTreeMap<String, StructureProcessorList>,
}

static PROCESSOR_CATALOG: OnceLock<StructureProcessorCatalog> = OnceLock::new();

impl StructureProcessorCatalog {
    /// Parses a catalog in the generated asset format.
    pub fn from_json(text: &str) -> Result<Self, ProcessorError> {
        let value: Value = serde_json::from_str(text).map_err(|error| {
            ProcessorError::InvalidDefinition(format!("invalid processor catalog JSON: {error}"))
        })?;
        let root = value.as_object().ok_or_else(|| {
            ProcessorError::InvalidDefinition("processor catalog root must be an object".into())
        })?;

        let version = required_str(root, "version", "processor catalog")?.to_owned();
        if version != TARGET_VERSION {
            return Err(ProcessorError::InvalidDefinition(format!(
                "processor catalog version {version} does not match target {TARGET_VERSION}"
            )));
        }
        let data_version = required_i32(root, "data_version", "processor catalog")?;
        if data_version != TARGET_DATA_VERSION {
            return Err(ProcessorError::InvalidDefinition(format!(
                "processor catalog data version {data_version} does not match target \
                 {TARGET_DATA_VERSION}"
            )));
        }

        let counts = required_object(root, "counts", "processor catalog")?;
        let total = required_usize(counts, "total", "processor catalog counts")?;
        let processors = required_usize(counts, "processors", "processor catalog counts")?;
        let by_type_object = required_object(counts, "by_type", "processor catalog counts")?;
        let mut by_type = BTreeMap::new();
        for (kind, count) in by_type_object {
            by_type.insert(kind.clone(), usize_value(count, "processor type count")?);
        }

        let lists_object = required_object(root, "processor_lists", "processor catalog")?;
        let mut lists = BTreeMap::new();
        for (name, value) in lists_object {
            let name = normalized_id(name);
            lists.insert(
                name.clone(),
                StructureProcessorList::from_value(&name, value)?,
            );
        }

        if lists.len() != total {
            return Err(ProcessorError::InvalidDefinition(format!(
                "processor catalog has {} lists but index reports {total}",
                lists.len()
            )));
        }
        let parsed_processors: usize = lists.values().map(|list| list.processors.len()).sum();
        if parsed_processors != processors {
            return Err(ProcessorError::InvalidDefinition(format!(
                "processor catalog has {parsed_processors} processors but index reports \
                 {processors}"
            )));
        }
        let type_sum: usize = by_type.values().sum();
        if type_sum != processors {
            return Err(ProcessorError::InvalidDefinition(format!(
                "processor catalog by_type sum {type_sum} does not match {processors}"
            )));
        }

        Ok(Self {
            index: ProcessorCatalogIndex {
                version,
                data_version,
                total,
                processors,
                by_type,
            },
            lists,
        })
    }

    /// Loads the embedded 1.21.11 catalog once.
    pub fn shipped() -> &'static Self {
        PROCESSOR_CATALOG.get_or_init(|| {
            Self::from_json(SHIPPED_PROCESSORS).unwrap_or_else(|error| {
                panic!("embedded structure_processors.json is invalid: {error}")
            })
        })
    }

    pub fn index(&self) -> &ProcessorCatalogIndex {
        &self.index
    }

    pub fn list(&self, name: &str) -> Option<&StructureProcessorList> {
        self.lists.get(&normalized_id(name))
    }

    pub fn lists(&self) -> impl Iterator<Item = (&str, &StructureProcessorList)> {
        self.lists.iter().map(|(name, list)| (name.as_str(), list))
    }

    pub fn len(&self) -> usize {
        self.lists.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lists.is_empty()
    }
}

/// A set of block ids used by `block_rot`'s optional `rottable_blocks` field.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockSet {
    Tag(String),
    Blocks(Vec<String>),
}

impl BlockSet {
    pub fn matches(&self, state: &BlockState) -> bool {
        match self {
            BlockSet::Tag(tag) => tags::block_has(tag, &state.name),
            BlockSet::Blocks(blocks) => blocks.iter().any(|block| block == &state.name),
        }
    }
}

/// A `RuleTest` predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleTest {
    AlwaysTrue,
    BlockMatch { block: String },
    BlockStateMatch { state: BlockState },
    TagMatch { tag: String },
    RandomBlockMatch { block: String, probability: f32 },
    RandomBlockStateMatch { state: BlockState, probability: f32 },
}

impl RuleTest {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        let object = as_object(value, "rule test")?;
        let type_name = normalized_id(required_str(object, "predicate_type", "rule test")?);
        match type_name.as_str() {
            "minecraft:always_true" => Ok(RuleTest::AlwaysTrue),
            "minecraft:block_match" => Ok(RuleTest::BlockMatch {
                block: parse_block_name(required(object, "block", "block_match")?)?,
            }),
            "minecraft:blockstate_match" => Ok(RuleTest::BlockStateMatch {
                state: parse_block_state(required(object, "block_state", "blockstate_match")?)?,
            }),
            "minecraft:tag_match" => Ok(RuleTest::TagMatch {
                tag: strip_tag_prefix(required_str(object, "tag", "tag_match")?).to_owned(),
            }),
            "minecraft:random_block_match" => Ok(RuleTest::RandomBlockMatch {
                block: parse_block_name(required(object, "block", "random_block_match")?)?,
                probability: required_f32(object, "probability", "random_block_match")?,
            }),
            "minecraft:random_blockstate_match" => Ok(RuleTest::RandomBlockStateMatch {
                state: parse_block_state(required(
                    object,
                    "block_state",
                    "random_blockstate_match",
                )?)?,
                probability: required_f32(object, "probability", "random_blockstate_match")?,
            }),
            other => Err(ProcessorError::UnknownRuleTest {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn test(&self, state: &BlockState, random: &mut impl RandomSource) -> bool {
        match self {
            RuleTest::AlwaysTrue => true,
            RuleTest::BlockMatch { block } => &state.name == block,
            RuleTest::BlockStateMatch { state: expected } => state == expected,
            RuleTest::TagMatch { tag } => tags::block_has(tag, &state.name),
            RuleTest::RandomBlockMatch { block, probability } => {
                &state.name == block && random.next_float() < *probability
            }
            RuleTest::RandomBlockStateMatch {
                state: expected,
                probability,
            } => state == expected && random.next_float() < *probability,
        }
    }
}

/// A `PosRuleTest` predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum PositionTest {
    AlwaysTrue,
    Linear {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
    },
    AxisAlignedLinear {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
        axis: Axis,
    },
}

impl PositionTest {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        let object = as_object(value, "position test")?;
        let type_name = normalized_id(required_str(object, "predicate_type", "position test")?);
        match type_name.as_str() {
            "minecraft:always_true" => Ok(PositionTest::AlwaysTrue),
            "minecraft:linear_pos" => {
                let (min_chance, max_chance, min_dist, max_dist) = position_range(object)?;
                validate_position_range(min_dist, max_dist)?;
                Ok(PositionTest::Linear {
                    min_chance,
                    max_chance,
                    min_dist,
                    max_dist,
                })
            }
            "minecraft:axis_aligned_linear_pos" => {
                let (min_chance, max_chance, min_dist, max_dist) = position_range(object)?;
                validate_position_range(min_dist, max_dist)?;
                let axis = match object.get("axis").and_then(Value::as_str) {
                    None => Axis::Y,
                    Some("x") => Axis::X,
                    Some("y") => Axis::Y,
                    Some("z") => Axis::Z,
                    Some(other) => {
                        return Err(ProcessorError::InvalidDefinition(format!(
                            "unknown axis_aligned_linear_pos axis `{other}`"
                        )));
                    }
                };
                Ok(PositionTest::AxisAlignedLinear {
                    min_chance,
                    max_chance,
                    min_dist,
                    max_dist,
                    axis,
                })
            }
            other => Err(ProcessorError::UnknownPositionTest {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn test(
        &self,
        _local_pos: BlockPos,
        relative_pos: BlockPos,
        structure_pos: BlockPos,
        random: &mut impl RandomSource,
    ) -> bool {
        match self {
            PositionTest::AlwaysTrue => true,
            PositionTest::Linear {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
            } => {
                let distance = (relative_pos.x() - structure_pos.x()).abs()
                    + (relative_pos.y() - structure_pos.y()).abs()
                    + (relative_pos.z() - structure_pos.z()).abs();
                linear_chance(
                    distance,
                    *min_dist,
                    *max_dist,
                    *min_chance,
                    *max_chance,
                    random,
                )
            }
            PositionTest::AxisAlignedLinear {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
                axis,
            } => {
                let distance = match axis {
                    Axis::X => (relative_pos.x() - structure_pos.x()).abs(),
                    Axis::Y => (relative_pos.y() - structure_pos.y()).abs(),
                    Axis::Z => (relative_pos.z() - structure_pos.z()).abs(),
                };
                linear_chance(
                    distance,
                    *min_dist,
                    *max_dist,
                    *min_chance,
                    *max_chance,
                    random,
                )
            }
        }
    }
}

fn linear_chance(
    distance: i32,
    min_dist: i32,
    max_dist: i32,
    min_chance: f32,
    max_chance: f32,
    random: &mut impl RandomSource,
) -> bool {
    let inverse = (distance as f32 - min_dist as f32) / (max_dist as f32 - min_dist as f32);
    let chance = clamped_lerp(inverse, min_chance, max_chance);
    random.next_float() <= chance
}

fn clamped_lerp(start: f32, end: f32, delta: f32) -> f32 {
    if start < 0.0 {
        end
    } else if start > 1.0 {
        delta
    } else {
        end + start * (delta - end)
    }
}

fn position_range(object: &Map<String, Value>) -> Result<(f32, f32, i32, i32), ProcessorError> {
    Ok((
        object
            .get("min_chance")
            .map(|value| number_value(value, "min_chance"))
            .transpose()?
            .unwrap_or(0.0) as f32,
        object
            .get("max_chance")
            .map(|value| number_value(value, "max_chance"))
            .transpose()?
            .unwrap_or(0.0) as f32,
        object
            .get("min_dist")
            .map(|value| i32_value(value, "min_dist"))
            .transpose()?
            .unwrap_or(0),
        object
            .get("max_dist")
            .map(|value| i32_value(value, "max_dist"))
            .transpose()?
            .unwrap_or(0),
    ))
}

fn validate_position_range(min_dist: i32, max_dist: i32) -> Result<(), ProcessorError> {
    if min_dist >= max_dist {
        return Err(ProcessorError::InvalidDefinition(format!(
            "position test range must have min_dist < max_dist, got [{min_dist}, {max_dist}]"
        )));
    }
    Ok(())
}

/// A `RuleBlockEntityModifier`.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockEntityModifier {
    Passthrough,
    Clear,
    AppendStatic { data: Tag },
    AppendLoot { loot_table: String },
}

impl BlockEntityModifier {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        let object = as_object(value, "block entity modifier")?;
        let type_name = normalized_id(required_str(object, "type", "block entity modifier")?);
        match type_name.as_str() {
            "minecraft:passthrough" => Ok(BlockEntityModifier::Passthrough),
            "minecraft:clear" => Ok(BlockEntityModifier::Clear),
            "minecraft:append_static" => Ok(BlockEntityModifier::AppendStatic {
                data: json_to_nbt(required(object, "data", "append_static")?)?,
            }),
            "minecraft:append_loot" => Ok(BlockEntityModifier::AppendLoot {
                loot_table: normalized_id(required_str(object, "loot_table", "append_loot")?),
            }),
            other => Err(ProcessorError::UnknownBlockEntityModifier {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn apply(&self, random: &mut impl RandomSource, tag: Option<&Tag>) -> Option<Tag> {
        match self {
            BlockEntityModifier::Passthrough => tag.cloned(),
            BlockEntityModifier::Clear => Some(Tag::Compound(Vec::new())),
            BlockEntityModifier::AppendStatic { data } => Some(merge_compound(tag, data)),
            BlockEntityModifier::AppendLoot { loot_table } => {
                let mut entries = match tag {
                    Some(Tag::Compound(entries)) => entries.clone(),
                    _ => Vec::new(),
                };
                set_compound_entry(&mut entries, "LootTable", Tag::String(loot_table.clone()));
                set_compound_entry(&mut entries, "LootTableSeed", Tag::Long(random.next_long()));
                Some(Tag::Compound(entries))
            }
        }
    }
}

/// `ProcessorRule`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRule {
    pub input_predicate: RuleTest,
    pub location_predicate: RuleTest,
    pub position_predicate: PositionTest,
    pub output_state: BlockState,
    pub block_entity_modifier: BlockEntityModifier,
}

impl ProcessorRule {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        let object = as_object(value, "processor rule")?;
        Ok(Self {
            input_predicate: RuleTest::from_json(required(
                object,
                "input_predicate",
                "processor rule",
            )?)?,
            location_predicate: RuleTest::from_json(required(
                object,
                "location_predicate",
                "processor rule",
            )?)?,
            position_predicate: match object.get("position_predicate") {
                None => PositionTest::AlwaysTrue,
                Some(value) => PositionTest::from_json(value)?,
            },
            output_state: parse_block_state(required(object, "output_state", "processor rule")?)?,
            block_entity_modifier: match object.get("block_entity_modifier") {
                None => BlockEntityModifier::Passthrough,
                Some(value) => BlockEntityModifier::from_json(value)?,
            },
        })
    }
}

/// An `IntProvider` subset used by `capped`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntProvider {
    Constant(i32),
    Uniform {
        min_inclusive: i32,
        max_inclusive: i32,
    },
}

impl IntProvider {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        if let Some(value) = value.as_i64() {
            return i32::try_from(value)
                .map(IntProvider::Constant)
                .map_err(|_| {
                    ProcessorError::InvalidDefinition(format!(
                        "int provider {value} is outside the int range"
                    ))
                });
        }
        let object = as_object(value, "int provider")?;
        let type_name = normalized_id(required_str(object, "type", "int provider")?);
        match type_name.as_str() {
            "minecraft:constant" => Ok(IntProvider::Constant(required_i32(
                object,
                "value",
                "constant int provider",
            )?)),
            "minecraft:uniform" => {
                let min_inclusive = required_i32(object, "min_inclusive", "uniform int provider")?;
                let max_inclusive = required_i32(object, "max_inclusive", "uniform int provider")?;
                if min_inclusive > max_inclusive {
                    return Err(ProcessorError::InvalidDefinition(format!(
                        "uniform int provider has min {min_inclusive} > max {max_inclusive}"
                    )));
                }
                Ok(IntProvider::Uniform {
                    min_inclusive,
                    max_inclusive,
                })
            }
            other => Err(ProcessorError::UnknownIntProvider {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn max_value(&self) -> i32 {
        match self {
            IntProvider::Constant(value) => *value,
            IntProvider::Uniform { max_inclusive, .. } => *max_inclusive,
        }
    }

    pub fn sample(&self, random: &mut impl RandomSource) -> i32 {
        match self {
            IntProvider::Constant(value) => *value,
            IntProvider::Uniform {
                min_inclusive,
                max_inclusive,
            } => random.next_int_between_inclusive(*min_inclusive, *max_inclusive),
        }
    }
}

/// A registered `StructureProcessor`.
#[derive(Debug, Clone, PartialEq)]
pub enum StructureProcessor {
    BlockIgnore {
        blocks: Vec<String>,
    },
    BlockRot {
        integrity: f32,
        rottable_blocks: Option<BlockSet>,
    },
    Gravity {
        heightmap: Heightmap,
        offset: i32,
    },
    JigsawReplacement,
    Rule {
        rules: Vec<ProcessorRule>,
    },
    Nop,
    BlockAge {
        mossiness: f32,
    },
    BlackstoneReplace,
    LavaSubmergedBlock,
    ProtectedBlocks {
        tag: String,
    },
    Capped {
        delegate: Box<StructureProcessor>,
        limit: IntProvider,
    },
}

impl StructureProcessor {
    pub fn from_json(value: &Value) -> Result<Self, ProcessorError> {
        let object = as_object(value, "processor")?;
        let type_name = normalized_id(required_str(object, "processor_type", "processor")?);
        match type_name.as_str() {
            "minecraft:block_ignore" => {
                let entries = required_array(object, "blocks", "block_ignore")?;
                let blocks = entries
                    .iter()
                    .map(parse_block_name)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(StructureProcessor::BlockIgnore { blocks })
            }
            "minecraft:block_rot" => {
                let integrity = required_f32(object, "integrity", "block_rot")?;
                if !(0.0..=1.0).contains(&integrity) {
                    return Err(ProcessorError::InvalidDefinition(format!(
                        "block_rot integrity must be in 0..=1, got {integrity}"
                    )));
                }
                let rottable_blocks = match object.get("rottable_blocks") {
                    None => None,
                    Some(value) => Some(parse_block_set(value)?),
                };
                Ok(StructureProcessor::BlockRot {
                    integrity,
                    rottable_blocks,
                })
            }
            "minecraft:gravity" => {
                let heightmap = match object.get("heightmap").and_then(Value::as_str) {
                    None => Heightmap::WorldSurfaceWg,
                    Some(name) => parse_heightmap(name)?,
                };
                let offset = object
                    .get("offset")
                    .map(|value| i32_value(value, "gravity offset"))
                    .transpose()?
                    .unwrap_or(0);
                Ok(StructureProcessor::Gravity { heightmap, offset })
            }
            "minecraft:jigsaw_replacement" => Ok(StructureProcessor::JigsawReplacement),
            "minecraft:rule" => {
                let rules = required_array(object, "rules", "rule")?
                    .iter()
                    .enumerate()
                    .map(|(index, rule)| {
                        ProcessorRule::from_json(rule).map_err(|error| {
                            ProcessorError::InvalidDefinition(format!(
                                "rule processor entry {index}: {error}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(StructureProcessor::Rule { rules })
            }
            "minecraft:nop" => Ok(StructureProcessor::Nop),
            "minecraft:block_age" => Ok(StructureProcessor::BlockAge {
                mossiness: required_f32(object, "mossiness", "block_age")?,
            }),
            "minecraft:blackstone_replace" => Ok(StructureProcessor::BlackstoneReplace),
            "minecraft:lava_submerged_block" => Ok(StructureProcessor::LavaSubmergedBlock),
            "minecraft:protected_blocks" => Ok(StructureProcessor::ProtectedBlocks {
                tag: strip_tag_prefix(required_str(object, "value", "protected_blocks")?)
                    .to_owned(),
            }),
            "minecraft:capped" => Ok(StructureProcessor::Capped {
                delegate: Box::new(StructureProcessor::from_json(required(
                    object, "delegate", "capped",
                )?)?),
                limit: IntProvider::from_json(required(object, "limit", "capped")?)?,
            }),
            other => Err(ProcessorError::UnknownProcessorType {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            StructureProcessor::BlockIgnore { .. } => "minecraft:block_ignore",
            StructureProcessor::BlockRot { .. } => "minecraft:block_rot",
            StructureProcessor::Gravity { .. } => "minecraft:gravity",
            StructureProcessor::JigsawReplacement => "minecraft:jigsaw_replacement",
            StructureProcessor::Rule { .. } => "minecraft:rule",
            StructureProcessor::Nop => "minecraft:nop",
            StructureProcessor::BlockAge { .. } => "minecraft:block_age",
            StructureProcessor::BlackstoneReplace => "minecraft:blackstone_replace",
            StructureProcessor::LavaSubmergedBlock => "minecraft:lava_submerged_block",
            StructureProcessor::ProtectedBlocks { .. } => "minecraft:protected_blocks",
            StructureProcessor::Capped { .. } => "minecraft:capped",
        }
    }

    /// Programmatic constructors used by the template-pool placement path.
    pub fn block_ignore_blocks(blocks: impl IntoIterator<Item = String>) -> Self {
        StructureProcessor::BlockIgnore {
            blocks: blocks.into_iter().collect(),
        }
    }

    pub fn block_ignore_structure_block() -> Self {
        Self::block_ignore_blocks(["minecraft:structure_block".to_owned()])
    }

    pub fn block_ignore_structure_and_air() -> Self {
        Self::block_ignore_blocks([
            "minecraft:air".to_owned(),
            "minecraft:structure_block".to_owned(),
        ])
    }

    /// Java's `StructureProcessor.processBlock`.
    pub fn process_block(
        &self,
        level: &dyn FeatureLevel,
        context: &ProcessorApplyContext,
        original: &RawBlockInfo,
        current: &ProcessedBlock,
    ) -> Option<ProcessedBlock> {
        match self {
            StructureProcessor::BlockIgnore { blocks } => {
                if blocks.iter().any(|block| block == &current.state.name) {
                    None
                } else {
                    Some(current.clone())
                }
            }
            StructureProcessor::BlockRot {
                integrity,
                rottable_blocks,
            } => {
                let mut random = LegacyRandomSource::new(get_seed(
                    current.pos.x(),
                    current.pos.y(),
                    current.pos.z(),
                ));
                let rottable = rottable_blocks
                    .as_ref()
                    .is_none_or(|set| set.matches(&original.state));
                if rottable && random.next_float() > *integrity {
                    None
                } else {
                    Some(current.clone())
                }
            }
            StructureProcessor::Gravity { heightmap, offset } => {
                let height = level
                    .heightmap(*heightmap, current.pos.x(), current.pos.z())
                    .unwrap_or_else(|| level.min_y());
                let y = height
                    .wrapping_add(*offset)
                    .wrapping_add(original.local_pos.y());
                Some(ProcessedBlock {
                    pos: BlockPos::new(current.pos.x(), y, current.pos.z()),
                    state: current.state.clone(),
                    nbt: current.nbt.clone(),
                })
            }
            StructureProcessor::JigsawReplacement => {
                if current.state.name != "minecraft:jigsaw" {
                    return Some(current.clone());
                }
                let Some(nbt) = current.nbt.as_ref() else {
                    return Some(current.clone());
                };
                let final_state = nbt
                    .get("final_state")
                    .and_then(Tag::as_str)
                    .unwrap_or("minecraft:air");
                match parse_block_state_string(final_state) {
                    Ok(state) if state.name == "minecraft:structure_void" => None,
                    Ok(state) => Some(ProcessedBlock {
                        pos: current.pos,
                        state,
                        nbt: None,
                    }),
                    Err(_) => None,
                }
            }
            StructureProcessor::Rule { rules } => {
                let mut random = LegacyRandomSource::new(get_seed(
                    current.pos.x(),
                    current.pos.y(),
                    current.pos.z(),
                ));
                let existing = block_or_air(level, current.pos);
                for rule in rules {
                    if rule.input_predicate.test(&original.state, &mut random)
                        && rule.location_predicate.test(&existing, &mut random)
                        && rule.position_predicate.test(
                            original.local_pos,
                            current.pos,
                            context.structure_pos,
                            &mut random,
                        )
                    {
                        return Some(ProcessedBlock {
                            pos: current.pos,
                            state: rule.output_state.clone(),
                            nbt: rule
                                .block_entity_modifier
                                .apply(&mut random, current.nbt.as_ref()),
                        });
                    }
                }
                Some(current.clone())
            }
            StructureProcessor::Nop => Some(current.clone()),
            StructureProcessor::BlockAge { mossiness } => {
                let mut random = LegacyRandomSource::new(get_seed(
                    current.pos.x(),
                    current.pos.y(),
                    current.pos.z(),
                ));
                match aged_state(&current.state, *mossiness, &mut random) {
                    Some(state) => Some(ProcessedBlock {
                        pos: current.pos,
                        state,
                        nbt: current.nbt.clone(),
                    }),
                    None => Some(current.clone()),
                }
            }
            StructureProcessor::BlackstoneReplace => match blackstone_replacement(&current.state) {
                Some(state) => Some(ProcessedBlock {
                    pos: current.pos,
                    state,
                    nbt: current.nbt.clone(),
                }),
                None => Some(current.clone()),
            },
            StructureProcessor::LavaSubmergedBlock => {
                let existing = block_or_air(level, current.pos);
                if existing.name == "minecraft:lava" && !is_full_block_shape(&current.state) {
                    Some(ProcessedBlock {
                        pos: current.pos,
                        state: BlockState {
                            name: "minecraft:lava".to_owned(),
                            properties: Vec::new(),
                        },
                        nbt: current.nbt.clone(),
                    })
                } else {
                    Some(current.clone())
                }
            }
            StructureProcessor::ProtectedBlocks { tag } => {
                let existing = block_or_air(level, current.pos);
                if tags::block_has(tag, &existing.name) {
                    None
                } else {
                    Some(current.clone())
                }
            }
            // `capped` has no per-block behaviour; it runs in finalization.
            StructureProcessor::Capped { .. } => Some(current.clone()),
        }
    }

    /// Java's `StructureProcessor.finalizeProcessing`.
    pub fn finalize_processing(
        &self,
        level: &dyn FeatureLevel,
        context: &ProcessorApplyContext,
        originals: &[RawBlockInfo],
        processed: Vec<ProcessedBlock>,
    ) -> Vec<ProcessedBlock> {
        match self {
            StructureProcessor::Capped { delegate, limit } => {
                capped_finalize(delegate, limit, level, context, originals, processed)
            }
            _ => processed,
        }
    }
}

fn capped_finalize(
    delegate: &StructureProcessor,
    limit: &IntProvider,
    level: &dyn FeatureLevel,
    context: &ProcessorApplyContext,
    originals: &[RawBlockInfo],
    mut processed: Vec<ProcessedBlock>,
) -> Vec<ProcessedBlock> {
    if limit.max_value() == 0 || processed.is_empty() {
        return processed;
    }
    // Java logs and skips when the two lists are out of sync.
    if originals.len() != processed.len() {
        return processed;
    }

    let mut base = LegacyRandomSource::new(context.seed);
    let factory = base.fork_positional();
    let mut random = factory.at(context.offset.x(), context.offset.y(), context.offset.z());
    let limit = limit.sample(&mut random);
    if limit < 1 {
        return processed;
    }
    let limit = (limit as usize).min(processed.len());

    let mut indices: Vec<usize> = (0..processed.len()).collect();
    for index in (2..=indices.len()).rev() {
        let swap = random.next_int_bound(index as i32) as usize;
        indices.swap(index - 1, swap);
    }

    let mut changed = 0;
    for index in indices {
        if changed >= limit {
            break;
        }
        if let Some(next) =
            delegate.process_block(level, context, &originals[index], &processed[index])
            && next != processed[index]
        {
            processed[index] = next;
            changed += 1;
        }
    }
    processed
}

/// Builds Java's `blockInfos` list for one template placement.
///
/// Positions are transformed with the settings mirror/rotation and translated
/// by `origin`; states stay raw so processors can inspect them before the
/// placement rotation is applied.
pub fn template_block_infos(
    template: &StructureTemplate,
    settings: &StructurePlaceSettings,
    origin: BlockPos,
) -> Vec<RawBlockInfo> {
    let Some(palette) = template.palette(0) else {
        return Vec::new();
    };
    template
        .blocks()
        .iter()
        .filter_map(|block| {
            let state = palette.get(block.state)?.clone();
            if settings.ignore_air() && is_air_state(&state) {
                return None;
            }
            Some(RawBlockInfo {
                local_pos: block.pos,
                pos: origin.offset_by(transform(
                    block.pos,
                    settings.mirror(),
                    settings.rotation(),
                    settings.rotation_pivot(),
                )),
                state,
                nbt: block.nbt.clone(),
            })
        })
        .collect()
}

/// Runs the full processor chain plus finalize pass over `blocks`.
pub fn apply_processors(
    processors: &[StructureProcessor],
    level: &dyn FeatureLevel,
    context: &ProcessorApplyContext,
    blocks: &[RawBlockInfo],
) -> Vec<ProcessedBlock> {
    let mut originals = Vec::new();
    let mut processed = Vec::new();

    for block in blocks {
        let mut current = Some(ProcessedBlock {
            pos: block.pos,
            state: block.state.clone(),
            nbt: block.nbt.clone(),
        });
        for processor in processors {
            let Some(value) = current.take() else {
                break;
            };
            current = processor.process_block(level, context, block, &value);
        }
        if let Some(value) = current {
            originals.push(block.clone());
            processed.push(value);
        }
    }

    for processor in processors {
        processed = processor.finalize_processing(level, context, &originals, processed);
    }
    processed
}

/// Errors from processor-list parsing or application setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessorError {
    InvalidDefinition(String),
    UnknownProcessorType { type_name: String },
    UnknownRuleTest { type_name: String },
    UnknownPositionTest { type_name: String },
    UnknownBlockEntityModifier { type_name: String },
    UnknownIntProvider { type_name: String },
}

impl fmt::Display for ProcessorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessorError::InvalidDefinition(message) => formatter.write_str(message),
            ProcessorError::UnknownProcessorType { type_name } => {
                write!(formatter, "unknown structure processor type `{type_name}`")
            }
            ProcessorError::UnknownRuleTest { type_name } => {
                write!(formatter, "unknown rule-test type `{type_name}`")
            }
            ProcessorError::UnknownPositionTest { type_name } => {
                write!(formatter, "unknown position-test type `{type_name}`")
            }
            ProcessorError::UnknownBlockEntityModifier { type_name } => {
                write!(
                    formatter,
                    "unknown rule block-entity modifier `{type_name}`"
                )
            }
            ProcessorError::UnknownIntProvider { type_name } => {
                write!(formatter, "unknown int-provider type `{type_name}`")
            }
        }
    }
}

impl std::error::Error for ProcessorError {}

fn parse_block_set(value: &Value) -> Result<BlockSet, ProcessorError> {
    if let Some(tag) = value.as_str() {
        return Ok(BlockSet::Tag(strip_tag_prefix(tag).to_owned()));
    }
    let entries = value.as_array().ok_or_else(|| {
        ProcessorError::InvalidDefinition(
            "rottable_blocks must be a tag string or a block list".to_owned(),
        )
    })?;
    Ok(BlockSet::Blocks(
        entries
            .iter()
            .map(parse_block_name)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn parse_block_name(value: &Value) -> Result<String, ProcessorError> {
    if let Some(name) = value.as_str() {
        return Ok(normalized_id(name));
    }
    let object = as_object(value, "block state")?;
    Ok(normalized_id(required_str(object, "Name", "block state")?))
}

fn parse_block_state(value: &Value) -> Result<BlockState, ProcessorError> {
    if let Some(text) = value.as_str() {
        return parse_block_state_string(text);
    }
    let object = as_object(value, "block state")?;
    let name = normalized_id(required_str(object, "Name", "block state")?);
    let mut properties = Vec::new();
    if let Some(value) = object.get("Properties") {
        let properties_object = as_object(value, "block state properties")?;
        for (key, value) in properties_object {
            let value = value.as_str().ok_or_else(|| {
                ProcessorError::InvalidDefinition(format!(
                    "block state property `{key}` must be a string"
                ))
            })?;
            properties.push((key.clone(), value.to_owned()));
        }
    }
    properties.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(BlockState { name, properties })
}

fn parse_block_state_string(text: &str) -> Result<BlockState, ProcessorError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ProcessorError::InvalidDefinition(
            "block state string is empty".to_owned(),
        ));
    }
    let (name, properties) = match text.split_once('[') {
        None => (text, None),
        Some((name, rest)) => {
            let rest = rest.strip_suffix(']').ok_or_else(|| {
                ProcessorError::InvalidDefinition(format!(
                    "block state `{text}` is missing its closing bracket"
                ))
            })?;
            (name, Some(rest))
        }
    };
    let name = normalized_id(name.trim());
    let mut parsed_properties = Vec::new();
    if let Some(properties) = properties
        && !properties.trim().is_empty()
    {
        for pair in properties.split(',') {
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                ProcessorError::InvalidDefinition(format!(
                    "block state `{text}` has a malformed property `{pair}`"
                ))
            })?;
            parsed_properties.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    parsed_properties.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(BlockState {
        name,
        properties: parsed_properties,
    })
}

fn parse_heightmap(name: &str) -> Result<Heightmap, ProcessorError> {
    match name {
        "WORLD_SURFACE" => Ok(Heightmap::WorldSurface),
        "WORLD_SURFACE_WG" => Ok(Heightmap::WorldSurfaceWg),
        "OCEAN_FLOOR" => Ok(Heightmap::OceanFloor),
        "OCEAN_FLOOR_WG" => Ok(Heightmap::OceanFloorWg),
        "MOTION_BLOCKING" => Ok(Heightmap::MotionBlocking),
        "MOTION_BLOCKING_NO_LEAVES" => Ok(Heightmap::MotionBlockingNoLeaves),
        other => Err(ProcessorError::InvalidDefinition(format!(
            "unknown heightmap type `{other}`"
        ))),
    }
}

fn block_or_air(level: &dyn FeatureLevel, pos: BlockPos) -> BlockState {
    level.get_block(pos).cloned().unwrap_or_else(|| BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    })
}

fn is_full_block_shape(state: &BlockState) -> bool {
    if is_air_state(state) {
        return false;
    }
    for tag in [
        "minecraft:stairs",
        "minecraft:slabs",
        "minecraft:fences",
        "minecraft:fence_gates",
        "minecraft:walls",
        "minecraft:doors",
        "minecraft:trapdoors",
        "minecraft:carpets",
        "minecraft:buttons",
        "minecraft:pressure_plates",
        "minecraft:all_signs",
        "minecraft:rails",
        "minecraft:flowers",
        "minecraft:crops",
        "minecraft:saplings",
    ] {
        if tags::block_has(tag, &state.name) {
            return false;
        }
    }
    crate::block_behaviour::has_collision(state)
}

fn aged_state(
    state: &BlockState,
    mossiness: f32,
    random: &mut impl RandomSource,
) -> Option<BlockState> {
    if matches!(
        state.name.as_str(),
        "minecraft:stone_bricks" | "minecraft:stone" | "minecraft:chiseled_stone_bricks"
    ) {
        return maybe_replace_full_stone(random, mossiness);
    }
    if tags::block_has("minecraft:stairs", &state.name) {
        return maybe_replace_stairs(state, mossiness, random);
    }
    if tags::block_has("minecraft:slabs", &state.name) {
        return (random.next_float() < mossiness)
            .then(|| with_properties_of("minecraft:mossy_stone_brick_slab", state));
    }
    if tags::block_has("minecraft:walls", &state.name) {
        return (random.next_float() < mossiness)
            .then(|| with_properties_of("minecraft:mossy_stone_brick_wall", state));
    }
    if state.name == "minecraft:obsidian" {
        return (random.next_float() < 0.15).then(|| BlockState {
            name: "minecraft:crying_obsidian".to_owned(),
            properties: Vec::new(),
        });
    }
    None
}

fn maybe_replace_full_stone(random: &mut impl RandomSource, mossiness: f32) -> Option<BlockState> {
    if random.next_float() >= 0.5 {
        return None;
    }
    // Java constructs both arrays (and therefore draws both random stair
    // facings/halves) before choosing a mossy or normal array.
    let normal_stairs = random_facing_stairs(random, "minecraft:stone_brick_stairs");
    let mossy_stairs = random_facing_stairs(random, "minecraft:mossy_stone_brick_stairs");
    let normal = [
        BlockState {
            name: "minecraft:cracked_stone_bricks".to_owned(),
            properties: Vec::new(),
        },
        normal_stairs,
    ];
    let mossy = [
        BlockState {
            name: "minecraft:mossy_stone_bricks".to_owned(),
            properties: Vec::new(),
        },
        mossy_stairs,
    ];
    Some(random_block(random, &normal, &mossy, mossiness))
}

fn maybe_replace_stairs(
    state: &BlockState,
    mossiness: f32,
    random: &mut impl RandomSource,
) -> Option<BlockState> {
    if random.next_float() >= 0.5 {
        return None;
    }
    let normal = [
        BlockState {
            name: "minecraft:stone_slab".to_owned(),
            properties: Vec::new(),
        },
        BlockState {
            name: "minecraft:stone_brick_slab".to_owned(),
            properties: Vec::new(),
        },
    ];
    let mossy = [
        with_properties_of("minecraft:mossy_stone_brick_stairs", state),
        BlockState {
            name: "minecraft:mossy_stone_brick_slab".to_owned(),
            properties: Vec::new(),
        },
    ];
    Some(random_block(random, &normal, &mossy, mossiness))
}

fn random_block(
    random: &mut impl RandomSource,
    normal: &[BlockState; 2],
    mossy: &[BlockState; 2],
    mossiness: f32,
) -> BlockState {
    let states = if random.next_float() < mossiness {
        mossy
    } else {
        normal
    };
    states[random.next_int_bound(states.len() as i32) as usize].clone()
}

fn random_facing_stairs(random: &mut impl RandomSource, block: &str) -> BlockState {
    // Direction.Plane.HORIZONTAL is [north, east, south, west], Half.values()
    // is [top, bottom].
    const FACINGS: [Direction; 4] = [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ];
    const HALVES: [&str; 2] = ["top", "bottom"];
    let facing = FACINGS[random.next_int_bound(4) as usize];
    let half = HALVES[random.next_int_bound(2) as usize];
    let mut state = default_block_state(block).unwrap_or(BlockState {
        name: block.to_owned(),
        properties: Vec::new(),
    });
    set_property(&mut state, "facing", direction_name(facing));
    set_property(&mut state, "half", half);
    state
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

fn with_properties_of(block: &str, source: &BlockState) -> BlockState {
    let mut state = default_block_state(block).unwrap_or(BlockState {
        name: block.to_owned(),
        properties: Vec::new(),
    });
    for (key, value) in &source.properties {
        if state.properties.iter().any(|(name, _)| name == key) {
            set_property(&mut state, key, value);
        }
    }
    state
}

fn set_property(state: &mut BlockState, key: &str, value: &str) {
    if let Some(entry) = state.properties.iter_mut().find(|(name, _)| name == key) {
        entry.1 = value.to_owned();
    } else {
        state.properties.push((key.to_owned(), value.to_owned()));
        state
            .properties
            .sort_unstable_by(|left, right| left.0.cmp(&right.0));
    }
}

fn blackstone_replacement(state: &BlockState) -> Option<BlockState> {
    let target = match state.name.as_str() {
        "minecraft:cobblestone" | "minecraft:mossy_cobblestone" => "minecraft:blackstone",
        "minecraft:stone" => "minecraft:polished_blackstone",
        "minecraft:stone_bricks" | "minecraft:mossy_stone_bricks" => {
            "minecraft:polished_blackstone_bricks"
        }
        "minecraft:cobblestone_stairs" | "minecraft:mossy_cobblestone_stairs" => {
            "minecraft:blackstone_stairs"
        }
        "minecraft:stone_stairs" => "minecraft:polished_blackstone_stairs",
        "minecraft:stone_brick_stairs" | "minecraft:mossy_stone_brick_stairs" => {
            "minecraft:polished_blackstone_brick_stairs"
        }
        "minecraft:cobblestone_slab" | "minecraft:mossy_cobblestone_slab" => {
            "minecraft:blackstone_slab"
        }
        "minecraft:smooth_stone_slab" | "minecraft:stone_slab" => {
            "minecraft:polished_blackstone_slab"
        }
        "minecraft:stone_brick_slab" | "minecraft:mossy_stone_brick_slab" => {
            "minecraft:polished_blackstone_brick_slab"
        }
        "minecraft:stone_brick_wall" | "minecraft:mossy_stone_brick_wall" => {
            "minecraft:polished_blackstone_brick_wall"
        }
        "minecraft:cobblestone_wall" | "minecraft:mossy_cobblestone_wall" => {
            "minecraft:blackstone_wall"
        }
        "minecraft:chiseled_stone_bricks" => "minecraft:chiseled_polished_blackstone",
        "minecraft:cracked_stone_bricks" => "minecraft:cracked_polished_blackstone_bricks",
        "minecraft:iron_bars" => "minecraft:iron_chain",
        _ => return None,
    };
    let mut output = default_block_state(target).unwrap_or(BlockState {
        name: target.to_owned(),
        properties: Vec::new(),
    });
    for key in ["facing", "half", "type"] {
        if let Some((_, value)) = state.properties.iter().find(|(name, _)| name == key)
            && output.properties.iter().any(|(name, _)| name == key)
        {
            set_property(&mut output, key, value);
        }
    }
    Some(output)
}

fn merge_compound(base: Option<&Tag>, data: &Tag) -> Tag {
    let mut entries = match base {
        Some(Tag::Compound(entries)) => entries.clone(),
        _ => Vec::new(),
    };
    if let Tag::Compound(other) = data {
        merge_compound_entries(&mut entries, other);
    }
    Tag::Compound(entries)
}

fn merge_compound_entries(entries: &mut Vec<(String, Tag)>, other: &[(String, Tag)]) {
    for (key, value) in other {
        if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
            if let (Tag::Compound(left), Tag::Compound(right)) = (&mut *existing, value) {
                merge_compound_entries(left, right);
            } else {
                *existing = value.clone();
            }
        } else {
            entries.push((key.clone(), value.clone()));
        }
    }
}

fn set_compound_entry(entries: &mut Vec<(String, Tag)>, key: &str, value: Tag) {
    if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
        *existing = value;
    } else {
        entries.push((key.to_owned(), value));
    }
}

fn json_to_nbt(value: &Value) -> Result<Tag, ProcessorError> {
    match value {
        Value::Null => Err(ProcessorError::InvalidDefinition(
            "NBT value cannot be null".to_owned(),
        )),
        Value::Bool(value) => Ok(Tag::Byte(if *value { 1 } else { 0 })),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(match i32::try_from(value) {
                    Ok(value) => Tag::Int(value),
                    Err(_) => Tag::Long(value),
                });
            }
            number
                .as_f64()
                .map(Tag::Double)
                .ok_or_else(|| ProcessorError::InvalidDefinition("invalid NBT number".to_owned()))
        }
        Value::String(value) => Ok(Tag::String(value.clone())),
        Value::Array(values) => values
            .iter()
            .map(json_to_nbt)
            .collect::<Result<Vec<_>, _>>()
            .map(Tag::List),
        Value::Object(object) => {
            let mut entries = Vec::with_capacity(object.len());
            for (key, value) in object {
                entries.push((key.clone(), json_to_nbt(value)?));
            }
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Ok(Tag::Compound(entries))
        }
    }
}

fn normalized_id(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

fn strip_tag_prefix(tag: &str) -> &str {
    tag.strip_prefix('#').unwrap_or(tag)
}

fn as_object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, ProcessorError> {
    value
        .as_object()
        .ok_or_else(|| ProcessorError::InvalidDefinition(format!("{what} must be a JSON object")))
}

fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Value, ProcessorError> {
    object
        .get(key)
        .ok_or_else(|| ProcessorError::InvalidDefinition(format!("{what} is missing `{key}`")))
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a str, ProcessorError> {
    required(object, key, what)?.as_str().ok_or_else(|| {
        ProcessorError::InvalidDefinition(format!("{what} field `{key}` must be a string"))
    })
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Vec<Value>, ProcessorError> {
    required(object, key, what)?.as_array().ok_or_else(|| {
        ProcessorError::InvalidDefinition(format!("{what} field `{key}` must be an array"))
    })
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Map<String, Value>, ProcessorError> {
    required(object, key, what)?.as_object().ok_or_else(|| {
        ProcessorError::InvalidDefinition(format!("{what} field `{key}` must be an object"))
    })
}

fn required_i32(object: &Map<String, Value>, key: &str, what: &str) -> Result<i32, ProcessorError> {
    i32_value(required(object, key, what)?, key)
}

fn required_usize(
    object: &Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<usize, ProcessorError> {
    usize_value(required(object, key, what)?, what)
}

fn required_f32(object: &Map<String, Value>, key: &str, what: &str) -> Result<f32, ProcessorError> {
    number_value(required(object, key, what)?, key).map(|value| value as f32)
}

fn i32_value(value: &Value, what: &str) -> Result<i32, ProcessorError> {
    let value = value
        .as_i64()
        .ok_or_else(|| ProcessorError::InvalidDefinition(format!("{what} must be an integer")))?;
    i32::try_from(value).map_err(|_| {
        ProcessorError::InvalidDefinition(format!("{what} {value} is outside the int range"))
    })
}

fn usize_value(value: &Value, what: &str) -> Result<usize, ProcessorError> {
    let value = value.as_u64().ok_or_else(|| {
        ProcessorError::InvalidDefinition(format!("{what} must be a non-negative integer"))
    })?;
    usize::try_from(value)
        .map_err(|_| ProcessorError::InvalidDefinition(format!("{what} {value} is too large")))
}

fn number_value(value: &Value, what: &str) -> Result<f64, ProcessorError> {
    value
        .as_f64()
        .ok_or_else(|| ProcessorError::InvalidDefinition(format!("{what} must be a number")))
}
