//! Ports of `StructureSet`, `StructurePlacement`, and
//! `RandomSpreadStructurePlacement`.
//!
//! The shipped structure-set JSON is embedded as the same documents extracted
//! from the 1.21.11 data pack (the project has no separate structure-set asset
//! yet). Parsing is the same entry point a datapack loader would use:
//! [`StructureSetRegistry::from_json_documents`].

use crate::region::ChunkPos;
use folia_random::{LegacyRandomSource, RandomSource};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// `WorldgenRandom.setLargeFeatureWithSalt`.
pub(crate) fn set_large_feature_with_salt(
    random: &mut LegacyRandomSource,
    level_seed: i64,
    region_x: i32,
    region_z: i32,
    salt: i32,
) {
    let seed = i64::from(region_x)
        .wrapping_mul(341_873_128_712)
        .wrapping_add(i64::from(region_z).wrapping_mul(132_897_987_541))
        .wrapping_add(level_seed)
        .wrapping_add(i64::from(salt));
    random.set_seed(seed);
}

/// `WorldgenRandom.setLargeFeatureSeed`.
pub(crate) fn set_large_feature_seed(
    random: &mut LegacyRandomSource,
    base_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
) {
    random.set_seed(base_seed);
    let first = random.next_long();
    let second = random.next_long();
    let seed = i64::from(chunk_x).wrapping_mul(first)
        ^ i64::from(chunk_z).wrapping_mul(second)
        ^ base_seed;
    random.set_seed(seed);
}

/// `RandomSpreadType`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RandomSpreadType {
    #[default]
    Linear,
    Triangular,
}

impl RandomSpreadType {
    pub const ALL: [RandomSpreadType; 2] = [RandomSpreadType::Linear, RandomSpreadType::Triangular];

    pub fn name(self) -> &'static str {
        match self {
            RandomSpreadType::Linear => "linear",
            RandomSpreadType::Triangular => "triangular",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// `RandomSpreadType.evaluate(RandomSource, int)`.
    pub fn evaluate(self, random: &mut impl RandomSource, bound: i32) -> i32 {
        match self {
            RandomSpreadType::Linear => random.next_int_bound(bound),
            RandomSpreadType::Triangular => {
                (random.next_int_bound(bound) + random.next_int_bound(bound)) / 2
            }
        }
    }
}

/// `StructurePlacement.FrequencyReductionMethod`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FrequencyReductionMethod {
    #[default]
    Default,
    LegacyType1,
    LegacyType2,
    LegacyType3,
}

impl FrequencyReductionMethod {
    pub const ALL: [FrequencyReductionMethod; 4] = [
        FrequencyReductionMethod::Default,
        FrequencyReductionMethod::LegacyType1,
        FrequencyReductionMethod::LegacyType2,
        FrequencyReductionMethod::LegacyType3,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FrequencyReductionMethod::Default => "default",
            FrequencyReductionMethod::LegacyType1 => "legacy_type_1",
            FrequencyReductionMethod::LegacyType2 => "legacy_type_2",
            FrequencyReductionMethod::LegacyType3 => "legacy_type_3",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.name() == name)
    }

    /// `FrequencyReductionMethod.shouldGenerate`.
    pub fn should_generate(
        self,
        level_seed: i64,
        salt: i32,
        region_x: i32,
        region_z: i32,
        frequency: f32,
        salt_override: Option<i32>,
    ) -> bool {
        match self {
            FrequencyReductionMethod::Default => {
                let mut random = LegacyRandomSource::new(0);
                set_large_feature_with_salt(&mut random, level_seed, region_x, region_z, salt);
                random.next_float() < frequency
            }
            FrequencyReductionMethod::LegacyType1 => {
                let i = region_x >> 4;
                let j = region_z >> 4;
                let mut random = LegacyRandomSource::new(0);
                random.set_seed(i64::from(i ^ (j << 4)) ^ level_seed);
                random.next_int();
                random.next_int_bound((1.0_f32 / frequency) as i32) == 0
            }
            FrequencyReductionMethod::LegacyType2 => {
                let mut random = LegacyRandomSource::new(0);
                set_large_feature_with_salt(
                    &mut random,
                    level_seed,
                    region_x,
                    region_z,
                    salt_override.unwrap_or(10_387_320),
                );
                random.next_float() < frequency
            }
            FrequencyReductionMethod::LegacyType3 => {
                let mut random = LegacyRandomSource::new(0);
                match salt_override {
                    None => set_large_feature_seed(&mut random, level_seed, region_x, region_z),
                    Some(salt_override) => set_large_feature_with_salt(
                        &mut random,
                        level_seed,
                        region_x,
                        region_z,
                        salt_override,
                    ),
                }
                random.next_double() < f64::from(frequency)
            }
        }
    }
}

