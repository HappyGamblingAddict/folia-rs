//! Ports of `BoundingBox` and the `StructurePiece` data model.
//!
//! The pieces themselves are data. Placement is a pure function of the piece,
//! its template/settings, and the chunk bounding box being generated, which
//! keeps starts deterministic and testable without a live world.

use crate::chunk::{BlockState, is_air_state};
use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, StructureTemplate,
    transform, transform_vec3,
};
use folia_math::{BlockPos, Direction, Vec3};

/// `net.minecraft.world.level.levelgen.structure.BoundingBox`.
///
/// Bounds are inclusive on every axis. Inverted input is normalized like
/// Java's constructor (Java logs first; a library port does not need a logger).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundingBox {
    min_x: i32,
    min_y: i32,
    min_z: i32,
    max_x: i32,
    max_y: i32,
    max_z: i32,
}

impl BoundingBox {
    pub fn new(min_x: i32, min_y: i32, min_z: i32, max_x: i32, max_y: i32, max_z: i32) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            min_z: min_z.min(max_z),
            max_x: min_x.max(max_x),
            max_y: min_y.max(max_y),
            max_z: min_z.max(max_z),
        }
    }

    pub fn from_pos(pos: BlockPos) -> Self {
        Self::new(pos.x(), pos.y(), pos.z(), pos.x(), pos.y(), pos.z())
    }

    pub fn from_corners(first: BlockPos, second: BlockPos) -> Self {
        Self::new(
            first.x().min(second.x()),
            first.y().min(second.y()),
            first.z().min(second.z()),
            first.x().max(second.x()),
            first.y().max(second.y()),
            first.z().max(second.z()),
        )
    }

    pub const fn min_x(self) -> i32 {
        self.min_x
    }

    pub const fn min_y(self) -> i32 {
        self.min_y
    }

    pub const fn min_z(self) -> i32 {
        self.min_z
    }

    pub const fn max_x(self) -> i32 {
        self.max_x
    }

    pub const fn max_y(self) -> i32 {
        self.max_y
    }

    pub const fn max_z(self) -> i32 {
        self.max_z
    }

    pub const fn x_span(self) -> i32 {
        self.max_x - self.min_x + 1
    }

    pub const fn y_span(self) -> i32 {
        self.max_y - self.min_y + 1
    }

    pub const fn z_span(self) -> i32 {
        self.max_z - self.min_z + 1
    }

    pub const fn is_inside(self, pos: BlockPos) -> bool {
        pos.x() >= self.min_x
            && pos.x() <= self.max_x
            && pos.z() >= self.min_z
            && pos.z() <= self.max_z
            && pos.y() >= self.min_y
            && pos.y() <= self.max_y
    }

    /// `BoundingBox.intersects(BoundingBox)`.
    pub const fn intersects(self, other: BoundingBox) -> bool {
        self.max_x >= other.min_x
            && self.min_x <= other.max_x
            && self.max_z >= other.min_z
            && self.min_z <= other.max_z
            && self.max_y >= other.min_y
            && self.min_y <= other.max_y
    }

    /// `BoundingBox.intersects(int, int, int, int)`: horizontal only.
    pub const fn intersects_xz(self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        self.max_x >= min_x && self.min_x <= max_x && self.max_z >= min_z && self.min_z <= max_z
    }

    /// `BoundingBox.encapsulate(BoundingBox)`.
    pub fn encapsulate(&mut self, other: BoundingBox) {
        self.min_x = self.min_x.min(other.min_x);
        self.min_y = self.min_y.min(other.min_y);
        self.min_z = self.min_z.min(other.min_z);
        self.max_x = self.max_x.max(other.max_x);
        self.max_y = self.max_y.max(other.max_y);
        self.max_z = self.max_z.max(other.max_z);
    }

    /// `BoundingBox.encapsulatingBoxes`.
    pub fn encapsulating(iter: impl IntoIterator<Item = BoundingBox>) -> Option<Self> {
        let mut iter = iter.into_iter();
        let first = iter.next()?;
        let mut result = first;
        for box_ in iter {
            result.encapsulate(box_);
        }
        Some(result)
    }

    /// `BoundingBox.moved(int, int, int)`.
    pub fn moved(self, x: i32, y: i32, z: i32) -> Self {
        Self::new(
            self.min_x + x,
            self.min_y + y,
            self.min_z + z,
            self.max_x + x,
            self.max_y + y,
            self.max_z + z,
        )
    }

    /// `BoundingBox.inflatedBy(int)`.
    pub fn inflated_by(self, value: i32) -> Self {
        self.inflated_by_xyz(value, value, value)
    }

    /// `BoundingBox.inflatedBy(int, int, int)`.
    pub fn inflated_by_xyz(self, x: i32, y: i32, z: i32) -> Self {
        Self::new(
            self.min_x - x,
            self.min_y - y,
            self.min_z - z,
            self.max_x + x,
            self.max_y + y,
            self.max_z + z,
        )
    }

    /// `BoundingBox.getCenter`.
    pub fn center(self) -> BlockPos {
        BlockPos::new(
            self.min_x + self.x_span() / 2,
            self.min_y + self.y_span() / 2,
            self.min_z + self.z_span() / 2,
        )
    }
}

