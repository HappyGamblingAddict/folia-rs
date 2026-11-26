//! Deterministic robustness / property tests for untrusted non-network data (P11.3).
//!
//! Every case is seeded, bounded and in-process: no external corpus, no new
//! dependencies, no hangs. Decoders may reject input with an `io::Error`; the
//! invariant under test is that they never panic.

use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use folia_core::item_components::{self, ComponentType, KNOWN_COMPONENT_TYPES};
use folia_core::light::SectionLight;
use folia_core::nbt::{self, Tag};
use folia_core::snbt;
use folia_random::{LegacyRandomSource, RandomSource};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};

const SEED: i64 = 0x5048_3900_4788_4A21;

fn seeded_random() -> LegacyRandomSource {
    LegacyRandomSource::new(SEED)
}

fn assert_no_panic<T>(what: &str, operation: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(value) => value,
        Err(_) => panic!("{what}: untrusted input caused a panic"),
    }
}

fn random_bytes(random: &mut LegacyRandomSource, length: usize) -> Vec<u8> {
    (0..length).map(|_| random.next_bits(8) as u8).collect()
}

fn random_buffer(random: &mut LegacyRandomSource, max_len: usize) -> Vec<u8> {
    let length = random.next_int_bound(max_len as i32) as usize;
    random_bytes(random, length)
}

fn random_string(random: &mut LegacyRandomSource, max_len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefxyz0123456789_-.: ";
    let length = random.next_int_bound(max_len as i32) as usize;
    (0..length)
        .map(|_| ALPHABET[random.next_int_bound(ALPHABET.len() as i32) as usize] as char)
        .collect()
}

// 1. NBT: random byte buffers through all three entry points, including gzip
// decoder on non-gzip garbage. Rejection via io::Error is expected.
#[test]
fn random_nbt_never_panics() {
    let mut random = seeded_random();
    for _ in 0..300 {
        let bytes = random_buffer(&mut random, 1024);
        let _: io::Result<Tag> =
            assert_no_panic("nbt::read_root", || nbt::read_root(&mut bytes.as_slice()));
        let _: io::Result<Tag> = assert_no_panic("nbt::read_any_tag", || {
            nbt::read_any_tag(&mut bytes.as_slice())
        });
        let _: io::Result<Tag> = assert_no_panic("nbt::read_gzip_root (garbage)", || {
            nbt::read_gzip_root(&bytes)
        });
    }
}

// 2. SNBT: random strings from a restricted alphabet plus hand-written valid
// values. Valid values must parse; none of the inputs may panic.
#[test]
fn snbt_random_and_valid_never_panic() {
    const ALPHABET: &[u8] = b"{}[]\"':,0123456789abcdefBILSTflseutr-+.; \t\n\\";
    let mut random = seeded_random();
    for _ in 0..300 {
        let length = random.next_int_bound(64) as usize;
        let text: String = (0..length)
            .map(|_| ALPHABET[random.next_int_bound(ALPHABET.len() as i32) as usize] as char)
            .collect();
        let _: Result<Tag, snbt::SnbtError> =
            assert_no_panic("snbt::parse (random)", || snbt::parse(&text));
    }

    let valid = [
        "{}",
        "[]",
        "{a: 1, b: [1, 2, 3]}",
        r#"{"name": "value", flag: true}"#,
        "[B; 1b, 2b, 3b]",
        "[I; 1, -2, 3]",
        "[L; 1L, 2L]",
        "42",
        "\"hello\"",
        "{nested: {deep: [1, 2]}}",
        "true",
        "false",
        "1.5f",
        "-3.5d",
        "{a: 1, a: 2}",
    ];
    for text in valid {
        let parsed = assert_no_panic("snbt::parse (valid)", || snbt::parse(text));
        assert!(
            parsed.is_ok(),
            "valid SNBT `{text}` was rejected: {parsed:?}"
        );
    }
}

fn assert_palette_round_trips(
    original: &PalettedContainer<BlockState>,
    decoded: &PalettedContainer<BlockState>,
) {
    assert_eq!(original.len(), decoded.len());
    for (index, _) in original.indices.iter().enumerate() {
        let expected = original.get(index).expect("original index was validated");
        let actual = decoded.get(index).expect("decoded index was validated");
        assert_eq!(
            actual, expected,
            "state at index {index} changed after round trip"
        );
    }
}

