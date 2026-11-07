//! Entity goals, AI tick, and pathing foundation.
//!
//! This module is deliberately independent of the player session so it can be
//! tested against a plain world. It ports the structural parts of Folia's
//! `Goal`, `GoalSelector`, `Goal.Flag`, `PathNavigation`, and `LookControl`,
//! plus the five goals needed by the first mob slice.
//!
//! The implementation is split into three layers:
//!
//! * [`WorldView`] is the injected block/light/biome source. Tests and the
//!   eventual region tick loop both provide this; nothing here touches the
//!   shared server storage.
//! * [`find_path`] and [`Navigation`] are a bounded deterministic A* over the
//!   walkable block surface.
//! * [`GoalSelector`] plus [`AiAgent`] reproduce the vanilla per-tick goal
//!   ordering, flag locks, and stop/start transitions.

use crate::block_behaviour;
use crate::chunk::BlockState;
use crate::math::{BlockPos, Vec3};
use crate::random::{LegacyRandomSource, RandomSource};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Stable entity id used by the AI layer.
pub type EntityId = i32;

/// Block, light, and biome access required by AI and spawning.
///
/// `None` means "outside the loaded/bounded world" and is treated as
/// unreachable rather than as air. Implementations must not mutate while a
/// query is in flight.
pub trait WorldView {
    /// State at `pos`, or `None` when the position is outside the world.
    fn block(&self, pos: BlockPos) -> Option<&BlockState>;

    /// Sky light at `pos` (`0..=15`), if known.
    fn sky_light(&self, _pos: BlockPos) -> Option<u8> {
        None
    }

    /// Block light at `pos` (`0..=15`), if known.
    fn block_light(&self, _pos: BlockPos) -> Option<u8> {
        None
    }

    /// Namespaced biome id at `pos`, if known.
    fn biome(&self, _pos: BlockPos) -> Option<&str> {
        None
    }

    /// First empty block above the surface column at `(x, z)`.
    fn surface_y(&self, _x: i32, _z: i32) -> Option<i32> {
        None
    }

    /// Inclusive minimum build height.
    fn min_y(&self) -> i32 {
        -64
    }

    /// Exclusive maximum build height.
    fn max_y(&self) -> i32 {
        320
    }
}

/// True when a state is one of the three air states.
pub fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// True when a state carries any fluid.
pub fn is_liquid(state: &BlockState) -> bool {
    block_behaviour::fluid(state).is_some()
}

/// True when a state carries water.
pub fn is_water(state: &BlockState) -> bool {
    block_behaviour::fluid(state) == Some("minecraft:water")
}

/// True when a state carries lava.
pub fn is_lava(state: &BlockState) -> bool {
    block_behaviour::fluid(state) == Some("minecraft:lava")
}

/// Vanilla `Goal.Flag` bitset.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct GoalFlags(u8);

impl GoalFlags {
    pub const UNKNOWN_BEHAVIOR: Self = Self(1 << 0);
    pub const MOVE: Self = Self(1 << 1);
    pub const LOOK: Self = Self(1 << 2);
    pub const JUMP: Self = Self(1 << 3);
    pub const TARGET: Self = Self(1 << 4);

    /// All five flags in vanilla declaration order.
    pub const ALL: [Self; 5] = [
        Self::UNKNOWN_BEHAVIOR,
        Self::MOVE,
        Self::LOOK,
        Self::JUMP,
        Self::TARGET,
    ];

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Vanilla adds `UNKNOWN_BEHAVIOR` when a goal sets no flags.
    pub fn effective(self) -> Self {
        if self.is_empty() {
            Self::UNKNOWN_BEHAVIOR
        } else {
            self
        }
    }

    /// Indices of every set flag, in declaration order.
    pub fn indices(self) -> impl Iterator<Item = usize> {
        (0..Self::ALL.len()).filter(move |index| self.0 & (1 << index) != 0)
    }
}

impl std::ops::BitOr for GoalFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// Minimal entity classification used by the goal slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorKind {
    Player,
    Zombie,
    Other,
}

/// A living entity snapshot visible to goals and target selectors.
#[derive(Clone, Debug)]
pub struct ActorView {
    pub id: EntityId,
    pub kind: ActorKind,
    pub pos: Vec3,
    pub eye_height: f64,
    pub health: f32,
    pub alive: bool,
    pub creative: bool,
    pub spectator: bool,
}

impl ActorView {
    pub fn player(id: EntityId, pos: Vec3) -> Self {
        Self {
            id,
            kind: ActorKind::Player,
            pos,
            eye_height: 1.62,
            health: 20.0,
            alive: true,
            creative: false,
            spectator: false,
        }
    }

    pub fn eye_pos(&self) -> Vec3 {
        self.pos.with_y(self.pos.y() + self.eye_height)
    }

    pub fn can_be_targeted(&self) -> bool {
        self.alive && !self.creative && !self.spectator
    }
}

/// Events produced while ticking goals.
#[derive(Clone, Debug, PartialEq)]
pub enum AiEvent {
    Attack {
        attacker: EntityId,
        target: EntityId,
        damage: f32,
    },
}

/// Context passed to a goal for one lifecycle call.
pub struct GoalContext<'a> {
    pub mob: &'a mut AiMob,
    pub world: &'a dyn WorldView,
    pub actors: &'a [ActorView],
    pub look: &'a mut LookControl,
    pub events: &'a mut Vec<AiEvent>,
    pub game_time: i64,
}

impl GoalContext<'_> {
    pub fn actor(&self, id: EntityId) -> Option<&ActorView> {
        self.actors.iter().find(|actor| actor.id == id)
    }

    pub fn nearest_player(&self, radius: f64) -> Option<&ActorView> {
        self.actors
            .iter()
            .filter(|actor| actor.kind == ActorKind::Player && actor.can_be_targeted())
            .filter(|actor| actor.pos.distance_to_sqr(self.mob.pos) <= radius * radius)
            .min_by(|left, right| {
                left.pos
                    .distance_to_sqr(self.mob.pos)
                    .total_cmp(&right.pos.distance_to_sqr(self.mob.pos))
                    .then_with(|| left.id.cmp(&right.id))
            })
    }
}

/// Vanilla `Goal` lifecycle with the injected world and actor snapshot.
pub trait Goal {
    fn name(&self) -> &'static str;

    fn flags(&self) -> GoalFlags;

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool;

