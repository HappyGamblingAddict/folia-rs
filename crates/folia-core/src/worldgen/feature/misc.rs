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
use crate::worldgen::structure::piece::{BoundingBox, template_bounding_box};
use crate::worldgen::structure::processor::{
    ProcessedBlock, ProcessorApplyContext, RawBlockInfo, StructureProcessor,
    StructureProcessorCatalog,
};
use crate::worldgen::structure::template::{
    Mirror, Rotation, StructurePlaceSettings, StructureTemplate, transform, transform_state,
};
use crate::worldgen::surface::height_adjusted_temperature;
use folia_math::{BlockPos, Direction};
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::io;

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

/// `minecraft:fossil`.
///
/// Ports `FossilFeature`: one random rotation, one weighted template index
/// shared by the base and overlay structures, the 15–24 block underground
/// depth draw, the empty-corner rejection, and the two processor passes. The
/// base and overlay templates are resolved through
/// [`FeatureLevel::template`]; a level without them fails explicitly instead of
/// pretending the fossil placed nothing.
#[derive(Debug, Clone)]
pub struct FossilFeature {
    fossil_structures: Vec<String>,
    overlay_structures: Vec<String>,
    fossil_processors: Vec<StructureProcessor>,
    overlay_processors: Vec<StructureProcessor>,
    max_empty_corners_allowed: i32,
}

impl FossilFeature {
    /// Shipped base fossil template ids in config order.
    pub fn fossil_structures(&self) -> &[String] {
        &self.fossil_structures
    }

    /// Shipped overlay fossil template ids in config order.
    pub fn overlay_structures(&self) -> &[String] {
        &self.overlay_structures
    }

    /// `max_empty_corners_allowed` from the configured feature.
    pub fn max_empty_corners_allowed(&self) -> i32 {
        self.max_empty_corners_allowed
    }

    pub(crate) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        random: &mut dyn RandomSource,
        origin: BlockPos,
    ) -> Result<bool, String> {
        let rotation = random_rotation(random);
        let index = random.next_int_bound(self.fossil_structures.len() as i32) as usize;
        let fossil_id = &self.fossil_structures[index];
        let overlay_id = &self.overlay_structures[index];
        // Resolve both templates before touching the world so a missing
        // provider entry cannot leave a half-placed fossil behind.
        let fossil = level
            .template(fossil_id)
            .cloned()
            .ok_or_else(|| missing_template(fossil_id))?;
        let overlay = level
            .template(overlay_id)
            .cloned()
            .ok_or_else(|| missing_template(overlay_id))?;

        let rotated = fossil.rotated_size(rotation);
        let footprint_x = rotated[0];
        let footprint_z = rotated[2];
        let base_x = origin.x() - footprint_x / 2;
        let base_z = origin.z() - footprint_z / 2;
        let mut surface = origin.y();
        for x in 0..footprint_x {
            for z in 0..footprint_z {
                if let Some(height) =
                    level.heightmap(Heightmap::OceanFloorWg, base_x + x, base_z + z)
                {
                    surface = surface.min(height);
                }
            }
        }
        let depth = random.next_int_bound(10);
        let y = (surface - 15 - depth).max(level.min_y() + 10);
        let target = BlockPos::new(base_x, y, base_z);
        let zero = zero_position_with_transform(&fossil, target, rotation);
        let template_box =
            template_bounding_box(&fossil, zero, rotation, BlockPos::ZERO, Mirror::None);
        if count_empty_corners(level, template_box) > self.max_empty_corners_allowed {
            return Ok(false);
        }

        let chunk_x = origin.x().div_euclid(16);
        let chunk_z = origin.z().div_euclid(16);
        let chunk_box = BoundingBox::new(
            chunk_x * 16 - 16,
            level.min_y(),
            chunk_z * 16 - 16,
            chunk_x * 16 + 31,
            level.min_y() + level.height() - 1,
            chunk_z * 16 + 31,
        );

        let settings = StructurePlaceSettings::new().with_rotation(rotation);
        let context = ProcessorApplyContext::new(level.seed(), zero, zero);

        let base_blocks = fossil_block_infos(&fossil, &settings, zero, random);
        let processed = apply_fossil_processors(
            &self.fossil_processors,
            level,
            &context,
            &base_blocks,
            random,
        );
        write_processed(level, &processed, rotation, chunk_box);

        // The overlay processors run against the world after the base write,
        // exactly like the second `placeInWorld` call in Java.
        let overlay_blocks = fossil_block_infos(&overlay, &settings, zero, random);
        let processed = apply_fossil_processors(
            &self.overlay_processors,
            level,
            &context,
            &overlay_blocks,
            random,
        );
        write_processed(level, &processed, rotation, chunk_box);
        Ok(true)
    }
}

