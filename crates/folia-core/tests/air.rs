//! Oxygen depletion and drowning.

use folia_core::player::{AirTick, PlayerSession};

fn player() -> PlayerSession {
    PlayerSession::new("Air".into(), [0x66; 16], 0.0, 64.0, 0.0)
}

#[test]
fn air_drains_at_one_per_tick_and_refills_at_four() {
    let mut player = player();
    assert_eq!(player.air, 300);
    assert_eq!(player.tick_air(true), AirTick::Consumed);
    assert_eq!(player.air, 299);
    assert_eq!(player.tick_air(false), AirTick::Refilled);
    assert_eq!(player.air, 300);
    assert_eq!(player.tick_air(false), AirTick::Nothing);
}

#[test]
fn drowning_deals_two_damage_and_resets_the_bar() {
    let mut player = player();
    player.air = 0;
    player.health = 20.0;
    assert_eq!(player.tick_air(true), AirTick::Consumed);
    assert_eq!(player.air, -1);
    for _ in 0..19 {
        player.tick_air(true);
    }
    assert_eq!(player.health, 18.0);
    assert_eq!(player.air, 0);
    assert_eq!(player.tick_air(true), AirTick::Consumed);
    for _ in 0..19 {
        player.tick_air(true);
    }
    assert_eq!(player.health, 16.0);
}

#[test]
fn drowning_cannot_push_health_below_zero() {
    let mut player = player();
    player.health = 1.0;
    player.air = 0;
    for _ in 0..20 {
        player.tick_air(true);
    }
    assert_eq!(player.health, 0.0);
}
