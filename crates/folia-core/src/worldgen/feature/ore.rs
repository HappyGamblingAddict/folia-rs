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
use std::collections::HashMap;
use std::io;

#[derive(Debug, Clone)]
pub struct OreFeature {
    size: i32,
    targets: Vec<BlockState>,
    target_indices: HashMap<String, usize>,
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

    let mut target_indices = HashMap::new();
    let mut target_states = Vec::with_capacity(parsed_targets.len());
    for (index, target) in parsed_targets.into_iter().enumerate() {
        match &target.target {
            OreTarget::Block(name) => {
                target_indices.entry(name.clone()).or_insert(index);
            }
            OreTarget::Tag(tag) => {
                for name in tags::members(crate::tags::TagRegistryKind::Block, tag) {
                    target_indices.entry(name.clone()).or_insert(index);
                }
            }
        }
        target_states.push(target.state);
    }

    Ok(Some(OreFeature {
        size,
        targets: target_states,
        target_indices,
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
                if level.has_motion_blocking_at_or_above(i, j, y) {
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
                        let Some(current) = level.get_block(pos) else {
                            continue;
                        };
                        if let Some(state) = self.target_state(level, random, current, pos)
                            && level.set_block_ref(pos, state)
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
    ) -> Option<&BlockState> {
        let index = *self.target_indices.get(current.name.as_str())?;
        if self.should_skip_air_check(random) || !is_adjacent_to_air(level, pos) {
            return Some(&self.targets[index]);
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
            let Some(current) = level.get_block(pos) else {
                continue;
            };
            if let Some(state) = self.ore.target_state(level, random, current, pos) {
                level.set_block_ref(pos, state);
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
    use crate::worldgen::diagnostics::chunk_digest;
    use crate::worldgen::feature::{WorldGenRegion, is_motion_blocking, scan_motion_blocking_top};
    use folia_random::LegacyRandomSource;
    use serde_json::Value;

    #[derive(Debug, Clone, Copy)]
    enum RandomDraw {
        Int(i32),
        IntBound(i32, i32),
        Long(i64),
        Double(u64),
        Float(u32),
        Boolean(bool),
        Gaussian(u64),
    }

    struct RecordingRandom {
        inner: LegacyRandomSource,
        draws: Vec<RandomDraw>,
    }

    impl RecordingRandom {
        fn new(seed: i64) -> Self {
            Self {
                inner: LegacyRandomSource::new(seed),
                draws: Vec::new(),
            }
        }

        fn digest(&self) -> u64 {
            fn feed(hash: &mut u64, bytes: &[u8]) {
                for byte in bytes {
                    *hash ^= u64::from(*byte);
                    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }

            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            feed(&mut hash, &(self.draws.len() as u64).to_le_bytes());
            for draw in &self.draws {
                match *draw {
                    RandomDraw::Int(value) => {
                        feed(&mut hash, &[0]);
                        feed(&mut hash, &value.to_le_bytes());
                    }
                    RandomDraw::IntBound(bound, value) => {
                        feed(&mut hash, &[1]);
                        feed(&mut hash, &bound.to_le_bytes());
                        feed(&mut hash, &value.to_le_bytes());
                    }
                    RandomDraw::Long(value) => {
                        feed(&mut hash, &[2]);
                        feed(&mut hash, &value.to_le_bytes());
                    }
                    RandomDraw::Double(value) => {
                        feed(&mut hash, &[3]);
                        feed(&mut hash, &value.to_le_bytes());
                    }
                    RandomDraw::Float(value) => {
                        feed(&mut hash, &[4]);
                        feed(&mut hash, &value.to_le_bytes());
                    }
                    RandomDraw::Boolean(value) => {
                        feed(&mut hash, &[5, u8::from(value)]);
                    }
                    RandomDraw::Gaussian(value) => {
                        feed(&mut hash, &[6]);
                        feed(&mut hash, &value.to_le_bytes());
                    }
                }
            }
            hash
        }
    }

    impl RandomSource for RecordingRandom {
        fn next_int(&mut self) -> i32 {
            let value = self.inner.next_int();
            self.draws.push(RandomDraw::Int(value));
            value
        }

        fn next_int_bound(&mut self, bound: i32) -> i32 {
            let value = self.inner.next_int_bound(bound);
            self.draws.push(RandomDraw::IntBound(bound, value));
            value
        }

        fn next_long(&mut self) -> i64 {
            let value = self.inner.next_long();
            self.draws.push(RandomDraw::Long(value));
            value
        }

        fn next_double(&mut self) -> f64 {
            let value = self.inner.next_double();
            self.draws.push(RandomDraw::Double(value.to_bits()));
            value
        }

        fn next_float(&mut self) -> f32 {
            let value = self.inner.next_float();
            self.draws.push(RandomDraw::Float(value.to_bits()));
            value
        }

        fn next_boolean(&mut self) -> bool {
            let value = self.inner.next_boolean();
            self.draws.push(RandomDraw::Boolean(value));
            value
        }

        fn next_gaussian(&mut self) -> f64 {
            let value = self.inner.next_gaussian();
            self.draws.push(RandomDraw::Gaussian(value.to_bits()));
            value
        }
    }

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
            light_present: false,
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
    fn resolved_targets_keep_the_first_matching_rule() {
        let config = serde_json::json!({
            "size": 3,
            "discard_chance_on_air_exposure": 1.0,
            "targets": [
                {
                    "target": {
                        "predicate_type": "minecraft:tag_match",
                        "tag": "minecraft:base_stone_overworld"
                    },
                    "state": { "Name": "minecraft:diamond_ore" }
                },
                {
                    "target": {
                        "predicate_type": "minecraft:block_match",
                        "block": "minecraft:stone"
                    },
                    "state": { "Name": "minecraft:gold_ore" }
                }
            ]
        });
        let feature = parse_ore_config(&config).unwrap().unwrap();
        let mut chunk = flat_stone();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            assert!(feature.place(&mut level, &mut random, BlockPos::new(8, 0, 8)));
        }
        assert!(count(&chunk, "minecraft:diamond_ore") > 0);
        assert_eq!(count(&chunk, "minecraft:gold_ore"), 0);
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

    #[test]
    fn column_precheck_matches_the_ocean_floor_predicate() {
        let mut chunk = Chunk {
            light_present: false,
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        for (y, name) in [
            (MIN_Y, "minecraft:water"),
            (-32, "minecraft:stone"),
            (0, "minecraft:oak_leaves"),
            (16, "minecraft:vine"),
            (100, "minecraft:grass_block"),
        ] {
            chunk.set_block_state(
                0,
                y,
                0,
                BlockState {
                    name: name.into(),
                    properties: Vec::new(),
                },
            );
        }
        let level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        for (x, z) in [(0, 0), (1, 0)] {
            let max_y = MIN_Y + HEIGHT - 1;
            let floor = (MIN_Y..=max_y)
                .rev()
                .find(|&y| {
                    level
                        .get_block(BlockPos::new(x, y, z))
                        .is_some_and(is_motion_blocking)
                })
                .unwrap_or(MIN_Y);
            for y in MIN_Y - 2..=max_y + 2 {
                assert_eq!(
                    level.has_motion_blocking_at_or_above(x, z, y),
                    y <= floor,
                    "column ({x},{z}), threshold {y}"
                );
            }
        }
    }

    #[test]
    fn ore_rng_draw_order_is_stable() {
        let diamond = ore("ore_diamond_medium");
        let mut diamond_chunk = flat_stone();
        diamond_chunk.set_block_state(
            8,
            0,
            8,
            BlockState {
                name: "minecraft:air".into(),
                properties: Vec::new(),
            },
        );
        let mut diamond_random = RecordingRandom::new(7);
        let diamond_success = {
            let mut level = WorldGenRegion::new(&mut diamond_chunk, MIN_Y, HEIGHT);
            diamond.place(&mut level, &mut diamond_random, BlockPos::new(8, 0, 8))
        };
        let diamond_draws = diamond_random.digest();
        let diamond_next = diamond_random.next_long();
        println!(
            "diamond rng: success={diamond_success}, draws={}, digest={diamond_draws:#018x}, next={diamond_next:#018x}, blocks={:#018x}",
            diamond_random.draws.len(),
            chunk_digest(&diamond_chunk)
        );

        let value = test_config("ore_ancient_debris_small");
        let scattered = parse_scattered_ore_config(&value).unwrap().unwrap();
        let mut scattered_chunk = flat_netherrack();
        let mut scattered_random = RecordingRandom::new(7);
        let scattered_success = {
            let mut level = WorldGenRegion::new(&mut scattered_chunk, MIN_Y, HEIGHT);
            scattered.place(&mut level, &mut scattered_random, BlockPos::new(8, 0, 8))
        };
        let scattered_draws = scattered_random.digest();
        let scattered_next = scattered_random.next_long();
        println!(
            "scattered rng: success={scattered_success}, draws={}, digest={scattered_draws:#018x}, next={scattered_next:#018x}, blocks={:#018x}",
            scattered_random.draws.len(),
            chunk_digest(&scattered_chunk)
        );

        assert!(diamond_success);
        assert_eq!(diamond_draws, 0xd03e_48eb_3aa1_8891);
        assert_eq!(diamond_next, 0x9732_95f1_4121_c8e1u64.cast_signed());
        assert_eq!(chunk_digest(&diamond_chunk), 0xf7e5_3335_07f3_ea9c);
        assert!(scattered_success);
        assert_eq!(scattered_draws, 0x1ea5_d17c_0d96_0d19);
        assert_eq!(scattered_next, 0xbe1a_4791_b54b_1905u64.cast_signed());
        assert_eq!(chunk_digest(&scattered_chunk), 0x480b_a613_4c04_5826);
    }

    fn column_chunk() -> Chunk {
        let mut chunk = Chunk {
            light_present: false,
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        for (x, z, y) in [(0, 0, 0), (1, 0, 5)] {
            chunk.set_block_state(
                x,
                y,
                z,
                BlockState {
                    name: "minecraft:stone".into(),
                    properties: Vec::new(),
                },
            );
        }
        chunk
    }

    #[test]
    fn cached_height_populates_hits_and_invalidates_only_the_written_column() {
        let mut chunk = column_chunk();
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            assert_eq!(level.motion_blocking_cache_len(), 0);

            assert!(level.has_motion_blocking_at_or_above(0, 0, 0));
            assert_eq!(level.motion_blocking_cache_len(), 1);

            assert!(!level.has_motion_blocking_at_or_above(0, 0, 1));
            assert_eq!(level.motion_blocking_cache_len(), 1);

            assert!(level.has_motion_blocking_at_or_above(1, 0, 5));
            assert_eq!(level.motion_blocking_cache_len(), 2);

            assert!(level.set_block(
                BlockPos::new(0, 100, 0),
                BlockState {
                    name: "minecraft:stone".into(),
                    properties: Vec::new(),
                }
            ));
            assert_eq!(level.motion_blocking_cache_len(), 1);

            assert!(level.has_motion_blocking_at_or_above(0, 0, 100));
            assert_eq!(level.motion_blocking_cache_len(), 2);
            assert!(level.has_motion_blocking_at_or_above(1, 0, 5));
            assert!(!level.has_motion_blocking_at_or_above(1, 0, 6));
            assert_eq!(level.motion_blocking_cache_len(), 2);
        }
    }

    #[test]
    fn empty_column_has_no_top_and_keeps_the_min_y_threshold() {
        let mut chunk = Chunk {
            light_present: false,
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        let level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        assert_eq!(level.motion_blocking_top(3, 3), None);
        assert_eq!(scan_motion_blocking_top(&level, 3, 3), None);
        assert!(level.has_motion_blocking_at_or_above(3, 3, MIN_Y - 2));
        assert!(level.has_motion_blocking_at_or_above(3, 3, MIN_Y));
        assert!(!level.has_motion_blocking_at_or_above(3, 3, MIN_Y + 1));
        assert_eq!(level.motion_blocking_cache_len(), 1);
    }

    #[test]
    fn cached_top_matches_uncached_scan_for_every_column_and_threshold() {
        let mut chunk = column_chunk();
        let level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
        for x in 0..16 {
            for z in 0..16 {
                let expected = scan_motion_blocking_top(&level, x, z);
                assert_eq!(level.motion_blocking_top(x, z), expected, "({x},{z})");
                for y in [MIN_Y - 2, MIN_Y, 0, 1, 5, 6, 100, MIN_Y + HEIGHT + 1] {
                    let scanned = y <= MIN_Y || expected.is_some_and(|top| top >= y);
                    assert_eq!(
                        level.has_motion_blocking_at_or_above(x, z, y),
                        scanned,
                        "({x},{z}) threshold {y}"
                    );
                }
            }
        }
        assert_eq!(level.motion_blocking_cache_len(), 16 * 16);
    }

    /// A level that deliberately keeps the trait's uncached default so the
    /// cached path can be compared against it byte for byte.
    struct UncachedLevel<'a> {
        inner: WorldGenRegion<'a>,
    }

    impl FeatureLevel for UncachedLevel<'_> {
        fn min_y(&self) -> i32 {
            self.inner.min_y()
        }

        fn height(&self) -> i32 {
            self.inner.height()
        }

        fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
            self.inner.get_block(pos)
        }

        fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
            self.inner.set_block(pos, state)
        }

        fn set_block_ref(&mut self, pos: BlockPos, state: &BlockState) -> bool {
            self.inner.set_block_ref(pos, state)
        }

        fn motion_blocking_top(&self, x: i32, z: i32) -> Option<i32> {
            scan_motion_blocking_top(self, x, z)
        }
    }

    #[test]
    fn cached_ore_placement_keeps_rng_and_block_digests() {
        let diamond = ore("ore_diamond_medium");
        let mut cached_chunk = flat_stone();
        let mut scanned_chunk = flat_stone();
        for chunk in [&mut cached_chunk, &mut scanned_chunk] {
            chunk.set_block_state(
                8,
                0,
                8,
                BlockState {
                    name: "minecraft:air".into(),
                    properties: Vec::new(),
                },
            );
        }

        let mut cached_random = RecordingRandom::new(7);
        let cached_success = {
            let mut level = WorldGenRegion::new(&mut cached_chunk, MIN_Y, HEIGHT);
            diamond.place(&mut level, &mut cached_random, BlockPos::new(8, 0, 8))
        };
        let mut scanned_random = RecordingRandom::new(7);
        let scanned_success = {
            let mut level = UncachedLevel {
                inner: WorldGenRegion::new(&mut scanned_chunk, MIN_Y, HEIGHT),
            };
            diamond.place(&mut level, &mut scanned_random, BlockPos::new(8, 0, 8))
        };

        assert_eq!(cached_success, scanned_success);
        assert_eq!(cached_random.digest(), 0xd03e_48eb_3aa1_8891);
        assert_eq!(cached_random.digest(), scanned_random.digest());
        assert_eq!(cached_random.next_long(), scanned_random.next_long());
        assert_eq!(chunk_digest(&cached_chunk), chunk_digest(&scanned_chunk));
        assert_eq!(chunk_digest(&cached_chunk), 0xf7e5_3335_07f3_ea9c);
    }

    /// One region serves both placements: the column cache survives the reset
    /// between them, while the RNG stream and generated blocks match the old
    /// behaviour of building a fresh region for every placement.
    #[test]
    fn shared_region_reuses_cache_between_ore_placements() {
        let diamond = ore("ore_diamond_medium");
        let origin = BlockPos::new(8, 0, 8);
        let mut shared_chunk = flat_stone();
        let mut fresh_chunk = flat_stone();
        for chunk in [&mut shared_chunk, &mut fresh_chunk] {
            chunk.set_block_state(
                8,
                0,
                8,
                BlockState {
                    name: "minecraft:air".into(),
                    properties: Vec::new(),
                },
            );
        }

        let mut shared_random = RecordingRandom::new(23);
        let (cached_after_first, second_writes) = {
            let mut level = WorldGenRegion::new(&mut shared_chunk, MIN_Y, HEIGHT).record_writes();
            assert!(diamond.place(&mut level, &mut shared_random, origin));
            let cached = level.motion_blocking_cache_len();
            assert!(cached > 0, "ore placement must populate the column cache");
            let first = level.take_written_blocks();
            assert!(!first.is_empty());

            level.reset_feature_output();
            assert_eq!(
                level.motion_blocking_cache_len(),
                cached,
                "reset must not discard terrain cache entries"
            );
            assert!(
                level.take_written_blocks().is_empty(),
                "reset must isolate writes"
            );

            assert!(diamond.place(&mut level, &mut shared_random, origin));
            assert!(
                level.motion_blocking_cache_len() >= cached,
                "reused region must keep serving cached columns"
            );
            (cached, level.take_written_blocks())
        };
        assert!(!second_writes.is_empty());

        let mut fresh_random = RecordingRandom::new(23);
        for _ in 0..2 {
            let mut level = WorldGenRegion::new(&mut fresh_chunk, MIN_Y, HEIGHT);
            assert!(diamond.place(&mut level, &mut fresh_random, origin));
        }

        assert_eq!(shared_random.digest(), fresh_random.digest());
        assert_eq!(chunk_digest(&shared_chunk), chunk_digest(&fresh_chunk));
        assert!(cached_after_first > 0);
    }
}
