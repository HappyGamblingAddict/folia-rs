use folia_core::chunk::BlockState;
use folia_core::math::{BlockPos, Direction};
use folia_core::region::ChunkPos;
use folia_core::worldgen::end::EndGenerator;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::pool::PoolCatalog;
use folia_core::worldgen::structure::processor::StructureProcessorCatalog;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructurePlacement,
};
use folia_core::worldgen::structure::start::{
    BiomeSet, StructureGenerationContext, StructureGenerator, StructureKind, TerrainAdjustment,
};
use folia_core::worldgen::structure::template::Rotation;
use folia_random::LegacyRandomSource;
use std::collections::{BTreeSet, HashMap};

const SEED: i64 = 12_345;

#[derive(Default)]
struct MockLevel {
    blocks: HashMap<BlockPos, BlockState>,
    height: i32,
}

impl FeatureLevel for MockLevel {
    fn min_y(&self) -> i32 {
        0
    }

    fn height(&self) -> i32 {
        256
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if !(0..256).contains(&pos.y()) {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        Some(self.height)
    }
}

struct StaticBiomes(&'static str);

impl StructureBiomeLookup for StaticBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        self.0
    }
}

#[test]
fn end_city_uses_the_shipped_procedural_definition_and_templates() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let definition = structures
        .definitions()
        .get("minecraft:end_city")
        .expect("shipped end city definition");
    assert_eq!(definition.name, "minecraft:end_city");
    assert_eq!(definition.kind, StructureKind::EndCity);
    assert_eq!(definition.step(), Decoration::SurfaceStructures);
    assert_eq!(definition.terrain_adaptation, TerrainAdjustment::None);
    assert_eq!(
        definition.biomes,
        BiomeSet::Tag("#minecraft:has_structure/end_city".to_owned())
    );

    let expected = [
        "minecraft:end_city/base_floor",
        "minecraft:end_city/base_roof",
        "minecraft:end_city/bridge_end",
        "minecraft:end_city/bridge_gentle_stairs",
        "minecraft:end_city/bridge_piece",
        "minecraft:end_city/bridge_steep_stairs",
        "minecraft:end_city/fat_tower_base",
        "minecraft:end_city/fat_tower_middle",
        "minecraft:end_city/fat_tower_top",
        "minecraft:end_city/second_floor_1",
        "minecraft:end_city/second_floor_2",
        "minecraft:end_city/second_roof",
        "minecraft:end_city/ship",
        "minecraft:end_city/third_floor_1",
        "minecraft:end_city/third_floor_2",
        "minecraft:end_city/third_roof",
        "minecraft:end_city/tower_base",
        "minecraft:end_city/tower_piece",
        "minecraft:end_city/tower_top",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let actual = StructureTemplateCatalog::shipped()
        .merged_ids()
        .filter(|id| id.starts_with("minecraft:end_city/"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual.len(), 20);
    assert!(
        expected.is_subset(&actual),
        "missing templates: {expected:?}"
    );
    assert!(
        PoolCatalog::shipped()
            .pools()
            .all(|(name, _)| !name.starts_with("minecraft:end_city/"))
    );
    assert!(
        StructureProcessorCatalog::shipped()
            .lists()
            .all(|(name, _)| !name.starts_with("minecraft:end_city"))
    );
    assert!(
        StructureGenerator::shipped_catalog()
            .expect("default generator")
            .structure_at_chunk(
                3,
                1,
                SEED,
                StructureDimension::End,
                64,
                &StaticBiomes("minecraft:end_highlands"),
            )
            .expect("default end city selection")
            .is_some()
    );
}

#[test]
fn end_city_selection_is_triangular_end_only_and_biome_filtered() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let set = structures.sets().get("end_cities").expect("end cities set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("end cities must use random spread");
    };
    assert_eq!((placement.spacing, placement.separation), (20, 11));
    assert_eq!(placement.salt, 10_387_313);
    assert_eq!(
        placement.get_potential_structure_chunk(SEED, 0, 0),
        ChunkPos::new(3, 1)
    );
    assert!(placement.is_placement_chunk(SEED, 3, 1));

    let selected = structures
        .structure_at_chunk(
            3,
            1,
            SEED,
            StructureDimension::End,
            64,
            &StaticBiomes("minecraft:end_highlands"),
        )
        .expect("end city selection")
        .expect("highlands end city");
    assert_eq!(selected.structure, "minecraft:end_city");
    assert_eq!(selected.set, "minecraft:end_cities");
    for dimension in [StructureDimension::Overworld, StructureDimension::Nether] {
        assert!(
            structures
                .structure_at_chunk(
                    3,
                    1,
                    SEED,
                    dimension,
                    64,
                    &StaticBiomes("minecraft:end_highlands"),
                )
                .expect("dimension rejection")
                .is_none()
        );
    }
    assert!(
        structures
            .structure_at_chunk(
                3,
                1,
                SEED,
                StructureDimension::End,
                64,
                &StaticBiomes("minecraft:end_barrens"),
            )
            .expect("barrens rejection")
            .is_none()
    );
}

