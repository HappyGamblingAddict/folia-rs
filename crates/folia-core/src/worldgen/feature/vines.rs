//! Vines, twisting vines, and weeping vines.
//!
//! Ports `VinesFeature`, `TwistingVinesFeature`, and `WeepingVinesFeature`.
//! The two nether column helpers are exposed to the huge-fungus feature, which
//! hangs weeping vines from its hat with the same draw order.

use super::FeatureLevel;
use super::aquatic::{
    config_object, default_state, face_sturdy, int_field, is_empty_block, mth_next_int,
    set_property,
};
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::Value;
use std::io;

/// `minecraft:vines`.
#[derive(Debug, Clone, Copy, Default)]
pub struct VinesFeature;

impl VinesFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if !is_empty_block(level, origin) {
            return false;
        }
        for direction in Direction::ALL {
            if direction != Direction::Down
                && acceptable_neighbour(level, origin.relative(direction))
            {
                let mut vine = default_state("minecraft:vine");
                set_property(&mut vine, face_property(direction), "true");
                return level.set_block(origin, vine);
            }
        }
        false
    }
}

/// `MultifaceBlock.canAttachTo`: the neighbour must expose a full face.
fn acceptable_neighbour(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_some_and(|state| face_sturdy(&state.name))
}

fn face_property(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

/// `minecraft:twisting_vines`.
#[derive(Debug, Clone)]
pub struct TwistingVinesFeature {
    spread_width: i32,
    spread_height: i32,
    max_height: i32,
}

impl TwistingVinesFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if is_invalid_placement_location(level, origin) {
            return false;
        }
        for _ in 0..self.spread_width * self.spread_width {
            let dx = mth_next_int(random, -self.spread_width, self.spread_width);
            let dy = mth_next_int(random, -self.spread_height, self.spread_height);
            let dz = mth_next_int(random, -self.spread_width, self.spread_width);
            let mut pos = origin.offset(dx, dy, dz);
            if find_first_air_block_above_ground(level, &mut pos)
                && !is_invalid_placement_location(level, pos)
            {
                let mut length = mth_next_int(random, 1, self.max_height);
                if random.next_int_bound(6) == 0 {
                    length *= 2;
                }
                if random.next_int_bound(5) == 0 {
                    length = 1;
                }
                place_twisting_vines_column(level, random, pos, length, 17, 25);
            }
        }
        true
    }
}

/// `TwistingVinesFeature.findFirstAirBlockAboveGround`.
fn find_first_air_block_above_ground(level: &dyn FeatureLevel, pos: &mut BlockPos) -> bool {
    loop {
        *pos = pos.below();
        if is_outside_build_height(level, *pos) {
            return false;
        }
        if !is_empty_block(level, *pos) {
            break;
        }
    }
    *pos = pos.above();
    true
}

/// `TwistingVinesFeature.placeWeepingVinesColumn`: grows upwards.
pub(crate) fn place_twisting_vines_column(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
    length: i32,
    min_age: i32,
    max_age: i32,
) {
    let mut pos = origin;
    for step in 1..=length {
        if is_empty_block(level, pos) {
            if step == length || !is_empty_block(level, pos.above()) {
                let mut head = default_state("minecraft:twisting_vines");
                let age = mth_next_int(random, min_age, max_age);
                set_property(&mut head, "age", &age.to_string());
                level.set_block(pos, head);
                break;
            }
            level.set_block(pos, default_state("minecraft:twisting_vines_plant"));
        }
        pos = pos.above();
    }
}

fn is_invalid_placement_location(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    if !is_empty_block(level, pos) {
        return true;
    }
    !level.get_block(pos.below()).is_some_and(|below| {
        matches!(
            below.name.as_str(),
            "minecraft:netherrack" | "minecraft:warped_nylium" | "minecraft:warped_wart_block"
        )
    })
}

/// `minecraft:weeping_vines`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WeepingVinesFeature;

impl WeepingVinesFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if !is_empty_block(level, origin) {
            return false;
        }
        let is_ceiling = level.get_block(origin.above()).is_some_and(|above| {
            matches!(
                above.name.as_str(),
                "minecraft:netherrack" | "minecraft:nether_wart_block"
            )
        });
        if !is_ceiling {
            return false;
        }
        place_roof_nether_wart(level, random, origin);
        place_roof_weeping_vines(level, random, origin);
        true
    }
}

fn place_roof_nether_wart(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
) {
    level.set_block(origin, default_state("minecraft:nether_wart_block"));
    for _ in 0..200 {
        let dx = random.next_int_bound(6) - random.next_int_bound(6);
        let dy = random.next_int_bound(2) - random.next_int_bound(5);
        let dz = random.next_int_bound(6) - random.next_int_bound(6);
        let pos = origin.offset(dx, dy, dz);
        if !is_empty_block(level, pos) {
            continue;
        }
        let mut neighbours = 0;
        for direction in Direction::ALL {
            let is_wart = level
                .get_block(pos.relative(direction))
                .is_some_and(|state| {
                    matches!(
                        state.name.as_str(),
                        "minecraft:netherrack" | "minecraft:nether_wart_block"
                    )
                });
            if is_wart {
                neighbours += 1;
            }
            if neighbours > 1 {
                break;
            }
        }
        if neighbours == 1 {
            level.set_block(pos, default_state("minecraft:nether_wart_block"));
        }
    }
}

