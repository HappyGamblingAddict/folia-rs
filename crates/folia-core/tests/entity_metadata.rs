//! Focused tests for the entity metadata, living-entity health/damage,
//! attributes, and equipment slice.
//!
//! Packet expectations follow Folia 1.21.11:
//! `ClientboundSetEntityDataPacket` (id 97), `ClientboundSetEquipmentPacket`
//! (100), `ClientboundEntityEventPacket` (34), and `ClientboundSetHealthPacket`
//! (102).

use folia_core::entity::{Entity, hurt_entity, remove, set_entity_equipment, spawn_living};
use folia_core::entity_metadata::{
    Attribute, CLIENTBOUND_ENTITY_EVENT, CLIENTBOUND_SET_ENTITY_DATA, CLIENTBOUND_SET_EQUIPMENT,
    CLIENTBOUND_SET_HEALTH, DATA_CUSTOM_NAME, DATA_HEALTH, DATA_POSE, DATA_SHARED_FLAGS, DataValue,
    ENTITY_EVENT_DEATH, ENTITY_EVENT_DEATH_POOF, EntityDataSerializer, EntityMetadata, Equipment,
    EquipmentSlot, LIVING_FLAG_USING_ITEM, LivingEntity, MetadataStore, MetadataValue, Pose,
    read_entity_event, read_set_entity_data, read_set_equipment, read_set_health,
    set_entity_data_packet, set_equipment_packet, set_equipment_subset_packet, set_health_packet,
};
use folia_core::item_components::ItemStack;
use folia_core::nbt::Tag;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn named(value: &str) -> MetadataValue {
    MetadataValue::OptionalComponent(Some(Tag::String(value.to_owned())))
}

#[test]
fn metadata_packet_body_matches_vanilla_layout() {
    let values = vec![
        DataValue::new(DATA_SHARED_FLAGS, MetadataValue::Byte(0)),
        DataValue::new(DATA_CUSTOM_NAME, named("Steve")),
        DataValue::new(DATA_POSE, MetadataValue::Pose(Pose::Standing)),
        DataValue::new(DATA_HEALTH, MetadataValue::Float(20.0)),
    ];

    let packet = set_entity_data_packet(42, &values);

    // id 97, entity 42; index/serializer/value triples; 0xff terminator.
    assert_eq!(
        hex(&packet),
        "612a0000000206010800055374657665061400090341a00000ff"
    );
    assert_eq!(packet[0], CLIENTBOUND_SET_ENTITY_DATA as u8);

    let (entity_id, decoded) = read_set_entity_data(&packet).expect("decode metadata");
    assert_eq!(entity_id, 42);
    assert_eq!(decoded, values);
    assert_eq!(
        decoded[1].value.serializer(),
        EntityDataSerializer::OptionalComponent
    );
}

#[test]
fn dirty_tracking_packs_only_changed_values() {
    let mut metadata = EntityMetadata::new_living(42);
    assert!(!metadata.is_dirty());
    assert!(metadata.dirty_packet().is_none());
    assert_eq!(metadata.pack_all().len(), 10);

    assert!(metadata.set_health(19.0).unwrap());
    let packet = metadata.dirty_packet().expect("dirty health");
    let (entity_id, values) = read_set_entity_data(&packet).expect("decode dirty packet");
    assert_eq!(entity_id, 42);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].index, DATA_HEALTH);
    assert_eq!(values[0].value, MetadataValue::Float(19.0));
    assert_eq!(hex(&packet), "612a090341980000ff");

    // The dirty flag is cleared after packing.
    assert!(metadata.dirty_packet().is_none());
    assert!(!metadata.is_dirty());

    // Setting the same value again is a no-op.
    assert!(!metadata.set_health(19.0).unwrap());
    assert!(metadata.dirty_packet().is_none());

    // Dirty values are packed in ascending index order.
    assert!(metadata.set_pose(Pose::Crouching).unwrap());
    assert!(
        metadata
            .set_custom_name(Some(Tag::String("Dinnerbone".into())))
            .unwrap()
    );
    let packet = metadata.dirty_packet().expect("dirty name and pose");
    let (_, values) = read_set_entity_data(&packet).expect("decode dirty packet");
    assert_eq!(
        values.iter().map(|value| value.index).collect::<Vec<_>>(),
        vec![DATA_CUSTOM_NAME, DATA_POSE]
    );
    assert_eq!(values[1].value, MetadataValue::Pose(Pose::Crouching));
}

#[test]
fn metadata_store_is_keyed_by_entity_id() {
    let mut store = MetadataStore::new();
    store.insert(EntityMetadata::new_living(1));
    store.insert(EntityMetadata::new_living(2));

    store
        .set(1, DATA_HEALTH, MetadataValue::Float(7.5))
        .expect("known entity");

    assert!(store.dirty_packet(2).is_none());
    let packet = store.dirty_packet(1).expect("entity 1 dirty");
    let (entity_id, values) = read_set_entity_data(&packet).expect("decode");
    assert_eq!(entity_id, 1);
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].value, MetadataValue::Float(7.5));

    assert_eq!(store.len(), 2);
    assert_eq!(store.get(2).unwrap().get_health(), Some(20.0));
    store.remove(2);
    assert_eq!(store.len(), 1);
}

