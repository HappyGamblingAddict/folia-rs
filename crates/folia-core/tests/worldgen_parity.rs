use folia_core::worldgen::diagnostics::{
    ChunkKey, DUMP_DATA_VERSION, DUMP_FORMAT_VERSION, FeatureDifference, FeatureMismatchKind,
    FeaturePlacement, ParityReport, SampledBlock, StructureDifference, StructureDump,
    StructureMismatchKind, TerrainComparison, WorldgenDump, block_class, chunk_digest,
    compare_features, compare_structures,
};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureGenerator;
use std::collections::{BTreeMap, BTreeSet};

const JAVA_CHUNK: &str = include_str!("../assets/java-chunk-0--1.txt");

#[derive(Debug, Clone, Copy)]
struct OracleFixture {
    name: &'static str,
    text: &'static str,
    seed: i64,
    chunk: (i32, i32),
}

const ORACLE_FIXTURES: &[OracleFixture] = &[OracleFixture {
    name: "java-chunk-0--1.txt",
    text: JAVA_CHUNK,
    seed: 12_345,
    chunk: (0, -1),
}];
const TARGET_SEEDS: &[i64] = &[12_345, 0, 1, -1];
const TARGET_CHUNKS: &[(i32, i32)] = &[(0, -1), (1, -1), (0, 0), (1, 0)];
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;

#[derive(Debug, Clone)]
struct JavaChunkFixture {
    seed: i64,
    chunk: (i32, i32),
    columns: BTreeMap<(i32, i32), BTreeMap<i32, String>>,
}

impl JavaChunkFixture {
    fn parse(text: &str) -> Self {
        let mut seed = None;
        let mut chunk = None;
        let mut columns = BTreeMap::new();
        for raw in text.lines() {
            let line = raw.trim_end();
            let mut parts = line.split('\t');
            match parts.next() {
                Some("seed") => {
                    seed = Some(
                        parts
                            .next()
                            .expect("seed value")
                            .parse()
                            .expect("seed integer"),
                    );
                }
                Some("chunk") => {
                    let x = parts
                        .next()
                        .expect("chunk x")
                        .parse()
                        .expect("chunk x integer");
                    let z = parts
                        .next()
                        .expect("chunk z")
                        .parse()
                        .expect("chunk z integer");
                    chunk = Some((x, z));
                }
                Some("column") => {
                    let x = parts
                        .next()
                        .expect("column x")
                        .parse()
                        .expect("column x integer");
                    let z = parts
                        .next()
                        .expect("column z")
                        .parse()
                        .expect("column z integer");
                    let mut blocks = BTreeMap::new();
                    for entry in parts.filter(|entry| !entry.is_empty()) {
                        let (y, block) = entry.split_once('=').expect("y=block column entry");
                        let y = y.parse().expect("column y integer");
                        assert!(
                            blocks.insert(y, block.to_owned()).is_none(),
                            "duplicate y={y}"
                        );
                    }
                    assert!(
                        columns.insert((x, z), blocks).is_none(),
                        "duplicate column ({x},{z})"
                    );
                }
                Some("") | None => {}
                Some(other) => panic!("unknown Java fixture record {other}"),
            }
        }
        let fixture = Self {
            seed: seed.expect("fixture seed"),
            chunk: chunk.expect("fixture chunk"),
            columns,
        };
        assert!(!fixture.columns.is_empty(), "fixture must list columns");
        for &(x, z) in fixture.columns.keys() {
            assert_eq!(x.div_euclid(16), fixture.chunk.0);
            assert_eq!(z.div_euclid(16), fixture.chunk.1);
        }
        fixture
    }

