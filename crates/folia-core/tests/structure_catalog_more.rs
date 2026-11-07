//! Non-village structure-template catalog tests.
//!
//! `structure_templates_more.json` ships the remaining 1.21.11 structure
//! families as one base64+gzip NBT stream per family plus an offset/length
//! index. These tests pin the asset bytes/hash, exercise slices from both
//! catalogs through `shipped()`, and verify unknown/corrupt payload failures.

use folia_core::worldgen::structure::assets::{
    CatalogError, EMBEDDED_STRUCTURE_TEMPLATES_MORE, StructureTemplateCatalog,
};
use folia_core::worldgen::structure::start::TemplateProvider;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const VILLAGE_MEETING_POINT: &str = "minecraft:village/plains/town_centers/plains_meeting_point_1";
const END_CITY_SHIP: &str = "minecraft:end_city/ship";
const FOSSIL_SKULL: &str = "minecraft:fossil/skull_1";
const MORE_TEMPLATES: usize = 528;
const MORE_COMPRESSED_BYTES: usize = 1_729_270;
const MORE_NBT_BYTES: usize = 23_972_845;
const VILLAGE_TEMPLATES: usize = 483;
const VILLAGE_COMPRESSED_BYTES: usize = 776_539;

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

#[test]
fn more_catalog_covers_the_remaining_families_and_is_compact() {
    let more = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE)
        .expect("embedded more catalog parses");
    assert_eq!(more.version(), "1.21.11");
    assert_eq!(more.data_version(), 4788);
    assert_eq!(more.len(), MORE_TEMPLATES);
    assert_eq!(more.total_bytes(), MORE_COMPRESSED_BYTES);
    assert_eq!(more.nbt_bytes(), MORE_NBT_BYTES);
    assert_eq!(more.all_len(), MORE_TEMPLATES);

    for (family, expected) in [
        ("(root)", 1),
        ("ancient_city", 58),
        ("bastion", 167),
        ("end_city", 20),
        ("fossil", 16),
        ("igloo", 3),
        ("nether_fossils", 14),
        ("pillager_outpost", 11),
        ("ruined_portal", 13),
        ("shipwreck", 20),
        ("trail_ruins", 84),
        ("underwater_ruin", 48),
        ("woodland_mansion", 73),
    ] {
        let count = if family == "(root)" {
            usize::from(more.contains("minecraft:empty"))
        } else {
            more.ids()
                .filter(|id| id.starts_with(&format!("minecraft:{family}/")))
                .count()
        };
        assert_eq!(
            count, expected,
            "non-village family `{family}` count changed"
        );
    }

    assert!(more.contains("minecraft:empty"));
    assert!(more.contains(END_CITY_SHIP));
    assert!(more.contains("end_city/ship"));
    assert!(more.contains(FOSSIL_SKULL));
    assert!(!more.contains("minecraft:village/plains/town_centers/plains_meeting_point_1"));
    assert!(
        !more
            .ids()
            .any(|id| id.starts_with("minecraft:trial_chambers/"))
    );

    let bytes = asset("structure_templates_more.json");
    assert!(
        bytes.len() < 2_500_000,
        "the non-village catalog must stay under the 2.5 MB budget"
    );
    assert_eq!(bytes.len(), 2_358_479, "catalog byte length changed");
    assert_eq!(
        fnv1a64(&bytes),
        0xb45a_cf21_43b0_63bc,
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
        StructureTemplateCatalog::from_indexed_json(std::str::from_utf8(&bytes).expect("UTF-8"))
            .expect("catalog reparses");
    assert_eq!(parsed.len(), more.len());
    assert_eq!(parsed.total_bytes(), more.total_bytes());
    assert_eq!(parsed.nbt_bytes(), more.nbt_bytes());
}

