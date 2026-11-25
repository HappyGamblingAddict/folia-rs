use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureError;
use folia_core::worldgen::structure::piece::BoundingBox;
use folia_core::worldgen::structure::set::{
    ConcentricRingsStructurePlacement, StructureBiomeLookup, StructureDimension,
    StructurePlacement, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{
    BiomeSet, StructureDefinition, StructureGenerationContext, StructureGenerator, StructureKind,
};
use folia_core::worldgen::structure::stronghold::{
    StrongholdPiece, StrongholdRoomKind, StrongholdStart,
};
use folia_core::worldgen::structure::template::Rotation;
use std::collections::{HashMap, HashSet};

const SEED: i64 = 12_345;
const STRONGHOLD_JSON: &str = r##"{ "type": "minecraft:stronghold", "biomes": "#minecraft:has_structure/stronghold", "spawn_overrides": {}, "step": "strongholds" }"##;

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

#[derive(Clone, Default)]
struct CaveLevel {
    min_y: i32,
    height: i32,
    surface: Option<i32>,
    cave_min: i32,
    cave_max: i32,
    density: bool,
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
            density: true,
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
        self.density.then(|| {
            if (self.cave_min..=self.cave_max).contains(&y) {
                -0.5
            } else {
                0.5
            }
        })
    }
}

fn structures() -> StructureGenerator {
    StructureGenerator::shipped(HashMap::new())
        .expect("shipped structure generator")
        .with_enabled_structures(["minecraft:stronghold"])
}

fn stronghold_definition(structures: &StructureGenerator) -> &StructureDefinition {
    structures
        .definitions()
        .get("minecraft:stronghold")
        .expect("shipped stronghold definition")
}

fn generate_start(
    structures: &StructureGenerator,
    chunk: ChunkPos,
    level: &CaveLevel,
) -> folia_core::worldgen::structure::start::StructureStart {
    let templates = HashMap::new();
    let context = StructureGenerationContext::new(SEED, chunk, 0, &templates).with_level(level);
    stronghold_definition(structures)
        .generate_start(&context)
        .expect("stronghold start")
}

fn ring_placement(registry: &StructureSetRegistry) -> &ConcentricRingsStructurePlacement {
    let set = registry.get("strongholds").expect("stronghold set");
    let StructurePlacement::ConcentricRings(placement) = &set.placement else {
        panic!("strongholds must use concentric rings");
    };
    placement
}

fn full_box(piece: &StrongholdPiece) -> BoundingBox {
    let bounds = piece.bounding_box();
    BoundingBox::new(
        bounds.min_x(),
        -64,
        bounds.min_z(),
        bounds.max_x(),
        319,
        bounds.max_z(),
    )
}

fn local_pos(piece: &StrongholdPiece, x: i32, y: i32, z: i32) -> BlockPos {
    let origin = piece.origin();
    let bounds = piece.bounding_box();
    BlockPos::new(bounds.min_x() + x, origin.y() + y, bounds.min_z() + z)
}

