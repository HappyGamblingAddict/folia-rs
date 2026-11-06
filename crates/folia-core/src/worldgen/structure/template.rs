//! Port of `StructureTemplate` loading and block placement.
//!
//! The Java class is both the template data model and the world writer. This
//! slice ports the data model, `StructurePlaceSettings` transforms, and the
//! block-writing half of `placeInWorld`. Processor lists, waterlogging, shape
//! updates, random palette selection, and entity spawning are intentionally
//! left to later structure slices.

use crate::chunk::{BlockState, is_air_state};
use crate::nbt::{self, Tag};
use crate::worldgen::feature::FeatureLevel;
use folia_math::{Axis, BlockPos, Direction, Vec3};
use std::io;

/// `NbtUtils.getDataVersion(tag, 500)`: templates without a `DataVersion` use
/// the same 500 fallback as Java. This port records the value but does not run
/// data fixers; the shipped 1.21.11 templates report 4671 and load directly.
pub const DEFAULT_DATA_VERSION: i32 = 500;

/// `net.minecraft.world.level.block.Rotation`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::CounterClockwise90,
    ];

    /// `Rotation.getSerializedName()`.
    pub fn name(self) -> &'static str {
        match self {
            Rotation::None => "none",
            Rotation::Clockwise90 => "clockwise_90",
            Rotation::Clockwise180 => "180",
            Rotation::CounterClockwise90 => "counterclockwise_90",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|rotation| rotation.name() == name)
    }

    /// Whether this rotation swaps the horizontal x/z extents.
    pub fn swaps_horizontal_axes(self) -> bool {
        matches!(self, Rotation::Clockwise90 | Rotation::CounterClockwise90)
    }

    /// `Rotation.rotate(Direction)`.
    pub fn rotate_direction(self, direction: Direction) -> Direction {
        if direction.axis().is_vertical() {
            return direction;
        }
        match self {
            Rotation::None => direction,
            Rotation::Clockwise90 => match direction {
                Direction::North => Direction::East,
                Direction::East => Direction::South,
                Direction::South => Direction::West,
                Direction::West => Direction::North,
                _ => direction,
            },
            Rotation::Clockwise180 => direction.opposite(),
            Rotation::CounterClockwise90 => match direction {
                Direction::North => Direction::West,
                Direction::West => Direction::South,
                Direction::South => Direction::East,
                Direction::East => Direction::North,
                _ => direction,
            },
        }
    }

    /// `Rotation.rotate(int rotation, int positionCount)`.
    pub fn rotate_index(self, index: i32, position_count: i32) -> i32 {
        if position_count <= 0 {
            return index;
        }
        match self {
            Rotation::None => index,
            Rotation::Clockwise90 => (index + position_count / 4).rem_euclid(position_count),
            Rotation::Clockwise180 => (index + position_count / 2).rem_euclid(position_count),
            Rotation::CounterClockwise90 => {
                (index + position_count * 3 / 4).rem_euclid(position_count)
            }
        }
    }
}

/// `net.minecraft.world.level.block.Mirror`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mirror {
    #[default]
    None,
    /// `LEFT_RIGHT`, inverting z.
    LeftRight,
    /// `FRONT_BACK`, inverting x.
    FrontBack,
}

impl Mirror {
    pub const ALL: [Mirror; 3] = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack];

    /// `Mirror.getSerializedName()`.
    pub fn name(self) -> &'static str {
        match self {
            Mirror::None => "none",
            Mirror::LeftRight => "left_right",
            Mirror::FrontBack => "front_back",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mirror| mirror.name() == name)
    }

    /// `Mirror.mirror(Direction)`.
    pub fn mirror_direction(self, direction: Direction) -> Direction {
        match self {
            Mirror::None => direction,
            Mirror::LeftRight => match direction {
                Direction::North => Direction::South,
                Direction::South => Direction::North,
                _ => direction,
            },
            Mirror::FrontBack => match direction {
                Direction::West => Direction::East,
                Direction::East => Direction::West,
                _ => direction,
            },
        }
    }

    /// `Mirror.mirror(int rotation, int rotationCount)`.
    pub fn mirror_index(self, index: i32, rotation_count: i32) -> i32 {
        if rotation_count <= 0 {
            return index;
        }
        let half = rotation_count / 2;
        let reduced = if index > half {
            index - rotation_count
        } else {
            index
        };
        match self {
            Mirror::None => index,
            Mirror::LeftRight => (half - reduced + rotation_count).rem_euclid(rotation_count),
            Mirror::FrontBack => (rotation_count - reduced).rem_euclid(rotation_count),
        }
    }
}

