//! Armor, status-effect, and damage-type behavior.

use folia_core::effects::{
    DamageType, EffectTick, StatusEffect, StatusEffectKind, StatusEffectSet, apply_damage,
    armor_values_from_equipment, damage_after_armor,
};
use folia_core::entity_metadata::{Equipment, EquipmentSlot, LivingEntity};
use folia_core::item_components::ItemStack;

fn full_set(equipment: &mut Equipment, prefix: &str) {
    equipment.set(
        EquipmentSlot::Feet,
        ItemStack::new(format!("minecraft:{prefix}_boots"), 1),
    );
    equipment.set(
        EquipmentSlot::Legs,
        ItemStack::new(format!("minecraft:{prefix}_leggings"), 1),
    );
    equipment.set(
        EquipmentSlot::Chest,
        ItemStack::new(format!("minecraft:{prefix}_chestplate"), 1),
    );
    equipment.set(
        EquipmentSlot::Head,
        ItemStack::new(format!("minecraft:{prefix}_helmet"), 1),
    );
}

#[test]
fn armor_values_match_the_vanilla_table() {
    let mut equipment = Equipment::new();
    full_set(&mut equipment, "diamond");
    assert_eq!(armor_values_from_equipment(&equipment), (20, 8));

    let mut iron = Equipment::new();
    full_set(&mut iron, "iron");
    assert_eq!(armor_values_from_equipment(&iron), (15, 0));

    let mut leather = Equipment::new();
    full_set(&mut leather, "leather");
    assert_eq!(armor_values_from_equipment(&leather), (7, 0));
}

#[test]
fn armor_reduction_curve_matches_java() {
    assert_eq!(damage_after_armor(10.0, 0, 0), 10.0);
    assert!((damage_after_armor(10.0, 15, 0) - 6.0).abs() < 1e-5);
    assert!((damage_after_armor(10.0, 20, 8) - 3.0).abs() < 1e-5);
    assert!((damage_after_armor(100.0, 20, 8) - 84.0).abs() < 1e-5);
}

#[test]
fn damage_pipeline_applies_armor_then_resistance() {
    let mut entity = LivingEntity::new(1);
    let mut equipment = Equipment::new();
    full_set(&mut equipment, "iron");
    entity.equipment = equipment;

    let outcome = apply_damage(
        &mut entity,
        10.0,
        DamageType::MobAttack,
        &StatusEffectSet::new(),
    );
    assert_eq!(outcome.armor_points, 15);
    assert!((outcome.after_armor - 6.0).abs() < 1e-5);
    assert!((entity.health - 14.0).abs() < 1e-5);
}

#[test]
fn resistance_reduces_final_damage() {
    let mut entity = LivingEntity::new(1);
    let mut effects = StatusEffectSet::new();
    effects.add(StatusEffect::new(StatusEffectKind::Resistance, 200, 1));
    let outcome = apply_damage(&mut entity, 10.0, DamageType::Generic, &effects);
    assert!((outcome.after_resistance - 6.0).abs() < 1e-5);
    assert!((entity.health - 14.0).abs() < 1e-5);
}

#[test]
fn poison_and_wither_bypass_armor() {
    let mut entity = LivingEntity::new(1);
    let mut equipment = Equipment::new();
    full_set(&mut equipment, "diamond");
    entity.equipment = equipment;

    let outcome = apply_damage(
        &mut entity,
        4.0,
        DamageType::Poison,
        &StatusEffectSet::new(),
    );
    assert_eq!(outcome.armor_points, 0);
    assert!((entity.health - 16.0).abs() < 1e-5);
}

#[test]
fn void_damage_bypasses_invulnerability() {
    let mut entity = LivingEntity::new(1);
    let first = entity.hurt(5.0);
    assert!(first.damaged);
    assert!(entity.invulnerable_time > 0);
    let outcome = apply_damage(&mut entity, 5.0, DamageType::Void, &StatusEffectSet::new());
    assert!(outcome.hurt.damaged);
    assert!((entity.health - 10.0).abs() < 1e-5);
}

#[test]
fn effect_intervals_follow_the_vanilla_shift() {
    let mut effects = StatusEffectSet::new();
    effects.add(StatusEffect::new(StatusEffectKind::Poison, 100, 0));
    assert_eq!(
        effects.tick(25),
        vec![EffectTick::Damage {
            amount: 1,
            kind: StatusEffectKind::Poison
        }]
    );
    assert_eq!(effects.tick(26), vec![]);

    let mut wither = StatusEffectSet::new();
    wither.add(StatusEffect::new(StatusEffectKind::Wither, 200, 1));
    assert_eq!(
        wither.tick(20),
        vec![EffectTick::Damage {
            amount: 1,
            kind: StatusEffectKind::Wither
        }]
    );
}

#[test]
fn regeneration_and_expiry_are_reported() {
    let mut effects = StatusEffectSet::new();
    effects.add(StatusEffect::new(StatusEffectKind::Regeneration, 100, 0));
    for tick in 1..50 {
        assert_eq!(effects.tick(tick), vec![]);
    }
    assert_eq!(
        effects.tick(50),
        vec![EffectTick::Heal {
            amount: 1,
            kind: StatusEffectKind::Regeneration
        }]
    );

    let mut short = StatusEffectSet::new();
    short.add(StatusEffect::new(StatusEffectKind::Speed, 2, 0));
    assert_eq!(short.tick(0), vec![]);
    assert_eq!(
        short.tick(1),
        vec![EffectTick::Expired(StatusEffectKind::Speed)]
    );
    assert!(!short.has(StatusEffectKind::Speed));
}

#[test]
fn stronger_effect_refreshes_the_existing_instance() {
    let mut effects = StatusEffectSet::new();
    effects.add(StatusEffect::new(StatusEffectKind::Speed, 40, 0));
    effects.add(StatusEffect::new(StatusEffectKind::Speed, 200, 2));
    assert_eq!(effects.iter().count(), 1);
    assert_eq!(effects.amplifier(StatusEffectKind::Speed), Some(2));
    assert_eq!(effects.iter().next().unwrap().duration, 200);
}

#[test]
fn speed_weakness_and_jump_modifiers_match_vanilla() {
    let mut effects = StatusEffectSet::new();
    effects.add(StatusEffect::new(StatusEffectKind::Speed, 100, 1));
    effects.add(StatusEffect::new(StatusEffectKind::Slowness, 100, 0));
    assert!((effects.movement_multiplier() - 1.25).abs() < 1e-5);

    let mut combat = StatusEffectSet::new();
    combat.add(StatusEffect::new(StatusEffectKind::Strength, 100, 0));
    combat.add(StatusEffect::new(StatusEffectKind::Weakness, 100, 0));
    assert!((combat.attack_damage_modifier() + 1.0).abs() < 1e-5);
    combat.add(StatusEffect::new(StatusEffectKind::JumpBoost, 100, 2));
    assert_eq!(combat.jump_boost(), 3);
}

#[test]
fn fire_resistance_flag_is_exposed() {
    let mut effects = StatusEffectSet::new();
    assert!(!effects.has_fire_resistance());
    effects.add(StatusEffect::new(StatusEffectKind::FireResistance, 20, 0));
    assert!(effects.has_fire_resistance());
}
