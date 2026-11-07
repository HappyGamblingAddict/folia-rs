//! Deterministic, dependency-free stress and round-trip tests for the Rust
//! Folia core.
//!
//! Everything in this file uses only `std` plus the crate under test. Random
//! data comes from a tiny linear congruential generator so that failures are
//! reproducible from the fixed seeds below.

use folia_core::anvil::{Compression, HEADER_SECTORS, MAX_CHUNK_LENGTH, RegionFile, SECTOR_SIZE};
use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use folia_core::config::ServerProperties;
use folia_core::nbt::{
    Tag, read_gzip_root, read_named_root, read_root, read_zlib_root, write_gzip_root, write_named,
    write_root, write_root_to_vec, write_zlib_root,
};
use folia_core::protocol::{MAX_PACKET_SIZE, read_packet, write_packet};
use folia_core::region::{ChunkPos, RegionKey, RegionScheduler};
use folia_core::varint::{
    read_varint, read_varint_slice, read_varlong, read_varlong_slice, varint_len, varlong_len,
    write_varint, write_varint_to_vec, write_varlong, write_varlong_to_vec,
};
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Deterministic helpers
// ---------------------------------------------------------------------------

/// A small deterministic LCG. The state update is the classic 64-bit
/// Knuth/MMIX recurrence; the returned value is lightly mixed so that taking
/// the high 32 bits does not expose the weak low bits of the recurrence.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state ^ (self.state >> 29)
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn below(&mut self, upper: usize) -> usize {
        assert!(upper > 0, "Lcg::below needs a non-zero bound");
        (self.next_u64() % upper as u64) as usize
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_path(label: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before the unix epoch")
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "folia-rs-bulk-validation-{label}-{}-{nanos}-{counter}.{extension}",
        std::process::id()
    ))
}

/// Removes the file (and an Anvil `.tmp` sibling) even if a test panics.
struct TempPath {
    path: PathBuf,
}

impl TempPath {
    fn new(label: &str, extension: &str) -> Self {
        Self {
            path: unique_temp_path(label, extension),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("tmp"));
    }
}

// ---------------------------------------------------------------------------
// VarInt / VarLong
// ---------------------------------------------------------------------------

