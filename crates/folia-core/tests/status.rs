use folia_core::protocol::{
    NextState, PacketReader, build_handshake, build_login_start, build_ping, build_status_request,
    parse_ping, parse_status_response, read_packet, write_packet,
};
use folia_core::status::{StatusConfig, serve};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn status_server_responds_to_status_and_ping() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let address = listener.local_addr().expect("listener address");
    let config = Arc::new(StatusConfig::default());

    std::thread::spawn(move || {
        let _ = serve(listener, config);
    });

    let mut stream = TcpStream::connect(address).expect("connect to status server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .expect("write timeout");

    let handshake = build_handshake(774, "localhost", address.port(), NextState::Status)
        .expect("build handshake");
    write_packet(&mut stream, &handshake).expect("write handshake");
    write_packet(&mut stream, &build_status_request()).expect("write status request");

    let response = read_packet(&mut stream).expect("read status response");
    let json = parse_status_response(&response).expect("parse status response");
    let value: serde_json::Value = serde_json::from_str(&json).expect("status JSON");
    assert_eq!(value["version"]["protocol"], 774);
    assert_eq!(value["version"]["name"], "1.21.11");
    assert_eq!(value["description"]["text"], "Folia-RS");
    assert_eq!(value["players"]["max"], 20);
    assert_eq!(value["players"]["online"], 0);

    write_packet(&mut stream, &build_ping(0x1122_3344_5566_7788)).expect("write ping");
    stream.flush().expect("flush ping");
    let pong = read_packet(&mut stream).expect("read pong");
    assert_eq!(
        parse_ping(&pong).expect("parse pong"),
        0x1122_3344_5566_7788
    );
}

#[test]
fn status_server_supports_legacy_ping() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let address = listener.local_addr().expect("listener address");
    let config = Arc::new(StatusConfig::default());

    std::thread::spawn(move || {
        let _ = serve(listener, config);
    });

    let mut stream = TcpStream::connect(address).expect("connect to status server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream.write_all(&[0xFE, 0x01]).expect("write legacy ping");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read legacy response");
    assert_eq!(response.first(), Some(&0xFF));

    let units: Vec<u16> = response[1..]
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    let text = String::from_utf16(&units).expect("UTF-16 response");
    assert!(text.starts_with('\u{00a7}'));
    assert!(text.contains("1.21.11"));
    assert!(text.contains("Folia-RS"));
}

#[test]
fn login_attempt_gets_an_explicit_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let address = listener.local_addr().expect("listener address");
    let config = Arc::new(StatusConfig::default());

    std::thread::spawn(move || {
        let _ = serve(listener, config);
    });

    let mut stream = TcpStream::connect(address).expect("connect to status server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");

    let handshake = build_handshake(774, "localhost", address.port(), NextState::Login)
        .expect("build login handshake");
    write_packet(&mut stream, &handshake).expect("write handshake");
    write_packet(&mut stream, &build_login_start("Tester", None).unwrap())
        .expect("write login start");

    let response = read_packet(&mut stream).expect("read login disconnect");
    let mut reader = PacketReader::new(&response);
    assert_eq!(reader.read_varint().unwrap(), 0);
    let reason = reader.read_string().unwrap();
    assert!(reason.contains("login is not available"));
}
