use folia_core::protocol::PacketReader;
use std::fs;
use std::io;

fn main() -> io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: inspect_chunk_packet <play-044-....bin>");
    let packet = fs::read(&path)?;
    let mut reader = PacketReader::new(&packet);
    let id = reader.read_varint()?;
    let x = reader.read_i32()?;
    let z = reader.read_i32()?;
    println!("packet id={id} chunk=({x},{z}) len={}", packet.len());

    let heightmaps = reader.read_varint()?;
    println!("heightmaps={heightmaps}");
    for _ in 0..heightmaps {
        let kind = reader.read_varint()?;
        let length = reader.read_varint()?;
        let bytes = reader.read_bytes(length as usize * 8)?;
        println!(
            "  heightmap kind={kind} longs={length} bytes={}",
            bytes.len()
        );
    }

    let chunk_data_len = reader.read_varint()?;
    let chunk_data = reader.read_bytes(chunk_data_len as usize)?.to_vec();
    let block_entities = reader.read_varint()?;
    println!("chunk_data={chunk_data_len} block_entities={block_entities}");

    let sky_mask = read_bitset(&mut reader)?;
    let block_mask = read_bitset(&mut reader)?;
    let empty_sky = read_bitset(&mut reader)?;
    let empty_block = read_bitset(&mut reader)?;
    let sky_updates = reader.read_varint()?;
    for _ in 0..sky_updates {
        let length = reader.read_varint()?;
        let _ = reader.read_bytes(length as usize)?;
    }
    let block_updates = reader.read_varint()?;
    for _ in 0..block_updates {
        let length = reader.read_varint()?;
        let _ = reader.read_bytes(length as usize)?;
    }
    println!(
        "light sky_mask={sky_mask} block_mask={block_mask} empty_sky={empty_sky} empty_block={empty_block} sky_updates={sky_updates} block_updates={block_updates} remaining={}",
        reader.remaining()
    );

    inspect_sections(&chunk_data)
}

fn read_bitset(reader: &mut PacketReader<'_>) -> io::Result<u64> {
    let words = reader.read_varint()?;
    let mut bits = 0u64;
    for index in 0..words {
        let word = reader.read_i64()? as u64;
        if word != 0 {
            bits |= word << (index * 64).min(63);
        }
    }
    Ok(bits)
}

fn inspect_sections(data: &[u8]) -> io::Result<()> {
    let mut reader = PacketReader::new(data);
    let mut section = 0;
    while reader.remaining() > 0 {
        let non_empty = reader.read_bytes(2)?;
        let non_empty = i16::from_be_bytes([non_empty[0], non_empty[1]]);
        let block_bits = reader.read_u8()?;
        let block_palette_len = reader.read_varint()?;
        let mut block_palette = Vec::new();
        for _ in 0..block_palette_len {
            block_palette.push(reader.read_varint()?);
        }
        let per_long = 64 / block_bits.max(1) as usize;
        let block_longs = if block_palette_len <= 1 {
            0
        } else {
            (4096usize).div_ceil(per_long)
        };
        for _ in 0..block_longs {
            let _ = reader.read_i64()?;
        }

        let biome_bits = reader.read_u8()?;
        let biome_palette_len = reader.read_varint()?;
        let mut biome_palette = Vec::new();
        for _ in 0..biome_palette_len {
            biome_palette.push(reader.read_varint()?);
        }
        let biome_per_long = 64 / biome_bits.max(1) as usize;
        let biome_longs = if biome_palette_len <= 1 {
            0
        } else {
            (64usize).div_ceil(biome_per_long)
        };
        for _ in 0..biome_longs {
            let _ = reader.read_i64()?;
        }

        println!(
            "  section {section}: non_empty={non_empty} block_bits={block_bits} palette={block_palette:?} biome_bits={biome_bits} biome_palette={biome_palette:?} remaining={}",
            reader.remaining()
        );
        section += 1;
        if section > 40 {
            break;
        }
    }
    Ok(())
}
