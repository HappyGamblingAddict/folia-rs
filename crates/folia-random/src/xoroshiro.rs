//! The xoroshiro128++ random source and its raw state transition.

use crate::support::{GOLDEN_RATIO_64, SILVER_RATIO_64, Seed128bit, upgrade_seed_to_128bit};
use crate::{MarsagliaPolarGaussian, RandomSource, assert_bound_positive};

/// Raw xoroshiro128++ generator state.
///
/// Exact port of
/// `net.minecraft.world.level.levelgen.Xoroshiro128PlusPlus`.
#[derive(Debug, Clone)]
pub struct Xoroshiro128PlusPlus {
    seed_lo: i64,
    seed_hi: i64,
}

impl Xoroshiro128PlusPlus {
    /// Creates a generator, replacing an all-zero state with the golden and
    /// silver ratio constants (xoroshiro cannot leave the zero state).
    ///
    /// Java: `new Xoroshiro128PlusPlus(long seedLo, long seedHi)`.
    pub fn new(seed_lo: i64, seed_hi: i64) -> Self {
        if (seed_lo | seed_hi) == 0 {
            Self {
                seed_lo: GOLDEN_RATIO_64,
                seed_hi: SILVER_RATIO_64,
            }
        } else {
            Self { seed_lo, seed_hi }
        }
    }

    /// Creates a generator from a `Seed128bit`.
    ///
    /// Java: `new Xoroshiro128PlusPlus(RandomSupport.Seed128bit)`.
    pub fn from_seed(seed: Seed128bit) -> Self {
        Self::new(seed.seed_lo, seed.seed_hi)
    }

    /// Returns the low half of the current state.
    pub const fn seed_lo(&self) -> i64 {
        self.seed_lo
    }

    /// Returns the high half of the current state.
    pub const fn seed_hi(&self) -> i64 {
        self.seed_hi
    }

    /// Advances the generator and returns the next 64-bit value.
    ///
    /// Java: `Xoroshiro128PlusPlus.nextLong()`.
    pub fn next_long(&mut self) -> i64 {
        let seed_lo = self.seed_lo;
        let mut seed_hi = self.seed_hi;

        let result = seed_lo
            .wrapping_add(seed_hi)
            .rotate_left(17)
            .wrapping_add(seed_lo);

        seed_hi ^= seed_lo;
        self.seed_lo = seed_lo.rotate_left(49) ^ seed_hi ^ seed_hi.wrapping_shl(21);
        self.seed_hi = seed_hi.rotate_left(28);

        result
    }
}

impl From<Seed128bit> for Xoroshiro128PlusPlus {
    fn from(seed: Seed128bit) -> Self {
        Self::from_seed(seed)
    }
}

/// Minecraft/Folia's xoroshiro128++ random source.
///
/// Exact port of
/// `net.minecraft.world.level.levelgen.XoroshiroRandomSource`.
#[derive(Debug, Clone)]
pub struct XoroshiroRandomSource {
    generator: Xoroshiro128PlusPlus,
    gaussian: MarsagliaPolarGaussian,
}

impl XoroshiroRandomSource {
    /// Creates a source from a single seed, mixing it into 128 bits first.
    ///
    /// Java: `new XoroshiroRandomSource(long seed)`.
    pub fn new(seed: i64) -> Self {
        Self {
            generator: Xoroshiro128PlusPlus::from_seed(upgrade_seed_to_128bit(seed)),
            gaussian: MarsagliaPolarGaussian::new(),
        }
    }

    /// Creates a source from an already-upgraded 128-bit seed.
    ///
    /// Java: `new XoroshiroRandomSource(RandomSupport.Seed128bit)`.
    pub fn from_seed_128(seed: Seed128bit) -> Self {
        Self {
            generator: Xoroshiro128PlusPlus::from_seed(seed),
            gaussian: MarsagliaPolarGaussian::new(),
        }
    }

    /// Creates a source from raw low/high halves, without seed mixing.
    ///
    /// Java: `new XoroshiroRandomSource(long seedLo, long seedHi)`.
    pub fn from_parts(seed_lo: i64, seed_hi: i64) -> Self {
        Self {
            generator: Xoroshiro128PlusPlus::new(seed_lo, seed_hi),
            gaussian: MarsagliaPolarGaussian::new(),
        }
    }

    /// Reseeds the source, mixing the seed into 128 bits again.
    ///
    /// Java: `XoroshiroRandomSource.setSeed(long)`.
    pub fn set_seed(&mut self, seed: i64) {
        self.generator = Xoroshiro128PlusPlus::from_seed(upgrade_seed_to_128bit(seed));
        self.gaussian.reset();
    }

    /// Returns the raw xoroshiro128++ state.
    pub const fn generator(&self) -> &Xoroshiro128PlusPlus {
        &self.generator
    }

