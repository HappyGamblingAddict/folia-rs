use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureError;
use folia_core::worldgen::structure::mineshaft::MineshaftPiece;
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{
    BiomeSet, StructureDefinition, StructureGenerationContext, StructureGenerator, StructureKind,
};
use std::collections::{HashMap, HashSet};

const SEED: i64 = 12_345;
const NORMAL_JSON: &str = r##"{ "type": "minecraft:mineshaft", "biomes": "#minecraft:has_structure/mineshaft", "spawn_overrides": {}, "step": "underground_structures" }"##;
const MESA_JSON: &str = r##"{ "type": "minecraft:mineshaft", "biomes": "#minecraft:has_structure/mineshaft_mesa", "spawn_overrides": {}, "step": "underground_structures" }"##;

struct StaticBiomes(&'static str);

impl StructureBiomeLookup for StaticBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        self.0
    }
}

struct GeneratedBiomes<'a> {
    generator: &'a WorldGenerator,
}

impl StructureBiomeLookup for GeneratedBiomes<'_> {
    fn biome_at(&self, x: i32, y: i32, z: i32) -> &str {
        self.generator.biome_at(x, y, z)
    }
}

#[derive(Default)]
struct CaveLevel {
    min_y: i32,
    height: i32,
    surface: Option<i32>,
    cave_min: i32,
    cave_max: i32,
    blocks: HashMap<BlockPos, BlockState>,
}

impl CaveLevel {
    fn new(min_y: i32, height: i32, surface: i32, cave_min: i32, cave_max: i32) -> Self {
        Self {
            min_y,
            height,
            surface: Some(surface),
            cave_min,
            cave_max,
            blocks: HashMap::new(),
        }
    }
}

impl FeatureLevel for CaveLevel {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if pos.y() < self.min_y || pos.y() >= self.min_y + self.height {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        self.surface
    }

    fn terrain_density_at(&self, _x: i32, y: i32, _z: i32) -> Option<f64> {
        Some(if (self.cave_min..=self.cave_max).contains(&y) {
            -0.5
        } else {
            0.5
        })
    }
}

fn normal_start(
    structures: &StructureGenerator,
    chunk: ChunkPos,
    level: &CaveLevel,
) -> folia_core::worldgen::structure::start::StructureStart {
    let definition = structures
        .definitions()
        .get("minecraft:mineshaft")
        .expect("normal mineshaft definition");
    let templates = HashMap::new();
    let context = StructureGenerationContext::new(SEED, chunk, 0, &templates).with_level(level);
    definition
        .generate_start(&context)
        .expect("mineshaft start")
}

#[test]
fn mineshaft_definitions_and_biome_selection_cover_normal_and_mesa() {
    for (name, json, mesa, tag) in [
        (
            "minecraft:mineshaft",
            NORMAL_JSON,
            false,
            "#minecraft:has_structure/mineshaft",
        ),
        (
            "minecraft:mineshaft_mesa",
            MESA_JSON,
            true,
            "#minecraft:has_structure/mineshaft_mesa",
        ),
    ] {
        let definition = StructureDefinition::from_json(name, json).expect("mineshaft JSON");
        assert_eq!(definition.kind, StructureKind::Mineshaft { mesa });
        assert_eq!(definition.step(), Decoration::UndergroundStructures);
        assert_eq!(definition.biomes, BiomeSet::Tag(tag.to_owned()));
    }

    let registry = StructureSetRegistry::shipped().expect("shipped sets");
    let mineshafts = registry.get("mineshafts").expect("mineshaft set");
    assert_eq!(mineshafts.structures.len(), 2);
    assert!(mineshafts.structures.iter().all(|entry| entry.weight == 1));

    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("mineshaft generator")
        .with_enabled_structures(["minecraft:mineshaft", "minecraft:mineshaft_mesa"]);
    for (biome, expected, wrong) in [
        (
            "minecraft:plains",
            "minecraft:mineshaft",
            "minecraft:badlands",
        ),
        (
            "minecraft:badlands",
            "minecraft:mineshaft_mesa",
            "minecraft:plains",
        ),
    ] {
        let chunk = ChunkPos::new(-128, 38);
        let selection = structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes(biome),
            )
            .expect("selection")
            .expect("matching mineshaft biome");
        assert_eq!(selection.structure, expected);
        assert_eq!(selection.set, "minecraft:mineshafts");
        let definition = structures
            .definitions()
            .get(expected)
            .expect("selected mineshaft definition");
        let templates = HashMap::new();
        let level = CaveLevel::new(-64, 384, 80, 18, 48);
        let context =
            StructureGenerationContext::new(SEED, chunk, 0, &templates).with_level(&level);
        let start = definition
            .generate_start(&context)
            .expect("mineshaft start");
        assert_eq!(
            start.mineshaft().expect("mineshaft piece").mesa(),
            expected == "minecraft:mineshaft_mesa"
        );
        let alternate = if expected == "minecraft:mineshaft" {
            "minecraft:mineshaft_mesa"
        } else {
            "minecraft:mineshaft"
        };
        assert_eq!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    SEED,
                    StructureDimension::Overworld,
                    63,
                    &StaticBiomes(wrong),
                )
                .expect("alternate mineshaft biome")
                .expect("weighted fallback variant")
                .structure,
            alternate
        );
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    SEED,
                    StructureDimension::Overworld,
                    63,
                    &StaticBiomes("minecraft:nether_wastes"),
                )
                .expect("incompatible biome selection")
                .is_none()
        );
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    SEED,
                    StructureDimension::Nether,
                    63,
                    &StaticBiomes(biome),
                )
                .expect("Nether selection")
                .is_none()
        );
    }
}

