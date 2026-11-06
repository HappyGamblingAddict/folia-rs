//! Per-state block behaviour dumped from the 1.21.11 Java oracle.
//!
//! `assets/block_state_behaviour.tsv.gz` contains one row per block state with
//! the properties lighting, physics, ticking, and breaking need. The source is
//! `DumpBlockStateBehaviour.java` in the reference `java-tools` tree. Keeping
//! the 18 MB TSV gzip-compressed embeds it in ~284 KB.

use crate::chunk::BlockState;
use crate::registry;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::OnceLock;

const BLOCK_STATE_BEHAVIOUR: &[u8] = include_bytes!("../assets/block_state_behaviour.tsv.gz");

/// The per-state properties this port currently consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockBehaviour {
    pub id: i32,
    pub name: String,
    pub air: bool,
    pub blocks_motion: bool,
    pub solid: bool,
    pub has_collision: bool,
    pub light_emission: u8,
    pub light_block: u8,
    pub random_tick: bool,
    pub occludes: bool,
    pub replaceable: bool,
    pub hardness: f32,
    pub blast_resistance: f32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub fluid: Option<String>,
}

impl BlockBehaviour {
    /// Whether a light ray can pass through this state without attenuating.
    pub fn is_transparent(&self) -> bool {
        self.light_block == 0
    }
}

struct BehaviourTable {
    states: Vec<BlockBehaviour>,
    defaults: HashMap<String, i32>,
}

static TABLE: OnceLock<io::Result<BehaviourTable>> = OnceLock::new();

fn parse() -> io::Result<BehaviourTable> {
    let mut decoder = GzDecoder::new(BLOCK_STATE_BEHAVIOUR);
    let mut text = String::new();
    decoder.read_to_string(&mut text)?;

    let mut states = Vec::with_capacity(29_671);
    let mut defaults = HashMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 18 {
            continue;
        }
        let parse_err = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("block behaviour row has invalid {what}"),
            )
        };
        let id: i32 = fields[0].parse().map_err(|_| parse_err("id"))?;
        let name = fields[1].to_owned();
        let boolean = |value: &str| value == "true";
        let light_emission: u8 = fields[7].parse().map_err(|_| parse_err("light emission"))?;
        let light_block: u8 = fields[8].parse().map_err(|_| parse_err("light block"))?;
        let hardness: f32 = fields[12].parse().map_err(|_| parse_err("hardness"))?;
        let blast_resistance: f32 = fields[13].parse().map_err(|_| parse_err("blast"))?;
        let friction: f32 = fields[14].parse().map_err(|_| parse_err("friction"))?;
        let speed_factor: f32 = fields[15].parse().map_err(|_| parse_err("speed"))?;
        let jump_factor: f32 = fields[16].parse().map_err(|_| parse_err("jump"))?;
        let fluid = if fields[17].is_empty() {
            None
        } else {
            Some(fields[17].to_owned())
        };
        defaults.entry(name.clone()).or_insert(id);
        states.push(BlockBehaviour {
            id,
            name,
            air: boolean(fields[3]),
            blocks_motion: boolean(fields[4]),
            solid: boolean(fields[5]),
            has_collision: boolean(fields[6]),
            light_emission,
            light_block,
            random_tick: boolean(fields[9]),
            occludes: boolean(fields[10]),
            replaceable: boolean(fields[11]),
            hardness,
            blast_resistance,
            friction,
            speed_factor,
            jump_factor,
            fluid,
        });
    }

    if states.len() != 29_671 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "expected 29,671 block behaviour rows, found {}",
                states.len()
            ),
        ));
    }
    for (index, state) in states.iter().enumerate() {
        if state.id != index as i32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "block behaviour ids are not dense",
            ));
        }
    }
    Ok(BehaviourTable { states, defaults })
}

fn table() -> Option<&'static BehaviourTable> {
    TABLE.get_or_init(parse).as_ref().ok()
}

/// Behaviour of a block-state id.
pub fn by_id(id: i32) -> Option<&'static BlockBehaviour> {
    usize::try_from(id)
        .ok()
        .and_then(|index| table()?.states.get(index))
}

/// Behaviour of a concrete [`BlockState`], resolving properties through the
/// registry and falling back to the block's default state.
pub fn for_state(state: &BlockState) -> Option<&'static BlockBehaviour> {
    by_id(registry::block_state_id(state)).or_else(|| for_block(&state.name))
}

