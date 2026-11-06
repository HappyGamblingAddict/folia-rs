//! `minecraft:tree` for straight, dark oak, forking, fancy, bending, giant,
//! mega jungle, cherry, and upwards-branching mangrove trunks with blob, bush,
//! random-spread, spruce, pine, dark oak, acacia, fancy, jungle, mega-pine, and
//! cherry foliage placers. Mangrove roots use the mangrove root placer.
//!
//! Tree decorators execute after the trunk and foliage: leaf litter, beehives,
//! hanging vines, trunk vines, cocoa, alter-ground, attached leaves/logs,
//! creaking hearts, and pale moss. The pale-moss ground pass delegates to the
//! embedded `minecraft:pale_moss_patch` vegetation feature. Configurations
//! that use any other trunk, foliage, or root placer or decorator load as
//! unsupported instead of being approximated.

use super::placement::{IntProvider, parse_block_state, parse_int_provider};
use super::{ConfiguredFeature, FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::carver::{mth_cos, mth_sin};
use folia_math::BlockPos;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io;

/// A parsed `minecraft:tree` configuration with supported placers.
#[derive(Debug, Clone)]
pub struct TreeFeature {
    trunk_provider: BlockState,
    foliage_provider: BlockStateProvider,
    dirt_provider: BlockState,
    force_dirt: bool,
    ignore_vines: bool,
    trunk_placer: TrunkPlacer,
    root_placer: Option<RootPlacer>,
    base_height: i32,
    height_rand_a: i32,
    height_rand_b: i32,
    foliage_placer: FoliagePlacer,
    foliage_radius: IntProvider,
    foliage_offset: IntProvider,
    size_limit: i32,
    size_lower: i32,
    size_middle: i32,
    size_upper_limit: Option<i32>,
    size_upper: i32,
    min_clipped_height: Option<i32>,
    /// Parsed decorators. An unsupported decorator type makes the whole tree
    /// configuration unsupported instead of silently skipping decoration.
    decorators: Vec<TreeDecorator>,
    pub decorator_count: usize,
}

/// A parsed `minecraft:fallen_tree` configuration.
#[derive(Debug, Clone)]
pub struct FallenTreeFeature {
    trunk_provider: BlockStateProvider,
    log_length: IntProvider,
    stump_decorators: Vec<TreeDecorator>,
    log_decorators: Vec<TreeDecorator>,
}

#[derive(Debug, Clone)]
enum TrunkPlacer {
    Straight,
    DarkOak,
    Forking,
    Fancy,
    Bending {
        min_height_for_leaves: i32,
        bend_length: IntProvider,
    },
    UpwardsBranching {
        extra_branch_steps: IntProvider,
        place_branch_per_log_probability: f32,
        extra_branch_length: IntProvider,
        can_grow_through: BlockSet,
    },
    Cherry {
        branch_count: IntProvider,
        branch_horizontal_length: IntProvider,
        branch_start_offset_from_top: IntProvider,
        second_branch_start_offset_from_top: IntProvider,
        branch_end_offset_from_top: IntProvider,
    },
    Giant,
    MegaJungle,
}

#[derive(Debug, Clone)]
enum FoliagePlacer {
    Blob {
        height: i32,
    },
    Bush {
        height: i32,
    },
    RandomSpread {
        foliage_height: IntProvider,
        leaf_placement_attempts: i32,
    },
    Spruce {
        trunk_height: IntProvider,
    },
    Pine {
        height: IntProvider,
    },
    DarkOak,
    Acacia,
    Fancy {
        height: i32,
    },
    MegaJungle {
        height: i32,
    },
    MegaPine {
        crown_height: IntProvider,
    },
    Cherry {
        height: IntProvider,
        wide_bottom_layer_hole_chance: f32,
        corner_hole_chance: f32,
        hanging_leaves_chance: f32,
        hanging_leaves_extension_chance: f32,
    },
}

#[derive(Debug, Clone)]
struct FoliageAttachment {
    pos: BlockPos,
    radius_offset: i32,
    double_trunk: bool,
}

/// The two mangrove growth tags, resolved from the 1.21.11 block-tag data.
/// Explicit block lists in a config are kept as supplied.
#[derive(Debug, Clone)]
enum BlockSet {
    MangroveRootsCanGrowThrough,
    MangroveLogsCanGrowThrough,
    Explicit(HashSet<String>),
}

impl BlockSet {
    fn contains(&self, state: &BlockState) -> bool {
        match self {
            Self::MangroveRootsCanGrowThrough => {
                MANGROVE_ROOTS_CAN_GROW_THROUGH.contains(&state.name.as_str())
            }
            Self::MangroveLogsCanGrowThrough => {
                MANGROVE_LOGS_CAN_GROW_THROUGH.contains(&state.name.as_str())
            }
            Self::Explicit(names) => names.contains(&state.name),
        }
    }
}

const MANGROVE_ROOTS_CAN_GROW_THROUGH: &[&str] = &[
    "minecraft:mud",
    "minecraft:muddy_mangrove_roots",
    "minecraft:mangrove_roots",
    "minecraft:moss_carpet",
    "minecraft:vine",
    "minecraft:mangrove_propagule",
    "minecraft:snow",
];

const MANGROVE_LOGS_CAN_GROW_THROUGH: &[&str] = &[
    "minecraft:mud",
    "minecraft:muddy_mangrove_roots",
    "minecraft:mangrove_roots",
    "minecraft:mangrove_leaves",
    "minecraft:mangrove_log",
    "minecraft:mangrove_propagule",
    "minecraft:moss_carpet",
    "minecraft:vine",
];

#[derive(Debug, Clone)]
struct AboveRootPlacement {
    above_root_provider: BlockStateProvider,
    above_root_placement_chance: f32,
}

#[derive(Debug, Clone)]
struct MangroveRootPlacement {
    can_grow_through: BlockSet,
    muddy_roots_in: HashSet<String>,
    muddy_roots_provider: BlockStateProvider,
    max_root_width: i32,
    max_root_length: i32,
    random_skew_chance: f32,
}

#[derive(Debug, Clone)]
struct RootPlacer {
    trunk_offset_y: IntProvider,
    root_provider: BlockStateProvider,
    above_root_placement: Option<AboveRootPlacement>,
    mangrove: MangroveRootPlacement,
}

#[derive(Debug, Clone)]
enum TreeDecorator {
    PlaceOnGround(PlaceOnGroundDecorator),
    Beehive(BeehiveDecorator),
    LeaveVine(LeaveVineDecorator),
    TrunkVine,
    Cocoa(CocoaDecorator),
    AlterGround(AlterGroundDecorator),
    AttachedToLeaves(AttachedToLeavesDecorator),
    AttachedToLogs(AttachedToLogsDecorator),
    CreakingHeart(CreakingHeartDecorator),
    PaleMoss(PaleMossDecorator),
}

#[derive(Debug, Clone)]
struct PaleMossDecorator {
    leaves_probability: f32,
    trunk_probability: f32,
    ground_probability: f32,
    patch: Box<ConfiguredFeature>,
}

#[derive(Debug, Clone)]
struct BeehiveDecorator {
    probability: f32,
}

#[derive(Debug, Clone, Copy)]
struct LeaveVineDecorator {
    probability: f32,
}

#[derive(Debug, Clone, Copy)]
struct CocoaDecorator {
    probability: f32,
}

#[derive(Debug, Clone)]
struct PlaceOnGroundDecorator {
    tries: i32,
    radius: i32,
    height: i32,
    block_state_provider: BlockStateProvider,
}

#[derive(Debug, Clone)]
struct AlterGroundDecorator {
    provider: BlockStateProvider,
}

#[derive(Debug, Clone)]
struct AttachedToLeavesDecorator {
    probability: f32,
    exclusion_radius_xz: i32,
    exclusion_radius_y: i32,
    block_provider: BlockStateProvider,
    required_empty_blocks: i32,
    directions: Vec<(i32, i32, i32)>,
}

#[derive(Debug, Clone)]
struct AttachedToLogsDecorator {
    probability: f32,
    block_provider: BlockStateProvider,
    directions: Vec<(i32, i32, i32)>,
}

#[derive(Debug, Clone, Copy)]
struct CreakingHeartDecorator {
    probability: f32,
}

/// A parsed `BlockStateProvider`; simple providers have one entry of weight 1.
#[derive(Debug, Clone)]
enum BlockStateProvider {
    Weighted {
        entries: Vec<(i32, BlockState)>,
        total_weight: i32,
    },
    RandomizedInt {
        source: Box<BlockStateProvider>,
        property: String,
        values: IntProvider,
    },
}

impl BlockStateProvider {
    fn simple(state: BlockState) -> Self {
        Self::Weighted {
            entries: vec![(1, state)],
            total_weight: 1,
        }
    }

    /// `WeightedList.getRandomOrThrow` with the compact selector's draw order;
    /// `RandomizedIntStateProvider` samples its source before its property value.
    fn sample(&self, random: &mut dyn RandomSource) -> Option<BlockState> {
        match self {
            Self::Weighted {
                entries,
                total_weight,
            } => {
                let mut roll = random.next_int_bound(*total_weight);
                for (weight, state) in entries {
                    if roll < *weight {
                        return Some(state.clone());
                    }
                    roll -= *weight;
                }
                None
            }
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let mut state = source.sample(random)?;
                let Some(value) = state
                    .properties
                    .iter_mut()
                    .find(|(name, _)| name == property)
                    .map(|(_, value)| value)
                else {
                    return Some(state);
                };
                *value = values.sample(random).ok()?.to_string();
                Some(state)
            }
        }
    }
}

/// Parses a `TreeConfiguration`. `Ok(None)` means a known vanilla component is
/// not ported yet; malformed JSON is an error.
#[cfg(test)]
pub fn parse_tree_config(config: &Value) -> io::Result<Option<TreeFeature>> {
    parse_tree_config_with_pale_moss(config, None)
}

/// Parses a tree with the configured `minecraft:pale_moss_patch` available to
/// the `minecraft:pale_moss` decorator. The patch is embedded so placement
/// needs no registry lookup at generation time.
pub(crate) fn parse_tree_config_with_pale_moss(
    config: &Value,
    pale_moss_patch: Option<&ConfiguredFeature>,
) -> io::Result<Option<TreeFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("tree config must be an object"))?;

    let trunk = component(object, "trunk_placer")?;
    let trunk_placer = match trunk.get("type").and_then(Value::as_str) {
        Some("minecraft:straight_trunk_placer") => TrunkPlacer::Straight,
        Some("minecraft:dark_oak_trunk_placer") => TrunkPlacer::DarkOak,
        Some("minecraft:forking_trunk_placer") => TrunkPlacer::Forking,
        Some("minecraft:fancy_trunk_placer") => TrunkPlacer::Fancy,
        Some("minecraft:bending_trunk_placer") => {
            let min_height_for_leaves = optional_int(trunk, "min_height_for_leaves", 1)?;
            if min_height_for_leaves <= 0 {
                return Err(invalid(
                    "bending trunk min_height_for_leaves must be positive",
                ));
            }
            let bend_length = parse_int_provider(
                trunk
                    .get("bend_length")
                    .ok_or_else(|| invalid("bending trunk is missing `bend_length`"))?,
            )?;
            if !bend_length.is_supported() {
                return Ok(None);
            }
            TrunkPlacer::Bending {
                min_height_for_leaves,
                bend_length,
            }
        }
        Some("minecraft:upwards_branching_trunk_placer") => {
            let extra_branch_steps =
                parse_int_provider(trunk.get("extra_branch_steps").ok_or_else(|| {
                    invalid("upwards branching trunk is missing `extra_branch_steps`")
                })?)?;
            let extra_branch_length =
                parse_int_provider(trunk.get("extra_branch_length").ok_or_else(|| {
                    invalid("upwards branching trunk is missing `extra_branch_length`")
                })?)?;
            if !extra_branch_steps.is_supported() || !extra_branch_length.is_supported() {
                return Ok(None);
            }
            let can_grow_through =
                match parse_block_set(trunk.get("can_grow_through").ok_or_else(|| {
                    invalid("upwards branching trunk is missing `can_grow_through`")
                })?)? {
                    Some(set) => set,
                    None => return Ok(None),
                };
            TrunkPlacer::UpwardsBranching {
                extra_branch_steps,
                place_branch_per_log_probability: parse_chance(
                    trunk,
                    "place_branch_per_log_probability",
                    "upwards branching trunk",
                )?,
                extra_branch_length,
                can_grow_through,
            }
        }
        Some("minecraft:giant_trunk_placer") => TrunkPlacer::Giant,
        Some("minecraft:mega_jungle_trunk_placer") => TrunkPlacer::MegaJungle,
        Some("minecraft:cherry_trunk_placer") => {
            let branch_count = parse_int_provider(
                trunk
                    .get("branch_count")
                    .ok_or_else(|| invalid("cherry trunk is missing `branch_count`"))?,
            )?;
            let branch_horizontal_length =
                parse_int_provider(trunk.get("branch_horizontal_length").ok_or_else(|| {
                    invalid("cherry trunk is missing `branch_horizontal_length`")
                })?)?;
            let branch_start_offset_from_top =
                parse_uniform_int(trunk.get("branch_start_offset_from_top").ok_or_else(|| {
                    invalid("cherry trunk is missing `branch_start_offset_from_top`")
                })?)?;
            let (start_min, start_max) = match &branch_start_offset_from_top {
                IntProvider::Uniform {
                    min_inclusive,
                    max_inclusive,
                } => (*min_inclusive, *max_inclusive),
                _ => return Ok(None),
            };
            if start_max - start_min < 1 {
                return Err(invalid("cherry branch starts need at least two values"));
            }
            let second_branch_start_offset_from_top = IntProvider::Uniform {
                min_inclusive: start_min,
                max_inclusive: start_max - 1,
            };
            let branch_end_offset_from_top =
                parse_int_provider(trunk.get("branch_end_offset_from_top").ok_or_else(|| {
                    invalid("cherry trunk is missing `branch_end_offset_from_top`")
                })?)?;
            if !branch_count.is_supported()
                || !branch_horizontal_length.is_supported()
                || !branch_end_offset_from_top.is_supported()
            {
                return Ok(None);
            }
            TrunkPlacer::Cherry {
                branch_count,
                branch_horizontal_length,
                branch_start_offset_from_top,
                second_branch_start_offset_from_top,
                branch_end_offset_from_top,
            }
        }
        _ => return Ok(None),
    };
    let foliage = component(object, "foliage_placer")?;
    let foliage_kind = foliage
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("foliage placer is missing `type`"))?;

    let Some(trunk_provider) = simple_state(object, "trunk_provider")? else {
        return Ok(None);
    };
    let foliage_provider_value = object
        .get("foliage_provider")
        .ok_or_else(|| invalid("tree config is missing the `foliage_provider` provider"))?;
    let Some(foliage_provider) = parse_state_provider(foliage_provider_value)? else {
        return Ok(None);
    };
    let Some(dirt_provider) = simple_state(object, "dirt_provider")? else {
        return Ok(None);
    };
    let minimum_size = component(object, "minimum_size")?;
    let min_clipped_height = match minimum_size.get("min_clipped_height") {
        None => None,
        Some(_) => {
            let height = int(minimum_size, "min_clipped_height")?;
            if !(0..=80).contains(&height) {
                return Err(invalid("min_clipped_height must be between 0 and 80"));
            }
            Some(height)
        }
    };
    let size_limit = optional_int(minimum_size, "limit", 1)?;
    let size_lower = optional_int(minimum_size, "lower_size", 0)?;
    let (size_middle, size_upper_limit, size_upper) =
        match minimum_size.get("type").and_then(Value::as_str) {
            Some("minecraft:two_layers_feature_size") => {
                (0, None, optional_int(minimum_size, "upper_size", 1)?)
            }
            Some("minecraft:three_layers_feature_size") => (
                optional_int(minimum_size, "middle_size", 0)?,
                Some(optional_int(minimum_size, "upper_limit", 0)?),
                optional_int(minimum_size, "upper_size", 1)?,
            ),
            _ => return Ok(None),
        };

    let root_placer = match object.get("root_placer") {
        None => None,
        Some(value) => match parse_root_placer(value)? {
            Some(placer) => Some(placer),
            None => return Ok(None),
        },
    };

    let radius = parse_int_provider(
        foliage
            .get("radius")
            .ok_or_else(|| invalid("foliage placer is missing `radius`"))?,
    )?;
    let offset = parse_int_provider(
        foliage
            .get("offset")
            .ok_or_else(|| invalid("foliage placer is missing `offset`"))?,
    )?;
    if !radius.is_supported() || !offset.is_supported() {
        return Ok(None);
    }

    let foliage_placer = match foliage_kind {
        "minecraft:blob_foliage_placer" => FoliagePlacer::Blob {
            height: int(foliage, "height")?,
        },
        "minecraft:bush_foliage_placer" => FoliagePlacer::Bush {
            height: int(foliage, "height")?,
        },
        "minecraft:random_spread_foliage_placer" => {
            let foliage_height =
                parse_int_provider(foliage.get("foliage_height").ok_or_else(|| {
                    invalid("random spread foliage placer is missing `foliage_height`")
                })?)?;
            if !foliage_height.is_supported() {
                return Ok(None);
            }
            let leaf_placement_attempts = int(foliage, "leaf_placement_attempts")?;
            if !(0..=256).contains(&leaf_placement_attempts) {
                return Err(invalid(
                    "random spread leaf placement attempts must be between 0 and 256",
                ));
            }
            FoliagePlacer::RandomSpread {
                foliage_height,
                leaf_placement_attempts,
            }
        }
        "minecraft:dark_oak_foliage_placer" => FoliagePlacer::DarkOak,
        "minecraft:acacia_foliage_placer" => FoliagePlacer::Acacia,
        "minecraft:fancy_foliage_placer" => FoliagePlacer::Fancy {
            height: int(foliage, "height")?,
        },
        "minecraft:jungle_foliage_placer" => FoliagePlacer::MegaJungle {
            height: int(foliage, "height")?,
        },
        "minecraft:cherry_foliage_placer" => {
            let height = parse_int_provider(
                foliage
                    .get("height")
                    .ok_or_else(|| invalid("cherry foliage placer is missing `height`"))?,
            )?;
            if !height.is_supported() {
                return Ok(None);
            }
            FoliagePlacer::Cherry {
                height,
                wide_bottom_layer_hole_chance: parse_chance(
                    foliage,
                    "wide_bottom_layer_hole_chance",
                    "cherry foliage",
                )?,
                corner_hole_chance: parse_chance(foliage, "corner_hole_chance", "cherry foliage")?,
                hanging_leaves_chance: parse_chance(
                    foliage,
                    "hanging_leaves_chance",
                    "cherry foliage",
                )?,
                hanging_leaves_extension_chance: parse_chance(
                    foliage,
                    "hanging_leaves_extension_chance",
                    "cherry foliage",
                )?,
            }
        }
        "minecraft:mega_pine_foliage_placer" => {
            let crown_height =
                parse_int_provider(foliage.get("crown_height").ok_or_else(|| {
                    invalid("mega pine foliage placer is missing `crown_height`")
                })?)?;
            if !crown_height.is_supported() {
                return Ok(None);
            }
            FoliagePlacer::MegaPine { crown_height }
        }
        "minecraft:spruce_foliage_placer" => {
            let trunk_height = parse_int_provider(
                foliage
                    .get("trunk_height")
                    .ok_or_else(|| invalid("spruce foliage placer is missing `trunk_height`"))?,
            )?;
            if !trunk_height.is_supported() {
                return Ok(None);
            }
            FoliagePlacer::Spruce { trunk_height }
        }
        "minecraft:pine_foliage_placer" => {
            let height = parse_int_provider(
                foliage
                    .get("height")
                    .ok_or_else(|| invalid("pine foliage placer is missing `height`"))?,
            )?;
            if !height.is_supported() {
                return Ok(None);
            }
            FoliagePlacer::Pine { height }
        }
        _ => return Ok(None),
    };

    let Some(decorators) = parse_decorators(object, pale_moss_patch)? else {
        return Ok(None);
    };
    let decorator_count = decorators.len();

    Ok(Some(TreeFeature {
        trunk_provider,
        foliage_provider,
        dirt_provider,
        force_dirt: object
            .get("force_dirt")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ignore_vines: object
            .get("ignore_vines")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        trunk_placer,
        root_placer,
        base_height: int(trunk, "base_height")?,
        height_rand_a: int(trunk, "height_rand_a")?,
        height_rand_b: int(trunk, "height_rand_b")?,
        foliage_placer,
        foliage_radius: radius,
        foliage_offset: offset,
        size_limit,
        size_lower,
        size_middle,
        size_upper_limit,
        size_upper,
        min_clipped_height,
        decorators,
        decorator_count,
    }))
}