#[test]
fn flags_are_bit_positions_for_shared_and_masks_for_living() {
    let mut metadata = EntityMetadata::new_living(7);
    metadata
        .set_shared_flag(folia_core::entity_metadata::FLAG_ON_FIRE, true)
        .unwrap();
    metadata
        .set_shared_flag(folia_core::entity_metadata::FLAG_INVISIBLE, true)
        .unwrap();
    assert_eq!(
        metadata.shared_flags(),
        (1 << folia_core::entity_metadata::FLAG_ON_FIRE)
            | (1 << folia_core::entity_metadata::FLAG_INVISIBLE)
    );

    metadata
        .set_living_flag(LIVING_FLAG_USING_ITEM, true)
        .unwrap();
    assert_eq!(
        metadata.get(folia_core::entity_metadata::DATA_LIVING_ENTITY_FLAGS),
        Some(&MetadataValue::Byte(1))
    );

    metadata
        .set_shared_flag(folia_core::entity_metadata::FLAG_ON_FIRE, false)
        .unwrap();
    assert_eq!(
        metadata.shared_flags(),
        1 << folia_core::entity_metadata::FLAG_INVISIBLE
    );
}

#[test]
fn damage_uses_vanilla_invulnerability_window() {
    let mut entity = LivingEntity::new(42);
    assert_eq!(entity.health, 20.0);

    let first = entity.hurt(5.0);
    assert!(first.damaged);
    assert!(!first.died);
    assert_eq!(first.amount, 5.0);
    assert_eq!(entity.health, 15.0);
    assert_eq!(entity.invulnerable_time, 20);
    assert_eq!(entity.hurt_duration, 10);
    assert_eq!(entity.hurt_time, 10);
    assert_eq!(entity.last_hurt, 5.0);
    assert_eq!(first.packets.len(), 1);
    let (_, values) = read_set_entity_data(&first.packets[0]).expect("health metadata");
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].index, DATA_HEALTH);
    assert_eq!(values[0].value, MetadataValue::Float(15.0));

    // In the first half of the window, an equal/weaker hit is ignored.
    let ignored = entity.hurt(5.0);
    assert!(!ignored.damaged);
    assert!(ignored.packets.is_empty());
    assert_eq!(entity.health, 15.0);

    // A stronger hit inside the window still lands and updates lastHurt.
    let stronger = entity.hurt(6.0);
    assert!(stronger.damaged);
    assert_eq!(entity.health, 9.0);
    assert_eq!(entity.last_hurt, 6.0);

    for _ in 0..10 {
        assert!(entity.tick().is_empty());
    }
    assert_eq!(entity.invulnerable_time, 10);
    assert_eq!(entity.hurt_time, 0);

    // `invulnerableTime > duration / 2` is now false, so an equal hit lands.
    let after_window = entity.hurt(1.0);
    assert!(after_window.damaged);
    assert_eq!(entity.health, 8.0);
    assert_eq!(entity.invulnerable_time, 20);
}

#[test]
fn lethal_damage_sends_death_event_and_dying_pose() {
    let mut entity = LivingEntity::new(42);
    entity.set_health(2.0);
    entity.metadata.pack_dirty();

    let hit = entity.hurt(3.0);
    assert!(hit.damaged);
    assert!(hit.died);
    assert!(entity.dead);
    assert_eq!(entity.health, 0.0);
    assert_eq!(entity.death_time, 0);
    assert_eq!(hit.packets.len(), 2);

    let (entity_id, event) = read_entity_event(&hit.packets[0]).expect("death event");
    assert_eq!(entity_id, 42);
    assert_eq!(event, ENTITY_EVENT_DEATH);
    assert_eq!(hit.packets[0][0], CLIENTBOUND_ENTITY_EVENT as u8);

    let (_, values) = read_set_entity_data(&hit.packets[1]).expect("death metadata");
    assert_eq!(
        values
            .iter()
            .find(|value| value.index == DATA_HEALTH)
            .map(|value| &value.value),
        Some(&MetadataValue::Float(0.0))
    );
    assert_eq!(
        values
            .iter()
            .find(|value| value.index == DATA_POSE)
            .map(|value| &value.value),
        Some(&MetadataValue::Pose(Pose::Dying))
    );

    // Vanilla ticks deathTime to 20 before the poof event and removal.
    for _ in 0..19 {
        assert!(entity.tick().is_empty());
    }
    let packets = entity.tick();
    assert_eq!(packets.len(), 1);
    let (_, event) = read_entity_event(&packets[0]).expect("poof event");
    assert_eq!(event, ENTITY_EVENT_DEATH_POOF);
    assert!(entity.removed);
}

