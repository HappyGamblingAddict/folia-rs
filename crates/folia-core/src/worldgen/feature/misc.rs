//! Dungeon, well, chest, and freeze feature ports.
//!
//! Ports `BonusChestFeature`, `DesertWellFeature`, `MonsterRoomFeature`, and
//! `SnowAndFreezeFeature` from the decompiled 1.21.11 tree. The first three
//! attach their vanilla block-entity payloads (loot-table chest, brushable
//! sand, mob spawner) through [`FeatureLevel::set_block_entity`]; the freeze
//! feature reads the chunk biome through [`FeatureLevel::biome_at`].

use super::placement::Heightmap;
use super::support::{state, state_with};
use super::{FeatureLevel, is_air_name};
use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::tags;
use crate::worldgen::surface::height_adjusted_temperature;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;

fn horizontal() -> [Direction; 4] {
    [
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ]
}

/// `Util.toShuffledList(IntStream.rangeClosed(min, max), random)`.
fn shuffled_range(min: i32, max: i32, random: &mut dyn RandomSource) -> Vec<i32> {
    let mut values: Vec<i32> = (min..=max).collect();
    for index in (1..values.len()).rev() {
        let pick = random.next_int_bound((index + 1) as i32) as usize;
        values.swap(index, pick);
    }
    values
}

fn empty(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level
        .get_block(pos)
        .is_none_or(|state| is_air_name(&state.name))
}

fn has_empty_collision(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    level.get_block(pos).is_none_or(|state| {
        is_air_name(&state.name) || crate::block_shapes::collision_boxes_for_state(state).is_empty()
    })
}

fn loot_nbt(id: &str, pos: BlockPos, table: &str, seed: i64) -> Tag {
    let mut fields = vec![
        ("id".to_owned(), Tag::String(id.to_owned())),
        ("x".to_owned(), Tag::Int(pos.x())),
        ("y".to_owned(), Tag::Int(pos.y())),
        ("z".to_owned(), Tag::Int(pos.z())),
        ("LootTable".to_owned(), Tag::String(table.to_owned())),
    ];
    if seed != 0 {
        fields.push(("LootTableSeed".to_owned(), Tag::Long(seed)));
    }
    Tag::Compound(fields)
}

fn chest_state(facing: Direction) -> BlockState {
    let facing = match facing {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down | Direction::Up => "north",
    };
    state_with(
        "minecraft:chest",
        &[
            ("facing", facing),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    )
}

/// `StructurePiece.reorient(level, pos, chest)`.
fn reorient_chest(level: &dyn FeatureLevel, pos: BlockPos) -> BlockState {
    let default = chest_state(Direction::North);
    let mut found = None;
    for direction in horizontal() {
        let neighbor = pos.relative(direction);
        if let Some(state) = level.get_block(neighbor) {
            if state.name == "minecraft:chest" {
                return default;
            }
            if crate::block_behaviour::is_solid(state) {
                if found.is_some() {
                    found = None;
                    break;
                }
                found = Some(direction);
            }
        }
    }
    if let Some(direction) = found {
        return chest_state(direction.opposite());
    }

    let mut facing = Direction::North;
    if level
        .get_block(pos.relative(facing))
        .is_some_and(crate::block_behaviour::is_solid)
    {
        facing = facing.opposite();
    }
    for _ in 0..3 {
        if !level
            .get_block(pos.relative(facing))
            .is_some_and(crate::block_behaviour::is_solid)
        {
            break;
        }
        facing = clockwise(facing);
    }
    chest_state(facing)
}

fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        Direction::Down | Direction::Up => direction,
    }
}

/// `minecraft:bonus_chest`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BonusChestFeature;

impl BonusChestFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let chunk_x = origin.x().div_euclid(16);
        let chunk_z = origin.z().div_euclid(16);
        let xs = shuffled_range(chunk_x * 16, chunk_x * 16 + 15, random);
        let zs = shuffled_range(chunk_z * 16, chunk_z * 16 + 15, random);
        let torch = state("minecraft:torch");
        for x in xs {
            for z in &zs {
                let Some(height) = level.heightmap(Heightmap::MotionBlockingNoLeaves, x, *z) else {
                    continue;
                };
                let pos = BlockPos::new(x, height, *z);
                if !empty(level, pos) && !has_empty_collision(level, pos) {
                    continue;
                }
                level.set_block(pos, chest_state(Direction::North));
                level.set_block_entity(
                    pos,
                    loot_nbt(
                        "minecraft:chest",
                        pos,
                        "minecraft:spawn_bonus_chest",
                        random.next_long(),
                    ),
                );
                for direction in horizontal() {
                    let torch_pos = pos.relative(direction);
                    if level
                        .get_block(torch_pos.below())
                        .is_some_and(crate::block_behaviour::is_solid)
                    {
                        level.set_block(torch_pos, torch.clone());
                    }
                }
                return true;
            }
        }
        false
    }
}