impl FoliagePlacer {
    /// `FoliagePlacer.foliageHeight`, including the spruce and pine overrides.
    fn foliage_height(&self, random: &mut dyn RandomSource, tree_height: i32) -> Option<i32> {
        match self {
            FoliagePlacer::Blob { height } => Some(*height),
            FoliagePlacer::Bush { height } => Some(*height),
            FoliagePlacer::RandomSpread { foliage_height, .. } => {
                foliage_height.sample(random).ok()
            }
            FoliagePlacer::Spruce { trunk_height } => {
                Some((tree_height - trunk_height.sample(random).ok()?).max(4))
            }
            FoliagePlacer::Pine { height } => height.sample(random).ok(),
            FoliagePlacer::DarkOak => Some(4),
            FoliagePlacer::Acacia => Some(0),
            FoliagePlacer::Fancy { height } => Some(*height),
            FoliagePlacer::MegaJungle { height } => Some(*height),
            FoliagePlacer::MegaPine { crown_height } => crown_height.sample(random).ok(),
            FoliagePlacer::Cherry { height, .. } => height.sample(random).ok(),
        }
    }

    /// `FoliagePlacer.foliageRadius`; pine adds a height-derived random offset.
    fn foliage_radius(
        &self,
        random: &mut dyn RandomSource,
        radius_argument: i32,
        radius: &IntProvider,
    ) -> Option<i32> {
        let radius = radius.sample(random).ok()?;
        match self {
            FoliagePlacer::Pine { .. } => {
                Some(radius + random.next_int_bound((radius_argument + 1).max(1)))
            }
            _ => Some(radius),
        }
    }
}

impl TreeFeature {
    /// `TreeFeature.doPlace` for the supported placers.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let tree_height = self.base_height
            + random.next_int_bound(self.height_rand_a + 1)
            + random.next_int_bound(self.height_rand_b + 1);
        let Some(foliage_height) = self.foliage_placer.foliage_height(random, tree_height) else {
            return false;
        };
        let radius_argument = tree_height - foliage_height;
        let Some(foliage_radius) =
            self.foliage_placer
                .foliage_radius(random, radius_argument, &self.foliage_radius)
        else {
            return false;
        };

        let trunk_origin = match &self.root_placer {
            Some(placer) => match placer.get_trunk_origin(origin, random) {
                Some(origin) => origin,
                None => return false,
            },
            None => origin,
        };
        let max_y = level.min_y() + level.height() - 1;
        let min_block_y = origin.y().min(trunk_origin.y());
        let top = origin
            .y()
            .max(trunk_origin.y())
            .wrapping_add(tree_height)
            .wrapping_add(1);
        if min_block_y < level.min_y() + 1 || top > max_y + 1 {
            return false;
        }

        let max_free_height = self.max_free_tree_height(level, tree_height, trunk_origin);
        if max_free_height < tree_height {
            let Some(min_clipped_height) = self.min_clipped_height else {
                return false;
            };
            if max_free_height < min_clipped_height {
                return false;
            }
        }

        if let Some(placer) = &self.root_placer
            && !placer.place_roots(level, random, origin, trunk_origin)
        {
            return false;
        }

