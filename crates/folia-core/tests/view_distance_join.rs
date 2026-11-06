use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A client that asks for view distance 4 receives a 9×9 chunk column, and
/// walking one chunk east streams a single new 9-chunk column.
#[test]
fn a_client_view_distance_bounds_the_streamed_area() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-view-distance-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    folia_core::join::configure_world_dir(root.clone());
    folia_core::join::configure_playerdata(root.join("playerdata"));
    folia_core::worldgen::generator::configure(4242).expect("configure generator");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    let handshake = Handshake {
        protocol_version: 774,
        server_address: "localhost".into(),
        server_port: address.port(),
        next_state: NextState::Login,
    };

    let server = std::thread::spawn(move || -> io::Result<()> {
        let (stream, _) = listener.accept()?;
        handle_login(stream, &handshake)
    });

    let mut client = TcpStream::connect(address).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    write_packet(
        &mut client,
        &build_login_start("ViewDistanceTest", Some([0x66; 16])).unwrap(),
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
    write_packet(&mut client, &client_information(4)).unwrap();
    write_packet(&mut client, &empty_known_packs()).unwrap();

    loop {
        let packet = read_packet(&mut client).expect("registry packet");
        if packet_id(&packet) == 3 {
            break;
        }
    }
    write_packet(&mut client, &simple_packet(3)).unwrap();

    let first = read_packet(&mut client).expect("play login");
    assert_eq!(packet_id(&first), 48, "Join Game");

    let (spawn_x, spawn_y, spawn_z) = loop {
        let packet = read_packet(&mut client).expect("play packet");
        if packet_id(&packet) == 70 {
            break decode_position(&packet).expect("join position payload");
        }
        assert_ne!(
            packet_id(&packet),
            44,
            "the position is sent before the chunk batch"
        );
    };

    let mut columns = Vec::new();
    loop {
        let packet = read_packet(&mut client).expect("chunk packet");
        if packet_id(&packet) == 44 {
            let mut reader = PacketReader::new(&packet);
            let _ = reader.read_varint();
            columns.push((reader.read_i32().unwrap(), reader.read_i32().unwrap()));
            if columns.len() == 81 {
                break;
            }
        }
    }
    assert_eq!(columns.len(), 81, "view distance 4 streams 9x9 columns");
    assert_eq!(
        columns[0],
        (0, 0),
        "the column under the player is sent first"
    );
    let distances: Vec<i32> = columns.iter().map(|(x, z)| x * x + z * z).collect();
    assert!(
        distances.windows(2).all(|pair| pair[0] <= pair[1]),
        "columns arrive nearest-first"
    );

    // Move 8.5 blocks east in one legal step (< the 10-block guard) so the
    // player crosses into chunk 1 without a correction.
    write_packet(&mut client, &move_packet(spawn_x + 8.5, spawn_y, spawn_z)).unwrap();
    let mut new_chunks = 0;
    loop {
        let packet = read_packet(&mut client).expect("streamed packet");
        if packet_id(&packet) == 44 {
            new_chunks += 1;
            if new_chunks == 9 {
                break;
            }
        }
    }
    assert_eq!(new_chunks, 9, "one new 9-chunk column when moving +X");

    drop(client);
    server.join().expect("server thread").expect("join flow");
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
    writer.write_u8(1).unwrap();
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

fn client_information(view_distance: u8) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0).unwrap();
    writer.write_string("en_us").unwrap();
    writer.write_u8(view_distance).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}
