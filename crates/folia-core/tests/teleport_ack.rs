//! End-to-end teleport acknowledgement gating for live movement validation.
//!
//! The login handshake and position packet mirror `tests/movement_validation.rs`;
//! the id flow is what this suite pins: the login teleport (id 1000) is not
//! tracked, the first play-loop correction carries 1002, a mismatched ack never
//! clears it, and the matching ack resumes validation with the next id.

use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_accept_teleportation,
    build_login_start, read_packet, write_packet,
};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn teleport_acknowledgement_gates_movement_validation() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-teleport-ack-{}-{}",
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
    let uuid = [0xA4; 16];

    let (done_tx, done_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        done_tx
            .send(folia_core::join::handle_login(stream, &handshake))
            .ok();
    });

    let mut client = TcpStream::connect(address).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    write_packet(
        &mut client,
        &build_login_start("TeleportAck", Some(uuid)).unwrap(),
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

    let (login_id, accepted_x, accepted_y, accepted_z) = loop {
        let packet = read_packet(&mut client).expect("join position");
        if packet_id(&packet) == 70 {
            break decode_position(&packet).expect("join position payload");
        }
    };
    assert_eq!(login_id, 1000, "the login position uses the reserved id");

    // An ack for an id the play loop never issued is ignored safely.
    write_packet(&mut client, &build_accept_teleportation(424_242)).unwrap();
    // Refresh the catch-up clock after the chunk batch before jumping.
    write_packet(&mut client, &rotation_packet(0.0, 0.0)).unwrap();
    write_packet(
        &mut client,
        &move_packet(accepted_x + 500.0, accepted_y, accepted_z),
    )
    .unwrap();

    let first = loop {
        let packet = read_packet(&mut client).expect("first correction");
        if packet_id(&packet) == 70 {
            break decode_position(&packet).expect("correction payload");
        }
    };
    assert_eq!(
        first.0, 1002,
        "the first play-loop correction carries a fresh id"
    );
    assert!((first.1 - accepted_x).abs() < 1.0e-6);
    assert!((first.2 - accepted_y).abs() < 1.0e-6);
    assert!((first.3 - accepted_z).abs() < 1.0e-6);

    // Wrong ack, an oversized move that must be ignored while pending, then
    // the matching ack, a legal walk, and a final oversized move. If the gate
    // or the ack were broken this would produce a second correction (1004).
    write_packet(&mut client, &build_accept_teleportation(first.0 + 1)).unwrap();
    write_packet(
        &mut client,
        &move_packet(accepted_x + 500.0, accepted_y, accepted_z),
    )
    .unwrap();
    write_packet(&mut client, &build_accept_teleportation(first.0)).unwrap();
    write_packet(
        &mut client,
        &move_packet(accepted_x + 0.1, accepted_y, accepted_z),
    )
    .unwrap();
    write_packet(
        &mut client,
        &move_packet(accepted_x + 500.0, accepted_y, accepted_z),
    )
    .unwrap();

    client
        .set_read_timeout(Some(Duration::from_millis(700)))
        .expect("short read timeout");
    let mut corrections = Vec::new();
    loop {
        match read_packet(&mut client) {
            Ok(packet) => {
                if packet_id(&packet) == 70 {
                    corrections.push(decode_position(&packet).expect("correction payload"));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("unexpected read error: {error}"),
        }
    }
    assert_eq!(
        corrections.len(),
        1,
        "only the post-ack jump is corrected; the pending jump and legal walk are not"
    );
    assert_eq!(corrections[0].0, first.0 + 1);
    assert!(
        (corrections[0].1 - accepted_x - 0.1).abs() < 1.0e-6,
        "the legal walk after the ack was kept, not rubber-banded"
    );

    drop(client);
    let result = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("session thread finishes");
    assert!(result.is_ok(), "session ends cleanly: {result:?}");
    server.join().expect("server thread");
    std::fs::remove_dir_all(root).ok();
}

fn decode_position(packet: &[u8]) -> Option<(i32, f64, f64, f64)> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint().ok()? != 70 {
        return None;
    }
    let teleport_id = reader.read_varint().ok()?;
    Some((
        teleport_id,
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