    fn compare_terrain(&self, rust_chunk: &folia_core::chunk::Chunk) -> TerrainComparison {
        let mut comparison = TerrainComparison::default();
        for (&(world_x, world_z), java_blocks) in &self.columns {
            let local_x = world_x.rem_euclid(16) as usize;
            let local_z = world_z.rem_euclid(16) as usize;
            for y in MIN_Y..MAX_Y {
                let rust_name = rust_chunk
                    .block_state(local_x, y, local_z)
                    .map(|state| state.name.as_str())
                    .unwrap_or("minecraft:air");
                let java_name = java_blocks.get(&y).map_or("minecraft:air", String::as_str);
                let rust_class = block_class(rust_name);
                let java_class = block_class(java_name);
                comparison.total += 1;
                if rust_name == java_name {
                    comparison.exact_matches += 1;
                } else {
                    *comparison
                        .name_confusions
                        .entry((rust_name.to_owned(), java_name.to_owned()))
                        .or_insert(0) += 1;
                }
                if rust_class == java_class {
                    comparison.class_matches += 1;
                } else {
                    *comparison
                        .class_confusions
                        .entry((rust_class, java_class))
                        .or_insert(0) += 1;
                }
                if rust_class == 2 {
                    comparison.rust_water += 1;
                }
                if java_class == 2 {
                    comparison.java_water += 1;
                }
                if (rust_class == 2) != (java_class == 2) {
                    comparison.water_mismatches += 1;
                }
            }
        }
        comparison
    }
}

#[test]
fn generated_chunk_matches_java_per_block() {
    let mut report = ParityReport::default();
    let mut compared_fixtures = 0;
    let mut expected_total = 0u64;
    for oracle in ORACLE_FIXTURES {
        let fixture = JavaChunkFixture::parse(oracle.text);
        assert_eq!(fixture.seed, oracle.seed, "{} seed", oracle.name);
        assert_eq!(fixture.chunk, oracle.chunk, "{} chunk", oracle.name);
        expected_total += fixture.columns.len() as u64 * (MAX_Y - MIN_Y) as u64;
        let generator = WorldGenerator::overworld(fixture.seed).unwrap();
        let chunk = generator
            .generate_chunk(fixture.chunk.0, fixture.chunk.1)
            .unwrap();
        report.merge_terrain(&fixture.compare_terrain(&chunk));
        compared_fixtures += 1;
    }

    assert_eq!(compared_fixtures, ORACLE_FIXTURES.len());
    assert_eq!(report.terrain.total, expected_total);
    println!("{report}");
    assert!(
        report.terrain.class_agreement() > 0.98,
        "block-class agreement too low: {:.3}",
        report.terrain.class_agreement()
    );
    assert!(
        report.terrain.exact_agreement() > 0.915,
        "exact block-name agreement too low: {:.3}",
        report.terrain.exact_agreement()
    );
}

#[test]
fn java_oracle_matrix_coverage_is_explicit() {
    let fixtures: Vec<_> = ORACLE_FIXTURES
        .iter()
        .map(|oracle| {
            let fixture = JavaChunkFixture::parse(oracle.text);
            assert_eq!((fixture.seed, fixture.chunk), (oracle.seed, oracle.chunk));
            fixture
        })
        .collect();
    let mut covered = BTreeSet::new();
    let mut missing = BTreeSet::new();
    for &seed in TARGET_SEEDS {
        for &(x, z) in TARGET_CHUNKS {
            let key = ChunkKey {
                seed,
                chunk: [x, z],
            };
            if fixtures
                .iter()
                .any(|fixture| fixture.seed == seed && fixture.chunk == (x, z))
            {
                covered.insert(key);
            } else {
                missing.insert(key);
            }
        }
    }

    assert_eq!(covered.len() + missing.len(), 16);
    assert!(!covered.is_empty());
    println!(
        "Java oracle matrix: {}/16 seed-chunk pairs, {}/4096 columns",
        covered.len(),
        fixtures
            .iter()
            .map(|fixture| fixture.columns.len())
            .sum::<usize>()
    );
    for fixture in &fixtures {
        println!(
            "Java fixture seed={} chunk={:?}: {}/256 columns; feature oracle=missing; structure oracle=missing",
            fixture.seed,
            fixture.chunk,
            fixture.columns.len()
        );
    }
    for key in &missing {
        println!("missing Java oracle: {key}");
    }
}

