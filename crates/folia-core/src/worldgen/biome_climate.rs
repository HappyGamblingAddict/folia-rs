//! Per-biome temperature and the vanilla `frozen` modifier.
//!
//! `scripts/extract_biome_climate.ps1` extracts `temperature` and
//! `temperature_modifier` from the shipped 1.21.11 biome JSON. This is enough
//! for `Biome.getTemperature` / `shouldMeltFrozenOceanIcebergSlightly`, which
//! the surface pass needs for iceberg edges.

use crate::worldgen::simplex::{biome_info_noise, frozen_temperature_noise};
use std::collections::HashMap;
use std::sync::OnceLock;

const BIOME_CLIMATE: &str = include_str!("../../assets/biome_climate.tsv");

/// `Biome.TemperatureModifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemperatureModifier {
    None,
    Frozen,
}

#[derive(Debug, Clone, Copy)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub modifier: TemperatureModifier,
}

static CLIMATE: OnceLock<HashMap<&'static str, BiomeClimate>> = OnceLock::new();

fn climates() -> &'static HashMap<&'static str, BiomeClimate> {
    CLIMATE.get_or_init(|| {
        let mut map = HashMap::new();
        for line in BIOME_CLIMATE.lines() {
            let mut parts = line.split('\t');
            let (Some(name), Some(temperature), Some(modifier)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(temperature) = temperature.parse::<f32>() else {
                continue;
            };
            map.insert(
                name,
                BiomeClimate {
                    temperature,
                    modifier: match modifier {
                        "frozen" => TemperatureModifier::Frozen,
                        _ => TemperatureModifier::None,
                    },
                },
            );
        }
        map
    })
}

/// Climate data for a namespaced biome id (`minecraft:plains`).
pub fn climate(biome: &str) -> Option<BiomeClimate> {
    climates().get(biome).copied()
}

/// `Biome.getTemperature(BlockPos, int)` including the `frozen` modifier.
pub fn temperature_at(biome: &str, x: i32, y: i32, z: i32) -> Option<f32> {
    let climate = climate(biome)?;
    if climate.modifier == TemperatureModifier::Frozen {
        let d =
            frozen_temperature_noise().get_value(f64::from(x) * 0.05, f64::from(z) * 0.05, false)
                * 7.0;
        let value = biome_info_noise().get_value(f64::from(x) * 0.2, f64::from(z) * 0.2, false);
        if d + value < 0.3 {
            let value1 =
                biome_info_noise().get_value(f64::from(x) * 0.09, f64::from(z) * 0.09, false);
            if value1 < 0.8 {
                return Some(0.2);
            }
        }
    }
    let _ = y;
    Some(climate.temperature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_ocean_and_deep_frozen_ocean_are_extracted() {
        let frozen = climate("minecraft:frozen_ocean").unwrap();
        assert_eq!(frozen.temperature, 0.0);
        assert_eq!(frozen.modifier, TemperatureModifier::Frozen);

        let deep = climate("minecraft:deep_frozen_ocean").unwrap();
        assert_eq!(deep.temperature, 0.5);
        assert_eq!(deep.modifier, TemperatureModifier::Frozen);

        assert_eq!(
            climate("minecraft:plains").unwrap().modifier,
            TemperatureModifier::None
        );
    }

    #[test]
    fn frozen_modifier_matches_the_vanilla_branch() {
        for (x, z) in [(0, 0), (100, -200), (-37, 81)] {
            let expected = {
                let d = frozen_temperature_noise().get_value(
                    f64::from(x) * 0.05,
                    f64::from(z) * 0.05,
                    false,
                ) * 7.0;
                let value =
                    biome_info_noise().get_value(f64::from(x) * 0.2, f64::from(z) * 0.2, false);
                if d + value < 0.3
                    && biome_info_noise().get_value(f64::from(x) * 0.09, f64::from(z) * 0.09, false)
                        < 0.8
                {
                    0.2
                } else {
                    0.0
                }
            };
            assert_eq!(
                temperature_at("minecraft:frozen_ocean", x, 63, z).unwrap(),
                expected
            );
        }
        assert!(temperature_at("minecraft:deep_frozen_ocean", 0, 63, 0).unwrap() > 0.1);
    }
}
