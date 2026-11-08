//! Server-side player physics and movement reconciliation.
//!
//! The Java client sends its own position every tick; vanilla's server mostly
//! trusts it and only applies coarse anti-cheat checks. This module keeps a
//! simulated player state that is advanced from those packets, collides the
//! movement against the shared world storage, and reports a correction when the
//! client's claim cannot be reproduced.
//!
//! The formulas follow `LivingEntity.travelInAir`, `Entity.move`,
//! `Entity.moveRelative`, `Entity.checkFallDamage`, and `Player.travel` for
//! 1.21.11: gravity `0.08`, drag `0.98`, ground friction, speed and jump
//! factors from the generated [`crate::block_behaviour`] table and collision
//! boxes from the generated [`crate::block_shapes`] table. Features that need
//! movement metadata this port does not carry yet (ladders, cobwebs, slime
//! bounce, elytra, water/lava currents, bubbles, powder snow, scaffolding) are
//! intentionally not simulated; see `DECISIONS.md` for the exact remaining
//! gaps.

use crate::chunk::{BlockState, Chunk};
use crate::world::WorldStorage;
use std::cmp::Ordering;
use std::collections::HashMap;

pub use crate::block_shapes::Aabb;

/// Vanilla living-entity gravity (`Attributes.GRAVITY` default).
pub const GRAVITY: f64 = 0.08;
/// Vertical drag applied after gravity every tick.
pub const DRAG: f64 = 0.98;
/// Horizontal drag applied by `LivingEntity.travelInAir` (`0.91` times the
/// block friction).
pub const AIR_FRICTION: f64 = 0.91;
/// Default block friction (`BlockBehaviour`).
pub const DEFAULT_BLOCK_FRICTION: f64 = 0.6;
/// Player jump impulse (`Attributes.JUMP_STRENGTH` default).
pub const JUMP_IMPULSE: f64 = 0.42;
/// Vanilla player step height (`Attributes.STEP_HEIGHT` default).
pub const STEP_HEIGHT: f64 = 0.6;
/// Player hitbox width; `Avatar.STANDING_DIMENSIONS`.
pub const WIDTH: f64 = 0.6;
/// Standing height; `Avatar.STANDING_DIMENSIONS`.
pub const STANDING_HEIGHT: f64 = 1.8;
/// Sneaking height; `Avatar.CROUCHING`.
pub const SNEAKING_HEIGHT: f64 = 1.5;
/// Swimming/gliding height; `Avatar.SWIMMING`.
pub const SWIMMING_HEIGHT: f64 = 0.6;
/// Base `Attributes.MOVEMENT_SPEED` for a player.
pub const BASE_MOVEMENT_SPEED: f64 = 0.1;
/// Sprinting adds `+30%` movement speed (`LivingEntity.SPEED_MODIFIER_SPRINTING`).
pub const SPRINT_SPEED_MULTIPLIER: f64 = 1.3;
/// Sneaking speed is `30%` of walking speed (`Attributes.SNEAKING_SPEED`).
pub const SNEAK_SPEED_MULTIPLIER: f64 = 0.3;
/// Creative flight speed (`PlayerAbilities.flyingSpeed`).
pub const FLYING_SPEED: f64 = 0.05;
/// Creative sprint-flight speed (`PlayerAbilities.flyingSpeed * 2`).
pub const SPRINT_FLYING_SPEED: f64 = 0.1;
/// Vanilla `Attributes.SAFE_FALL_DISTANCE` default.
pub const SAFE_FALL_DISTANCE: f64 = 3.0;
/// Fixed point of `(v - 0.08) * 0.98`; vanilla does not clamp explicitly.
pub const TERMINAL_VELOCITY: f64 = -3.92;
/// Largest one-packet displacement the existing safety net accepts.
pub const MAX_MOVEMENT_DISTANCE_SQUARED: f64 = 100.0;
/// A client claim and the simulation closer than this are treated as the same
/// position.
pub const POSITION_TOLERANCE: f64 = 0.0625;
/// Moves longer than this in one packet are treated as a client-side lag burst:
/// the destination is validated, but the intermediate path is not replayed
/// tick-by-tick. Vanilla's fastest grounded horizontal movement is well below
/// this (a sprint jump is roughly 0.36 blocks per tick).
pub const MAX_SIMULATED_HORIZONTAL_MOVE: f64 = 0.75;
/// How far a burst move may be lifted to put the player in free space.
pub const MAX_SETTLE_LIFT: f64 = 8.0;
/// Candidate lift spacing for burst moves.
const SETTLE_STEP: f64 = 0.25;
const EPSILON: f64 = 1.0e-7;