#[test]
fn rust_only_four_seed_four_chunk_matrix_has_stable_complete_columns() {
    let expected = BTreeMap::from([
        ((-1, 0, -1), 0x97f8_be1b_268f_3b7e),
        ((-1, 0, 0), 0xac42_5e88_8a3c_9aaf),
        ((-1, 1, -1), 0xdbc3_10f0_ce76_306c),
        ((-1, 1, 0), 0xc7d3_fd6c_edd9_0b4d),
        ((0, 0, -1), 0x9da5_20e0_15ac_20d2),
        ((0, 0, 0), 0x00fa_ad42_1adb_e9be),
        ((0, 1, -1), 0x7173_75de_f228_1582),
        ((0, 1, 0), 0x3d63_2cfd_aaa5_1130),
        ((1, 0, -1), 0x1bf8_a970_3acf_3c77),
        ((1, 0, 0), 0x21de_57b5_7c63_69b7),
        ((1, 1, -1), 0x757f_246b_1c09_ebcd),
        ((1, 1, 0), 0x93ea_007f_a52f_deda),
        ((12_345, 0, -1), 0x9976_5447_f433_a939),
        ((12_345, 0, 0), 0xf518_64c4_3b5a_13b7),
        ((12_345, 1, -1), 0x54d8_41df_61c4_4bd4),
        ((12_345, 1, 0), 0x3916_6acf_8368_5d76),
    ]);
    let mut actual = BTreeMap::new();
    let mut report = ParityReport::default();
    for &seed in TARGET_SEEDS {
        let generator = WorldGenerator::overworld(seed).unwrap();
        for &(x, z) in TARGET_CHUNKS {
            let generated = generator.generate_chunk_with_diagnostics(x, z).unwrap();
            let mut non_air = 0u64;
            for local_x in 0..16 {
                for local_z in 0..16 {
                    for y in MIN_Y..MAX_Y {
                        let state = generated
                            .chunk
                            .block_state(local_x, y, local_z)
                            .expect("every generated column block exists");
                        if state.name != "minecraft:air" {
                            non_air += 1;
                        }
                    }
                    assert_eq!(
                        generated
                            .chunk
                            .block_state(local_x, MIN_Y, local_z)
                            .unwrap()
                            .name,
                        "minecraft:bedrock"
                    );
                    assert_eq!(
                        generated
                            .chunk
                            .block_state(local_x, MAX_Y - 1, local_z)
                            .unwrap()
                            .name,
                        "minecraft:air"
                    );
                }
            }
            assert!(non_air > 0);
            assert!(!generated.diagnostics.features.is_empty());
            for feature in &generated.diagnostics.features {
                assert!(!feature.placed_feature.is_empty());
                assert!(!feature.configured_feature.is_empty());
                assert!(!feature.decoration.is_empty());
                assert!(
                    feature
                        .written_blocks
                        .windows(2)
                        .all(|pair| pair[0] < pair[1])
                );
                for &[wx, wy, wz] in &feature.written_blocks {
                    assert_eq!(wx.div_euclid(16), x);
                    assert_eq!(wz.div_euclid(16), z);
                    assert!((MIN_Y..MAX_Y).contains(&wy));
                }
            }
            report.observe_generation(&generated.diagnostics);
            actual.insert((seed, x, z), chunk_digest(&generated.chunk));
        }
    }

    println!("Rust-only matrix digests (not Java parity):");
    for ((seed, x, z), digest) in &actual {
        println!("  seed={seed} chunk=({x},{z}) digest={digest:#018x}");
    }
    println!(
        "Rust-only feature observations: {} placements across {} features",
        report.observed_features.values().sum::<u64>(),
        report.observed_features.len()
    );
    assert_eq!(actual.len(), 16);
    assert_eq!(
        actual, expected,
        "Rust-only worldgen regression baseline changed"
    );
}

#[test]
fn feature_placement_diagnostics_and_blocks_are_stable() {
    let seed = 12_345;
    let generator = WorldGenerator::overworld(seed).unwrap();
    let generated = generator.generate_chunk_with_diagnostics(0, -1).unwrap();
    assert_eq!(chunk_digest(&generated.chunk), 0x9976_5447_f433_a939);
    assert!(!generated.diagnostics.features.is_empty());
    let digest = feature_diagnostics_digest(&generated.diagnostics.features);
    println!(
        "seed={seed} chunk=(0,-1): {} feature placements, diagnostics digest={digest:#018x}",
        generated.diagnostics.features.len()
    );
    assert_eq!(
        digest, 0x7ea2_39c7_cd23_9e69,
        "feature placement diagnostics changed"
    );
}

