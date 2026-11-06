//! `net/minecraft/world/level/levelgen/placement/*`: placement modifiers.
//!
//! A [`PlacedFeature`](super::PlacedFeature) turns one candidate position into
//! zero or more positions by running its modifiers in order, exactly like
//! `PlacementModifier.getPositions`.
//!
//! Ported here: `count`, `rarity_filter`, `in_square`, `fixed_placement`,
//! `height_range`, `random_offset` and `biome`, plus the
//! `IntProvider`/`HeightProvider` distributions those use. Only `biome` needs
//! world state, through [`PlacementWorld`]. Modifiers that read the world or
//! the generator (`heightmap`, `block_predicate_filter`,
//! `surface_water_depth_filter`, `environment_scan`, `count_on_every_layer`,
//! the noise counts, `surface_relative_threshold_filter`) parse as
//! [`PlacementModifierKind::Unsupported`] and return [`UnsupportedModifier`]
//! instead of silently filtering everything.

use crate::chunk::BlockState;
use folia_math::block_pos::BlockPos;
use folia_math::mth::clamp;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::fmt;
use std::io;

use super::{invalid, resource_location};

/// `Heightmap.Types` values used by placement modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heightmap {
    /// `Heightmap.Types.WORLD_SURFACE`, the first empty block above the
    /// highest non-air block including fluids.
    WorldSurface,
    /// `Heightmap.Types.OCEAN_FLOOR`, the first empty block above the highest
    /// motion-blocking block, ignoring fluids.
    OceanFloor,
}

/// The subset of `BlockPredicate` the placement pipeline can evaluate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPredicate {
    /// `BlockPredicate.wouldSurvive(state, offset)` for a sapling.
    WouldSurvive(BlockState),
    /// Parsed but not ported.
    Unsupported { vanilla_type: String },
}

impl BlockPredicate {
    /// Whether this predicate can run against a [`PlacementWorld`].
    pub fn is_supported(&self) -> bool {
        matches!(self, BlockPredicate::WouldSurvive(_))
    }

    /// `BlockPredicate.test(LevelReader, BlockPos)`.
    pub fn test(&self, world: &dyn PlacementWorld, pos: BlockPos) -> bool {
        match self {
            BlockPredicate::WouldSurvive(_) => world
                .block_at(pos.below())
                .is_some_and(|below| is_sapling_soil(&below.name)),
            BlockPredicate::Unsupported { .. } => false,
        }
    }
}

/// Blocks the vanilla `minecraft:dirt` tag contains; saplings can survive on
/// all of them.
fn is_sapling_soil(name: &str) -> bool {
    matches!(
        name,
        "minecraft:dirt"
            | "minecraft:grass_block"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:mycelium"
            | "minecraft:rooted_dirt"
            | "minecraft:moss_block"
            | "minecraft:muddy_mangrove_roots"
    )
}

/// World state a placement modifier may need.
///
/// `PlacedFeature.placeWithContext` carries a `WorldGenLevel` and the
/// `PlacedFeature` being placed; these are the queries the ported filters use.
/// Implementations must return the biome `LevelReader.getBiome` would, i.e.
/// after `BiomeManager`'s fuzzy zoom, not the raw noise biome.
pub trait PlacementWorld {
    /// `LevelReader.getBiome(pos)`.
    fn biome_at(&self, pos: BlockPos) -> &str;

    /// `BiomeGenerationSettings.hasFeature(placedFeature)` for the biome at
    /// `pos`.
    fn biome_has_placed_feature(&self, biome: &str, placed_feature: &str) -> bool;

    /// `PlacementContext.getHeight(heightmap, x, z)`.
    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        None
    }

    /// `LevelReader.getBlockState(pos)`.
    fn block_at(&self, _pos: BlockPos) -> Option<&BlockState> {
        None
    }
}

/// A placement component whose vanilla type has no Rust port yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedModifier {
    pub vanilla_type: String,
}

impl UnsupportedModifier {
    fn new(vanilla_type: impl Into<String>) -> Self {
        Self {
            vanilla_type: vanilla_type.into(),
        }
    }
}

impl fmt::Display for UnsupportedModifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "placement component {} is not ported yet",
            self.vanilla_type
        )
    }
}

impl std::error::Error for UnsupportedModifier {}

/// What went wrong while placing a specific placed feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementFailure {
    pub placed_feature: String,
    pub unsupported: UnsupportedModifier,
}

impl fmt::Display for PlacementFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "placed feature {} cannot run: {}",
            self.placed_feature, self.unsupported
        )
    }
}

impl std::error::Error for PlacementFailure {}

/// `VerticalAnchor`: a height expressed relative to the world's build limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    /// `VerticalAnchor.Absolute`.
    Absolute(i32),
    /// `VerticalAnchor.AboveBottom`: `min_y + offset`.
    AboveBottom(i32),
    /// `VerticalAnchor.BelowTop`: `min_y + height - 1 - offset`.
    BelowTop(i32),
}

impl VerticalAnchor {
    /// `VerticalAnchor.resolveY(WorldGenerationContext)`.
    pub fn resolve_y(self, min_y: i32, height: i32) -> i32 {
        match self {
            VerticalAnchor::Absolute(y) => y,
            VerticalAnchor::AboveBottom(offset) => min_y + offset,
            VerticalAnchor::BelowTop(offset) => height - 1 + min_y - offset,
        }
    }
}

/// One entry of a `WeightedList`.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedEntry<T> {
    pub data: T,
    pub weight: i32,
}

