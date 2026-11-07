//! Port of `NetherFortressPieces` and `NetherFortressStructure`.
//!
//! Fortresses are procedural: the start is a `BridgeCrossing` at
//! `(chunkX * 16 + 2, 64, chunkZ * 16 + 2)`, and every child is selected from
//! the bridge/castle `PieceWeight` tables with Java's legacy random draw order.
//! The generated graph is then shifted by `StructurePiecesBuilder.moveInsideHeights`
//! into `[48, 70]`.
//!
//! Block placement is a direct port of every shipped piece's `postProcess`.
//! Chests and the blaze spawner are returned as block entities and data markers
//! instead of live block-entity objects, matching the rest of this structure
//! slice.

use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::FeatureLevel;
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{BoundingBox, StructureDataMarker, make_bounding_box};
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::StartPlacement;
use crate::worldgen::structure::template::PlacedBlock;
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};

/// `NetherFortressPieces.MAX_DEPTH`.
pub const MAX_DEPTH: i32 = 30;
/// `NetherFortressPieces.LOWEST_Y_POSITION`.
pub const LOWEST_Y_POSITION: i32 = 10;
/// `NetherFortressPieces.MAGIC_START_Y`.
pub const MAGIC_START_Y: i32 = 64;
/// `BridgeEndFiller` piece width.
pub const BRIDGE_END_WIDTH: i32 = 5;
/// `BridgeEndFiller` piece height.
pub const BRIDGE_END_HEIGHT: i32 = 10;
/// `BridgeEndFiller` piece depth.
pub const BRIDGE_END_DEPTH: i32 = 8;

const MAX_PIECE_DISTANCE: i32 = 112;

/// One of Java's `NetherFortressPieces` classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FortressPieceKind {
    /// `NetherFortressPieces.StartPiece` (a `BridgeCrossing`).
    Start,
    BridgeCrossing,
    BridgeEndFiller,
    BridgeStraight,
    RoomCrossing,
    StairsRoom,
    MonsterThrone,
    CastleEntrance,
    CastleSmallCorridor,
    CastleSmallCorridorCrossing,
    CastleSmallCorridorLeftTurn,
    CastleSmallCorridorRightTurn,
    CastleCorridorStairs,
    CastleCorridorTBalcony,
    CastleStalkRoom,
}

impl FortressPieceKind {
    pub fn name(self) -> &'static str {
        match self {
            FortressPieceKind::Start => "minecraft:nether_fortress_start",
            FortressPieceKind::BridgeCrossing => "minecraft:nether_fortress_bridge_crossing",
            FortressPieceKind::BridgeEndFiller => "minecraft:nether_fortress_bridge_end_filler",
            FortressPieceKind::BridgeStraight => "minecraft:nether_fortress_bridge_straight",
            FortressPieceKind::RoomCrossing => "minecraft:nether_fortress_room_crossing",
            FortressPieceKind::StairsRoom => "minecraft:nether_fortress_stairs_room",
            FortressPieceKind::MonsterThrone => "minecraft:nether_fortress_monster_throne",
            FortressPieceKind::CastleEntrance => "minecraft:nether_fortress_castle_entrance",
            FortressPieceKind::CastleSmallCorridor => {
                "minecraft:nether_fortress_castle_small_corridor"
            }
            FortressPieceKind::CastleSmallCorridorCrossing => {
                "minecraft:nether_fortress_castle_small_corridor_crossing"
            }
            FortressPieceKind::CastleSmallCorridorLeftTurn => {
                "minecraft:nether_fortress_castle_small_corridor_left_turn"
            }
            FortressPieceKind::CastleSmallCorridorRightTurn => {
                "minecraft:nether_fortress_castle_small_corridor_right_turn"
            }
            FortressPieceKind::CastleCorridorStairs => {
                "minecraft:nether_fortress_castle_corridor_stairs"
            }
            FortressPieceKind::CastleCorridorTBalcony => {
                "minecraft:nether_fortress_castle_corridor_t_balcony"
            }
            FortressPieceKind::CastleStalkRoom => "minecraft:nether_fortress_castle_stalk_room",
        }
    }

    #[allow(dead_code)]
    fn is_bridge_crossing(self) -> bool {
        matches!(
            self,
            FortressPieceKind::Start | FortressPieceKind::BridgeCrossing
        )
    }
}

/// One generated fortress piece.
#[derive(Debug, Clone, PartialEq)]
pub struct FortressPiece {
    pub kind: FortressPieceKind,
    pub bounding_box: BoundingBox,
    pub orientation: Direction,
    pub gen_depth: i32,
    /// `BridgeEndFiller.selfSeed`.
    pub self_seed: i32,
    /// `CastleSmallCorridor*TurnPiece.isNeedingChest`.
    pub needs_chest: bool,
    /// `MonsterThrone.hasPlacedSpawner`.
    pub has_placed_spawner: bool,
}

impl FortressPiece {
    fn new(
        kind: FortressPieceKind,
        bounding_box: BoundingBox,
        orientation: Direction,
        gen_depth: i32,
    ) -> Self {
        Self {
            kind,
            bounding_box,
            orientation,
            gen_depth,
            self_seed: 0,
            needs_chest: false,
            has_placed_spawner: false,
        }
    }

    pub fn name(&self) -> &'static str {
        self.kind.name()
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    /// `StructurePiece.postProcess`, clipped to `chunk_box`.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let mut writer = FortressWriter {
            level,
            piece: self,
            chunk_box,
            random,
            result: StartPlacement::default(),
        };
        writer.generate_body();
        Ok(writer.result)
    }
}

/// `NetherFortressPieces.StartPiece` output.
#[derive(Debug, Clone, PartialEq)]
pub struct FortressStart {
    pieces: Vec<FortressPiece>,
}

impl FortressStart {
    pub fn pieces(&self) -> &[FortressPiece] {
        &self.pieces
    }

    /// `StructureStart.getBoundingBox` before terrain adaptation.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        BoundingBox::encapsulating(self.pieces.iter().map(FortressPiece::bounding_box))
    }

    /// Places every intersecting piece into one chunk/region box.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        let mut result = StartPlacement::default();
        for piece in &self.pieces {
            if !piece.bounding_box.intersects(chunk_box) {
                result.skipped_pieces += 1;
                continue;
            }
            let placement = piece.place(level, chunk_box, random)?;
            result.placed_pieces += 1;
            result.written += placement.written;
            result.rejected += placement.rejected;
            result.written_blocks.extend(placement.written_blocks);
            result.block_entities.extend(placement.block_entities);
            result.entities.extend(placement.entities);
            result.data_markers.extend(placement.data_markers);
        }
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }

    /// `NetherFortressStructure.generatePieces` + `moveInsideHeights`.
    pub fn generate(seed: i64, chunk_x: i32, chunk_z: i32) -> Self {
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(&mut random, seed, chunk_x, chunk_z);
        let mut builder = FortressBuilder::new();
        builder.run(chunk_x, chunk_z, &mut random);
        builder.finish(&mut random)
    }
}

struct FortressBuilder {
    pieces: Vec<FortressPiece>,
    pending: Vec<usize>,
    previous: Option<PieceClass>,
    bridge_weights: Vec<PieceWeight>,
    castle_weights: Vec<PieceWeight>,
    start_box: BoundingBox,
}

impl FortressBuilder {
    fn new() -> Self {
        Self {
            pieces: Vec::new(),
            pending: Vec::new(),
            previous: None,
            bridge_weights: bridge_piece_weights(),
            castle_weights: castle_piece_weights(),
            start_box: BoundingBox::new(0, 0, 0, 0, 0, 0),
        }
    }

    fn run(&mut self, chunk_x: i32, chunk_z: i32, random: &mut dyn RandomSource) {
        let directions = [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ];
        let orientation = directions[random.next_int_bound(4) as usize];
        let start_x = chunk_x * 16 + 2;
        let start_z = chunk_z * 16 + 2;
        self.start_box =
            make_bounding_box(start_x, MAGIC_START_Y, start_z, orientation, 19, 10, 19);
        self.pieces.push(FortressPiece::new(
            FortressPieceKind::Start,
            self.start_box,
            orientation,
            0,
        ));
        self.add_children(0, random);

        while !self.pending.is_empty() {
            let index = random.next_int_bound(self.pending.len() as i32) as usize;
            let piece = self.pending.remove(index);
            self.add_children(piece, random);
        }
    }

