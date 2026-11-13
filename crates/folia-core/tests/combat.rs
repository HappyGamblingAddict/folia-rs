use folia_core::combat::{
    AttackCooldown, CRITICAL_MULTIPLIER, CriticalConditions, attack_damage, damage_scale,
    is_critical,
};

#[test]
fn cooldown_ticks_at_vanilla_speeds() {
    assert_eq!(AttackCooldown::new(4.0).cooldown_ticks(), 5);
    assert_eq!(AttackCooldown::new(1.6).cooldown_ticks(), 13);
}

#[test]
fn attack_speed_fallback_for_invalid_values() {
    for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let cooldown = AttackCooldown::new(speed);
        assert_eq!(cooldown.attack_speed(), 4.0);

        let mut mutable = AttackCooldown::new(4.0);
        mutable.set_attack_speed(speed);
        assert_eq!(mutable.attack_speed(), 4.0);
    }
}

#[test]
fn strength_scale_progression() {
    let mut cooldown = AttackCooldown::new(4.0);
    assert_eq!(cooldown.strength_scale(0.0), 0.0);

    for _ in 0..3 {
        cooldown.tick();
    }
    assert!((cooldown.strength_scale(0.0) - 0.6).abs() < 1e-6);

    for _ in 0..10 {
        cooldown.tick();
    }
    assert_eq!(cooldown.strength_scale(0.0), 1.0);

    cooldown.tick();
    assert_eq!(cooldown.strength_scale(0.0), 1.0);
}

#[test]
fn strength_scale_partial_ticks_are_clamped() {
    let mut cooldown = AttackCooldown::new(4.0);
    for _ in 0..3 {
        cooldown.tick();
    }
    assert!((cooldown.strength_scale(2.0) - 0.8).abs() < 1e-6);
    assert!((cooldown.strength_scale(-1.0) - 0.6).abs() < 1e-6);
}

#[test]
fn damage_scale_values() {
    assert!((damage_scale(0.0) - 0.2).abs() < 1e-6);
    assert!((damage_scale(1.0) - 1.0).abs() < 1e-6);
    assert!((damage_scale(0.5) - 0.4).abs() < 1e-6);
}

#[test]
fn critical_boundary_and_veto_conditions() {
    let allowed = CriticalConditions {
        falling: true,
        on_ground: false,
        in_water: false,
        on_climbable: false,
        blind: false,
        passenger: false,
        sprinting: false,
    };

    assert!(!is_critical(0.9, &allowed));
    assert!(is_critical(0.91, &allowed));

    let mut conditions = allowed;
    conditions.falling = false;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.on_ground = true;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.in_water = true;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.on_climbable = true;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.blind = true;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.passenger = true;
    assert!(!is_critical(0.91, &conditions));

    let mut conditions = allowed;
    conditions.sprinting = true;
    assert!(!is_critical(0.91, &conditions));

    let _ = CRITICAL_MULTIPLIER;
}

#[test]
fn attack_damage_values() {
    assert!((attack_damage(7.0, 1.0, false) - 7.0).abs() < 1e-6);
    assert!((attack_damage(7.0, 1.0, true) - 10.5).abs() < 1e-6);
    assert!((attack_damage(7.0, 0.0, false) - 1.4).abs() < 1e-6);
    assert_eq!(attack_damage(f32::NAN, 1.0, false), 0.0);
}
