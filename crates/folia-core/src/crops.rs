//! Crop growth for wheat, carrots, potatoes, and beetroots.
//!
//! This is the `CropBlock` random-tick subset: an age property 0..max, a
//! brightness gate, the vanilla growth-speed probability, and a
//! bone-meal-style growth API. Hydration comes from the shipped farmland
//! `moisture` property; the farmland itself is not ticked here.
//!
//! The probability formula and neighbour layout follow the 1.21 Java
//! `CropBlock.randomTick`/`getGrowthSpeed`. The local reference tree does not
//! contain the decompiled Folia source, so the exact formula was recovered
//! from the matching 1.21 server source and is documented in `DECISIONS.md`.

use crate::chunk::BlockState;
use crate::math::BlockPos;
use crate::tick::{BlockBehavior, TickWorld};
use folia_random::RandomSource;
use std::io;

/// One entry per crop block name, for `BehaviorTable::with` registration.
pub static BEHAVIORS: &[(&str, &dyn BlockBehavior)] = &[
    ("minecraft:wheat", &CROP_BEHAVIOR),
    ("minecraft:carrots", &CROP_BEHAVIOR),
    ("minecraft:potatoes", &CROP_BEHAVIOR),
    ("minecraft:beetroots", &CROP_BEHAVIOR),
];

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

/// Vanilla `CropBlock.isMaxAge`: a mature crop no longer receives random ticks.
pub fn is_mature(state: &BlockState) -> bool {
    age(state) >= max_age(state)
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

/// Bone-meal age increase (vanilla `CropBlock.getBonemealAgeIncrease`): 2..=4
/// in the portable approximation shipped by this slice.
pub fn bone_meal_amount(random: &mut impl RandomSource) -> u8 {
    2 + random.next_int_bound(3) as u8
}

/// Returns the bone-meal-grown state only for an immature crop.
///
/// Fully grown crops and non-crop blocks return `None`, matching vanilla where
/// `CropBlock.isBonemealSuccess` fails and bone meal is not consumed.
pub fn bone_meal_crop(state: &BlockState, amount: u8) -> Option<BlockState> {
    if is_crop(state) && !is_mature(state) {
        Some(grow(state, amount))
    } else {
        None
    }
}

fn same_crop(world: &mut TickWorld<'_>, pos: BlockPos, name: &str) -> io::Result<bool> {
    Ok(world
        .block_state(pos)?
        .is_some_and(|state| state.name == name))
}

/// Vanilla `CropBlock.getGrowthSpeed`.
///
/// The 3x3 farmland patch under the crop contributes 1 per dry farmland block
/// and 3 per hydrated one, with off-centre cells quartered. A same-crop
/// neighbour on both horizontal axes, or any diagonal neighbour, halves the
/// result. Returned value is at least 1/2, so the probability stays bounded.
pub fn growth_speed(world: &mut TickWorld<'_>, pos: BlockPos, name: &str) -> io::Result<f32> {
    let mut speed = 1.0_f32;
    let below = pos.below();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let mut contribution = 0.0_f32;
            if let Some(state) = world.block_state(below.offset(dx, 0, dz))?
                && state.name == "minecraft:farmland"
            {
                contribution = 1.0;
                if property(&state, "moisture")
                    .and_then(|value| value.parse::<u8>().ok())
                    .unwrap_or(0)
                    > 0
                {
                    contribution = 3.0;
                }
            }
            if dx != 0 || dz != 0 {
                contribution /= 4.0;
            }
            speed += contribution;
        }
    }

    let west = same_crop(world, pos.west(), name)?;
    let east = same_crop(world, pos.east(), name)?;
    let north = same_crop(world, pos.north(), name)?;
    let south = same_crop(world, pos.south(), name)?;
    if (west || east) && (north || south) {
        speed /= 2.0;
    } else {
        let diagonal = same_crop(world, pos.north().west(), name)?
            || same_crop(world, pos.north().east(), name)?
            || same_crop(world, pos.south().east(), name)?
            || same_crop(world, pos.south().west(), name)?;
        if diagonal {
            speed /= 2.0;
        }
    }
    Ok(speed.max(0.5))
}

fn random_tick(world: &mut TickWorld<'_>, pos: BlockPos, state: &BlockState) -> io::Result<()> {
    if is_mature(state) || crate::tick::light_at(world, pos)? < 9 {
        return Ok(());
    }
    // `CropBlock.mayPlaceOn` allows farmland only. A crop cannot survive on
    // anything else, so refuse growth there instead of growing a plant that a
    // shape update would immediately remove.
    if !world
        .block_state(pos.below())?
        .is_some_and(|below| below.name == "minecraft:farmland")
    {
        return Ok(());
    }
    let speed = growth_speed(world, pos, state.name.as_str())?;
    let bound = (25.0_f32 / speed) as i32 + 1;
    let mut random = world.random(pos);
    if random.next_int_bound(bound.max(1)) != 0 {
        return Ok(());
    }
    world.set_block(pos, grow(state, 1))?;
    Ok(())
}

pub struct CropBehavior;

pub static CROP_BEHAVIOR: CropBehavior = CropBehavior;

impl BlockBehavior for CropBehavior {
    fn is_randomly_ticking(&self, state: &BlockState) -> bool {
        is_crop(state) && !is_mature(state)
    }

    fn random_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        random_tick(world, pos, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use folia_random::LegacyRandomSource;

    #[test]
    fn bone_meal_amount_is_two_to_four() {
        for seed in 0..32_i64 {
            let mut random = LegacyRandomSource::new(seed);
            for _ in 0..16 {
                let amount = bone_meal_amount(&mut random);
                assert!((2..=4).contains(&amount), "seed {seed}: got {amount}");
            }
        }
    }

    #[test]
    fn bone_meal_crop_grows_only_immature_crops() {
        let wheat = BlockState {
            name: "minecraft:wheat".into(),
            properties: vec![("age".into(), "0".into())],
        };
        let grown = bone_meal_crop(&wheat, 3).expect("immature wheat grows");
        assert_eq!(grown.name, "minecraft:wheat");
        assert_eq!(age(&grown), 3);

        let beetroots = BlockState {
            name: "minecraft:beetroots".into(),
            properties: vec![("age".into(), "1".into())],
        };
        let grown = bone_meal_crop(&beetroots, 3).expect("immature beetroots grow");
        assert_eq!(age(&grown), max_age(&beetroots));

        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        assert!(bone_meal_crop(&stone, 3).is_none());

        let mature = BlockState {
            name: "minecraft:wheat".into(),
            properties: vec![("age".into(), "7".into())],
        };
        assert!(bone_meal_crop(&mature, 3).is_none());
    }
}
