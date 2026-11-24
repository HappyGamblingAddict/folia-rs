use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{BoundingBox, make_bounding_box};
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::PlacedBlock;
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};

pub const GENERATION_HEIGHT: i32 = 64;
pub const WIDTH: i32 = 12;
pub const DEPTH: i32 = 15;
pub const HEIGHT: i32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct JunglePyramidPiece {
    seed: i64,
    bounding_box: BoundingBox,
    direction: Direction,
}

impl JunglePyramidPiece {
    pub fn new(seed: i64, x: i32, z: i32, direction: Direction) -> Self {
        Self {
            seed,
            bounding_box: make_bounding_box(
                x,
                GENERATION_HEIGHT,
                z,
                direction,
                WIDTH,
                HEIGHT,
                DEPTH,
            ),
            direction,
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn direction(&self) -> Direction {
        self.direction
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
        let Some(base_y) = average_ground(source, self.bounding_box, chunk_box) else {
            return Ok(StartPlacement::default());
        };
        let random = LegacyRandomSource::new(
            self.seed ^ ((chunk_box.min_x() as i64) << 32) ^ (chunk_box.min_z() as i64),
        );
        let mut writer = TempleWriter {
            level,
            chunk_box,
            min_x: self.bounding_box.min_x(),
            min_z: self.bounding_box.min_z(),
            base_y,
            direction: self.direction,
            random,
            result: StartPlacement::default(),
        };
        writer.generate_body();
        let mut result = writer.result;
        result.placed_pieces = 1;
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }
}

pub struct JunglePyramidStart;

impl JunglePyramidStart {
    pub const WIDTH: i32 = WIDTH;
    pub const DEPTH: i32 = DEPTH;
    pub const HEIGHT: i32 = HEIGHT;
    pub const GENERATION_HEIGHT: i32 = GENERATION_HEIGHT;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let chunk = context.chunk_pos;
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut random, context.seed, chunk.x, chunk.z);
        let directions = [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ];
        let direction = directions[random.next_int_bound(4) as usize];
        let piece = JunglePyramidPiece::new(context.seed, chunk.x << 4, chunk.z << 4, direction);
        Ok(StructureStart::new_jungle_pyramid(
            structure_id,
            chunk,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

fn average_ground(
    level: &dyn FeatureLevel,
    bounds: BoundingBox,
    chunk_box: BoundingBox,
) -> Option<i32> {
    let min_x = bounds.min_x().max(chunk_box.min_x());
    let max_x = bounds.max_x().min(chunk_box.max_x());
    let min_z = bounds.min_z().max(chunk_box.min_z());
    let max_z = bounds.max_z().min(chunk_box.max_z());
    let mut total = 0_i64;
    let mut count = 0_i64;
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            if let Some(height) = level.heightmap(Heightmap::MotionBlockingNoLeaves, x, z) {
                total += i64::from(height);
                count += 1;
            }
        }
    }
    (count > 0).then(|| (total / count) as i32)
}

struct TempleWriter<'a> {
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    min_x: i32,
    min_z: i32,
    base_y: i32,
    direction: Direction,
    random: LegacyRandomSource,
    result: StartPlacement,
}

impl TempleWriter<'_> {
    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let (world_x, world_z) = match self.direction {
            Direction::North => (self.min_x + x, self.min_z + DEPTH - 1 - z),
            Direction::South => (self.min_x + x, self.min_z + z),
            Direction::West => (self.min_x + WIDTH - 1 - z, self.min_z + x),
            Direction::East => (self.min_x + z, self.min_z + DEPTH - 1 - x),
            _ => (self.min_x + x, self.min_z + z),
        };
        BlockPos::new(world_x, self.base_y + y, world_z)
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
        let state = rotate_state(state, self.direction);
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
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    self.place(state("minecraft:air"), x, y, z);
                }
            }
        }
    }

    fn fill_mossy(
        &mut self,
        min_x: i32,
        min_y: i32,
        min_z: i32,
        max_x: i32,
        max_y: i32,
        max_z: i32,
    ) {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    let block = if self.random.next_float() < 0.4 {
                        "minecraft:mossy_cobblestone"
                    } else {
                        "minecraft:cobblestone"
                    };
                    self.place(state(block), x, y, z);
                }
            }
        }
    }

    fn place_chest(&mut self, x: i32, y: i32, z: i32) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos)
            || self
                .level
                .get_block(pos)
                .is_some_and(|block| block.name == "minecraft:chest")
        {
            return false;
        }
        let chest = BlockState {
            name: "minecraft:chest".to_owned(),
            properties: vec![
                ("facing".to_owned(), "east".to_owned()),
                ("type".to_owned(), "single".to_owned()),
                ("waterlogged".to_owned(), "false".to_owned()),
            ],
        };
        let Some((pos, chest)) = self.place_state(chest, x, y, z) else {
            return false;
        };
        self.result.block_entities.push(PlacedBlock {
            pos,
            state: chest,
            nbt: Some(Tag::Compound(vec![
                ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                (
                    "LootTable".to_owned(),
                    Tag::String("minecraft:chests/jungle_temple".to_owned()),
                ),
                (
                    "LootTableSeed".to_owned(),
                    Tag::Long(self.random.next_long()),
                ),
            ])),
        });
        true
    }

    fn place_dispenser(&mut self, x: i32, y: i32, z: i32, facing: Direction) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos)
            || self
                .level
                .get_block(pos)
                .is_some_and(|block| block.name == "minecraft:dispenser")
        {
            return false;
        }
        let dispenser = BlockState {
            name: "minecraft:dispenser".to_owned(),
            properties: vec![
                ("facing".to_owned(), direction_name(facing).to_owned()),
                ("triggered".to_owned(), "false".to_owned()),
            ],
        };
        let Some((pos, dispenser)) = self.place_state(dispenser, x, y, z) else {
            return false;
        };
        self.result.block_entities.push(PlacedBlock {
            pos,
            state: dispenser,
            nbt: Some(Tag::Compound(vec![
                (
                    "id".to_owned(),
                    Tag::String("minecraft:dispenser".to_owned()),
                ),
                (
                    "LootTable".to_owned(),
                    Tag::String("minecraft:chests/jungle_temple_dispenser".to_owned()),
                ),
                (
                    "LootTableSeed".to_owned(),
                    Tag::Long(self.random.next_long()),
                ),
            ])),
        });
        true
    }

    fn generate_body(&mut self) {
        self.fill_mossy(0, -4, 0, WIDTH - 1, 0, DEPTH - 1);
        self.fill_mossy(2, 1, 2, 9, 2, 2);
        self.fill_mossy(2, 1, 12, 9, 2, 12);
        self.fill_mossy(2, 1, 3, 2, 2, 11);
        self.fill_mossy(9, 1, 3, 9, 2, 11);
        self.fill_mossy(1, 3, 1, 10, 6, 1);
        self.fill_mossy(1, 3, 13, 10, 6, 13);
        self.fill_mossy(1, 3, 2, 1, 6, 12);
        self.fill_mossy(10, 3, 2, 10, 6, 12);
        self.fill_mossy(2, 3, 2, 9, 3, 12);
        self.fill_mossy(2, 6, 2, 9, 6, 12);
        self.fill_mossy(3, 7, 3, 8, 7, 11);
        self.fill_mossy(4, 8, 4, 7, 8, 10);
        self.fill_air(3, 1, 3, 8, 2, 11);
        self.fill_air(4, 3, 6, 7, 3, 9);
        self.fill_air(2, 4, 2, 9, 5, 12);
        self.fill_air(4, 6, 5, 7, 6, 9);
        self.fill_air(5, 7, 6, 6, 7, 8);
        self.fill_air(5, 1, 2, 6, 2, 2);
        self.fill_air(5, 2, 12, 6, 2, 12);
        self.fill_air(5, 5, 1, 6, 5, 1);
        self.fill_air(5, 5, 13, 6, 5, 13);
        for (x, z) in [(1, 5), (10, 5), (1, 9), (10, 9)] {
            self.place(state("minecraft:air"), x, 5, z);
        }
        for z in [0, 14] {
            for x in [2, 4, 7, 9] {
                self.fill_mossy(x, 4, z, x, 5, z);
            }
        }
        self.fill_mossy(5, 6, 0, 6, 6, 0);
        for x in [0, 11] {
            for z in (2..=12).step_by(2) {
                self.fill_mossy(x, 4, z, x, 5, z);
            }
            for z in [5, 9] {
                self.fill_mossy(x, 6, z, x, 6, z);
            }
        }
        for (x, z) in [(2, 2), (9, 2), (2, 12), (9, 12)] {
            self.fill_mossy(x, 7, z, x, 9, z);
        }
        for (x, z) in [(4, 4), (7, 4), (4, 10), (7, 10)] {
            self.fill_mossy(x, 9, z, x, 9, z);
        }
        self.fill_mossy(5, 9, 7, 6, 9, 7);

        let stairs_east = stairs(Direction::East);
        let stairs_west = stairs(Direction::West);
        let stairs_south = stairs(Direction::South);
        let stairs_north = stairs(Direction::North);
        for (x, z) in [(5, 6), (6, 6)] {
            self.place(stairs_east.clone(), x, 9, z);
        }
        for (x, z) in [(5, 8), (6, 8)] {
            self.place(stairs_south.clone(), x, 9, z);
        }
        for x in 4..=7 {
            self.place(stairs_north.clone(), x, 0, 0);
        }
        for (x, y, z) in [
            (4, 1, 8),
            (4, 2, 9),
            (4, 3, 10),
            (7, 1, 8),
            (7, 2, 9),
            (7, 3, 10),
        ] {
            self.place(stairs_north.clone(), x, y, z);
        }
        self.fill_mossy(4, 1, 9, 4, 1, 9);
        self.fill_mossy(7, 1, 9, 7, 1, 9);
        self.fill_mossy(4, 1, 10, 7, 2, 10);
        self.fill_mossy(5, 4, 5, 6, 4, 5);
        self.place(stairs_east.clone(), 4, 4, 5);
        self.place(stairs_west.clone(), 7, 4, 5);
        for step in 0..4 {
            self.place(stairs_south.clone(), 5, -step, 6 + step);
            self.place(stairs_south.clone(), 6, -step, 6 + step);
            self.fill_air(5, -step, 7 + step, 6, -step, 9 + step);
        }
        self.fill_air(1, -3, 12, 10, -1, 13);
        self.fill_air(1, -3, 1, 3, -1, 13);
        self.fill_air(1, -3, 1, 9, -1, 5);
        for z in (1..=13).step_by(2) {
            self.fill_mossy(1, -3, z, 1, -2, z);
        }
        for z in (2..=12).step_by(2) {
            self.fill_mossy(1, -1, z, 3, -1, z);
        }
        self.fill_mossy(2, -2, 1, 5, -2, 1);
        self.fill_mossy(7, -2, 1, 9, -2, 1);
        self.fill_mossy(6, -3, 1, 6, -3, 1);
        self.fill_mossy(6, -1, 1, 6, -1, 1);

        self.place(tripwire_hook(Direction::East), 1, -3, 8);
        self.place(tripwire_hook(Direction::West), 4, -3, 8);
        self.place(tripwire(true, true), 2, -3, 8);
        self.place(tripwire(true, true), 3, -3, 8);
        for z in 2..=7 {
            self.place(
                redstone(&[(Direction::North, "side"), (Direction::South, "side")]),
                5,
                -3,
                z,
            );
        }
        self.place(
            redstone(&[(Direction::North, "side"), (Direction::West, "side")]),
            5,
            -3,
            1,
        );
        self.place(
            redstone(&[(Direction::East, "side"), (Direction::West, "side")]),
            4,
            -3,
            1,
        );
        self.place(state("minecraft:mossy_cobblestone"), 3, -3, 1);
        self.place_dispenser(3, -2, 1, Direction::North);
        self.place(vine(Direction::South), 3, -2, 2);
        self.place(tripwire_hook(Direction::North), 7, -3, 1);
        self.place(tripwire_hook(Direction::South), 7, -3, 5);
        for z in 2..=4 {
            self.place(tripwire(true, true), 7, -3, z);
        }
        self.place(
            redstone(&[(Direction::East, "side"), (Direction::West, "side")]),
            8,
            -3,
            6,
        );
        self.place(
            redstone(&[(Direction::West, "side"), (Direction::South, "side")]),
            9,
            -3,
            6,
        );
        self.place(
            redstone(&[(Direction::North, "side"), (Direction::South, "up")]),
            9,
            -3,
            5,
        );
        self.place(state("minecraft:mossy_cobblestone"), 9, -3, 4);
        self.place(
            redstone(&[(Direction::North, "side"), (Direction::South, "side")]),
            9,
            -2,
            4,
        );
        self.place_dispenser(9, -2, 3, Direction::West);
        self.place(vine(Direction::East), 8, -1, 3);
        self.place(vine(Direction::East), 8, -2, 3);
        self.place_chest(8, -3, 3);
        self.place(state("minecraft:mossy_cobblestone"), 9, -3, 2);
        self.place(state("minecraft:mossy_cobblestone"), 8, -3, 1);
        for (x, y, z) in [
            (4, -3, 5),
            (5, -2, 5),
            (5, -1, 5),
            (6, -3, 5),
            (7, -2, 5),
            (7, -1, 5),
            (8, -3, 5),
        ] {
            self.place(state("minecraft:mossy_cobblestone"), x, y, z);
        }
        self.fill_mossy(9, -1, 1, 9, -1, 5);
        self.fill_air(8, -3, 8, 10, -1, 10);
        for x in 8..=10 {
            self.place(state("minecraft:chiseled_stone_bricks"), x, -2, 11);
        }
        for x in 8..=10 {
            self.place(lever(), x, -2, 12);
        }
        self.fill_mossy(8, -3, 8, 8, -3, 10);
        self.fill_mossy(10, -3, 8, 10, -3, 10);
        self.place(state("minecraft:mossy_cobblestone"), 10, -2, 9);
        self.place(
            redstone(&[(Direction::North, "side"), (Direction::South, "side")]),
            8,
            -2,
            9,
        );
        self.place(
            redstone(&[(Direction::North, "side"), (Direction::South, "side")]),
            8,
            -2,
            10,
        );
        self.place(
            redstone(&[
                (Direction::North, "side"),
                (Direction::East, "side"),
                (Direction::South, "side"),
                (Direction::West, "side"),
            ]),
            10,
            -1,
            9,
        );
        self.place(sticky_piston(Direction::Up), 9, -2, 8);
        self.place(sticky_piston(Direction::West), 10, -2, 8);
        self.place(sticky_piston(Direction::West), 10, -1, 8);
        self.place(repeater(), 10, -2, 10);
        self.place_chest(9, -3, 10);
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn stairs(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:cobblestone_stairs".to_owned(),
        properties: vec![
            ("facing".to_owned(), direction_name(facing).to_owned()),
            ("half".to_owned(), "bottom".to_owned()),
            ("shape".to_owned(), "straight".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ],
    }
}

fn tripwire_hook(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:tripwire_hook".to_owned(),
        properties: vec![
            ("attached".to_owned(), "true".to_owned()),
            ("facing".to_owned(), direction_name(facing).to_owned()),
            ("powered".to_owned(), "false".to_owned()),
        ],
    }
}

