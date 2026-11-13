use folia_core::protocol::{
    ServerboundUseItem, build_serverbound_use_item, parse_serverbound_use_item,
};

#[test]
fn use_item_round_trips() {
    for hand in [0, 1] {
        let use_item = ServerboundUseItem {
            hand,
            sequence: 42,
            yaw: 12.5,
            pitch: -3.25,
        };
        let packet = build_serverbound_use_item(&use_item).unwrap();
        let parsed = parse_serverbound_use_item(&packet).unwrap();
        assert_eq!(parsed, use_item);
    }
}

#[test]
fn use_item_rejects_wrong_id() {
    let mut packet = build_serverbound_use_item(&ServerboundUseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    })
    .unwrap();
    packet[0] = 65;
    assert!(parse_serverbound_use_item(&packet).is_err());
}

#[test]
fn use_item_rejects_invalid_hand() {
    for hand in [2, -1] {
        let mut use_item = ServerboundUseItem {
            hand: 0,
            sequence: 1,
            yaw: 0.0,
            pitch: 0.0,
        };
        use_item.hand = hand;
        assert!(build_serverbound_use_item(&use_item).is_err());
    }
}

#[test]
fn use_item_rejects_trailing_byte() {
    let mut packet = build_serverbound_use_item(&ServerboundUseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    })
    .unwrap();
    packet.push(0);
    assert!(parse_serverbound_use_item(&packet).is_err());
}
