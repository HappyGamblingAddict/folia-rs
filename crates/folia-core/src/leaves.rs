//! Leaf distance and decay, ported from `LeavesBlock`.
//!
//! Leaves carry a `distance` property 1..7. A leaf touching a log has distance
//! 1, and a leaf touching another leaf is one greater. A non-persistent leaf
//! whose distance is still 7 on its random tick decays to air.
//!
//! The algorithm follows vanilla `LeavesBlock`:
//!
//! * `getDistanceAt` returns the stored `distance` of a leaf, `0` for a log,
//!   and `MAX_DISTANCE` for anything else.
//! * `updateDistance` takes the minimum `getDistanceAt(neighbour) + 1` over
//!   the six neighbours (a log is therefore distance 1).
//! * `randomTick` recomputes that distance and decays only when both the old
//!   and the new distance are 7, so a leaf whose support was just removed gets
//!   one random tick to settle before it disappears.
//! * A neighbour change schedules a one-tick recompute. That propagates the
//!   new distance outwards one leaf per tick, matching the way a canopy
//!   decays from the trunk outwards.
//!
//! Fixture gap: the local tree has no decompiled Java oracle for leaf-distance
//! propagation, so the exact tick on which each leaf changes and the exact
//! neighbour-update flag set are not differential-tested. The distance and
//! decay results are pinned by `tests/leaves.rs`.

use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::registry;
use crate::tags;
use crate::tick::{BlockBehavior, TickWorld};
use crate::worldgen::feature::FeatureLevel;
use std::io;

/// Maximum leaf distance before decay.
pub const MAX_DISTANCE: u8 = 7;

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".to_owned(),
        properties: Vec::new(),
    }
}

/// `minecraft:leaves` tag membership.
pub fn is_leaves(state: &BlockState) -> bool {
    tags::block_has("leaves", &state.name)
}

/// Logs, wood, hyphae, and stems support leaves at distance 0.
pub fn is_log(state: &BlockState) -> bool {
    tags::block_has("logs", &state.name)
        || state.name.ends_with("_log")
        || state.name.ends_with("_wood")
        || state.name.ends_with("_hyphae")
        || state.name.ends_with("_stem")
}

pub fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn with_property(state: &BlockState, key: &str, value: &str) -> BlockState {
    let mut next = state.clone();
    if let Some((_, current)) = next.properties.iter_mut().find(|(name, _)| name == key) {
        *current = value.to_owned();
    } else {
        next.properties.push((key.to_owned(), value.to_owned()));
    }
    next
}

/// The stored `distance`, defaulting to 7 when the property is absent.
pub fn distance_of(state: &BlockState) -> u8 {
    property(state, "distance")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(MAX_DISTANCE)
        .clamp(1, MAX_DISTANCE)
}

fn is_persistent(state: &BlockState) -> bool {
    property(state, "persistent") == Some("true")
}

/// Returns a copy of `state` with `persistent=true`.
pub fn persistent(state: BlockState) -> BlockState {
    with_property(&state, "persistent", "true")
}

/// Vanilla `LeavesBlock.getDistanceAt`: leaves use their stored distance,
/// logs are distance 0, everything else is `MAX_DISTANCE`.
pub fn distance_at(state: &BlockState) -> u8 {
    if is_leaves(state) {
        distance_of(state)
    } else if is_log(state) {
        0
    } else {
        MAX_DISTANCE
    }
}

/// Returns a registered leaf state with `distance` set to `value`.
///
/// The state is resolved through `block_states.tsv` so the full property set
/// (`distance`, `persistent`, `waterlogged`) is one the protocol registry
/// knows. Unknown leaf blocks fall back to editing the property in place.
pub fn with_distance(state: &BlockState, value: u8) -> BlockState {
    let distance = value.clamp(1, MAX_DISTANCE).to_string();
    let persistent = property(state, "persistent").unwrap_or("false").to_owned();
    let waterlogged = property(state, "waterlogged").unwrap_or("false").to_owned();
    registry::state_with_properties(
        &state.name,
        &[
            ("distance", distance.as_str()),
            ("persistent", persistent.as_str()),
            ("waterlogged", waterlogged.as_str()),
        ],
    )
    .unwrap_or_else(|| with_property(state, "distance", &distance))
}

/// Vanilla `LeavesBlock.updateDistance` over an arbitrary neighbour source.
///
/// The walk stops at distance 1 because no leaf can be closer to a log.
pub fn distance_near(mut neighbor: impl FnMut(Direction) -> Option<BlockState>) -> u8 {
    let mut best = MAX_DISTANCE;
    for direction in Direction::ALL {
        let Some(state) = neighbor(direction) else {
            continue;
        };
        best = best.min(distance_at(&state).saturating_add(1));
        if best == 1 {
            break;
        }
    }
    best
}

