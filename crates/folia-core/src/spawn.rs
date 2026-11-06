//! Mob spawning rules and spawn-cycle foundation.
//!
//! This is the first vertical slice of `NaturalSpawner`: category caps,
//! spawnable-block/light/height checks, weighted biome mob lists, a 3x3x3 pack
//! placement helper, and a deterministic per-chunk spawn cycle.
//!
//! It deliberately does not add entities to the shared server world. Callers
//! provide a [`WorldView`], player positions, and a [`RandomSource`]; the
//! result is a list of spawns the region tick loop can turn into entities.
//!
//! Biome spawn lists are embedded from the 1.21.11 vanilla data pack. Entity
//! type ids are resolved through [`crate::registry`], so the slice always uses
//! the real protocol ids.

use crate::chunk::BlockState;
use crate::entity_ai::{self, WorldView};
use crate::math::{BlockPos, Vec3};
use crate::random::RandomSource;
use crate::registry;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Vanilla `NaturalSpawner.MAGIC_NUMBER` (`17^2`).
pub const MAGIC_NUMBER: i32 = 289;

/// Vanilla `NaturalSpawner.MIN_SPAWN_DISTANCE`.
pub const MIN_SPAWN_DISTANCE: f64 = 24.0;

/// Vanilla `NaturalSpawner.SPAWN_DISTANCE_BLOCK`.
pub const SPAWN_DISTANCE_BLOCK: f64 = 128.0;

/// Chunk column coordinates used by the spawn cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub const fn min_block_x(self) -> i32 {
        self.x * 16
    }

    pub const fn min_block_z(self) -> i32 {
        self.z * 16
    }
}

/// `net.minecraft.world.entity.MobCategory`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MobCategory {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}

impl MobCategory {
    pub const ALL: [Self; 8] = [
        Self::Monster,
        Self::Creature,
        Self::Ambient,
        Self::Axolotls,
        Self::UndergroundWaterCreature,
        Self::WaterCreature,
        Self::WaterAmbient,
        Self::Misc,
    ];

    /// Categories iterated by `NaturalSpawner.spawnForChunk` (`MISC` excluded).
    pub const SPAWNING: [Self; 7] = [
        Self::Monster,
        Self::Creature,
        Self::Ambient,
        Self::Axolotls,
        Self::UndergroundWaterCreature,
        Self::WaterCreature,
        Self::WaterAmbient,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Monster => 0,
            Self::Creature => 1,
            Self::Ambient => 2,
            Self::Axolotls => 3,
            Self::UndergroundWaterCreature => 4,
            Self::WaterCreature => 5,
            Self::WaterAmbient => 6,
            Self::Misc => 7,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Monster => "monster",
            Self::Creature => "creature",
            Self::Ambient => "ambient",
            Self::Axolotls => "axolotls",
            Self::UndergroundWaterCreature => "underground_water_creature",
            Self::WaterCreature => "water_creature",
            Self::WaterAmbient => "water_ambient",
            Self::Misc => "misc",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.name() == name)
    }

    /// Vanilla `getMaxInstancesPerChunk`.
    pub const fn max_instances_per_chunk(self) -> i32 {
        match self {
            Self::Monster => 70,
            Self::Creature => 10,
            Self::Ambient => 15,
            Self::Axolotls => 5,
            Self::UndergroundWaterCreature | Self::WaterCreature => 5,
            Self::WaterAmbient => 20,
            Self::Misc => -1,
        }
    }

    pub const fn is_friendly(self) -> bool {
        !matches!(self, Self::Monster)
    }

    pub const fn is_persistent(self) -> bool {
        matches!(self, Self::Creature | Self::Misc)
    }

    pub const fn despawn_distance(self) -> f64 {
        match self {
            Self::WaterAmbient => 64.0,
            _ => SPAWN_DISTANCE_BLOCK,
        }
    }
}

/// Spawn-placement rule family used by the slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnRule {
    /// `Animal::checkAnimalSpawnRules`: grass below and raw light > 8.
    Animal,
    /// `Monster::checkMonsterSpawnRules`: dark enough for the overworld.
    Monster,
    /// `Monster::checkAnyLightMonsterSpawnRules`.
    AnyLightMonster,
    /// `Monster::checkSurfaceMonstersSpawnRules` (dark and sees sky).
    SurfaceMonster,
    /// `SpawnPlacementTypes.IN_WATER`.
    Water,
    /// `SpawnPlacementTypes.IN_LAVA`.
    Lava,
    /// Generic `Mob::checkMobSpawnRules` placement.
    Mob,
}

/// Static entity-type metadata needed by spawning and AI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityType {
    pub name: &'static str,
    pub category: MobCategory,
    pub width: f32,
    pub height: f32,
    pub rule: SpawnRule,
    pub can_spawn_far_from_player: bool,
}