    fn finish(mut self, random: &mut dyn RandomSource) -> FortressStart {
        let bounds =
            BoundingBox::encapsulating(self.pieces.iter().map(FortressPiece::bounding_box))
                .unwrap_or(self.start_box);
        let available = 70 - 48 + 1 - bounds.y_span();
        let min_y = if available > 1 {
            48 + random.next_int_bound(available)
        } else {
            48
        };
        let offset = min_y - bounds.min_y();
        for piece in &mut self.pieces {
            piece.bounding_box = piece.bounding_box.moved(0, offset, 0);
        }
        FortressStart {
            pieces: self.pieces,
        }
    }

    fn find_collision(&self, box_: BoundingBox) -> Option<&FortressPiece> {
        self.pieces
            .iter()
            .find(|piece| piece.bounding_box.intersects(box_))
    }

    fn add_children(&mut self, index: usize, random: &mut dyn RandomSource) {
        let piece = self.pieces[index].clone();
        let box_ = piece.bounding_box;
        let orientation = piece.orientation;
        let gen_depth = piece.gen_depth;
        match piece.kind {
            FortressPieceKind::Start | FortressPieceKind::BridgeCrossing => {
                self.child_forward(box_, orientation, gen_depth, 8, 3, false, random);
                self.child_left(box_, orientation, gen_depth, 3, 8, false, random);
                self.child_right(box_, orientation, gen_depth, 3, 8, false, random);
            }
            FortressPieceKind::BridgeStraight => {
                self.child_forward(box_, orientation, gen_depth, 1, 3, false, random);
            }
            FortressPieceKind::BridgeEndFiller => {}
            FortressPieceKind::RoomCrossing => {
                self.child_forward(box_, orientation, gen_depth, 2, 0, false, random);
                self.child_left(box_, orientation, gen_depth, 0, 2, false, random);
                self.child_right(box_, orientation, gen_depth, 0, 2, false, random);
            }
            FortressPieceKind::StairsRoom => {
                self.child_right(box_, orientation, gen_depth, 6, 2, false, random);
            }
            FortressPieceKind::MonsterThrone => {}
            FortressPieceKind::CastleEntrance => {
                self.child_forward(box_, orientation, gen_depth, 5, 3, true, random);
            }
            FortressPieceKind::CastleSmallCorridor => {
                self.child_forward(box_, orientation, gen_depth, 1, 0, true, random);
            }
            FortressPieceKind::CastleSmallCorridorCrossing => {
                self.child_forward(box_, orientation, gen_depth, 1, 0, true, random);
                self.child_left(box_, orientation, gen_depth, 0, 1, true, random);
                self.child_right(box_, orientation, gen_depth, 0, 1, true, random);
            }
            FortressPieceKind::CastleSmallCorridorLeftTurn => {
                self.child_left(box_, orientation, gen_depth, 0, 1, true, random);
            }
            FortressPieceKind::CastleSmallCorridorRightTurn => {
                self.child_right(box_, orientation, gen_depth, 0, 1, true, random);
            }
            FortressPieceKind::CastleCorridorStairs => {
                self.child_forward(box_, orientation, gen_depth, 1, 0, true, random);
            }
            FortressPieceKind::CastleCorridorTBalcony => {
                let offset = if matches!(orientation, Direction::West | Direction::North) {
                    5
                } else {
                    1
                };
                self.child_left(
                    box_,
                    orientation,
                    gen_depth,
                    0,
                    offset,
                    random.next_int_bound(8) > 0,
                    random,
                );
                self.child_right(
                    box_,
                    orientation,
                    gen_depth,
                    0,
                    offset,
                    random.next_int_bound(8) > 0,
                    random,
                );
            }
            FortressPieceKind::CastleStalkRoom => {
                self.child_forward(box_, orientation, gen_depth, 5, 3, true, random);
                self.child_forward(box_, orientation, gen_depth, 5, 11, true, random);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn child_forward(
        &mut self,
        box_: BoundingBox,
        orientation: Direction,
        gen_depth: i32,
        offset_x: i32,
        offset_y: i32,
        castle: bool,
        random: &mut dyn RandomSource,
    ) -> Option<usize> {
        let (x, z) = match orientation {
            Direction::North => (box_.min_x() + offset_x, box_.min_z() - 1),
            Direction::South => (box_.min_x() + offset_x, box_.max_z() + 1),
            Direction::West => (box_.min_x() - 1, box_.min_z() + offset_x),
            Direction::East => (box_.max_x() + 1, box_.min_z() + offset_x),
            _ => return None,
        };
        self.generate_and_add(
            x,
            box_.min_y() + offset_y,
            z,
            orientation,
            gen_depth,
            castle,
            random,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn child_left(
        &mut self,
        box_: BoundingBox,
        orientation: Direction,
        gen_depth: i32,
        offset_y: i32,
        offset_x: i32,
        castle: bool,
        random: &mut dyn RandomSource,
    ) -> Option<usize> {
        let (x, z, direction) = match orientation {
            Direction::North => (box_.min_x() - 1, box_.min_z() + offset_x, Direction::West),
            Direction::South => (box_.min_x() - 1, box_.min_z() + offset_x, Direction::West),
            Direction::West => (box_.min_x() + offset_x, box_.min_z() - 1, Direction::North),
            Direction::East => (box_.min_x() + offset_x, box_.min_z() - 1, Direction::North),
            _ => return None,
        };
        self.generate_and_add(
            x,
            box_.min_y() + offset_y,
            z,
            direction,
            gen_depth,
            castle,
            random,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn child_right(
        &mut self,
        box_: BoundingBox,
        orientation: Direction,
        gen_depth: i32,
        offset_y: i32,
        offset_x: i32,
        castle: bool,
        random: &mut dyn RandomSource,
    ) -> Option<usize> {
        let (x, z, direction) = match orientation {
            Direction::North => (box_.max_x() + 1, box_.min_z() + offset_x, Direction::East),
            Direction::South => (box_.max_x() + 1, box_.min_z() + offset_x, Direction::East),
            Direction::West => (box_.min_x() + offset_x, box_.max_z() + 1, Direction::South),
            Direction::East => (box_.min_x() + offset_x, box_.max_z() + 1, Direction::South),
            _ => return None,
        };
        self.generate_and_add(
            x,
            box_.min_y() + offset_y,
            z,
            direction,
            gen_depth,
            castle,
            random,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_and_add(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        orientation: Direction,
        gen_depth: i32,
        castle: bool,
        random: &mut dyn RandomSource,
    ) -> Option<usize> {
        if (x - self.start_box.min_x()).abs() <= MAX_PIECE_DISTANCE
            && (z - self.start_box.min_z()).abs() <= MAX_PIECE_DISTANCE
        {
            let weights = if castle {
                self.castle_weights.clone()
            } else {
                self.bridge_weights.clone()
            };
            let piece = self.generate_piece(weights, x, y, z, orientation, gen_depth + 1, random);
            if let Some(piece) = piece {
                let index = self.pieces.len();
                self.pieces.push(piece);
                self.pending.push(index);
                return Some(index);
            }
            None
        } else {
            let piece = self.create_bridge_end_filler(x, y, z, orientation, gen_depth, random)?;
            let _ = piece;
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_piece(
        &mut self,
        mut weights: Vec<PieceWeight>,
        x: i32,
        y: i32,
        z: i32,
        orientation: Direction,
        gen_depth: i32,
        random: &mut dyn RandomSource,
    ) -> Option<FortressPiece> {
        let mut total = 0;
        let mut available = false;
        for weight in &weights {
            if weight.max_place_count > 0 && weight.place_count < weight.max_place_count {
                available = true;
            }
            total += weight.weight;
        }
        let mut can_place = total > 0 && available && gen_depth <= MAX_DEPTH;
        let mut attempts = 0;
        while attempts < 5 && can_place {
            attempts += 1;
            let mut draw = random.next_int_bound(total);
            let mut chosen = None;
            for (index, weight) in weights.iter().enumerate() {
                draw -= weight.weight;
                if draw < 0 {
                    if !weight.do_place(gen_depth)
                        || (self.previous == Some(weight.class) && !weight.allow_in_row)
                    {
                        break;
                    }
                    chosen = Some(index);
                    break;
                }
            }
            let Some(index) = chosen else {
                continue;
            };
            let class = weights[index].class;
            if let Some(piece) = self.create_piece(class, x, y, z, orientation, gen_depth, random) {
                weights[index].place_count += 1;
                let count = weights[index].place_count;
                let max = weights[index].max_place_count;
                let still_valid = max == 0 || count < max;
                self.previous = Some(class);
                if class.is_bridge() {
                    self.bridge_weights = weights;
                } else {
                    self.castle_weights = weights;
                }
                if !still_valid {
                    // The caller keeps a local list only; prune here as well so
                    // the next child draw sees Java's updated table.
                    self.prune_weight(class);
                }
                return Some(piece);
            }
            // A colliding/out-of-range candidate falls through to the next
            // attempt exactly like Java's five-iteration retry loop.
            can_place = attempts < 5;
        }
        self.create_bridge_end_filler(x, y, z, orientation, gen_depth, random)
    }

    fn prune_weight(&mut self, class: PieceClass) {
        if class.is_bridge() {
            self.bridge_weights.retain(|weight| weight.class != class);
        } else {
            self.castle_weights.retain(|weight| weight.class != class);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_piece(
        &self,
        class: PieceClass,
        x: i32,
        y: i32,
        z: i32,
        orientation: Direction,
        gen_depth: i32,
        random: &mut dyn RandomSource,
    ) -> Option<FortressPiece> {
        let (kind, bounding_box) = match class {
            PieceClass::BridgeStraight => (
                FortressPieceKind::BridgeStraight,
                orient_box(x, y, z, -1, -3, 0, 5, 10, 19, orientation),
            ),
            PieceClass::BridgeCrossing => (
                FortressPieceKind::BridgeCrossing,
                orient_box(x, y, z, -8, -3, 0, 19, 10, 19, orientation),
            ),
            PieceClass::RoomCrossing => (
                FortressPieceKind::RoomCrossing,
                orient_box(x, y, z, -2, 0, 0, 7, 9, 7, orientation),
            ),
            PieceClass::StairsRoom => (
                FortressPieceKind::StairsRoom,
                orient_box(x, y, z, -2, 0, 0, 7, 11, 7, orientation),
            ),
            PieceClass::MonsterThrone => (
                FortressPieceKind::MonsterThrone,
                orient_box(x, y, z, -2, 0, 0, 7, 8, 9, orientation),
            ),
            PieceClass::CastleEntrance => (
                FortressPieceKind::CastleEntrance,
                orient_box(x, y, z, -5, -3, 0, 13, 14, 13, orientation),
            ),
            PieceClass::CastleSmallCorridor => (
                FortressPieceKind::CastleSmallCorridor,
                orient_box(x, y, z, -1, 0, 0, 5, 7, 5, orientation),
            ),
            PieceClass::CastleSmallCorridorCrossing => (
                FortressPieceKind::CastleSmallCorridorCrossing,
                orient_box(x, y, z, -1, 0, 0, 5, 7, 5, orientation),
            ),
            PieceClass::CastleSmallCorridorRightTurn => (
                FortressPieceKind::CastleSmallCorridorRightTurn,
                orient_box(x, y, z, -1, 0, 0, 5, 7, 5, orientation),
            ),
            PieceClass::CastleSmallCorridorLeftTurn => (
                FortressPieceKind::CastleSmallCorridorLeftTurn,
                orient_box(x, y, z, -1, 0, 0, 5, 7, 5, orientation),
            ),
            PieceClass::CastleCorridorStairs => (
                FortressPieceKind::CastleCorridorStairs,
                orient_box(x, y, z, -1, -7, 0, 5, 14, 10, orientation),
            ),
            PieceClass::CastleCorridorTBalcony => (
                FortressPieceKind::CastleCorridorTBalcony,
                orient_box(x, y, z, -3, 0, 0, 9, 7, 9, orientation),
            ),
            PieceClass::CastleStalkRoom => (
                FortressPieceKind::CastleStalkRoom,
                orient_box(x, y, z, -5, -3, 0, 13, 14, 13, orientation),
            ),
        };
        if !is_ok_box(bounding_box) || self.find_collision(bounding_box).is_some() {
            return None;
        }
        let mut piece = FortressPiece::new(kind, bounding_box, orientation, gen_depth);
        if matches!(
            kind,
            FortressPieceKind::CastleSmallCorridorRightTurn
                | FortressPieceKind::CastleSmallCorridorLeftTurn
        ) {
            piece.needs_chest = random.next_int_bound(3) == 0;
        }
        Some(piece)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_bridge_end_filler(
        &self,
        x: i32,
        y: i32,
        z: i32,
        orientation: Direction,
        gen_depth: i32,
        random: &mut dyn RandomSource,
    ) -> Option<FortressPiece> {
        let bounding_box = orient_box(x, y, z, -1, -3, 0, 5, 10, 8, orientation);
        if !is_ok_box(bounding_box) || self.find_collision(bounding_box).is_some() {
            return None;
        }
        let mut piece = FortressPiece::new(
            FortressPieceKind::BridgeEndFiller,
            bounding_box,
            orientation,
            gen_depth,
        );
        piece.self_seed = random.next_int();
        Some(piece)
    }
}

/// `NetherFortressPieces.PieceWeight` class identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PieceClass {
    BridgeStraight,
    BridgeCrossing,
    RoomCrossing,
    StairsRoom,
    MonsterThrone,
    CastleEntrance,
    CastleSmallCorridor,
    CastleSmallCorridorCrossing,
    CastleSmallCorridorRightTurn,
    CastleSmallCorridorLeftTurn,
    CastleCorridorStairs,
    CastleCorridorTBalcony,
    CastleStalkRoom,
}

impl PieceClass {
    fn is_bridge(self) -> bool {
        matches!(
            self,
            PieceClass::BridgeStraight
                | PieceClass::BridgeCrossing
                | PieceClass::RoomCrossing
                | PieceClass::StairsRoom
                | PieceClass::MonsterThrone
                | PieceClass::CastleEntrance
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct PieceWeight {
    class: PieceClass,
    weight: i32,
    place_count: i32,
    max_place_count: i32,
    allow_in_row: bool,
}

impl PieceWeight {
    fn do_place(self, _gen_depth: i32) -> bool {
        self.max_place_count == 0 || self.place_count < self.max_place_count
    }
}

fn bridge_piece_weights() -> Vec<PieceWeight> {
    vec![
        PieceWeight {
            class: PieceClass::BridgeStraight,
            weight: 30,
            place_count: 0,
            max_place_count: 0,
            allow_in_row: true,
        },
        PieceWeight {
            class: PieceClass::BridgeCrossing,
            weight: 10,
            place_count: 0,
            max_place_count: 4,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::RoomCrossing,
            weight: 10,
            place_count: 0,
            max_place_count: 4,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::StairsRoom,
            weight: 10,
            place_count: 0,
            max_place_count: 3,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::MonsterThrone,
            weight: 5,
            place_count: 0,
            max_place_count: 2,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::CastleEntrance,
            weight: 5,
            place_count: 0,
            max_place_count: 1,
            allow_in_row: false,
        },
    ]
}

fn castle_piece_weights() -> Vec<PieceWeight> {
    vec![
        PieceWeight {
            class: PieceClass::CastleSmallCorridor,
            weight: 25,
            place_count: 0,
            max_place_count: 0,
            allow_in_row: true,
        },
        PieceWeight {
            class: PieceClass::CastleSmallCorridorCrossing,
            weight: 15,
            place_count: 0,
            max_place_count: 5,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::CastleSmallCorridorRightTurn,
            weight: 5,
            place_count: 0,
            max_place_count: 10,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::CastleSmallCorridorLeftTurn,
            weight: 5,
            place_count: 0,
            max_place_count: 10,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::CastleCorridorStairs,
            weight: 10,
            place_count: 0,
            max_place_count: 3,
            allow_in_row: true,
        },
        PieceWeight {
            class: PieceClass::CastleCorridorTBalcony,
            weight: 7,
            place_count: 0,
            max_place_count: 2,
            allow_in_row: false,
        },
        PieceWeight {
            class: PieceClass::CastleStalkRoom,
            weight: 5,
            place_count: 0,
            max_place_count: 2,
            allow_in_row: false,
        },
    ]
}

/// `NetherBridgePiece.isOkBox`.
pub fn is_ok_box(box_: BoundingBox) -> bool {
    box_.min_y() > LOWEST_Y_POSITION
}

/// `BoundingBox.orientBox`.
#[allow(clippy::too_many_arguments)]
pub fn orient_box(
    structure_min_x: i32,
    structure_min_y: i32,
    structure_min_z: i32,
    x_min: i32,
    y_min: i32,
    z_min: i32,
    x_max: i32,
    y_max: i32,
    z_max: i32,
    facing: Direction,
) -> BoundingBox {
    match facing {
        Direction::North => BoundingBox::new(
            structure_min_x + x_min,
            structure_min_y + y_min,
            structure_min_z - z_max + 1 + z_min,
            structure_min_x + x_max - 1 + x_min,
            structure_min_y + y_max - 1 + y_min,
            structure_min_z + z_min,
        ),
        Direction::West => BoundingBox::new(
            structure_min_x - z_max + 1 + z_min,
            structure_min_y + y_min,
            structure_min_z + x_min,
            structure_min_x + z_min,
            structure_min_y + y_max - 1 + y_min,
            structure_min_z + x_max - 1 + x_min,
        ),
        Direction::East => BoundingBox::new(
            structure_min_x + z_min,
            structure_min_y + y_min,
            structure_min_z + x_min,
            structure_min_x + z_max - 1 + z_min,
            structure_min_y + y_max - 1 + y_min,
            structure_min_z + x_max - 1 + x_min,
        ),
        _ => BoundingBox::new(
            structure_min_x + x_min,
            structure_min_y + y_min,
            structure_min_z + z_min,
            structure_min_x + x_max - 1 + x_min,
            structure_min_y + y_max - 1 + y_min,
            structure_min_z + z_max - 1 + z_min,
        ),
    }
}

struct FortressWriter<'a> {
    level: &'a mut dyn FeatureLevel,
    piece: &'a FortressPiece,
    chunk_box: BoundingBox,
    random: &'a mut dyn RandomSource,
    result: StartPlacement,
}

impl FortressWriter<'_> {
    fn world_pos(&self, x: i32, y: i32, z: i32) -> BlockPos {
        let (world_x, world_z) = match self.piece.orientation {
            Direction::North => (
                self.piece.bounding_box.min_x() + x,
                self.piece.bounding_box.max_z() - z,
            ),
            Direction::South => (
                self.piece.bounding_box.min_x() + x,
                self.piece.bounding_box.min_z() + z,
            ),
            Direction::West => (
                self.piece.bounding_box.max_x() - z,
                self.piece.bounding_box.min_z() + x,
            ),
            Direction::East => (
                self.piece.bounding_box.min_x() + z,
                self.piece.bounding_box.min_z() + x,
            ),
            _ => (x, z),
        };
        BlockPos::new(world_x, self.piece.bounding_box.min_y() + y, world_z)
    }

    fn place_block(&mut self, state: &BlockState, x: i32, y: i32, z: i32) {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return;
        }
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
        } else {
            self.result.rejected += 1;
        }
    }

    fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return true;
        }
        self.level
            .get_block(pos)
            .is_none_or(crate::chunk::is_air_state)
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_box(
        &mut self,
        min_x: i32,
        min_y: i32,
        min_z: i32,
        max_x: i32,
        max_y: i32,
        max_z: i32,
        boundary: &BlockState,
        inside: &BlockState,
        existing_only: bool,
    ) {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    if existing_only && self.is_air(x, y, z) {
                        continue;
                    }
                    let state = if x != min_x
                        && x != max_x
                        && y != min_y
                        && y != max_y
                        && z != min_z
                        && z != max_z
                    {
                        inside
                    } else {
                        boundary
                    };
                    self.place_block(state, x, y, z);
                }
            }
        }
    }

    fn fill_column_down(&mut self, state: &BlockState, x: i32, y: i32, z: i32) {
        let mut pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return;
        }
        let min_y = self.level.min_y();
        while pos.y() > min_y + 1
            && self
                .level
                .get_block(pos)
                .is_none_or(is_replaceable_by_structures)
        {
            if self.level.set_block(pos, state.clone()) {
                self.result.written += 1;
                self.result.written_blocks.push(pos);
            }
            pos = pos.below();
        }
    }

    fn create_chest(&mut self, x: i32, y: i32, z: i32) -> bool {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos)
            || self
                .level
                .get_block(pos)
                .is_some_and(|state| state.name == "minecraft:chest")
        {
            return false;
        }
        let state = reorient_chest(self.level, pos);
        let seed = self.random.next_long();
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            let nbt = chest_nbt("minecraft:chests/nether_bridge", seed);
            self.result.block_entities.push(PlacedBlock {
                pos,
                state,
                nbt: Some(nbt.clone()),
            });
            self.result.data_markers.push(StructureDataMarker {
                name: "nether_bridge_chest".to_owned(),
                pos,
                nbt: Some(nbt),
            });
            true
        } else {
            self.result.rejected += 1;
            false
        }
    }

    fn place_spawner(&mut self, x: i32, y: i32, z: i32) {
        let pos = self.world_pos(x, y, z);
        if !self.chunk_box.is_inside(pos) {
            return;
        }
        let state = state("minecraft:spawner");
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            let nbt = blaze_spawner_nbt();
            self.result.block_entities.push(PlacedBlock {
                pos,
                state,
                nbt: Some(nbt.clone()),
            });
            self.result.data_markers.push(StructureDataMarker {
                name: "blaze_spawner".to_owned(),
                pos,
                nbt: Some(nbt),
            });
        } else {
            self.result.rejected += 1;
        }
    }

    fn generate_body(&mut self) {
        match self.piece.kind {
            FortressPieceKind::Start | FortressPieceKind::BridgeCrossing => {
                self.bridge_crossing();
            }
            FortressPieceKind::BridgeEndFiller => self.bridge_end_filler(),
            FortressPieceKind::BridgeStraight => self.bridge_straight(),
            FortressPieceKind::RoomCrossing => self.room_crossing(),
            FortressPieceKind::StairsRoom => self.stairs_room(),
            FortressPieceKind::MonsterThrone => self.monster_throne(),
            FortressPieceKind::CastleEntrance => self.castle_entrance(),
            FortressPieceKind::CastleSmallCorridor => self.castle_small_corridor(false, false),
            FortressPieceKind::CastleSmallCorridorCrossing => self.castle_small_corridor_crossing(),
            FortressPieceKind::CastleSmallCorridorLeftTurn => self.castle_turn(true),
            FortressPieceKind::CastleSmallCorridorRightTurn => self.castle_turn(false),
            FortressPieceKind::CastleCorridorStairs => self.castle_corridor_stairs(),
            FortressPieceKind::CastleCorridorTBalcony => self.castle_t_balcony(),
            FortressPieceKind::CastleStalkRoom => self.castle_stalk_room(),
        }
    }

    fn bridge_crossing(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(7, 3, 0, 11, 4, 18, &bricks, &bricks, false);
        self.generate_box(0, 3, 7, 18, 4, 11, &bricks, &bricks, false);
        self.generate_box(8, 5, 0, 10, 7, 18, &air, &air, false);
        self.generate_box(0, 5, 8, 18, 7, 10, &air, &air, false);
        self.generate_box(7, 5, 0, 7, 5, 7, &bricks, &bricks, false);
        self.generate_box(7, 5, 11, 7, 5, 18, &bricks, &bricks, false);
        self.generate_box(11, 5, 0, 11, 5, 7, &bricks, &bricks, false);
        self.generate_box(11, 5, 11, 11, 5, 18, &bricks, &bricks, false);
        self.generate_box(0, 5, 7, 7, 5, 7, &bricks, &bricks, false);
        self.generate_box(11, 5, 7, 18, 5, 7, &bricks, &bricks, false);
        self.generate_box(0, 5, 11, 7, 5, 11, &bricks, &bricks, false);
        self.generate_box(11, 5, 11, 18, 5, 11, &bricks, &bricks, false);
        self.generate_box(7, 2, 0, 11, 2, 5, &bricks, &bricks, false);
        self.generate_box(7, 2, 13, 11, 2, 18, &bricks, &bricks, false);
        self.generate_box(7, 0, 0, 11, 1, 3, &bricks, &bricks, false);
        self.generate_box(7, 0, 15, 11, 1, 18, &bricks, &bricks, false);
        for x in 7..=11 {
            for z in 0..=2 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, x, -1, 18 - z);
            }
        }
        self.generate_box(0, 2, 7, 5, 2, 11, &bricks, &bricks, false);
        self.generate_box(13, 2, 7, 18, 2, 11, &bricks, &bricks, false);
        self.generate_box(0, 0, 7, 3, 1, 11, &bricks, &bricks, false);
        self.generate_box(15, 0, 7, 18, 1, 11, &bricks, &bricks, false);
        for x in 0..=2 {
            for z in 7..=11 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, 18 - x, -1, z);
            }
        }
    }