fn varint_boundaries() -> Vec<i32> {
    let mut values = vec![
        i32::MIN,
        i32::MIN + 1,
        i32::MAX,
        i32::MAX - 1,
        -1,
        0,
        1,
        -2,
        2,
        127,
        128,
        -127,
        -128,
        16_383,
        16_384,
        -16_383,
        -16_384,
    ];
    for shift in 0..32 {
        let power = 1i32.wrapping_shl(shift);
        values.push(power);
        values.push(power.wrapping_neg());
        let below = power.wrapping_sub(1);
        values.push(below);
        values.push(below.wrapping_neg());
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn varlong_boundaries() -> Vec<i64> {
    let mut values = vec![
        i64::MIN,
        i64::MIN + 1,
        i64::MAX,
        i64::MAX - 1,
        -1,
        0,
        1,
        -2,
        2,
        127,
        128,
        -127,
        -128,
        16_383,
        16_384,
    ];
    for shift in 0..64 {
        let power = 1i64.wrapping_shl(shift);
        values.push(power);
        values.push(power.wrapping_neg());
        let below = power.wrapping_sub(1);
        values.push(below);
        values.push(below.wrapping_neg());
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn assert_varint_round_trip(value: i32) {
    let bytes = write_varint_to_vec(value);
    assert!(!bytes.is_empty(), "empty VarInt for {value}");
    assert!(bytes.len() <= 5, "VarInt too long for {value}: {bytes:?}");
    assert_eq!(
        bytes.len(),
        varint_len(value),
        "varint_len mismatch for {value}"
    );

    let mut written = Vec::new();
    let count = write_varint(&mut written, value).expect("write_varint");
    assert_eq!(count, bytes.len(), "write_varint count for {value}");
    assert_eq!(written, bytes, "write_varint bytes for {value}");

    assert_eq!(
        read_varint_slice(&bytes).expect("read_varint_slice"),
        (value, bytes.len()),
        "read_varint_slice for {value}"
    );

    let mut cursor = Cursor::new(bytes.as_slice());
    assert_eq!(
        read_varint(&mut cursor).expect("read_varint"),
        value,
        "read_varint for {value}"
    );
    assert_eq!(cursor.position() as usize, bytes.len());

    // Encodings must be canonical: only the final byte may omit the
    // continuation bit.
    assert_eq!(bytes[bytes.len() - 1] & 0x80, 0, "last byte for {value}");
    for byte in &bytes[..bytes.len() - 1] {
        assert_eq!(byte & 0x80, 0x80, "continuation byte for {value}");
    }
}

fn assert_varlong_round_trip(value: i64) {
    let bytes = write_varlong_to_vec(value);
    assert!(!bytes.is_empty(), "empty VarLong for {value}");
    assert!(bytes.len() <= 10, "VarLong too long for {value}: {bytes:?}");
    assert_eq!(
        bytes.len(),
        varlong_len(value),
        "varlong_len mismatch for {value}"
    );

    let mut written = Vec::new();
    let count = write_varlong(&mut written, value).expect("write_varlong");
    assert_eq!(count, bytes.len(), "write_varlong count for {value}");
    assert_eq!(written, bytes, "write_varlong bytes for {value}");

    assert_eq!(
        read_varlong_slice(&bytes).expect("read_varlong_slice"),
        (value, bytes.len()),
        "read_varlong_slice for {value}"
    );

    let mut cursor = Cursor::new(bytes.as_slice());
    assert_eq!(
        read_varlong(&mut cursor).expect("read_varlong"),
        value,
        "read_varlong for {value}"
    );
    assert_eq!(cursor.position() as usize, bytes.len());

    assert_eq!(bytes[bytes.len() - 1] & 0x80, 0, "last byte for {value}");
    for byte in &bytes[..bytes.len() - 1] {
        assert_eq!(byte & 0x80, 0x80, "continuation byte for {value}");
    }
}

#[test]
fn varint_round_trips_boundaries_and_lcg_values() {
    let boundaries = varint_boundaries();
    assert!(boundaries.len() > 100);
    for &value in &boundaries {
        assert_varint_round_trip(value);
    }

    // All negative i32 values are encoded as a full five-byte u32 VarInt.
    for &value in &boundaries {
        if value < 0 {
            assert_eq!(varint_len(value), 5, "negative VarInt length for {value}");
        }
    }

    let known_lengths: &[(i32, usize)] = &[
        (0, 1),
        (1, 1),
        (127, 1),
        (128, 2),
        (16_383, 2),
        (16_384, 3),
        (2_097_151, 3),
        (2_097_152, 4),
        (268_435_455, 4),
        (268_435_456, 5),
        (i32::MAX, 5),
        (-1, 5),
        (i32::MIN, 5),
    ];
    for &(value, expected) in known_lengths {
        assert_eq!(varint_len(value), expected, "length for {value}");
        assert_eq!(
            write_varint_to_vec(value).len(),
            expected,
            "encoded length for {value}"
        );
    }

    let mut rng = Lcg::new(0x5EED_1234_ABCD_0001);
    for index in 0..10_000 {
        let value = rng.next_u32() as i32;
        assert_varint_round_trip(value);
        assert!(varint_len(value) <= 5, "case {index}");
    }
}

#[test]
fn varlong_round_trips_boundaries_and_lcg_values() {
    let boundaries = varlong_boundaries();
    assert!(boundaries.len() > 100);
    for &value in &boundaries {
        assert_varlong_round_trip(value);
    }

    // Any negative i64 has the top bit set and therefore needs ten bytes.
    for &value in &boundaries {
        if value < 0 {
            assert_eq!(
                varlong_len(value),
                10,
                "negative VarLong length for {value}"
            );
        }
    }

    let known_lengths: &[(i64, usize)] = &[
        (0, 1),
        (127, 1),
        (128, 2),
        (16_383, 2),
        (16_384, 3),
        (2_097_151, 3),
        (2_097_152, 4),
        ((1i64 << 56) - 1, 8),
        (1i64 << 56, 9),
        (i64::MAX, 9),
        (-1, 10),
        (i64::MIN, 10),
    ];
    for &(value, expected) in known_lengths {
        assert_eq!(varlong_len(value), expected, "length for {value}");
        assert_eq!(
            write_varlong_to_vec(value).len(),
            expected,
            "encoded length for {value}"
        );
    }

    let mut rng = Lcg::new(0xFEED_FACE_CAFE_BEEF);
    for index in 0..10_000 {
        let value = rng.next_u64() as i64;
        assert_varlong_round_trip(value);
        assert!(varlong_len(value) <= 10, "case {index}");
    }
}

#[test]
fn varint_and_varlong_reject_malformed_input() {
    // Empty and truncated inputs.
    assert!(read_varint_slice(&[]).is_err());
    assert!(read_varint_slice(&[0x80]).is_err());
    assert!(read_varlong_slice(&[]).is_err());
    assert!(read_varlong_slice(&[0x80]).is_err());

    // Five continuation bytes never terminate a VarInt.
    assert!(read_varint_slice(&[0x80; 5]).is_err());
    assert!(read_varint_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]).is_err());
    // Ten continuation bytes never terminate a VarLong.
    assert!(read_varlong_slice(&[0x80; 10]).is_err());
    assert!(read_varlong_slice(&[0xFF; 10]).is_err());

    let full = write_varint_to_vec(i32::MIN);
    assert_eq!(full.len(), 5);
    for cut in 0..full.len() {
        assert!(
            read_varint_slice(&full[..cut]).is_err(),
            "truncated VarInt at {cut} bytes"
        );
    }

    let full = write_varlong_to_vec(i64::MIN);
    assert_eq!(full.len(), 10);
    for cut in 0..full.len() {
        assert!(
            read_varlong_slice(&full[..cut]).is_err(),
            "truncated VarLong at {cut} bytes"
        );
    }

    // Decoders stop at the terminator and leave trailing bytes alone.
    let mut varint = write_varint_to_vec(300);
    varint.extend_from_slice(&[0x7F, 0x00]);
    assert_eq!(read_varint_slice(&varint).unwrap(), (300, 2));

    let mut varlong = write_varlong_to_vec(i64::MAX);
    assert_eq!(varlong.len(), 9);
    varlong.extend_from_slice(&[0x00, 0x01, 0x02]);
    assert_eq!(read_varlong_slice(&varlong).unwrap(), (i64::MAX, 9));
}

// ---------------------------------------------------------------------------
// Packet framing
// ---------------------------------------------------------------------------

fn patterned_payload(length: usize) -> Vec<u8> {
    let mut block = [0u8; 256];
    let salt = (length as u32).wrapping_mul(2_654_435_761);
    for (index, slot) in block.iter_mut().enumerate() {
        *slot = (index as u32).wrapping_mul(131).wrapping_add(salt) as u8;
    }

    let mut payload = Vec::with_capacity(length);
    while payload.len() < length {
        let take = (length - payload.len()).min(block.len());
        payload.extend_from_slice(&block[..take]);
    }
    payload
}

fn weighted_packet_lengths(rng: &mut Lcg, count: usize) -> Vec<usize> {
    let mut lengths = Vec::with_capacity(count);
    for _ in 0..count {
        let bucket = rng.below(100);
        let length = match bucket {
            0..=69 => rng.below(256),
            70..=89 => 256 + rng.below(65_536 - 256),
            90..=97 => 65_536 + rng.below(512 * 1024),
            _ => rng.below(MAX_PACKET_SIZE + 1),
        };
        lengths.push(length.min(MAX_PACKET_SIZE));
    }
    lengths
}

#[test]
fn packet_framing_round_trips_one_thousand_weighted_payloads() {
    let mut rng = Lcg::new(0x0DDB_A112_0260_0001);
    let mut lengths = weighted_packet_lengths(&mut rng, 1_000);
    // Pin the smallest and largest legal sizes so extremes are always covered.
    lengths[0] = 0;
    lengths[1] = 1;
    lengths[2] = MAX_PACKET_SIZE;

    let small = lengths.iter().filter(|&&length| length < 256).count();
    let large = lengths.iter().filter(|&&length| length >= 65_536).count();
    assert!(small > 500, "not enough small packets: {small}");
    assert!(large > 5, "not enough large packets: {large}");

    for &length in &lengths {
        let payload = patterned_payload(length);
        let mut framed = Vec::with_capacity(length + 5);
        write_packet(&mut framed, &payload).expect("write_packet");
        assert_eq!(
            framed.len(),
            varint_len(length as i32) + length,
            "framed size for {length}"
        );

        let (declared, prefix_len) = read_varint_slice(&framed).expect("read prefix");
        assert_eq!(declared, length as i32, "declared length for {length}");
        assert_eq!(prefix_len, varint_len(length as i32));

        let mut cursor = Cursor::new(framed.as_slice());
        let decoded = read_packet(&mut cursor).expect("read_packet");
        assert_eq!(decoded, payload, "payload round trip for length {length}");
        assert_eq!(
            cursor.position() as usize,
            framed.len(),
            "read_packet left bytes behind for length {length}"
        );
    }
}

#[test]
fn packet_framing_accepts_max_size_and_sequences() {
    let max_payload = patterned_payload(MAX_PACKET_SIZE);
    let mut framed = Vec::with_capacity(MAX_PACKET_SIZE + 5);
    write_packet(&mut framed, &max_payload).expect("write max packet");
    assert_eq!(
        framed.len(),
        varint_len(MAX_PACKET_SIZE as i32) + MAX_PACKET_SIZE
    );
    assert_eq!(
        read_packet(&mut framed.as_slice()).expect("read max packet"),
        max_payload
    );

    let payloads = vec![
        Vec::new(),
        vec![0x00],
        vec![0xFF],
        patterned_payload(127),
        patterned_payload(128),
        patterned_payload(70_000),
    ];
    let mut stream = Vec::new();
    for payload in &payloads {
        write_packet(&mut stream, payload).expect("write sequence packet");
    }

    let mut cursor = Cursor::new(stream.as_slice());
    for payload in &payloads {
        assert_eq!(
            &read_packet(&mut cursor).expect("read sequence packet"),
            payload
        );
    }
    assert_eq!(cursor.position() as usize, stream.len());
    assert!(read_packet(&mut cursor).is_err(), "EOF must be an error");
}

#[test]
fn packet_framing_rejects_oversized_and_truncated_packets() {
    let oversized = patterned_payload(MAX_PACKET_SIZE + 1);
    let mut sink = Vec::new();
    assert!(write_packet(&mut sink, &oversized).is_err());
    assert!(
        sink.is_empty(),
        "failed write must not emit a length prefix"
    );

    let declared = write_varint_to_vec((MAX_PACKET_SIZE + 1) as i32);
    assert!(read_packet(&mut declared.as_slice()).is_err());
    assert!(read_packet(&mut write_varint_to_vec(-1).as_slice()).is_err());

    let mut truncated = write_varint_to_vec(16);
    truncated.extend_from_slice(&[1, 2, 3]);
    assert!(read_packet(&mut truncated.as_slice()).is_err());

    assert!(read_packet(&mut std::io::empty()).is_err());
    let half_prefix = [0x80u8];
    assert!(read_packet(&mut &half_prefix[..]).is_err());
    let overlong = [0x80u8, 0x80, 0x80, 0x80, 0x80, 0x00];
    assert!(read_packet(&mut &overlong[..]).is_err());

    let empty_packet = write_varint_to_vec(0);
    assert_eq!(
        read_packet(&mut empty_packet.as_slice()).expect("empty packet"),
        Vec::<u8>::new()
    );
}

// ---------------------------------------------------------------------------
// NBT
// ---------------------------------------------------------------------------

const NBT_MAX_DEPTH: usize = 6;

fn random_leaf_id(rng: &mut Lcg) -> u8 {
    const LEAVES: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 11, 12];
    LEAVES[rng.below(LEAVES.len())]
}

fn random_i32(rng: &mut Lcg) -> i32 {
    match rng.below(10) {
        0 => i32::MIN,
        1 => i32::MAX,
        2 => 0,
        3 => -1,
        _ => rng.next_u32() as i32,
    }
}

fn random_i64(rng: &mut Lcg) -> i64 {
    match rng.below(10) {
        0 => i64::MIN,
        1 => i64::MAX,
        2 => 0,
        3 => -1,
        _ => rng.next_u64() as i64,
    }
}

fn random_f32(rng: &mut Lcg) -> f32 {
    let bits = rng.next_u32();
    let value = f32::from_bits(bits);
    if value.is_finite() {
        value
    } else {
        // Keep the bit pattern non-NaN/non-infinite; equality survives.
        (bits & 0x7FFF_FFFF) as f32
    }
}

fn random_f64(rng: &mut Lcg) -> f64 {
    let bits = rng.next_u64();
    let value = f64::from_bits(bits);
    if value.is_finite() {
        value
    } else {
        (bits & 0x7FFF_FFFF_FFFF_FFFF) as f64
    }
}

fn random_nbt_string(rng: &mut Lcg) -> String {
    const CHARS: &[char] = &[
        'a', 'b', 'Z', '0', ' ', '_', '-', '.', '\\', '\n', '\t', '§', 'é', '✓', '日', '🦀',
    ];
    let length = rng.below(13);
    (0..length).map(|_| CHARS[rng.below(CHARS.len())]).collect()
}

fn random_tag_of_id(rng: &mut Lcg, id: u8, depth: usize) -> Tag {
    match id {
        1 => Tag::Byte(rng.next_u32() as i8),
        2 => Tag::Short(rng.next_u32() as i16),
        3 => Tag::Int(random_i32(rng)),
        4 => Tag::Long(random_i64(rng)),
        5 => Tag::Float(random_f32(rng)),
        6 => Tag::Double(random_f64(rng)),
        7 => {
            let length = rng.below(17);
            Tag::ByteArray((0..length).map(|_| rng.next_u32() as u8).collect())
        }
        8 => Tag::String(random_nbt_string(rng)),
        9 => random_nbt_list(rng, depth),
        10 => random_nbt_compound(rng, depth),
        11 => {
            let length = rng.below(9);
            Tag::IntArray((0..length).map(|_| random_i32(rng)).collect())
        }
        12 => {
            let length = rng.below(9);
            Tag::LongArray((0..length).map(|_| random_i64(rng)).collect())
        }
        other => panic!("cannot generate NBT payload id {other}"),
    }
}

fn random_tag(rng: &mut Lcg, depth: usize) -> Tag {
    let id = if depth >= NBT_MAX_DEPTH {
        random_leaf_id(rng)
    } else {
        (rng.below(12) + 1) as u8
    };
    random_tag_of_id(rng, id, depth)
}

fn random_nbt_list(rng: &mut Lcg, depth: usize) -> Tag {
    // A container at the maximum depth must be empty: its children would
    // otherwise exceed the requested depth bound.
    if depth >= NBT_MAX_DEPTH {
        return Tag::List(Vec::new());
    }

    let element_id = if depth + 1 >= NBT_MAX_DEPTH {
        random_leaf_id(rng)
    } else {
        (rng.below(12) + 1) as u8
    };
    let length = rng.below(4);
    let mut items = Vec::with_capacity(length);
    for _ in 0..length {
        items.push(random_tag_of_id(rng, element_id, depth + 1));
    }
    Tag::List(items)
}

fn random_nbt_compound(rng: &mut Lcg, depth: usize) -> Tag {
    if depth >= NBT_MAX_DEPTH {
        return Tag::Compound(Vec::new());
    }

    let count = rng.below(5);
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let name = match rng.below(5) {
            0 => String::new(),
            1 => format!("key.{index}"),
            2 => format!("ключ_{index}"),
            _ => format!("key_{index}_{}", rng.next_u32() & 0xFF),
        };
        entries.push((name, random_tag(rng, depth + 1)));
    }
    Tag::Compound(entries)
}

