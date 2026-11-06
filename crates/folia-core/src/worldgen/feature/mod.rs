//! Vanilla feature framework.
//!
//! Ports the shape of
//! `net/minecraft/world/level/levelgen/feature/{Feature,ConfiguredFeature}.java`
//! and the generation-step ordering from `GenerationStep.Decoration`.
//!
//! The `placement` module ports the placement-modifier pipeline
//! (`PlacedFeature.placeWithContext`): modifiers are typed and executed in
//! vanilla order. Only the `biome` filter needs world state, which it reads
//! through [`PlacementWorld`]; [`BiomePlacementWorld`] implements that from the
//! generator's biome source. The `vegetation` module adds the ground-cover
//! feature batch and its state providers. Configured features whose vanilla
//! type has no Rust port yet load as [`ConfiguredFeatureKind::Unsupported`] and
//! fail loudly with [`UnsupportedFeature`] instead of silently doing nothing;
//! placement modifiers that read the world do the same through
//! [`UnsupportedModifier`].

use crate::chunk::{BlockState, Chunk};
use crate::worldgen::biome::BiomeSampler;
use crate::worldgen::biome_zoom::BiomeZoomer;
use folia_math::BlockPos;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io;
use std::sync::OnceLock;

mod ore;
mod placement;
mod tree;
mod vegetation;

use vegetation::{
    BlockColumnFeature, BlockPileFeature, MultifaceGrowthFeature, RandomPatchFeature,
    SimpleBlockFeature, VegetationPatchFeature,
};

pub use placement::{
    BlockPredicate, HeightProvider, Heightmap, IntProvider, PlacementFailure, PlacementModifier,
    PlacementModifierKind, PlacementWorld, UnsupportedModifier, VerticalAnchor, WeightedEntry,
};
pub use tree::{FallenTreeFeature, TreeFeature};

const FEATURE_DATA: &str = include_str!("../../../assets/feature_data.json");

/// Number of `GenerationStep.Decoration` values. `BiomeGenerationSettings`
/// stores one placed-feature list per step in this order.
pub const DECORATION_STEPS: usize = 11;

/// `GenerationStep.Decoration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Decoration {
    RawGeneration,
    Lakes,
    LocalModifications,
    UndergroundStructures,
    SurfaceStructures,
    Strongholds,
    UndergroundOres,
    UndergroundDecoration,
    FluidSprings,
    VegetalDecoration,
    TopLayerModification,
}

impl Decoration {
    /// All steps in vanilla execution order.
    pub const ALL: [Decoration; DECORATION_STEPS] = [
        Decoration::RawGeneration,
        Decoration::Lakes,
        Decoration::LocalModifications,
        Decoration::UndergroundStructures,
        Decoration::SurfaceStructures,
        Decoration::Strongholds,
        Decoration::UndergroundOres,
        Decoration::UndergroundDecoration,
        Decoration::FluidSprings,
        Decoration::VegetalDecoration,
        Decoration::TopLayerModification,
    ];

    /// Index into a biome's per-step feature lists.
    pub fn index(self) -> usize {
        match self {
            Decoration::RawGeneration => 0,
            Decoration::Lakes => 1,
            Decoration::LocalModifications => 2,
            Decoration::UndergroundStructures => 3,
            Decoration::SurfaceStructures => 4,
            Decoration::Strongholds => 5,
            Decoration::UndergroundOres => 6,
            Decoration::UndergroundDecoration => 7,
            Decoration::FluidSprings => 8,
            Decoration::VegetalDecoration => 9,
            Decoration::TopLayerModification => 10,
        }
    }

    /// `Decoration.getSerializedName()`.
    pub fn name(self) -> &'static str {
        match self {
            Decoration::RawGeneration => "raw_generation",
            Decoration::Lakes => "lakes",
            Decoration::LocalModifications => "local_modifications",
            Decoration::UndergroundStructures => "underground_structures",
            Decoration::SurfaceStructures => "surface_structures",
            Decoration::Strongholds => "strongholds",
            Decoration::UndergroundOres => "underground_ores",
            Decoration::UndergroundDecoration => "underground_decoration",
            Decoration::FluidSprings => "fluid_springs",
            Decoration::VegetalDecoration => "vegetal_decoration",
            Decoration::TopLayerModification => "top_layer_modification",
        }
    }

    /// Parses `getSerializedName()` back into a step.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|decoration| decoration.name() == name)
    }
}

/// The subset of `WorldGenLevel` that features see here.
///
/// The real class can read neighbouring chunks; the current region is limited
/// to the chunk being generated, and out-of-region writes are rejected the same
/// way vanilla rejects writes outside its `WorldGenRegion` radius.
pub trait FeatureLevel {
    fn min_y(&self) -> i32;
    fn height(&self) -> i32;
    fn get_block(&self, pos: BlockPos) -> Option<&BlockState>;
    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool;
}

/// Chunk-bounded `WorldGenLevel` passed to features.
#[derive(Debug)]
pub struct WorldGenRegion<'a> {
    chunk: &'a mut Chunk,
    min_y: i32,
    height: i32,
}

impl<'a> WorldGenRegion<'a> {
    pub fn new(chunk: &'a mut Chunk, min_y: i32, height: i32) -> Self {
        WorldGenRegion {
            chunk,
            min_y,
            height,
        }
    }

    /// Maps absolute coordinates to chunk-local coordinates, returning `None`
    /// outside the chunk or the vertical range.
    fn local(&self, pos: BlockPos) -> Option<(usize, i32, usize)> {
        let max_y = self.min_y.checked_add(self.height)?;
        if pos.y() < self.min_y || pos.y() >= max_y {
            return None;
        }
        let local_x = i64::from(pos.x()) - i64::from(self.chunk.x) * 16;
        let local_z = i64::from(pos.z()) - i64::from(self.chunk.z) * 16;
        if !(0..16).contains(&local_x) || !(0..16).contains(&local_z) {
            return None;
        }
        Some((local_x as usize, pos.y(), local_z as usize))
    }
}

impl FeatureLevel for WorldGenRegion<'_> {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        let (x, y, z) = self.local(pos)?;
        self.chunk.block_state(x, y, z)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        let Some((x, y, z)) = self.local(pos) else {
            return false;
        };
        self.chunk.set_block_state(x, y, z, state);
        true
    }
}

