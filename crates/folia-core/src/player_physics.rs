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
//! boxes from the generated [`crate::block_shapes`] table. Ladder/vine/
//! scaffolding climbing, cobweb slowdown, and slime bounce follow
//! `LivingEntity.handleOnClimbable`, `WebBlock.entityInside`, and
//! `SlimeBlock.fallOn`/`bounceUp`; climbable and fall-damage-resetting blocks
//! come from the shipped block tags. Water/lava travel, downhill flow,
//! bubble-column push, and powder-snow sinking/freezing use the shipped fluid
//! states, behaviour metadata, and [`crate::fluid`] velocity helpers.
//!
//! The fluid, elytra, and riptide formulas are Rust-only goldens: no 1.21.11
//! Java movement oracle or decompiled source tree is available on this host, so
//! `FlowingFluid.getFlow`, the buoyancy constants, the glide constants, and the
//! riptide speed are pinned by Rust tests rather than a differential fixture
//! (see `DECISIONS.md`). Depth strider, slow falling, levitation, and elytra
//! modify movement through inert [`PlayerPhysics`] modifier fields; riptide is
//! applied as a one-shot impulse by the item-use path. Firework boost, elytra
//! durability wear, wall-impact damage, and live freeze-damage health wiring
//! are still intentionally unsupported.

use crate::block_behaviour::MovementTraits;
use crate::chunk::{BlockState, Chunk};
use crate::fluid::{self, BubbleColumnPlacement, FluidKind, FluidState};
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
/// `LivingEntity.handleRelativeFrictionAndCalculateMovement` climb impulse
/// when a climber is pressing into a wall or holding jump.
pub const CLIMB_SPEED: f64 = 0.2;
/// `LivingEntity.handleOnClimbable` clamps horizontal speed and slide-down
/// speed to `0.15` blocks per tick.
pub const CLIMB_MOVE_LIMIT: f64 = 0.15;
/// `WebBlock.entityInside` motion multiplier `(0.25, 0.05, 0.25)`.
pub const COBWEB_MOVE_MULTIPLIER: (f64, f64, f64) = (0.25, 0.05, 0.25);
/// `SlimeBlock.stepOn` multiplies horizontal motion by `0.4 + |dy| * 0.2`
/// for a player who is not stepping carefully.
pub const SLIME_STEP_BASE: f64 = 0.4;
pub const SLIME_STEP_FACTOR: f64 = 0.2;
/// `LivingEntity.travel` scales delta movement by `0.8` in water.
pub const WATER_DRAG: f64 = 0.8;
/// `LivingEntity.travel` scales delta movement by `0.5` in lava.
pub const LAVA_DRAG: f64 = 0.5;
/// Buoyancy: gravity is halved while submerged.
pub const WATER_GRAVITY_FACTOR: f64 = 0.5;
pub const LAVA_GRAVITY_FACTOR: f64 = 0.5;
/// `LivingEntity.travel` uses a `0.02` input speed in lava instead of
/// `Player.getSpeed()`, which is why lava feels much heavier than water.
pub const LAVA_MOVE_SPEED: f64 = 0.02;
/// Per-tick acceleration applied per unit of [`crate::fluid::flow_vector`] in
/// water (vanilla `Entity.getFluidPushFactor` is small, so flow is gradual).
pub const WATER_FLOW_ACCELERATION: f64 = 0.01;
pub const LAVA_FLOW_ACCELERATION: f64 = 0.005;
/// `PowderSnowBlock.entityInside` multiplies movement by `(0.9, 0.5, 0.9)`,
/// which makes an entity sink slowly and struggle to move sideways.
pub const POWDER_SNOW_MOVE_MULTIPLIER: (f64, f64, f64) = (0.9, 0.5, 0.9);
/// Frozen ticks added on every tick inside powder snow.
pub const POWDER_SNOW_FREEZE_PER_TICK: u16 = 7;
/// Vanilla `LivingEntity.MIN_FREEZE_TICKS`: freezing damage starts above this.
pub const MIN_FREEZE_TICKS: u16 = 140;
/// Vanilla `LivingEntity.MAX_FREEZE_TICKS` (`MIN_FREEZE_TICKS + 20`).
pub const MAX_FREEZE_TICKS: u16 = 160;
/// Vanilla `LivingEntity.getTicksRequiredToFreeze` default interval.
pub const FREEZE_DAMAGE_INTERVAL: u64 = 40;
pub const FREEZE_DAMAGE: f32 = 1.0;
/// Depth strider level 0 keeps the vanilla water speed unchanged; each level
/// adds two thirds of that base speed (levels 1..=3 multiply it by 5/3, 7/3,
/// and 3). No Java movement oracle exists on this host, so this is a Rust-only
/// golden following the 2/3-per-level spec; see `DECISIONS.md`.
pub const DEPTH_STRIDER_STEP: f64 = 2.0 / 3.0;
pub const MAX_DEPTH_STRIDER_LEVEL: i32 = 3;
/// Slow falling replaces the normal air gravity with a quarter of it and caps
/// the descent velocity (`SlowFallingMobEffect` prevents fall damage).
pub const SLOW_FALL_GRAVITY_FACTOR: f64 = 0.25;
pub const SLOW_FALL_TERMINAL_VELOCITY: f64 = -0.1;
/// Levitation approaches `LEVITATION_ACCELERATION * level` upward while active
/// (`LivingEntity` levitation branch).
pub const LEVITATION_ACCELERATION: f64 = 0.05;
pub const LEVITATION_APPROACH: f64 = 0.2;
/// `LivingEntity.updateFallFlyingMovement`: the gravity term is scaled by
/// `cos(xRot)^2 * 0.75`, so a level glide sheds a quarter of normal gravity.
pub const GLIDE_GRAVITY_FACTOR: f64 = 0.75;
/// Descending glides transfer ten percent of their vertical speed into the
/// look direction and back (`v.y * -0.1 * cos(xRot)^2`).
pub const GLIDE_DIVE_TRANSFER: f64 = 0.1;
/// Looking up transfers `horizontalSpeed * -sin(xRot) * 0.04` away from the
/// look direction, three and a fifth of it into upward velocity.
pub const GLIDE_LIFT_TRANSFER: f64 = 0.04;
pub const GLIDE_LIFT_MULTIPLIER: f64 = 3.2;
/// Per-tick alignment of horizontal velocity with the look direction.
pub const GLIDE_ALIGNMENT: f64 = 0.1;
/// `Entity.move` drag after `updateFallFlyingMovement`.
pub const GLIDE_HORIZONTAL_DRAG: f64 = 0.99;
pub const GLIDE_VERTICAL_DRAG: f64 = 0.98;
/// `LivingEntity.checkSlowFallDistance`: a descent slower than 0.5 blocks per
/// tick clears accumulated fall distance.
pub const GLIDE_SLOW_FALL_THRESHOLD: f64 = -0.5;
/// `TridentItem.releaseUsing` riptide launch: `3.0F * ((1 + level) / 4)`.
/// Levels 1..=3 give `1.5`, `2.25`, and `3.0`. This float constant is a
/// Rust-only golden (no local 1.21.11 Java source, see `DECISIONS.md`).
pub const RIPTIDE_BASE_SPEED: f64 = 3.0;
pub const MAX_RIPTIDE_LEVEL: i32 = 3;