#[test]
fn stronghold_definition_rings_and_biome_selection_are_fixed_seed() {
    let definition = StructureDefinition::from_json("minecraft:stronghold", STRONGHOLD_JSON)
        .expect("stronghold JSON");
    assert_eq!(definition.kind, StructureKind::Stronghold);
    assert_eq!(definition.step(), Decoration::Strongholds);
    assert_eq!(
        definition.biomes,
        BiomeSet::Tag("#minecraft:has_structure/stronghold".to_owned())
    );

    let registry = StructureSetRegistry::shipped().expect("shipped sets");
    let placement = ring_placement(&registry);
    assert_eq!(
        (placement.count, placement.distance, placement.spread),
        (128, 32, 3)
    );
    assert_eq!(
        placement.preferred_biomes,
        vec!["#minecraft:stronghold_biased_to".to_owned()]
    );
    assert!(registry.unsupported_placements().is_empty());
    let rings = placement.ring_positions(SEED);
    assert_eq!(rings.len(), 128);
    assert_eq!(
        &rings[..8],
        &[
            ChunkPos::new(-35, 0),
            ChunkPos::new(-42, -2),
            ChunkPos::new(-49, -4),
            ChunkPos::new(-55, -8),
            ChunkPos::new(-62, -11),
            ChunkPos::new(-68, -16),
            ChunkPos::new(-74, -21),
            ChunkPos::new(-80, -27),
        ]
    );
    assert!(placement.is_structure_chunk(SEED, 0, 0, None));
    assert!(placement.is_structure_chunk(SEED, rings[0].x, rings[0].z, None));
    assert!(!placement.is_structure_chunk(SEED, 1, 0, None));
    assert_eq!(
        placement.get_potential_structure_chunk(SEED, rings[0].x + 1, rings[0].z),
        rings[0]
    );

    let structures = structures();
    for chunk in [ChunkPos::new(0, 0), rings[0]] {
        let selection = structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes("minecraft:plains"),
            )
            .expect("stronghold selection")
            .expect("stronghold placement chunk");
        assert_eq!(selection.structure, "minecraft:stronghold");
        assert_eq!(selection.set, "minecraft:strongholds");
        assert_eq!(selection.chunk, chunk);
    }
    assert!(
        structures
            .structure_at_chunk(
                1,
                0,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes("minecraft:plains"),
            )
            .expect("non-ring selection")
            .is_none()
    );
    assert!(
        structures
            .structure_at_chunk(
                0,
                0,
                SEED,
                StructureDimension::Overworld,
                63,
                &StaticBiomes("minecraft:nether_wastes"),
            )
            .expect("incompatible biome")
            .is_none()
    );
    assert!(
        structures
            .structure_at_chunk(
                0,
                0,
                SEED,
                StructureDimension::Nether,
                63,
                &StaticBiomes("minecraft:plains"),
            )
            .expect("Nether selection")
            .is_none()
    );
}

#[test]
fn stronghold_fixed_seed_pins_density_height_bounds_and_symmetric_rooms() {
    let structures = structures();
    let chunk = ChunkPos::new(0, 0);
    let level = CaveLevel::new(-64, 384, 80, 10, 70);
    let start = generate_start(&structures, chunk, &level);
    assert!(start.is_valid());
    let piece = start.stronghold().expect("stronghold piece");
    assert_eq!(piece.origin(), BlockPos::new(8, 46, 8));
    assert_eq!(piece.rotation(), Rotation::Clockwise90);
    assert_eq!(
        piece.bounding_box(),
        BoundingBox::new(-20, 46, -14, 36, 57, 30)
    );
    assert_eq!(start.bounding_box(), Some(piece.bounding_box()));
    assert_eq!(piece.rooms().len(), 9);
    for kind in [
        StrongholdRoomKind::Hall,
        StrongholdRoomKind::Library,
        StrongholdRoomKind::Prison,
    ] {
        assert_eq!(
            piece
                .rooms()
                .iter()
                .filter(|room| room.kind() == kind)
                .count(),
            2
        );
    }
    assert_eq!(
        piece
            .rooms()
            .iter()
            .filter(|room| room.kind() == StrongholdRoomKind::PortalRoom)
            .count(),
        1
    );
    for kind in [
        StrongholdRoomKind::Hall,
        StrongholdRoomKind::Library,
        StrongholdRoomKind::Prison,
    ] {
        let pair: Vec<_> = piece
            .rooms()
            .iter()
            .filter(|room| room.kind() == kind)
            .map(|room| room.bounds())
            .collect();
        if pair[0].min_x() != pair[1].min_x() {
            assert_eq!(
                pair[0].min_x() + pair[0].max_x() + pair[1].min_x() + pair[1].max_x(),
                piece.origin().x() * 4
            );
            assert_eq!(pair[0].min_z(), pair[1].min_z());
            assert_eq!(pair[0].max_z(), pair[1].max_z());
        } else {
            assert_eq!(
                pair[0].min_z() + pair[0].max_z() + pair[1].min_z() + pair[1].max_z(),
                piece.origin().z() * 4
            );
            assert_eq!(pair[0].min_x(), pair[1].min_x());
            assert_eq!(pair[0].max_x(), pair[1].max_x());
        }
    }
    assert!(
        piece
            .bounding_box()
            .is_inside(piece.corridor_bounds().center())
    );
    assert!(
        piece
            .bounding_box()
            .is_inside(piece.stairs_bounds().center())
    );
    assert!(piece.bounding_box().is_inside(piece.portal_position()));
    assert!(80 - piece.bounding_box().max_y() >= StrongholdStart::SURFACE_CLEARANCE);
    assert_eq!(generate_start(&structures, chunk, &level), start);

    let templates = HashMap::new();
    let missing = StructureGenerationContext::new(SEED, chunk, 0, &templates);
    assert!(matches!(
        stronghold_definition(&structures).generate_start(&missing),
        Err(StructureError::MissingHeightSource { .. })
    ));
}

