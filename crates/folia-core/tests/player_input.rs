use folia_core::protocol::{
    PLAYER_INPUT_BACKWARD, PLAYER_INPUT_FORWARD, PLAYER_INPUT_JUMP, PLAYER_INPUT_LEFT,
    PLAYER_INPUT_MASK, PLAYER_INPUT_RIGHT, PLAYER_INPUT_SNEAK, PLAYER_INPUT_SPRINT,
    SERVERBOUND_CHAT, SERVERBOUND_PLAYER_INPUT, ServerboundPlayerInput,
    build_serverbound_player_input, parse_serverbound_player_input,
};

#[test]
fn player_input_round_trips() {
    for flags in [
        0x00u8,
        PLAYER_INPUT_SPRINT,
        PLAYER_INPUT_SPRINT | PLAYER_INPUT_SNEAK,
        PLAYER_INPUT_MASK,
    ] {
        let input = ServerboundPlayerInput { flags };
        let packet = build_serverbound_player_input(&input).expect("build");
        let parsed = parse_serverbound_player_input(&packet).expect("parse");
        assert_eq!(parsed, input);
    }
    assert_eq!(PLAYER_INPUT_FORWARD, 0x01);
    assert_eq!(PLAYER_INPUT_BACKWARD, 0x02);
    assert_eq!(PLAYER_INPUT_LEFT, 0x04);
    assert_eq!(PLAYER_INPUT_RIGHT, 0x08);
    assert_eq!(PLAYER_INPUT_JUMP, 0x10);
    assert_eq!(PLAYER_INPUT_SNEAK, 0x20);
    assert_eq!(PLAYER_INPUT_SPRINT, 0x40);
    assert_eq!(PLAYER_INPUT_MASK, 0x7F);
}

#[test]
fn player_input_rejects_wrong_id() {
    let packet = build_serverbound_player_input(&ServerboundPlayerInput { flags: 0 }).unwrap();
    let mut wrong = packet;
    wrong[0] = SERVERBOUND_CHAT as u8;
    assert!(parse_serverbound_player_input(&wrong).is_err());
}

#[test]
fn player_input_rejects_unknown_bit() {
    let mut packet = Vec::new();
    folia_core::protocol::PacketWriter::new();
    // build manually: id 42 varint + flags 0x80
    packet.push(SERVERBOUND_PLAYER_INPUT as u8);
    packet.push(0x80);
    assert!(parse_serverbound_player_input(&packet).is_err());
    assert!(build_serverbound_player_input(&ServerboundPlayerInput { flags: 0x80 }).is_err());
}

#[test]
fn player_input_rejects_trailing_byte() {
    let mut packet =
        build_serverbound_player_input(&ServerboundPlayerInput { flags: 0x40 }).unwrap();
    packet.push(0);
    assert!(parse_serverbound_player_input(&packet).is_err());
}