        let origin = trunk_origin;
        let mut logs = Vec::new();
        let mut attachments = Vec::new();
        match &self.trunk_placer {
            TrunkPlacer::Straight => self.place_straight_trunk(
                level,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::DarkOak => self.place_dark_oak_trunk(
                level,
                random,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::Forking => self.place_forking_trunk(
                level,
                random,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::Fancy => self.place_fancy_trunk(
                level,
                random,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::Bending {
                min_height_for_leaves,
                bend_length,
            } => self.place_bending_trunk(
                level,
                random,
                origin,
                max_free_height,
                *min_height_for_leaves,
                bend_length,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::UpwardsBranching {
                extra_branch_steps,
                place_branch_per_log_probability,
                extra_branch_length,
                can_grow_through,
            } => self.place_upwards_branching_trunk(
                level,
                random,
                origin,
                max_free_height,
                extra_branch_steps,
                *place_branch_per_log_probability,
                extra_branch_length,
                can_grow_through,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::Giant => {
                self.place_giant_trunk(level, origin, max_free_height, &mut logs, &mut attachments)
            }
            TrunkPlacer::MegaJungle => self.place_mega_jungle_trunk(
                level,
                random,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
            ),
            TrunkPlacer::Cherry {
                branch_count,
                branch_horizontal_length,
                branch_start_offset_from_top,
                second_branch_start_offset_from_top,
                branch_end_offset_from_top,
            } => self.place_cherry_trunk(
                level,
                random,
                origin,
                max_free_height,
                &mut logs,
                &mut attachments,
                branch_count,
                branch_horizontal_length,
                branch_start_offset_from_top,
                second_branch_start_offset_from_top,
                branch_end_offset_from_top,
            ),
        }

        let mut leaves = Vec::new();
        let mut placed = !logs.is_empty();
        for attachment in &attachments {
            if self.create_foliage(
                level,
                random,
                attachment,
                (foliage_height, foliage_radius),
                &mut leaves,
            ) {
                placed = true;
            }
        }

        if placed && (!logs.is_empty() || !leaves.is_empty()) {
            // `TreeDecorator.Context` sorts both collections by Y before any
            // decorator runs; beehive placement depends on the first/last log.
            logs.sort_by_key(|pos| pos.y());
            leaves.sort_by_key(|pos| pos.y());
            for decorator in &self.decorators {
                decorator.place(level, random, &logs, &leaves);
            }
        }
        placed
    }

    /// `TreeFeature.getMaxFreeTreeHeight`.
    fn max_free_tree_height(
        &self,
        level: &dyn FeatureLevel,
        trunk_height: i32,
        top: BlockPos,
    ) -> i32 {
        for index in 0..=trunk_height + 1 {
            let size = self.size_at_height(trunk_height, index);

            for dx in -size..=size {
                for dz in -size..=size {
                    let pos = top.offset(dx, index, dz);
                    let free = level
                        .get_block(pos)
                        .is_none_or(|state| self.trunk_is_free_state(state));
                    let vine = !self.ignore_vines
                        && level
                            .get_block(pos)
                            .is_some_and(|state| state.name == "minecraft:vine");
                    if !free || vine {
                        return index - 2;
                    }
                }
            }
        }
        trunk_height
    }

    /// `TrunkPlacer.isFree` with the upwards-trunk `validTreePos` override.
    fn trunk_is_free_state(&self, state: &BlockState) -> bool {
        if valid_tree_pos_state(state) || is_log(state) {
            return true;
        }
        matches!(&self.trunk_placer, TrunkPlacer::UpwardsBranching { can_grow_through, .. } if can_grow_through.contains(state))
    }

    /// `TrunkPlacer.setDirtAt`.
    fn set_dirt(&self, level: &mut dyn FeatureLevel, pos: BlockPos) {
        let replace = match level.get_block(pos) {
            Some(state) => {
                self.force_dirt
                    || !is_dirt(state)
                    || state.name == "minecraft:grass_block"
                    || state.name == "minecraft:mycelium"
            }
            None => false,
        };
        if replace {
            level.set_block(pos, self.dirt_provider.clone());
        }
    }

    /// `FeatureSize.getSizeAtHeight`.
    fn size_at_height(&self, trunk_height: i32, index: i32) -> i32 {
        if index < self.size_limit {
            self.size_lower
        } else if let Some(upper_limit) = self.size_upper_limit {
            if index >= trunk_height - upper_limit {
                self.size_upper
            } else {
                self.size_middle
            }
        } else {
            self.size_upper
        }
    }

    fn place_log(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        logs: &mut Vec<BlockPos>,
    ) -> bool {
        if valid_tree_pos(level, pos) && level.set_block(pos, self.trunk_provider.clone()) {
            logs.push(pos);
            true
        } else {
            false
        }
    }

    /// `TrunkPlacer.placeLogIfFree`.
    fn place_log_if_free(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        logs: &mut Vec<BlockPos>,
    ) {
        if is_tree_free(level, pos) {
            self.place_log(level, pos, logs);
        }
    }

    /// `TrunkPlacer.placeLog` with a `RotatedPillarBlock.AXIS` property setter.
    fn place_log_axis(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        logs: &mut Vec<BlockPos>,
        axis: &str,
    ) -> bool {
        if !valid_tree_pos(level, pos) {
            return false;
        }
        let mut state = self.trunk_provider.clone();
        if let Some((_, value)) = state
            .properties
            .iter_mut()
            .find(|(property, _)| property == "axis")
        {
            *value = axis.to_owned();
        }
        if level.set_block(pos, state) {
            logs.push(pos);
            true
        } else {
            false
        }
    }

    /// `StraightTrunkPlacer.placeTrunk`.
    fn place_straight_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        self.set_dirt(level, origin.below());
        for index in 0..free_height {
            self.place_log(level, origin.above_n(index), logs);
        }
        attachments.push(FoliageAttachment {
            pos: origin.above_n(free_height),
            radius_offset: 0,
            double_trunk: false,
        });
    }

    /// `BendingTrunkPlacer.placeTrunk`.
    #[allow(clippy::too_many_arguments)]
    fn place_bending_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        min_height_for_leaves: i32,
        bend_length: &IntProvider,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        self.set_dirt(level, origin.below());
        let directions = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        let (bend_x, bend_z) = directions[random.next_int_bound(4) as usize];
        let last = free_height - 1;
        let mut x = origin.x();
        let mut z = origin.z();

        for index in 0..=last {
            if index + 1 >= last + random.next_int_bound(2) {
                x += bend_x;
                z += bend_z;
            }
            let pos = BlockPos::new(x, origin.y() + index, z);
            self.place_log(level, pos, logs);
            if index >= min_height_for_leaves {
                attachments.push(FoliageAttachment {
                    pos,
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }

        let Ok(bend) = bend_length.sample(random) else {
            return;
        };
        for _ in 0..=bend {
            let pos = BlockPos::new(x, origin.y() + free_height, z);
            self.place_log(level, pos, logs);
            attachments.push(FoliageAttachment {
                pos,
                radius_offset: 0,
                double_trunk: false,
            });
            x += bend_x;
            z += bend_z;
        }
    }

    /// `UpwardsBranchingTrunkPlacer.placeTrunk`.
    #[allow(clippy::too_many_arguments)]
    fn place_upwards_branching_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        extra_branch_steps: &IntProvider,
        place_branch_per_log_probability: f32,
        extra_branch_length: &IntProvider,
        can_grow_through: &BlockSet,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        let directions = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        for index in 0..free_height {
            let y = origin.y() + index;
            let pos = BlockPos::new(origin.x(), y, origin.z());
            if self.place_log_through(level, pos, logs, can_grow_through)
                && index < free_height - 1
                && random.next_float() < place_branch_per_log_probability
            {
                let (dx, dz) = directions[random.next_int_bound(4) as usize];
                let Ok(length) = extra_branch_length.sample(random) else {
                    return;
                };
                let Ok(second_length) = extra_branch_length.sample(random) else {
                    return;
                };
                let extra_length = (length - second_length - 1).max(0);
                let Ok(steps) = extra_branch_steps.sample(random) else {
                    return;
                };
                self.place_upwards_branch(
                    level,
                    origin,
                    y,
                    free_height,
                    (dx, dz),
                    extra_length,
                    steps,
                    can_grow_through,
                    logs,
                    attachments,
                );
            }
            if index == free_height - 1 {
                attachments.push(FoliageAttachment {
                    pos: pos.above(),
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }
    }

    /// `UpwardsBranchingTrunkPlacer.placeBranch`.
    #[allow(clippy::too_many_arguments)]
    fn place_upwards_branch(
        &self,
        level: &mut dyn FeatureLevel,
        origin: BlockPos,
        y: i32,
        free_height: i32,
        direction: (i32, i32),
        extra_branch_length: i32,
        mut extra_branch_steps: i32,
        can_grow_through: &BlockSet,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        let mut x = origin.x();
        let mut z = origin.z();
        let mut top_y = y + extra_branch_length;
        let mut index = extra_branch_length;

        while index < free_height && extra_branch_steps > 0 {
            if index >= 1 {
                let branch_y = y + index;
                x += direction.0;
                z += direction.1;
                top_y = branch_y;
                let pos = BlockPos::new(x, branch_y, z);
                if self.place_log_through(level, pos, logs, can_grow_through) {
                    top_y = branch_y + 1;
                }
                attachments.push(FoliageAttachment {
                    pos,
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
            index += 1;
            extra_branch_steps -= 1;
        }

        if top_y - y > 1 {
            let top = BlockPos::new(x, top_y, z);
            attachments.push(FoliageAttachment {
                pos: top,
                radius_offset: 0,
                double_trunk: false,
            });
            attachments.push(FoliageAttachment {
                pos: top.below_n(2),
                radius_offset: 0,
                double_trunk: false,
            });
        }
    }

    /// `UpwardsBranchingTrunkPlacer.placeLog` through its growth tag.
    fn place_log_through(
        &self,
        level: &mut dyn FeatureLevel,
        pos: BlockPos,
        logs: &mut Vec<BlockPos>,
        can_grow_through: &BlockSet,
    ) -> bool {
        let replaceable = level
            .get_block(pos)
            .is_none_or(|state| valid_tree_pos_state(state) || can_grow_through.contains(state));
        if replaceable && level.set_block(pos, self.trunk_provider.clone()) {
            logs.push(pos);
            true
        } else {
            false
        }
    }

    /// `GiantTrunkPlacer.placeTrunk`.
    fn place_giant_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        let below = origin.below();
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            self.set_dirt(level, below.offset(dx, 0, dz));
        }
        for index in 0..free_height {
            self.place_log_if_free(level, origin.offset(0, index, 0), logs);
            if index < free_height - 1 {
                for (dx, dz) in [(1, 0), (1, 1), (0, 1)] {
                    self.place_log_if_free(level, origin.offset(dx, index, dz), logs);
                }
            }
        }
        attachments.push(FoliageAttachment {
            pos: origin.above_n(free_height),
            radius_offset: 0,
            double_trunk: true,
        });
    }

    /// `MegaJungleTrunkPlacer.placeTrunk`.
    fn place_mega_jungle_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        self.place_giant_trunk(level, origin, free_height, logs, attachments);

        let mut index = free_height - 2 - random.next_int_bound(4);
        while index > free_height / 2 {
            let angle = random.next_float() * std::f32::consts::TAU;
            let mut x = 0;
            let mut z = 0;
            for step in 0..5 {
                x = (1.5 + mth_cos(f64::from(angle)) * step as f32) as i32;
                z = (1.5 + mth_sin(f64::from(angle)) * step as f32) as i32;
                self.place_log(level, origin.offset(x, index - 3 + step / 2, z), logs);
            }
            attachments.push(FoliageAttachment {
                pos: origin.offset(x, index, z),
                radius_offset: -2,
                double_trunk: false,
            });
            index -= 2 + random.next_int_bound(4);
        }
    }

    /// `DarkOakTrunkPlacer.placeTrunk`.
    fn place_dark_oak_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        let below = origin.below();
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            self.set_dirt(level, below.offset(dx, 0, dz));
        }

        let directions = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        let (lean_x, lean_z) = directions[random.next_int_bound(4) as usize];
        let lean_height = free_height - random.next_int_bound(4);
        let mut lean_left = 2 - random.next_int_bound(3);
        let mut x = origin.x();
        let mut z = origin.z();

        for index in 0..free_height {
            if index >= lean_height && lean_left > 0 {
                x += lean_x;
                z += lean_z;
                lean_left -= 1;
            }
            let base = BlockPos::new(x, origin.y() + index, z);
            if is_air_or_leaves(level, base) {
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.place_log(level, base.offset(dx, 0, dz), logs);
                }
            }
        }

        let top_y = origin.y() + free_height - 1;
        attachments.push(FoliageAttachment {
            pos: BlockPos::new(x, top_y, z),
            radius_offset: 0,
            double_trunk: true,
        });
        for dx in -1..=2 {
            for dz in -1..=2 {
                if (!(0..=1).contains(&dx) || !(0..=1).contains(&dz))
                    && random.next_int_bound(3) == 0
                {
                    let branch_height = random.next_int_bound(3) + 2;
                    for index in 0..branch_height {
                        self.place_log(
                            level,
                            BlockPos::new(origin.x() + dx, top_y - index - 1, origin.z() + dz),
                            logs,
                        );
                    }
                    attachments.push(FoliageAttachment {
                        pos: BlockPos::new(origin.x() + dx, top_y, origin.z() + dz),
                        radius_offset: 0,
                        double_trunk: false,
                    });
                }
            }
        }
    }

    /// `ForkingTrunkPlacer.placeTrunk`.
    fn place_forking_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        self.set_dirt(level, origin.below());
        // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST.
        let directions = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        let direction = directions[random.next_int_bound(4) as usize];
        let split = free_height - random.next_int_bound(4) - 1;
        let mut lean_left = 3 - random.next_int_bound(3);
        let mut x = origin.x();
        let mut z = origin.z();
        let mut top: Option<i32> = None;

        for index in 0..free_height {
            let y = origin.y() + index;
            if index >= split && lean_left > 0 {
                x += direction.0;
                z += direction.1;
                lean_left -= 1;
            }
            if self.place_log(level, BlockPos::new(x, y, z), logs) {
                top = Some(y + 1);
            }
        }
        if let Some(top) = top {
            attachments.push(FoliageAttachment {
                pos: BlockPos::new(x, top, z),
                radius_offset: 1,
                double_trunk: false,
            });
        }

        x = origin.x();
        z = origin.z();
        let second = directions[random.next_int_bound(4) as usize];
        if second != direction {
            let start = split - random.next_int_bound(2) - 1;
            let mut remaining = 1 + random.next_int_bound(3);
            let mut top: Option<i32> = None;
            let mut index = start;
            while index < free_height && remaining > 0 {
                if index >= 1 {
                    let y = origin.y() + index;
                    x += second.0;
                    z += second.1;
                    if self.place_log(level, BlockPos::new(x, y, z), logs) {
                        top = Some(y + 1);
                    }
                }
                index += 1;
                remaining -= 1;
            }
            if let Some(top) = top {
                attachments.push(FoliageAttachment {
                    pos: BlockPos::new(x, top, z),
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }
    }

    /// `FancyTrunkPlacer.placeTrunk`.
    #[allow(clippy::too_many_arguments)]
    fn place_fancy_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
    ) {
        let max_height = free_height + 2;
        let floor = (f64::from(max_height) * 0.618).floor() as i32;
        self.set_dirt(level, origin.below());
        let min_branches = ((1.382 + (f64::from(max_height) / 13.0).powi(2)).floor() as i32).min(1);
        let branch_base_y = origin.y() + floor;
        let start = max_height - 5;
        let mut coords = vec![(origin.above_n(start), branch_base_y)];

        for current_y in (0..=start).rev() {
            let shape = fancy_tree_shape(max_height, current_y);
            if shape < 0.0 {
                continue;
            }
            for _ in 0..min_branches {
                let distance = f64::from(shape) * (f64::from(random.next_float()) + 0.328);
                let angle = f64::from(random.next_float()) * 2.0 * std::f64::consts::PI;
                let limb_start = origin.offset(
                    (distance * angle.sin() + 0.5).floor() as i32,
                    current_y - 1,
                    (distance * angle.cos() + 0.5).floor() as i32,
                );
                if !self.make_fancy_limb(
                    level,
                    random,
                    limb_start,
                    limb_start.above_n(5),
                    false,
                    logs,
                ) {
                    continue;
                }

                let dx = origin.x() - limb_start.x();
                let dz = origin.z() - limb_start.z();
                let branch_y =
                    limb_start.y() as f64 - (f64::from(dx * dx + dz * dz)).sqrt() * 0.381;
                let base_y = if branch_y > f64::from(branch_base_y) {
                    branch_base_y
                } else {
                    branch_y as i32
                };
                let branch_base = BlockPos::new(origin.x(), base_y, origin.z());
                if self.make_fancy_limb(level, random, branch_base, limb_start, false, logs) {
                    coords.push((limb_start, branch_base.y()));
                }
            }
        }

        self.make_fancy_limb(level, random, origin, origin.above_n(floor), true, logs);
        self.make_fancy_branches(level, random, max_height, origin, &coords, logs);

        for (pos, branch_base) in coords {
            if fancy_trim_branches(max_height, branch_base - origin.y()) {
                attachments.push(FoliageAttachment {
                    pos,
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }
    }

    /// `FancyTrunkPlacer.makeLimb`.
    #[allow(clippy::too_many_arguments)]
    fn make_fancy_limb(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        base: BlockPos,
        offset: BlockPos,
        modify_world: bool,
        logs: &mut Vec<BlockPos>,
    ) -> bool {
        if !modify_world && base == offset {
            return true;
        }
        if base == offset {
            self.place_log_axis(level, base, logs, "y");
            return true;
        }

        let dx = offset.x() - base.x();
        let dy = offset.y() - base.y();
        let dz = offset.z() - base.z();
        let steps = dx.abs().max(dy.abs()).max(dz.abs());
        let fx = dx as f32 / steps as f32;
        let fy = dy as f32 / steps as f32;
        let fz = dz as f32 / steps as f32;

        for index in 0..=steps {
            let pos = base.offset(
                (0.5 + index as f32 * fx).floor() as i32,
                (0.5 + index as f32 * fy).floor() as i32,
                (0.5 + index as f32 * fz).floor() as i32,
            );
            if modify_world {
                self.place_log_axis(level, pos, logs, fancy_log_axis(base, pos));
            } else if !is_tree_free(level, pos) {
                return false;
            }
        }
        true
    }

    /// `FancyTrunkPlacer.makeBranches`.
    fn make_fancy_branches(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        max_height: i32,
        origin: BlockPos,
        coords: &[(BlockPos, i32)],
        logs: &mut Vec<BlockPos>,
    ) {
        for &(attachment, branch_base) in coords {
            let base = BlockPos::new(origin.x(), branch_base, origin.z());
            if base != attachment && fancy_trim_branches(max_height, branch_base - origin.y()) {
                self.make_fancy_limb(level, random, base, attachment, true, logs);
            }
        }
    }

    /// `CherryTrunkPlacer.placeTrunk`.
    #[allow(clippy::too_many_arguments)]
    fn place_cherry_trunk(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        logs: &mut Vec<BlockPos>,
        attachments: &mut Vec<FoliageAttachment>,
        branch_count: &IntProvider,
        branch_horizontal_length: &IntProvider,
        branch_start_offset_from_top: &IntProvider,
        second_branch_start_offset_from_top: &IntProvider,
        branch_end_offset_from_top: &IntProvider,
    ) {
        self.set_dirt(level, origin.below());
        let Ok(start) = branch_start_offset_from_top.sample(random) else {
            return;
        };
        let Ok(second_start) = second_branch_start_offset_from_top.sample(random) else {
            return;
        };
        let max = (free_height - 1 + start).max(0);
        let mut max_second = (free_height - 1 + second_start).max(0);
        if max_second >= max {
            max_second += 1;
        }
        let Ok(count) = branch_count.sample(random) else {
            return;
        };
        let three = count == 3;
        let two = count >= 2;
        let trunk_height = if three {
            free_height
        } else if two {
            max.max(max_second) + 1
        } else {
            max + 1
        };
        for index in 0..trunk_height {
            self.place_log(level, origin.above_n(index), logs);
        }
        if three {
            attachments.push(FoliageAttachment {
                pos: origin.above_n(trunk_height),
                radius_offset: 0,
                double_trunk: false,
            });
        }

        let directions = [((0, -1), "z"), ((1, 0), "x"), ((0, 1), "z"), ((-1, 0), "x")];
        let (direction, axis) = directions[random.next_int_bound(4) as usize];
        if let Some(attachment) = self.generate_cherry_branch(
            level,
            random,
            origin,
            free_height,
            direction,
            axis,
            max,
            max < trunk_height - 1,
            branch_horizontal_length,
            branch_end_offset_from_top,
            logs,
        ) {
            attachments.push(attachment);
        }
        if two {
            let opposite = (-direction.0, -direction.1);
            if let Some(attachment) = self.generate_cherry_branch(
                level,
                random,
                origin,
                free_height,
                opposite,
                axis,
                max_second,
                max_second < trunk_height - 1,
                branch_horizontal_length,
                branch_end_offset_from_top,
                logs,
            ) {
                attachments.push(attachment);
            }
        }
    }

    /// `CherryTrunkPlacer.generateBranch`.
    #[allow(clippy::too_many_arguments)]
    fn generate_cherry_branch(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
        free_height: i32,
        direction: (i32, i32),
        axis: &str,
        start_offset: i32,
        double_branch: bool,
        branch_horizontal_length: &IntProvider,
        branch_end_offset_from_top: &IntProvider,
        logs: &mut Vec<BlockPos>,
    ) -> Option<FoliageAttachment> {
        let mut current = origin.above_n(start_offset);
        let end_offset = branch_end_offset_from_top.sample(random).ok()?;
        let end_y = free_height - 1 + end_offset;
        let double = double_branch || end_y < start_offset;
        let length = branch_horizontal_length.sample(random).ok()? + if double { 1 } else { 0 };
        let target = origin.offset(direction.0 * length, end_y, direction.1 * length);

        for _ in 0..if double { 2 } else { 1 } {
            current = current.offset(direction.0, 0, direction.1);
            self.place_log_axis(level, current, logs, axis);
        }

        let vertical = if target.y() > current.y() { 1 } else { -1 };
        loop {
            let distance = block_manhattan(current, target);
            if distance == 0 {
                return Some(FoliageAttachment {
                    pos: target.above(),
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
            let fraction = (target.y() - current.y()).abs() as f32 / distance as f32;
            let up = random.next_float() < fraction;
            current = if up {
                current.offset(0, vertical, 0)
            } else {
                current.offset(direction.0, 0, direction.1)
            };
            self.place_log_axis(level, current, logs, if up { "y" } else { axis });
        }
    }

    /// `FoliagePlacer.createFoliage` for the supported shapes.
    fn create_foliage(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        attachment: &FoliageAttachment,
        (foliage_height, foliage_radius): (i32, i32),
        leaves: &mut Vec<BlockPos>,
    ) -> bool {
        let Ok(offset) = self.foliage_offset.sample(random) else {
            return false;
        };
        let mut placed = false;
        let pos = attachment.pos;
        match &self.foliage_placer {
            FoliagePlacer::Blob { .. } => {
                for local_y in (offset - foliage_height..=offset).rev() {
                    let range = (foliage_radius - 1 - local_y / 2).max(0);
                    if self.place_leaves_row(level, random, pos, (range, local_y), false, leaves) {
                        placed = true;
                    }
                }
            }
            FoliagePlacer::Bush { .. } => {
                for local_y in (offset - foliage_height..=offset).rev() {
                    let range = foliage_radius + attachment.radius_offset - 1 - local_y;
                    if self.place_leaves_row(
                        level,
                        random,
                        pos,
                        (range, local_y),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                }
            }
            FoliagePlacer::RandomSpread {
                leaf_placement_attempts,
                ..
            } => {
                for _ in 0..*leaf_placement_attempts {
                    let dx = random.next_int_bound(foliage_radius)
                        - random.next_int_bound(foliage_radius);
                    let dy = random.next_int_bound(foliage_height)
                        - random.next_int_bound(foliage_height);
                    let dz = random.next_int_bound(foliage_radius)
                        - random.next_int_bound(foliage_radius);
                    let leaf = pos.offset(dx, dy, dz);
                    if self.try_place_leaf(level, random, leaf) {
                        leaves.push(leaf);
                        placed = true;
                    }
                }
            }
            FoliagePlacer::Spruce { .. } => {
                let mut range = random.next_int_bound(2);
                let mut threshold = 1;
                let mut next_range = 0;
                for local_y in (-foliage_height..=offset).rev() {
                    if self.place_leaves_row(level, random, pos, (range, local_y), false, leaves) {
                        placed = true;
                    }
                    if range >= threshold {
                        range = next_range;
                        next_range = 1;
                        threshold = (threshold + 1).min(foliage_radius);
                    } else {
                        range += 1;
                    }
                }
            }
            FoliagePlacer::Pine { .. } => {
                let mut range = 0;
                for local_y in (offset - foliage_height..=offset).rev() {
                    if self.place_leaves_row(level, random, pos, (range, local_y), false, leaves) {
                        placed = true;
                    }
                    if range >= 1 && local_y == offset - foliage_height + 1 {
                        range -= 1;
                    } else if range < foliage_radius {
                        range += 1;
                    }
                }
            }
            FoliagePlacer::MegaJungle { .. } => {
                let depth = if attachment.double_trunk {
                    foliage_height
                } else {
                    1 + random.next_int_bound(2)
                };
                for local_y in (offset - depth..=offset).rev() {
                    let range = foliage_radius + attachment.radius_offset + 1 - local_y;
                    if self.place_leaves_row(
                        level,
                        random,
                        pos,
                        (range, local_y),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                }
            }
            FoliagePlacer::MegaPine { .. } => {
                let mut previous = 0;
                for y in pos.y() - foliage_height + offset..=pos.y() + offset {
                    let dy = pos.y() - y;
                    let range = foliage_radius
                        + attachment.radius_offset
                        + (dy as f32 / foliage_height as f32 * 3.5).floor() as i32;
                    let actual = if dy > 0 && range == previous && (y & 1) == 0 {
                        range + 1
                    } else {
                        range
                    };
                    if self.place_leaves_row(
                        level,
                        random,
                        BlockPos::new(pos.x(), y, pos.z()),
                        (actual, 0),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                    previous = range;
                }
            }
            FoliagePlacer::Fancy { .. } => {
                for local_y in (offset - foliage_height..=offset).rev() {
                    let range = foliage_radius
                        + i32::from(local_y != offset && local_y != offset - foliage_height);
                    if self.place_leaves_row(
                        level,
                        random,
                        pos,
                        (range, local_y),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                }
            }
            FoliagePlacer::Cherry {
                hanging_leaves_chance,
                hanging_leaves_extension_chance,
                ..
            } => {
                let center = pos.above_n(offset);
                let inner = foliage_radius + attachment.radius_offset - 1;
                if self.place_leaves_row(
                    level,
                    random,
                    center,
                    (inner - 2, foliage_height - 3),
                    attachment.double_trunk,
                    leaves,
                ) {
                    placed = true;
                }
                if self.place_leaves_row(
                    level,
                    random,
                    center,
                    (inner - 1, foliage_height - 4),
                    attachment.double_trunk,
                    leaves,
                ) {
                    placed = true;
                }
                for local_y in (0..=foliage_height - 5).rev() {
                    if self.place_leaves_row(
                        level,
                        random,
                        center,
                        (inner, local_y),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                }
                if self.place_leaves_row_with_hanging(
                    level,
                    random,
                    center,
                    (inner, -1),
                    attachment.double_trunk,
                    *hanging_leaves_chance,
                    *hanging_leaves_extension_chance,
                    leaves,
                ) {
                    placed = true;
                }
                if self.place_leaves_row_with_hanging(
                    level,
                    random,
                    center,
                    (inner - 1, -2),
                    attachment.double_trunk,
                    *hanging_leaves_chance,
                    *hanging_leaves_extension_chance,
                    leaves,
                ) {
                    placed = true;
                }
            }
            FoliagePlacer::Acacia => {
                let center = pos.above_n(offset);
                for (range, local_y) in [
                    (
                        foliage_radius + attachment.radius_offset,
                        -1 - foliage_height,
                    ),
                    (foliage_radius - 1, -foliage_height),
                    (foliage_radius + attachment.radius_offset - 1, 0),
                ] {
                    if self.place_leaves_row(
                        level,
                        random,
                        center,
                        (range, local_y),
                        attachment.double_trunk,
                        leaves,
                    ) {
                        placed = true;
                    }
                }
            }
            FoliagePlacer::DarkOak => {
                let center = pos.above_n(offset);
                if attachment.double_trunk {
                    for (range, local_y) in [
                        (foliage_radius + 2, -1),
                        (foliage_radius + 3, 0),
                        (foliage_radius + 2, 1),
                    ] {
                        if self.place_leaves_row(
                            level,
                            random,
                            center,
                            (range, local_y),
                            true,
                            leaves,
                        ) {
                            placed = true;
                        }
                    }
                    if random.next_int_bound(2) == 0
                        && self.place_leaves_row(
                            level,
                            random,
                            center,
                            (foliage_radius, 2),
                            true,
                            leaves,
                        )
                    {
                        placed = true;
                    }
                } else {
                    for (range, local_y) in [(foliage_radius + 2, -1), (foliage_radius + 1, 0)] {
                        if self.place_leaves_row(
                            level,
                            random,
                            center,
                            (range, local_y),
                            false,
                            leaves,
                        ) {
                            placed = true;
                        }
                    }
                }
            }
        }
        placed
    }

    /// `FoliagePlacer.placeLeavesRowWithHangingLeavesBelow`.
    #[allow(clippy::too_many_arguments)]
    fn place_leaves_row_with_hanging(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        center: BlockPos,
        (range, local_y): (i32, i32),
        large: bool,
        hanging_chance: f32,
        extension_chance: f32,
        leaves: &mut Vec<BlockPos>,
    ) -> bool {
        let placed = self.place_leaves_row(level, random, center, (range, local_y), large, leaves);
        let extra = i32::from(large);
        let log_pos = center.below();
        for (direction, clockwise, clockwise_positive) in [
            ((0, -1), (1, 0), true),
            ((1, 0), (0, 1), true),
            ((0, 1), (-1, 0), false),
            ((-1, 0), (0, -1), false),
        ] {
            let clockwise_distance = if clockwise_positive {
                range + extra
            } else {
                range
            };
            let mut cursor = center
                .offset(0, local_y - 1, 0)
                .offset(
                    clockwise.0 * clockwise_distance,
                    0,
                    clockwise.1 * clockwise_distance,
                )
                .offset(-direction.0 * range, 0, -direction.1 * range);
            for _ in -range..range + extra {
                let above = cursor.above();
                if leaves.contains(&above)
                    && self.try_place_extension(
                        level,
                        random,
                        log_pos,
                        cursor,
                        hanging_chance,
                        leaves,
                    )
                {
                    self.try_place_extension(
                        level,
                        random,
                        log_pos,
                        cursor.below(),
                        extension_chance,
                        leaves,
                    );
                }
                cursor = cursor.offset(direction.0, 0, direction.1);
            }
        }
        placed
    }

    /// `FoliagePlacer.tryPlaceLeaf`.
    fn try_place_leaf(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) -> bool {
        let replaceable = level.get_block(pos).is_some_and(valid_tree_pos_state);
        if !replaceable {
            return false;
        }
        let Some(state) = self.foliage_provider.sample(random) else {
            return false;
        };
        level.set_block(pos, state)
    }

    /// `FoliagePlacer.tryPlaceExtension`.
    fn try_place_extension(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        log_pos: BlockPos,
        pos: BlockPos,
        chance: f32,
        leaves: &mut Vec<BlockPos>,
    ) -> bool {
        if block_manhattan(pos, log_pos) >= 7 || random.next_float() > chance {
            return false;
        }
        if self.try_place_leaf(level, random, pos) {
            leaves.push(pos);
            true
        } else {
            false
        }
    }

    /// `FoliagePlacer.placeLeavesRow`.
    #[allow(clippy::too_many_arguments)]
    fn place_leaves_row(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        attachment: BlockPos,
        (range, local_y): (i32, i32),
        large: bool,
        leaves: &mut Vec<BlockPos>,
    ) -> bool {
        let mut placed = false;
        let extra = i32::from(large);
        for dx in -range..=range + extra {
            for dz in -range..=range + extra {
                if self.skip_leaf(random, dx, dz, range, local_y, large) {
                    continue;
                }
                let pos = attachment.offset(dx, local_y, dz);
                if self.try_place_leaf(level, random, pos) {
                    leaves.push(pos);
                    placed = true;
                }
            }
        }
        placed
    }

    /// `shouldSkipLocationSigned` for each supported foliage placer.
    fn skip_leaf(
        &self,
        random: &mut dyn RandomSource,
        dx: i32,
        dz: i32,
        range: i32,
        local_y: i32,
        large: bool,
    ) -> bool {
        let abs_x = dx.abs();
        let abs_z = dz.abs();
        match &self.foliage_placer {
            FoliagePlacer::Blob { .. } => {
                abs_x == range && abs_z == range && (random.next_int_bound(2) == 0 || local_y == 0)
            }
            FoliagePlacer::Bush { .. } => {
                dx == range && dz == range && random.next_int_bound(2) == 0
            }
            FoliagePlacer::RandomSpread { .. } => false,
            FoliagePlacer::Spruce { .. } | FoliagePlacer::Pine { .. } => {
                range > 0 && abs_x == range && abs_z == range
            }
            FoliagePlacer::Acacia => {
                let min_x = if large {
                    abs_x.min((dx - 1).abs())
                } else {
                    abs_x
                };
                let min_z = if large {
                    abs_z.min((dz - 1).abs())
                } else {
                    abs_z
                };
                if local_y == 0 {
                    (min_x > 1 || min_z > 1) && min_x != 0 && min_z != 0
                } else {
                    min_x == range && min_z == range && range > 0
                }
            }
            FoliagePlacer::MegaPine { .. } | FoliagePlacer::MegaJungle { .. } => {
                let min_x = if large {
                    abs_x.min((dx - 1).abs())
                } else {
                    abs_x
                };
                let min_z = if large {
                    abs_z.min((dz - 1).abs())
                } else {
                    abs_z
                };
                min_x + min_z >= 7 || min_x * min_x + min_z * min_z > range * range
            }
            FoliagePlacer::Fancy { .. } => {
                let min_x = if large {
                    abs_x.min((dx - 1).abs())
                } else {
                    abs_x
                };
                let min_z = if large {
                    abs_z.min((dz - 1).abs())
                } else {
                    abs_z
                };
                (min_x as f32 + 0.5).powi(2) + (min_z as f32 + 0.5).powi(2) > (range * range) as f32
            }
            FoliagePlacer::Cherry {
                wide_bottom_layer_hole_chance,
                corner_hole_chance,
                ..
            } => {
                let min_x = if large {
                    abs_x.min((dx - 1).abs())
                } else {
                    abs_x
                };
                let min_z = if large {
                    abs_z.min((dz - 1).abs())
                } else {
                    abs_z
                };
                if local_y == -1
                    && (min_x == range || min_z == range)
                    && random.next_float() < *wide_bottom_layer_hole_chance
                {
                    return true;
                }
                let corner = min_x == range && min_z == range;
                if range > 2 {
                    corner
                        || (min_x + min_z > range * 2 - 2
                            && random.next_float() < *corner_hole_chance)
                } else {
                    corner && random.next_float() < *corner_hole_chance
                }
            }
            FoliagePlacer::DarkOak => {
                let min_x = if large {
                    abs_x.min((dx - 1).abs())
                } else {
                    abs_x
                };
                let min_z = if large {
                    abs_z.min((dz - 1).abs())
                } else {
                    abs_z
                };
                let corner = local_y == 0
                    && large
                    && (dx == -range || dx >= range)
                    && (dz == -range || dz >= range);
                corner
                    || if local_y == -1 && !large {
                        min_x == range && min_z == range
                    } else {
                        local_y == 1 && min_x + min_z > range * 2 - 2
                    }
            }
        }
    }
}

impl RootPlacer {
    fn get_trunk_origin(&self, pos: BlockPos, random: &mut dyn RandomSource) -> Option<BlockPos> {
        self.trunk_offset_y
            .sample(random)
            .ok()
            .map(|offset| pos.above_n(offset))
    }

    /// `MangroveRootPlacer.placeRoots`.
    fn place_roots(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        trunk_origin: BlockPos,
    ) -> bool {
        let mut cursor = pos;
        while cursor.y() < trunk_origin.y() {
            if !self.can_place_root(level, cursor) {
                return false;
            }
            cursor = cursor.above();
        }

        let mut roots = vec![trunk_origin.below()];
        for direction in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let start = trunk_origin.offset(direction.0, 0, direction.1);
            let mut branch = Vec::new();
            if !self.simulate_roots(
                level,
                random,
                start,
                direction,
                trunk_origin,
                &mut branch,
                0,
            ) {
                return false;
            }
            roots.extend(branch);
            roots.push(start);
        }

        for root in roots {
            self.place_root(level, random, root);
        }
        true
    }

    /// `MangroveRootPlacer.simulateRoots`.
    #[allow(clippy::too_many_arguments)]
    fn simulate_roots(
        &self,
        level: &dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        direction: (i32, i32),
        trunk_origin: BlockPos,
        roots: &mut Vec<BlockPos>,
        length: i32,
    ) -> bool {
        if length == self.mangrove.max_root_length
            || roots.len() > self.mangrove.max_root_length as usize
        {
            return false;
        }

        for candidate in self.potential_root_positions(pos, direction, random, trunk_origin) {
            if self.can_place_root(level, candidate) {
                roots.push(candidate);
                if !self.simulate_roots(
                    level,
                    random,
                    candidate,
                    direction,
                    trunk_origin,
                    roots,
                    length + 1,
                ) {
                    return false;
                }
            }
        }
        true
    }

    /// `MangroveRootPlacer.potentialRootPositions`.
    fn potential_root_positions(
        &self,
        pos: BlockPos,
        direction: (i32, i32),
        random: &mut dyn RandomSource,
        trunk_origin: BlockPos,
    ) -> Vec<BlockPos> {
        let below = pos.below();
        let side_below = pos.offset(direction.0, 0, direction.1).below();
        let distance = block_manhattan(pos, trunk_origin);
        let max_width = self.mangrove.max_root_width;
        let skew_chance = self.mangrove.random_skew_chance;
        if distance > max_width - 3 && distance <= max_width {
            if random.next_float() < skew_chance {
                vec![below, side_below]
            } else {
                vec![below]
            }
        } else if distance > max_width || random.next_float() < skew_chance {
            vec![below]
        } else if random.next_boolean() {
            vec![pos.offset(direction.0, 0, direction.1)]
        } else {
            vec![below]
        }
    }

    /// `MangroveRootPlacer.canPlaceRoot`.
    fn can_place_root(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        level.get_block(pos).is_some_and(|state| {
            valid_tree_pos_state(state) || self.mangrove.can_grow_through.contains(state)
        })
    }

    /// `MangroveRootPlacer.placeRoot`.
    fn place_root(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) {
        let muddy = level
            .get_block(pos)
            .is_some_and(|state| self.mangrove.muddy_roots_in.contains(state.name.as_str()));
        if muddy {
            if let Some(state) = self.mangrove.muddy_roots_provider.sample(random) {
                let state = potentially_waterlogged(level, pos, state);
                level.set_block(pos, state);
            }
            return;
        }

        if !self.can_place_root(level, pos) {
            return;
        }
        let Some(state) = self.root_provider.sample(random) else {
            return;
        };
        let state = potentially_waterlogged(level, pos, state);
        if !level.set_block(pos, state) {
            return;
        }

        if let Some(above) = &self.above_root_placement {
            let above_pos = pos.above();
            if random.next_float() < above.above_root_placement_chance
                && level
                    .get_block(above_pos)
                    .is_some_and(|state| is_air_name(&state.name))
                && let Some(state) = above.above_root_provider.sample(random)
            {
                let state = potentially_waterlogged(level, above_pos, state);
                level.set_block(above_pos, state);
            }
        }
    }
}

/// `RootPlacer.getPotentiallyWaterloggedState`.
fn potentially_waterlogged(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    mut state: BlockState,
) -> BlockState {
    let water = level.get_block(pos).is_some_and(|current| {
        matches!(
            current.name.as_str(),
            "minecraft:water" | "minecraft:seagrass" | "minecraft:tall_seagrass"
        ) || current
            .properties
            .iter()
            .any(|(name, value)| name == "waterlogged" && value == "true")
    });
    if let Some((_, value)) = state
        .properties
        .iter_mut()
        .find(|(name, _)| name == "waterlogged")
    {
        *value = if water { "true" } else { "false" }.to_owned();
    }
    state
}

impl TreeDecorator {
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
        leaves: &[BlockPos],
    ) {
        match self {
            TreeDecorator::PlaceOnGround(decorator) => decorator.place(level, random, logs),
            TreeDecorator::Beehive(decorator) => decorator.place(level, random, logs, leaves),
            TreeDecorator::LeaveVine(decorator) => decorator.place(level, random, leaves),
            TreeDecorator::TrunkVine => place_trunk_vines(level, random, logs),
            TreeDecorator::Cocoa(decorator) => decorator.place(level, random, logs),
            TreeDecorator::AlterGround(decorator) => decorator.place(level, random, logs),
            TreeDecorator::AttachedToLeaves(decorator) => decorator.place(level, random, leaves),
            TreeDecorator::AttachedToLogs(decorator) => decorator.place(level, random, logs),
            TreeDecorator::CreakingHeart(decorator) => decorator.place(level, random, logs),
            TreeDecorator::PaleMoss(decorator) => {
                decorator.place(level, random, logs, leaves);
            }
        }
    }
}

impl FallenTreeFeature {
    /// `FallenTreeFeature.place`; the feature reports success even when only
    /// the stump was placed, as vanilla does.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        self.place_fallen_tree(level, random, origin);
        true
    }

    /// `FallenTreeFeature.placeFallenTree`.
    fn place_fallen_tree(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) {
        self.place_stump(level, random, origin);

        // `Direction.Plane.HORIZONTAL`: NORTH, EAST, SOUTH, WEST.
        let directions = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        let direction = directions[random.next_int_bound(4) as usize];
        let Ok(log_length) = self.log_length.sample(random) else {
            return;
        };
        let log_length = log_length - 2;
        let distance = 2 + random.next_int_bound(2);
        let mut start = origin.offset(direction.0 * distance, 0, direction.1 * distance);
        self.set_ground_height(level, &mut start);
        if self.can_place_entire_fallen_log(level, log_length, start, direction) {
            self.place_fallen_log(level, random, log_length, start, direction);
        }
    }

    /// `FallenTreeFeature.placeStump`.
    fn place_stump(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) {
        let stump = self.place_log_block(level, random, origin, None);
        self.decorate_logs(level, random, vec![stump], &self.stump_decorators);
    }

    /// `FallenTreeFeature.setGroundHeightForFallenLogStartPos`.
    fn set_ground_height(&self, level: &dyn FeatureLevel, pos: &mut BlockPos) {
        *pos = pos.above();
        for _ in 0..6 {
            if self.may_place_on(level, *pos) {
                return;
            }
            *pos = pos.below();
        }
    }

    /// `FallenTreeFeature.canPlaceEntireFallenLog`.
    fn can_place_entire_fallen_log(
        &self,
        level: &dyn FeatureLevel,
        log_length: i32,
        pos: BlockPos,
        direction: (i32, i32),
    ) -> bool {
        let mut cursor = pos;
        let mut ground_gap = 0;
        for _ in 0..log_length {
            if !valid_tree_pos(level, cursor) {
                return false;
            }
            if self.is_over_solid_ground(level, cursor) {
                ground_gap = 0;
            } else {
                ground_gap += 1;
                if ground_gap > 2 {
                    return false;
                }
            }
            cursor = cursor.offset(direction.0, 0, direction.1);
        }
        true
    }

    /// `FallenTreeFeature.placeFallenLog`.
    fn place_fallen_log(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        log_length: i32,
        pos: BlockPos,
        direction: (i32, i32),
    ) {
        let axis = if direction.0 != 0 { "x" } else { "z" };
        let mut logs = Vec::with_capacity(log_length.max(0) as usize);
        let mut cursor = pos;
        for _ in 0..log_length {
            logs.push(self.place_log_block(level, random, cursor, Some(axis)));
            cursor = cursor.offset(direction.0, 0, direction.1);
        }
        self.decorate_logs(level, random, logs, &self.log_decorators);
    }

    /// `FallenTreeFeature.placeLogBlock`.
    fn place_log_block(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
        axis: Option<&str>,
    ) -> BlockPos {
        if let Some(mut state) = self.trunk_provider.sample(random) {
            if let Some(axis) = axis
                && let Some((_, value)) =
                    state.properties.iter_mut().find(|(name, _)| name == "axis")
            {
                *value = axis.to_owned();
            }
            level.set_block(pos, state);
        }
        pos
    }

    /// `FallenTreeFeature.mayPlaceOn`.
    fn may_place_on(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        valid_tree_pos(level, pos) && self.is_over_solid_ground(level, pos)
    }

    /// `FallenTreeFeature.isOverSolidGround`. Full block-behavior metadata is
    /// not ported yet; use the solid-render surface set already used by leaf
    /// litter and fall back to it only for terrain-like blocks.
    fn is_over_solid_ground(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        level.get_block(pos.below()).is_some_and(is_solid_render)
    }

    /// `FallenTreeFeature.decorateLogs`.
    fn decorate_logs(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        mut logs: Vec<BlockPos>,
        decorators: &[TreeDecorator],
    ) {
        if decorators.is_empty() {
            return;
        }
        logs.sort_by_key(|pos| pos.y());
        for decorator in decorators {
            decorator.place(level, random, &logs, &[]);
        }
    }
}

impl AttachedToLogsDecorator {
    /// `AttachedToLogsDecorator.place`.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
    ) {
        let mut candidates = logs.to_vec();
        shuffle(&mut candidates, random);
        for log in candidates {
            let direction =
                self.directions[random.next_int_bound(self.directions.len() as i32) as usize];
            let target = log.offset(direction.0, direction.1, direction.2);
            if random.next_float() <= self.probability
                && is_air(level, target)
                && let Some(state) = self.block_provider.sample(random)
            {
                level.set_block(target, state);
            }
        }
    }
}

impl CreakingHeartDecorator {
    /// `CreakingHeartDecorator.place`.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
    ) {
        if logs.is_empty() || random.next_float() >= self.probability {
            return;
        }

        // `Direction.values()`: DOWN, UP, NORTH, SOUTH, WEST, EAST.
        const DIRECTIONS: [(i32, i32, i32); 6] = [
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
            (-1, 0, 0),
            (1, 0, 0),
        ];
        let mut candidates = logs.to_vec();
        shuffle(&mut candidates, random);
        let Some(pos) = candidates.into_iter().find(|pos| {
            DIRECTIONS.iter().all(|(dx, dy, dz)| {
                level
                    .get_block(pos.offset(*dx, *dy, *dz))
                    .is_some_and(is_log)
            })
        }) else {
            return;
        };

        level.set_block(
            pos,
            BlockState {
                name: "minecraft:creaking_heart".into(),
                properties: vec![
                    ("axis".into(), "y".into()),
                    ("creaking_heart_state".into(), "dormant".into()),
                    ("natural".into(), "true".into()),
                ],
            },
        );
    }
}

impl PaleMossDecorator {
    /// `PaleMossDecorator.place`: shuffle the logs, optionally run the ground
    /// patch at the shuffled minimum-Y log, then hang moss below trunk logs and
    /// leaves. Random draws stay in vanilla order.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
        leaves: &[BlockPos],
    ) {
        let mut shuffled = logs.to_vec();
        shuffle(&mut shuffled, random);
        if shuffled.is_empty() {
            return;
        }
        let ground = *shuffled
            .iter()
            .min_by_key(|pos| pos.y())
            .expect("shuffled logs are non-empty");

        if random.next_float() < self.ground_probability {
            self.patch
                .place(level, random, ground.above())
                .expect("pale moss patch was validated when the tree was parsed");
        }
        for log in logs {
            if random.next_float() < self.trunk_probability {
                let below = log.below();
                if is_air(level, below) {
                    add_moss_hanger(level, below, random);
                }
            }
        }
        for leaf in leaves {
            if random.next_float() < self.leaves_probability {
                let below = leaf.below();
                if is_air(level, below) {
                    add_moss_hanger(level, below, random);
                }
            }
        }
    }
}

/// `PaleMossDecorator.addMossHanger`.
fn add_moss_hanger(level: &mut dyn FeatureLevel, mut pos: BlockPos, random: &mut dyn RandomSource) {
    while is_air(level, pos.below()) && random.next_float() >= 0.5 {
        level.set_block(pos, pale_hanging_moss(false));
        pos = pos.below();
    }
    level.set_block(pos, pale_hanging_moss(true));
}

fn pale_hanging_moss(tip: bool) -> BlockState {
    BlockState {
        name: "minecraft:pale_hanging_moss".into(),
        properties: vec![("tip".into(), if tip { "true" } else { "false" }.into())],
    }
}

impl PlaceOnGroundDecorator {
    /// `PlaceOnGroundDecorator.place`.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
    ) {
        let Some(lowest_y) = logs.iter().map(|pos| pos.y()).min() else {
            return;
        };
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        for pos in logs.iter().filter(|pos| pos.y() == lowest_y) {
            min_x = min_x.min(pos.x());
            max_x = max_x.max(pos.x());
            min_z = min_z.min(pos.z());
            max_z = max_z.max(pos.z());
        }
        if min_x == i32::MAX {
            return;
        }

        let min_x = min_x.saturating_sub(self.radius);
        let max_x = max_x.saturating_add(self.radius);
        let min_y = lowest_y.saturating_sub(self.height);
        let max_y = lowest_y.saturating_add(self.height);
        let min_z = min_z.saturating_sub(self.radius);
        let max_z = max_z.saturating_add(self.radius);
        for _ in 0..self.tries {
            let pos = BlockPos::new(
                random.next_int_between_inclusive(min_x, max_x),
                random.next_int_between_inclusive(min_y, max_y),
                random.next_int_between_inclusive(min_z, max_z),
            );
            self.attempt_to_place_above(level, random, pos);
        }
    }

    /// `PlaceOnGroundDecorator.attemptToPlaceBlockAbove`.
    fn attempt_to_place_above(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        pos: BlockPos,
    ) {
        let above = pos.offset(0, 1, 0);
        let replaceable = level
            .get_block(above)
            .is_some_and(|state| is_air_name(&state.name) || state.name == "minecraft:vine");
        if !replaceable || !level.get_block(pos).is_some_and(is_solid_render) {
            return;
        }
        if heightmap_no_leaves(level, pos) > above.y() {
            return;
        }
        if let Some(state) = self.block_state_provider.sample(random) {
            level.set_block(above, state);
        }
    }
}

impl AlterGroundDecorator {
    /// `AlterGroundDecorator.place`.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
    ) {
        let Some(lowest_y) = logs.iter().map(|pos| pos.y()).min() else {
            return;
        };
        let bases: Vec<BlockPos> = logs
            .iter()
            .filter(|pos| pos.y() == lowest_y)
            .copied()
            .collect();
        for base in bases {
            for (dx, dz) in [(-1, -1), (2, -1), (-1, 2), (2, 2)] {
                self.place_circle(level, random, base.offset(dx, 0, dz));
            }
            for _ in 0..5 {
                let value = random.next_int_bound(64);
                let x = value % 8;
                let z = value / 8;
                if x == 0 || x == 7 || z == 0 || z == 7 {
                    self.place_circle(level, random, base.offset(-3 + x, 0, -3 + z));
                }
            }
        }
    }

    /// `AlterGroundDecorator.placeCircle`.
    fn place_circle(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        base: BlockPos,
    ) {
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx.abs() != 2 || dz.abs() != 2 {
                    self.place_block_at(level, random, base.offset(dx, 0, dz));
                }
            }
        }
    }

    /// `AlterGroundDecorator.placeBlockAt`.
    fn place_block_at(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        base: BlockPos,
    ) {
        for dy in (-3..=2).rev() {
            let pos = base.above_n(dy);
            match level.get_block(pos) {
                Some(state) if is_dirt(state) => {
                    if let Some(state) = self.provider.sample(random) {
                        level.set_block(pos, state);
                    }
                    break;
                }
                Some(state) if !is_air_name(&state.name) && dy < 0 => break,
                None if dy < 0 => break,
                _ => {}
            }
        }
    }
}

impl AttachedToLeavesDecorator {
    /// `AttachedToLeavesDecorator.place`.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        leaves: &[BlockPos],
    ) {
        let mut excluded = HashSet::new();
        let mut candidates = leaves.to_vec();
        shuffle(&mut candidates, random);

        for leaf in candidates {
            let direction =
                self.directions[random.next_int_bound(self.directions.len() as i32) as usize];
            let target = leaf.offset(direction.0, direction.1, direction.2);
            if excluded.contains(&target)
                || random.next_float() >= self.probability
                || !self.has_required_empty_blocks(level, leaf, direction)
            {
                continue;
            }

            for dx in -self.exclusion_radius_xz..=self.exclusion_radius_xz {
                for dy in -self.exclusion_radius_y..=self.exclusion_radius_y {
                    for dz in -self.exclusion_radius_xz..=self.exclusion_radius_xz {
                        excluded.insert(target.offset(dx, dy, dz));
                    }
                }
            }

            if let Some(state) = self.block_provider.sample(random) {
                level.set_block(target, state);
            }
        }
    }

    /// `AttachedToLeavesDecorator.hasRequiredEmptyBlocks`.
    fn has_required_empty_blocks(
        &self,
        level: &dyn FeatureLevel,
        leaf: BlockPos,
        direction: (i32, i32, i32),
    ) -> bool {
        for index in 1..=self.required_empty_blocks {
            let pos = leaf.offset(
                direction.0 * index,
                direction.1 * index,
                direction.2 * index,
            );
            if !is_air(level, pos) {
                return false;
            }
        }
        true
    }
}

impl BeehiveDecorator {
    /// `BeehiveDecorator.place`. The nest's occupancy is stored as block-entity
    /// NBT: two or three bees, each with `ticks_in_hive` drawn from
    /// `nextInt(599)` and `min_ticks_in_hive` 600.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
        leaves: &[BlockPos],
    ) {
        if logs.is_empty() || random.next_float() >= self.probability {
            return;
        }
        let level_y = if !leaves.is_empty() {
            (leaves[0].y() - 1).max(logs[0].y() + 1)
        } else {
            (logs[0].y() + 1 + random.next_int_bound(3)).min(logs[logs.len() - 1].y())
        };

        // `Direction.Plane.HORIZONTAL` is NORTH, EAST, SOUTH, WEST; the
        // decorator removes NORTH (the opposite of its SOUTH-facing nest).
        let mut candidates = Vec::new();
        for log in logs.iter().filter(|pos| pos.y() == level_y) {
            for (dx, dz) in [(1, 0), (0, 1), (-1, 0)] {
                candidates.push(log.offset(dx, 0, dz));
            }
        }
        if candidates.is_empty() {
            return;
        }
        shuffle(&mut candidates, random);

        let Some(pos) = candidates
            .into_iter()
            .find(|pos| is_air(level, *pos) && is_air(level, pos.offset(0, 0, 1)))
        else {
            return;
        };
        let placed = level.set_block(
            pos,
            BlockState {
                name: "minecraft:bee_nest".into(),
                properties: vec![
                    ("facing".into(), "south".into()),
                    ("honey_level".into(), "0".into()),
                ],
            },
        );
        if placed {
            let count = 2 + random.next_int_bound(2);
            let mut ticks_in_hive = Vec::with_capacity(count as usize);
            for _ in 0..count {
                // `BeehiveBlockEntity.Occupant.create(random.nextInt(599))`.
                ticks_in_hive.push(random.next_int_bound(599));
            }
            level.set_block_entity(pos, beehive_block_entity_nbt(pos, &ticks_in_hive));
        }
    }
}

/// `BeehiveBlockEntity.saveAdditional`: the `bees` list plus the block entity
/// id and position fields a chunk's `BlockEntities` entry carries.
fn beehive_block_entity_nbt(pos: BlockPos, ticks_in_hive: &[i32]) -> Tag {
    let bees = ticks_in_hive
        .iter()
        .map(|ticks| {
            Tag::Compound(vec![
                (
                    "entity_data".into(),
                    Tag::Compound(vec![("id".into(), Tag::String("minecraft:bee".into()))]),
                ),
                ("ticks_in_hive".into(), Tag::Int(*ticks)),
                ("min_ticks_in_hive".into(), Tag::Int(600)),
            ])
        })
        .collect();
    Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:beehive".into())),
        ("x".into(), Tag::Int(pos.x())),
        ("y".into(), Tag::Int(pos.y())),
        ("z".into(), Tag::Int(pos.z())),
        ("bees".into(), Tag::List(bees)),
        ("Bukkit.MaxEntities".into(), Tag::Int(3)),
    ])
}

impl LeaveVineDecorator {
    /// `LeaveVineDecorator.place`. Every leaf draws all four side chances even
    /// when an earlier side already received a vine.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        leaves: &[BlockPos],
    ) {
        for leaf in leaves {
            for (dx, dz, side) in [
                (-1, 0, "east"),
                (1, 0, "west"),
                (0, -1, "south"),
                (0, 1, "north"),
            ] {
                if random.next_float() < self.probability {
                    let pos = leaf.offset(dx, 0, dz);
                    if is_air(level, pos) {
                        add_hanging_vine(level, pos, side);
                    }
                }
            }
        }
    }
}

fn add_hanging_vine(level: &mut dyn FeatureLevel, pos: BlockPos, side: &str) {
    level.set_block(pos, vine_state(side));
    let mut below = pos.below();
    for _ in 0..4 {
        if !is_air(level, below) {
            break;
        }
        level.set_block(below, vine_state(side));
        below = below.below();
    }
}

/// `TreeDecorator.Context.placeVine`: the default vine state with one side
/// property enabled.
fn vine_state(side: &str) -> BlockState {
    let properties = ["east", "north", "south", "up", "west"]
        .into_iter()
        .map(|property| {
            let value = if property == side { "true" } else { "false" };
            (property.to_owned(), value.to_owned())
        })
        .collect();
    BlockState {
        name: "minecraft:vine".into(),
        properties,
    }
}

impl CocoaDecorator {
    /// `CocoaDecorator.place` for the lowest three log layers.
    fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        logs: &[BlockPos],
    ) {
        let Some(first) = logs.first() else {
            return;
        };
        if random.next_float() >= self.probability {
            return;
        }
        let first_y = first.y();
        for log in logs.iter().filter(|pos| pos.y() - first_y <= 2) {
            for (name, dx, dz) in [
                ("north", 0, -1),
                ("east", 1, 0),
                ("south", 0, 1),
                ("west", -1, 0),
            ] {
                if random.next_float() <= 0.25 {
                    let pos = log.offset(-dx, 0, -dz);
                    if is_air(level, pos) {
                        let age = random.next_int_bound(3).to_string();
                        level.set_block(
                            pos,
                            BlockState {
                                name: "minecraft:cocoa".into(),
                                properties: vec![
                                    ("age".into(), age),
                                    ("facing".into(), name.into()),
                                ],
                            },
                        );
                    }
                }
            }
        }
    }
}

/// `TrunkVineDecorator.place`.
fn place_trunk_vines(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    logs: &[BlockPos],
) {
    for log in logs {
        for (dx, dz, side) in [
            (-1, 0, "east"),
            (1, 0, "west"),
            (0, -1, "south"),
            (0, 1, "north"),
        ] {
            if random.next_int_bound(3) > 0 {
                let pos = log.offset(dx, 0, dz);
                if is_air(level, pos) {
                    level.set_block(pos, vine_state(side));
                }
            }
        }
    }
}

fn is_air(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| is_air_name(&state.name))
}

