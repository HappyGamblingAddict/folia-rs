//! Noise-based overworld chunk generation.
//!
//! This evaluates the real `minecraft:overworld` final-density router on the
//! vanilla 4x8x4 interpolation grid and fills a Rust chunk. It is the base
//! terrain pass: bedrock, deepslate/stone, simple surface blocks and sea-level
//! water. Vanilla surface rules, carvers, aquifers, ores, features and
//! structures are not applied yet.

use crate::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use crate::worldgen::aquifer::Aquifer;
use crate::worldgen::biome::BiomeSampler;
use crate::worldgen::density::{NoiseRouter, WorldgenData};
use folia_random::{RandomSource, XoroshiroPositionalRandomFactory};
use std::io;
use std::sync::{Arc, OnceLock};

const CELL_WIDTH: i32 = 4;
const CELL_HEIGHT: i32 = 8;

static GENERATOR: OnceLock<Arc<WorldGenerator>> = OnceLock::new();

/// Builds and installs the process-wide overworld generator. The server calls
/// this once at startup with the level seed.
pub fn configure(seed: i64) -> io::Result<()> {
    if GENERATOR.get().is_some() {
        return Ok(());
    }
    let generator = WorldGenerator::overworld(seed)?;
    let _ = GENERATOR.set(Arc::new(generator));
    Ok(())
}

/// Returns the configured generator, if startup configured one.
pub fn configured() -> Option<&'static Arc<WorldGenerator>> {
    GENERATOR.get()
}

pub struct WorldGenerator {
    router: NoiseRouter,
    biomes: BiomeSampler,
    aquifer_factory: XoroshiroPositionalRandomFactory,
    deepslate_factory: XoroshiroPositionalRandomFactory,
    bedrock_factory: XoroshiroPositionalRandomFactory,
    seed: i64,
    min_y: i32,
    height: i32,
    sea_level: i32,
}