#[test]
fn structured_dump_is_deterministic_and_samples_real_starts() {
    let seed = 12_345;
    let generator = WorldGenerator::overworld(seed)
        .unwrap()
        .with_structure_generator(StructureGenerator::shipped_catalog().unwrap());
    let first = generator.generate_chunk_with_diagnostics(6, 123).unwrap();
    let second = generator.generate_chunk_with_diagnostics(6, 123).unwrap();
    let first_dump = WorldgenDump::from_chunk(seed, &first, 4);
    let second_dump = WorldgenDump::from_chunk(seed, &second, 4);
    assert_eq!(first_dump, second_dump);
    let first_json = first_dump.to_json_string();
    let second_json = second_dump.to_json_string();
    assert_eq!(first_json, second_json);

    let value: serde_json::Value = serde_json::from_str(&first_json).unwrap();
    assert_eq!(value["format_version"], DUMP_FORMAT_VERSION);
    assert_eq!(value["data_version"], DUMP_DATA_VERSION);
    assert_eq!(value["seed"], seed);
    assert_eq!(value["chunks"][0]["chunk"], serde_json::json!([6, 123]));
    assert!(!first.diagnostics.features.is_empty());
    assert!(first.diagnostics.features.iter().any(|feature| {
        feature.decoration == "structure_feature_pool" && !feature.written_blocks.is_empty()
    }));
    assert!(!first.diagnostics.structures.is_empty());
    for start in value["chunks"][0]["structures"].as_array().unwrap() {
        assert_eq!(start["start_chunk"].as_array().unwrap().len(), 2);
        assert_eq!(start["bounds"].as_array().unwrap().len(), 6);
        let samples = start["sampled_blocks"].as_array().unwrap();
        assert!(!samples.is_empty());
        for sample in samples {
            let position = sample["position"].as_array().unwrap();
            assert_eq!((position[0].as_i64().unwrap() / 16) as i32, 6);
            assert_eq!((position[2].as_i64().unwrap() / 16) as i32, 123);
            assert!(!sample["state"].as_str().unwrap().is_empty());
        }
    }
}

fn feature(
    id: &str,
    origin: [i32; 3],
    success: bool,
    written_blocks: Vec<[i32; 3]>,
) -> FeaturePlacement {
    FeaturePlacement {
        placed_feature: id.to_owned(),
        configured_feature: format!("{id}_configured"),
        decoration: "vegetal_decoration".to_owned(),
        origin,
        success,
        written_blocks,
    }
}

fn structure(
    id: &str,
    start_chunk: [i32; 2],
    bounds: [i32; 6],
    sampled_blocks: Vec<SampledBlock>,
) -> StructureDump {
    StructureDump {
        structure_id: id.to_owned(),
        start_chunk,
        bounds: Some(bounds),
        placed_pieces: 1,
        skipped_pieces: 0,
        written: 1,
        rejected: 0,
        feature_elements: 0,
        feature_placements: 0,
        sampled_blocks,
    }
}