/// `net.minecraft.util.valueproviders.IntProvider`.
///
/// `Unsupported` covers distributions that are not ported yet
/// (`clamped_normal`, `trapezoid`, ...). Sampling one fails loudly.
#[derive(Debug, Clone, PartialEq)]
pub enum IntProvider {
    /// `ConstantInt`; serialized as a bare JSON integer.
    Constant(i32),
    /// `UniformInt`.
    Uniform {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `BiasedToBottomInt`.
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `ClampedInt`.
    Clamped {
        source: Box<IntProvider>,
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `WeightedListInt`.
    WeightedList(Vec<WeightedEntry<IntProvider>>),
    /// Parsed but not ported.
    Unsupported { vanilla_type: String },
}

impl IntProvider {
    /// Registry name of the provider type.
    pub fn vanilla_type(&self) -> &str {
        match self {
            IntProvider::Constant(_) => "minecraft:constant",
            IntProvider::Uniform { .. } => "minecraft:uniform",
            IntProvider::BiasedToBottom { .. } => "minecraft:biased_to_bottom",
            IntProvider::Clamped { .. } => "minecraft:clamped",
            IntProvider::WeightedList(_) => "minecraft:weighted_list",
            IntProvider::Unsupported { vanilla_type } => vanilla_type,
        }
    }

    /// `IntProvider.sample(RandomSource)`.
    ///
    /// Draw order matches vanilla so a seeded feature replays identically.
    pub fn sample(&self, random: &mut dyn RandomSource) -> Result<i32, UnsupportedModifier> {
        match self {
            IntProvider::Constant(value) => Ok(*value),
            IntProvider::Uniform {
                min_inclusive,
                max_inclusive,
            } => Ok(random.next_int_between_inclusive(*min_inclusive, *max_inclusive)),
            IntProvider::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => {
                let bound = max_inclusive - min_inclusive + 1;
                let outer = random.next_int_bound(bound);
                Ok(min_inclusive + random.next_int_bound(outer + 1))
            }
            IntProvider::Clamped {
                source,
                min_inclusive,
                max_inclusive,
            } => Ok(clamp(
                source.sample(random)?,
                *min_inclusive,
                *max_inclusive,
            )),
            IntProvider::WeightedList(entries) => {
                let total: i32 = entries.iter().map(|entry| entry.weight).sum();
                let mut index = random.next_int_bound(total);
                for entry in entries {
                    index -= entry.weight;
                    if index < 0 {
                        return entry.data.sample(random);
                    }
                }
                // `parse_int_provider` rejects empty and non-positive lists, so
                // `index < total` always selects an entry. Report instead of
                // inventing a value if that invariant ever breaks.
                Err(UnsupportedModifier::new("minecraft:weighted_list"))
            }
            IntProvider::Unsupported { vanilla_type } => {
                Err(UnsupportedModifier::new(vanilla_type.clone()))
            }
        }
    }

    /// Whether [`Self::sample`] can run.
    pub fn is_supported(&self) -> bool {
        match self {
            IntProvider::Constant(_)
            | IntProvider::Uniform { .. }
            | IntProvider::BiasedToBottom { .. } => true,
            IntProvider::Clamped { source, .. } => source.is_supported(),
            IntProvider::WeightedList(entries) => {
                entries.iter().all(|entry| entry.data.is_supported())
            }
            IntProvider::Unsupported { .. } => false,
        }
    }
}

/// `net.minecraft.world.level.levelgen.heightproviders.HeightProvider`.
#[derive(Debug, Clone, PartialEq)]
pub enum HeightProvider {
    /// `ConstantHeight`, also the bare-anchor serialized form.
    Constant(VerticalAnchor),
    /// `UniformHeight`.
    Uniform {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    /// `TrapezoidHeight`.
    Trapezoid {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        plateau: i32,
    },
    /// `VeryBiasedToBottomHeight`.
    VeryBiasedToBottom {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        inner: i32,
    },
    /// Parsed but not ported.
    Unsupported { vanilla_type: String },
}

impl HeightProvider {
    /// Registry name of the provider type.
    pub fn vanilla_type(&self) -> &str {
        match self {
            HeightProvider::Constant(_) => "minecraft:constant",
            HeightProvider::Uniform { .. } => "minecraft:uniform",
            HeightProvider::Trapezoid { .. } => "minecraft:trapezoid",
            HeightProvider::VeryBiasedToBottom { .. } => "minecraft:very_biased_to_bottom",
            HeightProvider::Unsupported { vanilla_type } => vanilla_type,
        }
    }

    /// `HeightProvider.sample(RandomSource, WorldGenerationContext)`.
    pub fn sample(
        &self,
        random: &mut dyn RandomSource,
        min_y: i32,
        height: i32,
    ) -> Result<i32, UnsupportedModifier> {
        match self {
            HeightProvider::Constant(anchor) => Ok(anchor.resolve_y(min_y, height)),
            HeightProvider::Uniform {
                min_inclusive,
                max_inclusive,
            } => {
                let min = min_inclusive.resolve_y(min_y, height);
                let max = max_inclusive.resolve_y(min_y, height);
                if min > max {
                    // Vanilla logs "Empty height range" and returns the minimum.
                    return Ok(min);
                }
                Ok(random.next_int_between_inclusive(min, max))
            }
            HeightProvider::Trapezoid {
                min_inclusive,
                max_inclusive,
                plateau,
            } => {
                let min = min_inclusive.resolve_y(min_y, height);
                let max = max_inclusive.resolve_y(min_y, height);
                if min > max {
                    return Ok(min);
                }
                let span = max - min;
                if *plateau >= span {
                    return Ok(random.next_int_between_inclusive(min, max));
                }
                let low = (span - plateau) / 2;
                let high = span - low;
                Ok(min
                    + random.next_int_between_inclusive(0, high)
                    + random.next_int_between_inclusive(0, low))
            }
            HeightProvider::VeryBiasedToBottom {
                min_inclusive,
                max_inclusive,
                inner,
            } => {
                let min = min_inclusive.resolve_y(min_y, height);
                let max = max_inclusive.resolve_y(min_y, height);
                // Vanilla tests `max - min - inner + 1 <= 0`.
                if max - min - inner < 0 {
                    return Ok(min);
                }
                let first = next_int_or_min(random, min + inner, max);
                let second = next_int_or_min(random, min, first - 1);
                Ok(next_int_or_min(random, min, second - 1 + inner))
            }
            HeightProvider::Unsupported { vanilla_type } => {
                Err(UnsupportedModifier::new(vanilla_type.clone()))
            }
        }
    }

    /// Whether [`Self::sample`] can run.
    pub fn is_supported(&self) -> bool {
        !matches!(self, HeightProvider::Unsupported { .. })
    }
}

/// `Mth.nextInt(RandomSource, int, int)`: `min` without drawing when empty.
fn next_int_or_min(random: &mut dyn RandomSource, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        random.next_int_bound(max - min + 1) + min
    }
}

/// A `PlacementModifier` instance.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementModifier {
    kind: PlacementModifierKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacementModifierKind {
    /// `CountPlacement` (`RepeatingPlacement`).
    Count(IntProvider),
    /// `RarityFilter`.
    RarityFilter { chance: i32 },
    /// `InSquarePlacement`.
    InSquare,
    /// `FixedPlacement`.
    FixedPlacement(Vec<BlockPos>),
    /// `HeightRangePlacement`.
    HeightRange(HeightProvider),
    /// `RandomOffsetPlacement`.
    RandomOffset {
        xz_spread: IntProvider,
        y_spread: IntProvider,
    },
    /// `BiomeFilter`.
    Biome,
    /// `HeightmapPlacement`.
    Heightmap(Heightmap),
    /// `SurfaceWaterDepthFilter`.
    SurfaceWaterDepth { max_water_depth: i32 },
    /// `BlockPredicateFilter`.
    BlockPredicate(BlockPredicate),
    /// Parsed but not ported.
    Unsupported { vanilla_type: String },
}

impl PlacementModifier {
    /// Registry name of the modifier type, e.g. `minecraft:in_square`.
    pub fn vanilla_type(&self) -> &str {
        match &self.kind {
            PlacementModifierKind::Count(_) => "minecraft:count",
            PlacementModifierKind::RarityFilter { .. } => "minecraft:rarity_filter",
            PlacementModifierKind::InSquare => "minecraft:in_square",
            PlacementModifierKind::FixedPlacement(_) => "minecraft:fixed_placement",
            PlacementModifierKind::HeightRange(_) => "minecraft:height_range",
            PlacementModifierKind::RandomOffset { .. } => "minecraft:random_offset",
            PlacementModifierKind::Biome => "minecraft:biome",
            PlacementModifierKind::Heightmap(_) => "minecraft:heightmap",
            PlacementModifierKind::SurfaceWaterDepth { .. } => {
                "minecraft:surface_water_depth_filter"
            }
            PlacementModifierKind::BlockPredicate(_) => "minecraft:block_predicate_filter",
            PlacementModifierKind::Unsupported { vanilla_type } => vanilla_type,
        }
    }

    /// The typed configuration, for inspection and tests.
    pub fn kind(&self) -> &PlacementModifierKind {
        &self.kind
    }

    /// `PlacementModifier.getPositions`.
    pub fn get_positions(
        &self,
        pos: BlockPos,
        random: &mut dyn RandomSource,
        world: &dyn PlacementWorld,
        placed_feature: &str,
        min_y: i32,
        height: i32,
    ) -> Result<Vec<BlockPos>, UnsupportedModifier> {
        match &self.kind {
            // RepeatingPlacement emits `count` copies of the same position.
            PlacementModifierKind::Count(count) => {
                let count = count.sample(random)?;
                Ok(std::iter::repeat_n(pos, count.max(0) as usize).collect())
            }
            PlacementModifierKind::RarityFilter { chance } => {
                if random.next_float() < 1.0 / *chance as f32 {
                    Ok(vec![pos])
                } else {
                    Ok(Vec::new())
                }
            }
            PlacementModifierKind::InSquare => Ok(vec![BlockPos::new(
                pos.x() + random.next_int_bound(16),
                pos.y(),
                pos.z() + random.next_int_bound(16),
            )]),
            PlacementModifierKind::FixedPlacement(positions) => {
                let chunk_x = section_coord(pos.x());
                let chunk_z = section_coord(pos.z());
                Ok(positions
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        chunk_x == section_coord(candidate.x())
                            && chunk_z == section_coord(candidate.z())
                    })
                    .collect())
            }
            PlacementModifierKind::HeightRange(provider) => {
                let y = provider.sample(random, min_y, height)?;
                Ok(vec![BlockPos::new(pos.x(), y, pos.z())])
            }
            PlacementModifierKind::RandomOffset {
                xz_spread,
                y_spread,
            } => {
                // Vanilla draws x, then y, then z.
                let x = pos.x() + xz_spread.sample(random)?;
                let y = pos.y() + y_spread.sample(random)?;
                let z = pos.z() + xz_spread.sample(random)?;
                Ok(vec![BlockPos::new(x, y, z)])
            }
            PlacementModifierKind::Biome => {
                let biome = world.biome_at(pos);
                if world.biome_has_placed_feature(biome, placed_feature) {
                    Ok(vec![pos])
                } else {
                    Ok(Vec::new())
                }
            }
            PlacementModifierKind::Heightmap(kind) => {
                let Some(y) = world.heightmap(*kind, pos.x(), pos.z()) else {
                    return Ok(Vec::new());
                };
                if y > min_y {
                    Ok(vec![pos.with_y(y)])
                } else {
                    Ok(Vec::new())
                }
            }
            PlacementModifierKind::SurfaceWaterDepth { max_water_depth } => {
                let ocean_floor = world.heightmap(Heightmap::OceanFloor, pos.x(), pos.z());
                let world_surface = world.heightmap(Heightmap::WorldSurface, pos.x(), pos.z());
                match (ocean_floor, world_surface) {
                    (Some(floor), Some(surface)) if surface - floor <= *max_water_depth => {
                        Ok(vec![pos])
                    }
                    _ => Ok(Vec::new()),
                }
            }
            PlacementModifierKind::BlockPredicate(predicate) => {
                if predicate.test(world, pos) {
                    Ok(vec![pos])
                } else {
                    Ok(Vec::new())
                }
            }
            PlacementModifierKind::Unsupported { vanilla_type } => {
                Err(UnsupportedModifier::new(vanilla_type.clone()))
            }
        }
    }

