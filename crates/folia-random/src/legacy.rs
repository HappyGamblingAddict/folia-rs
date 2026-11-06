//! The classic 48-bit linear congruential random source.

use crate::{MarsagliaPolarGaussian, RandomSource, assert_bound_positive};

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
    gaussian: MarsagliaPolarGaussian,
}

impl LegacyRandomSource {
    /// Creates a source seeded exactly like Java's
    /// `new LegacyRandomSource(seed)`.
    pub const fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ MULTIPLIER) & MODULUS_MASK,
            gaussian: MarsagliaPolarGaussian::new(),
        }
    }

    /// Reseeds the generator, discarding all previous state.
    ///
    /// Java: `LegacyRandomSource.setSeed(long)`.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ MULTIPLIER) & MODULUS_MASK;
        self.gaussian.reset();
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

    /// Creates a positional factory seeded from the next `nextLong` value.
    ///
    /// Java: `LegacyRandomSource.forkPositional()`.
    pub fn fork_positional(&mut self) -> LegacyPositionalRandomFactory {
        LegacyPositionalRandomFactory {
            seed: self.next_long(),
        }
    }
}

/// `LegacyRandomSource.LegacyPositionalRandomFactory`.
#[derive(Debug, Clone)]
pub struct LegacyPositionalRandomFactory {
    seed: i64,
}

impl LegacyPositionalRandomFactory {
    /// `PositionalRandomFactory.at(int, int, int)`.
    pub fn at(&self, x: i32, y: i32, z: i32) -> LegacyRandomSource {
        LegacyRandomSource::new(mth_get_seed(x, y, z) ^ self.seed)
    }

    /// `PositionalRandomFactory.fromHashOf(String)`.
    pub fn from_hash_of(&self, name: &str) -> LegacyRandomSource {
        LegacyRandomSource::new(i64::from(java_string_hash(name)) ^ self.seed)
    }

    /// `PositionalRandomFactory.fromSeed(long)`.
    pub fn from_seed(&self, seed: i64) -> LegacyRandomSource {
        LegacyRandomSource::new(seed)
    }
}

/// `Mth.getSeed(int, int, int)`.
fn mth_get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut value = i64::from(x.wrapping_mul(3_129_871))
        ^ i64::from(z).wrapping_mul(116_129_781)
        ^ i64::from(y);
    value = value
        .wrapping_mul(value)
        .wrapping_mul(42_317_861)
        .wrapping_add(value.wrapping_mul(11));
    value >> 16
}

/// Java `String.hashCode()`.
fn java_string_hash(value: &str) -> i32 {
    value.encode_utf16().fold(0i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
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

    fn next_gaussian(&mut self) -> f64 {
        let mut gaussian = std::mem::take(&mut self.gaussian);
        let value = gaussian.next(self);
        self.gaussian = gaussian;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_factory_uses_legacy_seed_math() {
        let mut first = LegacyRandomSource::new(42);
        let factory = first.fork_positional();

        let mut expected_parent = LegacyRandomSource::new(42);
        let factory_seed = expected_parent.next_long();

        let from_seed = factory.from_seed(7).next_long();
        assert_eq!(from_seed, LegacyRandomSource::new(7).next_long());

        let at = factory.at(1, 2, 3).next_long();
        assert_eq!(
            at,
            LegacyRandomSource::new(mth_get_seed(1, 2, 3) ^ factory_seed).next_long()
        );

        let hashed = factory.from_hash_of("minecraft:aquifer").next_long();
        assert_eq!(
            hashed,
            LegacyRandomSource::new(
                i64::from(java_string_hash("minecraft:aquifer")) ^ factory_seed
            )
            .next_long()
        );
    }

    #[test]
    fn gaussian_matches_java() {
        let mut random = LegacyRandomSource::new(42);
        let expected = [
            1.141_905_249_038_952_5,
            0.919_407_969_973_102_7,
            -0.949_866_585_923_176_6,
            -1.106_990_284_364_309_4,
            0.280_977_589_971_606_55,
            0.684_622_599_980_920_8,
        ];
        for value in expected {
            let actual = random.next_gaussian();
            assert!(
                (actual - value).abs() < 1e-14,
                "expected {value}, got {actual}"
            );
        }
    }

    #[test]
    fn string_hash_matches_java() {
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("a"), 97);
        assert_eq!(java_string_hash("abc"), 96354);
        assert_eq!(java_string_hash("minecraft:aquifer"), -1_973_797_502);
    }
}
