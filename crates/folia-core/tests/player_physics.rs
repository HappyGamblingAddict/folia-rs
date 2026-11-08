use folia_core::player_physics::{
    AIR_FRICTION, Aabb, ClientMove, DRAG, FlatPhysicsWorld, GRAVITY, JUMP_IMPULSE,
    MovementCorrection, MovementInput, PhysicsWorld, PlayerPhysics, SNEAKING_HEIGHT,
    STANDING_HEIGHT, SWIMMING_HEIGHT, TERMINAL_VELOCITY, WIDTH,
};
use std::collections::HashMap;

struct GridWorld {
    ground_y: i32,
    blocks: HashMap<(i32, i32, i32), Aabb>,
}

impl GridWorld {
    fn new(ground_y: i32) -> Self {
        Self {
            ground_y,
            blocks: HashMap::new(),
        }
    }

    fn insert(&mut self, x: i32, y: i32, z: i32, block: Aabb) {
        self.blocks.insert((x, y, z), block);
    }

    fn wall(&mut self, at_x: i32, height: i32) {
        for y in 0..height {
            for z in -1..=1 {
                self.insert(at_x, y, z, Aabb::block(at_x, y, z));
            }
        }
    }
}

impl PhysicsWorld for GridWorld {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        if let Some(block) = self.blocks.get(&(x, y, z)) {
            return Some(*block);
        }
        (y < self.ground_y).then(|| Aabb::block(x, y, z))
    }
}

fn client_move(x: f64, y: f64, z: f64) -> ClientMove {
    ClientMove {
        x,
        y,
        z,
        on_ground: true,
        yaw: None,
        pitch: None,
    }
}

fn walk(physics: &mut PlayerPhysics, world: &mut impl PhysicsWorld, ticks: usize) {
    for _ in 0..ticks {
        physics.tick(world, MovementInput::walking_forward());
    }
}

#[test]
fn player_hitbox_matches_avatar_dimensions() {
    let mut physics = PlayerPhysics::new(0.0, 0.0, 0.0);
    assert_eq!(physics.width(), WIDTH);
    assert_eq!(physics.height(), STANDING_HEIGHT);
    let aabb = physics.aabb();
    assert_eq!(
        (aabb.min_x, aabb.max_x, aabb.min_y, aabb.max_y),
        (-0.3, 0.3, 0.0, 1.8)
    );

    physics.sneaking = true;
    assert_eq!(physics.height(), SNEAKING_HEIGHT);
    assert_eq!(physics.aabb().max_y, 1.5);

    physics.swimming = true;
    assert_eq!(physics.height(), SWIMMING_HEIGHT);
    assert_eq!(physics.aabb().max_y, 0.6);
}

#[test]
fn gravity_and_drag_use_vanilla_constants() {
    let mut physics = PlayerPhysics::new(0.5, 10.0, 0.5);
    let mut world = FlatPhysicsWorld::new(-100);

    let outcome = physics.tick(&mut world, MovementInput::default());
    assert!(
        !outcome.moved,
        "zero velocity does not move in the first tick"
    );
    assert_eq!(physics.y, 10.0);
    assert!((physics.velocity_y - (-GRAVITY * DRAG)).abs() < 1.0e-12);

    physics.tick(&mut world, MovementInput::default());
    assert!((physics.y - (10.0 - GRAVITY * DRAG)).abs() < 1.0e-12);
    assert!(
        (physics.velocity_y - ((-GRAVITY * DRAG) - GRAVITY) * DRAG).abs() < 1.0e-12,
        "velocity {}",
        physics.velocity_y
    );
}

#[test]
fn terminal_velocity_converges_to_the_vanilla_fixed_point() {
    let mut physics = PlayerPhysics::new(0.5, 1_000_000.0, 0.5);
    let mut world = FlatPhysicsWorld::new(-1_000_000);
    for _ in 0..2_000 {
        physics.tick(&mut world, MovementInput::default());
    }
    assert!(
        (physics.velocity_y - TERMINAL_VELOCITY).abs() < 1.0e-6,
        "terminal velocity {}",
        physics.velocity_y
    );
}

