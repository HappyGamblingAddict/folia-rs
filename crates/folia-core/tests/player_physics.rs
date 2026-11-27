use folia_core::block_behaviour::MovementTraits;
use folia_core::fluid::{self, FluidKind, FluidState};
use folia_core::player_physics::{
    AIR_FRICTION, Aabb, CLIMB_MOVE_LIMIT, CLIMB_SPEED, COBWEB_MOVE_MULTIPLIER, ClientMove, DRAG,
    FREEZE_DAMAGE, FlatPhysicsWorld, GRAVITY, JUMP_IMPULSE, MAX_FREEZE_TICKS, MIN_FREEZE_TICKS,
    MovementCorrection, MovementInput, PhysicsWorld, PlayerPhysics, SNEAKING_HEIGHT,
    STANDING_HEIGHT, SWIMMING_HEIGHT, TERMINAL_VELOCITY, WATER_FLOW_ACCELERATION, WIDTH,
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

const LADDER: MovementTraits = MovementTraits {
    climbable: true,
    resets_fall_damage: true,
    ..MovementTraits::NONE
};
const SCAFFOLDING: MovementTraits = MovementTraits {
    climbable: true,
    scaffolding: true,
    resets_fall_damage: true,
    ..MovementTraits::NONE
};
const COBWEB: MovementTraits = MovementTraits {
    cobweb: true,
    resets_fall_damage: true,
    ..MovementTraits::NONE
};
const SLIME: MovementTraits = MovementTraits {
    slime: true,
    ..MovementTraits::NONE
};

/// A grid world that can tag blocks with the shipped movement traits. Blocks
/// are full cubes only when explicitly marked collidable; ladders/vines/cobwebs
/// are passable, matching their shipped collision metadata.
struct SpecialWorld {
    ground_y: i32,
    blocks: HashMap<(i32, i32, i32), (Option<Aabb>, MovementTraits)>,
}

impl SpecialWorld {
    fn new(ground_y: i32) -> Self {
        Self {
            ground_y,
            blocks: HashMap::new(),
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, traits: MovementTraits, collision: bool) {
        let boxed = collision.then(|| Aabb::block(x, y, z));
        self.blocks.insert((x, y, z), (boxed, traits));
    }

    fn ladder_wall(&mut self) {
        for y in 0..12 {
            for z in -1..=1 {
                self.set(3, y, z, MovementTraits::NONE, true);
            }
            self.set(2, y, 0, LADDER, false);
        }
    }

    fn slime_floor(&mut self) {
        for x in -1..=1 {
            for z in -1..=1 {
                self.set(x, -1, z, SLIME, true);
            }
        }
    }
}

impl PhysicsWorld for SpecialWorld {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        self.blocks
            .get(&(x, y, z))
            .and_then(|(bounds, _)| *bounds)
            .or_else(|| (y < self.ground_y).then(|| Aabb::block(x, y, z)))
    }

    fn movement_traits(&mut self, x: i32, y: i32, z: i32) -> MovementTraits {
        self.blocks
            .get(&(x, y, z))
            .map(|(_, traits)| *traits)
            .unwrap_or(MovementTraits::NONE)
    }

    fn friction(&mut self, x: i32, y: i32, z: i32) -> f64 {
        if self.movement_traits(x, y, z).slime {
            0.8
        } else {
            0.6
        }
    }
}

#[test]
fn climbing_a_ladder_lifts_the_player_at_the_vanilla_climb_rate() {
    let mut world = SpecialWorld::new(0);
    world.ladder_wall();
    let mut physics = PlayerPhysics::new(2.5, 0.0, 0.5);
    physics.on_ground = true;
    physics.yaw = -90.0;
    let input = MovementInput {
        forward: 1.0,
        jumping: true,
        ..MovementInput::default()
    };

    physics.fall_distance = 5.0;
    for _ in 0..40 {
        let outcome = physics.tick(&mut world, input);
        assert_eq!(outcome.fall_damage, 0.0);
    }
    assert!(
        physics.fall_distance < CLIMB_MOVE_LIMIT,
        "climbing clears the fall distance it had on entry, got {}",
        physics.fall_distance
    );

    assert!(
        (physics.y - 5.006_4).abs() < 1.0e-3,
        "climbed to y={} (0.42 then 39 ticks of 0.1176)",
        physics.y
    );
    assert!(
        (physics.velocity_y - (CLIMB_SPEED - GRAVITY) * DRAG).abs() < 1.0e-9,
        "steady climb velocity {}",
        physics.velocity_y
    );
}

#[test]
fn ladder_slide_down_is_clamped_and_sneaking_holds_still() {
    let mut world = SpecialWorld::new(0);
    world.ladder_wall();
    let mut sliding = PlayerPhysics::new(2.5, 10.0, 0.5);
    for _ in 0..30 {
        let outcome = sliding.tick(&mut world, MovementInput::default());
        assert_eq!(outcome.fall_damage, 0.0);
    }
    assert!(
        (sliding.velocity_y - (-CLIMB_MOVE_LIMIT - GRAVITY) * DRAG).abs() < 1.0e-9,
        "post-tick slide velocity {}",
        sliding.velocity_y
    );
    assert!(sliding.y < 9.5, "the player slid down to y={}", sliding.y);
    assert!(
        sliding.fall_distance <= CLIMB_MOVE_LIMIT + 1.0e-9,
        "a climber never accumulates a damaging fall, got {}",
        sliding.fall_distance
    );

    let mut sneaking = PlayerPhysics::new(2.5, 10.0, 0.5);
    sneaking.sneaking = true;
    for _ in 0..30 {
        sneaking.tick(&mut world, MovementInput::default());
    }
    assert!(
        (sneaking.y - 10.0).abs() < 1.0e-9,
        "sneaking on a ladder holds position, y={}",
        sneaking.y
    );
}

#[test]
fn sneaking_on_scaffolding_still_slides_down() {
    let mut world = SpecialWorld::new(0);
    for y in 0..12 {
        world.set(2, y, 0, SCAFFOLDING, false);
    }
    let mut physics = PlayerPhysics::new(2.5, 10.0, 0.5);
    physics.sneaking = true;
    for _ in 0..20 {
        physics.tick(&mut world, MovementInput::default());
    }
    assert!(
        (physics.velocity_y - (-CLIMB_MOVE_LIMIT - GRAVITY) * DRAG).abs() < 1.0e-9,
        "post-tick scaffolding slide velocity {}",
        physics.velocity_y
    );
    assert!(physics.y < 9.0, "scaffolding ignores sneak suppression");
}

#[test]
fn cobweb_scales_horizontal_movement() {
    let mut flat = FlatPhysicsWorld::new(0);
    let mut web = SpecialWorld::new(0);
    web.set(0, 0, 0, COBWEB, false);

    let mut normal = PlayerPhysics::new(0.5, 0.0, 0.5);
    let mut stuck = PlayerPhysics::new(0.5, 0.0, 0.5);
    normal.on_ground = true;
    stuck.on_ground = true;
    normal.yaw = -90.0;
    stuck.yaw = -90.0;
    stuck.cobweb_stuck = true;

    for _ in 0..20 {
        normal.tick(&mut flat, MovementInput::walking_forward());
        stuck.tick(&mut web, MovementInput::walking_forward());
    }

    assert!(
        (normal.x - (0.5 + 4.140_388_435_264_874)).abs() < 1.0e-4,
        "normal walk x={}",
        normal.x
    );
    assert!(
        (stuck.x - (0.5 + 20.0 * 0.1 * COBWEB_MOVE_MULTIPLIER.0)).abs() < 1.0e-6,
        "cobweb walk x={}",
        stuck.x
    );
    assert!(stuck.x < normal.x / 4.0);
}

#[test]
fn cobweb_slows_a_fall_and_cancels_fall_damage() {
    let mut world = SpecialWorld::new(-100);
    for y in 0..10 {
        world.set(0, y, 0, COBWEB, false);
    }
    let mut physics = PlayerPhysics::new(0.5, 9.0, 0.5);
    physics.cobweb_stuck = true;
    let mut damage = 0.0;
    for _ in 0..20 {
        damage += physics
            .tick(&mut world, MovementInput::default())
            .fall_damage;
    }
    assert!(
        9.0 - physics.y < 0.2,
        "the web holds the fall to y={}",
        physics.y
    );
    assert_eq!(damage, 0.0);
    assert_eq!(physics.fall_distance, 0.0);
}

#[test]
fn slime_block_reverses_fall_velocity_and_cancels_fall_damage() {
    let mut world = SpecialWorld::new(0);
    world.slime_floor();
    let mut physics = PlayerPhysics::new(0.5, 40.0, 0.5);
    let mut bounced_velocity = 0.0;
    let mut damage = 0.0;
    let mut landed_at = f64::NAN;
    for _ in 0..100 {
        let outcome = physics.tick(&mut world, MovementInput::default());
        damage += outcome.fall_damage;
        if physics.velocity_y > 0.0 {
            bounced_velocity = physics.velocity_y;
            landed_at = physics.y;
            break;
        }
    }
    assert!(bounced_velocity > 1.0, "bounce velocity {bounced_velocity}");
    assert!(
        landed_at.abs() < 1.0e-9,
        "the bounce happens on the slime surface, y={landed_at}"
    );
    assert_eq!(damage, 0.0);

    let mut flat = FlatPhysicsWorld::new(0);
    let mut normal = PlayerPhysics::new(0.5, 40.0, 0.5);
    let mut normal_damage = 0.0;
    for _ in 0..100 {
        normal_damage += normal.tick(&mut flat, MovementInput::default()).fall_damage;
    }
    assert_eq!(
        normal_damage, 37.0,
        "the same fall hurts on ordinary ground"
    );
}

#[test]
fn slime_step_slows_horizontal_motion() {
    let mut flat = FlatPhysicsWorld::new(0);
    let mut slime = SpecialWorld::new(0);
    slime.slime_floor();
    let mut normal = PlayerPhysics::new(0.5, 0.0, 0.5);
    let mut slowed = PlayerPhysics::new(0.5, 0.0, 0.5);
    normal.on_ground = true;
    slowed.on_ground = true;
    normal.yaw = -90.0;
    slowed.yaw = -90.0;

    for _ in 0..20 {
        normal.tick(&mut flat, MovementInput::walking_forward());
        slowed.tick(&mut slime, MovementInput::walking_forward());
    }
    assert!(
        slowed.x < normal.x * 0.8,
        "slime step x={} vs normal x={}",
        slowed.x,
        normal.x
    );
}

#[test]
fn special_movement_is_deterministic() {
    fn trace() -> Vec<(f64, f64, f64, f64)> {
        let mut world = SpecialWorld::new(0);
        world.ladder_wall();
        let mut physics = PlayerPhysics::new(2.5, 0.0, 0.5);
        physics.on_ground = true;
        physics.yaw = -90.0;
        let input = MovementInput {
            forward: 1.0,
            jumping: true,
            ..MovementInput::default()
        };
        (0..30)
            .map(|_| {
                physics.tick(&mut world, input);
                (physics.x, physics.y, physics.z, physics.velocity_y)
            })
            .collect()
    }
    assert_eq!(trace(), trace());
}

#[derive(Clone, Copy, Default)]
struct FluidEntry {
    fluid: Option<FluidState>,
    bubble_drag: Option<bool>,
    powder_snow: bool,
    traits: MovementTraits,
    collision: bool,
}

/// A grid world carrying shipped-style fluid states, bubble columns, and
/// powder snow so the fluid branch of `PlayerPhysics::tick` can be exercised
/// without a generated chunk.
#[derive(Clone)]
struct FluidWorld {
    ground_y: i32,
    blocks: HashMap<(i32, i32, i32), FluidEntry>,
}

impl FluidWorld {
    fn new(ground_y: i32) -> Self {
        Self {
            ground_y,
            blocks: HashMap::new(),
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, entry: FluidEntry) {
        self.blocks.insert((x, y, z), entry);
    }

    fn water_column(&mut self, x: i32, y: i32, z: i32, height: i32) {
        for level in 0..height {
            self.set(
                x,
                y + level,
                z,
                FluidEntry {
                    fluid: Some(FluidState::source(FluidKind::Water)),
                    ..FluidEntry::default()
                },
            );
        }
    }
}

impl PhysicsWorld for FluidWorld {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        self.blocks
            .get(&(x, y, z))
            .is_some_and(|entry| entry.collision)
            .then(|| Aabb::block(x, y, z))
            .or_else(|| (y < self.ground_y).then(|| Aabb::block(x, y, z)))
    }

    fn movement_traits(&mut self, x: i32, y: i32, z: i32) -> MovementTraits {
        self.blocks
            .get(&(x, y, z))
            .map_or(MovementTraits::NONE, |entry| entry.traits)
    }

    fn fluid_state(&mut self, x: i32, y: i32, z: i32) -> Option<FluidState> {
        self.blocks.get(&(x, y, z)).and_then(|entry| entry.fluid)
    }

    fn bubble_column_drag(&mut self, x: i32, y: i32, z: i32) -> Option<bool> {
        self.blocks
            .get(&(x, y, z))
            .and_then(|entry| entry.bubble_drag)
    }

    fn is_powder_snow(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.blocks
            .get(&(x, y, z))
            .is_some_and(|entry| entry.powder_snow)
    }
}

#[test]
fn flowing_water_pushes_the_player_downstream() {
    let still_water = FluidState::source(FluidKind::Water);
    let lower_water = fluid::fluid_state(&folia_core::chunk::BlockState {
        name: "minecraft:water".into(),
        properties: vec![("level".into(), "1".into())],
    })
    .unwrap();

    let mut still = FluidWorld::new(0);
    still.water_column(0, 0, 0, 1);
    for (x, z) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        still.water_column(x, 0, z, 1);
    }
    let mut flowing = still.clone();
    flowing.set(
        1,
        0,
        0,
        FluidEntry {
            fluid: Some(lower_water),
            ..FluidEntry::default()
        },
    );

    let mut still_player = PlayerPhysics::new(0.5, 0.0, 0.5);
    let mut flowing_player = PlayerPhysics::new(0.5, 0.0, 0.5);
    for _ in 0..30 {
        still_player.tick(&mut still, MovementInput::default());
        flowing_player.tick(&mut flowing, MovementInput::default());
    }

    assert!(
        (still_player.x - 0.5).abs() < 1.0e-9,
        "level water has no flow, x={}",
        still_player.x
    );
    assert!(
        flowing_player.x > 0.5 + WATER_FLOW_ACCELERATION,
        "water flowing east pushes the player downstream, x={}",
        flowing_player.x
    );
    assert!((flowing_player.z - 0.5).abs() < 1.0e-9);
    assert_eq!(flowing_player.fall_distance, 0.0);
    assert_eq!(still_water.amount, lower_water.amount + 1);
}