// 3. Chunk palettes: random palettes and index arrays (some indices deliberately
// out of range). Encoding either round-trips through decode or returns an error.
#[test]
fn random_chunk_palettes_never_panic() {
    let mut random = seeded_random();
    for _ in 0..300 {
        let palette_len = random.next_int_bound(9) as usize;
        let palette: Vec<BlockState> = (0..palette_len)
            .map(|index| BlockState {
                name: format!("minecraft:fuzz_block_{index}"),
                properties: Vec::new(),
            })
            .collect();

        let value_bound = palette_len.saturating_add(2);
        let indices: Vec<u16> = if random.next_bits(1) == 0 {
            (0..4096)
                .map(|_| random.next_int_bound(value_bound.max(1) as i32) as u16)
                .collect()
        } else {
            let length = random.next_int_bound(4096) as usize;
            (0..length)
                .map(|_| random.next_int_bound(value_bound.max(1) as i32) as u16)
                .collect()
        };

        let chunk = Chunk {
            status: folia_core::chunk::ChunkStatus::Full,
            light_present: false,
            x: 0,
            z: 0,
            sections: vec![ChunkSection {
                y: -4,
                block_states: PalettedContainer { palette, indices },
                biomes: None,
                light: SectionLight::default(),
            }],
        };

        let encoded = assert_no_panic("encode_chunk (random palette)", || encode_chunk(&chunk));
        if let Ok(tag) = encoded {
            let decoded = assert_no_panic("decode_chunk (random palette)", || decode_chunk(&tag));
            let decoded = decoded.expect("a successfully encoded chunk must decode");
            let original_section = &chunk.sections[0];
            let decoded_section = decoded.section(-4).expect("section Y preserved");
            assert_palette_round_trips(
                &original_section.block_states,
                &decoded_section.block_states,
            );
        }
    }
}

const JSON_ASSETS: &[(&str, &str)] = &[
    ("worldgen.json", include_str!("../assets/worldgen.json")),
    (
        "feature_data.json",
        include_str!("../assets/feature_data.json"),
    ),
    (
        "item_prototypes.json",
        include_str!("../assets/item_prototypes.json"),
    ),
    (
        "structure_processors.json",
        include_str!("../assets/structure_processors.json"),
    ),
];

const MUTATION_BYTES: &[u8] = b"{}[]\",:\\\0nt0123456789xyz ";

fn parse_json_no_panic(label: &str, bytes: &[u8]) {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _: Result<serde_json::Value, _> =
            assert_no_panic(label, || serde_json::from_str::<serde_json::Value>(text));
    }
}

// 4. Worldgen/asset JSON: truncations and single-byte mutations.
#[test]
fn mutated_json_never_panics() {
    let mut random = seeded_random();
    for &(name, original) in JSON_ASSETS {
        let bytes = original.as_bytes();
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(original);
        assert!(parsed.is_ok(), "asset {name} is not valid JSON");
        assert!(!bytes.is_empty(), "asset {name} is empty");

        for _ in 0..30 {
            let end = random.next_int_bound(bytes.len() as i32) as usize;
            parse_json_no_panic("serde_json::from_str (truncated)", &bytes[..end]);
        }
        for _ in 0..30 {
            let mut mutated = bytes.to_vec();
            let offset = random.next_int_bound(bytes.len() as i32) as usize;
            mutated[offset] =
                MUTATION_BYTES[random.next_int_bound(MUTATION_BYTES.len() as i32) as usize];
            parse_json_no_panic("serde_json::from_str (mutated)", &mutated);
        }
    }
}

fn random_tag(random: &mut LegacyRandomSource, depth: usize) -> Tag {
    let variants = if depth == 0 { 7 } else { 12 };
    match random.next_int_bound(variants) {
        0 => Tag::Byte(random.next_bits(8) as i8),
        1 => Tag::Short(random.next_bits(16) as i16),
        2 => Tag::Int(random.next_int()),
        3 => Tag::Long(random.next_long()),
        4 => Tag::Float(f32::from_bits(random.next_bits(32) as u32)),
        5 => Tag::Double(f64::from_bits(random.next_long() as u64)),
        6 => Tag::String(random_string(random, 12)),
        7 => Tag::ByteArray(random_bytes(random, 8)),
        8 => Tag::IntArray(
            (0..random.next_int_bound(6))
                .map(|_| random.next_int())
                .collect(),
        ),
        9 => Tag::LongArray(
            (0..random.next_int_bound(6))
                .map(|_| random.next_long())
                .collect(),
        ),
        10 => {
            let length = random.next_int_bound(6) as usize;
            Tag::List((0..length).map(|_| random_tag(random, depth - 1)).collect())
        }
        _ => {
            let length = random.next_int_bound(6) as usize;
            Tag::Compound(
                (0..length)
                    .map(|_| (random_string(random, 8), random_tag(random, depth - 1)))
                    .collect(),
            )
        }
    }
}

// 5. Plugin-provided item data: random NBT tags through the item and component
// decoders. Malformed payloads are preserved as opaque or rejected with an error.
#[test]
fn random_item_data_never_panics() {
    let mut random = seeded_random();
    for _ in 0..200 {
        let tag = random_tag(&mut random, 3);
        let _ = assert_no_panic("item_stack_from_nbt (raw)", || {
            item_components::item_stack_from_nbt(&tag)
        });
        for (name, _) in KNOWN_COMPONENT_TYPES {
            let component = ComponentType::named(name);
            let _ = assert_no_panic("component_from_nbt (raw)", || {
                item_components::component_from_nbt(&component, &tag)
            });
        }

        let stack_tag = Tag::Compound(vec![
            ("id".into(), Tag::String(random_string(&mut random, 16))),
            ("count".into(), Tag::Int(random.next_int_bound(64) + 1)),
            ("components".into(), tag.clone()),
        ]);
        let _ = assert_no_panic("item_stack_from_nbt (stack-shaped)", || {
            item_components::item_stack_from_nbt(&stack_tag)
        });
    }
}