/// `StructurePlacement.ExclusionZone`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExclusionZone {
    /// Structure-set id (for example `minecraft:villages`).
    pub other_set: String,
    pub chunk_count: i32,
}

/// `RandomSpreadStructurePlacement`.
#[derive(Debug, Clone, PartialEq)]
pub struct RandomSpreadStructurePlacement {
    pub locate_offset: [i32; 3],
    pub frequency_reduction_method: FrequencyReductionMethod,
    pub frequency: f32,
    pub salt: i32,
    pub exclusion_zone: Option<ExclusionZone>,
    pub spacing: i32,
    pub separation: i32,
    pub spread_type: RandomSpreadType,
}

impl RandomSpreadStructurePlacement {
    pub fn new(spacing: i32, separation: i32, spread_type: RandomSpreadType, salt: i32) -> Self {
        Self {
            locate_offset: [0, 0, 0],
            frequency_reduction_method: FrequencyReductionMethod::Default,
            frequency: 1.0,
            salt,
            exclusion_zone: None,
            spacing,
            separation,
            spread_type,
        }
    }

    /// `RandomSpreadStructurePlacement.getPotentialStructureChunk`.
    pub fn get_potential_structure_chunk(
        &self,
        seed: i64,
        region_x: i32,
        region_z: i32,
    ) -> ChunkPos {
        let i = region_x.div_euclid(self.spacing);
        let i1 = region_z.div_euclid(self.spacing);
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_with_salt(&mut random, seed, i, i1, self.salt);
        let spread = self.spacing - self.separation;
        let x = self.spread_type.evaluate(&mut random, spread);
        let z = self.spread_type.evaluate(&mut random, spread);
        ChunkPos::new(
            i.wrapping_mul(self.spacing).wrapping_add(x),
            i1.wrapping_mul(self.spacing).wrapping_add(z),
        )
    }

    /// `RandomSpreadStructurePlacement.isPlacementChunk`.
    pub fn is_placement_chunk(&self, seed: i64, chunk_x: i32, chunk_z: i32) -> bool {
        let potential = self.get_potential_structure_chunk(seed, chunk_x, chunk_z);
        potential.x == chunk_x && potential.z == chunk_z
    }

    /// `StructurePlacement.applyAdditionalChunkRestrictions`.
    pub fn apply_additional_chunk_restrictions(
        &self,
        seed: i64,
        region_x: i32,
        region_z: i32,
        salt_override: Option<i32>,
    ) -> bool {
        if self.frequency >= 1.0 {
            return true;
        }
        self.frequency_reduction_method.should_generate(
            seed,
            self.salt,
            region_x,
            region_z,
            self.frequency,
            salt_override,
        )
    }

    /// `StructurePlacement.isStructureChunk` without the exclusion-zone check.
    /// [`StructureSetRegistry`] applies exclusion zones because they need the
    /// other set in the registry.
    pub fn is_structure_chunk(
        &self,
        seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        salt_override: Option<i32>,
    ) -> bool {
        self.is_placement_chunk(seed, chunk_x, chunk_z)
            && self.apply_additional_chunk_restrictions(seed, chunk_x, chunk_z, salt_override)
    }

    /// `StructurePlacement.getLocatePos`.
    pub fn get_locate_pos(&self, chunk: ChunkPos) -> [i32; 3] {
        [
            (chunk.x << 4).wrapping_add(self.locate_offset[0]),
            self.locate_offset[1],
            (chunk.z << 4).wrapping_add(self.locate_offset[2]),
        ]
    }
}

