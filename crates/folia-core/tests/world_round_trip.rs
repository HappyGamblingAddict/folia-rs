use folia_core::anvil::{Compression, RegionFile};
use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn sample_chunk(x: i32, z: i32) -> Chunk {
    Chunk {
        x,
        z,
        sections: vec![
            ChunkSection {
                y: 0,
                block_states: PalettedContainer {
                    palette: vec![state("minecraft:stone"), state("minecraft:dirt")],
                    indices: (0..4096).map(|index| (index % 2) as u16).collect(),
                },
                biomes: Some(PalettedContainer {
                    palette: vec!["minecraft:plains".into()],
                    indices: vec![0; 64],
                }),
            },
            ChunkSection {
                y: 1,
                block_states: PalettedContainer {
                    palette: vec![state("minecraft:air")],
                    indices: vec![0; 4096],
                },
                biomes: None,
            },
        ],
    }
}

#[test]
fn generated_chunk_round_trips_through_anvil() {
    let path = std::env::temp_dir().join(format!(
        "folia-rs-world-{}-{}.mca",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let chunk = sample_chunk(12, -34);
    let tag = encode_chunk(&chunk).expect("encode chunk");

    let mut region = RegionFile::create(&path).expect("create region");
    region
        .write_chunk(12, 2, &tag, Compression::Zlib)
        .expect("write chunk");
    region.save().expect("save region");

    let reopened = RegionFile::open(&path).expect("reopen region");
    let stored = reopened
        .read_chunk(12, 2)
        .expect("read chunk")
        .expect("chunk exists");
    let decoded = decode_chunk(&stored).expect("decode chunk");

    assert_eq!(decoded.x, 12);
    assert_eq!(decoded.z, -34);
    assert_eq!(decoded.sections.len(), 2);
    assert_eq!(
        decoded.block_state(0, 0, 0).unwrap().name,
        "minecraft:stone"
    );
    assert_eq!(decoded.block_state(1, 0, 0).unwrap().name, "minecraft:dirt");
    assert_eq!(decoded.block_state(0, 16, 0).unwrap().name, "minecraft:air");
    assert_eq!(decoded.biome(0, 0, 0).unwrap(), "minecraft:plains");

    std::fs::remove_file(path).expect("remove test region");
}