#[test]
fn fixed_seed_end_city_expands_core_towers_bridges_and_ship() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let definition = structures
        .definitions()
        .get("minecraft:end_city")
        .expect("end city definition");
    let level = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let context = StructureGenerationContext::new(
        1,
        ChunkPos::new(3, 1),
        0,
        structures.template_catalog().expect("template catalog"),
    )
    .with_level(&level);
    let start = definition.generate_start(&context).expect("end city start");
    assert!(start.is_valid());
    let pieces = start.end_city().expect("procedural end city").pieces();
    assert_eq!(pieces.len(), 126);
    assert_eq!(
        pieces[..4]
            .iter()
            .map(|piece| piece.template_name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "minecraft:end_city/base_floor",
            "minecraft:end_city/second_floor_1",
            "minecraft:end_city/third_floor_1",
            "minecraft:end_city/third_roof",
        ]
    );
    assert_eq!(pieces[0].template_position, BlockPos::new(55, 80, 23));
    assert_eq!(pieces[0].rotation, Rotation::Clockwise180);
    assert!(pieces.iter().any(|piece| {
        piece.template_name == "minecraft:end_city/tower_base"
            || piece.template_name == "minecraft:end_city/tower_piece"
            || piece.template_name == "minecraft:end_city/tower_top"
    }));
    let ships = pieces
        .iter()
        .filter(|piece| piece.template_name == "minecraft:end_city/ship")
        .collect::<Vec<_>>();
    assert_eq!(ships.len(), 1);
    assert_eq!(ships[0].template_position, BlockPos::new(133, 143, -7));
    assert_eq!(
        ships[0].bounding_box(),
        BoundingBox::new(105, 143, -7, 133, 166, 5)
    );

    let raw = start.pieces_bounding_box().expect("piece bounds");
    assert_eq!(raw, BoundingBox::new(11, 80, -38, 133, 180, 28));
    assert_eq!(start.bounding_box(), Some(raw));
    assert_eq!(
        start,
        definition.generate_start(&context).expect("second start")
    );
}

#[test]
fn end_city_places_chests_and_purpur_only_in_the_surface_step() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let mut level = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let placement = structures
        .place_for_chunk(
            &mut level,
            ChunkPos::new(3, 1),
            SEED,
            StructureDimension::End,
            64,
            &StaticBiomes("minecraft:end_highlands"),
            0,
            Decoration::SurfaceStructures,
        )
        .expect("end city surface placement");
    assert_eq!(placement.generated_starts, 1);
    assert!(placement.placed_pieces > 0);
    assert!(placement.written > 0);
    assert!(level.blocks.values().any(|state| {
        matches!(
            state.name.as_str(),
            "minecraft:purpur_block" | "minecraft:purpur_pillar" | "minecraft:end_stone_bricks"
        )
    }));
    assert!(
        level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw"
                && state.name != "minecraft:structure_block")
    );

    let mut underground = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let skipped = structures
        .place_for_chunk(
            &mut underground,
            ChunkPos::new(3, 1),
            SEED,
            StructureDimension::End,
            64,
            &StaticBiomes("minecraft:end_highlands"),
            0,
            Decoration::UndergroundStructures,
        )
        .expect("end city underground pass");
    assert_eq!(skipped.generated_starts, 0);
    assert_eq!(skipped.written, 0);
    assert!(underground.blocks.is_empty());
}

