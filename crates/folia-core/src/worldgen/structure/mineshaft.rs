//! Deterministic procedural abandoned mineshafts.
//!
//! This is a documented approximation rather than a Java block-for-block port.
//! A start has a 9x9 room and a seeded spine of five to seven corridors,
//! crossings, or stairs, with two to three sections branching from each
//! crossing. Corridors are 5..=11 blocks long; roughly one third use the
//! five-wide fence-and-beam support profile. The planned spine includes both a
//! crossing and a staircase, so fixed seeds exercise connections and height
//! changes. Interior overlap rejects branches, and every write is clipped to
//! the requested chunk box.
//!
//! Start Y is sampled 24..=47 blocks below `WORLD_SURFACE_WG`, then moved at
//! most 12 blocks to the nearest position where at least three of five final
//! terrain-density samples are non-positive. This keeps starts in cave-like
//! space without requiring the post-carver block world. If a level cannot
//! expose density, the deterministic surface-relative target is retained.
//! Normal mineshafts use oak supports/planks with cobblestone, mossy
//! cobblestone, rails, torches, cobwebs, and `abandoned_mineshaft` chest markers.
//! Mesa mineshafts replace oak with dark oak and add terracotta/red terracotta
//! adobe-style accents.
//!
//! Approximate proportions: about 55% of eligible sections carry rails, 20%
//! carry a chest, and 45% carry one or two cobweb pockets. Exact Java piece
//! weights/RNG order, chest minecarts and spawners, lava pockets, terrain
//! adaptation, and a Java differential fixture remain open.

use crate::chunk::BlockState;
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::BoundingBox;
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::PlacedBlock;
use folia_math::{BlockPos, Direction};
use folia_random::{LegacyRandomSource, RandomSource};

pub const ROOM_SIZE: i32 = 9;
pub const CORRIDOR_HEIGHT: i32 = 5;

const MIN_SPINE_SECTIONS: i32 = 5;
const SPINE_VARIATION: i32 = 3;
const MIN_BRANCH_SECTIONS: i32 = 2;
const BRANCH_VARIATION: i32 = 2;
const SURFACE_DEPTH: i32 = 24;
const DEPTH_VARIATION: i32 = 24;
const DENSITY_SEARCH_RADIUS: i32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MineshaftSectionKind {
    Room,
    Corridor,
    Crossing,
    Stairs,
}

impl MineshaftSectionKind {
    pub fn is_room(self) -> bool {
        matches!(self, Self::Room)
    }

    pub fn is_crossing(self) -> bool {
        matches!(self, Self::Crossing)
    }