#[test]
fn mineshaft_fixed_seed_pins_cave_height_bounds_and_branching() {
    let structures = StructureGenerator::shipped(HashMap::new()).expect("mineshaft generator");
    let chunk = ChunkPos::new(-128, 38);
    let level = CaveLevel::new(-64, 384, 80, 18, 48);
    let start = normal_start(&structures, chunk, &level);
    assert!(start.is_valid());
    let piece = start.mineshaft().expect("mineshaft piece");
    assert!(!piece.mesa());
    assert_eq!(piece.start_y(), 48);
    assert_eq!(piece.sections().len(), 12);
    assert!(piece.sections().iter().any(|section| section.support()));
    assert!(piece.sections().iter().any(|section| section.is_stairs()));
    let directions: HashSet<_> = piece
        .sections()
        .iter()
        .map(|section| section.direction())
        .collect();
    assert_eq!(directions.len(), 3);
    for direction in [
        folia_core::math::Direction::South,
        folia_core::math::Direction::East,
        folia_core::math::Direction::West,
    ] {
        assert!(directions.contains(&direction));
    }
    let bounds = start.bounding_box().expect("mineshaft bounds");
    assert_eq!(bounds, BoundingBox::new(-2093, 42, 608, -2022, 58, 657));
    assert_eq!(piece.bounding_box(), bounds);
    assert!(bounds.min_y() >= -64);
    assert!(bounds.max_y() <= 319);
    assert_eq!(normal_start(&structures, chunk, &level), start);

    let mut write_level = CaveLevel::new(-64, 384, 80, 18, 48);
    let plains = StaticBiomes("minecraft:plains");
    let underground = structures
        .place_for_chunk_with_height_source(
            &mut write_level,
            &level,
            chunk,
            SEED,
            StructureDimension::Overworld,
            63,
            &plains,
            0,
            Decoration::UndergroundStructures,
        )
        .expect("underground mineshaft pass");
    assert_eq!(underground.generated_starts, 1);
    assert_eq!(underground.placed_pieces, 1);
    assert!(underground.written > 0);
    assert!(!underground.block_entities.is_empty());
    let surface = structures
        .place_for_chunk_with_height_source(
            &mut write_level,
            &level,
            chunk,
            SEED,
            StructureDimension::Overworld,
            63,
            &plains,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("surface mineshaft pass");
    assert_eq!(surface.generated_starts, 0);
    assert_eq!(surface.written, 0);

    let shallow = CaveLevel::new(-64, 384, 80, 41, 51);
    let deep = CaveLevel::new(-64, 384, 80, 55, 68);
    let shallow_y = normal_start(&structures, chunk, &shallow)
        .mineshaft()
        .expect("shallow piece")
        .start_y();
    let deep_y = normal_start(&structures, chunk, &deep)
        .mineshaft()
        .expect("deep piece")
        .start_y();
    assert_eq!(shallow_y, 51);
    assert_eq!(deep_y, 55);
    assert_ne!(shallow_y, deep_y);
}

#[test]
fn mineshaft_placement_clips_chunks_and_preserves_rails_chests_and_supports() {
    let piece = MineshaftPiece::new(SEED, 8, 32, 8, false);
    let bounds = piece.bounding_box();
    let full_box = BoundingBox::new(
        bounds.min_x(),
        -64,
        bounds.min_z(),
        bounds.max_x(),
        319,
        bounds.max_z(),
    );
    let mut full_level = CaveLevel::new(-64, 384, 80, 18, 48);
    full_level.blocks.insert(
        BlockPos::new(10, 33, 10),
        BlockState {
            name: "minecraft:stone".to_owned(),
            properties: Vec::new(),
        },
    );
    let full = piece
        .place(&mut full_level, full_box)
        .expect("full mineshaft placement");
    assert_eq!(full.placed_pieces, 1);
    assert_eq!(full.written, 2734);
    assert_eq!(
        full_level
            .blocks
            .values()
            .filter(|block| block.name.contains("rail"))
            .count(),
        41
    );
    assert!(full.written > 100);
    assert_eq!(
        full_level
            .get_block(BlockPos::new(10, 33, 10))
            .expect("room interior")
            .name,
        "minecraft:air"
    );
    let names: HashSet<_> = full_level
        .blocks
        .values()
        .map(|block| block.name.as_str())
        .collect();
    for name in [
        "minecraft:oak_fence",
        "minecraft:oak_planks",
        "minecraft:cobblestone",
        "minecraft:cobweb",
        "minecraft:rail",
        "minecraft:chest",
    ] {
        assert!(names.contains(name), "normal mineshaft missing {name}");
    }
    assert!(names.contains("minecraft:powered_rail") || names.contains("minecraft:detector_rail"));
    let chests: Vec<_> = full
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .collect();
    assert!(!chests.is_empty());
    for chest in &chests {
        assert!(chest.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
                && nbt.get("LootTable").and_then(Tag::as_str)
                    == Some("minecraft:chests/abandoned_mineshaft")
                && nbt.get("LootTableSeed").and_then(Tag::as_long).is_some()
        }));
    }

    let mut split_level = CaveLevel::new(-64, 384, 80, 18, 48);
    let mut split_chests = Vec::new();
    for chunk_x in (bounds.min_x() >> 4)..=(bounds.max_x() >> 4) {
        for chunk_z in (bounds.min_z() >> 4)..=(bounds.max_z() >> 4) {
            let chunk_box = BoundingBox::new(
                chunk_x << 4,
                -64,
                chunk_z << 4,
                (chunk_x << 4) + 15,
                319,
                (chunk_z << 4) + 15,
            );
            let split = piece
                .place(&mut split_level, chunk_box)
                .expect("clipped mineshaft placement");
            assert!(
                split
                    .written_blocks
                    .iter()
                    .all(|pos| chunk_box.is_inside(*pos))
            );
            assert!(split.written_blocks.windows(2).all(|pair| {
                (pair[0].x(), pair[0].y(), pair[0].z()) < (pair[1].x(), pair[1].y(), pair[1].z())
            }));
            split_chests.extend(
                split
                    .block_entities
                    .iter()
                    .filter(|block| block.state.name == "minecraft:chest")
                    .cloned(),
            );
        }
    }
    assert_eq!(split_level.blocks, full_level.blocks);
    split_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
    let mut full_chests: Vec<_> = chests.into_iter().cloned().collect();
    full_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
    assert_eq!(
        full_chests
            .iter()
            .map(|chest| {
                (
                    chest.pos,
                    chest
                        .nbt
                        .as_ref()
                        .and_then(|nbt| nbt.get("LootTableSeed"))
                        .and_then(Tag::as_long),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (BlockPos::new(15, 33, 9), Some(-7_057_775_208_770_623_388)),
            (BlockPos::new(26, 34, 8), Some(-2_984_074_169_775_996_691)),
            (BlockPos::new(30, 34, 15), Some(4_616_064_995_278_126_490)),
            (BlockPos::new(49, 41, 9), Some(-5_915_675_836_347_226_949)),
        ]
    );
    assert_eq!(split_chests, full_chests);

    let mesa = MineshaftPiece::new(SEED, 8, 32, 8, true);
    let mut mesa_level = CaveLevel::new(-64, 384, 80, 18, 48);
    mesa.place(&mut mesa_level, full_box)
        .expect("mesa mineshaft placement");
    let mesa_names: HashSet<_> = mesa_level
        .blocks
        .values()
        .map(|block| block.name.as_str())
        .collect();
    for name in [
        "minecraft:dark_oak_fence",
        "minecraft:dark_oak_planks",
        "minecraft:dark_oak_log",
        "minecraft:terracotta",
        "minecraft:rail",
        "minecraft:chest",
    ] {
        assert!(mesa_names.contains(name), "mesa mineshaft missing {name}");
    }
    for name in [
        "minecraft:oak_fence",
        "minecraft:oak_planks",
        "minecraft:oak_log",
    ] {
        assert!(!mesa_names.contains(name), "mesa mineshaft leaked {name}");
    }
}

#[test]
fn mineshaft_requires_a_height_source_instead_of_guessing_a_y() {
    let structures = StructureGenerator::shipped(HashMap::new()).expect("mineshaft generator");
    let definition = structures
        .definitions()
        .get("minecraft:mineshaft")
        .expect("mineshaft definition");
    let templates = HashMap::new();
    let context = StructureGenerationContext::new(SEED, ChunkPos::new(0, 0), 0, &templates);
    assert!(matches!(
        definition.generate_start(&context),
        Err(StructureError::MissingHeightSource { .. })
    ));
}

#[test]
fn generated_chunk_contains_mineshaft_rails_supports_and_loot_chest() {
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("mineshaft generator")
        .with_enabled_structures(["minecraft:mineshaft"]);
    let generator = WorldGenerator::overworld(SEED)
        .expect("generator")
        .with_structure_generator(structures);
    let biomes = GeneratedBiomes {
        generator: &generator,
    };
    let structures = generator.structures().expect("structures");
    let chunk_pos = ChunkPos::new(-128, 38);
    let selection = structures
        .structure_at_chunk(
            chunk_pos.x,
            chunk_pos.z,
            SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
        )
        .expect("selection")
        .expect("generated normal mineshaft placement");
    assert_eq!(selection.structure, "minecraft:mineshaft");
    let plain = WorldGenerator::overworld(SEED)
        .expect("plain generator")
        .generate_chunk(chunk_pos.x, chunk_pos.z)
        .expect("plain chunk");
    let structured = generator
        .generate_chunk(chunk_pos.x, chunk_pos.z)
        .expect("structured chunk");
    let mut rails = 0;
    let mut supports = 0;
    let mut chests = Vec::new();
    let mut cobwebs = 0;
    for local_x in 0..16 {
        for local_z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                match structured
                    .block_state(local_x, y, local_z)
                    .expect("structured block")
                    .name
                    .as_str()
                {
                    "minecraft:rail" | "minecraft:powered_rail" | "minecraft:detector_rail" => {
                        rails += 1
                    }
                    "minecraft:oak_fence" | "minecraft:oak_log" => supports += 1,
                    "minecraft:chest" => chests.push((local_x, y, local_z)),
                    "minecraft:cobweb" => cobwebs += 1,
                    _ => {}
                }
            }
        }
    }
    assert!(rails > 0, "mineshaft rails at {chunk_pos:?}");
    assert!(supports > 0, "mineshaft supports at {chunk_pos:?}");
    assert!(!chests.is_empty(), "mineshaft chest at {chunk_pos:?}");
    let surface = generator.base_height((chunk_pos.x << 4) + 8, (chunk_pos.z << 4) + 8);
    assert!(
        chests
            .iter()
            .all(|(_, y, _)| *y >= generator.min_y() && *y + 4 < surface)
    );
    assert!(cobwebs > 0, "mineshaft cobwebs at {chunk_pos:?}");
    assert!((0..16).all(|local_x| (0..16).all(|local_z| {
        (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
            let name = &plain
                .block_state(local_x, y, local_z)
                .expect("plain block")
                .name;
            !matches!(
                name.as_str(),
                "minecraft:rail"
                    | "minecraft:powered_rail"
                    | "minecraft:detector_rail"
                    | "minecraft:chest"
            )
        })
    })));
}