    /// Whether [`Self::get_positions`] can run.
    pub fn is_supported(&self) -> bool {
        match &self.kind {
            PlacementModifierKind::Count(count) => count.is_supported(),
            PlacementModifierKind::RarityFilter { .. }
            | PlacementModifierKind::InSquare
            | PlacementModifierKind::FixedPlacement(_)
            | PlacementModifierKind::Biome
            | PlacementModifierKind::Heightmap(_)
            | PlacementModifierKind::SurfaceWaterDepth { .. } => true,
            PlacementModifierKind::BlockPredicate(predicate) => predicate.is_supported(),
            PlacementModifierKind::HeightRange(provider) => provider.is_supported(),
            PlacementModifierKind::RandomOffset {
                xz_spread,
                y_spread,
            } => xz_spread.is_supported() && y_spread.is_supported(),
            PlacementModifierKind::Unsupported { .. } => false,
        }
    }
}

/// `SectionPos.blockToSectionCoord`.
fn section_coord(value: i32) -> i32 {
    value >> 4
}

pub(crate) fn parse_modifier(value: &Value) -> io::Result<PlacementModifier> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("placement modifier must be an object"))?;
    let type_name = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("placement modifier is missing `type`"))?;
    let type_name = resource_location(type_name);

    let kind = match type_name.as_str() {
        "minecraft:count" => {
            PlacementModifierKind::Count(parse_int_provider(field(object, "count", &type_name)?)?)
        }
        "minecraft:rarity_filter" => {
            let chance = int_field(object, "chance", &type_name)?;
            if chance <= 0 {
                return Err(invalid(format!(
                    "{type_name} needs a positive `chance`, got {chance}"
                )));
            }
            PlacementModifierKind::RarityFilter { chance }
        }
        "minecraft:in_square" => PlacementModifierKind::InSquare,
        "minecraft:fixed_placement" => {
            PlacementModifierKind::FixedPlacement(parse_positions(object, &type_name)?)
        }
        "minecraft:height_range" => PlacementModifierKind::HeightRange(parse_height_provider(
            field(object, "height", &type_name)?,
        )?),
        "minecraft:random_offset" => PlacementModifierKind::RandomOffset {
            xz_spread: parse_int_provider(field(object, "xz_spread", &type_name)?)?,
            y_spread: parse_int_provider(field(object, "y_spread", &type_name)?)?,
        },
        "minecraft:biome" => PlacementModifierKind::Biome,
        "minecraft:heightmap" => {
            let heightmap = field(object, "heightmap", &type_name)?
                .as_str()
                .ok_or_else(|| invalid(format!("{type_name} needs a string `heightmap`")))?;
            match heightmap {
                "WORLD_SURFACE" => PlacementModifierKind::Heightmap(Heightmap::WorldSurface),
                "OCEAN_FLOOR" => PlacementModifierKind::Heightmap(Heightmap::OceanFloor),
                _ => PlacementModifierKind::Unsupported {
                    vanilla_type: type_name.clone(),
                },
            }
        }
        "minecraft:surface_water_depth_filter" => PlacementModifierKind::SurfaceWaterDepth {
            max_water_depth: int_field(object, "max_water_depth", &type_name)?,
        },
        "minecraft:block_predicate_filter" => PlacementModifierKind::BlockPredicate(
            parse_block_predicate(field(object, "predicate", &type_name)?)?,
        ),
        _ => PlacementModifierKind::Unsupported {
            vanilla_type: type_name,
        },
    };
    Ok(PlacementModifier { kind })
}

