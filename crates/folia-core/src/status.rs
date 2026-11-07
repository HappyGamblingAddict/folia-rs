use crate::protocol::{
    NextState, PacketWriter, build_pong, build_status_response, parse_handshake, parse_ping,
    read_packet, write_packet,
};
use serde_json::json;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct StatusConfig {
    pub version_name: String,
    pub protocol_version: i32,
    pub motd: String,
    pub max_players: i32,
    pub online_players: i32,
    /// When true, login attempts are handed to the captured 1.21.11 join flow.
    pub accept_logins: bool,
    /// Advertised as `enforcesSecureChat` in the status JSON.
    pub enforces_secure_chat: bool,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            version_name: "1.21.11".into(),
            protocol_version: 774,
            motd: "Folia-RS".into(),
            max_players: 20,
            online_players: 0,
            accept_logins: false,
            enforces_secure_chat: false,
        }
    }
}

pub fn status_json(config: &StatusConfig) -> String {
    json!({
        "version": {
            "name": config.version_name,
            "protocol": config.protocol_version,
        },
        "players": {
            "max": config.max_players,
            "online": config.online_players,
            "sample": [],
        },
        "description": {
            "text": config.motd,
        },
        "enforcesSecureChat": config.enforces_secure_chat,
        "previewsChat": false,
    })
    .to_string()
}

/// Serve Minecraft status pings from the given listener. This function blocks;
/// the server binary runs it on a dedicated thread.
pub fn serve(listener: TcpListener, config: Arc<StatusConfig>) -> io::Result<()> {
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &config) {
                    eprintln!("[status] connection error: {error}");
                }
            }
            Err(error) => {
                eprintln!("[status] accept error: {error}");
            }
        }
    }
    Ok(())
}

pub fn serve_addr(address: &str, config: Arc<StatusConfig>) -> io::Result<()> {
    let listener = TcpListener::bind(address)?;
    serve(listener, config)
}

fn handle_connection(mut stream: TcpStream, config: &StatusConfig) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut first_byte = [0u8; 1];
    stream.peek(&mut first_byte)?;
    if first_byte[0] == 0xFE {
        return handle_legacy_ping(&mut stream, config);
    }

    let handshake_payload = read_packet(&mut stream)?;
    let handshake = parse_handshake(&handshake_payload)?;

    match handshake.next_state {
        NextState::Status => handle_status(&mut stream, config),
        NextState::Login | NextState::Transfer => {
            if config.accept_logins {
                return crate::join::handle_login(stream, &handshake);
            }
            // Read Login Start before rejecting, like the vanilla login flow.
            // This also prevents a close-vs-write race with the client.
            let _ = read_packet(&mut stream);
            let mut writer = PacketWriter::new();
            writer.write_varint(0)?;
            writer.write_string(
                r#"{"text":"Folia-RS login is not available. Set online-mode=false to use the captured offline world."}"#,
            )?;
            write_packet(&mut stream, &writer.into_inner())
        }
    }
}

/// Pre-1.7 server list ping: `0xFE` followed by a UTF-16BE string prefixed by
/// `0xFF`. Modern clients never take this path.
fn handle_legacy_ping(stream: &mut TcpStream, config: &StatusConfig) -> io::Result<()> {
    let mut request = [0u8; 2];
    let _ = stream.read(&mut request);

    let payload = format!(
        "\u{00a7}1\u{0000}{}\u{0000}{}\u{0000}{}\u{0000}{}\u{0000}{}",
        config.protocol_version,
        config.version_name,
        config.motd,
        config.online_players,
        config.max_players
    );

    let mut response = vec![0xFF];
    for unit in payload.encode_utf16() {
        response.extend_from_slice(&unit.to_be_bytes());
    }
    stream.write_all(&response)
}

fn handle_status(stream: &mut TcpStream, config: &StatusConfig) -> io::Result<()> {
    let request = read_packet(stream)?;
    if request != [0x00] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a status request packet",
        ));
    }

    let response = build_status_response(&status_json(config))?;
    write_packet(stream, &response)?;

    let ping = read_packet(stream)?;
    let payload = parse_ping(&ping)?;
    write_packet(stream, &build_pong(payload))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_contains_protocol_and_motd() {
        let config = StatusConfig::default();
        let value: serde_json::Value = serde_json::from_str(&status_json(&config)).unwrap();
        assert_eq!(value["version"]["protocol"], 774);
        assert_eq!(value["version"]["name"], "1.21.11");
        assert_eq!(value["description"]["text"], "Folia-RS");
        assert_eq!(value["players"]["max"], 20);
    }
}