/// World queries player physics needs. Implemented by the shared world storage
/// and by the flat/test worlds used in focused tests.
pub trait PhysicsWorld {
    /// The first collision box of the block at `(x, y, z)`, or `None` when the
    /// block has no collision (air, fluids, replaceable plants, ...).
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb>;
    /// Every collision box of the block at `(x, y, z)`.
    ///
    /// Most blocks have a single full cube, but slabs, stairs, fences, walls,
    /// panes, snow layers, and similar states return multiple or partial
    /// boxes. The default keeps simple test worlds working by wrapping
    /// [`Self::collision_box`].
    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        self.collision_box(x, y, z).into_iter().collect()
    }
    /// Block friction of the supporting block.
    fn friction(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        DEFAULT_BLOCK_FRICTION
    }
    /// `Block.getSpeedFactor()` for the block at this position.
    fn speed_factor(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        1.0
    }
    /// `Block.getJumpFactor()` for the block at this position.
    fn jump_factor(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        1.0
    }
    /// Whether the block's behaviour reports `blocksMotion()`.
    fn blocks_motion(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.collision_box(x, y, z).is_some()
    }
    /// Whether the block's behaviour reports `isSolid()`.
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.collision_box(x, y, z).is_some()
    }
}

/// A simple flat world for tests and for servers without a configured level.
pub struct FlatPhysicsWorld {
    pub ground_y: i32,
    pub friction: f64,
    pub speed_factor: f64,
    pub jump_factor: f64,
}

impl FlatPhysicsWorld {
    pub fn new(ground_y: i32) -> Self {
        Self {
            ground_y,
            friction: DEFAULT_BLOCK_FRICTION,
            speed_factor: 1.0,
            jump_factor: 1.0,
        }
    }
}

impl PhysicsWorld for FlatPhysicsWorld {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        (y < self.ground_y).then(|| Aabb::block(x, y, z))
    }

    fn friction(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        self.friction
    }

    fn speed_factor(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        self.speed_factor
    }

    fn jump_factor(&mut self, _x: i32, _y: i32, _z: i32) -> f64 {
        self.jump_factor
    }
}

/// Physics over the shared overworld storage. Chunks are cached for the
/// duration of one movement packet so a single move does not decode the same
/// column repeatedly.
pub struct StoragePhysicsWorld<'a> {
    storage: &'a mut WorldStorage,
    chunks: HashMap<(i32, i32), Option<Chunk>>,
}

impl<'a> StoragePhysicsWorld<'a> {
    pub fn new(storage: &'a mut WorldStorage) -> Self {
        Self {
            storage,
            chunks: HashMap::new(),
        }
    }

    fn chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Option<&Chunk> {
        let key = (chunk_x, chunk_z);
        if !self.chunks.contains_key(&key) {
            let chunk = self.storage.load_chunk(chunk_x, chunk_z).ok().flatten();
            self.chunks.insert(key, chunk);
        }
        self.chunks.get(&key).and_then(Option::as_ref)
    }

    fn block_state(&mut self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        let chunk_x = x.div_euclid(16);
        let chunk_z = z.div_euclid(16);
        let chunk = self.chunk(chunk_x, chunk_z)?;
        let local_x = x.rem_euclid(16) as usize;
        let local_z = z.rem_euclid(16) as usize;
        chunk.block_state(local_x, y, local_z)
    }
}

impl PhysicsWorld for StoragePhysicsWorld<'_> {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        self.collision_boxes(x, y, z).into_iter().next()
    }

    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        let Some(state) = self.block_state(x, y, z) else {
            return Vec::new();
        };
        let state_id = crate::registry::block_state_id(state);
        crate::block_shapes::collision_boxes(state_id)
            .iter()
            .map(|bounds| bounds.offset(f64::from(x), f64::from(y), f64::from(z)))
            .collect()
    }

    fn friction(&mut self, x: i32, y: i32, z: i32) -> f64 {
        self.block_state(x, y, z)
            .map_or(DEFAULT_BLOCK_FRICTION, |state| {
                f64::from(crate::block_behaviour::friction(state))
            })
    }

    fn speed_factor(&mut self, x: i32, y: i32, z: i32) -> f64 {
        self.block_state(x, y, z).map_or(1.0, |state| {
            f64::from(crate::block_behaviour::speed_factor(state))
        })
    }

    fn jump_factor(&mut self, x: i32, y: i32, z: i32) -> f64 {
        self.block_state(x, y, z).map_or(1.0, |state| {
            f64::from(crate::block_behaviour::jump_factor(state))
        })
    }

    fn blocks_motion(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.block_state(x, y, z)
            .is_some_and(crate::block_behaviour::blocks_motion)
    }

    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.block_state(x, y, z)
            .is_some_and(crate::block_behaviour::is_solid)
    }
}