#[test]
fn lava_is_much_slower_than_air_and_damps_velocity_harder() {
    let mut lava = FluidWorld::new(0);
    for x in -2..=4 {
        for z in -1..=1 {
            lava.set(
                x,
                0,
                z,
                FluidEntry {
                    fluid: Some(FluidState::source(FluidKind::Lava)),
                    ..FluidEntry::default()
                },
            );
        }
    }
    let mut air = FluidWorld::new(0);

    let mut swimmers = PlayerPhysics::new(0.5, 0.0, 0.5);
    let mut walkers = PlayerPhysics::new(0.5, 0.0, 0.5);
    swimmers.on_ground = true;
    walkers.on_ground = true;
    swimmers.yaw = -90.0;
    walkers.yaw = -90.0;
    for _ in 0..20 {
        swimmers.tick(&mut lava, MovementInput::walking_forward());
        walkers.tick(&mut air, MovementInput::walking_forward());
    }

    let lava_distance = swimmers.x - 0.5;
    let air_distance = walkers.x - 0.5;
    assert!(
        lava_distance > 0.0 && lava_distance < air_distance * 0.25,
        "lava distance {lava_distance} vs air {air_distance}"
    );
    assert!(
        swimmers.velocity_y.abs() < walkers.velocity_y.abs(),
        "lava drag {} vs air {}",
        swimmers.velocity_y,
        walkers.velocity_y
    );
}

