use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// Java `.properties` parser for `server.properties`.
///
/// Minecraft's file follows `java.util.Properties`: `=`, `:` or whitespace
/// separates key and value; `#`/`!` start comments; backslash escapes and
/// continuation lines are supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProperties {
    pub server_ip: String,
    pub server_port: u16,
    pub motd: String,
    pub max_players: i32,
    pub online_mode: bool,
    pub level_name: String,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub enable_command_block: bool,
    pub allow_flight: bool,
    pub hardcore: bool,
    pub pvp: bool,
    pub white_list: bool,
    pub difficulty: String,
    pub gamemode: String,
    pub spawn_protection: i32,
    pub extra: BTreeMap<String, String>,
}

impl Default for ServerProperties {
    fn default() -> Self {
        Self {
            server_ip: String::new(),
            server_port: 25565,
            motd: "A Minecraft Server".into(),
            max_players: 20,
            online_mode: true,
            level_name: "world".into(),
            view_distance: 10,
            simulation_distance: 10,
            enable_command_block: false,
            allow_flight: false,
            hardcore: false,
            pvp: true,
            white_list: false,
            difficulty: "easy".into(),
            gamemode: "survival".into(),
            spawn_protection: 16,
            extra: BTreeMap::new(),
        }
    }
}

impl ServerProperties {
    pub fn parse(input: &str) -> Self {
        let mut properties = Self::default();
        for line in logical_lines(input) {
            let line = line.trim_start();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }

            let (raw_key, raw_value) = split_key_value(line);
            let key = unescape(raw_key);
            let value = unescape(raw_value);
            properties.apply(&key, value);
        }
        properties
    }

    pub fn read(path: impl AsRef<Path>) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::parse(&text))
    }

    fn apply(&mut self, key: &str, value: String) {
        match key {
            "server-ip" => self.server_ip = value,
            "server-port" => self.server_port = parse_u16(&value, 25565),
            "motd" => self.motd = value,
            "max-players" => self.max_players = parse_i32(&value, 20),
            "online-mode" => self.online_mode = parse_bool(&value),
            "level-name" => self.level_name = value,
            "view-distance" => self.view_distance = parse_i32(&value, 10),
            "simulation-distance" => self.simulation_distance = parse_i32(&value, 10),
            "enable-command-block" => self.enable_command_block = parse_bool(&value),
            "allow-flight" => self.allow_flight = parse_bool(&value),
            "hardcore" => self.hardcore = parse_bool(&value),
            "pvp" => self.pvp = parse_bool(&value),
            "white-list" => self.white_list = parse_bool(&value),
            "difficulty" => self.difficulty = value,
            "gamemode" => self.gamemode = value,
            "spawn-protection" => self.spawn_protection = parse_i32(&value, 16),
            other => {
                self.extra.insert(other.to_owned(), value);
            }
        }
    }
}

fn split_key_value(line: &str) -> (&str, &str) {
    let mut escaped = false;
    let mut separator = None;

    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '=' || character == ':' || character.is_whitespace() {
            separator = Some(index);
            break;
        }
    }

    let Some(index) = separator else {
        return (line, "");
    };

    let key = &line[..index];
    let mut rest = &line[index..];
    rest = rest.trim_start();
    if let Some(stripped) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) {
        rest = stripped.trim_start();
    }
    (key, rest)
}