    fn can_continue_to_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        self.can_use(ctx)
    }

    fn is_interruptable(&self) -> bool {
        true
    }

    fn start(&mut self, _ctx: &mut GoalContext<'_>) {}

    fn stop(&mut self, _ctx: &mut GoalContext<'_>) {}

    fn tick(&mut self, _ctx: &mut GoalContext<'_>) {}

    fn requires_update_every_tick(&self) -> bool {
        false
    }
}

/// One goal plus its priority and running state.
pub struct WrappedGoal {
    priority: i32,
    goal: Box<dyn Goal>,
    running: bool,
}

impl WrappedGoal {
    pub fn new(priority: i32, goal: Box<dyn Goal>) -> Self {
        Self {
            priority,
            goal,
            running: false,
        }
    }

    pub fn priority(&self) -> i32 {
        self.priority
    }

    pub fn name(&self) -> &'static str {
        self.goal.name()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn flags(&self) -> GoalFlags {
        self.goal.flags().effective()
    }

    fn start(&mut self, ctx: &mut GoalContext<'_>) {
        if !self.running {
            self.running = true;
            self.goal.start(ctx);
        }
    }

    fn stop(&mut self, ctx: &mut GoalContext<'_>) {
        if self.running {
            self.running = false;
            self.goal.stop(ctx);
        }
    }
}

/// Port of `GoalSelector`: insertion-ordered goals, priority-based flag locks.
pub struct GoalSelector {
    goals: Vec<WrappedGoal>,
    locked: [Option<usize>; GoalFlags::ALL.len()],
    disabled: GoalFlags,
    rate: u32,
}

impl Default for GoalSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalSelector {
    pub fn new() -> Self {
        Self {
            goals: Vec::new(),
            locked: [None; GoalFlags::ALL.len()],
            disabled: GoalFlags::empty(),
            rate: 0,
        }
    }

    pub fn add_goal(&mut self, priority: i32, goal: Box<dyn Goal>) -> usize {
        self.goals.push(WrappedGoal::new(priority, goal));
        self.goals.len() - 1
    }

    pub fn remove_goal(&mut self, index: usize) {
        if index < self.goals.len() {
            if self.goals[index].running {
                // The selector has no context here; callers that need stop
                // side effects should let a tick stop the goal first.
                self.goals[index].running = false;
            }
            self.goals.remove(index);
            self.locked = [None; GoalFlags::ALL.len()];
        }
    }

    pub fn remove_all_goals(&mut self) {
        for goal in &mut self.goals {
            goal.running = false;
        }
        self.goals.clear();
        self.locked = [None; GoalFlags::ALL.len()];
    }

    pub fn available_goals(&self) -> &[WrappedGoal] {
        &self.goals
    }

    pub fn has_tasks(&self) -> bool {
        self.goals.iter().any(WrappedGoal::is_running)
    }

    pub fn running_names(&self) -> Vec<&'static str> {
        self.goals
            .iter()
            .filter(|goal| goal.running)
            .map(WrappedGoal::name)
            .collect()
    }

    pub fn running_priorities(&self) -> Vec<i32> {
        self.goals
            .iter()
            .filter(|goal| goal.running)
            .map(WrappedGoal::priority)
            .collect()
    }

    pub fn disable_control_flag(&mut self, flag: GoalFlags) {
        self.disabled = self.disabled.union(flag);
    }

    pub fn enable_control_flag(&mut self, flag: GoalFlags) {
        self.disabled = GoalFlags(self.disabled.bits() & !flag.bits());
    }

    pub fn set_control_flag(&mut self, flag: GoalFlags, enabled: bool) {
        if enabled {
            self.enable_control_flag(flag);
        } else {
            self.disable_control_flag(flag);
        }
    }

    pub fn inactive_tick(&mut self) -> bool {
        self.rate = self.rate.wrapping_add(1);
        self.rate.is_multiple_of(3)
    }

    fn can_replace_locked(&self, index: usize) -> bool {
        let candidate = &self.goals[index];
        for flag in candidate.flags().indices() {
            if let Some(locked_index) = self.locked[flag] {
                let locked = &self.goals[locked_index];
                let replaceable =
                    locked.goal.is_interruptable() && candidate.priority < locked.priority;
                if !replaceable {
                    return false;
                }
            }
        }
        true
    }

    /// One vanilla `GoalSelector.tick()` pass.
    pub fn tick(&mut self, ctx: &mut GoalContext<'_>) {
        for wrapped in &mut self.goals {
            if wrapped.running {
                let flags = wrapped.flags();
                if flags.intersects(self.disabled) || !wrapped.goal.can_continue_to_use(ctx) {
                    wrapped.stop(ctx);
                }
            }
        }

        for slot in &mut self.locked {
            if let Some(index) = *slot
                && !self.goals[index].running
            {
                *slot = None;
            }
        }

        for index in 0..self.goals.len() {
            if self.goals[index].running
                || self.goals[index].flags().intersects(self.disabled)
                || !self.can_replace_locked(index)
            {
                continue;
            }

            if !self.goals[index].goal.can_use(ctx) {
                continue;
            }

            let flags = self.goals[index].flags();
            for flag in flags.indices() {
                if let Some(locked_index) = self.locked[flag]
                    && locked_index != index
                {
                    self.goals[locked_index].stop(ctx);
                }
                self.locked[flag] = Some(index);
            }
            self.goals[index].start(ctx);
        }

        self.tick_running_goals(ctx, true);
    }

    /// Ticks running goals. Vanilla calls this with `true` from `tick()`.
    pub fn tick_running_goals(&mut self, ctx: &mut GoalContext<'_>, tick_all: bool) {
        for wrapped in &mut self.goals {
            if wrapped.running && (tick_all || wrapped.goal.requires_update_every_tick()) {
                wrapped.goal.tick(ctx);
            }
        }
    }
}

/// Port of `LookControl`, reduced to the heading/pitch state goals need.
#[derive(Clone, Debug, Default)]
pub struct LookControl {
    wanted: Option<Vec3>,
    yaw: f32,
    pitch: f32,
    cooldown: i32,
}

impl LookControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_look_at(&mut self, x: f64, y: f64, z: f64) {
        self.wanted = Some(Vec3::new(x, y, z));
        self.cooldown = 2;
    }

    pub fn set_look_at_actor(&mut self, actor: &ActorView) {
        let eye = actor.eye_pos();
        self.set_look_at(eye.x(), eye.y(), eye.z());
    }

    pub fn is_looking_at_target(&self) -> bool {
        self.cooldown > 0
    }

    pub fn wanted(&self) -> Option<Vec3> {
        self.wanted
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    pub fn tick(&mut self) {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            let Some(wanted) = self.wanted else {
                return;
            };
            let dx = wanted.x();
            let dy = wanted.y();
            let dz = wanted.z();
            let horizontal = dx.hypot(dz);
            if horizontal > 1.0e-5 {
                self.yaw = (dz.atan2(dx).to_degrees() as f32) - 90.0;
            }
            if horizontal > 1.0e-5 || dy.abs() > 1.0e-5 {
                self.pitch = -(dy.atan2(horizontal).to_degrees() as f32);
            }
        }
    }
}

/// Explicit path failure. Navigation never silently returns an empty path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathFailure {
    InvalidStart,
    InvalidTarget,
    Unreachable,
    TooLarge,
    Stuck,
}

impl fmt::Display for PathFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStart => formatter.write_str("path start is not walkable"),
            Self::InvalidTarget => formatter.write_str("path target is not walkable"),
            Self::Unreachable => formatter.write_str("path target is unreachable"),
            Self::TooLarge => formatter.write_str("path search exceeded its node budget"),
            Self::Stuck => formatter.write_str("navigation made no progress and was aborted"),
        }
    }
}

impl std::error::Error for PathFailure {}

/// A deterministic walk path. Points are feet positions, starting at the
/// resolved start node and ending at the target node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Path {
    points: Vec<BlockPos>,
    target: BlockPos,
}

impl Path {
    pub fn points(&self) -> &[BlockPos] {
        &self.points
    }

    pub fn target(&self) -> BlockPos {
        self.target
    }

    pub fn node_count(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn end(&self) -> Option<BlockPos> {
        self.points.last().copied()
    }
}

/// Bounds, mobility, and node costs for one path search.
///
/// Defaults follow the vanilla walk evaluator: four-way walking plus diagonal
/// moves, a one-block step-up, a three-block fall, water at malus `8`, and no
/// light penalty. `allow_doors` and `allow_climb` enable the slice's door/gate
/// and climbable node types.
#[derive(Clone, Copy, Debug)]
pub struct PathOptions {
    pub max_nodes: usize,
    /// Straight-line search radius (`PathNavigation.getMaxPathLength`).
    pub max_range: f64,
    /// Maximum accumulated walked distance.
    pub max_path_length: f64,
    pub max_fall_distance: i32,
    pub allow_water: bool,
    /// Walk through wooden doors and fence gates as `WALKABLE_DOOR` nodes.
    pub allow_doors: bool,
    /// Treat ladders, vines, and scaffolding as climbable nodes.
    pub allow_climb: bool,
    /// Use [`PathOptions::sprint_cost`] for walkable steps.
    pub sprinting: bool,
    pub walk_cost: f64,
    pub sprint_cost: f64,
    pub water_cost: f64,
    pub water_border_cost: f64,
    pub damage_cost: f64,
    pub fire_damage_cost: f64,
    pub door_cost: f64,
    pub climb_cost: f64,
    /// Extra malus per light level below [`PathOptions::min_light`].
    pub light_malus: f64,
    pub min_light: u8,
}

impl Default for PathOptions {
    fn default() -> Self {
        Self {
            max_nodes: 4096,
            max_range: 64.0,
            max_path_length: 64.0,
            max_fall_distance: 3,
            allow_water: false,
            allow_doors: true,
            allow_climb: true,
            sprinting: false,
            walk_cost: 1.0,
            sprint_cost: 0.8,
            water_cost: 8.0,
            water_border_cost: 8.0,
            damage_cost: 8.0,
            fire_damage_cost: 16.0,
            door_cost: 0.0,
            climb_cost: 1.5,
            light_malus: 0.0,
            min_light: 0,
        }
    }
}

/// Vanilla `PathType` classification used by the node evaluator.
///
/// `vanilla_malus` mirrors `PathType.getMalus()`; negative values are not
/// traversable. Per-search costs can override the maluses in [`PathOptions`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PathType {
    Blocked,
    Walkable,
    WalkableDoor,
    Water,
    WaterBorder,
    DoorOpen,
    DoorWoodClosed,
    DoorIronClosed,
    Fence,
    Leaves,
    Lava,
    DamageFire,
    DangerFire,
    DamageOther,
    DangerOther,
    StickyHoney,
    Trapdoor,
}

impl PathType {
    /// Vanilla `PathType.getMalus()`, before [`PathOptions`] overrides.
    pub const fn vanilla_malus(self) -> f64 {
        match self {
            Self::Water
            | Self::WaterBorder
            | Self::DangerFire
            | Self::DangerOther
            | Self::StickyHoney => 8.0,
            Self::DamageFire => 16.0,
            Self::Blocked | Self::Fence | Self::Leaves | Self::Lava | Self::DamageOther => -1.0,
            Self::Walkable
            | Self::WalkableDoor
            | Self::DoorOpen
            | Self::DoorWoodClosed
            | Self::DoorIronClosed
            | Self::Trapdoor => 0.0,
        }
    }
}

/// Result of evaluating one path node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeEvaluation {
    pub path_type: PathType,
    /// Per-node cost malus. Negative means the node is not traversable.
    pub malus: f64,
    pub climbable: bool,
    pub swimmable: bool,
}

impl NodeEvaluation {
    fn blocked(path_type: PathType) -> Self {
        Self {
            path_type,
            malus: -1.0,
            climbable: false,
            swimmable: false,
        }
    }

    pub fn is_walkable(&self) -> bool {
        self.malus >= 0.0
    }
}

fn state_property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn state_bool(state: &BlockState, key: &str) -> Option<bool> {
    if let Some(value) = state_property(state, key) {
        return Some(value == "true");
    }
    let default = crate::registry::default_block_state(&state.name)?;
    state_property(&default, key).map(|value| value == "true")
}

fn is_door(state: &BlockState) -> bool {
    state.name.ends_with("_door") && !state.name.ends_with("_trapdoor")
}

fn is_fence_gate(state: &BlockState) -> bool {
    state.name.ends_with("_fence_gate")
}

fn is_trapdoor(state: &BlockState) -> bool {
    state.name.ends_with("_trapdoor")
}

fn is_leaves(state: &BlockState) -> bool {
    state.name.ends_with("_leaves")
}