#[test]
fn player_damage_sends_set_health_and_keeps_invulnerability() {
    let mut player = LivingEntity::new_player(7);
    assert!(player.is_player);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.attributes.get(Attribute::AttackDamage), 1.0);

    let hit = player.hurt(5.0);
    assert!(hit.damaged);
    assert_eq!(hit.packets.len(), 2);
    let (health, food, saturation) = read_set_health(&hit.packets[1]).expect("set health");
    assert_eq!(hit.packets[1][0], CLIENTBOUND_SET_HEALTH as u8);
    assert_eq!(health, 15.0);
    assert_eq!(food, 20);
    assert_eq!(saturation, 5.0);

    // LivingEntity.tick skips ServerPlayer invulnerability countdown.
    for _ in 0..20 {
        player.tick();
    }
    assert_eq!(player.invulnerable_time, 20);
}

#[test]
fn set_health_packet_round_trips() {
    let packet = set_health_packet(13.5, 18, 2.25);
    assert_eq!(packet[0], CLIENTBOUND_SET_HEALTH as u8);
    let (health, food, saturation) = read_set_health(&packet).expect("decode");
    assert_eq!(health, 13.5);
    assert_eq!(food, 18);
    assert_eq!(saturation, 2.25);
}

#[test]
fn equipment_packet_round_trips_all_slots() {
    let mut equipment = Equipment::default();
    let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
    sword.set_component(folia_core::item_components::ComponentValue::CustomName(
        Tag::String("Excalibur".into()),
    ));
    let helmet = ItemStack::new("minecraft:diamond_helmet", 1);
    let shield = ItemStack::new("minecraft:shield", 1);

    equipment.set(EquipmentSlot::MainHand, sword.clone());
    equipment.set(EquipmentSlot::OffHand, shield.clone());
    equipment.set(EquipmentSlot::Head, helmet.clone());

    let packet = set_equipment_packet(42, &equipment);
    assert_eq!(packet[0], CLIENTBOUND_SET_EQUIPMENT as u8);

    let (entity_id, slots) = read_set_equipment(&packet).expect("decode equipment");
    assert_eq!(entity_id, 42);
    assert_eq!(slots.len(), 8);
    assert_eq!(slots[0].0, EquipmentSlot::MainHand);
    assert_eq!(slots[0].1, sword);
    assert_eq!(slots[1].0, EquipmentSlot::OffHand);
    assert_eq!(slots[1].1, shield);
    assert_eq!(slots[5].0, EquipmentSlot::Head);
    assert_eq!(slots[5].1, helmet);
    assert!(slots[2].1.is_empty());
    assert!(slots[7].1.is_empty());
}

#[test]
fn equipment_uses_ordinal_and_continuation_bits() {
    let subset = set_equipment_subset_packet(
        42,
        &[
            (EquipmentSlot::MainHand, ItemStack::empty()),
            (EquipmentSlot::Head, ItemStack::empty()),
        ],
    );
    // id 100 (0x64), entity 42 (0x2a), mainhand|0x80, empty stack,
    // head (ordinal 5, last), empty stack.
    assert_eq!(hex(&subset), "642a80000500");

    let (entity_id, slots) = read_set_equipment(&subset).expect("decode subset");
    assert_eq!(entity_id, 42);
    assert_eq!(
        slots,
        vec![
            (EquipmentSlot::MainHand, ItemStack::empty()),
            (EquipmentSlot::Head, ItemStack::empty()),
        ]
    );
}

#[test]
fn equipment_is_readable_from_living_entity_and_store() {
    let mut entity = LivingEntity::new(9);
    assert!(entity.equipment.is_empty());
    assert!(entity.set_equipment(
        EquipmentSlot::Feet,
        ItemStack::new("minecraft:diamond_boots", 1)
    ));
    assert!(!entity.set_equipment(
        EquipmentSlot::Feet,
        ItemStack::new("minecraft:diamond_boots", 1)
    ));
    assert_eq!(
        entity.equipment.get(EquipmentSlot::Feet).item,
        "minecraft:diamond_boots"
    );

    let mut store = folia_core::entity_metadata::LivingEntityStore::new();
    store.insert(entity);
    let packet = store.equipment_packet(9).expect("registered");
    let (_, slots) = read_set_equipment(&packet).expect("decode");
    assert_eq!(slots[2].0, EquipmentSlot::Feet);
    assert_eq!(slots[2].1.item, "minecraft:diamond_boots");
}

#[test]
fn entity_bridge_registers_damage_and_equipment_by_id() {
    let mut uuid = [0u8; 16];
    uuid[15] = 0x5a;
    let add = spawn_living(Entity::armor_stand(9101, uuid, 0.0, 5.0, 0.0));
    assert_eq!(add[0], 1); // ClientboundAddEntity

    let hit = hurt_entity(9101, 4.0).expect("living record");
    assert!(hit.damaged);
    assert_eq!(
        folia_core::entity_metadata::living_entities()
            .get(9101)
            .unwrap()
            .health,
        16.0
    );

    let packet = set_entity_equipment(
        9101,
        EquipmentSlot::MainHand,
        ItemStack::new("minecraft:stone", 1),
    )
    .expect("equipment packet");
    let (_, slots) = read_set_equipment(&packet).expect("decode");
    assert_eq!(slots[0].1.item, "minecraft:stone");

    remove(9101);
    assert!(
        folia_core::entity_metadata::living_entities()
            .get(9101)
            .is_none()
    );
}