#[test]
fn stronghold_y_follows_cave_density_and_rejects_shallow_dimensions() {
    let structures = structures();
    let chunk = ChunkPos::new(0, 0);
    let shallow = CaveLevel::new(-64, 384, 80, 41, 51);
    let deep = CaveLevel::new(-64, 384, 100, 55, 75);
    let shallow_start = generate_start(&structures, chunk, &shallow);
    let deep_start = generate_start(&structures, chunk, &deep);
    let shallow_y = shallow_start
        .stronghold()
        .expect("shallow start")
        .origin()
        .y();
    let deep_y = deep_start.stronghold().expect("deep start").origin().y();
    assert_eq!(shallow_y, 45);
    assert!((55..=69).contains(&deep_y));
    assert_ne!(shallow_y, deep_y);
    for (start, level) in [(&shallow_start, &shallow), (&deep_start, &deep)] {
        let y = start.stronghold().expect("stronghold").origin().y();
        for sample_y in [y + 2, y + 6] {
            assert!((level.cave_min..=level.cave_max).contains(&sample_y));
        }
        assert!(
            level.surface.expect("surface") - start.bounding_box().expect("bounds").max_y() >= 20
        );
    }

    let flat = CaveLevel::new(-64, 384, 80, 10, 70);
    let mut no_density = flat.clone();
    no_density.density = false;
    let fallback = generate_start(&structures, chunk, &no_density);
    assert!(fallback.is_valid());
    assert!(fallback.stronghold().expect("fallback").origin().y() <= 49);

    let shallow_dimension = CaveLevel::new(0, 64, 20, 0, 19);
    assert!(!generate_start(&structures, chunk, &shallow_dimension).is_valid());
}

