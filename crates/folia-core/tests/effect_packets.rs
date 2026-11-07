//! Status-effect wire format and living-entity application/expiry tests.
//!
//! `ClientboundUpdateMobEffectPacket` is play-state packet 130 and
//! `ClientboundRemoveMobEffectPacket` is packet 76 in the 1.21.11
//! `GameProtocols.CLIENTBOUND_TEMPLATE` registration order. The effect field is
//! a registry-id VarInt from `BuiltInRegistries.MOB_EFFECT`.

use folia_core::effects::{
    CLIENTBOUND_REMOVE_MOB_EFFECT, CLIENTBOUND_UPDATE_MOB_EFFECT, EFFECT_FLAG_AMBIENT,
    EFFECT_FLAG_BLEND, EFFECT_FLAG_VISIBLE, EffectUpdate, StatusEffect, StatusEffectKind,
    read_remove_mob_effect, read_update_mob_effect, remove_mob_effect_packet,
    update_mob_effect_packet,
};
use folia_core::entity_metadata::{
    CLIENTBOUND_SET_ENTITY_DATA, DATA_HEALTH, LivingEntity, MetadataValue, read_set_entity_data,
};
use folia_core::item_components::{MobEffectInstance, PotionContents};
use folia_core::protocol::{PacketReader, PacketWriter};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn packet_id(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().expect("packet id")
}

fn decode_health(packet: &[u8]) -> f32 {
    let (_, values) = read_set_entity_data(packet).expect("metadata packet");
    values
        .iter()
        .find(|value| value.index == DATA_HEALTH)
        .and_then(|value| match &value.value {
            MetadataValue::Float(health) => Some(*health),
            _ => None,
        })
        .expect("health metadata")
}

#[test]
fn update_mob_effect_body_matches_vanilla_layout() {
    assert_eq!(CLIENTBOUND_UPDATE_MOB_EFFECT, 130);
    let effect = StatusEffect::new(StatusEffectKind::Speed, 300, 1).with_flags(true, true, true);
    let packet = update_mob_effect_packet(42, &effect, false);

    // id VarInt 130 (82 01), entity 42, effect id 0, amplifier 1, duration 300
    // (ac 02), flags ambient|visible|show-icon (07).
    assert_eq!(hex(&packet), "82012a0001ac0207");
    assert_eq!(packet_id(&packet), CLIENTBOUND_UPDATE_MOB_EFFECT);

    let (entity_id, decoded, blend) = read_update_mob_effect(&packet).expect("decode update");
    assert_eq!(entity_id, 42);
    assert_eq!(decoded, effect);
    assert!(!blend);
}

#[test]
fn update_flags_and_blend_round_trip() {
    let effect = StatusEffect::new(StatusEffectKind::Poison, 25, 2).with_flags(false, true, false);
    let packet = update_mob_effect_packet(7, &effect, true);
    let (_, decoded, blend) = read_update_mob_effect(&packet).expect("decode update");

    assert_eq!(decoded, effect);
    assert!(blend);
    assert!(!decoded.ambient);
    assert!(decoded.visible);
    assert!(!decoded.show_icon);
    assert_eq!(
        decoded.flags(),
        EFFECT_FLAG_VISIBLE,
        "blend is not stored on the instance"
    );
    assert_eq!(
        packet.last().copied(),
        Some(EFFECT_FLAG_VISIBLE | EFFECT_FLAG_BLEND)
    );
}

#[test]
fn remove_mob_effect_body_matches_vanilla_layout() {
    assert_eq!(CLIENTBOUND_REMOVE_MOB_EFFECT, 76);
    let packet = remove_mob_effect_packet(42, StatusEffectKind::Speed);
    // id 76 (4c), entity 42, effect id 0.
    assert_eq!(hex(&packet), "4c2a00");
    assert_eq!(packet_id(&packet), CLIENTBOUND_REMOVE_MOB_EFFECT);

    let (entity_id, kind) = read_remove_mob_effect(&packet).expect("decode remove");
    assert_eq!(entity_id, 42);
    assert_eq!(kind, StatusEffectKind::Speed);
}

#[test]
fn effect_registry_ids_follow_the_1_21_11_registration_order() {
    for (id, kind) in StatusEffectKind::ALL.into_iter().enumerate() {
        assert_eq!(kind.registry_id(), id as i32, "{kind:?}");
        assert_eq!(StatusEffectKind::from_registry_id(id as i32), Some(kind));
    }
    assert_eq!(StatusEffectKind::Speed.registry_id(), 0);
    assert_eq!(StatusEffectKind::InstantHealth.registry_id(), 5);
    assert_eq!(StatusEffectKind::Wither.registry_id(), 19);
    assert_eq!(StatusEffectKind::BreathOfTheNautilus.registry_id(), 39);
    assert_eq!(StatusEffectKind::from_registry_id(40), None);
    assert_eq!(StatusEffectKind::from_registry_id(-1), None);
}

