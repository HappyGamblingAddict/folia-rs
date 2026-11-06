use folia_core::protocol::{
    NextState, PacketReader, PacketWriter, build_handshake, build_login_start, read_packet,
    write_packet,
};
use std::io;
use std::net::TcpStream;
use std::time::{Duration, Instant};

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

    let started = Instant::now();
    let mut chunks = 0;
    // The server's position packet is where the client ends up; report it and
    // move from there instead of assuming a spawn height.
    let mut position = (0.5_f64, 64.0_f64, 0.5_f64);
    loop {
        let packet = read_packet(&mut stream)?;
        match packet_id(&packet) {
            44 => {
                if chunks == 0 {
                    println!("first chunk after {:.2}s", started.elapsed().as_secs_f64());
                }
                chunks += 1;
            }
            70 => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint();
                let _ = reader.read_varint();
                position.0 = reader.read_f64()?;
                position.1 = reader.read_f64()?;
                position.2 = reader.read_f64()?;
                break;
            }
            _ => {}
        }
    }
    println!("play stream received: {chunks} live chunk packets");
    println!(
        "live position packet received at ({:.1}, {:.1}, {:.1})",
        position.0, position.1, position.2
    );

    // Send one movement packet so the server persists a live position.
    let mut movement = PacketWriter::new();
    movement.write_varint(29).unwrap();
    movement.write_f64(position.0).unwrap();
    movement.write_f64(position.1).unwrap();
    movement.write_f64(position.2).unwrap();
    movement.write_u8(1).unwrap();
    write_packet(&mut stream, &movement.into_inner())?;
    std::thread::sleep(Duration::from_millis(250));
    println!("movement packet sent");

    if chunks == 0 {
        std::process::exit(4);
    }

    // Optional walk mode: `join_probe <address> <blocks-east>`.
    let walk_blocks: f64 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    if walk_blocks > 0.0 {
        let mut walked = 0.0;
        let mut extra_chunks = 0usize;
        stream.set_read_timeout(Some(Duration::from_millis(400)))?;
        while walked < walk_blocks {
            walked = (walked + 8.0).min(walk_blocks);
            let x = position.0 + walked;
            let mut movement = PacketWriter::new();
            movement.write_varint(29).unwrap();
            movement.write_f64(x).unwrap();
            movement.write_f64(position.1).unwrap();
            movement.write_f64(position.2).unwrap();
            movement.write_u8(1).unwrap();
            write_packet(&mut stream, &movement.into_inner())?;
            loop {
                match read_packet(&mut stream) {
                    Ok(packet) => {
                        if packet_id(&packet) == 44 {
                            extra_chunks += 1;
                        }
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            || error.kind() == io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        println!(
            "walked {walked:.0} blocks east from spawn: {extra_chunks} new live chunk packets"
        );
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
    writer.write_u8(6).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}
