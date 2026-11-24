use crate::nbt::Tag;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{
    BoundingBox, PiecePlacement, TemplateBlockFilter, TemplateStructurePiece,
};
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::{Rotation, StructurePlaceSettings, StructureTemplate};
use folia_math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource};

pub const GENERATION_HEIGHT: i32 = 90;
pub const DEFAULT_POSITION: BlockPos = BlockPos::new(4, 0, 15);

#[derive(Debug, Clone, PartialEq)]
pub struct ShipwreckPiece {
    template: TemplateStructurePiece,
    beached: bool,
}

impl ShipwreckPiece {
    pub fn new(
        template: StructureTemplate,
        position: BlockPos,
        rotation: Rotation,
        beached: bool,
    ) -> Self {
        let settings = StructurePlaceSettings::new()
            .with_rotation(rotation)
            .with_rotation_pivot(DEFAULT_POSITION)
            .with_ignore_air(true);
        let template =
            TemplateStructurePiece::new(ShipwreckStart::TEMPLATE, template, position, settings)
                .with_block_filter(TemplateBlockFilter::IgnoreStructureBlock);
        Self { template, beached }
    }

    pub fn template(&self) -> &TemplateStructurePiece {
        &self.template
    }

    pub fn is_beached(&self) -> bool {
        self.beached
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.template.bounding_box()
    }

    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        let mut random = LegacyRandomSource::new(
            (i64::from(chunk_box.min_x()) << 32) ^ i64::from(chunk_box.min_z()),
        );
        self.place_with_random(level, chunk_box, &mut random)
    }

    pub fn place_with_height_source(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: &dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        let mut random = LegacyRandomSource::new(
            (i64::from(chunk_box.min_x()) << 32) ^ i64::from(chunk_box.min_z()),
        );
        self.place_with_height_source_and_random(level, height_source, chunk_box, &mut random)
    }

    pub(crate) fn place_with_random(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        self.place_with_optional_height_source(level, None, chunk_box, random)
    }

    pub(crate) fn place_with_height_source_and_random(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: &dyn FeatureLevel,
        chunk_box: BoundingBox,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        self.place_with_optional_height_source(level, Some(height_source), chunk_box, random)
    }

    fn place_with_optional_height_source(
        &self,
        level: &mut dyn FeatureLevel,
        height_source: Option<&dyn FeatureLevel>,
        chunk_box: BoundingBox,
        random: &mut dyn RandomSource,
    ) -> Result<StartPlacement, StructureError> {
        if !self.bounding_box().intersects(chunk_box) {
            return Ok(StartPlacement::default());
        }
        let source = height_source.unwrap_or(&*level);
        let Some(y) = self.placement_y(source, random) else {
            return Ok(StartPlacement::default());
        };
        let mut placed = self.clone();
        placed.template.template_position = BlockPos::new(
            self.template.template_position.x(),
            y,
            self.template.template_position.z(),
        );
        let palette_count = placed.template.template.palette_count();
        if palette_count > 0 {
            placed
                .template
                .place_settings
                .set_palette(Some(random.next_int_bound(palette_count as i32) as usize));
        }
        let mut placement = placed.template.place(level, chunk_box);
        assign_chest_loot(&mut placement, random);
        let mut result = StartPlacement {
            placed_pieces: 1,
            written: placement.written,
            rejected: placement.rejected,
            written_blocks: placement.written_blocks,
            block_entities: placement.block_entities,
            entities: placement.entities,
            data_markers: placement.data_markers,
            ..StartPlacement::default()
        };
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }

    fn placement_y(&self, source: &dyn FeatureLevel, random: &mut dyn RandomSource) -> Option<i32> {
        let size = self.template.template.size();
        let heightmap = if self.beached {
            Heightmap::WorldSurfaceWg
        } else {
            Heightmap::OceanFloorWg
        };
        let mut total = 0_i64;
        let mut lowest = source.min_y().saturating_add(source.height());
        let start = self.template.template_position;
        for z in start.z()..start.z().saturating_add(size[2]) {
            for x in start.x()..start.x().saturating_add(size[0]) {
                let height = source.heightmap(heightmap, x, z)?;
                total += i64::from(height);
                lowest = lowest.min(height);
            }
        }
        let area = i64::from(size[0]) * i64::from(size[2]);
        if self.beached {
            Some(
                lowest
                    .saturating_sub(size[1] / 2)
                    .saturating_sub(random.next_int_bound(3)),
            )
        } else {
            Some((total / area) as i32)
        }
    }
}

fn assign_chest_loot(placement: &mut PiecePlacement, random: &mut dyn RandomSource) {
    for block in &mut placement.block_entities {
        if block.state.name == "minecraft:chest" && block.nbt.is_some() {
            let seed = random.next_long();
            set_nbt_field(&mut block.nbt, "LootTableSeed", Tag::Long(seed));
        }
    }
    for marker in &placement.data_markers {
        let loot_table = match marker.name.as_str() {
            "map_chest" => "minecraft:chests/shipwreck_map",
            "treasure_chest" => "minecraft:chests/shipwreck_treasure",
            "supply_chest" => "minecraft:chests/shipwreck_supply",
            _ => continue,
        };
        let seed = random.next_long();
        let chest_pos = marker.pos.below();
        if let Some(chest) = placement
            .block_entities
            .iter_mut()
            .find(|block| block.pos == chest_pos && block.state.name == "minecraft:chest")
        {
            set_nbt_field(
                &mut chest.nbt,
                "LootTable",
                Tag::String(loot_table.to_owned()),
            );
            set_nbt_field(&mut chest.nbt, "LootTableSeed", Tag::Long(seed));
        }
    }
}

fn set_nbt_field(nbt: &mut Option<Tag>, key: &str, value: Tag) {
    let Some(Tag::Compound(entries)) = nbt.as_mut() else {
        return;
    };
    if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
        *existing = value;
    } else {
        entries.push((key.to_owned(), value));
    }
}

pub struct ShipwreckStart;

impl ShipwreckStart {
    pub const TEMPLATE: &'static str = "minecraft:shipwreck/rightsideup_full";
    pub const GENERATION_HEIGHT: i32 = GENERATION_HEIGHT;
    pub const DEFAULT_POSITION: BlockPos = DEFAULT_POSITION;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
        beached: bool,
    ) -> Result<StructureStart, StructureError> {
        let template = context
            .templates
            .template(Self::TEMPLATE)
            .cloned()
            .ok_or_else(|| StructureError::UnknownTemplate {
                structure: structure_id.to_owned(),
                template: Self::TEMPLATE.to_owned(),
            })?;
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let position = BlockPos::new(
            context.chunk_pos.x << 4,
            Self::GENERATION_HEIGHT,
            context.chunk_pos.z << 4,
        );
        let piece = ShipwreckPiece::new(template, position, rotation, beached);
        Ok(StructureStart::new_shipwreck(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}
