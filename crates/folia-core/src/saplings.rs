//! Sapling random ticks: stage advance and tree growth.
//!
//! Ports the 1.21 `SaplingBlock.randomTick` / `advanceTree` and the relevant
//! `TreeGrower` selection: brightness >= 9 at the block above and a 1-in-7
//! roll advance `stage` 0 -> 1, then the next accepted roll grows a tree.
//! The configured tree feature is resolved by id from
//! [`crate::worldgen::feature::FeatureData`] and placed through
//! [`crate::worldgen::feature::WorldGenRegion`], so species shapes come from the
//! shipped 1.21.11 tree configurations rather than a local approximation.
//!
//! Local differences, recorded in `DECISIONS.md`: the RNG is the deterministic
//! per-position `TickWorld` source instead of `Level.random`, and the feature
//! is placed in the sapling's own chunk copy, so canopies crossing a chunk
//! border are clipped instead of reaching the neighbour chunk.

use crate::chunk::BlockState;
use crate::math::BlockPos;
use crate::tags;
use crate::tick::{BlockBehavior, TickWorld};
use crate::worldgen::feature::{FeatureData, WorldGenRegion};
use folia_random::RandomSource;
use std::io;

/// Sapling blocks a tree feature must clear before placing its trunk.
type Cleared = Vec<(BlockPos, BlockState)>;

/// Selected configured-feature id and the sapling blocks it replaces.
type FeatureSelection = Option<(String, Cleared)>;

/// The tree grower attached to a sapling block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Species {
    Oak,
    Spruce,
    Birch,
    Jungle,
    Acacia,
    DarkOak,
    Mangrove,
    Cherry,
    PaleOak,
}

impl Species {
    /// Configured feature used by a single sapling of this species.
    /// Dark oak has no single-sapling variant in `TreeGrower`.
    fn regular(self) -> Option<&'static str> {
        match self {
            Self::Oak => Some("minecraft:oak"),
            Self::Spruce => Some("minecraft:spruce"),
            Self::Birch => Some("minecraft:birch"),
            Self::Jungle => Some("minecraft:jungle_tree_no_vine"),
            Self::Acacia => Some("minecraft:acacia"),
            Self::DarkOak => None,
            Self::Mangrove => Some("minecraft:mangrove"),
            Self::Cherry => Some("minecraft:cherry"),
            Self::PaleOak => Some("minecraft:pale_oak"),
        }
    }

    /// Rare variant chosen by `TreeGrower.getConfiguredFeature`.
    fn secondary(self) -> Option<&'static str> {
        match self {
            Self::Oak => Some("minecraft:fancy_oak"),
            Self::Spruce => Some("minecraft:spruce"),
            Self::Mangrove => Some("minecraft:tall_mangrove"),
            _ => None,
        }
    }

    /// 2x2-only feature chosen by `TreeGrower.getConfiguredMegaFeature`.
    fn mega(self) -> Option<&'static str> {
        match self {
            Self::Spruce => Some("minecraft:mega_spruce"),
            Self::Jungle => Some("minecraft:mega_jungle_tree"),
            Self::DarkOak => Some("minecraft:dark_oak"),
            Self::PaleOak => Some("minecraft:pale_oak_bonemeal"),
            _ => None,
        }
    }

    /// Rare 2x2 variant; only spruce selects one at random.
    fn secondary_mega(self) -> Option<&'static str> {
        match self {
            Self::Spruce => Some("minecraft:mega_pine"),
            _ => None,
        }
    }

    fn secondary_chance(self) -> f32 {
        match self {
            Self::Oak => 0.1,
            Self::Spruce => 0.5,
            Self::Mangrove => 0.85,
            _ => 0.0,
        }
    }

    /// Bee-nested variant used when a flower is nearby.
    fn with_flowers(self) -> Option<&'static str> {
        match self {
            Self::Oak => Some("minecraft:oak_bees_005"),
            Self::Birch => Some("minecraft:birch_bees_005"),
            Self::Cherry => Some("minecraft:cherry_bees_005"),
            _ => None,
        }
    }

    fn rare_with_flowers(self) -> Option<&'static str> {
        match self {
            Self::Oak => Some("minecraft:fancy_oak_bees_005"),
            _ => None,
        }
    }

    pub fn log_name(self) -> &'static str {
        match self {
            Self::Oak => "minecraft:oak_log",
            Self::PaleOak => "minecraft:pale_oak_log",
            Self::Spruce => "minecraft:spruce_log",
            Self::Birch => "minecraft:birch_log",
            Self::Jungle => "minecraft:jungle_log",
            Self::Acacia => "minecraft:acacia_log",
            Self::DarkOak => "minecraft:dark_oak_log",
            Self::Mangrove => "minecraft:mangrove_log",
            Self::Cherry => "minecraft:cherry_log",
        }
    }

    pub fn leaf_name(self) -> &'static str {
        match self {
            Self::Oak => "minecraft:oak_leaves",
            Self::PaleOak => "minecraft:pale_oak_leaves",
            Self::Spruce => "minecraft:spruce_leaves",
            Self::Birch => "minecraft:birch_leaves",
            Self::Jungle => "minecraft:jungle_leaves",
            Self::Acacia => "minecraft:acacia_leaves",
            Self::DarkOak => "minecraft:dark_oak_leaves",
            Self::Mangrove => "minecraft:mangrove_leaves",
            Self::Cherry => "minecraft:cherry_leaves",
        }
    }
}