/// `StructureTemplate.StructureBlockInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateBlock {
    pub pos: BlockPos,
    /// Index into every palette. The parser rejects indices that do not fit.
    pub state: usize,
    pub nbt: Option<Tag>,
}

/// `StructureTemplate.StructureEntityInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateEntity {
    pub pos: Vec3,
    pub block_pos: BlockPos,
    pub nbt: Option<Tag>,
}

/// A block after `StructurePlaceSettings` transforms.
///
/// `pos` is relative to the placement origin for [`StructureTemplate::placement_blocks`],
/// and absolute for [`StructureTemplate::place_into`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedBlock {
    pub pos: BlockPos,
    pub state: BlockState,
    pub nbt: Option<Tag>,
}

/// An entity after `StructurePlaceSettings` transforms.
///
/// Entity spawning is not ported, so [`StructureTemplate::place_into`] returns
/// these instead of creating entities. `pos`/`block_pos` are absolute.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEntity {
    pub pos: Vec3,
    pub block_pos: BlockPos,
    pub nbt: Option<Tag>,
}

/// Result of [`StructureTemplate::place_into`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructurePlacement {
    /// Blocks for which `FeatureLevel::set_block` returned true.
    pub written: usize,
    /// Blocks the write region rejected (for example outside its vertical
    /// bounds or chunk). Vanilla's `LevelAccessor.setBlock` false path.
    pub rejected: usize,
    /// Written blocks that carried block-entity NBT, at absolute positions.
    /// The caller must create the actual block entities from this NBT.
    pub block_entities: Vec<PlacedBlock>,
    /// Template entities at absolute positions. They are not spawned.
    pub entities: Vec<PlacedEntity>,
}

/// `StructurePlaceSettings` subset needed for template block placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructurePlaceSettings {
    mirror: Mirror,
    rotation: Rotation,
    rotation_pivot: BlockPos,
    ignore_entities: bool,
    ignore_air: bool,
}

impl Default for StructurePlaceSettings {
    fn default() -> Self {
        Self {
            mirror: Mirror::None,
            rotation: Rotation::None,
            rotation_pivot: BlockPos::ZERO,
            ignore_entities: false,
            ignore_air: false,
        }
    }
}

impl StructurePlaceSettings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mirror(mut self, mirror: Mirror) -> Self {
        self.mirror = mirror;
        self
    }

    pub fn with_rotation(mut self, rotation: Rotation) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_rotation_pivot(mut self, pivot: BlockPos) -> Self {
        self.rotation_pivot = pivot;
        self
    }

    pub fn with_ignore_entities(mut self, ignore_entities: bool) -> Self {
        self.ignore_entities = ignore_entities;
        self
    }

    /// Skip air/cave-air/void-air entries. Vanilla places air too; this is an
    /// explicit convenience extension used by structure decoration callers.
    pub fn with_ignore_air(mut self, ignore_air: bool) -> Self {
        self.ignore_air = ignore_air;
        self
    }

    pub fn mirror(self) -> Mirror {
        self.mirror
    }

    pub fn rotation(self) -> Rotation {
        self.rotation
    }

    pub fn rotation_pivot(self) -> BlockPos {
        self.rotation_pivot
    }

    pub fn ignore_entities(self) -> bool {
        self.ignore_entities
    }

    pub fn ignore_air(self) -> bool {
        self.ignore_air
    }
}

/// A parsed vanilla structure template.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureTemplate {
    data_version: i32,
    size: [i32; 3],
    palettes: Vec<Vec<BlockState>>,
    blocks: Vec<TemplateBlock>,
    entities: Vec<TemplateEntity>,
}

impl StructureTemplate {
    /// Reads one gzip-compressed structure NBT file (the format stored under
    /// `data/<namespace>/structure`).
    pub fn from_gzip(bytes: &[u8]) -> io::Result<Self> {
        let root = nbt::read_gzip_root(bytes)?;
        Self::from_nbt(&root)
    }

