//! Seed-mixing helpers from Minecraft's `RandomSupport`.
//!
//! These are the exact 64-bit mixing primitives used to turn a single
//! `i64` seed into the 128-bit state used by xoroshiro128++.

/// Golden-ratio increment used by the 128-bit seed upgrade and the
/// xoroshiro zero-state fallback.
///
/// Java: `RandomSupport.GOLDEN_RATIO_64 = -7046029254386353131L`
pub const GOLDEN_RATIO_64: i64 = -7_046_029_254_386_353_131;

/// Silver-ratio constant used by the 128-bit seed upgrade and the
/// xoroshiro zero-state fallback.
///
/// Java: `RandomSupport.SILVER_RATIO_64 = 7640891576956012809L`
pub const SILVER_RATIO_64: i64 = 7_640_891_576_956_012_809;

/// A 128-bit seed, split into low and high halves.
///
/// Mirrors Java's `RandomSupport.Seed128bit` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Seed128bit {
    pub seed_lo: i64,
    pub seed_hi: i64,
}

impl Seed128bit {
    pub const fn new(seed_lo: i64, seed_hi: i64) -> Self {
        Self { seed_lo, seed_hi }
    }

    /// XOR the two halves with raw values.
    ///
    /// Mirrors Java's `Seed128bit.xor(long, long)`.
    pub const fn xor(self, seed_lo: i64, seed_hi: i64) -> Self {
        Self {
            seed_lo: self.seed_lo ^ seed_lo,
            seed_hi: self.seed_hi ^ seed_hi,
        }
    }

    /// XOR this seed with another `Seed128bit`.
    ///
    /// Mirrors Java's `Seed128bit.xor(Seed128bit)`.
    pub const fn xor_with(self, other: Self) -> Self {
        self.xor(other.seed_lo, other.seed_hi)
    }

    /// Apply Stafford's 13-mix to both halves.
    ///
    /// Mirrors Java's `Seed128bit.mixed()`.
    pub const fn mixed(self) -> Self {
        Self {
            seed_lo: mix_stafford13(self.seed_lo),
            seed_hi: mix_stafford13(self.seed_hi),
        }
    }
}

/// Stafford variant 13 mix function.
///
/// Java: `RandomSupport.mixStafford13(long)`.
///
/// Uses wrapping 64-bit arithmetic and logical right shifts, matching
/// Java's `>>>`.
pub const fn mix_stafford13(mut seed: i64) -> i64 {
    seed = (seed ^ ((seed as u64 >> 30) as i64)).wrapping_mul(-4_658_895_280_553_007_687);
    seed = (seed ^ ((seed as u64 >> 27) as i64)).wrapping_mul(-7_723_592_293_110_705_685);
    seed ^ ((seed as u64 >> 31) as i64)
}

/// Upgrade a single seed to 128 bits without mixing.
///
/// Java: `RandomSupport.upgradeSeedTo128bitUnmixed(long)`.
///
/// Note that `seed_hi` is derived from the already-xored `seed_lo`, not
/// from the original seed. This ordering is intentional in the reference.
pub const fn upgrade_seed_to_128bit_unmixed(seed: i64) -> Seed128bit {
    let seed_lo = seed ^ SILVER_RATIO_64;
    let seed_hi = seed_lo.wrapping_add(GOLDEN_RATIO_64);
    Seed128bit::new(seed_lo, seed_hi)
}

/// Upgrade a single seed to a mixed 128-bit seed.
///
/// Java: `RandomSupport.upgradeSeedTo128bit(long)`.
pub const fn upgrade_seed_to_128bit(seed: i64) -> Seed128bit {
    upgrade_seed_to_128bit_unmixed(seed).mixed()
}

/// Derives a 128-bit seed from a UTF-8 name using MD5.
///
/// Java:
/// `RandomSupport.seedFromHashOf(String)` — MD5 of the UTF-8 bytes, then two
/// big-endian `long` reads (`Longs.fromBytes`).
pub fn seed_from_hash_of(name: &str) -> Seed128bit {
    let digest = crate::md5::md5(name.as_bytes());
    let mut lo = [0u8; 8];
    let mut hi = [0u8; 8];
    lo.copy_from_slice(&digest[0..8]);
    hi.copy_from_slice(&digest[8..16]);
    Seed128bit::new(i64::from_be_bytes(lo), i64::from_be_bytes(hi))
}