fn logical_lines(input: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for raw in input.lines() {
        let line = raw.trim_start();
        if current.is_empty() {
            current.push_str(line);
        } else {
            current.push_str(line.trim_start());
        }

        if has_odd_trailing_backslash(&current) {
            current.pop();
            continue;
        }

        lines.push(std::mem::take(&mut current));
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn has_odd_trailing_backslash(value: &str) -> bool {
    let mut count = 0;
    for character in value.chars().rev() {
        if character == '\\' {
            count += 1;
        } else {
            break;
        }
    }
    count % 2 == 1
}

fn unescape(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut characters = value.chars();

    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }

        match characters.next() {
            Some('t') => result.push('\t'),
            Some('n') => result.push('\n'),
            Some('r') => result.push('\r'),
            Some('f') => result.push('\u{000c}'),
            Some('u') => {
                let mut digits = String::with_capacity(4);
                for _ in 0..4 {
                    match characters.next() {
                        Some(digit) if digit.is_ascii_hexdigit() => digits.push(digit),
                        Some(digit) => {
                            result.push_str("\\u");
                            result.push_str(&digits);
                            result.push(digit);
                            digits.clear();
                            break;
                        }
                        None => break,
                    }
                }
                if digits.len() == 4 {
                    if let Ok(code) = u16::from_str_radix(&digits, 16) {
                        result.push(char::from_u32(u32::from(code)).unwrap_or('\u{fffd}'));
                    }
                }
            }
            Some(other) => result.push(other),
            None => result.push('\\'),
        }
    }

    result
}

fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

fn parse_i32(value: &str, fallback: i32) -> i32 {
    value.trim().parse().unwrap_or(fallback)
}

fn parse_u16(value: &str, fallback: u16) -> u16 {
    value.trim().parse().unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_server_properties() {
        let properties = ServerProperties::parse(
            "\
# Minecraft server properties
server-ip=127.0.0.1
server-port=25566
motd=Rust \\u00a7aFolia
max-players=100
online-mode=false
level-name=rust-world
view-distance=12
simulation-distance=8
enable-command-block=true
allow-flight=true
hardcore=true
pvp=false
white-list=true
difficulty=hard
gamemode=creative
spawn-protection=0
unknown-key=kept
",
        );

        assert_eq!(properties.server_ip, "127.0.0.1");
        assert_eq!(properties.server_port, 25566);
        assert_eq!(properties.motd, "Rust §aFolia");
        assert_eq!(properties.max_players, 100);
        assert!(!properties.online_mode);
        assert_eq!(properties.level_name, "rust-world");
        assert_eq!(properties.view_distance, 12);
        assert_eq!(properties.simulation_distance, 8);
        assert!(properties.enable_command_block);
        assert!(properties.allow_flight);
        assert!(properties.hardcore);
        assert!(!properties.pvp);
        assert!(properties.white_list);
        assert_eq!(properties.difficulty, "hard");
        assert_eq!(properties.gamemode, "creative");
        assert_eq!(properties.spawn_protection, 0);
        assert_eq!(
            properties.extra.get("unknown-key").map(String::as_str),
            Some("kept")
        );
    }

    #[test]
    fn supports_colon_and_whitespace_separators() {
        let properties =
            ServerProperties::parse("server-port:25570\nmotd Rust Server\nmax-players 30\n");
        assert_eq!(properties.server_port, 25570);
        assert_eq!(properties.motd, "Rust Server");
        assert_eq!(properties.max_players, 30);
    }

    #[test]
    fn supports_escaped_keys_and_continuations() {
        let properties = ServerProperties::parse(
            "motd=line one \\\n     line two\nkey\\:with\\=separators=value\n",
        );
        assert_eq!(properties.motd, "line one line two");
        assert_eq!(
            properties
                .extra
                .get("key:with=separators")
                .map(String::as_str),
            Some("value")
        );
    }

    #[test]
    fn defaults_are_stable() {
        let properties = ServerProperties::parse("");
        assert_eq!(properties, ServerProperties::default());
        assert_eq!(properties.server_port, 25565);
        assert!(properties.online_mode);
        assert_eq!(properties.max_players, 20);
    }

    #[test]
    fn invalid_numbers_use_defaults() {
        let properties = ServerProperties::parse("server-port=nope\nmax-players=?\n");
        assert_eq!(properties.server_port, 25565);
        assert_eq!(properties.max_players, 20);
    }
}