#[test]
fn bubble_columns_lift_and_pull_the_player() {
    fn trace(drag_down: bool) -> (f64, f64) {
        let mut world = FluidWorld::new(-100);
        for y in -10..=10 {
            world.set(
                0,
                y,
                0,
                FluidEntry {
                    fluid: Some(FluidState::source(FluidKind::Water)),
                    bubble_drag: Some(drag_down),
                    ..FluidEntry::default()
                },
            );
            for (x, z) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                world.water_column(x, y, z, 1);
            }
        }
        let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
        for _ in 0..15 {
            physics.tick(&mut world, MovementInput::default());
        }
        (physics.y, physics.velocity_y)
    }

    let (up_y, up_velocity) = trace(false);
    let (down_y, down_velocity) = trace(true);
    assert!(up_y > 0.5, "soul sand lifts the player to y={up_y}");
    assert!(up_velocity > 0.0, "up velocity {up_velocity}");
    assert!(
        down_y < 0.0 && down_velocity < 0.0,
        "magma pulls the player to y={down_y}, velocity {down_velocity}"
    );
    assert!(
        fluid::bubble_column_velocity(false, fluid::BubbleColumnPlacement::Inside, 0.0) > 0.0
            && fluid::bubble_column_velocity(true, fluid::BubbleColumnPlacement::Inside, 0.0) < 0.0
    );
}

