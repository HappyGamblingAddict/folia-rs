//! Embedded structure-template catalog tests.
//!
//! The catalog is generated from the local 1.21.11 decompiled data tree by
//! `scripts/extract_structure_templates.ps1`. These tests pin the asset bytes,
//! exercise lazy base64/gzip/NBT loading, and run a real recursive village
//! through the jigsaw generator with the shipped pools/processors.

use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::structure::StructureGenerator;
use folia_core::worldgen::structure::assets::{CatalogError, StructureTemplateCatalog};
use folia_core::worldgen::structure::set::{
    StructureBiomeLookup, StructureDimension, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{StructureGenerationContext, TemplateProvider};
use folia_core::worldgen::structure::template::{Rotation, StructurePlaceSettings};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const MEETING_POINT: &str = "minecraft:village/plains/town_centers/plains_meeting_point_1";
const VILLAGE_SEED: i64 = 12_345;
const VILLAGE_CHUNK: (i32, i32) = (-22, -32);

#[derive(Default)]
struct MockLevel {
    min_y: i32,
    height: i32,
    fixed_height: Option<i32>,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            fixed_height: Some(64),
            blocks: HashMap::new(),
        }
    }
}

impl FeatureLevel for MockLevel {
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
        self.fixed_height
    }
}

struct PlainsBiomes;

impl StructureBiomeLookup for PlainsBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        "minecraft:plains"
    }
}

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// FNV-1a 64, the dependency-free checksum used by `tests/assets.rs`.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn plains_generator() -> StructureGenerator {
    StructureGenerator::shipped_catalog().expect("catalog-backed structure generator")
}

#[test]
fn catalog_covers_the_village_template_set_and_is_compact() {
    let catalog = StructureTemplateCatalog::shipped();
    assert_eq!(catalog.version(), "1.21.11");
    assert_eq!(catalog.data_version(), 4788);
    assert_eq!(catalog.len(), 483);
    assert_eq!(catalog.total_bytes(), 776_539);

    for (family, expected) in [
        ("plains", 104),
        ("desert", 76),
        ("savanna", 98),
        ("snowy", 89),
        ("taiga", 92),
        ("common", 21),
        ("decays", 3),
    ] {
        let prefix = format!("minecraft:village/{family}/");
        assert_eq!(
            catalog.ids().filter(|id| id.starts_with(&prefix)).count(),
            expected,
            "village/{family} template count changed"
        );
    }

    assert!(catalog.contains(MEETING_POINT));
    assert!(catalog.contains("village/plains/town_centers/plains_meeting_point_1"));
    assert!(!catalog.contains("minecraft:village/plains/town_centers/missing"));
    assert!(!catalog.is_loaded(MEETING_POINT));

    let bytes = asset("structure_templates.json");
    assert!(
        bytes.len() < 2_500_000,
        "structure-template asset must stay under the 2.5 MB budget"
    );
    assert_eq!(bytes.len(), 1_063_517, "catalog byte length changed");
    assert_eq!(
        fnv1a64(&bytes),
        0x0dde_46f3_a786_3ed8,
        "catalog bytes changed; regenerate with scripts/extract_structure_templates.ps1"
    );
    assert!(
        bytes.ends_with(b"}\n"),
        "catalog must end with a single newline"
    );
    assert!(
        !bytes.contains(&b'\r'),
        "catalog must use LF line endings only"
    );

    let parsed =
        StructureTemplateCatalog::from_json(std::str::from_utf8(&bytes).expect("catalog is UTF-8"))
            .expect("catalog reparses");
    assert_eq!(parsed.len(), catalog.len());
    assert_eq!(parsed.total_bytes(), catalog.total_bytes());
}