fn is_fence_like(state: &BlockState) -> bool {
    state.name.ends_with("_fence") || state.name.ends_with("_wall")
}

fn is_climbable_state(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:ladder"
            | "minecraft:vine"
            | "minecraft:scaffolding"
            | "minecraft:twisting_vines"
            | "minecraft:twisting_vines_plant"
            | "minecraft:weeping_vines"
            | "minecraft:weeping_vines_plant"
            | "minecraft:cave_vines"
            | "minecraft:cave_vines_plant"
    )
}

fn is_damage_other(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:cactus"
            | "minecraft:sweet_berry_bush"
            | "minecraft:pointed_dripstone"
            | "minecraft:wither_rose"
            | "minecraft:powder_snow"
    )
}

fn is_burning_state(state: &BlockState) -> bool {
    match state.name.as_str() {
        "minecraft:fire" | "minecraft:soul_fire" => true,
        "minecraft:magma_block" | "minecraft:lava_cauldron" => true,
        "minecraft:campfire" | "minecraft:soul_campfire" => {
            state_bool(state, "lit").unwrap_or(true)
        }
        _ => false,
    }
}

fn evaluate_door(
    feet: &BlockState,
    head: &BlockState,
    below: &BlockState,
    options: &PathOptions,
) -> NodeEvaluation {
    let door = if is_door(feet) { feet } else { head };
    let open = state_bool(door, "open").unwrap_or(false);
    let iron = door.name == "minecraft:iron_door";
    let supported = block_behaviour::has_collision(below);

    if open {
        if !options.allow_doors
            || !supported
            || block_behaviour::has_collision(feet)
            || block_behaviour::has_collision(head)
        {
            return NodeEvaluation::blocked(PathType::DoorOpen);
        }
        return NodeEvaluation {
            path_type: PathType::DoorOpen,
            malus: options.door_cost,
            climbable: false,
            swimmable: false,
        };
    }

    if iron {
        return NodeEvaluation::blocked(PathType::DoorIronClosed);
    }
    if !options.allow_doors {
        return NodeEvaluation::blocked(PathType::DoorWoodClosed);
    }
    let paired = is_door(feet) && is_door(head) && feet.name == head.name && supported;
    if !paired {
        return NodeEvaluation::blocked(PathType::DoorWoodClosed);
    }
    NodeEvaluation {
        path_type: PathType::WalkableDoor,
        malus: options.door_cost,
        climbable: false,
        swimmable: false,
    }
}

fn evaluate_gate(
    feet: &BlockState,
    head: &BlockState,
    below: &BlockState,
    options: &PathOptions,
) -> NodeEvaluation {
    let open = state_bool(feet, "open").unwrap_or(false);
    if block_behaviour::has_collision(head)
        || !block_behaviour::has_collision(below)
        || !options.allow_doors
    {
        return NodeEvaluation::blocked(if open {
            PathType::DoorOpen
        } else {
            PathType::Fence
        });
    }
    if open {
        if block_behaviour::has_collision(feet) {
            return NodeEvaluation::blocked(PathType::DoorOpen);
        }
        return NodeEvaluation {
            path_type: PathType::DoorOpen,
            malus: options.door_cost,
            climbable: false,
            swimmable: false,
        };
    }
    NodeEvaluation {
        path_type: PathType::WalkableDoor,
        malus: options.door_cost,
        climbable: false,
        swimmable: false,
    }
}

fn classify_node(
    world: &dyn WorldView,
    pos: BlockPos,
    options: &PathOptions,
) -> Option<NodeEvaluation> {
    let feet = world.block(pos)?;
    let head = world.block(pos.above())?;
    let below = world.block(pos.below())?;

    // Lava anywhere in the standing column is fatal, even when swimming.
    if is_lava(feet) || is_lava(head) || is_lava(below) {
        return Some(NodeEvaluation::blocked(PathType::Lava));
    }

    // Closed doors and gates still have collision shapes, so they are
    // classified before the generic collision checks.
    if is_door(feet) || is_door(head) {
        return Some(evaluate_door(feet, head, below, options));
    }
    if is_fence_gate(feet) {
        return Some(evaluate_gate(feet, head, below, options));
    }

    let feet_climbable = is_climbable_state(feet);
    let head_climbable = is_climbable_state(head);
    let below_climbable = is_climbable_state(below);
    if feet_climbable || head_climbable {
        if !options.allow_climb {
            return Some(NodeEvaluation::blocked(PathType::Blocked));
        }
        if !block_behaviour::has_collision(feet)
            && !block_behaviour::has_collision(head)
            && (feet_climbable || below_climbable)
        {
            return Some(NodeEvaluation {
                path_type: PathType::Walkable,
                malus: 0.0,
                climbable: true,
                swimmable: false,
            });
        }
        return Some(NodeEvaluation::blocked(PathType::Blocked));
    }

    let feet_water = is_water(feet);
    let head_water = is_water(head);
    if feet_water || head_water {
        if !options.allow_water
            || !feet_water
            || block_behaviour::has_collision(head)
            || !(is_water(below) || block_behaviour::has_collision(below))
        {
            return Some(NodeEvaluation::blocked(PathType::Water));
        }
        return Some(NodeEvaluation {
            path_type: PathType::Water,
            malus: options.water_cost,
            climbable: false,
            swimmable: true,
        });
    }

    if block_behaviour::has_collision(feet) || block_behaviour::has_collision(head) {
        let path_type = if is_leaves(feet) || is_leaves(head) {
            PathType::Leaves
        } else if is_fence_like(feet) || is_fence_like(head) {
            PathType::Fence
        } else if is_trapdoor(feet) {
            PathType::Trapdoor
        } else {
            PathType::Blocked
        };
        return Some(NodeEvaluation::blocked(path_type));
    }

    if !block_behaviour::has_collision(below) {
        return Some(NodeEvaluation::blocked(PathType::Blocked));
    }

    let mut path_type = PathType::Walkable;
    let mut malus = 0.0;
    if is_burning_state(feet) || is_burning_state(below) {
        path_type = PathType::DamageFire;
        malus = options.fire_damage_cost;
    } else {
        let mut danger_fire = false;
        let mut danger_other = false;
        let mut water_border = false;
        for dy in -1_i32..=1 {
            for dx in -1_i32..=1 {
                for dz in -1_i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let Some(neighbor) = world.block(pos.offset(dx, dy, dz)) else {
                        continue;
                    };
                    if is_burning_state(neighbor) || is_lava(neighbor) {
                        danger_fire = true;
                    }
                    if is_damage_other(neighbor) {
                        danger_other = true;
                    }
                    if is_water(neighbor) {
                        water_border = true;
                    }
                }
            }
        }
        if danger_fire {
            path_type = PathType::DangerFire;
            malus = options.damage_cost;
        } else if danger_other {
            path_type = PathType::DangerOther;
            malus = options.damage_cost;
        } else if water_border {
            path_type = PathType::WaterBorder;
            malus = options.water_border_cost;
        }
    }

    Some(NodeEvaluation {
        path_type,
        malus,
        climbable: false,
        swimmable: false,
    })
}

/// Classify a standing node using vanilla path-type rules and the cost options.
///
/// `None` means the position is outside the loaded/bounded world. A returned
/// evaluation with a negative [`NodeEvaluation::malus`] is not traversable.
pub fn evaluate_node(
    world: &dyn WorldView,
    pos: BlockPos,
    options: &PathOptions,
) -> Option<NodeEvaluation> {
    let mut evaluation = classify_node(world, pos, options)?;
    if evaluation.is_walkable()
        && let Some(light) = world
            .sky_light(pos)
            .into_iter()
            .chain(world.block_light(pos))
            .max()
        && light < options.min_light
    {
        evaluation.malus += options.light_malus * f64::from(options.min_light - light);
    }
    Some(evaluation)
}

