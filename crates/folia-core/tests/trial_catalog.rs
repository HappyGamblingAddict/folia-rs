//! Trial-chamber structure-template catalog tests.
//!
//! `structure_templates_trial.json` ships the 191 1.21.11
//! `minecraft:trial_chambers/**` templates as one base64+gzip NBT stream per
//! first-level subdirectory (`chamber`, `chests`, `corridor`, ...) plus an
//! offset/length index. These tests pin the asset bytes/hash, exercise
//! templates from different split parts through `shipped()`, check a real
//! chamber's bounds/palette/block entities, and verify unknown/corrupt
//! payload failures.

use folia_core::nbt::Tag;
use folia_core::worldgen::structure::assets::{
    CatalogError, EMBEDDED_STRUCTURE_TEMPLATES_TRIAL, StructureTemplateCatalog,
};
use folia_core::worldgen::structure::start::TemplateProvider;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const CHAMBER_1: &str = "minecraft:trial_chambers/chamber/chamber_1";
const CORRIDOR_STRAIGHT_1: &str = "minecraft:trial_chambers/corridor/straight_1";
const REWARD_VAULT: &str = "minecraft:trial_chambers/reward/vault";
const SPAWNER_BREEZE: &str = "minecraft:trial_chambers/spawner/breeze/breeze";
const VILLAGE_MEETING_POINT: &str = "minecraft:village/plains/town_centers/plains_meeting_point_1";
const END_CITY_SHIP: &str = "minecraft:end_city/ship";
const TRIAL_TEMPLATES: usize = 191;
const TRIAL_COMPRESSED_BYTES: usize = 732_355;
const TRIAL_NBT_BYTES: usize = 9_844_155;
const TRIAL_ASSET_BYTES: usize = 996_296;
const TRIAL_ASSET_FNV1A64: u64 = 0xe88c_a768_c8c4_1c9a;
const VILLAGE_TEMPLATES: usize = 483;
const VILLAGE_COMPRESSED_BYTES: usize = 776_539;
const MORE_TEMPLATES: usize = 528;
const MORE_COMPRESSED_BYTES: usize = 1_729_270;
const MORE_NBT_BYTES: usize = 23_972_845;
const TRIAL_PARTS: [(&str, usize); 9] = [
    ("chamber", 73),
    ("chests", 2),
    ("corridor", 38),
    ("decor", 28),
    ("dispensers", 3),
    ("hallway", 23),
    ("intersection", 3),
    ("reward", 2),
    ("spawner", 19),
];

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

