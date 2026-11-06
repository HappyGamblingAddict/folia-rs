//! `OreVeinifier`: the copper/iron vein pass applied after the base ore
//! features. This module is deliberately standalone: the generator can call it
//! once the chunk-status/density pass exposes the same block context.
//!
//! Ports `net/minecraft/world/level/levelgen/OreVeinifier.java`.

use crate::chunk::BlockState;
use crate::worldgen::density::{DensityFunction, NoiseRouter};
use folia_random::{RandomSource, XoroshiroPositionalRandomFactory};
use std::sync::Arc;

const VEININESS_THRESHOLD: f64 = 0.4_f32 as f64;
const EDGE_ROUNDOFF_BEGIN: f64 = 20.0;
const MAX_EDGE_ROUNDOFF: f64 = 0.2;
const VEIN_SOLIDNESS: f32 = 0.7;
// Java declares these as `float` and passes them to `double` parameters, so
// the binary value is the f32 rounding, not the decimal literal.
const MIN_RICHNESS: f64 = 0.1_f32 as f64;
const MAX_RICHNESS: f64 = 0.3_f32 as f64;
const MAX_RICHNESS_THRESHOLD: f64 = 0.6_f32 as f64;
const CHANCE_OF_RAW_ORE_BLOCK: f32 = 0.02;
const SKIP_ORE_IF_GAP_NOISE_IS_BELOW: f64 = -0.3_f32 as f64;

/// The two vanilla vein types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VeinType {
    Copper,
    Iron,
}

impl VeinType {
    pub fn min_y(self) -> i32 {
        match self {
            Self::Copper => 0,
            Self::Iron => -60,
        }
    }

    pub fn max_y(self) -> i32 {
        match self {
            Self::Copper => 50,
            Self::Iron => -8,
        }
    }

    pub fn ore(self) -> BlockState {
        state(match self {
            Self::Copper => "minecraft:copper_ore",
            Self::Iron => "minecraft:deepslate_iron_ore",
        })
    }

    pub fn raw_ore(self) -> BlockState {
        state(match self {
            Self::Copper => "minecraft:raw_copper_block",
            Self::Iron => "minecraft:raw_iron_block",
        })
    }

    pub fn filler(self) -> BlockState {
        state(match self {
            Self::Copper => "minecraft:granite",
            Self::Iron => "minecraft:tuff",
        })
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

/// A configured vein pass for one noise router and level seed.
#[derive(Clone)]
pub struct OreVeinifier {
    vein_toggle: Arc<DensityFunction>,
    vein_ridged: Arc<DensityFunction>,
    vein_gap: Arc<DensityFunction>,
    random: XoroshiroPositionalRandomFactory,
}

impl OreVeinifier {
    /// `OreVeinifier.create(router.veinToggle(), router.veinRidged(),
    /// router.veinGap(), random.oreRandom())`.
    pub fn new(
        router: &NoiseRouter,
        root_factory: &XoroshiroPositionalRandomFactory,
    ) -> std::io::Result<Self> {
        let mut ore = root_factory.from_hash_of("minecraft:ore");
        let random = ore.fork_positional();
        Ok(Self {
            vein_toggle: Arc::clone(router.function("vein_toggle")?),
            vein_ridged: Arc::clone(router.function("vein_ridged")?),
            vein_gap: Arc::clone(router.function("vein_gap")?),
            random,
        })
    }

    /// Returns the block the vein pass would write at `(x, y, z)`, or `None`
    /// when it leaves the existing block unchanged.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        let toggle = self.vein_toggle.compute(x, y, z);
        let vein_type = if toggle > 0.0 {
            VeinType::Copper
        } else {
            VeinType::Iron
        };
        let abs = toggle.abs();
        let max_distance = f64::from(vein_type.max_y() - y);
        let min_distance = f64::from(y - vein_type.min_y());
        if min_distance < 0.0 || max_distance < 0.0 {
            return None;
        }

        let edge = min_distance.min(max_distance);
        let roundoff = clamped_map(edge, 0.0, EDGE_ROUNDOFF_BEGIN, -MAX_EDGE_ROUNDOFF, 0.0);
        if abs + roundoff < VEININESS_THRESHOLD {
            return None;
        }

        let mut random = self.random.at(x, y, z);
        if random.next_float() > VEIN_SOLIDNESS {
            return None;
        }
        if self.vein_ridged.compute(x, y, z) >= 0.0 {
            return None;
        }

        let richness = clamped_map(
            abs,
            VEININESS_THRESHOLD,
            MAX_RICHNESS_THRESHOLD,
            MIN_RICHNESS,
            MAX_RICHNESS,
        );
        if (random.next_float() as f64) < richness
            && self.vein_gap.compute(x, y, z) > SKIP_ORE_IF_GAP_NOISE_IS_BELOW
        {
            if random.next_float() < CHANCE_OF_RAW_ORE_BLOCK {
                return Some(vein_type.raw_ore());
            }
            return Some(vein_type.ore());
        }

        Some(vein_type.filler())
    }
}

/// `Mth.clampedMap(double, double, double, double, double)`.
fn clamped_map(
    value: f64,
    input_min: f64,
    input_max: f64,
    output_min: f64,
    output_max: f64,
) -> f64 {
    let delta = (value - input_min) / (input_max - input_min);
    output_min + delta.clamp(0.0, 1.0) * (output_max - output_min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::density::WorldgenData;

    #[test]
    fn vein_pass_is_deterministic_and_finds_real_ores() {
        let data = WorldgenData::overworld(12345).unwrap();
        let router = data.router().unwrap();
        let veins = OreVeinifier::new(&router, data.seed()).unwrap();
        let other = OreVeinifier::new(&router, data.seed()).unwrap();

        let mut found = 0usize;
        for y in -60..50 {
            for z in 0..16 {
                for x in 0..16 {
                    let first = veins.block_at(x, y, z);
                    assert_eq!(first, other.block_at(x, y, z));
                    if let Some(state) = first
                        && matches!(
                            state.name.as_str(),
                            "minecraft:copper_ore"
                                | "minecraft:raw_copper_block"
                                | "minecraft:deepslate_iron_ore"
                                | "minecraft:raw_iron_block"
                                | "minecraft:granite"
                                | "minecraft:tuff"
                        )
                    {
                        found += 1;
                    }
                }
            }
        }
        assert!(found > 0, "the overworld vein pass should write some veins");
    }
}