impl WorldGenerator {
    pub fn overworld(seed: i64) -> io::Result<Self> {
        let data = WorldgenData::overworld(seed)?;
        let min_y = data.min_y();
        let height = data.height();
        let sea_level = data.sea_level();
        let aquifer_factory = data
            .seed()
            .from_hash_of("minecraft:aquifer")
            .fork_positional();
        let deepslate_factory = data
            .seed()
            .from_hash_of("minecraft:deepslate")
            .fork_positional();
        let bedrock_factory = data
            .seed()
            .from_hash_of("minecraft:bedrock_floor")
            .fork_positional();
        let biomes = BiomeSampler::from_data(&data)?;
        let router = data.router()?;
        Ok(Self {
            router,
            biomes,
            aquifer_factory,
            deepslate_factory,
            bedrock_factory,
            seed,
            min_y,
            height,
            sea_level,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    /// Evaluates the final density at a block position (no interpolation).
    pub fn density(&self, x: i32, y: i32, z: i32) -> f64 {
        self.router.final_density().compute(x, y, z)
    }

    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.biomes.sample_block(x, y, z)
    }

    /// Vanilla `bedrock_floor` vertical-gradient surface rule.
    fn is_bedrock(&self, x: i32, y: i32, z: i32) -> bool {
        if y <= self.min_y {
            return true;
        }
        if y >= self.min_y + 5 {
            return false;
        }
        let probability = map(
            f64::from(y),
            f64::from(self.min_y),
            f64::from(self.min_y + 5),
            1.0,
            0.0,
        ) as f32;
        self.bedrock_factory.at(x, y, z).next_float() < probability
    }

    /// Vanilla `deepslate` vertical-gradient surface rule.
    fn is_deepslate(&self, x: i32, y: i32, z: i32) -> bool {
        if y <= 0 {
            return true;
        }
        if y >= 8 {
            return false;
        }
        let probability = map(f64::from(y), 0.0, 8.0, 1.0, 0.0) as f32;
        self.deepslate_factory.at(x, y, z).next_float() < probability
    }

    pub fn generate_chunk(&self, chunk_x: i32, chunk_z: i32) -> io::Result<Chunk> {
        let cells_x = (16 / CELL_WIDTH + 1) as usize;
        let cells_z = cells_x;
        let cells_y = (self.height / CELL_HEIGHT + 1) as usize;
        let mut grid = vec![0.0f64; cells_x * cells_z * cells_y];

        let density = self.router.final_density();
        for xi in 0..cells_x {
            for zi in 0..cells_z {
                for yi in 0..cells_y {
                    let x = chunk_x * 16 + xi as i32 * CELL_WIDTH;
                    let y = self.min_y + yi as i32 * CELL_HEIGHT;
                    let z = chunk_z * 16 + zi as i32 * CELL_WIDTH;
                    grid[(xi * cells_z + zi) * cells_y + yi] = density.compute(x, y, z);
                }
            }
        }

        let min_section = self.min_y.div_euclid(16);
        let section_count = self.height / 16;
        let mut sections = Vec::with_capacity(section_count as usize);
        for index in 0..section_count {
            sections.push(ChunkSection {
                y: (min_section + index) as i8,
                block_states: PalettedContainer {
                    palette: vec![air()],
                    indices: vec![0; 4096],
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
                    indices: vec![0; 64],
                }),
            });
        }

        let mut chunk = Chunk {
            x: chunk_x,
            z: chunk_z,
            sections,
        };

        let mut aquifer = Aquifer::new(
            chunk_x,
            chunk_z,
            &self.router,
            &self.aquifer_factory,
            self.min_y,
            self.height,
            self.sea_level,
        );

        let mut solid = vec![false; self.height as usize];
        let mut fluids: Vec<Option<BlockState>> = vec![None; self.height as usize];
        for local_x in 0..16 {
            for local_z in 0..16 {
                let world_x = chunk_x * 16 + local_x;
                let world_z = chunk_z * 16 + local_z;
                let mut top_solid = self.min_y - 1;
                for index in 0..self.height as usize {
                    let y = self.min_y + index as i32;
                    let value = sample_grid(
                        &grid,
                        cells_x,
                        cells_z,
                        cells_y,
                        local_x,
                        local_z,
                        y - self.min_y,
                    );
                    if value > 0.0 {
                        solid[index] = true;
                    } else if let Some(state) =
                        aquifer.compute_substance(world_x, y, world_z, value)
                    {
                        if state.name != "minecraft:air" {
                            fluids[index] = Some(state);
                        }
                    } else {
                        // Aquifer pressure can turn a negative density into
                        // solid terrain.
                        solid[index] = true;
                    }
                    if solid[index] {
                        top_solid = y;
                    }
                }

                for index in 0..self.height as usize {
                    let y = self.min_y + index as i32;
                    let state = if self.is_bedrock(world_x, y, world_z) {
                        Some(BlockState {
                            name: "minecraft:bedrock".into(),
                            properties: vec![],
                        })
                    } else if solid[index] {
                        Some(surface_block(
                            y,
                            top_solid,
                            self.sea_level,
                            self.is_deepslate(world_x, y, world_z),
                        ))
                    } else {
                        fluids[index].clone()
                    };
                    if let Some(state) = state {
                        chunk.set_block_state(local_x as usize, y, local_z as usize, state);
                    }
                }
            }
        }

        Ok(chunk)
    }
}

fn sample_grid(
    grid: &[f64],
    cells_x: usize,
    cells_z: usize,
    cells_y: usize,
    local_x: i32,
    local_z: i32,
    local_y: i32,
) -> f64 {
    let cell_x = (local_x / CELL_WIDTH).clamp(0, cells_x as i32 - 2) as usize;
    let cell_z = (local_z / CELL_WIDTH).clamp(0, cells_z as i32 - 2) as usize;
    let cell_y = (local_y / CELL_HEIGHT).clamp(0, cells_y as i32 - 2) as usize;
    let frac_x = f64::from(local_x.rem_euclid(CELL_WIDTH)) / f64::from(CELL_WIDTH);
    let frac_z = f64::from(local_z.rem_euclid(CELL_WIDTH)) / f64::from(CELL_WIDTH);
    let frac_y = f64::from(local_y.rem_euclid(CELL_HEIGHT)) / f64::from(CELL_HEIGHT);

    let value = |x: usize, y: usize, z: usize| grid[(x * cells_z + z) * cells_y + y];
    let x0 = cell_x;
    let x1 = cell_x + 1;
    let z0 = cell_z;
    let z1 = cell_z + 1;
    let y0 = cell_y;
    let y1 = cell_y + 1;

    let v000 = value(x0, y0, z0);
    let v100 = value(x1, y0, z0);
    let v010 = value(x0, y1, z0);
    let v110 = value(x1, y1, z0);
    let v001 = value(x0, y0, z1);
    let v101 = value(x1, y0, z1);
    let v011 = value(x0, y1, z1);
    let v111 = value(x1, y1, z1);

    lerp(
        frac_z,
        lerp2(frac_x, frac_y, v000, v100, v010, v110),
        lerp2(frac_x, frac_y, v001, v101, v011, v111),
    )
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

fn map(input: f64, input_min: f64, input_max: f64, output_min: f64, output_max: f64) -> f64 {
    let inverse = (input - input_min) / (input_max - input_min);
    output_min + inverse * (output_max - output_min)
}

fn surface_block(y: i32, top_solid: i32, sea_level: i32, deepslate: bool) -> BlockState {
    if top_solid >= sea_level {
        if y == top_solid {
            return BlockState {
                name: "minecraft:grass_block".into(),
                properties: vec![("snowy".into(), "false".into())],
            };
        }
        if y >= top_solid - 3 {
            return BlockState {
                name: "minecraft:dirt".into(),
                properties: vec![],
            };
        }
    } else if y >= top_solid - 2 {
        return BlockState {
            name: "minecraft:sand".into(),
            properties: vec![],
        };
    }

    if deepslate {
        BlockState {
            name: "minecraft:deepslate".into(),
            properties: vec![("axis".into(), "y".into())],
        }
    } else {
        BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        }
    }
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
    use std::collections::HashMap;

    const JAVA_GRID: &str = include_str!("../../assets/java-density-grid.txt");
    const JAVA_CHUNK: &str = include_str!("../../assets/java-chunk-0--1.txt");

    #[test]
    fn generator_grid_matches_java_density_grid() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let mut checked = 0;
        for line in JAVA_GRID.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let first = parts.next().unwrap();
            if first == "seed" {
                continue;
            }
            let x: i32 = first.parse().unwrap();
            let y: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let expected: f64 = parts.next().unwrap().parse().unwrap();
            let actual = generator.density(x, y, z);
            assert!(
                (actual - expected).abs() < 1.0e-9,
                "density ({x},{y},{z}): {actual} != {expected}"
            );
            checked += 1;
        }
        assert_eq!(checked, 5 * 5 * 49);
    }

    #[test]
    fn generated_chunk_is_not_flat_and_has_water() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();
        let mut heights = Vec::new();
        let mut water = 0;
        for x in 0..16 {
            for z in 0..16 {
                let mut top = generator.min_y();
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    let state = chunk.block_state(x, y, z).unwrap();
                    if state.name == "minecraft:water" {
                        water += 1;
                    }
                    if state.name != "minecraft:air" && state.name != "minecraft:water" {
                        top = y;
                    }
                }
                heights.push(top);
            }
        }
        let min = *heights.iter().min().unwrap();
        let max = *heights.iter().max().unwrap();
        println!("generated chunk top-solid range: {min}..{max}, water blocks: {water}");
        assert!(max - min >= 4, "terrain is too flat: min={min} max={max}");
        assert!(water > 0, "expected sea-level water");
    }

    #[test]
    fn bedrock_layers_match_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut java = Vec::new();
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                if block == "minecraft:bedrock" {
                    java.push(y.parse::<i32>().unwrap());
                }
            }
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            let mut rust = Vec::new();
            for y in -64..-58 {
                if chunk.block_state(local_x, y, local_z).unwrap().name == "minecraft:bedrock" {
                    rust.push(y);
                }
            }
            assert_eq!(rust, java, "bedrock layers differ at column ({x},{z})");
        }
    }

    #[test]
    fn generated_chunk_block_classes_are_close_to_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        let mut matched = 0u64;
        let mut exact_matched = 0u64;
        let mut total = 0u64;
        let mut water_matched = 0u64;
        let mut java_water = 0u64;
        let mut rust_water = 0u64;

        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut java: std::collections::HashMap<i32, &str> = std::collections::HashMap::new();
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                java.insert(y.parse::<i32>().unwrap(), block);
            }
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            for y in -64..320 {
                let state = chunk.block_state(local_x, y, local_z).unwrap();
                let rust = rust_class(&state.name);
                let expected_name = java.get(&y).copied().unwrap_or("minecraft:air");
                let expected = java_class(expected_name);
                if state.name == expected_name {
                    exact_matched += 1;
                }
                if rust == expected {
                    matched += 1;
                }
                if rust == 2 {
                    rust_water += 1;
                }
                if expected == 2 {
                    java_water += 1;
                }
                if rust == 2 && expected == 2 {
                    water_matched += 1;
                }
                total += 1;
            }
        }
        let agreement = matched as f64 / total as f64;
        println!(
            "block-class agreement: {:.1}% ({matched}/{total}), exact names: {:.1}%, water Rust={rust_water} Java={java_water} matched={water_matched}",
            agreement * 100.0,
            exact_matched as f64 / total as f64 * 100.0
        );
        assert!(
            agreement > 0.80,
            "block-class agreement too low: {agreement}"
        );
    }

    fn java_class(block: &str) -> u8 {
        match block {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
            "minecraft:water" => 2,
            "minecraft:lava" => 3,
            _ => 1,
        }
    }

    fn rust_class(name: &str) -> u8 {
        match name {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
            "minecraft:water" => 2,
            "minecraft:lava" => 3,
            _ => 1,
        }
    }

    #[test]
    fn generated_surface_height_is_close_to_java() {
        let generator = WorldGenerator::overworld(12345).unwrap();
        let chunk = generator.generate_chunk(0, -1).unwrap();

        let mut java_heights: HashMap<(i32, i32), i32> = HashMap::new();
        for line in JAVA_CHUNK.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("column") {
                continue;
            }
            let x: i32 = parts.next().unwrap().parse().unwrap();
            let z: i32 = parts.next().unwrap().parse().unwrap();
            let mut top = -64;
            for entry in parts {
                let (y, block) = entry.split_once('=').unwrap();
                let y: i32 = y.parse().unwrap();
                if !matches!(
                    block,
                    "minecraft:air"
                        | "minecraft:cave_air"
                        | "minecraft:water"
                        | "minecraft:lava"
                        | "minecraft:short_grass"
                        | "minecraft:seagrass"
                        | "minecraft:moss_carpet"
                ) {
                    top = top.max(y);
                }
            }
            java_heights.insert((x, z), top);
        }

        let mut total_difference = 0;
        let mut count = 0;
        for ((x, z), java_top) in java_heights {
            let local_x = x.rem_euclid(16) as usize;
            let local_z = z.rem_euclid(16) as usize;
            let mut rust_top = -64;
            for y in -64..320 {
                let state = chunk.block_state(local_x, y, local_z).unwrap();
                if !matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:water" | "minecraft:cave_air"
                ) {
                    rust_top = y;
                }
            }
            total_difference += (rust_top - java_top).abs();
            count += 1;
        }
        let average = f64::from(total_difference) / f64::from(count);
        println!("Java/Rust surface-height columns: {count}, average difference: {average:.2}");
        assert!(
            average < 12.0,
            "average surface height difference too large: {average}"
        );
    }
}
