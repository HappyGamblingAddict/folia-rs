//! Crop growth for wheat, carrots, potatoes, and beetroots.
//!
//! This is the `CropBlock` random-tick subset: an age property 0..max, sky-light
//! gate, deterministic per-position growth roll, and a bone-meal-style growth
//! API. Hydration/farmland-neighbour growth speed is not modeled yet.

use crate::chunk::BlockState;
use crate::tick::{BlockBehavior, TickWorld};
use folia_random::RandomSource;
use std::io;

pub fn is_crop(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:wheat" | "minecraft:carrots" | "minecraft:potatoes" | "minecraft:beetroots"
    )
}

pub fn max_age(state: &BlockState) -> u8 {
    if state.name == "minecraft:beetroots" {
        3
    } else {
        7
    }
}

pub fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

pub fn age(state: &BlockState) -> u8 {
    property(state, "age")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0)
}

fn with_age(state: &BlockState, value: u8) -> BlockState {
    let mut next = state.clone();
    if let Some((_, current)) = next.properties.iter_mut().find(|(name, _)| name == "age") {
        *current = value.to_string();
    } else {
        next.properties.push(("age".to_owned(), value.to_string()));
    }
    next
}

/// Applies one growth step, clamped at the crop's maximum age.
pub fn grow(state: &BlockState, amount: u8) -> BlockState {
    let age = age(state).saturating_add(amount).min(max_age(state));
    with_age(state, age)
}

fn light_ok(world: &mut TickWorld<'_>, pos: crate::math::BlockPos) -> bool {
    world
        .storage()
        .sky_light_at(pos.x(), pos.y() + 1, pos.z())
        .is_some_and(|light| light >= 9)
}

fn random_tick(
    world: &mut TickWorld<'_>,
    pos: crate::math::BlockPos,
    state: &BlockState,
) -> io::Result<()> {
    if age(state) >= max_age(state) || !light_ok(world, pos) {
        return Ok(());
    }
    let mut random = world.random(pos);
    if random.next_int_bound(4) != 0 {
        return Ok(());
    }
    world.set_block(pos, grow(state, 1))?;
    Ok(())
}

pub struct CropBehavior;

pub static CROP_BEHAVIOR: CropBehavior = CropBehavior;

impl BlockBehavior for CropBehavior {
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        is_crop(state)
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: crate::math::BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        random_tick(world, pos, state)
    }
}
