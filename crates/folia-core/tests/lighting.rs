use folia_core::anvil::Compression;
use folia_core::block_update::set_block;
use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use folia_core::chunk_packet::light_bytes;
use folia_core::light::{DataLayer, SectionLight};
use folia_core::protocol::PacketReader;
use folia_core::world::WorldStorage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MIN_SECTION: i8 = -4;
const MAX_SECTION: i8 = 19;

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: vec![],
    }
}

fn stone() -> BlockState {
    BlockState {
        name: "minecraft:stone".into(),
        properties: vec![],
    }
}

fn torch() -> BlockState {
    BlockState {
        name: "minecraft:torch".into(),
        properties: vec![],
    }
}

fn air_chunk(x: i32, z: i32) -> Chunk {
    let mut sections = Vec::with_capacity(24);
    for y in MIN_SECTION..=MAX_SECTION {
        sections.push(ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![air()],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        });
    }
    Chunk { x, z, sections }
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-light-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[derive(Debug)]
struct ParsedLight {
    sky_mask: u64,
    block_mask: u64,
    empty_sky: u64,
    empty_block: u64,
    sky: Vec<(usize, Vec<u8>)>,
    block: Vec<(usize, Vec<u8>)>,
}

fn parse_bitset(reader: &mut PacketReader<'_>) -> u64 {
    let count = reader.read_varint().unwrap();
    let mut mask = 0u64;
    for index in 0..count {
        let value = reader.read_i64().unwrap() as u64;
        if index == 0 {
            mask = value;
        }
    }
    mask
}

fn bits(mask: u64) -> Vec<usize> {
    (0..64).filter(|index| mask & (1 << index) != 0).collect()
}

fn parse_light(bytes: &[u8]) -> ParsedLight {
    let mut reader = PacketReader::new(bytes);
    let sky_mask = parse_bitset(&mut reader);
    let block_mask = parse_bitset(&mut reader);
    let empty_sky = parse_bitset(&mut reader);
    let empty_block = parse_bitset(&mut reader);

    let mut read_updates = |mask: u64| {
        let count = reader.read_varint().unwrap();
        assert_eq!(
            count as usize,
            bits(mask).len(),
            "update count matches mask"
        );
        let mut updates = Vec::with_capacity(count as usize);
        for index in bits(mask) {
            let length = reader.read_varint().unwrap();
            assert_eq!(length, 2048);
            updates.push((index, reader.read_bytes(length as usize).unwrap().to_vec()));
        }
        updates
    };

    let sky = read_updates(sky_mask);
    let block = read_updates(block_mask);
    assert!(reader.is_empty());
    ParsedLight {
        sky_mask,
        block_mask,
        empty_sky,
        empty_block,
        sky,
        block,
    }
}

fn layer_bytes(layer: &DataLayer) -> Vec<u8> {
    layer.as_bytes().to_vec()
}

#[test]
fn nibble_layers_round_trip_through_chunk_nbt() {
    let mut chunk = air_chunk(2, -3);
    let section = chunk
        .sections
        .iter_mut()
        .find(|section| section.y == 0)
        .unwrap();
    let mut sky = DataLayer::filled(15);
    sky.set(3, 4, 5, 0);
    let mut block = DataLayer::new();
    block.set(8, 9, 10, 14);
    block.set(0, 0, 0, 1);
    section.light = SectionLight {
        sky: Some(sky.clone()),
        block: Some(block.clone()),
    };

    let tag = encode_chunk(&chunk).expect("encode chunk");
    let encoded = tag
        .get("sections")
        .and_then(|tag| tag.as_list())
        .unwrap()
        .iter()
        .find(|section| section.get("Y").and_then(|tag| tag.as_byte()) == Some(0))
        .expect("section 0");
    assert_eq!(
        encoded
            .get("SkyLight")
            .and_then(|tag| tag.as_byte_array())
            .map(<[u8]>::len),
        Some(2048)
    );
    assert_eq!(
        encoded
            .get("BlockLight")
            .and_then(|tag| tag.as_byte_array())
            .map(<[u8]>::len),
        Some(2048)
    );

    let decoded = decode_chunk(&tag).expect("decode chunk");
    let decoded_section = decoded.section(0).unwrap();
    assert_eq!(decoded_section.light.sky.as_ref(), Some(&sky));
    assert_eq!(decoded_section.light.block.as_ref(), Some(&block));
    assert_eq!(decoded.sky_light(3, 4, 5), Some(0));
    assert_eq!(decoded.block_light(8, 9, 10), Some(14));
    assert_eq!(decoded.block_light(0, 0, 0), Some(1));

    // A block edit invalidates cached light instead of persisting stale arrays.
    let mut edited = decoded;
    assert!(edited.set_block_state(8, 9, 10, stone()));
    assert!(!edited.has_light());
}

