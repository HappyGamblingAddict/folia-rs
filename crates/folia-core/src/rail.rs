//! Stage-A rail block shapes and neighbour-driven reconnection/powering.

use crate::chunk::BlockState;
use crate::math::{BlockPos, Direction};
use crate::redstone;
use crate::tick::{BlockBehavior, TickWorld};
use std::collections::HashSet;
use std::io;

pub const RAIL_BLOCKS: [&str; 4] = [
    "minecraft:rail",
    "minecraft:powered_rail",
    "minecraft:detector_rail",
    "minecraft:activator_rail",
];

pub fn is_rail(state: &BlockState) -> bool {
    RAIL_BLOCKS.contains(&state.name.as_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RailShape {
    NorthSouth,
    EastWest,
    AscendingNorth,
    AscendingSouth,
    AscendingEast,
    AscendingWest,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl RailShape {
    pub fn name(self) -> &'static str {
        match self {
            Self::NorthSouth => "north_south",
            Self::EastWest => "east_west",
            Self::AscendingNorth => "ascending_north",
            Self::AscendingSouth => "ascending_south",
            Self::AscendingEast => "ascending_east",
            Self::AscendingWest => "ascending_west",
            Self::SouthEast => "south_east",
            Self::SouthWest => "south_west",
            Self::NorthWest => "north_west",
            Self::NorthEast => "north_east",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "north_south" => Some(Self::NorthSouth),
            "east_west" => Some(Self::EastWest),
            "ascending_north" => Some(Self::AscendingNorth),
            "ascending_south" => Some(Self::AscendingSouth),
            "ascending_east" => Some(Self::AscendingEast),
            "ascending_west" => Some(Self::AscendingWest),
            "south_east" => Some(Self::SouthEast),
            "south_west" => Some(Self::SouthWest),
            "north_west" => Some(Self::NorthWest),
            "north_east" => Some(Self::NorthEast),
            _ => None,
        }
    }

    pub fn is_ascending(self) -> bool {
        matches!(
            self,
            Self::AscendingNorth | Self::AscendingSouth | Self::AscendingEast | Self::AscendingWest
        )
    }
}

pub fn shape_of(state: &BlockState) -> Option<RailShape> {
    redstone::property(state, "shape").and_then(RailShape::from_name)
}

pub fn connected_shape(
    current: RailShape,
    north: bool,
    south: bool,
    east: bool,
    west: bool,
) -> RailShape {
    if current.is_ascending() {
        return current;
    }

    let count = usize::from(north) + usize::from(south) + usize::from(east) + usize::from(west);

    let current_connected = match current {
        RailShape::NorthSouth => north && south,
        RailShape::EastWest => east && west,
        RailShape::SouthEast => south && east,
        RailShape::SouthWest => south && west,
        RailShape::NorthWest => north && west,
        RailShape::NorthEast => north && east,
        _ => false,
    };

    if count >= 3 {
        if north && south {
            return RailShape::NorthSouth;
        }
        if east && west {
            return RailShape::EastWest;
        }
        return RailShape::NorthSouth;
    }

    if current_connected {
        return current;
    }

    if north && south {
        return RailShape::NorthSouth;
    }
    if east && west {
        return RailShape::EastWest;
    }
    if north && east {
        return RailShape::NorthEast;
    }
    if north && west {
        return RailShape::NorthWest;
    }
    if south && east {
        return RailShape::SouthEast;
    }
    if south && west {
        return RailShape::SouthWest;
    }
    RailShape::NorthSouth
}

fn set_property(state: &mut BlockState, key: &str, value: &str) {
    if let Some((_, existing)) = state.properties.iter_mut().find(|(name, _)| name == key) {
        *existing = value.to_owned();
    } else {
        state.properties.push((key.to_owned(), value.to_owned()));
    }
}

fn property_bool(state: &BlockState, key: &str) -> bool {
    matches!(redstone::property(state, key), Some("true"))
}

fn rail_power_source(world: &mut TickWorld<'_>, pos: BlockPos) -> io::Result<bool> {
    let offsets = [
        (0, 1, 0),
        (0, -1, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    for (dx, dy, dz) in offsets {
        let Some(state) = world.block_state(pos.offset(dx, dy, dz))? else {
            continue;
        };
        let powered = match state.name.as_str() {
            "minecraft:redstone_block" => true,
            "minecraft:redstone_wire" => {
                redstone::property(&state, "power")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
                    > 0
            }
            name if name.ends_with("_button")
                || name.ends_with("_pressure_plate")
                || name == "minecraft:lever"
                || name == "minecraft:tripwire_hook"
                || name == "minecraft:tripwire" =>
            {
                property_bool(&state, "powered")
            }
            "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => {
                property_bool(&state, "lit")
            }
            "minecraft:target" => {
                redstone::property(&state, "power")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0)
                    > 0
            }
            "minecraft:detector_rail" => property_bool(&state, "powered"),
            _ => false,
        };
        if powered {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Simplification: powered-rail chain propagation searches horizontal rail
/// neighbours up to 8 blocks without separate ascending/turn-specific rules.
fn powered_rail_active(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    depth: i32,
    visited: &mut HashSet<BlockPos>,
) -> io::Result<bool> {
    if depth > 8 || !visited.insert(pos) {
        return Ok(false);
    }
    if rail_power_source(world, pos)? {
        return Ok(true);
    }
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let neighbor = pos.offset(dx, 0, dz);
        let Some(state) = world.block_state(neighbor)? else {
            continue;
        };
        if is_rail(&state) && powered_rail_active(world, neighbor, depth + 1, visited)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub struct RailBehavior;

pub static RAIL_BEHAVIOR: RailBehavior = RailBehavior;

impl BlockBehavior for RailBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _old: Option<&BlockState>,
    ) -> io::Result<()> {
        let new_state = compute_rail_state(world, pos, state)?;
        if new_state != *state {
            let _ = world.set_block(pos, new_state)?;
        }
        Ok(())
    }

    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        let new_state = compute_rail_state(world, pos, state)?;
        if new_state != *state {
            let _ = world.set_block(pos, new_state)?;
        }
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        _direction: Direction,
        _neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        let new_state = compute_rail_state(world, pos, state)?;
        if new_state != *state {
            Ok(Some(new_state))
        } else {
            Ok(None)
        }
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
    ) -> io::Result<()> {
        let new_state = compute_rail_state(world, pos, state)?;
        if new_state != *state {
            let _ = world.set_block(pos, new_state)?;
        }
        Ok(())
    }
}

fn compute_rail_state(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<BlockState> {
    let mut new_state = compute_rail_shape(world, pos, state)?;
    if new_state.name == "minecraft:powered_rail" {
        let mut visited = HashSet::new();
        let active = powered_rail_active(world, pos, 0, &mut visited)?;
        set_property(
            &mut new_state,
            "powered",
            if active { "true" } else { "false" },
        );
    }
    Ok(new_state)
}

fn compute_rail_shape(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<BlockState> {
    let current = shape_of(state).unwrap_or(RailShape::NorthSouth);
    let north = world
        .block_state(pos.offset(0, 0, -1))?
        .map(|neighbor| is_rail(&neighbor))
        .unwrap_or(false);
    let south = world
        .block_state(pos.offset(0, 0, 1))?
        .map(|neighbor| is_rail(&neighbor))
        .unwrap_or(false);
    let east = world
        .block_state(pos.offset(1, 0, 0))?
        .map(|neighbor| is_rail(&neighbor))
        .unwrap_or(false);
    let west = world
        .block_state(pos.offset(-1, 0, 0))?
        .map(|neighbor| is_rail(&neighbor))
        .unwrap_or(false);

    let new_shape = connected_shape(current, north, south, east, west);
    if Some(new_shape) == shape_of(state) {
        return Ok(state.clone());
    }

    let mut new_state = state.clone();
    set_property(&mut new_state, "shape", new_shape.name());
    Ok(new_state)
}
