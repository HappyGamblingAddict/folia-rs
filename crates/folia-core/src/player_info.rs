//! Clientbound player-list (tab-list) info packets for protocol 774 (1.21.11).
//!
//! This slice only maintains tab-list entries (name, skin properties, game
//! mode). Player entities and world visibility are handled separately; see
//! `entity.rs` and `join.rs`.

use crate::protocol::{PacketWriter, ProfileProperty};

/// `ClientboundPlayerInfoUpdatePacket` play id.
pub const PLAYER_INFO_UPDATE: i32 = 68;
/// `ClientboundPlayerInfoRemovePacket` play id.
pub const PLAYER_INFO_REMOVE: i32 = 67;

/// Action bit masks, in enum (bit) order.
pub const ACTION_ADD_PLAYER: u8 = 0x01;
pub const ACTION_INITIALIZE_CHAT: u8 = 0x02;
pub const ACTION_UPDATE_GAMEMODE: u8 = 0x04;
pub const ACTION_UPDATE_LISTED: u8 = 0x08;
pub const ACTION_UPDATE_LATENCY: u8 = 0x10;
pub const ACTION_UPDATE_DISPLAY_NAME: u8 = 0x20;
pub const ACTION_UPDATE_LIST_ORDER: u8 = 0x40;
pub const ACTION_UPDATE_HAT: u8 = 0x80;

/// Mask used by [`player_info_add_packet`]: ADD_PLAYER | UPDATE_GAMEMODE |
/// UPDATE_LISTED | UPDATE_LATENCY (0x1D).
pub const ADD_PLAYER_MASK: u8 =
    ACTION_ADD_PLAYER | ACTION_UPDATE_GAMEMODE | ACTION_UPDATE_LISTED | ACTION_UPDATE_LATENCY;

/// Build a `ClientboundPlayerInfoUpdatePacket` that adds one listed player with
/// zero latency.
///
/// Layout (single entry): id, actions mask, entry count, UUID, then the entry's
/// action payloads in bit order: name, property count and properties (ADD_PLAYER),
/// game mode (UPDATE_GAMEMODE), listed flag (UPDATE_LISTED), latency
/// (UPDATE_LATENCY).
pub fn player_info_add_packet(
    uuid: [u8; 16],
    name: &str,
    properties: &[ProfileProperty],
    game_mode: i32,
) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAYER_INFO_UPDATE).unwrap();
    writer.write_u8(ADD_PLAYER_MASK).unwrap();
    writer.write_varint(1).unwrap();
    writer.write_bytes(&uuid);

    writer.write_string(name).unwrap();
    writer.write_varint(properties.len() as i32).unwrap();
    for property in properties {
        writer.write_string(&property.name).unwrap();
        writer.write_string(&property.value).unwrap();
        match &property.signature {
            Some(signature) => {
                writer.write_u8(1).unwrap();
                writer.write_string(signature).unwrap();
            }
            None => writer.write_u8(0).unwrap(),
        }
    }

    writer.write_varint(game_mode).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}

/// Build a `ClientboundPlayerInfoRemovePacket` for one UUID.
pub fn player_info_remove_packet(uuid: [u8; 16]) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(PLAYER_INFO_REMOVE).unwrap();
    writer.write_varint(1).unwrap();
    writer.write_bytes(&uuid);
    writer.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketReader;

    fn sample_properties() -> Vec<ProfileProperty> {
        vec![
            ProfileProperty::new("textures", "base64-value"),
            ProfileProperty {
                name: "signed".to_owned(),
                value: "signed-value".to_owned(),
                signature: Some("signature-value".to_owned()),
            },
        ]
    }

    #[test]
    fn add_packet_decodes_all_actions() {
        let uuid = [0x42; 16];
        let name = "TabPlayer";
        let properties = sample_properties();
        let game_mode = 1;

        let packet = player_info_add_packet(uuid, name, &properties, game_mode);
        let mut reader = PacketReader::new(&packet);

        assert_eq!(reader.read_varint().unwrap(), PLAYER_INFO_UPDATE);
        assert_eq!(reader.read_u8().unwrap(), ADD_PLAYER_MASK);
        assert_eq!(ADD_PLAYER_MASK, 0x1D);
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_bytes(16).unwrap(), uuid);

        assert_eq!(reader.read_string().unwrap(), name);
        let count = reader.read_varint().unwrap();
        assert_eq!(count, properties.len() as i32);
        for expected in &properties {
            assert_eq!(reader.read_string().unwrap(), expected.name);
            assert_eq!(reader.read_string().unwrap(), expected.value);
            match &expected.signature {
                Some(signature) => {
                    assert!(reader.read_bool().unwrap());
                    assert_eq!(reader.read_string().unwrap(), *signature);
                }
                None => assert!(!reader.read_bool().unwrap()),
            }
        }

        assert_eq!(reader.read_varint().unwrap(), game_mode);
        assert!(reader.read_bool().unwrap(), "listed");
        assert_eq!(reader.read_varint().unwrap(), 0, "latency");
        assert!(reader.is_empty());
    }

    #[test]
    fn add_packet_with_no_properties_decodes() {
        let uuid = [0x07; 16];
        let packet = player_info_add_packet(uuid, "Solo", &[], 0);
        let mut reader = PacketReader::new(&packet);

        assert_eq!(reader.read_varint().unwrap(), PLAYER_INFO_UPDATE);
        assert_eq!(reader.read_u8().unwrap(), 0x1D);
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_bytes(16).unwrap(), uuid);
        assert_eq!(reader.read_string().unwrap(), "Solo");
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert!(reader.read_bool().unwrap());
        assert_eq!(reader.read_varint().unwrap(), 0);
        assert!(reader.is_empty());
    }

    #[test]
    fn remove_packet_decodes() {
        let uuid = [0xAB; 16];
        let packet = player_info_remove_packet(uuid);
        let mut reader = PacketReader::new(&packet);

        assert_eq!(reader.read_varint().unwrap(), PLAYER_INFO_REMOVE);
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_bytes(16).unwrap(), uuid);
        assert!(reader.is_empty());
    }

    #[test]
    fn action_masks_are_in_enum_order() {
        assert_eq!(ACTION_ADD_PLAYER, 0x01);
        assert_eq!(ACTION_INITIALIZE_CHAT, 0x02);
        assert_eq!(ACTION_UPDATE_GAMEMODE, 0x04);
        assert_eq!(ACTION_UPDATE_LISTED, 0x08);
        assert_eq!(ACTION_UPDATE_LATENCY, 0x10);
        assert_eq!(ACTION_UPDATE_DISPLAY_NAME, 0x20);
        assert_eq!(ACTION_UPDATE_LIST_ORDER, 0x40);
        assert_eq!(ACTION_UPDATE_HAT, 0x80);
    }
}