#[test]
fn powder_snow_sinks_slowly_and_freezes_the_player() {
    let mut snow = FluidWorld::new(-100);
    for y in -20..=3 {
        snow.set(
            0,
            y,
            0,
            FluidEntry {
                powder_snow: true,
                ..FluidEntry::default()
            },
        );
    }
    let mut air = FluidWorld::new(-100);
    let mut sunk = PlayerPhysics::new(0.5, 3.0, 0.5);
    let mut falling = PlayerPhysics::new(0.5, 3.0, 0.5);

    let mut damage = 0.0;
    for _ in 0..45 {
        damage += sunk.tick(&mut snow, MovementInput::default()).freeze_damage;
        falling.tick(&mut air, MovementInput::default());
    }
    assert!(
        sunk.y > falling.y + 1.0,
        "powder snow slows the sink: y={} vs air {}",
        sunk.y,
        falling.y
    );
    assert_eq!(sunk.fall_distance, 0.0);
    assert_eq!(sunk.ticks_frozen, MAX_FREEZE_TICKS);
    assert!(sunk.ticks_frozen > MIN_FREEZE_TICKS);
    assert!(
        damage >= FREEZE_DAMAGE,
        "sustained exposure deals freeze damage, got {damage}"
    );

    sunk.y = 20.0;
    let before = sunk.ticks_frozen;
    for _ in 0..10 {
        sunk.tick(&mut snow, MovementInput::default());
    }
    assert_eq!(
        sunk.ticks_frozen,
        before - 10,
        "freezing thaws outside snow"
    );
}

#[test]
fn fluid_and_powder_snow_movement_is_deterministic() {
    fn trace() -> Vec<(f64, f64, f64, u16)> {
        let mut world = FluidWorld::new(0);
        world.set(
            0,
            0,
            0,
            FluidEntry {
                fluid: Some(FluidState::source(FluidKind::Water)),
                bubble_drag: Some(false),
                ..FluidEntry::default()
            },
        );
        world.set(
            1,
            0,
            0,
            FluidEntry {
                fluid: Some(FluidState::source(FluidKind::Lava)),
                ..FluidEntry::default()
            },
        );
        world.set(
            0,
            1,
            0,
            FluidEntry {
                powder_snow: true,
                ..FluidEntry::default()
            },
        );
        let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
        (0..40)
            .map(|_| {
                physics.tick(
                    &mut world,
                    MovementInput {
                        forward: 0.5,
                        strafe: -0.25,
                        jumping: false,
                    },
                );
                (physics.x, physics.y, physics.z, physics.ticks_frozen)
            })
            .collect()
    }
    assert_eq!(trace(), trace());
}
