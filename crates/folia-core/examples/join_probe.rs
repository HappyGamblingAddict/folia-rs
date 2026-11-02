use folia_core::protocol::{
    NextState, PacketReader, PacketWriter, build_handshake, build_login_start, read_packet,
    write_packet,
};
use std::io;
use std::net::TcpStream;
use std::time::Duration;

fn main() -> io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:25565".to_owned());
    let port = address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(25565);

    let mut stream = TcpStream::connect(&address)?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    write_packet(
        &mut stream,
        &build_handshake(774, "localhost", port, NextState::Login)?,
    )?;
    write_packet(
        &mut stream,
        &build_login_start("RustProbe", Some([0x24; 16]))?,
    )?;

    let login_success = read_packet(&mut stream)?;
    let id = packet_id(&login_success);
    if id != 2 {
        let mut reader = PacketReader::new(&login_success);
        let _ = reader.read_varint();
        println!(
            "login rejected: {}",
            reader.read_string().unwrap_or_default()
        );
        std::process::exit(2);
    }
    println!("login accepted (id 2)");
    write_packet(&mut stream, &simple_packet(3))?;

    let mut config_packets = 0;
    loop {
        let packet = read_packet(&mut stream)?;
        config_packets += 1;
        match packet_id(&packet) {
            14 => {
                write_packet(&mut stream, &client_information())?;
                write_packet(&mut stream, &empty_known_packs())?;
            }
            3 => {
                write_packet(&mut stream, &simple_packet(3))?;
                break;
            }
            _ => {}
        }
    }
    println!("configuration complete ({config_packets} packets)");

    let first = read_packet(&mut stream)?;
    let first_id = packet_id(&first);
    if first_id != 48 {
        println!("expected Join Game id 48, got {first_id}");
        std::process::exit(3);
    }
    println!("Join Game received (id 48)");

    let mut chunks = 0;
    loop {
        let packet = read_packet(&mut stream)?;
        match packet_id(&packet) {
            44 => chunks += 1,
            70 => break,
            _ => {}
        }
    }
    println!("play stream received: {chunks} live chunk packets");
    println!("live position packet received");

    // Send one movement packet so the server persists a live position.
    let mut movement = PacketWriter::new();
    movement.write_varint(29).unwrap();
    movement.write_f64(12.5).unwrap();
    movement.write_f64(-60.0).unwrap();
    movement.write_f64(8.25).unwrap();
    movement.write_u8(1).unwrap();
    write_packet(&mut stream, &movement.into_inner())?;
    std::thread::sleep(Duration::from_millis(250));
    println!("movement packet sent");

    if chunks == 0 {
        std::process::exit(4);
    }
    Ok(())
}

fn packet_id(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().unwrap()
}

fn simple_packet(id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(id).unwrap();
    writer.into_inner()
}

fn empty_known_packs() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(7).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}

fn client_information() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0).unwrap();
    writer.write_string("en_us").unwrap();
    writer.write_u8(2).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}