impl EntityType {
    /// Real protocol entity-type id from `entity_type_ids.tsv`.
    pub fn id(self) -> i32 {
        registry::entity_type_id(self.name).unwrap_or(-1)
    }
}

const fn e(
    name: &'static str,
    category: MobCategory,
    width: f32,
    height: f32,
    rule: SpawnRule,
    can_spawn_far_from_player: bool,
) -> EntityType {
    EntityType {
        name,
        category,
        width,
        height,
        rule,
        can_spawn_far_from_player,
    }
}

/// Every entity type that appears in a vanilla biome spawn list.
static ENTITY_TYPES: &[EntityType] = &[
    e(
        "minecraft:armadillo",
        MobCategory::Creature,
        0.7,
        0.65,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:axolotl",
        MobCategory::Axolotls,
        0.75,
        0.42,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:bat",
        MobCategory::Ambient,
        0.5,
        0.9,
        SpawnRule::AnyLightMonster,
        false,
    ),
    e(
        "minecraft:bogged",
        MobCategory::Monster,
        0.6,
        1.99,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:camel",
        MobCategory::Creature,
        1.7,
        2.375,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:chicken",
        MobCategory::Creature,
        0.4,
        0.7,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:cod",
        MobCategory::WaterAmbient,
        0.5,
        0.3,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:cow",
        MobCategory::Creature,
        0.9,
        1.4,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:creeper",
        MobCategory::Monster,
        0.6,
        1.7,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:dolphin",
        MobCategory::WaterCreature,
        0.9,
        0.6,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:donkey",
        MobCategory::Creature,
        1.3964844,
        1.5,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:drowned",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:enderman",
        MobCategory::Monster,
        0.6,
        2.9,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:fox",
        MobCategory::Creature,
        0.6,
        0.7,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:frog",
        MobCategory::Creature,
        0.5,
        0.5,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:ghast",
        MobCategory::Monster,
        4.0,
        4.0,
        SpawnRule::AnyLightMonster,
        false,
    ),
    e(
        "minecraft:glow_squid",
        MobCategory::UndergroundWaterCreature,
        0.8,
        0.8,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:goat",
        MobCategory::Creature,
        0.9,
        1.3,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:hoglin",
        MobCategory::Monster,
        1.3964844,
        1.4,
        SpawnRule::AnyLightMonster,
        false,
    ),
    e(
        "minecraft:horse",
        MobCategory::Creature,
        1.3964844,
        1.6,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:husk",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::SurfaceMonster,
        false,
    ),
    e(
        "minecraft:llama",
        MobCategory::Creature,
        0.9,
        1.87,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:magma_cube",
        MobCategory::Monster,
        0.52,
        0.52,
        SpawnRule::AnyLightMonster,
        false,
    ),
    e(
        "minecraft:mooshroom",
        MobCategory::Creature,
        0.9,
        1.4,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:nautilus",
        MobCategory::WaterCreature,
        0.875,
        0.95,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:ocelot",
        MobCategory::Creature,
        0.6,
        0.7,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:panda",
        MobCategory::Creature,
        1.3,
        1.25,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:parched",
        MobCategory::Monster,
        0.6,
        1.99,
        SpawnRule::SurfaceMonster,
        false,
    ),
    e(
        "minecraft:parrot",
        MobCategory::Creature,
        0.5,
        0.9,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:pig",
        MobCategory::Creature,
        0.9,
        0.9,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:piglin",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::AnyLightMonster,
        false,
    ),
    e(
        "minecraft:polar_bear",
        MobCategory::Creature,
        1.4,
        1.4,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:pufferfish",
        MobCategory::WaterAmbient,
        0.7,
        0.7,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:rabbit",
        MobCategory::Creature,
        0.4,
        0.5,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:salmon",
        MobCategory::WaterAmbient,
        0.7,
        0.4,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:sheep",
        MobCategory::Creature,
        0.9,
        1.3,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:skeleton",
        MobCategory::Monster,
        0.6,
        1.99,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:slime",
        MobCategory::Monster,
        0.52,
        0.52,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:spider",
        MobCategory::Monster,
        1.4,
        0.9,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:squid",
        MobCategory::WaterCreature,
        0.8,
        0.8,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:stray",
        MobCategory::Monster,
        0.6,
        1.99,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:strider",
        MobCategory::Creature,
        0.9,
        1.7,
        SpawnRule::Lava,
        true,
    ),
    e(
        "minecraft:tropical_fish",
        MobCategory::WaterAmbient,
        0.5,
        0.4,
        SpawnRule::Water,
        false,
    ),
    e(
        "minecraft:turtle",
        MobCategory::Creature,
        1.2,
        0.4,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:witch",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:wolf",
        MobCategory::Creature,
        0.6,
        0.85,
        SpawnRule::Animal,
        true,
    ),
    e(
        "minecraft:zombie",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:zombie_horse",
        MobCategory::Monster,
        1.3964844,
        1.6,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:zombie_villager",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::Monster,
        false,
    ),
    e(
        "minecraft:zombified_piglin",
        MobCategory::Monster,
        0.6,
        1.95,
        SpawnRule::Monster,
        false,
    ),
];

