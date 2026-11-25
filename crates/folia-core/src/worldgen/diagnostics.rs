use crate::chunk::{BlockState, Chunk};
pub use crate::worldgen::feature::FeaturePlacement;
use crate::worldgen::structure::piece::BoundingBox;
use crate::worldgen::structure::start::StructureStartSummary;
use folia_math::BlockPos;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const DUMP_FORMAT_VERSION: u32 = 1;
pub const DUMP_DATA_VERSION: i32 = 4788;
pub const DEFAULT_SAMPLE_STRIDE: i32 = 4;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerationDiagnostics {
    pub features: Vec<FeaturePlacement>,
    pub structures: Vec<StructureStartSummary>,
    capture: bool,
}

impl GenerationDiagnostics {
    pub fn capturing() -> Self {
        Self {
            features: Vec::new(),
            structures: Vec::new(),
            capture: true,
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.capture
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosedChunk {
    pub chunk: Chunk,
    pub diagnostics: GenerationDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SampledBlock {
    pub position: [i32; 3],
    pub state: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructureDump {
    pub structure_id: String,
    pub start_chunk: [i32; 2],
    pub bounds: Option<[i32; 6]>,
    pub placed_pieces: usize,
    pub skipped_pieces: usize,
    pub written: usize,
    pub rejected: usize,
    pub feature_elements: usize,
    pub feature_placements: usize,
    pub sampled_blocks: Vec<SampledBlock>,
}

impl StructureDump {
    pub fn match_key(&self) -> (&str, [i32; 2]) {
        (&self.structure_id, self.start_chunk)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkDump {
    pub chunk: [i32; 2],
    pub block_digest: u64,
    pub features: Vec<FeaturePlacement>,
    pub structures: Vec<StructureDump>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldgenDump {
    pub seed: i64,
    pub chunks: Vec<ChunkDump>,
}

impl WorldgenDump {
    pub fn from_chunk(seed: i64, generated: &DiagnosedChunk, sample_stride: i32) -> Self {
        Self {
            seed,
            chunks: vec![ChunkDump::from_generated(generated, sample_stride)],
        }
    }

    pub fn from_chunks<'a>(
        seed: i64,
        generated: impl IntoIterator<Item = &'a DiagnosedChunk>,
        sample_stride: i32,
    ) -> Self {
        Self {
            seed,
            chunks: generated
                .into_iter()
                .map(|chunk| ChunkDump::from_generated(chunk, sample_stride))
                .collect(),
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "format_version": DUMP_FORMAT_VERSION,
            "data_version": DUMP_DATA_VERSION,
            "seed": self.seed,
            "chunks": self.chunks.iter().map(chunk_value).collect::<Vec<_>>(),
        })
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(&self.to_value()).expect("worldgen dump is JSON serializable")
    }
}

impl ChunkDump {
    fn from_generated(generated: &DiagnosedChunk, sample_stride: i32) -> Self {
        let mut features = generated.diagnostics.features.clone();
        features.sort_by(|left, right| {
            left.match_key()
                .cmp(&right.match_key())
                .then_with(|| left.success.cmp(&right.success))
                .then_with(|| left.written_blocks.cmp(&right.written_blocks))
        });
        let mut structures: Vec<_> = generated
            .diagnostics
            .structures
            .iter()
            .map(|start| structure_dump(&generated.chunk, start, sample_stride))
            .collect();
        structures.sort_by(|left, right| {
            left.match_key()
                .cmp(&right.match_key())
                .then_with(|| left.bounds.cmp(&right.bounds))
        });
        Self {
            chunk: [generated.chunk.x, generated.chunk.z],
            block_digest: chunk_digest(&generated.chunk),
            features,
            structures,
        }
    }
}

fn chunk_value(chunk: &ChunkDump) -> serde_json::Value {
    serde_json::json!({
        "chunk": chunk.chunk,
        "block_digest": chunk.block_digest,
        "features": chunk.features.iter().map(feature_value).collect::<Vec<_>>(),
        "structures": chunk.structures.iter().map(structure_value).collect::<Vec<_>>(),
    })
}

fn feature_value(feature: &FeaturePlacement) -> serde_json::Value {
    serde_json::json!({
        "placed_feature": feature.placed_feature,
        "configured_feature": feature.configured_feature,
        "decoration": feature.decoration,
        "origin": feature.origin,
        "success": feature.success,
        "written_blocks": feature.written_blocks,
    })
}

fn structure_value(structure: &StructureDump) -> serde_json::Value {
    serde_json::json!({
        "structure_id": structure.structure_id,
        "start_chunk": structure.start_chunk,
        "bounds": structure.bounds,
        "placed_pieces": structure.placed_pieces,
        "skipped_pieces": structure.skipped_pieces,
        "written": structure.written,
        "rejected": structure.rejected,
        "feature_elements": structure.feature_elements,
        "feature_placements": structure.feature_placements,
        "sampled_blocks": structure.sampled_blocks.iter().map(|block| {
            serde_json::json!({
                "position": block.position,
                "state": block.state,
            })
        }).collect::<Vec<_>>(),
    })
}

fn structure_dump(
    chunk: &Chunk,
    start: &StructureStartSummary,
    sample_stride: i32,
) -> StructureDump {
    let sampled_blocks = start
        .bounding_box
        .map(|bounds| sample_blocks(chunk, bounds_value(bounds), sample_stride))
        .unwrap_or_default();
    StructureDump {
        structure_id: start.structure_id.clone(),
        start_chunk: [start.chunk_pos.x, start.chunk_pos.z],
        bounds: start.bounding_box.map(bounds_value),
        placed_pieces: start.placed_pieces,
        skipped_pieces: start.skipped_pieces,
        written: start.written,
        rejected: start.rejected,
        feature_elements: start.feature_elements,
        feature_placements: start.feature_placements,
        sampled_blocks,
    }
}

fn sample_blocks(chunk: &Chunk, bounds: [i32; 6], stride: i32) -> Vec<SampledBlock> {
    let stride = stride.max(1);
    let min_x = bounds[0].max(chunk.x * 16);
    let max_x = bounds[3].min(chunk.x * 16 + 15);
    let min_y = bounds[1];
    let max_y = bounds[4];
    let min_z = bounds[2].max(chunk.z * 16);
    let max_z = bounds[5].min(chunk.z * 16 + 15);
    let mut blocks = Vec::new();
    if min_x > max_x || min_y > max_y || min_z > max_z {
        return blocks;
    }
    for x in sample_axis(min_x, max_x, stride) {
        for y in sample_axis(min_y, max_y, stride) {
            for z in sample_axis(min_z, max_z, stride) {
                let Some(state) = chunk.block_state((x & 15) as usize, y, (z & 15) as usize) else {
                    continue;
                };
                blocks.push(SampledBlock {
                    position: [x, y, z],
                    state: state_value(state),
                });
            }
        }
    }
    blocks
}

fn sample_axis(min: i32, max: i32, stride: i32) -> Vec<i32> {
    let mut values: Vec<_> = (min..=max).step_by(stride as usize).collect();
    if values.last().copied() != Some(max) {
        values.push(max);
    }
    values
}

fn state_value(state: &BlockState) -> String {
    if state.properties.is_empty() {
        return state.name.clone();
    }
    let mut properties = state.properties.clone();
    properties.sort();
    let properties = properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}[{}]", state.name, properties)
}

fn bounds_value(bounds: BoundingBox) -> [i32; 6] {
    [
        bounds.min_x(),
        bounds.min_y(),
        bounds.min_z(),
        bounds.max_x(),
        bounds.max_y(),
        bounds.max_z(),
    ]
}

pub fn chunk_digest(chunk: &Chunk) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for x in 0..16 {
        for z in 0..16 {
            for y in -64..320 {
                let state = chunk
                    .block_state(x, y, z)
                    .map_or_else(|| "minecraft:air".to_owned(), state_value);
                hash_bytes(&mut hash, &(chunk.x).to_le_bytes());
                hash_bytes(&mut hash, &(chunk.z).to_le_bytes());
                hash_bytes(&mut hash, &(x as i32).to_le_bytes());
                hash_bytes(&mut hash, &(y).to_le_bytes());
                hash_bytes(&mut hash, &(z as i32).to_le_bytes());
                hash_bytes(&mut hash, &(state.len() as u64).to_le_bytes());
                hash_bytes(&mut hash, state.as_bytes());
            }
        }
    }
    hash
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChunkKey {
    pub seed: i64,
    pub chunk: [i32; 2],
}

impl fmt::Display for ChunkKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "seed={} chunk=({},{})",
            self.seed, self.chunk[0], self.chunk[1]
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerrainComparison {
    pub total: u64,
    pub exact_matches: u64,
    pub class_matches: u64,
    pub rust_water: u64,
    pub java_water: u64,
    pub water_mismatches: u64,
    pub class_confusions: BTreeMap<(u8, u8), u64>,
    pub name_confusions: BTreeMap<(String, String), u64>,
}

impl TerrainComparison {
    pub fn class_agreement(&self) -> f64 {
        ratio(self.class_matches, self.total)
    }

    pub fn exact_agreement(&self) -> f64 {
        ratio(self.exact_matches, self.total)
    }

    pub fn merge(&mut self, other: &Self) {
        self.total += other.total;
        self.exact_matches += other.exact_matches;
        self.class_matches += other.class_matches;
        self.rust_water += other.rust_water;
        self.java_water += other.java_water;
        self.water_mismatches += other.water_mismatches;
        merge_counts(&mut self.class_confusions, &other.class_confusions);
        merge_counts(&mut self.name_confusions, &other.name_confusions);
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn merge_counts<K: Ord + Clone>(target: &mut BTreeMap<K, u64>, source: &BTreeMap<K, u64>) {
    for (key, count) in source {
        *target.entry(key.clone()).or_insert(0) += count;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureMismatchKind {
    MissingInRust,
    UnexpectedInRust,
    Success,
    WrittenBlocks,
}

impl FeatureMismatchKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingInRust => "missing_in_rust",
            Self::UnexpectedInRust => "unexpected_in_rust",
            Self::Success => "success",
            Self::WrittenBlocks => "written_blocks",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeatureMismatch {
    pub kind: FeatureMismatchKind,
    pub origin: [i32; 3],
    pub java: Option<FeaturePlacement>,
    pub rust: Option<FeaturePlacement>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeatureDifference {
    pub java_placements: u64,
    pub rust_placements: u64,
    pub matched_placements: u64,
    pub mismatches: Vec<FeatureMismatch>,
}

impl FeatureDifference {
    pub fn merge(&mut self, other: &Self) {
        self.java_placements += other.java_placements;
        self.rust_placements += other.rust_placements;
        self.matched_placements += other.matched_placements;
        self.mismatches.extend(other.mismatches.iter().cloned());
        self.mismatches.sort_by(|left, right| {
            left.origin
                .cmp(&right.origin)
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
        });
    }

    pub fn is_match(&self) -> bool {
        self.mismatches.is_empty()
            && self.java_placements == self.rust_placements
            && self.java_placements == self.matched_placements
    }
}

pub fn compare_features(
    java: &[FeaturePlacement],
    rust: &[FeaturePlacement],
) -> BTreeMap<String, FeatureDifference> {
    let mut java_groups: BTreeMap<(&str, &str, &str, [i32; 3]), Vec<&FeaturePlacement>> =
        BTreeMap::new();
    let mut rust_groups: BTreeMap<(&str, &str, &str, [i32; 3]), Vec<&FeaturePlacement>> =
        BTreeMap::new();
    for placement in java {
        java_groups
            .entry(placement.match_key())
            .or_default()
            .push(placement);
    }
    for placement in rust {
        rust_groups
            .entry(placement.match_key())
            .or_default()
            .push(placement);
    }
    let mut keys: BTreeSet<_> = java_groups.keys().copied().collect();
    keys.extend(rust_groups.keys().copied());
    let mut result: BTreeMap<String, FeatureDifference> = BTreeMap::new();
    for key in keys {
        let mut java_items = java_groups.remove(&key).unwrap_or_default();
        let mut rust_items = rust_groups.remove(&key).unwrap_or_default();
        java_items.sort_by(|left, right| {
            left.success
                .cmp(&right.success)
                .then_with(|| left.written_blocks.cmp(&right.written_blocks))
        });
        rust_items.sort_by(|left, right| {
            left.success
                .cmp(&right.success)
                .then_with(|| left.written_blocks.cmp(&right.written_blocks))
        });
        let difference = result.entry(key.0.to_owned()).or_default();
        difference.java_placements += java_items.len() as u64;
        difference.rust_placements += rust_items.len() as u64;
        let common = java_items.len().min(rust_items.len());
        for index in 0..common {
            let java_placement = java_items[index];
            let rust_placement = rust_items[index];
            difference.matched_placements += 1;
            if java_placement.success != rust_placement.success {
                difference.mismatches.push(FeatureMismatch {
                    kind: FeatureMismatchKind::Success,
                    origin: key.3,
                    java: Some((*java_placement).clone()),
                    rust: Some((*rust_placement).clone()),
                });
            }
            if java_placement.written_blocks != rust_placement.written_blocks {
                difference.mismatches.push(FeatureMismatch {
                    kind: FeatureMismatchKind::WrittenBlocks,
                    origin: key.3,
                    java: Some((*java_placement).clone()),
                    rust: Some((*rust_placement).clone()),
                });
            }
        }
        for java_placement in &java_items[common..] {
            difference.mismatches.push(FeatureMismatch {
                kind: FeatureMismatchKind::MissingInRust,
                origin: key.3,
                java: Some((*java_placement).clone()),
                rust: None,
            });
        }
        for rust_placement in &rust_items[common..] {
            difference.mismatches.push(FeatureMismatch {
                kind: FeatureMismatchKind::UnexpectedInRust,
                origin: key.3,
                java: None,
                rust: Some((*rust_placement).clone()),
            });
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureMismatchKind {
    MissingInRust,
    UnexpectedInRust,
    Bounds,
    PieceCounts,
    SampledBlocks,
}

impl StructureMismatchKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingInRust => "missing_in_rust",
            Self::UnexpectedInRust => "unexpected_in_rust",
            Self::Bounds => "bounds",
            Self::PieceCounts => "piece_counts",
            Self::SampledBlocks => "sampled_blocks",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructureMismatch {
    pub kind: StructureMismatchKind,
    pub start_chunk: [i32; 2],
    pub java: Option<StructureDump>,
    pub rust: Option<StructureDump>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructureDifference {
    pub java_starts: u64,
    pub rust_starts: u64,
    pub matched_starts: u64,
    pub mismatches: Vec<StructureMismatch>,
}

impl StructureDifference {
    pub fn merge(&mut self, other: &Self) {
        self.java_starts += other.java_starts;
        self.rust_starts += other.rust_starts;
        self.matched_starts += other.matched_starts;
        self.mismatches.extend(other.mismatches.iter().cloned());
        self.mismatches.sort_by(|left, right| {
            left.start_chunk
                .cmp(&right.start_chunk)
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
        });
    }

    pub fn is_match(&self) -> bool {
        self.mismatches.is_empty()
            && self.java_starts == self.rust_starts
            && self.java_starts == self.matched_starts
    }
}

pub fn compare_structures(
    java: &[StructureDump],
    rust: &[StructureDump],
) -> BTreeMap<String, StructureDifference> {
    let mut java_groups: BTreeMap<(&str, [i32; 2]), Vec<&StructureDump>> = BTreeMap::new();
    let mut rust_groups: BTreeMap<(&str, [i32; 2]), Vec<&StructureDump>> = BTreeMap::new();
    for start in java {
        java_groups
            .entry(start.match_key())
            .or_default()
            .push(start);
    }
    for start in rust {
        rust_groups
            .entry(start.match_key())
            .or_default()
            .push(start);
    }
    let mut keys: BTreeSet<_> = java_groups.keys().copied().collect();
    keys.extend(rust_groups.keys().copied());
    let mut result: BTreeMap<String, StructureDifference> = BTreeMap::new();
    for key in keys {
        let mut java_items = java_groups.remove(&key).unwrap_or_default();
        let mut rust_items = rust_groups.remove(&key).unwrap_or_default();
        java_items.sort_by(|left, right| {
            left.bounds
                .cmp(&right.bounds)
                .then_with(|| left.sampled_blocks.cmp(&right.sampled_blocks))
        });
        rust_items.sort_by(|left, right| {
            left.bounds
                .cmp(&right.bounds)
                .then_with(|| left.sampled_blocks.cmp(&right.sampled_blocks))
        });
        let difference = result.entry(key.0.to_owned()).or_default();
        difference.java_starts += java_items.len() as u64;
        difference.rust_starts += rust_items.len() as u64;
        let common = java_items.len().min(rust_items.len());
        for index in 0..common {
            let java_start = java_items[index];
            let rust_start = rust_items[index];
            difference.matched_starts += 1;
            if java_start.bounds != rust_start.bounds {
                difference.mismatches.push(StructureMismatch {
                    kind: StructureMismatchKind::Bounds,
                    start_chunk: key.1,
                    java: Some((*java_start).clone()),
                    rust: Some((*rust_start).clone()),
                });
            }
            if (java_start.placed_pieces, java_start.skipped_pieces)
                != (rust_start.placed_pieces, rust_start.skipped_pieces)
            {
                difference.mismatches.push(StructureMismatch {
                    kind: StructureMismatchKind::PieceCounts,
                    start_chunk: key.1,
                    java: Some((*java_start).clone()),
                    rust: Some((*rust_start).clone()),
                });
            }
            if java_start.sampled_blocks != rust_start.sampled_blocks {
                difference.mismatches.push(StructureMismatch {
                    kind: StructureMismatchKind::SampledBlocks,
                    start_chunk: key.1,
                    java: Some((*java_start).clone()),
                    rust: Some((*rust_start).clone()),
                });
            }
        }
        for java_start in &java_items[common..] {
            difference.mismatches.push(StructureMismatch {
                kind: StructureMismatchKind::MissingInRust,
                start_chunk: key.1,
                java: Some((*java_start).clone()),
                rust: None,
            });
        }
        for rust_start in &rust_items[common..] {
            difference.mismatches.push(StructureMismatch {
                kind: StructureMismatchKind::UnexpectedInRust,
                start_chunk: key.1,
                java: None,
                rust: Some((*rust_start).clone()),
            });
        }
    }
    result
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParityReport {
    pub terrain: TerrainComparison,
    pub features: BTreeMap<String, FeatureDifference>,
    pub structures: BTreeMap<String, StructureDifference>,
    pub feature_oracle_available: bool,
    pub structure_oracle_available: bool,
    pub observed_features: BTreeMap<String, u64>,
    pub observed_structures: BTreeMap<String, u64>,
    pub missing_oracle_chunks: BTreeSet<ChunkKey>,
}

impl ParityReport {
    pub fn merge_terrain(&mut self, comparison: &TerrainComparison) {
        self.terrain.merge(comparison);
    }

    pub fn merge_features(&mut self, comparisons: &BTreeMap<String, FeatureDifference>) {
        self.feature_oracle_available = true;
        for (name, difference) in comparisons {
            self.features
                .entry(name.clone())
                .or_default()
                .merge(difference);
        }
    }

    pub fn merge_structures(&mut self, comparisons: &BTreeMap<String, StructureDifference>) {
        self.structure_oracle_available = true;
        for (name, difference) in comparisons {
            self.structures
                .entry(name.clone())
                .or_default()
                .merge(difference);
        }
    }

    pub fn observe_generation(&mut self, diagnostics: &GenerationDiagnostics) {
        for feature in &diagnostics.features {
            *self
                .observed_features
                .entry(feature.placed_feature.clone())
                .or_insert(0) += 1;
        }
        for structure in &diagnostics.structures {
            *self
                .observed_structures
                .entry(structure.structure_id.clone())
                .or_insert(0) += 1;
        }
    }

    pub fn has_named_mismatches(&self) -> bool {
        self.features
            .values()
            .any(|difference| !difference.is_match())
            || self
                .structures
                .values()
                .any(|difference| !difference.is_match())
    }
}

impl fmt::Display for TerrainComparison {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "compared blocks: {}", self.total)?;
        writeln!(
            formatter,
            "block-class agreement: {:.2}% ({}/{})",
            self.class_agreement() * 100.0,
            self.class_matches,
            self.total
        )?;
        writeln!(
            formatter,
            "exact-name agreement: {:.2}% ({}/{})",
            self.exact_agreement() * 100.0,
            self.exact_matches,
            self.total
        )?;
        writeln!(
            formatter,
            "water blocks: Rust={} Java={} mismatches={}",
            self.rust_water, self.java_water, self.water_mismatches
        )?;
        let mut confusions: Vec<_> = self
            .class_confusions
            .iter()
            .map(|(classes, count)| (*classes, *count))
            .collect();
        confusions.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        writeln!(formatter, "top class confusions (rust -> java):")?;
        for ((rust, java), count) in confusions.into_iter().take(8) {
            writeln!(
                formatter,
                "  {} -> {}: {count}",
                class_name(rust),
                class_name(java)
            )?;
        }
        let mut names: Vec<_> = self
            .name_confusions
            .iter()
            .map(|(names, count)| (names, *count))
            .collect();
        names.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
        writeln!(formatter, "top exact-name confusions (rust -> java):")?;
        for ((rust, java), count) in names.into_iter().take(8) {
            writeln!(formatter, "  {rust} -> {java}: {count}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ParityReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.terrain)?;
        if self.feature_oracle_available {
            writeln!(formatter, "feature oracle: available")?;
            for (name, difference) in &self.features {
                writeln!(
                    formatter,
                    "feature {name}: Java={} Rust={} matched={} mismatches={}",
                    difference.java_placements,
                    difference.rust_placements,
                    difference.matched_placements,
                    difference.mismatches.len()
                )?;
                for mismatch in &difference.mismatches {
                    writeln!(
                        formatter,
                        "  {} origin={:?}",
                        mismatch.kind.as_str(),
                        mismatch.origin
                    )?;
                }
            }
        } else {
            writeln!(
                formatter,
                "feature oracle: unavailable; observed Rust placements={} across {} features",
                self.observed_features.values().sum::<u64>(),
                self.observed_features.len()
            )?;
        }
        if self.structure_oracle_available {
            writeln!(formatter, "structure oracle: available")?;
            for (name, difference) in &self.structures {
                writeln!(
                    formatter,
                    "structure {name}: Java={} Rust={} matched={} mismatches={}",
                    difference.java_starts,
                    difference.rust_starts,
                    difference.matched_starts,
                    difference.mismatches.len()
                )?;
                for mismatch in &difference.mismatches {
                    writeln!(
                        formatter,
                        "  {} start_chunk={:?}",
                        mismatch.kind.as_str(),
                        mismatch.start_chunk
                    )?;
                }
            }
        } else {
            writeln!(
                formatter,
                "structure oracle: unavailable; observed Rust starts={} across {} structures",
                self.observed_structures.values().sum::<u64>(),
                self.observed_structures.len()
            )?;
        }
        if !self.missing_oracle_chunks.is_empty() {
            writeln!(formatter, "missing oracle chunks:")?;
            for key in &self.missing_oracle_chunks {
                writeln!(formatter, "  {key}")?;
            }
        }
        Ok(())
    }
}

pub fn block_class(name: &str) -> u8 {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => 0,
        "minecraft:water" => 2,
        "minecraft:lava" => 3,
        _ => 1,
    }
}

pub fn class_name(class: u8) -> &'static str {
    match class {
        0 => "air",
        2 => "water",
        3 => "lava",
        _ => "solid",
    }
}

pub fn position_value(pos: BlockPos) -> [i32; 3] {
    [pos.x(), pos.y(), pos.z()]
}
