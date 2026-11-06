//! End dimension generation.
//!
//! Ports the shipped `worldgen/noise_settings/end.json` router, the
//! `DensityFunctions.EndIslandDensityFunction` island field, and
//! `TheEndBiomeSource`. The central island and the outer island ring both come
//! from the shipped density functions; the obsidian arrival platform is placed
//! at vanilla's `(100, 49, 0)` feature origin.
//!
//! Parity notes:
//! - Like the Nether, the End uses `legacy_random_source: true`. The island
//!   field is exact (`LegacyRandomSource(seed)`, `consumeCount(17292)`,
//!   `SimplexNoise`), while the `end/base_3d_noise` blended noise is built from
//!   the world seed through the public positional API instead of a sequential
//!   `LegacyRandomSource`. Terrain is deterministic and the island layout is
//!   correct, but not guaranteed block-identical to Java.
//! - End cities, gateways, the dragon fight, and the central spike pillar are
//!   not generated in this slice. `end_spike` has an empty spike list in the
//!   shipped data and produces no terrain, so only the platform is material.

use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use crate::worldgen::generator::sample_grid;
use crate::worldgen::noise::BlendedNoise;
use crate::worldgen::simplex::SimplexNoise;
use folia_random::{LegacyRandomSource, RandomSource, XoroshiroRandomSource};
use std::io;

pub const MIN_Y: i32 = 0;
pub const HEIGHT: i32 = 128;
/// The End dimension type is 256 blocks tall; the noise column is 128 blocks.
pub const DIMENSION_HEIGHT: i32 = 256;
pub const SEA_LEVEL: i32 = 0;
pub const PLATFORM_ORIGIN: (i32, i32, i32) = (100, 49, 0);

const CELL_WIDTH: i32 = 8;
const CELL_HEIGHT: i32 = 4;
const END_STONE: &str = "minecraft:end_stone";

pub struct EndGenerator {
    seed: i64,
    islands: EndIslandDensity,
    base_noise: BlendedNoise,
}

