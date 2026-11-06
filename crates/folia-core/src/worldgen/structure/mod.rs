//! Structure templates (`StructureTemplate`).
//!
//! This is the first P2.8 slice: loading vanilla gzip-compressed template NBT
//! and placing their blocks with rotation/mirror settings. Structure starts,
//! pieces, structure sets, references, processor lists, template pools, and
//! jigsaw expansion are **not** ported yet. `place_into` returns block-entity
//! and entity NBT to the caller instead of pretending those objects were
//! created.
//!
//! Multi-palette templates load and validate all of their palettes, but
//! placement uses the first palette. Vanilla picks a palette through
//! `StructurePlaceSettings.getRandomPalette`, which belongs to the structure
//! piece pipeline.

pub mod template;

pub use template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructurePlacement,
    StructureTemplate, TemplateBlock, TemplateEntity, transform, transform_state, transform_vec3,
};