/// Returns true when a mob can stand with its feet at `pos`.
///
/// Swimming and climbable-only nodes are traversable by the pathfinder but not
/// considered stable standing destinations for goals such as random stroll.
pub fn is_standable(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> bool {
    evaluate_node(world, pos, options)
        .is_some_and(|node| node.is_walkable() && !node.swimmable && !node.climbable)
}

fn is_traversable(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> bool {
    evaluate_node(world, pos, options).is_some_and(|node| node.is_walkable())
}

fn resolve_start(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> Option<BlockPos> {
    for dy in [0, -1, 1, -2, 2, -3, 3] {
        let candidate = pos.offset(0, dy, 0);
        if is_traversable(world, candidate, options) {
            return Some(candidate);
        }
    }
    None
}

fn resolve_target(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> Option<BlockPos> {
    for dy in [0, 1, -1, 2, -2, -3, 3] {
        let candidate = pos.offset(0, dy, 0);
        if is_traversable(world, candidate, options) {
            return Some(candidate);
        }
    }
    None
}

fn node_key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x(), pos.y(), pos.z())
}

fn clear_vertical(world: &dyn WorldView, x: i32, z: i32, low: i32, high: i32) -> bool {
    for y in low..=high {
        let pos = BlockPos::new(x, y, z);
        let Some(state) = world.block(pos) else {
            return false;
        };
        if block_behaviour::has_collision(state) {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy, Debug)]
struct Neighbor {
    pos: BlockPos,
    evaluation: NodeEvaluation,
    distance: f64,
    drop: i32,
}

fn find_fall_neighbor(
    world: &dyn WorldView,
    from: BlockPos,
    dx: i32,
    dz: i32,
    options: &PathOptions,
) -> Option<Neighbor> {
    for drop in 1..=options.max_fall_distance {
        let candidate = from.offset(dx, -drop, dz);
        let feet = world.block(candidate)?;
        let head = world.block(candidate.above())?;
        if block_behaviour::has_collision(feet) || block_behaviour::has_collision(head) {
            return None;
        }
        if is_lava(feet) || is_lava(head) {
            return None;
        }
        let evaluation = evaluate_node(world, candidate, options)?;
        if evaluation.is_walkable() {
            if !clear_vertical(world, candidate.x(), candidate.z(), candidate.y(), from.y()) {
                return None;
            }
            return Some(Neighbor {
                pos: candidate,
                evaluation,
                distance: 1.0,
                drop,
            });
        }
        if is_liquid(feet) || is_liquid(head) {
            return None;
        }
    }
    None
}

fn orthogonal_neighbor(
    world: &dyn WorldView,
    from: BlockPos,
    dx: i32,
    dz: i32,
    options: &PathOptions,
) -> Option<Neighbor> {
    let level = from.offset(dx, 0, dz);
    if let Some(evaluation) = evaluate_node(world, level, options)
        && evaluation.is_walkable()
    {
        return Some(Neighbor {
            pos: level,
            evaluation,
            distance: 1.0,
            drop: 0,
        });
    }

    let up = from.offset(dx, 1, dz);
    if let Some(evaluation) = evaluate_node(world, up, options)
        && evaluation.is_walkable()
    {
        return Some(Neighbor {
            pos: up,
            evaluation,
            distance: std::f64::consts::SQRT_2,
            drop: 0,
        });
    }

    find_fall_neighbor(world, from, dx, dz, options)
}

fn diagonal_neighbor(
    world: &dyn WorldView,
    from: BlockPos,
    dx: i32,
    dz: i32,
    options: &PathOptions,
) -> Option<Neighbor> {
    // Corner-cut prevention: both orthogonal nodes must exist and must not be
    // above the root node. Vanilla also forbids diagonal moves through doors.
    let side_x = orthogonal_neighbor(world, from, dx, 0, options)?;
    let side_z = orthogonal_neighbor(world, from, 0, dz, options)?;
    if side_x.pos.y() > from.y() || side_z.pos.y() > from.y() {
        return None;
    }
    if side_x.evaluation.path_type == PathType::WalkableDoor
        || side_z.evaluation.path_type == PathType::WalkableDoor
    {
        return None;
    }

    let dest = from.offset(dx, 0, dz);
    if let Some(evaluation) = evaluate_node(world, dest, options)
        && evaluation.is_walkable()
        && evaluation.path_type != PathType::WalkableDoor
    {
        return Some(Neighbor {
            pos: dest,
            evaluation,
            distance: std::f64::consts::SQRT_2,
            drop: 0,
        });
    }

    let fall = find_fall_neighbor(world, from, dx, dz, options)?;
    if fall.pos.y() >= from.y() || fall.evaluation.path_type == PathType::WalkableDoor {
        return None;
    }
    Some(Neighbor {
        distance: std::f64::consts::SQRT_2,
        ..fall
    })
}

fn path_neighbors(world: &dyn WorldView, from: BlockPos, options: &PathOptions) -> Vec<Neighbor> {
    const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    const DIAGONALS: [(i32, i32); 4] = [(1, -1), (1, 1), (-1, 1), (-1, -1)];
    let mut result = Vec::with_capacity(12);

    for (dx, dz) in DIRECTIONS {
        if let Some(neighbor) = orthogonal_neighbor(world, from, dx, dz, options) {
            result.push(neighbor);
        }
    }

    for (dx, dz) in DIAGONALS {
        if let Some(neighbor) = diagonal_neighbor(world, from, dx, dz, options) {
            result.push(neighbor);
        }
    }

    // Vertical swim/climb moves are not part of the horizontal walk
    // directions, but water columns and ladders need them to progress.
    if options.allow_water || options.allow_climb {
        for dy in [1, -1] {
            let candidate = from.offset(0, dy, 0);
            if let Some(evaluation) = evaluate_node(world, candidate, options)
                && evaluation.is_walkable()
                && ((evaluation.swimmable && options.allow_water)
                    || (evaluation.climbable && options.allow_climb))
            {
                result.push(Neighbor {
                    pos: candidate,
                    evaluation,
                    distance: 1.0,
                    drop: i32::from(dy < 0),
                });
            }
        }
    }

    result
}

fn movement_cost(options: &PathOptions, evaluation: &NodeEvaluation) -> f64 {
    if evaluation.swimmable {
        options.walk_cost
    } else if evaluation.climbable {
        options.climb_cost
    } else if options.sprinting {
        options.sprint_cost
    } else {
        options.walk_cost
    }
}

fn neighbor_cost(options: &PathOptions, neighbor: &Neighbor) -> f64 {
    let mut cost = neighbor.distance * movement_cost(options, &neighbor.evaluation).max(0.0);
    if neighbor.drop > 0 {
        cost += f64::from(neighbor.drop) * 0.25;
    }
    cost + neighbor.evaluation.malus.max(0.0)
}

fn heuristic(from: BlockPos, to: BlockPos, options: &PathOptions) -> f64 {
    let dx = f64::from((from.x() - to.x()).unsigned_abs());
    let dy = f64::from((from.y() - to.y()).unsigned_abs());
    let dz = f64::from((from.z() - to.z()).unsigned_abs());
    let diagonal = dx.min(dz);
    let horizontal = dx.max(dz) - diagonal + diagonal * std::f64::consts::SQRT_2;
    let minimum_movement = if options.sprinting {
        options.sprint_cost.min(options.walk_cost)
    } else {
        options.walk_cost
    }
    .min(options.climb_cost)
    .max(0.0);
    (horizontal + dy) * minimum_movement
}

/// Deterministic bounded A* over block collision metadata.
///
/// The search walks eight horizontal directions with corner-cut prevention,
/// steps up exactly one block, drops at most [`PathOptions::max_fall_distance`]
/// blocks, and can swim or climb when the options allow it. Lava is always
/// rejected. Ties are broken by `(f, h, x, y, z)` so the same world and
/// endpoints always produce the same path.
pub fn find_path(
    world: &dyn WorldView,
    start: BlockPos,
    target: BlockPos,
    options: &PathOptions,
) -> Result<Path, PathFailure> {
    let start = resolve_start(world, start, options).ok_or(PathFailure::InvalidStart)?;
    let target = resolve_target(world, target, options).ok_or(PathFailure::InvalidTarget)?;

    let dx = f64::from(start.x() - target.x());
    let dy = f64::from(start.y() - target.y());
    let dz = f64::from(start.z() - target.z());
    let direct = (dx * dx + dy * dy + dz * dz).sqrt();
    if direct > options.max_range || direct > options.max_path_length {
        return Err(PathFailure::TooLarge);
    }
    if start == target {
        return Ok(Path {
            points: vec![start],
            target,
        });
    }

    let mut open: Vec<BlockPos> = vec![start];
    let mut best_cost: HashMap<BlockPos, f64> = HashMap::new();
    best_cost.insert(start, 0.0);
    let mut walked: HashMap<BlockPos, f64> = HashMap::new();
    walked.insert(start, 0.0);
    let mut parents: HashMap<BlockPos, BlockPos> = HashMap::new();
    let mut closed: HashSet<BlockPos> = HashSet::new();
    let mut expansions = 0usize;
    let mut length_limited = false;

    while !open.is_empty() {
        let mut best_index = 0usize;
        let mut best_score = f64::INFINITY;
        let mut best_estimate = f64::INFINITY;
        let mut best_key = (i32::MAX, i32::MAX, i32::MAX);
        for (index, candidate) in open.iter().enumerate() {
            let cost = best_cost.get(candidate).copied().unwrap_or(f64::INFINITY);
            let estimate = heuristic(*candidate, target, options);
            let score = cost + estimate;
            let key = node_key(*candidate);
            let better = score < best_score
                || (score == best_score
                    && (estimate < best_estimate || (estimate == best_estimate && key < best_key)));
            if better {
                best_index = index;
                best_score = score;
                best_estimate = estimate;
                best_key = key;
            }
        }

        let current = open.remove(best_index);
        if !closed.insert(current) {
            continue;
        }
        expansions += 1;
        if expansions > options.max_nodes {
            return Err(PathFailure::TooLarge);
        }

        if current == target {
            let mut points = vec![current];
            let mut cursor = current;
            while let Some(parent) = parents.get(&cursor).copied() {
                points.push(parent);
                cursor = parent;
                if cursor == start {
                    break;
                }
            }
            points.reverse();
            return Ok(Path { points, target });
        }

        let current_cost = best_cost.get(&current).copied().unwrap_or(0.0);
        let current_walked = walked.get(&current).copied().unwrap_or(0.0);
        for neighbor in path_neighbors(world, current, options) {
            if closed.contains(&neighbor.pos) {
                continue;
            }
            let neighbor_walked = current_walked + neighbor.distance;
            if neighbor_walked > options.max_path_length + 1.0e-9 {
                length_limited = true;
                continue;
            }
            let candidate_cost = current_cost + neighbor_cost(options, &neighbor);
            let previous = best_cost
                .get(&neighbor.pos)
                .copied()
                .unwrap_or(f64::INFINITY);
            if candidate_cost + 1.0e-9 < previous {
                best_cost.insert(neighbor.pos, candidate_cost);
                walked.insert(neighbor.pos, neighbor_walked);
                parents.insert(neighbor.pos, current);
                if !open.contains(&neighbor.pos) {
                    open.push(neighbor.pos);
                }
            }
        }
    }

    if length_limited {
        Err(PathFailure::TooLarge)
    } else {
        Err(PathFailure::Unreachable)
    }
}

/// Port of the movement half of `PathNavigation`, plus stuck detection, path
/// invalidation, and automatic repath/abort.
#[derive(Clone, Debug)]
pub struct Navigation {
    path: Option<Path>,
    index: usize,
    speed: f64,
    done: bool,
    options: PathOptions,
    last_failure: Option<PathFailure>,
    failures: u32,
    target: Option<BlockPos>,
    needs_repath: bool,
    aborts: u32,
    stuck: bool,
    tick_count: u32,
    stuck_check_interval: u32,
    stuck_after_ticks: u32,
    min_progress: f64,
    last_stuck_check_tick: u32,
    last_stuck_check_pos: Vec3,
    stalled_ticks: u32,
    reset_stuck_baseline: bool,
}

impl Default for Navigation {
    fn default() -> Self {
        Self::new()
    }
}

impl Navigation {
    pub fn new() -> Self {
        Self {
            path: None,
            index: 0,
            speed: 1.0,
            done: true,
            options: PathOptions::default(),
            last_failure: None,
            failures: 0,
            target: None,
            needs_repath: false,
            aborts: 0,
            stuck: false,
            tick_count: 0,
            stuck_check_interval: 20,
            stuck_after_ticks: 100,
            min_progress: 0.5,
            last_stuck_check_tick: 0,
            last_stuck_check_pos: Vec3::new(0.0, 0.0, 0.0),
            stalled_ticks: 0,
            reset_stuck_baseline: true,
        }
    }

    pub fn options(&self) -> &PathOptions {
        &self.options
    }

    pub fn options_mut(&mut self) -> &mut PathOptions {
        &mut self.options
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_ref()
    }

    pub fn last_failure(&self) -> Option<PathFailure> {
        self.last_failure
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Number of paths aborted by a failed repath or stuck detection.
    pub fn aborts(&self) -> u32 {
        self.aborts
    }

    /// True after [`Navigation::tick`]/[`Navigation::tick_with_world`] decided
    /// the mob made no progress for [`Navigation::stuck_after_ticks`].
    pub fn is_stuck(&self) -> bool {
        self.stuck
    }

    pub fn target(&self) -> Option<BlockPos> {
        self.target
    }

    /// Tune stuck detection. `abort_after_ticks` is a total budget of stalled
    /// time; `min_progress` is the movement below which a check window counts
    /// as stalled.
    pub fn set_stuck_limits(
        &mut self,
        check_interval: u32,
        abort_after_ticks: u32,
        min_progress: f64,
    ) {
        self.stuck_check_interval = check_interval.max(1);
        self.stuck_after_ticks = abort_after_ticks;
        self.min_progress = min_progress.max(0.0);
        self.reset_stuck_baseline = true;
    }

    pub fn is_done(&self) -> bool {
        self.done
            || self
                .path
                .as_ref()
                .is_none_or(|path| self.index >= path.node_count())
    }

    pub fn stop(&mut self) {
        self.path = None;
        self.index = 0;
        self.done = true;
        self.needs_repath = false;
        self.stuck = false;
        self.stalled_ticks = 0;
    }

    fn abort(&mut self, failure: Option<PathFailure>) {
        self.path = None;
        self.index = 0;
        self.done = true;
        self.needs_repath = false;
        self.target = None;
        self.stalled_ticks = 0;
        self.aborts = self.aborts.saturating_add(1);
        if failure.is_some() {
            self.last_failure = failure;
        }
    }

    pub fn move_to(&mut self, world: &dyn WorldView, from: BlockPos, target: BlockPos) -> bool {
        self.move_to_with_speed(world, from, target, 1.0)
    }

    pub fn move_to_with_speed(
        &mut self,
        world: &dyn WorldView,
        from: BlockPos,
        target: BlockPos,
        speed: f64,
    ) -> bool {
        match find_path(world, from, target, &self.options) {
            Ok(path) => self.move_to_path(path, speed),
            Err(failure) => {
                self.path = None;
                self.index = 0;
                self.done = true;
                self.target = None;
                self.needs_repath = false;
                self.last_failure = Some(failure);
                self.failures = self.failures.saturating_add(1);
                false
            }
        }
    }

    pub fn move_to_path(&mut self, path: Path, speed: f64) -> bool {
        if path.is_empty() {
            self.stop();
            return false;
        }
        self.speed = speed.max(0.05);
        self.index = usize::from(path.node_count() > 1);
        self.done = false;
        self.last_failure = None;
        self.target = Some(path.target());
        self.needs_repath = false;
        self.stuck = false;
        self.stalled_ticks = 0;
        self.reset_stuck_baseline = true;
        self.path = Some(path);
        true
    }

    /// Invalidates the path when a changed block lies on or beside any
    /// remaining node. The stored target is kept so `tick_with_world` or
    /// [`Navigation::repath`] can rebuild the route.
    pub fn on_block_changed(&mut self, pos: BlockPos) -> bool {
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        if self.index >= path.node_count() {
            return false;
        }
        let touches = path.points()[self.index..].iter().any(|node| {
            (node.x() - pos.x()).abs() <= 1
                && (node.y() - pos.y()).abs() <= 1
                && (node.z() - pos.z()).abs() <= 1
        });
        if !touches {
            return false;
        }

        self.path = None;
        self.index = 0;
        self.done = true;
        self.needs_repath = true;
        self.stalled_ticks = 0;
        self.reset_stuck_baseline = true;
        true
    }

    /// Rebuilds the current target from `from`. Returns false when the
    /// navigation has no target or the route is now unreachable.
    pub fn repath(&mut self, world: &dyn WorldView, from: BlockPos) -> bool {
        let Some(target) = self.target else {
            return false;
        };
        self.needs_repath = false;
        self.move_to_with_speed(world, from, target, self.speed)
    }

    fn path_is_active(&self) -> bool {
        !self.done
            && self
                .path
                .as_ref()
                .is_some_and(|path| self.index < path.node_count())
    }

    fn next_node_is_valid(&self, world: &dyn WorldView) -> bool {
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        let Some(node) = path.points().get(self.index) else {
            return true;
        };
        evaluate_node(world, *node, &self.options).is_some_and(|node| node.is_walkable())
    }

    fn sync_stuck_baseline(&mut self, pos: Vec3) {
        if self.reset_stuck_baseline {
            self.last_stuck_check_pos = pos;
            self.last_stuck_check_tick = self.tick_count;
            self.stalled_ticks = 0;
            self.reset_stuck_baseline = false;
        }
    }

    fn check_stuck(&mut self, pos: Vec3) {
        if self.tick_count.saturating_sub(self.last_stuck_check_tick) < self.stuck_check_interval {
            return;
        }
        let moved = self.last_stuck_check_pos.distance_to(pos);
        if moved < self.min_progress {
            self.stalled_ticks = self.stalled_ticks.saturating_add(self.stuck_check_interval);
        } else {
            self.stalled_ticks = 0;
        }
        self.last_stuck_check_tick = self.tick_count;
        self.last_stuck_check_pos = pos;
        if self.stuck_after_ticks > 0 && self.stalled_ticks >= self.stuck_after_ticks {
            self.stuck = true;
        }
    }

    fn advance(&mut self, pos: &mut Vec3) -> bool {
        if self.is_done() {
            self.done = true;
            return false;
        }
        let Some(path) = self.path.as_ref() else {
            return false;
        };
        let Some(node) = path.points().get(self.index).copied() else {
            self.done = true;
            return false;
        };
        let node_count = path.node_count();

        let target_x = f64::from(node.x()) + 0.5;
        let target_z = f64::from(node.z()) + 0.5;
        let dx = target_x - pos.x();
        let dz = target_z - pos.z();
        let distance = dx.hypot(dz);
        let step = (0.3 * self.speed).max(0.05);

        if distance <= step {
            *pos = Vec3::new(target_x, f64::from(node.y()), target_z);
            self.index += 1;
            if self.index >= node_count {
                self.done = true;
            }
        } else {
            *pos = Vec3::new(
                pos.x() + dx / distance * step,
                pos.y(),
                pos.z() + dz / distance * step,
            );
        }
        true
    }

    /// Advances the owner along the current path by one tick and tracks stuck
    /// progress. This does not read the world; use `tick_with_world` for path
    /// validation and automatic repathing.
    pub fn tick(&mut self, pos: &mut Vec3) -> bool {
        self.tick_count = self.tick_count.saturating_add(1);
        self.sync_stuck_baseline(*pos);
        let moved = self.advance(pos);
        if moved && !self.is_done() {
            self.check_stuck(*pos);
            if self.stuck {
                self.abort(Some(PathFailure::Stuck));
                return false;
            }
        }
        moved
    }

    /// Tick against the world: validate the next node, repath once when a
    /// block change invalidated it, and abort on a failed repath or no
    /// progress for the configured stuck budget.
    pub fn tick_with_world(&mut self, world: &dyn WorldView, pos: &mut Vec3) -> bool {
        self.tick_count = self.tick_count.saturating_add(1);
        self.sync_stuck_baseline(*pos);

        let from = BlockPos::new(
            pos.x().floor() as i32,
            pos.y().floor() as i32,
            pos.z().floor() as i32,
        );
        if (self.needs_repath || (self.path_is_active() && !self.next_node_is_valid(world)))
            && !self.repath(world, from)
        {
            self.abort(None);
            return false;
        }

        let moved = self.advance(pos);
        if moved && !self.is_done() {
            self.check_stuck(*pos);
            if self.stuck {
                self.abort(Some(PathFailure::Stuck));
                return false;
            }
        }
        moved
    }
}

/// A mob driven by goal selectors and navigation.
#[derive(Clone, Debug)]
pub struct AiMob {
    pub id: EntityId,
    pub kind: ActorKind,
    pub pos: Vec3,
    pub width: f32,
    pub height: f32,
    pub health: f32,
    pub alive: bool,
    pub target: Option<EntityId>,
    pub last_hurt_by: Option<EntityId>,
    pub last_hurt_by_timestamp: i64,
    pub in_water: bool,
    pub in_lava: bool,
    pub fluid_height: f64,
    pub on_ground: bool,
    pub no_action_time: i32,
    pub aggressive: bool,
    pub jump_requested: bool,
    pub jump_requests: u32,
    pub attack_count: u32,
    pub follow_range: f64,
    pub random: LegacyRandomSource,
    pub navigation: Navigation,
}

impl AiMob {
    pub fn new(id: EntityId, kind: ActorKind, pos: Vec3, seed: i64) -> Self {
        let (width, height) = match kind {
            ActorKind::Player => (0.6, 1.8),
            ActorKind::Zombie => (0.6, 1.95),
            ActorKind::Other => (0.6, 1.8),
        };
        Self {
            id,
            kind,
            pos,
            width,
            height,
            health: 20.0,
            alive: true,
            target: None,
            last_hurt_by: None,
            last_hurt_by_timestamp: 0,
            in_water: false,
            in_lava: false,
            fluid_height: 0.0,
            on_ground: false,
            no_action_time: 0,
            aggressive: false,
            jump_requested: false,
            jump_requests: 0,
            attack_count: 0,
            follow_range: 35.0,
            random: LegacyRandomSource::new(seed),
            navigation: Navigation::new(),
        }
    }

    pub fn block_pos(&self) -> BlockPos {
        BlockPos::new(
            self.pos.x().floor() as i32,
            self.pos.y().floor() as i32,
            self.pos.z().floor() as i32,
        )
    }

    pub fn eye_pos(&self) -> Vec3 {
        self.pos
            .with_y(self.pos.y() + f64::from(self.height) * 0.85)
    }

    pub fn actor_view(&self) -> ActorView {
        ActorView {
            id: self.id,
            kind: self.kind,
            pos: self.pos,
            eye_height: f64::from(self.height) * 0.85,
            health: self.health,
            alive: self.alive,
            creative: false,
            spectator: false,
        }
    }

    /// Refresh the environment from two resolved block states. Live sessions
    /// use this with the real collider; the injected [`WorldView`] variant
    /// remains the fallback for tests and unloaded chunks.
    pub fn update_environment_from_blocks(
        &mut self,
        feet: Option<&BlockState>,
        below: Option<&BlockState>,
    ) {
        self.in_water = feet.is_some_and(is_water);
        self.in_lava = feet.is_some_and(is_lava);
        self.fluid_height = if self.in_water { 1.0 } else { 0.0 };
        self.on_ground = below.is_some_and(block_behaviour::has_collision);
    }

    pub fn update_environment(&mut self, world: &dyn WorldView) {
        let feet = self.block_pos();
        let below = feet.below();
        self.in_water = world.block(feet).is_some_and(is_water);
        self.in_lava = world.block(feet).is_some_and(is_lava);
        self.fluid_height = if self.in_water { 1.0 } else { 0.0 };
        self.on_ground = world
            .block(below)
            .is_some_and(block_behaviour::has_collision);
    }
}

/// Selector set and controls owned by one mob.
pub struct AiAgent {
    pub mob: AiMob,
    pub goal_selector: GoalSelector,
    pub target_selector: GoalSelector,
    pub look: LookControl,
}

impl AiAgent {
    pub fn new(mob: AiMob) -> Self {
        Self {
            mob,
            goal_selector: GoalSelector::new(),
            target_selector: GoalSelector::new(),
            look: LookControl::new(),
        }
    }

    /// Zombie goal subset: float, melee, stroll, look, hurt-by-target.
    pub fn zombie(id: EntityId, pos: Vec3, seed: i64) -> Self {
        let mut agent = Self::new(AiMob::new(id, ActorKind::Zombie, pos, seed));
        agent.goal_selector.add_goal(0, Box::new(FloatGoal::new()));
        agent
            .goal_selector
            .add_goal(2, Box::new(MeleeAttackGoal::new(1.0, false)));
        agent
            .goal_selector
            .add_goal(7, Box::new(RandomStrollGoal::new(1.0)));
        agent
            .goal_selector
            .add_goal(8, Box::new(LookAtPlayerGoal::new(8.0)));
        agent
            .target_selector
            .add_goal(1, Box::new(HurtByTargetGoal::new()));
        agent
    }

    pub fn tick(
        &mut self,
        world: &dyn WorldView,
        actors: &[ActorView],
        game_time: i64,
        events: &mut Vec<AiEvent>,
    ) {
        self.tick_with_environment(world, None, actors, game_time, events);
    }

    /// Tick goals/navigation with an optional real-block environment override.
    /// Live sessions pass the collider's feet/below states so water, lava, and
    /// ground checks are not limited to the flat memory world.
    pub fn tick_with_environment(
        &mut self,
        world: &dyn WorldView,
        environment: Option<(Option<&BlockState>, Option<&BlockState>)>,
        actors: &[ActorView],
        game_time: i64,
        events: &mut Vec<AiEvent>,
    ) {
        let Self {
            mob,
            goal_selector,
            target_selector,
            look,
        } = self;
        match environment {
            Some((feet, below)) => mob.update_environment_from_blocks(feet, below),
            None => mob.update_environment(world),
        }

        {
            let mut ctx = GoalContext {
                mob,
                world,
                actors,
                look,
                events,
                game_time,
            };
            goal_selector.tick(&mut ctx);
            target_selector.tick(&mut ctx);
        }

        let AiMob {
            navigation, pos, ..
        } = mob;
        navigation.tick_with_world(world, pos);
        look.tick();
    }
}

/// Small AI tick driver over an injected world.
pub struct AiDriver {
    pub agents: Vec<AiAgent>,
    pub actors: Vec<ActorView>,
    pub game_time: i64,
}

impl Default for AiDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AiDriver {
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            actors: Vec::new(),
            game_time: 0,
        }
    }

    pub fn add_agent(&mut self, agent: AiAgent) -> EntityId {
        let id = agent.mob.id;
        self.agents.push(agent);
        id
    }

    pub fn add_zombie(&mut self, id: EntityId, pos: Vec3, seed: i64) -> EntityId {
        self.add_agent(AiAgent::zombie(id, pos, seed))
    }

    pub fn add_player(&mut self, player: ActorView) {
        self.actors.push(player);
    }

    pub fn agent(&self, id: EntityId) -> Option<&AiAgent> {
        self.agents.iter().find(|agent| agent.mob.id == id)
    }

    pub fn agent_mut(&mut self, id: EntityId) -> Option<&mut AiAgent> {
        self.agents.iter_mut().find(|agent| agent.mob.id == id)
    }

    /// Ticks every agent and applies melee damage events.
    pub fn tick(&mut self, world: &dyn WorldView) -> Vec<AiEvent> {
        self.game_time += 1;
        let mut snapshots = self.actors.clone();
        snapshots.extend(self.agents.iter().map(|agent| agent.mob.actor_view()));

        let mut events = Vec::new();
        for agent in &mut self.agents {
            agent.tick(world, &snapshots, self.game_time, &mut events);
        }

        for event in &events {
            let AiEvent::Attack { target, damage, .. } = event;
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.mob.id == *target) {
                agent.mob.health = (agent.mob.health - damage).max(0.0);
                if agent.mob.health <= 0.0 {
                    agent.mob.alive = false;
                }
            }
            if let Some(actor) = self.actors.iter_mut().find(|actor| actor.id == *target) {
                actor.health = (actor.health - damage).max(0.0);
                if actor.health <= 0.0 {
                    actor.alive = false;
                }
            }
        }

        events
    }
}

fn reduced_tick_delay(reduction: i32) -> i32 {
    (reduction.max(1) + 1) / 2
}

fn adjusted_tick_delay(reduction: i32, every_tick: bool) -> i32 {
    if every_tick {
        reduction
    } else {
        reduced_tick_delay(reduction)
    }
}

fn within_melee(mob: Vec3, target: Vec3) -> bool {
    let dx = mob.x() - target.x();
    let dz = mob.z() - target.z();
    dx.hypot(dz) <= 2.0 && (mob.y() - target.y()).abs() <= 1.5
}

fn has_line_of_sight(world: &dyn WorldView, from: Vec3, to: Vec3) -> bool {
    let delta = to.subtract(from);
    let distance = delta.length();
    if distance <= 1.0e-5 {
        return true;
    }
    let steps = ((distance * 4.0).ceil() as i32).max(1);
    for step in 1..steps {
        let t = f64::from(step) / f64::from(steps);
        let point = from.add(delta.scale(t));
        let pos = BlockPos::new(
            point.x().floor() as i32,
            point.y().floor() as i32,
            point.z().floor() as i32,
        );
        let Some(state) = world.block(pos) else {
            return false;
        };
        if block_behaviour::occludes(state) {
            return false;
        }
    }
    true
}

/// `FloatGoal`: jumps out of water and lava.
pub struct FloatGoal {
    threshold: f64,
}

impl FloatGoal {
    pub fn new() -> Self {
        Self { threshold: 0.4 }
    }

    pub fn with_threshold(threshold: f64) -> Self {
        Self { threshold }
    }
}

impl Default for FloatGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for FloatGoal {
    fn name(&self) -> &'static str {
        "FloatGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::JUMP
    }

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        (ctx.mob.in_water && ctx.mob.fluid_height > self.threshold) || ctx.mob.in_lava
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, ctx: &mut GoalContext<'_>) {
        if ctx.mob.random.next_float() < 0.8 {
            ctx.mob.jump_requested = true;
            ctx.mob.jump_requests = ctx.mob.jump_requests.saturating_add(1);
        }
    }
}