/// `TreeFeature.isAirOrLeaves` used by the dark oak trunk.
fn is_air_or_leaves(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| is_air_name(&state.name) || state.name.ends_with("_leaves"))
}

/// `FancyTrunkPlacer.treeShape`.
fn fancy_tree_shape(height: i32, current_y: i32) -> f32 {
    if (current_y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let half = height as f32 / 2.0;
    let delta = half - current_y as f32;
    if delta == 0.0 {
        return half * 0.5;
    }
    if delta.abs() >= half {
        return 0.0;
    }
    (half * half - delta * delta).sqrt() * 0.5
}

/// `FancyTrunkPlacer.trimBranches`.
fn fancy_trim_branches(max_height: i32, current_height: i32) -> bool {
    current_height as f64 >= f64::from(max_height) * 0.2
}

/// `FancyTrunkPlacer.getLogAxis`.
fn fancy_log_axis(base: BlockPos, other: BlockPos) -> &'static str {
    let dx = (other.x() - base.x()).abs();
    let dz = (other.z() - base.z()).abs();
    let max = dx.max(dz);
    if max == 0 {
        "y"
    } else if dx == max {
        "x"
    } else {
        "z"
    }
}

/// `TrunkPlacer.isFree`: a replaceable tree position or an existing log.
fn is_tree_free(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| valid_tree_pos_state(state) || is_log(state))
}

