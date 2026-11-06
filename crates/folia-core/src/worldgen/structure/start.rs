//! Ports of `Structure`, `StructureStart`, and the igloo template structure.
//!
//! Structures are deterministic functions of `(seed, chunkPos)`: the generation
//! context seeds a `LegacyRandomSource` with `WorldgenRandom.setLargeFeatureSeed`
//! exactly like Java. Starts are data; they are not attached to the generator's
//! chunk-status pipeline in this slice.

use crate::region::ChunkPos;
use crate::worldgen::feature::{Decoration, FeatureLevel};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{
    BoundingBox, IglooPart, StructureDataMarker, StructurePiece, TemplateBlockFilter,
    TemplatePostProcess, TemplateStructurePiece,
};
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::template::{
    PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructureTemplate,
};
use folia_math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// `Structure.TerrainAdjustment`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TerrainAdjustment {
    #[default]
    None,
    BeardThin,
    BeardBox,
    Bury,
    Encapsulate,
}

impl TerrainAdjustment {
    pub const ALL: [TerrainAdjustment; 5] = [
        TerrainAdjustment::None,
        TerrainAdjustment::BeardThin,
        TerrainAdjustment::BeardBox,
        TerrainAdjustment::Bury,
        TerrainAdjustment::Encapsulate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TerrainAdjustment::None => "none",
            TerrainAdjustment::BeardThin => "beard_thin",
            TerrainAdjustment::BeardBox => "beard_box",
            TerrainAdjustment::Bury => "bury",
            TerrainAdjustment::Encapsulate => "encapsulate",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// `Structure.adjustBoundingBox` inflates every non-NONE adaptation by 12.
    pub fn inflates_bounding_box(self) -> bool {
        !matches!(self, TerrainAdjustment::None)
    }
}

/// `Structure.StructureSettings.biomes` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiomeSet {
    Tag(String),
    List(Vec<String>),
}

/// Structure codec kinds this slice can distinguish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureKind {
    Igloo,
    Jigsaw { start_pool: String, size: i32 },
    Unsupported { type_name: String },
}

/// Parsed `worldgen/structure/*.json` definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureDefinition {
    pub name: String,
    pub kind: StructureKind,
    pub biomes: BiomeSet,
    pub step: Decoration,
    pub terrain_adaptation: TerrainAdjustment,
}

impl StructureDefinition {
    /// Parses one structure JSON document. `name` is the resource id because
    /// vanilla structure JSON does not repeat its own registry key.
    pub fn from_json(name: &str, json: &str) -> Result<Self, StructureError> {
        let value: Value = serde_json::from_str(json).map_err(|error| {
            StructureError::InvalidDefinition(format!("{name}: invalid JSON: {error}"))
        })?;
        let root = value.as_object().ok_or_else(|| {
            StructureError::InvalidDefinition(format!("{name}: root must be an object"))
        })?;

        let type_name = required_str(root, "type", name)?;
        let biomes = parse_biomes(root, name)?;
        let step_name = required_str(root, "step", name)?;
        let step = Decoration::from_name(step_name).ok_or_else(|| {
            StructureError::InvalidDefinition(format!(
                "{name}: unknown generation step `{step_name}`"
            ))
        })?;
        let terrain_adaptation = match root.get("terrain_adaptation").and_then(Value::as_str) {
            None => TerrainAdjustment::None,
            Some(value) => TerrainAdjustment::from_name(value).ok_or_else(|| {
                StructureError::InvalidDefinition(format!(
                    "{name}: unknown terrain_adaptation `{value}`"
                ))
            })?,
        };

        let kind = match type_name {
            "minecraft:igloo" => StructureKind::Igloo,
            "minecraft:jigsaw" => {
                let start_pool = required_str(root, "start_pool", name)?;
                let size = root.get("size").and_then(Value::as_i64).unwrap_or(0);
                StructureKind::Jigsaw {
                    start_pool: start_pool.to_owned(),
                    size: i32::try_from(size).unwrap_or(0),
                }
            }
            other => StructureKind::Unsupported {
                type_name: other.to_owned(),
            },
        };

        Ok(Self {
            name: name.to_owned(),
            kind,
            biomes,
            step,
            terrain_adaptation,
        })
    }

    /// `Structure.generate`: deterministic start for this definition.
    pub fn generate_start(
        &self,
        context: &StructureGenerationContext<'_>,
    ) -> Result<StructureStart, StructureError> {
        match &self.kind {
            StructureKind::Igloo => {
                IglooStructure::generate(&self.name, context, self.terrain_adaptation)
            }
            StructureKind::Jigsaw { start_pool, size } => Err(StructureError::UnsupportedJigsaw {
                structure: self.name.clone(),
                start_pool: if *size > 0 {
                    format!("{start_pool} (size {size})")
                } else {
                    start_pool.clone()
                },
            }),
            StructureKind::Unsupported { type_name } => {
                Err(StructureError::UnsupportedStructureType {
                    structure: self.name.clone(),
                    type_name: type_name.clone(),
                })
            }
        }
    }
}