/// `FeaturePlaceContext`: origin, RNG and write region for one placement.
pub struct FeaturePlaceContext<'a> {
    pub origin: BlockPos,
    pub random: &'a mut dyn RandomSource,
    pub level: &'a mut dyn FeatureLevel,
}

impl FeaturePlaceContext<'_> {
    /// `LevelSimulatedReader.isStateAtPosition` for air.
    pub fn is_air(&self, pos: BlockPos) -> bool {
        self.level
            .get_block(pos)
            .is_none_or(|state| is_air_name(&state.name))
    }
}

/// `BlockBehaviour.BlockStateBase.isAir`.
pub fn is_air_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `Feature`: a configured feature type that writes into a
/// [`FeaturePlaceContext`] and reports whether it placed anything.
pub trait Feature {
    /// Registry name, e.g. `minecraft:no_op`.
    fn name(&self) -> &'static str;
    fn place(&self, context: &mut FeaturePlaceContext<'_>) -> bool;
}

/// `minecraft:no_op`: succeeds without touching the world.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoOpFeature;

impl Feature for NoOpFeature {
    fn name(&self) -> &'static str {
        "minecraft:no_op"
    }

    fn place(&self, _context: &mut FeaturePlaceContext<'_>) -> bool {
        true
    }
}

/// A configured feature that has no Rust implementation yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedFeature {
    pub feature: String,
    pub vanilla_type: String,
}

impl fmt::Display for UnsupportedFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "configured feature {} uses {}, which is not ported yet",
            self.feature, self.vanilla_type
        )
    }
}

impl std::error::Error for UnsupportedFeature {}

/// `ConfiguredFeature`: a feature type plus its configuration.
#[derive(Debug, Clone)]
pub struct ConfiguredFeature {
    id: String,
    kind: ConfiguredFeatureKind,
}

#[derive(Debug, Clone)]
pub enum ConfiguredFeatureKind {
    NoOp(NoOpFeature),
    /// `minecraft:tree` with the supported straight-trunk/blob-foliage placers.
    Tree(Box<TreeFeature>),
    /// `minecraft:fallen_tree`.
    FallenTree(Box<FallenTreeFeature>),
    /// `minecraft:random_selector`.
    RandomSelector(RandomSelectorFeature),
    /// `minecraft:ore`.
    Ore(Box<ore::OreFeature>),
    /// `minecraft:scattered_ore`.
    ScatteredOre(Box<ore::ScatteredOreFeature>),
    /// `minecraft:random_patch` and `minecraft:flower`.
    RandomPatch(Box<RandomPatchFeature>),
    /// `minecraft:simple_block`.
    SimpleBlock(Box<SimpleBlockFeature>),
    /// `minecraft:block_pile`.
    BlockPile(Box<BlockPileFeature>),
    /// `minecraft:block_column`.
    BlockColumn(Box<BlockColumnFeature>),
    /// `minecraft:multiface_growth`.
    MultifaceGrowth(Box<MultifaceGrowthFeature>),
    /// `minecraft:vegetation_patch` and `minecraft:waterlogged_vegetation_patch`.
    VegetationPatch(Box<VegetationPatchFeature>),
    /// Parsed and validated, but the vanilla feature type has no port yet.
    Unsupported {
        vanilla_type: String,
    },
}

/// `RandomSelectorFeature`: try each weighted branch, then the default.
///
/// Branches whose configured feature has no port are kept in the list so the
/// RNG draw order still matches vanilla; if one is chosen it places nothing.
#[derive(Debug, Clone)]
pub struct RandomSelectorFeature {
    default: Box<ConfiguredFeature>,
    features: Vec<(f32, Option<Box<ConfiguredFeature>>)>,
}

impl RandomSelectorFeature {
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        for (chance, feature) in &self.features {
            if random.next_float() < *chance {
                return match feature {
                    Some(feature) => feature.place(level, random, origin),
                    None => Ok(false),
                };
            }
        }
        self.default.place(level, random, origin)
    }
}

impl ConfiguredFeature {
    /// Namespaced configured-feature id, e.g. `minecraft:ore_dirt`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Vanilla feature type, e.g. `minecraft:ore`.
    pub fn vanilla_type(&self) -> &str {
        match &self.kind {
            ConfiguredFeatureKind::NoOp(feature) => feature.name(),
            ConfiguredFeatureKind::Tree(_) => "minecraft:tree",
            ConfiguredFeatureKind::FallenTree(_) => "minecraft:fallen_tree",
            ConfiguredFeatureKind::RandomSelector(_) => "minecraft:random_selector",
            ConfiguredFeatureKind::Ore(_) => "minecraft:ore",
            ConfiguredFeatureKind::ScatteredOre(_) => "minecraft:scattered_ore",
            ConfiguredFeatureKind::RandomPatch(feature) => feature.vanilla_type(),
            ConfiguredFeatureKind::SimpleBlock(_) => "minecraft:simple_block",
            ConfiguredFeatureKind::BlockPile(_) => "minecraft:block_pile",
            ConfiguredFeatureKind::BlockColumn(_) => "minecraft:block_column",
            ConfiguredFeatureKind::MultifaceGrowth(_) => "minecraft:multiface_growth",
            ConfiguredFeatureKind::VegetationPatch(feature) => feature.vanilla_type(),
            ConfiguredFeatureKind::Unsupported { vanilla_type } => vanilla_type,
        }
    }

    /// Whether [`Self::place`] can execute this feature.
    pub fn is_supported(&self) -> bool {
        matches!(
            self.kind,
            ConfiguredFeatureKind::NoOp(_)
                | ConfiguredFeatureKind::Tree(_)
                | ConfiguredFeatureKind::FallenTree(_)
                | ConfiguredFeatureKind::RandomSelector(_)
                | ConfiguredFeatureKind::Ore(_)
                | ConfiguredFeatureKind::ScatteredOre(_)
                | ConfiguredFeatureKind::RandomPatch(_)
                | ConfiguredFeatureKind::SimpleBlock(_)
                | ConfiguredFeatureKind::BlockPile(_)
                | ConfiguredFeatureKind::BlockColumn(_)
                | ConfiguredFeatureKind::MultifaceGrowth(_)
                | ConfiguredFeatureKind::VegetationPatch(_)
        )
    }