/// `StructurePiece.makeBoundingBox`.
pub fn make_bounding_box(
    x: i32,
    y: i32,
    z: i32,
    direction: Direction,
    offset_x: i32,
    offset_y: i32,
    offset_z: i32,
) -> BoundingBox {
    if matches!(direction, Direction::North | Direction::South) {
        BoundingBox::new(
            x,
            y,
            z,
            x + offset_x - 1,
            y + offset_y - 1,
            z + offset_z - 1,
        )
    } else {
        BoundingBox::new(
            x,
            y,
            z,
            x + offset_z - 1,
            y + offset_y - 1,
            z + offset_x - 1,
        )
    }
}

/// `StructureTemplate.getBoundingBox(startPos, rotation, pivot, mirror)`.
pub fn template_bounding_box(
    template: &StructureTemplate,
    start_pos: BlockPos,
    rotation: Rotation,
    pivot: BlockPos,
    mirror: Mirror,
) -> BoundingBox {
    let last = BlockPos::new(
        template.size_x() - 1,
        template.size_y() - 1,
        template.size_z() - 1,
    );
    let first_corner = transform(BlockPos::ZERO, mirror, rotation, pivot);
    let second_corner = transform(last, mirror, rotation, pivot);
    BoundingBox::from_corners(first_corner, second_corner).moved(
        start_pos.x(),
        start_pos.y(),
        start_pos.z(),
    )
}

/// `IglooPieces` template part. The pivot and offset tables live with the
/// structure generator, but placement needs the part to apply Java's
/// `postProcess` height shift and top snow-block fix-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IglooPart {
    Top,
    Middle,
    Bottom,
}

impl IglooPart {
    /// `IglooPieces.OFFSETS.get(location)`.
    pub const fn offset(self) -> BlockPos {
        match self {
            IglooPart::Top => BlockPos::new(0, 0, 0),
            IglooPart::Middle => BlockPos::new(2, -3, 4),
            IglooPart::Bottom => BlockPos::new(0, -3, -2),
        }
    }

    /// `IglooPieces.PIVOTS.get(location)`.
    pub const fn rotation_pivot(self) -> BlockPos {
        match self {
            IglooPart::Top => BlockPos::new(3, 5, 5),
            IglooPart::Middle => BlockPos::new(1, 3, 1),
            IglooPart::Bottom => BlockPos::new(3, 6, 7),
        }
    }
}

/// Piece-specific post-processing that runs before the template is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplatePostProcess {
    /// Use the template position unchanged.
    None,
    /// `IglooPieces.IglooPiece.postProcess`: snap the generation height of 90
    /// to the local `WORLD_SURFACE_WG` and add the top snow block when needed.
    Igloo(IglooPart),
}

/// `BlockIgnoreProcessor` subset used by the shipped template structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemplateBlockFilter {
    /// Place every template entry.
    #[default]
    None,
    /// `BlockIgnoreProcessor.STRUCTURE_BLOCK`: skip `minecraft:structure_block`.
    IgnoreStructureBlock,
}

impl TemplateBlockFilter {
    fn ignores(self, state: &BlockState) -> bool {
        matches!(self, TemplateBlockFilter::IgnoreStructureBlock)
            && state.name == "minecraft:structure_block"
    }
}

/// `TemplateStructurePiece`.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateStructurePiece {
    pub template_name: String,
    pub template: StructureTemplate,
    pub place_settings: StructurePlaceSettings,
    pub template_position: BlockPos,
    pub post_process: TemplatePostProcess,
    pub block_filter: TemplateBlockFilter,
    bounding_box: BoundingBox,
}

