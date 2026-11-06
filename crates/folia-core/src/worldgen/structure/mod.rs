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
//!
//! Jigsaw structures parse their definition but fail loudly with
//! [`StructureError::UnsupportedJigsaw`] instead of returning an empty start.
//! Concentric-ring placement parses, but it needs a biome source and is skipped
//! by [`set::StructureSetRegistry::structure_at_chunk`].
//!
//! Data markers (for example the igloo chest) are returned in
//! [`piece::PiecePlacement::data_markers`] rather than being turned into block
//! entities, and template entities are returned rather than spawned. Those
//! remain explicit later slices.

pub mod piece;
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
                 template-pool and jigsaw expansion are not implemented"
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
        }
    }
}

impl std::error::Error for StructureError {}

pub use piece::{
    BoundingBox, IglooPart, PiecePlacement, StructureDataMarker, StructurePiece,
    StructurePieceKind, TemplateBlockFilter, TemplatePostProcess, TemplateStructurePiece,
    template_bounding_box,
};
pub use set::{
    ConcentricRingsStructurePlacement, ExclusionZone, FrequencyReductionMethod, PlacementError,
    RandomSpreadStructurePlacement, RandomSpreadType, SelectedStructure, StructurePlacement,
    StructureSelectionEntry, StructureSet, StructureSetConfig, StructureSetError,
    StructureSetRegistry,
};
pub use start::{
    BiomeSet, IglooStructure, StartPlacement, StructureDefinition, StructureGenerationContext,
    StructureKind, StructureStart, TemplateProvider, TerrainAdjustment,
};
pub use template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructureTemplate,
    TemplateBlock, TemplateEntity, transform, transform_state, transform_vec3,
};
