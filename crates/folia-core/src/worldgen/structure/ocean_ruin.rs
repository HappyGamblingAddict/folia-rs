//! Procedural cold and warm ocean ruins.
//!
//! The piece uses a fixed 21x17 footprint with a stepped entrance arch, broken
//! perimeter walls, two corner towers, interior wall stubs, and rubble. Cold
//! ruins favor stone bricks, cobblestone, cracked/mossy stone, and andesite with
//! sparse prismarine; warm ruins favor prismarine bricks, prismarine, and dark
//! prismarine with andesite/stone accents. Materials are selected by a seeded
//! coordinate hash, so chunk clipping and placement order cannot change them.
//!
//! This is a deterministic procedural approximation rather than Java template
//! parity. Rotation, chest count, and chest loot seeds share the vanilla
//! large-feature seed stream. Vertical placement averages `OCEAN_FLOOR_WG` over
//! the footprint and adds a capped four-block foundation skirt through
//! replaceable blocks. Exact vanilla template choice, size/weight variation,
//! height jitter, waterlogging processors, and Java block fixtures remain open.

use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::BoundingBox;
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::{Mirror, PlacedBlock, Rotation, transform_state};
use folia_math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource};

pub const GENERATION_HEIGHT: i32 = 90;
pub const WIDTH: i32 = 21;
pub const DEPTH: i32 = 17;
pub const HEIGHT: i32 = 12;
pub const FOUNDATION_DEPTH: i32 = 4;

const COLD_MATERIALS: [&str; 6] = [
    "minecraft:stone_bricks",
    "minecraft:cobblestone",
    "minecraft:cracked_stone_bricks",
    "minecraft:andesite",
    "minecraft:prismarine",
    "minecraft:mossy_stone_bricks",
];

const WARM_MATERIALS: [&str; 6] = [
    "minecraft:prismarine_bricks",
    "minecraft:prismarine",
    "minecraft:dark_prismarine",
    "minecraft:andesite",
    "minecraft:stone_bricks",
    "minecraft:cobblestone",
];

const CHEST_POSITIONS: [(i32, i32); 2] = [(5, 5), (10, 10)];

#[derive(Debug, Clone, PartialEq)]
pub struct OceanRuinPiece {
    seed: i64,
    bounding_box: BoundingBox,
    rotation: Rotation,
    warm: bool,
    chest_count: i32,
    chest_seeds: [i64; 2],
}

impl OceanRuinPiece {
    pub fn new(seed: i64, x: i32, z: i32, rotation: Rotation, warm: bool) -> Self {
        let mut random = LegacyRandomSource::new(seed);
        Self::from_random(seed, x, z, rotation, warm, &mut random)
    }

