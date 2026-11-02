//! The classic 48-bit linear congruential random source.

use crate::{RandomSource, assert_bound_positive};

/// Number of significant state bits in the LCG.
pub const MODULUS_BITS: u32 = 48;

/// Mask selecting the low 48 bits of the LCG state.
pub const MODULUS_MASK: i64 = (1 << MODULUS_BITS) - 1;

/// LCG multiplier (`0x5DEECE66D`).
pub const MULTIPLIER: i64 = 25_214_903_917;

/// LCG increment.
pub const INCREMENT: i64 = 11;

/// The classic Minecraft/Java `Random`-style random source.
///
/// This is an exact port of
/// `net.minecraft.world.level.levelgen.LegacyRandomSource`.
#[derive(Debug, Clone)]
pub struct LegacyRandomSource {
    seed: i64,
}

impl LegacyRandomSource {
    /// Creates a source seeded exactly like Java's
    /// `new LegacyRandomSource(seed)`.
    pub const fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ MULTIPLIER) & MODULUS_MASK,
        }
    }

    /// Reseeds the generator, discarding all previous state.
    ///
    /// Java: `LegacyRandomSource.setSeed(long)`.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ MULTIPLIER) & MODULUS_MASK;
    }

    /// Returns the next `bits` high bits of the LCG state.
    ///
    /// The state is advanced first, then the high bits are returned. The
    /// result is sign-extended just like Java's `(int)` cast.
    ///
    /// `bits` must be in `1..=32`. The reference implementation only calls
    /// this with 1, 24, 26, 27, 31, and 32.
    ///
    /// Java: `BitRandomSource.next(int)`.
    pub fn next_bits(&mut self, bits: u32) -> i32 {
        assert!((1..=32).contains(&bits), "bits must be between 1 and 32");

        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT) & MODULUS_MASK;
        (self.seed >> (MODULUS_BITS - bits)) as i32
    }

    /// Creates a child source seeded from the next `nextLong` value.
    ///
    /// Java: `LegacyRandomSource.fork()`.
    pub fn fork(&mut self) -> Self {
        Self::new(self.next_long())
    }
}

impl RandomSource for LegacyRandomSource {
    fn next_int(&mut self) -> i32 {
        self.next_bits(32)
    }

    fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert_bound_positive(bound);

        if (bound & (bound - 1)) == 0 {
            // Java: (int)((long)bound * this.next(31) >> 31)
            return ((bound as i64).wrapping_mul(self.next_bits(31) as i64) >> 31) as i32;
        }

        loop {
            let value = self.next_bits(31);
            let remainder = value % bound;

            // Java int arithmetic overflows here for large bounds; the
            // wrapping operations reproduce that exactly.
            if value.wrapping_sub(remainder).wrapping_add(bound - 1) >= 0 {
                return remainder;
            }
        }
    }

    fn next_long(&mut self) -> i64 {
        let high = self.next_bits(32) as i64;
        let low = self.next_bits(32) as i64;
        (high << 32).wrapping_add(low)
    }

    fn next_double(&mut self) -> f64 {
        // Java deliberately computes this in f32 and widens on return.
        let high = self.next_bits(26) as i64;
        let low = self.next_bits(27) as i64;
        let value = (high << 27).wrapping_add(low);
        ((value as f32) * 1.110223e-16_f32) as f64
    }

    fn next_float(&mut self) -> f32 {
        (self.next_bits(24) as f32) * 5.9604645e-8_f32
    }

    fn next_boolean(&mut self) -> bool {
        self.next_bits(1) != 0
    }
}
