use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn login_generates_noise_terrain_when_world_is_missing() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-worldgen-join-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    folia_core::join::configure_world_dir(root.clone());
    folia_core::join::configure_playerdata(root.join("playerdata"));
    folia_core::worldgen::generator::configure(12345).expect("configure generator");

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
        &build_login_start("WorldGenTest", Some([0x55; 16])).unwrap(),
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

    let first = read_packet(&mut client).expect("play login");
    assert_eq!(packet_id(&first), 48, "Join Game");

    let (spawn_x, spawn_y, spawn_z) = loop {
        let packet = read_packet(&mut client).expect("play packet");
        if packet_id(&packet) == 70 {
            let mut reader = PacketReader::new(&packet);
            let _ = reader.read_varint().unwrap();
            let _ = reader.read_varint().unwrap();
            let x = reader.read_f64().unwrap();
            let y = reader.read_f64().unwrap();
            let z = reader.read_f64().unwrap();
            break (x, y, z);
        }
        assert_ne!(
            packet_id(&packet),
            44,
            "the position is sent before the chunk batch"
        );
    };

    let mut chunks = 0;
    let mut saw_bedrock = false;
    while chunks < 169 {
        let packet = read_packet(&mut client).expect("chunk packet");
        if packet_id(&packet) == 44 {
            chunks += 1;
            if chunk_palette_contains(&packet, 85) {
                saw_bedrock = true;
            }
        }
    }
    assert_eq!(chunks, 169, "13x13 generated chunks");
    assert!(saw_bedrock, "generated chunk palette contains bedrock");
    assert!(
        spawn_y > 0.0,
        "a fresh player spawns on the generated surface, not at the captured \
         y = -60 (got {spawn_y})"
    );

    // Crossing a chunk border streams the newly visible column and moves the
    // chunk cache center with the player.
    // Move 8.5 blocks east in one legal step (< the 10-block guard) so the
    // player crosses into chunk 1 without a correction.
    write_packet(&mut client, &move_packet(spawn_x + 8.5, spawn_y, spawn_z)).unwrap();
    let mut new_chunks = 0;
    let mut recentered = false;
    loop {
        let packet = read_packet(&mut client).expect("streamed packet");
        match packet_id(&packet) {
            44 => {
                new_chunks += 1;
                if new_chunks == 13 {
                    break;
                }
            }
            92 => recentered = true,
            _ => {}
        }
    }
    assert_eq!(new_chunks, 13, "one new chunk column when moving +X");
    assert!(recentered, "chunk cache center follows the player");

    drop(client);
    server.join().expect("server thread").expect("join flow");

    let level = folia_core::level::read(root.join("level.dat")).expect("generated level.dat");
    assert!(level.own_level, "the level file marks the world as ours");
    assert_eq!(level.data_version, folia_core::level::DATA_VERSION);
    assert_eq!(level.seed, Some(12345), "the generator seed is saved");
    let (level_x, level_y, level_z) = level.spawn.expect("world spawn");
    assert_eq!((level_x, level_z), (8, 8), "spawn column");
    assert_eq!(
        (spawn_x, spawn_y),
        (f64::from(level_x) + 0.5, f64::from(level_y)),
        "a new player spawns at the saved world spawn"
    );

    std::fs::remove_dir_all(root).ok();
}

fn chunk_palette_contains(packet: &[u8], state_id: i32) -> bool {
    let mut reader = PacketReader::new(packet);
    let _ = reader.read_varint().unwrap();
    let _ = reader.read_i32().unwrap();
    let _ = reader.read_i32().unwrap();
    let heightmaps = reader.read_varint().unwrap();
    for _ in 0..heightmaps {
        let _ = reader.read_varint().unwrap();
        let longs = reader.read_varint().unwrap();
        let _ = reader.read_bytes(longs as usize * 8).unwrap();
    }
    let length = reader.read_varint().unwrap();
    let data = reader.read_bytes(length as usize).unwrap().to_vec();
    let mut data = PacketReader::new(&data);
    let _ = data.read_bytes(2).unwrap();
    let bits = data.read_u8().unwrap();
    if bits == 0 {
        return data.read_varint().unwrap() == state_id;
    }
    let palette_len = data.read_varint().unwrap();
    for _ in 0..palette_len {
        if data.read_varint().unwrap() == state_id {
            return true;
        }
    }
    false
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