    pub(crate) fn from_random(
        seed: i64,
        x: i32,
        z: i32,
        rotation: Rotation,
        warm: bool,
        random: &mut impl RandomSource,
    ) -> Self {
        let chest_count = 1 + random.next_int_bound(2);
        let chest_seeds = [random.next_long(), random.next_long()];
        Self {
            seed: piece_seed(seed, x, z),
            bounding_box: bounding_box(x, z, rotation),
            rotation,
            warm,
            chest_count,
            chest_seeds,
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    pub fn warm(&self) -> bool {
        self.warm
    }

    pub fn chest_count(&self) -> i32 {
        self.chest_count
    }

    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        self.place_with_optional_height_source(level, None, chunk_box)
    }

    pub fn place_with_height_source(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: &dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        self.place_with_optional_height_source(level, Some(height_source), chunk_box)
    }

    fn place_with_optional_height_source(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: Option<&dyn FeatureLevel>,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box.intersects_xz(
            chunk_box.min_x(),
            chunk_box.min_z(),
            chunk_box.max_x(),
            chunk_box.max_z(),
        ) {
            return Ok(StartPlacement::default());
        }
        let source = height_source.unwrap_or(&*level);
        let Some(base_y) = average_ocean_floor(source, self.bounding_box) else {
            return Ok(StartPlacement::default());
        };
        let actual_bounds = self.bounding_box.moved(0, base_y - GENERATION_HEIGHT, 0);
        if !actual_bounds.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let mut writer = RuinWriter {
            level,
            chunk_box,
            min_x: self.bounding_box.min_x(),
            min_z: self.bounding_box.min_z(),
            base_y,
            rotation: self.rotation,
            warm: self.warm,
            seed: self.seed,
            chest_count: self.chest_count,
            chest_seeds: self.chest_seeds,
            result: StartPlacement::default(),
        };
        writer.generate();
        let mut result = writer.result;
        result.placed_pieces = 1;
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }
}

pub struct OceanRuinStart;

impl OceanRuinStart {
    pub const WIDTH: i32 = WIDTH;
    pub const DEPTH: i32 = DEPTH;
    pub const HEIGHT: i32 = HEIGHT;
    pub const GENERATION_HEIGHT: i32 = GENERATION_HEIGHT;
    pub const FOUNDATION_DEPTH: i32 = FOUNDATION_DEPTH;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
        warm: bool,
    ) -> Result<StructureStart, StructureError> {
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let piece = OceanRuinPiece::from_random(
            context.seed,
            context.chunk_pos.x << 4,
            context.chunk_pos.z << 4,
            rotation,
            warm,
            &mut random,
        );
        Ok(StructureStart::new_ocean_ruin(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

fn bounding_box(x: i32, z: i32, rotation: Rotation) -> BoundingBox {
    let (width, depth) = rotated_size(rotation);
    BoundingBox::new(
        x,
        GENERATION_HEIGHT - FOUNDATION_DEPTH,
        z,
        x + width - 1,
        GENERATION_HEIGHT + HEIGHT - 1,
        z + depth - 1,
    )
}

fn rotated_size(rotation: Rotation) -> (i32, i32) {
    if rotation.swaps_horizontal_axes() {
        (DEPTH, WIDTH)
    } else {
        (WIDTH, DEPTH)
    }
}

fn average_ocean_floor(source: &dyn FeatureLevel, bounds: BoundingBox) -> Option<i32> {
    let mut total = 0_i64;
    let mut count = 0_i64;
    for z in bounds.min_z()..=bounds.max_z() {
        for x in bounds.min_x()..=bounds.max_x() {
            total += i64::from(source.heightmap(Heightmap::OceanFloorWg, x, z)?);
            count += 1;
        }
    }
    (count > 0).then(|| (total / count) as i32 - 1)
}

struct RuinWriter<'a> {
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    min_x: i32,
    min_z: i32,
    base_y: i32,
    rotation: Rotation,
    warm: bool,
    seed: i64,
    chest_count: i32,
    chest_seeds: [i64; 2],
    result: StartPlacement,
}

impl RuinWriter<'_> {
    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let (world_x, world_z) = match self.rotation {
            Rotation::None => (x, z),
            Rotation::Clockwise90 => (DEPTH - 1 - z, x),
            Rotation::Clockwise180 => (WIDTH - 1 - x, DEPTH - 1 - z),
            Rotation::CounterClockwise90 => (z, WIDTH - 1 - x),
        };
        BlockPos::new(self.min_x + world_x, self.base_y + y, self.min_z + world_z)
    }

    fn place_state(
        &mut self,
        state: BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<(BlockPos, BlockState)> {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return None;
        }
        let state = transform_state(&state, Mirror::None, self.rotation);
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            Some((pos, state))
        } else {
            self.result.rejected += 1;
            None
        }
    }

    fn place(&mut self, state: BlockState, x: i32, y: i32, z: i32) -> bool {
        self.place_state(state, x, y, z).is_some()
    }

    fn material(&self, x: i32, y: i32, z: i32, role: u64) -> BlockState {
        let palette = if self.warm {
            &WARM_MATERIALS
        } else {
            &COLD_MATERIALS
        };
        let index = coordinate_hash(self.seed, x, y, z, role) % palette.len() as u64;
        state(palette[index as usize])
    }

    fn broken(&self, x: i32, y: i32, z: i32, height: i32) -> bool {
        let hash = coordinate_hash(self.seed, x, y, z, 0x517e_00d5);
        (y == height && hash.is_multiple_of(4)) || (y > 1 && hash.is_multiple_of(23))
    }

    fn generate(&mut self) {
        self.generate_foundation();
        self.generate_perimeter_walls();
        self.generate_arch();
        self.generate_tower(0, 0);
        self.generate_tower(WIDTH - 5, DEPTH - 5);
        self.generate_interior_walls();
        self.generate_rubble();
        self.generate_chests();
    }

