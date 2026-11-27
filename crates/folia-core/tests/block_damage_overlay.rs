//! Live block-damage overlay frames for an active survival dig.
//!
//! The client must see the ten-stage crack overlay advance while digging and
//! receive a clear frame when the dig stops. This drives a real session so the
//! wire bytes are checked, not just the packet builder.

use folia_core::block_update::block_pos_long;
use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use folia_core::world::{Dimension, WorldStorage};
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BLOCK_DESTRUCTION: i32 = 5;
const CLEAR_STAGE: i32 = -1;

#[test]
fn active_dig_sends_overlay_stages_and_a_clear_on_stop() {
    let temp_world = std::env::temp_dir().join(format!(
        "folia-rs-overlay-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_world.join("region")).unwrap();
    {
        let mut fixture = WorldStorage::for_dimension(&temp_world, Dimension::Overworld);
        for chunk_x in -1..=1 {
            for chunk_z in -1..=1 {
                fixture
                    .save_chunk(
                        &folia_core::chunk_packet::flat_chunk(chunk_x, chunk_z),
                        folia_core::anvil::Compression::Zlib,
                    )
                    .unwrap();
            }
        }
        fixture.flush().unwrap();
    }
    folia_core::join::configure_world_dir(temp_world.clone());
    folia_core::join::configure_playerdata(temp_world.join("playerdata"));
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    client
        .set_write_timeout(Some(Duration::from_secs(10)))
        .expect("write timeout");

    write_packet(
        &mut client,
        &build_login_start("OverlayDig", Some([0x22; 16])).unwrap(),
    )
    .unwrap();
    read_until_packet(&mut client, 2); // Login Success
    write_packet(&mut client, &simple_packet(3)).unwrap();
    loop {
        if packet_id(&read_packet(&mut client).expect("config packet")) == 14 {
            break;
        }
    }
    write_packet(&mut client, &client_information()).unwrap();
    write_packet(&mut client, &empty_known_packs()).unwrap();
    loop {
        if packet_id(&read_packet(&mut client).expect("registry packet")) == 3 {
            break;
        }
    }
    write_packet(&mut client, &simple_packet(3)).unwrap();
    read_until_packet(&mut client, 70); // Synchronize Player Position
    let mut chunks = 0;
    while chunks < 169 {
        if packet_id(&read_packet(&mut client).expect("chunk packet")) == 44 {
            chunks += 1;
        }
    }

    let target = block_pos_long(1, -61, 0);
    write_packet(&mut client, &player_action(0, target)).expect("START_DESTROY_BLOCK");

    let mut stages = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let packet = read_packet(&mut client).expect("dig overlay packet");
        if packet_id(&packet) != BLOCK_DESTRUCTION {
            continue;
        }
        let (entity_id, position, stage) = read_overlay(&packet);
        assert_eq!(position, target, "overlay targets the dug block");
        assert!(entity_id > 0, "overlay is keyed by the digging player's id");
        if stage == CLEAR_STAGE {
            break;
        }
        assert!(
            (0..=9).contains(&stage),
            "visible stage in 0..=9, got {stage}"
        );
        stages.push(stage);
        if stages.len() >= 4 {
            break;
        }
    }
    assert!(!stages.is_empty(), "received at least one crack frame");
    assert_eq!(stages[0], 0, "the first frame is the initial crack stage");
    assert!(
        stages.iter().copied().max().unwrap_or(0) > 0,
        "the overlay progressed past the first frame: {stages:?}"
    );

    write_packet(&mut client, &player_action(2, target)).expect("STOP_DESTROY_BLOCK");
    let mut cleared = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let packet = read_packet(&mut client).expect("clear overlay packet");
        if packet_id(&packet) != BLOCK_DESTRUCTION {
            continue;
        }
        let (_, position, stage) = read_overlay(&packet);
        if position == target && stage == CLEAR_STAGE {
            cleared = true;
            break;
        }
    }
    assert!(cleared, "STOP removes the block-damage overlay");

    drop(client);
    server.join().expect("server thread").expect("join flow");
    fs::remove_dir_all(temp_world).expect("remove temp world");
}

fn read_overlay(packet: &[u8]) -> (i32, i64, i32) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), BLOCK_DESTRUCTION);
    let entity_id = reader.read_varint().unwrap();
    let position = reader.read_i64().unwrap();
    let stage = reader.read_u8().unwrap() as i8 as i32;
    assert!(reader.is_empty(), "no trailing bytes in block destruction");
    (entity_id, position, stage)
}

fn player_action(action: i32, position: i64) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(folia_core::block_update::SERVERBOUND_PLAYER_ACTION)
        .unwrap();
    writer.write_varint(action).unwrap();
    writer.write_i64(position).unwrap();
    writer.write_u8(1).unwrap(); // UP
    writer.write_varint(0).unwrap(); // sequence
    writer.into_inner()
}

fn packet_id(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().unwrap()
}

fn read_until_packet(client: &mut TcpStream, id: i32) {
    loop {
        let packet = read_packet(client).expect("expected packet");
        if packet_id(&packet) == id {
            return;
        }
    }
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
