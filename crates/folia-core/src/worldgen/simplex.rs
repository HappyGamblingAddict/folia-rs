//! `SimplexNoise` and `PerlinSimplexNoise`.
//!
//! Used by `Biome.BIOME_INFO_NOISE` for placement noise counts. Ported from
//! `net/minecraft/world/level/levelgen/synth/{SimplexNoise,PerlinSimplexNoise}`.

use folia_random::{LegacyRandomSource, RandomSource};
use std::sync::OnceLock;

const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// `net.minecraft.world.level.levelgen.synth.SimplexNoise`.
#[derive(Debug, Clone)]
pub struct SimplexNoise {
    p: [i32; 512],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl SimplexNoise {
    pub fn new(random: &mut dyn RandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;

        let mut p = [0i32; 512];
        for (index, slot) in p.iter_mut().take(256).enumerate() {
            *slot = index as i32;
        }
        for index in 0..256 {
            let random_int = random.next_int_bound((256 - index) as i32) as usize;
            p.swap(index, random_int + index);
        }
        for index in 0..256 {
            p[index + 256] = p[index];
        }

        Self { p, xo, yo, zo }
    }

    fn permutation(&self, index: i32) -> i32 {
        self.p[(index & 0xFF) as usize]
    }

    fn corner_noise(gradient: usize, x: f64, y: f64, z: f64, offset: f64) -> f64 {
        let mut d = offset - x * x - y * y - z * z;
        if d < 0.0 {
            0.0
        } else {
            d *= d;
            let gradient = GRADIENT[gradient];
            d * d
                * (f64::from(gradient[0]) * x
                    + f64::from(gradient[1]) * y
                    + f64::from(gradient[2]) * z)
        }
    }

    /// Java: `SimplexNoise.getValue(double, double)`.
    pub fn get_value(&self, x: f64, y: f64) -> f64 {
        const SQRT_3: f64 = 1.732_050_807_568_877_2;
        const F2: f64 = 0.5 * (SQRT_3 - 1.0);
        const G2: f64 = (3.0 - SQRT_3) / 6.0;

        let d = (x + y) * F2;
        let floor_x = (x + d).floor();
        let floor_y = (y + d).floor();
        let d1 = (floor_x + floor_y) * G2;
        let d2 = floor_x - d1;
        let d3 = floor_y - d1;
        let d4 = x - d2;
        let d5 = y - d3;
        let (i, i1) = if d4 > d5 { (1, 0) } else { (0, 1) };

        let d6 = d4 - f64::from(i) + G2;
        let d7 = d5 - f64::from(i1) + G2;
        let d8 = d4 - 1.0 + 2.0 * G2;
        let d9 = d5 - 1.0 + 2.0 * G2;
        let i2 = floor_x as i32 & 0xFF;
        let i3 = floor_y as i32 & 0xFF;
        let i4 = self.permutation(i2 + self.permutation(i3)) % 12;
        let i5 = self.permutation(i2 + i + self.permutation(i3 + i1)) % 12;
        let i6 = self.permutation(i2 + 1 + self.permutation(i3 + 1)) % 12;

        let corner0 = Self::corner_noise(i4 as usize, d4, d5, 0.0, 0.5);
        let corner1 = Self::corner_noise(i5 as usize, d6, d7, 0.0, 0.5);
        let corner2 = Self::corner_noise(i6 as usize, d8, d9, 0.0, 0.5);
        70.0 * (corner0 + corner1 + corner2)
    }
}

/// `net.minecraft.world.level.levelgen.synth.PerlinSimplexNoise`.
#[derive(Debug, Clone)]
pub struct PerlinSimplexNoise {
    noise_levels: Vec<Option<SimplexNoise>>,
    highest_freq_value_factor: f64,
    highest_freq_input_factor: f64,
}

impl PerlinSimplexNoise {
    /// Java: `new PerlinSimplexNoise(RandomSource, List<Integer>)`.
    pub fn new(random: &mut dyn RandomSource, octaves: &[i32]) -> Self {
        let mut octaves = octaves.to_vec();
        octaves.sort_unstable();
        octaves.dedup();
        assert!(!octaves.is_empty(), "Need some octaves!");

        let low = -*octaves.first().expect("octaves is non-empty");
        let high = *octaves.last().expect("octaves is non-empty");
        let count = (low + high + 1).max(0) as usize;
        assert!(count >= 1, "Total number of octaves needs to be >= 1");

        let simplex = SimplexNoise::new(random);
        let mut noise_levels: Vec<Option<SimplexNoise>> = vec![None; count];
        let high_index = high;
        if high_index >= 0 && (high_index as usize) < count && octaves.contains(&0) {
            noise_levels[high_index as usize] = Some(simplex.clone());
        }

        for index in (high_index + 1)..count as i32 {
            if index >= 0 && octaves.contains(&(high_index - index)) {
                noise_levels[index as usize] = Some(SimplexNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }

        if high_index > 0 {
            let value = simplex.get_value(simplex.xo, simplex.yo);
            let seed = (value * 9.223_372_036_854_776e18) as i64;
            let mut random = LegacyRandomSource::new(seed);
            for index in (0..high_index).rev() {
                if index < count as i32 && octaves.contains(&(high_index - index)) {
                    noise_levels[index as usize] = Some(SimplexNoise::new(&mut random));
                } else {
                    random.consume_count(262);
                }
            }
        }

        Self {
            noise_levels,
            highest_freq_input_factor: 2f64.powi(high),
            highest_freq_value_factor: 1.0 / (2f64.powi(count as i32) - 1.0),
        }
    }

    /// Java: `PerlinSimplexNoise.getValue(double, double, boolean)`.
    pub fn get_value(&self, x: f64, y: f64, use_noise_offsets: bool) -> f64 {
        let mut value = 0.0;
        let mut input_factor = self.highest_freq_input_factor;
        let mut value_factor = self.highest_freq_value_factor;
        for noise in &self.noise_levels {
            if let Some(noise) = noise {
                let offset_x = if use_noise_offsets { noise.xo } else { 0.0 };
                let offset_y = if use_noise_offsets { noise.yo } else { 0.0 };
                value += noise.get_value(x * input_factor + offset_x, y * input_factor + offset_y)
                    * value_factor;
            }
            input_factor /= 2.0;
            value_factor *= 2.0;
        }
        value
    }
}

static BIOME_INFO_NOISE: OnceLock<PerlinSimplexNoise> = OnceLock::new();

/// `Biome.BIOME_INFO_NOISE` (`LegacyRandomSource(2345)`, octave 0).
pub fn biome_info_noise() -> &'static PerlinSimplexNoise {
    BIOME_INFO_NOISE.get_or_init(|| {
        let mut random = LegacyRandomSource::new(2345);
        PerlinSimplexNoise::new(&mut random, &[0])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplex_noise_is_deterministic_and_bounded() {
        let mut random = LegacyRandomSource::new(2345);
        let noise = SimplexNoise::new(&mut random);
        let first = noise.get_value(12.5, -7.25);
        assert!((first - -0.436_292_480_959_983_6).abs() < 1e-12);
        assert_eq!(first, noise.get_value(12.5, -7.25));

        let mut other = LegacyRandomSource::new(2345);
        let noise2 = SimplexNoise::new(&mut other);
        assert_eq!(first, noise2.get_value(12.5, -7.25));
    }

    #[test]
    fn biome_info_noise_matches_a_reference_value() {
        let value = biome_info_noise().get_value(0.5, -0.25, false);
        assert!((value - -0.566_760_365_682_524_6).abs() < 1e-12);
    }
}
