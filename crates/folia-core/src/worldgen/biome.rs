//! Multi-noise biome sampling.
//!
//! Parses the overworld `MultiNoiseBiomeSource` parameter list dumped from a
//! real Java server, samples the six climate dimensions from the existing
//! noise router, and finds the biome with the minimum parameter fitness.

use crate::worldgen::density::{DensityFunction, WorldgenData};
use std::io;
use std::sync::Arc;

const BIOME_PARAMS: &str = include_str!("../../assets/biome_params.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameter {
    pub min: i64,
    pub max: i64,
}

impl Parameter {
    fn distance(self, point: i64) -> i64 {
        let above = point - self.max;
        let below = self.min - point;
        if above > 0 { above } else { below.max(0) }
    }
}

#[derive(Debug, Clone)]
pub struct BiomeEntry {
    pub biome: String,
    pub temperature: Parameter,
    pub humidity: Parameter,
    pub continentalness: Parameter,
    pub erosion: Parameter,
    pub depth: Parameter,
    pub weirdness: Parameter,
    pub offset: i64,
}

pub struct BiomeSampler {
    entries: Vec<BiomeEntry>,
    temperature: Arc<DensityFunction>,
    humidity: Arc<DensityFunction>,
    continentalness: Arc<DensityFunction>,
    erosion: Arc<DensityFunction>,
    depth: Arc<DensityFunction>,
    weirdness: Arc<DensityFunction>,
}

impl BiomeSampler {
    pub fn overworld(seed: i64) -> io::Result<Self> {
        let data = WorldgenData::overworld(seed)?;
        Self::from_data(&data)
    }

    pub fn from_data(data: &WorldgenData) -> io::Result<Self> {
        let router = data.router()?;
        let entries = parse_params(BIOME_PARAMS)?;
        Ok(Self {
            entries,
            temperature: router.function("temperature")?.clone(),
            humidity: router.function("vegetation")?.clone(),
            continentalness: router.function("continents")?.clone(),
            erosion: router.function("erosion")?.clone(),
            depth: router.function("depth")?.clone(),
            weirdness: router.function("ridges")?.clone(),
        })
    }

    /// Samples the biome for a quart position, matching
    /// `MultiNoiseBiomeSource.getNoiseBiome`.
    pub fn sample_quart(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> &str {
        let x = quart_x << 2;
        let y = quart_y << 2;
        let z = quart_z << 2;
        let target = [
            quantize(self.temperature.compute(x, y, z) as f32),
            quantize(self.humidity.compute(x, y, z) as f32),
            quantize(self.continentalness.compute(x, y, z) as f32),
            quantize(self.erosion.compute(x, y, z) as f32),
            quantize(self.depth.compute(x, y, z) as f32),
            quantize(self.weirdness.compute(x, y, z) as f32),
        ];

        let mut best = &self.entries[0];
        let mut best_fitness = fitness(best, &target);
        for entry in &self.entries[1..] {
            let value = fitness(entry, &target);
            if value < best_fitness {
                best = entry;
                best_fitness = value;
            }
        }
        &best.biome
    }

    /// Samples the biome containing a block position.
    pub fn sample_block(&self, x: i32, y: i32, z: i32) -> &str {
        self.sample_quart(x >> 2, y >> 2, z >> 2)
    }
}

fn fitness(entry: &BiomeEntry, target: &[i64; 6]) -> i64 {
    let mut value = 0i64;
    value = value.saturating_add(square(entry.temperature.distance(target[0])));
    value = value.saturating_add(square(entry.humidity.distance(target[1])));
    value = value.saturating_add(square(entry.continentalness.distance(target[2])));
    value = value.saturating_add(square(entry.erosion.distance(target[3])));
    value = value.saturating_add(square(entry.depth.distance(target[4])));
    value = value.saturating_add(square(entry.weirdness.distance(target[5])));
    value.saturating_add(square(entry.offset))
}

fn square(value: i64) -> i64 {
    value.saturating_mul(value)
}

fn quantize(value: f32) -> i64 {
    (value * 10000.0) as i64
}

fn parse_params(input: &str) -> io::Result<Vec<BiomeEntry>> {
    let mut entries = Vec::new();
    for line in input.lines() {
        let mut parts = line.split('\t');
        if parts.next() != Some("param") {
            continue;
        }
        let biome = parts
            .next()
            .ok_or_else(|| invalid("biome parameter is missing biome"))?
            .to_owned();
        let mut values = [0i64; 13];
        for value in &mut values {
            *value = parts
                .next()
                .ok_or_else(|| invalid("biome parameter is truncated"))?
                .parse()
                .map_err(|error| invalid(format!("invalid biome parameter: {error}")))?;
        }
        entries.push(BiomeEntry {
            biome,
            temperature: Parameter {
                min: values[0],
                max: values[1],
            },
            humidity: Parameter {
                min: values[2],
                max: values[3],
            },
            continentalness: Parameter {
                min: values[4],
                max: values[5],
            },
            erosion: Parameter {
                min: values[6],
                max: values[7],
            },
            depth: Parameter {
                min: values[8],
                max: values[9],
            },
            weirdness: Parameter {
                min: values[10],
                max: values[11],
            },
            offset: values[12],
        });
    }
    if entries.is_empty() {
        return Err(invalid("biome parameter list is empty"));
    }
    Ok(entries)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAVA_SAMPLES: &str = include_str!("../../assets/java-biome-samples.txt");

    #[test]
    fn biome_sampling_matches_java_oracle() {
        let sampler = BiomeSampler::overworld(12345).unwrap();
        let mut checked = 0;
        for line in JAVA_SAMPLES.lines() {
            let mut parts = line.split('\t');
            if parts.next() != Some("biome") {
                continue;
            }
            let qx: i32 = parts.next().unwrap().parse().unwrap();
            let qy: i32 = parts.next().unwrap().parse().unwrap();
            let qz: i32 = parts.next().unwrap().parse().unwrap();
            let expected = parts.next().unwrap();
            let actual = sampler.sample_quart(qx, qy, qz);
            assert_eq!(actual, expected, "biome mismatch at quart ({qx},{qy},{qz})");
            checked += 1;
        }
        assert_eq!(checked, 1536);
    }
}