static ENTITY_INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();

fn entity_index() -> &'static HashMap<&'static str, usize> {
    ENTITY_INDEX.get_or_init(|| {
        ENTITY_TYPES
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.name, index))
            .collect()
    })
}

/// All spawn-list entity types in stable table order.
pub fn entity_types() -> &'static [EntityType] {
    ENTITY_TYPES
}

/// Looks up an entity type by namespaced or bare name.
pub fn entity_type(name: &str) -> Option<&'static EntityType> {
    let normalized = normalize(name);
    entity_index()
        .get(normalized.as_ref())
        .map(|index| &ENTITY_TYPES[*index])
}

/// Looks up a type by protocol id.
pub fn entity_type_by_id(id: i32) -> Option<&'static EntityType> {
    let name = registry::entity_type_name(id)?;
    entity_type(name)
}

/// One weighted biome spawn entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnerData {
    pub entity_type: &'static str,
    pub min_count: i32,
    pub max_count: i32,
    pub weight: i32,
}

/// The mob lists and creature probability for one biome.
#[derive(Clone, Debug)]
pub struct BiomeSpawns {
    pub name: &'static str,
    pub creature_spawn_probability: f32,
    spawners: [Vec<SpawnerData>; 8],
}

impl BiomeSpawns {
    pub fn spawners(&self, category: MobCategory) -> &[SpawnerData] {
        &self.spawners[category.index()]
    }

    pub fn is_empty(&self, category: MobCategory) -> bool {
        self.spawners(category).is_empty()
    }
}