/// `ConcentricRingsStructurePlacement`.
///
/// Parsed faithfully, but [`StructurePlacement::is_structure_chunk`] returns
/// [`PlacementError::RequiresBiomeSource`]: selecting stronghold rings needs
/// the generator's biome source and seeded ring search, which is a later slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ConcentricRingsStructurePlacement {
    pub locate_offset: [i32; 3],
    pub frequency_reduction_method: FrequencyReductionMethod,
    pub frequency: f32,
    pub salt: i32,
    pub exclusion_zone: Option<ExclusionZone>,
    pub distance: i32,
    pub spread: i32,
    pub count: i32,
    pub preferred_biomes: Vec<String>,
}

/// `StructurePlacement` kind.
#[derive(Debug, Clone, PartialEq)]
pub enum StructurePlacement {
    RandomSpread(RandomSpreadStructurePlacement),
    ConcentricRings(ConcentricRingsStructurePlacement),
}

impl StructurePlacement {
    pub fn type_name(&self) -> &'static str {
        match self {
            StructurePlacement::RandomSpread(_) => "minecraft:random_spread",
            StructurePlacement::ConcentricRings(_) => "minecraft:concentric_rings",
        }
    }

    pub fn exclusion_zone(&self) -> Option<&ExclusionZone> {
        match self {
            StructurePlacement::RandomSpread(placement) => placement.exclusion_zone.as_ref(),
            StructurePlacement::ConcentricRings(placement) => placement.exclusion_zone.as_ref(),
        }
    }

    pub fn locate_offset(&self) -> [i32; 3] {
        match self {
            StructurePlacement::RandomSpread(placement) => placement.locate_offset,
            StructurePlacement::ConcentricRings(placement) => placement.locate_offset,
        }
    }

    pub fn get_locate_pos(&self, chunk: ChunkPos) -> [i32; 3] {
        let offset = self.locate_offset();
        [
            (chunk.x << 4).wrapping_add(offset[0]),
            offset[1],
            (chunk.z << 4).wrapping_add(offset[2]),
        ]
    }

    /// Java's `isStructureChunk` without exclusion-zone interactions.
    pub fn is_structure_chunk(
        &self,
        seed: i64,
        chunk_x: i32,
        chunk_z: i32,
        salt_override: Option<i32>,
    ) -> Result<bool, PlacementError> {
        match self {
            StructurePlacement::RandomSpread(placement) => {
                Ok(placement.is_structure_chunk(seed, chunk_x, chunk_z, salt_override))
            }
            StructurePlacement::ConcentricRings(_) => Err(PlacementError::RequiresBiomeSource {
                placement: "minecraft:concentric_rings",
            }),
        }
    }
}

/// Failure while evaluating a placement that this slice cannot answer alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementError {
    /// Concentric rings need the biome source/ring search.
    RequiresBiomeSource { placement: &'static str },
}

impl fmt::Display for PlacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlacementError::RequiresBiomeSource { placement } => write!(
                formatter,
                "placement `{placement}` requires the biome source/ring search"
            ),
        }
    }
}

impl std::error::Error for PlacementError {}

/// `StructureSet.StructureSelectionEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructureSelectionEntry {
    pub structure: String,
    pub weight: i32,
}

/// `StructureSet`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSet {
    pub name: String,
    pub structures: Vec<StructureSelectionEntry>,
    pub placement: StructurePlacement,
}

impl StructureSet {
    pub fn from_json(name: &str, json: &str) -> Result<Self, StructureSetError> {
        let value: Value = serde_json::from_str(json).map_err(|error| StructureSetError::Json {
            set: name.to_owned(),
            message: format!("invalid JSON: {error}"),
        })?;
        let root = value.as_object().ok_or_else(|| StructureSetError::Json {
            set: name.to_owned(),
            message: "root must be an object".to_owned(),
        })?;

        let structures_value = root
            .get("structures")
            .ok_or_else(|| StructureSetError::Json {
                set: name.to_owned(),
                message: "missing `structures`".to_owned(),
            })?;
        let structures_array =
            structures_value
                .as_array()
                .ok_or_else(|| StructureSetError::Json {
                    set: name.to_owned(),
                    message: "`structures` must be an array".to_owned(),
                })?;
        if structures_array.is_empty() {
            return Err(StructureSetError::Json {
                set: name.to_owned(),
                message: "`structures` must not be empty".to_owned(),
            });
        }

        let mut structures = Vec::with_capacity(structures_array.len());
        for (index, entry) in structures_array.iter().enumerate() {
            let entry = entry.as_object().ok_or_else(|| StructureSetError::Json {
                set: name.to_owned(),
                message: format!("structures[{index}] must be an object"),
            })?;
            let structure = required_str(entry, "structure", name, index)?;
            let weight = required_i32(entry, "weight", name, index)?;
            if weight <= 0 {
                return Err(StructureSetError::Json {
                    set: name.to_owned(),
                    message: format!("structures[{index}].weight must be positive, got {weight}"),
                });
            }
            structures.push(StructureSelectionEntry {
                structure: structure.to_owned(),
                weight,
            });
        }

        let placement_value = root
            .get("placement")
            .ok_or_else(|| StructureSetError::Json {
                set: name.to_owned(),
                message: "missing `placement`".to_owned(),
            })?;
        let placement = parse_placement(placement_value, name)?;

        Ok(Self {
            name: name.to_owned(),
            structures,
            placement,
        })
    }