/// Maps a block name to its `TreeGrower`, including `mangrove_propagule`.
pub fn species(state: &BlockState) -> Option<Species> {
    match state.name.as_str() {
        "minecraft:oak_sapling" => Some(Species::Oak),
        "minecraft:spruce_sapling" => Some(Species::Spruce),
        "minecraft:birch_sapling" => Some(Species::Birch),
        "minecraft:jungle_sapling" => Some(Species::Jungle),
        "minecraft:acacia_sapling" => Some(Species::Acacia),
        "minecraft:dark_oak_sapling" => Some(Species::DarkOak),
        "minecraft:mangrove_propagule" => Some(Species::Mangrove),
        "minecraft:cherry_sapling" => Some(Species::Cherry),
        "minecraft:pale_oak_sapling" => Some(Species::PaleOak),
        _ => None,
    }
}

/// True for any block in the `minecraft:saplings` tag, including species this
/// port does not grow (azaleas).
pub fn is_sapling(state: &BlockState) -> bool {
    tags::block_has("saplings", state.name.as_str())
}

/// True when this block has a `stage` property and a known tree grower.
pub fn is_growable_sapling(state: &BlockState) -> bool {
    species(state).is_some() && has_property(state, "stage")
}

/// One entry per growable sapling block name, for `BehaviorTable::with`.
pub static BEHAVIORS: &[(&str, &dyn BlockBehavior)] = &[
    ("minecraft:oak_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:spruce_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:birch_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:jungle_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:acacia_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:dark_oak_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:mangrove_propagule", &SAPLING_BEHAVIOR),
    ("minecraft:cherry_sapling", &SAPLING_BEHAVIOR),
    ("minecraft:pale_oak_sapling", &SAPLING_BEHAVIOR),
];

fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn has_property(state: &BlockState, key: &str) -> bool {
    state.properties.iter().any(|(name, _)| name == key)
}

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    }
}

fn is_free(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    ) || tags::block_has("fire", state.name.as_str())
        || matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava")
        || tags::block_has("replaceable", state.name.as_str())
}

pub fn stage(state: &BlockState) -> u8 {
    property(state, "stage")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
}

fn with_stage(state: &BlockState, value: u8) -> BlockState {
    let mut next = state.clone();
    if let Some((_, current)) = next.properties.iter_mut().find(|(key, _)| key == "stage") {
        *current = value.to_string();
    } else {
        next.properties
            .push(("stage".to_owned(), value.to_string()));
    }
    next
}

fn supported_soil(state: &BlockState) -> bool {
    tags::block_has("dirt", state.name.as_str())
        || state.name == "minecraft:farmland"
        || state.name == "minecraft:water"
}

