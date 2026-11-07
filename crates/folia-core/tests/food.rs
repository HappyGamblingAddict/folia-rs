//! Player hunger, saturation, regeneration, and starvation.

use folia_core::player::{FoodTick, PlayerSession};

fn player() -> PlayerSession {
    PlayerSession::new("Food".into(), [0x33; 16], 0.0, 64.0, 0.0)
}

#[test]
fn exhaustion_burns_saturation_then_food() {
    let mut player = player();
    player.food_saturation = 2.0;
    player.food_level = 20;
    player.add_exhaustion(4.0);
    assert_eq!(player.food_saturation, 1.0);
    assert_eq!(player.food_level, 20);
    player.add_exhaustion(8.0);
    assert_eq!(player.food_saturation, 0.0);
    assert_eq!(player.food_level, 19);
}

#[test]
fn eating_caps_food_and_saturation() {
    let mut player = player();
    player.food_level = 10;
    player.food_saturation = 0.0;
    player.eat(8, 0.6);
    assert_eq!(player.food_level, 18);
    assert!((player.food_saturation - 9.6).abs() < 1e-5);
    player.eat(20, 1.0);
    assert_eq!(player.food_level, 20);
    assert_eq!(player.food_saturation, 20.0);
}

#[test]
fn regeneration_heals_while_fed_and_saturated() {
    let mut player = player();
    player.health = 10.0;
    player.food_level = 20;
    player.food_saturation = 5.0;
    assert_eq!(player.tick_food(10), FoodTick::Heal);
    assert_eq!(player.health, 11.0);
    assert!((player.food_exhaustion - 2.0).abs() < 1e-5);
}

#[test]
fn starvation_damages_at_zero_food() {
    let mut player = player();
    player.health = 5.0;
    player.food_level = 0;
    player.food_saturation = 0.0;
    assert_eq!(player.tick_food(80), FoodTick::Starve);
    assert_eq!(player.health, 4.0);
    assert_eq!(player.tick_food(81), FoodTick::Nothing);
}

#[test]
fn food_tick_is_quiet_between_intervals() {
    let mut player = player();
    player.food_level = 20;
    player.food_saturation = 5.0;
    assert_eq!(player.tick_food(11), FoodTick::Nothing);
    assert_eq!(player.health, 20.0);
}