    /// `ConfiguredFeature.place`.
    ///
    /// Returns `Err` for an unported vanilla type; it never silently returns
    /// `false`, which would look like "feature placed nothing".
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, UnsupportedFeature> {
        match &self.kind {
            ConfiguredFeatureKind::NoOp(feature) => {
                let mut context = FeaturePlaceContext {
                    origin,
                    random,
                    level,
                };
                Ok(feature.place(&mut context))
            }
            ConfiguredFeatureKind::Tree(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::FallenTree(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::RandomSelector(selector) => {
                selector.place(level, random, origin)
            }
            ConfiguredFeatureKind::Ore(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::ScatteredOre(feature) => {
                Ok(feature.place(level, random, origin))
            }
            ConfiguredFeatureKind::RandomPatch(feature) => feature.place(level, random, origin),
            ConfiguredFeatureKind::SimpleBlock(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::BlockPile(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::BlockColumn(feature) => Ok(feature.place(level, random, origin)),
            ConfiguredFeatureKind::MultifaceGrowth(feature) => {
                Ok(feature.place(level, random, origin))
            }
            ConfiguredFeatureKind::VegetationPatch(feature) => feature.place(level, random, origin),
            ConfiguredFeatureKind::Unsupported { vanilla_type } => Err(UnsupportedFeature {
                feature: self.id.clone(),
                vanilla_type: vanilla_type.clone(),
            }),
        }
    }
}

/// `PlacedFeature`: a configured feature plus its placement modifiers.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedFeature {
    id: String,
    feature: String,
    modifiers: Vec<PlacementModifier>,
}

impl PlacedFeature {
    /// Namespaced placed-feature id, e.g. `minecraft:ore_dirt`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Namespaced configured-feature id this placement refers to.
    pub fn feature(&self) -> &str {
        &self.feature
    }

    /// `PlacedFeature.placement`: modifiers in vanilla order.
    pub fn placement_modifiers(&self) -> &[PlacementModifier] {
        &self.modifiers
    }

    /// Number of `PlacementModifier` entries.
    pub fn placement_modifier_count(&self) -> usize {
        self.modifiers.len()
    }

    /// Whether every placement modifier has a Rust port.
    pub fn placement_is_supported(&self) -> bool {
        self.modifiers.iter().all(PlacementModifier::is_supported)
    }

    /// `PlacedFeature.placeWithContext`: the positions this placement tries
    /// for one origin, in vanilla RNG order.
    ///
    /// Runs every modifier left to right, so each modifier sees the positions
    /// produced by the previous one. Returns [`PlacementFailure`] when a
    /// modifier has no port; it never quietly drops positions.
    pub fn placement_positions(
        &self,
        origin: BlockPos,
        random: &mut dyn RandomSource,
        world: &dyn PlacementWorld,
        min_y: i32,
        height: i32,
    ) -> Result<Vec<BlockPos>, PlacementFailure> {
        self.apply_modifiers(0, origin, random, world, min_y, height)
    }

    /// Applies modifiers depth-first, like Java's chained `Stream.flatMap`.
    ///
    /// The distinction matters for RNG: after `count` produces several copies,
    /// vanilla runs every later modifier for the first copy before drawing the
    /// second copy's `in_square` coordinates. Applying one modifier to the
    /// whole list at a time would draw all x/z values first and desynchronise
    /// the height samples.
    fn apply_modifiers(
        &self,
        index: usize,
        pos: BlockPos,
        random: &mut dyn RandomSource,
        world: &dyn PlacementWorld,
        min_y: i32,
        height: i32,
    ) -> Result<Vec<BlockPos>, PlacementFailure> {
        let Some(modifier) = self.modifiers.get(index) else {
            return Ok(vec![pos]);
        };
        let mut positions = Vec::new();
        for next in modifier
            .get_positions(pos, random, world, &self.id, min_y, height)
            .map_err(|unsupported| PlacementFailure {
                placed_feature: self.id.clone(),
                unsupported,
            })?
        {
            positions.extend(self.apply_modifiers(
                index + 1,
                next,
                random,
                world,
                min_y,
                height,
            )?);
        }
        Ok(positions)
    }
}

/// [`PlacementWorld`] backed by the generator's biome source.
///
/// `LevelReader.getBiome` goes through `BiomeManager`'s fuzzy zoom, so this
/// pairs a [`BiomeSampler`] with a [`BiomeZoomer`] and answers
/// `BiomeGenerationSettings.hasFeature` from the biome feature lists loaded
/// from `feature_data.json`.
pub struct BiomePlacementWorld<'a> {
    data: &'a FeatureData,
    biomes: &'a BiomeSampler,
    zoomer: BiomeZoomer,
}

impl<'a> BiomePlacementWorld<'a> {
    pub fn new(data: &'a FeatureData, biomes: &'a BiomeSampler, world_seed: i64) -> Self {
        Self {
            data,
            biomes,
            zoomer: BiomeZoomer::new(world_seed),
        }
    }
}

impl PlacementWorld for BiomePlacementWorld<'_> {
    fn biome_at(&self, pos: BlockPos) -> &str {
        self.zoomer.biome_at(self.biomes, pos)
    }

    fn biome_has_placed_feature(&self, biome: &str, placed_feature: &str) -> bool {
        self.data.biome_features(biome).is_some_and(|steps| {
            steps
                .iter()
                .any(|step| step.iter().any(|feature| feature == placed_feature))
        })
    }
}

/// All vanilla feature definitions needed to execute generation steps.
#[derive(Debug)]
pub struct FeatureData {
    configured: HashMap<String, ConfiguredFeature>,
    placed: HashMap<String, PlacedFeature>,
    biome_features: HashMap<String, Vec<Vec<String>>>,
    tags: HashMap<String, Vec<String>>,
}

impl FeatureData {
    /// Parses the embedded `assets/feature_data.json` once.
    pub fn vanilla() -> &'static FeatureData {
        static DATA: OnceLock<FeatureData> = OnceLock::new();
        DATA.get_or_init(|| {
            Self::parse(FEATURE_DATA).expect("embedded feature_data.json must stay valid")
        })
    }

    /// Parses and validates a `feature_data.json` document.
    pub fn parse(json: &str) -> io::Result<Self> {
        let value: Value = serde_json::from_str(json)
            .map_err(|error| invalid(format!("feature data is not valid JSON: {error}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("feature data must be a JSON object"))?;

        let configured = parse_configured(section(object, "configured")?)?;
        let placed = parse_placed(section(object, "placed")?)?;

        for (name, placement) in &placed {
            if !configured.contains_key(placement.feature()) {
                return Err(invalid(format!(
                    "placed feature {name} refers to unknown configured feature {}",
                    placement.feature()
                )));
            }
        }

        let biome_features = parse_biomes(section(object, "biomes")?, &placed)?;
        let tags = parse_tags(section(object, "tags")?)?;

        Ok(FeatureData {
            configured,
            placed,
            biome_features,
            tags,
        })
    }

    pub fn configured(&self, id: &str) -> Option<&ConfiguredFeature> {
        self.configured.get(id)
    }

    pub fn placed(&self, id: &str) -> Option<&PlacedFeature> {
        self.placed.get(id)
    }

    /// Placed-feature lists for a biome, indexed by [`Decoration::index`].
    pub fn biome_features(&self, biome: &str) -> Option<&[Vec<String>]> {
        self.biome_features.get(biome).map(Vec::as_slice)
    }

    /// Placed features per decoration step in `FeatureSorter` order.
    ///
    /// Ports `FeatureSorter.buildFeaturesPerStep`: every adjacent pair in a
    /// biome's step list becomes a dependency edge, the graph is traversed in
    /// `(step, first-encounter index)` order, and the post-order result is
    /// reversed. The position in each returned list is the `featureIndex`
    /// vanilla uses in `WorldgenRandom.setFeatureSeed`.
    pub fn step_feature_order(&self, possible_biomes: &[&str]) -> Vec<Vec<String>> {
        let mut feature_indices: HashMap<String, usize> = HashMap::new();
        let mut features: Vec<String> = Vec::new();
        let mut graph: BTreeMap<(usize, usize), BTreeSet<(usize, usize)>> = BTreeMap::new();

        for biome in possible_biomes {
            let Some(steps) = self.biome_features(biome) else {
                continue;
            };
            for (step, list) in steps.iter().enumerate() {
                let mut nodes = Vec::with_capacity(list.len());
                for id in list {
                    let index = match feature_indices.get(id) {
                        Some(index) => *index,
                        None => {
                            let index = features.len();
                            features.push(id.clone());
                            feature_indices.insert(id.clone(), index);
                            index
                        }
                    };
                    nodes.push((step, index));
                }
                for pair in nodes.windows(2) {
                    graph.entry(pair[0]).or_default().insert(pair[1]);
                }
            }
        }

        let mut visited = BTreeSet::new();
        let mut in_progress = BTreeSet::new();
        let mut ordered = Vec::new();
        for root in graph.keys().copied().collect::<Vec<_>>() {
            if !visited.contains(&root) {
                depth_first_search(&graph, root, &mut visited, &mut in_progress, &mut ordered);
            }
        }
        ordered.reverse();

        let mut by_step = vec![Vec::new(); DECORATION_STEPS];
        for (step, index) in ordered {
            if step < DECORATION_STEPS {
                by_step[step].push(features[index].clone());
            }
        }
        by_step
    }

    /// Values of one extracted block tag, e.g. `stone_ore_replaceables`.
    pub fn tag(&self, tag: &str) -> Option<&[String]> {
        self.tags.get(tag).map(Vec::as_slice)
    }

    pub fn configured_count(&self) -> usize {
        self.configured.len()
    }

    pub fn placed_count(&self) -> usize {
        self.placed.len()
    }

    pub fn biome_count(&self) -> usize {
        self.biome_features.len()
    }

    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    /// Configured features whose vanilla type has a Rust implementation.
    pub fn supported_feature_count(&self) -> usize {
        self.configured
            .values()
            .filter(|feature| feature.is_supported())
            .count()
    }

    /// Count of unported features per vanilla type, for TODO/parity reporting.
    pub fn unsupported_type_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::new();
        for feature in self.configured.values() {
            if matches!(feature.kind, ConfiguredFeatureKind::Unsupported { .. }) {
                *counts.entry(feature.vanilla_type()).or_insert(0) += 1;
            }
        }
        counts
    }
}

fn depth_first_search(
    graph: &BTreeMap<(usize, usize), BTreeSet<(usize, usize)>>,
    node: (usize, usize),
    visited: &mut BTreeSet<(usize, usize)>,
    in_progress: &mut BTreeSet<(usize, usize)>,
    ordered: &mut Vec<(usize, usize)>,
) {
    if visited.contains(&node) {
        return;
    }
    if !in_progress.insert(node) {
        panic!("feature order cycle found");
    }
    if let Some(neighbours) = graph.get(&node) {
        for next in neighbours {
            depth_first_search(graph, *next, visited, in_progress, ordered);
        }
    }
    in_progress.remove(&node);
    visited.insert(node);
    ordered.push(node);
}

fn section<'a>(object: &'a Map<String, Value>, key: &str) -> io::Result<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("feature data is missing the `{key}` object")))
}