fn tripwire(north: bool, east: bool) -> BlockState {
    BlockState {
        name: "minecraft:tripwire".to_owned(),
        properties: vec![
            ("attached".to_owned(), "true".to_owned()),
            ("disarmed".to_owned(), "false".to_owned()),
            ("east".to_owned(), east.to_string()),
            ("north".to_owned(), north.to_string()),
            ("powered".to_owned(), "false".to_owned()),
            ("south".to_owned(), "false".to_owned()),
            ("west".to_owned(), "false".to_owned()),
        ],
    }
}

fn redstone(connections: &[(Direction, &str)]) -> BlockState {
    let mut properties = vec![("power".to_owned(), "0".to_owned())];
    for direction in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ] {
        let value = connections
            .iter()
            .find_map(|(candidate, side)| (*candidate == direction).then_some(*side))
            .unwrap_or("none");
        properties.push((direction_name(direction).to_owned(), value.to_owned()));
    }
    BlockState {
        name: "minecraft:redstone_wire".to_owned(),
        properties,
    }
}

fn vine(facing: Direction) -> BlockState {
    let mut properties = Vec::new();
    for direction in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
        Direction::Up,
    ] {
        properties.push((
            direction_name(direction).to_owned(),
            (direction == facing).to_string(),
        ));
    }
    BlockState {
        name: "minecraft:vine".to_owned(),
        properties,
    }
}

