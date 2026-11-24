use crate::chunk::{BlockState, is_air_state};
use crate::nbt::Tag;
use crate::tags::TagRegistryKind;
use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::BoundingBox;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::PlacedBlock;
use folia_math::{BlockPos, Direction};

pub const GENERATION_HEIGHT: i32 = 90;

#[derive(Debug, Clone, PartialEq)]
pub struct BuriedTreasurePiece {
    bounding_box: BoundingBox,
}

impl BuriedTreasurePiece {
    pub fn new(pos: BlockPos) -> Self {
        Self {
            bounding_box: BoundingBox::from_pos(pos),
        }
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box
    }

    pub fn find_chest_position(
        &self,
        height_source: &dyn FeatureLevel,
        level: &dyn FeatureLevel,
    ) -> Option<BlockPos> {
        let mut y = height_source.heightmap(
            Heightmap::OceanFloorWg,
            self.bounding_box.min_x(),
            self.bounding_box.min_z(),
        )?;
        let min_y = level.min_y();
        while y > min_y {
            let pos = BlockPos::new(self.bounding_box.min_x(), y, self.bounding_box.min_z());
            if level.get_block(pos.below()).is_some_and(is_chest_support) {
                return Some(pos);
            }
            y -= 1;
        }
        None
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
        let Some(chest_pos) = self.find_chest_position(source, level) else {
            return Ok(StartPlacement::default());
        };
        let support = level
            .get_block(chest_pos.below())
            .cloned()
            .expect("chest support was found in the writable level");
        let current = level.get_block(chest_pos).cloned();
        let waterlogged = current.as_ref().is_some_and(is_liquid);
        let soft = current
            .filter(|state| !is_air_state(state) && !is_liquid(state))
            .unwrap_or_else(|| state("minecraft:sand"));
        let mut result = StartPlacement::default();

        for direction in Direction::ALL {
            let pos = chest_pos.offset(direction.step_x(), direction.step_y(), direction.step_z());
            if !chunk_box.is_inside(pos)
                || !level
                    .get_block(pos)
                    .is_none_or(|state| is_air_state(state) || is_liquid(state))
            {
                continue;
            }
            let below_is_open = level
                .get_block(pos.below())
                .is_none_or(|state| is_air_state(state) || is_liquid(state));
            let replacement = if below_is_open && direction != Direction::Up {
                support.clone()
            } else {
                soft.clone()
            };
            if level.set_block(pos, replacement) {
                result.written += 1;
                result.written_blocks.push(pos);
            } else {
                result.rejected += 1;
            }
        }

        let mut chest = state("minecraft:chest");
        chest.properties = vec![
            ("facing".to_owned(), "east".to_owned()),
            ("type".to_owned(), "single".to_owned()),
            ("waterlogged".to_owned(), waterlogged.to_string()),
        ];
        if level.set_block(chest_pos, chest.clone()) {
            result.written += 1;
            result.written_blocks.push(chest_pos);
            result.block_entities.push(PlacedBlock {
                pos: chest_pos,
                state: chest,
                nbt: Some(Tag::Compound(vec![
                    ("id".to_owned(), Tag::String("minecraft:chest".to_owned())),
                    (
                        "LootTable".to_owned(),
                        Tag::String("minecraft:chests/buried_treasure".to_owned()),
                    ),
                ])),
            });
        } else {
            result.rejected += 1;
        }
        result.placed_pieces = 1;
        result
            .written_blocks
            .sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
        result.written_blocks.dedup();
        Ok(result)
    }
}

pub struct BuriedTreasureStart;

impl BuriedTreasureStart {
    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
    ) -> Result<StructureStart, StructureError> {
        let pos = BlockPos::new(
            (context.chunk_pos.x << 4) + 9,
            GENERATION_HEIGHT,
            (context.chunk_pos.z << 4) + 9,
        );
        Ok(StructureStart::new_buried_treasure(
            structure_id,
            context.chunk_pos,
            context.references,
            BuriedTreasurePiece::new(pos),
            terrain_adaptation,
        ))
    }
}

fn is_chest_support(state: &BlockState) -> bool {
    state.name == "minecraft:sandstone"
        || crate::tags::block_has("minecraft:base_stone_overworld", &state.name)
}

fn is_liquid(state: &BlockState) -> bool {
    crate::tags::has(TagRegistryKind::Fluid, "minecraft:water", &state.name)
        || crate::tags::has(TagRegistryKind::Fluid, "minecraft:lava", &state.name)
        || state
            .properties
            .iter()
            .any(|(key, value)| key == "waterlogged" && value == "true")
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}