fn parse_configured(
    entries: &Map<String, Value>,
) -> io::Result<HashMap<String, ConfiguredFeature>> {
    let mut configured = HashMap::with_capacity(entries.len());
    let mut patches = Vec::new();
    let mut vegetation_patches = Vec::new();
    let mut selectors = Vec::new();
    for (name, entry) in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid(format!("configured feature {name} must be an object")))?;
        let vanilla_type = entry
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("configured feature {name} is missing `type`")))?;
        let id = resource_location(name);

        match vanilla_type {
            "minecraft:random_patch" | "minecraft:flower" => {
                patches.push((id, vanilla_type.to_owned(), entry.clone()));
                continue;
            }
            "minecraft:vegetation_patch" | "minecraft:waterlogged_vegetation_patch" => {
                vegetation_patches.push((id, vanilla_type.to_owned(), entry.clone()));
                continue;
            }
            "minecraft:random_selector" => {
                selectors.push((id, vanilla_type.to_owned(), entry.clone()));
                continue;
            }
            _ => {}
        }

        let kind = parse_configured_kind(&id, vanilla_type, entry, &configured)?;
        configured.insert(id.clone(), ConfiguredFeature { id, kind });
    }

    // Patches nest inline placed features; vegetation patches also reference
    // simple-block configured features. Resolve those after the direct types.
    for (id, vanilla_type, entry) in patches {
        let kind = parse_configured_kind(&id, &vanilla_type, &entry, &configured)?;
        configured.insert(id.clone(), ConfiguredFeature { id, kind });
    }
    for (id, vanilla_type, entry) in vegetation_patches {
        let kind = parse_configured_kind(&id, &vanilla_type, &entry, &configured)?;
        configured.insert(id.clone(), ConfiguredFeature { id, kind });
    }
    // Selectors reference other configured features, so they are resolved last.
    for (id, vanilla_type, entry) in selectors {
        let kind = parse_configured_kind(&id, &vanilla_type, &entry, &configured)?;
        configured.insert(id.clone(), ConfiguredFeature { id, kind });
    }
    Ok(configured)
}