fn parse_block_predicate(value: &Value) -> io::Result<BlockPredicate> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("block predicate must be an object"))?;
    let type_name = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("block predicate is missing `type`"))?;
    let type_name = resource_location(type_name);

    if type_name == "minecraft:would_survive" {
        let state = parse_block_state(field(object, "state", &type_name)?)?;
        if state.name.ends_with("_sapling") {
            return Ok(BlockPredicate::WouldSurvive(state));
        }
        return Ok(BlockPredicate::Unsupported {
            vanilla_type: format!("{type_name}:{}", state.name),
        });
    }

    Ok(BlockPredicate::Unsupported {
        vanilla_type: type_name,
    })
}

pub(crate) fn parse_block_state(value: &Value) -> io::Result<BlockState> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("block state must be an object"))?;
    let name = object
        .get("Name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("block state is missing `Name`"))?;
    let mut properties = Vec::new();
    if let Some(values) = object.get("Properties") {
        let values = values
            .as_object()
            .ok_or_else(|| invalid("block state `Properties` must be an object"))?;
        for (key, value) in values {
            let value = value
                .as_str()
                .ok_or_else(|| invalid("block state property values must be strings"))?;
            properties.push((key.clone(), value.to_owned()));
        }
        properties.sort_by(|left, right| left.0.cmp(&right.0));
    }
    Ok(BlockState {
        name: resource_location(name),
        properties,
    })
}

