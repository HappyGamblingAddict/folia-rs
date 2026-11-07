//! Structure starts, pieces, sets, and templates.
//!
//! This is the second P2.8 slice. It builds the start/piece/set foundation on
//! top of [`template`]:
//!
//! - [`set`] parses the shipped `worldgen/structure_set` JSON, ports
//!   `RandomSpreadStructurePlacement` (linear and triangular spread, legacy
//!   frequency reducers, exclusion zones) and selects structures with vanilla's
//!   weighted `WorldgenRandom` draw.
//! - [`piece`] ports the `BoundingBox` and `StructurePiece` data model, plus
//!   template pieces (`TemplateStructurePiece`) with Java's per-piece rotation,
//!   mirror, rotation pivot, and chunk-box write filtering.
//! - [`start`] ports `StructureStart`, deterministic generation contexts, and
//!   one non-jigsaw template structure end to end: the igloo
//!   (`IglooStructure` + `IglooPieces`).
//! - [`processor`] parses the shipped processor-list catalog and applies every
//!   registered 1.21.11 processor kind over Java's raw-block chain, including
//!   `capped` finalization.
//! - [`pool`] parses the shipped template-pool catalog, implements Java's
//!   weighted element selection, and resolves one start-pool element through
//!   the template-piece path.
//!
//! Jigsaw structures parse their definition but fail loudly with
//! [`StructureError::UnsupportedJigsaw`] instead of returning an empty start.
//! [`pool::PoolCatalog::expand_start`] resolves and places one pool element;
//! recursive jigsaw connection, junction expansion, and terrain adaptation are
//! explicit later slices. Concentric-ring placement parses, but it needs a
//! biome source and is skipped by
//! [`set::StructureSetRegistry::structure_at_chunk`].
//!
//! Data markers (for example the igloo chest) are returned in
//! [`piece::PiecePlacement::data_markers`] rather than being turned into block
//! entities, and template entities are returned rather than spawned. Those
//! remain explicit later slices.

pub mod assets;
pub mod jigsaw;
pub mod piece;
pub mod pool;
pub mod processor;
pub mod set;
pub mod start;
pub mod template;

use std::fmt;

/// Errors shared by structure definitions, generation, and placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureError {
    /// A structure JSON document is malformed.
    InvalidDefinition(String),
    /// A template referenced by a structure is not available.
    UnknownTemplate { structure: String, template: String },
    /// A jigsaw structure needs the unported template-pool expander.
    UnsupportedJigsaw {
        structure: String,
        start_pool: String,
    },
    /// A non-jigsaw structure type has no Rust implementation yet.
    UnsupportedStructureType {
        structure: String,
        type_name: String,
    },
    /// A piece kind cannot be placed by this slice.
    UnsupportedPiece { piece_type: String },
    /// A template pool was referenced before it was registered.
    UnknownPool { pool: String },
    /// A referenced template pool has no elements.
    EmptyPool { pool: String },
    /// `start_jigsaw_name` did not match any connector in the start element.
    StartJigsawNotFound { structure: String, name: String },
    /// Jigsaw expansion needs a level for heightmap/dimension queries.
    MissingHeightSource { structure: String },
    /// A jigsaw start was placed without its template/processor catalogs.
    JigsawRequiresTemplates { structure: String },
    /// A pool element kind cannot be expanded/placed by this slice.
    UnsupportedPoolElement { pool: String, element_type: String },
    /// A `feature_pool_element` cannot execute its placed feature.
    UnsupportedFeatureElement {
        pool: String,
        feature: String,
        reason: String,
    },
    /// `pool_aliases` are not implemented.
    UnsupportedPoolAliases { structure: String },
    /// A `start_height` provider kind is not implemented.
    UnsupportedHeightProvider {
        structure: String,
        type_name: String,
    },
    /// A template-pool placement error.
    Pool { message: String },
    /// A structure-set or biome-filter error.
    StructureSet { message: String },
    /// A structure has no biome rule in the shipped catalog.
    MissingBiomeRule { structure: String },
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StructureError::InvalidDefinition(message) => {
                write!(formatter, "invalid structure definition: {message}")
            }
            StructureError::UnknownTemplate {
                structure,
                template,
            } => write!(
                formatter,
                "structure `{structure}` references missing template `{template}`"
            ),
            StructureError::UnsupportedJigsaw {
                structure,
                start_pool,
            } => write!(
                formatter,
                "structure `{structure}` is a jigsaw structure (start pool `{start_pool}`); \
                 recursive jigsaw expansion is not implemented"
            ),
            StructureError::UnsupportedStructureType {
                structure,
                type_name,
            } => write!(
                formatter,
                "structure `{structure}` has unsupported type `{type_name}`"
            ),
            StructureError::UnsupportedPiece { piece_type } => {
                write!(
                    formatter,
                    "structure piece type `{piece_type}` is not supported"
                )
            }
            StructureError::UnknownPool { pool } => {
                write!(formatter, "unknown structure template pool `{pool}`")
            }
            StructureError::EmptyPool { pool } => {
                write!(
                    formatter,
                    "structure template pool `{pool}` has no elements"
                )
            }
            StructureError::StartJigsawNotFound { structure, name } => write!(
                formatter,
                "structure `{structure}` start element has no jigsaw named `{name}`"
            ),
            StructureError::MissingHeightSource { structure } => write!(
                formatter,
                "structure `{structure}` needs a level/height source for jigsaw expansion"
            ),
            StructureError::JigsawRequiresTemplates { structure } => write!(
                formatter,
                "structure `{structure}` is a jigsaw start; use place_jigsaw_in_chunk with its \
                 template and processor catalogs"
            ),
            StructureError::UnsupportedPoolElement { pool, element_type } => write!(
                formatter,
                "template pool `{pool}` selected unsupported element `{element_type}`"
            ),
            StructureError::UnsupportedFeatureElement {
                pool,
                feature,
                reason,
            } => write!(
                formatter,
                "template pool `{pool}` selected feature element `{feature}`, which cannot be \
                 placed: {reason}"
            ),
            StructureError::UnsupportedPoolAliases { structure } => write!(
                formatter,
                "structure `{structure}` uses pool aliases, which are not implemented"
            ),
            StructureError::UnsupportedHeightProvider {
                structure,
                type_name,
            } => write!(
                formatter,
                "structure `{structure}` uses unsupported start_height provider `{type_name}`"
            ),
            StructureError::Pool { message } => {
                write!(formatter, "structure pool error: {message}")
            }
            StructureError::StructureSet { message } => {
                write!(formatter, "structure set error: {message}")
            }
            StructureError::MissingBiomeRule { structure } => write!(
                formatter,
                "structure `{structure}` has no shipped biome rule"
            ),
        }
    }
}