/// `RandomStrollGoal`: picks a deterministic random nearby walk target.
pub struct RandomStrollGoal {
    speed: f64,
    interval: i32,
    force_trigger: bool,
    check_no_action_time: bool,
    wanted: Option<BlockPos>,
}

impl RandomStrollGoal {
    pub fn new(speed: f64) -> Self {
        Self::with_interval(speed, 120)
    }

    pub fn with_interval(speed: f64, interval: i32) -> Self {
        Self {
            speed,
            interval: interval.max(1),
            force_trigger: false,
            check_no_action_time: true,
            wanted: None,
        }
    }

    pub fn without_no_action_check(mut self) -> Self {
        self.check_no_action_time = false;
        self
    }

    pub fn trigger(&mut self) {
        self.force_trigger = true;
    }

    pub fn set_interval(&mut self, interval: i32) {
        self.interval = interval.max(1);
    }

    fn find_position(&self, ctx: &mut GoalContext<'_>) -> Option<BlockPos> {
        let origin = ctx.mob.block_pos();
        let options = *ctx.mob.navigation.options();
        for _ in 0..10 {
            let dx = ctx.mob.random.next_int_bound(21) - 10;
            let dz = ctx.mob.random.next_int_bound(21) - 10;
            let dy = ctx.mob.random.next_int_bound(15) - 7;
            let candidate = origin.offset(dx, dy, dz);
            if is_standable(ctx.world, candidate, &options) {
                return Some(candidate);
            }
        }
        None
    }
}