/// Vanilla 1.21.11 biome spawn data, one line per biome:
///
/// `biome|creature_probability|category:type,min,max,weight;...`
const BIOME_SPAWN_DATA: &str = r#"minecraft:badlands|0.03|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:armadillo,1,2,6|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:bamboo_jungle|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:chicken,4,4,10;minecraft:parrot,1,2,40;minecraft:panda,1,2,80|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:ocelot,1,1,2|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:basalt_deltas|0.1|creature:minecraft:strider,1,2,60|monster:minecraft:ghast,1,1,40;minecraft:magma_cube,2,5,100
minecraft:beach|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:turtle,2,5,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:birch_forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:cherry_grove|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:pig,1,2,1;minecraft:rabbit,2,6,2;minecraft:sheep,2,4,2|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:cold_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,15;minecraft:salmon,1,5,15|water_creature:minecraft:squid,1,4,3;minecraft:nautilus,1,1,2
minecraft:crimson_forest|0.1|creature:minecraft:strider,1,2,60|monster:minecraft:zombified_piglin,2,4,1;minecraft:hoglin,3,4,9;minecraft:piglin,3,4,5
minecraft:dark_forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:deep_cold_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,15;minecraft:salmon,1,5,15|water_creature:minecraft:squid,1,4,3;minecraft:nautilus,1,1,2
minecraft:deep_dark|0.1|
minecraft:deep_frozen_ocean|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:polar_bear,1,2,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:salmon,1,5,15|water_creature:minecraft:squid,1,4,1;minecraft:nautilus,1,1,2
minecraft:deep_lukewarm_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,8;minecraft:pufferfish,1,3,5;minecraft:tropical_fish,8,8,25|water_creature:minecraft:squid,1,4,8;minecraft:dolphin,1,2,2;minecraft:nautilus,1,1,5
minecraft:deep_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,10|water_creature:minecraft:squid,1,4,1;minecraft:dolphin,1,2,1;minecraft:nautilus,1,1,5
minecraft:desert|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:rabbit,2,3,12;minecraft:camel,1,1,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,19;minecraft:zombie_villager,1,1,1;minecraft:skeleton,4,4,50;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:husk,4,4,80;minecraft:parched,4,4,50|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:dripstone_caves|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,4,4,95|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:end_barrens|0.1|monster:minecraft:enderman,4,4,10
minecraft:end_highlands|0.1|monster:minecraft:enderman,4,4,10
minecraft:end_midlands|0.1|monster:minecraft:enderman,4,4,10
minecraft:eroded_badlands|0.03|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:armadillo,1,2,6|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:flower_forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:rabbit,2,3,4|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:wolf,4,4,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:frozen_ocean|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:polar_bear,1,2,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:salmon,1,5,15|water_creature:minecraft:squid,1,4,1;minecraft:nautilus,1,1,2
minecraft:frozen_peaks|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:goat,1,3,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:frozen_river|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,1|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:salmon,1,5,5|water_creature:minecraft:squid,1,4,2
minecraft:grove|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:wolf,1,1,1;minecraft:rabbit,2,3,8;minecraft:fox,2,4,4|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:ice_spikes|0.07|ambient:minecraft:bat,8,8,10|creature:minecraft:rabbit,2,3,10;minecraft:polar_bear,1,2,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,20;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:stray,4,4,80|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:jagged_peaks|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:goat,1,3,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:jungle|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:chicken,4,4,10;minecraft:parrot,1,2,40;minecraft:panda,1,2,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:ocelot,1,3,2|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:lukewarm_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,15;minecraft:pufferfish,1,3,5;minecraft:tropical_fish,8,8,25|water_creature:minecraft:squid,1,2,10;minecraft:dolphin,1,2,2;minecraft:nautilus,1,1,5
minecraft:lush_caves|0.1|ambient:minecraft:bat,8,8,10|axolotls:minecraft:axolotl,4,6,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:tropical_fish,8,8,25
minecraft:mangrove_swamp|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:frog,2,5,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,70;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:slime,1,1,1;minecraft:bogged,4,4,30|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:tropical_fish,8,8,25
minecraft:meadow|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:donkey,1,2,1;minecraft:rabbit,2,6,2;minecraft:sheep,2,4,2|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:mushroom_fields|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:mooshroom,4,8,8|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:nether_wastes|0.1|creature:minecraft:strider,1,2,60|monster:minecraft:ghast,4,4,50;minecraft:zombified_piglin,4,4,100;minecraft:magma_cube,4,4,2;minecraft:enderman,4,4,1;minecraft:piglin,4,4,15
minecraft:ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:cod,3,6,10|water_creature:minecraft:squid,1,4,1;minecraft:dolphin,1,2,1;minecraft:nautilus,1,1,5
minecraft:old_growth_birch_forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:old_growth_pine_taiga|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:wolf,4,4,8;minecraft:rabbit,2,3,4;minecraft:fox,2,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,100;minecraft:zombie_villager,1,1,25;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:old_growth_spruce_taiga|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:wolf,4,4,8;minecraft:rabbit,2,3,4;minecraft:fox,2,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:pale_garden|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:plains|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:horse,2,6,5;minecraft:donkey,1,3,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:river|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:drowned,1,1,100|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:salmon,1,5,5|water_creature:minecraft:squid,1,4,2
minecraft:savanna|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:horse,2,6,1;minecraft:donkey,1,1,1;minecraft:armadillo,2,3,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:savanna_plateau|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:horse,2,6,1;minecraft:donkey,1,1,1;minecraft:armadillo,2,3,10;minecraft:llama,4,4,8;minecraft:wolf,4,8,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:small_end_islands|0.1|monster:minecraft:enderman,4,4,10
minecraft:snowy_beach|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:snowy_plains|0.07|ambient:minecraft:bat,8,8,10|creature:minecraft:rabbit,2,3,10;minecraft:polar_bear,1,2,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,20;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:stray,4,4,80|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:snowy_slopes|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:rabbit,2,3,4;minecraft:goat,1,3,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:snowy_taiga|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:wolf,4,4,8;minecraft:rabbit,2,3,4;minecraft:fox,2,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:soul_sand_valley|0.1|creature:minecraft:strider,1,2,60|monster:minecraft:skeleton,5,5,20;minecraft:ghast,4,4,50;minecraft:enderman,4,4,1
minecraft:sparse_jungle|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:chicken,4,4,10;minecraft:wolf,2,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:stony_peaks|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:stony_shore|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:sunflower_plains|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:horse,2,6,5;minecraft:donkey,1,3,1|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:swamp|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:frog,2,5,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,70;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5;minecraft:slime,1,1,1;minecraft:bogged,4,4,30|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:taiga|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:wolf,4,4,8;minecraft:rabbit,2,3,4;minecraft:fox,2,4,8|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:the_end|0.1|monster:minecraft:enderman,4,4,10
minecraft:the_void|0.1|
minecraft:warm_ocean|0.1|ambient:minecraft:bat,8,8,10|monster:minecraft:drowned,1,1,5;minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10|water_ambient:minecraft:pufferfish,1,3,15;minecraft:tropical_fish,8,8,25|water_creature:minecraft:nautilus,1,1,5;minecraft:squid,4,4,10;minecraft:dolphin,1,2,2
minecraft:warped_forest|0.1|creature:minecraft:strider,1,2,60|monster:minecraft:enderman,4,4,1
minecraft:windswept_forest|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:llama,4,6,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:windswept_gravelly_hills|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:llama,4,6,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:windswept_hills|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:llama,4,6,5|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:windswept_savanna|0.1|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:horse,2,6,1;minecraft:donkey,1,1,1;minecraft:armadillo,2,3,10|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,90;minecraft:zombie_villager,1,1,5;minecraft:zombie_horse,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10
minecraft:wooded_badlands|0.04|ambient:minecraft:bat,8,8,10|creature:minecraft:sheep,4,4,12;minecraft:pig,4,4,10;minecraft:chicken,4,4,10;minecraft:cow,4,4,8;minecraft:armadillo,1,2,6;minecraft:wolf,4,8,2|monster:minecraft:spider,4,4,100;minecraft:zombie,4,4,95;minecraft:zombie_villager,1,1,5;minecraft:skeleton,4,4,100;minecraft:creeper,4,4,100;minecraft:slime,4,4,100;minecraft:enderman,1,4,10;minecraft:witch,1,1,5|underground_water_creature:minecraft:glow_squid,4,6,10"#;