fn parse_configured_kind(
    id: &str,
    vanilla_type: &str,
    entry: &Map<String, Value>,
    configured: &HashMap<String, ConfiguredFeature>,
) -> io::Result<ConfiguredFeatureKind> {
    if vanilla_type == "minecraft:no_op" {
        return Ok(ConfiguredFeatureKind::NoOp(NoOpFeature));
    }
    let config = entry
        .get("config")
        .ok_or_else(|| invalid(format!("configured feature {id} is missing `config`")))?;
    let unsupported = || ConfiguredFeatureKind::Unsupported {
        vanilla_type: vanilla_type.to_owned(),
    };
    match vanilla_type {
        "minecraft:tree" => Ok(match tree::parse_tree_config(config)? {
            Some(feature) => ConfiguredFeatureKind::Tree(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:fallen_tree" => Ok(match tree::parse_fallen_tree_config(config)? {
            Some(feature) => ConfiguredFeatureKind::FallenTree(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:random_selector" => Ok(match parse_selector(config, configured)? {
            Some(selector) => ConfiguredFeatureKind::RandomSelector(selector),
            None => unsupported(),
        }),
        "minecraft:ore" => Ok(match ore::parse_ore_config(config)? {
            Some(feature) => ConfiguredFeatureKind::Ore(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:scattered_ore" => Ok(match ore::parse_scattered_ore_config(config)? {
            Some(feature) => ConfiguredFeatureKind::ScatteredOre(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:random_patch" | "minecraft:flower" => Ok(
            match vegetation::parse_random_patch_config(config, configured, id, vanilla_type)? {
                Some(feature) => ConfiguredFeatureKind::RandomPatch(Box::new(feature)),
                None => unsupported(),
            },
        ),
        "minecraft:simple_block" => Ok(match vegetation::parse_simple_block_config(config)? {
            Some(feature) => ConfiguredFeatureKind::SimpleBlock(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:block_pile" => Ok(match vegetation::parse_block_pile_config(config)? {
            Some(feature) => ConfiguredFeatureKind::BlockPile(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:block_column" => Ok(match vegetation::parse_block_column_config(config)? {
            Some(feature) => ConfiguredFeatureKind::BlockColumn(Box::new(feature)),
            None => unsupported(),
        }),
        "minecraft:multiface_growth" => {
            Ok(match vegetation::parse_multiface_growth_config(config)? {
                Some(feature) => ConfiguredFeatureKind::MultifaceGrowth(Box::new(feature)),
                None => unsupported(),
            })
        }
        "minecraft:vegetation_patch" | "minecraft:waterlogged_vegetation_patch" => Ok(
            match vegetation::parse_vegetation_patch_config(
                config,
                configured,
                id,
                vanilla_type == "minecraft:waterlogged_vegetation_patch",
            )? {
                Some(feature) => ConfiguredFeatureKind::VegetationPatch(Box::new(feature)),
                None => unsupported(),
            },
        ),
        _ => Ok(unsupported()),
    }
}

/// Parses a configured feature embedded in a nested placed feature.
///
/// Inline features can themselves be containers (`random_patch` inside
/// `random_patch`) or references to already-parsed configured features.
pub(crate) fn parse_inline_configured_feature(
    id: &str,
    value: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
) -> io::Result<Option<Box<ConfiguredFeature>>> {
    if let Some(reference) = value.as_str() {
        let reference = resource_location(reference);
        return Ok(configured
            .get(&reference)
            .filter(|feature| feature.is_supported())
            .map(|feature| Box::new(feature.clone())));
    }
    let entry = value
        .as_object()
        .ok_or_else(|| invalid("inline configured feature must be an object"))?;
    let vanilla_type = entry
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("inline configured feature {id} is missing `type`")))?;
    let kind = parse_configured_kind(id, vanilla_type, entry, configured)?;
    let feature = ConfiguredFeature {
        id: id.to_owned(),
        kind,
    };
    Ok(feature.is_supported().then(|| Box::new(feature)))
}

fn parse_selector(
    config: &Value,
    configured: &HashMap<String, ConfiguredFeature>,
) -> io::Result<Option<RandomSelectorFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("random selector config must be an object"))?;
    let default_reference = feature_reference(
        object
            .get("default")
            .ok_or_else(|| invalid("random selector is missing `default`"))?,
    )?;
    let Some(default) = configured
        .get(&default_reference)
        .filter(|feature| feature.is_supported())
    else {
        return Ok(None);
    };

    let mut features = Vec::new();
    if let Some(list) = object.get("features") {
        let list = list
            .as_array()
            .ok_or_else(|| invalid("random selector `features` must be a list"))?;
        for entry in list {
            let entry = entry
                .as_object()
                .ok_or_else(|| invalid("random selector entry must be an object"))?;
            let chance = entry
                .get("chance")
                .and_then(Value::as_f64)
                .ok_or_else(|| invalid("random selector entry is missing `chance`"))?
                as f32;
            let reference = feature_reference(
                entry
                    .get("feature")
                    .ok_or_else(|| invalid("random selector entry is missing `feature`"))?,
            )?;
            let feature = configured
                .get(&reference)
                .filter(|feature| feature.is_supported())
                .map(|feature| Box::new(feature.clone()));
            features.push((chance, feature));
        }
    }

    Ok(Some(RandomSelectorFeature {
        default: Box::new(default.clone()),
        features,
    }))
}

/// A selector branch may be a bare configured-feature id or an inline
/// `{ "feature": ..., "placement": ... }` pair.
fn feature_reference(value: &Value) -> io::Result<String> {
    if let Some(name) = value.as_str() {
        return Ok(resource_location(name));
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("feature reference must be a string or an object"))?;
    let name = object
        .get("feature")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("feature reference object is missing `feature`"))?;
    Ok(resource_location(name))
}

fn parse_placed(entries: &Map<String, Value>) -> io::Result<HashMap<String, PlacedFeature>> {
    let mut placed = HashMap::with_capacity(entries.len());
    for (name, entry) in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid(format!("placed feature {name} must be an object")))?;
        let feature = entry
            .get("feature")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("placed feature {name} is missing `feature`")))?;
        let placement = entry
            .get("placement")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(format!("placed feature {name} is missing `placement`")))?;
        let id = resource_location(name);
        let modifiers = placement
            .iter()
            .map(placement::parse_modifier)
            .collect::<io::Result<Vec<_>>>()?;
        placed.insert(
            id.clone(),
            PlacedFeature {
                id,
                feature: resource_location(feature),
                modifiers,
            },
        );
    }
    Ok(placed)
}

