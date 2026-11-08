use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A move that jumps hundreds of blocks in one packet is reverted and the
/// client is sent back to the last accepted position.
#[test]
fn an_oversized_move_is_reverted_and_corrected() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-movement-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let playerdata = root.join("playerdata");
    std::fs::create_dir_all(&playerdata).unwrap();
    folia_core::join::configure_world_dir(root.clone());
    folia_core::join::configure_playerdata(playerdata);
    folia_core::worldgen::generator::configure(11).expect("configure generator");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    let handshake = Handshake {
        protocol_version: 774,
        server_address: "localhost".into(),
        server_port: address.port(),
        next_state: NextState::Login,
    };
    let uuid = [0x91; 16];

    let (done_tx, done_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let result = handle_login(stream, &handshake);
        done_tx.send(result).ok();
    });

    let mut client = TcpStream::connect(address).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    write_packet(
        &mut client,
        &build_login_start("MoveTest", Some(uuid)).unwrap(),
    )
    .unwrap();

    let login_success = read_packet(&mut client).expect("login success");
    let mut reader = PacketReader::new(&login_success);
    assert_eq!(reader.read_varint().unwrap(), 2);
    write_packet(&mut client, &simple_packet(3)).unwrap();

    loop {
        let packet = read_packet(&mut client).expect("config packet");
        if packet_id(&packet) == 14 {
            break;
        }
    }
    write_packet(&mut client, &client_information()).unwrap();
    write_packet(&mut client, &empty_known_packs()).unwrap();

    loop {
        let packet = read_packet(&mut client).expect("registry packet");
        if packet_id(&packet) == 3 {
            break;
        }
    }
    write_packet(&mut client, &simple_packet(3)).unwrap();
    assert_eq!(packet_id(&read_packet(&mut client).expect("join game")), 48);

    let (accepted_x, accepted_y, accepted_z) = loop {
        let packet = read_packet(&mut client).expect("join position");
        if packet_id(&packet) == 70 {
            break decode_position(&packet).expect("join position payload");
        }
    };

    // Refresh the movement catch-up clock without moving; the 500-block jump
    // below is then measured by the normal per-packet guard.
    write_packet(&mut client, &rotation_packet(0.0, 0.0)).unwrap();
    write_packet(
        &mut client,
        &move_packet(accepted_x + 500.0, accepted_y, accepted_z),
    )
    .unwrap();

    let correction = loop {
        let packet = read_packet(&mut client).expect("correction");
        if packet_id(&packet) == 70 {
            break decode_position(&packet).expect("position payload");
        }
    };
    assert!((correction.0 - accepted_x).abs() < 1.0e-6, "x is restored");
    assert!((correction.1 - accepted_y).abs() < 1.0e-6, "y is restored");
    assert!((correction.2 - accepted_z).abs() < 1.0e-6, "z is restored");

    drop(client);
    let result = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("session thread finishes");
    assert!(result.is_ok(), "session ends cleanly: {result:?}");
    server.join().expect("server thread");
    std::fs::remove_dir_all(root).ok();
}

fn decode_position(packet: &[u8]) -> Option<(f64, f64, f64)> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint().ok()? != 70 {
        return None;
    }
    let _teleport_id = reader.read_varint().ok()?;
    Some((
        reader.read_f64().ok()?,
        reader.read_f64().ok()?,
        reader.read_f64().ok()?,
    ))
}

fn move_packet(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(29).unwrap();
    writer.write_f64(x).unwrap();
    writer.write_f64(y).unwrap();
    writer.write_f64(z).unwrap();
    writer.write_u8(0).unwrap();
    writer.into_inner()
}

fn rotation_packet(yaw: f32, pitch: f32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(31).unwrap();
    writer.write_f32(yaw).unwrap();
    writer.write_f32(pitch).unwrap();
    writer.write_u8(0).unwrap();
    writer.into_inner()
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