/// `minecraft:desert_well`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DesertWellFeature;

impl DesertWellFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let mut pos = origin.above();
        while empty(level, pos) && pos.y() > level.min_y() + 2 {
            pos = pos.below();
        }
        if level.get_block(pos).map(|state| state.name.as_str()) != Some("minecraft:sand") {
            return false;
        }

        for x in -2..=2 {
            for z in -2..=2 {
                if empty(level, pos.offset(x, -1, z)) && empty(level, pos.offset(x, -2, z)) {
                    return false;
                }
            }
        }

        let sandstone = state("minecraft:sandstone");
        let sand = state("minecraft:sand");
        let water = state("minecraft:water");
        let slab = state_with(
            "minecraft:sandstone_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );

        for y in -2..=0 {
            for x in -2..=2 {
                for z in -2..=2 {
                    level.set_block(pos.offset(x, y, z), sandstone.clone());
                }
            }
        }
        level.set_block(pos, water.clone());
        for direction in horizontal() {
            level.set_block(pos.relative(direction), water.clone());
        }
        let below = pos.below();
        level.set_block(below, sand.clone());
        for direction in horizontal() {
            level.set_block(below.relative(direction), sand.clone());
        }

        for x in -2..=2 {
            for z in -2..=2 {
                if x == -2 || x == 2 || z == -2 || z == 2 {
                    level.set_block(pos.offset(x, 1, z), sandstone.clone());
                }
            }
        }
        for (x, z) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
            level.set_block(pos.offset(x, 1, z), slab.clone());
        }

        for x in -1..=1 {
            for z in -1..=1 {
                if x == 0 && z == 0 {
                    level.set_block(pos.offset(x, 4, z), sandstone.clone());
                } else {
                    level.set_block(pos.offset(x, 4, z), slab.clone());
                }
            }
        }

        for y in 1..=3 {
            for (x, z) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                level.set_block(pos.offset(x, y, z), sandstone.clone());
            }
        }

        let candidates = [pos, pos.east(), pos.south(), pos.west(), pos.north()];
        for depth in 1..=2 {
            let pick = random.next_int_bound(candidates.len() as i32) as usize;
            let target = candidates[pick].below_n(depth);
            level.set_block(target, state("minecraft:suspicious_sand"));
            level.set_block_entity(
                target,
                loot_nbt(
                    "minecraft:brushable_block",
                    target,
                    "minecraft:desert_well_archaeology",
                    target.as_long(),
                ),
            );
        }
        true
    }
}

/// `minecraft:monster_room`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonsterRoomFeature;

