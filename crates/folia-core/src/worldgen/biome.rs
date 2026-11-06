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
    root: Node,
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
        let root = build_tree(&entries);
        Ok(Self {
            entries,
            root,
            temperature: router.function("temperature")?.clone(),
            humidity: router.function("vegetation")?.clone(),
            continentalness: router.function("continents")?.clone(),
            erosion: router.function("erosion")?.clone(),
            depth: router.function("depth")?.clone(),
            weirdness: router.function("ridges")?.clone(),
        })
    }

    /// Biome names in the parameter list's first-encounter order.
    ///
    /// Mirrors `BiomeSource.possibleBiomes()`, whose Guava immutable-set
    /// iteration order is the order `Climate.ParameterList.values()` produced.
    pub fn possible_biomes(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for entry in &self.entries {
            if seen.insert(entry.biome.as_str()) {
                names.push(entry.biome.as_str());
            }
        }
        names
    }

    /// Samples the biome for a quart position and returns its entry index.
    ///
    /// The index is stable for the lifetime of the sampler and lets callers
    /// cache a whole chunk's biome cells without cloning names.
    pub fn sample_quart_index(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> usize {
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
            0,
        ];
        self.root.search(&target, &self.entries)
    }

    /// Samples a whole vertical quart column.
    ///
    /// In the overworld router `temperature`, `vegetation`, `continents`,
    /// `erosion` and `ridges` all have `y_scale: 0` (and the three terrain
    /// dimensions are `flat_cache`/`cache_2d` wrappers), so they are constant
    /// down a column. Evaluate them once and only recompute `depth`, which is
    /// the y gradient plus the 2D offset.
    pub fn sample_quart_column(
        &self,
        quart_x: i32,
        quart_z: i32,
        quart_y_start: i32,
        count: usize,
    ) -> Vec<usize> {
        let x = quart_x << 2;
        let z = quart_z << 2;
        let temperature = quantize(self.temperature.compute(x, 0, z) as f32);
        let humidity = quantize(self.humidity.compute(x, 0, z) as f32);
        let continentalness = quantize(self.continentalness.compute(x, 0, z) as f32);
        let erosion = quantize(self.erosion.compute(x, 0, z) as f32);
        let weirdness = quantize(self.weirdness.compute(x, 0, z) as f32);

        let mut result = Vec::with_capacity(count);
        for layer in 0..count {
            let quart_y = quart_y_start + layer as i32;
            let depth = quantize(self.depth.compute(x, quart_y << 2, z) as f32);
            let target = [
                temperature,
                humidity,
                continentalness,
                erosion,
                depth,
                weirdness,
                0,
            ];
            result.push(self.root.search(&target, &self.entries));
        }
        result
    }

    /// Resolves a biome entry index returned by [`Self::sample_quart_index`].
    pub fn biome_name(&self, index: usize) -> &str {
        &self.entries[index].biome
    }

    /// Samples the biome for a quart position, matching
    /// `MultiNoiseBiomeSource.getNoiseBiome`.
    pub fn sample_quart(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> &str {
        self.biome_name(self.sample_quart_index(quart_x, quart_y, quart_z))
    }

    /// Samples the biome containing a block position.
    pub fn sample_block(&self, x: i32, y: i32, z: i32) -> &str {
        self.sample_quart(x >> 2, y >> 2, z >> 2)
    }
}

impl BiomeEntry {
    fn parameter_space(&self) -> [Parameter; 7] {
        [
            self.temperature,
            self.humidity,
            self.continentalness,
            self.erosion,
            self.depth,
            self.weirdness,
            Parameter {
                min: self.offset,
                max: self.offset,
            },
        ]
    }
}

#[derive(Debug, Clone)]
enum Node {
    Leaf(usize),
    SubTree {
        space: [Parameter; 7],
        children: Vec<Node>,
    },
}

fn build_tree(entries: &[BiomeEntry]) -> Node {
    let leaves = entries
        .iter()
        .enumerate()
        .map(|(index, _)| Node::Leaf(index))
        .collect();
    build(leaves, entries)
}

