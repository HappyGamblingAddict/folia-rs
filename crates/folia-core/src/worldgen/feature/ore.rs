//! `minecraft:ore` and `minecraft:scattered_ore`.
//!
//! `OreFeature` builds a chain of ellipsoids from its size, rejects targets
//! exposed to air at the configured chance, and replaces the first matching
//! target state. `ScatteredOreFeature` samples a small number of independent
//! positions around the origin. The four `RuleTest` tags used by the shipped
//! configs resolve through the generated vanilla block-tag table; unknown
//! predicates make the whole config unsupported rather than guessing.

use super::placement::parse_block_state;
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::tags;
use crate::worldgen::carver::mth_sin;
use folia_math::BlockPos;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

#[derive(Debug, Clone)]
pub struct OreFeature {
    size: i32,
    targets: Vec<OreTargetBlock>,
    discard_chance_on_air_exposure: f32,
}

#[derive(Debug, Clone)]
struct OreTargetBlock {
    target: OreTarget,
    state: BlockState,
}

#[derive(Debug, Clone)]
enum OreTarget {
    Block(String),
    Tag(&'static str),
}

impl OreTarget {
    fn matches(&self, state: &BlockState) -> bool {
        match self {
            Self::Block(name) => state.name == *name,
            Self::Tag(tag) => tags::block_has(tag, &state.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScatteredOreFeature {
    ore: OreFeature,
}

/// Parses an `OreConfiguration`; `Ok(None)` means a target predicate has no
/// port yet.
pub fn parse_ore_config(config: &Value) -> io::Result<Option<OreFeature>> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("ore config must be an object"))?;
    let Some(targets) = object.get("targets").and_then(Value::as_array) else {
        return Err(invalid("ore config is missing `targets`"));
    };
    let size = object
        .get("size")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid("ore config is missing integer `size`"))?;
    let size = i32::try_from(size).map_err(|_| invalid("ore `size` does not fit in i32"))?;
    if !(0..=64).contains(&size) {
        return Err(invalid("ore `size` must be between 0 and 64"));
    }
    let discard_chance_on_air_exposure = match object.get("discard_chance_on_air_exposure") {
        None => 0.0,
        Some(value) => {
            let chance = value
                .as_f64()
                .ok_or_else(|| invalid("ore `discard_chance_on_air_exposure` must be a number"))?
                as f32;
            if !(0.0..=1.0).contains(&chance) {
                return Err(invalid(
                    "ore `discard_chance_on_air_exposure` must be between 0 and 1",
                ));
            }
            chance
        }
    };

    let mut parsed_targets = Vec::with_capacity(targets.len());
    for target in targets {
        let target_object = target
            .as_object()
            .ok_or_else(|| invalid("ore target must be an object"))?;
        let target_rule = target_object
            .get("target")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("ore target is missing `target`"))?;
        let Some(target) = parse_target(target_rule)? else {
            return Ok(None);
        };
        let state = parse_block_state(
            target_object
                .get("state")
                .ok_or_else(|| invalid("ore target is missing `state`"))?,
        )?;
        parsed_targets.push(OreTargetBlock { target, state });
    }