/// `BlockPos.distManhattan`.
fn block_manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x()).abs() + (a.y() - b.y()).abs() + (a.z() - b.z()).abs()
}

/// `Collections.shuffle` / `Util.shuffle`: Fisher-Yates from the end.
fn shuffle<T>(values: &mut [T], random: &mut dyn RandomSource) {
    for index in (2..=values.len()).rev() {
        let swap_with = random.next_int_bound(index as i32) as usize;
        values.swap(index - 1, swap_with);
    }
}

/// Heightmap height (highest taken block) used by `place_on_ground`.
fn heightmap_no_leaves(level: &dyn FeatureLevel, pos: BlockPos) -> i32 {
    let max_y = level.min_y() + level.height() - 1;
    for y in (level.min_y()..=max_y).rev() {
        let state = level.get_block(BlockPos::new(pos.x(), y, pos.z()));
        if state.is_some_and(|state| !is_air_name(&state.name) && !state.name.ends_with("_leaves"))
        {
            return y;
        }
    }
    level.min_y()
}

/// `BlockStateBase.isSolidRender` for the surface blocks this slice supports.
/// Full block-behavior metadata is not ported yet; unknown blocks are skipped
/// rather than treated as full cubes. This is enough for leaf litter on the
/// grass/dirt/podzol floors of the supported forests and plains.
fn is_solid_render(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:grass_block"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:mycelium"
            | "minecraft:rooted_dirt"
            | "minecraft:moss_block"
            | "minecraft:mud"
            | "minecraft:packed_mud"
            | "minecraft:clay"
            | "minecraft:gravel"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:sandstone"
            | "minecraft:stone"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:deepslate"
            | "minecraft:tuff"
            | "minecraft:calcite"
            | "minecraft:snow_block"
            | "minecraft:dirt_path"
    )
}

