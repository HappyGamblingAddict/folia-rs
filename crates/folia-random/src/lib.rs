//! Folia-RS random number sources.
//!
//! Ports of the random sources used by Minecraft/Folia:
//!
//! * [`LegacyRandomSource`] — the classic 48-bit linear congruential
//!   generator (`java.util.Random`-style `next(bits)`).
//! * [`XoroshiroRandomSource`] — xoroshiro128++, seeded through
//!   [`upgrade_seed_to_128bit`].
//! * [`Xoroshiro128PlusPlus`] — the raw xoroshiro128++ state transition.
//!
//! All 64-bit arithmetic wraps exactly like Java's `long` arithmetic, and
//! conversions to `f32`/`f64` happen at the same points as in the reference
//! implementation. This crate intentionally has no dependencies.
//!
//! Not ported yet: the Marsaglia polar Gaussian generator (`nextGaussian`).
//! The legacy and xoroshiro positional factories (`at`, `fromHashOf`,
//! `fromSeed`) are available for worldgen noise construction.

mod gaussian;
mod legacy;
mod support;
mod worldgen;
mod xoroshiro;

pub use gaussian::MarsagliaPolarGaussian;
pub use legacy::{LegacyPositionalRandomFactory, LegacyRandomSource};
pub use md5::md5;
pub use support::{
    GOLDEN_RATIO_64, SILVER_RATIO_64, Seed128bit, mix_stafford13, seed_from_hash_of,
    upgrade_seed_to_128bit, upgrade_seed_to_128bit_unmixed,
};
pub use worldgen::WorldgenRandom;
pub use xoroshiro::{
    Xoroshiro128PlusPlus, XoroshiroPositionalRandomFactory, XoroshiroRandomSource, get_seed,
};

mod md5;

/// Common interface for Folia-RS random sources.
///
/// This mirrors the six core methods of Minecraft's
/// `net.minecraft.util.RandomSource`.
pub trait RandomSource {
    /// Returns the next uniformly distributed `i32`.
    ///
    /// Java: `RandomSource.nextInt()`.
    fn next_int(&mut self) -> i32;

    /// Returns a uniformly distributed value in `0..bound`.
    ///
    /// # Panics
    ///
    /// Panics with `"Bound must be positive"` when `bound <= 0`, matching
    /// Java's `IllegalArgumentException`.
    ///
    /// Java: `RandomSource.nextInt(int bound)`.
    fn next_int_bound(&mut self, bound: i32) -> i32;

    /// Returns the next uniformly distributed `i64`.
    ///
    /// Java: `RandomSource.nextLong()`.
    fn next_long(&mut self) -> i64;

    /// Returns a uniformly distributed value in `[0.0, 1.0)`.
    ///
    /// Java: `RandomSource.nextDouble()`.
    fn next_double(&mut self) -> f64;

    /// Returns a uniformly distributed value in `[0.0, 1.0)`.
    ///
    /// Java: `RandomSource.nextFloat()`.
    fn next_float(&mut self) -> f32;

    /// Returns the next uniformly distributed boolean.
    ///
    /// Java: `RandomSource.nextBoolean()`.
    fn next_boolean(&mut self) -> bool;

    /// Returns the next standard-normal sample.
    ///
    /// Java: `RandomSource.nextGaussian()`.
    fn next_gaussian(&mut self) -> f64;

    /// Returns a value in `min..=max`.
    ///
    /// Mirrors Java's `RandomSource.nextIntBetweenInclusive(int, int)`,
    /// including wrapping `i32` arithmetic.
    fn next_int_between_inclusive(&mut self, min: i32, max: i32) -> i32 {
        let bound = max.wrapping_sub(min).wrapping_add(1);
        self.next_int_bound(bound).wrapping_add(min)
    }

    /// Returns a value in `origin..bound`.
    ///
    /// # Panics
    ///
    /// Panics when `origin >= bound`, matching Java's
    /// `IllegalArgumentException("bound - origin is non positive")`.
    ///
    /// Java: `RandomSource.nextInt(int origin, int bound)`.
    fn next_int_in_range(&mut self, origin: i32, bound: i32) -> i32 {
        assert!(origin < bound, "bound - origin is non positive");
        self.next_int_bound(bound.wrapping_sub(origin))
            .wrapping_add(origin)
    }

    /// Advances the source by `count` draws.
    ///
    /// The default implementation draws `next_int`, matching the default
    /// method on Java's `RandomSource`. [`XoroshiroRandomSource`] overrides
    /// it to draw whole `nextLong` values, as the Java implementation does.
    fn consume_count(&mut self, count: u32) {
        for _ in 0..count {
            self.next_int();
        }
    }
}

/// Shared bound validation. Java throws `IllegalArgumentException`; Rust
/// panics with the same message.
pub(crate) fn assert_bound_positive(bound: i32) {
    assert!(bound > 0, "Bound must be positive");
}
