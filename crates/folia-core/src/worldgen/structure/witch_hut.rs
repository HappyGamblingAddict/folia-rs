use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{BoundingBox, make_bounding_box};
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::PlacedBlock;
use folia_math::{BlockPos, Direction};

pub const GENERATION_HEIGHT: i32 = 64;
pub const FLOOR_OFFSET: i32 = 3;
pub const WIDTH: i32 = 5;
pub const DEPTH: i32 = 5;
pub const HEIGHT: i32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct WitchHutPiece {
    bounding_box: BoundingBox,
}

impl WitchHutPiece {
    pub fn new(x: i32, z: i32) -> Self {
        Self {
            bounding_box: make_bounding_box(
                x,
                GENERATION_HEIGHT - FLOOR_OFFSET,
                z,
                Direction::North,
                WIDTH,
                HEIGHT,
                DEPTH,
            ),
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
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
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let source = height_source.unwrap_or(&*level);
        let Some(lowest) = lowest_ground(source, self.bounding_box) else {
            return Ok(StartPlacement::default());
        };
        let mut hut = self.clone();
        hut.bounding_box = hut
            .bounding_box
            .moved(0, lowest - hut.bounding_box.min_y(), 0);
        let floor_y = hut.bounding_box.min_y() + FLOOR_OFFSET;
        let mut writer = HutWriter {
            level,
            chunk_box,
            min_x: hut.bounding_box.min_x(),
            min_z: hut.bounding_box.min_z(),
            floor_y,
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

pub struct WitchHutStart;

impl WitchHutStart {
    pub const WIDTH: i32 = WIDTH;
    pub const DEPTH: i32 = DEPTH;
    pub const HEIGHT: i32 = HEIGHT;
    pub const GENERATION_HEIGHT: i32 = GENERATION_HEIGHT;
    pub const FLOOR_OFFSET: i32 = FLOOR_OFFSET;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let piece = WitchHutPiece::new(context.chunk_pos.x << 4, context.chunk_pos.z << 4);
        Ok(StructureStart::new_witch_hut(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

fn lowest_ground(level: &dyn FeatureLevel, bounds: BoundingBox) -> Option<i32> {
    let mut lowest = level.min_y() + level.height();
    let mut found = false;
    for z in bounds.min_z()..=bounds.max_z() {
        for x in bounds.min_x()..=bounds.max_x() {
            if let Some(height) = level.heightmap(Heightmap::MotionBlockingNoLeaves, x, z) {
                lowest = lowest.min(height);
                found = true;
            }
        }
    }
    found.then_some(lowest)
}

struct HutWriter<'a> {
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    min_x: i32,
    min_z: i32,
    floor_y: i32,
    result: StartPlacement,
}

impl HutWriter<'_> {
    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(self.min_x + x, self.floor_y + y, self.min_z + z)
    }

    fn place(&mut self, state: BlockState, x: i32, y: i32, z: i32) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return false;
        }
        if self.level.set_block(pos, state) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            true
        } else {
            self.result.rejected += 1;
            false
        }
    }

    fn fill_column_down(&mut self, state: &BlockState, x: i32, y: i32, z: i32) {
        let mut pos = self.world_pos(x, y, z);
        if pos.x() < self.chunk_box.min_x()
            || pos.x() > self.chunk_box.max_x()
            || pos.z() < self.chunk_box.min_z()
            || pos.z() > self.chunk_box.max_z()
        {
            return;
        }
        while pos.y() > self.level.min_y() + 1 {
            let replaceable = self
                .level
                .get_block(pos)
                .is_none_or(is_replaceable_by_structures);
            if !replaceable {
                break;
            }
            if self.chunk_box.is_inside(pos) && self.level.set_block(pos, state.clone()) {
                self.result.written += 1;
                self.result.written_blocks.push(pos);
            }
            pos = pos.below();
        }
    }

    fn generate(&mut self) {
        let planks = state("minecraft:spruce_planks");
        let log = state("minecraft:spruce_log");
        let air = state("minecraft:air");
        let glass = state("minecraft:glass_pane");

        for x in 0..WIDTH {
            for z in 0..DEPTH {
                self.place(planks.clone(), x, 0, z);
            }
        }
        for (x, z) in [
            (0, 0),
            (0, DEPTH - 1),
            (WIDTH - 1, 0),
            (WIDTH - 1, DEPTH - 1),
        ] {
            self.fill_column_down(&log, x, -1, z);
        }

        for y in 1..=3 {
            for x in 0..WIDTH {
                for z in 0..DEPTH {
                    let boundary = x == 0 || x == WIDTH - 1 || z == 0 || z == DEPTH - 1;
                    let block = if boundary {
                        if (x == 0 || x == WIDTH - 1) && (z == 0 || z == DEPTH - 1) {
                            log.clone()
                        } else {
                            planks.clone()
                        }
                    } else {
                        air.clone()
                    };
                    self.place(block, x, y, z);
                }
            }
        }

        for y in 1..=2 {
            self.place(air.clone(), 2, y, DEPTH - 1);
        }
        for (x, y, z) in [
            (0, 2, 2),
            (WIDTH - 1, 2, 2),
            (1, 2, DEPTH - 1),
            (WIDTH - 2, 2, DEPTH - 1),
            (2, 2, 0),
        ] {
            self.place(glass.clone(), x, y, z);
        }

        for x in [0, WIDTH - 1] {
            for z in 0..DEPTH {
                let roof_y = 4 + z.min(DEPTH - 1 - z);
                for y in 4..=roof_y {
                    self.place(planks.clone(), x, y, z);
                }
            }
        }
        for z in 0..DEPTH {
            let roof_y = 4 + z.min(DEPTH - 1 - z);
            let facing = if z < DEPTH / 2 {
                Direction::South
            } else {
                Direction::North
            };
            for x in 0..WIDTH {
                self.place(spruce_stairs(facing), x, roof_y, z);
            }
        }

        self.place_cauldron(2, 1, 2);
    }

    fn place_cauldron(&mut self, x: i32, y: i32, z: i32) {
        let cauldron = state("minecraft:cauldron");
        if self.place(cauldron.clone(), x, y, z) {
            self.result.block_entities.push(PlacedBlock {
                pos: self.world_pos(x, y, z),
                state: cauldron,
                nbt: Some(Tag::Compound(vec![(
                    "id".to_owned(),
                    Tag::String("minecraft:cauldron".to_owned()),
                )])),
            });
        }
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn spruce_stairs(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:spruce_stairs".to_owned(),
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
            | "minecraft:lava"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    ) || crate::tags::block_has("minecraft:replaceable", &block.name)
}