    Ok(Some(OreFeature {
        size,
        targets: parsed_targets,
        discard_chance_on_air_exposure,
    }))
}

/// Parses a scattered-ore configuration. It shares `OreConfiguration` with
/// `minecraft:ore`.
pub fn parse_scattered_ore_config(config: &Value) -> io::Result<Option<ScatteredOreFeature>> {
    Ok(parse_ore_config(config)?.map(|ore| ScatteredOreFeature { ore }))
}

fn parse_target(object: &Map<String, Value>) -> io::Result<Option<OreTarget>> {
    let kind = object
        .get("predicate_type")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid(format!(
                "ore target is missing `predicate_type` (keys: {:?})",
                object.keys().collect::<Vec<_>>()
            ))
        })?;
    match kind {
        "minecraft:block_match" => {
            let block = object
                .get("block")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("block_match ore target is missing `block`"))?;
            Ok(Some(OreTarget::Block(resource_location(block))))
        }
        "minecraft:tag_match" => {
            let tag = object
                .get("tag")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("tag_match ore target is missing `tag`"))?;
            Ok(match tag {
                "minecraft:base_stone_overworld" => {
                    Some(OreTarget::Tag("minecraft:base_stone_overworld"))
                }
                "minecraft:stone_ore_replaceables" => {
                    Some(OreTarget::Tag("minecraft:stone_ore_replaceables"))
                }
                "minecraft:deepslate_ore_replaceables" => {
                    Some(OreTarget::Tag("minecraft:deepslate_ore_replaceables"))
                }
                "minecraft:base_stone_nether" => {
                    Some(OreTarget::Tag("minecraft:base_stone_nether"))
                }
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

impl OreFeature {
    /// `OreFeature.place`.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let f = random.next_float() * std::f32::consts::PI;
        let f1 = self.size as f32 / 8.0;
        let ceil = mth_ceil((self.size as f32 / 16.0 * 2.0 + 1.0) / 2.0);
        let d = f64::from(origin.x()) + (f as f64).sin() * f64::from(f1);
        let d1 = f64::from(origin.x()) - (f as f64).sin() * f64::from(f1);
        let d2 = f64::from(origin.z()) + (f as f64).cos() * f64::from(f1);
        let d3 = f64::from(origin.z()) - (f as f64).cos() * f64::from(f1);
        let d4 = f64::from(origin.y()) + f64::from(random.next_int_bound(3)) - 2.0;
        let d5 = f64::from(origin.y()) + f64::from(random.next_int_bound(3)) - 2.0;
        let x = origin.x() - mth_ceil(f1) - ceil;
        let y = origin.y() - 2 - ceil;
        let z = origin.z() - mth_ceil(f1) - ceil;
        let width = 2 * (mth_ceil(f1) + ceil);
        let height = 2 * (2 + ceil);

        for i in x..=x + width {
            for j in z..=z + width {
                if y <= ocean_floor_wg_height(level, i, j) {
                    return self
                        .do_place(level, random, d, d1, d2, d3, d4, d5, x, y, z, width, height);
                }
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn do_place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        min_x: f64,
        max_x: f64,
        min_z: f64,
        max_z: f64,
        min_y: f64,
        max_y: f64,
        x: i32,
        y: i32,
        z: i32,
        width: i32,
        height: i32,
    ) -> bool {
        if self.size == 0 {
            return false;
        }
        let mut placed = 0;
        let mut visited = vec![false; (width * height * width) as usize];
        let count = self.size as usize;
        let mut branches = vec![0.0f64; count * 4];

        for index in 0..count {
            let f = index as f32 / count as f32;
            let delta = f64::from(f);
            let px = lerp(delta, min_x, max_x);
            let py = lerp(delta, min_y, max_y);
            let pz = lerp(delta, min_z, max_z);
            let radius = random.next_double() * f64::from(self.size) / 16.0;
            let size =
                (f64::from(mth_sin(f64::from(std::f32::consts::PI * f))) + 1.0) * radius + 1.0;
            let size = size / 2.0;
            branches[index * 4] = px;
            branches[index * 4 + 1] = py;
            branches[index * 4 + 2] = pz;
            branches[index * 4 + 3] = size;
        }

        for left in 0..count {
            if branches[left * 4 + 3] <= 0.0 {
                continue;
            }
            for right in left + 1..count {
                if branches[right * 4 + 3] <= 0.0 {
                    continue;
                }
                let dx = branches[left * 4] - branches[right * 4];
                let dy = branches[left * 4 + 1] - branches[right * 4 + 1];
                let dz = branches[left * 4 + 2] - branches[right * 4 + 2];
                let radius = branches[left * 4 + 3] - branches[right * 4 + 3];
                if radius * radius > dx * dx + dy * dy + dz * dz {
                    if radius > 0.0 {
                        branches[right * 4 + 3] = -1.0;
                    } else {
                        branches[left * 4 + 3] = -1.0;
                    }
                }
            }
        }

        for index in 0..count {
            let radius = branches[index * 4 + 3];
            if radius < 0.0 {
                continue;
            }
            let px = branches[index * 4];
            let py = branches[index * 4 + 1];
            let pz = branches[index * 4 + 2];
            let min_xi = mth_floor(px - radius).max(x);
            let min_yi = mth_floor(py - radius).max(y);
            let min_zi = mth_floor(pz - radius).max(z);
            let max_xi = mth_floor(px + radius).max(min_xi);
            let max_yi = mth_floor(py + radius).max(min_yi);
            let max_zi = mth_floor(pz + radius).max(min_zi);

            for bx in min_xi..=max_xi {
                let rx = (f64::from(bx) + 0.5 - px) / radius;
                if rx * rx >= 1.0 {
                    continue;
                }
                for by in min_yi..=max_yi {
                    let ry = (f64::from(by) + 0.5 - py) / radius;
                    if rx * rx + ry * ry >= 1.0 {
                        continue;
                    }
                    for bz in min_zi..=max_zi {
                        let rz = (f64::from(bz) + 0.5 - pz) / radius;
                        if rx * rx + ry * ry + rz * rz >= 1.0 || is_outside_build_height(level, by)
                        {
                            continue;
                        }
                        let bit = (bx - x + (by - y) * width + (bz - z) * width * height) as usize;
                        if visited[bit] {
                            continue;
                        }
                        visited[bit] = true;
                        let pos = BlockPos::new(bx, by, bz);
                        let Some(current) = level.get_block(pos).cloned() else {
                            continue;
                        };
                        if let Some(state) = self.target_state(level, random, &current, pos)
                            && level.set_block(pos, state)
                        {
                            placed += 1;
                        }
                    }
                }
            }
        }
        placed > 0
    }

    fn target_state(
        &self,
        level: &dyn FeatureLevel,
        random: &mut dyn RandomSource,
        current: &BlockState,
        pos: BlockPos,
    ) -> Option<BlockState> {
        for target in &self.targets {
            if !target.target.matches(current) {
                continue;
            }
            if self.should_skip_air_check(random) || !is_adjacent_to_air(level, pos) {
                return Some(target.state.clone());
            }
        }
        None
    }

    fn should_skip_air_check(&self, random: &mut dyn RandomSource) -> bool {
        let chance = self.discard_chance_on_air_exposure;
        chance <= 0.0 || (chance < 1.0 && random.next_float() >= chance)
    }
}

impl ScatteredOreFeature {
    /// `ScatteredOreFeature.place`; vanilla reports success even when no ore
    /// block is placed.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let count = random.next_int_bound(self.ore.size + 1);
        for index in 0..count {
            let magnitude = index.min(7);
            let dx = java_round((random.next_float() - random.next_float()) * magnitude as f32);
            let dy = java_round((random.next_float() - random.next_float()) * magnitude as f32);
            let dz = java_round((random.next_float() - random.next_float()) * magnitude as f32);
            let pos = origin.offset(dx, dy, dz);
            let Some(current) = level.get_block(pos).cloned() else {
                continue;
            };
            if let Some(state) = self.ore.target_state(level, random, &current, pos) {
                level.set_block(pos, state);
            }
        }
        true
    }
}

fn is_outside_build_height(level: &dyn FeatureLevel, y: i32) -> bool {
    y < level.min_y() || y >= level.min_y() + level.height()
}

fn is_adjacent_to_air(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    const DIRECTIONS: [(i32, i32, i32); 6] = [
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (-1, 0, 0),
        (1, 0, 0),
    ];
    DIRECTIONS.iter().any(|(dx, dy, dz)| {
        level
            .get_block(pos.offset(*dx, *dy, *dz))
            .is_some_and(|state| is_air_name(&state.name))
    })
}

fn ocean_floor_wg_height(level: &dyn FeatureLevel, x: i32, z: i32) -> i32 {
    let max_y = level.min_y() + level.height() - 1;
    for y in (level.min_y()..=max_y).rev() {
        if level
            .get_block(BlockPos::new(x, y, z))
            .is_some_and(is_motion_blocking)
        {
            return y;
        }
    }
    level.min_y()
}

fn is_motion_blocking(state: &BlockState) -> bool {
    !is_air_name(&state.name)
        && !matches!(
            state.name.as_str(),
            "minecraft:water"
                | "minecraft:lava"
                | "minecraft:short_grass"
                | "minecraft:tall_grass"
                | "minecraft:fern"
                | "minecraft:large_fern"
                | "minecraft:vine"
                | "minecraft:glow_lichen"
        )
        && !state.name.ends_with("_leaves")
}

fn mth_floor(value: f64) -> i32 {
    value.floor() as i32
}

fn mth_ceil(value: f32) -> i32 {
    let value = f64::from(value);
    value.ceil() as i32
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}

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
    use crate::chunk::Chunk;
    use crate::worldgen::feature::WorldGenRegion;
    use folia_random::LegacyRandomSource;
    use serde_json::Value;

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;
    fn test_config(name: &str) -> Value {
        let data: Value = serde_json::from_str(include_str!("../../../assets/feature_data.json"))
            .expect("vanilla feature data parses");
        data["configured"][name]["config"].clone()
    }

    fn ore(name: &str) -> OreFeature {
        let value = test_config(name);
        parse_ore_config(&value)
            .unwrap()
            .unwrap_or_else(|| panic!("{name} should be supported"))
    }

    fn flat_stone() -> Chunk {
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        for x in 0..16 {
            for z in 0..16 {
                for y in MIN_Y..=20 {
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
            }
        }
        chunk
    }

    fn flat_netherrack() -> Chunk {
        let mut chunk = flat_stone();
        for x in 0..16 {
            for z in 0..16 {
                for y in MIN_Y..=20 {
                    chunk.set_block_state(
                        x,
                        y,
                        z,
                        BlockState {
                            name: "minecraft:netherrack".into(),
                            properties: vec![],
                        },
                    );
                }
            }
        }
        chunk
    }

    fn count(chunk: &Chunk, name: &str) -> usize {
        let mut count = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in MIN_Y..=MIN_Y + HEIGHT {
                    if chunk
                        .block_state(x, y, z)
                        .is_some_and(|state| state.name == name)
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn every_shipped_ore_config_parses() {
        let data: Value = serde_json::from_str(include_str!("../../../assets/feature_data.json"))
            .expect("vanilla feature data parses");
        let mut ores = 0;
        let mut scattered = 0;
        for (name, entry) in data["configured"].as_object().unwrap() {
            match entry["type"].as_str() {
                Some("minecraft:ore") => {
                    assert!(
                        parse_ore_config(&entry["config"]).unwrap().is_some(),
                        "{name} should be supported"
                    );
                    ores += 1;
                }
                Some("minecraft:scattered_ore") => {
                    assert!(
                        parse_scattered_ore_config(&entry["config"])
                            .unwrap()
                            .is_some(),
                        "{name} should be supported"
                    );
                    scattered += 1;
                }
                _ => {}
            }
        }
        assert_eq!(ores, 30);
        assert_eq!(scattered, 2);
    }

    #[test]
    fn ore_configs_parse_and_place_into_stone() {
        let diamond = ore("ore_diamond_medium");
        let mut chunk = flat_stone();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(diamond.place(&mut level, &mut random, BlockPos::new(8, 0, 8)));
        }
        assert!(count(&chunk, "minecraft:diamond_ore") > 0);
    }

    #[test]
    fn scattered_ore_targets_netherrack() {
        let value = test_config("ore_ancient_debris_small");
        let scattered = parse_scattered_ore_config(&value)
            .unwrap()
            .expect("scattered ore should be supported");
        let mut chunk = flat_netherrack();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(scattered.place(&mut level, &mut random, BlockPos::new(8, 0, 8)));
        }
        assert!(count(&chunk, "minecraft:ancient_debris") > 0);
    }
}