fn build(children: Vec<Node>, entries: &[BiomeEntry]) -> Node {
    if children.len() == 1 {
        return children.into_iter().next().unwrap();
    }
    if children.len() <= 6 {
        let mut children = children;
        children.sort_by_key(|node| {
            let space = node_space(node, entries);
            let mut value = 0i64;
            for parameter in space {
                value += ((parameter.min + parameter.max) / 2).abs();
            }
            value
        });
        let space = union_space(&children, entries);
        return Node::SubTree { space, children };
    }

    let mut best: Option<(i64, usize, Vec<Vec<Node>>)> = None;
    for dimension in 0..7 {
        let mut sorted = children.clone();
        sort_nodes(&mut sorted, dimension, false, entries);
        let buckets = bucketize(&sorted);
        let mut cost = 0i64;
        for bucket in &buckets {
            let space = union_space(bucket, entries);
            for parameter in space {
                cost += (parameter.max - parameter.min).abs();
            }
        }
        if best
            .as_ref()
            .is_none_or(|(best_cost, _, _)| cost < *best_cost)
        {
            best = Some((cost, dimension, buckets));
        }
    }

    let (_, dimension, mut buckets) = best.expect("at least one dimension");
    buckets.sort_by_key(|bucket| {
        let space = union_space(bucket, entries);
        let parameter = space[dimension];
        ((parameter.min + parameter.max) / 2).abs()
    });
    let built: Vec<Node> = buckets
        .into_iter()
        .map(|bucket| build(bucket, entries))
        .collect();
    let space = union_space(&built, entries);
    Node::SubTree {
        space,
        children: built,
    }
}

fn bucketize(nodes: &[Node]) -> Vec<Vec<Node>> {
    let count = nodes.len();
    let exponent = (((count as f64 - 0.01).ln()) / 6.0f64.ln()).floor() as u32;
    let bucket_size = 6usize.pow(exponent);
    let mut buckets = Vec::new();
    let mut current = Vec::new();
    for node in nodes {
        current.push(node.clone());
        if current.len() >= bucket_size {
            buckets.push(current);
            current = Vec::new();
        }
    }
    if !current.is_empty() {
        buckets.push(current);
    }
    buckets
}

fn sort_nodes(nodes: &mut [Node], dimension: usize, absolute: bool, entries: &[BiomeEntry]) {
    nodes.sort_by(|first, second| {
        for step in 0..7 {
            let dim = (dimension + step) % 7;
            let first_center = {
                let parameter = node_space(first, entries)[dim];
                (parameter.min + parameter.max) / 2
            };
            let second_center = {
                let parameter = node_space(second, entries)[dim];
                (parameter.min + parameter.max) / 2
            };
            let first_value = if absolute {
                first_center.abs()
            } else {
                first_center
            };
            let second_value = if absolute {
                second_center.abs()
            } else {
                second_center
            };
            match first_value.cmp(&second_value) {
                std::cmp::Ordering::Equal => continue,
                ordering => return ordering,
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn node_space(node: &Node, entries: &[BiomeEntry]) -> [Parameter; 7] {
    match node {
        Node::Leaf(index) => entries[*index].parameter_space(),
        Node::SubTree { space, .. } => *space,
    }
}

fn union_space(nodes: &[Node], entries: &[BiomeEntry]) -> [Parameter; 7] {
    let mut space = node_space(&nodes[0], entries);
    for node in &nodes[1..] {
        let other = node_space(node, entries);
        for (slot, parameter) in space.iter_mut().zip(other) {
            slot.min = slot.min.min(parameter.min);
            slot.max = slot.max.max(parameter.max);
        }
    }
    space
}

impl Node {
    fn distance(&self, target: &[i64; 7], entries: &[BiomeEntry]) -> i64 {
        let space = node_space(self, entries);
        let mut value = 0i64;
        for (parameter, point) in space.iter().zip(target) {
            value = value.saturating_add(square(parameter.distance(*point)));
        }
        value
    }

    fn search(&self, target: &[i64; 7], entries: &[BiomeEntry]) -> usize {
        match self {
            Node::Leaf(index) => *index,
            Node::SubTree { children, .. } => {
                let mut best_leaf = None;
                let mut best_distance = i64::MAX;
                for child in children {
                    let child_distance = child.distance(target, entries);
                    if child_distance < best_distance {
                        let leaf = child.search(target, entries);
                        let leaf_distance = if matches!(child, Node::Leaf(_)) {
                            child_distance
                        } else {
                            Node::Leaf(leaf).distance(target, entries)
                        };
                        if leaf_distance < best_distance {
                            best_distance = leaf_distance;
                            best_leaf = Some(leaf);
                        }
                    }
                }
                best_leaf.unwrap_or(0)
            }
        }
    }
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