/// Launch speed of a riptide enchantment level. Level `0` (or negative) is
/// inert, matching an unenchanted trident.
pub fn riptide_speed(level: i32) -> f64 {
    if level <= 0 {
        return 0.0;
    }
    RIPTIDE_BASE_SPEED * (f64::from(level.clamp(1, MAX_RIPTIDE_LEVEL)) + 1.0) / 4.0
}

/// The velocity Java applies on a wet riptide use: the look vector is
/// normalised by its length (vanilla's `f5`) and scaled by [`riptide_speed`].
/// Returns `None` for an inert level or a degenerate direction, so callers can
/// fall through to the normal trident throw.
pub fn riptide_launch_velocity(level: i32, dir: [f64; 3]) -> Option<[f64; 3]> {
    let speed = riptide_speed(level);
    if speed == 0.0 {
        return None;
    }
    let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if !length.is_finite() || length <= EPSILON {
        return None;
    }
    Some([
        dir[0] / length * speed,
        dir[1] / length * speed,
        dir[2] / length * speed,
    ])
}

/// Horizontal submerged-water speed multiplier for a depth strider level.
/// Levels outside `0..=3` are clamped, so missing or invalid enchantment data
/// stays inert.
pub fn depth_strider_speed_multiplier(level: i32) -> f64 {
    1.0 + f64::from(level.clamp(0, MAX_DEPTH_STRIDER_LEVEL)) * DEPTH_STRIDER_STEP
}

