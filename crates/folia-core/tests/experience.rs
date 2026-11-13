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

#[test]
fn set_experience_packet_pins_the_wire_layout() {
    let packet = folia_core::entity_metadata::set_experience_packet(0.5, 7, 123);
    assert_eq!(packet[0], 0x65, "id 101 VarInt");
    let (progress, level, total) =
        folia_core::entity_metadata::read_set_experience(&packet).unwrap();
    assert!((progress - 0.5).abs() < 1e-6);
    assert_eq!(level, 7);
    assert_eq!(total, 123);
}

#[test]
fn set_experience_clamps_progress_and_negatives() {
    let high = folia_core::entity_metadata::set_experience_packet(2.0, 1, 1);
    let (progress, _, _) = folia_core::entity_metadata::read_set_experience(&high).unwrap();
    assert!((progress - 1.0).abs() < 1e-6);

    let low = folia_core::entity_metadata::set_experience_packet(-1.0, -5, -10);
    let (progress, level, total) = folia_core::entity_metadata::read_set_experience(&low).unwrap();
    assert!(progress.abs() < 1e-6);
    assert_eq!(level, 0);
    assert_eq!(total, 0);
}

#[test]
fn set_experience_rejects_wrong_id_and_trailing_bytes() {
    let wrong_id = folia_core::entity_metadata::set_experience_packet(0.0, 0, 0);
    let mut tampered = wrong_id.clone();
    tampered[0] = 0x01;
    assert!(folia_core::entity_metadata::read_set_experience(&tampered).is_err());

    let mut trailing = wrong_id;
    trailing.push(0);
    assert!(folia_core::entity_metadata::read_set_experience(&trailing).is_err());
}