fn property<'a>(state: &'a folia_core::chunk::BlockState, name: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn trial_catalog_covers_all_parts_and_is_compact() {
    let trial = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_TRIAL)
        .expect("embedded trial catalog parses");
    assert_eq!(trial.version(), "1.21.11");
    assert_eq!(trial.data_version(), 4788);
    assert_eq!(trial.len(), TRIAL_TEMPLATES);
    assert_eq!(trial.total_bytes(), TRIAL_COMPRESSED_BYTES);
    assert_eq!(trial.nbt_bytes(), TRIAL_NBT_BYTES);
    assert_eq!(trial.all_len(), TRIAL_TEMPLATES);
    assert_eq!(trial.all_total_bytes(), TRIAL_COMPRESSED_BYTES);
    assert_eq!(trial.merged_len(), TRIAL_TEMPLATES);
    assert_eq!(trial.merged_total_bytes(), TRIAL_COMPRESSED_BYTES);
    assert_eq!(trial.merged_nbt_bytes(), TRIAL_NBT_BYTES);

    for (part, expected) in TRIAL_PARTS {
        let prefix = format!("minecraft:trial_chambers/{part}/");
        assert_eq!(
            trial.ids().filter(|id| id.starts_with(&prefix)).count(),
            expected,
            "trial-chamber part `{part}` count changed"
        );
    }
    assert_eq!(
        trial
            .ids()
            .filter(|id| id.starts_with("minecraft:trial_chambers/"))
            .count(),
        TRIAL_TEMPLATES
    );

    assert!(trial.contains(CHAMBER_1));
    assert!(trial.contains("trial_chambers/chamber/chamber_1"));
    assert!(trial.contains(CORRIDOR_STRAIGHT_1));
    assert!(trial.contains(REWARD_VAULT));
    assert!(trial.contains(SPAWNER_BREEZE));
    assert!(!trial.contains(VILLAGE_MEETING_POINT));
    assert!(!trial.contains(END_CITY_SHIP));

    // The catalog is split into one gzip stream per first-level directory, and
    // every template record names its part instead of an inline payload.
    assert_eq!(
        EMBEDDED_STRUCTURE_TEMPLATES_TRIAL
            .matches("\"payload\":")
            .count(),
        TRIAL_PARTS.len(),
        "each trial part must carry exactly one gzip payload"
    );
    for (part, _) in TRIAL_PARTS {
        assert!(
            EMBEDDED_STRUCTURE_TEMPLATES_TRIAL.contains(&format!("\"blob\":\"{part}\"")),
            "no template references the `{part}` split part"
        );
    }

    let bytes = asset("structure_templates_trial.json");
    assert!(
        bytes.len() < 2_500_000,
        "the trial-chamber catalog must stay under the 2.5 MB budget"
    );
    assert_eq!(
        bytes.len(),
        TRIAL_ASSET_BYTES,
        "catalog byte length changed"
    );
    assert_eq!(
        fnv1a64(&bytes),
        TRIAL_ASSET_FNV1A64,
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
    assert_eq!(parsed.len(), trial.len());
    assert_eq!(parsed.total_bytes(), trial.total_bytes());
    assert_eq!(parsed.nbt_bytes(), trial.nbt_bytes());
}

#[test]
fn shipped_catalog_merges_trial_parts_without_moving_the_primary_pins() {
    let catalog = StructureTemplateCatalog::shipped();

    // Village-only and village+non-village pins stay exactly where they were
    // before trial chambers shipped.
    assert_eq!(catalog.len(), VILLAGE_TEMPLATES);
    assert_eq!(catalog.total_bytes(), VILLAGE_COMPRESSED_BYTES);
    assert_eq!(catalog.all_len(), VILLAGE_TEMPLATES + MORE_TEMPLATES);
    assert_eq!(
        catalog.all_total_bytes(),
        VILLAGE_COMPRESSED_BYTES + MORE_COMPRESSED_BYTES
    );
    assert_eq!(
        catalog.all_ids().count(),
        VILLAGE_TEMPLATES + MORE_TEMPLATES
    );
    assert!(
        !catalog
            .all_ids()
            .any(|id| id.starts_with("minecraft:trial_chambers/")),
        "the secondary pin must stay village + non-village"
    );

    // The merged view adds every trial-chamber part.
    assert_eq!(
        catalog.merged_len(),
        VILLAGE_TEMPLATES + MORE_TEMPLATES + TRIAL_TEMPLATES
    );
    assert_eq!(
        catalog.merged_total_bytes(),
        VILLAGE_COMPRESSED_BYTES + MORE_COMPRESSED_BYTES + TRIAL_COMPRESSED_BYTES
    );
    assert_eq!(catalog.merged_nbt_bytes(), MORE_NBT_BYTES + TRIAL_NBT_BYTES);
    assert_eq!(
        catalog.merged_ids().count(),
        VILLAGE_TEMPLATES + MORE_TEMPLATES + TRIAL_TEMPLATES
    );

    assert!(catalog.contains(CHAMBER_1));
    assert!(catalog.contains(CORRIDOR_STRAIGHT_1));
    assert!(catalog.contains(REWARD_VAULT));
    assert!(catalog.contains(SPAWNER_BREEZE));

    // Lookups cross split-part boundaries: each id comes from a different
    // gzip stream in the trial asset.
    let chamber = catalog.template(CHAMBER_1).expect("chamber part");
    assert_eq!(chamber.size(), [24, 19, 29]);
    assert!(TemplateProvider::template(catalog, CHAMBER_1).is_some());
    assert!(catalog.is_loaded(CHAMBER_1));

    let corridor = catalog
        .template(CORRIDOR_STRAIGHT_1)
        .expect("corridor part");
    assert_eq!(corridor.size(), [19, 19, 5]);
    assert!(catalog.is_loaded(CORRIDOR_STRAIGHT_1));

    let reward = catalog.template(REWARD_VAULT).expect("reward part");
    assert_eq!(reward.size(), [3, 3, 2]);
    assert!(catalog.is_loaded(REWARD_VAULT));

    let spawner = catalog.template(SPAWNER_BREEZE).expect("spawner part");
    assert_eq!(spawner.size(), [3, 2, 3]);
    assert!(catalog.is_loaded(SPAWNER_BREEZE));

    // A trial-only catalog cannot resolve village or non-village ids.
    let trial = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_TRIAL)
        .expect("trial catalog parses");
    assert_eq!(
        trial.template(VILLAGE_MEETING_POINT).unwrap_err(),
        CatalogError::UnknownTemplate {
            template: VILLAGE_MEETING_POINT.to_owned()
        }
    );
    assert_eq!(
        trial.template(END_CITY_SHIP).unwrap_err(),
        CatalogError::UnknownTemplate {
            template: END_CITY_SHIP.to_owned()
        }
    );
}