pub(crate) fn parse_int_provider(value: &Value) -> io::Result<IntProvider> {
    if let Some(constant) = value.as_i64() {
        return i32::try_from(constant)
            .map(IntProvider::Constant)
            .map_err(|_| invalid(format!("int provider {constant} does not fit in i32")));
    }

    let object = value
        .as_object()
        .ok_or_else(|| invalid("int provider must be an integer or an object"))?;
    let type_name = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("int provider is missing `type`"))?;
    let type_name = resource_location(type_name);

    Ok(match type_name.as_str() {
        "minecraft:uniform" => IntProvider::Uniform {
            min_inclusive: int_field(object, "min_inclusive", &type_name)?,
            max_inclusive: int_field(object, "max_inclusive", &type_name)?,
        },
        "minecraft:biased_to_bottom" => IntProvider::BiasedToBottom {
            min_inclusive: int_field(object, "min_inclusive", &type_name)?,
            max_inclusive: int_field(object, "max_inclusive", &type_name)?,
        },
        "minecraft:clamped" => IntProvider::Clamped {
            source: Box::new(parse_int_provider(field(object, "source", &type_name)?)?),
            min_inclusive: int_field(object, "min_inclusive", &type_name)?,
            max_inclusive: int_field(object, "max_inclusive", &type_name)?,
        },
        "minecraft:weighted_list" => parse_weighted_list(object, &type_name)?,
        _ => IntProvider::Unsupported {
            vanilla_type: type_name,
        },
    })
}

fn parse_weighted_list(object: &Map<String, Value>, type_name: &str) -> io::Result<IntProvider> {
    let distribution = field(object, "distribution", type_name)?
        .as_array()
        .ok_or_else(|| invalid(format!("{type_name} needs a `distribution` list")))?;
    if distribution.is_empty() {
        return Err(invalid(format!("{type_name} needs at least one entry")));
    }

    let mut entries = Vec::with_capacity(distribution.len());
    let mut total = 0_i32;
    for entry in distribution {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid(format!("{type_name} entry must be an object")))?;
        let weight = int_field(entry, "weight", type_name)?;
        if weight <= 0 {
            return Err(invalid(format!(
                "{type_name} needs positive weights, got {weight}"
            )));
        }
        total += weight;
        entries.push(WeightedEntry {
            data: parse_int_provider(field(entry, "data", type_name)?)?,
            weight,
        });
    }

    // Vanilla switches to a pre-sorted selector at 64 total weight; the
    // in-order scan below only reproduces the flat selector. Every shipped
    // list totals 20 or less, so refuse the divergent case instead of
    // silently sampling differently.
    if total >= 64 {
        return Ok(IntProvider::Unsupported {
            vanilla_type: type_name.to_owned(),
        });
    }
    Ok(IntProvider::WeightedList(entries))
}

fn parse_height_provider(value: &Value) -> io::Result<HeightProvider> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("height provider must be an object"))?;

    let Some(type_name) = object.get("type").and_then(Value::as_str) else {
        // `HeightProvider.CODEC` serializes a constant as a bare anchor.
        return Ok(HeightProvider::Constant(parse_anchor(object)?));
    };
    let type_name = resource_location(type_name);

    Ok(match type_name.as_str() {
        "minecraft:constant" => {
            HeightProvider::Constant(parse_anchor_value(field(object, "value", &type_name)?)?)
        }
        "minecraft:uniform" => HeightProvider::Uniform {
            min_inclusive: parse_anchor_value(field(object, "min_inclusive", &type_name)?)?,
            max_inclusive: parse_anchor_value(field(object, "max_inclusive", &type_name)?)?,
        },
        "minecraft:trapezoid" => HeightProvider::Trapezoid {
            min_inclusive: parse_anchor_value(field(object, "min_inclusive", &type_name)?)?,
            max_inclusive: parse_anchor_value(field(object, "max_inclusive", &type_name)?)?,
            plateau: optional_int(object, "plateau", &type_name)?.unwrap_or(0),
        },
        "minecraft:very_biased_to_bottom" => HeightProvider::VeryBiasedToBottom {
            min_inclusive: parse_anchor_value(field(object, "min_inclusive", &type_name)?)?,
            max_inclusive: parse_anchor_value(field(object, "max_inclusive", &type_name)?)?,
            inner: optional_int(object, "inner", &type_name)?.unwrap_or(1),
        },
        _ => HeightProvider::Unsupported {
            vanilla_type: type_name,
        },
    })
}

