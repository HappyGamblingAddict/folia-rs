//! Port of `JigsawPlacement` and `PoolElementStructurePiece`.
//!
//! This is the recursive half of the P2.8/P2.9 structure work. It takes a
//! start-pool element, walks every jigsaw connector breadth-first, matches
//! parent targets to child names, and places child elements with Java's
//! rigid/terrain-matching height rules, expansion hack, free-space collision
//! test, max-distance window and serialized placement priorities.
//!
//! The RNG order is the Java order: start rotation, start element, shuffled
//! jigsaws of the start element, then for every connector the pool templates,
//! the four rotations, and the child jigsaws in the order Java consumes them.
//! [`shuffle`] is `Util.shuffle`; [`SequencedPriorityQueue`] mirrors
//! `SequencedPriorityIterator` in insertion order.
//!
//! Template entities and data markers are returned in
//! [`crate::worldgen::structure::piece::PiecePlacement`]; no entities are
//! spawned and no loot-table chest is created yet. Waterlogging is applied by
//! a wrapper level so the shared pool placement path stays untouched.

use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{
    BoundingBox, PiecePlacement, StructureDataMarker, template_bounding_box,
};
use crate::worldgen::structure::pool::{
    LiquidSettings, PoolCatalog, PoolElement, PoolExpansion, Projection, StructureTemplatePool,
};
use crate::worldgen::structure::processor::StructureProcessorCatalog;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, TemplateProvider, TerrainAdjustment,
};
use crate::worldgen::structure::template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructureTemplate, transform, transform_state,
};
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};
use std::collections::BTreeMap;
use std::collections::VecDeque;

const UNSET_HEIGHT: i32 = i32::MIN;

/// `JigsawBlockEntity.JointType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JointType {
    Rollable,
    Aligned,
}

impl JointType {
    pub fn name(self) -> &'static str {
        match self {
            JointType::Rollable => "rollable",
            JointType::Aligned => "aligned",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "rollable" => Some(JointType::Rollable),
            "aligned" => Some(JointType::Aligned),
            _ => None,
        }
    }
}

/// `StructureTemplate.JigsawBlockInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct JigsawBlockInfo {
    pub pos: BlockPos,
    pub state: BlockState,
    pub nbt: Option<Tag>,
    pub joint: JointType,
    pub name: String,
    pub pool: String,
    pub target: String,
    pub placement_priority: i32,
    pub selection_priority: i32,
}

impl JigsawBlockInfo {
    /// `JigsawBlockInfo.of` for one palette block.
    pub fn from_template_block(
        template: &StructureTemplate,
        block: &crate::worldgen::structure::template::TemplateBlock,
        rotation: Rotation,
        origin: BlockPos,
    ) -> Option<Self> {
        let state = template.palette(0)?.get(block.state)?;
        if state.name != "minecraft:jigsaw" {
            return None;
        }
        let nbt = block.nbt.clone();
        let compound = nbt.as_ref();
        let joint = compound
            .and_then(|tag| tag.get("joint"))
            .and_then(Tag::as_str)
            .and_then(JointType::from_name)
            .unwrap_or_else(|| default_joint(state));
        let name = compound
            .and_then(|tag| tag.get("name"))
            .and_then(Tag::as_str)
            .unwrap_or("minecraft:empty")
            .to_owned();
        let pool = compound
            .and_then(|tag| tag.get("pool"))
            .and_then(Tag::as_str)
            .map(normalized_id)
            .unwrap_or_else(|| "minecraft:empty".to_owned());
        let target = compound
            .and_then(|tag| tag.get("target"))
            .and_then(Tag::as_str)
            .unwrap_or("minecraft:empty")
            .to_owned();
        let placement_priority = compound
            .and_then(|tag| tag.get("placement_priority"))
            .and_then(Tag::as_int)
            .unwrap_or(0);
        let selection_priority = compound
            .and_then(|tag| tag.get("selection_priority"))
            .and_then(Tag::as_int)
            .unwrap_or(0);
        let pos = origin.offset_by(transform(block.pos, Mirror::None, rotation, BlockPos::ZERO));
        Some(Self {
            pos,
            state: transform_state(state, Mirror::None, rotation),
            nbt,
            joint,
            name,
            pool,
            target,
            placement_priority,
            selection_priority,
        })
    }

    /// `JigsawBlock.canAttach`.
    pub fn can_attach(&self, child: &JigsawBlockInfo) -> bool {
        let front = front_facing(&self.state);
        let child_front = front_facing(&child.state);
        let top = top_facing(&self.state);
        let child_top = top_facing(&child.state);
        front == child_front.opposite()
            && (self.joint == JointType::Rollable || top == child_top)
            && self.target == child.name
    }
}