impl MonsterRoomFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let radius_x = random.next_int_bound(2) + 2;
        let radius_z = random.next_int_bound(2) + 2;
        let min_x = -radius_x - 1;
        let max_x = radius_x + 1;
        let min_z = -radius_z - 1;
        let max_z = radius_z + 1;
        let air = state("minecraft:cave_air");

        let mut openings = 0;
        for x in min_x..=max_x {
            for y in -1..=4 {
                for z in min_z..=max_z {
                    let pos = origin.offset(x, y, z);
                    let solid = level
                        .get_block(pos)
                        .is_some_and(crate::block_behaviour::is_solid);
                    if y == -1 && !solid {
                        return false;
                    }
                    if y == 4 && !solid {
                        return false;
                    }
                    if (x == min_x || x == max_x || z == min_z || z == max_z)
                        && y == 0
                        && empty(level, pos)
                        && empty(level, pos.above())
                    {
                        openings += 1;
                    }
                }
            }
        }
        if !(1..=5).contains(&openings) {
            return false;
        }

        let replaceable = |state: &BlockState| {
            !crate::tags::block_has("minecraft:features_cannot_replace", &state.name)
        };
        let cobblestone = state("minecraft:cobblestone");
        let mossy = state("minecraft:mossy_cobblestone");
        let spawner = state("minecraft:spawner");

        for x in min_x..=max_x {
            for y in (-1..=3).rev() {
                for z in min_z..=max_z {
                    let pos = origin.offset(x, y, z);
                    let current = level
                        .get_block(pos)
                        .cloned()
                        .unwrap_or_else(|| state("minecraft:air"));
                    let boundary =
                        x == min_x || y == -1 || z == min_z || x == max_x || y == 4 || z == max_z;
                    if boundary {
                        if pos.y() >= level.min_y()
                            && !level
                                .get_block(pos.below())
                                .is_some_and(crate::block_behaviour::is_solid)
                        {
                            level.set_block(pos, air.clone());
                        } else if crate::block_behaviour::is_solid(&current)
                            && current.name != "minecraft:chest"
                            && replaceable(&current)
                        {
                            if y == -1 && random.next_int_bound(4) != 0 {
                                level.set_block(pos, mossy.clone());
                            } else {
                                level.set_block(pos, cobblestone.clone());
                            }
                        }
                    } else if current.name != "minecraft:chest"
                        && current.name != "minecraft:spawner"
                        && replaceable(&current)
                    {
                        level.set_block(pos, air.clone());
                    }
                }
            }
        }

        let width_x = radius_x * 2 + 1;
        let width_z = radius_z * 2 + 1;
        for _ in 0..2 {
            for _ in 0..3 {
                let x = origin.x() + random.next_int_bound(width_x) - radius_x;
                let z = origin.z() + random.next_int_bound(width_z) - radius_z;
                let pos = BlockPos::new(x, origin.y(), z);
                if !empty(level, pos) {
                    continue;
                }
                let solid_sides = horizontal()
                    .into_iter()
                    .filter(|direction| {
                        level
                            .get_block(pos.relative(*direction))
                            .is_some_and(crate::block_behaviour::is_solid)
                    })
                    .count();
                if solid_sides == 1 {
                    level.set_block(pos, reorient_chest(level, pos));
                    level.set_block_entity(
                        pos,
                        loot_nbt(
                            "minecraft:chest",
                            pos,
                            "minecraft:simple_dungeon",
                            random.next_long(),
                        ),
                    );
                    break;
                }
            }
        }

        level.set_block(origin, spawner);
        let mobs = [
            "minecraft:skeleton",
            "minecraft:zombie",
            "minecraft:zombie",
            "minecraft:spider",
        ];
        let mob = mobs[random.next_int_bound(mobs.len() as i32) as usize];
        level.set_block_entity(origin, spawner_nbt(origin, mob));
        true
    }
}

fn spawner_nbt(pos: BlockPos, mob: &str) -> Tag {
    Tag::Compound(vec![
        (
            "id".to_owned(),
            Tag::String("minecraft:mob_spawner".to_owned()),
        ),
        ("x".to_owned(), Tag::Int(pos.x())),
        ("y".to_owned(), Tag::Int(pos.y())),
        ("z".to_owned(), Tag::Int(pos.z())),
        ("Delay".to_owned(), Tag::Short(20)),
        ("MinSpawnDelay".to_owned(), Tag::Short(200)),
        ("MaxSpawnDelay".to_owned(), Tag::Short(800)),
        ("SpawnCount".to_owned(), Tag::Short(4)),
        ("MaxNearbyEntities".to_owned(), Tag::Short(6)),
        ("RequiredPlayerRange".to_owned(), Tag::Short(16)),
        ("SpawnRange".to_owned(), Tag::Short(4)),
        (
            "SpawnData".to_owned(),
            Tag::Compound(vec![(
                "entity".to_owned(),
                Tag::Compound(vec![("id".to_owned(), Tag::String(mob.to_owned()))]),
            )]),
        ),
        ("SpawnPotentials".to_owned(), Tag::List(Vec::new())),
    ])
}

/// `SnowAndFreezeFeature` (`minecraft:freeze_top_layer`).
///
/// Snow and ice are decided from the generated chunk's biome palette. The
/// feature runs at generation time, when vanilla's light engine has no block
/// light above the surface, so `shouldFreeze`/`shouldSnow` see a block light
/// of 0 (the `< 10` gate is therefore always open).
#[derive(Debug, Clone, Copy, Default)]
pub struct FreezeTopLayerFeature;