    /// `ChunkGenerator.createStructures` weighted selection. A single-entry set
    /// consumes no random values.
    pub fn select_structure(
        &self,
        seed: i64,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Option<&StructureSelectionEntry> {
        if self.structures.len() == 1 {
            return self.structures.first();
        }

        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut random, seed, chunk_x, chunk_z);
        let total: i32 = self.structures.iter().map(|entry| entry.weight).sum();
        if total <= 0 {
            return None;
        }
        let mut draw = random.next_int_bound(total);
        for entry in &self.structures {
            draw -= entry.weight;
            if draw < 0 {
                return Some(entry);
            }
        }
        self.structures.last()
    }
}

/// `ChunkGenerator.createStructures` result for one chunk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectedStructure {
    pub set: String,
    pub structure: String,
    pub chunk: ChunkPos,
}

/// Spigot/Paper seed overrides that Java threads into the legacy reducers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StructureSetConfig {
    pub mineshaft_seed: Option<i32>,
    pub buried_treasure_seed: Option<i32>,
}

/// Parsed structure sets in deterministic (resource-name) order.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSetRegistry {
    sets: Vec<StructureSet>,
}

impl StructureSetRegistry {
    pub fn from_json_documents<'a>(
        documents: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, StructureSetError> {
        let mut sets = BTreeMap::new();
        for (name, json) in documents {
            let name = canonical_name(name);
            let set = StructureSet::from_json(&name, json)?;
            if sets.insert(name.clone(), set).is_some() {
                return Err(StructureSetError::Json {
                    set: name,
                    message: "duplicate structure-set name".to_owned(),
                });
            }
        }
        Ok(Self {
            sets: sets.into_values().collect(),
        })
    }

    /// Parses the 20 shipped 1.21.11 structure-set documents embedded in this
    /// module.
    pub fn shipped() -> Result<Self, StructureSetError> {
        Self::from_json_documents(SHIPPED_STRUCTURE_SETS.iter().copied())
    }

    pub fn sets(&self) -> &[StructureSet] {
        &self.sets
    }

    pub fn get(&self, name: &str) -> Option<&StructureSet> {
        let name = canonical_name(name);
        self.sets.iter().find(|set| set.name == name)
    }

    /// Evaluates one named set, including its exclusion zone. Concentric rings
    /// return a [`PlacementError::RequiresBiomeSource`] wrapped in
    /// [`StructureSetError::Placement`].
    pub fn is_structure_chunk(
        &self,
        set_name: &str,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
    ) -> Result<bool, StructureSetError> {
        self.is_structure_chunk_with_config(
            set_name,
            chunk_x,
            chunk_z,
            seed,
            StructureSetConfig::default(),
        )
    }

    pub fn is_structure_chunk_with_config(
        &self,
        set_name: &str,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
        config: StructureSetConfig,
    ) -> Result<bool, StructureSetError> {
        let mut stack = Vec::new();
        self.is_structure_chunk_named(set_name, chunk_x, chunk_z, seed, config, &mut stack)
    }

    /// Placement types that parse but cannot be evaluated without more world
    /// state. `structure_at_chunk` skips them.
    pub fn unsupported_placements(&self) -> Vec<(&str, &'static str)> {
        self.sets
            .iter()
            .filter(|set| matches!(set.placement, StructurePlacement::ConcentricRings(_)))
            .map(|set| (set.name.as_str(), set.placement.type_name()))
            .collect()
    }