fn kitchen_sink_tree(rng: &mut Lcg) -> Tag {
    let mut entries = Vec::new();
    for id in 1..=12u8 {
        entries.push((format!("type_{id}"), random_tag_of_id(rng, id, 1)));
    }
    entries.push((
        "extra".into(),
        Tag::Compound(vec![
            ("empty_list".into(), Tag::List(Vec::new())),
            ("empty_compound".into(), Tag::Compound(Vec::new())),
            (
                "list_of_lists".into(),
                Tag::List(vec![
                    Tag::List(vec![Tag::Int(1), Tag::Int(-2)]),
                    Tag::List(Vec::new()),
                ]),
            ),
            (
                "list_of_compounds".into(),
                Tag::List(vec![
                    Tag::Compound(vec![("id".into(), Tag::Int(1))]),
                    Tag::Compound(Vec::new()),
                ]),
            ),
        ]),
    ));
    Tag::Compound(entries)
}

#[test]
fn nbt_round_trips_random_nested_trees() {
    let mut rng = Lcg::new(0xA11C_EBEE_F123_4567);
    for case in 0..300 {
        let tag = if case == 0 {
            // Guarantees every NBT tag type appears in at least one tree.
            kitchen_sink_tree(&mut rng)
        } else if case <= 12 {
            // Exercise every possible root tag type.
            random_tag_of_id(&mut rng, case as u8, 0)
        } else {
            random_tag(&mut rng, 0)
        };

        let bytes = write_root_to_vec(&tag).expect("write_root_to_vec");
        let mut direct = Vec::new();
        write_root(&mut direct, &tag).expect("write_root");
        assert_eq!(direct, bytes, "writer paths disagree in case {case}");

        let mut cursor = Cursor::new(bytes.as_slice());
        let decoded = read_root(&mut cursor).expect("read_root");
        assert_eq!(decoded, tag, "NBT round trip in case {case}");
        assert_eq!(
            cursor.position() as usize,
            bytes.len(),
            "trailing bytes in NBT case {case}"
        );
    }
}