fn parse_biomes(
    entries: &Map<String, Value>,
    placed: &HashMap<String, PlacedFeature>,
) -> io::Result<HashMap<String, Vec<Vec<String>>>> {
    let mut biomes = HashMap::with_capacity(entries.len());
    for (name, entry) in entries {
        let steps = entry
            .as_array()
            .ok_or_else(|| invalid(format!("biome {name} must hold a list of feature steps")))?;
        if steps.len() > DECORATION_STEPS {
            return Err(invalid(format!(
                "biome {name} has {} feature steps, more than {DECORATION_STEPS}",
                steps.len()
            )));
        }

        let mut parsed_steps = Vec::with_capacity(DECORATION_STEPS);
        for (index, step) in steps.iter().enumerate() {
            let step = step.as_array().ok_or_else(|| {
                invalid(format!("biome {name} feature step {index} must be a list"))
            })?;
            let mut features = Vec::with_capacity(step.len());
            for value in step {
                let reference = value.as_str().ok_or_else(|| {
                    invalid(format!(
                        "biome {name} feature step {index} contains a non-string entry"
                    ))
                })?;
                let reference = resource_location(reference);
                if !placed.contains_key(&reference) {
                    return Err(invalid(format!(
                        "biome {name} refers to unknown placed feature {reference}"
                    )));
                }
                features.push(reference);
            }
            parsed_steps.push(features);
        }
        parsed_steps.resize(DECORATION_STEPS, Vec::new());
        biomes.insert(resource_location(name), parsed_steps);
    }
    Ok(biomes)
}

fn parse_tags(entries: &Map<String, Value>) -> io::Result<HashMap<String, Vec<String>>> {
    let mut tags = HashMap::with_capacity(entries.len());
    for (name, entry) in entries {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid(format!("tag {name} must be an object")))?;
        let values = entry
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(format!("tag {name} is missing `values`")))?;
        let mut parsed = Vec::with_capacity(values.len());
        for value in values {
            let value = value
                .as_str()
                .ok_or_else(|| invalid(format!("tag {name} contains a non-string entry")))?;
            parsed.push(resource_location(value));
        }
        tags.insert(resource_location(name), parsed);
    }
    Ok(tags)
}

