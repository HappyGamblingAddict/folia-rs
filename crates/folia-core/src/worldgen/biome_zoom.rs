//! `BiomeManager`: the fuzzy quart-position biome lookup behind
//! `LevelReader.getBiome`.
//!
//! Features never see raw `getNoiseBiome` results: `LevelReader.getBiome(pos)`
//! delegates to `BiomeManager.getBiome`, which considers the eight noise biomes
//! around the position and keeps the one with the smallest "fiddled" distance.
//! Placement filters (`BiomeFilter`) go through this path, so the distance
//! function has to match bit for bit.

use folia_math::BlockPos;

use crate::worldgen::biome::BiomeSampler;

/// `BiomeManager.ZOOM_BITS` / `ZOOM_MASK`: one quart is four blocks.
const ZOOM_BITS: i32 = 2;
const ZOOM_MASK: i32 = 3;

/// `BiomeManager.obfuscateSeed`: `Hashing.sha256().hashLong(seed).asLong()`.
///
/// Guava's `putLong` writes the seed little-endian and `HashCode.asLong`
/// reads the first eight digest bytes little-endian.
pub fn obfuscate_seed(world_seed: i64) -> i64 {
    let digest = sha256(&world_seed.to_le_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_le_bytes(first)
}

/// Precomputed [`BiomeManager`] state for one world seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BiomeZoomer {
    seed: i64,
}

impl BiomeZoomer {
    /// `new BiomeManager(source, BiomeManager.obfuscateSeed(seed))`.
    pub fn new(world_seed: i64) -> Self {
        Self {
            seed: obfuscate_seed(world_seed),
        }
    }

    /// The obfuscated seed used by the distance function.
    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// `BiomeManager.getBiome` reduced to the noise-biome quart it selects.
    ///
    /// The search works on `pos - 2`, so its quart coordinates are
    /// `(pos - 2) >> 2`, not `QuartPos.fromBlock(pos)`.
    pub fn nearest_quart(&self, pos: BlockPos) -> (i32, i32, i32) {
        let x = pos.x() - 2;
        let y = pos.y() - 2;
        let z = pos.z() - 2;
        let quart_x = x >> ZOOM_BITS;
        let quart_y = y >> ZOOM_BITS;
        let quart_z = z >> ZOOM_BITS;
        let fraction_x = f64::from(x & ZOOM_MASK) / 4.0;
        let fraction_y = f64::from(y & ZOOM_MASK) / 4.0;
        let fraction_z = f64::from(z & ZOOM_MASK) / 4.0;

        let mut nearest = 0usize;
        let mut nearest_distance = f64::INFINITY;
        for index in 0..8 {
            let negative_x = index & 4 == 0;
            let negative_y = index & 2 == 0;
            let negative_z = index & 1 == 0;
            let corner_x = if negative_x { quart_x } else { quart_x + 1 };
            let corner_y = if negative_y { quart_y } else { quart_y + 1 };
            let corner_z = if negative_z { quart_z } else { quart_z + 1 };
            let offset_x = if negative_x {
                fraction_x
            } else {
                fraction_x - 1.0
            };
            let offset_y = if negative_y {
                fraction_y
            } else {
                fraction_y - 1.0
            };
            let offset_z = if negative_z {
                fraction_z
            } else {
                fraction_z - 1.0
            };
            let distance = fiddled_distance(
                self.seed, corner_x, corner_y, corner_z, offset_x, offset_y, offset_z,
            );
            if nearest_distance > distance {
                nearest = index;
                nearest_distance = distance;
            }
        }

        let negative_x = nearest & 4 == 0;
        let negative_y = nearest & 2 == 0;
        let negative_z = nearest & 1 == 0;
        (
            if negative_x { quart_x } else { quart_x + 1 },
            if negative_y { quart_y } else { quart_y + 1 },
            if negative_z { quart_z } else { quart_z + 1 },
        )
    }

    /// `BiomeManager.getBiome` against a noise biome source.
    pub fn biome_at<'a>(&self, biomes: &'a BiomeSampler, pos: BlockPos) -> &'a str {
        let (quart_x, quart_y, quart_z) = self.nearest_quart(pos);
        biomes.sample_quart(quart_x, quart_y, quart_z)
    }
}

