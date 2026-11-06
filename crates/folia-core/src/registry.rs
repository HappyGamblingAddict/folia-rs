//! Full 1.21.11 registry lookups generated from the Java server.
//!
//! The assets are tab-separated dumps:
//!
//! * `block_states.tsv`: state id, block name, sorted `key=value` properties
//! * `block_defaults.tsv`: block name, default state id
//! * `item_ids.tsv`: item id, item name
//! * `entity_type_ids.tsv`: entity type id, entity type name
//!
//! Block state ids are protocol registry ids. States created by worldgen often
//! omit properties, so an unknown/partial property set falls back to the
//! block's default state rather than to stone.

use crate::chunk::BlockState;
use std::collections::HashMap;
use std::sync::OnceLock;

const BLOCK_STATES: &str = include_str!("../assets/block_states.tsv");
const BLOCK_DEFAULTS: &str = include_str!("../assets/block_defaults.tsv");
const ITEM_IDS: &str = include_str!("../assets/item_ids.tsv");
const ENTITY_TYPE_IDS: &str = include_str!("../assets/entity_type_ids.tsv");

const STONE: i32 = 1;

struct BlockRegistry {
    defaults: HashMap<&'static str, i32>,
    exact: HashMap<&'static str, HashMap<String, i32>>,
    states_by_id: HashMap<i32, (&'static str, &'static str)>,
    state_count: usize,
}

static BLOCKS: OnceLock<BlockRegistry> = OnceLock::new();

fn blocks() -> &'static BlockRegistry {
    BLOCKS.get_or_init(|| {
        let mut defaults = HashMap::new();
        for line in BLOCK_DEFAULTS.lines() {
            let Some((name, id)) = line.split_once('\t') else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            defaults.insert(name, id);
        }

        let mut exact: HashMap<&'static str, HashMap<String, i32>> = HashMap::new();
        let mut states_by_id = HashMap::new();
        let mut state_count = 0usize;
        for line in BLOCK_STATES.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(id), Some(name)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            let properties = parts.next().unwrap_or("");
            states_by_id.insert(id, (name, properties));
            if !properties.is_empty() {
                let mut property_names: Vec<&str> = properties.split(',').collect();
                property_names.sort_unstable();
                exact
                    .entry(name)
                    .or_default()
                    .insert(property_names.join(","), id);
            }
            state_count += 1;
        }

        BlockRegistry {
            defaults,
            exact,
            states_by_id,
            state_count,
        }
    })
}

/// Returns the protocol block-state id for `state`.
pub fn block_state_id(state: &BlockState) -> i32 {
    let registry = blocks();
    if !state.properties.is_empty() {
        let mut properties: Vec<(&str, &str)> = state
            .properties
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        properties.sort_unstable();
        let mut key = String::new();
        for (index, (name, value)) in properties.iter().enumerate() {
            if index > 0 {
                key.push(',');
            }
            key.push_str(name);
            key.push('=');
            key.push_str(value);
        }
        if let Some(states) = registry.exact.get(state.name.as_str())
            && let Some(id) = states.get(key.as_str())
        {
            return *id;
        }
    }
    registry
        .defaults
        .get(state.name.as_str())
        .copied()
        .unwrap_or(STONE)
}

/// Returns the number of block states loaded from `block_states.tsv`.
pub fn block_state_count() -> usize {
    blocks().state_count
}

/// Returns the default state id for a block name.
pub fn block_default_id(name: &str) -> Option<i32> {
    blocks().defaults.get(name).copied()
}

/// Returns the full default state (including its default properties) for a
/// block name.
pub fn default_block_state(name: &str) -> Option<BlockState> {
    let registry = blocks();
    let id = *registry.defaults.get(name)?;
    let (name, properties) = *registry.states_by_id.get(&id)?;
    let properties = if properties.is_empty() {
        Vec::new()
    } else {
        properties
            .split(',')
            .map(|pair| {
                let (key, value) = pair.split_once('=')?;
                Some((key.to_owned(), value.to_owned()))
            })
            .collect::<Option<Vec<_>>>()?
    };
    Some(BlockState {
        name: name.to_owned(),
        properties,
    })
}

struct ItemRegistry {
    ids: HashMap<&'static str, i32>,
    names: Vec<Option<&'static str>>,
}