#[test]
fn simulated_walk_matches_the_vanilla_acceleration_recurrence() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    physics.yaw = -90.0; // face east (+X)
    let mut world = FlatPhysicsWorld::new(0);

    // Ground input is speed * 0.216 / friction^3 = 0.1, then the movement is
    // scaled by friction * 0.91 each tick. The per-tick displacement is the
    // sum of that recurrence; the tiny 1.00000009 factor from 0.21600002 is
    // included by the tolerance.
    let mut velocity = 0.0;
    let mut expected = 0.0;
    for _ in 0..20 {
        velocity += 0.1;
        expected += velocity;
        velocity *= 0.6 * AIR_FRICTION;
        physics.tick(&mut world, MovementInput::walking_forward());
    }

    assert!(
        (physics.x - (0.5 + expected)).abs() < 1.0e-4,
        "x={} expected={}",
        physics.x,
        0.5 + expected
    );
    assert!(
        (physics.x - (0.5 + 4.140_388_435_264_874)).abs() < 1.0e-4,
        "reference displacement x={}",
        physics.x
    );
    assert!((physics.z - 0.5).abs() < 1.0e-12);
    assert!(physics.on_ground);
}

#[test]
fn walking_into_a_wall_stops_at_the_hitbox() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    physics.yaw = -90.0;
    let mut world = GridWorld::new(0);
    world.wall(3, 2);

    walk(&mut physics, &mut world, 100);

    assert!(
        (physics.x - 2.7).abs() < 1.0e-6,
        "player stopped at x={}",
        physics.x
    );
    assert_eq!(physics.y, 0.0);
    assert!(physics.on_ground);
    assert_eq!(physics.velocity_x, 0.0);
}

#[test]
fn step_height_climbs_a_half_block() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    physics.yaw = -90.0;
    let mut world = GridWorld::new(0);
    for x in 1..=20 {
        let min_x = f64::from(x);
        world.insert(x, 0, 0, Aabb::new(min_x, 0.0, 0.0, min_x + 1.0, 0.5, 1.0));
    }

    walk(&mut physics, &mut world, 40);

    assert!(
        physics.x > 2.0,
        "player crossed the half block, x={}",
        physics.x
    );
    assert!(
        (physics.y - 0.5).abs() < 1.0e-9,
        "player stands on the half block, y={}",
        physics.y
    );
    assert!(physics.on_ground);
}

#[test]
fn jump_apex_matches_the_vanilla_impulse() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    let mut world = FlatPhysicsWorld::new(0);

    physics.tick(
        &mut world,
        MovementInput {
            jumping: true,
            ..MovementInput::default()
        },
    );
    let mut apex = physics.y;
    for _ in 0..30 {
        physics.tick(&mut world, MovementInput::default());
        apex = apex.max(physics.y);
    }

    assert!(
        (apex - 1.252_203_352_512).abs() < 1.0e-9,
        "jump apex {apex}"
    );
    assert!(physics.on_ground, "the player lands again");
}

#[test]
fn falling_accumulates_distance_and_landing_applies_damage() {
    let mut physics = PlayerPhysics::new(0.5, 10.0, 0.5);
    let mut world = FlatPhysicsWorld::new(0);
    let mut damage = 0.0;
    let mut ticks = 0;
    while !physics.on_ground && ticks < 200 {
        let outcome = physics.tick(&mut world, MovementInput::default());
        damage += outcome.fall_damage;
        ticks += 1;
    }

    assert!(physics.on_ground);
    assert_eq!(physics.y, 0.0);
    assert_eq!(damage, 7.0, "ten blocks minus three safe blocks");
    assert_eq!(
        physics.fall_distance, 0.0,
        "landing resets the accumulated fall distance"
    );

    let mut short_fall = PlayerPhysics::new(0.5, 2.0, 0.5);
    let mut short_damage = 0.0;
    for _ in 0..100 {
        short_damage += short_fall
            .tick(&mut world, MovementInput::default())
            .fall_damage;
    }
    assert_eq!(short_damage, 0.0);
    assert!(short_fall.on_ground);
}