static BIOMES: OnceLock<HashMap<&'static str, BiomeSpawns>> = OnceLock::new();

fn biomes() -> &'static HashMap<&'static str, BiomeSpawns> {
    BIOMES.get_or_init(|| {
        let mut map = HashMap::new();
        for line in BIOME_SPAWN_DATA.lines() {
            let mut sections = line.split('|');
            let (Some(name), Some(probability)) = (sections.next(), sections.next()) else {
                continue;
            };
            let Ok(creature_spawn_probability) = probability.parse::<f32>() else {
                continue;
            };
            let mut spawners: [Vec<SpawnerData>; 8] = std::array::from_fn(|_| Vec::new());
            for section in sections {
                let Some((category_name, entries)) = section.split_once(':') else {
                    continue;
                };
                let Some(category) = MobCategory::from_name(category_name) else {
                    continue;
                };
                for entry in entries.split(';') {
                    let mut fields = entry.split(',');
                    let (Some(entity_type), Some(min_count), Some(max_count), Some(weight)) =
                        (fields.next(), fields.next(), fields.next(), fields.next())
                    else {
                        continue;
                    };
                    let (Ok(min_count), Ok(max_count), Ok(weight)) = (
                        min_count.parse::<i32>(),
                        max_count.parse::<i32>(),
                        weight.parse::<i32>(),
                    ) else {
                        continue;
                    };
                    spawners[category.index()].push(SpawnerData {
                        entity_type,
                        min_count,
                        max_count,
                        weight,
                    });
                }
            }
            map.insert(
                name,
                BiomeSpawns {
                    name,
                    creature_spawn_probability,
                    spawners,
                },
            );
        }
        map
    })
}

/// Vanilla biome spawn settings by namespaced or bare biome id.
pub fn biome_spawns(name: &str) -> Option<&'static BiomeSpawns> {
    let normalized = normalize(name);
    biomes().get(normalized.as_ref())
}

/// Number of embedded biome spawn tables.
pub fn biome_spawn_count() -> usize {
    biomes().len()
}

fn normalize(name: &str) -> Cow<'_, str> {
    if name.contains(':') {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(format!("minecraft:{name}"))
    }
}

fn prevents_spawning_inside(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:rail"
            | "minecraft:powered_rail"
            | "minecraft:detector_rail"
            | "minecraft:activator_rail"
            | "minecraft:lever"
            | "minecraft:stone_button"
            | "minecraft:polished_blackstone_button"
            | "minecraft:oak_button"
            | "minecraft:spruce_button"
            | "minecraft:birch_button"
            | "minecraft:jungle_button"
            | "minecraft:acacia_button"
            | "minecraft:dark_oak_button"
            | "minecraft:mangrove_button"
            | "minecraft:cherry_button"
            | "minecraft:bamboo_button"
            | "minecraft:crimson_button"
            | "minecraft:warped_button"
            | "minecraft:pale_oak_button"
            | "minecraft:light_weighted_pressure_plate"
            | "minecraft:heavy_weighted_pressure_plate"
            | "minecraft:stone_pressure_plate"
            | "minecraft:oak_pressure_plate"
            | "minecraft:spruce_pressure_plate"
            | "minecraft:birch_pressure_plate"
            | "minecraft:jungle_pressure_plate"
            | "minecraft:acacia_pressure_plate"
            | "minecraft:dark_oak_pressure_plate"
            | "minecraft:mangrove_pressure_plate"
            | "minecraft:cherry_pressure_plate"
            | "minecraft:bamboo_pressure_plate"
            | "minecraft:crimson_pressure_plate"
            | "minecraft:warped_pressure_plate"
            | "minecraft:pale_oak_pressure_plate"
    )
}

fn light_at(world: &dyn WorldView, pos: BlockPos) -> Option<(u8, u8)> {
    Some((world.sky_light(pos)?, world.block_light(pos)?))
}

fn raw_light(world: &dyn WorldView, pos: BlockPos) -> Option<u8> {
    let (sky, block) = light_at(world, pos)?;
    Some(sky.max(block))
}