#[test]
fn unknown_effect_ids_fail_loudly() {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_UPDATE_MOB_EFFECT).unwrap();
    writer.write_varint(1).unwrap();
    writer.write_varint(999).unwrap(); // not a 1.21.11 mob effect
    writer.write_varint(0).unwrap();
    writer.write_varint(20).unwrap();
    writer.write_u8(0).unwrap();
    let error = read_update_mob_effect(&writer.into_inner()).expect_err("unknown effect id");
    assert!(error.to_string().contains("999"), "{error}");

    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_REMOVE_MOB_EFFECT).unwrap();
    writer.write_varint(1).unwrap();
    writer.write_varint(999).unwrap();
    let error = read_remove_mob_effect(&writer.into_inner()).expect_err("unknown effect id");
    assert!(error.to_string().contains("999"), "{error}");
}

#[test]
fn component_effects_carry_show_icon() {
    let mut component = MobEffectInstance::new("minecraft:speed", 200, 3);
    component.ambient = true;
    component.show_particles = false;
    component.show_icon = false;
    let effect = StatusEffect::from_component(&component).expect("known effect");
    assert_eq!(effect.kind, StatusEffectKind::Speed);
    assert_eq!(effect.amplifier, 3);
    assert_eq!(effect.duration, 200);
    assert!(effect.ambient);
    assert!(!effect.visible);
    assert!(!effect.show_icon);
    assert_eq!(
        effect.flags(),
        EFFECT_FLAG_AMBIENT,
        "ambient only when visible and show-icon are off"
    );
}

#[test]
fn applying_an_effect_emits_an_update_and_merges_stronger_values() {
    let mut entity = LivingEntity::new(42);
    let first = entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 100, 0));
    assert_eq!(first.update, EffectUpdate::Added);
    assert_eq!(first.packets.len(), 1);
    let (entity_id, decoded, _) = read_update_mob_effect(&first.packets[0]).expect("update");
    assert_eq!(entity_id, 42);
    assert_eq!(decoded.kind, StatusEffectKind::Speed);
    assert_eq!(decoded.amplifier, 0);
    assert_eq!(decoded.duration, 100);

    let stronger = entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 160, 2));
    assert_eq!(stronger.update, EffectUpdate::Updated);
    let (_, decoded, _) = read_update_mob_effect(&stronger.packets[0]).expect("update");
    assert_eq!(decoded.amplifier, 2);
    assert_eq!(decoded.duration, 160);
    assert_eq!(entity.effects.len(), 1);

    // A weaker, shorter instance cannot downgrade the active one.
    let weaker = entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 50, 1));
    assert_eq!(weaker.update, EffectUpdate::Unchanged);
    assert!(weaker.packets.is_empty());
    let active = entity.effects.get(StatusEffectKind::Speed).expect("speed");
    assert_eq!(active.amplifier, 2);
    assert_eq!(active.duration, 160);
}

#[test]
fn replace_effect_overwrites_instead_of_merging() {
    let mut entity = LivingEntity::new(42);
    entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 1000, 0));
    let replaced = entity.replace_effect(StatusEffect::new(StatusEffectKind::Speed, 20, 3));
    assert_eq!(replaced.update, EffectUpdate::Updated);
    let (_, decoded, _) = read_update_mob_effect(&replaced.packets[0]).expect("update");
    assert_eq!(decoded.amplifier, 3);
    assert_eq!(decoded.duration, 20);

    // Replacing with identical values is not a packet-worthy change.
    let identical = entity.replace_effect(StatusEffect::new(StatusEffectKind::Speed, 20, 3));
    assert_eq!(identical.update, EffectUpdate::Unchanged);
    assert!(identical.packets.is_empty());
}

#[test]
fn expiry_emits_a_remove_packet() {
    let mut entity = LivingEntity::new(42);
    entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 2, 0));
    assert!(entity.effects.has(StatusEffectKind::Speed));

    let first = entity.tick();
    assert!(
        !first
            .iter()
            .any(|packet| packet_id(packet) == CLIENTBOUND_REMOVE_MOB_EFFECT),
        "effect still has one tick"
    );
    let second = entity.tick();
    let removal = second
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_REMOVE_MOB_EFFECT)
        .expect("expiry removes the effect");
    let (entity_id, kind) = read_remove_mob_effect(removal).expect("decode remove");
    assert_eq!((entity_id, kind), (42, StatusEffectKind::Speed));
    assert!(!entity.effects.has(StatusEffectKind::Speed));
}

#[test]
fn infinite_duration_effects_never_expire() {
    let mut entity = LivingEntity::new(42);
    let applied = entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, -1, 0));
    assert_eq!(applied.update, EffectUpdate::Added);
    for _ in 0..600 {
        entity.tick();
    }
    assert_eq!(
        entity
            .effects
            .get(StatusEffectKind::Speed)
            .map(|e| e.duration),
        Some(-1)
    );
}