    fn generate_foundation(&mut self) {
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let material = self.material(x, 0, z, 0x00f0_0f0f);
                self.place(material.clone(), x, 0, z);
                if x == 0 || x == WIDTH - 1 || z == 0 || z == DEPTH - 1 {
                    for depth in 1..=FOUNDATION_DEPTH {
                        let pos = self.world_pos(x, -depth, z);
                        if pos.x() < self.chunk_box.min_x()
                            || pos.x() > self.chunk_box.max_x()
                            || pos.z() < self.chunk_box.min_z()
                            || pos.z() > self.chunk_box.max_z()
                        {
                            continue;
                        }
                        if self
                            .level
                            .get_block(pos)
                            .is_some_and(|block| !is_replaceable_by_structures(block))
                        {
                            break;
                        }
                        self.place(material.clone(), x, -depth, z);
                    }
                }
            }
        }
    }

    fn generate_perimeter_walls(&mut self) {
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                if x != 0 && x != WIDTH - 1 && z != 0 && z != DEPTH - 1 {
                    continue;
                }
                let hash = coordinate_hash(self.seed, x, 0, z, 0xfa11_0a11);
                let height = 3 + (hash % 3) as i32;
                for y in 1..=height {
                    if self.doorway(x, y, z) || self.broken(x, y, z, height) {
                        continue;
                    }
                    self.place(self.material(x, y, z, 0x10a1_10a1), x, y, z);
                }
            }
        }
    }

    fn doorway(&self, x: i32, y: i32, z: i32) -> bool {
        let front = z == DEPTH - 1 && (8..=12).contains(&x) && y <= 4;
        let back = z == 0 && (8..=12).contains(&x) && y <= 3;
        let side = (x == 0 || x == WIDTH - 1) && (6..=10).contains(&z) && y <= 3;
        front || back || side
    }

    fn generate_arch(&mut self) {
        let z = DEPTH - 1;
        for x in 8..=12 {
            self.place(self.material(x, 6, z, 0xa2c4), x, 6, z);
        }
        for x in [9, 11] {
            self.place(self.material(x, 7, z, 0xa2c4), x, 7, z);
        }
        self.place(self.material(10, 8, z, 0xa2c4), 10, 8, z);
    }

    fn generate_tower(&mut self, min_x: i32, min_z: i32) {
        let hash = coordinate_hash(self.seed, min_x, 0, min_z, 0x70e3);
        let height = 8 + (hash % 3) as i32;
        for z in 0..5 {
            for x in 0..5 {
                let world_x = min_x + x;
                let world_z = min_z + z;
                let boundary = x == 0 || x == 4 || z == 0 || z == 4;
                if !boundary {
                    continue;
                }
                for y in 1..=height {
                    if self.broken(world_x, y, world_z, height) {
                        continue;
                    }
                    self.place(
                        self.material(world_x, y, world_z, 0x70a3),
                        world_x,
                        y,
                        world_z,
                    );
                }
                if (x == 0 || x == 4) && (z == 0 || z == 4) {
                    self.place(
                        self.material(world_x, height + 1, world_z, 0x70a3),
                        world_x,
                        height + 1,
                        world_z,
                    );
                }
            }
        }
    }

    fn generate_interior_walls(&mut self) {
        for z in 2..=7 {
            for y in 1..=3 {
                if (z == 4 || z == 5) && y <= 2 {
                    continue;
                }
                self.place(self.material(7, y, z, 0x1a7e), 7, y, z);
            }
        }
        for z in 9..=14 {
            for y in 1..=3 {
                if z == 11 && y <= 2 {
                    continue;
                }
                self.place(self.material(13, y, z, 0x1a7e), 13, y, z);
            }
        }
        for x in 7..=13 {
            for y in 1..=2 {
                if x == 10 {
                    continue;
                }
                self.place(self.material(x, y, 8, 0x1a7e), x, y, 8);
            }
        }
    }

    fn generate_rubble(&mut self) {
        for z in 2..DEPTH - 2 {
            for x in 2..WIDTH - 2 {
                if coordinate_hash(self.seed, x, 1, z, 0x8bb1e).is_multiple_of(41) {
                    self.place(self.material(x, 1, z, 0x8bb1e), x, 1, z);
                }
            }
        }
    }

    fn generate_chests(&mut self) {
        for (index, (x, z)) in CHEST_POSITIONS.into_iter().enumerate() {
            if index as i32 >= self.chest_count {
                break;
            }
            let pos = self.world_pos(x, 1, z);
            let waterlogged = self.level.get_block(pos).is_some_and(|block| {
                block.name == "minecraft:water"
                    || block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "waterlogged" && value == "true")
            });
            let chest = BlockState {
                name: "minecraft:chest".to_owned(),
                properties: vec![
                    ("facing".to_owned(), "north".to_owned()),
                    ("type".to_owned(), "single".to_owned()),
                    ("waterlogged".to_owned(), waterlogged.to_string()),
                ],
            };
            let Some((pos, chest)) = self.place_state(chest, x, 1, z) else {
                continue;
            };
            self.result.block_entities.push(PlacedBlock {
                pos,
                state: chest,
                nbt: Some(Tag::Compound(vec![
                    ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                    (
                        "LootTable".to_owned(),
                        Tag::String("minecraft:chests/ocean_ruin".to_owned()),
                    ),
                    (
                        "LootTableSeed".to_owned(),
                        Tag::Long(self.chest_seeds[index]),
                    ),
                ])),
            });
        }
    }
}

fn piece_seed(seed: i64, x: i32, z: i32) -> i64 {
    seed ^ i64::from(x).wrapping_mul(0x9e37_79b1_85eb_ca87_u64.cast_signed())
        ^ i64::from(z).wrapping_mul(0xc2b2_ae3d_27d4_eb4f_u64.cast_signed())
}

fn coordinate_hash(seed: i64, x: i32, y: i32, z: i32, role: u64) -> u64 {
    let mut value = (seed as u64).wrapping_add(role)
        ^ (x as u64).wrapping_mul(0x9e37_79b1_85eb_ca87)
        ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (z as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn is_replaceable_by_structures(block: &BlockState) -> bool {
    matches!(
        block.name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:flowing_water"
            | "minecraft:lava"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    ) || crate::tags::block_has("minecraft:replaceable", &block.name)
}