/// Vanilla `Monster.isDarkEnoughToSpawn` for the overworld dimension.
fn is_dark_enough(world: &dyn WorldView, pos: BlockPos, random: &mut dyn RandomSource) -> bool {
    let Some((sky, block)) = light_at(world, pos) else {
        return false;
    };
    if sky > random.next_int_bound(32) as u8 {
        return false;
    }
    // Overworld `monster_spawn_block_light_limit` is 0.
    if block > 0 {
        return false;
    }
    let threshold = random.next_int_between_inclusive(0, 7) as u8;
    sky.max(block) <= threshold
}

/// Checks the vanilla spawn-placement rules for one candidate position.
pub fn is_spawnable_position(
    world: &dyn WorldView,
    pos: BlockPos,
    entity_type: &EntityType,
    random: &mut dyn RandomSource,
) -> bool {
    if pos.y() < world.min_y() + 1 || pos.y() >= world.max_y() {
        return false;
    }
    let Some(surface) = world.surface_y(pos.x(), pos.z()) else {
        return false;
    };
    if pos.y() > surface {
        return false;
    }

    let (Some(feet), Some(head), Some(below)) = (
        world.block(pos),
        world.block(pos.above()),
        world.block(pos.below()),
    ) else {
        return false;
    };

    match entity_type.rule {
        SpawnRule::Water => {
            return entity_ai::is_water(feet) && entity_ai::is_water(head);
        }
        SpawnRule::Lava => {
            return entity_ai::is_lava(feet);
        }
        _ => {}
    }

    if entity_ai::is_liquid(feet)
        || entity_ai::is_liquid(head)
        || entity_ai::is_liquid(below)
        || prevents_spawning_inside(feet)
    {
        return false;
    }
    if crate::block_behaviour::has_collision(feet)
        || crate::block_behaviour::has_collision(head)
        || !crate::block_behaviour::has_collision(below)
    {
        return false;
    }

    match entity_type.rule {
        SpawnRule::Animal => {
            below.name == "minecraft:grass_block"
                && raw_light(world, pos).is_some_and(|light| light > 8)
        }
        SpawnRule::Monster => is_dark_enough(world, pos, random),
        SpawnRule::AnyLightMonster | SpawnRule::Mob => true,
        SpawnRule::SurfaceMonster => {
            is_dark_enough(world, pos, random) && world.sky_light(pos) == Some(15)
        }
        SpawnRule::Water | SpawnRule::Lava => false,
    }
}

/// One mob produced by a spawn attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnedMob {
    pub entity_type: &'static EntityType,
    pub pos: BlockPos,
}

impl SpawnedMob {
    pub fn entity_type_id(self) -> i32 {
        self.entity_type.id()
    }
}

/// Why a spawn attempt produced no mobs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnFailure {
    Capacity,
    NoBiomeSpawners,
    NoValidPosition,
    Distance,
    LightOrHeight,
    WorldEdge,
}

/// Result of one category spawn attempt.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkSpawnResult {
    pub category: MobCategory,
    pub spawned: Vec<SpawnedMob>,
    pub failure: Option<SpawnFailure>,
}

impl ChunkSpawnResult {
    fn empty(category: MobCategory, failure: SpawnFailure) -> Self {
        Self {
            category,
            spawned: Vec::new(),
            failure: Some(failure),
        }
    }
}

/// Vanilla global category counts for one spawn cycle.
#[derive(Clone, Debug, Default)]
pub struct SpawnState {
    spawnable_chunk_count: i32,
    counts: [i32; 8],
}

impl SpawnState {
    pub fn new(spawnable_chunk_count: i32) -> Self {
        Self {
            spawnable_chunk_count: spawnable_chunk_count.max(0),
            counts: [0; 8],
        }
    }

    pub fn spawnable_chunk_count(&self) -> i32 {
        self.spawnable_chunk_count
    }

    pub fn count(&self, category: MobCategory) -> i32 {
        self.counts[category.index()]
    }

    pub fn set_count(&mut self, category: MobCategory, count: i32) {
        self.counts[category.index()] = count.max(0);
    }

    pub fn add_mob(&mut self, category: MobCategory) {
        self.counts[category.index()] = self.counts[category.index()].saturating_add(1);
    }

    /// Vanilla `SpawnState.canSpawnForCategoryGlobal`.
    pub fn can_spawn_for_category_global(&self, category: MobCategory, limit: i32) -> bool {
        if limit <= 0 || self.spawnable_chunk_count <= 0 {
            return false;
        }
        let cap =
            i64::from(limit) * i64::from(self.spawnable_chunk_count) / i64::from(MAGIC_NUMBER);
        i64::from(self.count(category)) < cap
    }
}

/// Spawn-cycle parameters, including the distance-from-player gates.
#[derive(Clone, Debug)]
pub struct SpawnSettings {
    pub limits: [i32; 8],
    pub min_player_distance: f64,
    pub max_spawns_per_chunk: i32,
}

impl Default for SpawnSettings {
    fn default() -> Self {
        Self {
            limits: MobCategory::ALL.map(MobCategory::max_instances_per_chunk),
            min_player_distance: MIN_SPAWN_DISTANCE,
            max_spawns_per_chunk: 4,
        }
    }
}

