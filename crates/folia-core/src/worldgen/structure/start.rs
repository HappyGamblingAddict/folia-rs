//! Ports of `Structure`, `StructureStart`, and the igloo template structure.
//!
//! Structures are deterministic functions of `(seed, chunkPos)`: the generation
//! context seeds a `LegacyRandomSource` with `WorldgenRandom.setLargeFeatureSeed`
//! exactly like Java. Starts are data; they are not attached to the generator's
//! chunk-status pipeline in this slice.

use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::region::ChunkPos;
use crate::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::assets::StructureTemplateCatalog;
use crate::worldgen::structure::jigsaw::{JigsawConfig, JigsawPlacement, JigsawStart};
use crate::worldgen::structure::piece::{
    BoundingBox, IglooPart, StructureDataMarker, StructurePiece, TemplateBlockFilter,
    TemplatePostProcess, TemplateStructurePiece,
};
use crate::worldgen::structure::pool::{LiquidSettings, PoolCatalog};
use crate::worldgen::structure::processor::StructureProcessorCatalog;
use crate::worldgen::structure::set::{
    SelectedStructure, StructureBiomeLookup, StructureDimension, StructureSetError,
    StructureSetRegistry, set_large_feature_seed,
};
use crate::worldgen::structure::template::{
    PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructureTemplate,
};
use folia_math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource, WorldgenRandom};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

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
    DesertPyramid,
    Jigsaw {
        start_pool: String,
        start_jigsaw_name: Option<String>,
        size: i32,
        start_height: i32,
        use_expansion_hack: bool,
        project_start_to_heightmap: Option<Heightmap>,
        max_distance_horizontal: i32,
        max_distance_vertical: i32,
        dimension_padding_bottom: i32,
        dimension_padding_top: i32,
        liquid_settings: LiquidSettings,
    },
    Unsupported {
        type_name: String,
    },
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
            "minecraft:desert_pyramid" => StructureKind::DesertPyramid,
            "minecraft:jigsaw" => {
                if root.contains_key("pool_aliases")
                    && root
                        .get("pool_aliases")
                        .and_then(Value::as_array)
                        .is_some_and(|aliases| !aliases.is_empty())
                {
                    return Err(StructureError::UnsupportedPoolAliases {
                        structure: name.to_owned(),
                    });
                }
                let start_pool = required_str(root, "start_pool", name)?;
                let size = required_i32(root, "size", name)?;
                if !(0..=20).contains(&size) {
                    return Err(StructureError::InvalidDefinition(format!(
                        "{name}: jigsaw size must be in 0..=20, got {size}"
                    )));
                }
                let start_height = parse_start_height(root.get("start_height"), name)?;
                let use_expansion_hack = root
                    .get("use_expansion_hack")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let project_start_to_heightmap = match root
                    .get("project_start_to_heightmap")
                    .and_then(Value::as_str)
                {
                    None => None,
                    Some(value) => Some(parse_heightmap(value).ok_or_else(|| {
                        StructureError::InvalidDefinition(format!(
                            "{name}: unknown heightmap `{value}`"
                        ))
                    })?),
                };
                let (max_distance_horizontal, max_distance_vertical) =
                    parse_max_distance(root.get("max_distance_from_center"), name)?;
                let (dimension_padding_bottom, dimension_padding_top) =
                    parse_dimension_padding(root.get("dimension_padding"), name)?;
                let liquid_settings = match root.get("liquid_settings").and_then(Value::as_str) {
                    None => LiquidSettings::ApplyWaterlogging,
                    Some(value) => LiquidSettings::from_name(value).ok_or_else(|| {
                        StructureError::InvalidDefinition(format!(
                            "{name}: unknown liquid_settings `{value}`"
                        ))
                    })?,
                };
                StructureKind::Jigsaw {
                    start_pool: start_pool.to_owned(),
                    start_jigsaw_name: root
                        .get("start_jigsaw_name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    size,
                    start_height,
                    use_expansion_hack,
                    project_start_to_heightmap,
                    max_distance_horizontal,
                    max_distance_vertical,
                    dimension_padding_bottom,
                    dimension_padding_top,
                    liquid_settings,
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

    pub fn step(&self) -> Decoration {
        self.step
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
            StructureKind::Jigsaw {
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
            } => {
                let mut config = JigsawConfig::new(start_pool.clone(), *size, *start_height);
                config.start_jigsaw_name = start_jigsaw_name.clone();
                config.use_expansion_hack = *use_expansion_hack;
                config.project_start_to_heightmap = *project_start_to_heightmap;
                config.max_distance_horizontal = *max_distance_horizontal;
                config.max_distance_vertical = *max_distance_vertical;
                config.dimension_padding_bottom = *dimension_padding_bottom;
                config.dimension_padding_top = *dimension_padding_top;
                config.liquid_settings = *liquid_settings;
                let placement = JigsawPlacement::new(
                    PoolCatalog::shipped(),
                    context.templates,
                    StructureProcessorCatalog::shipped(),
                    config,
                );
                match placement.generate(&self.name, context, self.terrain_adaptation)? {
                    Some(jigsaw) => Ok(StructureStart::new_jigsaw(
                        &self.name,
                        context.chunk_pos,
                        context.references,
                        jigsaw,
                        self.terrain_adaptation,
                    )),
                    None => Ok(StructureStart::invalid()),
                }
            }
            StructureKind::DesertPyramid => {
                DesertPyramidStart::generate(&self.name, context, self.terrain_adaptation)
            }
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

/// `Structure.GenerationContext` subset needed by the supported structure.
#[derive(Clone, Copy)]
pub struct StructureGenerationContext<'a> {
    pub seed: i64,
    pub chunk_pos: ChunkPos,
    pub references: i32,
    pub templates: &'a dyn TemplateProvider,
    /// Level used for `getFirstFreeHeight`/dimension-padding checks during
    /// jigsaw start generation. `generate_start` called without one fails for
    /// jigsaw structures that need height sampling.
    pub height_source: Option<&'a dyn FeatureLevel>,
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
            height_source: None,
        }
    }

    pub fn with_level(mut self, level: &'a dyn FeatureLevel) -> Self {
        self.height_source = Some(level);
        self
    }
}

/// Result of placing a start into one chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartPlacement {
    pub placed_pieces: usize,
    pub skipped_pieces: usize,
    /// Template blocks plus feature-pool blocks that were written.
    pub written: usize,
    /// Template blocks rejected by the write box. Feature rejections are
    /// reported separately so template parity counts stay stable.
    pub rejected: usize,
    /// Feature-pool elements executed at their piece origin.
    pub feature_elements: usize,
    /// Feature-pool elements whose configured feature reported success.
    pub feature_placements: usize,
    pub feature_written: usize,
    pub feature_rejected: usize,
    /// Every block written by this start, sorted and deduplicated.
    pub written_blocks: Vec<BlockPos>,
    /// Blocks written by feature-pool elements, sorted and deduplicated.
    pub feature_blocks: Vec<BlockPos>,
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
    jigsaw: Option<JigsawStart>,
    desert_pyramid: Option<DesertPyramidPiece>,
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
            jigsaw: None,
            desert_pyramid: None,
            terrain_adaptation,
        }
    }

    pub fn new_jigsaw(
        structure_id: impl Into<String>,
        chunk_pos: ChunkPos,
        references: i32,
        jigsaw: JigsawStart,
        terrain_adaptation: TerrainAdjustment,
    ) -> Self {
        Self {
            structure_id: structure_id.into(),
            chunk_pos,
            references,
            pieces: Vec::new(),
            jigsaw: Some(jigsaw),
            desert_pyramid: None,
            terrain_adaptation,
        }
    }

    pub fn new_desert_pyramid(
        structure_id: impl Into<String>,
        chunk_pos: ChunkPos,
        references: i32,
        piece: DesertPyramidPiece,
        terrain_adaptation: TerrainAdjustment,
    ) -> Self {
        Self {
            structure_id: structure_id.into(),
            chunk_pos,
            references,
            pieces: Vec::new(),
            jigsaw: None,
            desert_pyramid: Some(piece),
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
            jigsaw: None,
            desert_pyramid: None,
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

    /// Jigsaw payload, if this is a jigsaw start.
    pub fn jigsaw(&self) -> Option<&JigsawStart> {
        self.jigsaw.as_ref()
    }

    pub fn jigsaw_pieces(&self) -> &[crate::worldgen::structure::jigsaw::JigsawPiece] {
        self.jigsaw
            .as_ref()
            .map_or(&[], crate::worldgen::structure::jigsaw::JigsawStart::pieces)
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
            || self.jigsaw.as_ref().is_some_and(JigsawStart::is_valid)
            || self.desert_pyramid.is_some()
    }

    /// `StructureStart.getBoundingBox` before `Structure.adjustBoundingBox`.
    pub fn pieces_bounding_box(&self) -> Option<BoundingBox> {
        let mut boxes: Vec<BoundingBox> = self
            .pieces
            .iter()
            .map(StructurePiece::bounding_box)
            .collect();
        if let Some(jigsaw) = &self.jigsaw
            && let Some(bounding_box) = jigsaw.pieces_bounding_box()
        {
            boxes.push(bounding_box);
        }
        if let Some(pyramid) = &self.desert_pyramid {
            boxes.push(pyramid.bounding_box());
        }
        BoundingBox::encapsulating(boxes)
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
        if self.jigsaw.is_some() {
            return Err(StructureError::JigsawRequiresTemplates {
                structure: self.structure_id.clone(),
            });
        }
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
        if let Some(pyramid) = &self.desert_pyramid {
            let placement = pyramid.place(level, chunk_box)?;
            if placement.placed_pieces > 0 {
                result.placed_pieces += placement.placed_pieces;
                result.written += placement.written;
                result.rejected += placement.rejected;
                result.block_entities.extend(placement.block_entities);
                result.data_markers.extend(placement.data_markers);
            } else {
                result.skipped_pieces += 1;
            }
        }
        Ok(result)
    }

    /// Places the jigsaw payload of this start clipped to one chunk box.
    ///
    /// Java's `StructureStart.placeInChunk` filters pieces by
    /// `getBoundingBox().intersects(box)` and passes the first piece's center
    /// as the processor position. Templates/processors are explicit because a
    /// start is data and owns no world-level template manager.
    pub fn place_jigsaw_in_chunk(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        world_seed: i64,
    ) -> Result<StartPlacement, StructureError> {
        match &self.jigsaw {
            Some(jigsaw) => {
                jigsaw.place_in_chunk(level, chunk_box, templates, processors, world_seed)
            }
            None => Ok(StartPlacement::default()),
        }
    }

    /// Places the jigsaw payload with an explicit structure placement RNG.
    ///
    /// Structure-level feature seeds are applied by [`StructureGenerator`];
    /// this keeps the per-chunk decoration stream shared by every piece.
    #[allow(clippy::too_many_arguments)]
    pub fn place_jigsaw_in_chunk_with_random(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        world_seed: i64,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        match &self.jigsaw {
            Some(jigsaw) => jigsaw.place_in_chunk_with_random(
                level, chunk_box, templates, processors, world_seed, random,
            ),
            None => Ok(StartPlacement::default()),
        }
    }
}

/// `DesertPyramidStructure` + `DesertPyramidPiece`.
pub struct DesertPyramidStart;

impl DesertPyramidStart {
    pub const WIDTH: i32 = 21;
    pub const DEPTH: i32 = 21;
    pub const HEIGHT: i32 = 15;
    pub const GENERATION_HEIGHT: i32 = 64;
    pub const SEA_LEVEL: i32 = 63;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let chunk = context.chunk_pos;
        let x = chunk.x << 4;
        let z = chunk.z << 4;
        if let Some(level) = context.height_source {
            let lowest = Self::lowest_ground(level, x, z);
            if lowest < Self::SEA_LEVEL {
                return Ok(StructureStart::invalid());
            }
        }

        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut random, context.seed, chunk.x, chunk.z);
        let directions = [
            folia_math::Direction::North,
            folia_math::Direction::East,
            folia_math::Direction::South,
            folia_math::Direction::West,
        ];
        let direction = directions[random.next_int_bound(4) as usize];
        let piece = DesertPyramidPiece::new(context.seed, x, z, direction);
        Ok(StructureStart::new_desert_pyramid(
            structure_id,
            chunk,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }

    fn lowest_ground(level: &dyn FeatureLevel, x: i32, z: i32) -> i32 {
        let mut lowest = level.min_y() + level.height();
        for (dx, dz) in [(0, 0), (0, 20), (20, 0), (20, 20)] {
            if let Some(height) = level.heightmap(Heightmap::MotionBlockingNoLeaves, x + dx, z + dz)
            {
                lowest = lowest.min(height);
            }
        }
        lowest
    }
}

/// `DesertPyramidPiece`. The body is a direct port of Java's `postProcess`;
/// only the positional random stream differs because this engine does not yet
/// expose vanilla's per-chunk decoration random to structure starts.
#[derive(Debug, Clone, PartialEq)]
pub struct DesertPyramidPiece {
    seed: i64,
    width: i32,
    height: i32,
    depth: i32,
    bounding_box: BoundingBox,
    direction: folia_math::Direction,
}

impl DesertPyramidPiece {
    pub fn new(seed: i64, x: i32, z: i32, direction: folia_math::Direction) -> Self {
        let width = DesertPyramidStart::WIDTH;
        let height = DesertPyramidStart::HEIGHT;
        let depth = DesertPyramidStart::DEPTH;
        Self {
            seed,
            width,
            height,
            depth,
            bounding_box: crate::worldgen::structure::piece::make_bounding_box(
                x,
                DesertPyramidStart::GENERATION_HEIGHT,
                z,
                direction,
                width,
                height,
                depth,
            ),
            direction,
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    /// `ScatteredFeaturePiece.updateHeightPositionToLowestGroundHeight` plus
    /// the whole `postProcess` body, clipped to `chunk_box`.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let mut pyramid = self.clone();
        let mut random = LegacyRandomSource::new(
            self.seed ^ ((chunk_box.min_x() as i64) << 32) ^ (chunk_box.min_z() as i64),
        );
        let height = -random.next_int_bound(3);
        if !pyramid.update_height_position_to_lowest_ground(level, height) {
            return Ok(StartPlacement::default());
        }

        let mut writer = PyramidWriter {
            pyramid: &mut pyramid,
            level,
            chunk_box,
            random,
            result: StartPlacement::default(),
        };
        writer.generate_body();
        writer.add_cellar();
        writer.after_place();
        let mut result = writer.result;
        result.placed_pieces = 1;
        Ok(result)
    }

    fn update_height_position_to_lowest_ground(
        &mut self,
        level: &dyn FeatureLevel,
        height: i32,
    ) -> bool {
        let mut lowest = level.min_y() + level.height();
        let mut found = false;
        for z in self.bounding_box.min_z()..=self.bounding_box.max_z() {
            for x in self.bounding_box.min_x()..=self.bounding_box.max_x() {
                if let Some(value) = level.heightmap(Heightmap::MotionBlockingNoLeaves, x, z) {
                    lowest = lowest.min(value);
                    found = true;
                }
            }
        }
        if !found {
            return false;
        }
        let delta = lowest - self.bounding_box.min_y() + height;
        self.bounding_box = self.bounding_box.moved(0, delta, 0);
        true
    }
}

struct PyramidWriter<'a> {
    pyramid: &'a mut DesertPyramidPiece,
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    random: LegacyRandomSource,
    result: StartPlacement,
}

impl PyramidWriter<'_> {
    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let max_z = self.pyramid.bounding_box.max_z();
        let min_z = self.pyramid.bounding_box.min_z();
        let max_x = self.pyramid.bounding_box.max_x();
        let min_x = self.pyramid.bounding_box.min_x();
        let (world_x, world_z) = match self.pyramid.direction {
            folia_math::Direction::North => (min_x + x, max_z - z),
            folia_math::Direction::South => (min_x + x, min_z + z),
            folia_math::Direction::West => (max_x - z, min_z + x),
            folia_math::Direction::East => (min_x + z, min_z + x),
            _ => (x, z),
        };
        BlockPos::new(world_x, y + self.pyramid.bounding_box.min_y(), world_z)
    }

    fn place_block(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return;
        }
        if self.level.set_block(pos, state) {
            self.result.written += 1;
        } else {
            self.result.rejected += 1;
        }
    }

    fn place_chest(&mut self, x: i32, y: i32, z: i32, index: usize) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return false;
        }
        if self
            .level
            .get_block(pos)
            .is_some_and(|state| state.name == "minecraft:chest")
        {
            return false;
        }
        let chest = BlockState {
            name: "minecraft:chest".to_owned(),
            properties: vec![("facing".to_owned(), "north".to_owned())],
        };
        if self.level.set_block(pos, chest.clone()) {
            self.result.written += 1;
            self.result.block_entities.push(PlacedBlock {
                pos,
                state: chest,
                nbt: Some(Tag::Compound(vec![
                    ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                    (
                        "LootTable".to_owned(),
                        Tag::String("minecraft:chests/desert_pyramid".to_owned()),
                    ),
                ])),
            });
            let _ = index;
            true
        } else {
            self.result.rejected += 1;
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_box(
        &mut self,
        min_x: i32,
        min_y: i32,
        min_z: i32,
        max_x: i32,
        max_y: i32,
        max_z: i32,
        boundary: &BlockState,
        inside: &BlockState,
        existing_only: bool,
    ) {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    if existing_only {
                        let pos = self.world_pos(x, y, z);
                        if !self.chunk_box.is_inside(pos)
                            || self
                                .level
                                .get_block(pos)
                                .is_some_and(|state| state.name == "minecraft:air")
                        {
                            continue;
                        }
                    }
                    let state = if x != min_x
                        && x != max_x
                        && y != min_y
                        && y != max_y
                        && z != min_z
                        && z != max_z
                    {
                        inside.clone()
                    } else {
                        boundary.clone()
                    };
                    self.place_block(state, x, y, z);
                }
            }
        }
    }

    fn fill_column_down(&mut self, state: &BlockState, x: i32, y: i32, z: i32, floor: i32) {
        let mut pos = self.world_pos(x, y, z);
        let min_y = self.level.min_y();
        while pos.y() > min_y + 1 {
            let replaceable = self
                .level
                .get_block(pos)
                .is_some_and(is_replaceable_by_structures);
            if !replaceable {
                break;
            }
            if self.chunk_box.is_inside(pos) && self.level.set_block(pos, state.clone()) {
                self.result.written += 1;
            }
            pos = pos.below();
        }
        let _ = floor;
    }

    fn generate_body(&mut self) {
        let width = self.pyramid.width;
        let depth = self.pyramid.depth;
        let sandstone = state("minecraft:sandstone");
        let air = state("minecraft:air");
        let cut = state("minecraft:cut_sandstone");
        let chiseled = state("minecraft:chiseled_sandstone");
        let orange = state("minecraft:orange_terracotta");
        let blue = state("minecraft:blue_terracotta");
        let sand = state("minecraft:sand");
        let tnt = state("minecraft:tnt");
        let stairs_north = stairs(folia_math::Direction::North);
        let stairs_south = stairs(folia_math::Direction::South);
        let stairs_east = stairs(folia_math::Direction::East);
        let stairs_west = stairs(folia_math::Direction::West);

        self.generate_box(
            0,
            -4,
            0,
            width - 1,
            0,
            depth - 1,
            &sandstone,
            &sandstone,
            false,
        );
        for i in 1..=9 {
            self.generate_box(
                i,
                i,
                i,
                width - 1 - i,
                i,
                depth - 1 - i,
                &sandstone,
                &sandstone,
                false,
            );
            self.generate_box(
                i + 1,
                i,
                i + 1,
                width - 2 - i,
                i,
                depth - 2 - i,
                &air,
                &air,
                false,
            );
        }
        for x in 0..width {
            for z in 0..depth {
                self.fill_column_down(&sandstone, x, -5, z, self.chunk_box.min_y());
            }
        }

        self.generate_box(0, 0, 0, 4, 9, 4, &sandstone, &air, false);
        self.generate_box(1, 10, 1, 3, 10, 3, &sandstone, &sandstone, false);
        self.place_block(sandstone.clone(), 2, 10, 0);
        self.place_block(sandstone.clone(), 2, 10, 4);
        self.place_block(sandstone.clone(), 0, 10, 2);
        self.place_block(sandstone.clone(), 4, 10, 2);
        self.generate_box(width - 5, 0, 0, width - 1, 9, 4, &sandstone, &air, false);
        self.generate_box(
            width - 4,
            10,
            1,
            width - 2,
            10,
            3,
            &sandstone,
            &sandstone,
            false,
        );
        self.place_block(sandstone.clone(), width - 3, 10, 0);
        self.place_block(sandstone.clone(), width - 3, 10, 4);
        self.place_block(sandstone.clone(), width - 5, 10, 2);
        self.place_block(sandstone.clone(), width - 1, 10, 2);
        self.generate_box(8, 0, 0, 12, 4, 4, &sandstone, &air, false);
        self.generate_box(9, 1, 0, 11, 3, 4, &air, &air, false);
        for (x, y, z) in [
            (9, 1, 1),
            (9, 2, 1),
            (9, 3, 1),
            (10, 3, 1),
            (11, 3, 1),
            (11, 2, 1),
            (11, 1, 1),
        ] {
            self.place_block(cut.clone(), x, y, z);
        }
        self.generate_box(4, 1, 1, 8, 3, 3, &sandstone, &air, false);
        self.generate_box(4, 1, 2, 8, 2, 2, &air, &air, false);
        self.generate_box(12, 1, 1, 16, 3, 3, &sandstone, &air, false);
        self.generate_box(12, 1, 2, 16, 2, 2, &air, &air, false);
        self.generate_box(
            5,
            4,
            5,
            width - 6,
            4,
            depth - 6,
            &sandstone,
            &sandstone,
            false,
        );
        self.generate_box(9, 4, 9, 11, 4, 11, &air, &air, false);
        for (x, z) in [(8, 8), (12, 8), (8, 12), (12, 12)] {
            self.generate_box(x, 1, z, x, 3, z, &cut, &cut, false);
        }
        self.generate_box(1, 1, 5, 4, 4, 11, &sandstone, &sandstone, false);
        self.generate_box(
            width - 5,
            1,
            5,
            width - 2,
            4,
            11,
            &sandstone,
            &sandstone,
            false,
        );
        self.generate_box(6, 7, 9, 6, 7, 11, &sandstone, &sandstone, false);
        self.generate_box(
            width - 7,
            7,
            9,
            width - 7,
            7,
            11,
            &sandstone,
            &sandstone,
            false,
        );
        self.generate_box(5, 5, 9, 5, 7, 11, &cut, &cut, false);
        self.generate_box(width - 6, 5, 9, width - 6, 7, 11, &cut, &cut, false);
        for (x, y) in [
            (5, 5),
            (5, 6),
            (6, 6),
            (width - 6, 5),
            (width - 6, 6),
            (width - 7, 6),
        ] {
            self.place_block(air.clone(), x, y, 10);
        }
        for x in [2, width - 3] {
            self.generate_box(x, 4, 4, x, 6, 4, &air, &air, false);
        }
        self.place_block(sandstone.clone(), 2, 4, 5);
        self.place_block(sandstone.clone(), 2, 3, 4);
        self.place_block(sandstone.clone(), width - 3, 4, 5);
        self.place_block(sandstone.clone(), width - 3, 3, 4);
        self.generate_box(1, 1, 3, 2, 2, 3, &sandstone, &sandstone, false);
        self.generate_box(
            width - 3,
            1,
            3,
            width - 2,
            2,
            3,
            &sandstone,
            &sandstone,
            false,
        );
        self.place_block(sandstone.clone(), 1, 1, 2);
        self.place_block(sandstone.clone(), width - 2, 1, 2);
        self.place_block(state("minecraft:sandstone_slab"), 1, 2, 2);
        self.place_block(state("minecraft:sandstone_slab"), width - 2, 2, 2);
        self.place_block(stairs_west.clone(), 2, 1, 2);
        self.place_block(stairs_east.clone(), width - 3, 1, 2);
        self.generate_box(4, 3, 5, 4, 3, 17, &sandstone, &sandstone, false);
        self.generate_box(
            width - 5,
            3,
            5,
            width - 5,
            3,
            17,
            &sandstone,
            &sandstone,
            false,
        );
        self.generate_box(3, 1, 5, 4, 2, 16, &air, &air, false);
        self.generate_box(width - 6, 1, 5, width - 5, 2, 16, &air, &air, false);
        for z in (5..=17).step_by(2) {
            self.place_block(cut.clone(), 4, 1, z);
            self.place_block(chiseled.clone(), 4, 2, z);
            self.place_block(cut.clone(), width - 5, 1, z);
            self.place_block(chiseled.clone(), width - 5, 2, z);
        }
        for (x, z) in [
            (10, 7),
            (10, 8),
            (9, 9),
            (11, 9),
            (8, 10),
            (12, 10),
            (7, 10),
            (13, 10),
            (9, 11),
            (11, 11),
            (10, 12),
            (10, 13),
        ] {
            self.place_block(orange.clone(), x, 0, z);
        }
        self.place_block(blue.clone(), 10, 0, 10);
        for x in [0, width - 1] {
            self.place_block(cut.clone(), x, 2, 1);
            self.place_block(orange.clone(), x, 2, 2);
            self.place_block(cut.clone(), x, 2, 3);
            self.place_block(cut.clone(), x, 3, 1);
            self.place_block(orange.clone(), x, 3, 2);
            self.place_block(cut.clone(), x, 3, 3);
            self.place_block(orange.clone(), x, 4, 1);
            self.place_block(chiseled.clone(), x, 4, 2);
            self.place_block(orange.clone(), x, 4, 3);
            self.place_block(cut.clone(), x, 5, 1);
            self.place_block(orange.clone(), x, 5, 2);
            self.place_block(cut.clone(), x, 5, 3);
            self.place_block(orange.clone(), x, 6, 1);
            self.place_block(chiseled.clone(), x, 6, 2);
            self.place_block(orange.clone(), x, 6, 3);
            self.place_block(orange.clone(), x, 7, 1);
            self.place_block(orange.clone(), x, 7, 2);
            self.place_block(orange.clone(), x, 7, 3);
            self.place_block(cut.clone(), x, 8, 1);
            self.place_block(cut.clone(), x, 8, 2);
            self.place_block(cut.clone(), x, 8, 3);
        }
        for x in [2, width - 3] {
            self.place_block(cut.clone(), x - 1, 2, 0);
            self.place_block(orange.clone(), x, 2, 0);
            self.place_block(cut.clone(), x + 1, 2, 0);
            self.place_block(cut.clone(), x - 1, 3, 0);
            self.place_block(orange.clone(), x, 3, 0);
            self.place_block(cut.clone(), x + 1, 3, 0);
            self.place_block(orange.clone(), x - 1, 4, 0);
            self.place_block(chiseled.clone(), x, 4, 0);
            self.place_block(orange.clone(), x + 1, 4, 0);
            self.place_block(cut.clone(), x - 1, 5, 0);
            self.place_block(orange.clone(), x, 5, 0);
            self.place_block(cut.clone(), x + 1, 5, 0);
            self.place_block(orange.clone(), x - 1, 6, 0);
            self.place_block(chiseled.clone(), x, 6, 0);
            self.place_block(orange.clone(), x + 1, 6, 0);
            self.place_block(orange.clone(), x - 1, 7, 0);
            self.place_block(orange.clone(), x, 7, 0);
            self.place_block(orange.clone(), x + 1, 7, 0);
            self.place_block(cut.clone(), x - 1, 8, 0);
            self.place_block(cut.clone(), x, 8, 0);
            self.place_block(cut.clone(), x + 1, 8, 0);
        }
        self.generate_box(8, 4, 0, 12, 6, 0, &cut, &cut, false);
        self.place_block(air.clone(), 8, 6, 0);
        self.place_block(air.clone(), 12, 6, 0);
        self.place_block(orange.clone(), 9, 5, 0);
        self.place_block(chiseled.clone(), 10, 5, 0);
        self.place_block(orange.clone(), 11, 5, 0);
        self.generate_box(8, -14, 8, 12, -11, 12, &cut, &cut, false);
        self.generate_box(8, -10, 8, 12, -10, 12, &chiseled, &chiseled, false);
        self.generate_box(8, -9, 8, 12, -9, 12, &cut, &cut, false);
        self.generate_box(8, -8, 8, 12, -1, 12, &sandstone, &sandstone, false);
        self.generate_box(9, -11, 9, 11, -1, 11, &air, &air, false);
        self.place_block(state("minecraft:stone_pressure_plate"), 10, -11, 10);
        self.generate_box(9, -13, 9, 11, -13, 11, &tnt, &air, false);
        for (x, y, z) in [
            (8, -11, 10),
            (8, -10, 10),
            (12, -11, 10),
            (12, -10, 10),
            (10, -11, 8),
            (10, -10, 8),
            (10, -11, 12),
            (10, -10, 12),
        ] {
            self.place_block(air.clone(), x, y, z);
        }
        self.place_block(chiseled.clone(), 7, -10, 10);
        self.place_block(cut.clone(), 7, -11, 10);
        self.place_block(chiseled.clone(), 13, -10, 10);
        self.place_block(cut.clone(), 13, -11, 10);
        self.place_block(chiseled.clone(), 10, -10, 7);
        self.place_block(cut.clone(), 10, -11, 7);
        self.place_block(chiseled.clone(), 10, -10, 13);
        self.place_block(cut.clone(), 10, -11, 13);
        let _ = stairs_north;
        let _ = stairs_south;

        for (index, direction) in [
            folia_math::Direction::North,
            folia_math::Direction::East,
            folia_math::Direction::South,
            folia_math::Direction::West,
        ]
        .into_iter()
        .enumerate()
        {
            let x = 10 + direction.step_x() * 2;
            let z = 10 + direction.step_z() * 2;
            if self.place_chest(x, -11, z, index) && index > 3 {
                break;
            }
        }
        let _ = sand;
    }

    fn add_cellar(&mut self) {
        self.add_cellar_stairs(BlockPos::new(16, -4, 13));
        self.add_cellar_room(BlockPos::new(16, -4, 13));
    }

    fn add_cellar_stairs(&mut self, pos: BlockPos) {
        let x = pos.x();
        let y = pos.y();
        let z = pos.z();
        let cnt90 = stairs_rotated(folia_math::Direction::South, Rotation::CounterClockwise90);
        self.place_block(cnt90.clone(), 13, -1, 17);
        self.place_block(cnt90.clone(), 14, -2, 17);
        self.place_block(cnt90, 15, -3, 17);
        let sand = state("minecraft:sand");
        let sandstone = state("minecraft:sandstone");
        let random_boolean = self.random.next_boolean();
        for (dx, dy) in [(-4, 4), (-3, 4), (-2, 4), (-1, 4), (0, 4)] {
            self.place_block(sand.clone(), x + dx, y + dy, z + 4);
        }
        self.place_block(sand.clone(), x - 2, y + 3, z + 4);
        self.place_block(
            if random_boolean {
                sand.clone()
            } else {
                sandstone.clone()
            },
            x - 1,
            y + 3,
            z + 4,
        );
        self.place_block(
            if random_boolean {
                sandstone.clone()
            } else {
                sand.clone()
            },
            x,
            y + 3,
            z + 4,
        );
        self.place_block(sand, x - 1, y + 2, z + 4);
        self.place_block(sandstone, x, y + 2, z + 4);
        self.place_block(state("minecraft:sand"), x, y + 1, z + 4);
        let _ = z;
    }

    fn add_cellar_room(&mut self, pos: BlockPos) {
        let x = pos.x();
        let y = pos.y();
        let z = pos.z();
        let cut = state("minecraft:cut_sandstone");
        let chiseled = state("minecraft:chiseled_sandstone");
        let orange = state("minecraft:orange_terracotta");
        let blue = state("minecraft:blue_terracotta");
        for (ay, by) in [(1, 1), (2, 2)] {
            self.generate_box(x - 3, y + ay, z - 3, x - 3, y + ay, z + 2, &cut, &cut, true);
            self.generate_box(x + 3, y + ay, z - 3, x + 3, y + ay, z + 2, &cut, &cut, true);
            self.generate_box(x - 3, y + ay, z - 3, x + 3, y + ay, z - 2, &cut, &cut, true);
            self.generate_box(x - 3, y + ay, z + 3, x + 3, y + ay, z + 3, &cut, &cut, true);
            let _ = by;
        }
        self.generate_box(x - 3, -1, z - 3, x - 3, -1, z + 2, &cut, &cut, true);
        self.generate_box(x + 3, -1, z - 3, x + 3, -1, z + 2, &cut, &cut, true);
        self.generate_box(x - 3, -1, z - 3, x + 3, -1, z - 2, &cut, &cut, true);
        self.generate_box(x - 3, -1, z + 3, x + 3, -1, z + 3, &cut, &cut, true);
        for y2 in y + 1..=y + 3 {
            for x2 in x - 2..=x + 2 {
                for z2 in z - 2..=z + 2 {
                    self.place_sand(x2, y2, z2);
                }
            }
        }
        self.place_collapsed_roof(x - 2, y + 4, z - 2, x + 2, z + 2);
        self.place_block(blue.clone(), x, y, z);
        for (dx, dz) in [
            (1, -1),
            (1, 1),
            (-1, -1),
            (-1, 1),
            (2, 0),
            (-2, 0),
            (0, 2),
            (0, -2),
        ] {
            self.place_block(orange.clone(), x + dx, y, z + dz);
        }
        self.place_block(orange.clone(), x + 3, y, z);
        self.place_sand(x + 3, y + 1, z);
        self.place_sand(x + 3, y + 2, z);
        self.place_block(cut.clone(), x + 4, y + 1, z);
        self.place_block(chiseled.clone(), x + 4, y + 2, z);
        self.place_block(orange.clone(), x - 3, y, z);
        self.place_sand(x - 3, y + 1, z);
        self.place_sand(x - 3, y + 2, z);
        self.place_block(cut.clone(), x - 4, y + 1, z);
        self.place_block(chiseled.clone(), x - 4, y + 2, z);
        self.place_block(orange.clone(), x, y, z + 3);
        self.place_sand(x, y + 1, z + 3);
        self.place_sand(x, y + 2, z + 3);
        self.place_block(orange, x, y, z - 3);
        self.place_sand(x, y + 1, z - 3);
        self.place_sand(x, y + 2, z - 3);
        self.place_block(cut, x, y + 1, z - 4);
        self.place_block(chiseled, x, -2, z - 4);
    }

    fn place_sand(&mut self, x: i32, y: i32, z: i32) {
        let pos = self.world_pos(x, y, z);
        self.result.data_markers.push(StructureDataMarker {
            name: "potential_suspicious_sand".to_owned(),
            pos,
            nbt: None,
        });
    }

    fn place_collapsed_roof(&mut self, min_x: i32, y: i32, min_z: i32, max_x: i32, max_z: i32) {
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let state = if self.random.next_float() < 0.33 {
                    state("minecraft:sandstone")
                } else {
                    state("minecraft:sand")
                };
                self.place_block(state, x, y, z);
            }
        }
        let mut positional = LegacyRandomSource::new(
            self.pyramid.seed ^ ((min_x as i64) << 32) ^ (min_z as i64) ^ (y as i64),
        );
        let random_x = positional.next_int_between_inclusive(min_x, max_x);
        let random_z = positional.next_int_between_inclusive(min_z, max_z);
        let pos = self.world_pos(random_x, y, random_z);
        self.result.data_markers.push(StructureDataMarker {
            name: "collapsed_roof".to_owned(),
            pos,
            nbt: None,
        });
    }

    fn after_place(&mut self) {
        let mut suspicious: Vec<BlockPos> = self
            .result
            .data_markers
            .iter()
            .filter(|marker| marker.name == "potential_suspicious_sand")
            .map(|marker| marker.pos)
            .filter(|pos| self.chunk_box.is_inside(*pos))
            .collect();
        suspicious.sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        suspicious.dedup();
        let collapsed = self
            .result
            .data_markers
            .iter()
            .find(|marker| marker.name == "collapsed_roof")
            .map(|marker| marker.pos);
        if let Some(pos) = collapsed
            && self.chunk_box.is_inside(pos)
        {
            self.place_suspicious_sand(pos);
        }
        let count = suspicious
            .len()
            .min(self.random.next_int_between_inclusive(5, 7) as usize);
        for (index, pos) in suspicious.into_iter().enumerate() {
            if index < count {
                self.place_suspicious_sand(pos);
            } else if self.level.set_block(pos, state("minecraft:sand")) {
                self.result.written += 1;
            }
        }
        self.result
            .data_markers
            .retain(|marker| marker.name == "collapsed_roof");
    }

    fn place_suspicious_sand(&mut self, pos: BlockPos) {
        if self
            .level
            .get_block(pos)
            .is_some_and(|state| state.name == "minecraft:suspicious_sand")
        {
            return;
        }
        let state = state("minecraft:suspicious_sand");
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.block_entities.push(PlacedBlock {
                pos,
                state,
                nbt: Some(Tag::Compound(vec![
                    (
                        "id".to_owned(),
                        Tag::String("minecraft:brushable_block".to_owned()),
                    ),
                    (
                        "LootTable".to_owned(),
                        Tag::String("minecraft:archaeology/desert_pyramid".to_owned()),
                    ),
                ])),
            });
        } else {
            self.result.rejected += 1;
        }
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn stairs(facing: folia_math::Direction) -> BlockState {
    BlockState {
        name: "minecraft:sandstone_stairs".to_owned(),
        properties: vec![(
            "facing".to_owned(),
            match facing {
                folia_math::Direction::North => "north",
                folia_math::Direction::South => "south",
                folia_math::Direction::East => "east",
                folia_math::Direction::West => "west",
                _ => "north",
            }
            .to_owned(),
        )],
    }
}