impl FreezeTopLayerFeature {
    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        _random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> bool {
        let sea_level = level.sea_level();
        let ice = state("minecraft:ice");
        let snow = state_with("minecraft:snow", &[("layers", "1")]);

        for offset_x in 0..16 {
            for offset_z in 0..16 {
                let x = origin.x() + offset_x;
                let z = origin.z() + offset_z;
                let Some(height) = level.heightmap(Heightmap::MotionBlocking, x, z) else {
                    continue;
                };
                let pos = BlockPos::new(x, height, z);
                let below = pos.below();
                let Some(biome) = level.biome_at(pos).map(str::to_owned) else {
                    continue;
                };

                if should_freeze(level, &biome, below, sea_level) {
                    level.set_block(below, ice.clone());
                }

                if should_snow(level, &biome, pos, sea_level) {
                    level.set_block(pos, snow.clone());
                    if let Some(block) = level.get_block(below)
                        && block.properties.iter().any(|(key, _)| key == "snowy")
                    {
                        let mut snowy = block.clone();
                        if let Some((_, value)) =
                            snowy.properties.iter_mut().find(|(key, _)| key == "snowy")
                        {
                            *value = "true".to_owned();
                        }
                        level.set_block(below, snowy);
                    }
                }
            }
        }
        true
    }
}

fn should_freeze(level: &dyn FeatureLevel, biome: &str, pos: BlockPos, sea_level: i32) -> bool {
    let Some(temperature) =
        height_adjusted_temperature(biome, pos.x(), pos.y(), pos.z(), sea_level)
    else {
        return false;
    };
    if temperature >= 0.15 {
        return false;
    }
    if !level.get_block(pos).is_some_and(is_water_block) {
        return false;
    }
    // `mustBeAtEdge`: a water block surrounded on all four horizontal sides is
    // not frozen.
    let surrounded = level.get_block(pos.west()).is_some_and(is_water_block)
        && level.get_block(pos.east()).is_some_and(is_water_block)
        && level.get_block(pos.north()).is_some_and(is_water_block)
        && level.get_block(pos.south()).is_some_and(is_water_block);
    !surrounded
}

fn should_snow(level: &dyn FeatureLevel, biome: &str, pos: BlockPos, sea_level: i32) -> bool {
    if !has_precipitation(biome) {
        return false;
    }
    let Some(temperature) =
        height_adjusted_temperature(biome, pos.x(), pos.y(), pos.z(), sea_level)
    else {
        return false;
    };
    if temperature >= 0.15 {
        return false;
    }
    let Some(state) = level.get_block(pos) else {
        return false;
    };
    if !(is_air_name(&state.name) || state.name == "minecraft:snow") {
        return false;
    }
    snow_can_survive(level, pos)
}

fn snow_can_survive(level: &dyn FeatureLevel, pos: BlockPos) -> bool {
    let Some(below) = level.get_block(pos.below()) else {
        return false;
    };
    if tags::block_has("minecraft:snow_layer_cannot_survive_on", &below.name) {
        return false;
    }
    if tags::block_has("minecraft:snow_layer_can_survive_on", &below.name) {
        return true;
    }
    if below.name == "minecraft:snow"
        && below
            .properties
            .iter()
            .any(|(key, value)| key == "layers" && value == "8")
    {
        return true;
    }
    crate::block_shapes::collision_boxes_for_state(below)
        .iter()
        .any(|box_| {
            box_.min_x <= 0.0
                && box_.max_x >= 1.0
                && box_.min_z <= 0.0
                && box_.max_z >= 1.0
                && box_.max_y >= 1.0
        })
}

fn is_water_block(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:water" | "minecraft:flowing_water"
    ) || state
        .properties
        .iter()
        .any(|(key, value)| key == "waterlogged" && value == "true")
}

/// `Biome.ClimateSettings.hasPrecipitation` for the shipped 1.21.11 biomes.
///
/// Every shipped biome defaults to `true` except these explicit `false`
/// entries in the data pack; the asset extractor does not carry the flag yet.
fn has_precipitation(biome: &str) -> bool {
    !matches!(
        biome,
        "minecraft:badlands"
            | "minecraft:basalt_deltas"
            | "minecraft:crimson_forest"
            | "minecraft:desert"
            | "minecraft:end_barrens"
            | "minecraft:end_highlands"
            | "minecraft:end_midlands"
            | "minecraft:eroded_badlands"
            | "minecraft:nether_wastes"
            | "minecraft:savanna"
            | "minecraft:savanna_plateau"
            | "minecraft:small_end_islands"
            | "minecraft:soul_sand_valley"
            | "minecraft:the_end"
            | "minecraft:the_void"
            | "minecraft:warped_forest"
            | "minecraft:windswept_savanna"
            | "minecraft:wooded_badlands"
    )
}