impl SpawnSettings {
    pub fn limit(&self, category: MobCategory) -> i32 {
        self.limits[category.index()]
    }

    pub fn set_limit(&mut self, category: MobCategory, limit: i32) {
        self.limits[category.index()] = limit;
    }
}

/// Deterministic 3x3x3 pack positions around `center`, in y/z/x order.
pub fn pack_positions(center: BlockPos) -> Vec<BlockPos> {
    let mut positions = Vec::with_capacity(27);
    for dy in -1..=1 {
        for dz in -1..=1 {
            for dx in -1..=1 {
                positions.push(center.offset(dx, dy, dz));
            }
        }
    }
    positions
}

fn nearest_player_distance_sq(players: &[Vec3], pos: BlockPos) -> Option<f64> {
    let center = Vec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    );
    players
        .iter()
        .map(|player| player.distance_to_sqr(center))
        .min_by(f64::total_cmp)
}

fn weighted_pick<'a>(
    spawners: &'a [SpawnerData],
    random: &mut dyn RandomSource,
) -> Option<&'a SpawnerData> {
    let total_weight: i32 = spawners.iter().map(|spawner| spawner.weight.max(0)).sum();
    if total_weight <= 0 {
        return None;
    }
    let mut pick = random.next_int_bound(total_weight);
    for spawner in spawners {
        pick -= spawner.weight.max(0);
        if pick < 0 {
            return Some(spawner);
        }
    }
    None
}

/// Runs one category's `spawnCategoryForPosition` attempt at `center`.
pub fn spawn_category_for_position(
    state: &mut SpawnState,
    world: &dyn WorldView,
    center: BlockPos,
    category: MobCategory,
    settings: &SpawnSettings,
    players: &[Vec3],
    random: &mut dyn RandomSource,
) -> ChunkSpawnResult {
    if category == MobCategory::Misc
        || !state.can_spawn_for_category_global(category, settings.limit(category))
    {
        return ChunkSpawnResult::empty(category, SpawnFailure::Capacity);
    }
    if players.is_empty() {
        return ChunkSpawnResult::empty(category, SpawnFailure::Distance);
    }

    let biome_name = world.biome(center).unwrap_or("minecraft:plains");
    let Some(biome) = biome_spawns(biome_name) else {
        return ChunkSpawnResult::empty(category, SpawnFailure::NoBiomeSpawners);
    };
    let spawners = biome.spawners(category);
    if spawners.is_empty() {
        return ChunkSpawnResult::empty(category, SpawnFailure::NoBiomeSpawners);
    }
    let Some(spawner) = weighted_pick(spawners, random).copied() else {
        return ChunkSpawnResult::empty(category, SpawnFailure::NoBiomeSpawners);
    };
    let Some(entity_type) = entity_type(spawner.entity_type) else {
        return ChunkSpawnResult::empty(category, SpawnFailure::NoBiomeSpawners);
    };

    let pack_size = spawner.min_count
        + random.next_int_bound(1 + (spawner.max_count - spawner.min_count).max(0));
    let pack_size = pack_size.clamp(1, 27);
    let mut spawned = Vec::new();
    let mut saw_position = false;
    let mut saw_distance = false;

    for pos in pack_positions(center) {
        if spawned.len() as i32 >= pack_size
            || spawned.len() as i32 >= settings.max_spawns_per_chunk
        {
            break;
        }
        if pos.y() < world.min_y() + 1 || pos.y() >= world.max_y() {
            continue;
        }
        let Some(surface) = world.surface_y(pos.x(), pos.z()) else {
            continue;
        };
        if pos.y() > surface {
            continue;
        }
        saw_position = true;

        let Some(distance_sq) = nearest_player_distance_sq(players, pos) else {
            continue;
        };
        if distance_sq <= settings.min_player_distance * settings.min_player_distance {
            saw_distance = true;
            continue;
        }
        let despawn = entity_type.category.despawn_distance();
        if !entity_type.can_spawn_far_from_player && distance_sq > despawn * despawn {
            saw_distance = true;
            continue;
        }

        if !is_spawnable_position(world, pos, entity_type, random) {
            continue;
        }

        spawned.push(SpawnedMob { entity_type, pos });
        state.add_mob(category);
    }

    if spawned.is_empty() {
        let failure = if saw_distance {
            SpawnFailure::Distance
        } else if saw_position {
            SpawnFailure::LightOrHeight
        } else {
            SpawnFailure::WorldEdge
        };
        return ChunkSpawnResult::empty(category, failure);
    }

    ChunkSpawnResult {
        category,
        spawned,
        failure: None,
    }
}

