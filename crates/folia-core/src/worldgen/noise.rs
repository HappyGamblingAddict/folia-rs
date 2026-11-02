//! Java-compatible worldgen noise primitives.
//!
//! Ports `net.minecraft.world.level.levelgen.synth.{SimplexNoise,
//! ImprovedNoise, PerlinNoise, NormalNoise, BlendedNoise}`. These operate on
//! the Xoroshiro random source and positional random factories so noise
//! instances match Folia for the same level seed.

use folia_random::{RandomSource, XoroshiroPositionalRandomFactory, XoroshiroRandomSource};

/// `SimplexNoise.GRADIENT`.
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

const ROUND_OFF: f64 = 33_554_432.0;

fn floor(value: f64) -> i32 {
    value.floor() as i32
}

fn smoothstep(input: f64) -> f64 {
    input * input * input * (input * (input * 6.0 - 15.0) + 10.0)
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

fn lerp2(delta1: f64, delta2: f64, start1: f64, end1: f64, start2: f64, end2: f64) -> f64 {
    lerp(
        delta2,
        lerp(delta1, start1, end1),
        lerp(delta1, start2, end2),
    )
}

#[allow(clippy::too_many_arguments)]
fn lerp3(
    delta1: f64,
    delta2: f64,
    delta3: f64,
    start1: f64,
    end1: f64,
    start2: f64,
    end2: f64,
    start3: f64,
    end3: f64,
    start4: f64,
    end4: f64,
) -> f64 {
    lerp(
        delta3,
        lerp2(delta1, delta2, start1, end1, start2, end2),
        lerp2(delta1, delta2, start3, end3, start4, end4),
    )
}

fn grad_dot(index: usize, x: f64, y: f64, z: f64) -> f64 {
    let gradient = GRADIENT[index & 15];
    f64::from(gradient[0]) * x + f64::from(gradient[1]) * y + f64::from(gradient[2]) * z
}

/// `net.minecraft.world.level.levelgen.synth.ImprovedNoise`.
#[derive(Debug, Clone)]
pub struct ImprovedNoise {
    p: [u8; 256],
    pub xo: f64,
    pub yo: f64,
    pub zo: f64,
}

impl ImprovedNoise {
    pub fn new(random: &mut XoroshiroRandomSource) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;

        let mut p = [0u8; 256];
        for (index, value) in p.iter_mut().enumerate() {
            *value = index as u8;
        }
        for i in 0..256 {
            let random_int = random.next_int_bound((256 - i) as i32) as usize;
            p.swap(i, i + random_int);
        }

        Self { p, xo, yo, zo }
    }

    #[inline]
    fn p(&self, index: i32) -> usize {
        self.p[(index & 0xFF) as usize] as usize
    }

    pub fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise_with_scale(x, y, z, 0.0, 0.0)
    }

    /// Java's deprecated `noise(x, y, z, yScale, yMax)`.
    pub fn noise_with_scale(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let d = x + self.xo;
        let d1 = y + self.yo;
        let d2 = z + self.zo;
        let grid_x = floor(d);
        let grid_y = floor(d1);
        let grid_z = floor(d2);
        let delta_x = d - f64::from(grid_x);
        let delta_y = d1 - f64::from(grid_y);
        let delta_z = d2 - f64::from(grid_z);

        let weird_delta_y = if y_scale != 0.0 {
            let clamped = if y_max >= 0.0 && y_max < delta_y {
                y_max
            } else {
                delta_y
            };
            let shifted = floor(clamped / y_scale + 1.0e-7) as f64 * y_scale;
            delta_y - shifted
        } else {
            delta_y
        };

        self.sample_and_lerp(
            grid_x,
            grid_y,
            grid_z,
            delta_x,
            weird_delta_y,
            delta_z,
            delta_y,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(
        &self,
        grid_x: i32,
        grid_y: i32,
        grid_z: i32,
        delta_x: f64,
        weird_delta_y: f64,
        delta_z: f64,
        delta_y: f64,
    ) -> f64 {
        let i = self.p(grid_x);
        let i1 = self.p(grid_x + 1);
        let i2 = self.p(i as i32 + grid_y);
        let i3 = self.p(i as i32 + grid_y + 1);
        let i4 = self.p(i1 as i32 + grid_y);
        let i5 = self.p(i1 as i32 + grid_y + 1);

        let d = grad_dot(self.p(i2 as i32 + grid_z), delta_x, weird_delta_y, delta_z);
        let d1 = grad_dot(
            self.p(i4 as i32 + grid_z),
            delta_x - 1.0,
            weird_delta_y,
            delta_z,
        );
        let d2 = grad_dot(
            self.p(i3 as i32 + grid_z),
            delta_x,
            weird_delta_y - 1.0,
            delta_z,
        );
        let d3 = grad_dot(
            self.p(i5 as i32 + grid_z),
            delta_x - 1.0,
            weird_delta_y - 1.0,
            delta_z,
        );
        let d4 = grad_dot(
            self.p(i2 as i32 + grid_z + 1),
            delta_x,
            weird_delta_y,
            delta_z - 1.0,
        );
        let d5 = grad_dot(
            self.p(i4 as i32 + grid_z + 1),
            delta_x - 1.0,
            weird_delta_y,
            delta_z - 1.0,
        );
        let d6 = grad_dot(
            self.p(i3 as i32 + grid_z + 1),
            delta_x,
            weird_delta_y - 1.0,
            delta_z - 1.0,
        );
        let d7 = grad_dot(
            self.p(i5 as i32 + grid_z + 1),
            delta_x - 1.0,
            weird_delta_y - 1.0,
            delta_z - 1.0,
        );

        let d8 = smoothstep(delta_x);
        let d9 = smoothstep(delta_y);
        let d10 = smoothstep(delta_z);
        lerp3(d8, d9, d10, d, d1, d2, d3, d4, d5, d6, d7)
    }
}

/// `net.minecraft.world.level.levelgen.synth.PerlinNoise`.
#[derive(Debug, Clone)]
pub struct PerlinNoise {
    noise_levels: Vec<Option<ImprovedNoise>>,
    first_octave: i32,
    amplitudes: Vec<f64>,
    lowest_freq_value_factor: f64,
    lowest_freq_input_factor: f64,
    max_value: f64,
}

impl PerlinNoise {
    pub fn create(
        random: &mut XoroshiroRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        Self::new(random, first_octave, amplitudes, true)
    }

    /// Java: `createLegacyForLegacyNetherBiome` / `createLegacyForBlendedNoise`.
    pub fn create_legacy(
        random: &mut XoroshiroRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        Self::new(random, first_octave, amplitudes, false)
    }

    /// Builds the amplitude list used by `PerlinNoise.create(random,
    /// IntStream.rangeClosed(first, last))`.
    pub fn amplitudes_for_octaves(first: i32, last: i32) -> (i32, Vec<f64>) {
        let low = -first;
        let size = low + last + 1;
        let mut amplitudes = vec![0.0; size.max(0) as usize];
        for octave in first..=last {
            amplitudes[(octave + low) as usize] = 1.0;
        }
        (-low, amplitudes)
    }

    pub fn create_legacy_for_blended_noise(
        random: &mut XoroshiroRandomSource,
        first: i32,
        last: i32,
    ) -> Self {
        let (first_octave, amplitudes) = Self::amplitudes_for_octaves(first, last);
        Self::create_legacy(random, first_octave, &amplitudes)
    }

    fn new(
        random: &mut XoroshiroRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
        use_new_factory: bool,
    ) -> Self {
        let size = amplitudes.len();
        let lowest = -first_octave;
        let mut noise_levels: Vec<Option<ImprovedNoise>> = vec![None; size];

        if use_new_factory {
            let factory = random.fork_positional();
            for (index, amplitude) in amplitudes.iter().enumerate() {
                if *amplitude != 0.0 {
                    let octave = first_octave + index as i32;
                    let mut octave_random = factory.from_hash_of(&format!("octave_{octave}"));
                    noise_levels[index] = Some(ImprovedNoise::new(&mut octave_random));
                }
            }
        } else {
            let improved = ImprovedNoise::new(random);
            if lowest >= 0 && (lowest as usize) < size && amplitudes[lowest as usize] != 0.0 {
                noise_levels[lowest as usize] = Some(improved);
            }
            for index in (0..lowest).rev() {
                if (index as usize) < size {
                    if amplitudes[index as usize] != 0.0 {
                        noise_levels[index as usize] = Some(ImprovedNoise::new(random));
                    } else {
                        skip_octave(random);
                    }
                } else {
                    skip_octave(random);
                }
            }
        }

        let lowest_freq_input_factor = 2f64.powi(-lowest);
        let lowest_freq_value_factor = 2f64.powi(size as i32 - 1) / (2f64.powi(size as i32) - 1.0);
        let mut noise = Self {
            noise_levels,
            first_octave,
            amplitudes: amplitudes.to_vec(),
            lowest_freq_value_factor,
            lowest_freq_input_factor,
            max_value: 0.0,
        };
        noise.max_value = noise.edge_value(2.0);
        noise
    }

    pub fn first_octave(&self) -> i32 {
        self.first_octave
    }

    pub fn amplitudes(&self) -> &[f64] {
        &self.amplitudes
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    fn edge_value(&self, multiplier: f64) -> f64 {
        let mut value = 0.0;
        let mut value_factor = self.lowest_freq_value_factor;
        for (index, level) in self.noise_levels.iter().enumerate() {
            if level.is_some() {
                value += self.amplitudes[index] * multiplier * value_factor;
            }
            value_factor /= 2.0;
        }
        value
    }

    pub fn max_broken_value(&self, y_multiplier: f64) -> f64 {
        self.edge_value(y_multiplier + 2.0)
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut value = 0.0;
        let mut input_factor = self.lowest_freq_input_factor;
        let mut value_factor = self.lowest_freq_value_factor;

        for (index, level) in self.noise_levels.iter().enumerate() {
            if let Some(level) = level {
                let sampled = level.noise(
                    wrap(x * input_factor),
                    wrap(y * input_factor),
                    wrap(z * input_factor),
                );
                value += self.amplitudes[index] * sampled * value_factor;
            }
            input_factor *= 2.0;
            value_factor /= 2.0;
        }

        value
    }

    /// Java: `getOctaveNoise(int octave)`.
    pub fn get_octave_noise(&self, octave: usize) -> Option<&ImprovedNoise> {
        self.noise_levels
            .get(self.noise_levels.len().wrapping_sub(1).wrapping_sub(octave))
            .and_then(Option::as_ref)
    }

    pub fn wrap(value: f64) -> f64 {
        wrap(value)
    }
}

fn skip_octave(random: &mut XoroshiroRandomSource) {
    random.consume_count(262);
}

pub fn wrap(value: f64) -> f64 {
    let rounded = (value / ROUND_OFF + 0.5).floor();
    value - rounded * ROUND_OFF
}

/// `net.minecraft.world.level.levelgen.synth.NormalNoise`.
#[derive(Debug, Clone)]
pub struct NormalNoise {
    value_factor: f64,
    first: PerlinNoise,
    second: PerlinNoise,
    max_value: f64,
}

impl NormalNoise {
    pub const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;

    pub fn create(
        random: &mut XoroshiroRandomSource,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let first = PerlinNoise::create(random, first_octave, amplitudes);
        let second = PerlinNoise::create(random, first_octave, amplitudes);

        let mut min_index = i32::MAX;
        let mut max_index = i32::MIN;
        for (index, amplitude) in amplitudes.iter().enumerate() {
            if *amplitude != 0.0 {
                min_index = min_index.min(index as i32);
                max_index = max_index.max(index as i32);
            }
        }
        let expected_deviation = if min_index == i32::MAX {
            0.1
        } else {
            0.1 * (1.0 + 1.0 / f64::from(max_index - min_index + 1))
        };
        let value_factor = (1.0 / 6.0) / expected_deviation;
        let max_value = (first.max_value() + second.max_value()) * value_factor;

        Self {
            value_factor,
            first,
            second,
            max_value,
        }
    }

    pub fn from_factory(
        factory: &XoroshiroPositionalRandomFactory,
        name: &str,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let mut random = factory.from_hash_of(name);
        Self::create(&mut random, first_octave, amplitudes)
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let scaled_x = x * Self::INPUT_FACTOR;
        let scaled_y = y * Self::INPUT_FACTOR;
        let scaled_z = z * Self::INPUT_FACTOR;
        (self.first.get_value(x, y, z) + self.second.get_value(scaled_x, scaled_y, scaled_z))
            * self.value_factor
    }
}

/// `net.minecraft.world.level.levelgen.synth.BlendedNoise` limited to the
/// fields needed by the overworld `base_3d_noise` density function.
#[derive(Debug, Clone)]
pub struct BlendedNoise {
    min_limit_noise: PerlinNoise,
    max_limit_noise: PerlinNoise,
    main_noise: PerlinNoise,
    xz_multiplier: f64,
    y_multiplier: f64,
    xz_factor: f64,
    y_factor: f64,
    smear_scale_multiplier: f64,
    max_value: f64,
}

impl BlendedNoise {
    pub fn create(
        random: &mut XoroshiroRandomSource,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let min_limit_noise = PerlinNoise::create_legacy_for_blended_noise(random, -15, 0);
        let max_limit_noise = PerlinNoise::create_legacy_for_blended_noise(random, -15, 0);
        let main_noise = PerlinNoise::create_legacy_for_blended_noise(random, -7, 0);

        let xz_multiplier = 684.412 * xz_scale;
        let y_multiplier = 684.412 * y_scale;
        let max_value = min_limit_noise.max_broken_value(y_multiplier);

        Self {
            min_limit_noise,
            max_limit_noise,
            main_noise,
            xz_multiplier,
            y_multiplier,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
            max_value,
        }
    }

    pub fn from_factory(
        factory: &XoroshiroPositionalRandomFactory,
        name: &str,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let mut random = factory.from_hash_of(name);
        Self::create(
            &mut random,
            xz_scale,
            y_scale,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
        )
    }

    pub fn max_value(&self) -> f64 {
        self.max_value
    }

    pub fn compute(&self, block_x: f64, block_y: f64, block_z: f64) -> f64 {
        let d = block_x * self.xz_multiplier;
        let d1 = block_y * self.y_multiplier;
        let d2 = block_z * self.xz_multiplier;
        let d3 = d / self.xz_factor;
        let d4 = d1 / self.y_factor;
        let d5 = d2 / self.xz_factor;
        let d6 = self.y_multiplier * self.smear_scale_multiplier;
        let d7 = d6 / self.y_factor;

        let mut d8 = 0.0;
        let mut d9 = 0.0;
        let mut d10 = 0.0;
        let mut d11 = 1.0;

        for i in 0..8 {
            if let Some(octave) = self.main_noise.get_octave_noise(i) {
                d10 += octave.noise_with_scale(
                    wrap(d3 * d11),
                    wrap(d4 * d11),
                    wrap(d5 * d11),
                    d7 * d11,
                    d4 * d11,
                ) / d11;
            }
            d11 /= 2.0;
        }

        let d12 = (d10 / 10.0 + 1.0) / 2.0;
        let above_one = d12 >= 1.0;
        let below_zero = d12 <= 0.0;
        d11 = 1.0;

        for i in 0..16 {
            let d13 = wrap(d * d11);
            let d14 = wrap(d1 * d11);
            let d15 = wrap(d2 * d11);
            let d16 = d6 * d11;
            if !above_one {
                if let Some(octave) = self.min_limit_noise.get_octave_noise(i) {
                    d8 += octave.noise_with_scale(d13, d14, d15, d16, d1 * d11) / d11;
                }
            }
            if !below_zero {
                if let Some(octave) = self.max_limit_noise.get_octave_noise(i) {
                    d9 += octave.noise_with_scale(d13, d14, d15, d16, d1 * d11) / d11;
                }
            }
            d11 /= 2.0;
        }

        clamped_lerp(d12, d8 / 512.0, d9 / 512.0) / 128.0
    }
}

fn clamped_lerp(start: f64, end: f64, delta: f64) -> f64 {
    // Java: clampedLerp(double start, double end, double delta) is oddly named:
    // the first argument is the clamped lerp fraction.
    if start < 0.0 {
        end
    } else if start > 1.0 {
        delta
    } else {
        lerp(start, end, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factory(seed: i64) -> XoroshiroPositionalRandomFactory {
        XoroshiroRandomSource::new(seed).fork_positional()
    }

    #[test]
    fn normal_noise_matches_java_oracle() {
        let factory = factory(12345);
        let continental = NormalNoise::from_factory(
            &factory,
            "minecraft:continentalness",
            -9,
            &[1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
        );
        let cases = [
            ((0.0, 0.0, 0.0), -0.402_595_828_344_468_7),
            ((1.0, 2.0, 3.0), -0.424_641_269_527_063_44),
            ((123.0, 64.0, -456.0), -0.123_902_765_970_305_92),
            ((-1000.0, 80.0, 2000.0), -0.299_576_587_736_838_26),
            ((15.0, -16.0, 31.0), -0.350_936_318_544_100_84),
            ((16.0, 0.0, -16.0), -0.175_287_590_747_068_43),
            ((0.0, -64.0, 0.0), -0.306_289_235_741_718_57),
            ((0.0, 63.0, 0.0), -0.150_324_837_852_373_8),
        ];
        for ((x, y, z), expected) in cases {
            let actual = continental.get_value(x, y, z);
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "continental ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }

    #[test]
    fn offset_noise_matches_java_oracle() {
        let factory = factory(12345);
        let offset =
            NormalNoise::from_factory(&factory, "minecraft:offset", -3, &[1.0, 1.0, 1.0, 0.0]);
        let cases = [
            ((0.0, 0.0, 0.0), -0.537_158_998_636_881_4),
            ((1.0, 2.0, 3.0), -0.186_383_134_636_690_76),
            ((123.0, 64.0, -456.0), -0.012_087_612_379_624_146),
            ((-1000.0, 80.0, 2000.0), -0.253_309_974_650_522_46),
            ((15.0, -16.0, 31.0), 0.143_856_200_786_171_62),
            ((16.0, 0.0, -16.0), 0.249_156_872_033_072_52),
        ];
        for ((x, y, z), expected) in cases {
            let actual = offset.get_value(x, y, z);
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "offset ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }

    #[test]
    fn blended_noise_matches_java_oracle() {
        let factory = factory(12345);
        let blended = BlendedNoise::from_factory(
            &factory,
            "minecraft:terrain",
            0.25,
            0.125,
            80.0,
            160.0,
            8.0,
        );
        let cases = [
            ((0.0, 0.0, 0.0), -0.125_669_929_116_037_78),
            ((1.0, 2.0, 3.0), -0.090_694_953_409_812_86),
            ((123.0, 64.0, -456.0), -0.073_768_689_507_827_94),
            ((-1000.0, 80.0, 2000.0), -0.209_081_307_622_186_3),
            ((15.0, -16.0, 31.0), 0.542_031_464_691_316_1),
            ((16.0, 0.0, -16.0), -0.069_748_278_805_741_82),
            ((0.0, -64.0, 0.0), -0.238_644_751_895_350_62),
            ((0.0, 63.0, 0.0), -0.107_422_144_957_281_28),
        ];
        for ((x, y, z), expected) in cases {
            let actual = blended.compute(x, y, z);
            assert!(
                (actual - expected).abs() < 1.0e-12,
                "blended ({x},{y},{z}): {actual} != {expected}"
            );
        }
    }

    #[test]
    fn seed_hash_matches_java() {
        let seed = folia_random::seed_from_hash_of("minecraft:continentalness");
        assert_eq!(seed.seed_lo, -8_968_799_236_159_986_078);
        assert_eq!(seed.seed_hi, -5_789_877_984_865_097_555);
    }
}