    /// Returns the next `bits` high bits of a raw `nextLong` draw.
    ///
    /// Java: `XoroshiroRandomSource.nextBits(int)` (private there).
    pub fn next_bits(&mut self, bits: u32) -> i64 {
        assert!((1..=64).contains(&bits), "bits must be between 1 and 64");
        (self.generator.next_long() as u64 >> (64 - bits)) as i64
    }

    /// Creates a child source from two raw `nextLong` draws.
    ///
    /// Java: `XoroshiroRandomSource.fork()`.
    pub fn fork(&mut self) -> Self {
        let seed_lo = self.generator.next_long();
        let seed_hi = self.generator.next_long();
        Self::from_parts(seed_lo, seed_hi)
    }

    /// Creates a positional random factory from two raw `nextLong` draws.
    ///
    /// Java: `XoroshiroRandomSource.forkPositional()`.
    pub fn fork_positional(&mut self) -> XoroshiroPositionalRandomFactory {
        let seed_lo = self.generator.next_long();
        let seed_hi = self.generator.next_long();
        XoroshiroPositionalRandomFactory { seed_lo, seed_hi }
    }
}

/// Positional random factory matching
/// `XoroshiroRandomSource.XoroshiroPositionalRandomFactory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XoroshiroPositionalRandomFactory {
    seed_lo: i64,
    seed_hi: i64,
}

impl XoroshiroPositionalRandomFactory {
    /// Creates a source from a name hash, exactly like
    /// `PositionalRandomFactory.fromHashOf(String)`.
    pub fn from_hash_of(&self, name: &str) -> XoroshiroRandomSource {
        let seed = crate::seed_from_hash_of(name).xor(self.seed_lo, self.seed_hi);
        XoroshiroRandomSource::from_seed_128(seed)
    }

    /// Creates a source from a positional seed.
    ///
    /// Java: `PositionalRandomFactory.at(int, int, int)`.
    pub fn at(&self, x: i32, y: i32, z: i32) -> XoroshiroRandomSource {
        let seed = get_seed(x, y, z);
        XoroshiroRandomSource::from_parts(seed ^ self.seed_lo, self.seed_hi)
    }

    /// Creates a source from an explicit seed.
    ///
    /// Java: `PositionalRandomFactory.fromSeed(long)`.
    pub fn from_seed(&self, seed: i64) -> XoroshiroRandomSource {
        XoroshiroRandomSource::from_parts(seed ^ self.seed_lo, seed ^ self.seed_hi)
    }

    pub const fn seed_lo(&self) -> i64 {
        self.seed_lo
    }

    pub const fn seed_hi(&self) -> i64 {
        self.seed_hi
    }
}

/// Minecraft's `Mth.getSeed(int, int, int)`.
///
/// The `x * 3129871` multiply is intentionally a 32-bit wrapping multiply,
/// while the `z * 116129781` multiply is 64-bit, matching Java's promotion
/// rules.
pub fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    let x_part = i64::from(x.wrapping_mul(3_129_871));
    let z_part = i64::from(z).wrapping_mul(116_129_781);
    let mixed = x_part ^ z_part ^ i64::from(y);
    mixed
        .wrapping_mul(mixed)
        .wrapping_mul(42_317_861)
        .wrapping_add(mixed.wrapping_mul(11))
        >> 16
}

impl RandomSource for XoroshiroRandomSource {
    fn next_int(&mut self) -> i32 {
        self.generator.next_long() as i32
    }

    fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert_bound_positive(bound);

        // Java works with unsigned 32-bit values here.
        let bound = bound as u32;
        let mut value = self.next_int() as u32 as u64;
        let mut product = value * bound as u64;
        let mut low = product & 0xFFFF_FFFF;

        if low < bound as u64 {
            // Integer.remainderUnsigned(~bound + 1, bound)
            let threshold = bound.wrapping_neg() % bound;

            while low < threshold as u64 {
                value = self.next_int() as u32 as u64;
                product = value * bound as u64;
                low = product & 0xFFFF_FFFF;
            }
        }

        (product >> 32) as u32 as i32
    }

    fn next_long(&mut self) -> i64 {
        self.generator.next_long()
    }

    fn next_double(&mut self) -> f64 {
        // Java casts the 53-bit value to f32 before multiplying, so the
        // low bits are intentionally lost.
        (self.next_bits(53) as f32 * 1.110223e-16_f32) as f64
    }

    fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.9604645e-8_f32
    }

    fn next_boolean(&mut self) -> bool {
        (self.generator.next_long() & 1) != 0
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut gaussian = std::mem::take(&mut self.gaussian);
        let value = gaussian.next(self);
        self.gaussian = gaussian;
        value
    }

    fn consume_count(&mut self, count: u32) {
        // Java's XoroshiroRandomSource consumes whole longs, unlike the
        // RandomSource default which consumes ints.
        for _ in 0..count {
            self.generator.next_long();
        }
    }
}
