//! Multi-noise biome sampling.
//!
//! Parses the overworld `MultiNoiseBiomeSource` parameter list dumped from a
//! real Java server, samples the six climate dimensions from the existing
//! noise router, and finds the biome with the minimum parameter fitness.

use crate::worldgen::density::{DensityFunction, WorldgenData};
use serde_json::Value;
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

/// Climate parameters for a biome added by a datapack.
///
/// Values are the quantized `Climate.Parameter` integers used by
/// `assets/biome_params.txt` (Java's `quantize()` scales the raw density by
/// 10,000). Custom entries are placed before the vanilla entries so an exact
/// match wins the `ParameterList.search` tie-break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomBiomeClimate {
    pub temperature: [i64; 2],
    pub humidity: [i64; 2],
    pub continentalness: [i64; 2],
    pub erosion: [i64; 2],
    pub depth: [i64; 2],
    pub weirdness: [i64; 2],
    pub offset: i64,
}

impl CustomBiomeClimate {
    /// Parses the supported `climate` object from a datapack biome document.
    pub fn parse(value: &Value) -> io::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("biome `climate` must be an object"))?;
        let pair = |key: &str| -> io::Result<[i64; 2]> {
            let array = object
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| invalid(format!("biome `climate` is missing `{key}`")))?;
            if array.len() != 2 {
                return Err(invalid(format!(
                    "biome `climate.{key}` must hold exactly two integers"
                )));
            }
            let min = array[0]
                .as_i64()
                .ok_or_else(|| invalid(format!("biome `climate.{key}` min is not an integer")))?;
            let max = array[1]
                .as_i64()
                .ok_or_else(|| invalid(format!("biome `climate.{key}` max is not an integer")))?;
            if min > max {
                return Err(invalid(format!(
                    "biome `climate.{key}` has min {min} greater than max {max}"
                )));
            }
            Ok([min, max])
        };
        let offset = object
            .get("offset")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("biome `climate` is missing `offset`"))?;
        Ok(Self {
            temperature: pair("temperature")?,
            humidity: pair("humidity")?,
            continentalness: pair("continentalness")?,
            erosion: pair("erosion")?,
            depth: pair("depth")?,
            weirdness: pair("weirdness")?,
            offset,
        })
    }

    /// Converts to a parameter-list entry with the datapack biome id.
    pub fn entry(&self, id: &str) -> BiomeEntry {
        let parameter = |range: [i64; 2]| Parameter {
            min: range[0],
            max: range[1],
        };
        BiomeEntry {
            biome: id.to_owned(),
            temperature: parameter(self.temperature),
            humidity: parameter(self.humidity),
            continentalness: parameter(self.continentalness),
            erosion: parameter(self.erosion),
            depth: parameter(self.depth),
            weirdness: parameter(self.weirdness),
            offset: self.offset,
        }
    }
}

#[derive(Clone)]
pub struct BiomeSampler {
    entries: Vec<BiomeEntry>,
    /// Number of leading entries added by datapacks. Their exact parameter
    /// ranges take precedence over the vanilla parameter-list search.
    custom_count: usize,
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
            custom_count: 0,
            root,
            temperature: router.function("temperature")?.clone(),
            humidity: router.function("vegetation")?.clone(),
            continentalness: router.function("continents")?.clone(),
            erosion: router.function("erosion")?.clone(),
            depth: router.function("depth")?.clone(),
            weirdness: router.function("ridges")?.clone(),
        })
    }

    /// Extends the climate parameter list with datapack biomes.
    ///
    /// Custom entries are prepended so an exact parameter match wins
    /// `ParameterList.search`'s strict `<` tie-break over a vanilla entry with
    /// an overlapping range. The parameter tree is rebuilt from the combined
    /// list; all vanilla entries are kept, so unmodified samples are unchanged.
    pub fn with_custom_biomes(
        mut self,
        extras: Vec<(String, CustomBiomeClimate)>,
    ) -> io::Result<Self> {
        if extras.is_empty() {
            return Ok(self);
        }
        let mut seen = std::collections::HashSet::new();
        let mut custom = Vec::with_capacity(extras.len());
        for (id, climate) in extras {
            if !seen.insert(id.clone()) {
                return Err(invalid(format!("duplicate custom biome `{id}`")));
            }
            if self.entries.iter().any(|entry| entry.biome == id) {
                return Err(invalid(format!(
                    "custom biome `{id}` collides with a vanilla biome; only new ids can be added \
                     to the climate parameter list"
                )));
            }
            custom.push(climate.entry(&id));
        }
        self.custom_count = custom.len();
        custom.extend(std::mem::take(&mut self.entries));
        self.entries = custom;
        self.root = build_tree(&self.entries);
        Ok(self)
    }

    /// Exact-match override for datapack biomes, checked before the vanilla
    /// parameter tree. Without custom entries this always returns `None`, so
    /// vanilla sampling is byte-for-byte unchanged.
    fn exact_custom(&self, target: &[i64; 7]) -> Option<usize> {
        self.entries
            .iter()
            .take(self.custom_count)
            .position(|entry| {
                entry
                    .parameter_space()
                    .iter()
                    .zip(target)
                    .all(|(parameter, point)| parameter.distance(*point) == 0)
            })
    }

    /// Quantized climate point sampled at a quart, in the order used by
    /// `Parameter.distance`: temperature, humidity, continentalness, erosion,
    /// depth, weirdness, offset.
    pub fn climate_at(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> [i64; 7] {
        let x = quart_x << 2;
        let y = quart_y << 2;
        let z = quart_z << 2;
        [
            quantize(self.temperature.compute(x, y, z) as f32),
            quantize(self.humidity.compute(x, y, z) as f32),
            quantize(self.continentalness.compute(x, y, z) as f32),
            quantize(self.erosion.compute(x, y, z) as f32),
            quantize(self.depth.compute(x, y, z) as f32),
            quantize(self.weirdness.compute(x, y, z) as f32),
            0,
        ]
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
        self.exact_custom(&target)
            .unwrap_or_else(|| self.root.search(&target, &self.entries))
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
            result.push(
                self.exact_custom(&target)
                    .unwrap_or_else(|| self.root.search(&target, &self.entries)),
            );
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