#[test]
fn sky_light_spreads_horizontally_under_an_overhang() {
    let mut chunk = air_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 70, z, stone()));
        }
    }
    // Punch a one-column shaft so the roof has no open chunk edge for light
    // to leak around; the only light source is directly above (8, 8). The
    // surrounding roofed columns are supplied as neighbours so the chunk
    // border does not act as an artificial open edge.
    assert!(chunk.set_block_state(8, 70, 8, air()));
    let mut neighbours = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let mut neighbour = air_chunk(dx, dz);
            for x in 0..16 {
                for z in 0..16 {
                    assert!(neighbour.set_block_state(x, 70, z, stone()));
                }
            }
            neighbours.push(neighbour);
        }
    }
    chunk.recompute_light_with_neighbours(&neighbours);

    assert_eq!(
        chunk.sky_light(8, 69, 8),
        Some(15),
        "the shaft keeps full sky light"
    );
    assert_eq!(
        chunk.sky_light(7, 69, 8),
        Some(14),
        "one step under the roof edge is 14"
    );
    assert_eq!(
        chunk.sky_light(0, 69, 8),
        Some(7),
        "light falls by one per block along the roof"
    );
    assert_eq!(
        chunk.sky_light(0, 70, 8),
        Some(0),
        "the roof itself blocks sky light"
    );
    assert_eq!(
        chunk.sky_light(0, 71, 8),
        Some(15),
        "sky above the roof is still full"
    );
}

#[test]
fn torch_block_light_has_the_expected_radius() {
    let mut chunk = air_chunk(0, 0);
    assert!(chunk.set_block_state(8, 70, 8, torch()));
    chunk.recompute_light();

    assert_eq!(chunk.block_light(8, 70, 8), Some(14), "torch source");
    assert_eq!(chunk.block_light(7, 70, 8), Some(13), "one block away");
    assert_eq!(chunk.block_light(8, 71, 8), Some(13), "one block above");
    assert_eq!(chunk.block_light(8, 70, 15), Some(7), "seven blocks away");
    assert_eq!(
        chunk.sky_light(8, 71, 8),
        Some(15),
        "sky layer is separate from block light"
    );
}

#[test]
fn unsupported_emitters_emit_zero_without_corrupting_storage() {
    let mut chunk = air_chunk(0, 0);
    let unsupported = BlockState {
        name: "minecraft:not_a_real_emitter".into(),
        properties: vec![],
    };
    assert!(chunk.set_block_state(2, 70, 2, unsupported));
    chunk.recompute_light();

    assert_eq!(chunk.block_light(2, 70, 2), Some(0));
    assert_eq!(chunk.block_light(1, 70, 2), Some(0));
    assert_eq!(chunk.sky_light(2, 72, 2), Some(15));
    assert!(chunk.has_light());
}