#[test]
fn explicit_removal_and_clear_emit_remove_packets() {
    let mut entity = LivingEntity::new(42);
    entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 100, 0));
    entity.apply_effect(StatusEffect::new(StatusEffectKind::NightVision, 100, 0));

    let removal = entity
        .remove_effect(StatusEffectKind::Speed)
        .expect("remove");
    let (_, kind) = read_remove_mob_effect(&removal).expect("decode remove");
    assert_eq!(kind, StatusEffectKind::Speed);
    assert!(entity.remove_effect(StatusEffectKind::Speed).is_none());

    let cleared = entity.clear_effects();
    assert_eq!(cleared.len(), 1);
    let (_, kind) = read_remove_mob_effect(&cleared[0]).expect("decode remove");
    assert_eq!(kind, StatusEffectKind::NightVision);
    assert!(entity.effects.is_empty());
}

#[test]
fn poison_and_regeneration_tick_through_the_living_pipeline() {
    let mut poisoned = LivingEntity::new(42);
    poisoned.set_health(10.0);
    poisoned.metadata.pack_dirty();
    poisoned.apply_effect(StatusEffect::new(StatusEffectKind::Poison, 100, 0));
    let mut damaged = false;
    for _ in 0..25 {
        let packets = poisoned.tick();
        if let Some(metadata) = packets
            .iter()
            .find(|packet| packet_id(packet) == CLIENTBOUND_SET_ENTITY_DATA)
        {
            assert!(decode_health(metadata) < 10.0);
            damaged = true;
        }
    }
    assert!(damaged, "poison ticks damage the living entity");
    assert!(poisoned.health < 10.0);

    let mut regenerating = LivingEntity::new(43);
    regenerating.set_health(10.0);
    regenerating.metadata.pack_dirty();
    regenerating.apply_effect(StatusEffect::new(StatusEffectKind::Regeneration, 100, 0));
    for _ in 0..50 {
        regenerating.tick();
    }
    assert!(regenerating.health > 10.0);
}

#[test]
fn potion_application_sends_effect_and_instant_metadata_packets() {
    let mut entity = LivingEntity::new(7);
    entity.set_health(10.0);
    entity.metadata.pack_dirty();

    let mut contents = PotionContents::new("minecraft:swiftness");
    contents
        .custom_effects
        .push(MobEffectInstance::new("minecraft:instant_health", 1, 0));
    let packets = entity.apply_potion_contents(&contents);

    let update = packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_UPDATE_MOB_EFFECT)
        .expect("swiftness update packet");
    let (entity_id, effect, _) = read_update_mob_effect(update).expect("decode update");
    assert_eq!(entity_id, 7);
    assert_eq!(effect.kind, StatusEffectKind::Speed);
    assert_eq!(effect.duration, 3600);

    let metadata = packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_SET_ENTITY_DATA)
        .expect("instant healing metadata");
    assert_eq!(decode_health(metadata), 14.0);
    assert_eq!(entity.health, 14.0);

    // Instant effects are never stored; the swiftness effect is.
    assert!(!entity.effects.has(StatusEffectKind::InstantHealth));
    assert!(entity.effects.has(StatusEffectKind::Speed));

    // A second healing potion applies once more, not twice.
    entity.apply_potion_contents(&PotionContents::new("minecraft:healing"));
    assert_eq!(entity.health, 18.0);
}

#[test]
fn instant_damage_applies_once_and_updates_metadata() {
    let mut entity = LivingEntity::new(8);
    let result = entity.apply_effect(StatusEffect::new(StatusEffectKind::InstantDamage, 1, 0));
    assert_eq!(result.update, EffectUpdate::Instant);
    assert_eq!(entity.health, 14.0);
    let metadata = result
        .packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_SET_ENTITY_DATA)
        .expect("health metadata");
    assert_eq!(decode_health(metadata), 14.0);
    assert!(!entity.effects.has(StatusEffectKind::InstantDamage));

    // The 20-tick invulnerability window rejects an equal second magic hit, so
    // the payload is not re-applied within the window.
    let repeated = entity.apply_effect(StatusEffect::new(StatusEffectKind::InstantDamage, 1, 0));
    assert!(repeated.packets.is_empty());
    assert_eq!(entity.health, 14.0);
}

#[test]
fn active_effect_packets_are_ordered_by_add_order() {
    let mut entity = LivingEntity::new(9);
    assert!(entity.active_effect_packets().is_empty());
    entity.apply_effect(StatusEffect::new(StatusEffectKind::NightVision, 100, 0));
    entity.apply_effect(StatusEffect::new(StatusEffectKind::Speed, 100, 1));

    let packets = entity.active_effect_packets();
    assert_eq!(packets.len(), 2);
    let kinds: Vec<_> = packets
        .iter()
        .map(|packet| read_update_mob_effect(packet).expect("decode").1.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![StatusEffectKind::NightVision, StatusEffectKind::Speed]
    );
}