/// Player movement input for one tick. Vanilla's server reads this from
/// `ServerboundPlayerInput`; until that packet is ported, the live session
/// derives movement from the client's position packet and this struct is used
/// by the simulation tests and future tick integration.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MovementInput {
    /// Forward/back input, `-1.0..=1.0`.
    pub forward: f64,
    /// Left/right input, `-1.0..=1.0`.
    pub strafe: f64,
    pub jumping: bool,
}

impl MovementInput {
    pub fn walking_forward() -> Self {
        Self {
            forward: 1.0,
            ..Self::default()
        }
    }
}

/// A movement packet's claimed state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientMove {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: bool,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
}

/// Why the simulation did not accept a claimed client position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MovementCorrection {
    /// The packet moved more than ten blocks; the existing safety net applies.
    TooFar { distance_squared: f64 },
    /// Collision stopped the player before the claimed position.
    Blocked,
    /// A burst move landed inside terrain and was lifted to free space.
    Adjusted,
}

impl MovementCorrection {
    pub fn label(self) -> &'static str {
        match self {
            Self::TooFar { .. } => "too far",
            Self::Blocked => "blocked by terrain",
            Self::Adjusted => "adjusted out of terrain",
        }
    }
}

/// Result of one simulated tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickOutcome {
    pub moved: bool,
    pub on_ground: bool,
    pub landed: bool,
    pub fall_damage: f32,
}

/// Result of reconciling one client movement packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reconciliation {
    pub moved: bool,
    pub on_ground: bool,
    pub landed: bool,
    pub fall_damage: f32,
    pub correction: Option<MovementCorrection>,
}

/// One player's server-authoritative physics state.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPhysics {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub velocity_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub sprinting: bool,
    pub sneaking: bool,
    pub flying: bool,
    pub swimming: bool,
    pub fall_distance: f64,
    /// Last position reported by the client. Deltas are measured from here,
    /// matching how the server tracks a session's last accepted position.
    reported_x: f64,
    reported_y: f64,
    reported_z: f64,
}

