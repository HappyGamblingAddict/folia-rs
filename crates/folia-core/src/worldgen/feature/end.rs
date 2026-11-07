//! End dimension and void feature ports.
//!
//! Ports `ChorusPlantFeature`, `EndIslandFeature`, `EndGatewayFeature`,
//! `EndPlatformFeature`, and `VoidStartPlatformFeature` from the decompiled
//! 1.21.11 tree. These features write block states only; the gateway's exit
//! coordinate is attached through the existing [`FeatureLevel`] block-entity
//! sink. `EndSpikeFeature` stays unsupported because it needs the level seed
//! plus End-crystal entity spawning.

use super::support::{optional_bool, set_property, state, state_with};
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::nbt::Tag;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::Value;
use std::io;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn horizontal() -> impl Iterator<Item = Direction> {
    Direction::ALL.into_iter().filter(|d| d.is_horizontal())
}

fn random_horizontal(random: &mut dyn RandomSource) -> Direction {
    // `Direction.Plane.HORIZONTAL.getRandomDirection`: from2DDataValue.
    Direction::from_2d_data(random.next_int_bound(4))
}

fn is_empty(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| is_air_name(&state.name))
}

fn name_at(level: &dyn FeatureLevel, pos: BlockPos) -> Option<&str> {
    level.get_block(pos).map(|state| state.name.as_str())
}

fn is_chorus_or_flower(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:chorus_plant" | "minecraft:chorus_flower"
    )
}

/// `ChorusPlantBlock.getStateWithConnections`.
fn chorus_plant_state(level: &dyn FeatureLevel, pos: BlockPos) -> BlockState {
    let mut state = state("minecraft:chorus_plant");
    for (key, value) in [
        ("down", false),
        ("up", false),
        ("north", false),
        ("east", false),
        ("south", false),
        ("west", false),
    ] {
        state.properties.push((key.to_owned(), value.to_string()));
    }
    let connect = |neighbor: BlockPos, allow_end_stone: bool| {
        level.get_block(neighbor).is_some_and(|other| {
            is_chorus_or_flower(other) || (allow_end_stone && other.name == "minecraft:end_stone")
        })
    };
    let below = connect(pos.below(), true);
    let up = connect(pos.above(), false);
    let north = connect(pos.north(), false);
    let east = connect(pos.east(), false);
    let south = connect(pos.south(), false);
    let west = connect(pos.west(), false);
    let _ = set_property(&mut state, "down", if below { "true" } else { "false" });
    let _ = set_property(&mut state, "up", if up { "true" } else { "false" });
    let _ = set_property(&mut state, "north", if north { "true" } else { "false" });
    let _ = set_property(&mut state, "east", if east { "true" } else { "false" });
    let _ = set_property(&mut state, "south", if south { "true" } else { "false" });
    let _ = set_property(&mut state, "west", if west { "true" } else { "false" });
    state
}

fn all_neighbors_empty(
    level: &dyn FeatureLevel,
    pos: BlockPos,
    excluding: Option<Direction>,
) -> bool {
    horizontal().all(|direction| {
        if excluding == Some(direction) {
            true
        } else {
            is_empty(level, pos.relative(direction))
        }
    })
}

fn place_chorus_plant(
    level: &mut dyn FeatureLevel,
    branch: BlockPos,
    original: BlockPos,
    random: &mut dyn RandomSource,
    max_horizontal_distance: i32,
    iterations: i32,
) {
    let mut height = random.next_int_bound(4) + 1;
    if iterations == 0 {
        height += 1;
    }
    for index in 0..height {
        let pos = branch.above_n(index + 1);
        if !all_neighbors_empty(level, pos, None) {
            return;
        }
        let plant = chorus_plant_state(level, pos);
        level.set_block(pos, plant);
        let below = pos.below();
        let plant = chorus_plant_state(level, below);
        level.set_block(below, plant);
    }

    let mut branched = false;
    if iterations < 4 {
        let mut branches = random.next_int_bound(4);
        if iterations == 0 {
            branches += 1;
        }
        for _ in 0..branches {
            let direction = random_horizontal(random);
            let pos = branch.above_n(height).relative(direction);
            if (pos.x() - original.x()).abs() < max_horizontal_distance
                && (pos.z() - original.z()).abs() < max_horizontal_distance
                && is_empty(level, pos)
                && is_empty(level, pos.below())
                && all_neighbors_empty(level, pos, Some(direction.opposite()))
            {
                branched = true;
                let plant = chorus_plant_state(level, pos);
                level.set_block(pos, plant);
                let support = pos.relative(direction.opposite());
                let plant = chorus_plant_state(level, support);
                level.set_block(support, plant);
                place_chorus_plant(
                    level,
                    pos,
                    original,
                    random,
                    max_horizontal_distance,
                    iterations + 1,
                );
            }
        }
    }

    if !branched {
        let flower = state_with("minecraft:chorus_flower", &[("age", "5")]);
        level.set_block(branch.above_n(height), flower);
    }
}

/// `ChorusFlowerBlock.generatePlant`.
fn generate_chorus_plant(
    level: &mut dyn FeatureLevel,
    pos: BlockPos,
    random: &mut dyn RandomSource,
    max_horizontal_distance: i32,
) {
    let plant = chorus_plant_state(level, pos);
    level.set_block(pos, plant);
    place_chorus_plant(level, pos, pos, random, max_horizontal_distance, 0);
}

/// `minecraft:chorus_plant`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChorusPlantFeature;