#[test]
fn representative_chamber_has_bounds_palette_and_block_entities() {
    let catalog = StructureTemplateCatalog::shipped();
    let template = catalog.template(CHAMBER_1).expect("chamber_1 template");

    assert_eq!(template.data_version(), 4671);
    assert_eq!(template.size(), [24, 19, 29]);
    assert_eq!(template.palette_count(), 1);
    assert_eq!(template.block_count(), 13_224);
    assert_eq!(template.entity_count(), 0);

    let palette = template.palette(0).expect("chamber_1 palette");
    assert_eq!(palette.len(), 21);
    let expected_names = [
        "minecraft:tuff_bricks",
        "minecraft:waxed_oxidized_cut_copper",
        "minecraft:waxed_oxidized_copper",
        "minecraft:waxed_copper_block",
        "minecraft:waxed_copper_bulb",
        "minecraft:chiseled_tuff_bricks",
        "minecraft:polished_tuff",
        "minecraft:oxidized_cut_copper",
        "minecraft:air",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
        "minecraft:jigsaw",
    ];
    assert_eq!(
        palette
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>(),
        expected_names
    );
    assert_eq!(property(&palette[4], "lit"), Some("true"));
    assert_eq!(property(&palette[4], "powered"), Some("false"));
    let orientations: Vec<&str> = palette[9..]
        .iter()
        .map(|state| property(state, "orientation").expect("jigsaw orientation"))
        .collect();
    assert_eq!(
        orientations,
        [
            "up_north",
            "west_up",
            "up_east",
            "east_up",
            "up_west",
            "up_south",
            "down_east",
            "down_south",
            "down_north",
            "north_up",
            "south_up",
            "down_west",
        ]
    );

    let blocks = template.blocks();
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
    assert_eq!(max, (23, 18, 28));

    let block_entities: Vec<_> = blocks.iter().filter(|block| block.nbt.is_some()).collect();
    assert_eq!(block_entities.len(), 29);
    assert!(
        block_entities.iter().all(|block| {
            block
                .nbt
                .as_ref()
                .and_then(|nbt| nbt.get("id"))
                .and_then(Tag::as_str)
                == Some("minecraft:jigsaw")
        }),
        "every chamber_1 block entity is a jigsaw connector"
    );

    let connector = blocks
        .iter()
        .find(|block| block.pos.x() == 7 && block.pos.y() == 1 && block.pos.z() == 12)
        .expect("first jigsaw connector");
    assert_eq!(
        connector.state, 9,
        "connector uses the up_north palette entry"
    );
    let nbt = connector.nbt.as_ref().expect("jigsaw block-entity nbt");
    assert_eq!(
        nbt.get("joint").and_then(Tag::as_str),
        Some("aligned"),
        "joint"
    );
    assert_eq!(
        nbt.get("name").and_then(Tag::as_str),
        Some("minecraft:empty"),
        "name"
    );
    assert_eq!(
        nbt.get("pool").and_then(Tag::as_str),
        Some("minecraft:trial_chambers/chamber/addon"),
        "pool"
    );
    assert_eq!(
        nbt.get("target").and_then(Tag::as_str),
        Some("minecraft:c1_breeze"),
        "target"
    );
    assert_eq!(
        nbt.get("final_state").and_then(Tag::as_str),
        Some("minecraft:waxed_oxidized_copper"),
        "final_state"
    );
    assert_eq!(
        nbt.get("placement_priority").and_then(Tag::as_int),
        Some(2),
        "placement_priority"
    );
    assert_eq!(
        nbt.get("selection_priority").and_then(Tag::as_int),
        Some(2),
        "selection_priority"
    );
}