/// `NaturalSpawner.spawnCategoryForChunk`: picks a random position in the chunk
/// and runs one category attempt there.
pub fn spawn_for_chunk(
    state: &mut SpawnState,
    world: &dyn WorldView,
    chunk: ChunkPos,
    category: MobCategory,
    settings: &SpawnSettings,
    players: &[Vec3],
    random: &mut dyn RandomSource,
) -> ChunkSpawnResult {
    if !state.can_spawn_for_category_global(category, settings.limit(category)) {
        return ChunkSpawnResult::empty(category, SpawnFailure::Capacity);
    }

    let x = chunk.min_block_x() + random.next_int_bound(16);
    let z = chunk.min_block_z() + random.next_int_bound(16);
    let Some(surface) = world.surface_y(x, z) else {
        return ChunkSpawnResult::empty(category, SpawnFailure::WorldEdge);
    };
    let min_y = world.min_y();
    let max_y = surface.min(world.max_y() - 1);
    if max_y <= min_y {
        return ChunkSpawnResult::empty(category, SpawnFailure::WorldEdge);
    }
    let y = random.next_int_between_inclusive(min_y, max_y);
    if y < min_y + 1 {
        return ChunkSpawnResult::empty(category, SpawnFailure::WorldEdge);
    }

    spawn_category_for_position(
        state,
        world,
        BlockPos::new(x, y, z),
        category,
        settings,
        players,
        random,
    )
}

/// Stateful per-chunk spawn cycle.
///
/// Each [`Self::tick_chunk`] advances the category cursor and attempts the
/// first spawnable category. [`Self::run_cycle`] attempts every spawning
/// category once, matching the vanilla `spawnForChunk` loop shape.
pub struct NaturalSpawner {
    pub state: SpawnState,
    pub settings: SpawnSettings,
    next_category: usize,
}

impl NaturalSpawner {
    pub fn new(spawnable_chunk_count: i32, settings: SpawnSettings) -> Self {
        Self {
            state: SpawnState::new(spawnable_chunk_count),
            settings,
            next_category: 0,
        }
    }

    /// Attempts one category for one chunk, rotating the cursor on success.
    pub fn tick_chunk(
        &mut self,
        world: &dyn WorldView,
        chunk: ChunkPos,
        players: &[Vec3],
        random: &mut dyn RandomSource,
    ) -> ChunkSpawnResult {
        for offset in 0..MobCategory::SPAWNING.len() {
            let index = (self.next_category + offset) % MobCategory::SPAWNING.len();
            let category = MobCategory::SPAWNING[index];
            let biome = world
                .biome(BlockPos::new(chunk.min_block_x(), 64, chunk.min_block_z()))
                .unwrap_or("minecraft:plains");
            if biome_spawns(biome).is_none_or(|spawns| spawns.is_empty(category)) {
                continue;
            }
            if !self
                .state
                .can_spawn_for_category_global(category, self.settings.limit(category))
            {
                continue;
            }
            self.next_category = (index + 1) % MobCategory::SPAWNING.len();
            return spawn_for_chunk(
                &mut self.state,
                world,
                chunk,
                category,
                &self.settings,
                players,
                random,
            );
        }
        ChunkSpawnResult::empty(MobCategory::Monster, SpawnFailure::NoBiomeSpawners)
    }

    /// Runs every spawning category once for this chunk.
    pub fn run_cycle(
        &mut self,
        world: &dyn WorldView,
        chunk: ChunkPos,
        players: &[Vec3],
        random: &mut dyn RandomSource,
    ) -> Vec<ChunkSpawnResult> {
        let mut results = Vec::with_capacity(MobCategory::SPAWNING.len());
        for category in MobCategory::SPAWNING {
            results.push(spawn_for_chunk(
                &mut self.state,
                world,
                chunk,
                category,
                &self.settings,
                players,
                random,
            ));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_positions_cover_the_full_3x3x3() {
        let center = BlockPos::new(5, 64, 5);
        let positions = pack_positions(center);
        assert_eq!(positions.len(), 27);
        assert!(positions.contains(&center));
        assert!(positions.contains(&BlockPos::new(4, 63, 4)));
        assert!(positions.contains(&BlockPos::new(6, 65, 6)));
        assert_eq!(positions[0], BlockPos::new(4, 63, 4));
        assert_eq!(positions[26], BlockPos::new(6, 65, 6));
    }

    #[test]
    fn category_caps_match_vanilla_mobcategory() {
        assert_eq!(MobCategory::Monster.max_instances_per_chunk(), 70);
        assert_eq!(MobCategory::Creature.max_instances_per_chunk(), 10);
        assert_eq!(MobCategory::WaterAmbient.max_instances_per_chunk(), 20);
        assert_eq!(MobCategory::WaterAmbient.despawn_distance(), 64.0);
        assert_eq!(MobCategory::Monster.despawn_distance(), 128.0);
        assert!(MobCategory::Creature.is_persistent());
        assert!(!MobCategory::Monster.is_persistent());
    }
}