#[test]
fn nbt_round_trips_random_trees_through_gzip_and_zlib() {
    let mut rng = Lcg::new(0x0BAD_C0DE_1BAD_F00D);
    for case in 0..100 {
        let tag = random_tag(&mut rng, 0);

        let gzip = write_gzip_root(&tag).expect("write_gzip_root");
        assert!(gzip.len() >= 2, "gzip case {case}");
        assert_eq!(&gzip[..2], &[0x1F, 0x8B], "gzip magic in case {case}");
        assert_eq!(
            read_gzip_root(&gzip).expect("read_gzip_root"),
            tag,
            "gzip round trip in case {case}"
        );

        let zlib = write_zlib_root(&tag).expect("write_zlib_root");
        assert!(zlib.len() >= 2, "zlib case {case}");
        assert_eq!(
            read_zlib_root(&zlib).expect("read_zlib_root"),
            tag,
            "zlib round trip in case {case}"
        );
    }
}

#[test]
fn nbt_named_roots_and_end_roots_round_trip() {
    let tag = Tag::Compound(vec![
        ("answer".into(), Tag::Int(42)),
        ("name".into(), Tag::String("Folia".into())),
    ]);

    let mut bytes = Vec::new();
    write_named(&mut bytes, "level", &tag).expect("write_named");
    assert_eq!(
        read_named_root(&mut bytes.as_slice()).expect("read_named_root"),
        ("level".to_string(), tag.clone())
    );
    assert_eq!(read_root(&mut bytes.as_slice()).unwrap(), tag);

    let end_bytes = write_root_to_vec(&Tag::End).expect("write End root");
    assert_eq!(end_bytes, vec![0]);
    assert_eq!(read_root(&mut end_bytes.as_slice()).unwrap(), Tag::End);

    // A named empty compound and a named empty list exercise the two
    // zero-length container spells.
    for child in [Tag::Compound(Vec::new()), Tag::List(Vec::new())] {
        let tag = Tag::Compound(vec![("child".into(), child)]);
        let bytes = write_root_to_vec(&tag).unwrap();
        assert_eq!(read_root(&mut bytes.as_slice()).unwrap(), tag);
    }
}