    fn bridge_end_filler(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let mut random = LegacyRandomSource::new(i64::from(self.piece.self_seed));
        for x in 0..=4 {
            for y in 3..=4 {
                let end = random.next_int_bound(8);
                self.generate_box(x, y, 0, x, y, end, &bricks, &bricks, false);
            }
        }
        let end = random.next_int_bound(8);
        self.generate_box(0, 5, 0, 0, 5, end, &bricks, &bricks, false);
        let end = random.next_int_bound(8);
        self.generate_box(4, 5, 0, 4, 5, end, &bricks, &bricks, false);
        for x in 0..=4 {
            let end = random.next_int_bound(5);
            self.generate_box(x, 2, 0, x, 2, end, &bricks, &bricks, false);
        }
        for x in 0..=4 {
            for y in 0..=1 {
                let end = random.next_int_bound(3);
                self.generate_box(x, y, 0, x, y, end, &bricks, &bricks, false);
            }
        }
    }

    fn bridge_straight(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 3, 0, 4, 4, 18, &bricks, &bricks, false);
        self.generate_box(1, 5, 0, 3, 7, 18, &air, &air, false);
        self.generate_box(0, 5, 0, 0, 5, 18, &bricks, &bricks, false);
        self.generate_box(4, 5, 0, 4, 5, 18, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 4, 2, 5, &bricks, &bricks, false);
        self.generate_box(0, 2, 13, 4, 2, 18, &bricks, &bricks, false);
        self.generate_box(0, 0, 0, 4, 1, 3, &bricks, &bricks, false);
        self.generate_box(0, 0, 15, 4, 1, 18, &bricks, &bricks, false);
        for x in 0..=4 {
            for z in 0..=2 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, x, -1, 18 - z);
            }
        }
        let east = nether_fence(true, false, false, false);
        let west = nether_fence(false, false, false, true);
        self.generate_box(0, 1, 1, 0, 4, 1, &east, &east, false);
        self.generate_box(0, 3, 4, 0, 4, 4, &east, &east, false);
        self.generate_box(0, 3, 14, 0, 4, 14, &east, &east, false);
        self.generate_box(0, 1, 17, 0, 4, 17, &east, &east, false);
        self.generate_box(4, 1, 1, 4, 4, 1, &west, &west, false);
        self.generate_box(4, 3, 4, 4, 4, 4, &west, &west, false);
        self.generate_box(4, 3, 14, 4, 4, 14, &west, &west, false);
        self.generate_box(4, 1, 17, 4, 4, 17, &west, &west, false);
    }

    fn room_crossing(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 0, 0, 6, 1, 6, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 6, 7, 6, &air, &air, false);
        self.generate_box(0, 2, 0, 1, 6, 0, &bricks, &bricks, false);
        self.generate_box(0, 2, 6, 1, 6, 6, &bricks, &bricks, false);
        self.generate_box(5, 2, 0, 6, 6, 0, &bricks, &bricks, false);
        self.generate_box(5, 2, 6, 6, 6, 6, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 0, 6, 1, &bricks, &bricks, false);
        self.generate_box(0, 2, 5, 0, 6, 6, &bricks, &bricks, false);
        self.generate_box(6, 2, 0, 6, 6, 1, &bricks, &bricks, false);
        self.generate_box(6, 2, 5, 6, 6, 6, &bricks, &bricks, false);
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        self.generate_box(2, 6, 0, 4, 6, 0, &bricks, &bricks, false);
        self.generate_box(2, 5, 0, 4, 5, 0, &east_west, &east_west, false);
        self.generate_box(2, 6, 6, 4, 6, 6, &bricks, &bricks, false);
        self.generate_box(2, 5, 6, 4, 5, 6, &east_west, &east_west, false);
        self.generate_box(0, 6, 2, 0, 6, 4, &bricks, &bricks, false);
        self.generate_box(0, 5, 2, 0, 5, 4, &north_south, &north_south, false);
        self.generate_box(6, 6, 2, 6, 6, 4, &bricks, &bricks, false);
        self.generate_box(6, 5, 2, 6, 5, 4, &north_south, &north_south, false);
        for x in 0..=6 {
            for z in 0..=6 {
                self.fill_column_down(&bricks, x, -1, z);
            }
        }
    }

    fn stairs_room(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 0, 0, 6, 1, 6, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 6, 10, 6, &air, &air, false);
        self.generate_box(0, 2, 0, 1, 8, 0, &bricks, &bricks, false);
        self.generate_box(5, 2, 0, 6, 8, 0, &bricks, &bricks, false);
        self.generate_box(0, 2, 1, 0, 8, 6, &bricks, &bricks, false);
        self.generate_box(6, 2, 1, 6, 8, 6, &bricks, &bricks, false);
        self.generate_box(1, 2, 6, 5, 8, 6, &bricks, &bricks, false);
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        self.generate_box(0, 3, 2, 0, 5, 4, &north_south, &north_south, false);
        self.generate_box(6, 3, 2, 6, 5, 2, &north_south, &north_south, false);
        self.generate_box(6, 3, 4, 6, 5, 4, &north_south, &north_south, false);
        self.place_block(&bricks, 5, 2, 5);
        self.generate_box(4, 2, 5, 4, 3, 5, &bricks, &bricks, false);
        self.generate_box(3, 2, 5, 3, 4, 5, &bricks, &bricks, false);
        self.generate_box(2, 2, 5, 2, 5, 5, &bricks, &bricks, false);
        self.generate_box(1, 2, 5, 1, 6, 5, &bricks, &bricks, false);
        self.generate_box(1, 7, 1, 5, 7, 4, &bricks, &bricks, false);
        self.generate_box(6, 8, 2, 6, 8, 4, &air, &air, false);
        self.generate_box(2, 6, 0, 4, 8, 0, &bricks, &bricks, false);
        self.generate_box(2, 5, 0, 4, 5, 0, &east_west, &east_west, false);
        for x in 0..=6 {
            for z in 0..=6 {
                self.fill_column_down(&bricks, x, -1, z);
            }
        }
    }

    fn monster_throne(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 2, 0, 6, 7, 7, &air, &air, false);
        self.generate_box(1, 0, 0, 5, 1, 7, &bricks, &bricks, false);
        self.generate_box(1, 2, 1, 5, 2, 7, &bricks, &bricks, false);
        self.generate_box(1, 3, 2, 5, 3, 7, &bricks, &bricks, false);
        self.generate_box(1, 4, 3, 5, 4, 7, &bricks, &bricks, false);
        self.generate_box(1, 2, 0, 1, 4, 2, &bricks, &bricks, false);
        self.generate_box(5, 2, 0, 5, 4, 2, &bricks, &bricks, false);
        self.generate_box(1, 5, 2, 1, 5, 3, &bricks, &bricks, false);
        self.generate_box(5, 5, 2, 5, 5, 3, &bricks, &bricks, false);
        self.generate_box(0, 5, 3, 0, 5, 8, &bricks, &bricks, false);
        self.generate_box(6, 5, 3, 6, 5, 8, &bricks, &bricks, false);
        self.generate_box(1, 5, 8, 5, 5, 8, &bricks, &bricks, false);
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        self.place_block(&nether_fence(false, false, false, true), 1, 6, 3);
        self.place_block(&nether_fence(true, false, false, false), 5, 6, 3);
        self.place_block(&nether_fence(true, true, false, false), 0, 6, 3);
        self.place_block(&nether_fence(false, true, false, true), 6, 6, 3);
        self.generate_box(0, 6, 4, 0, 6, 7, &north_south, &north_south, false);
        self.generate_box(6, 6, 4, 6, 6, 7, &north_south, &north_south, false);
        self.place_block(&nether_fence(true, false, true, false), 0, 6, 8);
        self.place_block(&nether_fence(false, false, true, true), 6, 6, 8);
        self.generate_box(1, 6, 8, 5, 6, 8, &east_west, &east_west, false);
        self.place_block(&nether_fence(true, false, false, false), 1, 7, 8);
        self.generate_box(2, 7, 8, 4, 7, 8, &east_west, &east_west, false);
        self.place_block(&nether_fence(false, false, false, true), 5, 7, 8);
        self.place_block(&nether_fence(true, false, false, false), 2, 8, 8);
        self.place_block(&east_west, 3, 8, 8);
        self.place_block(&nether_fence(false, false, false, true), 4, 8, 8);
        if !self.piece.has_placed_spawner {
            let pos = self.world_pos(3, 5, 5);
            if self.chunk_box.is_inside(pos) {
                self.place_spawner(3, 5, 5);
            }
        }
        for x in 0..=6 {
            for z in 0..=6 {
                self.fill_column_down(&bricks, x, -1, z);
            }
        }
    }

    fn castle_entrance(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 3, 0, 12, 4, 12, &bricks, &bricks, false);
        self.generate_box(0, 5, 0, 12, 13, 12, &air, &air, false);
        self.generate_box(0, 5, 0, 1, 12, 12, &bricks, &bricks, false);
        self.generate_box(11, 5, 0, 12, 12, 12, &bricks, &bricks, false);
        self.generate_box(2, 5, 11, 4, 12, 12, &bricks, &bricks, false);
        self.generate_box(8, 5, 11, 10, 12, 12, &bricks, &bricks, false);
        self.generate_box(5, 9, 11, 7, 12, 12, &bricks, &bricks, false);
        self.generate_box(2, 5, 0, 4, 12, 1, &bricks, &bricks, false);
        self.generate_box(8, 5, 0, 10, 12, 1, &bricks, &bricks, false);
        self.generate_box(5, 9, 0, 7, 12, 1, &bricks, &bricks, false);
        self.generate_box(2, 11, 2, 10, 12, 10, &bricks, &bricks, false);
        self.generate_box(
            5,
            8,
            0,
            7,
            8,
            0,
            &state("minecraft:nether_brick_fence"),
            &state("minecraft:nether_brick_fence"),
            false,
        );
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        for i in (1..=11).step_by(2) {
            self.generate_box(i, 10, 0, i, 11, 0, &east_west, &east_west, false);
            self.generate_box(i, 10, 12, i, 11, 12, &east_west, &east_west, false);
            self.generate_box(0, 10, i, 0, 11, i, &north_south, &north_south, false);
            self.generate_box(12, 10, i, 12, 11, i, &north_south, &north_south, false);
            self.place_block(&bricks, i, 13, 0);
            self.place_block(&bricks, i, 13, 12);
            self.place_block(&bricks, 0, 13, i);
            self.place_block(&bricks, 12, 13, i);
            if i != 11 {
                self.place_block(&east_west, i + 1, 13, 0);
                self.place_block(&east_west, i + 1, 13, 12);
                self.place_block(&north_south, 0, 13, i + 1);
                self.place_block(&north_south, 12, 13, i + 1);
            }
        }
        self.place_block(&nether_fence(true, true, false, false), 0, 13, 0);
        self.place_block(&nether_fence(true, false, true, false), 0, 13, 12);
        self.place_block(&nether_fence(false, false, true, true), 12, 13, 12);
        self.place_block(&nether_fence(false, true, false, true), 12, 13, 0);
        for i in (3..=9).step_by(2) {
            self.generate_box(1, 7, i, 1, 8, i, &north_south, &north_south, false);
            self.generate_box(11, 7, i, 11, 8, i, &north_south, &north_south, false);
        }
        self.generate_box(4, 2, 0, 8, 2, 12, &bricks, &bricks, false);
        self.generate_box(0, 2, 4, 12, 2, 8, &bricks, &bricks, false);
        self.generate_box(4, 0, 0, 8, 1, 3, &bricks, &bricks, false);
        self.generate_box(4, 0, 9, 8, 1, 12, &bricks, &bricks, false);
        self.generate_box(0, 0, 4, 3, 1, 8, &bricks, &bricks, false);
        self.generate_box(9, 0, 4, 12, 1, 8, &bricks, &bricks, false);
        for x in 4..=8 {
            for z in 0..=2 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, x, -1, 12 - z);
            }
        }
        for x in 0..=2 {
            for z in 4..=8 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, 12 - x, -1, z);
            }
        }
        self.generate_box(5, 5, 5, 7, 5, 7, &bricks, &bricks, false);
        self.generate_box(6, 1, 6, 6, 4, 6, &air, &air, false);
        self.place_block(&bricks, 6, 0, 6);
        self.place_block(&state("minecraft:lava"), 6, 5, 6);
    }

    fn castle_small_corridor(&mut self, left_turn: bool, right_turn: bool) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 0, 0, 4, 1, 4, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 4, 5, 4, &air, &air, false);
        if left_turn {
            self.generate_box(4, 2, 0, 4, 5, 4, &bricks, &bricks, false);
            self.generate_box(
                4,
                3,
                1,
                4,
                4,
                1,
                &nether_fence(false, true, false, false),
                &nether_fence(false, true, false, false),
                false,
            );
            self.generate_box(
                4,
                3,
                3,
                4,
                4,
                3,
                &nether_fence(false, true, false, false),
                &nether_fence(false, true, false, false),
                false,
            );
            self.generate_box(0, 2, 0, 0, 5, 0, &bricks, &bricks, false);
            self.generate_box(0, 2, 4, 3, 5, 4, &bricks, &bricks, false);
            self.generate_box(
                1,
                3,
                4,
                1,
                4,
                4,
                &nether_fence(true, false, false, true),
                &nether_fence(true, false, false, true),
                false,
            );
            self.generate_box(
                3,
                3,
                4,
                3,
                4,
                4,
                &nether_fence(true, false, false, true),
                &nether_fence(true, false, false, true),
                false,
            );
            if self.piece.needs_chest {
                let pos = self.world_pos(3, 2, 3);
                if self.chunk_box.is_inside(pos) {
                    let _ = self.create_chest(3, 2, 3);
                }
            }
        } else if right_turn {
            let north_south = nether_fence(false, true, false, false);
            let east_west = nether_fence(true, false, false, true);
            self.generate_box(0, 2, 0, 0, 5, 4, &bricks, &bricks, false);
            self.generate_box(0, 3, 1, 0, 4, 1, &north_south, &north_south, false);
            self.generate_box(0, 3, 3, 0, 4, 3, &north_south, &north_south, false);
            self.generate_box(4, 2, 0, 4, 5, 0, &bricks, &bricks, false);
            self.generate_box(1, 2, 4, 4, 5, 4, &bricks, &bricks, false);
            self.generate_box(1, 3, 4, 1, 4, 4, &east_west, &east_west, false);
            self.generate_box(3, 3, 4, 3, 4, 4, &east_west, &east_west, false);
            if self.piece.needs_chest {
                let pos = self.world_pos(1, 2, 3);
                if self.chunk_box.is_inside(pos) {
                    let _ = self.create_chest(1, 2, 3);
                }
            }
        } else {
            let north_south = nether_fence(false, true, false, false);
            self.generate_box(0, 2, 0, 0, 5, 4, &bricks, &bricks, false);
            self.generate_box(4, 2, 0, 4, 5, 4, &bricks, &bricks, false);
            self.generate_box(0, 3, 1, 0, 4, 1, &north_south, &north_south, false);
            self.generate_box(0, 3, 3, 0, 4, 3, &north_south, &north_south, false);
            self.generate_box(4, 3, 1, 4, 4, 1, &north_south, &north_south, false);
            self.generate_box(4, 3, 3, 4, 4, 3, &north_south, &north_south, false);
        }
        self.generate_box(0, 6, 0, 4, 6, 4, &bricks, &bricks, false);
        for x in 0..=4 {
            for z in 0..=4 {
                self.fill_column_down(&bricks, x, -1, z);
            }
        }
    }

    fn castle_small_corridor_crossing(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 0, 0, 4, 1, 4, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 4, 5, 4, &air, &air, false);
        self.generate_box(0, 2, 0, 0, 5, 0, &bricks, &bricks, false);
        self.generate_box(4, 2, 0, 4, 5, 0, &bricks, &bricks, false);
        self.generate_box(0, 2, 4, 0, 5, 4, &bricks, &bricks, false);
        self.generate_box(4, 2, 4, 4, 5, 4, &bricks, &bricks, false);
        self.generate_box(0, 6, 0, 4, 6, 4, &bricks, &bricks, false);
        for x in 0..=4 {
            for z in 0..=4 {
                self.fill_column_down(&bricks, x, -1, z);
            }
        }
    }

    fn castle_turn(&mut self, left: bool) {
        self.castle_small_corridor(left, !left);
    }

    fn castle_corridor_stairs(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        let stairs = stairs(Direction::South);
        let north_south = nether_fence(false, true, false, false);
        for i in 0..=9 {
            let max = 1.max(7 - i);
            let min = (max + 5).max(14 - i).min(13);
            self.generate_box(0, 0, i, 4, max, i, &bricks, &bricks, false);
            self.generate_box(1, max + 1, i, 3, min - 1, i, &air, &air, false);
            if i <= 6 {
                self.place_block(&stairs, 1, max + 1, i);
                self.place_block(&stairs, 2, max + 1, i);
                self.place_block(&stairs, 3, max + 1, i);
            }
            self.generate_box(0, min, i, 4, min, i, &bricks, &bricks, false);
            self.generate_box(0, max + 1, i, 0, min - 1, i, &bricks, &bricks, false);
            self.generate_box(4, max + 1, i, 4, min - 1, i, &bricks, &bricks, false);
            if (i & 1) == 0 {
                self.generate_box(
                    0,
                    max + 2,
                    i,
                    0,
                    max + 3,
                    i,
                    &north_south,
                    &north_south,
                    false,
                );
                self.generate_box(
                    4,
                    max + 2,
                    i,
                    4,
                    max + 3,
                    i,
                    &north_south,
                    &north_south,
                    false,
                );
            }
            for x in 0..=4 {
                self.fill_column_down(&bricks, x, -1, i);
            }
        }
    }

    fn castle_t_balcony(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        self.generate_box(0, 0, 0, 8, 1, 8, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 8, 5, 8, &air, &air, false);
        self.generate_box(0, 6, 0, 8, 6, 5, &bricks, &bricks, false);
        self.generate_box(0, 2, 0, 2, 5, 0, &bricks, &bricks, false);
        self.generate_box(6, 2, 0, 8, 5, 0, &bricks, &bricks, false);
        self.generate_box(1, 3, 0, 1, 4, 0, &east_west, &east_west, false);
        self.generate_box(7, 3, 0, 7, 4, 0, &east_west, &east_west, false);
        self.generate_box(0, 2, 4, 8, 2, 8, &bricks, &bricks, false);
        self.generate_box(1, 1, 4, 2, 2, 4, &air, &air, false);
        self.generate_box(6, 1, 4, 7, 2, 4, &air, &air, false);
        self.generate_box(1, 3, 8, 7, 3, 8, &east_west, &east_west, false);
        self.place_block(&nether_fence(true, false, true, false), 0, 3, 8);
        self.place_block(&nether_fence(false, false, true, true), 8, 3, 8);
        self.generate_box(0, 3, 6, 0, 3, 7, &north_south, &north_south, false);
        self.generate_box(8, 3, 6, 8, 3, 7, &north_south, &north_south, false);
        self.generate_box(0, 3, 4, 0, 5, 5, &bricks, &bricks, false);
        self.generate_box(8, 3, 4, 8, 5, 5, &bricks, &bricks, false);
        self.generate_box(1, 3, 5, 2, 5, 5, &bricks, &bricks, false);
        self.generate_box(6, 3, 5, 7, 5, 5, &bricks, &bricks, false);
        self.generate_box(1, 4, 5, 1, 5, 5, &east_west, &east_west, false);
        self.generate_box(7, 4, 5, 7, 5, 5, &east_west, &east_west, false);
        for x in 0..=5 {
            for z in 0..=8 {
                self.fill_column_down(&bricks, z, -1, x);
            }
        }
    }

    fn castle_stalk_room(&mut self) {
        let bricks = state("minecraft:nether_bricks");
        let air = state("minecraft:air");
        self.generate_box(0, 3, 0, 12, 4, 12, &bricks, &bricks, false);
        self.generate_box(0, 5, 0, 12, 13, 12, &air, &air, false);
        self.generate_box(0, 5, 0, 1, 12, 12, &bricks, &bricks, false);
        self.generate_box(11, 5, 0, 12, 12, 12, &bricks, &bricks, false);
        self.generate_box(2, 5, 11, 4, 12, 12, &bricks, &bricks, false);
        self.generate_box(8, 5, 11, 10, 12, 12, &bricks, &bricks, false);
        self.generate_box(5, 9, 11, 7, 12, 12, &bricks, &bricks, false);
        self.generate_box(2, 5, 0, 4, 12, 1, &bricks, &bricks, false);
        self.generate_box(8, 5, 0, 10, 12, 1, &bricks, &bricks, false);
        self.generate_box(5, 9, 0, 7, 12, 1, &bricks, &bricks, false);
        self.generate_box(2, 11, 2, 10, 12, 10, &bricks, &bricks, false);
        let east_west = nether_fence(true, false, false, true);
        let north_south = nether_fence(false, true, false, false);
        let north_south_west = nether_fence(false, true, false, true);
        let north_south_east = nether_fence(true, true, false, false);
        for i in (1..=11).step_by(2) {
            self.generate_box(i, 10, 0, i, 11, 0, &east_west, &east_west, false);
            self.generate_box(i, 10, 12, i, 11, 12, &east_west, &east_west, false);
            self.generate_box(0, 10, i, 0, 11, i, &north_south, &north_south, false);
            self.generate_box(12, 10, i, 12, 11, i, &north_south, &north_south, false);
            self.place_block(&bricks, i, 13, 0);
            self.place_block(&bricks, i, 13, 12);
            self.place_block(&bricks, 0, 13, i);
            self.place_block(&bricks, 12, 13, i);
            if i != 11 {
                self.place_block(&east_west, i + 1, 13, 0);
                self.place_block(&east_west, i + 1, 13, 12);
                self.place_block(&north_south, 0, 13, i + 1);
                self.place_block(&north_south, 12, 13, i + 1);
            }
        }
        self.place_block(&nether_fence(true, true, false, false), 0, 13, 0);
        self.place_block(&nether_fence(true, false, true, false), 0, 13, 12);
        self.place_block(&nether_fence(false, false, true, true), 12, 13, 12);
        self.place_block(&nether_fence(false, true, false, true), 12, 13, 0);
        for i in (3..=9).step_by(2) {
            self.generate_box(
                1,
                7,
                i,
                1,
                8,
                i,
                &north_south_west,
                &north_south_west,
                false,
            );
            self.generate_box(
                11,
                7,
                i,
                11,
                8,
                i,
                &north_south_east,
                &north_south_east,
                false,
            );
        }
        let stairs_north = stairs(Direction::North);
        for i in 0..=6 {
            let z = i + 4;
            for x in 5..=7 {
                self.place_block(&stairs_north, x, 5 + i, z);
            }
            if (5..=8).contains(&z) {
                self.generate_box(5, 5, z, 7, i + 4, z, &bricks, &bricks, false);
            } else if (9..=10).contains(&z) {
                self.generate_box(5, 8, z, 7, i + 4, z, &bricks, &bricks, false);
            }
            if i >= 1 {
                self.generate_box(5, 6 + i, z, 7, 9 + i, z, &air, &air, false);
            }
        }
        for x in 5..=7 {
            self.place_block(&stairs_north, x, 12, 11);
        }
        self.generate_box(
            5,
            6,
            7,
            5,
            7,
            7,
            &north_south_east,
            &north_south_east,
            false,
        );
        self.generate_box(
            7,
            6,
            7,
            7,
            7,
            7,
            &north_south_west,
            &north_south_west,
            false,
        );
        self.generate_box(5, 13, 12, 7, 13, 12, &air, &air, false);
        self.generate_box(2, 5, 2, 3, 5, 3, &bricks, &bricks, false);
        self.generate_box(2, 5, 9, 3, 5, 10, &bricks, &bricks, false);
        self.generate_box(2, 5, 4, 2, 5, 8, &bricks, &bricks, false);
        self.generate_box(9, 5, 2, 10, 5, 3, &bricks, &bricks, false);
        self.generate_box(9, 5, 9, 10, 5, 10, &bricks, &bricks, false);
        self.generate_box(10, 5, 4, 10, 5, 8, &bricks, &bricks, false);
        let stairs_east = stairs(Direction::East);
        let stairs_west = stairs(Direction::West);
        self.place_block(&stairs_west, 4, 5, 2);
        self.place_block(&stairs_west, 4, 5, 3);
        self.place_block(&stairs_west, 4, 5, 9);
        self.place_block(&stairs_west, 4, 5, 10);
        self.place_block(&stairs_east, 8, 5, 2);
        self.place_block(&stairs_east, 8, 5, 3);
        self.place_block(&stairs_east, 8, 5, 9);
        self.place_block(&stairs_east, 8, 5, 10);
        self.generate_box(
            3,
            4,
            4,
            4,
            4,
            8,
            &state("minecraft:soul_sand"),
            &state("minecraft:soul_sand"),
            false,
        );
        self.generate_box(
            8,
            4,
            4,
            9,
            4,
            8,
            &state("minecraft:soul_sand"),
            &state("minecraft:soul_sand"),
            false,
        );
        self.generate_box(
            3,
            5,
            4,
            4,
            5,
            8,
            &state("minecraft:nether_wart"),
            &state("minecraft:nether_wart"),
            false,
        );
        self.generate_box(
            8,
            5,
            4,
            9,
            5,
            8,
            &state("minecraft:nether_wart"),
            &state("minecraft:nether_wart"),
            false,
        );
        self.generate_box(4, 2, 0, 8, 2, 12, &bricks, &bricks, false);
        self.generate_box(0, 2, 4, 12, 2, 8, &bricks, &bricks, false);
        self.generate_box(4, 0, 0, 8, 1, 3, &bricks, &bricks, false);
        self.generate_box(4, 0, 9, 8, 1, 12, &bricks, &bricks, false);
        self.generate_box(0, 0, 4, 3, 1, 8, &bricks, &bricks, false);
        self.generate_box(9, 0, 4, 12, 1, 8, &bricks, &bricks, false);
        for x in 4..=8 {
            for z in 0..=2 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, x, -1, 12 - z);
            }
        }
        for x in 0..=2 {
            for z in 4..=8 {
                self.fill_column_down(&bricks, x, -1, z);
                self.fill_column_down(&bricks, 12 - x, -1, z);
            }
        }
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn nether_fence(north: bool, east: bool, south: bool, west: bool) -> BlockState {
    BlockState {
        name: "minecraft:nether_brick_fence".to_owned(),
        properties: vec![
            ("east".to_owned(), bool_property(east)),
            ("north".to_owned(), bool_property(north)),
            ("south".to_owned(), bool_property(south)),
            ("waterlogged".to_owned(), "false".to_owned()),
            ("west".to_owned(), bool_property(west)),
        ],
    }
}

