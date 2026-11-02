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

    let mut chunks = 0;
    let mut saw_bedrock = false;
    loop {
        let packet = read_packet(&mut client).expect("play packet");
        if packet_id(&packet) == 44 {
            chunks += 1;
            if chunk_palette_contains(&packet, 85) {
                saw_bedrock = true;
            }
        }
        if packet_id(&packet) == 70 {
            break;
        }
    }
    assert_eq!(chunks, 49, "7x7 generated chunks");
    assert!(saw_bedrock, "generated chunk palette contains bedrock");

    drop(client);
    server.join().expect("server thread").expect("join flow");
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