fn place_roof_weeping_vines(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
) {
    for _ in 0..100 {
        let dx = random.next_int_bound(8) - random.next_int_bound(8);
        let dy = random.next_int_bound(2) - random.next_int_bound(7);
        let dz = random.next_int_bound(8) - random.next_int_bound(8);
        let pos = origin.offset(dx, dy, dz);
        if !is_empty_block(level, pos) {
            continue;
        }
        let is_ceiling = level.get_block(pos.above()).is_some_and(|above| {
            matches!(
                above.name.as_str(),
                "minecraft:netherrack" | "minecraft:nether_wart_block"
            )
        });
        if !is_ceiling {
            continue;
        }
        let mut length = mth_next_int(random, 1, 8);
        if random.next_int_bound(6) == 0 {
            length *= 2;
        }
        if random.next_int_bound(5) == 0 {
            length = 1;
        }
        place_weeping_vines_column(level, random, pos, length, 17, 25);
    }
}

/// `WeepingVinesFeature.placeWeepingVinesColumn`: grows downwards.
pub(crate) fn place_weeping_vines_column(
    level: &mut dyn FeatureLevel,
    random: &mut dyn RandomSource,
    origin: BlockPos,
    height: i32,
    min_age: i32,
    max_age: i32,
) {
    let mut pos = origin;
    for step in 0..=height {
        if is_empty_block(level, pos) {
            if step == height || !is_empty_block(level, pos.below()) {
                let mut head = default_state("minecraft:weeping_vines");
                let age = mth_next_int(random, min_age, max_age);
                set_property(&mut head, "age", &age.to_string());
                level.set_block(pos, head);
                break;
            }
            level.set_block(pos, default_state("minecraft:weeping_vines_plant"));
        }
        pos = pos.below();
    }
}

pub(crate) fn is_outside_build_height(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    pos.y() < level.min_y() || pos.y() >= level.min_y() + level.height()
}

pub(crate) fn parse_vines_config(config: &Value) -> io::Result<Option<VinesFeature>> {
    config_object(config, "vines")?;
    Ok(Some(VinesFeature))
}

pub(crate) fn parse_twisting_vines_config(
    config: &Value,
) -> io::Result<Option<TwistingVinesFeature>> {
    let object = config_object(config, "twisting vines")?;
    Ok(Some(TwistingVinesFeature {
        spread_width: int_field(object, "spread_width", 1, i32::MAX)?,
        spread_height: int_field(object, "spread_height", 1, i32::MAX)?,
        max_height: int_field(object, "max_height", 1, i32::MAX)?,
    }))
}

pub(crate) fn parse_weeping_vines_config(
    config: &Value,
) -> io::Result<Option<WeepingVinesFeature>> {
    config_object(config, "weeping vines")?;
    Ok(Some(WeepingVinesFeature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::worldgen::feature::WorldGenRegion;
    use folia_random::LegacyRandomSource;

    const MIN_Y: i32 = -64;
    const HEIGHT: i32 = 384;

    #[test]
    fn column_helpers_place_head_and_body_in_java_order() {
        let mut chunk = Chunk {
            x: 0,
            z: 0,
            sections: Vec::new(),
        };
        {
            let mut level = WorldGenRegion::new(&mut chunk, MIN_Y, HEIGHT);
            let mut random = LegacyRandomSource::new(7);
            place_twisting_vines_column(
                &mut level,
                &mut random,
                BlockPos::new(4, 70, 4),
                3,
                17,
                25,
            );
            place_weeping_vines_column(&mut level, &mut random, BlockPos::new(8, 70, 8), 2, 23, 25);
        }
        let twisting_plant = chunk.block_state(4, 70, 4).unwrap();
        assert_eq!(twisting_plant.name, "minecraft:twisting_vines_plant");
        let twisting_head = chunk.block_state(4, 72, 4).unwrap();
        assert_eq!(twisting_head.name, "minecraft:twisting_vines");
        let age = twisting_head
            .properties
            .iter()
            .find(|(key, _)| key == "age")
            .map(|(_, value)| value.parse::<i32>().unwrap());
        assert!((17..=25).contains(&age.unwrap()));
        assert_eq!(
            chunk.block_state(8, 68, 8).unwrap().name,
            "minecraft:weeping_vines"
        );
        assert_eq!(
            chunk.block_state(8, 69, 8).unwrap().name,
            "minecraft:weeping_vines_plant"
        );
    }
}