fn stairs(facing: Direction) -> BlockState {
    BlockState {
        name: "minecraft:nether_brick_stairs".to_owned(),
        properties: vec![(
            "facing".to_owned(),
            match facing {
                Direction::North => "north",
                Direction::East => "east",
                Direction::South => "south",
                Direction::West => "west",
                _ => "north",
            }
            .to_owned(),
        )],
    }
}

fn bool_property(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

fn is_replaceable_by_structures(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
            | "minecraft:glow_lichen"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    )
}

fn is_solid_render(state: &BlockState) -> bool {
    !is_replaceable_by_structures(state) && state.name != "minecraft:chest"
}

fn reorient_chest(level: &dyn FeatureLevel, pos: BlockPos) -> BlockState {
    let mut direction = None;
    for candidate in [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ] {
        let neighbour = pos.relative(candidate);
        let state = level.get_block(neighbour);
        if state.is_some_and(|state| state.name == "minecraft:chest") {
            return chest_state("north");
        }
        if state.is_some_and(is_solid_render) {
            if direction.is_some() {
                direction = None;
                break;
            }
            direction = Some(candidate);
        }
    }
    if let Some(direction) = direction {
        return chest_state(direction_name(direction.opposite()));
    }
    let mut facing = Direction::North;
    let mut neighbour = pos.relative(facing);
    if level.get_block(neighbour).is_some_and(is_solid_render) {
        facing = facing.opposite();
        neighbour = pos.relative(facing);
    }
    if level.get_block(neighbour).is_some_and(is_solid_render) {
        facing = clockwise(facing);
        neighbour = pos.relative(facing);
    }
    if level.get_block(neighbour).is_some_and(is_solid_render) {
        facing = facing.opposite();
    }
    chest_state(direction_name(facing))
}