#[test]
fn nbt_rejects_malformed_data() {
    let mixed = Tag::List(vec![Tag::Int(1), Tag::String("two".into())]);
    assert!(write_root_to_vec(&mixed).is_err(), "mixed list");

    // Unknown root tag id (0x7F) with an empty name.
    assert!(read_root(&mut &[0x7Fu8, 0x00, 0x00][..]).is_err());

    // Non-empty list whose element type is End.
    let non_empty_end_list = [0x09u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
    assert!(read_root(&mut &non_empty_end_list[..]).is_err());

    // Negative IntArray length.
    let negative_length = [0x0Bu8, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
    assert!(read_root(&mut &negative_length[..]).is_err());

    // Truncated string payload.
    let short_string = [0x08u8, 0x00, 0x00, 0x00, 0x05, b'a'];
    assert!(read_root(&mut &short_string[..]).is_err());

    // Invalid UTF-8 in a string payload.
    let invalid_utf8 = [0x08u8, 0x00, 0x00, 0x00, 0x01, 0xFF];
    assert!(read_root(&mut &invalid_utf8[..]).is_err());

    // NBT strings are limited to 65,535 encoded bytes.
    let huge = Tag::String("x".repeat(u16::MAX as usize + 1));
    assert!(write_root_to_vec(&huge).is_err());

    // A valid encoding truncated anywhere in its final payload must fail.
    let tag = Tag::Compound(vec![("value".into(), Tag::Long(123))]);
    let bytes = write_root_to_vec(&tag).unwrap();
    assert!(read_root(&mut &bytes[..bytes.len() - 1]).is_err());
}

// ---------------------------------------------------------------------------
// Anvil region files
// ---------------------------------------------------------------------------

fn anvil_chunk(local_x: u8, local_z: u8, serial: usize) -> Tag {
    let filler_length = if serial.is_multiple_of(11) {
        12_000
    } else {
        (serial * 53) % 400
    };
    let filler: String = (0..filler_length)
        .map(|index| (((index + serial * 7) % 26) as u8 + b'a') as char)
        .collect();

    Tag::Compound(vec![
        ("DataVersion".into(), Tag::Int(4671)),
        ("xPos".into(), Tag::Int(i32::from(local_x))),
        ("zPos".into(), Tag::Int(i32::from(local_z))),
        ("yPos".into(), Tag::Int(-4)),
        ("Status".into(), Tag::String("minecraft:full".into())),
        ("serial".into(), Tag::Long(serial as i64)),
        ("filler".into(), Tag::String(filler)),
    ])
}

#[test]
fn anvil_round_trips_sixty_four_chunks_with_all_compressions() {
    let temp = TempPath::new("anvil-bulk", "mca");
    let path = temp.path().to_path_buf();

    let compressions = [Compression::Gzip, Compression::Zlib, Compression::None];
    let mut expected = Vec::new();
    {
        let mut region = RegionFile::create(&path).expect("create region");
        assert!(region.is_dirty());

        for serial in 0..64usize {
            let local_x = ((serial % 8) * 4) as u8;
            let local_z = ((serial / 8) * 4) as u8;
            let compression = compressions[serial % compressions.len()];
            let tag = anvil_chunk(local_x, local_z, serial);

            region
                .write_chunk(local_x, local_z, &tag, compression)
                .unwrap_or_else(|error| panic!("write chunk {serial}: {error}"));

            let location = region.location(local_x, local_z);
            assert!(!location.is_empty(), "location {serial}");
            assert!(location.offset as usize >= HEADER_SECTORS);
            assert!(location.sectors > 0);
            expected.push((
                local_x,
                local_z,
                tag,
                compression,
                location,
                region.timestamp(local_x, local_z),
            ));
        }

        assert!(region.location(1, 1).is_empty());
        assert!(region.save_if_dirty().expect("save_if_dirty"));
        assert!(!region.is_dirty());
        assert!(!region.save_if_dirty().unwrap());
    }

    let reopened = RegionFile::open(&path).expect("reopen region");
    assert!(!reopened.is_dirty());
    assert!(reopened.location(1, 1).is_empty());
    assert!(reopened.read_chunk(1, 1).unwrap().is_none());

    let file_len = fs::metadata(&path).expect("region metadata").len() as usize;
    assert_eq!(file_len % SECTOR_SIZE, 0, "region file sector alignment");

    for (local_x, local_z, tag, _compression, location, timestamp) in &expected {
        assert_eq!(
            reopened.location(*local_x, *local_z),
            *location,
            "location for ({local_x}, {local_z})"
        );
        assert_eq!(
            reopened.timestamp(*local_x, *local_z),
            *timestamp,
            "timestamp for ({local_x}, {local_z})"
        );

        let read = reopened
            .read_chunk(*local_x, *local_z)
            .unwrap_or_else(|error| panic!("read ({local_x}, {local_z}): {error}"))
            .unwrap_or_else(|| panic!("missing chunk ({local_x}, {local_z})"));
        assert_eq!(&read, tag, "chunk payload ({local_x}, {local_z})");

        let end = (location.offset as usize + location.sectors as usize) * SECTOR_SIZE;
        assert!(end <= file_len, "sector range for ({local_x}, {local_z})");
    }

    drop(reopened);
    drop(temp);
    assert!(!path.exists(), "region file was not cleaned up");
}

#[test]
fn anvil_remove_chunk_rewrite_and_reopen() {
    let temp = TempPath::new("anvil-remove", "mca");
    let path = temp.path().to_path_buf();

    let compressions = [Compression::Gzip, Compression::Zlib, Compression::None];
    let original: Vec<(u8, u8, Tag)> = (0..8usize)
        .map(|index| {
            let x = (index * 2) as u8;
            let z = (index * 2) as u8;
            (x, z, anvil_chunk(x, z, index))
        })
        .collect();

    {
        let mut region = RegionFile::create(&path).expect("create region");
        for (index, (x, z, tag)) in original.iter().enumerate() {
            region
                .write_chunk(*x, *z, tag, compressions[index % compressions.len()])
                .unwrap();
        }
        assert!(region.save_if_dirty().expect("save region"));
    }

    let replacement = anvil_chunk(0, 0, 999);
    {
        let mut region = RegionFile::open(&path).expect("reopen for rewrite");
        region
            .write_chunk(0, 0, &replacement, Compression::Gzip)
            .expect("rewrite chunk");
        assert!(region.is_dirty());
        for index in [2usize, 4, 6] {
            let (x, z, _) = &original[index];
            region.remove_chunk(*x, *z);
        }
        assert!(region.save_if_dirty().unwrap());
        assert!(!region.is_dirty());
    }

    {
        let region = RegionFile::open(&path).expect("reopen after removal");
        assert_eq!(region.read_chunk(0, 0).unwrap().unwrap(), replacement);

        for index in [2usize, 4, 6] {
            let (x, z, _) = &original[index];
            assert!(region.location(*x, *z).is_empty(), "location {index}");
            assert_eq!(region.timestamp(*x, *z), 0, "timestamp {index}");
            assert!(region.read_chunk(*x, *z).unwrap().is_none());
        }

        for index in [1usize, 3, 5, 7] {
            let (x, z, tag) = &original[index];
            let read = region.read_chunk(*x, *z).unwrap().unwrap();
            assert_eq!(&read, tag, "untouched chunk {index}");
        }
    }

    let re_added = anvil_chunk(4, 4, 1_234);
    {
        let mut region = RegionFile::open(&path).expect("reopen for re-add");
        region
            .write_chunk(4, 4, &re_added, Compression::None)
            .expect("re-add chunk");
        assert!(region.save_if_dirty().unwrap());
    }
    {
        let region = RegionFile::open(&path).expect("reopen after re-add");
        assert_eq!(region.read_chunk(4, 4).unwrap().unwrap(), re_added);
        assert!(region.read_chunk(0, 0).unwrap().is_some());
    }

    drop(temp);
    assert!(!path.exists(), "region file was not cleaned up");
}

#[test]
fn anvil_rejects_invalid_chunks_and_corrupt_records() {
    let temp = TempPath::new("anvil-invalid", "mca");
    let path = temp.path().to_path_buf();

    let location = {
        let mut region = RegionFile::create(&path).expect("create region");

        assert!(
            region
                .write_chunk(
                    0,
                    0,
                    &Tag::String("not a compound".into()),
                    Compression::None
                )
                .is_err(),
            "non-compound chunk root must be rejected"
        );

        let oversized = Tag::Compound(vec![(
            "bytes".into(),
            Tag::ByteArray(vec![0x5A; MAX_CHUNK_LENGTH + 1]),
        )]);
        assert!(
            region
                .write_chunk(0, 0, &oversized, Compression::None)
                .is_err(),
            "uncompressed chunk above 1 MiB must be rejected"
        );
        assert!(region.location(0, 0).is_empty());

        region
            .write_chunk(1, 1, &anvil_chunk(1, 1, 7), Compression::Zlib)
            .unwrap();
        assert!(region.save_if_dirty().unwrap());
        region.location(1, 1)
    };

    // Corrupt the compression id to an unsupported value.
    let mut bytes = fs::read(&path).expect("read region");
    let compression_offset = location.offset as usize * SECTOR_SIZE + 4;
    bytes[compression_offset] = 9;
    fs::write(&path, &bytes).expect("write corrupt compression");
    let reopened = RegionFile::open(&path).expect("reopen corrupt compression");
    assert!(
        reopened.read_chunk(1, 1).is_err(),
        "unsupported compression id must be rejected"
    );
    drop(reopened);

    // Make the record length claim more bytes than the file holds.
    let mut bytes = fs::read(&path).expect("read region");
    let start = location.offset as usize * SECTOR_SIZE;
    bytes[start..start + 4].copy_from_slice(&(MAX_CHUNK_LENGTH as u32).to_be_bytes());
    fs::write(&path, &bytes).expect("write truncated record");
    let reopened = RegionFile::open(&path).expect("reopen truncated record");
    assert!(
        reopened.read_chunk(1, 1).is_err(),
        "truncated chunk record must be rejected"
    );
    drop(reopened);

    drop(temp);
    assert!(!path.exists(), "region file was not cleaned up");
}

// ---------------------------------------------------------------------------
// Chunks and paletted containers
// ---------------------------------------------------------------------------

fn chunk_biomes(rng: &mut Lcg, case: usize, section_index: usize) -> PalettedContainer<String> {
    let palette_len = 1 + rng.below(8);
    let palette: Vec<String> = (0..palette_len)
        .map(|entry| format!("minecraft:bulk_biome_{case}_{section_index}_{entry}"))
        .collect();
    let mut indices: Vec<u16> = (0..64).map(|_| rng.below(palette_len) as u16).collect();
    for (entry, index) in indices.iter_mut().enumerate().take(palette_len) {
        *index = entry as u16;
    }
    PalettedContainer { palette, indices }
}

#[test]
fn chunk_round_trips_fifty_random_palettes_with_biomes() {
    let mut rng = Lcg::new(0xC4A0_6D17_2026_0001);

    for case in 0..50usize {
        let section_count = 1 + case % 3;
        let section_ys = [-3i8, 0, 2];

        let (x, z) = match case {
            0 => (i32::MIN, i32::MAX),
            1 => (-1, -1),
            _ => (case as i32 * 37 - 900, 400 - case as i32 * 53),
        };

        let mut sections = Vec::with_capacity(section_count);
        let mut expected_sections = Vec::with_capacity(section_count);

        for (section_index, &y) in section_ys.iter().enumerate().take(section_count) {
            let palette_len = 1 + (case * 7 + section_index * 5) % 32;
            let mut palette = Vec::with_capacity(palette_len);
            for entry in 0..palette_len {
                let mut properties = Vec::new();
                if (entry + case + section_index) % 3 == 0 {
                    let facing = ["north", "east", "south", "west"][(entry + case) % 4];
                    properties.push(("facing".to_string(), facing.to_string()));
                }
                if (entry + case) % 5 == 0 {
                    properties.push((
                        "waterlogged".to_string(),
                        if entry % 2 == 0 { "false" } else { "true" }.to_string(),
                    ));
                }
                palette.push(BlockState {
                    name: format!("minecraft:bulk_{case}_{section_index}_{entry}"),
                    properties,
                });
            }

            let mut indices: Vec<u16> = (0..4096).map(|_| rng.below(palette_len) as u16).collect();
            // Force every palette entry to be referenced, so canonicalization
            // must keep all of them and high palette indices are exercised.
            for (entry, index) in indices.iter_mut().enumerate().take(palette_len) {
                *index = entry as u16;
            }

            let biomes = if (case + section_index) % 2 == 0 {
                Some(chunk_biomes(&mut rng, case, section_index))
            } else {
                None
            };

            sections.push(ChunkSection {
                y,
                block_states: PalettedContainer {
                    palette: palette.clone(),
                    indices: indices.clone(),
                },
                biomes: biomes.clone(),
                light: folia_core::light::SectionLight::default(),
            });
            expected_sections.push((y, palette, indices, biomes));
        }

        let chunk = Chunk { x, z, sections };
        let encoded = encode_chunk(&chunk).expect("encode random chunk");
        let decoded = decode_chunk(&encoded).expect("decode random chunk");

        assert_eq!(decoded.x, x, "x in case {case}");
        assert_eq!(decoded.z, z, "z in case {case}");
        assert_eq!(
            decoded.sections.len(),
            section_count,
            "section count {case}"
        );

        for (y, palette, indices, biomes) in &expected_sections {
            let section = decoded
                .section(*y)
                .unwrap_or_else(|| panic!("missing section {y} in case {case}"));
            assert_eq!(section.block_states.indices.len(), 4096);
            assert!(
                (1..=32).contains(&section.block_states.palette.len()),
                "decoded palette size in case {case}"
            );

            for index in 0..4096 {
                let local_y = index / 256;
                let local_z = (index / 16) % 16;
                let local_x = index % 16;
                let world_y = i32::from(*y) * 16 + local_y as i32;
                let actual = decoded
                    .block_state(local_x, world_y, local_z)
                    .unwrap_or_else(|| {
                        panic!("missing block at ({local_x}, {world_y}, {local_z}) in case {case}")
                    });
                let expected = &palette[indices[index] as usize];
                assert_eq!(
                    actual, expected,
                    "block index {index} in section {y} of case {case}"
                );
            }

            match biomes {
                Some(expected_biomes) => {
                    let container = section
                        .biomes
                        .as_ref()
                        .unwrap_or_else(|| panic!("missing biomes in case {case}"));
                    assert_eq!(container.indices.len(), 64);
                    assert!((1..=8).contains(&container.palette.len()));
                    for index in 0..64 {
                        let expected =
                            &expected_biomes.palette[expected_biomes.indices[index] as usize];
                        assert_eq!(
                            container.get(index),
                            Some(expected),
                            "biome index {index} in section {y} of case {case}"
                        );
                    }
                }
                None => assert!(section.biomes.is_none(), "unexpected biomes in case {case}"),
            }
        }

        // Canonicalization is idempotent: a second encode/decode is stable.
        let stable = decode_chunk(&encode_chunk(&decoded).unwrap()).unwrap();
        assert_eq!(stable, decoded, "chunk re-encode stability in case {case}");
    }
}

#[test]
fn chunk_encoder_rejects_invalid_sections() {
    let air = || BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    };
    let stone = || BlockState {
        name: "minecraft:stone".into(),
        properties: Vec::new(),
    };

    let mut chunk = Chunk {
        x: 0,
        z: 0,
        sections: vec![ChunkSection {
            y: 0,
            block_states: PalettedContainer {
                palette: vec![air()],
                indices: vec![0; 4095],
            },
            biomes: None,
            light: folia_core::light::SectionLight::default(),
        }],
    };
    assert!(encode_chunk(&chunk).is_err(), "4095 block states");

    chunk.sections[0].block_states = PalettedContainer {
        palette: vec![air(), stone()],
        indices: vec![2; 4096],
    };
    assert!(encode_chunk(&chunk).is_err(), "out-of-range block index");

    chunk.sections[0].block_states = PalettedContainer {
        palette: Vec::new(),
        indices: vec![0; 4096],
    };
    assert!(encode_chunk(&chunk).is_err(), "empty block palette");

    chunk.sections[0].block_states = PalettedContainer {
        palette: vec![air()],
        indices: vec![0; 4096],
    };
    chunk.sections[0].biomes = Some(PalettedContainer {
        palette: vec!["minecraft:plains".into()],
        indices: vec![0; 63],
    });
    assert!(encode_chunk(&chunk).is_err(), "63 biome entries");

    chunk.sections[0].biomes = Some(PalettedContainer {
        palette: vec!["minecraft:plains".into()],
        indices: vec![1; 64],
    });
    assert!(encode_chunk(&chunk).is_err(), "out-of-range biome index");

    // Unused palette entries are dropped and the surviving state is correct.
    chunk.sections[0].biomes = None;
    chunk.sections[0].block_states = PalettedContainer {
        palette: vec![air(), stone()],
        indices: vec![0; 4096],
    };
    let decoded = decode_chunk(&encode_chunk(&chunk).unwrap()).unwrap();
    assert_eq!(decoded.sections[0].block_states.palette.len(), 1);
    assert_eq!(decoded.sections[0].block_states.palette[0], air());

    let empty = Chunk {
        x: i32::MAX,
        z: i32::MIN,
        sections: Vec::new(),
    };
    assert_eq!(decode_chunk(&encode_chunk(&empty).unwrap()).unwrap(), empty);
}

#[test]
fn biome_lookup_maps_4x4x4_coordinates() {
    let mut indices = vec![0u16; 64];
    // 4x4x4 biome cell (x=15, y=0, z=15) maps to flat index 15.
    indices[15] = 1;

    let chunk = Chunk {
        x: 0,
        z: 0,
        sections: vec![ChunkSection {
            y: 0,
            block_states: PalettedContainer {
                palette: vec![BlockState {
                    name: "minecraft:air".into(),
                    properties: Vec::new(),
                }],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into(), "minecraft:desert".into()],
                indices,
            }),
            light: folia_core::light::SectionLight::default(),
        }],
    };

    let decoded = decode_chunk(&encode_chunk(&chunk).unwrap()).unwrap();
    let expected = "minecraft:desert".to_string();
    assert_eq!(decoded.biome(15, 0, 15), Some(&expected));
}