    /// `ChunkGenerator.createStructures` placement test plus weighted
    /// structure selection, without generating a start.
    pub fn structure_at_chunk(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
    ) -> Result<Option<SelectedStructure>, StructureSetError> {
        self.structure_at_chunk_with_config(chunk_x, chunk_z, seed, StructureSetConfig::default())
    }

    pub fn structure_at_chunk_with_config(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
        config: StructureSetConfig,
    ) -> Result<Option<SelectedStructure>, StructureSetError> {
        for set in &self.sets {
            let mut stack = Vec::new();
            match self
                .is_structure_chunk_named(&set.name, chunk_x, chunk_z, seed, config, &mut stack)
            {
                Ok(true) => {
                    let Some(selection) = set.select_structure(seed, chunk_x, chunk_z) else {
                        continue;
                    };
                    return Ok(Some(SelectedStructure {
                        set: set.name.clone(),
                        structure: selection.structure.clone(),
                        chunk: ChunkPos::new(chunk_x, chunk_z),
                    }));
                }
                Ok(false) => {}
                Err(StructureSetError::Placement {
                    source: PlacementError::RequiresBiomeSource { .. },
                    ..
                }) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn is_structure_chunk_named(
        &self,
        name: &str,
        chunk_x: i32,
        chunk_z: i32,
        seed: i64,
        config: StructureSetConfig,
        stack: &mut Vec<String>,
    ) -> Result<bool, StructureSetError> {
        let name = canonical_name(name);
        if stack.contains(&name) {
            return Err(StructureSetError::ExclusionCycle { set: name });
        }
        let set = self
            .get(&name)
            .ok_or_else(|| StructureSetError::MissingSet {
                referenced: name.clone(),
                from: stack.last().cloned().unwrap_or_default(),
            })?;
        let placement = set.placement.clone();
        let salt_override = match name.as_str() {
            "minecraft:mineshafts" => config.mineshaft_seed,
            "minecraft:buried_treasures" => config.buried_treasure_seed,
            _ => None,
        };
        let placement_chunk = placement
            .is_structure_chunk(seed, chunk_x, chunk_z, salt_override)
            .map_err(|source| StructureSetError::Placement {
                set: name.clone(),
                source,
            })?;
        if !placement_chunk {
            return Ok(false);
        }

        let Some(zone) = placement.exclusion_zone().cloned() else {
            return Ok(true);
        };

        stack.push(name);
        for x in (chunk_x - zone.chunk_count)..=(chunk_x + zone.chunk_count) {
            for z in (chunk_z - zone.chunk_count)..=(chunk_z + zone.chunk_count) {
                let forbidden =
                    self.is_structure_chunk_named(&zone.other_set, x, z, seed, config, stack);
                match forbidden {
                    Ok(true) => {
                        stack.pop();
                        return Ok(false);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        stack.pop();
                        return Err(error);
                    }
                }
            }
        }
        stack.pop();
        Ok(true)
    }
}

fn canonical_name(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

/// Errors from structure-set parsing or placement evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureSetError {
    Json { set: String, message: String },
    Placement { set: String, source: PlacementError },
    MissingSet { referenced: String, from: String },
    ExclusionCycle { set: String },
}

impl fmt::Display for StructureSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StructureSetError::Json { set, message } => {
                write!(formatter, "structure set `{set}`: {message}")
            }
            StructureSetError::Placement { set, source } => {
                write!(formatter, "structure set `{set}`: {source}")
            }
            StructureSetError::MissingSet { referenced, from } => {
                if from.is_empty() {
                    write!(formatter, "missing structure set `{referenced}`")
                } else {
                    write!(
                        formatter,
                        "structure set `{from}` references missing set `{referenced}`"
                    )
                }
            }
            StructureSetError::ExclusionCycle { set } => {
                write!(formatter, "structure-set exclusion cycle through `{set}`")
            }
        }
    }
}

impl std::error::Error for StructureSetError {}