#[test]
fn end_city_and_fossil_templates_have_the_shipped_bounds_and_palettes() {
    let more = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE)
        .expect("embedded more catalog parses");

    let ship = more.template(END_CITY_SHIP).expect("end city ship");
    assert_eq!(ship.data_version(), 4671);
    assert_eq!(ship.size(), [13, 24, 29]);
    assert_eq!(ship.palette_count(), 1);
    assert_eq!(ship.block_count(), 9048);
    assert_eq!(ship.entity_count(), 0);

    let palette = ship.palette(0).expect("ship palette");
    assert_eq!(palette.len(), 37);
    assert_eq!(
        palette[..7]
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>(),
        [
            "minecraft:purpur_pillar",
            "minecraft:purpur_block",
            "minecraft:obsidian",
            "minecraft:purpur_pillar",
            "minecraft:end_stone_bricks",
            "minecraft:magenta_stained_glass",
            "minecraft:air",
        ]
    );
    assert_eq!(
        palette[0].properties,
        vec![("axis".to_owned(), "z".to_owned())]
    );
    assert_eq!(
        palette[3].properties,
        vec![("axis".to_owned(), "y".to_owned())]
    );
    assert_eq!(palette[37 - 1].name, "minecraft:dragon_wall_head");

    let blocks = ship.blocks();
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
    assert_eq!(max, (12, 23, 28));

    let fossil = more.template(FOSSIL_SKULL).expect("fossil skull");
    assert_eq!(fossil.data_version(), 4671);
    assert_eq!(fossil.size(), [6, 5, 7]);
    assert_eq!(fossil.palette_count(), 1);
    assert_eq!(fossil.block_count(), 86);
    let palette = fossil.palette(0).expect("fossil palette");
    assert_eq!(palette.len(), 3);
    assert_eq!(
        palette
            .iter()
            .map(|state| {
                (
                    state.name.as_str(),
                    state
                        .properties
                        .iter()
                        .find(|(key, _)| key == "axis")
                        .map(|(_, value)| value.as_str()),
                )
            })
            .collect::<Vec<_>>(),
        [
            ("minecraft:bone_block", Some("x")),
            ("minecraft:bone_block", Some("y")),
            ("minecraft:bone_block", Some("z")),
        ]
    );
    let blocks = fossil.blocks();
    assert_eq!(
        (
            blocks.iter().map(|block| block.pos.x()).min().unwrap(),
            blocks.iter().map(|block| block.pos.y()).min().unwrap(),
            blocks.iter().map(|block| block.pos.z()).min().unwrap(),
        ),
        (0, 0, 0)
    );
    assert_eq!(
        (
            blocks.iter().map(|block| block.pos.x()).max().unwrap(),
            blocks.iter().map(|block| block.pos.y()).max().unwrap(),
            blocks.iter().map(|block| block.pos.z()).max().unwrap(),
        ),
        (5, 4, 6)
    );
}

#[test]
fn shipped_catalog_resolves_village_and_non_village_ids() {
    let catalog = StructureTemplateCatalog::shipped();

    // Backwards-compatible village pins and the merged totals.
    assert_eq!(catalog.len(), VILLAGE_TEMPLATES);
    assert_eq!(catalog.total_bytes(), VILLAGE_COMPRESSED_BYTES);
    assert_eq!(catalog.all_len(), VILLAGE_TEMPLATES + MORE_TEMPLATES);
    assert_eq!(
        catalog.all_total_bytes(),
        VILLAGE_COMPRESSED_BYTES + MORE_COMPRESSED_BYTES
    );
    assert_eq!(catalog.ids().count(), VILLAGE_TEMPLATES);
    assert_eq!(
        catalog.all_ids().count(),
        VILLAGE_TEMPLATES + MORE_TEMPLATES
    );

    assert!(catalog.contains(VILLAGE_MEETING_POINT));
    assert!(catalog.contains(END_CITY_SHIP));
    assert!(catalog.contains(FOSSIL_SKULL));

    let village = catalog
        .template(VILLAGE_MEETING_POINT)
        .expect("village template through the merged catalog");
    assert_eq!(village.size(), [10, 7, 10]);
    assert!(TemplateProvider::template(catalog, VILLAGE_MEETING_POINT).is_some());
    assert!(catalog.is_loaded(VILLAGE_MEETING_POINT));

    let ship = catalog
        .template(END_CITY_SHIP)
        .expect("non-village template through the merged catalog");
    assert_eq!(ship.size(), [13, 24, 29]);
    assert!(TemplateProvider::template(catalog, END_CITY_SHIP).is_some());
    assert!(catalog.is_loaded(END_CITY_SHIP));

    let fossil = TemplateProvider::template(catalog, FOSSIL_SKULL).expect("fossil provider lookup");
    assert_eq!(fossil.size(), [6, 5, 7]);

    let more = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE)
        .expect("more catalog parses");
    assert_eq!(
        more.template(VILLAGE_MEETING_POINT).unwrap_err(),
        CatalogError::UnknownTemplate {
            template: VILLAGE_MEETING_POINT.to_owned()
        }
    );
}

