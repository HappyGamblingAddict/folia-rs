use folia_core::protocol::{
    NextState, PacketReader, PacketWriter, build_handshake, build_login_start,
    read_compressed_packet, read_packet, write_compressed_packet, write_packet,
};
use std::fs;
use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const MAX_PACKET: usize = 8 * 1024 * 1024;
static DUMP_INDEX: AtomicUsize = AtomicUsize::new(0);

fn main() -> io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:25566".to_owned());
    let port = address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(25566);

    fs::create_dir_all("capture")?;
    let mut stream = TcpStream::connect(&address)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut compression: Option<usize> = None;
    let uuid = [0x42u8; 16];

    let handshake = build_handshake(774, "localhost", port, NextState::Login)?;
    send(&mut stream, &handshake, compression)?;
    send(
        &mut stream,
        &build_login_start("Capture", Some(uuid))?,
        compression,
    )?;
    println!("sent login start");

    // Login state.
    loop {
        let packet = recv(&mut stream, compression)?;
        let id = packet_id(&packet)?;
        dump("login", id, &packet)?;
        println!("login packet id={id} len={}", packet.len());

        match id {
            0 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                println!(
                    "login disconnect: {}",
                    reader.read_string().unwrap_or_default()
                );
                return Ok(());
            }
            2 => {
                // Login Success: UUID + name + properties.
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let _ = reader.read_bytes(16)?;
                let name = reader.read_string()?;
                println!("login success for {name}");
                send(&mut stream, &simple_packet(3), compression)?;
                break;
            }
            3 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let threshold = reader.read_varint()?;
                println!("compression threshold={threshold}");
                if threshold >= 0 {
                    compression = Some(threshold as usize);
                }
            }
            other => println!("unhandled login packet {other}"),
        }
    }

    // Configuration state.
    send(&mut stream, &client_information(), compression)?;
    println!("sent configuration client information");
    let mut config_packets = 0usize;
    loop {
        let packet = recv(&mut stream, compression)?;
        let id = packet_id(&packet)?;
        dump("config", id, &packet)?;
        config_packets += 1;
        println!(
            "config packet id={id} len={} (#{config_packets})",
            packet.len()
        );

        match id {
            3 => {
                send(&mut stream, &simple_packet(3), compression)?;
                println!("sent finish-configuration ack");
                break;
            }
            4 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let nonce = reader.read_i64()?;
                let mut writer = PacketWriter::new();
                writer.write_varint(4).unwrap();
                writer.write_i64(nonce).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            5 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let nonce = reader.read_i32()?;
                let mut writer = PacketWriter::new();
                writer.write_varint(5).unwrap();
                writer.write_varint(nonce).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            14 => {
                // SelectKnownPacks: empty selection means "send everything".
                let mut writer = PacketWriter::new();
                writer.write_varint(7).unwrap();
                writer.write_varint(0).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            19 => {
                // Code of conduct: accept.
                send(&mut stream, &simple_packet(9), compression)?;
            }
            _ => {}
        }
    }

    // Play state: send client information and keep the session alive while
    // capturing the initial Join Game / position / chunk stream.
    send(&mut stream, &client_information_play(), compression)?;
    println!("entered play, capturing for 8 seconds");

    let started = Instant::now();
    let mut play_packets = 0usize;
    while started.elapsed() < Duration::from_secs(8) {
        let packet = match recv(&mut stream, compression) {
            Ok(packet) => packet,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => break,
            Err(error) => return Err(error),
        };
        let id = packet_id(&packet)?;
        dump("play", id, &packet)?;
        play_packets += 1;
        println!("play packet id={id} len={} (#{play_packets})", packet.len());

        match id {
            11 => {
                // ChunkBatchFinished -> ChunkBatchReceived with desired rate.
                let mut writer = PacketWriter::new();
                writer.write_varint(10).unwrap();
                writer.write_bytes(&64.0f32.to_be_bytes());
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            43 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let nonce = reader.read_i64()?;
                let mut writer = PacketWriter::new();
                writer.write_varint(27).unwrap();
                writer.write_i64(nonce).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            59 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let nonce = reader.read_i32()?;
                let mut writer = PacketWriter::new();
                writer.write_varint(44).unwrap();
                writer.write_varint(nonce).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            70 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let teleport_id = reader.read_varint()?;
                let mut writer = PacketWriter::new();
                writer.write_varint(0).unwrap();
                writer.write_varint(teleport_id).unwrap();
                send(&mut stream, &writer.into_inner(), compression)?;
            }
            _ => {}
        }
    }

    println!("capture complete: {config_packets} config packets, {play_packets} play packets");
    Ok(())
}

fn packet_id(packet: &[u8]) -> io::Result<i32> {
    let mut reader = PacketReader::new(packet);
    reader.read_varint()
}

fn simple_packet(id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(id).unwrap();
    writer.into_inner()
}

fn client_information_body() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_string("en_us").unwrap();
    writer.write_u8(2).unwrap(); // view distance
    writer.write_varint(0).unwrap(); // ChatVisiblity.FULL
    writer.write_u8(1).unwrap(); // chat colors
    writer.write_u8(0).unwrap(); // model customisation
    writer.write_varint(0).unwrap(); // HumanoidArm.RIGHT
    writer.write_u8(0).unwrap(); // text filtering
    writer.write_u8(0).unwrap(); // allows listing
    writer.write_varint(0).unwrap(); // ParticleStatus.ALL
    writer.into_inner()
}

fn client_information() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0).unwrap(); // configuration ServerboundClientInformation
    writer.write_bytes(&client_information_body());
    writer.into_inner()
}

fn client_information_play() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(13).unwrap(); // play ServerboundClientInformation
    writer.write_bytes(&client_information_body());
    writer.into_inner()
}

fn send(stream: &mut TcpStream, payload: &[u8], compression: Option<usize>) -> io::Result<()> {
    match compression {
        Some(threshold) => write_compressed_packet(stream, payload, threshold),
        None => write_packet(stream, payload),
    }
}

fn recv(stream: &mut TcpStream, compression: Option<usize>) -> io::Result<Vec<u8>> {
    match compression {
        Some(_) => read_compressed_packet(stream, MAX_PACKET),
        None => read_packet(stream),
    }
}

fn dump(state: &str, id: i32, packet: &[u8]) -> io::Result<()> {
    let index = DUMP_INDEX.fetch_add(1, Ordering::Relaxed);
    fs::write(
        format!("capture/{state}-{id:03}-{index:04}-{:05}.bin", packet.len()),
        packet,
    )
}
