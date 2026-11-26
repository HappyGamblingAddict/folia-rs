use folia_core::chunk_packet_diagnostics::ChunkPacketDump;

fn main() {
    let dump = ChunkPacketDump::build().expect("build chunk packet dump");
    println!("{}", dump.to_json_string());
}