#[test]
fn unknown_and_corrupt_trial_payloads_are_explicit_errors() {
    let trial = StructureTemplateCatalog::from_indexed_json(EMBEDDED_STRUCTURE_TEMPLATES_TRIAL)
        .expect("embedded trial catalog parses");
    let error = trial
        .template("minecraft:trial_chambers/chamber/missing")
        .expect_err("unknown template");
    assert!(matches!(
        error,
        CatalogError::UnknownTemplate { ref template }
            if template == "minecraft:trial_chambers/chamber/missing"
    ));

    // Valid schema and base64, but the decoded bytes are not gzip NBT.
    let corrupt = StructureTemplateCatalog::from_indexed_json(
        r#"{"blobs":{"part":{"bytes":8,"payload":"bm90IGd6aXA="}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":8,"offset":0}},"total_bytes":8,"version":"1.21.11"}"#,
    )
    .expect("schema is valid");
    let error = corrupt
        .template("minecraft:trial_chambers/test")
        .expect_err("corrupt split-part payload");
    assert!(matches!(
        error,
        CatalogError::CorruptPayload { ref template, .. }
            if template == "minecraft:trial_chambers/test"
    ));

    for malformed in [
        // Invalid base64 alphabet in a split part.
        r#"{"blobs":{"part":{"bytes":1,"payload":"!!!"}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // A split slice extends past its part.
        r#"{"blobs":{"part":{"bytes":8,"payload":"eA=="}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":4,"offset":5}},"total_bytes":1,"version":"1.21.11"}"#,
        // Slices inside a split part are not contiguous.
        r#"{"blobs":{"part":{"bytes":8,"payload":"eA=="}},"counts":{"by_root":{"part":2},"templates":2},"data_version":4788,"nbt_bytes":8,"templates":{"minecraft:trial_chambers/a":{"blob":"part","length":4,"offset":0},"minecraft:trial_chambers/b":{"blob":"part","length":3,"offset":5}},"total_bytes":1,"version":"1.21.11"}"#,
        // Unknown split-part reference.
        r#"{"blobs":{"part":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:trial_chambers/test":{"blob":"missing","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // total_bytes disagrees with the split-part base64 length.
        r#"{"blobs":{"part":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":1,"offset":0}},"total_bytes":2,"version":"1.21.11"}"#,
        // nbt_bytes disagrees with the split part's byte count.
        r#"{"blobs":{"part":{"bytes":2,"payload":"eA=="}},"counts":{"by_root":{"part":1},"templates":1},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":2,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
        // counts.templates disagrees with the map.
        r#"{"blobs":{"part":{"bytes":1,"payload":"eA=="}},"counts":{"by_root":{"part":1},"templates":2},"data_version":4788,"nbt_bytes":1,"templates":{"minecraft:trial_chambers/test":{"blob":"part","length":1,"offset":0}},"total_bytes":1,"version":"1.21.11"}"#,
    ] {
        assert!(
            StructureTemplateCatalog::from_indexed_json(malformed).is_err(),
            "malformed split-part catalog must fail loudly: {malformed}"
        );
    }
}

#[test]
fn extractor_regenerates_byte_identical_trial_catalog() {
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
            && stdout.contains("structure_templates_more.json")
            && stdout.contains("structure_templates_trial.json"),
        "extractor -Check must validate all three catalogs:\n{stdout}"
    );
}
