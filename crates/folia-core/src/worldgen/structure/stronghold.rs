//! Deterministic procedural strongholds.
//!
//! The 45x57x12 piece is a documented approximation, not a Java piece-layout
//! port. A five-wide central corridor crosses a grand hall and a nine-block
//! staircase. Symmetric side halls, libraries, and prisons connect to it, while
//! a tall portal room closes the north end. The union of those rooms forms an
//! irregular stone-brick shell with stepped roofs, arched openings, columns,
//! bookshelves, prison bars, torches, four loot chests, and a 3x3 End portal in
//! a 5x5 frame. Room proportions, fixed room count, material distribution, and
//! chest placement are approximations; Java's recursive `StrongholdPieces`
//! graph, staircase well, Silverfish spawners, exact RNG order, and a Java block
//! fixture remain open.
//!
//! The origin is the horizontal center of the placement chunk. Rotation is
//! drawn from the large-feature seed and applies to geometry, directional block
//! states, room bounds, and loot positions. Start Y targets 26..=41 blocks below
//! the highest sampled surface, then searches at most twelve blocks for cave
//! density at two interior heights. Bounds keep at least twenty blocks between
//! the shell roof and the sampled surface; starts with no such underground band
//! are rejected. All writes and block-entity markers are clipped to the requested
//! chunk box.

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
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};
use std::collections::BTreeMap;

pub const WIDTH: i32 = 45;
pub const DEPTH: i32 = 57;
pub const HEIGHT: i32 = 12;