/// `JigsawJunction`: the source-side connector bookkeeping saved on a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JigsawJunction {
    pub source_x: i32,
    pub source_y: i32,
    pub source_z: i32,
    pub delta_y: i32,
    pub dest_projection: Projection,
}

/// `PoolElementStructurePiece`.
#[derive(Debug, Clone, PartialEq)]
pub struct JigsawPiece {
    pub pool: String,
    pub element: PoolElement,
    /// `PoolElementStructurePiece.position`.
    pub position: BlockPos,
    pub rotation: Rotation,
    pub ground_level_delta: i32,
    pub bounding_box: BoundingBox,
    pub liquid_settings: LiquidSettings,
    pub junctions: Vec<JigsawJunction>,
    pub gen_depth: i32,
}

impl JigsawPiece {
    pub fn projection(&self) -> Projection {
        self.element.projection()
    }

    /// `PoolElementStructurePiece.place` clipped to one chunk box.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        chunk_box: BoundingBox,
        structure_pos: BlockPos,
        world_seed: i64,
    ) -> Result<PiecePlacement, StructureError> {
        let expansion = PoolExpansion {
            pool: self.pool.clone(),
            rotation: self.rotation,
            position: self.position,
            element: self.element.clone(),
            piece: None,
            bounding_box: Some(self.bounding_box),
        };
        let mut waterlogging = WaterloggingLevel::new(level, self.liquid_settings);
        let placement = expansion
            .place(
                &mut waterlogging,
                templates,
                processors,
                chunk_box,
                structure_pos,
                world_seed,
                false,
            )
            .map_err(|error| StructureError::Pool {
                message: error.to_string(),
            })?;
        Ok(placement)
    }
}

/// `StructureStart` payload for jigsaw structures.
#[derive(Debug, Clone, PartialEq)]
pub struct JigsawStart {
    structure_id: String,
    chunk_pos: crate::region::ChunkPos,
    references: i32,
    terrain_adaptation: TerrainAdjustment,
    pieces: Vec<JigsawPiece>,
}

impl JigsawStart {
    pub fn new(
        structure_id: impl Into<String>,
        chunk_pos: crate::region::ChunkPos,
        references: i32,
        terrain_adaptation: TerrainAdjustment,
        pieces: Vec<JigsawPiece>,
    ) -> Self {
        Self {
            structure_id: structure_id.into(),
            chunk_pos,
            references,
            terrain_adaptation,
            pieces,
        }
    }

    pub fn structure_id(&self) -> &str {
        &self.structure_id
    }

    pub fn chunk_pos(&self) -> crate::region::ChunkPos {
        self.chunk_pos
    }

    pub fn references(&self) -> i32 {
        self.references
    }

    pub fn terrain_adaptation(&self) -> TerrainAdjustment {
        self.terrain_adaptation
    }

    pub fn pieces(&self) -> &[JigsawPiece] {
        &self.pieces
    }

    pub fn pieces_mut(&mut self) -> &mut [JigsawPiece] {
        &mut self.pieces
    }

    pub fn is_valid(&self) -> bool {
        !self.pieces.is_empty()
    }

    pub fn pieces_bounding_box(&self) -> Option<BoundingBox> {
        BoundingBox::encapsulating(self.pieces.iter().map(|piece| piece.bounding_box))
    }

    pub fn bounding_box(&self) -> Option<BoundingBox> {
        let box_ = self.pieces_bounding_box()?;
        if self.terrain_adaptation.inflates_bounding_box() {
            Some(box_.inflated_by(12))
        } else {
            Some(box_)
        }
    }

    /// `StructureStart.placeInChunk` for a jigsaw start.
    pub fn place_in_chunk(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        world_seed: i64,
    ) -> Result<StartPlacement, StructureError> {
        let mut result = StartPlacement::default();
        let Some(first) = self.pieces.first() else {
            return Ok(result);
        };
        let center = first.bounding_box.center();
        let structure_pos = BlockPos::new(center.x(), first.bounding_box.min_y(), center.z());
        for piece in &self.pieces {
            if !piece.bounding_box.intersects(chunk_box) {
                result.skipped_pieces += 1;
                continue;
            }
            let placement = piece.place(
                level,
                templates,
                processors,
                chunk_box,
                structure_pos,
                world_seed,
            )?;
            result.placed_pieces += 1;
            result.written += placement.written;
            result.rejected += placement.rejected;
            result.block_entities.extend(placement.block_entities);
            result.entities.extend(placement.entities);
            result.data_markers.extend(placement.data_markers);
        }
        Ok(result)
    }
}