fn parse_placement(value: &Value, set: &str) -> Result<StructurePlacement, StructureSetError> {
    let object = value.as_object().ok_or_else(|| StructureSetError::Json {
        set: set.to_owned(),
        message: "`placement` must be an object".to_owned(),
    })?;
    let type_name = required_str(object, "type", set, 0)?;
    let common = parse_common_placement(object, set)?;
    match type_name {
        "minecraft:random_spread" => {
            let spacing = required_i32(object, "spacing", set, 0)?;
            let separation = required_i32(object, "separation", set, 0)?;
            if !(0..=4096).contains(&spacing) {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("spacing must be in 0..=4096, got {spacing}"),
                });
            }
            if !(0..=4096).contains(&separation) {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("separation must be in 0..=4096, got {separation}"),
                });
            }
            if spacing <= separation {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!(
                        "spacing must be larger than separation, got {spacing} <= {separation}"
                    ),
                });
            }
            let spread_type = match object.get("spread_type").and_then(Value::as_str) {
                None => RandomSpreadType::Linear,
                Some(name) => {
                    RandomSpreadType::from_name(name).ok_or_else(|| StructureSetError::Json {
                        set: set.to_owned(),
                        message: format!("unknown spread_type `{name}`"),
                    })?
                }
            };
            Ok(StructurePlacement::RandomSpread(
                RandomSpreadStructurePlacement {
                    locate_offset: common.locate_offset,
                    frequency_reduction_method: common.frequency_reduction_method,
                    frequency: common.frequency,
                    salt: common.salt,
                    exclusion_zone: common.exclusion_zone,
                    spacing,
                    separation,
                    spread_type,
                },
            ))
        }
        "minecraft:concentric_rings" => {
            let distance = required_i32(object, "distance", set, 0)?;
            let spread = required_i32(object, "spread", set, 0)?;
            let count = required_i32(object, "count", set, 0)?;
            if !(0..=1023).contains(&distance) || !(0..=1023).contains(&spread) {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: "distance and spread must be in 0..=1023".to_owned(),
                });
            }
            if !(1..=4095).contains(&count) {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("count must be in 1..=4095, got {count}"),
                });
            }
            let preferred =
                object
                    .get("preferred_biomes")
                    .ok_or_else(|| StructureSetError::Json {
                        set: set.to_owned(),
                        message: "missing `preferred_biomes`".to_owned(),
                    })?;
            let preferred_biomes = match preferred {
                Value::String(tag) => vec![tag.clone()],
                Value::Array(values) => values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| StructureSetError::Json {
                                set: set.to_owned(),
                                message: "`preferred_biomes` entries must be strings".to_owned(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => {
                    return Err(StructureSetError::Json {
                        set: set.to_owned(),
                        message: "`preferred_biomes` must be a tag string or array".to_owned(),
                    });
                }
            };
            Ok(StructurePlacement::ConcentricRings(
                ConcentricRingsStructurePlacement {
                    locate_offset: common.locate_offset,
                    frequency_reduction_method: common.frequency_reduction_method,
                    frequency: common.frequency,
                    salt: common.salt,
                    exclusion_zone: common.exclusion_zone,
                    distance,
                    spread,
                    count,
                    preferred_biomes,
                },
            ))
        }
        other => Err(StructureSetError::Json {
            set: set.to_owned(),
            message: format!("unknown placement type `{other}`"),
        }),
    }
}

struct CommonPlacement {
    locate_offset: [i32; 3],
    frequency_reduction_method: FrequencyReductionMethod,
    frequency: f32,
    salt: i32,
    exclusion_zone: Option<ExclusionZone>,
}