    pub fn is_stairs(self) -> bool {
        matches!(self, Self::Stairs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MineshaftRailKind {
    Normal,
    Powered,
    Detector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MineshaftSection {
    kind: MineshaftSectionKind,
    origin: BlockPos,
    direction: Direction,
    width: i32,
    length: i32,
    support: bool,
    y_step: i32,
    rail_kind: Option<MineshaftRailKind>,
    cobwebs: i32,
    torches: i32,
    chest: bool,
    chest_side: bool,
    chest_seed: i64,
}

impl MineshaftSection {
    pub fn kind(&self) -> MineshaftSectionKind {
        self.kind
    }

    pub fn origin(&self) -> BlockPos {
        self.origin
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn length(&self) -> i32 {
        self.length
    }

    pub fn support(&self) -> bool {
        self.support
    }

    pub fn is_stairs(&self) -> bool {
        self.kind.is_stairs()
    }

    pub fn y_step(&self) -> i32 {
        self.y_step
    }

    pub fn rail_kind(&self) -> Option<MineshaftRailKind> {
        self.rail_kind
    }

    pub fn cobwebs(&self) -> i32 {
        self.cobwebs
    }

    pub fn torches(&self) -> i32 {
        self.torches
    }

    pub fn has_chest(&self) -> bool {
        self.chest
    }

    pub fn chest_seed(&self) -> i64 {
        self.chest_seed
    }

    pub fn floor_y(&self, along: i32) -> i32 {
        let divisor = (self.length - 1).max(1);
        self.origin.y() + (self.y_step * along) / divisor
    }

    fn bounding_box(&self) -> BoundingBox {
        let (min_x, max_x, min_z, max_z) =
            footprint(self.origin, self.direction, self.width, self.length);
        let min_y = self.origin.y().min(self.origin.y() + self.y_step);
        BoundingBox::new(
            min_x,
            min_y,
            min_z,
            max_x,
            min_y + CORRIDOR_HEIGHT - 1,
            max_z,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MineshaftPiece {
    seed: i64,
    mesa: bool,
    sections: Vec<MineshaftSection>,
    bounding_box: BoundingBox,
}

impl MineshaftPiece {
    pub fn new(seed: i64, x: i32, y: i32, z: i32, mesa: bool) -> Self {
        let mut random = LegacyRandomSource::new(piece_seed(seed, x, z));
        Self::from_random(seed, BlockPos::new(x, y, z), mesa, -64, 319, &mut random)
    }

    pub(crate) fn from_random(
        seed: i64,
        origin: BlockPos,
        mesa: bool,
        min_y: i32,
        max_y: i32,
        random: &mut impl RandomSource,
    ) -> Self {
        let direction = horizontal_direction(random.next_int_bound(4));
        let root = MineshaftSection {
            kind: MineshaftSectionKind::Room,
            origin,
            direction,
            width: ROOM_SIZE,
            length: ROOM_SIZE,
            support: true,
            y_step: 0,
            rail_kind: Some(MineshaftRailKind::Powered),
            cobwebs: 2 + random.next_int_bound(3),
            torches: 2,
            chest: true,
            chest_side: random.next_boolean(),
            chest_seed: random.next_long(),
        };
        let root_bounds = root.bounding_box();
        let mut piece = Self {
            seed: piece_seed(seed, origin.x(), origin.z()),
            mesa,
            sections: vec![root],
            bounding_box: root_bounds,
        };
        piece.generate_spine(direction, min_y, max_y, random);
        piece
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn mesa(&self) -> bool {
        self.mesa
    }

    pub fn start_y(&self) -> i32 {
        self.sections[0].origin.y()
    }

    pub fn sections(&self) -> &[MineshaftSection] {
        &self.sections
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let mut writer = MineshaftWriter {
            piece: self,
            level,
            chunk_box,
            result: StartPlacement::default(),
        };
        for (index, section) in self.sections.iter().enumerate() {
            if !section.bounding_box().intersects(chunk_box) {
                continue;
            }
            writer.generate_section(index, section);
        }
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

    fn generate_spine(
        &mut self,
        mut direction: Direction,
        min_y: i32,
        max_y: i32,
        random: &mut impl RandomSource,
    ) {
        let depth = MIN_SPINE_SECTIONS + random.next_int_bound(SPINE_VARIATION);
        let mut parent = 0;
        for step in 0..depth {
            let kind = match step {
                1 | 4 => MineshaftSectionKind::Crossing,
                3 => MineshaftSectionKind::Stairs,
                _ => MineshaftSectionKind::Corridor,
            };
            let support = step == 0 || kind.is_crossing() || random.next_float() < 0.35;
            let width = if kind.is_crossing() {
                ROOM_SIZE
            } else if support {
                5
            } else {
                3
            };
            let length = if kind.is_crossing() {
                ROOM_SIZE
            } else if kind.is_stairs() {
                5
            } else {
                5 + random.next_int_bound(7)
            };
            let y_step = if kind.is_stairs() {
                let magnitude = 2 + random.next_int_bound(2);
                if random.next_boolean() {
                    magnitude
                } else {
                    -magnitude
                }
            } else if random.next_float() < 0.15 {
                if random.next_boolean() { 1 } else { -1 }
            } else {
                0
            };
            let rail_kind = if step == 0 {
                Some(MineshaftRailKind::Normal)
            } else {
                roll_rail(random, kind.is_crossing() || kind.is_stairs())
            };
            let section = MineshaftSection {
                kind,
                origin: self.attach_origin(parent, direction, width, length, y_step),
                direction,
                width,
                length,
                support,
                y_step,
                rail_kind,
                cobwebs: roll_cobwebs(random),
                torches: if support {
                    1 + random.next_int_bound(2)
                } else {
                    0
                },
                chest: random.next_float() < 0.2,
                chest_side: random.next_boolean(),
                chest_seed: random.next_long(),
            };
            let Some(index) = self.try_add(section, parent, min_y, max_y) else {
                let alternate = clockwise(direction);
                let section = MineshaftSection {
                    kind,
                    origin: self.attach_origin(parent, alternate, width, length, y_step),
                    direction: alternate,
                    width,
                    length,
                    support,
                    y_step,
                    rail_kind,
                    cobwebs: roll_cobwebs(random),
                    torches: if support {
                        1 + random.next_int_bound(2)
                    } else {
                        0
                    },
                    chest: random.next_float() < 0.2,
                    chest_side: random.next_boolean(),
                    chest_seed: random.next_long(),
                };
                let Some(alternate_index) = self.try_add(section, parent, min_y, max_y) else {
                    break;
                };
                parent = alternate_index;
                direction = alternate;
                if kind.is_crossing() {
                    self.generate_branch(parent, min_y, max_y, random);
                }
                continue;
            };
            parent = index;
            if self.sections[parent].kind.is_crossing() {
                self.generate_branch(parent, min_y, max_y, random);
            }
        }
    }

    fn generate_branch(
        &mut self,
        parent: usize,
        min_y: i32,
        max_y: i32,
        random: &mut impl RandomSource,
    ) {
        let count = MIN_BRANCH_SECTIONS + random.next_int_bound(BRANCH_VARIATION);
        let first_direction = if random.next_boolean() {
            clockwise(self.sections[parent].direction)
        } else {
            counter_clockwise(self.sections[parent].direction)
        };
        for preferred in [first_direction, first_direction.opposite()] {
            let mut branch_parent = parent;
            let mut direction = preferred;
            let mut attached = false;
            for step in 0..count {
                let kind = if step == 1 {
                    MineshaftSectionKind::Stairs
                } else {
                    MineshaftSectionKind::Corridor
                };
                let support = step == 0 || random.next_float() < 0.3;
                let width = if support { 5 } else { 3 };
                let length = if kind.is_stairs() {
                    5
                } else {
                    5 + random.next_int_bound(7)
                };
                let y_step = if kind.is_stairs() {
                    let magnitude = 2 + random.next_int_bound(2);
                    if random.next_boolean() {
                        magnitude
                    } else {
                        -magnitude
                    }
                } else if random.next_float() < 0.12 {
                    if random.next_boolean() { 1 } else { -1 }
                } else {
                    0
                };
                let section = MineshaftSection {
                    kind,
                    origin: self.attach_origin(branch_parent, direction, width, length, y_step),
                    direction,
                    width,
                    length,
                    support,
                    y_step,
                    rail_kind: roll_rail(random, kind.is_stairs()),
                    cobwebs: roll_cobwebs(random),
                    torches: if support { 1 } else { 0 },
                    chest: random.next_float() < 0.2,
                    chest_side: random.next_boolean(),
                    chest_seed: random.next_long(),
                };
                let Some(index) = self.try_add(section, branch_parent, min_y, max_y) else {
                    let alternate = clockwise(direction);
                    let section = MineshaftSection {
                        kind,
                        origin: self.attach_origin(branch_parent, alternate, width, length, y_step),
                        direction: alternate,
                        width,
                        length,
                        support,
                        y_step,
                        rail_kind: roll_rail(random, kind.is_stairs()),
                        cobwebs: roll_cobwebs(random),
                        torches: if support { 1 } else { 0 },
                        chest: random.next_float() < 0.2,
                        chest_side: random.next_boolean(),
                        chest_seed: random.next_long(),
                    };
                    let Some(index) = self.try_add(section, branch_parent, min_y, max_y) else {
                        break;
                    };
                    branch_parent = index;
                    direction = alternate;
                    attached = true;
                    continue;
                };
                branch_parent = index;
                attached = true;
            }
            if attached {
                return;
            }
        }
    }

    fn attach_origin(
        &self,
        parent: usize,
        direction: Direction,
        width: i32,
        length: i32,
        y_step: i32,
    ) -> BlockPos {
        let section = &self.sections[parent];
        let bounds = section.bounding_box();
        let center_x = bounds.min_x() + bounds.x_span() / 2;
        let center_z = bounds.min_z() + bounds.z_span() / 2;
        let along = if direction == section.direction {
            section.length - 1
        } else {
            section.length / 2
        };
        let y = section.floor_y(along) + y_step;
        match direction {
            Direction::North => BlockPos::new(center_x - width / 2, y, bounds.min_z() + length - 2),
            Direction::South => BlockPos::new(center_x - width / 2, y, bounds.max_z() + 1),
            Direction::East => BlockPos::new(bounds.max_x() + 1, y, center_z - width / 2),
            Direction::West => BlockPos::new(bounds.min_x() - length, y, center_z - width / 2),
            _ => BlockPos::new(center_x - width / 2, y, center_z - width / 2),
        }
    }

    fn try_add(
        &mut self,
        section: MineshaftSection,
        parent: usize,
        min_y: i32,
        max_y: i32,
    ) -> Option<usize> {
        let bounds = section.bounding_box();
        if bounds.min_y() < min_y || bounds.max_y() > max_y {
            return None;
        }
        for (index, existing) in self.sections.iter().enumerate() {
            if index != parent && interiors_intersect(bounds, existing.bounding_box()) {
                return None;
            }
        }
        self.bounding_box.encapsulate(bounds);
        self.sections.push(section);
        Some(self.sections.len() - 1)
    }
}

pub struct MineshaftStart;

impl MineshaftStart {
    pub const ROOM_SIZE: i32 = ROOM_SIZE;
    pub const CORRIDOR_HEIGHT: i32 = CORRIDOR_HEIGHT;

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
        let x = context.chunk_pos.x << 4;
        let z = context.chunk_pos.z << 4;
        let y = choose_start_y(source, x, z, &mut random)?;
        let mesa = structure_id == "minecraft:mineshaft_mesa";
        let max_y = source.min_y() + source.height() - 1;
        let piece = MineshaftPiece::from_random(
            context.seed,
            BlockPos::new(x, y, z),
            mesa,
            source.min_y(),
            max_y,
            &mut random,
        );
        Ok(StructureStart::new_mineshaft(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

fn choose_start_y(
    source: &dyn FeatureLevel,
    x: i32,
    z: i32,
    random: &mut impl RandomSource,
) -> Result<i32, StructureError> {
    let surface = source
        .heightmap(Heightmap::WorldSurfaceWg, x + 8, z + 8)
        .ok_or_else(|| StructureError::MissingHeightSource {
            structure: "minecraft:mineshaft".to_owned(),
        })?;
    let min_y = source.min_y() + 8;
    let max_y = (source.min_y() + source.height() - CORRIDOR_HEIGHT)
        .min(surface - 12)
        .max(min_y);
    let target =
        (surface - SURFACE_DEPTH - random.next_int_bound(DEPTH_VARIATION)).clamp(min_y, max_y);
    if source.terrain_density_at(x + 8, target, z + 8).is_none() {
        return Ok(target);
    }

    let cave_score = |y: i32| {
        [(0, 0), (-4, 0), (4, 0), (0, -4), (0, 4)]
            .into_iter()
            .filter_map(|(dx, dz)| source.terrain_density_at(x + 8 + dx, y, z + 8 + dz))
            .filter(|density| *density <= 0.0)
            .count()
    };
    let mut best = target;
    let mut best_score = cave_score(target);
    let mut best_distance = 0;
    for radius in 0..=DENSITY_SEARCH_RADIUS {
        for delta in if radius == 0 {
            [0, 0]
        } else {
            [-radius, radius]
        } {
            if radius > 0 && delta == 0 {
                continue;
            }
            let y = (target + delta).clamp(min_y, max_y);
            let score = cave_score(y);
            if score >= 3 {
                return Ok(y);
            }
            let distance = (y - target).abs();
            if score > best_score || (score == best_score && distance < best_distance) {
                best = y;
                best_score = score;
                best_distance = distance;
            }
        }
    }
    Ok(best)
}

struct MineshaftWriter<'a> {
    piece: &'a MineshaftPiece,
    level: &'a mut dyn FeatureLevel,
    chunk_box: BoundingBox,
    result: StartPlacement,
}

impl MineshaftWriter<'_> {
    fn generate_section(&mut self, index: usize, section: &MineshaftSection) {
        match section.kind {
            MineshaftSectionKind::Room | MineshaftSectionKind::Crossing => {
                self.generate_room(index, section)
            }
            MineshaftSectionKind::Corridor => self.generate_corridor(index, section),
            MineshaftSectionKind::Stairs => self.generate_stairs(index, section),
        }
        self.generate_cobwebs(index, section);
        self.generate_torches(index, section);
        self.generate_chest(section);
    }

    fn generate_room(&mut self, index: usize, section: &MineshaftSection) {
        for along in 1..section.length - 1 {
            for across in 1..section.width - 1 {
                for y in 1..=3 {
                    self.place_local(air(), section, across, along, y);
                }
            }
        }
        for along in 0..section.length {
            for across in 0..section.width {
                self.place_floor(index, section, across, along);
                self.place_local(
                    self.piece.ceiling_material(index, across, along),
                    section,
                    across,
                    along,
                    CORRIDOR_HEIGHT - 1,
                );
            }
        }
        for along in 0..section.length {
            for across in 0..section.width {
                let boundary = across == 0
                    || across == section.width - 1
                    || along == 0
                    || along == section.length - 1;
                if !boundary {
                    continue;
                }
                let center = (3..=5).contains(&across);
                let doorway = ((along == 0 || along == section.length - 1) && center)
                    || ((across == 0 || across == section.width - 1) && (3..=5).contains(&along));
                for y in 1..=3 {
                    if !doorway {
                        self.place_local(
                            self.piece.wall_material(index, across, along),
                            section,
                            across,
                            along,
                            y,
                        );
                    } else {
                        self.place_local(air(), section, across, along, y);
                    }
                }
            }
        }
        for (across, along) in [(0, 0), (8, 0), (0, 8), (8, 8)] {
            for y in 1..CORRIDOR_HEIGHT {
                self.place_local(self.piece.beam_material("y"), section, across, along, y);
            }
        }
        self.place_rail_line(section, section.direction);
        if section.kind.is_crossing() {
            self.place_rail_line(section, clockwise(section.direction));
        }
    }

    fn generate_corridor(&mut self, index: usize, section: &MineshaftSection) {
        for along in 0..section.length {
            for across in 0..section.width {
                self.place_floor(index, section, across, along);
                self.place_local(
                    self.piece.ceiling_material(index, across, along),
                    section,
                    across,
                    along,
                    CORRIDOR_HEIGHT - 1,
                );
                for y in 1..=3 {
                    if across == 0 || across == section.width - 1 {
                        if section.support {
                            self.place_local(self.piece.fence_state(), section, across, along, 1);
                        } else {
                            self.place_local(
                                self.piece.wall_material(index, across, along),
                                section,
                                across,
                                along,
                                y,
                            );
                        }
                    } else {
                        self.place_local(air(), section, across, along, y);
                    }
                }
            }
            if section.support {
                if along % 4 == 0 || along == section.length - 1 {
                    for across in [0, section.width - 1] {
                        for y in 2..CORRIDOR_HEIGHT {
                            self.place_local(
                                self.piece.beam_material("y"),
                                section,
                                across,
                                along,
                                y,
                            );
                        }
                    }
                }
                let axis = if matches!(section.direction, Direction::North | Direction::South) {
                    "z"
                } else {
                    "x"
                };
                self.place_local(
                    self.piece.beam_material(axis),
                    section,
                    section.width / 2,
                    along,
                    CORRIDOR_HEIGHT - 1,
                );
            }
        }
        self.place_rail_line(section, section.direction);
    }

    fn generate_stairs(&mut self, index: usize, section: &MineshaftSection) {
        for along in 0..section.length {
            for across in 0..section.width {
                let floor = self.piece.stairs_state(section.direction, section.y_step);
                self.place_local(floor, section, across, along, 0);
                self.place_local(
                    self.piece.wall_material(index, across, along),
                    section,
                    across,
                    along,
                    -1,
                );
                let ceiling_y = section.floor_y(along) + CORRIDOR_HEIGHT - 1;
                self.place_local_at_y(
                    self.piece.ceiling_material(index, across, along),
                    section,
                    across,
                    along,
                    ceiling_y,
                );
                for y in 1..=3 {
                    if across == 0 || across == section.width - 1 {
                        if section.support {
                            self.place_local_at_y(
                                self.piece.fence_state(),
                                section,
                                across,
                                along,
                                section.floor_y(along) + y,
                            );
                        } else {
                            self.place_local_at_y(
                                self.piece.wall_material(index, across, along),
                                section,
                                across,
                                along,
                                section.floor_y(along) + y,
                            );
                        }
                    } else {
                        self.place_local_at_y(
                            air(),
                            section,
                            across,
                            along,
                            section.floor_y(along) + y,
                        );
                    }
                }
            }
        }
        self.place_rail_line(section, section.direction);
    }

    fn place_floor(&mut self, index: usize, section: &MineshaftSection, across: i32, along: i32) {
        self.place_local(
            self.piece.floor_material(index, across, along),
            section,
            across,
            along,
            0,
        );
    }

    fn place_rail_line(&mut self, section: &MineshaftSection, direction: Direction) {
        let Some(kind) = section.rail_kind else {
            return;
        };
        for along in 1..section.length - 1 {
            let rail_across = if direction == section.direction {
                section.width / 2
            } else {
                along
            };
            let rail_along = if direction == section.direction {
                along
            } else {
                section.length / 2
            };
            let y = section.floor_y(rail_along) + 1;
            self.place_local_at_y(
                self.piece.rail_state(kind, direction, section.y_step),
                section,
                rail_across,
                rail_along,
                y,
            );
        }
    }

    fn generate_cobwebs(&mut self, index: usize, section: &MineshaftSection) {
        for web in 0..section.cobwebs {
            let across_width = (section.width - 2).max(1) as u64;
            let along_width = (section.length - 2).max(1) as u64;
            let across =
                1 + (coordinate_hash(self.piece.seed, index, web, 0x0c0b) % across_width) as i32;
            let along =
                1 + (coordinate_hash(self.piece.seed, index, web, 0x0e6b) % along_width) as i32;
            let y = section.floor_y(along) + 3;
            self.place_local_at_y(state("minecraft:cobweb"), section, across, along, y);
        }
    }

    fn generate_torches(&mut self, index: usize, section: &MineshaftSection) {
        for torch in 0..section.torches {
            let along = 2 + ((torch * 3 + index as i32) % (section.length - 3).max(1));
            let y = section.floor_y(along) + 2;
            let torch_state = if section.support {
                BlockState {
                    name: "minecraft:wall_torch".to_owned(),
                    properties: vec![(
                        "facing".to_owned(),
                        direction_name(across_direction(section.direction)).to_owned(),
                    )],
                }
            } else {
                state("minecraft:torch")
            };
            let across = if section.support { 0 } else { 1 };
            self.place_local_at_y(torch_state, section, across, along, y);
        }
    }

    fn generate_chest(&mut self, section: &MineshaftSection) {
        if !section.chest {
            return;
        }
        let across = if section.chest_side {
            section.width - 2
        } else {
            1
        };
        let along = section.length - 2;
        let y = section.floor_y(along) + 1;
        let pos = world_pos(section, across, along, y);
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
            properties: vec![
                (
                    "facing".to_owned(),
                    direction_name(section.direction.opposite()).to_owned(),
                ),
                ("type".to_owned(), "SINGLE".to_owned()),
                ("waterlogged".to_owned(), "false".to_owned()),
            ],
        };
        let Some((pos, chest)) = self.place_state(chest, section, across, along, y) else {
            return;
        };
        self.result.block_entities.push(PlacedBlock {
            pos,
            state: chest,
            nbt: Some(Tag::Compound(vec![
                ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                (
                    "LootTable".to_owned(),
                    Tag::String("minecraft:chests/abandoned_mineshaft".to_owned()),
                ),
                ("LootTableSeed".to_owned(), Tag::Long(section.chest_seed)),
            ])),
        });
    }

    fn place_local(
        &mut self,
        state: BlockState,
        section: &MineshaftSection,
        across: i32,
        along: i32,
        y_offset: i32,
    ) -> bool {
        let y = if section.kind.is_stairs() {
            section.floor_y(along) + y_offset
        } else {
            section.origin.y() + y_offset
        };
        self.place_local_at_y(state, section, across, along, y)
    }

    fn place_local_at_y(
        &mut self,
        state: BlockState,
        section: &MineshaftSection,
        across: i32,
        along: i32,
        y: i32,
    ) -> bool {
        self.place_state(state, section, across, along, y).is_some()
    }

    fn place_state(
        &mut self,
        state: BlockState,
        section: &MineshaftSection,
        across: i32,
        along: i32,
        y: i32,
    ) -> Option<(BlockPos, BlockState)> {
        let pos = world_pos(section, across, along, y);
        if !self.chunk_box.is_inside(pos) {
            return None;
        }
        if self.level.set_block(pos, state.clone()) {
            self.result.written += 1;
            self.result.written_blocks.push(pos);
            Some((pos, state))
        } else {
            self.result.rejected += 1;
            None
        }
    }
}

impl MineshaftPiece {
    fn floor_material(&self, index: usize, across: i32, along: i32) -> BlockState {
        let hash = coordinate_hash(self.seed, index, across, along);
        if self.mesa {
            match hash % 7 {
                0 => state("minecraft:red_terracotta"),
                1 | 2 => state("minecraft:terracotta"),
                3 => state("minecraft:dark_oak_planks"),
                _ => state("minecraft:cobblestone"),
            }
        } else {
            match hash % 7 {
                0 => state("minecraft:mossy_cobblestone"),
                1 | 2 => state("minecraft:oak_planks"),
                _ => state("minecraft:cobblestone"),
            }
        }
    }

    fn wall_material(&self, index: usize, across: i32, along: i32) -> BlockState {
        let hash = coordinate_hash(self.seed, index, across ^ 0x5a, along);
        if self.mesa && hash.is_multiple_of(5) {
            state("minecraft:terracotta")
        } else {
            state("minecraft:cobblestone")
        }
    }

    fn ceiling_material(&self, index: usize, across: i32, along: i32) -> BlockState {
        self.floor_material(index, across, along)
    }

    fn fence_state(&self) -> BlockState {
        BlockState {
            name: if self.mesa {
                "minecraft:dark_oak_fence"
            } else {
                "minecraft:oak_fence"
            }
            .to_owned(),
            properties: vec![
                ("east".to_owned(), "false".to_owned()),
                ("north".to_owned(), "false".to_owned()),
                ("south".to_owned(), "false".to_owned()),
                ("waterlogged".to_owned(), "false".to_owned()),
                ("west".to_owned(), "false".to_owned()),
            ],
        }
    }

    fn beam_material(&self, axis: &str) -> BlockState {
        BlockState {
            name: if self.mesa {
                "minecraft:dark_oak_log"
            } else {
                "minecraft:oak_log"
            }
            .to_owned(),
            properties: vec![("axis".to_owned(), axis.to_owned())],
        }
    }

    fn stairs_state(&self, direction: Direction, y_step: i32) -> BlockState {
        let facing = if y_step < 0 {
            direction.opposite()
        } else {
            direction
        };
        BlockState {
            name: if self.mesa {
                "minecraft:dark_oak_stairs"
            } else {
                "minecraft:oak_stairs"
            }
            .to_owned(),
            properties: vec![
                ("facing".to_owned(), direction_name(facing).to_owned()),
                ("half".to_owned(), "bottom".to_owned()),
                ("shape".to_owned(), "straight".to_owned()),
                ("waterlogged".to_owned(), "false".to_owned()),
            ],
        }
    }

    fn rail_state(&self, kind: MineshaftRailKind, direction: Direction, y_step: i32) -> BlockState {
        let name = match kind {
            MineshaftRailKind::Normal => "minecraft:rail",
            MineshaftRailKind::Powered => "minecraft:powered_rail",
            MineshaftRailKind::Detector => "minecraft:detector_rail",
        };
        let shape = if y_step != 0 {
            let ascending = if y_step > 0 {
                direction
            } else {
                direction.opposite()
            };
            format!("ascending_{}", direction_name(ascending))
        } else if matches!(direction, Direction::North | Direction::South) {
            "north_south".to_owned()
        } else {
            "east_west".to_owned()
        };
        let mut properties = vec![("shape".to_owned(), shape)];
        if !matches!(kind, MineshaftRailKind::Normal) {
            properties.push(("powered".to_owned(), "false".to_owned()));
        }
        properties.push(("waterlogged".to_owned(), "false".to_owned()));
        BlockState {
            name: name.to_owned(),
            properties,
        }
    }
}

fn roll_rail(random: &mut impl RandomSource, force: bool) -> Option<MineshaftRailKind> {
    if !force && random.next_float() >= 0.55 {
        return None;
    }
    Some(match random.next_int_bound(10) {
        0 => MineshaftRailKind::Detector,
        1..=2 => MineshaftRailKind::Powered,
        _ => MineshaftRailKind::Normal,
    })
}

fn roll_cobwebs(random: &mut impl RandomSource) -> i32 {
    if random.next_float() < 0.45 {
        1 + random.next_int_bound(2)
    } else {
        0
    }
}

fn footprint(
    origin: BlockPos,
    direction: Direction,
    width: i32,
    length: i32,
) -> (i32, i32, i32, i32) {
    match direction {
        Direction::North => (
            origin.x(),
            origin.x() + width - 1,
            origin.z() - length + 1,
            origin.z(),
        ),
        Direction::South => (
            origin.x(),
            origin.x() + width - 1,
            origin.z(),
            origin.z() + length - 1,
        ),
        Direction::East => (
            origin.x(),
            origin.x() + length - 1,
            origin.z(),
            origin.z() + width - 1,
        ),
        Direction::West => (
            origin.x() - length + 1,
            origin.x(),
            origin.z(),
            origin.z() + width - 1,
        ),
        _ => (
            origin.x(),
            origin.x() + width - 1,
            origin.z(),
            origin.z() + length - 1,
        ),
    }
}

fn world_pos(section: &MineshaftSection, across: i32, along: i32, y: i32) -> BlockPos {
    let origin = section.origin;
    match section.direction {
        Direction::North => BlockPos::new(origin.x() + across, y, origin.z() - along),
        Direction::South => BlockPos::new(origin.x() + across, y, origin.z() + along),
        Direction::East => BlockPos::new(origin.x() + along, y, origin.z() + across),
        Direction::West => BlockPos::new(origin.x() - along, y, origin.z() + across),
        _ => BlockPos::new(origin.x() + across, y, origin.z() + along),
    }
}

fn interiors_intersect(first: BoundingBox, second: BoundingBox) -> bool {
    first.max_x() > second.min_x()
        && first.min_x() < second.max_x()
        && first.max_y() > second.min_y()
        && first.min_y() < second.max_y()
        && first.max_z() > second.min_z()
        && first.min_z() < second.max_z()
}

fn horizontal_direction(index: i32) -> Direction {
    match index {
        0 => Direction::North,
        1 => Direction::East,
        2 => Direction::South,
        _ => Direction::West,
    }
}

fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        other => other,
    }
}

fn counter_clockwise(direction: Direction) -> Direction {
    clockwise(direction.opposite())
}

fn across_direction(direction: Direction) -> Direction {
    match direction {
        Direction::East | Direction::West => Direction::South,
        _ => Direction::East,
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

fn piece_seed(seed: i64, x: i32, z: i32) -> i64 {
    seed ^ i64::from(x).wrapping_mul(0x9e37_79b1_85eb_ca87_u64.cast_signed())
        ^ i64::from(z).wrapping_mul(0xc2b2_ae3d_27d4_eb4f_u64.cast_signed())
}

fn coordinate_hash(seed: i64, first: usize, second: i32, third: i32) -> u64 {
    let mut value = (seed as u64)
        .wrapping_add(first as u64)
        .wrapping_mul(0x9e37_79b1_85eb_ca87)
        ^ (second as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
        ^ (third as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
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

fn air() -> BlockState {
    state("minecraft:air")
}