/// Behaviour of a block's default state.
pub fn for_block(name: &str) -> Option<&'static BlockBehaviour> {
    let table = table()?;
    let id = table
        .defaults
        .get(name)
        .copied()
        .or_else(|| registry::block_default_id(name))?;
    by_id(id)
}

/// Light level emitted by a state.
pub fn light_emission(state: &BlockState) -> u8 {
    for_state(state).map_or(0, |behaviour| behaviour.light_emission)
}

/// Light attenuation through a state (0 = transparent, 15 = opaque).
pub fn light_block(state: &BlockState) -> u8 {
    for_state(state).map_or(15, |behaviour| behaviour.light_block)
}

/// Whether a state blocks motion.
pub fn blocks_motion(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.blocks_motion)
}

/// Whether a state has a collision shape.
pub fn has_collision(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.has_collision)
}

/// Whether a state is solid.
pub fn is_solid(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.solid)
}

/// Whether a state randomly ticks.
pub fn random_tick(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.random_tick)
}

/// Whether a state fully occludes neighbour faces.
pub fn occludes(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.occludes)
}

/// Whether a state is replaceable without breaking.
pub fn replaceable(state: &BlockState) -> bool {
    for_state(state).is_some_and(|behaviour| behaviour.replaceable)
}

/// Block hardness (`Block.defaultDestroyTime`).
pub fn hardness(state: &BlockState) -> f32 {
    for_state(state).map_or(0.0, |behaviour| behaviour.hardness)
}

/// Blast resistance.
pub fn blast_resistance(state: &BlockState) -> f32 {
    for_state(state).map_or(0.0, |behaviour| behaviour.blast_resistance)
}

/// Friction (0.6 by default, 0.98 for ice, 0.8 for slime).
pub fn friction(state: &BlockState) -> f32 {
    for_state(state).map_or(0.6, |behaviour| behaviour.friction)
}

/// Speed factor (`Block.getSpeedFactor`).
pub fn speed_factor(state: &BlockState) -> f32 {
    for_state(state).map_or(1.0, |behaviour| behaviour.speed_factor)
}

/// Jump factor (`Block.getJumpFactor`).
pub fn jump_factor(state: &BlockState) -> f32 {
    for_state(state).map_or(1.0, |behaviour| behaviour.jump_factor)
}

/// Non-empty fluid state, if the block state carries one.
pub fn fluid(state: &BlockState) -> Option<&'static str> {
    for_state(state)?.fluid.as_deref()
}

/// Number of loaded block-state rows.
pub fn state_count() -> usize {
    table().map_or(0, |table| table.states.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    #[test]
    fn every_block_state_has_a_behaviour_row() {
        assert_eq!(state_count(), 29_671);
        assert_eq!(
            by_id(0).map(|behaviour| behaviour.name.as_str()),
            Some("minecraft:air")
        );
        assert_eq!(by_id(29_670).map(|behaviour| behaviour.id), Some(29_670));
        assert!(by_id(29_671).is_none());
    }

    #[test]
    fn stone_air_and_torch_metadata_match_java() {
        let stone = for_block("minecraft:stone").unwrap();
        assert!(stone.blocks_motion && stone.solid && stone.has_collision);
        assert!(stone.occludes);
        assert_eq!(stone.light_block, 15);
        assert_eq!(stone.hardness, 1.5);

        let air = for_block("minecraft:air").unwrap();
        assert!(air.air && air.replaceable && !air.blocks_motion);

        let torch = state("minecraft:torch", &[]);
        assert_eq!(light_emission(&torch), 14);
        assert_eq!(light_block(&torch), 0);
    }

    #[test]
    fn fluid_and_state_specific_metadata() {
        let water = state("minecraft:water", &[("level", "0")]);
        assert_eq!(fluid(&water), Some("minecraft:water"));
        assert!(!blocks_motion(&water));

        let ice = for_block("minecraft:ice").unwrap();
        assert_eq!(ice.friction, 0.98);
        assert_eq!(ice.speed_factor, 1.0);

        let lit_furnace = state("minecraft:furnace", &[("lit", "true"), ("facing", "north")]);
        assert_eq!(light_emission(&lit_furnace), 13);
    }
}