#[test]
fn packet_masks_match_stored_light_arrays() {
    let mut chunk = air_chunk(0, 0);
    for x in 0..8 {
        for z in 0..16 {
            assert!(chunk.set_block_state(x, 70, z, stone()));
        }
    }
    assert!(chunk.set_block_state(4, 69, 4, torch()));
    chunk.recompute_light();

    let parsed = parse_light(&light_bytes(&chunk));
    let sky_by_index: HashMap<usize, Vec<u8>> = parsed.sky.clone().into_iter().collect();
    let block_by_index: HashMap<usize, Vec<u8>> = parsed.block.clone().into_iter().collect();

    for section in &chunk.sections {
        let index = (section.y as i32 - (MIN_SECTION as i32 - 1)) as usize;
        if let Some(sky) = &section.light.sky {
            if sky.is_empty() {
                assert_ne!(
                    parsed.empty_sky & (1 << index),
                    0,
                    "empty sky section {index} is in the empty mask"
                );
            } else {
                assert_ne!(
                    parsed.sky_mask & (1 << index),
                    0,
                    "non-empty sky section {index} has a mask bit"
                );
                assert_eq!(
                    sky_by_index.get(&index),
                    Some(&layer_bytes(sky)),
                    "sky bytes for section {index}"
                );
            }
        }
        if let Some(block) = &section.light.block {
            if block.is_empty() {
                assert_ne!(
                    parsed.empty_block & (1 << index),
                    0,
                    "empty block section {index} is in the empty mask"
                );
            } else {
                assert_ne!(
                    parsed.block_mask & (1 << index),
                    0,
                    "non-empty block section {index} has a mask bit"
                );
                assert_eq!(
                    block_by_index.get(&index),
                    Some(&layer_bytes(block)),
                    "block bytes for section {index}"
                );
            }
        }
    }
}

#[test]
fn neighbour_torch_light_crosses_the_chunk_border() {
    let mut center = air_chunk(0, 0);
    let mut east = air_chunk(1, 0);
    assert!(east.set_block_state(0, 70, 8, torch()));

    center.recompute_light();
    assert_eq!(center.block_light(15, 70, 8), Some(0));

    center.recompute_light_with_neighbours(&[east]);
    assert_eq!(
        center.block_light(15, 70, 8),
        Some(13),
        "torch one block into the neighbour lights the border"
    );
    assert_eq!(center.block_light(14, 70, 8), Some(12));
}

#[test]
fn block_edit_recomputes_and_persists_light() {
    let root = temp_dir("edit");
    let region_dir = root.join("region");
    let mut storage = WorldStorage::new(&region_dir);
    let flat = folia_core::chunk_packet::flat_chunk(0, 0);
    let before = light_bytes(&flat);
    storage
        .save_chunk(&flat, Compression::Zlib)
        .expect("save flat chunk");

    let change = set_block(&mut storage, 8, -60, 8, torch())
        .expect("set torch")
        .expect("torch changed the chunk");
    assert_ne!(change.light, before, "light payload changes after a torch");

    let parsed = parse_light(&change.light);
    let index = ((-60i32).div_euclid(16) - (MIN_SECTION as i32 - 1)) as usize;
    assert_ne!(
        parsed.block_mask & (1 << index),
        0,
        "torch section has a block-light update"
    );

    storage.flush().expect("flush");
    let mut reopened = WorldStorage::new(&region_dir);
    let chunk = reopened.load_chunk(0, 0).expect("load").expect("chunk");
    assert_eq!(chunk.block_light(8, -60, 8), Some(14));
    assert_eq!(chunk.block_light(7, -60, 8), Some(13));
    assert!(chunk.sky_light(8, -59, 8).is_some());

    // The stored NBT itself carries both layers, so a later reader sees the
    // same light without recomputing it.
    let mut encoded = encode_chunk(&chunk).expect("encode");
    let decoded = decode_chunk(&encoded).expect("decode");
    assert_eq!(decoded.block_light(8, -60, 8), Some(14));
    assert_eq!(decoded.sky_light(8, -59, 8), chunk.sky_light(8, -59, 8));

    // Re-saving a decoded chunk must not append duplicate or stale fields.
    storage
        .save_chunk(&decoded, Compression::Zlib)
        .expect("save decoded");
    encoded = encode_chunk(&decoded).expect("encode again");
    assert_eq!(
        decode_chunk(&encoded).unwrap().block_light(8, -60, 8),
        Some(14)
    );

    std::fs::remove_dir_all(root).unwrap();
}