// ---------------------------------------------------------------------------
// Region scheduler
// ---------------------------------------------------------------------------

fn scheduler_chunk(index: usize) -> ChunkPos {
    ChunkPos::new(
        ((index % 50) as i32 - 25) * 8 + (index % 8) as i32,
        (((index / 50) % 40) as i32 - 20) * 8 + ((index / 8) % 8) as i32,
    )
}

/// Mirrors `tick_region`'s countdown semantics: the first callback runs on
/// tick `delay` (zero based), and a non-zero period adds `period` ticks
/// between subsequent callbacks.
fn runs_on_tick(delay: u32, period: u32, tick: u32) -> bool {
    if tick < delay {
        false
    } else if period == 0 {
        tick == delay
    } else {
        (tick - delay).is_multiple_of(period)
    }
}

fn expected_runs(delay: u32, period: u32, ticks: u32) -> usize {
    if delay >= ticks {
        0
    } else {
        1 + (ticks - 1 - delay).checked_div(period).unwrap_or(0) as usize
    }
}

#[test]
fn scheduler_delay_period_and_cancellation_counts() {
    const TASKS: usize = 1_000;
    const TICKS: u32 = 12;

    let mut scheduler = RegionScheduler::new(8);
    let counts = Arc::new(Mutex::new(vec![0usize; TASKS + 1]));
    let mut delays = vec![0u32; TASKS + 1];
    let mut periods = vec![0u32; TASKS + 1];
    let mut cancelled = vec![false; TASKS + 1];
    let mut expected_regions = BTreeSet::new();

    for index in 0..TASKS {
        let id = (index + 1) as u64;
        let chunk = scheduler_chunk(index);
        let delay = (index % 5) as u32;
        let period = if index % 4 == 0 {
            ((index % 7) + 1) as u32
        } else {
            0
        };
        delays[id as usize] = delay;
        periods[id as usize] = period;
        expected_regions.insert(chunk.region_key(8));

        let callback_counts = Arc::clone(&counts);
        let callback_id = id;
        let scheduled = scheduler.schedule_at(chunk, delay, period, move || {
            callback_counts.lock().unwrap()[callback_id as usize] += 1;
        });
        assert_eq!(scheduled, id, "task ids must be sequential");

        if index % 17 == 0 {
            assert!(scheduler.cancel(id), "cancel scheduled task {id}");
            cancelled[id as usize] = true;
        }
    }

    assert_eq!(scheduler.task_count(), TASKS);
    assert_eq!(scheduler.region_count(), expected_regions.len());
    assert!(scheduler.region_count() > 1);
    assert!(!scheduler.cancel(999_999), "unknown task id");

    for tick in 0..TICKS {
        let expected_tick = (1..=TASKS)
            .filter(|id| !cancelled[*id] && runs_on_tick(delays[*id], periods[*id], tick))
            .count();
        let executed = scheduler.tick();
        assert_eq!(executed, expected_tick, "tick {tick} execution count");
        assert_eq!(scheduler.global_ticks(), u64::from(tick + 1));
    }

    {
        let counts = counts.lock().unwrap();
        for id in 1..=TASKS {
            let expected = if cancelled[id] {
                0
            } else {
                expected_runs(delays[id], periods[id], TICKS)
            };
            assert_eq!(
                counts[id], expected,
                "task {id} with delay {} and period {}",
                delays[id], periods[id]
            );
        }
    }

    let active_periodic = (1..=TASKS)
        .filter(|id| !cancelled[*id] && periods[*id] > 0)
        .count();
    assert_eq!(
        scheduler.task_count(),
        active_periodic,
        "only periodic tasks should survive all ticks"
    );

    let sample_key = scheduler_chunk(0).region_key(8);
    assert_eq!(scheduler.region_ticks(sample_key), Some(u64::from(TICKS)));

    for id in 1..=TASKS {
        if !cancelled[id] && periods[id] > 0 {
            assert!(scheduler.cancel(id as u64));
        }
    }
    assert_eq!(scheduler.tick(), 0);
    assert_eq!(scheduler.task_count(), 0);
    assert_eq!(
        scheduler.region_ticks(sample_key),
        Some(u64::from(TICKS + 1))
    );
}