/// Template lookup used by structure generation. The manager is a trait so the
/// generator/asset layer can supply the shipped templates without this module
/// owning a new asset file.
pub trait TemplateProvider {
    fn template(&self, name: &str) -> Option<&StructureTemplate>;
}

impl TemplateProvider for HashMap<String, StructureTemplate> {
    fn template(&self, name: &str) -> Option<&StructureTemplate> {
        HashMap::get(self, name)
    }
}

/// `Structure.GenerationContext` subset needed by the supported structure.
#[derive(Clone, Copy)]
pub struct StructureGenerationContext<'a> {
    pub seed: i64,
    pub chunk_pos: ChunkPos,
    pub references: i32,
    pub templates: &'a dyn TemplateProvider,
}

impl<'a> StructureGenerationContext<'a> {
    pub fn new(
        seed: i64,
        chunk_pos: ChunkPos,
        references: i32,
        templates: &'a dyn TemplateProvider,
    ) -> Self {
        Self {
            seed,
            chunk_pos,
            references,
            templates,
        }
    }
}

/// Result of placing a start into one chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartPlacement {
    pub placed_pieces: usize,
    pub skipped_pieces: usize,
    pub written: usize,
    pub rejected: usize,
    pub block_entities: Vec<PlacedBlock>,
    pub entities: Vec<PlacedEntity>,
    pub data_markers: Vec<StructureDataMarker>,
}

/// `StructureStart`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureStart {
    structure_id: String,
    chunk_pos: ChunkPos,
    references: i32,
    pieces: Vec<StructurePiece>,
    terrain_adaptation: TerrainAdjustment,
}

impl StructureStart {
    pub fn new(
        structure_id: impl Into<String>,
        chunk_pos: ChunkPos,
        references: i32,
        pieces: Vec<StructurePiece>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Self {
        Self {
            structure_id: structure_id.into(),
            chunk_pos,
            references,
            pieces,
            terrain_adaptation,
        }
    }

    /// `StructureStart.INVALID_START`.
    pub fn invalid() -> Self {
        Self {
            structure_id: "INVALID".to_owned(),
            chunk_pos: ChunkPos::new(0, 0),
            references: 0,
            pieces: Vec::new(),
            terrain_adaptation: TerrainAdjustment::None,
        }
    }

    pub fn structure_id(&self) -> &str {
        &self.structure_id
    }

    pub fn chunk_pos(&self) -> ChunkPos {
        self.chunk_pos
    }

    pub fn pieces(&self) -> &[StructurePiece] {
        &self.pieces
    }

    pub fn references(&self) -> i32 {
        self.references
    }

    /// `StructureStart.getMaxReferences` is 1 for all vanilla starts.
    pub fn max_references(&self) -> i32 {
        1
    }

    /// `StructureStart.tryReference`.
    pub fn try_reference(&mut self) -> bool {
        if self.references < self.max_references() {
            self.references += 1;
            true
        } else {
            false
        }
    }

    /// `StructureStart.isValid`.
    pub fn is_valid(&self) -> bool {
        !self.pieces.is_empty()
    }

    /// `StructureStart.getBoundingBox` before `Structure.adjustBoundingBox`.
    pub fn pieces_bounding_box(&self) -> Option<BoundingBox> {
        BoundingBox::encapsulating(self.pieces.iter().map(StructurePiece::bounding_box))
    }

    /// `StructureStart.getBoundingBox`, including terrain-adaptation inflation.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        let bounding_box = self.pieces_bounding_box()?;
        if self.terrain_adaptation.inflates_bounding_box() {
            Some(bounding_box.inflated_by(12))
        } else {
            Some(bounding_box)
        }
    }

    /// `StructureStart.placeInChunk`.
    pub fn place_in_chunk(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_pos: ChunkPos,
    ) -> Result<StartPlacement, StructureError> {
        let min_x = chunk_pos.x << 4;
        let min_z = chunk_pos.z << 4;
        let max_y = level.min_y() + level.height() - 1;
        let chunk_box =
            BoundingBox::new(min_x, level.min_y(), min_z, min_x + 15, max_y, min_z + 15);
        self.place_in_box(level, chunk_box)
    }