fn lever() -> BlockState {
    BlockState {
        name: "minecraft:lever".to_owned(),
        properties: vec![
            ("face".to_owned(), "wall".to_owned()),
            ("facing".to_owned(), "north".to_owned()),
            ("powered".to_owned(), "false".to_owned()),
        ],
    }
}

fn sticky_piston(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:sticky_piston".to_owned(),
        properties: vec![
            ("extended".to_owned(), "true".to_owned()),
            ("facing".to_owned(), direction_name(facing).to_owned()),
        ],
    }
}

fn repeater() -> BlockState {
    BlockState {
        name: "minecraft:repeater".to_owned(),
        properties: vec![
            ("delay".to_owned(), "1".to_owned()),
            ("facing".to_owned(), "north".to_owned()),
            ("locked".to_owned(), "false".to_owned()),
            ("powered".to_owned(), "false".to_owned()),
        ],
    }
}

fn rotate_state(state: BlockState, orientation: Direction) -> BlockState {
    let mut properties = state.properties.clone();
    if let Some((_, value)) = properties.iter_mut().find(|(key, _)| key == "facing")
        && let Some(direction) = direction_from_name(value)
    {
        *value = direction_name(rotate_direction(direction, orientation)).to_owned();
    }
    let directional: Vec<_> = [
        ("north", Direction::North),
        ("east", Direction::East),
        ("south", Direction::South),
        ("west", Direction::West),
    ]
    .into_iter()
    .filter_map(|(key, direction)| {
        properties
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| (key, direction, value.clone()))
    })
    .collect();
    if !directional.is_empty() {
        properties.retain(|(key, _)| !matches!(key.as_str(), "north" | "east" | "south" | "west"));
        for (_, direction, value) in directional {
            properties.push((
                direction_name(rotate_direction(direction, orientation)).to_owned(),
                value,
            ));
        }
    }
    BlockState {
        name: state.name,
        properties,
    }
}

fn rotate_direction(direction: Direction, orientation: Direction) -> Direction {
    if matches!(direction, Direction::Up | Direction::Down) {
        return direction;
    }
    match orientation {
        Direction::North => match direction {
            Direction::North => Direction::South,
            Direction::East => Direction::East,
            Direction::South => Direction::North,
            Direction::West => Direction::West,
            _ => direction,
        },
        Direction::South => direction,
        Direction::West => match direction {
            Direction::North => Direction::East,
            Direction::East => Direction::West,
            Direction::South => Direction::West,
            Direction::West => Direction::East,
            _ => direction,
        },
        Direction::East => match direction {
            Direction::North => Direction::East,
            Direction::East => Direction::North,
            Direction::South => Direction::West,
            Direction::West => Direction::South,
            _ => direction,
        },
        _ => direction,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

fn direction_from_name(name: &str) -> Option<Direction> {
    match name {
        "down" => Some(Direction::Down),
        "up" => Some(Direction::Up),
        "north" => Some(Direction::North),
        "south" => Some(Direction::South),
        "west" => Some(Direction::West),
        "east" => Some(Direction::East),
        _ => None,
    }
}