fn tick_distance(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<u8> {
    let mut best = MAX_DISTANCE;
    for direction in Direction::ALL {
        let Some(state) = world.block_state(pos.relative(direction))? else {
            continue;
        };
        best = best.min(distance_at(&state).saturating_add(1));
        if best == 1 {
            break;
        }
    }
    Ok(best)
}

/// The leaf state a worldgen feature should place at `pos`.
///
/// Tree foliage is written before the enclosing chunk is ticked, so the
/// distance has to be computed from the feature level's current blocks. This
/// is the same [`with_distance`] path used by placement and random ticks, so
/// generated and player-grown trees agree on the persisted state.
pub fn feature_leaf_state(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    sampled: BlockState,
) -> BlockState {
    if !is_leaves(&sampled) {
        return sampled;
    }
    let distance = distance_near(|direction| level.get_block(pos.relative(direction)).cloned());
    with_distance(&sampled, distance)
}

/// A breadth-first distance that ignores stored leaf distances.
///
/// Kept for callers that need a fresh log search independent of any stale
/// `distance` property; the tick path uses the vanilla neighbour-based
/// [`distance_near`] instead.
pub fn natural_distance(world: &mut TickWorld<'_>, pos: BlockPos) -> u8 {
    use std::collections::HashSet;
    let mut visited = HashSet::new();
    visited.insert(pos);
    let mut frontier = vec![(pos, 0u8)];
    while let Some((current, depth)) = frontier.pop() {
        if depth >= MAX_DISTANCE {
            continue;
        }
        for direction in Direction::ALL {
            let neighbor_pos = current.relative(direction);
            if !visited.insert(neighbor_pos) {
                continue;
            }
            let Some(neighbor) = world.block_state(neighbor_pos).ok().flatten() else {
                continue;
            };
            if is_log(&neighbor) {
                return depth + 1;
            }
            if is_leaves(&neighbor) {
                frontier.push((neighbor_pos, depth + 1));
            }
        }
    }
    MAX_DISTANCE
}

/// Recomputes one leaf's distance, decaying it on a random tick when both the
/// old and the new distance are 7.
pub fn update(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    allow_decay: bool,
) -> io::Result<()> {
    if !is_leaves(state) {
        return Ok(());
    }
    let current = distance_of(state);
    let next = tick_distance(world, pos)?;
    if allow_decay && !is_persistent(state) && current == MAX_DISTANCE && next == MAX_DISTANCE {
        world.set_block(pos, air())?;
        return Ok(());
    }
    if next != current {
        world.set_block(pos, with_distance(state, next))?;
    }
    Ok(())
}

pub struct LeavesBehavior;

pub static LEAVES_BEHAVIOR: LeavesBehavior = LeavesBehavior;

impl BlockBehavior for LeavesBehavior {
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        is_leaves(state)
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        if is_persistent(state) {
            return Ok(());
        }
        update(world, pos, state, true)
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        update(world, pos, state, false)
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if !is_leaves(state) {
            return Ok(());
        }
        // A conservative superset of vanilla's dependency check: recompute on
        // the next tick whenever any neighbour changed. This costs a few extra
        // one-tick scheduled ticks but always converges to the same distance.
        world.schedule(pos, state.name.clone(), 1);
        Ok(())
    }

    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _old: Option<&BlockState>,
    ) -> io::Result<()> {
        update(world, pos, state, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_leaves() -> BlockState {
        crate::registry::default_block_state("minecraft:oak_leaves").unwrap()
    }

    #[test]
    fn get_distance_at_matches_vanilla_inputs() {
        let log = BlockState {
            name: "minecraft:oak_log".into(),
            properties: Vec::new(),
        };
        let leaf = with_distance(&default_leaves(), 3);
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: Vec::new(),
        };
        assert_eq!(distance_at(&log), 0);
        assert_eq!(distance_at(&leaf), 3);
        assert_eq!(distance_at(&stone), MAX_DISTANCE);
    }

    #[test]
    fn with_distance_uses_the_registered_leaf_state() {
        let near = with_distance(&default_leaves(), 1);
        assert_eq!(crate::registry::block_state_id(&near), 255);
        assert_eq!(property(&near, "distance"), Some("1"));
        assert_eq!(property(&near, "persistent"), Some("false"));
        assert_eq!(property(&near, "waterlogged"), Some("false"));
    }

    #[test]
    fn distance_near_prefers_a_log_over_other_leaves() {
        let log = BlockState {
            name: "minecraft:oak_log".into(),
            properties: Vec::new(),
        };
        let far_leaf = with_distance(&default_leaves(), 5);
        assert_eq!(
            distance_near(|direction| match direction {
                Direction::West => Some(log.clone()),
                Direction::East => Some(far_leaf.clone()),
                _ => None,
            }),
            1
        );
    }
}