    /// Placement with an explicit chunk/region box, useful for tests and for a
    /// later chunk-status pipeline that owns its own write box.
    pub fn place_in_box(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        let mut result = StartPlacement::default();
        for piece in &self.pieces {
            match piece.place(level, chunk_box)? {
                Some(placement) => {
                    result.placed_pieces += 1;
                    result.written += placement.written;
                    result.rejected += placement.rejected;
                    result.block_entities.extend(placement.block_entities);
                    result.entities.extend(placement.entities);
                    result.data_markers.extend(placement.data_markers);
                }
                None => result.skipped_pieces += 1,
            }
        }
        Ok(result)
    }
}

/// `IglooStructure` plus `IglooPieces`.
pub struct IglooStructure;

impl IglooStructure {
    /// `IglooPieces.GENERATION_HEIGHT`.
    pub const GENERATION_HEIGHT: i32 = 90;

    pub const TOP_TEMPLATE: &'static str = "minecraft:igloo/top";
    pub const MIDDLE_TEMPLATE: &'static str = "minecraft:igloo/middle";
    pub const BOTTOM_TEMPLATE: &'static str = "minecraft:igloo/bottom";

    /// Deterministic port of `IglooStructure.generatePieces` +
    /// `IglooPieces.addPieces`.
    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let top = template_for(context, structure_id, Self::TOP_TEMPLATE)?;
        let middle = template_for(context, structure_id, Self::MIDDLE_TEMPLATE)?;
        let bottom = template_for(context, structure_id, Self::BOTTOM_TEMPLATE)?;

        let chunk_pos = context.chunk_pos;
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut random, context.seed, chunk_pos.x, chunk_pos.z);

        // `IglooStructure.generatePieces`: rotation first, then
        // `IglooPieces.addPieces`.
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let start_pos = BlockPos::new(chunk_pos.x << 4, Self::GENERATION_HEIGHT, chunk_pos.z << 4);
        let mut pieces = Vec::new();

        if random.next_double() < 0.5 {
            let count = random.next_int_bound(8) + 4;
            pieces.push(igloo_piece(
                Self::BOTTOM_TEMPLATE,
                bottom,
                start_pos,
                IglooPart::Bottom,
                rotation,
                count * 3,
            ));
            for index in 0..count - 1 {
                pieces.push(igloo_piece(
                    Self::MIDDLE_TEMPLATE,
                    middle.clone(),
                    start_pos,
                    IglooPart::Middle,
                    rotation,
                    index * 3,
                ));
            }
        }

        pieces.push(igloo_piece(
            Self::TOP_TEMPLATE,
            top,
            start_pos,
            IglooPart::Top,
            rotation,
            0,
        ));

        Ok(StructureStart::new(
            structure_id,
            chunk_pos,
            context.references,
            pieces,
            terrain_adaptation,
        ))
    }
}

fn template_for(
    context: &StructureGenerationContext<'_>,
    structure_id: &str,
    template_name: &str,
) -> Result<StructureTemplate, StructureError> {
    context
        .templates
        .template(template_name)
        .cloned()
        .ok_or_else(|| StructureError::UnknownTemplate {
            structure: structure_id.to_owned(),
            template: template_name.to_owned(),
        })
}

fn igloo_piece(
    template_name: &str,
    template: StructureTemplate,
    start_pos: BlockPos,
    part: IglooPart,
    rotation: Rotation,
    down: i32,
) -> StructurePiece {
    let settings = StructurePlaceSettings::new()
        .with_rotation(rotation)
        .with_rotation_pivot(part.rotation_pivot());
    let position = start_pos.offset_by(part.offset()).below_n(down);
    let piece = TemplateStructurePiece::new(template_name, template, position, settings)
        .with_post_process(TemplatePostProcess::Igloo(part))
        .with_block_filter(TemplateBlockFilter::IgnoreStructureBlock);
    StructurePiece::template(piece)
}

fn parse_biomes(root: &Map<String, Value>, name: &str) -> Result<BiomeSet, StructureError> {
    let value = root
        .get("biomes")
        .ok_or_else(|| StructureError::InvalidDefinition(format!("{name}: missing `biomes`")))?;
    match value {
        Value::String(tag) => Ok(BiomeSet::Tag(tag.clone())),
        Value::Array(values) => {
            let list = values
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        StructureError::InvalidDefinition(format!(
                            "{name}: `biomes` entries must be strings"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(BiomeSet::List(list))
        }
        _ => Err(StructureError::InvalidDefinition(format!(
            "{name}: `biomes` must be a tag string or an array"
        ))),
    }
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
) -> Result<&'a str, StructureError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| StructureError::InvalidDefinition(format!("{name}: missing string `{key}`")))
}