impl ChorusPlantFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        if is_empty(level, origin) && name_at(level, origin.below()) == Some("minecraft:end_stone")
        {
            generate_chorus_plant(level, origin, random, 8);
            true
        } else {
            false
        }
    }
}

/// `minecraft:end_island`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EndIslandFeature;

impl EndIslandFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut radius = random.next_int_bound(3) as f32 + 4.0;
        let end_stone = state("minecraft:end_stone");
        for index in 0.. {
            if radius <= 0.5 {
                break;
            }
            let floor = (-radius).floor() as i32;
            let ceil = radius.ceil() as i32;
            for x in floor..=ceil {
                for z in floor..=ceil {
                    if (x * x + z * z) as f32 <= (radius + 1.0) * (radius + 1.0) {
                        level.set_block(origin.offset(x, index, z), end_stone.clone());
                    }
                }
            }
            radius -= random.next_int_bound(2) as f32 + 0.5;
        }
        true
    }
}

/// `minecraft:end_gateway`.
#[derive(Debug, Clone)]
pub struct EndGatewayFeature {
    exit: Option<BlockPos>,
    exact: bool,
}

impl EndGatewayFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        for x in -1..=1 {
            for y in -2..=2 {
                for z in -1..=1 {
                    let pos = origin.offset(x, y, z);
                    let same_x = pos.x() == origin.x();
                    let same_y = pos.y() == origin.y();
                    let same_z = pos.z() == origin.z();
                    let high_y = (pos.y() - origin.y()).abs() == 2;
                    if same_x && same_y && same_z {
                        level.set_block(pos, state("minecraft:end_gateway"));
                        level.set_block_entity(pos, self.block_entity_nbt(pos));
                    } else if same_y {
                        level.set_block(pos, state("minecraft:air"));
                    } else if (high_y && same_x && same_z) || ((same_x || same_z) && !high_y) {
                        level.set_block(pos, state("minecraft:bedrock"));
                    } else {
                        level.set_block(pos, state("minecraft:air"));
                    }
                }
            }
        }
        true
    }

    fn block_entity_nbt(&self, pos: BlockPos) -> Tag {
        let mut fields = vec![
            (
                "id".to_owned(),
                Tag::String("minecraft:end_gateway".to_owned()),
            ),
            ("x".to_owned(), Tag::Int(pos.x())),
            ("y".to_owned(), Tag::Int(pos.y())),
            ("z".to_owned(), Tag::Int(pos.z())),
            ("Age".to_owned(), Tag::Long(0)),
        ];
        if let Some(exit) = self.exit {
            fields.push((
                "exit_portal".to_owned(),
                Tag::IntArray(vec![exit.x(), exit.y(), exit.z()]),
            ));
            if self.exact {
                fields.push(("ExactTeleport".to_owned(), Tag::Byte(1)));
            }
        }
        Tag::Compound(fields)
    }
}

pub(crate) fn parse_end_gateway_config(config: &Value) -> io::Result<EndGatewayFeature> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("end_gateway config must be an object"))?;
    let exit = match object.get("exit") {
        None => None,
        Some(value) => {
            let values = value
                .as_array()
                .ok_or_else(|| invalid("end_gateway `exit` must be a position list"))?;
            if values.len() != 3 {
                return Err(invalid("end_gateway `exit` must have three values"));
            }
            let mut coords = [0i32; 3];
            for (index, value) in values.iter().enumerate() {
                coords[index] = i32::try_from(
                    value
                        .as_i64()
                        .ok_or_else(|| invalid("end_gateway exit coordinates must be integers"))?,
                )
                .map_err(|_| invalid("end_gateway exit coordinate does not fit in i32"))?;
            }
            Some(BlockPos::new(coords[0], coords[1], coords[2]))
        }
    };
    Ok(EndGatewayFeature {
        exit,
        exact: optional_bool(object, "exact", false),
    })
}

/// `minecraft:end_platform`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EndPlatformFeature;

impl EndPlatformFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let obsidian = state("minecraft:obsidian");
        let air = state("minecraft:air");
        for x in -2..=2 {
            for z in -2..=2 {
                for y in -1..3 {
                    let pos = origin.offset(z, y, x);
                    if y == -1 {
                        level.set_block(pos, obsidian.clone());
                    } else {
                        level.set_block(pos, air.clone());
                    }
                }
            }
        }
        true
    }
}

/// `minecraft:void_start_platform`.
#[derive(Debug, Clone, Copy, Default)]
pub struct VoidStartPlatformFeature;

impl VoidStartPlatformFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let origin_chunk_x = origin.x().div_euclid(16);
        let origin_chunk_z = origin.z().div_euclid(16);
        if origin_chunk_x.abs().max(origin_chunk_z.abs()) > 1 {
            return true;
        }
        let platform = BlockPos::new(8, origin.y() + 3, 8);
        let min_x = origin_chunk_x * 16;
        let min_z = origin_chunk_z * 16;
        let stone = state("minecraft:stone");
        let cobblestone = state("minecraft:cobblestone");
        for z in min_z..min_z + 16 {
            for x in min_x..min_x + 16 {
                let distance = (platform.x() - x).abs().max((platform.z() - z).abs());
                if distance <= 16 {
                    let pos = BlockPos::new(x, platform.y(), z);
                    if pos == platform {
                        level.set_block(pos, cobblestone.clone());
                    } else {
                        level.set_block(pos, stone.clone());
                    }
                }
            }
        }
        true
    }
}