impl TemplateStructurePiece {
    pub fn new(
        template_name: impl Into<String>,
        template: StructureTemplate,
        template_position: BlockPos,
        place_settings: StructurePlaceSettings,
    ) -> Self {
        let template_name = template_name.into();
        let bounding_box = template_bounding_box(
            &template,
            template_position,
            place_settings.rotation(),
            place_settings.rotation_pivot(),
            place_settings.mirror(),
        );
        Self {
            template_name,
            template,
            place_settings,
            template_position,
            post_process: TemplatePostProcess::None,
            block_filter: TemplateBlockFilter::None,
            bounding_box,
        }
    }

    pub fn with_post_process(mut self, post_process: TemplatePostProcess) -> Self {
        self.post_process = post_process;
        self
    }

    pub fn with_block_filter(mut self, block_filter: TemplateBlockFilter) -> Self {
        self.block_filter = block_filter;
        self
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    /// Java's `templatePosition` after the piece's `postProcess` adjustment.
    pub fn effective_position(&self, level: &dyn FeatureLevel) -> BlockPos {
        match self.post_process {
            TemplatePostProcess::None => self.template_position,
            TemplatePostProcess::Igloo(part) => {
                let offset = part.offset();
                let reference = transform(
                    BlockPos::new(3 - offset.x(), 0, -offset.z()),
                    self.place_settings.mirror(),
                    self.place_settings.rotation(),
                    self.place_settings.rotation_pivot(),
                );
                let reference = self.template_position.offset_by(reference);
                let height = level
                    .heightmap(Heightmap::WorldSurfaceWg, reference.x(), reference.z())
                    .unwrap_or_else(|| level.min_y());
                self.template_position.offset(0, height - 90 - 1, 0)
            }
        }
    }

    /// Places this template clipped to `chunk_box`, exactly like
    /// `StructureTemplate.placeInWorld` with
    /// `StructurePlaceSettings.setBoundingBox(chunkBox)`.
    pub fn place(&self, level: &mut dyn FeatureLevel, chunk_box: BoundingBox) -> PiecePlacement {
        let origin = self.effective_position(level);
        let mut placement = PiecePlacement::default();
        let origin_vec = Vec3::new(
            f64::from(origin.x()),
            f64::from(origin.y()),
            f64::from(origin.z()),
        );

        for block in self.template.placement_blocks(&self.place_settings) {
            let pos = origin.offset_by(block.pos);
            let marker = data_marker_name(block.nbt.as_ref());
            if let Some(name) = &marker
                && chunk_box.is_inside(pos)
            {
                placement.data_markers.push(StructureDataMarker {
                    name: name.clone(),
                    pos,
                    nbt: block.nbt.clone(),
                });
            }

            if self.block_filter.ignores(&block.state) {
                continue;
            }
            if !chunk_box.is_inside(pos) {
                continue;
            }

            if level.set_block(pos, block.state.clone()) {
                placement.written += 1;
                if block.nbt.is_some() && marker.is_none() {
                    placement.block_entities.push(PlacedBlock { pos, ..block });
                }
            } else {
                placement.rejected += 1;
            }
        }

        if !self.place_settings.ignore_entities() {
            for entity in self.template.entities() {
                let pos = transform_vec3(
                    entity.pos,
                    self.place_settings.mirror(),
                    self.place_settings.rotation(),
                    self.place_settings.rotation_pivot(),
                )
                .add(origin_vec);
                let block_pos = origin.offset_by(transform(
                    entity.block_pos,
                    self.place_settings.mirror(),
                    self.place_settings.rotation(),
                    self.place_settings.rotation_pivot(),
                ));
                if chunk_box.is_inside(block_pos) {
                    placement.entities.push(PlacedEntity {
                        pos,
                        block_pos,
                        nbt: entity.nbt.clone(),
                    });
                }
            }
        }

        if matches!(
            self.post_process,
            TemplatePostProcess::Igloo(IglooPart::Top)
        ) {
            let marker = transform(
                BlockPos::new(3, 0, 5),
                self.place_settings.mirror(),
                self.place_settings.rotation(),
                self.place_settings.rotation_pivot(),
            );
            let marker = origin.offset_by(marker);
            let blocked = level
                .get_block(marker.below())
                .map(|state| !is_air_state(state) && state.name != "minecraft:ladder")
                .unwrap_or(false);
            if blocked
                && level.set_block(
                    marker,
                    BlockState {
                        name: "minecraft:snow_block".to_owned(),
                        properties: Vec::new(),
                    },
                )
            {
                placement.written += 1;
            }
        }

        placement
    }
}

fn data_marker_name(nbt: Option<&Tag>) -> Option<String> {
    let nbt = nbt?;
    if nbt.get("id").and_then(Tag::as_str) != Some("minecraft:structure_block") {
        return None;
    }
    if nbt.get("mode").and_then(Tag::as_str) != Some("DATA")
        && nbt.get("mode").and_then(Tag::as_str) != Some("data")
    {
        return None;
    }
    Some(
        nbt.get("metadata")
            .and_then(Tag::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

/// Result of placing one piece.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PiecePlacement {
    pub written: usize,
    pub rejected: usize,
    pub block_entities: Vec<PlacedBlock>,
    pub entities: Vec<PlacedEntity>,
    /// Structure-block `mode: data` markers. The caller decides whether a
    /// chest/loot-table block entity should be created.
    pub data_markers: Vec<StructureDataMarker>,
}

/// A transformed structure-block data marker.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureDataMarker {
    pub name: String,
    pub pos: BlockPos,
    pub nbt: Option<Tag>,
}

/// Piece payloads supported by this slice.
#[derive(Debug, Clone, PartialEq)]
pub enum StructurePieceKind {
    Template(TemplateStructurePiece),
    /// Parsed but not generated piece kind. Placement fails explicitly.
    Unsupported {
        type_name: String,
    },
}

/// `StructurePiece` data shared by every piece kind.
#[derive(Debug, Clone, PartialEq)]
pub struct StructurePiece {
    kind: StructurePieceKind,
    bounding_box: BoundingBox,
    gen_depth: i32,
    orientation: Option<Direction>,
}

impl StructurePiece {
    pub fn new(
        kind: StructurePieceKind,
        bounding_box: BoundingBox,
        gen_depth: i32,
        orientation: Option<Direction>,
    ) -> Self {
        Self {
            kind,
            bounding_box,
            gen_depth,
            orientation,
        }
    }

    pub fn template(piece: TemplateStructurePiece) -> Self {
        let bounding_box = piece.bounding_box();
        Self {
            kind: StructurePieceKind::Template(piece),
            bounding_box,
            gen_depth: 0,
            // `TemplateStructurePiece` sets orientation NORTH in Java.
            orientation: Some(Direction::North),
        }
    }

    pub fn unsupported(type_name: impl Into<String>, bounding_box: BoundingBox) -> Self {
        Self {
            kind: StructurePieceKind::Unsupported {
                type_name: type_name.into(),
            },
            bounding_box,
            gen_depth: 0,
            orientation: None,
        }
    }

    pub fn kind(&self) -> &StructurePieceKind {
        &self.kind
    }

    pub fn as_template(&self) -> Option<&TemplateStructurePiece> {
        match &self.kind {
            StructurePieceKind::Template(piece) => Some(piece),
            StructurePieceKind::Unsupported { .. } => None,
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn gen_depth(&self) -> i32 {
        self.gen_depth
    }

    pub fn orientation(&self) -> Option<Direction> {
        self.orientation
    }

    pub fn rotation(&self) -> Rotation {
        match &self.kind {
            StructurePieceKind::Template(piece) => piece.place_settings.rotation(),
            StructurePieceKind::Unsupported { .. } => Rotation::None,
        }
    }

    pub fn mirror(&self) -> Mirror {
        match &self.kind {
            StructurePieceKind::Template(piece) => piece.place_settings.mirror(),
            StructurePieceKind::Unsupported { .. } => Mirror::None,
        }
    }

    /// `StructurePiece.getLocatorPosition`.
    pub fn locator_position(&self) -> BlockPos {
        self.bounding_box.center()
    }

    /// `StructurePiece.isCloseToChunk`.
    pub fn is_close_to_chunk(&self, chunk_x: i32, chunk_z: i32, distance: i32) -> bool {
        let min_x = chunk_x << 4;
        let min_z = chunk_z << 4;
        self.bounding_box.intersects_xz(
            min_x - distance,
            min_z - distance,
            min_x + 15 + distance,
            min_z + 15 + distance,
        )
    }

    /// `StructurePiece.postProcess` for the supported piece kinds.
    ///
    /// `None` means the piece does not intersect the requested chunk box and
    /// was not attempted, matching `StructureStart.placeInChunk`'s filter.
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<Option<PiecePlacement>, StructureError> {
        if !self.bounding_box.intersects(chunk_box) {
            return Ok(None);
        }
        match &self.kind {
            StructurePieceKind::Template(piece) => Ok(Some(piece.place(level, chunk_box))),
            StructurePieceKind::Unsupported { type_name } => {
                Err(StructureError::UnsupportedPiece {
                    piece_type: type_name.clone(),
                })
            }
        }
    }
}