    /// Builds a template from an already decoded root compound.
    pub fn from_nbt(tag: &Tag) -> io::Result<Self> {
        let root = tag
            .as_compound()
            .ok_or_else(|| invalid("structure template root must be a compound tag"))?;

        let data_version = match field(root, "DataVersion") {
            None => DEFAULT_DATA_VERSION,
            Some(tag) => tag
                .as_int()
                .ok_or_else(|| invalid("structure template DataVersion must be an int"))?,
        };

        let size = int_list3(field(root, "size"), "structure template size")?;
        if let Some((axis, value)) = size
            .iter()
            .copied()
            .enumerate()
            .find(|(_, value)| *value <= 0)
        {
            return Err(invalid(format!(
                "structure template size axis {axis} must be positive, got {value}"
            )));
        }

        let palettes = match field(root, "palettes") {
            Some(tag) => {
                let entries = list(tag, "structure template palettes")?;
                if entries.is_empty() {
                    return Err(invalid(
                        "structure template palettes must contain at least one palette",
                    ));
                }
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, palette)| parse_palette(palette, index))
                    .collect::<io::Result<Vec<_>>>()?
            }
            None => {
                let palette = field(root, "palette")
                    .ok_or_else(|| invalid("structure template is missing `palette`"))?;
                vec![parse_palette(palette, 0)?]
            }
        };

        let blocks = parse_blocks(field(root, "blocks"), size, &palettes)?;
        let entities = parse_entities(field(root, "entities"))?;

        Ok(Self {
            data_version,
            size,
            palettes,
            blocks,
            entities,
        })
    }

    /// Value of the template's `DataVersion` (default 500 when absent).
    pub fn data_version(&self) -> i32 {
        self.data_version
    }

    /// Template extents as `[x, y, z]`.
    pub fn size(&self) -> [i32; 3] {
        self.size
    }

    pub fn size_x(&self) -> i32 {
        self.size[0]
    }

    pub fn size_y(&self) -> i32 {
        self.size[1]
    }

    pub fn size_z(&self) -> i32 {
        self.size[2]
    }

    /// `StructureTemplate.getSize(Rotation)`: a quarter turn swaps x and z.
    pub fn rotated_size(&self, rotation: Rotation) -> [i32; 3] {
        if rotation.swaps_horizontal_axes() {
            [self.size[2], self.size[1], self.size[0]]
        } else {
            self.size
        }
    }

    pub fn palette_count(&self) -> usize {
        self.palettes.len()
    }

    /// Palette entries for one palette, or `None` when the index is invalid.
    pub fn palette(&self, index: usize) -> Option<&[BlockState]> {
        self.palettes.get(index).map(Vec::as_slice)
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn blocks(&self) -> &[TemplateBlock] {
        &self.blocks
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub fn entities(&self) -> &[TemplateEntity] {
        &self.entities
    }

    /// `StructureTemplate.placeInWorld` block half, without writing: every
    /// template block transformed by `settings`, in Java's
    /// normal/special/NBT grouping order (NBT-bearing blocks last, each group
    /// sorted by y, x, z).
    ///
    /// The return value is relative to the placement origin. The first palette
    /// is used; see the module docs for multi-palette selection.
    pub fn placement_blocks(&self, settings: &StructurePlaceSettings) -> Vec<PlacedBlock> {
        let Some(palette) = self.palettes.first() else {
            return Vec::new();
        };

        self.blocks
            .iter()
            .filter_map(|block| {
                let state = palette.get(block.state)?;
                if settings.ignore_air() && is_air_state(state) {
                    return None;
                }
                Some(PlacedBlock {
                    pos: transform(
                        block.pos,
                        settings.mirror(),
                        settings.rotation(),
                        settings.rotation_pivot(),
                    ),
                    state: transform_state(state, settings.mirror(), settings.rotation()),
                    nbt: block.nbt.clone(),
                })
            })
            .collect()
    }

    /// Writes the transformed template into `level`.
    ///
    /// `origin` is Java's `offset` argument: relative block/entity positions
    /// are transformed with the rotation pivot, then translated by `origin`.
    /// Block-entity NBT is returned in [`StructurePlacement::block_entities`];
    /// entity NBT is returned in [`StructurePlacement::entities`] rather than
    /// spawned.
    pub fn place_into(
        &self,
        level: &mut dyn FeatureLevel,
        origin: BlockPos,
        settings: &StructurePlaceSettings,
    ) -> StructurePlacement {
        let mut placement = StructurePlacement::default();

        for block in self.placement_blocks(settings) {
            let pos = origin.offset_by(block.pos);
            if level.set_block(pos, block.state.clone()) {
                placement.written += 1;
                if block.nbt.is_some() {
                    placement.block_entities.push(PlacedBlock { pos, ..block });
                }
            } else {
                placement.rejected += 1;
            }
        }

        if !settings.ignore_entities() {
            let origin_vec = Vec3::new(
                f64::from(origin.x()),
                f64::from(origin.y()),
                f64::from(origin.z()),
            );
            placement.entities = self
                .entities
                .iter()
                .map(|entity| PlacedEntity {
                    pos: transform_vec3(
                        entity.pos,
                        settings.mirror(),
                        settings.rotation(),
                        settings.rotation_pivot(),
                    )
                    .add(origin_vec),
                    block_pos: origin.offset_by(transform(
                        entity.block_pos,
                        settings.mirror(),
                        settings.rotation(),
                        settings.rotation_pivot(),
                    )),
                    nbt: entity.nbt.clone(),
                })
                .collect();
        }

        placement
    }
}

/// `StructureTemplate.transform(BlockPos, Mirror, Rotation, BlockPos)`.
///
/// The mirror is applied first, then the rotation around `pivot`. The
/// `Mirror::None`/`Rotation::None` case returns the input position unchanged,
/// exactly like Java's `flag` branch.
pub fn transform(pos: BlockPos, mirror: Mirror, rotation: Rotation, pivot: BlockPos) -> BlockPos {
    let mut x = pos.x();
    let y = pos.y();
    let mut z = pos.z();

    let mirrored = match mirror {
        Mirror::None => false,
        Mirror::LeftRight => {
            z = z.wrapping_neg();
            true
        }
        Mirror::FrontBack => {
            x = x.wrapping_neg();
            true
        }
    };

    let pivot_x = pivot.x();
    let pivot_z = pivot.z();
    match rotation {
        Rotation::CounterClockwise90 => BlockPos::new(
            pivot_x.wrapping_sub(pivot_z).wrapping_add(z),
            y,
            pivot_x.wrapping_add(pivot_z).wrapping_sub(x),
        ),
        Rotation::Clockwise90 => BlockPos::new(
            pivot_x.wrapping_add(pivot_z).wrapping_sub(z),
            y,
            pivot_z.wrapping_sub(pivot_x).wrapping_add(x),
        ),
        Rotation::Clockwise180 => BlockPos::new(
            pivot_x.wrapping_add(pivot_x).wrapping_sub(x),
            y,
            pivot_z.wrapping_add(pivot_z).wrapping_sub(z),
        ),
        Rotation::None => {
            if mirrored {
                BlockPos::new(x, y, z)
            } else {
                pos
            }
        }
    }
}

/// `StructureTemplate.transform(Vec3, Mirror, Rotation, BlockPos)`.
pub fn transform_vec3(pos: Vec3, mirror: Mirror, rotation: Rotation, pivot: BlockPos) -> Vec3 {
    let mut x = pos.x();
    let y = pos.y();
    let mut z = pos.z();

    let mirrored = match mirror {
        Mirror::None => false,
        Mirror::LeftRight => {
            z = 1.0 - z;
            true
        }
        Mirror::FrontBack => {
            x = 1.0 - x;
            true
        }
    };

    let pivot_x = f64::from(pivot.x());
    let pivot_z = f64::from(pivot.z());
    match rotation {
        Rotation::CounterClockwise90 => {
            Vec3::new(pivot_x - pivot_z + z, y, pivot_x + pivot_z + 1.0 - x)
        }
        Rotation::Clockwise90 => Vec3::new(pivot_x + pivot_z + 1.0 - z, y, pivot_z - pivot_x + x),
        Rotation::Clockwise180 => {
            Vec3::new(pivot_x + pivot_x + 1.0 - x, y, pivot_z + pivot_z + 1.0 - z)
        }
        Rotation::None => {
            if mirrored {
                Vec3::new(x, y, z)
            } else {
                pos
            }
        }
    }
}

/// `BlockState.mirror(mirror).rotate(rotation)` for the standard properties
/// that do not need per-block shape knowledge.
///
/// Handles `facing`, `axis`, `rotation` (0..15, e.g. signs), and the jigsaw
/// `orientation` (`front_top`). Blocks whose rotation also rewrites `shape`,
/// `hinge`, `type`, etc. keep those values for now; exact per-block transforms
/// need the P1.5 block-behaviour table.
pub fn transform_state(state: &BlockState, mirror: Mirror, rotation: Rotation) -> BlockState {
    if mirror == Mirror::None && rotation == Rotation::None {
        return state.clone();
    }

    let mut transformed = state.clone();
    for (key, value) in &mut transformed.properties {
        match key.as_str() {
            "facing" => {
                if let Some(direction) = direction_from_name(value) {
                    *value =
                        direction_name(transform_direction(direction, mirror, rotation)).to_owned();
                }
            }
            "axis" => {
                if let Some(axis) = axis_from_name(value) {
                    *value = axis_name(transform_axis(axis, rotation)).to_owned();
                }
            }
            "rotation" => {
                if let Ok(index) = value.parse::<i32>()
                    && (0..16).contains(&index)
                {
                    let index = rotation.rotate_index(mirror.mirror_index(index, 16), 16);
                    *value = index.to_string();
                }
            }
            "orientation" => {
                if let Some((front, top)) = value.split_once('_')
                    && let (Some(front), Some(top)) =
                        (direction_from_name(front), direction_from_name(top))
                {
                    *value = format!(
                        "{}_{}",
                        direction_name(transform_direction(front, mirror, rotation)),
                        direction_name(transform_direction(top, mirror, rotation))
                    );
                }
            }
            _ => {}
        }
    }
    transformed
}

fn transform_direction(direction: Direction, mirror: Mirror, rotation: Rotation) -> Direction {
    rotation.rotate_direction(mirror.mirror_direction(direction))
}

fn transform_axis(axis: Axis, rotation: Rotation) -> Axis {
    if !rotation.swaps_horizontal_axes() {
        return axis;
    }
    match axis {
        Axis::X => Axis::Z,
        Axis::Z => Axis::X,
        Axis::Y => Axis::Y,
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

fn axis_from_name(name: &str) -> Option<Axis> {
    match name {
        "x" => Some(Axis::X),
        "y" => Some(Axis::Y),
        "z" => Some(Axis::Z),
        _ => None,
    }
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn parse_palette(tag: &Tag, palette_index: usize) -> io::Result<Vec<BlockState>> {
    let what = format!("structure template palette {palette_index}");
    let entries = list(tag, &what)?;
    if entries.is_empty() {
        return Err(invalid(format!("{what} must contain at least one entry")));
    }

    entries
        .iter()
        .enumerate()
        .map(|(entry_index, entry)| {
            let entry = entry.as_compound().ok_or_else(|| {
                invalid(format!("{what} entry {entry_index} must be a compound tag"))
            })?;
            let name = field(entry, "Name").and_then(Tag::as_str).ok_or_else(|| {
                invalid(format!(
                    "{what} entry {entry_index} is missing a string `Name`"
                ))
            })?;

            let mut properties = Vec::new();
            if let Some(tag) = field(entry, "Properties") {
                let properties_tag = tag.as_compound().ok_or_else(|| {
                    invalid(format!(
                        "{what} entry {entry_index} `Properties` must be a compound tag"
                    ))
                })?;
                for (key, value) in properties_tag {
                    let value = value.as_str().ok_or_else(|| {
                        invalid(format!(
                            "{what} entry {entry_index} property `{key}` must be a string"
                        ))
                    })?;
                    properties.push((key.clone(), value.to_owned()));
                }
                // `BlockState` property order is canonical in Java. This crate
                // stores properties as a vector, so sort them to make parsing
                // deterministic and comparisons independent of NBT order.
                properties.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            }

            Ok(BlockState {
                name: name.to_owned(),
                properties,
            })
        })
        .collect()
}

fn parse_blocks(
    tag: Option<&Tag>,
    size: [i32; 3],
    palettes: &[Vec<BlockState>],
) -> io::Result<Vec<TemplateBlock>> {
    let Some(tag) = tag else {
        return Ok(Vec::new());
    };
    let entries = list(tag, "structure template blocks")?;
    let mut blocks = Vec::with_capacity(entries.len());

    for (index, entry) in entries.iter().enumerate() {
        let entry = entry.as_compound().ok_or_else(|| {
            invalid(format!(
                "structure template block {index} must be a compound tag"
            ))
        })?;
        let pos = int_list3(
            field(entry, "pos"),
            &format!("structure template block {index} pos"),
        )?;
        for (axis, value) in pos.iter().copied().enumerate() {
            if value < 0 || value >= size[axis] {
                return Err(invalid(format!(
                    "structure template block {index} pos {pos:?} is outside size {size:?}"
                )));
            }
        }

        let state = match field(entry, "state") {
            None => 0,
            Some(tag) => tag.as_int().ok_or_else(|| {
                invalid(format!(
                    "structure template block {index} state must be an int"
                ))
            })?,
        };
        if state < 0 {
            return Err(invalid(format!(
                "structure template block {index} has negative palette index {state}"
            )));
        }
        for (palette_index, palette) in palettes.iter().enumerate() {
            if state as usize >= palette.len() {
                return Err(invalid(format!(
                    "structure template block {index} palette index {state} is out of range \
                     for palette {palette_index} with {} entries",
                    palette.len()
                )));
            }
        }

        let nbt = match field(entry, "nbt") {
            None => None,
            Some(tag) => {
                tag.as_compound().ok_or_else(|| {
                    invalid(format!(
                        "structure template block {index} nbt must be a compound tag"
                    ))
                })?;
                Some(tag.clone())
            }
        };

        blocks.push(TemplateBlock {
            pos: BlockPos::new(pos[0], pos[1], pos[2]),
            state: state as usize,
            nbt,
        });
    }

    // `StructureTemplate.buildInfoList` sorts normal blocks, special-shape
    // blocks, and NBT-bearing blocks by y/x/z and concatenates the groups. The
    // port cannot distinguish special shapes without block behaviour, so it
    // keeps the observable split that matters here: NBT-bearing blocks last.
    blocks.sort_by_key(|block| {
        (
            block.nbt.is_some(),
            block.pos.y(),
            block.pos.x(),
            block.pos.z(),
        )
    });
    Ok(blocks)
}

fn parse_entities(tag: Option<&Tag>) -> io::Result<Vec<TemplateEntity>> {
    let Some(tag) = tag else {
        return Ok(Vec::new());
    };
    let entries = list(tag, "structure template entities")?;
    let mut entities = Vec::with_capacity(entries.len());

    for (index, entry) in entries.iter().enumerate() {
        let entry = entry.as_compound().ok_or_else(|| {
            invalid(format!(
                "structure template entity {index} must be a compound tag"
            ))
        })?;
        let pos = match field(entry, "pos") {
            None => Vec3::ZERO,
            Some(tag) => numeric_list3(tag, &format!("structure template entity {index} pos"))?,
        };
        let block_pos = match field(entry, "blockPos") {
            None => BlockPos::ZERO,
            Some(tag) => {
                let values = int_list3(
                    Some(tag),
                    &format!("structure template entity {index} blockPos"),
                )?;
                BlockPos::new(values[0], values[1], values[2])
            }
        };
        let nbt = match field(entry, "nbt") {
            None => None,
            Some(tag) => {
                tag.as_compound().ok_or_else(|| {
                    invalid(format!(
                        "structure template entity {index} nbt must be a compound tag"
                    ))
                })?;
                Some(tag.clone())
            }
        };

        entities.push(TemplateEntity {
            pos,
            block_pos,
            nbt,
        });
    }

    Ok(entities)
}

fn field<'a>(entries: &'a [(String, Tag)], key: &str) -> Option<&'a Tag> {
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn list<'a>(tag: &'a Tag, what: &str) -> io::Result<&'a [Tag]> {
    tag.as_list()
        .ok_or_else(|| invalid(format!("{what} must be a list tag")))
}

fn int_list3(tag: Option<&Tag>, what: &str) -> io::Result<[i32; 3]> {
    let tag = tag.ok_or_else(|| invalid(format!("{what} is missing")))?;
    let values = list(tag, what)?;
    if values.len() != 3 {
        return Err(invalid(format!(
            "{what} must contain exactly 3 ints, got {} entries",
            values.len()
        )));
    }
    let mut result = [0; 3];
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .as_int()
            .ok_or_else(|| invalid(format!("{what} entry {index} must be an int")))?;
    }
    Ok(result)
}

fn numeric_list3(tag: &Tag, what: &str) -> io::Result<Vec3> {
    let values = list(tag, what)?;
    if values.len() != 3 {
        return Err(invalid(format!(
            "{what} must contain exactly 3 numbers, got {} entries",
            values.len()
        )));
    }
    Ok(Vec3::new(
        numeric(&values[0], what, 0)?,
        numeric(&values[1], what, 1)?,
        numeric(&values[2], what, 2)?,
    ))
}

fn numeric(tag: &Tag, what: &str, index: usize) -> io::Result<f64> {
    match tag {
        Tag::Byte(value) => Ok(f64::from(*value)),
        Tag::Short(value) => Ok(f64::from(*value)),
        Tag::Int(value) => Ok(f64::from(*value)),
        Tag::Long(value) => Ok(*value as f64),
        Tag::Float(value) => Ok(f64::from(*value)),
        Tag::Double(value) => Ok(*value),
        _ => Err(invalid(format!("{what} entry {index} must be numeric"))),
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