/// `LinearCongruentialGenerator.next`; every step wraps like Java's `long`.
fn lcg_next(left: i64, right: i64) -> i64 {
    const MULTIPLIER: i64 = 6_364_136_223_846_793_005;
    const INCREMENT: i64 = 1_442_695_040_888_963_407;
    left.wrapping_mul(left.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT))
        .wrapping_add(right)
}

/// `BiomeManager.getFiddledDistance`.
fn fiddled_distance(
    seed: i64,
    x: i32,
    y: i32,
    z: i32,
    x_noise: f64,
    y_noise: f64,
    z_noise: f64,
) -> f64 {
    let mut state = lcg_next(seed, i64::from(x));
    state = lcg_next(state, i64::from(y));
    state = lcg_next(state, i64::from(z));
    state = lcg_next(state, i64::from(x));
    state = lcg_next(state, i64::from(y));
    state = lcg_next(state, i64::from(z));
    let fiddle_x = fiddle(state);
    state = lcg_next(state, seed);
    let fiddle_y = fiddle(state);
    state = lcg_next(state, seed);
    let fiddle_z = fiddle(state);
    square(z_noise + fiddle_z) + square(y_noise + fiddle_y) + square(x_noise + fiddle_x)
}

/// `BiomeManager.getFiddle`.
fn fiddle(state: i64) -> f64 {
    (((state >> 24) & 1023) - 512) as f64 * 8.789_062_5e-4
}

fn square(value: f64) -> f64 {
    value * value
}

/// SHA-256 (FIPS 180-4), used only to derive the biome zoom seed.
fn sha256(data: &[u8]) -> [u8; 32] {
    #[rustfmt::skip]
    const K: [u32; 64] = [
        0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1,
        0x923f_82a4, 0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3,
        0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786,
        0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
        0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147,
        0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13,
        0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
        0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
        0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a,
        0x5b9c_ca4f, 0x682e_6ff3, 0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208,
        0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
    ];

    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let bit_length = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in block.chunks_exact(4).enumerate() {
            schedule[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let first = schedule[index - 15];
            let second = schedule[index - 2];
            let sigma0 = first.rotate_right(7) ^ first.rotate_right(18) ^ (first >> 3);
            let sigma1 = second.rotate_right(17) ^ second.rotate_right(19) ^ (second >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(sigma0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(sigma1);
        }

        let mut working = state;
        for index in 0..64 {
            let [a, b, c, d, e, f, g, h] = working;
            let sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(sigma1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sigma0.wrapping_add(majority);
            working = [
                temp1.wrapping_add(temp2),
                a,
                b,
                c,
                d.wrapping_add(temp1),
                e,
                f,
                g,
            ];
        }
        for (slot, value) in state.iter_mut().zip(working) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (chunk, word) in digest.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn obfuscated_seed_uses_the_first_digest_eight_little_endian() {
        assert_eq!(obfuscate_seed(0), 8_794_265_229_978_523_055);
        assert_eq!(obfuscate_seed(12_345), 293_737_985_876_514_017);
    }

    #[test]
    fn nearest_quart_matches_the_vanilla_fiddled_distance_search() {
        let zoomer = BiomeZoomer::new(0);
        assert_eq!(zoomer.nearest_quart(BlockPos::new(0, 0, 0)), (-1, 0, -1));
        assert_eq!(zoomer.nearest_quart(BlockPos::new(96, 49, 0)), (23, 12, -1));
        assert_eq!(zoomer.nearest_quart(BlockPos::new(15, 70, -9)), (3, 17, -3));
        assert_eq!(
            zoomer.nearest_quart(BlockPos::new(-37, 120, 500)),
            (-10, 30, 124)
        );
    }

    #[test]
    fn nearest_quart_stays_within_one_quart_of_the_position() {
        let zoomer = BiomeZoomer::new(0x5eed);
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(-1, -1, -1),
            BlockPos::new(1234, -60, -9876),
        ] {
            let (quart_x, quart_y, quart_z) = zoomer.nearest_quart(pos);
            let base_x = (pos.x() - 2) >> ZOOM_BITS;
            let base_y = (pos.y() - 2) >> ZOOM_BITS;
            let base_z = (pos.z() - 2) >> ZOOM_BITS;
            assert!((base_x..=base_x + 1).contains(&quart_x));
            assert!((base_y..=base_y + 1).contains(&quart_y));
            assert!((base_z..=base_z + 1).contains(&quart_z));
        }
    }
}