fn parse_common_placement(
    object: &Map<String, Value>,
    set: &str,
) -> Result<CommonPlacement, StructureSetError> {
    let locate_offset = match object.get("locate_offset") {
        None => [0, 0, 0],
        Some(value) => {
            let array = value.as_array().ok_or_else(|| StructureSetError::Json {
                set: set.to_owned(),
                message: "`locate_offset` must be an array of 3 ints".to_owned(),
            })?;
            if array.len() != 3 {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("`locate_offset` must contain 3 ints, got {}", array.len()),
                });
            }
            let mut result = [0; 3];
            for (index, value) in array.iter().enumerate() {
                let value = value.as_i64().ok_or_else(|| StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("`locate_offset[{index}]` must be an int"),
                })?;
                result[index] = i32::try_from(value).map_err(|_| StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!("`locate_offset[{index}]` is outside the int range"),
                })?;
            }
            result
        }
    };

    let frequency_reduction_method = match object
        .get("frequency_reduction_method")
        .and_then(Value::as_str)
    {
        None => FrequencyReductionMethod::Default,
        Some(name) => {
            FrequencyReductionMethod::from_name(name).ok_or_else(|| StructureSetError::Json {
                set: set.to_owned(),
                message: format!("unknown frequency_reduction_method `{name}`"),
            })?
        }
    };
    let frequency = match object.get("frequency") {
        None => 1.0,
        Some(value) => value.as_f64().ok_or_else(|| StructureSetError::Json {
            set: set.to_owned(),
            message: "`frequency` must be a number".to_owned(),
        })? as f32,
    };
    if !(0.0..=1.0).contains(&frequency) {
        return Err(StructureSetError::Json {
            set: set.to_owned(),
            message: format!("frequency must be in 0..=1, got {frequency}"),
        });
    }
    let salt = required_i32(object, "salt", set, 0)?;
    if salt < 0 {
        return Err(StructureSetError::Json {
            set: set.to_owned(),
            message: format!("salt must be non-negative, got {salt}"),
        });
    }

    let exclusion_zone = match object.get("exclusion_zone") {
        None => None,
        Some(value) => {
            let zone = value.as_object().ok_or_else(|| StructureSetError::Json {
                set: set.to_owned(),
                message: "`exclusion_zone` must be an object".to_owned(),
            })?;
            let other_set = required_str(zone, "other_set", set, 0)?.to_owned();
            let chunk_count = required_i32(zone, "chunk_count", set, 0)?;
            if !(1..=16).contains(&chunk_count) {
                return Err(StructureSetError::Json {
                    set: set.to_owned(),
                    message: format!(
                        "exclusion_zone.chunk_count must be in 1..=16, got {chunk_count}"
                    ),
                });
            }
            Some(ExclusionZone {
                other_set,
                chunk_count,
            })
        }
    };

    Ok(CommonPlacement {
        locate_offset,
        frequency_reduction_method,
        frequency,
        salt,
        exclusion_zone,
    })
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    set: &str,
    index: usize,
) -> Result<&'a str, StructureSetError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| StructureSetError::Json {
            set: set.to_owned(),
            message: format!("entry {index} is missing string `{key}`"),
        })
}

fn required_i32(
    object: &Map<String, Value>,
    key: &str,
    set: &str,
    index: usize,
) -> Result<i32, StructureSetError> {
    let value = object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| StructureSetError::Json {
            set: set.to_owned(),
            message: format!("entry {index} is missing integer `{key}`"),
        })?;
    i32::try_from(value).map_err(|_| StructureSetError::Json {
        set: set.to_owned(),
        message: format!("entry {index} `{key}` is outside the int range"),
    })
}

