//! Experience level curve and point accumulation.

use folia_core::player::PlayerSession;

#[test]
fn xp_curve_matches_vanilla_thresholds() {
    assert_eq!(PlayerSession::xp_needed_for_next_level(0), 7);
    assert_eq!(PlayerSession::xp_needed_for_next_level(14), 35);
    assert_eq!(PlayerSession::xp_needed_for_next_level(15), 37);
    assert_eq!(PlayerSession::xp_needed_for_next_level(29), 107);
    assert_eq!(PlayerSession::xp_needed_for_next_level(30), 112);
    assert_eq!(PlayerSession::xp_needed_for_next_level(31), 121);
}

#[test]
fn total_xp_is_the_sum_of_the_curve() {
    assert_eq!(PlayerSession::total_xp_for_level(0), 0);
    assert_eq!(PlayerSession::total_xp_for_level(1), 7);
    assert_eq!(PlayerSession::total_xp_for_level(2), 16);
    assert_eq!(PlayerSession::total_xp_for_level(16), 352);
}

#[test]
fn adding_points_levels_up_and_carries_progress() {
    let mut player = PlayerSession::new("Xp".into(), [0x44; 16], 0.0, 64.0, 0.0);
    player.add_experience(5);
    assert_eq!(player.xp_level, 0);
    assert!((player.xp_progress - 5.0 / 7.0).abs() < 1e-5);

    player.add_experience(2);
    assert_eq!(player.xp_level, 1);
    assert_eq!(player.xp_total, 7);
    assert!(player.xp_progress < 1.0);

    player.add_experience(PlayerSession::xp_needed_for_next_level(1));
    assert_eq!(player.xp_level, 2);
    assert_eq!(player.xp_total, 16);
}

#[test]
fn negative_and_zero_points_are_ignored() {
    let mut player = PlayerSession::new("Xp".into(), [0x55; 16], 0.0, 64.0, 0.0);
    player.add_experience(0);
    player.add_experience(-100);
    assert_eq!(player.xp_level, 0);
    assert_eq!(player.xp_total, 0);
    assert_eq!(player.xp_progress, 0.0);
}