fn parse_anchor_value(value: &Value) -> io::Result<VerticalAnchor> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("vertical anchor must be an object"))?;
    parse_anchor(object)
}

fn parse_anchor(object: &Map<String, Value>) -> io::Result<VerticalAnchor> {
    let mut anchors = object
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "absolute" | "above_bottom" | "below_top"));

    let Some((key, value)) = anchors.next() else {
        return Err(invalid(
            "vertical anchor needs `absolute`, `above_bottom`, or `below_top`",
        ));
    };
    if anchors.next().is_some() {
        return Err(invalid("vertical anchor must hold exactly one of its keys"));
    }
    let offset = value
        .as_i64()
        .and_then(|offset| i32::try_from(offset).ok())
        .ok_or_else(|| invalid("vertical anchor value must be an i32"))?;

    Ok(match key.as_str() {
        "absolute" => VerticalAnchor::Absolute(offset),
        "above_bottom" => VerticalAnchor::AboveBottom(offset),
        _ => VerticalAnchor::BelowTop(offset),
    })
}

fn parse_positions(object: &Map<String, Value>, type_name: &str) -> io::Result<Vec<BlockPos>> {
    let positions = field(object, "positions", type_name)?
        .as_array()
        .ok_or_else(|| invalid(format!("{type_name} needs a `positions` list")))?;

    let mut parsed = Vec::with_capacity(positions.len());
    for position in positions {
        let coordinates = position
            .as_array()
            .filter(|coordinates| coordinates.len() == 3)
            .ok_or_else(|| invalid(format!("{type_name} positions must be [x, y, z]")))?;
        let mut values = [0_i32; 3];
        for (index, coordinate) in coordinates.iter().enumerate() {
            values[index] = coordinate
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| invalid(format!("{type_name} coordinates must be i32")))?;
        }
        parsed.push(BlockPos::new(values[0], values[1], values[2]));
    }
    Ok(parsed)
}

fn field<'a>(object: &'a Map<String, Value>, key: &str, context: &str) -> io::Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| invalid(format!("{context} is missing `{key}`")))
}

fn int_field(object: &Map<String, Value>, key: &str, context: &str) -> io::Result<i32> {
    field(object, key, context)?
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| invalid(format!("{context} needs an i32 `{key}`")))
}