fn stairs_rotated(facing: folia_math::Direction, rotation: Rotation) -> BlockState {
    let rotated = rotation.rotate_direction(facing);
    stairs(rotated)
}

fn is_replaceable_by_structures(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
            | "minecraft:glow_lichen"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    )
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

fn parse_start_height(value: Option<&Value>, name: &str) -> Result<i32, StructureError> {
    let value = value.ok_or_else(|| {
        StructureError::InvalidDefinition(format!("{name}: missing `start_height`"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        StructureError::InvalidDefinition(format!("{name}: `start_height` must be an object"))
    })?;
    if let Some(absolute) = object.get("absolute").and_then(Value::as_i64) {
        return i32::try_from(absolute).map_err(|_| {
            StructureError::InvalidDefinition(format!(
                "{name}: absolute start_height is outside the int range"
            ))
        });
    }
    if let Some(constant) = object.get("constant").and_then(Value::as_i64) {
        return i32::try_from(constant).map_err(|_| {
            StructureError::InvalidDefinition(format!(
                "{name}: constant start_height is outside the int range"
            ))
        });
    }
    let type_name = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<unspecified>");
    Err(StructureError::UnsupportedHeightProvider {
        structure: name.to_owned(),
        type_name: type_name.to_owned(),
    })
}

fn parse_max_distance(value: Option<&Value>, name: &str) -> Result<(i32, i32), StructureError> {
    let Some(value) = value else {
        return Ok((80, 4064));
    };
    if let Some(horizontal) = value.as_i64() {
        let horizontal = i32::try_from(horizontal).map_err(|_| {
            StructureError::InvalidDefinition(format!(
                "{name}: max_distance_from_center is outside the int range"
            ))
        })?;
        return Ok((horizontal, 4064));
    }
    let object = value.as_object().ok_or_else(|| {
        StructureError::InvalidDefinition(format!(
            "{name}: max_distance_from_center must be an int or object"
        ))
    })?;
    let horizontal = required_i32(object, "horizontal", name)?;
    let vertical = object
        .get("vertical")
        .map(|value| {
            value.as_i64().ok_or_else(|| {
                StructureError::InvalidDefinition(format!(
                    "{name}: max_distance_from_center.vertical must be an int"
                ))
            })
        })
        .transpose()?
        .map(|value| {
            i32::try_from(value).map_err(|_| {
                StructureError::InvalidDefinition(format!(
                    "{name}: max_distance_from_center.vertical is outside the int range"
                ))
            })
        })
        .transpose()?
        .unwrap_or(4064);
    Ok((horizontal, vertical))
}

fn parse_dimension_padding(
    value: Option<&Value>,
    name: &str,
) -> Result<(i32, i32), StructureError> {
    let Some(value) = value else {
        return Ok((0, 0));
    };
    if let Some(padding) = value.as_i64() {
        let padding = i32::try_from(padding).map_err(|_| {
            StructureError::InvalidDefinition(format!(
                "{name}: dimension_padding is outside the int range"
            ))
        })?;
        return Ok((padding, padding));
    }
    let object = value.as_object().ok_or_else(|| {
        StructureError::InvalidDefinition(format!(
            "{name}: dimension_padding must be an int or object"
        ))
    })?;
    let bottom = object
        .get("bottom")
        .map(|_| required_i32(object, "bottom", name))
        .transpose()?
        .unwrap_or(0);
    let top = object
        .get("top")
        .map(|_| required_i32(object, "top", name))
        .transpose()?
        .unwrap_or(0);
    Ok((bottom, top))
}

fn parse_heightmap(name: &str) -> Option<Heightmap> {
    match name {
        "WORLD_SURFACE" => Some(Heightmap::WorldSurface),
        "WORLD_SURFACE_WG" => Some(Heightmap::WorldSurfaceWg),
        "OCEAN_FLOOR" => Some(Heightmap::OceanFloor),
        "OCEAN_FLOOR_WG" => Some(Heightmap::OceanFloorWg),
        "MOTION_BLOCKING" => Some(Heightmap::MotionBlocking),
        "MOTION_BLOCKING_NO_LEAVES" => Some(Heightmap::MotionBlockingNoLeaves),
        _ => None,
    }
}

fn required_i32(object: &Map<String, Value>, key: &str, name: &str) -> Result<i32, StructureError> {
    let value = object.get(key).and_then(Value::as_i64).ok_or_else(|| {
        StructureError::InvalidDefinition(format!("{name}: missing integer `{key}`"))
    })?;
    i32::try_from(value).map_err(|_| {
        StructureError::InvalidDefinition(format!("{name}: `{key}` is outside the int range"))
    })
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

/// The shipped structure definitions this slice can generate end to end.
///
/// Villages, igloos and desert pyramids are complete vertical slices; other
/// structure types stay parsed-but-unsupported and are skipped explicitly by
/// [`StructureGenerator`]. The JSON is the 1.21.11 data-pack document with
/// insignificant whitespace removed.
pub const SHIPPED_STRUCTURE_JSON: &[(&str, &str)] = &[
    (
        "minecraft:igloo",
        r##"{ "type": "minecraft:igloo", "biomes": "#minecraft:has_structure/igloo", "spawn_overrides": {}, "step": "surface_structures" }"##,
    ),
    (
        "minecraft:desert_pyramid",
        r##"{ "type": "minecraft:desert_pyramid", "biomes": "#minecraft:has_structure/desert_pyramid", "spawn_overrides": {}, "step": "surface_structures" }"##,
    ),
    (
        "minecraft:village_plains",
        r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_plains", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/plains/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##,
    ),
    (
        "minecraft:village_desert",
        r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_desert", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/desert/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##,
    ),
    (
        "minecraft:village_savanna",
        r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_savanna", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/savanna/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##,
    ),
    (
        "minecraft:village_snowy",
        r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_snowy", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/snowy/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##,
    ),
    (
        "minecraft:village_taiga",
        r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_taiga", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/taiga/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##,
    ),
];

/// `StructureManager` + biome-filtered set selection for the generator.
///
/// The manager regenerates a start deterministically for every candidate
/// placement chunk instead of caching live starts. That keeps chunk generation
/// independent and testable; a future region/structure manager should replace
/// it and own serialization rather than changing this deterministic contract.
#[derive(Debug, Clone)]
enum TemplateStore {
    Map(HashMap<String, StructureTemplate>),
    Catalog(Arc<StructureTemplateCatalog>),
}

impl TemplateProvider for TemplateStore {
    fn template(&self, name: &str) -> Option<&StructureTemplate> {
        match self {
            TemplateStore::Map(templates) => templates.template(name),
            TemplateStore::Catalog(catalog) => {
                StructureTemplateCatalog::template(catalog, name).ok()
            }
        }
    }
}

impl TemplateStore {
    fn as_map(&self) -> &HashMap<String, StructureTemplate> {
        match self {
            TemplateStore::Map(templates) => templates,
            TemplateStore::Catalog(_) => {
                static EMPTY: OnceLock<HashMap<String, StructureTemplate>> = OnceLock::new();
                EMPTY.get_or_init(HashMap::new)
            }
        }
    }
}

impl TemplateProvider for HashMap<String, StructureTemplate> {
    fn template(&self, name: &str) -> Option<&StructureTemplate> {
        HashMap::get(self, name)
    }
}

/// [`StructureGenerator`] backed by the embedded village template catalog.
pub struct StructureGenerator {
    sets: StructureSetRegistry,
    definitions: BTreeMap<String, StructureDefinition>,
    templates: TemplateStore,
    enabled: BTreeSet<String>,
}

impl std::fmt::Debug for StructureGenerator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StructureGenerator")
            .field("sets", &self.sets)
            .field("definitions", &self.definitions.keys())
            .field("templates", &self.templates)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl Clone for StructureGenerator {
    fn clone(&self) -> Self {
        Self {
            sets: self.sets.clone(),
            definitions: self.definitions.clone(),
            templates: self.templates.clone(),
            enabled: self.enabled.clone(),
        }
    }
}

impl StructureGenerator {
    /// Builds a manager over the shipped sets and supported definitions.
    pub fn shipped(templates: HashMap<String, StructureTemplate>) -> Result<Self, StructureError> {
        let sets =
            StructureSetRegistry::shipped().map_err(|error| StructureError::StructureSet {
                message: error.to_string(),
            })?;
        let mut definitions = BTreeMap::new();
        for (name, json) in SHIPPED_STRUCTURE_JSON {
            definitions.insert(
                (*name).to_owned(),
                StructureDefinition::from_json(name, json)?,
            );
        }
        let enabled = definitions.keys().cloned().collect();
        Ok(Self {
            sets,
            definitions,
            templates: TemplateStore::Map(templates),
            enabled,
        })
    }

    /// Builds a manager that lazily loads templates from a catalog.
    pub fn from_catalog(catalog: StructureTemplateCatalog) -> Result<Self, StructureError> {
        let mut generator = Self::shipped(HashMap::new())?;
        generator.templates = TemplateStore::Catalog(Arc::new(catalog));
        Ok(generator)
    }

    /// Builds a manager backed by the embedded village catalog.
    pub fn shipped_catalog() -> Result<Self, StructureError> {
        Ok(
            Self::from_catalog(StructureTemplateCatalog::shipped().clone())?
                .with_enabled_structures([
                    "minecraft:village_plains",
                    "minecraft:village_desert",
                    "minecraft:village_savanna",
                    "minecraft:village_snowy",
                    "minecraft:village_taiga",
                ]),
        )
    }

    /// Restricts generation to the named structures. Tests use this to keep a
    /// fixture provider minimal without silently ignoring missing templates.
    pub fn with_enabled_structures(
        mut self,
        structures: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.enabled = structures.into_iter().map(Into::into).collect();
        self
    }

    pub fn sets(&self) -> &StructureSetRegistry {
        &self.sets
    }

    pub fn definitions(&self) -> &BTreeMap<String, StructureDefinition> {
        &self.definitions
    }

    /// Templates supplied directly through [`StructureGenerator::shipped`].
    /// A catalog-backed manager loads templates lazily instead; this map is
    /// empty for it and [`StructureGenerator::template_catalog`] is `Some`.
    pub fn templates(&self) -> &HashMap<String, StructureTemplate> {
        self.templates.as_map()
    }

    /// Embedded catalog backing this manager, when one was supplied.
    pub fn template_catalog(&self) -> Option<&StructureTemplateCatalog> {
        match &self.templates {
            TemplateStore::Map(_) => None,
            TemplateStore::Catalog(catalog) => Some(catalog),
        }
    }

    pub fn structure_at_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
        dimension: StructureDimension,
        biome_y: i32,
        biomes: &dyn StructureBiomeLookup,
    ) -> Result<Option<SelectedStructure>, StructureError> {
        self.sets
            .structure_at_chunk_filtered_matching(
                chunk_x,
                chunk_z,
                seed,
                dimension,
                biome_y,
                biomes,
                |structure| self.enabled.contains(structure),
            )
            .map_err(|error| StructureError::StructureSet {
                message: error.to_string(),
            })
    }

    /// Places every enabled structure start whose pieces intersect one chunk.
    ///
    /// `radius` is the maximum horizontal search distance in chunks; 8 covers
    /// the shipped 80..=116 block max-distance windows.
    #[allow(clippy::too_many_arguments)]
    pub fn place_for_chunk(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_pos: ChunkPos,
        seed: i64,
        dimension: StructureDimension,
        biome_y: i32,
        biomes: &dyn StructureBiomeLookup,
        radius: i32,
        step: Decoration,
    ) -> Result<StructureChunkPlacement, StructureError> {
        self.place_for_chunk_inner(
            level, None, chunk_pos, seed, dimension, biome_y, biomes, radius, step,
        )
    }

    /// Places every enabled start using a separate height source for start
    /// generation and piece placement.
    ///
    /// `height_source` supplies `WorldSurfaceWg` sampling for neighbouring
    /// start chunks that are not in the writable chunk; `level` still receives
    /// all writes. The generator uses this to answer base-height queries for
    /// surrounding chunks without materialising them.
    #[allow(clippy::too_many_arguments)]
    pub fn place_for_chunk_with_height_source(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: &dyn FeatureLevel,
        chunk_pos: ChunkPos,
        seed: i64,
        dimension: StructureDimension,
        biome_y: i32,
        biomes: &dyn StructureBiomeLookup,
        radius: i32,
        step: Decoration,
    ) -> Result<StructureChunkPlacement, StructureError> {
        self.place_for_chunk_inner(
            level,
            Some(height_source),
            chunk_pos,
            seed,
            dimension,
            biome_y,
            biomes,
            radius,
            step,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn place_for_chunk_inner(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: Option<&dyn FeatureLevel>,
        chunk_pos: ChunkPos,
        seed: i64,
        dimension: StructureDimension,
        biome_y: i32,
        biomes: &dyn StructureBiomeLookup,
        radius: i32,
        step: Decoration,
    ) -> Result<StructureChunkPlacement, StructureError> {
        let processors = StructureProcessorCatalog::shipped();
        let mut result = StructureChunkPlacement::default();
        let min_x = chunk_pos.x << 4;
        let min_z = chunk_pos.z << 4;
        let max_y = level.min_y() + level.height() - 1;
        let chunk_box =
            BoundingBox::new(min_x, level.min_y(), min_z, min_x + 15, max_y, min_z + 15);
        let mut random = WorldgenRandom::new(0);
        let decoration_seed = random.set_decoration_seed(seed, min_x, min_z);
        let mut seeded_structures = BTreeSet::new();

        for x in (chunk_pos.x - radius)..=(chunk_pos.x + radius) {
            for z in (chunk_pos.z - radius)..=(chunk_pos.z + radius) {
                let selection =
                    match self.structure_at_chunk(x, z, seed, dimension, biome_y, biomes)? {
                        Some(selection) => selection,
                        None => continue,
                    };
                if !self.enabled.contains(&selection.structure) {
                    continue;
                }
                let Some(definition) = self.definitions.get(&selection.structure) else {
                    result.skipped_unsupported.push(selection.structure);
                    continue;
                };
                if definition.step() != step {
                    continue;
                }
                let start = {
                    let source: &dyn FeatureLevel = match height_source {
                        Some(source) => source,
                        None => &*level,
                    };
                    let context = StructureGenerationContext::new(
                        seed,
                        ChunkPos::new(x, z),
                        0,
                        &self.templates,
                    )
                    .with_level(source);
                    match definition.generate_start(&context) {
                        Ok(start) => start,
                        Err(StructureError::UnsupportedStructureType { .. })
                        | Err(StructureError::UnsupportedHeightProvider { .. }) => {
                            result.skipped_unsupported.push(selection.structure);
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                };
                if !start.is_valid() {
                    continue;
                }
                result.generated_starts += 1;
                let placement = if start.jigsaw().is_some() {
                    if seeded_structures.insert(selection.structure.clone()) {
                        random.set_feature_seed(
                            decoration_seed,
                            self.structure_step_index(&selection.structure, step),
                            step.index() as i32,
                        );
                    }
                    start.place_jigsaw_in_chunk_with_random(
                        level,
                        chunk_box,
                        &self.templates,
                        processors,
                        seed,
                        &mut random,
                    )?
                } else {
                    start.place_in_chunk(level, chunk_pos)?
                };
                result.placed_pieces += placement.placed_pieces;
                result.skipped_pieces += placement.skipped_pieces;
                result.written += placement.written;
                result.rejected += placement.rejected;
                result.feature_elements += placement.feature_elements;
                result.feature_placements += placement.feature_placements;
                result.feature_written += placement.feature_written;
                result.feature_rejected += placement.feature_rejected;
                result.written_blocks.extend(placement.written_blocks);
                result.feature_blocks.extend(placement.feature_blocks);
                result.block_entities.extend(placement.block_entities);
                result.entities.extend(placement.entities);
                result.data_markers.extend(placement.data_markers);
            }
        }
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        result
            .feature_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.feature_blocks.dedup();
        Ok(result)
    }

    /// Approximate Java's `i1` in `ChunkGenerator.addVanillaDecorations`: the
    /// index of a structure within its decoration step.
    ///
    /// The shipped catalog only contains the seven implemented definitions, so
    /// this is alphabetical over those definitions rather than the full
    /// vanilla registry order. That is deterministic and recorded as a parity
    /// gap; it does not change which blocks a feature writes once the seed is
    /// fixed.
    fn structure_step_index(&self, structure: &str, step: Decoration) -> i32 {
        self.definitions
            .values()
            .filter(|definition| definition.step() == step)
            .position(|definition| definition.name == structure)
            .unwrap_or(0) as i32
    }
}

/// Result of one chunk's structure pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructureChunkPlacement {
    pub generated_starts: usize,
    pub placed_pieces: usize,
    pub skipped_pieces: usize,
    pub written: usize,
    pub rejected: usize,
    pub feature_elements: usize,
    pub feature_placements: usize,
    pub feature_written: usize,
    pub feature_rejected: usize,
    pub written_blocks: Vec<BlockPos>,
    pub feature_blocks: Vec<BlockPos>,
    pub skipped_unsupported: Vec<String>,
    pub block_entities: Vec<PlacedBlock>,
    pub entities: Vec<PlacedEntity>,
    pub data_markers: Vec<StructureDataMarker>,
}

/// `StructureSetError` convenience conversion used by the manager.
impl From<StructureSetError> for StructureError {
    fn from(error: StructureSetError) -> Self {
        StructureError::StructureSet {
            message: error.to_string(),
        }
    }
}