static ITEMS: OnceLock<ItemRegistry> = OnceLock::new();

fn items() -> &'static ItemRegistry {
    ITEMS.get_or_init(|| {
        let mut ids = HashMap::new();
        let mut names: Vec<Option<&'static str>> = Vec::new();
        for line in ITEM_IDS.lines() {
            let Some((id, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            if id < 0 {
                continue;
            }
            let id = id as usize;
            if names.len() <= id {
                names.resize(id + 1, None);
            }
            names[id] = Some(name);
            ids.insert(name, id as i32);
        }
        ItemRegistry { ids, names }
    })
}

pub fn item_id(item: &str) -> Option<i32> {
    items().ids.get(item).copied()
}

pub fn item_name(id: i32) -> Option<&'static str> {
    if id < 0 {
        return None;
    }
    items().names.get(id as usize).copied().flatten()
}

struct EntityTypeRegistry {
    ids: HashMap<&'static str, i32>,
    names: Vec<Option<&'static str>>,
}

static ENTITY_TYPES: OnceLock<EntityTypeRegistry> = OnceLock::new();

fn entity_types() -> &'static EntityTypeRegistry {
    ENTITY_TYPES.get_or_init(|| {
        let mut ids = HashMap::new();
        let mut names: Vec<Option<&'static str>> = Vec::new();
        for line in ENTITY_TYPE_IDS.lines() {
            let Some((id, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            if id < 0 {
                continue;
            }
            let id = id as usize;
            if names.len() <= id {
                names.resize(id + 1, None);
            }
            names[id] = Some(name);
            ids.insert(name, id as i32);
        }
        EntityTypeRegistry { ids, names }
    })
}

pub fn entity_type_id(name: &str) -> Option<i32> {
    entity_types().ids.get(name).copied()
}

pub fn entity_type_name(id: i32) -> Option<&'static str> {
    if id < 0 {
        return None;
    }
    entity_types().names.get(id as usize).copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.to_owned(),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn block_registry_is_complete_and_uses_defaults() {
        assert_eq!(block_state_count(), 29_671);
        assert_eq!(blocks().defaults.len(), 1_166);
        assert_eq!(block_state_id(&state("minecraft:air", &[])), 0);
        assert_eq!(block_state_id(&state("minecraft:stone", &[])), 1);
        assert_eq!(block_state_id(&state("minecraft:bedrock", &[])), 85);
        assert_eq!(
            block_state_id(&state("minecraft:grass_block", &[("snowy", "false")])),
            9
        );
        assert_eq!(
            block_state_id(&state("minecraft:grass_block", &[("snowy", "true")])),
            8
        );
        assert_eq!(
            block_state_id(&state("minecraft:water", &[("level", "0")])),
            86
        );
        assert_eq!(block_state_id(&state("minecraft:water", &[])), 86);
        assert_eq!(block_state_id(&state("minecraft:lava", &[])), 102);
        assert_eq!(
            block_state_id(&state("minecraft:deepslate", &[])),
            block_default_id("minecraft:deepslate").unwrap()
        );
    }

    #[test]
    fn default_block_state_has_default_properties() {
        let grass = default_block_state("minecraft:grass_block").unwrap();
        assert_eq!(grass.name, "minecraft:grass_block");
        assert_eq!(grass.properties, vec![("snowy".into(), "false".into())]);
        let log = default_block_state("minecraft:oak_log").unwrap();
        assert_eq!(log.properties, vec![("axis".into(), "y".into())]);
        assert!(default_block_state("minecraft:not_a_block").is_none());
    }

    #[test]
    fn item_and_entity_registries_are_complete() {
        assert_eq!(item_id("minecraft:stone"), Some(1));
        assert_eq!(item_id("minecraft:dirt"), Some(28));
        assert_eq!(item_id("minecraft:grass_block"), Some(27));
        assert_eq!(item_id("minecraft:diamond_sword"), Some(936));
        assert_eq!(item_name(936), Some("minecraft:diamond_sword"));
        assert!(item_id("minecraft:not_an_item").is_none());
        assert_eq!(entity_type_id("minecraft:armor_stand"), Some(5));
        assert_eq!(entity_type_id("minecraft:zombie"), Some(150));
        assert_eq!(entity_type_name(150), Some("minecraft:zombie"));
    }
}
