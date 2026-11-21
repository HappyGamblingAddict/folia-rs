//! Deterministic robustness / property tests for untrusted network input (P8.9).
//!
//! Every case is seeded, bounded and runs in-process so the suite is stable in
//! CI: no external fuzz corpus, no new dependencies, no hangs. Parsers are
//! allowed to reject input with an `io::Error`; the invariant under test is
//! that they never panic and never read outside the framed payload.

use folia_core::compression::{self, DEFAULT_MAX_DECOMPRESSED};
use folia_core::nbt::{self, Tag};
use folia_core::protocol::{
    self, PacketReader, PacketWriter, parse_handshake, parse_login_start, parse_serverbound_chat,
    parse_serverbound_chat_command,
};
use folia_core::varint::{varint_len, write_varint_to_vec};
use folia_random::{LegacyRandomSource, RandomSource};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Fixed seed: these are property tests, so a failure is always reproducible.
const SEED: i64 = 0x5048_3900_4788_1A21;
const VARINT_BOUNDARIES: [i32; 7] = [i32::MIN, -1, 0, 1, 1 << 21, 1 << 28, i32::MAX];

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

/// Random buffer whose length is in `0..max_len`.
fn random_buffer(random: &mut LegacyRandomSource, max_len: usize) -> Vec<u8> {
    let length = random.next_int_bound(max_len as i32) as usize;
    random_bytes(random, length)
}

fn assert_varint_round_trip(value: i32) {
    let bytes = write_varint_to_vec(value);
    assert_eq!(bytes.len(), varint_len(value), "length for {value}");
    let mut reader = PacketReader::new(&bytes);
    assert_eq!(reader.read_varint().unwrap(), value, "decode {value}");
    assert!(reader.is_empty(), "trailing bytes for {value}");
}

// 1. VarInt boundaries plus 500 seeded random i32 values.
#[test]
fn varint_round_trips_boundaries_and_seeded_values() {
    let mut writer = PacketWriter::new();
    for &value in &VARINT_BOUNDARIES {
        assert_varint_round_trip(value);
        writer.write_varint(value).unwrap();
    }
    let mut random = seeded_random();
    for _ in 0..500 {
        assert_varint_round_trip(random.next_int());
    }

    let encoded = writer.into_inner();
    let mut reader = PacketReader::new(&encoded);
    for &value in &VARINT_BOUNDARIES {
        assert_eq!(reader.read_varint().unwrap(), value);
    }
    assert!(reader.is_empty());
}

// 2. Malformed VarInts, including truncation and >5-byte sequences.
#[test]
fn malformed_varints_never_panic() {
    let explicit: [&[u8]; 6] = [
        &[],
        &[0x80],
        &[0x80, 0x80, 0x80, 0x80],
        &[0x80; 6],
        &[0xFF; 16],
        &[0x80, 0x80, 0x80, 0x80, 0x10],
    ];
    for bytes in explicit {
        let _: io::Result<i32> = assert_no_panic("read_varint (explicit)", || {
            PacketReader::new(bytes).read_varint()
        });
    }

    let mut random = seeded_random();
    for _ in 0..500 {
        let length = 1 + random.next_int_bound(16) as usize;
        let bytes = random_bytes(&mut random, length);
        let _: io::Result<i32> = assert_no_panic("read_varint (random)", || {
            PacketReader::new(&bytes).read_varint()
        });
    }

    // Regression guard: an overlong VarInt is rejected, not silently accepted.
    assert!(PacketReader::new(&[0x80; 6]).read_varint().is_err());
}