/// `TreeFeature.validTreePos` against the chunk-bounded region.
fn valid_tree_pos(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level.get_block(pos).is_none_or(valid_tree_pos_state)
}

fn valid_tree_pos_state(state: &BlockState) -> bool {
    is_air_name(&state.name)
        || state.name.ends_with("_leaves")
        || matches!(
            state.name.as_str(),
            "minecraft:water"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
                | "minecraft:short_grass"
                | "minecraft:tall_grass"
                | "minecraft:fern"
                | "minecraft:large_fern"
                | "minecraft:bush"
                | "minecraft:firefly_bush"
                | "minecraft:sweet_berry_bush"
                | "minecraft:dead_bush"
                | "minecraft:snow"
                | "minecraft:vine"
                | "minecraft:glow_lichen"
        )
}

fn is_log(state: &BlockState) -> bool {
    state.name.ends_with("_log") || state.name.ends_with("_wood")
}

fn is_dirt(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
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

fn component<'a>(object: &'a Map<String, Value>, key: &str) -> io::Result<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("tree config is missing the `{key}` object")))
}

fn simple_state(object: &Map<String, Value>, key: &str) -> io::Result<Option<BlockState>> {
    let provider = object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("tree config is missing the `{key}` provider")))?;
    if provider.get("type").and_then(Value::as_str) != Some("minecraft:simple_state_provider") {
        return Ok(None);
    }
    let state = provider
        .get("state")
        .ok_or_else(|| invalid(format!("{key} is missing `state`")))?;
    parse_block_state(state).map(Some)
}

/// Parses the supported tree decorators. A known vanilla decorator that is not
/// ported makes the whole tree configuration unsupported, so a decorated tree
/// is never generated without its decoration.
fn parse_decorators(
    object: &Map<String, Value>,
    pale_moss_patch: Option<&ConfiguredFeature>,
) -> io::Result<Option<Vec<TreeDecorator>>> {
    let Some(list) = object.get("decorators") else {
        return Ok(Some(Vec::new()));
    };
    parse_decorator_list(list, pale_moss_patch)
}

fn parse_decorator_list(
    list: &Value,
    pale_moss_patch: Option<&ConfiguredFeature>,
) -> io::Result<Option<Vec<TreeDecorator>>> {
    let list = list
        .as_array()
        .ok_or_else(|| invalid("tree `decorators` must be a list"))?;
    let mut decorators = Vec::with_capacity(list.len());
    for value in list {
        let decorator = value
            .as_object()
            .ok_or_else(|| invalid("tree decorator must be an object"))?;
        let kind = decorator
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("tree decorator is missing `type`"))?;
        match kind {
            "minecraft:place_on_ground" => {
                let tries = optional_int(decorator, "tries", 128)?;
                let radius = optional_int(decorator, "radius", 2)?;
                let height = optional_int(decorator, "height", 1)?;
                if tries <= 0 || radius < 0 || height < 0 {
                    return Err(invalid(
                        "place_on_ground needs positive `tries` and non-negative `radius`/`height`",
                    ));
                }
                let provider = decorator
                    .get("block_state_provider")
                    .ok_or_else(|| invalid("place_on_ground is missing `block_state_provider`"))?;
                let Some(block_state_provider) = parse_state_provider(provider)? else {
                    return Ok(None);
                };
                decorators.push(TreeDecorator::PlaceOnGround(PlaceOnGroundDecorator {
                    tries,
                    radius,
                    height,
                    block_state_provider,
                }));
            }
            "minecraft:alter_ground" => {
                let provider = decorator
                    .get("provider")
                    .ok_or_else(|| invalid("alter_ground is missing `provider`"))?;
                let Some(provider) = parse_state_provider(provider)? else {
                    return Ok(None);
                };
                decorators.push(TreeDecorator::AlterGround(AlterGroundDecorator {
                    provider,
                }));
            }
            "minecraft:attached_to_leaves" => {
                let probability = parse_probability(decorator, "attached_to_leaves")?;
                let exclusion_radius_xz = int(decorator, "exclusion_radius_xz")?;
                let exclusion_radius_y = int(decorator, "exclusion_radius_y")?;
                let required_empty_blocks = int(decorator, "required_empty_blocks")?;
                if !(0..=16).contains(&exclusion_radius_xz)
                    || !(0..=16).contains(&exclusion_radius_y)
                    || !(1..=16).contains(&required_empty_blocks)
                {
                    return Err(invalid(
                        "attached_to_leaves radii or empty-block count are out of range",
                    ));
                }
                let provider = decorator
                    .get("block_provider")
                    .ok_or_else(|| invalid("attached_to_leaves is missing `block_provider`"))?;
                let Some(block_provider) = parse_state_provider(provider)? else {
                    return Ok(None);
                };
                let direction_values = decorator
                    .get("directions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("attached_to_leaves is missing `directions`"))?;
                if direction_values.is_empty() {
                    return Err(invalid("attached_to_leaves needs at least one direction"));
                }
                let mut directions = Vec::with_capacity(direction_values.len());
                for value in direction_values {
                    let name = value
                        .as_str()
                        .ok_or_else(|| invalid("attached_to_leaves direction must be a string"))?;
                    let direction = parse_direction(name).ok_or_else(|| {
                        invalid(format!("unknown attached_to_leaves direction `{name}`"))
                    })?;
                    directions.push(direction);
                }
                decorators.push(TreeDecorator::AttachedToLeaves(AttachedToLeavesDecorator {
                    probability,
                    exclusion_radius_xz,
                    exclusion_radius_y,
                    block_provider,
                    required_empty_blocks,
                    directions,
                }));
            }
            "minecraft:attached_to_logs" => {
                let probability = parse_probability(decorator, "attached_to_logs")?;
                let provider = decorator
                    .get("block_provider")
                    .ok_or_else(|| invalid("attached_to_logs is missing `block_provider`"))?;
                let Some(block_provider) = parse_state_provider(provider)? else {
                    return Ok(None);
                };
                let direction_values = decorator
                    .get("directions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("attached_to_logs is missing `directions`"))?;
                if direction_values.is_empty() {
                    return Err(invalid("attached_to_logs needs at least one direction"));
                }
                let mut directions = Vec::with_capacity(direction_values.len());
                for value in direction_values {
                    let name = value
                        .as_str()
                        .ok_or_else(|| invalid("attached_to_logs direction must be a string"))?;
                    let direction = parse_direction(name).ok_or_else(|| {
                        invalid(format!("unknown attached_to_logs direction `{name}`"))
                    })?;
                    directions.push(direction);
                }
                decorators.push(TreeDecorator::AttachedToLogs(AttachedToLogsDecorator {
                    probability,
                    block_provider,
                    directions,
                }));
            }
            "minecraft:beehive" => {
                let probability = parse_probability(decorator, "beehive")?;
                decorators.push(TreeDecorator::Beehive(BeehiveDecorator { probability }));
            }
            "minecraft:leave_vine" => {
                let probability = parse_probability(decorator, "leave_vine")?;
                decorators.push(TreeDecorator::LeaveVine(LeaveVineDecorator { probability }));
            }
            "minecraft:trunk_vine" => decorators.push(TreeDecorator::TrunkVine),
            "minecraft:cocoa" => {
                let probability = parse_probability(decorator, "cocoa")?;
                decorators.push(TreeDecorator::Cocoa(CocoaDecorator { probability }));
            }
            "minecraft:creaking_heart" => {
                let probability = parse_probability(decorator, "creaking_heart")?;
                decorators.push(TreeDecorator::CreakingHeart(CreakingHeartDecorator {
                    probability,
                }));
            }
            "minecraft:pale_moss" => {
                let Some(patch) = pale_moss_patch else {
                    return Ok(None);
                };
                if !patch.is_supported() {
                    return Ok(None);
                }
                decorators.push(TreeDecorator::PaleMoss(PaleMossDecorator {
                    leaves_probability: parse_chance(decorator, "leaves_probability", "pale_moss")?,
                    trunk_probability: parse_chance(decorator, "trunk_probability", "pale_moss")?,
                    ground_probability: parse_chance(decorator, "ground_probability", "pale_moss")?,
                    patch: Box::new(patch.clone()),
                }));
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(decorators))
}

/// Parses a `minecraft:fallen_tree` configuration. `Ok(None)` means a
/// component is not ported yet; malformed JSON is an error.
pub fn parse_fallen_tree_config(config: &Value) -> io::Result<Option<FallenTreeFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("fallen tree config must be an object"))?;
    let Some(trunk_provider) = parse_state_provider(
        object
            .get("trunk_provider")
            .ok_or_else(|| invalid("fallen tree is missing `trunk_provider`"))?,
    )?
    else {
        return Ok(None);
    };
    let log_length = parse_int_provider(
        object
            .get("log_length")
            .ok_or_else(|| invalid("fallen tree is missing `log_length`"))?,
    )?;
    if !log_length.is_supported() {
        return Ok(None);
    }
    let Some(stump_decorators) = parse_decorator_list(
        object
            .get("stump_decorators")
            .ok_or_else(|| invalid("fallen tree is missing `stump_decorators`"))?,
        None,
    )?
    else {
        return Ok(None);
    };
    let Some(log_decorators) = parse_decorator_list(
        object
            .get("log_decorators")
            .ok_or_else(|| invalid("fallen tree is missing `log_decorators`"))?,
        None,
    )?
    else {
        return Ok(None);
    };

    Ok(Some(FallenTreeFeature {
        trunk_provider,
        log_length,
        stump_decorators,
        log_decorators,
    }))
}

fn parse_probability(object: &Map<String, Value>, kind: &str) -> io::Result<f32> {
    parse_chance(object, "probability", kind)
}

fn parse_chance(object: &Map<String, Value>, key: &str, kind: &str) -> io::Result<f32> {
    let chance = object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| invalid(format!("{kind} is missing `{key}`")))? as f32;
    if !(0.0..=1.0).contains(&chance) {
        return Err(invalid(format!("{kind} `{key}` must be between 0 and 1")));
    }
    Ok(chance)
}

fn parse_block_set(value: &Value) -> io::Result<Option<BlockSet>> {
    match value {
        Value::String(tag) => {
            if let Some(tag) = tag.strip_prefix('#') {
                match tag {
                    "minecraft:mangrove_roots_can_grow_through" => {
                        Ok(Some(BlockSet::MangroveRootsCanGrowThrough))
                    }
                    "minecraft:mangrove_logs_can_grow_through" => {
                        Ok(Some(BlockSet::MangroveLogsCanGrowThrough))
                    }
                    _ => Ok(None),
                }
            } else {
                Ok(Some(BlockSet::Explicit(HashSet::from([tag.to_owned()]))))
            }
        }
        Value::Array(values) => {
            let mut names = HashSet::with_capacity(values.len());
            for value in values {
                names.insert(
                    value
                        .as_str()
                        .ok_or_else(|| invalid("block-set entries must be strings"))?
                        .to_owned(),
                );
            }
            Ok(Some(BlockSet::Explicit(names)))
        }
        _ => Err(invalid("block set must be a tag string or a block list")),
    }
}