impl PlayerPhysics {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z,
            velocity_x: 0.0,
            velocity_y: 0.0,
            velocity_z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            sprinting: false,
            sneaking: false,
            flying: false,
            swimming: false,
            fall_distance: 0.0,
            reported_x: x,
            reported_y: y,
            reported_z: z,
        }
    }

    pub fn width(&self) -> f64 {
        WIDTH
    }

    pub fn height(&self) -> f64 {
        if self.swimming {
            SWIMMING_HEIGHT
        } else if self.sneaking {
            SNEAKING_HEIGHT
        } else {
            STANDING_HEIGHT
        }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::player(self.x, self.y, self.z, self.width(), self.height())
    }

    pub fn reported_position(&self) -> (f64, f64, f64) {
        (self.reported_x, self.reported_y, self.reported_z)
    }

    /// Place the player without simulating movement. Used by login settling,
    /// respawn, and command teleports.
    pub fn teleport(&mut self, x: f64, y: f64, z: f64) {
        self.x = x;
        self.y = y;
        self.z = z;
        self.velocity_x = 0.0;
        self.velocity_y = 0.0;
        self.velocity_z = 0.0;
        self.fall_distance = 0.0;
        self.on_ground = false;
        self.reported_x = x;
        self.reported_y = y;
        self.reported_z = z;
    }

    /// Vanilla `Player.getSpeed()` with the sprint/sneak modifiers applied.
    pub fn movement_speed(&self) -> f64 {
        let modifier = if self.sneaking {
            SNEAK_SPEED_MULTIPLIER
        } else if self.sprinting {
            SPRINT_SPEED_MULTIPLIER
        } else {
            1.0
        };
        BASE_MOVEMENT_SPEED * modifier
    }

    /// `LivingEntity.getFrictionInfluencedSpeed`.
    fn friction_influenced_speed(&self, friction: f64) -> f64 {
        if self.flying {
            if self.sprinting {
                return SPRINT_FLYING_SPEED;
            }
            return FLYING_SPEED;
        }
        if self.on_ground {
            self.movement_speed() * (0.216_000_02 / (friction * friction * friction))
        } else if self.sprinting {
            0.025_999_999
        } else {
            0.02
        }
    }

    /// `LivingEntity.jumpFromGround` with the default jump factor.
    pub fn jump(&mut self) {
        self.jump_with_factor(1.0);
    }

    /// `LivingEntity.jumpFromGround` with the factor of the block under the
    /// player. A sprint jump adds the forward impulse.
    fn jump_with_factor(&mut self, jump_factor: f64) {
        if self.flying {
            return;
        }
        let jump_power = JUMP_IMPULSE * jump_factor;
        if jump_power <= 1.0e-5 {
            return;
        }
        self.velocity_y = self.velocity_y.max(jump_power);
        if self.sprinting {
            let yaw = f64::from(self.yaw).to_radians();
            self.velocity_x -= yaw.sin() * 0.2;
            self.velocity_z += yaw.cos() * 0.2;
        }
    }

    /// `Entity.getBlockJumpFactor`: use the block at the player's feet unless
    /// its factor is the default, in which case the block below the feet wins.
    fn block_jump_factor(&self, world: &mut impl PhysicsWorld) -> f64 {
        let x = self.x.floor() as i32;
        let y = self.y.floor() as i32;
        let z = self.z.floor() as i32;
        let feet = world.jump_factor(x, y, z);
        if feet == 1.0 {
            world.jump_factor(x, (self.y - 0.500_001).floor() as i32, z)
        } else {
            feet
        }
    }

    /// `Entity.getBlockSpeedFactor`: the block at the player's feet wins when
    /// it is not the default, otherwise the block below the feet is used.
    fn block_speed_factor(&self, world: &mut impl PhysicsWorld) -> f64 {
        let x = self.x.floor() as i32;
        let y = self.y.floor() as i32;
        let z = self.z.floor() as i32;
        let feet = world.speed_factor(x, y, z);
        if feet == 1.0 {
            world.speed_factor(x, (self.y - 0.500_001).floor() as i32, z)
        } else {
            feet
        }
    }

    /// `Entity.moveRelative`: rotate the local input by yaw and add it to the
    /// current velocity.
    fn move_relative(&mut self, speed: f64, input: MovementInput) {
        let length_squared = input.forward * input.forward + input.strafe * input.strafe;
        if length_squared < 1.0e-7 {
            return;
        }
        let scale = if length_squared > 1.0 {
            speed / length_squared.sqrt()
        } else {
            speed
        };
        let local_x = input.strafe * scale;
        let local_z = input.forward * scale;
        let yaw = f64::from(self.yaw).to_radians();
        let (sin, cos) = yaw.sin_cos();
        self.velocity_x += local_x * cos - local_z * sin;
        self.velocity_z += local_z * cos + local_x * sin;
    }

    /// One vanilla living-entity travel tick (`LivingEntity.travelInAir`,
    /// plus the flying branch of `Player.travel`).
    pub fn tick(&mut self, world: &mut impl PhysicsWorld, input: MovementInput) -> TickOutcome {
        // `LivingEntity.aiStep` zeros tiny horizontal velocities for players.
        if self.velocity_x * self.velocity_x + self.velocity_z * self.velocity_z < 9.0e-6 {
            self.velocity_x = 0.0;
            self.velocity_z = 0.0;
        }
        if input.jumping && self.on_ground {
            let jump_factor = self.block_jump_factor(world);
            self.jump_with_factor(jump_factor);
        }

        let start = (self.x, self.y, self.z);
        if self.flying {
            let speed = if self.sprinting {
                SPRINT_FLYING_SPEED
            } else {
                FLYING_SPEED
            };
            self.move_relative(speed, input);
            let previous_velocity_y = self.velocity_y;
            let moved = self.move_by(world, self.velocity_x, self.velocity_y, self.velocity_z);
            // `Player.travel` restores 60% of the pre-travel vertical velocity
            // while the ability to fly is active.
            self.velocity_x = moved.dx * AIR_FRICTION;
            self.velocity_y = previous_velocity_y * 0.6;
            self.velocity_z = moved.dz * AIR_FRICTION;
            self.fall_distance = 0.0;
            let outcome = TickOutcome {
                moved: (self.x, self.y, self.z) != start,
                on_ground: self.on_ground,
                landed: false,
                fall_damage: 0.0,
            };
            return outcome;
        }

        let friction = if self.on_ground {
            world.friction(
                self.x.floor() as i32,
                (self.y - 0.500_001).floor() as i32,
                self.z.floor() as i32,
            )
        } else {
            1.0
        };
        let speed = self.friction_influenced_speed(friction);
        self.move_relative(speed, input);
        let moved = self.move_by(world, self.velocity_x, self.velocity_y, self.velocity_z);
        // `Entity.move` scales horizontal delta movement by the destination
        // block's speed factor after the collision move.
        let speed_factor = self.block_speed_factor(world);
        self.velocity_x = moved.dx * friction * AIR_FRICTION * speed_factor;
        self.velocity_y = (moved.dy - GRAVITY) * DRAG;
        self.velocity_z = moved.dz * friction * AIR_FRICTION * speed_factor;

        let mut landed = false;
        let mut fall_damage = 0.0;
        if self.swimming {
            self.fall_distance = 0.0;
        } else {
            if moved.dy < 0.0 {
                self.fall_distance += -moved.dy;
            }
            if moved.on_ground && self.fall_distance > 0.0 {
                fall_damage = fall_damage_for(self.fall_distance);
                self.fall_distance = 0.0;
                landed = true;
            }
        }
        TickOutcome {
            moved: (self.x, self.y, self.z) != start,
            on_ground: self.on_ground,
            landed,
            fall_damage,
        }
    }

    /// Reconcile one client movement packet with the simulation.
    ///
    /// Small moves are replayed through the axis-separated collider and are
    /// accepted exactly when the client's position is reproducible within
    /// [`POSITION_TOLERANCE`]. Larger moves are client-side lag bursts; their
    /// destination is validated instead of replaying every intermediate tick,
    /// so a 8.5-block test/probe move over natural terrain does not rubber-band.
    pub fn reconcile(
        &mut self,
        world: &mut impl PhysicsWorld,
        client: &ClientMove,
    ) -> Reconciliation {
        self.reconcile_with_allowance(world, client, MAX_MOVEMENT_DISTANCE_SQUARED)
    }

    /// [`Self::reconcile`] with an explicit movement budget.
    ///
    /// The normal budget is 10 blocks per packet. After a long server stall
    /// (for example the initial chunk batch finishing) the client can legitimately
    /// be hundreds of blocks away; callers scale this budget by the elapsed
    /// time so the server can resync instead of entering a correction loop.
    pub fn reconcile_with_allowance(
        &mut self,
        world: &mut impl PhysicsWorld,
        client: &ClientMove,
        max_distance_squared: f64,
    ) -> Reconciliation {
        if let Some(yaw) = client.yaw {
            self.yaw = yaw;
        }
        if let Some(pitch) = client.pitch {
            self.pitch = pitch;
        }

        let dx = client.x - self.reported_x;
        let dy = client.y - self.reported_y;
        let dz = client.z - self.reported_z;
        let distance_squared = dx * dx + dy * dy + dz * dz;
        if !distance_squared.is_finite() || distance_squared > max_distance_squared {
            self.reported_x = self.x;
            self.reported_y = self.y;
            self.reported_z = self.z;
            return Reconciliation {
                moved: false,
                on_ground: self.on_ground,
                landed: false,
                fall_damage: 0.0,
                correction: Some(MovementCorrection::TooFar { distance_squared }),
            };
        }

        let start = (self.x, self.y, self.z);
        let horizontal = (dx * dx + dz * dz).sqrt();
        let mut correction = None;
        let mut fall_damage = 0.0;
        let moved;
        let landed;

        if horizontal <= MAX_SIMULATED_HORIZONTAL_MOVE {
            let movement = self.move_by(world, dx, dy, dz);
            moved = (self.x, self.y, self.z) != start;
            if !self.flying && !self.swimming && movement.dy < 0.0 {
                self.fall_distance += -movement.dy;
            }
            let grounded = movement.on_ground || (client.on_ground && self.is_supported(world));
            landed = grounded;
            if landed && self.fall_distance > 0.0 {
                fall_damage = fall_damage_for(self.fall_distance);
                self.fall_distance = 0.0;
            }
            self.on_ground = grounded || client.on_ground;
            self.velocity_x = movement.dx;
            self.velocity_y = movement.dy;
            self.velocity_z = movement.dz;

            let error_x = client.x - self.x;
            let error_y = client.y - self.y;
            let error_z = client.z - self.z;
            if error_x * error_x + error_y * error_y + error_z * error_z
                > POSITION_TOLERANCE * POSITION_TOLERANCE
            {
                correction = Some(MovementCorrection::Blocked);
                self.reported_x = self.x;
                self.reported_y = self.y;
                self.reported_z = self.z;
            } else {
                // Trust the client's exact values when the simulation agrees;
                // this avoids tiny floating-point drift accumulating.
                self.x = client.x;
                self.y = client.y;
                self.z = client.z;
                self.on_ground = grounded || client.on_ground;
                self.reported_x = client.x;
                self.reported_y = client.y;
                self.reported_z = client.z;
            }
        } else {
            // A multi-tick burst: validate the destination.
            let previous = (self.x, self.y, self.z);
            self.x = client.x;
            self.y = client.y;
            self.z = client.z;
            if !self.is_free(world) {
                match self.first_free_y(world, client.y, client.x, client.z) {
                    Some(y) => {
                        self.y = y;
                        correction = Some(MovementCorrection::Adjusted);
                    }
                    None => {
                        self.x = previous.0;
                        self.y = previous.1;
                        self.z = previous.2;
                        self.reported_x = previous.0;
                        self.reported_y = previous.1;
                        self.reported_z = previous.2;
                        return Reconciliation {
                            moved: false,
                            on_ground: self.on_ground,
                            landed: false,
                            fall_damage: 0.0,
                            correction: Some(MovementCorrection::Blocked),
                        };
                    }
                }
            }
            moved = (self.x, self.y, self.z) != start;
            if !self.flying && !self.swimming && dy < 0.0 {
                self.fall_distance += -dy;
            }
            landed = client.on_ground && self.is_supported(world);
            if landed && self.fall_distance > 0.0 {
                fall_damage = fall_damage_for(self.fall_distance);
                self.fall_distance = 0.0;
            }
            self.on_ground = landed || client.on_ground;
            self.velocity_x = dx;
            self.velocity_y = dy;
            self.velocity_z = dz;
            if correction.is_some() {
                self.reported_x = self.x;
                self.reported_y = self.y;
                self.reported_z = self.z;
            } else {
                self.reported_x = client.x;
                self.reported_y = client.y;
                self.reported_z = client.z;
            }
        }

        Reconciliation {
            moved,
            on_ground: self.on_ground,
            landed,
            fall_damage,
            correction,
        }
    }

    fn is_free(&mut self, world: &mut impl PhysicsWorld) -> bool {
        let aabb = self.aabb();
        collision_boxes(world, aabb).is_empty()
    }

    fn first_free_y(
        &mut self,
        world: &mut impl PhysicsWorld,
        start_y: f64,
        x: f64,
        z: f64,
    ) -> Option<f64> {
        let steps = (MAX_SETTLE_LIFT / SETTLE_STEP).ceil() as usize;
        for step in 0..=steps {
            let y = start_y + step as f64 * SETTLE_STEP;
            let aabb = Aabb::player(x, y, z, self.width(), self.height());
            if collision_boxes(world, aabb).is_empty() {
                return Some(y);
            }
        }
        None
    }

    fn is_supported(&mut self, world: &mut impl PhysicsWorld) -> bool {
        let aabb = self.aabb();
        let probe = Aabb::new(
            aabb.min_x + EPSILON,
            aabb.min_y - 1.0e-3,
            aabb.min_z + EPSILON,
            aabb.max_x - EPSILON,
            aabb.min_y - EPSILON,
            aabb.max_z - EPSILON,
        );
        collision_boxes(world, probe).into_iter().any(|block| {
            let x = block.min_x.floor() as i32;
            let y = block.min_y.floor() as i32;
            let z = block.min_z.floor() as i32;
            world.blocks_motion(x, y, z) && world.is_solid(x, y, z)
        })
    }

    /// Move by `(dx, dy, dz)` with vanilla's Y-first, largest-horizontal-first
    /// axis order and step-up handling.
    fn move_by(&mut self, world: &mut impl PhysicsWorld, dx: f64, dy: f64, dz: f64) -> Moved {
        let start_box = self.aabb();
        let y_allowed = collide_axis(world, start_box, dy, Axis::Y);
        let after_y = start_box.offset(0.0, y_allowed, 0.0);
        let previously_on_ground = self.on_ground;
        let mut on_ground = if dy < 0.0 {
            y_allowed != dy
        } else if dy > 0.0 {
            false
        } else {
            previously_on_ground
        };

        let (x_allowed, z_allowed) = collide_horizontal(world, after_y, dx, dz);
        let mut applied_x = x_allowed;
        let mut applied_y = y_allowed;
        let mut applied_z = z_allowed;

        let vertical_collision = dy < 0.0 && y_allowed != dy;
        if (on_ground || vertical_collision) && (x_allowed != dx || z_allowed != dz) {
            let original_horizontal_squared = x_allowed * x_allowed + z_allowed * z_allowed;
            if let Some((step_x, step_y, step_z)) = step_up(
                world,
                after_y,
                dx,
                dz,
                y_allowed,
                original_horizontal_squared,
            ) {
                applied_x = step_x;
                applied_y = y_allowed + step_y;
                applied_z = step_z;
                on_ground = true;
            }
        }

        self.x += applied_x;
        self.y += applied_y;
        self.z += applied_z;
        if applied_x != dx {
            self.velocity_x = 0.0;
        }
        if applied_y != dy {
            self.velocity_y = 0.0;
        }
        if applied_z != dz {
            self.velocity_z = 0.0;
        }
        self.on_ground = on_ground;
        Moved {
            dx: applied_x,
            dy: applied_y,
            dz: applied_z,
            on_ground,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Moved {
    dx: f64,
    dy: f64,
    dz: f64,
    on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

/// Vanilla fall damage: `floor(fallDistance + 1e-6 - safeFallDistance)`.
pub fn fall_damage_for(fall_distance: f64) -> f32 {
    if !fall_distance.is_finite() || fall_distance <= SAFE_FALL_DISTANCE {
        return 0.0;
    }
    ((fall_distance + 1.0e-6 - SAFE_FALL_DISTANCE).floor() as f32).max(0.0)
}

fn collision_boxes(world: &mut impl PhysicsWorld, area: Aabb) -> Vec<Aabb> {
    // Vanilla `BlockCollisions` widens the block cursor by one block on every
    // side so shapes that extend past their own block (fences 1.5 high, walls
    // 1.5 high for collision) are still considered.
    let x0 = (area.min_x - EPSILON).floor() as i32 - 1;
    let x1 = (area.max_x + EPSILON).floor() as i32 + 1;
    let y0 = (area.min_y - EPSILON).floor() as i32 - 1;
    let y1 = (area.max_y + EPSILON).floor() as i32 + 1;
    let z0 = (area.min_z - EPSILON).floor() as i32 - 1;
    let z1 = (area.max_z + EPSILON).floor() as i32 + 1;
    let mut boxes = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                for collision in world.collision_boxes(x, y, z) {
                    if collision.intersects(&area) {
                        boxes.push(collision);
                    }
                }
            }
        }
    }
    boxes
}

