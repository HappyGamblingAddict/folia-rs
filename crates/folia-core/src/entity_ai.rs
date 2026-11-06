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
}

impl fmt::Display for PathFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStart => formatter.write_str("path start is not walkable"),
            Self::InvalidTarget => formatter.write_str("path target is not walkable"),
            Self::Unreachable => formatter.write_str("path target is unreachable"),
            Self::TooLarge => formatter.write_str("path search exceeded its node budget"),
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

/// Bounds and mobility for one path search.
#[derive(Clone, Copy, Debug)]
pub struct PathOptions {
    pub max_nodes: usize,
    pub max_range: f64,
    pub max_fall_distance: i32,
    pub allow_water: bool,
}

impl Default for PathOptions {
    fn default() -> Self {
        Self {
            max_nodes: 4096,
            max_range: 64.0,
            max_fall_distance: 3,
            allow_water: false,
        }
    }
}

/// Returns true when a mob can stand with its feet at `pos`.
pub fn is_standable(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> bool {
    let (Some(feet), Some(head), Some(below)) = (
        world.block(pos),
        world.block(pos.above()),
        world.block(pos.below()),
    ) else {
        return false;
    };

    if block_behaviour::has_collision(feet) || block_behaviour::has_collision(head) {
        return false;
    }
    if !block_behaviour::has_collision(below) {
        return false;
    }
    if is_liquid(feet) || is_liquid(head) {
        return false;
    }
    if is_lava(below) {
        return false;
    }
    if !options.allow_water && is_water(below) {
        return false;
    }
    true
}

fn resolve_start(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> Option<BlockPos> {
    for dy in [0, -1, 1, -2, 2, -3, 3] {
        let candidate = pos.offset(0, dy, 0);
        if is_standable(world, candidate, options) {
            return Some(candidate);
        }
    }
    None
}

fn resolve_target(world: &dyn WorldView, pos: BlockPos, options: &PathOptions) -> Option<BlockPos> {
    for dy in [0, 1, -1, 2, -2] {
        let candidate = pos.offset(0, dy, 0);
        if is_standable(world, candidate, options) {
            return Some(candidate);
        }
    }
    None
}

fn heuristic(from: BlockPos, to: BlockPos) -> f64 {
    let dx = (from.x() - to.x()).unsigned_abs();
    let dy = (from.y() - to.y()).unsigned_abs();
    let dz = (from.z() - to.z()).unsigned_abs();
    f64::from(dx + dy + dz)
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

fn path_neighbors(
    world: &dyn WorldView,
    from: BlockPos,
    options: &PathOptions,
) -> Vec<(BlockPos, f64)> {
    const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    let mut result = Vec::with_capacity(8);

    for (dx, dz) in DIRECTIONS {
        let level = from.offset(dx, 0, dz);
        if is_standable(world, level, options) {
            result.push((level, 1.0));
        }

        let up = from.offset(dx, 1, dz);
        if is_standable(world, up, options) {
            result.push((up, 1.5));
        }

        for drop in 1..=options.max_fall_distance {
            let down = from.offset(dx, -drop, dz);
            if !is_standable(world, down, options) {
                continue;
            }
            if !clear_vertical(world, down.x(), down.z(), down.y(), from.y() + 1) {
                continue;
            }
            result.push((down, 1.0 + f64::from(drop) * 0.25));
            break;
        }
    }

    result
}

/// Deterministic bounded A* over block collision metadata.
///
/// The search walks four horizontal directions, steps up exactly one block,
/// and drops at most [`PathOptions::max_fall_distance`] blocks. Liquids and
/// lava are rejected. Ties are broken by `(x, y, z)` so the same world and
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
    if (dx * dx + dy * dy + dz * dz).sqrt() > options.max_range {
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
    let mut parents: HashMap<BlockPos, BlockPos> = HashMap::new();
    let mut closed: HashSet<BlockPos> = HashSet::new();
    let mut expansions = 0usize;

    while !open.is_empty() {
        let mut best_index = 0usize;
        let mut best_score = f64::INFINITY;
        for (index, candidate) in open.iter().enumerate() {
            let cost = best_cost.get(candidate).copied().unwrap_or(f64::INFINITY);
            let score = cost + heuristic(*candidate, target);
            let better = score < best_score
                || (score == best_score && node_key(*candidate) < node_key(open[best_index]));
            if better {
                best_index = index;
                best_score = score;
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
        for (neighbor, step_cost) in path_neighbors(world, current, options) {
            if closed.contains(&neighbor) {
                continue;
            }
            let candidate_cost = current_cost + step_cost;
            let previous = best_cost.get(&neighbor).copied().unwrap_or(f64::INFINITY);
            if candidate_cost + 1.0e-9 < previous {
                best_cost.insert(neighbor, candidate_cost);
                parents.insert(neighbor, current);
                if !open.contains(&neighbor) {
                    open.push(neighbor);
                }
            }
        }
    }

    Err(PathFailure::Unreachable)
}

/// Port of the movement half of `PathNavigation`.
#[derive(Clone, Debug)]
pub struct Navigation {
    path: Option<Path>,
    index: usize,
    speed: f64,
    done: bool,
    options: PathOptions,
    last_failure: Option<PathFailure>,
    failures: u32,
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
        self.path = Some(path);
        true
    }

    /// Advances the owner along the current path by one tick.
    pub fn tick(&mut self, pos: &mut Vec3) -> bool {
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
        let Self {
            mob,
            goal_selector,
            target_selector,
            look,
        } = self;
        mob.update_environment(world);

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
        navigation.tick(pos);
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
}