fn parse_block_names(value: &Value, field: &str) -> io::Result<HashSet<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{field} must be a block list")))?;
    let mut names = HashSet::with_capacity(values.len());
    for value in values {
        names.insert(
            value
                .as_str()
                .ok_or_else(|| invalid(format!("{field} entries must be strings")))?
                .to_owned(),
        );
    }
    Ok(names)
}

fn parse_root_placer(value: &Value) -> io::Result<Option<RootPlacer>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("root placer must be an object"))?;
    if object.get("type").and_then(Value::as_str) != Some("minecraft:mangrove_root_placer") {
        return Ok(None);
    }

    let trunk_offset_y = parse_int_provider(
        object
            .get("trunk_offset_y")
            .ok_or_else(|| invalid("mangrove root placer is missing `trunk_offset_y`"))?,
    )?;
    if !trunk_offset_y.is_supported() {
        return Ok(None);
    }
    let Some(root_provider) = parse_state_provider(
        object
            .get("root_provider")
            .ok_or_else(|| invalid("mangrove root placer is missing `root_provider`"))?,
    )?
    else {
        return Ok(None);
    };

    let above_root_placement = match object.get("above_root_placement") {
        None => None,
        Some(value) => {
            let above = value
                .as_object()
                .ok_or_else(|| invalid("above root placement must be an object"))?;
            let Some(provider) =
                parse_state_provider(above.get("above_root_provider").ok_or_else(|| {
                    invalid("above root placement is missing `above_root_provider`")
                })?)?
            else {
                return Ok(None);
            };
            Some(AboveRootPlacement {
                above_root_provider: provider,
                above_root_placement_chance: parse_chance(
                    above,
                    "above_root_placement_chance",
                    "above root placement",
                )?,
            })
        }
    };

    let mangrove = object
        .get("mangrove_root_placement")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("mangrove root placer is missing `mangrove_root_placement`"))?;
    let can_grow_through = match parse_block_set(
        mangrove
            .get("can_grow_through")
            .ok_or_else(|| invalid("mangrove roots are missing `can_grow_through`"))?,
    )? {
        Some(set) => set,
        None => return Ok(None),
    };
    let Some(muddy_roots_provider) = parse_state_provider(
        mangrove
            .get("muddy_roots_provider")
            .ok_or_else(|| invalid("mangrove roots are missing `muddy_roots_provider`"))?,
    )?
    else {
        return Ok(None);
    };
    let max_root_width = int(mangrove, "max_root_width")?;
    if !(1..=12).contains(&max_root_width) {
        return Err(invalid("mangrove max_root_width must be between 1 and 12"));
    }
    let max_root_length = int(mangrove, "max_root_length")?;
    if !(1..=64).contains(&max_root_length) {
        return Err(invalid("mangrove max_root_length must be between 1 and 64"));
    }

    Ok(Some(RootPlacer {
        trunk_offset_y,
        root_provider,
        above_root_placement,
        mangrove: MangroveRootPlacement {
            can_grow_through,
            muddy_roots_in: parse_block_names(
                mangrove
                    .get("muddy_roots_in")
                    .ok_or_else(|| invalid("mangrove roots are missing `muddy_roots_in`"))?,
                "mangrove `muddy_roots_in`",
            )?,
            muddy_roots_provider,
            max_root_width,
            max_root_length,
            random_skew_chance: parse_chance(
                mangrove,
                "random_skew_chance",
                "mangrove root placement",
            )?,
        },
    }))
}

fn parse_state_provider(value: &Value) -> io::Result<Option<BlockStateProvider>> {
    let provider = value
        .as_object()
        .ok_or_else(|| invalid("block state provider must be an object"))?;
    let kind = provider
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("block state provider is missing `type`"))?;
    match kind {
        "minecraft:simple_state_provider" => {
            let state = provider
                .get("state")
                .ok_or_else(|| invalid("simple state provider is missing `state`"))?;
            Ok(Some(BlockStateProvider::simple(parse_block_state(state)?)))
        }
        "minecraft:weighted_state_provider" => {
            let entries = provider
                .get("entries")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("weighted state provider is missing `entries`"))?;
            if entries.is_empty() {
                return Err(invalid("weighted state provider has no entries"));
            }
            let mut parsed = Vec::with_capacity(entries.len());
            let mut total_weight = 0i64;
            for entry in entries {
                let entry = entry
                    .as_object()
                    .ok_or_else(|| invalid("weighted state provider entry must be an object"))?;
                let weight = entry
                    .get("weight")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| invalid("weighted state provider entry is missing `weight`"))?;
                if weight <= 0 {
                    return Err(invalid("weighted state provider weights must be positive"));
                }
                total_weight += weight;
                if total_weight > i64::from(i32::MAX) {
                    return Err(invalid("weighted state provider weights exceed i32"));
                }
                let data = entry
                    .get("data")
                    .ok_or_else(|| invalid("weighted state provider entry is missing `data`"))?;
                parsed.push((weight as i32, parse_block_state(data)?));
            }
            Ok(Some(BlockStateProvider::Weighted {
                entries: parsed,
                total_weight: total_weight as i32,
            }))
        }
        "minecraft:randomized_int_state_provider" => {
            let source = provider
                .get("source")
                .ok_or_else(|| invalid("randomized int state provider is missing `source`"))?;
            let Some(source) = parse_state_provider(source)? else {
                return Ok(None);
            };
            let property = provider
                .get("property")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("randomized int state provider is missing `property`"))?
                .to_owned();
            let values =
                parse_int_provider(provider.get("values").ok_or_else(|| {
                    invalid("randomized int state provider is missing `values`")
                })?)?;
            if !values.is_supported() {
                return Ok(None);
            }
            Ok(Some(BlockStateProvider::RandomizedInt {
                source: Box::new(source),
                property,
                values,
            }))
        }
        _ => Ok(None),
    }
}

fn parse_direction(name: &str) -> Option<(i32, i32, i32)> {
    match name {
        "down" => Some((0, -1, 0)),
        "up" => Some((0, 1, 0)),
        "north" => Some((0, 0, -1)),
        "south" => Some((0, 0, 1)),
        "west" => Some((-1, 0, 0)),
        "east" => Some((1, 0, 0)),
        _ => None,
    }
}

/// Parses a bare `UniformInt` JSON object (`min_inclusive`/`max_inclusive`).
fn parse_uniform_int(value: &Value) -> io::Result<IntProvider> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("uniform int must be an object"))?;
    if object.contains_key("type") {
        return parse_int_provider(value);
    }
    let min_inclusive = int(object, "min_inclusive")?;
    let max_inclusive = int(object, "max_inclusive")?;
    if min_inclusive > max_inclusive {
        return Err(invalid("uniform int minimum exceeds maximum"));
    }
    Ok(IntProvider::Uniform {
        min_inclusive,
        max_inclusive,
    })
}

fn int(object: &Map<String, Value>, key: &str) -> io::Result<i32> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| invalid(format!("tree config needs an i32 `{key}`")))
}

