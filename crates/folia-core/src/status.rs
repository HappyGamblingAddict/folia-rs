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
    /// Server-list favicon as a `data:image/png;base64,...` URI, when present.
    pub favicon: Option<String>,
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
            favicon: None,
        }
    }
}

pub fn status_json(config: &StatusConfig) -> String {
    let mut value = json!({
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
    });
    if let Some(favicon) = &config.favicon {
        value["favicon"] = json!(favicon);
    }
    value.to_string()
}

/// The first eight bytes of every PNG file, per the PNG specification.
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode raw bytes as standard padded base64. This is a small private helper
/// so the crate does not need a new dependency for one fixed-size use case.
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut full = input.chunks_exact(3);
    for chunk in &mut full {
        let (a, b, c) = (chunk[0], chunk[1], chunk[2]);
        out.push(BASE64_ALPHABET[(a >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        out.push(BASE64_ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        out.push(BASE64_ALPHABET[(c & 0x3f) as usize] as char);
    }
    match full.remainder() {
        [a] => {
            out.push(BASE64_ALPHABET[(a >> 2) as usize] as char);
            out.push(BASE64_ALPHABET[((a & 0x03) << 4) as usize] as char);
            out.push('=');
            out.push('=');
        }
        [a, b] => {
            out.push(BASE64_ALPHABET[(a >> 2) as usize] as char);
            out.push(BASE64_ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
            out.push(BASE64_ALPHABET[((b & 0x0f) << 2) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Load an optional server-list favicon from `path`, which is conventionally
/// `server-icon.png` in the server root. A missing file is not an error; an
/// existing file that is not a PNG is, so a misconfigured icon never silently
/// advertises garbage.
pub fn load_favicon(path: impl AsRef<std::path::Path>) -> io::Result<Option<String>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("favicon {} is not a PNG (bad signature)", path.display()),
        ));
    }
    Ok(Some(format!(
        "data:image/png;base64,{}",
        base64_encode(&bytes)
    )))
}

/// Serve Minecraft status pings from the given listener. This function blocks;
/// the server binary runs it on a dedicated thread.
pub fn serve(listener: TcpListener, config: Arc<StatusConfig>) -> io::Result<()> {
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &config)
                    && !is_disconnect(&error)
                {
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

/// Server-list pings are often opened and closed by clients, monitors, and
/// port scanners without completing a handshake. Those are not server errors.
fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::TimedOut
    )
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
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("folia-status-{label}-{}-{n}", std::process::id()))
    }

    #[test]
    fn status_json_contains_protocol_and_motd() {
        let config = StatusConfig::default();
        let value: serde_json::Value = serde_json::from_str(&status_json(&config)).unwrap();
        assert_eq!(value["version"]["protocol"], 774);
        assert_eq!(value["version"]["name"], "1.21.11");
        assert_eq!(value["description"]["text"], "Folia-RS");
        assert_eq!(value["players"]["max"], 20);
    }

    #[test]
    fn status_json_omits_favicon_when_none() {
        let config = StatusConfig::default();
        assert!(config.favicon.is_none());
        let value: serde_json::Value = serde_json::from_str(&status_json(&config)).unwrap();
        assert!(value.get("favicon").is_none());
    }

    #[test]
    fn status_json_includes_exact_favicon_when_present() {
        let uri = "data:image/png;base64,iVBORw0KGgo=".to_owned();
        let config = StatusConfig {
            favicon: Some(uri.clone()),
            ..StatusConfig::default()
        };
        let value: serde_json::Value = serde_json::from_str(&status_json(&config)).unwrap();
        assert_eq!(value["favicon"], uri);
        assert_eq!(value["version"]["name"], "1.21.11");
        assert_eq!(value["version"]["protocol"], 774);
    }

    #[test]
    fn base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&PNG_SIGNATURE), "iVBORw0KGgo=");
    }

    #[test]
    fn load_favicon_missing_path_returns_none() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(load_favicon(&path).unwrap().is_none());
    }

    #[test]
    fn load_favicon_rejects_non_png() {
        let path = temp_path("not-png");
        std::fs::write(&path, b"this is definitely not a PNG").unwrap();
        let error = load_favicon(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("not a PNG"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_favicon_encodes_minimal_png_header() {
        let path = temp_path("png");
        std::fs::write(&path, PNG_SIGNATURE).unwrap();
        assert_eq!(
            load_favicon(&path).unwrap(),
            Some("data:image/png;base64,iVBORw0KGgo=".to_owned())
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn status_probe_disconnects_are_not_server_errors() {
        for kind in [
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert!(is_disconnect(&io::Error::from(kind)), "{kind:?}");
        }
        assert!(!is_disconnect(&io::Error::from(io::ErrorKind::InvalidData)));
    }
}