/// The shipped `data/minecraft/worldgen/structure_set` documents with
/// insignificant JSON whitespace normalized. Source: decompiled Folia 1.21.11
/// data pack. They are embedded rather than added as a new asset file so this
/// slice stays inside its file budget.
const SHIPPED_STRUCTURE_SETS: &[(&str, &str)] = &[
    (
        "ancient_cities",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 20083232, "separation": 8, "spacing": 24 }, "structures": [ { "structure": "minecraft:ancient_city", "weight": 1 } ] }"##,
    ),
    (
        "buried_treasures",
        r##"{ "placement": { "type": "minecraft:random_spread", "frequency": 0.01, "frequency_reduction_method": "legacy_type_2", "locate_offset": [ 9, 0, 9 ], "salt": 0, "separation": 0, "spacing": 1 }, "structures": [ { "structure": "minecraft:buried_treasure", "weight": 1 } ] }"##,
    ),
    (
        "desert_pyramids",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357617, "separation": 8, "spacing": 32 }, "structures": [ { "structure": "minecraft:desert_pyramid", "weight": 1 } ] }"##,
    ),
    (
        "end_cities",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 10387313, "separation": 11, "spacing": 20, "spread_type": "triangular" }, "structures": [ { "structure": "minecraft:end_city", "weight": 1 } ] }"##,
    ),
    (
        "igloos",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357618, "separation": 8, "spacing": 32 }, "structures": [ { "structure": "minecraft:igloo", "weight": 1 } ] }"##,
    ),
    (
        "jungle_temples",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357619, "separation": 8, "spacing": 32 }, "structures": [ { "structure": "minecraft:jungle_pyramid", "weight": 1 } ] }"##,
    ),
    (
        "mineshafts",
        r##"{ "placement": { "type": "minecraft:random_spread", "frequency": 0.004, "frequency_reduction_method": "legacy_type_3", "salt": 0, "separation": 0, "spacing": 1 }, "structures": [ { "structure": "minecraft:mineshaft", "weight": 1 }, { "structure": "minecraft:mineshaft_mesa", "weight": 1 } ] }"##,
    ),
    (
        "nether_complexes",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 30084232, "separation": 4, "spacing": 27 }, "structures": [ { "structure": "minecraft:fortress", "weight": 2 }, { "structure": "minecraft:bastion_remnant", "weight": 3 } ] }"##,
    ),
    (
        "nether_fossils",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357921, "separation": 1, "spacing": 2 }, "structures": [ { "structure": "minecraft:nether_fossil", "weight": 1 } ] }"##,
    ),
    (
        "ocean_monuments",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 10387313, "separation": 5, "spacing": 32, "spread_type": "triangular" }, "structures": [ { "structure": "minecraft:monument", "weight": 1 } ] }"##,
    ),
    (
        "ocean_ruins",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357621, "separation": 8, "spacing": 20 }, "structures": [ { "structure": "minecraft:ocean_ruin_cold", "weight": 1 }, { "structure": "minecraft:ocean_ruin_warm", "weight": 1 } ] }"##,
    ),
    (
        "pillager_outposts",
        r##"{ "placement": { "type": "minecraft:random_spread", "exclusion_zone": { "chunk_count": 10, "other_set": "minecraft:villages" }, "frequency": 0.2, "frequency_reduction_method": "legacy_type_1", "salt": 165745296, "separation": 8, "spacing": 32 }, "structures": [ { "structure": "minecraft:pillager_outpost", "weight": 1 } ] }"##,
    ),
    (
        "ruined_portals",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 34222645, "separation": 15, "spacing": 40 }, "structures": [ { "structure": "minecraft:ruined_portal", "weight": 1 }, { "structure": "minecraft:ruined_portal_desert", "weight": 1 }, { "structure": "minecraft:ruined_portal_jungle", "weight": 1 }, { "structure": "minecraft:ruined_portal_swamp", "weight": 1 }, { "structure": "minecraft:ruined_portal_mountain", "weight": 1 }, { "structure": "minecraft:ruined_portal_ocean", "weight": 1 }, { "structure": "minecraft:ruined_portal_nether", "weight": 1 } ] }"##,
    ),
    (
        "shipwrecks",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 165745295, "separation": 4, "spacing": 24 }, "structures": [ { "structure": "minecraft:shipwreck", "weight": 1 }, { "structure": "minecraft:shipwreck_beached", "weight": 1 } ] }"##,
    ),
    (
        "strongholds",
        r##"{ "placement": { "type": "minecraft:concentric_rings", "count": 128, "distance": 32, "preferred_biomes": "#minecraft:stronghold_biased_to", "salt": 0, "spread": 3 }, "structures": [ { "structure": "minecraft:stronghold", "weight": 1 } ] }"##,
    ),
    (
        "swamp_huts",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 14357620, "separation": 8, "spacing": 32 }, "structures": [ { "structure": "minecraft:swamp_hut", "weight": 1 } ] }"##,
    ),
    (
        "trail_ruins",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 83469867, "separation": 8, "spacing": 34 }, "structures": [ { "structure": "minecraft:trail_ruins", "weight": 1 } ] }"##,
    ),
    (
        "trial_chambers",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 94251327, "separation": 12, "spacing": 34 }, "structures": [ { "structure": "minecraft:trial_chambers", "weight": 1 } ] }"##,
    ),
    (
        "villages",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 10387312, "separation": 8, "spacing": 34 }, "structures": [ { "structure": "minecraft:village_plains", "weight": 1 }, { "structure": "minecraft:village_desert", "weight": 1 }, { "structure": "minecraft:village_savanna", "weight": 1 }, { "structure": "minecraft:village_snowy", "weight": 1 }, { "structure": "minecraft:village_taiga", "weight": 1 } ] }"##,
    ),
    (
        "woodland_mansions",
        r##"{ "placement": { "type": "minecraft:random_spread", "salt": 10387319, "separation": 20, "spacing": 80, "spread_type": "triangular" }, "structures": [ { "structure": "minecraft:mansion", "weight": 1 } ] }"##,
    ),
];