/// One `JigsawPlacement.addPieces` call.
pub struct JigsawPlacement<'a> {
    pub pools: &'a PoolCatalog,
    pub templates: &'a dyn TemplateProvider,
    pub processors: &'a StructureProcessorCatalog,
    pub config: JigsawConfig,
}

/// The subset of `JigsawStructure` needed by the placement walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JigsawConfig {
    pub start_pool: String,
    pub start_jigsaw_name: Option<String>,
    pub max_depth: i32,
    /// Sampled absolute `start_height`.
    pub start_height: i32,
    pub use_expansion_hack: bool,
    pub project_start_to_heightmap: Option<Heightmap>,
    pub max_distance_horizontal: i32,
    pub max_distance_vertical: i32,
    pub dimension_padding_bottom: i32,
    pub dimension_padding_top: i32,
    pub liquid_settings: LiquidSettings,
}

impl JigsawConfig {
    /// `JigsawStructure` defaults for the fields a structure JSON may omit.
    pub fn new(start_pool: impl Into<String>, max_depth: i32, start_height: i32) -> Self {
        Self {
            start_pool: start_pool.into(),
            start_jigsaw_name: None,
            max_depth,
            start_height,
            use_expansion_hack: false,
            project_start_to_heightmap: None,
            max_distance_horizontal: 80,
            max_distance_vertical: 4064,
            dimension_padding_bottom: 0,
            dimension_padding_top: 0,
            liquid_settings: LiquidSettings::ApplyWaterlogging,
        }
    }
}

impl<'a> JigsawPlacement<'a> {
    pub fn new(
        pools: &'a PoolCatalog,
        templates: &'a dyn TemplateProvider,
        processors: &'a StructureProcessorCatalog,
        config: JigsawConfig,
    ) -> Self {
        Self {
            pools,
            templates,
            processors,
            config,
        }
    }

    /// `JigsawPlacement.addPieces` for one chunk.
    pub fn generate(
        &self,
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<Option<JigsawStart>, StructureError> {
        let Some(level) = context.height_source else {
            return Err(StructureError::MissingHeightSource {
                structure: structure_id.to_owned(),
            });
        };
        let mut random = LegacyRandomSource::new(0);
        crate::worldgen::structure::set::set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );

        // `JigsawPlacement.addPieces`: rotation, then weighted start element.
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let start_pool_name = normalized_id(&self.config.start_pool);
        if is_builtin_empty_pool(&start_pool_name) {
            return Ok(None);
        }
        let pool =
            self.pools
                .pool(&start_pool_name)
                .ok_or_else(|| StructureError::UnknownPool {
                    pool: start_pool_name.clone(),
                })?;
        let Some(element) = pool.weighted_element(&mut random) else {
            return Ok(None);
        };
        let element = element.clone();
        match element {
            PoolElement::Empty => return Ok(None),
            PoolElement::Feature { feature, .. } => {
                return Err(StructureError::UnsupportedPoolElement {
                    pool: pool.name().to_owned(),
                    element_type: format!("feature_pool_element({feature})"),
                });
            }
            _ => {}
        }

        let chunk_min = BlockPos::new(
            context.chunk_pos.x << 4,
            self.config.start_height,
            context.chunk_pos.z << 4,
        );
        let (start_position, jigsaw_offset) = if let Some(name) =
            self.config.start_jigsaw_name.as_deref()
        {
            let jigsaws =
                shuffled_jigsaw_blocks(&element, self.templates, chunk_min, rotation, &mut random)?;
            let found = jigsaws
                .iter()
                .find(|jigsaw| jigsaw.name == name)
                .ok_or_else(|| StructureError::StartJigsawNotFound {
                    structure: structure_id.to_owned(),
                    name: name.to_owned(),
                })?;
            let offset = found
                .pos
                .offset(-chunk_min.x(), -chunk_min.y(), -chunk_min.z());
            (
                BlockPos::new(
                    chunk_min.x() - offset.x(),
                    chunk_min.y() - offset.y(),
                    chunk_min.z() - offset.z(),
                ),
                offset,
            )
        } else {
            (chunk_min, BlockPos::ZERO)
        };

        let mut start_box = element_bounds(
            &element,
            self.templates,
            start_position,
            rotation,
            pool.name(),
        )?;
        let center_x = (start_box.max_x() + start_box.min_x()) / 2;
        let center_z = (start_box.max_z() + start_box.min_z()) / 2;
        let ground_delta = element_ground_level_delta();
        let projected_y = match self.config.project_start_to_heightmap {
            None => start_position.y(),
            Some(heightmap) => {
                chunk_min.y()
                    + level
                        .heightmap(heightmap, center_x, center_z)
                        .ok_or_else(|| StructureError::MissingHeightSource {
                            structure: structure_id.to_owned(),
                        })?
            }
        };
        let delta_y = projected_y - (start_box.min_y() + ground_delta);
        start_box = start_box.moved(0, delta_y, 0);
        let start_position = start_position.offset(0, delta_y, 0);
        if self.start_too_close_to_world_limits(level, start_box) {
            return Ok(None);
        }

        let stub_y = projected_y + jigsaw_offset.y();
        let start_piece = JigsawPiece {
            pool: pool.name().to_owned(),
            element: element.clone(),
            position: start_position,
            rotation,
            ground_level_delta: ground_delta,
            bounding_box: start_box,
            liquid_settings: self.config.liquid_settings,
            junctions: Vec::new(),
            gen_depth: 0,
        };

        let mut pieces = vec![start_piece];
        if self.config.max_depth > 0 {
            let min_y = level.min_y() + self.config.dimension_padding_bottom;
            let max_y = level.min_y() + level.height() - 1 - self.config.dimension_padding_top;
            let allowed = BoundingBox::new(
                center_x - self.config.max_distance_horizontal,
                (stub_y - self.config.max_distance_vertical).max(min_y),
                center_z - self.config.max_distance_horizontal,
                center_x + self.config.max_distance_horizontal,
                (stub_y + self.config.max_distance_vertical).min(max_y),
                center_z + self.config.max_distance_horizontal,
            );
            let mut occupied = vec![start_box];
            let mut placer = Placer {
                pools: self.pools,
                templates: self.templates,
                config: &self.config,
                level,
                random: &mut random,
                pieces: &mut pieces,
                occupied: &mut occupied,
                allowed,
                placing: SequencedPriorityQueue::new(),
            };
            placer.try_placing_children(0, 0)?;
            while let Some((piece_index, depth)) = placer.placing.pop() {
                placer.try_placing_children(piece_index, depth)?;
            }
        }

        Ok(Some(JigsawStart::new(
            structure_id,
            context.chunk_pos,
            context.references,
            terrain_adaptation,
            pieces,
        )))
    }