/// `LivingEntity.updateFallFlyingMovement`, ported to Rust scalars.
///
/// `yaw`/`pitch` are Minecraft degrees: positive pitch looks down, matching
/// `calculateViewVector(xRot, yRot)`. The look vector's horizontal length is
/// `cos(pitch)` because the client clamps pitch to `[-90, 90]`.
pub fn glide_velocity(
    velocity_x: f64,
    velocity_y: f64,
    velocity_z: f64,
    yaw: f32,
    pitch: f32,
    gravity: f64,
) -> (f64, f64, f64) {
    let horizontal = (velocity_x * velocity_x + velocity_z * velocity_z).sqrt();
    let yaw_rad = f64::from(yaw).to_radians();
    let pitch_rad = f64::from(pitch).to_radians();
    let (sin_yaw, cos_yaw) = yaw_rad.sin_cos();
    let cos_pitch = pitch_rad.cos();
    let sin_pitch = pitch_rad.sin();
    let look_x = -sin_yaw * cos_pitch;
    let look_z = cos_yaw * cos_pitch;
    let cos_squared = cos_pitch * cos_pitch;

    let mut x = velocity_x;
    let mut y = velocity_y;
    let mut z = velocity_z;
    y += gravity * (-1.0 + cos_squared * GLIDE_GRAVITY_FACTOR);
    if y < 0.0 && cos_pitch > 0.0 {
        let transfer = y * -GLIDE_DIVE_TRANSFER * cos_squared;
        x += look_x * transfer / cos_pitch;
        y += transfer;
        z += look_z * transfer / cos_pitch;
    }
    if pitch_rad < 0.0 && cos_pitch > 0.0 {
        let lift = horizontal * -sin_pitch * GLIDE_LIFT_TRANSFER;
        x -= look_x * lift / cos_pitch;
        y += lift * GLIDE_LIFT_MULTIPLIER;
        z -= look_z * lift / cos_pitch;
    }
    if cos_pitch > 0.0 {
        x += (look_x / cos_pitch * horizontal - x) * GLIDE_ALIGNMENT;
        z += (look_z / cos_pitch * horizontal - z) * GLIDE_ALIGNMENT;
    }
    (
        x * GLIDE_HORIZONTAL_DRAG,
        y * GLIDE_VERTICAL_DRAG,
        z * GLIDE_HORIZONTAL_DRAG,
    )
}

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
    /// Special movement properties of the block at this position. Defaulting
    /// to none keeps simple and flat test worlds on generic air/ground physics.
    fn movement_traits(&mut self, _x: i32, _y: i32, _z: i32) -> MovementTraits {
        MovementTraits::NONE
    }
    /// `LivingEntity.onClimbable` for a player: a climbable block at the feet
    /// (open trapdoors above ladders are not modeled).
    fn is_climbable(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).climbable
    }
    /// Scaffolding is climbable but ignores the sneaking slide-down suppression.
    fn is_scaffolding(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).scaffolding
    }
    fn is_cobweb(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).cobweb
    }
    fn is_slime(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).slime
    }
    fn resets_fall_damage(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).resets_fall_damage
    }
    /// Powder snow at this position. Vanilla has no tag isolating it, so the
    /// shipped `movement_traits` resolves it by registry name.
    fn is_powder_snow(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.movement_traits(x, y, z).powder_snow
    }
    /// The fluid state at this position from the shipped block-behaviour
    /// metadata. Defaults to none so flat/grid fixture worlds keep air physics.
    fn fluid_state(&mut self, _x: i32, _y: i32, _z: i32) -> Option<FluidState> {
        None
    }
    /// Downhill flow vector of the fluid at this position. The default samples
    /// the four horizontal neighbour fluid states and applies
    /// [`crate::fluid::flow_vector`].
    fn fluid_flow(&mut self, x: i32, y: i32, z: i32) -> (f64, f64, f64) {
        let Some(here) = self.fluid_state(x, y, z) else {
            return (0.0, 0.0, 0.0);
        };
        let neighbors = [
            self.fluid_state(x, y, z - 1),
            self.fluid_state(x + 1, y, z),
            self.fluid_state(x, y, z + 1),
            self.fluid_state(x - 1, y, z),
        ];
        let (flow_x, flow_y, flow_z) = fluid::flow_vector(here, neighbors);
        (f64::from(flow_x), f64::from(flow_y), f64::from(flow_z))
    }
    /// Bubble-column direction at this position: `Some(true)` is magma (pull
    /// down), `Some(false)` is soul sand (push up). Uses the shipped
    /// `drag` block-state property; defaults to none for fixture worlds.
    fn bubble_column_drag(&mut self, _x: i32, _y: i32, _z: i32) -> Option<bool> {
        None
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

    fn movement_traits(&mut self, x: i32, y: i32, z: i32) -> MovementTraits {
        self.block_state(x, y, z).map_or(
            MovementTraits::NONE,
            crate::block_behaviour::movement_traits,
        )
    }

    fn fluid_state(&mut self, x: i32, y: i32, z: i32) -> Option<FluidState> {
        self.block_state(x, y, z).and_then(fluid::fluid_state)
    }

    fn bubble_column_drag(&mut self, x: i32, y: i32, z: i32) -> Option<bool> {
        self.block_state(x, y, z)
            .and_then(fluid::bubble_column_drag)
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
    /// Optional per-tick pitch override for elytra steering. `None` uses the
    /// pitch already synced onto [`PlayerPhysics`] by movement packets, so
    /// every existing caller stays inert without setting this field.
    pub pitch: Option<f32>,
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
    /// Damage from sustained powder-snow exposure. The live session does not
    /// yet consume this value (see `DECISIONS.md`); the simulation reports it
    /// so a future tick integration can apply it without recomputing.
    pub freeze_damage: f32,
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
    /// One-shot motion multiplier set when the player is inside a cobweb
    /// (`Entity.stuckSpeedMultiplier`) and consumed by the next move.
    pub cobweb_stuck: bool,
    /// `LivingEntity.ticksFrozen`, advanced by powder-snow exposure.
    pub ticks_frozen: u16,
    /// Depth strider level from the boots slot; `0` keeps water movement inert.
    pub depth_strider_level: i32,
    /// Whether slow falling is active; suppresses gravity and fall damage.
    pub slow_falling: bool,
    /// Levitation level (`amplifier + 1`); `0` disables the upward push.
    pub levitation_level: i32,
    /// Whether a glider is equipped in the chest slot. Derived from equipment
    /// by [`crate::effects::apply_movement_modifiers`]; `false` is inert.
    pub elytra: bool,
    /// Whether the player is actively gliding (vanilla shared flag 7). Only
    /// set by [`Self::try_start_gliding`] while [`Self::elytra`] is equipped.
    pub fall_flying: bool,
    /// Previous tick's jump input, used for the vanilla rising-edge launch.
    previous_jumping: bool,
    /// Monotonic simulation tick count used for the freeze-damage interval.
    pub tick_count: u64,
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
            cobweb_stuck: false,
            ticks_frozen: 0,
            depth_strider_level: 0,
            slow_falling: false,
            levitation_level: 0,
            elytra: false,
            fall_flying: false,
            previous_jumping: false,
            tick_count: 0,
            reported_x: x,
            reported_y: y,
            reported_z: z,
        }
    }

    pub fn width(&self) -> f64 {
        WIDTH
    }

    pub fn height(&self) -> f64 {
        // `Pose.FALL_FLYING` uses the same 0.6-high dimensions as `Pose.SWIMMING`.
        if self.swimming || self.fall_flying {
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

    /// Resynchronise the movement-baseline position without moving the player.
    /// Used while a teleport is awaiting acknowledgement so the next accepted
    /// packet is measured from the server's position, not the stale client
    /// position reported before the teleport was applied.
    pub fn sync_reported_position(&mut self) {
        self.reported_x = self.x;
        self.reported_y = self.y;
        self.reported_z = self.z;
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
        self.cobweb_stuck = false;
        self.ticks_frozen = 0;
        self.on_ground = false;
        self.fall_flying = false;
        self.previous_jumping = false;
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

    /// Apply the one-shot `TridentItem.releaseUsing` riptide impulse along a
    /// look vector. Java also clears sneaking (`removeSneaking`) and the fall
    /// distance on this path; the client velocity sync is the caller's motion
    /// packet, and the riptide sound/particles are not modelled (see
    /// `DECISIONS.md`). Returns `false` for an inert level or bad direction,
    /// leaving physics untouched.
    pub fn launch_riptide(&mut self, level: i32, dir: [f64; 3]) -> bool {
        let Some(velocity) = riptide_launch_velocity(level, dir) else {
            return false;
        };
        self.velocity_x = velocity[0];
        self.velocity_y = velocity[1];
        self.velocity_z = velocity[2];
        self.sneaking = false;
        self.fall_distance = 0.0;
        true
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

    /// The block column at the player's feet (`Entity.blockPosition`).
    fn feet(&self) -> (i32, i32, i32) {
        (
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }

    /// Whether the player is on a climbable block. Flying players never climb
    /// (`Player.onClimbable`).
    fn on_climbable(&mut self, world: &mut impl PhysicsWorld) -> bool {
        if self.flying {
            return false;
        }
        let (x, y, z) = self.feet();
        world.is_climbable(x, y, z)
    }

    /// Whether the block directly under the player's feet is slime.
    fn standing_on_slime(&mut self, world: &mut impl PhysicsWorld) -> bool {
        let (x, _, z) = self.feet();
        world.is_slime(x, (self.y - 1.0e-3).floor() as i32, z)
    }

    /// Fall-damage exemptions vanilla applies when landing in a climbable
    /// block or on a slime block.
    fn fall_damage_suppressed(&mut self, world: &mut impl PhysicsWorld) -> bool {
        self.on_climbable(world) || (!self.sneaking && self.standing_on_slime(world))
    }

    /// Fluid at the player's block position (`Entity.isInWater`/`isInLava`).
    fn fluid_at_feet(&self, world: &mut impl PhysicsWorld) -> Option<FluidState> {
        let (x, y, z) = self.feet();
        world.fluid_state(x, y, z)
    }

    /// Powder snow at the player's block position.
    fn powder_snow_at(&self, world: &mut impl PhysicsWorld) -> bool {
        let (x, y, z) = self.feet();
        world.is_powder_snow(x, y, z)
    }

    /// Bubble column affecting the player: inside the feet block gets the
    /// weaker velocity, the block below gets the surface launch.
    fn bubble_column_at(
        &self,
        world: &mut impl PhysicsWorld,
    ) -> Option<(bool, BubbleColumnPlacement)> {
        let (x, y, z) = self.feet();
        if let Some(drag_down) = world.bubble_column_drag(x, y, z) {
            return Some((drag_down, BubbleColumnPlacement::Inside));
        }
        world
            .bubble_column_drag(x, y - 1, z)
            .map(|drag_down| (drag_down, BubbleColumnPlacement::AboveSurface))
    }

    /// Advance `LivingEntity.ticksFrozen` and return freeze damage for this
    /// tick. Powder snow adds seven ticks each tick; outside it thaws one tick
    /// at a time. Damage starts above `MIN_FREEZE_TICKS` every 40 ticks.
    fn tick_frozen_state(&mut self, world: &mut impl PhysicsWorld) -> f32 {
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.powder_snow_at(world) {
            self.ticks_frozen =
                (self.ticks_frozen + POWDER_SNOW_FREEZE_PER_TICK).min(MAX_FREEZE_TICKS);
        } else if self.ticks_frozen > 0 {
            self.ticks_frozen -= 1;
        }
        if self.ticks_frozen > MIN_FREEZE_TICKS
            && self.tick_count.is_multiple_of(FREEZE_DAMAGE_INTERVAL)
        {
            FREEZE_DAMAGE
        } else {
            0.0
        }
    }

    /// One `LivingEntity.travel` tick while submerged in water or lava.
    ///
    /// Input speed, buoyancy, drag, and flow acceleration come from the fluid
    /// family; fall distance is cleared every tick, as vanilla does while
    /// submerged. Bubble columns override gravity with the exact
    /// [`fluid::bubble_column_velocity`] caps.
    fn tick_in_fluid(
        &mut self,
        world: &mut impl PhysicsWorld,
        state: FluidState,
        input: MovementInput,
        start: (f64, f64, f64),
        freeze_damage: f32,
    ) -> TickOutcome {
        let kind = state.kind;
        let speed = match kind {
            // Depth strider only speeds up submerged water travel; lava and
            // every other movement branch stay unchanged.
            FluidKind::Water => {
                self.movement_speed() * depth_strider_speed_multiplier(self.depth_strider_level)
            }
            FluidKind::Lava => LAVA_MOVE_SPEED,
        };
        self.move_relative(speed, input);

        let (x, y, z) = self.feet();
        let (flow_x, flow_y, flow_z) = world.fluid_flow(x, y, z);
        let acceleration = match kind {
            FluidKind::Water => WATER_FLOW_ACCELERATION,
            FluidKind::Lava => LAVA_FLOW_ACCELERATION,
        };
        self.velocity_x += flow_x * acceleration;
        self.velocity_z += flow_z * acceleration;
        if flow_y < 0.0 {
            self.velocity_y += flow_y * acceleration;
        }

        let gravity = match kind {
            FluidKind::Water => GRAVITY * WATER_GRAVITY_FACTOR,
            FluidKind::Lava => GRAVITY * LAVA_GRAVITY_FACTOR,
        };
        let bubble = if kind == FluidKind::Water {
            self.bubble_column_at(world)
        } else {
            None
        };
        if let Some((drag_down, placement)) = bubble {
            self.velocity_y = fluid::bubble_column_velocity(drag_down, placement, self.velocity_y);
        } else {
            self.velocity_y -= gravity;
        }

        let moved = self.move_by(world, self.velocity_x, self.velocity_y, self.velocity_z);
        let drag = match kind {
            FluidKind::Water => WATER_DRAG,
            FluidKind::Lava => LAVA_DRAG,
        };
        self.velocity_x = moved.dx * drag;
        self.velocity_y = moved.dy * drag;
        self.velocity_z = moved.dz * drag;
        self.fall_distance = 0.0;
        TickOutcome {
            moved: (self.x, self.y, self.z) != start,
            on_ground: self.on_ground,
            landed: false,
            fall_damage: 0.0,
            freeze_damage,
        }
    }

    /// Vanilla `Player.tryToStartFallFlying` plus the client's jump-edge and
    /// climbable guards. A glider must be equipped, the player airborne, not
    /// fly-mode, not levitating, out of fluids, and off a climbable block.
    /// Returns whether gliding actually started.
    pub fn try_start_gliding(&mut self, world: &mut impl PhysicsWorld) -> bool {
        if self.fall_flying
            || !self.elytra
            || self.on_ground
            || self.flying
            || self.levitation_level > 0
            || self.fluid_at_feet(world).is_some()
            || self.on_climbable(world)
        {
            return false;
        }
        self.fall_flying = true;
        true
    }

    /// `LivingEntity.canGlide` with the conservative water/climbable guards
    /// used at entry. Checked each tick so landing, submersion, removing the
    /// elytra, or levitation cancels an active glide.
    fn can_continue_gliding(&mut self, world: &mut impl PhysicsWorld) -> bool {
        self.elytra
            && !self.on_ground
            && !self.flying
            && self.levitation_level == 0
            && self.fluid_at_feet(world).is_none()
            && !self.on_climbable(world)
    }

    /// `LivingEntity.travelFallFlying`: update velocity from look direction,
    /// move with the shared collider, and clear accumulated fall distance the
    /// way `checkSlowFallDistance` does. Wall-impact damage and durability
    /// wear are not modeled (see `DECISIONS.md`).
    fn tick_gliding(
        &mut self,
        world: &mut impl PhysicsWorld,
        pitch: f32,
        start: (f64, f64, f64),
        freeze_damage: f32,
    ) -> TickOutcome {
        let gravity = if self.slow_falling && self.velocity_y <= 0.0 {
            GRAVITY.min(0.01)
        } else {
            GRAVITY
        };
        let (x, y, z) = glide_velocity(
            self.velocity_x,
            self.velocity_y,
            self.velocity_z,
            self.yaw,
            pitch,
            gravity,
        );
        self.velocity_x = x;
        self.velocity_y = y;
        self.velocity_z = z;
        if self.velocity_y > GLIDE_SLOW_FALL_THRESHOLD {
            self.fall_distance = 0.0;
        }
        let moved = self.move_by(world, x, y, z);
        self.velocity_x = moved.dx;
        self.velocity_y = moved.dy;
        self.velocity_z = moved.dz;
        if moved.on_ground {
            self.fall_flying = false;
        }
        TickOutcome {
            moved: (self.x, self.y, self.z) != start,
            on_ground: self.on_ground,
            landed: moved.on_ground,
            fall_damage: 0.0,
            freeze_damage,
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
        // Vanilla launches a glide on a rising jump edge (the client sends a
        // START_FALL_FLYING command). Track the edge before any early return.
        let jump_pressed = input.jumping && !self.previous_jumping;
        self.previous_jumping = input.jumping;
        // `LivingEntity.aiStep` zeros tiny horizontal velocities for players.
        if self.velocity_x * self.velocity_x + self.velocity_z * self.velocity_z < 9.0e-6 {
            self.velocity_x = 0.0;
            self.velocity_z = 0.0;
        }
        if input.jumping && self.on_ground {
            let jump_factor = self.block_jump_factor(world);
            self.jump_with_factor(jump_factor);
        }
        if jump_pressed {
            self.try_start_gliding(world);
        }
        let freeze_damage = self.tick_frozen_state(world);

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
                freeze_damage,
            };
            return outcome;
        }

        // Fluids win over gliding in `LivingEntity.travel`; water and lava
        // also cancel an active glide here so leaving the fluid does not
        // silently resume flight.
        let fluid = self.fluid_at_feet(world);
        if self.fall_flying && (fluid.is_some() || !self.can_continue_gliding(world)) {
            self.fall_flying = false;
        }
        if let Some(state) = fluid {
            return self.tick_in_fluid(world, state, input, start, freeze_damage);
        }
        if self.fall_flying {
            let pitch = input.pitch.unwrap_or(self.pitch);
            return self.tick_gliding(world, pitch, start, freeze_damage);
        }

        let powder_snow = self.powder_snow_at(world);
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

        // `LivingEntity.handleOnClimbable`: a climber is held to 0.15 blocks
        // per tick horizontally and downwards, stops sliding when sneaking,
        // and resets fall distance every tick. Scaffolding is the exception
        // to the sneaking rule.
        let climbable = self.on_climbable(world);
        let scaffolding = climbable && {
            let (x, y, z) = self.feet();
            world.is_scaffolding(x, y, z)
        };
        if climbable {
            self.fall_distance = 0.0;
            self.velocity_x = self.velocity_x.clamp(-CLIMB_MOVE_LIMIT, CLIMB_MOVE_LIMIT);
            self.velocity_z = self.velocity_z.clamp(-CLIMB_MOVE_LIMIT, CLIMB_MOVE_LIMIT);
            self.velocity_y = self.velocity_y.max(-CLIMB_MOVE_LIMIT);
            if self.velocity_y < 0.0 && !scaffolding && self.sneaking {
                self.velocity_y = 0.0;
            }
        }

        // Levitation replaces gravity with an upward approach to a per-level
        // target; slow falling keeps a fraction of gravity and caps descent.
        // Both are no-ops with the inert defaults, so ordinary air travel keeps
        // the same GRAVITY recurrence and existing fixtures stay unchanged.
        let gravity = if self.levitation_level > 0 {
            let target = LEVITATION_ACCELERATION * f64::from(self.levitation_level);
            self.velocity_y += (target - self.velocity_y) * LEVITATION_APPROACH;
            0.0
        } else if self.slow_falling {
            GRAVITY * SLOW_FALL_GRAVITY_FACTOR
        } else {
            GRAVITY
        };

        // `WebBlock.entityInside` arms the multiplier at the end of the tick
        // in which the player enters the web; the following move consumes it
        // and `Entity.move` zeroes delta movement.
        let stuck = std::mem::replace(&mut self.cobweb_stuck, false);
        let (mut move_x, mut move_y, mut move_z) =
            (self.velocity_x, self.velocity_y, self.velocity_z);
        if stuck {
            move_x *= COBWEB_MOVE_MULTIPLIER.0;
            move_y *= COBWEB_MOVE_MULTIPLIER.1;
            move_z *= COBWEB_MOVE_MULTIPLIER.2;
        }
        // `PowderSnowBlock.entityInside` damps movement before the collision
        // move, so an entity sinks slowly and cannot sprint through the snow.
        if powder_snow {
            move_x *= POWDER_SNOW_MOVE_MULTIPLIER.0;
            move_y *= POWDER_SNOW_MOVE_MULTIPLIER.1;
            move_z *= POWDER_SNOW_MOVE_MULTIPLIER.2;
        }
        let moved = self.move_by(world, move_x, move_y, move_z);
        if stuck {
            self.velocity_x = 0.0;
            self.velocity_y = 0.0;
            self.velocity_z = 0.0;
        }
        let climb_lift = climbable && (moved.horizontal_collision || input.jumping);
        if climb_lift {
            // `handleRelativeFrictionAndCalculateMovement` returns a 0.2 y
            // velocity for a climber pressing into a wall or holding jump.
            // `travelInAir` then subtracts gravity and applies drag to that
            // returned velocity, not to the displacement just made.
            self.velocity_y = CLIMB_SPEED;
        }

        // `Entity.move` scales horizontal delta movement by the destination
        // block's speed factor after the collision move. A slime landing
        // reverses the pre-collision vertical velocity (`SlimeBlock.bounceUp`).
        let speed_factor = self.block_speed_factor(world);
        if stuck {
            self.velocity_x = 0.0;
            self.velocity_y = -GRAVITY * DRAG;
            self.velocity_z = 0.0;
        } else {
            self.velocity_x = moved.dx * friction * AIR_FRICTION * speed_factor;
            self.velocity_y = match (moved.bounce_velocity, climb_lift) {
                (Some(bounce), _) => (bounce - gravity) * DRAG,
                (None, true) => (CLIMB_SPEED - gravity) * DRAG,
                (None, false) => (moved.dy - gravity) * DRAG,
            };
            self.velocity_z = moved.dz * friction * AIR_FRICTION * speed_factor;
        }
        if self.slow_falling && self.levitation_level == 0 {
            self.velocity_y = self.velocity_y.max(SLOW_FALL_TERMINAL_VELOCITY);
        }
        // `Entity.applyEffectsFromBlocks` calls `SlimeBlock.stepOn` at the end
        // of the tick, after `travelInAir` has applied gravity and drag.
        if self.on_ground
            && !self.sneaking
            && self.velocity_y.abs() < 0.1
            && self.standing_on_slime(world)
        {
            let factor = SLIME_STEP_BASE + self.velocity_y.abs() * SLIME_STEP_FACTOR;
            self.velocity_x *= factor;
            self.velocity_z *= factor;
        }

        let mut landed = false;
        let mut fall_damage = 0.0;
        if self.swimming || powder_snow || self.slow_falling || self.levitation_level > 0 {
            // Slow falling and levitation clear the accumulated fall distance
            // every tick, so landing under either effect deals no damage.
            self.fall_distance = 0.0;
        } else {
            if moved.dy < 0.0 {
                self.fall_distance += -moved.dy;
            }
            if moved.on_ground && self.fall_distance > 0.0 {
                // `SlimeBlock.fallOn` calls `causeFallDamage(distance, 0.0)`.
                if moved.bounce_velocity.is_none() {
                    fall_damage = fall_damage_for(self.fall_distance);
                }
                self.fall_distance = 0.0;
                landed = true;
            }
        }
        if !self.flying {
            let (x, y, z) = self.feet();
            if world.is_cobweb(x, y, z) {
                self.cobweb_stuck = true;
                self.fall_distance = 0.0;
            }
        }
        TickOutcome {
            moved: (self.x, self.y, self.z) != start,
            on_ground: self.on_ground,
            landed,
            fall_damage,
            freeze_damage,
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
            if !self.flying
                && !self.swimming
                && !self.powder_snow_at(world)
                && self.fluid_at_feet(world).is_none()
                && movement.dy < 0.0
            {
                self.fall_distance += -movement.dy;
            }
            let grounded = movement.on_ground || (client.on_ground && self.is_supported(world));
            landed = grounded;
            if landed && self.fall_distance > 0.0 {
                if !self.fall_damage_suppressed(world) {
                    fall_damage = fall_damage_for(self.fall_distance);
                }
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
            if !self.flying
                && !self.swimming
                && !self.powder_snow_at(world)
                && self.fluid_at_feet(world).is_none()
                && dy < 0.0
            {
                self.fall_distance += -dy;
            }
            landed = client.on_ground && self.is_supported(world);
            if landed && self.fall_distance > 0.0 {
                if !self.fall_damage_suppressed(world) {
                    fall_damage = fall_damage_for(self.fall_distance);
                }
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
        let horizontal_collision = applied_x != dx || applied_z != dz;
        let mut bounce_velocity = None;
        if vertical_collision && !self.sneaking {
            let below_y = (self.y - 1.0e-3).floor() as i32;
            if world.is_slime(self.x.floor() as i32, below_y, self.z.floor() as i32) {
                // `SlimeBlock.bounceUp` reverses downward motion for living
                // entities (multiplier 1.0); sneaking suppresses the bounce.
                bounce_velocity = Some(-dy);
            }
        }
        // `Entity.move` clips the fall path against the
        // `FALL_DAMAGE_RESETTING` tag once per movement of at least one block.
        if self.fall_distance != 0.0
            && applied_x * applied_x + applied_y * applied_y + applied_z * applied_z >= 1.0
            && swept_has_fall_reset(world, start_box, applied_x, applied_y, applied_z)
        {
            self.fall_distance = 0.0;
        }
        Moved {
            dx: applied_x,
            dy: applied_y,
            dz: applied_z,
            on_ground,
            horizontal_collision,
            bounce_velocity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Moved {
    dx: f64,
    dy: f64,
    dz: f64,
    on_ground: bool,
    horizontal_collision: bool,
    bounce_velocity: Option<f64>,
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

/// Whether the block positions covered by a swept player AABB contain a
/// `fall_damage_resetting` block (cobwebs, ladders, vines, scaffolding, ...).
fn swept_has_fall_reset(
    world: &mut impl PhysicsWorld,
    area: Aabb,
    dx: f64,
    dy: f64,
    dz: f64,
) -> bool {
    let end = area.offset(dx, dy, dz);
    let x0 = area.min_x.min(end.min_x).floor() as i32;
    let x1 = area.max_x.max(end.max_x).floor() as i32;
    let y0 = area.min_y.min(end.min_y).floor() as i32;
    let y1 = area.max_y.max(end.max_y).floor() as i32;
    let z0 = area.min_z.min(end.min_z).floor() as i32;
    let z1 = area.max_z.max(end.max_z).floor() as i32;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                if world.resets_fall_damage(x, y, z) {
                    return true;
                }
            }
        }
    }
    false
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

    #[test]
    fn special_movement_defaults_are_inert_for_flat_worlds() {
        let mut world = FlatPhysicsWorld::new(0);
        assert!(!world.is_climbable(0, 0, 0));
        assert!(!world.is_scaffolding(0, 0, 0));
        assert!(!world.is_cobweb(0, 0, 0));
        assert!(!world.is_slime(0, 0, 0));
        assert!(!world.is_powder_snow(0, 0, 0));
        assert!(!world.resets_fall_damage(0, 0, 0));
        assert!(world.fluid_state(0, 0, 0).is_none());
        assert_eq!(world.fluid_flow(0, 0, 0), (0.0, 0.0, 0.0));
        assert_eq!(world.bubble_column_drag(0, 0, 0), None);

        let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
        physics.on_ground = true;
        let outcome = physics.tick(&mut world, MovementInput::default());
        assert!(!outcome.moved);
        assert!(!physics.cobweb_stuck);
        assert_eq!(physics.velocity_y, -GRAVITY * DRAG);
    }
}
