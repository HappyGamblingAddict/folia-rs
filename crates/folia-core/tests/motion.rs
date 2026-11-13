use folia_core::entity::{FlatCollider, SessionEntityTracker};
use folia_core::protocol::{read_set_entity_motion, set_entity_motion_packet, write_lp_vec3};

#[test]
fn lp_vec3_round_trips_representative_velocities() {
    for velocity in [
        [0.4, 0.4, -0.4],
        [12.3, -4.5, 0.0],
        [0.0, 0.0, 0.0],
        [1000.0, -2000.0, 3000.0],
    ] {
        let bytes = write_lp_vec3(velocity);
        let (decoded, consumed) = folia_core::protocol::read_lp_vec3(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        let tolerance = if velocity[0]
            .abs()
            .max(velocity[1].abs())
            .max(velocity[2].abs())
            > 100.0
        {
            1e-2
        } else {
            1e-3
        };
        for (a, b) in decoded.iter().zip(velocity.iter()) {
            assert!((a - b).abs() <= tolerance, "{a} vs {b}");
        }
    }
}

#[test]
fn zero_lp_vec3_is_a_single_byte() {
    assert_eq!(write_lp_vec3([0.0, 0.0, 0.0]), vec![0u8]);
}

#[test]
fn set_entity_motion_packet_pins_the_wire_layout() {
    let packet = set_entity_motion_packet(42, [0.4, 0.4, -0.4]);
    assert_eq!(packet[0], 99);
    let (entity_id, velocity) = read_set_entity_motion(&packet).unwrap();
    assert_eq!(entity_id, 42);
    assert!((velocity[0] - 0.4).abs() < 1e-4);

    let mut wrong = packet.clone();
    wrong[0] = 100;
    assert!(read_set_entity_motion(&wrong).is_err());

    let mut trailing = packet.clone();
    trailing.push(0);
    assert!(read_set_entity_motion(&trailing).is_err());
}

#[test]
fn session_tick_reports_player_knockback_from_mob_attacks() {
    let mut tracker = SessionEntityTracker::deterministic((0.5, 63.0, 0.5), 63, 7);
    let mut zombie = None;
    for id in tracker.tracked_ids() {
        if tracker
            .entity(id)
            .is_some_and(|tracked| tracked.ai.is_some())
        {
            zombie = Some(id);
            break;
        }
    }
    let zombie = zombie.expect("session tracker spawns a zombie");
    tracker.damage(zombie, 1.0).expect("zombie takes damage");

    let mut collider = FlatCollider { ground_y: 63 };
    let player = (0.5, 63.0, 0.5);
    let mut knockback = None;
    for _ in 0..80 {
        let tick = tracker.tick(player, &mut collider);
        if tick.player_knockback.is_some() {
            knockback = tick.player_knockback;
            break;
        }
    }
    let knockback = knockback.expect("a retaliating zombie produces knockback");
    let zombie = &tracker.entity(zombie).expect("zombie").entity;
    let to_player = (player.0 - zombie.x, player.2 - zombie.z);
    assert!(knockback[0] * to_player.0 + knockback[2] * to_player.1 > 0.0);
}