/// Vanilla registry keys are resource locations; the dump writes bare names for
/// `minecraft:` entries.
fn resource_location(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::biome::BiomeSampler;
    use folia_random::{LegacyRandomSource, WorldgenRandom};

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;

    /// Placement world stub with one biome and a fixed feature list.
    struct TestWorld {
        biome: &'static str,
        features: &'static [&'static str],
    }

    impl PlacementWorld for TestWorld {
        fn biome_at(&self, _pos: BlockPos) -> &str {
            self.biome
        }

        fn biome_has_placed_feature(&self, biome: &str, placed_feature: &str) -> bool {
            biome == self.biome && self.features.contains(&placed_feature)
        }
    }

    const NO_FEATURES: TestWorld = TestWorld {
        biome: "minecraft:plains",
        features: &[],
    };

    fn stone() -> BlockState {
        BlockState {
            name: "minecraft:stone".to_owned(),
            properties: Vec::new(),
        }
    }

    fn chunk() -> Chunk {
        Chunk {
            x: 0,
            z: 0,
            sections: Vec::new(),
        }
    }

    /// Test-only feature: writes one stone block at a random column offset so
    /// determinism depends on the seeded RNG.
    struct RandomProbeFeature;

    impl Feature for RandomProbeFeature {
        fn name(&self) -> &'static str {
            "test:random_probe"
        }

        fn place(&self, context: &mut FeaturePlaceContext<'_>) -> bool {
            let x = context.origin.x() + context.random.next_int_bound(16);
            let z = context.origin.z() + context.random.next_int_bound(16);
            context
                .level
                .set_block(BlockPos::new(x, context.origin.y(), z), stone())
        }
    }

    fn place_repeatedly(seed: i64, placements: usize) -> Chunk {
        let feature = RandomProbeFeature;
        let mut chunk = chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(seed);
            for _ in 0..placements {
                let mut context = FeaturePlaceContext {
                    origin: BlockPos::new(0, 0, 0),
                    random: &mut random,
                    level: &mut level,
                };
                assert!(feature.place(&mut context));
            }
        }
        chunk
    }

    struct DataWorld(&'static FeatureData);

    impl PlacementWorld for DataWorld {
        fn biome_at(&self, _pos: BlockPos) -> &str {
            "minecraft:ocean"
        }

        fn biome_has_placed_feature(&self, biome: &str, placed_feature: &str) -> bool {
            self.0.biome_features(biome).is_some_and(|steps| {
                steps
                    .iter()
                    .any(|step| step.iter().any(|feature| feature == placed_feature))
            })
        }
    }

    /// Differential fixture from the Java oracle: `ore_dirt` is index 0 of
    /// `underground_ores`, and chunk (0, -1) at seed 12345 starts with these
    /// exact positions. This also pins the depth-first modifier order: the
    /// pre-fix breadth-first traversal produced y = 15 instead of 135 first.
    #[test]
    fn ore_placement_matches_the_java_oracle() {
        let data = FeatureData::vanilla();
        let sampler = BiomeSampler::overworld(12_345).unwrap();
        let order = data.step_feature_order(&sampler.possible_biomes());
        assert_eq!(order[Decoration::UndergroundOres.index()].len(), 34);
        assert_eq!(
            order[Decoration::UndergroundOres.index()][0],
            "minecraft:ore_dirt"
        );
        assert_eq!(
            order[Decoration::UndergroundOres.index()][10],
            "minecraft:ore_coal_lower"
        );

        let mut random = WorldgenRandom::new(0);
        let decoration_seed = random.set_decoration_seed(12_345, 0, -16);
        random.set_feature_seed(
            decoration_seed,
            0,
            Decoration::UndergroundOres.index() as i32,
        );
        let placed = data.placed("minecraft:ore_dirt").unwrap();
        let positions = placed
            .placement_positions(
                BlockPos::new(0, MIN_Y, -16),
                &mut random,
                &DataWorld(data),
                MIN_Y,
                HEIGHT,
            )
            .unwrap();
        let positions: Vec<_> = positions
            .iter()
            .map(|pos| (pos.x(), pos.y(), pos.z()))
            .collect();
        assert_eq!(
            &positions[..4],
            &[(15, 135, -9), (15, 7, -4), (10, 116, -6), (11, 64, -5)]
        );
    }

    #[test]
    fn a_feature_replays_deterministically_for_a_seed() {
        let first = place_repeatedly(42, 16);
        let second = place_repeatedly(42, 16);
        assert_eq!(first, second);

        let placed = first
            .sections
            .iter()
            .flat_map(|section| section.block_states.palette.iter())
            .filter(|state| state.name == "minecraft:stone")
            .count();
        assert_eq!(placed, 1, "one palette entry for the repeated block");
    }

    #[test]
    fn different_seeds_place_at_different_positions() {
        let first = place_repeatedly(1, 8);
        let second = place_repeatedly(2, 8);
        assert_ne!(first, second);
    }

    #[test]
    fn writes_outside_the_region_are_rejected() {
        let feature = RandomProbeFeature;
        let mut chunk = chunk();
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        let mut random = LegacyRandomSource::new(7);
        // Origin in the next chunk over: the random offset cannot bring it back.
        let mut context = FeaturePlaceContext {
            origin: BlockPos::new(32, 0, 0),
            random: &mut random,
            level: &mut level,
        };
        assert!(!feature.place(&mut context));
        assert!(context.level.get_block(BlockPos::new(32, 0, 0)).is_none());
        assert!(level.chunk.sections.is_empty());
    }

    #[test]
    fn vertical_bounds_are_enforced() {
        let mut chunk = chunk();
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        assert!(level.set_block(BlockPos::new(0, MIN_Y, 0), stone()));
        assert!(level.set_block(BlockPos::new(0, MIN_Y + HEIGHT - 1, 0), stone()));
        assert!(!level.set_block(BlockPos::new(0, MIN_Y - 1, 0), stone()));
        assert!(!level.set_block(BlockPos::new(0, MIN_Y + HEIGHT, 0), stone()));
    }

    #[test]
    fn no_op_succeeds_without_writing() {
        let mut chunk = chunk();
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        let mut random = LegacyRandomSource::new(3);
        let mut context = FeaturePlaceContext {
            origin: BlockPos::new(0, 0, 0),
            random: &mut random,
            level: &mut level,
        };
        assert!(NoOpFeature.place(&mut context));
        assert!(level.chunk.sections.is_empty());
    }

    #[test]
    fn decoration_steps_match_vanilla_order() {
        let names: Vec<&str> = Decoration::ALL.iter().map(|step| step.name()).collect();
        assert_eq!(
            names,
            [
                "raw_generation",
                "lakes",
                "local_modifications",
                "underground_structures",
                "surface_structures",
                "strongholds",
                "underground_ores",
                "underground_decoration",
                "fluid_springs",
                "vegetal_decoration",
                "top_layer_modification",
            ]
        );
        for (index, step) in Decoration::ALL.into_iter().enumerate() {
            assert_eq!(step.index(), index);
            assert_eq!(Decoration::from_name(step.name()), Some(step));
        }
    }

    #[test]
    fn vanilla_feature_data_loads_and_resolves_every_reference() {
        let data = FeatureData::vanilla();
        assert_eq!(data.configured_count(), 224);
        assert_eq!(data.placed_count(), 258);
        assert_eq!(data.biome_count(), 65);
        assert_eq!(data.tag_count(), 2);

        for feature in data.configured.values() {
            assert!(feature.id().starts_with("minecraft:"));
        }
        for placement in data.placed.values() {
            assert!(
                data.configured(placement.feature()).is_some(),
                "{} lost its configured feature",
                placement.id()
            );
        }

        let plains = data
            .biome_features("minecraft:plains")
            .expect("plains has feature steps");
        assert_eq!(plains.len(), DECORATION_STEPS);
        let ores = &plains[Decoration::UndergroundOres.index()];
        assert!(
            ores.iter()
                .any(|feature| feature == "minecraft:ore_diamond"),
            "plains underground-ore list should contain ore_diamond"
        );

        let stone_ores = data
            .tag("minecraft:stone_ore_replaceables")
            .expect("stone ore replaceables tag");
        assert_eq!(
            stone_ores,
            [
                "minecraft:stone",
                "minecraft:granite",
                "minecraft:diorite",
                "minecraft:andesite",
            ]
        );

        assert_eq!(
            data.supported_feature_count() + data.unsupported_type_counts().values().sum::<usize>(),
            224,
            "every configured feature is either supported or explicitly counted"
        );
    }

    #[test]
    fn vanilla_placement_modifiers_load_and_split_by_support() {
        let data = FeatureData::vanilla();
        let mut total = 0;
        let mut supported = 0;
        let mut unsupported: BTreeMap<&str, usize> = BTreeMap::new();
        for placement in data.placed.values() {
            total += placement.placement_modifier_count();
            for modifier in placement.placement_modifiers() {
                if modifier.is_supported() {
                    supported += 1;
                } else {
                    *unsupported.entry(modifier.vanilla_type()).or_insert(0) += 1;
                }
            }
        }

        assert_eq!(total, 888, "every placed feature's modifiers load");
        assert_eq!(supported + unsupported.values().sum::<usize>(), total);
        assert_eq!(
            supported, 762,
            "world-free modifiers, `biome`, ocean-floor heightmaps, water depth \
             filters, and sapling survival predicates"
        );
        assert_eq!(unsupported["minecraft:heightmap"], 83);
        assert_eq!(unsupported["minecraft:block_predicate_filter"], 12);
        assert_eq!(unsupported["minecraft:count_on_every_layer"], 8);
        assert_eq!(unsupported["minecraft:noise_threshold_count"], 6);
        assert_eq!(unsupported["minecraft:noise_based_count"], 4);

        // `end_platform` is a fixed position followed by a biome filter.
        let end_platform = data
            .placed("minecraft:end_platform")
            .expect("end_platform is in the dump");
        assert_eq!(end_platform.placement_modifier_count(), 2);
        assert!(end_platform.placement_is_supported());
        let mut random = LegacyRandomSource::new(0);
        let end_world = TestWorld {
            biome: "minecraft:the_end",
            features: &["minecraft:end_platform"],
        };
        assert_eq!(
            end_platform
                .placement_positions(
                    BlockPos::new(96, 49, 0),
                    &mut random,
                    &end_world,
                    MIN_Y,
                    HEIGHT
                )
                .expect("fixed placement and biome filter are both ported"),
            vec![BlockPos::new(100, 49, 0)]
        );
        assert!(
            end_platform
                .placement_positions(
                    BlockPos::new(96, 49, 0),
                    &mut random,
                    &NO_FEATURES,
                    MIN_Y,
                    HEIGHT
                )
                .expect("biome filter is ported")
                .is_empty(),
            "a biome that does not list the feature removes the position"
        );

        // A modifier-less placement yields exactly its origin without drawing.
        let pile_snow = data
            .placed("minecraft:pile_snow")
            .expect("pile_snow is in the dump");
        assert_eq!(pile_snow.placement_modifier_count(), 0);
        assert_eq!(
            pile_snow
                .placement_positions(
                    BlockPos::new(3, 70, -9),
                    &mut random,
                    &NO_FEATURES,
                    MIN_Y,
                    HEIGHT
                )
                .expect("no modifiers means no unsupported step"),
            vec![BlockPos::new(3, 70, -9)]
        );
    }

    #[test]
    fn biome_placement_world_uses_the_generators_biome_lists() {
        let data = FeatureData::vanilla();
        let biomes = BiomeSampler::overworld(0).expect("embedded worldgen data loads");
        let world = BiomePlacementWorld::new(data, &biomes, 0);

        let pos = BlockPos::new(0, 64, 0);
        let biome = world.biome_at(pos);
        let listed = data
            .biome_features(biome)
            .expect("the sampled biome has a feature list")
            .iter()
            .flatten()
            .next()
            .cloned()
            .expect("every biome lists at least one placed feature");
        assert!(world.biome_has_placed_feature(biome, &listed));
        assert!(
            !world.biome_has_placed_feature(biome, "minecraft:not_a_feature"),
            "membership is exact"
        );
        assert_eq!(
            biome,
            BiomeZoomer::new(0).biome_at(&biomes, pos),
            "LevelReader.getBiome goes through the fuzzy zoomer"
        );
    }

    #[test]
    fn fallen_tree_features_are_dispatched_as_supported() {
        let data = FeatureData::vanilla();
        for id in [
            "minecraft:fallen_oak_tree",
            "minecraft:fallen_birch_tree",
            "minecraft:fallen_jungle_tree",
            "minecraft:fallen_spruce_tree",
            "minecraft:fallen_super_birch_tree",
        ] {
            let feature = data
                .configured(id)
                .unwrap_or_else(|| panic!("{id} is in the dump"));
            assert!(feature.is_supported(), "{id} should be executable");
            assert_eq!(feature.vanilla_type(), "minecraft:fallen_tree");
        }
    }

    #[test]
    fn ore_features_are_dispatched_as_supported() {
        let data = FeatureData::vanilla();
        for id in [
            "minecraft:ore_dirt",
            "minecraft:ore_diamond_medium",
            "minecraft:ore_ancient_debris_small",
        ] {
            let feature = data
                .configured(id)
                .unwrap_or_else(|| panic!("{id} is in the dump"));
            assert!(feature.is_supported(), "{id} should be executable");
            assert!(matches!(
                feature.vanilla_type(),
                "minecraft:ore" | "minecraft:scattered_ore"
            ));
        }
    }

    #[test]
    fn unsupported_features_fail_loudly() {
        let data = FeatureData::vanilla();
        let feature = data
            .configured("minecraft:huge_brown_mushroom")
            .expect("huge_brown_mushroom is in the dump");
        assert!(!feature.is_supported());
        let mut chunk = chunk();
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        let mut random = LegacyRandomSource::new(0);
        let error = feature
            .place(&mut level, &mut random, BlockPos::new(0, 0, 0))
            .expect_err("unported feature must not pretend to place");
        assert_eq!(error.feature, "minecraft:huge_brown_mushroom");
        assert_eq!(error.vanilla_type, "minecraft:huge_brown_mushroom");
        assert!(level.chunk.sections.is_empty());
    }

    #[test]
    fn ground_cover_features_are_dispatched_as_supported() {
        let data = FeatureData::vanilla();
        for (id, vanilla_type) in [
            ("minecraft:patch_grass", "minecraft:random_patch"),
            ("minecraft:flower_plain", "minecraft:flower"),
            ("minecraft:single_piece_of_grass", "minecraft:simple_block"),
            ("minecraft:pile_hay", "minecraft:block_pile"),
            ("minecraft:cave_vine", "minecraft:block_column"),
            ("minecraft:glow_lichen", "minecraft:multiface_growth"),
            ("minecraft:moss_patch", "minecraft:vegetation_patch"),
        ] {
            let feature = data
                .configured(id)
                .unwrap_or_else(|| panic!("{id} is in the dump"));
            assert!(feature.is_supported(), "{id} should be executable");
            assert_eq!(feature.vanilla_type(), vanilla_type);
        }
    }

    #[test]
    fn loader_rejects_unknown_references_and_step_overflow() {
        let unknown_placement = r#"{
            "configured": {"a": {"type": "minecraft:no_op"}},
            "placed": {"a": {"feature": "b", "placement": []}},
            "biomes": {},
            "tags": {}
        }"#;
        assert!(FeatureData::parse(unknown_placement).is_err());

        let too_many_steps = format!(
            r#"{{
                "configured": {{}},
                "placed": {{}},
                "biomes": {{"minecraft:x": [{}]}},
                "tags": {{}}
            }}"#,
            "[]".repeat(DECORATION_STEPS + 1).replace("][", "],[")
        );
        assert!(FeatureData::parse(&too_many_steps).is_err());
    }
}