#[test]
fn stronghold_geometry_contains_shell_core_stairs_library_prison_and_portal() {
    let piece = StrongholdPiece::new(SEED, 8, 32, 8, Rotation::None);
    let mut level = CaveLevel::new(-64, 384, 80, 10, 70);
    let placement = piece
        .place(&mut level, full_box(&piece))
        .expect("full stronghold placement");
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.rejected, 0);
    assert_eq!(placement.written, 19_427);
    assert!(placement.written_blocks.len() < placement.written);

    let names: HashSet<_> = level
        .blocks
        .values()
        .map(|block| block.name.as_str())
        .collect();
    for name in [
        "minecraft:stone_bricks",
        "minecraft:cracked_stone_bricks",
        "minecraft:mossy_stone_bricks",
        "minecraft:chiseled_stone_bricks",
        "minecraft:stone_brick_stairs",
        "minecraft:stone_brick_slab",
        "minecraft:bookshelf",
        "minecraft:iron_bars",
        "minecraft:wall_torch",
        "minecraft:chest",
        "minecraft:end_portal",
        "minecraft:end_portal_frame",
        "minecraft:lava",
        "minecraft:air",
    ] {
        assert!(names.contains(name), "stronghold missing {name}");
    }
    assert_eq!(
        level
            .blocks
            .values()
            .filter(|block| block.name == "minecraft:end_portal")
            .count(),
        9
    );
    assert_eq!(
        level
            .blocks
            .values()
            .filter(|block| block.name == "minecraft:end_portal_frame")
            .count(),
        16
    );
    assert_eq!(
        level
            .blocks
            .values()
            .filter(|block| block.name == "minecraft:lava")
            .count(),
        9
    );
    assert_eq!(
        level
            .get_block(piece.portal_position())
            .expect("portal center")
            .name,
        "minecraft:end_portal"
    );
    assert!(matches!(
        level
            .get_block(local_pos(&piece, 0, 1, 4))
            .expect("stone-brick shell")
            .name
            .as_str(),
        "minecraft:stone_bricks"
            | "minecraft:cracked_stone_bricks"
            | "minecraft:mossy_stone_bricks"
            | "minecraft:chiseled_stone_bricks"
    ));
    assert_eq!(
        level
            .get_block(local_pos(&piece, 22, 3, 28))
            .expect("grand hall core")
            .name,
        "minecraft:air"
    );
    assert_eq!(
        level
            .get_block(local_pos(&piece, 22, 1, 5))
            .expect("central corridor")
            .name,
        "minecraft:air"
    );
    assert_eq!(
        level
            .get_block(local_pos(&piece, 22, 0, 23))
            .expect("central staircase")
            .name,
        "minecraft:stone_brick_stairs"
    );
    assert_eq!(
        level
            .get_block(local_pos(&piece, 22, 1, 0))
            .expect("arched entrance")
            .name,
        "minecraft:air"
    );
    assert_eq!(
        level
            .get_block(local_pos(&piece, 22, 5, 0))
            .expect("arch keystone")
            .name,
        "minecraft:chiseled_stone_bricks"
    );

    let chests: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .collect();
    assert_eq!(chests.len(), 4);
    for table in [
        "minecraft:chests/stronghold_library",
        "minecraft:chests/stronghold_crossing",
    ] {
        assert_eq!(
            chests
                .iter()
                .filter(|chest| chest
                    .nbt
                    .as_ref()
                    .and_then(|nbt| nbt.get("LootTable"))
                    .and_then(Tag::as_str)
                    == Some(table))
                .count(),
            2
        );
    }
    for chest in chests {
        assert!(chest.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
                && nbt.get("LootTableSeed").and_then(Tag::as_long).is_some()
        }));
    }
}

