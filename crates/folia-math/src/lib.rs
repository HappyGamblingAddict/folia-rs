//! Folia-RS coordinate and vector math.
//!
//! This crate is a dependency-free port of the coordinate, direction, and
//! vector primitives used by Minecraft/Folia:
//!
//! - [`BlockPos`][]: `net.minecraft.core.BlockPos`
//! - [`SectionPos`][]: `net.minecraft.core.SectionPos`
//! - [`Direction`] / [`Axis`]: `net.minecraft.core.Direction`
//! - [`Vec3`][]: `net.minecraft.world.phys.Vec3`
//! - [`AABB`][]: `net.minecraft.world.phys.AABB`
//! - [`mth`]: selected helpers from `net.minecraft.util.Mth`
//!
//! Bit packing intentionally mirrors the Java bit layouts exactly, including
//! sign extension when unpacking coordinates.

pub mod aabb;
pub mod block_pos;
pub mod direction;
pub mod mth;
pub mod section_pos;
pub mod vec3;

pub use aabb::AABB;
pub use block_pos::BlockPos;
pub use direction::{Axis, Direction};
pub use section_pos::SectionPos;
pub use vec3::Vec3;