impl Goal for RandomStrollGoal {
    fn name(&self) -> &'static str {
        "RandomStrollGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::MOVE
    }

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        if !self.force_trigger {
            if self.check_no_action_time && ctx.mob.no_action_time >= 100 {
                return false;
            }
            if ctx
                .mob
                .random
                .next_int_bound(reduced_tick_delay(self.interval))
                != 0
            {
                return false;
            }
        }

        let Some(position) = self.find_position(ctx) else {
            return false;
        };
        self.wanted = Some(position);
        self.force_trigger = false;
        true
    }

    fn can_continue_to_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        !ctx.mob.navigation.is_done()
    }

    fn start(&mut self, ctx: &mut GoalContext<'_>) {
        if let Some(target) = self.wanted {
            ctx.mob.navigation.move_to_with_speed(
                ctx.world,
                ctx.mob.block_pos(),
                target,
                self.speed,
            );
        }
    }

    fn stop(&mut self, ctx: &mut GoalContext<'_>) {
        ctx.mob.navigation.stop();
    }
}

/// `LookAtPlayerGoal`: looks at the current target or the nearest player.
pub struct LookAtPlayerGoal {
    look_distance: f32,
    probability: f32,
    look_time: i32,
    look_at: Option<EntityId>,
}

impl LookAtPlayerGoal {
    pub fn new(look_distance: f32) -> Self {
        Self::with_probability(look_distance, 0.02)
    }