fn collide_axis(world: &mut impl PhysicsWorld, area: Aabb, amount: f64, axis: Axis) -> f64 {
    if amount.abs() < EPSILON {
        return 0.0;
    }
    let sweep = match axis {
        Axis::X => area.expand_towards(amount, 0.0, 0.0),
        Axis::Y => area.expand_towards(0.0, amount, 0.0),
        Axis::Z => area.expand_towards(0.0, 0.0, amount),
    };
    let mut allowed = amount;
    for block in collision_boxes(world, sweep) {
        match axis {
            Axis::X => {
                if amount > 0.0
                    && area.max_x <= block.min_x + EPSILON
                    && area.max_x + amount > block.min_x
                {
                    allowed = allowed.min(block.min_x - area.max_x);
                } else if amount < 0.0
                    && area.min_x >= block.max_x - EPSILON
                    && area.min_x + amount < block.max_x
                {
                    allowed = allowed.max(block.max_x - area.min_x);
                }
            }
            Axis::Y => {
                if amount > 0.0
                    && area.max_y <= block.min_y + EPSILON
                    && area.max_y + amount > block.min_y
                {
                    allowed = allowed.min(block.min_y - area.max_y);
                } else if amount < 0.0
                    && area.min_y >= block.max_y - EPSILON
                    && area.min_y + amount < block.max_y
                {
                    allowed = allowed.max(block.max_y - area.min_y);
                }
            }
            Axis::Z => {
                if amount > 0.0
                    && area.max_z <= block.min_z + EPSILON
                    && area.max_z + amount > block.min_z
                {
                    allowed = allowed.min(block.min_z - area.max_z);
                } else if amount < 0.0
                    && area.min_z >= block.max_z - EPSILON
                    && area.min_z + amount < block.max_z
                {
                    allowed = allowed.max(block.max_z - area.min_z);
                }
            }
        }
    }
    allowed
}