#[test]
fn meeting_point_template_has_the_shipped_palette_blocks_and_bounds() {
    let catalog = StructureTemplateCatalog::shipped();
    let template = catalog.template(MEETING_POINT).expect("meeting point");
    assert_eq!(template.data_version(), 4671);
    assert_eq!(template.size(), [10, 7, 10]);
    assert_eq!(template.rotated_size(Rotation::Clockwise90), [10, 7, 10]);
    assert_eq!(template.palette_count(), 1);
    assert_eq!(template.block_count(), 153);
    assert_eq!(template.entity_count(), 0);

    let palette = template.palette(0).expect("palette");
    assert_eq!(palette.len(), 12);
    assert_eq!(
        palette
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>(),
        [
            "minecraft:cobblestone",
            "minecraft:dirt_path",
            "minecraft:water",
            "minecraft:oak_fence",
            "minecraft:torch",
            "minecraft:jigsaw",
            "minecraft:jigsaw",
            "minecraft:jigsaw",
            "minecraft:jigsaw",
            "minecraft:jigsaw",
            "minecraft:jigsaw",
            "minecraft:bell",
        ]
    );
    assert_eq!(
        palette[2].properties,
        vec![("level".to_owned(), "0".to_owned())]
    );
    assert_eq!(
        palette[3].properties,
        vec![
            ("east".to_owned(), "false".to_owned()),
            ("north".to_owned(), "false".to_owned()),
            ("south".to_owned(), "false".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
            ("west".to_owned(), "false".to_owned()),
        ]
    );
    let orientations: Vec<&str> = palette[5..11]
        .iter()
        .map(|state| {
            state
                .properties
                .iter()
                .find(|(key, _)| key == "orientation")
                .map(|(_, value)| value.as_str())
                .expect("jigsaw orientation")
        })
        .collect();
    assert_eq!(
        orientations,
        [
            "up_north",
            "down_south",
            "west_up",
            "south_up",
            "north_up",
            "east_up",
        ]
    );
    assert_eq!(
        palette[11].properties,
        vec![
            ("attachment".to_owned(), "floor".to_owned()),
            ("facing".to_owned(), "north".to_owned()),
            ("powered".to_owned(), "false".to_owned()),
        ]
    );

    let blocks = template.blocks();
    assert_eq!(
        blocks.iter().filter(|block| block.nbt.is_some()).count(),
        12,
        "eleven jigsaw blocks plus the bell's block entity carry NBT"
    );
    let min = (
        blocks.iter().map(|block| block.pos.x()).min().unwrap(),
        blocks.iter().map(|block| block.pos.y()).min().unwrap(),
        blocks.iter().map(|block| block.pos.z()).min().unwrap(),
    );
    let max = (
        blocks.iter().map(|block| block.pos.x()).max().unwrap(),
        blocks.iter().map(|block| block.pos.y()).max().unwrap(),
        blocks.iter().map(|block| block.pos.z()).max().unwrap(),
    );
    assert_eq!(min, (0, 0, 0));
    assert_eq!(max, (9, 6, 9));

    let placed = template.placement_blocks(&StructurePlaceSettings::new());
    assert_eq!(placed.len(), 153);
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:cobblestone")
            .count(),
        68
    );
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:jigsaw")
            .count(),
        11
    );

    // The provider path caches the parsed template after the first request.
    assert!(TemplateProvider::template(catalog, MEETING_POINT).is_some());
    assert!(catalog.is_loaded(MEETING_POINT));
}