// 3. Random packet bodies passed to the common serverbound parsers.
#[test]
fn random_packet_buffers_never_panic() {
    let mut random = seeded_random();
    for _ in 0..500 {
        let bytes = random_buffer(&mut random, 512);
        let _: io::Result<i32> =
            assert_no_panic("packet id", || PacketReader::new(&bytes).read_varint());
        let _ = assert_no_panic("parse_handshake", || parse_handshake(&bytes));
        let _ = assert_no_panic("parse_login_start", || parse_login_start(&bytes));
        let _ = assert_no_panic("parse_serverbound_chat", || parse_serverbound_chat(&bytes));
        let _ = assert_no_panic("parse_serverbound_chat_command", || {
            parse_serverbound_chat_command(&bytes)
        });
    }

    // Bounds are enforced explicitly, not only "did not panic":
    // an absurd string length and an over-long player name must be rejected.
    let mut writer = PacketWriter::new();
    writer.write_varint(0).unwrap();
    writer.write_varint(i32::MAX).unwrap();
    assert!(parse_handshake(&writer.into_inner()).is_err());

    let too_long_name = "a".repeat(17);
    assert!(
        parse_login_start(&protocol::build_login_start(&too_long_name, None).unwrap()).is_err()
    );
}

// 3b. Framing with hostile length prefixes and small random frames.
#[test]
fn framed_read_rejects_hostile_lengths_without_panicking() {
    let hostile: [&[u8]; 5] = [
        &[0xFF, 0xFF, 0xFF, 0xFF, 0x07], // i32::MAX
        &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F], // would decode negative
        &[0x80; 6],                      // overlong
        &[0x80],                         // truncated
        &[0x00],                         // valid empty payload
    ];
    for bytes in hostile {
        let result: io::Result<Vec<u8>> = assert_no_panic("read_packet (hostile)", || {
            let mut cursor = bytes;
            protocol::read_packet(&mut cursor)
        });
        if bytes.len() > 1 && bytes[0] != 0x00 {
            assert!(result.is_err(), "hostile length {bytes:?} was accepted");
        }
    }

    let mut random = seeded_random();
    for _ in 0..200 {
        let mut bytes = random_buffer(&mut random, 32);
        if let Some(first) = bytes.first_mut() {
            // Keep the declared frame length small so the test never asks the
            // framing layer for a multi-megabyte allocation.
            *first &= 0x1F;
        }
        let _: io::Result<Vec<u8>> = assert_no_panic("read_packet (random)", || {
            let mut cursor = bytes.as_slice();
            protocol::read_packet(&mut cursor)
        });
    }
}

// 4. ASCII/UTF-8 string round-trips plus truncated string buffers.
#[test]
fn strings_round_trip_and_truncation_never_panics() {
    const ALPHABET: &str = "abcXYZ019_-. \n\t\0é中🙂û";
    let choices: Vec<char> = ALPHABET.chars().collect();
    let mut cases: Vec<String> = vec![
        String::new(),
        "a".into(),
        "é中🙂".into(),
        "\0\n\t".into(),
        "x".repeat(255),
    ];

    let mut random = seeded_random();
    for _ in 0..200 {
        let length = random.next_int_bound(64) as usize;
        let mut text = String::new();
        for _ in 0..length {
            let index = random.next_int_bound(choices.len() as i32) as usize;
            text.push(choices[index]);
        }
        cases.push(text);
    }

    for text in &cases {
        let mut writer = PacketWriter::new();
        writer.write_string(text).unwrap();
        let bytes = writer.into_inner();
        let decoded =
            assert_no_panic("read_string", || PacketReader::new(&bytes).read_string()).unwrap();
        assert_eq!(&decoded, text);
    }

    let mut writer = PacketWriter::new();
    writer.write_string("truncated é中🙂 string").unwrap();
    let encoded = writer.into_inner();
    for end in 0..encoded.len() {
        let prefix = &encoded[..end];
        let _: io::Result<String> = assert_no_panic("read_string (truncated)", || {
            PacketReader::new(prefix).read_string()
        });
    }
}

// 5. Compression round-trips and garbage decompression.
#[test]
fn compression_round_trips_random_payloads() {
    let mut random = seeded_random();
    for _ in 0..100 {
        let length = random.next_int_bound(4096) as usize;
        let data = random_bytes(&mut random, length);
        let compressed = assert_no_panic("compress", || compression::compress(&data)).unwrap();
        let decoded = assert_no_panic("decompress", || {
            compression::decompress(&compressed, DEFAULT_MAX_DECOMPRESSED)
        })
        .unwrap();
        assert_eq!(decoded, data);
    }
}