const CENTER_X: i32 = WIDTH / 2;
const CORRIDOR_CEILING: i32 = 5;
const SURFACE_CLEARANCE: i32 = 20;
const SURFACE_DEPTH: i32 = 26;
const DEPTH_VARIATION: i32 = 16;
const DENSITY_SEARCH_RADIUS: i32 = 12;
const BRICKS: [&str; 6] = [
    "minecraft:stone_bricks",
    "minecraft:stone_bricks",
    "minecraft:stone_bricks",
    "minecraft:cracked_stone_bricks",
    "minecraft:mossy_stone_bricks",
    "minecraft:chiseled_stone_bricks",
];
const CHESTS: [(i32, i32, i32, &str); 4] = [
    (4, 1, 15, "minecraft:chests/stronghold_library"),
    (40, 1, 15, "minecraft:chests/stronghold_library"),
    (3, 1, 32, "minecraft:chests/stronghold_crossing"),
    (41, 1, 32, "minecraft:chests/stronghold_crossing"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrongholdRoomKind {
    EntranceHall,
    GreatHall,
    Hall,
    Library,
    Prison,
    PortalRoom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrongholdRoom {
    kind: StrongholdRoomKind,
    bounds: BoundingBox,
}

impl StrongholdRoom {
    pub fn kind(self) -> StrongholdRoomKind {
        self.kind
    }

    pub fn bounds(self) -> BoundingBox {
        self.bounds
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrongholdPiece {
    seed: i64,
    origin: BlockPos,
    rotation: Rotation,
    bounding_box: BoundingBox,
    corridor_bounds: BoundingBox,
    stairs_bounds: BoundingBox,
    portal_position: BlockPos,
    rooms: Vec<StrongholdRoom>,
    chest_seeds: [i64; 4],
}

impl StrongholdPiece {
    pub fn new(seed: i64, x: i32, y: i32, z: i32, rotation: Rotation) -> Self {
        let piece_seed = piece_seed(seed, x, z);
        let mut random = LegacyRandomSource::new(piece_seed);
        let chest_seeds = [
            random.next_long(),
            random.next_long(),
            random.next_long(),
            random.next_long(),
        ];
        let origin = BlockPos::new(x, y, z);
        let (width, depth) = rotated_size(rotation);
        let min_x = x - (width - 1) / 2;
        let min_z = z - (depth - 1) / 2;
        let mut piece = Self {
            seed: piece_seed,
            origin,
            rotation,
            bounding_box: BoundingBox::new(
                min_x,
                y,
                min_z,
                min_x + width - 1,
                y + HEIGHT - 1,
                min_z + depth - 1,
            ),
            corridor_bounds: BoundingBox::new(x, y, z, x, y, z),
            stairs_bounds: BoundingBox::new(x, y, z, x, y, z),
            portal_position: origin,
            rooms: Vec::new(),
            chest_seeds,
        };
        piece.corridor_bounds = piece.local_bounds(20, 0, 0, 24, CORRIDOR_CEILING, 54);
        piece.stairs_bounds = piece.local_bounds(21, 0, 23, 23, 5, 34);
        piece.portal_position = piece.world_pos(CENTER_X, 1, 50);
        piece.rooms = ROOM_SPECS
            .iter()
            .map(|room| StrongholdRoom {
                kind: room.kind,
                bounds: piece.local_bounds(
                    room.min_x,
                    0,
                    room.min_z,
                    room.max_x,
                    room.ceiling,
                    room.max_z,
                ),
            })
            .collect();
        piece
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn origin(&self) -> BlockPos {
        self.origin
    }

    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn corridor_bounds(&self) -> BoundingBox {
        self.corridor_bounds
    }

    pub fn stairs_bounds(&self) -> BoundingBox {
        self.stairs_bounds
    }

    pub fn portal_position(&self) -> BlockPos {
        self.portal_position
    }

    pub fn rooms(&self) -> &[StrongholdRoom] {
        &self.rooms
    }

    pub fn room_bounds(&self, kind: StrongholdRoomKind) -> Option<BoundingBox> {
        self.rooms
            .iter()
            .find(|room| room.kind == kind)
            .map(|room| room.bounds)
    }

    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let ceilings = cell_ceilings();
        let mut writer = StrongholdWriter {
            piece: self,
            level,
            chunk_box,
            ceilings: &ceilings,
            result: StartPlacement::default(),
        };
        writer.generate_shell();
        writer.generate_arches();
        writer.generate_stairs();
        writer.generate_room_features();
        writer.generate_portal();
        writer.generate_torches();
        writer.generate_chests();
        let mut result = writer.result;
        if result.written > 0 {
            result.placed_pieces = 1;
        }
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }

    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let (rotated_x, rotated_z) = match self.rotation {
            Rotation::None => (x, z),
            Rotation::Clockwise90 => (DEPTH - 1 - z, x),
            Rotation::Clockwise180 => (WIDTH - 1 - x, DEPTH - 1 - z),
            Rotation::CounterClockwise90 => (z, WIDTH - 1 - x),
        };
        let (width, depth) = rotated_size(self.rotation);
        BlockPos::new(
            self.origin.x() - (width - 1) / 2 + rotated_x,
            self.origin.y() + y,
            self.origin.z() - (depth - 1) / 2 + rotated_z,
        )
    }

    fn local_bounds(
        &self,
        min_x: i32,
        min_y: i32,
        min_z: i32,
        max_x: i32,
        max_y: i32,
        max_z: i32,
    ) -> BoundingBox {
        let corners = [
            self.world_pos(min_x, min_y, min_z),
            self.world_pos(max_x, max_y, max_z),
            self.world_pos(min_x, min_y, max_z),
            self.world_pos(max_x, max_y, min_z),
        ];
        BoundingBox::new(
            corners.iter().map(|pos| pos.x()).min().expect("corner"),
            self.origin.y() + min_y,
            corners.iter().map(|pos| pos.z()).min().expect("corner"),
            corners.iter().map(|pos| pos.x()).max().expect("corner"),
            self.origin.y() + max_y,
            corners.iter().map(|pos| pos.z()).max().expect("corner"),
        )
    }
}

pub struct StrongholdStart;

impl StrongholdStart {
    pub const WIDTH: i32 = WIDTH;
    pub const DEPTH: i32 = DEPTH;
    pub const HEIGHT: i32 = HEIGHT;
    pub const SURFACE_CLEARANCE: i32 = SURFACE_CLEARANCE;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let source = context
            .height_source
            .ok_or_else(|| StructureError::MissingHeightSource {
                structure: structure_id.to_owned(),
            })?;
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let x = (context.chunk_pos.x << 4) + 8;
        let z = (context.chunk_pos.z << 4) + 8;
        let Some(y) = choose_start_y(source, x, z, rotation, &mut random) else {
            return Ok(StructureStart::invalid());
        };
        let piece = StrongholdPiece::new(context.seed, x, y, z, rotation);
        Ok(StructureStart::new_stronghold(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct RoomSpec {
    kind: StrongholdRoomKind,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    ceiling: i32,
}

const ROOM_SPECS: [RoomSpec; 9] = [
    RoomSpec {
        kind: StrongholdRoomKind::EntranceHall,
        min_x: 20,
        max_x: 24,
        min_z: 0,
        max_z: 8,
        ceiling: 6,
    },
    RoomSpec {
        kind: StrongholdRoomKind::GreatHall,
        min_x: 14,
        max_x: 30,
        min_z: 19,
        max_z: 36,
        ceiling: 8,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Hall,
        min_x: 0,
        max_x: 17,
        min_z: 21,
        max_z: 34,
        ceiling: 7,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Hall,
        min_x: 27,
        max_x: 44,
        min_z: 21,
        max_z: 34,
        ceiling: 7,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Library,
        min_x: 0,
        max_x: 16,
        min_z: 4,
        max_z: 17,
        ceiling: 7,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Library,
        min_x: 28,
        max_x: 44,
        min_z: 4,
        max_z: 17,
        ceiling: 7,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Prison,
        min_x: 0,
        max_x: 16,
        min_z: 39,
        max_z: 52,
        ceiling: 6,
    },
    RoomSpec {
        kind: StrongholdRoomKind::Prison,
        min_x: 28,
        max_x: 44,
        min_z: 39,
        max_z: 52,
        ceiling: 6,
    },
    RoomSpec {
        kind: StrongholdRoomKind::PortalRoom,
        min_x: 13,
        max_x: 31,
        min_z: 44,
        max_z: 56,
        ceiling: 11,
    },
];

fn cell_ceilings() -> BTreeMap<(i32, i32), i32> {
    let mut cells = BTreeMap::new();
    add_rectangle(&mut cells, 20, 0, 24, 54, CORRIDOR_CEILING);
    for (min_x, min_z, max_x, max_z) in [
        (16, 9, 24, 13),
        (20, 9, 28, 13),
        (17, 25, 24, 29),
        (20, 25, 27, 29),
        (16, 44, 24, 48),
        (20, 44, 28, 48),
    ] {
        add_rectangle(&mut cells, min_x, min_z, max_x, max_z, CORRIDOR_CEILING);
    }
    for room in ROOM_SPECS {
        add_rectangle(
            &mut cells,
            room.min_x,
            room.min_z,
            room.max_x,
            room.max_z,
            room.ceiling,
        );
    }
    cells
}

fn add_rectangle(
    cells: &mut BTreeMap<(i32, i32), i32>,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    ceiling: i32,
) {
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            cells.insert((x, z), ceiling);
        }
    }
}

fn choose_start_y(
    source: &dyn FeatureLevel,
    x: i32,
    z: i32,
    rotation: Rotation,
    random: &mut impl RandomSource,
) -> Option<i32> {
    let (width, depth) = rotated_size(rotation);
    let half_width = (width - 1) / 2;
    let half_depth = (depth - 1) / 2;
    let mut surface = i32::MIN;
    for dx in [-half_width, 0, half_width] {
        for dz in [-half_depth, 0, half_depth] {
            surface = surface.max(source.heightmap(Heightmap::WorldSurfaceWg, x + dx, z + dz)?);
        }
    }
    let min_y = source.min_y() + 8;
    let max_y =
        (surface - (HEIGHT - 1) - SURFACE_CLEARANCE).min(source.min_y() + source.height() - HEIGHT);
    if max_y < min_y {
        return None;
    }
    let target =
        (surface - SURFACE_DEPTH - random.next_int_bound(DEPTH_VARIATION)).clamp(min_y, max_y);
    if source.terrain_density_at(x, target + 2, z).is_none() {
        return Some(target);
    }
    let cave_score = |y: i32| {
        let mut score = 0;
        for dy in [2, 6] {
            for (dx, dz) in [(0, 0), (-4, 0), (4, 0), (0, -4), (0, 4)] {
                if source
                    .terrain_density_at(x + dx, y + dy, z + dz)
                    .is_some_and(|density| density <= 0.0)
                {
                    score += 1;
                }
            }
        }
        score
    };
    let mut best = target;
    let mut best_score = cave_score(target);
    let mut best_distance = i32::MAX;
    for radius in 0..=DENSITY_SEARCH_RADIUS {
        for delta in if radius == 0 {
            [0, 0]
        } else {
            [-radius, radius]
        } {
            let y = (target + delta).clamp(min_y, max_y);
            let score = cave_score(y);
            if score >= 6 {
                return Some(y);
            }
            let distance = (y - target).abs();
            if score > best_score || (score == best_score && distance < best_distance) {
                best = y;
                best_score = score;
                best_distance = distance;
            }
        }
    }
    Some(best)
}

struct StrongholdWriter<'a> {
    piece: &'a StrongholdPiece,
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    ceilings: &'a BTreeMap<(i32, i32), i32>,
    result: StartPlacement,
}

impl StrongholdWriter<'_> {
    fn place(&mut self, state: BlockState, x: i32, y: i32, z: i32) -> bool {
        self.place_state(state, x, y, z).is_some()
    }

    fn place_state(&mut self, state: BlockState, x: i32, y: i32, z: i32) -> Option<BlockPos> {
        let pos = self.piece.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return None;
        }
        let state = transform_state(&state, Mirror::None, self.piece.rotation);
        if self.level.set_block(pos, state) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            Some(pos)
        } else {
            self.result.rejected += 1;
            None
        }
    }

    fn material(&self, x: i32, y: i32, z: i32, role: u64) -> BlockState {
        let index = coordinate_hash(self.piece.seed, x, y, z, role) % BRICKS.len() as u64;
        state(BRICKS[index as usize])
    }

    fn generate_shell(&mut self) {
        for (&(x, z), &ceiling) in self.ceilings {
            let boundary = [(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)]
                .into_iter()
                .any(|neighbor| !self.ceilings.contains_key(&neighbor));
            self.place(self.material(x, 0, z, 0x5100), x, 0, z);
            for y in 1..ceiling {
                self.place(state("minecraft:air"), x, y, z);
            }
            if boundary {
                for y in 1..=ceiling {
                    self.place(self.material(x, y, z, 0x5200), x, y, z);
                }
            } else {
                self.place(self.material(x, ceiling, z, 0x5300), x, ceiling, z);
            }
        }
    }

    fn generate_arches(&mut self) {
        self.carve_arch_z(0, 20, 24, CORRIDOR_CEILING);
        self.build_arch_wall(19);
        self.build_arch_wall(36);
        for x in [0, 44] {
            self.carve_arch_x(x, 10, 12, 7);
        }
    }

    fn build_arch_wall(&mut self, z: i32) {
        for x in 19..=25 {
            for y in 1..=CORRIDOR_CEILING {
                self.place(self.material(x, y, z, 0x5400), x, y, z);
            }
        }
        self.carve_arch_z(z, 21, 23, CORRIDOR_CEILING);
    }

    fn carve_arch_z(&mut self, z: i32, min_x: i32, max_x: i32, ceiling: i32) {
        let center = (min_x + max_x) / 2;
        for x in min_x..=max_x {
            let height = (4 - (x - center).abs() / 2).min(ceiling - 1);
            for y in 1..=height {
                self.place(state("minecraft:air"), x, y, z);
            }
            self.place(state("minecraft:chiseled_stone_bricks"), x, height + 1, z);
        }
    }

    fn carve_arch_x(&mut self, x: i32, min_z: i32, max_z: i32, ceiling: i32) {
        let center = (min_z + max_z) / 2;
        for z in min_z..=max_z {
            let height = (4 - (z - center).abs() / 2).min(ceiling - 1);
            for y in 1..=height {
                self.place(state("minecraft:air"), x, y, z);
            }
            self.place(state("minecraft:chiseled_stone_bricks"), x, height + 1, z);
        }
    }

    fn generate_stairs(&mut self) {
        for z in 23..=31 {
            let step = (z - 23) / 2;
            for x in 21..=23 {
                for y in 0..step {
                    self.place(state("minecraft:chiseled_stone_bricks"), x, y, z);
                }
                self.place(stone_stairs(Direction::North), x, step, z);
            }
        }
        for z in 32..=34 {
            for x in 20..=24 {
                self.place(stone_slab(), x, 4, z);
            }
        }
    }

    fn generate_room_features(&mut self) {
        for room in ROOM_SPECS {
            match room.kind {
                StrongholdRoomKind::Library => self.generate_library(room),
                StrongholdRoomKind::Prison => self.generate_prison(room),
                StrongholdRoomKind::GreatHall | StrongholdRoomKind::Hall => {
                    self.generate_columns(room)
                }
                StrongholdRoomKind::EntranceHall | StrongholdRoomKind::PortalRoom => {}
            }
        }
    }

    fn generate_library(&mut self, room: RoomSpec) {
        for z in [room.min_z, room.max_z] {
            for x in room.min_x + 1..room.max_x {
                for y in 1..=2 {
                    self.place(state("minecraft:bookshelf"), x, y, z);
                }
            }
        }
        for x in [room.min_x, room.max_x] {
            for z in room.min_z + 2..room.max_z - 1 {
                for y in 1..=2 {
                    self.place(state("minecraft:bookshelf"), x, y, z);
                }
            }
        }
    }

    fn generate_prison(&mut self, room: RoomSpec) {
        let z = room.min_z + 2;
        for x in room.min_x + 1..room.max_x {
            if x == (room.min_x + room.max_x) / 2 {
                continue;
            }
            for y in 1..=3 {
                self.place(iron_bars(), x, y, z);
            }
        }
        for x in room.min_x + 1..room.max_x {
            for z in room.min_z + 1..=room.max_z - 1 {
                if coordinate_hash(self.piece.seed, x, 0, z, 0x5500).is_multiple_of(5) {
                    self.place(state("minecraft:cobblestone"), x, 0, z);
                }
            }
        }
    }

    fn generate_columns(&mut self, room: RoomSpec) {
        for (x, z) in [
            (room.min_x + 2, room.min_z + 2),
            (room.max_x - 2, room.min_z + 2),
            (room.min_x + 2, room.max_z - 2),
            (room.max_x - 2, room.max_z - 2),
        ] {
            for y in 1..room.ceiling - 1 {
                self.place(state("minecraft:chiseled_stone_bricks"), x, y, z);
            }
        }
    }

    fn generate_portal(&mut self) {
        for dz in -2_i32..=2 {
            for dx in -2_i32..=2 {
                let x = CENTER_X + dx;
                let z = 50 + dz;
                if dx.abs().max(dz.abs()) == 2 {
                    let facing = if dx == -2 {
                        Direction::East
                    } else if dx == 2 {
                        Direction::West
                    } else if dz == -2 {
                        Direction::South
                    } else {
                        Direction::North
                    };
                    self.place(portal_frame(facing), x, 1, z);
                } else {
                    self.place(state("minecraft:lava"), x, 0, z);
                    self.place(state("minecraft:end_portal"), x, 1, z);
                }
            }
        }
        for (x, z, facing) in [
            (15, 46, Direction::East),
            (29, 46, Direction::West),
            (15, 54, Direction::East),
            (29, 54, Direction::West),
        ] {
            self.place(wall_torch(facing), x, 3, z);
        }
    }

    fn generate_torches(&mut self) {
        for z in [5, 13, 40, 48] {
            self.place(wall_torch(Direction::East), 19, 3, z);
            self.place(wall_torch(Direction::West), 25, 3, z);
        }
    }

    fn generate_chests(&mut self) {
        for (index, (x, y, z, loot_table)) in CHESTS.into_iter().enumerate() {
            self.place_chest(x, y, z, loot_table, self.piece.chest_seeds[index]);
        }
    }

    fn place_chest(&mut self, x: i32, y: i32, z: i32, loot_table: &str, seed: i64) {
        let pos = self.piece.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos)
            || self
                .level
                .get_block(pos)
                .is_some_and(|block| block.name == "minecraft:chest")
        {
            return;
        }
        let chest = BlockState {
            name: "minecraft:chest".to_owned(),
            properties: vec![("facing".to_owned(), "north".to_owned())],
        };
        if self.place_state(chest.clone(), x, y, z).is_some() {
            self.result.block_entities.push(PlacedBlock {
                pos,
                state: transform_state(&chest, Mirror::None, self.piece.rotation),
                nbt: Some(Tag::Compound(vec![
                    ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                    ("LootTable".to_owned(), Tag::String(loot_table.to_owned())),
                    ("LootTableSeed".to_owned(), Tag::Long(seed)),
                ])),
            });
        }
    }
}