#[test]
fn stronghold_rotation_and_chunk_clipping_preserve_the_full_layout() {
    let piece = StrongholdPiece::new(SEED, 8, 32, 8, Rotation::Clockwise90);
    assert_eq!(piece.bounding_box().x_span(), StrongholdStart::DEPTH);
    assert_eq!(piece.bounding_box().z_span(), StrongholdStart::WIDTH);
    assert_eq!(piece.portal_position(), BlockPos::new(-14, 33, 8));
    let stairs = BlockPos::new(13, 32, 8);

    let mut full_level = CaveLevel::new(-64, 384, 80, 10, 70);
    let full = piece
        .place(&mut full_level, full_box(&piece))
        .expect("full rotated stronghold");
    assert_eq!(
        full_level
            .get_block(stairs)
            .expect("rotated staircase")
            .properties,
        vec![
            ("facing".to_owned(), "east".to_owned()),
            ("half".to_owned(), "bottom".to_owned()),
            ("shape".to_owned(), "straight".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ]
    );

    let bounds = piece.bounding_box();
    let mut split_level = CaveLevel::new(-64, 384, 80, 10, 70);
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
                .expect("clipped stronghold");
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
    let mut full_chests: Vec<_> = full
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .cloned()
        .collect();
    full_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
    split_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
    assert_eq!(split_chests, full_chests);

    let mut repeat_level = CaveLevel::new(-64, 384, 80, 10, 70);
    let repeat = piece
        .place(&mut repeat_level, full_box(&piece))
        .expect("repeated stronghold");
    assert_eq!(repeat, full);
    assert_eq!(repeat_level.blocks, full_level.blocks);
}

#[test]
fn stronghold_set_pipeline_uses_the_stronghold_decoration_step() {
    let structures = structures();
    let level = CaveLevel::new(-64, 384, 80, 10, 70);
    let mut write_level = CaveLevel::new(-64, 384, 80, 10, 70);
    let plains = StaticBiomes("minecraft:plains");
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut write_level,
            &level,
            ChunkPos::new(0, 0),
            SEED,
            StructureDimension::Overworld,
            63,
            &plains,
            0,
            Decoration::Strongholds,
        )
        .expect("stronghold structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert!(placement.written > 0);
    assert!(placement.skipped_unsupported.is_empty());
    assert!(
        placement
            .written_blocks
            .iter()
            .all(|pos| { (0..16).contains(&pos.x()) && (0..16).contains(&pos.z()) })
    );

    let underground = structures
        .place_for_chunk_with_height_source(
            &mut write_level,
            &level,
            ChunkPos::new(0, 0),
            SEED,
            StructureDimension::Overworld,
            63,
            &plains,
            0,
            Decoration::UndergroundStructures,
        )
        .expect("underground structure pass");
    assert_eq!(underground.generated_starts, 0);
    assert_eq!(underground.written, 0);
}

#[test]
fn generated_chunk_contains_stronghold_stairs_and_stone_brick_shell() {
    let structures = structures();
    let generator = WorldGenerator::overworld(SEED)
        .expect("generator")
        .with_structure_generator(structures.clone());
    let biomes = GeneratedBiomes {
        generator: &generator,
    };
    let registry = StructureSetRegistry::shipped().expect("sets");
    let rings = ring_placement(&registry).ring_positions(SEED);
    let mut found = None;
    for chunk in std::iter::once(ChunkPos::new(0, 0)).chain(rings.into_iter().take(32)) {
        let Some(selection) = structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                SEED,
                StructureDimension::Overworld,
                63,
                &biomes,
            )
            .expect("selection")
        else {
            continue;
        };
        if selection.structure != "minecraft:stronghold" {
            continue;
        }
        let structured = generator
            .generate_chunk(chunk.x, chunk.z)
            .expect("structured chunk");
        let mut stairs = Vec::new();
        for x in 0..16 {
            for z in 0..16 {
                for y in generator.min_y()..generator.min_y() + generator.height() {
                    if structured.block_state(x, y, z).expect("block").name
                        == "minecraft:stone_brick_stairs"
                    {
                        stairs.push((x, y, z));
                    }
                }
            }
        }
        if !stairs.is_empty() {
            found = Some((chunk, structured, stairs));
            break;
        }
    }
    let (chunk, structured, stairs) = found.expect("generated stronghold staircase near origin");
    let plain = WorldGenerator::overworld(SEED)
        .expect("plain generator")
        .generate_chunk(chunk.x, chunk.z)
        .expect("plain chunk");
    let mut stone_bricks = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in generator.min_y()..generator.min_y() + generator.height() {
                if structured
                    .block_state(x, y, z)
                    .expect("structured block")
                    .name
                    == "minecraft:stone_bricks"
                {
                    stone_bricks += 1;
                }
            }
        }
    }
    assert!(stone_bricks > 20, "stronghold shell at {chunk:?}");
    assert!((0..16).all(|x| {
        (0..16).all(|z| {
            (generator.min_y()..generator.min_y() + generator.height()).all(|y| {
                plain.block_state(x, y, z).expect("plain block").name
                    != "minecraft:stone_brick_stairs"
            })
        })
    }));
    let surface = generator.base_height((chunk.x << 4) + 8, (chunk.z << 4) + 8);
    let highest_stair = stairs.iter().map(|(_, y, _)| *y).max().expect("stair");
    assert!(surface - (highest_stair + 7) >= StrongholdStart::SURFACE_CLEARANCE);
}