#[test]
fn end_city_chest_markers_materialize_rotated_loot_chests() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let definition = structures
        .definitions()
        .get("minecraft:end_city")
        .expect("end city definition");
    let level = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let context = StructureGenerationContext::new(
        1,
        ChunkPos::new(3, 1),
        0,
        structures.template_catalog().expect("template catalog"),
    )
    .with_level(&level);
    let start = definition.generate_start(&context).expect("end city start");
    let piece = start
        .end_city()
        .expect("procedural end city")
        .pieces()
        .iter()
        .find(|piece| {
            piece.template.blocks().iter().any(|block| {
                block
                    .nbt
                    .as_ref()
                    .and_then(|nbt| nbt.get("metadata"))
                    .and_then(folia_core::nbt::Tag::as_str)
                    .is_some_and(|name| name.starts_with("Chest"))
            })
        })
        .expect("chest marker piece");
    let mut placed = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let mut random = LegacyRandomSource::new(7);
    let placement = piece
        .place(&mut placed, piece.bounding_box(), &mut random)
        .expect("chest marker placement");
    let expected_facing = match piece.rotation.rotate_direction(Direction::South) {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    };
    assert!(placed.blocks.values().any(|state| {
        state.name == "minecraft:chest"
            && state
                .properties
                .iter()
                .any(|(key, value)| key == "facing" && value == expected_facing)
    }));
    assert!(placement.block_entities.iter().any(|block| {
        block.state.name == "minecraft:chest"
            && block
                .nbt
                .as_ref()
                .and_then(|nbt| nbt.get("LootTable"))
                .and_then(folia_core::nbt::Tag::as_str)
                == Some("minecraft:chests/end_city_treasure")
    }));
}

fn count_purpur(chunk: &folia_core::chunk::Chunk) -> usize {
    let mut purpur = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..256 {
                match chunk.block_state(x, y, z).expect("End block").name.as_str() {
                    "minecraft:purpur_block" | "minecraft:purpur_pillar" => purpur += 1,
                    "minecraft:jigsaw" | "minecraft:structure_block" => {
                        panic!("structure marker survived placement")
                    }
                    _ => {}
                }
            }
        }
    }
    purpur
}

#[test]
fn generated_end_chunks_contain_end_city_blocks_across_borders() {
    let generator = EndGenerator::new(SEED).expect("End generator");
    let selection = generator
        .structure_at_chunk(-236, -177)
        .expect("End structure selection")
        .expect("generated End city");
    assert_eq!(selection.structure, "minecraft:end_city");
    assert_eq!(selection.set, "minecraft:end_cities");
    assert!(matches!(
        generator.biome_at_block((-236 << 4) + 8, 64, (-177 << 4) + 8),
        "minecraft:end_highlands" | "minecraft:end_midlands"
    ));
    let start = generator
        .structure_start(&selection)
        .expect("End city start");
    assert!(start.is_valid());
    assert_eq!(
        start.pieces_bounding_box(),
        Some(BoundingBox::new(-3781, 61, -2903, -3750, 103, -2813))
    );

    let chunk = generator
        .generate_chunk(-236, -177)
        .expect("End city chunk");
    assert_eq!(count_purpur(&chunk), 751);
    let neighbour = generator
        .generate_chunk(-237, -177)
        .expect("neighbouring End city chunk");
    assert_eq!(count_purpur(&neighbour), 112);
}

#[test]
fn end_city_seed_sweep_stays_inside_the_structure_search_radius() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let definition = structures
        .definitions()
        .get("minecraft:end_city")
        .expect("end city definition");
    let level = MockLevel {
        height: 80,
        ..MockLevel::default()
    };
    let mut max_chunk_distance = 0;
    for seed in 0..512 {
        let context = StructureGenerationContext::new(
            seed,
            ChunkPos::new(0, 0),
            0,
            structures.template_catalog().expect("template catalog"),
        )
        .with_level(&level);
        let start = definition.generate_start(&context).expect("end city start");
        let Some(bounds) = start.pieces_bounding_box() else {
            continue;
        };
        max_chunk_distance = max_chunk_distance.max(
            bounds
                .min_x()
                .div_euclid(16)
                .abs()
                .max(bounds.max_x().div_euclid(16).abs())
                .max(
                    bounds
                        .min_z()
                        .div_euclid(16)
                        .abs()
                        .max(bounds.max_z().div_euclid(16).abs()),
                ),
        );
    }
    assert!(max_chunk_distance <= 16);
}

#[test]
fn end_city_rejects_a_start_below_y_60() {
    let structures = StructureGenerator::shipped_catalog().expect("shipped structure generator");
    let definition = structures
        .definitions()
        .get("minecraft:end_city")
        .expect("end city definition");
    let level = MockLevel {
        height: 59,
        ..MockLevel::default()
    };
    let context = StructureGenerationContext::new(
        SEED,
        ChunkPos::new(3, 1),
        0,
        structures.template_catalog().expect("template catalog"),
    )
    .with_level(&level);
    let start = definition
        .generate_start(&context)
        .expect("low end city start");
    assert!(!start.is_valid());
}