fn optional_int(object: &Map<String, Value>, key: &str, default: i32) -> io::Result<i32> {
    match object.get(key) {
        None => Ok(default),
        Some(_) => int(object, key),
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::worldgen::feature::WorldGenRegion;
    use folia_random::LegacyRandomSource;
    use serde_json::json;

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;

    fn oak_value() -> Value {
        json!({
            "type": "minecraft:tree",
            "config": {
                "decorators": [],
                "dirt_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": {"Name": "minecraft:dirt"}
                },
                "foliage_placer": {
                    "type": "minecraft:blob_foliage_placer",
                    "height": 3,
                    "offset": 0,
                    "radius": 2
                },
                "foliage_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": {
                        "Name": "minecraft:oak_leaves",
                        "Properties": {
                            "distance": "7",
                            "persistent": "false",
                            "waterlogged": "false"
                        }
                    }
                },
                "force_dirt": false,
                "ignore_vines": true,
                "minimum_size": {
                    "type": "minecraft:two_layers_feature_size",
                    "limit": 1,
                    "lower_size": 0,
                    "upper_size": 1
                },
                "trunk_placer": {
                    "type": "minecraft:straight_trunk_placer",
                    "base_height": 4,
                    "height_rand_a": 2,
                    "height_rand_b": 0
                },
                "trunk_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": {"Name": "minecraft:oak_log", "Properties": {"axis": "y"}}
                }
            }
        })
    }

    fn oak() -> TreeFeature {
        parse_tree_config(&oak_value()["config"])
            .unwrap()
            .expect("oak tree is supported")
    }

    fn vanilla_tree_config(name: &str) -> Value {
        let data: Value = serde_json::from_str(include_str!("../../../assets/feature_data.json"))
            .expect("vanilla feature data parses");
        data["configured"][name].clone()
    }

    fn mangrove_water_chunk() -> Chunk {
        let mut chunk = flat_chunk();
        for x in 0..16 {
            for z in 0..16 {
                for y in 56..=62 {
                    chunk.set_block_state(
                        x,
                        y,
                        z,
                        BlockState {
                            name: "minecraft:stone".into(),
                            properties: vec![],
                        },
                    );
                }
                chunk.set_block_state(
                    x,
                    63,
                    z,
                    BlockState {
                        name: "minecraft:mud".into(),
                        properties: vec![],
                    },
                );
                chunk.set_block_state(
                    x,
                    64,
                    z,
                    BlockState {
                        name: "minecraft:water".into(),
                        properties: vec![("level".into(), "0".into())],
                    },
                );
            }
        }
        chunk
    }

    fn oak_log() -> BlockState {
        BlockState {
            name: "minecraft:oak_log".into(),
            properties: vec![("axis".into(), "y".into())],
        }
    }

    fn flat_chunk() -> Chunk {
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block_state(
                    x,
                    63,
                    z,
                    BlockState {
                        name: "minecraft:grass_block".into(),
                        properties: vec![("snowy".into(), "false".into())],
                    },
                );
            }
        }
        chunk
    }

    #[test]
    fn oak_tree_places_a_trunk_and_blob_foliage() {
        let tree = oak();
        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        let mut dirt = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..80 {
                    let state = chunk.block_state(x, y, z).unwrap();
                    match state.name.as_str() {
                        "minecraft:oak_log" => logs += 1,
                        "minecraft:oak_leaves" => leaves += 1,
                        "minecraft:dirt" => dirt += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!((4..=6).contains(&logs), "unexpected trunk height {logs}");
        assert!(leaves > 0, "tree placed no foliage");
        assert_eq!(dirt, 1, "the grass block under the trunk becomes dirt");
        assert_eq!(
            chunk.block_state(8, 64, 8).unwrap().name,
            "minecraft:oak_log"
        );
    }

    #[test]
    fn place_on_ground_decorator_places_leaf_litter() {
        let mut value = oak_value();
        value["config"]["decorators"] = json!([{
            "type": "minecraft:place_on_ground",
            "tries": 256,
            "radius": 2,
            "height": 1,
            "block_state_provider": {
                "type": "minecraft:weighted_state_provider",
                "entries": [
                    {
                        "weight": 1,
                        "data": {
                            "Name": "minecraft:leaf_litter",
                            "Properties": {"facing": "north", "segment_amount": "1"}
                        }
                    },
                    {
                        "weight": 1,
                        "data": {
                            "Name": "minecraft:leaf_litter",
                            "Properties": {"facing": "east", "segment_amount": "2"}
                        }
                    }
                ]
            }
        }]);
        let tree = parse_tree_config(&value["config"])
            .unwrap()
            .expect("place_on_ground is supported");
        assert_eq!(tree.decorator_count, 1);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut leaf_litter = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..80 {
                    let state = chunk.block_state(x, y, z).unwrap();
                    if state.name == "minecraft:leaf_litter" {
                        leaf_litter += 1;
                        assert!(!state.properties.is_empty());
                    }
                }
            }
        }
        assert!(leaf_litter > 0, "place_on_ground placed no leaf litter");
    }
    #[test]
    fn beehive_decorator_places_a_bee_nest() {
        let mut value = oak_value();
        value["config"]["decorators"] = json!([{"type": "minecraft:beehive", "probability": 1.0}]);
        let tree = parse_tree_config(&value["config"])
            .unwrap()
            .expect("beehive is supported");

        let mut chunk = flat_chunk();
        let entities = {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
            level.block_entities().to_vec()
        };

        let mut nests = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..80 {
                    let state = chunk.block_state(x, y, z).unwrap();
                    if state.name == "minecraft:bee_nest" {
                        nests += 1;
                        assert!(
                            state
                                .properties
                                .iter()
                                .any(|(key, value)| key == "facing" && value == "south")
                        );
                    }
                }
            }
        }
        assert_eq!(nests, 1, "beehive decorator should place one nest");
        assert_eq!(entities.len(), 1, "the nest carries one block entity");
        let entity = &entities[0];
        assert_eq!(
            entity.nbt.get("id").and_then(Tag::as_str),
            Some("minecraft:beehive")
        );
        let bees = entity
            .nbt
            .get("bees")
            .and_then(Tag::as_list)
            .expect("the nest stores a `bees` list");
        assert!(
            (2..=3).contains(&bees.len()),
            "vanilla stores two or three bees"
        );
        for bee in bees {
            let bee = bee.as_compound().expect("each bee is a compound");
            let get_int = |name: &str| {
                bee.iter()
                    .find(|(key, _)| key == name)
                    .and_then(|(_, value)| value.as_int())
            };
            assert_eq!(get_int("min_ticks_in_hive"), Some(600));
            let ticks = get_int("ticks_in_hive").expect("ticks_in_hive");
            assert!((0..599).contains(&ticks));
            assert_eq!(
                bee.iter()
                    .find(|(key, _)| key == "entity_data")
                    .and_then(|(_, value)| value.get("id"))
                    .and_then(Tag::as_str),
                Some("minecraft:bee")
            );
        }
    }

    #[test]
    fn vanilla_jungle_and_swamp_trees_are_supported() {
        let jungle = parse_tree_config(&vanilla_tree_config("jungle_tree")["config"])
            .unwrap()
            .expect("jungle_tree should be supported");
        assert_eq!(jungle.decorator_count, 3);
        let swamp = parse_tree_config(&vanilla_tree_config("swamp_oak")["config"])
            .unwrap()
            .expect("swamp_oak should be supported");
        assert_eq!(swamp.decorator_count, 1);
    }

    #[test]
    fn spruce_and_pine_trees_parse_and_place_layered_foliage() {
        for name in ["spruce", "pine"] {
            let tree = parse_tree_config(&vanilla_tree_config(name)["config"])
                .unwrap()
                .unwrap_or_else(|| panic!("{name} should be supported"));
            assert_eq!(tree.decorator_count, 0);

            let mut chunk = flat_chunk();
            {
                let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
                let mut random = LegacyRandomSource::new(7);
                assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
            }

            let mut logs = 0;
            let mut leaves = 0;
            for x in 0..16 {
                for z in 0..16 {
                    for y in 60..100 {
                        let Some(state) = chunk.block_state(x, y, z) else {
                            continue;
                        };
                        if state.name == "minecraft:spruce_log" {
                            logs += 1;
                        } else if state.name == "minecraft:spruce_leaves" {
                            leaves += 1;
                        }
                    }
                }
            }
            println!("{name} logs={logs} leaves={leaves}");
            assert!(logs > 0, "{name} should place a trunk");
            assert!(leaves > 0, "{name} should place foliage");
        }
    }

    #[test]
    fn dark_oak_tree_parses_and_places_a_2x2_trunk() {
        let tree = parse_tree_config(&vanilla_tree_config("dark_oak")["config"])
            .unwrap()
            .expect("dark_oak should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..100 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:dark_oak_log" {
                        logs += 1;
                    } else if state.name == "minecraft:dark_oak_leaves" {
                        leaves += 1;
                    }
                }
            }
        }
        println!("dark_oak logs={logs} leaves={leaves}");
        assert!(logs >= 4, "dark oak should place a 2x2 trunk");
        assert!(leaves > 0, "dark oak should place foliage");
    }

    #[test]
    fn acacia_tree_parses_and_places_forking_branches() {
        let tree = parse_tree_config(&vanilla_tree_config("acacia")["config"])
            .unwrap()
            .expect("acacia should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut forked = false;
        for seed in 0..16 {
            let mut chunk = flat_chunk();
            {
                let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
                let mut random = LegacyRandomSource::new(seed);
                assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
            }

            let mut logs = 0;
            let mut leaves = 0;
            for x in 0..16 {
                for z in 0..16 {
                    for y in 60..110 {
                        let Some(state) = chunk.block_state(x, y, z) else {
                            continue;
                        };
                        if state.name == "minecraft:acacia_log" {
                            logs += 1;
                            if x != 8 || z != 8 {
                                forked = true;
                            }
                        } else if state.name == "minecraft:acacia_leaves" {
                            leaves += 1;
                        }
                    }
                }
            }
            if seed == 7 {
                println!("acacia logs={logs} leaves={leaves}");
                assert!(logs > 0, "acacia should place a trunk");
                assert!(leaves > 0, "acacia should place foliage");
            }
        }
        assert!(forked, "acacia should place at least one leaning branch");
    }

    #[test]
    fn fancy_oak_tree_parses_and_places_axis_branches() {
        let tree = parse_tree_config(&vanilla_tree_config("fancy_oak")["config"])
            .unwrap()
            .expect("fancy_oak should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        let mut horizontal_logs = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..110 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:oak_log" {
                        logs += 1;
                        if state
                            .properties
                            .iter()
                            .any(|(key, value)| key == "axis" && value != "y")
                        {
                            horizontal_logs += 1;
                        }
                    } else if state.name == "minecraft:oak_leaves" {
                        leaves += 1;
                    }
                }
            }
        }
        println!("fancy_oak logs={logs} leaves={leaves} horizontal_logs={horizontal_logs}");
        assert!(logs > 0, "fancy oak should place a trunk");
        assert!(leaves > 0, "fancy oak should place foliage");
        assert!(horizontal_logs > 0, "fancy oak should place axis branches");
    }

    #[test]
    fn fancy_oak_can_clip_to_its_minimum_height() {
        let tree = parse_tree_config(&vanilla_tree_config("fancy_oak")["config"])
            .unwrap()
            .expect("fancy_oak should be supported");
        let mut chunk = flat_chunk();
        chunk.set_block_state(
            8,
            70,
            8,
            BlockState {
                name: "minecraft:stone".into(),
                properties: Vec::new(),
            },
        );
        let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        let mut random = LegacyRandomSource::new(7);
        assert!(
            tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)),
            "fancy oak should clip to its minimum height of four"
        );
    }

    #[test]
    fn cherry_tree_parses_and_places_branches() {
        let tree = parse_tree_config(&vanilla_tree_config("cherry")["config"])
            .unwrap()
            .expect("cherry should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        let mut horizontal_logs = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..110 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:cherry_log" {
                        logs += 1;
                        if state
                            .properties
                            .iter()
                            .any(|(key, value)| key == "axis" && value != "y")
                        {
                            horizontal_logs += 1;
                        }
                    } else if state.name == "minecraft:cherry_leaves" {
                        leaves += 1;
                    }
                }
            }
        }
        println!("cherry logs={logs} leaves={leaves} horizontal_logs={horizontal_logs}");
        assert!(logs > 0, "cherry should place a trunk");
        assert!(leaves > 0, "cherry should place foliage");
        assert!(horizontal_logs > 0, "cherry should place a branch");
    }

    #[test]
    fn mega_jungle_tree_parses_and_places_branches() {
        let tree = parse_tree_config(&vanilla_tree_config("mega_jungle_tree")["config"])
            .unwrap()
            .expect("mega_jungle_tree should be supported");
        assert_eq!(tree.decorator_count, 2);

        let mut branch_logs_total = 0;
        for seed in 0..16 {
            let mut chunk = flat_chunk();
            {
                let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
                let mut random = LegacyRandomSource::new(seed);
                assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
            }

            let mut logs = 0;
            let mut branch_logs = 0;
            let mut leaves = 0;
            for x in 0..16 {
                for z in 0..16 {
                    for y in 60..140 {
                        let Some(state) = chunk.block_state(x, y, z) else {
                            continue;
                        };
                        if state.name == "minecraft:jungle_log" {
                            logs += 1;
                            if !(8..=9).contains(&x) || !(8..=9).contains(&z) {
                                branch_logs += 1;
                            }
                        } else if state.name == "minecraft:jungle_leaves" {
                            leaves += 1;
                        }
                    }
                }
            }
            if seed == 7 {
                println!("mega_jungle logs={logs} branch_logs={branch_logs} leaves={leaves}");
            }
            assert!(logs >= 4, "mega jungle should place a 2x2 trunk");
            assert!(leaves > 0, "mega jungle should place foliage");
            branch_logs_total += branch_logs;
        }
        assert!(branch_logs_total > 0, "mega jungle should place branches");
    }

    #[test]
    fn mega_spruce_and_pine_place_alter_ground() {
        for name in ["mega_spruce", "mega_pine"] {
            let tree = parse_tree_config(&vanilla_tree_config(name)["config"])
                .unwrap()
                .unwrap_or_else(|| panic!("{name} should be supported"));
            assert_eq!(tree.decorator_count, 1);

            let mut chunk = flat_chunk();
            {
                let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
                let mut random = LegacyRandomSource::new(7);
                assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
            }

            let mut logs = 0;
            let mut leaves = 0;
            let mut podzol = 0;
            for x in 0..16 {
                for z in 0..16 {
                    for y in 60..130 {
                        let Some(state) = chunk.block_state(x, y, z) else {
                            continue;
                        };
                        if state.name == "minecraft:spruce_log" {
                            logs += 1;
                        } else if state.name == "minecraft:spruce_leaves" {
                            leaves += 1;
                        } else if state.name == "minecraft:podzol" {
                            podzol += 1;
                        }
                    }
                }
            }
            println!("{name} logs={logs} leaves={leaves} podzol={podzol}");
            assert!(logs > 0, "{name} should place a trunk");
            assert!(leaves > 0, "{name} should place foliage");
            assert!(podzol > 0, "{name} should alter the ground");
        }
    }

    #[test]
    fn azalea_tree_parses_bending_trunk_and_random_spread_foliage() {
        let tree = parse_tree_config(&vanilla_tree_config("azalea_tree")["config"])
            .unwrap()
            .expect("azalea_tree should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut azalea = 0;
        let mut flowering = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..85 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    match state.name.as_str() {
                        "minecraft:oak_log" => logs += 1,
                        "minecraft:azalea_leaves" => azalea += 1,
                        "minecraft:flowering_azalea_leaves" => flowering += 1,
                        _ => {}
                    }
                }
            }
        }
        println!("azalea logs={logs} azalea_leaves={azalea} flowering_azalea_leaves={flowering}");
        assert!(logs > 0, "azalea should place a bending trunk");
        assert!(azalea > 0, "azalea should place plain azalea leaves");
        assert!(flowering > 0, "azalea should place flowering azalea leaves");
    }

    #[test]
    fn mangrove_tree_parses_branches_and_places_waterlogged_roots() {
        let tree = parse_tree_config(&vanilla_tree_config("mangrove")["config"])
            .unwrap()
            .expect("mangrove should be supported");
        assert_eq!(tree.decorator_count, 3);
        assert!(tree.root_placer.is_some());
        parse_tree_config(&vanilla_tree_config("tall_mangrove")["config"])
            .unwrap()
            .expect("tall_mangrove should be supported");

        let mut chunk = mangrove_water_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        let mut roots = 0;
        let mut waterlogged_roots = 0;
        let mut muddy_roots = 0;
        let mut moss = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..90 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    match state.name.as_str() {
                        "minecraft:mangrove_log" => logs += 1,
                        "minecraft:mangrove_leaves" => leaves += 1,
                        "minecraft:mangrove_roots" => {
                            roots += 1;
                            if state
                                .properties
                                .iter()
                                .any(|(name, value)| name == "waterlogged" && value == "true")
                            {
                                waterlogged_roots += 1;
                            }
                        }
                        "minecraft:muddy_mangrove_roots" => muddy_roots += 1,
                        "minecraft:moss_carpet" => moss += 1,
                        _ => {}
                    }
                }
            }
        }
        println!(
            "mangrove logs={logs} leaves={leaves} roots={roots} waterlogged_roots={waterlogged_roots} muddy_roots={muddy_roots} moss={moss}"
        );
        assert!(logs > 0, "mangrove should place a trunk");
        assert!(leaves > 0, "mangrove should place foliage");
        assert!(roots > 0, "mangrove should place roots");
        assert!(
            waterlogged_roots > 0,
            "roots placed in water should be waterlogged"
        );
        assert!(muddy_roots > 0, "roots placed in mud should be muddy");
        assert!(moss > 0, "above-root moss should be placed");
    }

    #[test]
    fn fallen_oak_tree_places_stump_and_attached_logs() {
        let mut value = vanilla_tree_config("fallen_oak_tree");
        value["config"]["log_length"] = json!(6);
        value["config"]["log_decorators"][0]["probability"] = json!(1.0);
        let tree = parse_fallen_tree_config(&value["config"])
            .unwrap()
            .expect("fallen_oak_tree should be supported");
        assert_eq!(tree.stump_decorators.len(), 1);
        assert_eq!(tree.log_decorators.len(), 1);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut horizontal_logs = 0;
        let mut mushrooms = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..80 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:oak_log" {
                        logs += 1;
                        if state.properties.iter().any(|(name, value)| {
                            name == "axis" && matches!(value.as_str(), "x" | "z")
                        }) {
                            horizontal_logs += 1;
                        }
                    } else if matches!(
                        state.name.as_str(),
                        "minecraft:red_mushroom" | "minecraft:brown_mushroom"
                    ) {
                        mushrooms += 1;
                    }
                }
            }
        }
        println!("fallen oak logs={logs} horizontal={horizontal_logs} mushrooms={mushrooms}");
        assert!(logs >= 2, "fallen tree should place a stump and log");
        assert!(horizontal_logs > 0, "fallen log should be sideways");
        assert!(mushrooms > 0, "attached_to_logs placed no mushrooms");
    }

    #[test]
    fn jungle_bush_parses_and_places_bush_foliage() {
        let tree = parse_tree_config(&vanilla_tree_config("jungle_bush")["config"])
            .unwrap()
            .expect("jungle_bush should be supported");
        assert_eq!(tree.decorator_count, 0);

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut logs = 0;
        let mut leaves = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..80 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:jungle_log" {
                        logs += 1;
                    } else if state.name == "minecraft:oak_leaves" {
                        leaves += 1;
                    }
                }
            }
        }
        println!("jungle_bush logs={logs} leaves={leaves}");
        assert_eq!(logs, 1, "jungle bush should place a one-block trunk");
        assert!(leaves > 0, "jungle bush should place foliage");
    }

    #[test]
    fn attached_to_leaves_places_randomized_hanging_blocks() {
        let mut value = oak_value();
        value["config"]["decorators"] = json!([{
            "type": "minecraft:attached_to_leaves",
            "probability": 1.0,
            "exclusion_radius_xz": 1,
            "exclusion_radius_y": 0,
            "block_provider": {
                "type": "minecraft:randomized_int_state_provider",
                "property": "age",
                "source": {
                    "type": "minecraft:simple_state_provider",
                    "state": {
                        "Name": "minecraft:mangrove_propagule",
                        "Properties": {
                            "age": "0",
                            "hanging": "true",
                            "stage": "0",
                            "waterlogged": "false"
                        }
                    }
                },
                "values": {
                    "type": "minecraft:uniform",
                    "min_inclusive": 0,
                    "max_inclusive": 4
                }
            },
            "directions": ["down"],
            "required_empty_blocks": 2
        }]);
        let tree = parse_tree_config(&value["config"])
            .unwrap()
            .expect("attached_to_leaves should be supported");

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut propagules = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..90 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:mangrove_propagule" {
                        propagules += 1;
                        assert!(state.properties.iter().any(|(key, value)| key == "age"
                            && matches!(value.as_str(), "0" | "1" | "2" | "3" | "4")));
                    }
                }
            }
        }
        println!("attached propagules={propagules}");
        assert!(propagules > 0, "attached_to_leaves placed nothing");
    }

    #[test]
    fn jungle_tree_places_hanging_vines_trunk_vines_and_cocoa() {
        let mut value = vanilla_tree_config("jungle_tree");
        value["config"]["decorators"][0]["probability"] = json!(1.0);
        value["config"]["decorators"][2]["probability"] = json!(1.0);
        let tree = parse_tree_config(&value["config"])
            .unwrap()
            .expect("jungle_tree should be supported");

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8)));
        }

        let mut vines = 0;
        let mut cocoa = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 60..90 {
                    let Some(state) = chunk.block_state(x, y, z) else {
                        continue;
                    };
                    if state.name == "minecraft:vine" {
                        vines += 1;
                        assert_eq!(state.properties.len(), 5);
                        assert!(
                            state
                                .properties
                                .iter()
                                .any(|(key, value)| key != "up" && value == "true")
                        );
                    } else if state.name == "minecraft:cocoa" {
                        cocoa += 1;
                        assert!(state.properties.iter().any(|(key, value)| key == "facing"
                            && matches!(value.as_str(), "north" | "east" | "south" | "west")));
                        assert!(state.properties.iter().any(|(key, value)| key == "age"
                            && matches!(value.as_str(), "0" | "1" | "2")));
                    }
                }
            }
        }
        println!("jungle tree vines={vines} cocoa={cocoa}");
        assert!(vines > 0, "jungle tree placed no vines");
        assert!(cocoa > 0, "jungle tree placed no cocoa");
    }

    #[test]
    fn creaking_heart_decorator_requires_six_surrounding_logs() {
        let mut value = oak_value();
        value["config"]["decorators"] =
            json!([{"type": "minecraft:creaking_heart", "probability": 1.0}]);
        let tree = parse_tree_config(&value["config"])
            .unwrap()
            .expect("creaking_heart is supported");
        assert_eq!(tree.decorator_count, 1);
        let decorator = tree.decorators.first().unwrap();

        let center = BlockPos::new(8, 65, 8);
        let offsets = [
            (0, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
            (-1, 0, 0),
            (1, 0, 0),
        ];
        let mut logs = offsets
            .iter()
            .map(|(dx, dy, dz)| center.offset(*dx, *dy, *dz))
            .collect::<Vec<_>>();
        logs.sort_by_key(|pos| pos.y());

        let mut chunk = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            for pos in &logs {
                level.set_block(*pos, oak_log());
            }
            let mut random = LegacyRandomSource::new(7);
            decorator.place(&mut level, &mut random, &logs, &[]);
        }
        let heart = chunk.block_state(center.x() as usize, center.y(), center.z() as usize);
        let heart = heart.expect("center block exists");
        assert_eq!(heart.name, "minecraft:creaking_heart");
        assert!(heart.properties.contains(&("axis".into(), "y".into())));
        assert!(
            heart
                .properties
                .contains(&("creaking_heart_state".into(), "dormant".into()))
        );
        assert!(
            heart
                .properties
                .contains(&("natural".into(), "true".into()))
        );

        // Removing one neighbour must leave every candidate ineligible.
        let mut incomplete = flat_chunk();
        {
            let mut level = WorldGenRegion::new(&mut incomplete, MIN_Y, HEIGHT);
            for pos in logs.iter().filter(|pos| **pos != center.offset(1, 0, 0)) {
                level.set_block(*pos, oak_log());
            }
            let mut random = LegacyRandomSource::new(7);
            decorator.place(&mut level, &mut random, &logs, &[]);
        }
        assert_eq!(
            incomplete
                .block_state(center.x() as usize, center.y(), center.z() as usize)
                .unwrap()
                .name,
            "minecraft:oak_log"
        );
    }

    #[test]
    fn unsupported_tree_decorator_makes_the_tree_unsupported() {
        let mut value = oak_value();
        value["config"]["decorators"] = json!([{"type": "minecraft:test_unsupported_decorator"}]);
        assert!(parse_tree_config(&value["config"]).unwrap().is_none());
    }

    #[test]
    fn tree_placement_is_deterministic() {
        let tree = oak();
        let mut chunks = Vec::new();
        for _ in 0..2 {
            let mut chunk = flat_chunk();
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(99);
            tree.place(&mut level, &mut random, BlockPos::new(8, 64, 8));
            chunks.push(chunk);
        }
        assert_eq!(chunks[0], chunks[1]);
    }

    #[test]
    fn unsupported_placer_is_reported_instead_of_approximated() {
        let value = json!({
            "type": "minecraft:tree",
            "config": {
                "trunk_placer": {"type": "minecraft:test_unsupported_trunk_placer"},
                "foliage_placer": {"type": "minecraft:blob_foliage_placer"}
            }
        });
        let parsed = parse_tree_config(&value["config"]).unwrap();
        assert!(parsed.is_none());
    }
}
