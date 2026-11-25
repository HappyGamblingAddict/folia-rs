//! Procedural ocean monuments.
//!
//! The fixed 35x28x26 piece has a 19x19 prismarine core, two 8x11 side wings,
//! and a 7x6 entrance. Its five-block foundation sits on the averaged
//! `OCEAN_FLOOR_WG` height beneath the cross-shaped floor, the core walls rise
//! eleven blocks, an elliptical shell forms the roof from Y=12 through Y=21,
//! and four corner finials reach Y=25. The core contains an elder-guardian room
//! with a gold block and four sea lanterns; each wing contains a sponge room.
//!
//! These dimensions and room shapes are documented procedural approximations,
//! not Java template parity. Rotation uses the existing large-feature seed
//! stream, while a seeded coordinate hash selects prismarine and sponge
//! variants independently of chunk clipping and placement order. Placement
//! averages `OCEAN_FLOOR_WG` over the whole footprint, subtracts one block, and
//! extends support through at most five replaceable blocks. Guardian spawning,
//! loot, exact vanilla room sizes, processors, waterlogging of every template
//! block, and a Java block fixture remain open.

use crate::chunk::BlockState;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::BoundingBox;
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::{Mirror, Rotation, transform_state};
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};

pub const GENERATION_HEIGHT: i32 = 90;
pub const WIDTH: i32 = 35;
pub const DEPTH: i32 = 28;
pub const HEIGHT: i32 = 26;
pub const FOUNDATION_DEPTH: i32 = 5;

const CORE_MIN_X: i32 = 8;
const CORE_MAX_X: i32 = 26;
const CORE_MIN_Z: i32 = 3;
const CORE_MAX_Z: i32 = 21;
const CENTER_X: i32 = 17;
const CENTER_Z: i32 = 12;
const WING_MIN_Z: i32 = 7;
const WING_MAX_Z: i32 = 17;
const ENTRANCE_MIN_X: i32 = 14;
const ENTRANCE_MAX_X: i32 = 20;
const ENTRANCE_MIN_Z: i32 = 22;
const ENTRANCE_MAX_Z: i32 = 27;

const PRISMARINE_MATERIALS: [&str; 3] = [
    "minecraft:prismarine_bricks",
    "minecraft:prismarine",
    "minecraft:dark_prismarine",
];

#[derive(Debug, Clone, PartialEq)]
pub struct OceanMonumentPiece {
    seed: i64,
    bounding_box: BoundingBox,
    rotation: Rotation,
}