/// Parses `FossilFeatureConfiguration`.
pub(crate) fn parse_fossil_config(config: &Value) -> io::Result<FossilFeature> {
    let object = config
        .as_object()
        .ok_or_else(|| invalid("fossil config must be an object"))?;
    let fossil_structures = parse_template_list(object, "fossil_structures")?;
    let overlay_structures = parse_template_list(object, "overlay_structures")?;
    if fossil_structures.is_empty() {
        return Err(invalid("fossil config needs at least one fossil structure"));
    }
    if fossil_structures.len() != overlay_structures.len() {
        return Err(invalid(format!(
            "fossil config has {} fossil structures but {} overlay structures",
            fossil_structures.len(),
            overlay_structures.len()
        )));
    }
    let fossil_processors = parse_processor_list(object, "fossil_processors")?;
    let overlay_processors = parse_processor_list(object, "overlay_processors")?;
    let max_empty_corners_allowed = object
        .get("max_empty_corners_allowed")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            invalid("fossil config is missing an integer `max_empty_corners_allowed`")
        })?;
    let max_empty_corners_allowed = i32::try_from(max_empty_corners_allowed)
        .map_err(|_| invalid("fossil `max_empty_corners_allowed` is outside the i32 range"))?;

    Ok(FossilFeature {
        fossil_structures,
        overlay_structures,
        fossil_processors,
        overlay_processors,
        max_empty_corners_allowed,
    })
}

fn parse_template_list(object: &Map<String, Value>, key: &str) -> io::Result<Vec<String>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("fossil config is missing the `{key}` list")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(normalize_id)
                .ok_or_else(|| invalid(format!("fossil `{key}` entries must be template ids")))
        })
        .collect()
}

fn parse_processor_list(
    object: &Map<String, Value>,
    key: &str,
) -> io::Result<Vec<StructureProcessor>> {
    let name = object.get(key).and_then(Value::as_str).ok_or_else(|| {
        invalid(format!(
            "fossil config is missing the `{key}` processor list"
        ))
    })?;
    let name = normalize_id(name);
    StructureProcessorCatalog::shipped()
        .list(&name)
        .map(|list| list.processors().to_vec())
        .ok_or_else(|| {
            invalid(format!(
                "fossil processor list `{name}` is not in the shipped processor catalog"
            ))
        })
}