impl EndGenerator {
    pub fn new(seed: i64) -> io::Result<Self> {
        let mut random = XoroshiroRandomSource::new(seed);
        let base_noise = BlendedNoise::create(&mut random, 0.25, 0.25, 80.0, 160.0, 4.0);
        Ok(Self {
            seed,
            islands: EndIslandDensity::new(seed),
            base_noise,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn min_y(&self) -> i32 {
        MIN_Y
    }

    pub fn height(&self) -> i32 {
        HEIGHT
    }

    /// `TheEndBiomeSource.getNoiseBiome` for a quart position.
    pub fn biome_at_quart(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> &'static str {
        let _ = quart_y;
        let section_x = quart_x >> 2;
        let section_z = quart_z >> 2;
        if i64::from(section_x) * i64::from(section_x) + i64::from(section_z) * i64::from(section_z)
            <= 4096
        {
            return "minecraft:the_end";
        }
        let x = (section_x * 2 + 1) * 8;
        let z = (section_z * 2 + 1) * 8;
        let erosion = self.islands.compute(x, z);
        if erosion > 0.25 {
            "minecraft:end_highlands"
        } else if erosion >= -0.0625 {
            "minecraft:end_midlands"
        } else if erosion < -0.21875 {
            "minecraft:small_end_islands"
        } else {
            "minecraft:end_barrens"
        }
    }

    pub fn biome_at_block(&self, x: i32, y: i32, z: i32) -> &'static str {
        self.biome_at_quart(x >> 2, y >> 2, z >> 2)
    }

    /// `DensityFunctions.EndIslandDensityFunction.compute` at a block position.
    pub fn island_density(&self, x: i32, z: i32) -> f64 {
        self.islands.compute(x, z)
    }

    /// Final End density at one block position (`postProcess(slideEnd(
    /// sloped_cheese))`), before the 8x4x8 interpolation grid.
    pub fn density(&self, x: i32, y: i32, z: i32) -> f64 {
        let sloped = self.islands.compute(x, z)
            + self
                .base_noise
                .compute(f64::from(x), f64::from(y), f64::from(z));
        let top_gradient = clamped_map(f64::from(y), 56.0, 312.0, 1.0, 0.0);
        let top = lerp(top_gradient, -23.4375, sloped);
        let bottom_gradient = clamped_map(f64::from(y), 4.0, 32.0, 0.0, 1.0);
        let slid = lerp(bottom_gradient, -0.234375, top);
        let value = (0.64 * slid).clamp(-1.0, 1.0);
        value / 2.0 - value * value * value / 24.0
    }

    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32) -> io::Result<Chunk> {
        let cells_x = (16 / CELL_WIDTH + 1) as usize;
        let cells_z = cells_x;
        let cells_y = (HEIGHT / CELL_HEIGHT + 1) as usize;
        let mut grid = vec![0.0f64; cells_x * cells_z * cells_y];
        for xi in 0..cells_x {
            for zi in 0..cells_z {
                for yi in 0..cells_y {
                    let x = chunk_x * 16 + xi as i32 * CELL_WIDTH;
                    let y = MIN_Y + yi as i32 * CELL_HEIGHT;
                    let z = chunk_z * 16 + zi as i32 * CELL_WIDTH;
                    grid[(xi * cells_z + zi) * cells_y + yi] = self.density(x, y, z);
                }
            }
        }

        let section_count = DIMENSION_HEIGHT / 16;
        let mut sections = Vec::with_capacity(section_count as usize);
        for index in 0..section_count {
            sections.push(ChunkSection {
                y: (MIN_Y.div_euclid(16) + index) as i8,
                block_states: PalettedContainer {
                    palette: vec![air()],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:the_end".into()],
                    indices: vec![0; 64],
                }),
                light: crate::light::SectionLight::default(),
            });
        }
        let mut chunk = Chunk {
            x: chunk_x,
            z: chunk_z,
            sections,
        };

        let end_stone = BlockState {
            name: END_STONE.into(),
            properties: vec![],
        };
        for local_x in 0..16 {
            for local_z in 0..16 {
                for index in 0..HEIGHT {
                    let y = MIN_Y + index;
                    let value = sample_grid(
                        &grid,
                        cells_x,
                        cells_z,
                        cells_y,
                        CELL_WIDTH,
                        CELL_HEIGHT,
                        local_x,
                        local_z,
                        index,
                    );
                    if value > 0.0 {
                        chunk.set_block_state_ref(
                            local_x as usize,
                            y,
                            local_z as usize,
                            &end_stone,
                        );
                    }
                }
            }
        }

        self.fill_biomes(&mut chunk, chunk_x, chunk_z);
        self.place_platform(&mut chunk);
        Ok(chunk)
    }

    /// Places `EndPlatformFeature.createEndPlatform` for every chunk the
    /// vanilla `(100, 49, 0)` 5x5 platform overlaps.
    fn place_platform(&self, chunk: &mut Chunk) {
        let (origin_x, origin_y, origin_z) = PLATFORM_ORIGIN;
        let min_x = origin_x - 2;
        let max_x = origin_x + 2;
        let min_z = origin_z - 2;
        let max_z = origin_z + 2;
        let chunk_min_x = chunk.x * 16;
        let chunk_min_z = chunk.z * 16;
        if max_x < chunk_min_x
            || min_x >= chunk_min_x + 16
            || max_z < chunk_min_z
            || min_z >= chunk_min_z + 16
        {
            return;
        }
        let obsidian = BlockState {
            name: "minecraft:obsidian".into(),
            properties: vec![],
        };
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let local_x = x - chunk_min_x;
                let local_z = z - chunk_min_z;
                if !(0..16).contains(&local_x) || !(0..16).contains(&local_z) {
                    continue;
                }
                // `i2 == -1` is obsidian; the three layers above are cleared.
                chunk.set_block_state_ref(
                    local_x as usize,
                    origin_y - 1,
                    local_z as usize,
                    &obsidian,
                );
                for y in origin_y..origin_y + 3 {
                    chunk.set_block_state(local_x as usize, y, local_z as usize, air());
                }
            }
        }
    }

    fn fill_biomes(&self, chunk: &mut Chunk, chunk_x: i32, chunk_z: i32) {
        let quart_layers = (DIMENSION_HEIGHT >> 2) as usize;
        let mut cursor = 0usize;
        for section in &mut chunk.sections {
            let mut palette: Vec<String> = Vec::new();
            let mut indices = Vec::with_capacity(64);
            for _ in 0..64 {
                let layer = cursor / 16;
                let quart_y = layer;
                let qz = (cursor % 16) / 4;
                let qx = cursor % 4;
                let biome = self
                    .biome_at_quart(
                        chunk_x * 4 + qx as i32,
                        quart_y as i32,
                        chunk_z * 4 + qz as i32,
                    )
                    .to_owned();
                cursor += 1;
                let index = match palette.iter().position(|value| value == &biome) {
                    Some(index) => index,
                    None => {
                        palette.push(biome);
                        palette.len() - 1
                    }
                };
                indices.push(index as u16);
            }
            section.biomes = Some(PalettedContainer { palette, indices });
        }
        debug_assert_eq!(cursor, quart_layers * 16);
    }
}