fn optional_int(object: &Map<String, Value>, key: &str, context: &str) -> io::Result<Option<i32>> {
    match object.get(key) {
        None => Ok(None),
        Some(_) => int_field(object, key, context).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use folia_random::LegacyRandomSource;
    use serde_json::json;
    use std::collections::VecDeque;

    /// Random source that serves scripted values and records requested bounds.
    struct ScriptedRandom {
        ints: VecDeque<i32>,
        floats: VecDeque<f32>,
        bounds: Vec<i32>,
    }

    impl ScriptedRandom {
        fn new(ints: impl IntoIterator<Item = i32>, floats: impl IntoIterator<Item = f32>) -> Self {
            Self {
                ints: ints.into_iter().collect(),
                floats: floats.into_iter().collect(),
                bounds: Vec::new(),
            }
        }
    }

    impl RandomSource for ScriptedRandom {
        fn next_int(&mut self) -> i32 {
            self.ints.pop_front().expect("unscripted next_int")
        }

        fn next_int_bound(&mut self, bound: i32) -> i32 {
            self.bounds.push(bound);
            self.ints.pop_front().expect("unscripted next_int_bound")
        }

        fn next_long(&mut self) -> i64 {
            panic!("unscripted next_long")
        }

        fn next_double(&mut self) -> f64 {
            panic!("unscripted next_double")
        }

        fn next_float(&mut self) -> f32 {
            self.floats.pop_front().expect("unscripted next_float")
        }

        fn next_boolean(&mut self) -> bool {
            panic!("unscripted next_boolean")
        }
    }

    fn parse(value: Value) -> PlacementModifier {
        parse_modifier(&value).expect("modifier should parse")
    }

    /// World stub: a single biome that lists exactly `features`.
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

    const TEST_WORLD: TestWorld = TestWorld {
        biome: "minecraft:plains",
        features: &["minecraft:test"],
    };

    fn positions(
        modifier: &PlacementModifier,
        pos: BlockPos,
        random: &mut ScriptedRandom,
    ) -> Vec<BlockPos> {
        modifier
            .get_positions(pos, random, &TEST_WORLD, "minecraft:test", -64, 384)
            .expect("supported modifier")
    }

    #[test]
    fn uniform_int_provider_uses_next_int_between_inclusive() {
        let provider = parse_int_provider(&json!({
            "type": "minecraft:uniform",
            "min_inclusive": 44,
            "max_inclusive": 52
        }))
        .unwrap();
        let mut random = ScriptedRandom::new([4], []);
        assert_eq!(provider.sample(&mut random).unwrap(), 48);
        assert_eq!(random.bounds, vec![9]);
    }

    #[test]
    fn biased_to_bottom_draws_outer_bound_then_inner() {
        let provider = parse_int_provider(&json!({
            "type": "minecraft:biased_to_bottom",
            "min_inclusive": 10,
            "max_inclusive": 20
        }))
        .unwrap();
        let mut random = ScriptedRandom::new([5, 2], []);
        assert_eq!(provider.sample(&mut random).unwrap(), 12);
        assert_eq!(random.bounds, vec![11, 6]);
    }

    #[test]
    fn clamped_provider_clamps_its_source() {
        let provider = parse_int_provider(&json!({
            "type": "minecraft:clamped",
            "source": {"type": "minecraft:uniform", "min_inclusive": -10, "max_inclusive": 10},
            "min_inclusive": 0,
            "max_inclusive": 5
        }))
        .unwrap();
        let mut high = ScriptedRandom::new([20], []);
        assert_eq!(provider.sample(&mut high).unwrap(), 5);
        let mut low = ScriptedRandom::new([0], []);
        assert_eq!(provider.sample(&mut low).unwrap(), 0);
    }

    #[test]
    fn weighted_list_scans_weights_in_declaration_order() {
        let provider = parse_int_provider(&json!({
            "type": "minecraft:weighted_list",
            "distribution": [
                {"data": 2, "weight": 9},
                {"data": 3, "weight": 1}
            ]
        }))
        .unwrap();
        // Index 8 lands in the 9-weight first entry, index 9 in the second.
        let mut first = ScriptedRandom::new([8], []);
        assert_eq!(provider.sample(&mut first).unwrap(), 2);
        assert_eq!(first.bounds, vec![10]);

        let mut second = ScriptedRandom::new([9], []);
        assert_eq!(provider.sample(&mut second).unwrap(), 3);
    }

    #[test]
    fn oversized_weighted_list_is_reported_as_unsupported() {
        let provider = parse_int_provider(&json!({
            "type": "minecraft:weighted_list",
            "distribution": [{"data": 1, "weight": 64}]
        }))
        .unwrap();
        assert!(!provider.is_supported());
        let mut random = ScriptedRandom::new([], []);
        let error = provider.sample(&mut random).unwrap_err();
        assert_eq!(error.vanilla_type, "minecraft:weighted_list");
    }

    #[test]
    fn vertical_anchors_resolve_against_build_limits() {
        assert_eq!(VerticalAnchor::Absolute(100).resolve_y(-64, 384), 100);
        assert_eq!(VerticalAnchor::AboveBottom(0).resolve_y(-64, 384), -64);
        assert_eq!(VerticalAnchor::AboveBottom(-32).resolve_y(-64, 384), -96);
        assert_eq!(VerticalAnchor::BelowTop(0).resolve_y(-64, 384), 319);
        assert_eq!(VerticalAnchor::BelowTop(8).resolve_y(-64, 384), 311);
    }

    #[test]
    fn height_uniform_samples_between_resolved_anchors() {
        let provider = parse_height_provider(&json!({
            "type": "minecraft:uniform",
            "min_inclusive": {"above_bottom": 0},
            "max_inclusive": {"below_top": 0}
        }))
        .unwrap();
        let mut random = ScriptedRandom::new([200], []);
        // Offset 200 inside -64..=319.
        assert_eq!(provider.sample(&mut random, -64, 384).unwrap(), 136);
        assert_eq!(random.bounds, vec![384], "range is -64..=319");
    }

    #[test]
    fn height_trapezoid_uses_two_draws_inside_the_plateau_gap() {
        let provider = parse_height_provider(&json!({
            "type": "minecraft:trapezoid",
            "min_inclusive": {"absolute": 0},
            "max_inclusive": {"absolute": 100}
        }))
        .unwrap();
        // span 100 -> low 50, high 50; 49+49+0
        let mut random = ScriptedRandom::new([49, 49], []);
        assert_eq!(provider.sample(&mut random, -64, 384).unwrap(), 98);
        assert_eq!(random.bounds, vec![51, 51]);
    }

    #[test]
    fn height_very_biased_to_bottom_matches_mth_next_int() {
        let provider = parse_height_provider(&json!({
            "type": "minecraft:very_biased_to_bottom",
            "min_inclusive": {"absolute": 0},
            "max_inclusive": {"absolute": 100},
            "inner": 8
        }))
        .unwrap();
        // Mth.nextInt(8, 100) -> 42; Mth.nextInt(0, 41) -> 10;
        // Mth.nextInt(0, 10 - 1 + 8 = 17) -> 3, so the result is 0 + 3.
        let mut random = ScriptedRandom::new([34, 10, 3], []);
        assert_eq!(provider.sample(&mut random, -64, 384).unwrap(), 3);
        assert_eq!(random.bounds, vec![93, 42, 18]);
    }

    #[test]
    fn empty_height_ranges_return_the_minimum() {
        let provider = parse_height_provider(&json!({
            "type": "minecraft:uniform",
            "min_inclusive": {"absolute": 50},
            "max_inclusive": {"absolute": 10}
        }))
        .unwrap();
        let mut random = ScriptedRandom::new([], []);
        assert_eq!(provider.sample(&mut random, -64, 384).unwrap(), 50);
        assert!(random.bounds.is_empty(), "no draw for an empty range");
    }

    #[test]
    fn count_repeats_the_position_and_consumes_one_draw() {
        let modifier = parse(json!({"type": "minecraft:count", "count": 3}));
        let mut random = ScriptedRandom::new([], []);
        assert_eq!(
            positions(&modifier, BlockPos::new(1, 2, 3), &mut random),
            vec![BlockPos::new(1, 2, 3); 3]
        );
        assert!(random.bounds.is_empty(), "constant counts draw nothing");
    }

    #[test]
    fn in_square_offsets_x_then_z() {
        let modifier = parse(json!({"type": "minecraft:in_square"}));
        let mut random = ScriptedRandom::new([7, 15], []);
        assert_eq!(
            positions(&modifier, BlockPos::new(0, 5, 0), &mut random),
            vec![BlockPos::new(7, 5, 15)]
        );
        assert_eq!(random.bounds, vec![16, 16]);
    }

    #[test]
    fn rarity_filter_keeps_values_strictly_below_one_over_chance() {
        let modifier = parse(json!({"type": "minecraft:rarity_filter", "chance": 2}));
        let mut random = ScriptedRandom::new([], [0.5, 0.4999]);
        assert!(positions(&modifier, BlockPos::ZERO, &mut random).is_empty());
        assert_eq!(
            positions(&modifier, BlockPos::ZERO, &mut random),
            vec![BlockPos::ZERO]
        );
    }

    #[test]
    fn fixed_placement_keeps_positions_in_the_origin_chunk() {
        let modifier = parse(json!({
            "type": "minecraft:fixed_placement",
            "positions": [[0, 10, 0], [20, 10, 0]]
        }));
        let mut random = ScriptedRandom::new([], []);
        assert_eq!(
            positions(&modifier, BlockPos::new(5, 10, 5), &mut random),
            vec![BlockPos::new(0, 10, 0)]
        );
        assert_eq!(
            positions(&modifier, BlockPos::new(25, 10, 5), &mut random),
            vec![BlockPos::new(20, 10, 0)]
        );
        // Chunk boundaries are compared per axis: z = -1 is chunk -1, not 0.
        assert!(
            positions(&modifier, BlockPos::new(25, 10, -1), &mut random).is_empty(),
            "position 20,10,0 lies in a different z chunk"
        );
    }

    #[test]
    fn modifier_chain_draws_in_vanilla_order() {
        let modifier = parse(json!({
            "type": "minecraft:random_offset",
            "xz_spread": {"type": "minecraft:uniform", "min_inclusive": 0, "max_inclusive": 4},
            "y_spread": -1
        }));
        let mut random = ScriptedRandom::new([3, 0], []);
        assert_eq!(
            positions(&modifier, BlockPos::new(100, 64, 100), &mut random),
            vec![BlockPos::new(103, 63, 100)]
        );
        assert_eq!(
            random.bounds,
            vec![5, 5],
            "xz drawn for x, then again for z"
        );
    }

    #[test]
    fn unported_modifier_fails_loudly() {
        let modifier =
            parse(json!({"type": "minecraft:heightmap", "heightmap": "MOTION_BLOCKING"}));
        let mut random = ScriptedRandom::new([], []);
        let error = modifier
            .get_positions(
                BlockPos::ZERO,
                &mut random,
                &TEST_WORLD,
                "minecraft:test",
                -64,
                384,
            )
            .unwrap_err();
        assert_eq!(error.vanilla_type, "minecraft:heightmap");
        assert!(!modifier.is_supported());
    }

    #[test]
    fn biome_filter_checks_the_biomes_feature_list_and_draws_nothing() {
        let modifier = parse(json!({"type": "minecraft:biome"}));
        let mut random = ScriptedRandom::new([], []);

        assert_eq!(
            modifier
                .get_positions(
                    BlockPos::new(1, 2, 3),
                    &mut random,
                    &TEST_WORLD,
                    "minecraft:test",
                    -64,
                    384,
                )
                .unwrap(),
            vec![BlockPos::new(1, 2, 3)],
            "the biome lists the placed feature"
        );

        let other = TestWorld {
            biome: "minecraft:desert",
            features: &["minecraft:desert_well"],
        };
        assert!(
            modifier
                .get_positions(
                    BlockPos::new(1, 2, 3),
                    &mut random,
                    &other,
                    "minecraft:test",
                    -64,
                    384,
                )
                .unwrap()
                .is_empty(),
            "the desert does not list minecraft:test"
        );
        assert!(
            random.bounds.is_empty() && random.ints.is_empty() && random.floats.is_empty(),
            "a biome filter consumes no random draws"
        );
        assert!(modifier.is_supported());
    }

    #[test]
    fn placement_replays_deterministically_with_a_seeded_source() {
        let modifier = parse(json!({"type": "minecraft:in_square"}));
        let sample = |seed: i64| {
            let mut random = LegacyRandomSource::new(seed);
            let mut draws = Vec::new();
            for _ in 0..8 {
                draws.push(
                    modifier
                        .get_positions(
                            BlockPos::ZERO,
                            &mut random,
                            &TEST_WORLD,
                            "minecraft:test",
                            -64,
                            384,
                        )
                        .unwrap(),
                );
            }
            draws
        };
        assert_eq!(sample(42), sample(42));
        assert_ne!(sample(1), sample(2));
    }

    #[test]
    fn malformed_modifiers_are_rejected_at_parse_time() {
        assert!(parse_modifier(&json!({"type": "minecraft:rarity_filter", "chance": 0})).is_err());
        assert!(parse_modifier(&json!({"type": "minecraft:count"})).is_err());
        assert!(
            parse_modifier(&json!({
                "type": "minecraft:fixed_placement",
                "positions": [[0, 1]]
            }))
            .is_err()
        );
        assert!(
            parse_modifier(&json!({
                "type": "minecraft:height_range",
                "height": {"type": "minecraft:uniform", "min_inclusive": {"absolute": 1}}
            }))
            .is_err()
        );
        assert!(
            parse_modifier(&json!({
                "type": "minecraft:count",
                "count": {
                    "type": "minecraft:weighted_list",
                    "distribution": [{"data": 1, "weight": 0}]
                }
            }))
            .is_err()
        );
    }
}