#[test]
fn parity_report_aggregates_and_classifies_named_mismatches() {
    let java_features = vec![
        feature("minecraft:test:same", [0, 64, 0], true, vec![[0, 64, 0]]),
        feature("minecraft:test:missing", [1, 64, 0], true, vec![]),
        feature("minecraft:test:result", [2, 64, 0], true, vec![]),
        feature("minecraft:test:writes", [3, 64, 0], true, vec![[3, 64, 0]]),
    ];
    let rust_features = vec![
        feature("minecraft:test:same", [0, 64, 0], true, vec![[0, 64, 0]]),
        feature("minecraft:test:extra", [4, 64, 0], true, vec![]),
        feature("minecraft:test:result", [2, 64, 0], false, vec![]),
        feature("minecraft:test:writes", [3, 64, 0], true, vec![[3, 65, 0]]),
    ];
    let feature_differences = compare_features(&java_features, &rust_features);
    assert!(feature_differences["minecraft:test:same"].is_match());
    assert_eq!(
        feature_differences["minecraft:test:missing"].mismatches[0].kind,
        FeatureMismatchKind::MissingInRust
    );
    assert_eq!(
        feature_differences["minecraft:test:extra"].mismatches[0].kind,
        FeatureMismatchKind::UnexpectedInRust
    );
    assert_eq!(
        feature_differences["minecraft:test:result"].mismatches[0].kind,
        FeatureMismatchKind::Success
    );
    assert_eq!(
        feature_differences["minecraft:test:writes"].mismatches[0].kind,
        FeatureMismatchKind::WrittenBlocks
    );

    let java_structures = vec![
        structure(
            "minecraft:test:compound",
            [0, 0],
            [0, 64, 0, 15, 80, 15],
            vec![],
        ),
        structure(
            "minecraft:test:missing",
            [1, 0],
            [16, 64, 0, 31, 80, 15],
            vec![],
        ),
    ];
    let rust_structures = vec![
        structure(
            "minecraft:test:compound",
            [0, 0],
            [0, 65, 0, 15, 80, 15],
            vec![],
        ),
        structure(
            "minecraft:test:extra",
            [2, 0],
            [32, 64, 0, 47, 80, 15],
            vec![],
        ),
    ];
    let structure_differences = compare_structures(&java_structures, &rust_structures);
    assert_eq!(
        structure_differences["minecraft:test:compound"].mismatches[0].kind,
        StructureMismatchKind::Bounds
    );
    assert_eq!(
        structure_differences["minecraft:test:missing"].mismatches[0].kind,
        StructureMismatchKind::MissingInRust
    );
    assert_eq!(
        structure_differences["minecraft:test:extra"].mismatches[0].kind,
        StructureMismatchKind::UnexpectedInRust
    );

    let mut report = ParityReport::default();
    report.merge_terrain(&TerrainComparison {
        total: 10,
        exact_matches: 8,
        class_matches: 9,
        ..TerrainComparison::default()
    });
    report.merge_features(&feature_differences);
    report.merge_features(&compare_features(&java_features, &rust_features));
    report.merge_structures(&structure_differences);
    report.merge_structures(&compare_structures(&java_structures, &rust_structures));
    assert_eq!(report.terrain.total, 10);
    assert_eq!(report.features["minecraft:test:same"].java_placements, 2);
    assert_eq!(report.structures["minecraft:test:compound"].java_starts, 2);
    assert!(report.has_named_mismatches());
    let rendered = report.to_string();
    assert!(rendered.contains("feature minecraft:test:missing"));
    assert!(rendered.contains("structure minecraft:test:compound"));
    assert!(rendered.contains("missing_in_rust"));
    assert!(rendered.contains("bounds"));
}

#[test]
fn empty_oracle_categories_are_not_reported_as_parity() {
    let mut report = ParityReport::default();
    let mut diagnostics = folia_core::worldgen::diagnostics::GenerationDiagnostics::default();
    diagnostics
        .features
        .push(feature("minecraft:test:observed", [0, 64, 0], true, vec![]));
    report.observe_generation(&diagnostics);
    let rendered = report.to_string();
    assert!(rendered.contains("feature oracle: unavailable"));
    assert!(rendered.contains("structure oracle: unavailable"));
    assert!(!report.has_named_mismatches());
    assert_eq!(report.observed_features["minecraft:test:observed"], 1);
}

fn feature_diagnostics_digest(features: &[FeaturePlacement]) -> u64 {
    fn feed_bytes(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn feed_str(hash: &mut u64, value: &str) {
        feed_bytes(hash, &(value.len() as u64).to_le_bytes());
        feed_bytes(hash, value.as_bytes());
    }

    fn feed_i32(hash: &mut u64, value: i32) {
        feed_bytes(hash, &value.to_le_bytes());
    }

    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    feed_bytes(&mut hash, &(features.len() as u64).to_le_bytes());
    for feature in features {
        feed_str(&mut hash, &feature.placed_feature);
        feed_str(&mut hash, &feature.configured_feature);
        feed_str(&mut hash, &feature.decoration);
        for coordinate in feature.origin {
            feed_i32(&mut hash, coordinate);
        }
        feed_bytes(&mut hash, &[u8::from(feature.success)]);
        feed_bytes(
            &mut hash,
            &(feature.written_blocks.len() as u64).to_le_bytes(),
        );
        for position in &feature.written_blocks {
            for coordinate in position {
                feed_i32(&mut hash, *coordinate);
            }
        }
    }
    hash
}

#[test]
fn report_merge_preserves_zero_count_difference_types() {
    let mut features = BTreeMap::new();
    features.insert(
        "minecraft:test:empty".to_owned(),
        FeatureDifference::default(),
    );
    let mut structures = BTreeMap::new();
    structures.insert(
        "minecraft:test:empty".to_owned(),
        StructureDifference::default(),
    );
    let mut report = ParityReport::default();
    report.merge_features(&features);
    report.merge_structures(&structures);
    assert!(report.feature_oracle_available);
    assert!(report.structure_oracle_available);
    assert!(report.features["minecraft:test:empty"].is_match());
    assert!(report.structures["minecraft:test:empty"].is_match());
}