    pub fn with_probability(look_distance: f32, probability: f32) -> Self {
        Self {
            look_distance: look_distance.max(0.0),
            probability,
            look_time: 0,
            look_at: None,
        }
    }
}

impl Goal for LookAtPlayerGoal {
    fn name(&self) -> &'static str {
        "LookAtPlayerGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::LOOK
    }

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        if ctx.mob.random.next_float() >= self.probability {
            return false;
        }

        let radius = f64::from(self.look_distance);
        let candidate = ctx
            .nearest_player(radius)
            .map(|actor| actor.id)
            .or_else(|| {
                ctx.mob
                    .target
                    .and_then(|id| ctx.actor(id))
                    .filter(|actor| {
                        actor.alive && actor.can_be_targeted() && actor.kind == ActorKind::Player
                    })
                    .map(|actor| actor.id)
            });
        self.look_at = candidate;
        candidate.is_some()
    }

    fn can_continue_to_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        let Some(id) = self.look_at else {
            return false;
        };
        let Some(actor) = ctx.actor(id) else {
            return false;
        };
        let range = f64::from(self.look_distance);
        actor.alive && self.look_time > 0 && actor.pos.distance_to_sqr(ctx.mob.pos) <= range * range
    }

    fn start(&mut self, ctx: &mut GoalContext<'_>) {
        self.look_time = adjusted_tick_delay(40 + ctx.mob.random.next_int_bound(40), false);
    }

    fn stop(&mut self, _ctx: &mut GoalContext<'_>) {
        self.look_at = None;
    }

    fn tick(&mut self, ctx: &mut GoalContext<'_>) {
        let Some(id) = self.look_at else {
            return;
        };
        let Some(actor) = ctx.actor(id).cloned() else {
            return;
        };
        if actor.alive {
            ctx.look.set_look_at_actor(&actor);
            self.look_time -= 1;
        }
    }
}

/// `MeleeAttackGoal`: paths to the current target and attacks in range.
pub struct MeleeAttackGoal {
    speed: f64,
    following_target_even_if_not_seen: bool,
    path: Option<Path>,
    pathed_target: Option<Vec3>,
    ticks_until_next_path_recalculation: i32,
    ticks_until_next_attack: i32,
    last_can_use_check: i64,
}

impl MeleeAttackGoal {
    pub fn new(speed: f64, following_target_even_if_not_seen: bool) -> Self {
        Self {
            speed,
            following_target_even_if_not_seen,
            path: None,
            pathed_target: None,
            ticks_until_next_path_recalculation: 0,
            ticks_until_next_attack: 0,
            last_can_use_check: i64::MIN / 2,
        }
    }

    pub fn ticks_until_next_attack(&self) -> i32 {
        self.ticks_until_next_attack
    }
}

impl Goal for MeleeAttackGoal {
    fn name(&self) -> &'static str {
        "MeleeAttackGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::MOVE | GoalFlags::LOOK
    }

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        if ctx.game_time - self.last_can_use_check < 20 {
            return false;
        }
        self.last_can_use_check = ctx.game_time;

        let Some(target_id) = ctx.mob.target else {
            return false;
        };
        let Some(target) = ctx.actor(target_id) else {
            return false;
        };
        if !target.alive || !target.can_be_targeted() {
            return false;
        }
        let target_pos = target.pos;
        let target_block = BlockPos::new(
            target_pos.x().floor() as i32,
            target_pos.y().floor() as i32,
            target_pos.z().floor() as i32,
        );
        let options = *ctx.mob.navigation.options();
        self.path = find_path(ctx.world, ctx.mob.block_pos(), target_block, &options).ok();
        self.path.is_some() || within_melee(ctx.mob.pos, target_pos)
    }

    fn can_continue_to_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        let Some(target_id) = ctx.mob.target else {
            return false;
        };
        let Some(target) = ctx.actor(target_id) else {
            return false;
        };
        if !target.alive || target.spectator || target.creative {
            return false;
        }
        if self.following_target_even_if_not_seen {
            true
        } else {
            !ctx.mob.navigation.is_done() || within_melee(ctx.mob.pos, target.pos)
        }
    }

    fn start(&mut self, ctx: &mut GoalContext<'_>) {
        if let Some(path) = self.path.take() {
            ctx.mob.navigation.move_to_path(path, self.speed);
        }
        ctx.mob.aggressive = true;
        self.ticks_until_next_path_recalculation = 0;
        self.ticks_until_next_attack = 0;
    }

    fn stop(&mut self, ctx: &mut GoalContext<'_>) {
        ctx.mob.target = None;
        ctx.mob.aggressive = false;
        ctx.mob.navigation.stop();
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, ctx: &mut GoalContext<'_>) {
        let Some(target_id) = ctx.mob.target else {
            return;
        };
        let Some(target) = ctx.actor(target_id).cloned() else {
            return;
        };
        if !target.alive {
            return;
        }

        ctx.look.set_look_at_actor(&target);
        self.ticks_until_next_path_recalculation =
            (self.ticks_until_next_path_recalculation - 1).max(0);

        let target_moved = self.pathed_target.is_none_or(|pathed| {
            target.pos.distance_to_sqr(pathed) >= 1.0 || ctx.mob.random.next_float() < 0.05
        });
        let sees_target = has_line_of_sight(ctx.world, ctx.mob.eye_pos(), target.eye_pos());
        if (self.following_target_even_if_not_seen || sees_target)
            && self.ticks_until_next_path_recalculation <= 0
            && target_moved
        {
            self.pathed_target = Some(target.pos);
            self.ticks_until_next_path_recalculation =
                adjusted_tick_delay(4 + ctx.mob.random.next_int_bound(7), true);
            let target_block = BlockPos::new(
                target.pos.x().floor() as i32,
                target.pos.y().floor() as i32,
                target.pos.z().floor() as i32,
            );
            if !ctx.mob.navigation.move_to_with_speed(
                ctx.world,
                ctx.mob.block_pos(),
                target_block,
                self.speed,
            ) {
                self.ticks_until_next_path_recalculation += 15;
            }
            self.ticks_until_next_path_recalculation =
                adjusted_tick_delay(self.ticks_until_next_path_recalculation, true);
        }

        self.ticks_until_next_attack = (self.ticks_until_next_attack - 1).max(0);
        if self.ticks_until_next_attack <= 0 && within_melee(ctx.mob.pos, target.pos) && sees_target
        {
            self.ticks_until_next_attack = adjusted_tick_delay(20, true);
            ctx.mob.attack_count = ctx.mob.attack_count.saturating_add(1);
            ctx.events.push(AiEvent::Attack {
                attacker: ctx.mob.id,
                target: target.id,
                damage: 3.0,
            });
        }
    }
}