impl std::error::Error for StructureError {}

pub use assets::{
    CatalogError, EMBEDDED_STRUCTURE_TEMPLATES, StructureTemplateCatalog, normalize_structure_id,
};
pub use jigsaw::{
    JigsawBlockInfo, JigsawConfig, JigsawJunction, JigsawPiece, JigsawPiecePlacement,
    JigsawPlacement, JigsawStart, JointType, SequencedPriorityQueue, element_bounds, front_facing,
    shuffle, top_facing,
};
pub use piece::{
    BoundingBox, IglooPart, PiecePlacement, StructureDataMarker, StructurePiece,
    StructurePieceKind, TemplateBlockFilter, TemplatePostProcess, TemplateStructurePiece,
    template_bounding_box,
};
pub use pool::{
    LiquidSettings, PoolCatalog, PoolCatalogIndex, PoolElement, PoolElementEntry, PoolError,
    PoolExpansion, PoolPlacement, ProcessorListRef, Projection, StructureTemplatePool,
};
pub use processor::{
    BlockEntityModifier, BlockSet, IntProvider, PositionTest, ProcessedBlock,
    ProcessorApplyContext, ProcessorCatalogIndex, ProcessorError, ProcessorRule, RawBlockInfo,
    RuleTest, StructureProcessor, StructureProcessorCatalog, StructureProcessorList,
    apply_processors, template_block_infos,
};
pub use set::{
    ConcentricRingsStructurePlacement, ExclusionZone, FrequencyReductionMethod, PlacementError,
    RandomSpreadStructurePlacement, RandomSpreadType, SHIPPED_STRUCTURE_BIOME_RULES,
    SelectedStructure, StructureBiomeLookup, StructureBiomeRule, StructureDimension,
    StructurePlacement, StructureSelectionEntry, StructureSet, StructureSetConfig,
    StructureSetError, StructureSetRegistry, set_dimension, structure_biome_rule,
};
pub use start::{
    BiomeSet, DesertPyramidPiece, DesertPyramidStart, IglooStructure, SHIPPED_STRUCTURE_JSON,
    StartPlacement, StructureChunkPlacement, StructureDefinition, StructureGenerationContext,
    StructureGenerator, StructureKind, StructureStart, TemplateProvider, TerrainAdjustment,
};
pub use template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructureTemplate,
    TemplateBlock, TemplateEntity, transform, transform_state, transform_vec3,
};