fn chest_state(facing: &str) -> BlockState {
    BlockState {
        name: "minecraft:chest".to_owned(),
        properties: vec![("facing".to_owned(), facing.to_owned())],
    }
}

fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        _ => direction,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::East => "east",
        Direction::South => "south",
        Direction::West => "west",
        _ => "north",
    }
}

fn chest_nbt(loot_table: &str, seed: i64) -> Tag {
    Tag::Compound(vec![
        ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
        ("LootTable".to_owned(), Tag::String(loot_table.to_owned())),
        ("LootTableSeed".to_owned(), Tag::Long(seed)),
    ])
}

fn blaze_spawner_nbt() -> Tag {
    let blaze = Tag::Compound(vec![(
        "id".to_owned(),
        Tag::String("minecraft:blaze".to_owned()),
    )]);
    Tag::Compound(vec![
        (
            "id".to_owned(),
            Tag::String("minecraft:mob_spawner".to_owned()),
        ),
        (
            "SpawnData".to_owned(),
            Tag::Compound(vec![("entity".to_owned(), blaze.clone())]),
        ),
        (
            "SpawnPotentials".to_owned(),
            Tag::List(vec![Tag::Compound(vec![
                ("data".to_owned(), blaze),
                ("weight".to_owned(), Tag::Int(1)),
            ])]),
        ),
        ("Delay".to_owned(), Tag::Short(0)),
        ("MinSpawnDelay".to_owned(), Tag::Short(200)),
        ("MaxSpawnDelay".to_owned(), Tag::Short(800)),
        ("SpawnCount".to_owned(), Tag::Short(4)),
        ("MaxNearbyEntities".to_owned(), Tag::Short(6)),
        ("RequiredPlayerRange".to_owned(), Tag::Short(16)),
        ("SpawnRange".to_owned(), Tag::Short(4)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orient_box_matches_java_for_each_facing() {
        let south = orient_box(0, 64, 0, -1, -3, 0, 5, 10, 19, Direction::South);
        assert_eq!(south, BoundingBox::new(-1, 61, 0, 3, 70, 18));
        let north = orient_box(0, 64, 0, -1, -3, 0, 5, 10, 19, Direction::North);
        assert_eq!(north, BoundingBox::new(-1, 61, -18, 3, 70, 0));
        let west = orient_box(0, 64, 0, -1, -3, 0, 5, 10, 19, Direction::West);
        assert_eq!(west, BoundingBox::new(-18, 61, -1, 0, 70, 3));
        let east = orient_box(0, 64, 0, -1, -3, 0, 5, 10, 19, Direction::East);
        assert_eq!(east, BoundingBox::new(0, 61, -1, 18, 70, 3));
    }

    #[test]
    fn fixed_seed_generates_multiple_pieces_and_moves_inside_heights() {
        let first = FortressStart::generate(12_345, 6, -32);
        let second = FortressStart::generate(12_345, 6, -32);
        assert_eq!(first, second);
        assert!(first.pieces().len() > 1);
        let bounds = first.bounding_box().expect("fortress bounds");
        assert!(bounds.min_y() >= 48);
        if bounds.y_span() <= 23 {
            assert!(bounds.max_y() <= 70);
        } else {
            assert_eq!(bounds.min_y(), 48);
        }
        assert!(
            first
                .pieces()
                .iter()
                .any(|piece| piece.kind == FortressPieceKind::BridgeStraight)
        );
    }
}
