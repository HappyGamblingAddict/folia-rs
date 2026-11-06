use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use folia_core::world::{Dimension, WorldStorage};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const JOIN_GAME: i32 = 48;
const GAME_EVENT: i32 = 38;
const CHUNK: i32 = 44;
const PLAYER_ABILITIES: i32 = 62;
const PLAYER_POSITION: i32 = 70;

/// The captured Join Game packet says creative + flat; the live one must say
/// the server's configured mode and neither the flat nor debug flags. A
/// client-requested change and `/gamemode` both update the mode and survive a
/// reconnect through playerdata.
#[test]
fn game_mode_changes_round_trip_through_join_and_playerdata() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-game-mode-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("region")).unwrap();
    {
        let mut fixture = WorldStorage::for_dimension(&root, Dimension::Overworld);
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
    folia_core::join::configure_world_dir(root.clone());
    folia_core::join::configure_playerdata(root.join("playerdata"));
    folia_core::join::configure_game_mode(0);
    folia_core::join::execute_console_command("op GameModeTest");

    let uuid = [0x77; 16];

    // First session: Join Game says survival, then a change-game-mode packet
    // switches to creative and saves it.
    let (game_type, mut client) = join(uuid);
    assert_eq!(game_type, 0, "Join Game uses the configured survival mode");
    drain_chunks(&mut client);

    write_packet(&mut client, &change_game_mode(1)).unwrap();
    let (event, abilities) = read_mode_change(&mut client);
    assert_eq!(event.event, 3, "Change Game Mode event");
    assert_eq!(event.param, 1.0, "creative");
    assert_eq!(abilities.flags & 0x0d, 0x0d, "creative abilities");
    drop(client);

    let saved = wait_for_saved_mode(&root, uuid, 1);
    assert_eq!(saved.game_mode, 1, "creative is persisted");

    // Second session: Join Game now reports creative. `/gamemode survival`
    // switches back, and the third session reports survival again.
    let (game_type, mut client) = join(uuid);
    assert_eq!(game_type, 1, "the saved game mode reaches Join Game");
    drain_chunks(&mut client);

    write_packet(&mut client, &chat_command("/gamemode survival")).unwrap();
    let (event, abilities) = read_mode_change(&mut client);
    assert_eq!(event.event, 3);
    assert_eq!(event.param, 0.0, "survival");
    assert_eq!(abilities.flags, 0, "survival has no creative abilities");
    drop(client);

    wait_for_saved_mode(&root, uuid, 0);
    let (game_type, client) = join(uuid);
    assert_eq!(game_type, 0, "the command result persists");
    drop(client);
    std::fs::remove_dir_all(root).ok();
}

/// Run one login through configuration and return the Join Game game mode plus
/// the connected stream, positioned before the first chunk packet.
fn join(uuid: [u8; 16]) -> (i32, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("server address");
    let handshake = Handshake {
        protocol_version: 774,
        server_address: "localhost".into(),
        server_port: address.port(),
        next_state: NextState::Login,
    };
    let _server = std::thread::spawn(move || -> io::Result<()> {
        let (stream, _) = listener.accept()?;
        handle_login(stream, &handshake)
    });

    let mut client = TcpStream::connect(address).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    client
        .set_write_timeout(Some(Duration::from_secs(15)))
        .expect("write timeout");
    write_packet(
        &mut client,
        &build_login_start("GameModeTest", Some(uuid)).unwrap(),
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

    let join_game = read_packet(&mut client).expect("join game");
    assert_eq!(packet_id(&join_game), JOIN_GAME);
    let game_type = join_game_mode(&join_game);
    loop {
        let packet = read_packet(&mut client).expect("position");
        if packet_id(&packet) == PLAYER_POSITION {
            break;
        }
    }

    // The session thread exits when the client drops; the handle is detached.
    (game_type, client)
}

fn drain_chunks(client: &mut TcpStream) {
    let mut chunks = 0;
    while chunks < 25 {
        let packet = read_packet(client).expect("chunk");
        if packet_id(&packet) == CHUNK {
            chunks += 1;
        }
    }
}

#[derive(Debug)]
struct GameEvent {
    event: u8,
    param: f32,
}

#[derive(Debug)]
struct Abilities {
    flags: u8,
}

fn read_mode_change(client: &mut TcpStream) -> (GameEvent, Abilities) {
    let mut event = None;
    let mut abilities = None;
    while event.is_none() || abilities.is_none() {
        let packet = read_packet(client).expect("mode change packet");
        match packet_id(&packet) {
            GAME_EVENT => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint().unwrap();
                let kind = reader.read_u8().unwrap();
                let param = reader.read_f32().unwrap();
                event = Some(GameEvent { event: kind, param });
            }
            PLAYER_ABILITIES => {
                let mut reader = PacketReader::new(&packet);
                let _ = reader.read_varint().unwrap();
                let flags = reader.read_u8().unwrap();
                let _ = reader.read_f32().unwrap();
                let _ = reader.read_f32().unwrap();
                abilities = Some(Abilities { flags });
            }
            _ => {}
        }
    }
    (event.unwrap(), abilities.unwrap())
}

fn join_game_mode(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), JOIN_GAME);
    let _ = reader.read_i32().unwrap();
    let _ = reader.read_bool().unwrap();
    let levels = reader.read_varint().unwrap();
    for _ in 0..levels {
        let _ = reader.read_string().unwrap();
    }
    let _ = reader.read_varint().unwrap();
    let _ = reader.read_varint().unwrap();
    let _ = reader.read_varint().unwrap();
    let _ = (
        reader.read_bool().unwrap(),
        reader.read_bool().unwrap(),
        reader.read_bool().unwrap(),
    );
    let _ = reader.read_varint().unwrap();
    let _ = reader.read_string().unwrap();
    let _ = reader.read_i64().unwrap();
    let game_type = reader.read_u8().unwrap() as i32;
    assert_eq!(reader.read_u8().unwrap(), 0xff, "no previous mode");
    assert!(!reader.read_bool().unwrap(), "not debug");
    assert!(!reader.read_bool().unwrap(), "not flat");
    game_type
}

fn change_game_mode(mode: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(4).unwrap();
    writer.write_varint(mode).unwrap();
    writer.into_inner()
}

fn chat_command(command: &str) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(6).unwrap();
    writer.write_string(command).unwrap();
    writer.into_inner()
}

fn wait_for_saved_mode(
    root: &std::path::Path,
    uuid: [u8; 16],
    expected: i32,
) -> folia_core::player::PlayerSession {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(session)) =
            folia_core::player::PlayerSession::load(root.join("playerdata"), uuid)
            && session.game_mode == expected
        {
            return session;
        }
        if std::time::Instant::now() >= deadline {
            panic!("playerdata did not persist game mode {expected} on disconnect");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
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