/// `HurtByTargetGoal`: retargets the last attacker when its timestamp changes.
pub struct HurtByTargetGoal {
    timestamp: i64,
    follow_range: f64,
}

impl HurtByTargetGoal {
    pub fn new() -> Self {
        Self {
            timestamp: i64::MIN,
            follow_range: 35.0,
        }
    }

    pub fn with_follow_range(follow_range: f64) -> Self {
        Self {
            timestamp: i64::MIN,
            follow_range,
        }
    }
}

impl Default for HurtByTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for HurtByTargetGoal {
    fn name(&self) -> &'static str {
        "HurtByTargetGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::TARGET
    }

    fn can_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        let timestamp = ctx.mob.last_hurt_by_timestamp;
        if timestamp == self.timestamp {
            return false;
        }
        let Some(attacker) = ctx.mob.last_hurt_by else {
            return false;
        };
        ctx.actor(attacker)
            .is_some_and(|actor| actor.can_be_targeted())
    }

    fn can_continue_to_use(&mut self, ctx: &mut GoalContext<'_>) -> bool {
        let Some(target) = ctx.mob.target else {
            return false;
        };
        let Some(actor) = ctx.actor(target) else {
            return false;
        };
        actor.can_be_targeted()
            && actor.pos.distance_to_sqr(ctx.mob.pos) <= self.follow_range * self.follow_range
    }

    fn start(&mut self, ctx: &mut GoalContext<'_>) {
        ctx.mob.target = ctx.mob.last_hurt_by;
        self.timestamp = ctx.mob.last_hurt_by_timestamp;
    }

    fn stop(&mut self, ctx: &mut GoalContext<'_>) {
        if let Some(target) = ctx.mob.target
            && let Some(actor) = ctx.actor(target)
            && (actor.creative || actor.spectator)
        {
            return;
        }
        ctx.mob.target = None;
    }
}

/// In-memory [`WorldView`] used by tests and small scenarios.
///
/// Blocks outside the explicit map are air. Y outside `min_y..max_y` is
/// unloaded, which lets pathfinding fail explicitly instead of walking out of
/// the world.
#[derive(Clone, Debug)]
pub struct MemoryWorld {
    blocks: HashMap<BlockPos, BlockState>,
    sky_light: HashMap<BlockPos, u8>,
    block_light: HashMap<BlockPos, u8>,
    biomes: HashMap<BlockPos, String>,
    surfaces: HashMap<(i32, i32), i32>,
    default_biome: Option<String>,
    default_sky_light: Option<u8>,
    default_block_light: Option<u8>,
    air: BlockState,
    min_y: i32,
    max_y: i32,
}

impl MemoryWorld {
    pub fn new(min_y: i32, max_y: i32) -> Self {
        Self {
            blocks: HashMap::new(),
            sky_light: HashMap::new(),
            block_light: HashMap::new(),
            biomes: HashMap::new(),
            surfaces: HashMap::new(),
            default_biome: None,
            default_sky_light: None,
            default_block_light: None,
            air: BlockState {
                name: "minecraft:air".into(),
                properties: Vec::new(),
            },
            min_y,
            max_y,
        }
    }

    /// A square of ground at `ground_y` centered on the origin.
    pub fn flat(min_y: i32, max_y: i32, ground_y: i32, radius: i32) -> Self {
        let mut world = Self::new(min_y, max_y);
        let ground = BlockState {
            name: "minecraft:grass_block".into(),
            properties: vec![("snowy".into(), "false".into())],
        };
        for x in -radius..=radius {
            for z in -radius..=radius {
                world.set_block(x, ground_y, z, ground.clone());
            }
        }
        world
    }

    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert(BlockPos::new(x, y, z), state);
    }

    pub fn set_block_name(&mut self, x: i32, y: i32, z: i32, name: &str) {
        self.set_block(
            x,
            y,
            z,
            BlockState {
                name: name.to_owned(),
                properties: Vec::new(),
            },
        );
    }

    pub fn fill(&mut self, min: BlockPos, max: BlockPos, state: &BlockState) {
        for x in min.x()..=max.x() {
            for y in min.y()..=max.y() {
                for z in min.z()..=max.z() {
                    self.set_block(x, y, z, state.clone());
                }
            }
        }
    }

    pub fn set_sky_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
        self.sky_light.insert(BlockPos::new(x, y, z), level.min(15));
    }

    pub fn set_block_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
        self.block_light
            .insert(BlockPos::new(x, y, z), level.min(15));
    }

    pub fn set_biome(&mut self, x: i32, y: i32, z: i32, biome: &str) {
        self.biomes.insert(BlockPos::new(x, y, z), biome.to_owned());
    }

    pub fn set_default_biome(&mut self, biome: &str) {
        self.default_biome = Some(biome.to_owned());
    }

    pub fn set_default_sky_light(&mut self, level: u8) {
        self.default_sky_light = Some(level.min(15));
    }

    pub fn set_default_block_light(&mut self, level: u8) {
        self.default_block_light = Some(level.min(15));
    }

    pub fn set_surface(&mut self, x: i32, z: i32, y: i32) {
        self.surfaces.insert((x, z), y);
    }
}

impl WorldView for MemoryWorld {
    fn block(&self, pos: BlockPos) -> Option<&BlockState> {
        if pos.y() < self.min_y || pos.y() >= self.max_y {
            return None;
        }
        Some(self.blocks.get(&pos).unwrap_or(&self.air))
    }

    fn sky_light(&self, pos: BlockPos) -> Option<u8> {
        if pos.y() < self.min_y || pos.y() >= self.max_y {
            return None;
        }
        self.sky_light.get(&pos).copied().or(self.default_sky_light)
    }

    fn block_light(&self, pos: BlockPos) -> Option<u8> {
        if pos.y() < self.min_y || pos.y() >= self.max_y {
            return None;
        }
        self.block_light
            .get(&pos)
            .copied()
            .or(self.default_block_light)
    }

    fn biome(&self, pos: BlockPos) -> Option<&str> {
        if pos.y() < self.min_y || pos.y() >= self.max_y {
            return None;
        }
        self.biomes
            .get(&pos)
            .map(String::as_str)
            .or(self.default_biome.as_deref())
    }

    fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        if let Some(y) = self.surfaces.get(&(x, z)) {
            return Some(*y);
        }
        for y in (self.min_y..self.max_y).rev() {
            let state = self
                .blocks
                .get(&BlockPos::new(x, y, z))
                .unwrap_or(&self.air);
            if !is_air(state) {
                return Some(y + 1);
            }
        }
        Some(self.min_y)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn max_y(&self) -> i32 {
        self.max_y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_flag_bits_match_vanilla_order() {
        assert_eq!(GoalFlags::UNKNOWN_BEHAVIOR.bits(), 1);
        assert_eq!(GoalFlags::MOVE.bits(), 2);
        assert_eq!(GoalFlags::LOOK.bits(), 4);
        assert_eq!(GoalFlags::JUMP.bits(), 8);
        assert_eq!(GoalFlags::TARGET.bits(), 16);
        assert!(
            GoalFlags::MOVE
                .union(GoalFlags::LOOK)
                .contains(GoalFlags::MOVE)
        );
        assert!(
            GoalFlags::MOVE
                .union(GoalFlags::LOOK)
                .intersects(GoalFlags::LOOK)
        );
        assert!(!GoalFlags::MOVE.intersects(GoalFlags::LOOK));
    }

    #[test]
    fn flat_memory_world_reports_the_first_empty_y() {
        let world = MemoryWorld::flat(-64, 320, 63, 2);
        assert_eq!(world.surface_y(0, 0), Some(64));
        assert_eq!(
            world
                .block(BlockPos::new(0, 63, 0))
                .map(|state| state.name.as_str()),
            Some("minecraft:grass_block")
        );
        assert!(is_air(world.block(BlockPos::new(0, 64, 0)).unwrap()));
        assert!(world.block(BlockPos::new(0, 320, 0)).is_none());
    }

    #[test]
    fn environment_from_blocks_uses_water_lava_and_collision() {
        let state = |name: &str| BlockState {
            name: name.to_owned(),
            properties: Vec::new(),
        };
        let water = state("minecraft:water");
        let lava = state("minecraft:lava");
        let stone = state("minecraft:stone");
        let mut mob = AiMob::new(1, ActorKind::Zombie, Vec3::new(0.0, 0.0, 0.0), 1);

        mob.update_environment_from_blocks(Some(&water), Some(&stone));
        assert!(mob.in_water);
        assert!(!mob.in_lava);
        assert!(mob.on_ground);
        assert_eq!(mob.fluid_height, 1.0);

        mob.update_environment_from_blocks(Some(&lava), None);
        assert!(!mob.in_water);
        assert!(mob.in_lava);
        assert!(!mob.on_ground);
        assert_eq!(mob.fluid_height, 0.0);
    }
}