fn rotated_size(rotation: Rotation) -> (i32, i32) {
    if rotation.swaps_horizontal_axes() {
        (DEPTH, WIDTH)
    } else {
        (WIDTH, DEPTH)
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

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        _ => "north",
    }
}

fn stone_stairs(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:stone_brick_stairs".to_owned(),
        properties: vec![
            ("facing".to_owned(), direction_name(facing).to_owned()),
            ("half".to_owned(), "bottom".to_owned()),
            ("shape".to_owned(), "straight".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ],
    }
}

fn stone_slab() -> BlockState {
    BlockState {
        name: "minecraft:stone_brick_slab".to_owned(),
        properties: vec![
            ("type".to_owned(), "bottom".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ],
    }
}

fn wall_torch(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:wall_torch".to_owned(),
        properties: vec![("facing".to_owned(), direction_name(facing).to_owned())],
    }
}

fn portal_frame(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:end_portal_frame".to_owned(),
        properties: vec![
            ("facing".to_owned(), direction_name(facing).to_owned()),
            ("eye".to_owned(), "true".to_owned()),
        ],
    }
}

fn iron_bars() -> BlockState {
    BlockState {
        name: "minecraft:iron_bars".to_owned(),
        properties: vec![
            ("east".to_owned(), "false".to_owned()),
            ("north".to_owned(), "false".to_owned()),
            ("south".to_owned(), "false".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
            ("west".to_owned(), "false".to_owned()),
        ],
    }
}