#[test]
fn decompress_never_panics_on_garbage() {
    let mut random = seeded_random();
    for _ in 0..100 {
        let bytes = random_buffer(&mut random, 256);
        let _: io::Result<Vec<u8>> = assert_no_panic("decompress (garbage)", || {
            compression::decompress(&bytes, DEFAULT_MAX_DECOMPRESSED)
        });
    }
}

fn nested_tag(depth: usize) -> Tag {
    let mut tag = Tag::Compound(vec![("value".into(), Tag::Int(42))]);
    for level in 0..depth {
        if level % 2 == 0 {
            tag = Tag::Compound(vec![("child".into(), tag)]);
        } else {
            tag = Tag::List(vec![tag]);
        }
    }
    tag
}

// 6. NBT nesting depth 64 survives every codec in the crate.
#[test]
fn nbt_depth_64_round_trips() {
    let tag = nested_tag(64);

    let bytes = nbt::write_root_to_vec(&tag).unwrap();
    let decoded = assert_no_panic("read_root", || nbt::read_root(&mut bytes.as_slice())).unwrap();
    assert_eq!(decoded, tag);

    let mut any = Vec::new();
    nbt::write_any_tag(&mut any, &tag).unwrap();
    let decoded =
        assert_no_panic("read_any_tag", || nbt::read_any_tag(&mut any.as_slice())).unwrap();
    assert_eq!(decoded, tag);

    let gzip = nbt::write_gzip_root(&tag).unwrap();
    let decoded = assert_no_panic("read_gzip_root", || nbt::read_gzip_root(&gzip)).unwrap();
    assert_eq!(decoded, tag);

    let zlib = nbt::write_zlib_root(&tag).unwrap();
    let decoded = assert_no_panic("read_zlib_root", || nbt::read_zlib_root(&zlib)).unwrap();
    assert_eq!(decoded, tag);
}

#[test]
fn truncated_and_random_nbt_never_panics() {
    let tag = nested_tag(8);
    let bytes = nbt::write_root_to_vec(&tag).unwrap();
    for end in 0..=bytes.len() {
        let prefix = &bytes[..end];
        let _: io::Result<Tag> = assert_no_panic("read_root (truncated)", || {
            let mut cursor: &[u8] = prefix;
            nbt::read_root(&mut cursor)
        });
        let _: io::Result<Tag> = assert_no_panic("read_any_tag (truncated)", || {
            let mut cursor: &[u8] = prefix;
            nbt::read_any_tag(&mut cursor)
        });
    }

    let mut random = seeded_random();
    for _ in 0..200 {
        let bytes = random_buffer(&mut random, 256);
        let _: io::Result<Tag> = assert_no_panic("read_root (random)", || {
            nbt::read_root(&mut bytes.as_slice())
        });
        let _: io::Result<Tag> = assert_no_panic("read_any_tag (random)", || {
            nbt::read_any_tag(&mut bytes.as_slice())
        });
        let _: io::Result<Tag> = assert_no_panic("PacketReader::read_nbt_root", || {
            PacketReader::new(&bytes).read_nbt_root()
        });
        let _: io::Result<Tag> = assert_no_panic("PacketReader::read_nbt_any", || {
            PacketReader::new(&bytes).read_nbt_any()
        });
    }
}

// 7. A small network payload of deeply nested lists must be rejected by the
// binary NBT depth guard instead of overflowing the parser's stack (P11.8).
#[test]
fn nbt_network_deep_nesting_is_rejected() {
    let depth = 2048;
    let mut bytes = Vec::with_capacity(depth * 5 + 7);
    bytes.push(9); // network root tag id: List
    for _ in 0..depth {
        bytes.extend_from_slice(&[9, 0, 0, 0, 1]); // element type List, length 1
    }
    bytes.extend_from_slice(&[1, 0, 0, 0, 1, 0]); // final List of one Byte

    let error = assert_no_panic("PacketReader::read_nbt_any (deep)", || {
        PacketReader::new(&bytes).read_nbt_any()
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("nesting depth"),
        "unexpected error: {error}"
    );
}