#[test]
fn real_catalog_drives_recursive_village_generation_deterministically() {
    let catalog = StructureTemplateCatalog::shipped();
    let generator = plains_generator();
    let level = MockLevel::new(0, 384);

    let definition = generator
        .definitions()
        .get("minecraft:village_plains")
        .expect("village_plains definition");
    let context = StructureGenerationContext::new(VILLAGE_SEED, ChunkPos::new(0, 0), 0, catalog)
        .with_level(&level);
    let start = definition
        .generate_start(&context)
        .expect("real village start");
    assert!(start.is_valid());
    assert!(
        start.jigsaw_pieces().len() > 1,
        "recursive placement produced only the start piece"
    );
    assert_eq!(start.jigsaw_pieces().len(), 48);
    let bounds = start.pieces_bounding_box().expect("village bounds");
    assert_eq!(bounds.min_y(), 63);
    assert_eq!(bounds.max_y(), 75);
    let skipped = start
        .jigsaw()
        .expect("jigsaw payload")
        .skipped_feature_elements();
    assert!(
        !skipped.is_empty(),
        "feature pool elements must be recorded as skipped, not silently ignored"
    );
    assert!(
        skipped
            .iter()
            .all(|feature| feature.starts_with("minecraft:"))
    );

    let again = definition
        .generate_start(&context)
        .expect("second village start");
    assert_eq!(start, again, "same seed must produce the same start");
    assert_eq!(start.pieces_bounding_box(), again.pieces_bounding_box());

    let other_context =
        StructureGenerationContext::new(54_321, ChunkPos::new(0, 0), 0, catalog).with_level(&level);
    let other = definition
        .generate_start(&other_context)
        .expect("other village start");
    assert_ne!(
        start.pieces_bounding_box(),
        other.pieces_bounding_box(),
        "a different seed must produce a different bounds"
    );

    // Wire the same catalog through `StructureGenerator`: find the selected
    // village chunk, then place its pieces into a chunk-sized write box twice.
    let biomes = PlainsBiomes;
    let sets = StructureSetRegistry::shipped().expect("structure sets");
    let selected = sets
        .structure_at_chunk_filtered_matching(
            VILLAGE_CHUNK.0,
            VILLAGE_CHUNK.1,
            VILLAGE_SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
            |structure| structure == "minecraft:village_plains",
        )
        .expect("village selection")
        .expect("village set chunk");
    assert_eq!(selected.structure, "minecraft:village_plains");

    let mut first_level = MockLevel::new(0, 384);
    let first = generator
        .place_for_chunk(
            &mut first_level,
            ChunkPos::new(VILLAGE_CHUNK.0, VILLAGE_CHUNK.1),
            VILLAGE_SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("village placement");
    assert_eq!(first.generated_starts, 1);
    assert!(first.placed_pieces >= 1);
    assert!(first.written > 0);
    assert_eq!(first.rejected, 0);
    assert!(
        first_level
            .blocks
            .values()
            .any(|state| state.name == "minecraft:cobblestone"),
        "the real meeting-point/house palette must reach the world"
    );
    assert!(
        first_level
            .blocks
            .values()
            .all(|state| state.name != "minecraft:jigsaw"),
        "normal generation must replace jigsaw blocks"
    );
    assert!(
        generator
            .template_catalog()
            .expect("catalog-backed generator")
            .ids()
            .any(|id| generator.template_catalog().unwrap().is_loaded(id)),
        "the generator must load templates lazily through the catalog"
    );

    let mut second_level = MockLevel::new(0, 384);
    let second = generator
        .place_for_chunk(
            &mut second_level,
            ChunkPos::new(VILLAGE_CHUNK.0, VILLAGE_CHUNK.1),
            VILLAGE_SEED,
            StructureDimension::Overworld,
            63,
            &biomes,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("second village placement");
    assert_eq!(first, second);
    assert_eq!(first_level.blocks, second_level.blocks);
}

#[test]
fn unknown_ids_and_corrupt_payloads_are_explicit_errors() {
    let catalog = StructureTemplateCatalog::shipped();
    let error = catalog
        .template("minecraft:village/plains/town_centers/missing")
        .expect_err("unknown template");
    assert!(matches!(
        error,
        CatalogError::UnknownTemplate { ref template }
            if template == "minecraft:village/plains/town_centers/missing"
    ));

    // Valid base64, but the decoded bytes are not gzip NBT.
    let corrupt = StructureTemplateCatalog::from_json(
        r#"{"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"templates":{"minecraft:test":"bm90IGd6aXA="},"total_bytes":8,"version":"1.21.11"}"#,
    )
    .expect("catalog schema is valid");
    let error = corrupt
        .template("minecraft:test")
        .expect_err("corrupt payload");
    assert!(matches!(
        error,
        CatalogError::CorruptPayload { ref template, .. } if template == "minecraft:test"
    ));

    for malformed in [
        // Invalid base64 alphabet.
        r#"{"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"templates":{"minecraft:test":"!!!"},"total_bytes":2,"version":"1.21.11"}"#,
        // total_bytes disagrees with the decoded base64 length (one byte).
        r#"{"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"templates":{"minecraft:test":"eA=="},"total_bytes":2,"version":"1.21.11"}"#,
        // counts.templates disagrees with the map.
        r#"{"counts":{"by_root":{"test":1},"templates":2},"data_version":4788,"templates":{"minecraft:test":"eA=="},"total_bytes":1,"version":"1.21.11"}"#,
        // by_root disagrees with counts.templates.
        r#"{"counts":{"by_root":{"test":2},"templates":1},"data_version":4788,"templates":{"minecraft:test":"eA=="},"total_bytes":1,"version":"1.21.11"}"#,
        // Unknown top-level field.
        r#"{"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"templates":{"minecraft:test":"eA=="},"total_bytes":1,"unknown":true,"version":"1.21.11"}"#,
        // Wrong target version.
        r#"{"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"templates":{"minecraft:test":"eA=="},"total_bytes":1,"version":"1.20.1"}"#,
    ] {
        assert!(
            StructureTemplateCatalog::from_json(malformed).is_err(),
            "malformed catalog must fail loudly: {malformed}"
        );
    }
}

#[test]
fn extractor_regenerates_byte_identical_output() {
    if !cfg!(windows) {
        eprintln!("skipping extractor check: the local decompiled tree is Windows-only");
        return;
    }
    if std::env::var_os("FOLIA_SKIP_STRUCTURE_ORACLE").is_some() {
        eprintln!("skipping extractor check: FOLIA_SKIP_STRUCTURE_ORACLE is set");
        return;
    }
    let source =
        PathBuf::from("C:/Users/austr/Downloads/rustfol/decompiled/data/minecraft/structure");
    if !source.is_dir() {
        eprintln!("skipping extractor check: local decompiled structure tree is absent");
        return;
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/extract_structure_templates.ps1");
    assert!(script.is_file(), "extractor script is missing");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Check")
        .output()
        .expect("run extract_structure_templates.ps1 -Check");
    assert!(
        output.status.success(),
        "extractor check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