#[test]
fn scheduler_parallel_executes_every_region_task_exactly_once() {
    const REGIONS_X: usize = 25;
    const REGIONS_Z: usize = 10;
    const TASKS_PER_REGION: usize = 4;
    const TASKS: usize = REGIONS_X * REGIONS_Z * TASKS_PER_REGION;

    let mut scheduler = RegionScheduler::new(8);
    let executed_tokens = Arc::new(Mutex::new(Vec::new()));

    for region_index in 0..REGIONS_X * REGIONS_Z {
        let region_x = (region_index % REGIONS_X) as i32;
        let region_z = (region_index / REGIONS_X) as i32;
        for task_index in 0..TASKS_PER_REGION {
            let chunk = ChunkPos::new(region_x * 8 + task_index as i32, region_z * 8 + 3);
            let token = region_index * TASKS_PER_REGION + task_index;
            let callback_tokens = Arc::clone(&executed_tokens);
            scheduler.schedule_at(chunk, 0, 0, move || {
                callback_tokens.lock().unwrap().push(token);
            });
        }
    }

    let global_count = Arc::new(AtomicUsize::new(0));
    {
        let callback_count = Arc::clone(&global_count);
        scheduler.schedule_global(0, 0, move || {
            callback_count.fetch_add(1, Ordering::Relaxed);
        });
    }

    assert_eq!(scheduler.region_count(), REGIONS_X * REGIONS_Z);
    assert_eq!(scheduler.task_count(), TASKS + 1);

    let executed = scheduler.tick_parallel(4);
    assert_eq!(executed, TASKS + 1);
    assert_eq!(global_count.load(Ordering::Relaxed), 1);
    assert_eq!(scheduler.global_ticks(), 1);
    assert_eq!(scheduler.task_count(), 0);
    assert_eq!(scheduler.region_count(), REGIONS_X * REGIONS_Z);
    assert_eq!(scheduler.region_ticks(RegionKey { x: 0, z: 0 }), Some(1));
    assert_eq!(
        scheduler.region_ticks(RegionKey {
            x: (REGIONS_X - 1) as i32,
            z: (REGIONS_Z - 1) as i32,
        }),
        Some(1)
    );

    {
        let mut tokens = executed_tokens.lock().unwrap().clone();
        tokens.sort_unstable();
        let expected: Vec<usize> = (0..TASKS).collect();
        assert_eq!(
            tokens, expected,
            "every region task must execute exactly once"
        );
    }

    // A second pass has no work left, but still advances every region clock.
    assert_eq!(scheduler.tick_parallel(4), 0);
    assert_eq!(scheduler.global_ticks(), 2);
    assert_eq!(scheduler.region_ticks(RegionKey { x: 0, z: 0 }), Some(2));
}