fn collide_horizontal(world: &mut impl PhysicsWorld, area: Aabb, dx: f64, dz: f64) -> (f64, f64) {
    if dx.abs() < dz.abs() {
        let z = collide_axis(world, area, dz, Axis::Z);
        let after_z = area.offset(0.0, 0.0, z);
        let x = collide_axis(world, after_z, dx, Axis::X);
        (x, z)
    } else {
        let x = collide_axis(world, area, dx, Axis::X);
        let after_x = area.offset(x, 0.0, 0.0);
        let z = collide_axis(world, after_x, dz, Axis::Z);
        (x, z)
    }
}

/// Vanilla `Entity.collide` step retrieval: try collision heights up to the
/// step height and keep the first one that moves horizontally further.
fn step_up(
    world: &mut impl PhysicsWorld,
    area_after_y: Aabb,
    dx: f64,
    dz: f64,
    y_allowed: f64,
    original_horizontal_squared: f64,
) -> Option<(f64, f64, f64)> {
    let retrieval = area_after_y
        .expand_towards(dx, STEP_HEIGHT, dz)
        .expand_towards(0.0, -1.0e-5, 0.0);
    let mut heights = Vec::new();
    for block in collision_boxes(world, retrieval) {
        for y in [
            block.min_y - area_after_y.min_y,
            block.max_y - area_after_y.min_y,
        ] {
            if y > EPSILON && y <= STEP_HEIGHT + EPSILON && (y - y_allowed).abs() > EPSILON {
                heights.push(y);
            }
        }
    }
    heights.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    heights.dedup_by(|left, right| (*left - *right).abs() <= EPSILON);

    for step in heights {
        let raised = area_after_y.offset(0.0, step, 0.0);
        let (x, z) = collide_horizontal(world, raised, dx, dz);
        if x * x + z * z > original_horizontal_squared + 1.0e-12 {
            return Some((x, step, z));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_uses_safe_fall_distance() {
        assert_eq!(fall_damage_for(0.0), 0.0);
        assert_eq!(fall_damage_for(3.0), 0.0);
        assert_eq!(fall_damage_for(3.9), 0.0);
        assert_eq!(fall_damage_for(4.0), 1.0);
        assert_eq!(fall_damage_for(10.0), 7.0);
    }

    #[test]
    fn terminal_velocity_is_a_fixed_point() {
        assert!(((TERMINAL_VELOCITY - GRAVITY) * DRAG - TERMINAL_VELOCITY).abs() < 1.0e-12);
    }

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: vec![],
        }
    }

    #[test]
    fn block_friction_materials_match_vanilla() {
        assert_eq!(
            crate::block_behaviour::friction(&state("minecraft:stone")),
            0.6
        );
        assert_eq!(
            crate::block_behaviour::friction(&state("minecraft:ice")),
            0.98
        );
        assert_eq!(
            crate::block_behaviour::friction(&state("minecraft:blue_ice")),
            0.989
        );
        assert_eq!(
            crate::block_behaviour::friction(&state("minecraft:slime_block")),
            0.8
        );
    }

    #[test]
    fn collision_metadata_covers_plants_fluids_and_solid_blocks() {
        for name in [
            "minecraft:short_grass",
            "minecraft:poppy",
            "minecraft:sunflower",
            "minecraft:oak_sapling",
            "minecraft:wheat",
            "minecraft:torch",
            "minecraft:oak_sign",
            "minecraft:water",
        ] {
            assert!(
                !crate::block_behaviour::has_collision(&state(name)),
                "{name} should be passable"
            );
        }
        for name in [
            "minecraft:stone",
            "minecraft:grass_block",
            "minecraft:oak_leaves",
            "minecraft:cherry_leaves",
            "minecraft:flowering_azalea_leaves",
            "minecraft:azalea",
            "minecraft:flowering_azalea",
            "minecraft:snow",
        ] {
            assert!(
                crate::block_behaviour::has_collision(&state(name)),
                "{name} must collide"
            );
        }
        assert!(!crate::block_behaviour::is_solid(&state(
            "minecraft:short_grass"
        )));
        assert!(crate::block_behaviour::is_solid(&state("minecraft:stone")));
        assert!(!crate::block_behaviour::blocks_motion(&state(
            "minecraft:water"
        )));
        assert!(crate::block_behaviour::blocks_motion(&state(
            "minecraft:ice"
        )));
    }

    #[test]
    fn speed_factor_scales_the_horizontal_velocity() {
        let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
        physics.on_ground = true;
        physics.yaw = -90.0;
        let mut world = FlatPhysicsWorld::new(0);
        world.speed_factor = 0.4;

        physics.tick(&mut world, MovementInput::walking_forward());
        let expected = 0.1 * 0.6 * AIR_FRICTION * 0.4;
        assert!(
            (physics.velocity_x - expected).abs() < 1.0e-7,
            "velocity {} expected {expected}",
            physics.velocity_x
        );
    }

    #[test]
    fn jump_factor_scales_the_jump_impulse() {
        let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
        physics.on_ground = true;
        let mut world = FlatPhysicsWorld::new(0);
        world.jump_factor = 0.5;

        physics.tick(
            &mut world,
            MovementInput {
                jumping: true,
                ..MovementInput::default()
            },
        );
        assert!(
            (physics.y - JUMP_IMPULSE * 0.5).abs() < 1.0e-12,
            "jump height {}",
            physics.y
        );
    }
}
