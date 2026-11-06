//! Prints `REG<TAB>registry<TAB>id<TAB>name` for every registry entry in the
//! captured configuration stream.
//!
//! Run with:
//! `cargo run -q -p folia-core --example dump_registries > registries.tsv`

use folia_core::protocol::PacketReader;
use std::io;

const CONFIG: &[u8] = include_bytes!("../assets/join_config.bin");
const REGISTRY_DATA_PACKET: i32 = 7;

fn main() -> io::Result<()> {
    let mut offset = 0usize;
    while offset + 4 <= CONFIG.len() {
        let mut length = [0u8; 4];
        length.copy_from_slice(&CONFIG[offset..offset + 4]);
        offset += 4;
        let length = u32::from_be_bytes(length) as usize;
        if offset + length > CONFIG.len() {
            break;
        }
        let packet = &CONFIG[offset..offset + length];
        offset += length;

        let mut reader = PacketReader::new(packet);
        if reader.read_varint()? != REGISTRY_DATA_PACKET {
            continue;
        }
        let registry = reader.read_string()?;
        let count = reader.read_varint()?;
        for id in 0..count {
            let name = reader.read_string()?;
            if reader.read_bool()? {
                let _ = reader.read_nbt_any()?;
            }
            println!("REG\t{registry}\t{id}\t{name}");
        }
    }
    Ok(())
}