#[test]
fn scheduler_region_cancellation_and_tick_counters() {
    let mut scheduler = RegionScheduler::new(4);
    let left = Arc::new(AtomicUsize::new(0));
    let right = Arc::new(AtomicUsize::new(0));

    for index in 0..10 {
        let callback_count = Arc::clone(&left);
        scheduler.schedule_at(ChunkPos::new(index % 4, index % 4), 0, 1, move || {
            callback_count.fetch_add(1, Ordering::Relaxed);
        });
    }
    for index in 0..5 {
        let callback_count = Arc::clone(&right);
        scheduler.schedule_at(ChunkPos::new(8 + (index % 4), 8), 1, 0, move || {
            callback_count.fetch_add(1, Ordering::Relaxed);
        });
    }

    let left_key = ChunkPos::new(0, 0).region_key(4);
    let right_key = ChunkPos::new(8, 8).region_key(4);
    assert_eq!(scheduler.region_ticks(left_key), Some(0));

    scheduler.cancel_region(left_key);
    assert_eq!(scheduler.tick_region(left_key), 0);
    assert_eq!(left.load(Ordering::Relaxed), 0);
    assert_eq!(scheduler.region_ticks(left_key), Some(1));

    assert_eq!(scheduler.tick_region(right_key), 0, "one tick of delay");
    assert_eq!(right.load(Ordering::Relaxed), 0);
    assert_eq!(scheduler.tick_region(right_key), 5);
    assert_eq!(right.load(Ordering::Relaxed), 5);
    assert_eq!(scheduler.tick_region(right_key), 0, "one-shots are removed");

    assert_eq!(scheduler.task_count(), 0);
    assert_eq!(scheduler.tick(), 0);
    assert_eq!(scheduler.region_ticks(left_key), Some(2));
    assert_eq!(scheduler.region_ticks(right_key), Some(4));
}

// ---------------------------------------------------------------------------
// server.properties
// ---------------------------------------------------------------------------

#[test]
fn config_parses_generated_properties_with_comments_and_escapes() {
    let input = r#"
# generated server.properties
! alternate comment marker
   # indented comment

server-ip=192.0.2.10
server-port:25580
motd=First line\nSecond \t tabbed \\ slash \u00a7aGreen
max-players 42
online-mode=FALSE
level-name=bulk world
view-distance=16
simulation-distance:6
enable-command-block=TRUE
allow-flight=false
hardcore=true
pvp=false
white-list=TRUE
difficulty=hard
gamemode=creative
spawn-protection=0
unknown-key=kept as is
custom\:key=escaped separator
space\ key=escaped space key
unicode-key=value \u2713
continued=alpha \
    beta
solo-key
"#;

    let properties = ServerProperties::parse(input);

    assert_eq!(properties.server_ip, "192.0.2.10");
    assert_eq!(properties.server_port, 25_580);
    assert_eq!(
        properties.motd,
        "First line\nSecond \t tabbed \\ slash §aGreen"
    );
    assert_eq!(properties.max_players, 42);
    assert!(!properties.online_mode);
    assert_eq!(properties.level_name, "bulk world");
    assert_eq!(properties.view_distance, 16);
    assert_eq!(properties.simulation_distance, 6);
    assert!(properties.enable_command_block);
    assert!(!properties.allow_flight);
    assert!(properties.hardcore);
    assert!(!properties.pvp);
    assert!(properties.white_list);
    assert_eq!(properties.difficulty, "hard");
    assert_eq!(properties.gamemode, "creative");
    assert_eq!(properties.spawn_protection, 0);

    assert_eq!(properties.extra.len(), 6, "extra: {:?}", properties.extra);
    assert_eq!(
        properties.extra.get("unknown-key").map(String::as_str),
        Some("kept as is")
    );
    assert_eq!(
        properties.extra.get("custom:key").map(String::as_str),
        Some("escaped separator")
    );
    assert_eq!(
        properties.extra.get("space key").map(String::as_str),
        Some("escaped space key")
    );
    assert_eq!(
        properties.extra.get("unicode-key").map(String::as_str),
        Some("value ✓")
    );
    assert_eq!(
        properties.extra.get("continued").map(String::as_str),
        Some("alpha beta")
    );
    assert_eq!(
        properties.extra.get("solo-key").map(String::as_str),
        Some("")
    );
}

#[test]
fn config_reads_generated_file_and_applies_defaults() {
    let input = "# generated\n\
server-port=not-a-number\n\
max-players=?\n\
view-distance=nope\n\
online-mode=TRUE\n\
unknown=first\n\
unknown=second\n\
motd=line one \\\n    line two \\\n    line three\n";

    let properties = ServerProperties::parse(input);
    assert_eq!(properties.server_port, 25_565);
    assert_eq!(properties.max_players, 20);
    assert_eq!(properties.view_distance, 10);
    assert_eq!(properties.simulation_distance, 10);
    assert!(properties.online_mode);
    assert_eq!(properties.motd, "line one line two line three");
    assert_eq!(properties.extra.len(), 1);
    assert_eq!(
        properties.extra.get("unknown").map(String::as_str),
        Some("second"),
        "later duplicate keys win"
    );

    let temp = TempPath::new("config", "properties");
    let path = temp.path().to_path_buf();
    fs::write(&path, input).expect("write generated properties");
    let from_disk = ServerProperties::read(&path).expect("read generated properties");
    assert_eq!(from_disk, properties);
    assert!(path.exists());

    let missing = unique_temp_path("config-missing", "properties");
    assert!(ServerProperties::read(&missing).is_err());
    assert!(!missing.exists());

    drop(temp);
    assert!(!path.exists(), "properties file was not cleaned up");
}