#[test]
fn legal_walking_is_not_rubber_banded() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    let mut world = FlatPhysicsWorld::new(0);

    let mut client_x = 0.5;
    for _ in 0..30 {
        client_x += 0.2;
        let reconciliation = physics.reconcile(&mut world, &client_move(client_x, 0.0, 0.5));
        assert_eq!(reconciliation.correction, None);
        assert!((physics.x - client_x).abs() < 1.0e-12);
        assert!(reconciliation.on_ground);
    }
}

#[test]
fn a_small_move_through_a_wall_is_rejected_and_corrects_the_client() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    let mut world = GridWorld::new(0);
    world.wall(3, 2);

    let legal = physics.reconcile(&mut world, &client_move(0.8, 0.0, 0.5));
    assert_eq!(legal.correction, None);
    assert!((physics.x - 0.8).abs() < 1.0e-12);

    // A long legal burst may be accepted up to the wall face.
    let legal = physics.reconcile(&mut world, &client_move(2.7, 0.0, 0.5));
    assert_eq!(legal.correction, None);
    assert!((physics.x - 2.7).abs() < 1.0e-12);

    let blocked = physics.reconcile(&mut world, &client_move(3.2, 0.0, 0.5));
    assert_eq!(blocked.correction, Some(MovementCorrection::Blocked));
    assert!(
        (physics.x - 2.7).abs() < 1.0e-12,
        "collision keeps the server at the wall, x={}",
        physics.x
    );
    assert_eq!(
        physics.reported_position(),
        (physics.x, physics.y, physics.z),
        "the next packet is measured from the correction"
    );

    let too_far = physics.reconcile(&mut world, &client_move(500.0, 0.0, 0.5));
    assert!(matches!(
        too_far.correction,
        Some(MovementCorrection::TooFar { .. })
    ));
    assert!((physics.x - 2.7).abs() < 1.0e-12);
}

#[test]
fn a_stalled_client_can_catch_up_with_the_time_scaled_budget() {
    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    let mut world = FlatPhysicsWorld::new(0);

    let rejected = physics.reconcile(&mut world, &client_move(500.0, 0.0, 0.5));
    assert!(matches!(
        rejected.correction,
        Some(MovementCorrection::TooFar { .. })
    ));

    let accepted = physics.reconcile_with_allowance(
        &mut world,
        &client_move(500.0, 0.0, 0.5),
        1_000.0 * 1_000.0,
    );
    assert!(
        !matches!(accepted.correction, Some(MovementCorrection::TooFar { .. })),
        "a long stall may legitimately move hundreds of blocks"
    );
    assert!((physics.x - 500.0).abs() < 1.0e-9);
}

#[test]
fn burst_moves_validate_the_destination_and_lift_out_of_terrain() {
    let mut physics = PlayerPhysics::new(2.7, 0.0, 0.5);
    physics.on_ground = true;
    let mut world = GridWorld::new(0);
    world.wall(3, 2);

    // 1.3 blocks in one packet is a lag burst; the destination is inside the
    // two-block wall, so it is lifted to the first free position.
    let reconciliation = physics.reconcile(&mut world, &client_move(4.0, 0.0, 0.5));
    assert_eq!(
        reconciliation.correction,
        Some(MovementCorrection::Adjusted)
    );
    assert!((physics.x - 4.0).abs() < 1.0e-12);
    assert!(
        (physics.y - 2.0).abs() < 1.0e-12,
        "lifted above the wall, y={}",
        physics.y
    );
    assert!(
        world.collision_box(4, 2, 0).is_none(),
        "the lifted player is outside the wall"
    );
}

#[test]
fn jump_impulse_and_air_friction_are_vanilla_constants() {
    assert_eq!(JUMP_IMPULSE, 0.42);
    assert_eq!(GRAVITY, 0.08);
    assert_eq!(DRAG, 0.98);
    assert_eq!(AIR_FRICTION, 0.91);
}
