use folia_core::inventory::ItemStack;
use folia_core::join::{damage_player, register_player_entity};
use folia_core::player::PlayerSession;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("folia-armor-{name}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn session_with_uuid(uuid: [u8; 16]) -> PlayerSession {
    PlayerSession::new("ArmorTest".to_string(), uuid, 0.0, 64.0, 0.0)
}

#[test]
fn playerdata_round_trips_armor_slots() {
    let dir = temp_dir("roundtrip");
    let uuid = [0xA1; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[0] = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    session.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    session.save(&dir).unwrap();

    let loaded = PlayerSession::load(&dir, uuid).unwrap().unwrap();
    assert_eq!(loaded.armor[0].item, "minecraft:diamond_boots");
    assert_eq!(loaded.armor[1].item, "minecraft:diamond_leggings");
    assert_eq!(loaded.armor[2].item, "minecraft:diamond_chestplate");
    assert_eq!(loaded.armor[3].item, "minecraft:diamond_helmet");
    assert_eq!(loaded.armor[0].count, 1);
    assert_eq!(loaded.armor[1].count, 1);
    assert_eq!(loaded.armor[2].count, 1);
    assert_eq!(loaded.armor[3].count, 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn diamond_armor_reduces_player_damage() {
    let uuid = [0xB2; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[0] = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    session.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    register_player_entity(&session, 4242);

    let result = damage_player(&mut session, 4242, 10.0);
    assert!(result.damaged);
    assert!(
        (session.health - 17.0).abs() < 1e-4,
        "health {}",
        session.health
    );
}

#[test]
fn no_armor_takes_full_damage() {
    let uuid = [0xC3; 16];
    let mut session = session_with_uuid(uuid);
    register_player_entity(&session, 4243);

    let result = damage_player(&mut session, 4243, 10.0);
    assert!(result.damaged);
    assert!(
        (session.health - 10.0).abs() < 1e-4,
        "health {}",
        session.health
    );
}

#[test]
fn partial_armor_uses_the_curve() {
    let uuid = [0xD4; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    register_player_entity(&session, 4244);

    let expected_after = folia_core::effects::damage_after_armor(10.0, 8, 2);
    let result = damage_player(&mut session, 4244, 10.0);
    assert!(result.damaged);
    let expected_health = 20.0 - expected_after;
    assert!(
        (session.health - expected_health).abs() < 1e-4,
        "health {} expected {}",
        session.health,
        expected_health
    );
}

#[test]
fn armor_takes_a_quarter_of_the_raw_damage() {
    let uuid = [0xE1; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[0] = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    session.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    for piece in &mut session.armor {
        folia_core::item_prototypes::apply_defaults(piece);
    }
    register_player_entity(&session, 4301);

    let mut rng = folia_random::LegacyRandomSource::new(1);
    let result = folia_core::join::damage_player_with_rng(&mut session, 4301, 10.0, &mut rng);
    assert!(result.damaged);
    assert_eq!(session.armor[0].damage(), 2);
    assert_eq!(session.armor[1].damage(), 2);
    assert_eq!(session.armor[2].damage(), 2);
    assert_eq!(session.armor[3].damage(), 2);
}

#[test]
fn armor_minimum_wear_is_one() {
    let uuid = [0xE2; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[0] = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    session.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    for piece in &mut session.armor {
        folia_core::item_prototypes::apply_defaults(piece);
    }
    register_player_entity(&session, 4302);

    let mut rng = folia_random::LegacyRandomSource::new(2);
    let result = folia_core::join::damage_player_with_rng(&mut session, 4302, 3.0, &mut rng);
    assert!(result.damaged);
    assert_eq!(session.armor[0].damage(), 1);
    assert_eq!(session.armor[1].damage(), 1);
    assert_eq!(session.armor[2].damage(), 1);
    assert_eq!(session.armor[3].damage(), 1);
}

#[test]
fn broken_armor_is_cleared_and_stops_mitigating() {
    let uuid = [0xE3; 16];
    let mut session = session_with_uuid(uuid);
    session.armor[0] = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    session.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    for piece in &mut session.armor {
        folia_core::item_prototypes::apply_defaults(piece);
    }
    let max = session.armor[0].max_damage();
    session.armor[0].set_damage(max - 1);
    register_player_entity(&session, 4303);

    let mut rng = folia_random::LegacyRandomSource::new(3);
    let health_before = session.health;
    let result = folia_core::join::damage_player_with_rng(&mut session, 4303, 10.0, &mut rng);
    assert!(result.damaged);
    assert!(
        session.armor[0].is_empty(),
        "boots should break and be cleared"
    );
    assert!(
        result.packets.iter().any(|packet| {
            folia_core::protocol::PacketReader::new(packet)
                .read_varint()
                .ok()
                == Some(100)
        }),
        "armor change should emit set equipment packet"
    );
    let loss_with_break = health_before - session.health;

    // Second hit has no boots mitigation; use a fresh entity/session state to
    // isolate mitigation difference.
    let uuid2 = [0xE4; 16];
    let mut session2 = session_with_uuid(uuid2);
    session2.armor[1] = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    session2.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    session2.armor[3] = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    for index in [1, 2, 3] {
        folia_core::item_prototypes::apply_defaults(&mut session2.armor[index]);
    }
    register_player_entity(&session2, 4304);
    let mut rng2 = folia_random::LegacyRandomSource::new(4);
    let before2 = session2.health;
    let _ = folia_core::join::damage_player_with_rng(&mut session2, 4304, 10.0, &mut rng2);
    let loss_without_boots = before2 - session2.health;
    assert!(
        loss_without_boots >= loss_with_break - 1e-4,
        "missing boots should not mitigate better: {loss_without_boots} vs {loss_with_break}"
    );
}

#[test]
fn unbreaking_can_skip_wear() {
    let uuid = [0xE5; 16];
    let mut found = false;
    for seed in 0u64..64 {
        let mut session = session_with_uuid(uuid);
        session.armor[2] = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
        folia_core::item_prototypes::apply_defaults(&mut session.armor[2]);
        session.armor[2].set_component(folia_core::item_components::ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:unbreaking".to_string(), 3)]),
        ));
        register_player_entity(&session, 4305);
        let mut rng = folia_random::LegacyRandomSource::new(seed as i64);
        let _ = folia_core::join::damage_player_with_rng(&mut session, 4305, 10.0, &mut rng);
        let wear = session.armor[2].damage();
        assert!(wear <= 2, "wear {wear} should be <= points (2)");
        if wear < 2 {
            found = true;
            break;
        }
    }
    assert!(found, "expected some seed to skip unbreaking wear");
}