#[test]
fn unknown_and_corrupt_more_payloads_are_explicit_errors() {
    let more = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE)
        .expect("embedded more catalog parses");
    let error = more
        .template("minecraft:end_city/missing")
        .expect_err("unknown template");
    assert!(matches!(
        error,
        CatalogError::UnknownTemplate { ref template }
            if template == "minecraft:end_city/missing"
    ));

    // Valid schema and base64, but the decoded bytes are not gzip NBT.
    let corrupt = StructureTemplateCatalog::from_indexed_json(
        r#"{"blobs":{"test":{"bytes":8,"payload":"bm90IGd6aXA="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:test":{"blob":"test","length":8,"offset":0}},"total_bytes":8,"version":"1.21.11"}"#,
    )
    .expect("schema is valid");
    let error = corrupt
        .template("minecraft:test")
        .expect_err("corrupt shared payload");
    assert!(matches!(
        error,
        CatalogError::CorruptPayload { ref template, .. } if template == "minecraft:test"
    ));

    for malformed in [
        // Invalid base64 alphabet.
        r#"{"blobs":{"test":{"bytes":1,"payload":"!!!"}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // Slice extends past the blob.
        r#"{"blobs":{"test":{"bytes":8,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:test":{"blob":"test","length":4,"offset":5}},"total_bytes":1,"version":"1.21.11"}"#,
        // Slices are not contiguous.
        r#"{"blobs":{"test":{"bytes":8,"payload":"eA=="}},"counts":{"by_root":{"test":2},"templates":2},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:a":{"blob":"test","length":4,"offset":0},"minecraft:b":{"blob":"test","length":3,"offset":5}},"total_bytes":1,"version":"1.21.11"}"#,
        // Unknown blob reference.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"missing","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // total_bytes disagrees with the base64 length.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0}},"total_bytes":2,"version":"1.21.11"}"#,
        // nbt_bytes disagrees with the blob length.
        r#"{"blobs":{"test":{"bytes":2,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":2,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // Unknown template-entry field.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0,"unknown":1}},"total_bytes":1,"version":"1.21.11"}"#,
        // Zero-length slice.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":0,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // counts.templates disagrees with the map.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":2},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // Unknown top-level field.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0}},"total_bytes":1,"unknown":true,"version":"1.21.11"}"#,
        // Wrong target version.
        r#"{"blobs":{"test":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"test":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:test":{"blob":"test","length":1,"offset":0}},"total_bytes":1,"version":"1.20.1"}"#,
    ] {
        assert!(
            StructureTemplateCatalog::from_indexed_json(malformed).is_err(),
            "malformed catalog must fail loudly: {malformed}"
        );
    }

    // Merging catalogs with a duplicate id is explicit instead of silently
    // replacing one payload with the other.
    let mut left =
        StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE).unwrap();
    let right =
        StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_MORE).unwrap();
    assert!(left.merge(right).is_err());
}

#[test]
fn extractor_regenerates_byte_identical_catalogs() {
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("structure_templates.json")
            && stdout.contains("structure_templates_more.json"),
        "extractor -Check must validate both catalogs:\n{stdout}"
    );
}