/// `DensityFunctions.EndIslandDensityFunction`.
struct EndIslandDensity {
    noise: SimplexNoise,
}

impl EndIslandDensity {
    fn new(seed: i64) -> Self {
        let mut random = LegacyRandomSource::new(seed);
        random.consume_count(17292);
        Self {
            noise: SimplexNoise::new(&mut random),
        }
    }

    fn compute(&self, x: i32, z: i32) -> f64 {
        (f64::from(self.height_value(x / 8, z / 8)) - 8.0) / 128.0
    }

    fn height_value(&self, x: i32, z: i32) -> f32 {
        let i = x / 2;
        let i1 = z / 2;
        let i2 = x % 2;
        let i3 = z % 2;
        let mut value = 100.0
            - ((i64::from(x) * i64::from(x) + i64::from(z) * i64::from(z)) as f32).sqrt() * 8.0;
        value = value.clamp(-100.0, 80.0);

        for i4 in -12..=12 {
            for i5 in -12..=12 {
                let l = i + i4;
                let l1 = i1 + i5;
                let mut island = f32::MIN;
                if i64::from(l) * i64::from(l) + i64::from(l1) * i64::from(l1) > 4096
                    && (self.noise.get_value(f64::from(l), f64::from(l1)) as f32) < -0.9
                {
                    island = (l.abs() as f32 * 3439.0 + l1.abs() as f32 * 147.0) % 13.0 + 9.0;
                }
                if island != f32::MIN {
                    let f2 = (i2 - i4 * 2) as f32;
                    let f3 = (i3 - i5 * 2) as f32;
                    let f4 = (100.0 - (f2 * f2 + f3 * f3).sqrt() * island).clamp(-100.0, 80.0);
                    value = value.max(f4);
                }
            }
        }
        value
    }
}

fn clamped_map(
    input: f64,
    input_min: f64,
    input_max: f64,
    output_min: f64,
    output_max: f64,
) -> f64 {
    let inverse = (input - input_min) / (input_max - input_min);
    if inverse < 0.0 {
        output_min
    } else if inverse > 1.0 {
        output_max
    } else {
        output_min + inverse * (output_max - output_min)
    }
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_island_field_is_positive_at_the_centre() {
        let generator = EndGenerator::new(12345).unwrap();
        assert!(generator.island_density(0, 0) > 0.4);
    }

    #[test]
    fn end_biomes_follow_the_section_ring() {
        let generator = EndGenerator::new(12345).unwrap();
        assert_eq!(generator.biome_at_quart(0, 16, 0), "minecraft:the_end");
        // 64 chunks away is outside the central-island ring.
        let far = generator.biome_at_quart(64 * 4, 16, 64 * 4);
        assert!(matches!(
            far,
            "minecraft:end_highlands"
                | "minecraft:end_midlands"
                | "minecraft:small_end_islands"
                | "minecraft:end_barrens"
        ));
    }

    #[test]
    fn end_density_is_deterministic() {
        let first = EndGenerator::new(12345).unwrap();
        let second = EndGenerator::new(12345).unwrap();
        for (x, y, z) in [(0, 0, 0), (0, 64, 0), (0, 127, 0), (15, 32, -16)] {
            assert_eq!(first.density(x, y, z), second.density(x, y, z));
        }
    }
}
