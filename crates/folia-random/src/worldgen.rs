//! `WorldgenRandom`: the feature-placement wrapper around xoroshiro128++.
//!
//! Ports `net.minecraft.world.level.levelgen.WorldgenRandom` for the
//! Xoroshiro-backed path used by chunk decoration. The wrapper's
//! `setSeed` replaces the underlying generator state, so every placed feature
//! starts from a fresh, fully deterministic stream.

use crate::{RandomSource, XoroshiroRandomSource};

/// Vanilla's feature-placement random source.
#[derive(Debug, Clone)]
pub struct WorldgenRandom {
    random: XoroshiroRandomSource,
}

impl WorldgenRandom {
    /// Creates a source with an initial seed. The seed is replaced by
    /// [`Self::set_decoration_seed`] before decoration in normal use.
    pub fn new(seed: i64) -> Self {
        Self {
            random: XoroshiroRandomSource::new(seed),
        }
    }

    /// Returns the next `bits` high bits of an underlying xoroshiro draw.
    ///
    /// Java: `WorldgenRandom.next(int)`, which delegates to the wrapped
    /// source's `nextLong` and keeps the high bits.
    fn next_bits(&mut self, bits: u32) -> i64 {
        self.random.next_bits(bits)
    }

    /// Replaces the underlying xoroshiro state.
    ///
    /// Java: `WorldgenRandom.setSeed(long)`.
    pub fn set_seed(&mut self, seed: i64) {
        self.random.set_seed(seed);
    }

    /// Seeds a decoration pass for one chunk and returns the decoration seed.
    ///
    /// Java: `WorldgenRandom.setDecorationSeed(long, int, int)`. Both
    /// `nextLong` draws come from the Xoroshiro source and are forced odd
    /// before they participate in the mix.
    pub fn set_decoration_seed(
        &mut self,
        level_seed: i64,
        min_chunk_block_x: i32,
        min_chunk_block_z: i32,
    ) -> i64 {
        self.set_seed(level_seed);
        let x_factor = self.next_long() | 1;
        let z_factor = self.next_long() | 1;
        let seed = i64::from(min_chunk_block_x)
            .wrapping_mul(x_factor)
            .wrapping_add(i64::from(min_chunk_block_z).wrapping_mul(z_factor))
            ^ level_seed;
        self.set_seed(seed);
        seed
    }

    /// Reseeds for one placed feature within a decoration step.
    ///
    /// Java: `WorldgenRandom.setFeatureSeed(long, int, int)`.
    pub fn set_feature_seed(&mut self, decoration_seed: i64, index: i32, decoration_step: i32) {
        let seed = decoration_seed
            .wrapping_add(i64::from(index))
            .wrapping_add(10_000_i64.wrapping_mul(i64::from(decoration_step)));
        self.set_seed(seed);
    }
}

impl RandomSource for WorldgenRandom {
    fn next_int(&mut self) -> i32 {
        self.next_bits(32) as i32
    }

    fn next_int_bound(&mut self, bound: i32) -> i32 {
        crate::assert_bound_positive(bound);

        if (bound & (bound - 1)) == 0 {
            return ((i64::from(bound) * self.next_bits(31)) >> 31) as i32;
        }

        loop {
            let value = self.next_bits(31) as i32;
            let remainder = value % bound;
            if value.wrapping_sub(remainder).wrapping_add(bound - 1) >= 0 {
                return remainder;
            }
        }
    }

    fn next_long(&mut self) -> i64 {
        let high = self.next_bits(32) as i32 as i64;
        let low = self.next_bits(32) as i32 as i64;
        (high << 32).wrapping_add(low)
    }

    fn next_double(&mut self) -> f64 {
        let high = self.next_bits(26);
        let low = self.next_bits(27);
        let value = (high << 27).wrapping_add(low);
        ((value as f32) * 1.110_223e-16_f32) as f64
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.960_464_5e-8_f32
    }

    fn next_boolean(&mut self) -> bool {
        self.next_bits(1) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values from the Java oracle:
    /// `new WorldgenRandom(new XoroshiroRandomSource(42L))`,
    /// `setDecorationSeed(123456789L, 1, -2)`, one draw of each kind, then
    /// `setFeatureSeed(decorationSeed, 17, 6)` and four more draws.
    #[test]
    fn matches_the_java_oracle() {
        let mut random = WorldgenRandom::new(42);
        let decoration_seed = random.set_decoration_seed(123_456_789, 1, -2);
        assert_eq!(decoration_seed, 8_798_328_949_467_112_410);
        assert_eq!(random.next_long(), -3_299_454_464_332_781_919);
        assert_eq!(random.next_int(), -1_833_333_545);
        assert_eq!(random.next_float(), 0.634_996_1);

        random.set_feature_seed(decoration_seed, 17, 6);
        assert_eq!(random.next_long(), 1_934_728_442_425_403_754);
        assert_eq!(random.next_int_bound(100), 35);
        assert_eq!(random.next_double(), 0.820_681_571_960_449_2);
        assert!(!random.next_boolean());
    }

    #[test]
    fn ore_seed_matches_the_java_oracle() {
        let mut random = WorldgenRandom::new(0);
        let seed = random.set_decoration_seed(12_345, 0, -16);
        assert_eq!(seed, 7_486_895_795_072_515_401);
        random.set_feature_seed(seed, 0, 6);
        assert_eq!(random.next_float(), 0.940_056_8);
        assert_eq!(random.next_int_bound(16), 7);
        assert_eq!(random.next_int_bound(16), 14);
        assert_eq!(random.next_int_bound(3), 1);
        assert_eq!(random.next_double(), 0.801_710_844_039_917);
        assert_eq!(random.next_float(), 0.671_327_53);
        random.set_feature_seed(seed, 1, 6);
        assert_eq!(random.next_float(), 0.557_729);
        assert_eq!(random.next_int_bound(16), 11);
    }

    #[test]
    fn dirt_height_matches_the_java_oracle() {
        let mut random = WorldgenRandom::new(0);
        let seed = random.set_decoration_seed(12_345, 0, -16);
        random.set_feature_seed(seed, 0, 6);
        assert_eq!(random.next_int_bound(16), 15);
        assert_eq!(random.next_int_bound(16), 7);
        assert_eq!(random.next_int_bound(161), 135);
    }

    #[test]
    fn feature_seed_resets_the_stream() {
        let mut first = WorldgenRandom::new(0);
        first.set_feature_seed(1234, 5, 6);
        let mut second = WorldgenRandom::new(999);
        second.set_feature_seed(1234, 5, 6);
        for _ in 0..8 {
            assert_eq!(first.next_long(), second.next_long());
        }
    }
}