fn normalize_id(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

fn missing_template(id: &str) -> String {
    format!(
        "requires structure template `{id}` via FeatureLevel::template, but the level has no \
         template provider entry"
    )
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// `Util.getRandom(Rotation.values(), random)`.
fn random_rotation(random: &mut dyn RandomSource) -> Rotation {
    match random.next_int_bound(4) {
        0 => Rotation::None,
        1 => Rotation::Clockwise90,
        2 => Rotation::Clockwise180,
        _ => Rotation::CounterClockwise90,
    }
}

/// `StructureTemplate.getZeroPositionWithTransform(pos, Mirror.NONE, rotation)`.
///
/// Java uses the template's *unrotated* x/z extents for this shift even when
/// the footprint used for the origin and height scan is rotated.
fn zero_position_with_transform(
    template: &StructureTemplate,
    pos: BlockPos,
    rotation: Rotation,
) -> BlockPos {
    let size_x = template.size_x() - 1;
    let size_z = template.size_z() - 1;
    match rotation {
        Rotation::CounterClockwise90 => pos.offset(0, 0, size_x),
        Rotation::Clockwise90 => pos.offset(size_z, 0, 0),
        Rotation::Clockwise180 => pos.offset(size_x, 0, size_z),
        Rotation::None => pos,
    }
}

/// `BoundingBox.forAllCorners` plus the air/lava/water predicate.
///
/// A missing block is outside this port's single-chunk write region, not an
/// observed air block; it is treated as non-empty so the port does not reject
/// fossils at chunk borders purely because the neighbour is unreadable.
fn count_empty_corners(level: &dyn FeatureLevel, bounds: BoundingBox) -> i32 {
    let xs = [bounds.min_x(), bounds.max_x()];
    let ys = [bounds.min_y(), bounds.max_y()];
    let zs = [bounds.min_z(), bounds.max_z()];
    let mut empty = 0;
    for x in xs {
        for y in ys {
            for z in zs {
                if let Some(state) = level.get_block(BlockPos::new(x, y, z))
                    && (is_air_name(&state.name) || is_lava_or_water(&state.name))
                {
                    empty += 1;
                }
            }
        }
    }
    empty
}

fn is_lava_or_water(name: &str) -> bool {
    matches!(
        name,
        "minecraft:water" | "minecraft:lava" | "minecraft:flowing_water" | "minecraft:flowing_lava"
    )
}

/// `StructurePlaceSettings.getRandomPalette(palettes, pos)` plus
/// `StructureTemplate.processBlockInfos` position math.
///
/// The palette draw happens once per `placeInWorld` call and must stay in the
/// feature RNG stream even when there is only one palette.
fn fossil_block_infos(
    template: &StructureTemplate,
    settings: &StructurePlaceSettings,
    origin: BlockPos,
    random: &mut dyn RandomSource,
) -> Vec<RawBlockInfo> {
    let palette_count = template.palette_count();
    // Java draws `nextInt(palettes.size())` even when there is only one
    // palette, which consumes the legacy RNG stream.
    let palette_index = random.next_int_bound(palette_count as i32) as usize;
    let Some(palette) = template.palette(palette_index) else {
        return Vec::new();
    };
    template
        .blocks()
        .iter()
        .filter_map(|block| {
            let state = palette.get(block.state)?.clone();
            if settings.ignore_air() && is_air_name(&state.name) {
                return None;
            }
            Some(RawBlockInfo {
                local_pos: block.pos,
                pos: origin.offset_by(transform(
                    block.pos,
                    settings.mirror(),
                    settings.rotation(),
                    settings.rotation_pivot(),
                )),
                state,
                nbt: block.nbt.clone(),
            })
        })
        .collect()
}

/// `StructureTemplate.processBlockInfos` with Java's feature RNG.
///
/// `block_rot` reads `StructurePlaceSettings.getRandom(pos)`, which is the
/// feature's random source for fossils. The shipped processor lists are still
/// parsed from [`StructureProcessorCatalog`]; only the random source differs
/// from the generic structure-pool pass.
fn apply_fossil_processors(
    processors: &[StructureProcessor],
    level: &dyn FeatureLevel,
    context: &ProcessorApplyContext,
    blocks: &[RawBlockInfo],
    random: &mut dyn RandomSource,
) -> Vec<ProcessedBlock> {
    let mut originals = Vec::new();
    let mut processed = Vec::new();
    for block in blocks {
        let mut current = Some(ProcessedBlock {
            pos: block.pos,
            state: block.state.clone(),
            nbt: block.nbt.clone(),
        });
        for processor in processors {
            let Some(value) = current.take() else {
                break;
            };
            current = match processor {
                StructureProcessor::BlockRot {
                    integrity,
                    rottable_blocks,
                } => {
                    let rottable = rottable_blocks
                        .as_ref()
                        .is_none_or(|set| set.matches(&block.state));
                    if rottable && random.next_float() > *integrity {
                        None
                    } else {
                        Some(value)
                    }
                }
                other => other.process_block(level, context, block, &value),
            };
        }
        if let Some(value) = current {
            originals.push(block.clone());
            processed.push(value);
        }
    }
    for processor in processors {
        processed = processor.finalize_processing(level, context, &originals, processed);
    }
    processed
}

/// Writes the processed template blocks with the placement rotation and the
/// chunk±16 bounding box that `FossilFeature` passes to `placeInWorld`.
///
/// `minecraft:structure_void` is a template marker, not a world block; the
/// fossil path drops it instead of leaking it into the chunk.
fn write_processed(
    level: &mut dyn FeatureLevel,
    processed: &[ProcessedBlock],
    rotation: Rotation,
    chunk_box: BoundingBox,
) {
    for block in processed {
        if !inside(block.pos, chunk_box) || block.state.name == "minecraft:structure_void" {
            continue;
        }
        let state = transform_state(&block.state, Mirror::None, rotation);
        if level.set_block(block.pos, state)
            && let Some(nbt) = &block.nbt
        {
            level.set_block_entity(block.pos, nbt.clone());
        }
    }
}

fn inside(pos: BlockPos, bounds: BoundingBox) -> bool {
    pos.x() >= bounds.min_x()
        && pos.x() <= bounds.max_x()
        && pos.y() >= bounds.min_y()
        && pos.y() <= bounds.max_y()
        && pos.z() >= bounds.min_z()
        && pos.z() <= bounds.max_z()
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
