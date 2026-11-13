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
