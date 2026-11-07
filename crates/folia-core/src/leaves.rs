//! Leaf distance and decay, ported from `LeavesBlock`.
//!
//! Leaves carry a `distance` property 1..7. A leaf touching a log has distance
//! 1, and a leaf touching another leaf is one greater. A non-persistent leaf
//! with distance 7 decays on its random tick.

use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::tags;
use crate::tick::{BlockBehavior, TickWorld};
use std::collections::HashSet;
use std::io;

/// Maximum leaf distance before decay.
pub const MAX_DISTANCE: u8 = 7;

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

fn distance_of(state: &BlockState) -> u8 {
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

/// The distance this leaf should have from the nearest log through adjacent
/// leaves. `MAX_DISTANCE` means no support was found. A breadth-first walk is
/// used so stale `distance` properties cannot make a leaf jump further away.
pub fn natural_distance(world: &mut TickWorld<'_>, pos: BlockPos) -> u8 {
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

/// One `LeavesBlock.tick`/neighbour update step.
pub fn update(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
    allow_decay: bool,
) -> io::Result<()> {
    if !is_leaves(state) {
        return Ok(());
    }
    let persistent = is_persistent(state);
    let current = distance_of(state);
    let support = natural_distance(world, pos);
    if allow_decay && !persistent && current >= MAX_DISTANCE && support >= MAX_DISTANCE {
        world.set_block(
            pos,
            BlockState {
                name: "minecraft:air".to_owned(),
                properties: Vec::new(),
            },
        )?;
        return Ok(());
    }
    let next = if persistent {
        current
    } else {
        support.clamp(1, MAX_DISTANCE)
    };
    if next != current {
        world.set_block(pos, with_property(state, "distance", &next.to_string()))?;
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
        update(world, pos, state, true)
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        update(world, pos, state, false)
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
