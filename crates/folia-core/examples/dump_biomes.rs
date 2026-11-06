use folia_core::protocol::PacketReader;
use std::fs;
use std::io;

fn main() -> io::Result<()> {
    let bytes = fs::read("crates/folia-core/assets/join_config.bin")?;
    for packet in unpack(&bytes) {
        let mut reader = PacketReader::new(&packet);
        if reader.read_varint()? != 7 {
            continue;
        }
        if reader.read_string()? != "minecraft:worldgen/biome" {
            continue;
        }
        let count = reader.read_varint()?;
        for index in 0..count {
            let name = reader.read_string()?;
            if reader.read_bool()? {
                let _ = reader.read_nbt_any()?;
            }
            println!("{index}\t{name}");
        }
    }
    Ok(())
}

fn unpack(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut offset = 0;
    while offset + 4 <= bytes.len() {
        let mut length = [0u8; 4];
        length.copy_from_slice(&bytes[offset..offset + 4]);
        offset += 4;
        let length = u32::from_be_bytes(length) as usize;
        if offset + length > bytes.len() {
            break;
        }
        packets.push(bytes[offset..offset + length].to_vec());
        offset += length;
    }
    packets
}
