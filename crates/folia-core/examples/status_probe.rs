use folia_core::protocol::{
    NextState, build_handshake, build_ping, build_status_request, parse_ping,
    parse_status_response, read_packet, write_packet,
};
use std::io;
use std::net::TcpStream;
use std::time::Duration;

fn main() -> io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:25565".to_owned());
    let port = address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(25565);

    let mut stream = TcpStream::connect(&address)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let handshake = build_handshake(774, "localhost", port, NextState::Status)?;
    write_packet(&mut stream, &handshake)?;
    write_packet(&mut stream, &build_status_request())?;

    let response = read_packet(&mut stream)?;
    println!("{}", parse_status_response(&response)?);

    write_packet(&mut stream, &build_ping(42))?;
    let pong = read_packet(&mut stream)?;
    println!("pong={}", parse_ping(&pong)?);
    Ok(())
}
