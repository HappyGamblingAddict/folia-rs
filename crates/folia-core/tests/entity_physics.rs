//! Differential physics test against a real Folia 1.21.11 armor-stand fall.
//!
//! The trace was captured from `java-test` with the `EntitySpawn` plugin
//! spawning an armor stand at `(0, 50, 0)` in the superflat world, then
//! recorded from `ClientboundAddEntity` and `ClientboundMoveEntityPacket.Pos`
//! packets (`capture/play-001-*`, `capture/play-051-*`).
//!
//! Java samples entity positions every 3 ticks for armor stands. The add
//! packet was sent at tick 5 (`y = 48.85489329934836`), then positions were
//! reported at ticks 8, 11, ..., 59. The values below are the positions
//! reconstructed by applying the packet deltas (`delta / 4096.0`).
//!
//! Folia then landed the stand at `y = -60` on top of the grass block whose
//! block position is `y = -61`.

use folia_core::entity::{BlockCollider, Entity, FlatCollider};

const JAVA_TICK_5_Y: f64 = 48.854_893_299_348_36;

/// Java positions at ticks 8, 11, 14, ..., 59.
const JAVA_TRACE: [(usize, f64); 18] = [
    (8, 47.305_332_752_473_36),
    (11, 45.155_430_408_723_36),
    (14, 42.440_342_518_098_36),
    (17, 39.193_272_205_598_36),
    (20, 35.445_713_611_848_36),
    (23, 31.226_963_611_848_362),
    (26, 26.564_610_096_223_362),
    (29, 21.485_020_252_473_362),
    (32, 16.012_364_002_473_362),
    (35, 10.170_078_846_223_362),
    (38, 3.979_893_299_348_362),
    (41, -2.537_928_966_276_638),
    (44, -9.364_100_841_276_638),
    (47, -16.480_555_919_401_638),
    (50, -23.869_716_075_651_638),
    (53, -31.516_200_450_651_638),
    (56, -39.404_628_185_026_64),
    (59, -47.520_447_671_420_6),
];

/// Java reported a velocity of `-2.7535929981792506` when the stand was at
/// `-47.5204476714206` (the sync packet samples velocity one tick later than
/// the position it carries).
const JAVA_VELOCITY_NEAR_47: f64 = -2.753_592_998_179_250_6;

#[test]
fn armor_stand_fall_matches_java_trace() {
    let mut entity = Entity::armor_stand(2, [0; 16], 0.0, 50.0, 0.0);
    let mut collider = FlatCollider { ground_y: -60 };

    let mut tick_5_y = None;
    let mut samples = Vec::new();
    let mut velocity_before_47 = None;

    for tick in 1..=80 {
        entity.tick_physics(&mut collider);
        if tick == 5 {
            tick_5_y = Some(entity.y);
        }
        if JAVA_TRACE.iter().any(|(sample, _)| *sample == tick) {
            samples.push((tick, entity.y));
        }
        if velocity_before_47.is_none() && entity.y < -44.0 {
            velocity_before_47 = Some((entity.y, entity.velocity[1]));
        }
    }

    let tick_5_y = tick_5_y.expect("tick 5");
    assert!(
        (tick_5_y - JAVA_TICK_5_Y).abs() < 1.0e-6,
        "tick 5 y {tick_5_y} != Java {JAVA_TICK_5_Y}"
    );

    assert_eq!(samples.len(), JAVA_TRACE.len());
    for ((tick, actual), (java_tick, expected)) in samples.iter().zip(JAVA_TRACE) {
        assert_eq!(*tick, java_tick);
        assert!(
            (actual - expected).abs() < 5.0e-4,
            "tick {tick}: Rust {actual} != Java {expected} (diff {})",
            actual - expected
        );
    }

    // The captured sync packet reports velocity after one further physics
    // tick, so compare it against the tick where the stand first passes the
    // Java sync position.
    let (velocity_y, velocity) = velocity_before_47.expect("velocity sample");
    assert!(
        (velocity_y - (-47.520_447_671_420_6)).abs() < 3.0,
        "unexpected velocity sample position {velocity_y}"
    );
    let next_velocity = (velocity - 0.08) * 0.98;
    assert!(
        (next_velocity - JAVA_VELOCITY_NEAR_47).abs() < 0.03,
        "velocity {next_velocity} != Java {JAVA_VELOCITY_NEAR_47}"
    );

    assert!(entity.on_ground, "armor stand should be on the ground");
    assert!(
        (entity.y - (-60.0)).abs() < 1.0e-9,
        "armor stand should land at y=-60, got {}",
        entity.y
    );
}

#[test]
fn flat_collider_blocks_one_block_below_ground_level() {
    let mut collider = FlatCollider { ground_y: -60 };
    assert!(
        !collider.is_solid(0, -60, 0),
        "air above the ground surface"
    );
    assert!(
        collider.is_solid(0, -61, 0),
        "grass block below the surface"
    );
}
