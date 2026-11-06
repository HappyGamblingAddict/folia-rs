use folia_core::join::{handle_login, shutdown_sessions};
use folia_core::player::uuid_string;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// An idle player is asked to leave on shutdown: the session saves its
/// playerdata, flushes terrain, and the login thread returns before the
/// timeout.
#[test]
fn shutdown_closes_an_idle_session_and_saves_playerdata() {
    let root = std::env::temp_dir().join(format!(
        "folia-rs-shutdown-session-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let playerdata = root.join("playerdata");
    std::fs::create_dir_all(&playerdata).unwrap();
    folia_core::join::configure_world_dir(root.clone());
    folia_core::join::configure_playerdata(playerdata.clone());
    folia_core::worldgen::generator::configure(99).expect("configure generator");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    let handshake = Handshake {
        protocol_version: 774,
        server_address: "localhost".into(),
        server_port: address.port(),
        next_state: NextState::Login,
    };
    let uuid = [0x77; 16];

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
        &build_login_start("ShutdownTest", Some(uuid)).unwrap(),
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

    // Read until the position packet: joining finished and the session is now
    // idle in its play loop.
    loop {
        let packet = read_packet(&mut client).expect("play packet");
        if packet_id(&packet) == 70 {
            break;
        }
    }

    let started = Instant::now();
    shutdown_sessions(Duration::from_secs(3));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the idle session exits before the timeout"
    );
    let result = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("session thread finishes");
    assert!(result.is_ok(), "session ends cleanly: {result:?}");
    server.join().expect("server thread");

    let saved = playerdata.join(format!("{}.dat", uuid_string(&uuid)));
    assert!(saved.is_file(), "shutdown saved {}", saved.display());
    assert!(
        std::fs::metadata(&saved).unwrap().len() > 0,
        "playerdata is not empty"
    );
    std::fs::remove_dir_all(root).ok();
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