fn valid_position(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    let Some(below) = world.block_state(pos.below())? else {
        return Ok(false);
    };
    if !supported_soil(&below) {
        return Ok(false);
    }
    let Some(above) = world.block_state(pos.above())? else {
        return Ok(false);
    };
    Ok(is_free(&above))
}

fn flowers_nearby(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    for dx in -2..=2 {
        for dy in -1..=1 {
            for dz in -2..=2 {
                if let Some(state) = world.block_state(pos.offset(dx, dy, dz))?
                    && tags::block_has("flowers", state.name.as_str())
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// `TreeGrower.getConfiguredFeature` plus `getConfiguredMegaFeature`, including
/// the exact `nextFloat()` consumption order.
fn select_feature(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    random: &mut impl RandomSource,
) -> io::Result<FeatureSelection> {
    let Some(species) = species(state) else {
        return Ok(None);
    };

    if let Some(mega) = species.mega() {
        let chosen = if random.next_float() < species.secondary_chance() {
            species.secondary_mega().unwrap_or(mega)
        } else {
            mega
        };
        if let Some(id) = FeatureData::active()
            .configured(chosen)
            .filter(|feature| feature.is_supported())
            .map(|_| chosen.to_owned())
        {
            for dx in [0, -1] {
                for dz in [0, -1] {
                    let mut square = Vec::with_capacity(4);
                    let mut all = true;
                    for ox in 0..=1 {
                        for oz in 0..=1 {
                            let corner = pos.offset(dx + ox, 0, dz + oz);
                            let Some(corner_state) = world.block_state(corner)? else {
                                all = false;
                                break;
                            };
                            if corner_state.name != state.name {
                                all = false;
                                break;
                            }
                            square.push((corner, corner_state));
                        }
                    }
                    if all {
                        return Ok(Some((id, square)));
                    }
                }
            }
        }
    }

    let flowers = flowers_nearby(world, pos)?;
    let rare = random.next_float() < species.secondary_chance();
    // `TreeGrower.getConfiguredFeature` falls through when a bee variant is
    // absent: a rare bee variant, then the rare tree, then the bee variant,
    // then the regular tree.
    let mut chosen = if rare && flowers {
        species.rare_with_flowers()
    } else if rare {
        species.secondary()
    } else {
        None
    };
    if chosen.is_none() && rare {
        chosen = species.secondary();
    }
    if chosen.is_none() && flowers {
        chosen = species.with_flowers();
    }
    let Some(id) = chosen.or_else(|| species.regular()) else {
        return Ok(None);
    };
    if !FeatureData::active()
        .configured(id)
        .is_some_and(|feature| feature.is_supported())
    {
        return Ok(None);
    }
    Ok(Some((id.to_owned(), vec![(pos, state.clone())])))
}

fn place_tree(
    world: &mut TickWorld<'_>,
    id: &str,
    cleared: &[(BlockPos, BlockState)],
    origin: BlockPos,
    random: &mut impl RandomSource,
) -> io::Result<bool> {
    let Some(feature) = FeatureData::active().configured(id) else {
        return Ok(false);
    };
    let chunk_x = origin.x().div_euclid(16);
    let chunk_z = origin.z().div_euclid(16);
    let Some(mut chunk) = world.chunk_copy(chunk_x, chunk_z)? else {
        return Ok(false);
    };
    for (pos, _) in cleared {
        let local_x = pos.x().rem_euclid(16) as usize;
        let local_z = pos.z().rem_euclid(16) as usize;
        chunk.set_block_state(local_x, pos.y(), local_z, air());
    }

    let (placed, writes) = {
        let mut region = WorldGenRegion::new(
            &mut chunk,
            crate::chunk_packet::OVERWORLD_MIN_Y,
            crate::chunk_packet::OVERWORLD_HEIGHT as i32,
        )
        .with_seed(world.seed())
        .record_writes();
        let placed = feature.place(&mut region, random, origin).unwrap_or(false);
        (placed, region.take_written_blocks())
    };
    if !placed {
        return Ok(false);
    }

    // Only touch the live world once the feature succeeded, so a failed
    // placement leaves the sapling exactly where it was.
    for (pos, _) in cleared {
        world.set_block_quiet(*pos, air())?;
    }
    for pos in writes {
        let local_x = pos.x().rem_euclid(16) as usize;
        let local_z = pos.z().rem_euclid(16) as usize;
        let Some(new_state) = chunk.block_state(local_x, pos.y(), local_z) else {
            continue;
        };
        let current = world.block_state(pos)?;
        if current.as_ref() != Some(new_state) {
            world.set_block_quiet(pos, new_state.clone())?;
        }
    }
    // Tree features write with `UPDATE_INVISIBLE` (4) in vanilla: the log and
    // leaf placements do not enqueue neighbour updates, so neither do these.
    Ok(true)
}

/// Ports `SaplingBlock.randomTick` and `advanceTree` for one forced tick.
pub fn random_tick(world: &mut TickWorld<'_>, pos: BlockPos, state: &BlockState) -> io::Result<()> {
    let Some(_species) = species(state) else {
        return Ok(());
    };
    if crate::tick::light_at(world, pos.above())? < 9 {
        return Ok(());
    }
    let mut random = world.random(pos);
    if random.next_int_bound(7) != 0 {
        return Ok(());
    }
    if stage(state) == 0 {
        world.set_block(pos, with_stage(state, 1))?;
        return Ok(());
    }

    if !valid_position(world, pos)? {
        return Ok(());
    }
    let Some((id, cleared)) = select_feature(world, pos, state, &mut random)? else {
        world.record_unsupported_random_tick(&state.name);
        return Ok(());
    };
    let origin = cleared
        .iter()
        .find(|(candidate, _)| *candidate == pos)
        .map(|(candidate, _)| *candidate)
        .unwrap_or(pos);
    if !place_tree(world, &id, &cleared, origin, &mut random)? {
        world.record_unsupported_random_tick(&id);
    }
    Ok(())
}

pub struct SaplingBehavior;

pub static SAPLING_BEHAVIOR: SaplingBehavior = SaplingBehavior;

impl BlockBehavior for SaplingBehavior {
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        is_growable_sapling(state)
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        random_tick(world, pos, state)
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: crate::math::Direction,
        _neighbor: &BlockState,
        _state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        // `BushBlock.canSurvive`: dirt/farmland below, and water is allowed for
        // the mangrove propagule. Horizontal updates never remove a sapling.
        if direction != crate::math::Direction::Down && direction != crate::math::Direction::Up {
            return Ok(None);
        }
        let Some(below) = world.block_state(pos.below())? else {
            return Ok(Some(air()));
        };
        Ok(if supported_soil(&below) {
            None
        } else {
            Some(air())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn species_mapping_covers_every_grower() {
        for (name, expected) in [
            ("minecraft:oak_sapling", Species::Oak),
            ("minecraft:spruce_sapling", Species::Spruce),
            ("minecraft:birch_sapling", Species::Birch),
            ("minecraft:jungle_sapling", Species::Jungle),
            ("minecraft:acacia_sapling", Species::Acacia),
            ("minecraft:dark_oak_sapling", Species::DarkOak),
            ("minecraft:mangrove_propagule", Species::Mangrove),
            ("minecraft:cherry_sapling", Species::Cherry),
            ("minecraft:pale_oak_sapling", Species::PaleOak),
        ] {
            let state = BlockState {
                name: name.into(),
                properties: vec![("stage".into(), "0".into())],
            };
            assert_eq!(species(&state), Some(expected));
            assert!(is_growable_sapling(&state));
        }
        let azalea = BlockState {
            name: "minecraft:azalea".into(),
            properties: Vec::new(),
        };
        assert!(is_sapling(&azalea));
        assert!(!is_growable_sapling(&azalea));
    }

    #[test]
    fn tree_log_and_leaf_names_are_species_specific() {
        assert_eq!(Species::Oak.log_name(), "minecraft:oak_log");
        assert_eq!(Species::Cherry.leaf_name(), "minecraft:cherry_leaves");
        assert_eq!(Species::DarkOak.log_name(), "minecraft:dark_oak_log");
    }
}