impl OceanMonumentPiece {
    pub fn new(seed: i64, x: i32, z: i32, rotation: Rotation) -> Self {
        Self {
            seed: piece_seed(seed, x, z),
            bounding_box: bounding_box(x, z, rotation),
            rotation,
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn rotation(&self) -> Rotation {
        self.rotation
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
        let mut writer = MonumentWriter {
            level,
            chunk_box,
            min_x: self.bounding_box.min_x(),
            min_z: self.bounding_box.min_z(),
            base_y,
            rotation: self.rotation,
            seed: self.seed,
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

pub struct OceanMonumentStart;

impl OceanMonumentStart {
    pub const WIDTH: i32 = WIDTH;
    pub const DEPTH: i32 = DEPTH;
    pub const HEIGHT: i32 = HEIGHT;
    pub const GENERATION_HEIGHT: i32 = GENERATION_HEIGHT;
    pub const FOUNDATION_DEPTH: i32 = FOUNDATION_DEPTH;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let piece = OceanMonumentPiece::new(
            context.seed,
            context.chunk_pos.x << 4,
            context.chunk_pos.z << 4,
            rotation,
        );
        Ok(StructureStart::new_ocean_monument(
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

struct MonumentWriter<'a> {
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    min_x: i32,
    min_z: i32,
    base_y: i32,
    rotation: Rotation,
    seed: i64,
    result: StartPlacement,
}

impl MonumentWriter<'_> {
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
        mut state: BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<(BlockPos, BlockState)> {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return None;
        }
        if let Some((_, waterlogged)) = state
            .properties
            .iter_mut()
            .find(|(key, _)| key == "waterlogged")
        {
            let submerged = self.level.get_block(pos).is_some_and(|block| {
                block.name == "minecraft:water"
                    || block.name == "minecraft:flowing_water"
                    || block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "waterlogged" && value == "true")
            });
            *waterlogged = submerged.to_string();
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

    fn fill_air(&mut self, min_x: i32, min_y: i32, min_z: i32, max_x: i32, max_y: i32, max_z: i32) {
        let air = state("minecraft:air");
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    self.place(air.clone(), x, y, z);
                }
            }
        }
    }

    fn material(&self, x: i32, y: i32, z: i32, role: u64) -> BlockState {
        let index = coordinate_hash(self.seed, x, y, z, role) % PRISMARINE_MATERIALS.len() as u64;
        state(PRISMARINE_MATERIALS[index as usize])
    }

    fn sponge(&self, x: i32, y: i32, z: i32, role: u64) -> BlockState {
        if coordinate_hash(self.seed, x, y, z, role).is_multiple_of(4) {
            state("minecraft:wet_sponge")
        } else {
            state("minecraft:sponge")
        }
    }

    fn generate(&mut self) {
        self.generate_foundation_and_floor();
        self.generate_core_shell();
        self.generate_wings();
        self.generate_entrance();
        self.carve_interiors();
        self.generate_guardian_room();
        self.generate_sponge_rooms();
        self.generate_dome();
        self.generate_finials();
        self.generate_openings_and_arches();
    }

    fn generate_foundation_and_floor(&mut self) {
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                if !in_footprint(x, z) {
                    continue;
                }
                let material = self.material(x, 0, z, 0x0c10_0001);
                self.place(material.clone(), x, 0, z);
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

    fn generate_core_shell(&mut self) {
        for y in 1..=11 {
            for z in CORE_MIN_Z..=CORE_MAX_Z {
                for x in CORE_MIN_X..=CORE_MAX_X {
                    if x != CORE_MIN_X && x != CORE_MAX_X && z != CORE_MIN_Z && z != CORE_MAX_Z {
                        continue;
                    }
                    self.place(self.material(x, y, z, 0x0c10_0010), x, y, z);
                }
            }
        }
    }

    fn generate_wings(&mut self) {
        for min_x in [0, 27] {
            for z in WING_MIN_Z..=WING_MAX_Z {
                for x in min_x..min_x + 8 {
                    let boundary =
                        x == min_x || x == min_x + 7 || z == WING_MIN_Z || z == WING_MAX_Z;
                    let material = self.material(x, 0, z, 0x0c10_0020);
                    self.place(material.clone(), x, 0, z);
                    for y in 1..=6 {
                        if boundary {
                            self.place(material.clone(), x, y, z);
                        }
                    }
                    self.place(material, x, 6, z);
                    if boundary {
                        self.place(self.material(x, 7, z, 0x0c10_0021), x, 7, z);
                    }
                }
            }
        }
    }

    fn generate_entrance(&mut self) {
        for z in ENTRANCE_MIN_Z..=ENTRANCE_MAX_Z {
            for x in ENTRANCE_MIN_X..=ENTRANCE_MAX_X {
                let material = self.material(x, 0, z, 0x0c10_0030);
                self.place(material.clone(), x, 0, z);
                if x == ENTRANCE_MIN_X || x == ENTRANCE_MAX_X || z == ENTRANCE_MAX_Z {
                    for y in 1..=5 {
                        self.place(material.clone(), x, y, z);
                    }
                }
                self.place(material, x, 6, z);
            }
        }
    }

    fn carve_interiors(&mut self) {
        self.fill_air(
            CORE_MIN_X + 1,
            1,
            CORE_MIN_Z + 1,
            CORE_MAX_X - 1,
            11,
            CORE_MAX_Z - 1,
        );
        self.fill_air(
            CENTER_X - 7,
            12,
            CENTER_Z - 7,
            CENTER_X + 7,
            20,
            CENTER_Z + 7,
        );
        for min_x in [0, 27] {
            self.fill_air(min_x + 1, 1, WING_MIN_Z + 1, min_x + 6, 5, WING_MAX_Z - 1);
        }
        self.fill_air(
            ENTRANCE_MIN_X + 1,
            1,
            ENTRANCE_MIN_Z,
            ENTRANCE_MAX_X - 1,
            5,
            ENTRANCE_MAX_Z - 1,
        );
    }

    fn generate_guardian_room(&mut self) {
        for y in 1..=6 {
            for z in 6..=18 {
                for x in 11..=23 {
                    if x != 11 && x != 23 && z != 6 && z != 18 {
                        continue;
                    }
                    self.place(state("minecraft:dark_prismarine"), x, y, z);
                }
            }
        }
        self.place(state("minecraft:gold_block"), CENTER_X, 0, CENTER_Z);
        for (x, z) in [
            (CENTER_X - 4, CENTER_Z),
            (CENTER_X + 4, CENTER_Z),
            (CENTER_X, CENTER_Z - 4),
            (CENTER_X, CENTER_Z + 4),
        ] {
            self.place(state("minecraft:sea_lantern"), x, 3, z);
        }
    }

    fn generate_sponge_rooms(&mut self) {
        for (room, min_x) in [0_u64, 1_u64].into_iter().zip([0, 27]) {
            for z in 9..=15 {
                for x in min_x + 1..=min_x + 6 {
                    self.place(self.sponge(x, 0, z, 0x0c10_0400 + room), x, 0, z);
                    if x == min_x + 1 || x == min_x + 6 || z == 9 || z == 15 {
                        for y in 1..=3 {
                            self.place(self.sponge(x, y, z, 0x0c10_0410 + room), x, y, z);
                        }
                    }
                }
            }
        }
        self.place(state("minecraft:sponge"), 2, 0, 10);
        self.place(state("minecraft:wet_sponge"), 29, 0, 10);
    }

    fn generate_dome(&mut self) {
        for z in CORE_MIN_Z..=CORE_MAX_Z {
            for x in CORE_MIN_X..=CORE_MAX_X {
                if x == CORE_MIN_X || x == CORE_MAX_X || z == CORE_MIN_Z || z == CORE_MAX_Z {
                    self.place(self.material(x, 12, z, 0x0c10_0500), x, 12, z);
                }
            }
        }
        for y in 12..=21 {
            let vertical = f64::from(y - 12) / 9.0;
            let horizontal = (1.0 - vertical * vertical).max(0.0).sqrt();
            let radius_x = (10.0 * horizontal).round() as i32;
            let radius_z = (10.0 * horizontal).round() as i32;
            if radius_x == 0 || radius_z == 0 {
                self.place(
                    self.material(CENTER_X, y, CENTER_Z, 0x0c10_0501),
                    CENTER_X,
                    y,
                    CENTER_Z,
                );
                continue;
            }
            for z in CENTER_Z - radius_z..=CENTER_Z + radius_z {
                for x in CENTER_X - radius_x..=CENTER_X + radius_x {
                    let dx = f64::from(x - CENTER_X) / f64::from(radius_x);
                    let dz = f64::from(z - CENTER_Z) / f64::from(radius_z);
                    let radius_squared = dx * dx + dz * dz;
                    if (0.55..=1.0).contains(&radius_squared) {
                        self.place(self.material(x, y, z, 0x0c10_0502), x, y, z);
                    }
                }
            }
        }
    }

    fn generate_finials(&mut self) {
        for (x, z) in [
            (CORE_MIN_X, CORE_MIN_Z),
            (CORE_MAX_X, CORE_MIN_Z),
            (CORE_MIN_X, CORE_MAX_Z),
            (CORE_MAX_X, CORE_MAX_Z),
        ] {
            for y in 12..=HEIGHT - 1 {
                self.place(self.material(x, y, z, 0x0c10_0600), x, y, z);
            }
        }
    }

    fn generate_openings_and_arches(&mut self) {
        for y in 1..=4 {
            self.fill_air(15, y, CORE_MAX_Z, 19, y, CORE_MAX_Z);
        }
        for y in 1..=3 {
            self.fill_air(15, y, CORE_MIN_Z, 19, y, CORE_MIN_Z);
            self.fill_air(CORE_MIN_X, y, 10, CORE_MIN_X, y, 14);
            self.fill_air(CORE_MAX_X, y, 10, CORE_MAX_X, y, 14);
            self.fill_air(7, y, 11, 7, y, 13);
            self.fill_air(27, y, 11, 27, y, 13);
            self.fill_air(15, y, ENTRANCE_MAX_Z, 19, y, ENTRANCE_MAX_Z);
            self.fill_air(11, y, 10, 11, y, 14);
            self.fill_air(23, y, 10, 23, y, 14);
            self.fill_air(16, y, 6, 18, y, 6);
            self.fill_air(16, y, 18, 18, y, 18);
        }
        for y in 2..=3 {
            self.fill_air(0, y, 10, 0, y, 11);
            self.fill_air(34, y, 10, 34, y, 11);
        }
        for y in 5..=7 {
            for z in [8, 16] {
                self.fill_air(CORE_MIN_X, y, z, CORE_MIN_X, y, z);
                self.fill_air(CORE_MAX_X, y, z, CORE_MAX_X, y, z);
            }
            for x in [12, 22] {
                self.fill_air(x, y, CORE_MIN_Z, x, y, CORE_MIN_Z);
                self.fill_air(x, y, CORE_MAX_Z, x, y, CORE_MAX_Z);
            }
        }

        let arch = stairs(Direction::North);
        for x in 15..=19 {
            self.place(arch.clone(), x, 4, ENTRANCE_MAX_Z);
        }
        for x in 16..=18 {
            self.place(arch.clone(), x, 5, ENTRANCE_MAX_Z);
        }
        for x in 15..=19 {
            self.place(arch.clone(), x, 5, CORE_MAX_Z);
        }
        for x in 16..=18 {
            self.place(arch.clone(), x, 6, CORE_MAX_Z);
        }
    }
}

fn in_footprint(x: i32, z: i32) -> bool {
    (CORE_MIN_X..=CORE_MAX_X).contains(&x) && (CORE_MIN_Z..=CORE_MAX_Z).contains(&z)
        || (x <= 7 || x >= 27) && (WING_MIN_Z..=WING_MAX_Z).contains(&z)
        || (ENTRANCE_MIN_X..=ENTRANCE_MAX_X).contains(&x)
            && (ENTRANCE_MIN_Z..=ENTRANCE_MAX_Z).contains(&z)
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

fn stairs(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:prismarine_brick_stairs".to_owned(),
        properties: vec![
            (
                "facing".to_owned(),
                match facing {
                    Direction::North => "north",
                    Direction::South => "south",
                    Direction::East => "east",
                    Direction::West => "west",
                    _ => "north",
                }
                .to_owned(),
            ),
            ("half".to_owned(), "bottom".to_owned()),
            ("shape".to_owned(), "straight".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ],
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
            | "minecraft:flowing_lava"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    ) || crate::tags::block_has("minecraft:replaceable", &block.name)
}