    fn start_too_close_to_world_limits(
        &self,
        level: &dyn FeatureLevel,
        bounding_box: BoundingBox,
    ) -> bool {
        if self.config.dimension_padding_bottom == 0 && self.config.dimension_padding_top == 0 {
            return false;
        }
        let min_y = level.min_y() + self.config.dimension_padding_bottom;
        let max_y = level.min_y() + level.height() - 1 - self.config.dimension_padding_top;
        bounding_box.min_y() < min_y || bounding_box.max_y() > max_y
    }
}

struct Placer<'a> {
    pools: &'a PoolCatalog,
    templates: &'a dyn TemplateProvider,
    config: &'a JigsawConfig,
    level: &'a dyn FeatureLevel,
    random: &'a mut LegacyRandomSource,
    pieces: &'a mut Vec<JigsawPiece>,
    occupied: &'a mut Vec<BoundingBox>,
    allowed: BoundingBox,
    placing: SequencedPriorityQueue<(usize, usize)>,
}

impl Placer<'_> {
    /// Java's `tryPlacingChildren`. `piece_index` addresses the already-added
    /// parent piece; `depth` is its generation depth.
    fn try_placing_children(
        &mut self,
        piece_index: usize,
        depth: usize,
    ) -> Result<(), StructureError> {
        let piece = self.pieces[piece_index].clone();
        let element = piece.element.clone();
        let projection = element.projection();
        let parent_rigid = projection == Projection::Rigid;
        let parent_bounds = piece.bounding_box;
        let parent_min_y = parent_bounds.min_y();
        let mut height_at_connector = UNSET_HEIGHT;

        let jigsaws = shuffled_jigsaw_blocks(
            &element,
            self.templates,
            piece.position,
            piece.rotation,
            self.random,
        )?;

        'parent: for parent_jigsaw in &jigsaws {
            let front = front_facing(&parent_jigsaw.state);
            let connector = parent_jigsaw.pos;
            let front_pos = connector.offset(front.step_x(), front.step_y(), front.step_z());
            let connector_y = connector.y() - parent_min_y;

            let pool_name = normalized_id(&parent_jigsaw.pool);
            if is_builtin_empty_pool(&pool_name) {
                continue;
            }
            let pool = self
                .pools
                .pool(&pool_name)
                .ok_or_else(|| StructureError::UnknownPool {
                    pool: pool_name.clone(),
                })?;
            if pool.size() == 0 {
                return Err(StructureError::EmptyPool { pool: pool_name });
            }
            let fallback_name = normalized_id(pool.fallback());
            let fallback = if is_builtin_empty_pool(&fallback_name) {
                None
            } else {
                let fallback =
                    self.pools
                        .pool(&fallback_name)
                        .ok_or_else(|| StructureError::UnknownPool {
                            pool: fallback_name.clone(),
                        })?;
                if fallback.size() == 0 {
                    return Err(StructureError::EmptyPool {
                        pool: fallback_name,
                    });
                }
                Some(fallback)
            };

            let connector_inside = parent_bounds.is_inside(front_pos);
            // Java uses the piece's own box as the free-space seed when the
            // connector points back into the piece, and the inherited free
            // shape otherwise. Both cases reject overlap with the parent.
            let mut local_occupied = if connector_inside {
                vec![parent_bounds]
            } else {
                Vec::new()
            };

            let mut candidates = Vec::new();
            if depth != self.config.max_depth as usize {
                candidates.extend(shuffled_templates(pool, self.random));
            }
            if let Some(fallback) = fallback {
                candidates.extend(shuffled_templates(fallback, self.random));
            }

            for candidate in candidates {
                if matches!(candidate, PoolElement::Empty) {
                    // Java `break`s the whole element list, not just this item.
                    break;
                }
                if matches!(candidate, PoolElement::Feature { .. }) {
                    return Err(StructureError::UnsupportedPoolElement {
                        pool: pool.name().to_owned(),
                        element_type: candidate.element_type().to_owned(),
                    });
                }
                for child_rotation in shuffled_rotations(self.random) {
                    // `getShuffledJigsawBlocks` is called even when the child
                    // has no jigsaws; it consumes no RNG in that case.
                    let child_jigsaws = shuffled_jigsaw_blocks(
                        &candidate,
                        self.templates,
                        BlockPos::ZERO,
                        child_rotation,
                        self.random,
                    )?;
                    let child_zero_box = element_bounds(
                        &candidate,
                        self.templates,
                        BlockPos::ZERO,
                        child_rotation,
                        pool.name(),
                    )?;

                    let expansion_size =
                        if self.config.use_expansion_hack && child_zero_box.y_span() <= 16 {
                            expansion_hack_size(
                                &child_jigsaws,
                                &child_zero_box,
                                front_facing,
                                self.pools,
                                self.templates,
                            )?
                        } else {
                            0
                        };

                    for child_jigsaw in &child_jigsaws {
                        if !parent_jigsaw.can_attach(child_jigsaw) {
                            continue;
                        }
                        let child_connector = child_jigsaw.pos;
                        let offset = BlockPos::new(
                            front_pos.x() - child_connector.x(),
                            front_pos.y() - child_connector.y(),
                            front_pos.z() - child_connector.z(),
                        );
                        let mut child_box = element_bounds(
                            &candidate,
                            self.templates,
                            offset,
                            child_rotation,
                            pool.name(),
                        )?;
                        let child_min_y = child_box.min_y();
                        let child_rigid = candidate.projection() == Projection::Rigid;
                        let child_connector_y = child_connector.y();
                        let y_delta = connector_y - child_connector_y + front.step_y();
                        let target_y = if parent_rigid && child_rigid {
                            parent_min_y + y_delta
                        } else {
                            if height_at_connector == UNSET_HEIGHT {
                                height_at_connector = self
                                    .level
                                    .heightmap(
                                        Heightmap::WorldSurfaceWg,
                                        connector.x(),
                                        connector.z(),
                                    )
                                    .unwrap_or_else(|| self.level.min_y());
                            }
                            height_at_connector - child_connector_y
                        };
                        let shift = target_y - child_min_y;
                        child_box = child_box.moved(0, shift, 0);
                        let child_position = offset.offset(0, shift, 0);
                        if expansion_size > 0 {
                            let expanded = (expansion_size + 1).max(child_box.y_span());
                            child_box = BoundingBox::new(
                                child_box.min_x(),
                                child_box.min_y(),
                                child_box.min_z(),
                                child_box.max_x(),
                                child_box.min_y() + expanded - 1,
                                child_box.max_z(),
                            );
                        }
                        if !self.is_free(&child_box, &local_occupied, !connector_inside) {
                            continue;
                        }
                        if let Some(parent_piece) = self.pieces.get_mut(piece_index) {
                            let parent_delta = parent_piece.ground_level_delta;
                            let child_delta = if child_rigid {
                                parent_delta - y_delta
                            } else {
                                element_ground_level_delta()
                            };
                            let junction_y = if parent_rigid {
                                parent_min_y + connector_y
                            } else if child_rigid {
                                target_y + child_connector_y
                            } else {
                                if height_at_connector == UNSET_HEIGHT {
                                    height_at_connector = self
                                        .level
                                        .heightmap(
                                            Heightmap::WorldSurfaceWg,
                                            connector.x(),
                                            connector.z(),
                                        )
                                        .unwrap_or_else(|| self.level.min_y());
                                }
                                height_at_connector + y_delta / 2
                            };
                            parent_piece.junctions.push(JigsawJunction {
                                source_x: front_pos.x(),
                                source_y: junction_y - connector_y + parent_delta,
                                source_z: front_pos.z(),
                                delta_y: y_delta,
                                dest_projection: candidate.projection(),
                            });
                            let child_junction = JigsawJunction {
                                source_x: connector.x(),
                                source_y: junction_y - child_connector_y + child_delta,
                                source_z: connector.z(),
                                delta_y: -y_delta,
                                dest_projection: projection,
                            };
                            self.occupied.push(child_box);
                            local_occupied.push(child_box);
                            let child_index = self.pieces.len();
                            self.pieces.push(JigsawPiece {
                                pool: pool.name().to_owned(),
                                element: candidate.clone(),
                                position: child_position,
                                rotation: child_rotation,
                                ground_level_delta: child_delta,
                                bounding_box: child_box,
                                liquid_settings: self.config.liquid_settings,
                                junctions: vec![child_junction],
                                gen_depth: depth as i32 + 1,
                            });
                            if depth < self.config.max_depth as usize {
                                self.placing.add(
                                    (child_index, depth + 1),
                                    parent_jigsaw.placement_priority,
                                );
                            }
                            continue 'parent;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn is_free(
        &self,
        bounds: &BoundingBox,
        local_occupied: &[BoundingBox],
        check_global: bool,
    ) -> bool {
        if bounds.min_x() < self.allowed.min_x()
            || bounds.max_x() > self.allowed.max_x()
            || bounds.min_y() < self.allowed.min_y()
            || bounds.max_y() > self.allowed.max_y()
            || bounds.min_z() < self.allowed.min_z()
            || bounds.max_z() > self.allowed.max_z()
        {
            return false;
        }
        if local_occupied
            .iter()
            .any(|occupied| occupied.intersects(*bounds))
        {
            return false;
        }
        !check_global
            || !self
                .occupied
                .iter()
                .any(|occupied| occupied.intersects(*bounds))
    }
}

/// `StructureTemplatePool.getShuffledTemplates`: copy the expanded weighted
/// list and run `Util.shuffle`.
fn shuffled_templates(
    pool: &StructureTemplatePool,
    random: &mut impl RandomSource,
) -> Vec<PoolElement> {
    let mut templates = Vec::new();
    for entry in pool.entries() {
        for _ in 0..entry.weight {
            templates.push(entry.element.clone());
        }
    }
    shuffle(&mut templates, random);
    templates
}

/// `Rotation.getShuffled`.
fn shuffled_rotations(random: &mut impl RandomSource) -> [Rotation; 4] {
    let mut rotations = Rotation::ALL;
    shuffle(&mut rotations, random);
    rotations
}

/// `Util.shuffle`.
pub fn shuffle<T>(list: &mut [T], random: &mut impl RandomSource) {
    for i in (1..list.len()).rev() {
        let index = random.next_int_bound((i + 1) as i32) as usize;
        list.swap(i, index);
    }
}

/// `StructurePoolElement.getShuffledJigsawBlocks`: template jigsaws, shuffled,
/// then stable-sorted by descending selection priority.
fn shuffled_jigsaw_blocks(
    element: &PoolElement,
    templates: &dyn TemplateProvider,
    origin: BlockPos,
    rotation: Rotation,
    random: &mut impl RandomSource,
) -> Result<Vec<JigsawBlockInfo>, StructureError> {
    let mut jigsaws = match element {
        PoolElement::Empty => Vec::new(),
        PoolElement::Feature { .. } => {
            // `FeaturePoolElement` contributes a synthetic downward jigsaw.
            let nbt = Tag::Compound(vec![
                (
                    "name".to_owned(),
                    Tag::String("minecraft:bottom".to_owned()),
                ),
                (
                    "final_state".to_owned(),
                    Tag::String("minecraft:air".to_owned()),
                ),
                ("pool".to_owned(), Tag::String("minecraft:empty".to_owned())),
                (
                    "target".to_owned(),
                    Tag::String("minecraft:empty".to_owned()),
                ),
                ("joint".to_owned(), Tag::String("rollable".to_owned())),
            ]);
            vec![JigsawBlockInfo {
                pos: origin,
                state: BlockState {
                    name: "minecraft:jigsaw".to_owned(),
                    properties: vec![("orientation".to_owned(), "down_south".to_owned())],
                },
                nbt: Some(nbt),
                joint: JointType::Rollable,
                name: "minecraft:bottom".to_owned(),
                pool: "minecraft:empty".to_owned(),
                target: "minecraft:empty".to_owned(),
                placement_priority: 0,
                selection_priority: 0,
            }]
        }
        PoolElement::List { .. } => {
            // `ListPoolElement` uses only its first child for jigsaws.
            let first = element
                .children()
                .and_then(|children| children.first())
                .ok_or_else(|| {
                    StructureError::InvalidDefinition(
                        "list pool element has no children".to_owned(),
                    )
                })?;
            shuffled_jigsaw_blocks(first, templates, origin, rotation, random)?
        }
        PoolElement::Single { .. } | PoolElement::LegacySingle { .. } => {
            let location = element.template_location().ok_or_else(|| {
                StructureError::InvalidDefinition(
                    "single pool element is missing its template location".to_owned(),
                )
            })?;
            let template =
                templates
                    .template(location)
                    .ok_or_else(|| StructureError::UnknownTemplate {
                        structure: "<jigsaw pool>".to_owned(),
                        template: location.to_owned(),
                    })?;
            let mut result = Vec::new();
            for block in template.blocks() {
                if let Some(info) =
                    JigsawBlockInfo::from_template_block(template, block, rotation, origin)
                {
                    result.push(info);
                }
            }
            result
        }
    };
    shuffle(&mut jigsaws, random);
    jigsaws.sort_by(|left, right| right.selection_priority.cmp(&left.selection_priority));
    Ok(jigsaws)
}

/// `StructurePoolElement.getBoundingBox`.
pub fn element_bounds(
    element: &PoolElement,
    templates: &dyn TemplateProvider,
    position: BlockPos,
    rotation: Rotation,
    pool: &str,
) -> Result<BoundingBox, StructureError> {
    match element {
        PoolElement::Empty => Ok(BoundingBox::from_pos(position)),
        PoolElement::Feature { .. } => Ok(BoundingBox::from_pos(position)),
        PoolElement::Single { template, .. } | PoolElement::LegacySingle { template, .. } => {
            let template =
                templates
                    .template(template)
                    .ok_or_else(|| StructureError::UnknownTemplate {
                        structure: pool.to_owned(),
                        template: template.clone(),
                    })?;
            Ok(template_bounding_box(
                template,
                position,
                rotation,
                BlockPos::ZERO,
                Mirror::None,
            ))
        }
        PoolElement::List { elements, .. } => BoundingBox::encapsulating(
            elements
                .iter()
                .filter(|element| !matches!(element, PoolElement::Empty))
                .map(|element| element_bounds(element, templates, position, rotation, pool))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .ok_or_else(|| {
            StructureError::InvalidDefinition(format!(
                "pool `{pool}` list element has no non-empty children"
            ))
        }),
    }
}

fn element_ground_level_delta() -> i32 {
    1
}

/// `StructureTemplatePool.getMaxSize` for the expansion hack.
fn pool_max_size(
    pool: &StructureTemplatePool,
    templates: &dyn TemplateProvider,
) -> Result<i32, StructureError> {
    let mut max = 0;
    for entry in pool.entries() {
        if matches!(entry.element, PoolElement::Empty) {
            continue;
        }
        let bounds = element_bounds(
            &entry.element,
            templates,
            BlockPos::ZERO,
            Rotation::None,
            pool.name(),
        )?;
        max = max.max(bounds.y_span());
    }
    Ok(max)
}

fn expansion_hack_size(
    jigsaws: &[JigsawBlockInfo],
    child_zero_box: &BoundingBox,
    front_facing: fn(&BlockState) -> Direction,
    pools: &PoolCatalog,
    templates: &dyn TemplateProvider,
) -> Result<i32, StructureError> {
    let mut max = 0;
    for jigsaw in jigsaws {
        let front = front_facing(&jigsaw.state);
        let front_pos = jigsaw
            .pos
            .offset(front.step_x(), front.step_y(), front.step_z());
        if !child_zero_box.is_inside(front_pos) {
            continue;
        }
        let pool_name = normalized_id(&jigsaw.pool);
        if is_builtin_empty_pool(&pool_name) {
            continue;
        }
        let pool = pools
            .pool(&pool_name)
            .ok_or_else(|| StructureError::UnknownPool {
                pool: pool_name.clone(),
            })?;
        let fallback_name = normalized_id(pool.fallback());
        let mut sizes = vec![pool_max_size(pool, templates)?];
        if !is_builtin_empty_pool(&fallback_name) {
            let fallback = pools
                .pool(&fallback_name)
                .ok_or(StructureError::UnknownPool {
                    pool: fallback_name,
                })?;
            sizes.push(pool_max_size(fallback, templates)?);
        }
        max = max.max(sizes.into_iter().max().unwrap_or(0));
    }
    Ok(max)
}

/// Waterlogging wrapper around the caller's feature level. Java reads the
/// fluid at the target before writing and calls `placeLiquid` for
/// waterloggable blocks; this port implements the observable waterlogged
/// property update for water sources.
struct WaterloggingLevel<'a> {
    inner: &'a mut dyn FeatureLevel,
    settings: LiquidSettings,
    waterlogged: usize,
}

impl<'a> WaterloggingLevel<'a> {
    fn new(inner: &'a mut dyn FeatureLevel, settings: LiquidSettings) -> Self {
        Self {
            inner,
            settings,
            waterlogged: 0,
        }
    }
}

impl FeatureLevel for WaterloggingLevel<'_> {
    fn min_y(&self) -> i32 {
        self.inner.min_y()
    }

    fn height(&self) -> i32 {
        self.inner.height()
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.inner.get_block(pos)
    }

    fn set_block(&mut self, pos: BlockPos, mut state: BlockState) -> bool {
        if self.settings == LiquidSettings::ApplyWaterlogging
            && is_water_loggable(&state)
            && self.inner.get_block(pos).is_some_and(is_water_source)
        {
            set_waterlogged(&mut state);
            self.waterlogged += 1;
        }
        self.inner.set_block(pos, state)
    }

    fn heightmap(&self, kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        self.inner.heightmap(kind, x, z)
    }
}

fn is_water_loggable(state: &BlockState) -> bool {
    state
        .properties
        .iter()
        .any(|(key, value)| key == "waterlogged" && value == "false")
}

fn is_water_source(state: &BlockState) -> bool {
    state.name == "minecraft:water"
        && state
            .properties
            .iter()
            .all(|(key, value)| key != "level" || value == "0")
}

fn set_waterlogged(state: &mut BlockState) {
    for (key, value) in &mut state.properties {
        if key == "waterlogged" {
            *value = "true".to_owned();
        }
    }
}

/// `BlockState` jigsaw orientation parsing.
pub fn front_facing(state: &BlockState) -> Direction {
    orientation(state).0
}

pub fn top_facing(state: &BlockState) -> Direction {
    orientation(state).1
}

fn orientation(state: &BlockState) -> (Direction, Direction) {
    let value = state
        .properties
        .iter()
        .find(|(key, _)| key == "orientation")
        .map(|(_, value)| value.as_str());
    if let Some((front, top)) = value.and_then(|value| value.split_once('_'))
        && let (Some(front), Some(top)) = (direction_from_name(front), direction_from_name(top))
    {
        return (front, top);
    }
    (Direction::North, Direction::Up)
}

fn direction_from_name(name: &str) -> Option<Direction> {
    match name {
        "down" => Some(Direction::Down),
        "up" => Some(Direction::Up),
        "north" => Some(Direction::North),
        "south" => Some(Direction::South),
        "west" => Some(Direction::West),
        "east" => Some(Direction::East),
        _ => None,
    }
}

fn default_joint(state: &BlockState) -> JointType {
    if front_facing(state).axis().is_horizontal() {
        JointType::Aligned
    } else {
        JointType::Rollable
    }
}

fn normalized_id(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

/// `Pools.EMPTY` is a built-in registry entry, not a data-pack file, so it is
/// absent from `template_pools.json` and has no entries.
fn is_builtin_empty_pool(name: &str) -> bool {
    normalized_id(name) == "minecraft:empty"
}

/// `SequencedPriorityIterator`: highest priority first, FIFO within a
/// priority. `BTreeMap` iteration is ordered, but only the highest non-empty
/// queue can ever be selected, so this is equivalent to Java's hash map scan.
#[derive(Debug, Default)]
pub struct SequencedPriorityQueue<T> {
    queues: BTreeMap<i32, VecDeque<T>>,
}

impl<T> SequencedPriorityQueue<T> {
    pub fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, value: T, priority: i32) {
        self.queues.entry(priority).or_default().push_back(value);
    }

    pub fn pop(&mut self) -> Option<T> {
        let priority = *self.queues.keys().next_back()?;
        let queue = self.queues.get_mut(&priority)?;
        let value = queue.pop_front();
        if queue.is_empty() {
            self.queues.remove(&priority);
        }
        value
    }

    pub fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }
}

// Re-exported placement result types keep `jigsaw` self-contained for tests.
pub type JigsawPlacementResult = PiecePlacement;
pub type JigsawDataMarker = StructureDataMarker;
pub type JigsawPlacedBlock = PlacedBlock;
pub type JigsawPlacedEntity = PlacedEntity;
