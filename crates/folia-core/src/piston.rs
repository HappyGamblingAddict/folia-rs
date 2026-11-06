//! Piston movement, sticky/slime/honey interaction, and block updates.
//!
//! The reference implementation is Folia 1.21.11:
//!
//! * `net/minecraft/world/level/block/piston/PistonBaseBlock`
//! * `PistonHeadBlock`, `MovingPistonBlock`, `PistonMovingBlockEntity`
//! * `PistonStructureResolver`
//!
//! This slice ports the block-level behaviour: extension/retraction, the
//! 12-block structure resolver, sticky chains (slime/honey), immovable blocks,
//! plant/fluid destruction, and the two-tick moving-block payload. Moving
//! blocks are represented explicitly in [`MovingPiston`] because
//! [`crate::block_entity`] has no typed moving-piston payload yet; the payload
//! carries the moved state, facing, extending flag, and progress so block
//! entities can be threaded through unchanged once that API exists.
//!
//! All world mutations go through [`TickWorld::set_block`], so shape and
//! neighbour updates stay on the existing tick queue and are drained by the
//! enclosing tick.

use crate::block_behaviour;
use crate::block_shapes;
use crate::chunk::{BlockState, is_air_state};
use crate::math::{AABB, BlockPos, Direction};
use crate::redstone::SignalReader;
use crate::registry;
use crate::tick::{BlockBehavior, TickWorld, UpdateFlags};
use crate::world::WorldStorage;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// `minecraft:piston`.
pub const PISTON: &str = "minecraft:piston";
/// `minecraft:sticky_piston`.
pub const STICKY_PISTON: &str = "minecraft:sticky_piston";
/// `minecraft:piston_head`.
pub const PISTON_HEAD: &str = "minecraft:piston_head";
/// `minecraft:moving_piston`.
pub const MOVING_PISTON: &str = "minecraft:moving_piston";
/// `minecraft:slime_block`.
pub const SLIME_BLOCK: &str = "minecraft:slime_block";
/// `minecraft:honey_block`.
pub const HONEY_BLOCK: &str = "minecraft:honey_block";

/// `PistonStructureResolver.MAX_PUSH_DEPTH`.
pub const MAX_PUSH_DEPTH: usize = 12;

fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    }
}

fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn bool_property(state: &BlockState, key: &str) -> bool {
    property(state, key) == Some("true")
}

fn bool_value(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn with_property(mut state: BlockState, key: &str, value: &str) -> BlockState {
    if let Some(entry) = state.properties.iter_mut().find(|(name, _)| name == key) {
        entry.1 = value.to_owned();
    } else {
        state.properties.push((key.to_owned(), value.to_owned()));
    }
    state
}

/// The `facing` property name for a direction.
pub fn direction_property(direction: Direction) -> &'static str {
    match direction {
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
    }
}

/// Parses a `facing` property, defaulting to north like the block state.
pub fn direction_from_property(value: &str) -> Direction {
    match value {
        "down" => Direction::Down,
        "up" => Direction::Up,
        "south" => Direction::South,
        "west" => Direction::West,
        "east" => Direction::East,
        _ => Direction::North,
    }
}

/// Whether a state is a regular or sticky piston base.
pub fn is_piston(state: &BlockState) -> bool {
    matches!(state.name.as_str(), PISTON | STICKY_PISTON)
}

/// Whether a state is the extended head block.
pub fn is_piston_head(state: &BlockState) -> bool {
    state.name == PISTON_HEAD
}

/// Whether a state is the transient moving-piston block.
pub fn is_moving_piston(state: &BlockState) -> bool {
    state.name == MOVING_PISTON
}

/// The piston base facing (`DirectionalBlock.FACING`).
pub fn piston_facing(state: &BlockState) -> Direction {
    property(state, "facing").map_or(Direction::North, direction_from_property)
}

/// Whether a piston base is extended.
pub fn piston_extended(state: &BlockState) -> bool {
    bool_property(state, "extended")
}

/// Whether a piston base block is the sticky variant.
pub fn piston_is_sticky(state: &BlockState) -> bool {
    state.name == STICKY_PISTON
}

/// The `type` property (`normal`/`sticky`) of a head or moving piston.
pub fn piston_type(state: &BlockState) -> &str {
    property(state, "type").unwrap_or("normal")
}

/// Whether a head or moving piston carries the sticky type.
pub fn piston_type_is_sticky(state: &BlockState) -> bool {
    piston_type(state) == "sticky"
}

fn type_name(sticky: bool) -> &'static str {
    if sticky { "sticky" } else { "normal" }
}

fn default_state(name: &str) -> BlockState {
    registry::default_block_state(name).unwrap_or_else(|| BlockState {
        name: name.into(),
        properties: Vec::new(),
    })
}

/// Builds a piston base state with the given facing and extension flag.
pub fn piston_base_state(sticky: bool, facing: Direction, extended: bool) -> BlockState {
    let name = if sticky { STICKY_PISTON } else { PISTON };
    let state = default_state(name);
    let state = with_property(state, "facing", direction_property(facing));
    with_property(state, "extended", bool_value(extended))
}

/// Builds a piston head state.
pub fn piston_head_state(facing: Direction, sticky: bool, short: bool) -> BlockState {
    let state = default_state(PISTON_HEAD);
    let state = with_property(state, "facing", direction_property(facing));
    let state = with_property(state, "type", type_name(sticky));
    with_property(state, "short", bool_value(short))
}

/// Builds the transient moving-piston block state.
pub fn moving_piston_state(facing: Direction, sticky: bool) -> BlockState {
    let state = default_state(MOVING_PISTON);
    let state = with_property(state, "facing", direction_property(facing));
    with_property(state, "type", type_name(sticky))
}

/// The moved payload of a `minecraft:moving_piston` block.
///
/// Vanilla stores this in `PistonMovingBlockEntity`. Until
/// [`crate::block_entity`] exposes typed payloads, the tick state carries it
/// explicitly so the moved block state survives the two movement ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct MovingPiston {
    /// The block state being moved (the piston head for a source piston).
    pub moved_state: BlockState,
    /// The piston's facing direction.
    pub direction: Direction,
    /// True while extending; false while retracting.
    pub extending: bool,
    /// Vanilla `isSourcePiston`.
    pub is_source_piston: bool,
    /// Movement progress: `0` = start, `1` = half, `2` = complete.
    pub progress: u8,
}

impl MovingPiston {
    /// Vanilla `progress`: 0.0, 0.5, or 1.0.
    pub fn progress(&self) -> f32 {
        f32::from(self.progress.min(2)) * 0.5
    }

    /// `PistonMovingBlockEntity.getExtendedProgress`.
    pub fn extended_progress(&self) -> f32 {
        if self.extending {
            self.progress() - 1.0
        } else {
            1.0 - self.progress()
        }
    }

    /// `PistonMovingBlockEntity.getMovementDirection`.
    pub fn movement_direction(&self) -> Direction {
        if self.extending {
            self.direction
        } else {
            self.direction.opposite()
        }
    }

    /// The state used for collision checks.
    ///
    /// A retracting source piston exposes a short piston head while it moves,
    /// matching `getCollisionRelatedBlockState`.
    pub fn collision_state(&self) -> BlockState {
        if !self.extending && self.is_source_piston && is_piston(&self.moved_state) {
            piston_head_state(
                piston_facing(&self.moved_state),
                piston_is_sticky(&self.moved_state),
                self.progress > 0,
            )
        } else {
            self.moved_state.clone()
        }
    }
}

/// Per-world moving-piston payloads.
#[derive(Clone, Debug, Default)]
pub struct PistonState {
    moving: BTreeMap<i64, MovingPiston>,
}

impl PistonState {
    /// The payload at `pos`, if a moving piston is present.
    pub fn get(&self, pos: BlockPos) -> Option<&MovingPiston> {
        self.moving.get(&pos.as_long())
    }

    /// Inserts or replaces the payload at `pos`.
    pub fn insert(&mut self, pos: BlockPos, payload: MovingPiston) {
        self.moving.insert(pos.as_long(), payload);
    }

    /// Removes and returns the payload at `pos`.
    pub fn remove(&mut self, pos: BlockPos) -> Option<MovingPiston> {
        self.moving.remove(&pos.as_long())
    }

    /// Number of active moving pistons.
    pub fn len(&self) -> usize {
        self.moving.len()
    }

    /// True when no moving piston is active.
    pub fn is_empty(&self) -> bool {
        self.moving.is_empty()
    }

    /// Payload positions in deterministic order.
    pub fn positions(&self) -> impl Iterator<Item = BlockPos> + '_ {
        self.moving.keys().copied().map(BlockPos::from_long)
    }

    /// Payloads in deterministic position order.
    pub fn iter(&self) -> impl Iterator<Item = (BlockPos, &MovingPiston)> + '_ {
        self.moving
            .iter()
            .map(|(packed, payload)| (BlockPos::from_long(*packed), payload))
    }

    /// Removes every payload.
    pub fn clear(&mut self) {
        self.moving.clear();
    }
}

/// Read-only world view used by [`PistonStructureResolver`].
pub trait PistonWorld {
    /// The block state at `pos`, or `None` when the chunk is not loaded.
    fn piston_block_state(&self, pos: BlockPos) -> Option<BlockState>;

    /// Whether `pos` is inside the world build limits.
    fn within_build_limits(&self, pos: BlockPos) -> bool {
        let min = crate::chunk_packet::OVERWORLD_MIN_Y;
        let max = min + crate::chunk_packet::OVERWORLD_HEIGHT as i32 - 1;
        (min..=max).contains(&pos.y())
    }
}

impl PistonWorld for WorldStorage {
    fn piston_block_state(&self, pos: BlockPos) -> Option<BlockState> {
        self.block_state_at(pos.x(), pos.y(), pos.z())
    }
}

/// `PushReaction`, reduced to the block-level cases this slice needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushReaction {
    Normal,
    Destroy,
    Block,
    PushOnly,
}

fn is_slime(state: &BlockState) -> bool {
    state.name == SLIME_BLOCK
}

fn is_honey(state: &BlockState) -> bool {
    state.name == HONEY_BLOCK
}

fn is_sticky(state: &BlockState) -> bool {
    is_slime(state) || is_honey(state)
}

/// `PistonStructureResolver.canStickToEachOther`: honey never sticks to slime.
fn can_stick_to_each_other(first: &BlockState, second: &BlockState) -> bool {
    !(is_honey(first) && is_slime(second) || is_slime(first) && is_honey(second))
        && (is_sticky(first) || is_sticky(second))
}

fn immovable_block(name: &str) -> bool {
    matches!(
        name,
        "minecraft:obsidian"
            | "minecraft:crying_obsidian"
            | "minecraft:respawn_anchor"
            | "minecraft:reinforced_deepslate"
            | "minecraft:barrier"
            | "minecraft:anvil"
            | "minecraft:chipped_anvil"
            | "minecraft:damaged_anvil"
            | "minecraft:grindstone"
            | "minecraft:lodestone"
            | "minecraft:end_gateway"
            | "minecraft:end_portal"
            | "minecraft:nether_portal"
            | "minecraft:piston_head"
            | "minecraft:moving_piston"
    )
}

fn has_block_entity(name: &str) -> bool {
    name.ends_with("_sign")
        || name.ends_with("_wall_sign")
        || name.ends_with("_hanging_sign")
        || name.ends_with("_wall_hanging_sign")
        || name.ends_with("_banner")
        || name.ends_with("_wall_banner")
        || name.ends_with("_bed")
        || name.ends_with("_shulker_box")
        || name == "minecraft:shulker_box"
        || name.ends_with("_shelf")
        || matches!(
            name,
            "minecraft:barrel"
                | "minecraft:beacon"
                | "minecraft:bee_nest"
                | "minecraft:beehive"
                | "minecraft:bell"
                | "minecraft:blast_furnace"
                | "minecraft:brewing_stand"
                | "minecraft:calibrated_sculk_sensor"
                | "minecraft:campfire"
                | "minecraft:soul_campfire"
                | "minecraft:chest"
                | "minecraft:trapped_chest"
                | "minecraft:chiseled_bookshelf"
                | "minecraft:command_block"
                | "minecraft:chain_command_block"
                | "minecraft:repeating_command_block"
                | "minecraft:comparator"
                | "minecraft:conduit"
                | "minecraft:crafter"
                | "minecraft:creaking_heart"
                | "minecraft:daylight_detector"
                | "minecraft:dispenser"
                | "minecraft:dropper"
                | "minecraft:enchanting_table"
                | "minecraft:ender_chest"
                | "minecraft:furnace"
                | "minecraft:hopper"
                | "minecraft:jigsaw"
                | "minecraft:jukebox"
                | "minecraft:lectern"
                | "minecraft:sculk_catalyst"
                | "minecraft:sculk_sensor"
                | "minecraft:sculk_shrieker"
                | "minecraft:smoker"
                | "minecraft:spawner"
                | "minecraft:structure_block"
                | "minecraft:test_block"
                | "minecraft:test_instance_block"
                | "minecraft:trial_spawner"
                | "minecraft:vault"
        )
        || name.contains("copper_chest")
        || name.contains("copper_golem_statue")
}

fn is_destroy_reaction(state: &BlockState) -> bool {
    let name = state.name.as_str();
    crate::tags::block_has("replaceable", name)
        || crate::tags::block_has("fire", name)
        || crate::tags::block_has("crops", name)
        || crate::tags::block_has("flowers", name)
        || crate::tags::block_has("saplings", name)
        || crate::redstone::is_wire(state)
        || crate::redstone::is_lever(state)
        || crate::redstone::is_button(state)
        || crate::redstone::is_repeater(state)
        || crate::redstone::is_comparator(state)
        || crate::redstone::is_redstone_torch(state)
        || crate::redstone::is_pressure_plate(state)
        || crate::redstone::is_tripwire(state)
        || crate::redstone::is_tripwire_hook(state)
        || name.ends_with("_door")
        || name == "minecraft:torch"
        || name.ends_with("_torch")
        || name.ends_with("_wall_torch")
        || name.ends_with("_coral")
        || name.ends_with("_coral_fan")
        || name.ends_with("_coral_wall_fan")
        || name.ends_with("_head")
        || name.ends_with("_wall_head")
        || name.ends_with("_skull")
        || matches!(
            name,
            "minecraft:ladder"
                | "minecraft:bell"
                | "minecraft:lantern"
                | "minecraft:soul_lantern"
                | "minecraft:cactus"
                | "minecraft:cactus_flower"
                | "minecraft:cake"
                | "minecraft:carved_pumpkin"
                | "minecraft:jack_o_lantern"
                | "minecraft:cobweb"
                | "minecraft:sugar_cane"
                | "minecraft:bamboo"
                | "minecraft:kelp"
                | "minecraft:kelp_plant"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
                | "minecraft:sea_pickle"
                | "minecraft:turtle_egg"
                | "minecraft:scaffolding"
                | "minecraft:moss_block"
                | "minecraft:pale_moss_block"
                | "minecraft:moss_carpet"
                | "minecraft:pale_moss_carpet"
                | "minecraft:lily_pad"
                | "minecraft:nether_wart"
                | "minecraft:cocoa"
                | "minecraft:sweet_berry_bush"
                | "minecraft:chorus_flower"
                | "minecraft:chorus_plant"
                | "minecraft:amethyst_cluster"
                | "minecraft:budding_amethyst"
                | "minecraft:azalea"
                | "minecraft:flowering_azalea"
                | "minecraft:big_dripleaf"
                | "minecraft:big_dripleaf_stem"
                | "minecraft:small_dripleaf"
                | "minecraft:spore_blossom"
                | "minecraft:pointed_dripstone"
                | "minecraft:decorated_pot"
                | "minecraft:suspicious_sand"
                | "minecraft:suspicious_gravel"
                | "minecraft:dragon_egg"
                | "minecraft:frogspawn"
                | "minecraft:pink_petals"
                | "minecraft:leaf_litter"
                | "minecraft:wildflowers"
                | "minecraft:firefly_bush"
                | "minecraft:bush"
                | "minecraft:dead_bush"
                | "minecraft:short_dry_grass"
                | "minecraft:tall_dry_grass"
                | "minecraft:closed_eyeblossom"
                | "minecraft:open_eyeblossom"
                | "minecraft:pitcher_crop"
                | "minecraft:pitcher_plant"
                | "minecraft:torchflower"
                | "minecraft:torchflower_crop"
                | "minecraft:resin_clump"
                | "minecraft:pale_hanging_moss"
                | "minecraft:hanging_roots"
                | "minecraft:glow_lichen"
                | "minecraft:sculk_vein"
                | "minecraft:cave_vines"
                | "minecraft:cave_vines_plant"
                | "minecraft:twisting_vines"
                | "minecraft:twisting_vines_plant"
                | "minecraft:weeping_vines"
                | "minecraft:weeping_vines_plant"
                | "minecraft:cherry_leaves"
                | "minecraft:pale_oak_leaves"
                | "minecraft:structure_void"
        )
}

fn push_reaction(state: &BlockState) -> PushReaction {
    if is_air_state(state) {
        return PushReaction::Normal;
    }
    if block_behaviour::hardness(state) < 0.0 || immovable_block(state.name.as_str()) {
        return PushReaction::Block;
    }
    if is_destroy_reaction(state) {
        return PushReaction::Destroy;
    }
    if has_block_entity(state.name.as_str()) {
        return PushReaction::Block;
    }
    if state.name.ends_with("_glazed_terracotta") {
        return PushReaction::PushOnly;
    }
    PushReaction::Normal
}

/// `PistonBaseBlock.isPushable`.
fn is_pushable<W: PistonWorld + ?Sized>(
    state: &BlockState,
    world: &W,
    pos: BlockPos,
    movement_direction: Direction,
    allow_destroy: bool,
    piston_facing: Direction,
) -> bool {
    if !world.within_build_limits(pos)
        || !world.within_build_limits(pos.relative(movement_direction))
    {
        return false;
    }
    if is_air_state(state) {
        return true;
    }
    if immovable_block(state.name.as_str()) {
        return false;
    }
    let min_y = crate::chunk_packet::OVERWORLD_MIN_Y;
    let max_y = min_y + crate::chunk_packet::OVERWORLD_HEIGHT as i32 - 1;
    if movement_direction == Direction::Down && pos.y() == min_y {
        return false;
    }
    if movement_direction == Direction::Up && pos.y() == max_y {
        return false;
    }
    if !is_piston(state) {
        if block_behaviour::hardness(state) < 0.0 {
            return false;
        }
        match push_reaction(state) {
            PushReaction::Block => return false,
            PushReaction::Destroy => return allow_destroy,
            PushReaction::PushOnly => return movement_direction == piston_facing,
            PushReaction::Normal => {}
        }
    } else if piston_extended(state) {
        return false;
    }
    !has_block_entity(state.name.as_str())
}

/// One resolved piston move, captured before the world is mutated.
#[derive(Clone, Debug, PartialEq)]
pub struct MovePlan {
    /// Blocks moved by the piston, in Java resolver order.
    pub to_push: Vec<BlockPos>,
    /// Blocks destroyed by the piston, in resolver discovery order.
    pub to_destroy: Vec<BlockPos>,
    /// States parallel to `to_push`, captured before mutation.
    pub states: Vec<BlockState>,
    /// States parallel to `to_destroy`, captured before mutation.
    pub destroyed_states: Vec<BlockState>,
    /// The movement direction in vanilla's resolver (`pushDirection`).
    pub push_direction: Direction,
}

/// Port of `PistonStructureResolver`.
pub struct PistonStructureResolver<'a, W: PistonWorld + ?Sized> {
    level: &'a W,
    piston_pos: BlockPos,
    piston_direction: Direction,
    extending: bool,
    start_pos: BlockPos,
    push_direction: Direction,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
}

impl<'a, W: PistonWorld + ?Sized> PistonStructureResolver<'a, W> {
    /// Creates a resolver for one piston action.
    pub fn new(
        level: &'a W,
        piston_pos: BlockPos,
        piston_direction: Direction,
        extending: bool,
    ) -> Self {
        let (push_direction, start_pos) = if extending {
            (piston_direction, piston_pos.relative(piston_direction))
        } else {
            (
                piston_direction.opposite(),
                piston_pos.relative_n(piston_direction, 2),
            )
        };
        Self {
            level,
            piston_pos,
            piston_direction,
            extending,
            start_pos,
            push_direction,
            to_push: Vec::new(),
            to_destroy: Vec::new(),
        }
    }

    /// The movement direction (`pushDirection`).
    pub fn push_direction(&self) -> Direction {
        self.push_direction
    }

    /// The piston base position.
    pub fn piston_pos(&self) -> BlockPos {
        self.piston_pos
    }

    /// The piston facing direction.
    pub fn piston_direction(&self) -> Direction {
        self.piston_direction
    }

    /// Blocks to move.
    pub fn to_push(&self) -> &[BlockPos] {
        &self.to_push
    }

    /// Blocks to destroy.
    pub fn to_destroy(&self) -> &[BlockPos] {
        &self.to_destroy
    }

    fn block(&self, pos: BlockPos) -> BlockState {
        self.level.piston_block_state(pos).unwrap_or_else(air)
    }

    fn is_pushable(
        &self,
        state: &BlockState,
        pos: BlockPos,
        movement_direction: Direction,
        allow_destroy: bool,
        facing: Direction,
    ) -> bool {
        is_pushable(
            state,
            self.level,
            pos,
            movement_direction,
            allow_destroy,
            facing,
        )
    }

    /// Port of `PistonStructureResolver.resolve`.
    pub fn resolve(&mut self) -> bool {
        self.to_push.clear();
        self.to_destroy.clear();
        let state = self.block(self.start_pos);
        if !self.is_pushable(
            &state,
            self.start_pos,
            self.push_direction,
            false,
            self.piston_direction,
        ) {
            if self.extending && push_reaction(&state) == PushReaction::Destroy {
                self.to_destroy.push(self.start_pos);
                return true;
            }
            return false;
        }

        if !self.add_block_line(self.start_pos, self.push_direction) {
            return false;
        }

        let mut index = 0;
        while index < self.to_push.len() {
            let pos = self.to_push[index];
            if is_sticky(&self.block(pos)) && !self.add_branching_blocks(pos) {
                return false;
            }
            index += 1;
        }
        true
    }

    fn add_block_line(&mut self, origin_pos: BlockPos, direction: Direction) -> bool {
        let mut state = self.block(origin_pos);
        if is_air_state(&state) {
            return true;
        }
        if !self.is_pushable(&state, origin_pos, self.push_direction, false, direction) {
            return true;
        }
        if origin_pos == self.piston_pos || self.to_push.contains(&origin_pos) {
            return true;
        }

        let mut distance = 1usize;
        if distance + self.to_push.len() > MAX_PUSH_DEPTH {
            return false;
        }
        while is_sticky(&state) {
            let pos = origin_pos.relative_n(self.push_direction.opposite(), distance as i32);
            let previous = state;
            state = self.block(pos);
            if is_air_state(&state)
                || !can_stick_to_each_other(&previous, &state)
                || !self.is_pushable(
                    &state,
                    pos,
                    self.push_direction,
                    false,
                    self.push_direction.opposite(),
                )
                || pos == self.piston_pos
            {
                break;
            }
            distance += 1;
            if distance + self.to_push.len() > MAX_PUSH_DEPTH {
                return false;
            }
        }

        let mut offsets = 0usize;
        for delta in (0..distance).rev() {
            self.to_push
                .push(origin_pos.relative_n(self.push_direction.opposite(), delta as i32));
            offsets += 1;
        }

        let mut forward = 1usize;
        loop {
            let pos = origin_pos.relative_n(self.push_direction, forward as i32);
            if let Some(index) = self.to_push.iter().position(|candidate| *candidate == pos) {
                self.reorder_list_at_collision(offsets, index);
                for candidate in 0..=index + offsets {
                    let pos = self.to_push[candidate];
                    if is_sticky(&self.block(pos)) && !self.add_branching_blocks(pos) {
                        return false;
                    }
                }
                return true;
            }

            state = self.block(pos);
            if is_air_state(&state) {
                return true;
            }
            if !self.is_pushable(&state, pos, self.push_direction, true, self.push_direction)
                || pos == self.piston_pos
            {
                return false;
            }
            if push_reaction(&state) == PushReaction::Destroy {
                self.to_destroy.push(pos);
                return true;
            }
            if self.to_push.len() >= MAX_PUSH_DEPTH {
                return false;
            }
            self.to_push.push(pos);
            offsets += 1;
            forward += 1;
        }
    }

    fn reorder_list_at_collision(&mut self, offsets: usize, index: usize) {
        let mut first = self.to_push[..index].to_vec();
        let last = self.to_push[self.to_push.len() - offsets..].to_vec();
        let middle = self.to_push[index..self.to_push.len() - offsets].to_vec();
        self.to_push.clear();
        first.extend(last);
        first.extend(middle);
        self.to_push = first;
    }

    fn add_branching_blocks(&mut self, from_pos: BlockPos) -> bool {
        let state = self.block(from_pos);
        for direction in Direction::ALL {
            if direction.axis() != self.push_direction.axis() {
                let pos = from_pos.relative(direction);
                let neighbor = self.block(pos);
                if can_stick_to_each_other(&neighbor, &state)
                    && !self.add_block_line(pos, direction)
                {
                    return false;
                }
            }
        }
        true
    }

    /// Captures the resolved plan, reading each moved/destroyed state before
    /// the world changes.
    pub fn into_plan(self) -> MovePlan {
        let states = self.to_push.iter().map(|pos| self.block(*pos)).collect();
        let destroyed_states = self.to_destroy.iter().map(|pos| self.block(*pos)).collect();
        MovePlan {
            to_push: self.to_push,
            to_destroy: self.to_destroy,
            states,
            destroyed_states,
            push_direction: self.push_direction,
        }
    }
}

/// Runs the resolver and returns its plan without touching the world.
pub fn resolve_plan<W: PistonWorld + ?Sized>(
    world: &W,
    piston_pos: BlockPos,
    piston_direction: Direction,
    extending: bool,
) -> Option<MovePlan> {
    let mut resolver = PistonStructureResolver::new(world, piston_pos, piston_direction, extending);
    if !resolver.resolve() {
        return None;
    }
    Some(resolver.into_plan())
}

/// `PistonBaseBlock.getNeighborSignal`.
fn neighbor_signal(world: &WorldStorage, pos: BlockPos, facing: Direction) -> bool {
    let reader = SignalReader::world(world);
    for direction in Direction::ALL {
        if direction != facing && reader.has_signal(pos.relative(direction), direction) {
            return true;
        }
    }
    if reader.has_signal(pos, Direction::Down) {
        return true;
    }
    let above = pos.above();
    for direction in Direction::ALL {
        if direction != Direction::Down && reader.has_signal(above.relative(direction), direction) {
            return true;
        }
    }
    false
}

fn positions_loaded(
    world: &mut TickWorld<'_>,
    plan: &MovePlan,
    direction: Direction,
) -> io::Result<bool> {
    for &old in &plan.to_push {
        if world.block_state(old)?.is_none() {
            return Ok(false);
        }
        if world.block_state(old.relative(direction))?.is_none() {
            return Ok(false);
        }
    }
    for &pos in &plan.to_destroy {
        if world.block_state(pos)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn set_moving_block(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    moved_state: BlockState,
    direction: Direction,
    extending: bool,
    is_source_piston: bool,
    sticky: bool,
) -> io::Result<()> {
    world.set_piston_payload(
        pos,
        MovingPiston {
            moved_state,
            direction,
            extending,
            is_source_piston,
            progress: 0,
        },
    );
    world.set_block(pos, moving_piston_state(direction, sticky))?;
    Ok(())
}

fn extend(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    facing: Direction,
    sticky: bool,
) -> io::Result<bool> {
    let Some(plan) = resolve_plan(world.storage(), pos, facing, true) else {
        return Ok(false);
    };
    if !positions_loaded(world, &plan, facing)? {
        return Ok(false);
    }

    for &destroyed in plan.to_destroy.iter().rev() {
        world.set_block(destroyed, air())?;
    }

    let mut destinations = BTreeSet::new();
    for index in (0..plan.to_push.len()).rev() {
        let old = plan.to_push[index];
        let destination = old.relative(facing);
        destinations.insert(destination.as_long());
        set_moving_block(
            world,
            destination,
            plan.states[index].clone(),
            facing,
            true,
            false,
            sticky,
        )?;
    }
    for &old in plan.to_push.iter().rev() {
        if !destinations.contains(&old.as_long()) {
            world.set_block(old, air())?;
        }
    }

    let front = pos.relative(facing);
    set_moving_block(
        world,
        front,
        piston_head_state(facing, sticky, false),
        facing,
        true,
        true,
        sticky,
    )?;
    world.set_block(pos, piston_base_state(sticky, facing, true))?;
    Ok(true)
}

fn pull(
    world: &mut TickWorld<'_>,
    plan: &MovePlan,
    facing: Direction,
    sticky: bool,
) -> io::Result<bool> {
    let direction = facing.opposite();
    if !positions_loaded(world, plan, direction)? {
        return Ok(false);
    }

    let mut destinations = BTreeSet::new();
    for index in (0..plan.to_push.len()).rev() {
        let old = plan.to_push[index];
        let destination = old.relative(direction);
        destinations.insert(destination.as_long());
        set_moving_block(
            world,
            destination,
            plan.states[index].clone(),
            facing,
            false,
            false,
            sticky,
        )?;
    }
    for &old in plan.to_push.iter().rev() {
        if !destinations.contains(&old.as_long()) {
            world.set_block(old, air())?;
        }
    }
    Ok(true)
}

fn retract(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    facing: Direction,
    sticky: bool,
) -> io::Result<()> {
    let front = pos.relative(facing);
    if world
        .block_state(front)?
        .is_some_and(|state| is_piston_head(&state))
    {
        world.set_block(front, air())?;
    }

    // Java sets the base to a moving block before resolving the pull.
    set_moving_block(
        world,
        pos,
        piston_base_state(sticky, facing, false),
        facing,
        false,
        true,
        sticky,
    )?;

    if sticky && let Some(plan) = resolve_plan(world.storage(), pos, facing, false) {
        pull(world, &plan, facing, sticky)?;
    }
    Ok(())
}

/// `PistonBaseBlock.checkIfExtend`.
pub fn check_if_extend(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<()> {
    if !is_piston(state) {
        return Ok(());
    }
    let facing = piston_facing(state);
    let powered = neighbor_signal(world.storage(), pos, facing);
    let extended = piston_extended(state);
    let sticky = piston_is_sticky(state);
    if powered && !extended {
        extend(world, pos, facing, sticky)?;
    } else if !powered && extended {
        retract(world, pos, facing, sticky)?;
    }
    Ok(())
}

/// `PistonHeadBlock.isFittingBase`.
fn is_fitting_base(head: &BlockState, base: &BlockState) -> bool {
    if !is_piston(base) || !piston_extended(base) || piston_facing(base) != piston_facing(head) {
        return false;
    }
    piston_is_sticky(base) == piston_type_is_sticky(head)
}

/// `PistonHeadBlock.canSurvive`.
fn head_can_survive(
    world: &mut TickWorld<'_>,
    pos: BlockPos,
    state: &BlockState,
) -> io::Result<bool> {
    let behind = pos.relative(piston_facing(state).opposite());
    let Some(behind_state) = world.block_state(behind)? else {
        return Ok(false);
    };
    Ok(is_fitting_base(state, &behind_state)
        || (is_moving_piston(&behind_state)
            && piston_facing(&behind_state) == piston_facing(state)))
}

/// Collision boxes of a `minecraft:moving_piston`, resolved through the
/// generated `block_shapes` table and translated by the movement progress.
///
/// The returned boxes are world-space AABBs for the moved state (or the short
/// piston head while a source piston retracts).
pub fn moving_collision_boxes(world: &WorldStorage, pos: BlockPos) -> Vec<AABB> {
    let Some(payload) = world.tick_state().pistons.get(pos) else {
        return Vec::new();
    };
    let state = payload.collision_state();
    let offset = payload.extended_progress();
    let dx = f64::from(payload.direction.step_x()) * f64::from(offset);
    let dy = f64::from(payload.direction.step_y()) * f64::from(offset);
    let dz = f64::from(payload.direction.step_z()) * f64::from(offset);
    block_shapes::collision_boxes_for_state(&state)
        .iter()
        .map(|bounds| {
            AABB::new(
                bounds.min_x + f64::from(pos.x()) + dx,
                bounds.min_y + f64::from(pos.y()) + dy,
                bounds.min_z + f64::from(pos.z()) + dz,
                bounds.max_x + f64::from(pos.x()) + dx,
                bounds.max_y + f64::from(pos.y()) + dy,
                bounds.max_z + f64::from(pos.z()) + dz,
            )
        })
        .collect()
}

/// `PistonBaseBlock` behaviour.
pub struct PistonBehavior;
/// Static piston-base behaviour.
pub static PISTON_BEHAVIOR: PistonBehavior = PistonBehavior;

impl BlockBehavior for PistonBehavior {
    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        check_if_extend(world, pos, state)
    }

    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        old: Option<&BlockState>,
    ) -> io::Result<()> {
        if old.is_none_or(|old| old.name != state.name) {
            check_if_extend(world, pos, state)?;
        }
        Ok(())
    }
}

/// `PistonHeadBlock` behaviour.
pub struct PistonHeadBehavior;
/// Static piston-head behaviour.
pub static PISTON_HEAD_BEHAVIOR: PistonHeadBehavior = PistonHeadBehavior;

impl BlockBehavior for PistonHeadBehavior {
    fn neighbor_changed(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _source: BlockPos,
    ) -> io::Result<()> {
        if head_can_survive(world, pos, state)? {
            let behind = pos.relative(piston_facing(state).opposite());
            world.queue_neighbor_update(pos, behind, UpdateFlags::ALL | UpdateFlags::SHAPE);
        }
        Ok(())
    }

    fn update_shape(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        direction: Direction,
        _neighbor: &BlockState,
        state: &BlockState,
    ) -> io::Result<Option<BlockState>> {
        if direction.opposite() == piston_facing(state) && !head_can_survive(world, pos, state)? {
            Ok(Some(air()))
        } else {
            Ok(None)
        }
    }
}

/// `MovingPistonBlock` behaviour.
pub struct MovingPistonBehavior;
/// Static moving-piston behaviour.
pub static MOVING_PISTON_BEHAVIOR: MovingPistonBehavior = MovingPistonBehavior;

impl BlockBehavior for MovingPistonBehavior {
    fn on_place(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        state: &BlockState,
        _old: Option<&BlockState>,
    ) -> io::Result<()> {
        if is_moving_piston(state) {
            world.schedule(pos, MOVING_PISTON, 1);
        }
        Ok(())
    }

    fn scheduled_tick(
        &self,
        world: &mut TickWorld<'_>,
        pos: BlockPos,
        _state: &BlockState,
    ) -> io::Result<()> {
        let Some(mut payload) = world.piston_payload(pos) else {
            // A moving block without a payload cannot resolve; vanilla removes
            // it on interaction. Leaving it would be a silent stuck block.
            world.set_block(pos, air())?;
            return Ok(());
        };
        if payload.progress < 2 {
            payload.progress += 1;
            world.set_piston_payload(pos, payload);
            world.schedule(pos, MOVING_PISTON, 1);
            return Ok(());
        }

        world.remove_piston_payload(pos);
        world.set_block(pos, payload.moved_state)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWorld {
        blocks: BTreeMap<i64, BlockState>,
    }

    impl TestWorld {
        fn new() -> Self {
            Self {
                blocks: BTreeMap::new(),
            }
        }

        fn set(&mut self, pos: BlockPos, state: BlockState) {
            self.blocks.insert(pos.as_long(), state);
        }
    }

    impl PistonWorld for TestWorld {
        fn piston_block_state(&self, pos: BlockPos) -> Option<BlockState> {
            Some(self.blocks.get(&pos.as_long()).cloned().unwrap_or_else(air))
        }
    }

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    #[test]
    fn state_helpers_build_vanilla_states() {
        let piston = piston_base_state(false, Direction::East, true);
        assert_eq!(piston.name, PISTON);
        assert!(piston_extended(&piston));
        assert_eq!(piston_facing(&piston), Direction::East);

        let head = piston_head_state(Direction::Up, true, true);
        assert_eq!(head.name, PISTON_HEAD);
        assert_eq!(piston_facing(&head), Direction::Up);
        assert!(piston_type_is_sticky(&head));
        assert_eq!(property(&head, "short"), Some("true"));

        let moving = moving_piston_state(Direction::South, false);
        assert!(is_moving_piston(&moving));
        assert_eq!(piston_type(&moving), "normal");
    }

    #[test]
    fn resolver_pushes_a_line_up_to_twelve_blocks() {
        let origin = BlockPos::new(0, 0, 0);
        let mut world = TestWorld::new();
        world.set(origin, state(PISTON));
        for distance in 1..=12 {
            world.set(
                origin.relative_n(Direction::East, distance),
                state("minecraft:stone"),
            );
        }
        let plan = resolve_plan(&world, origin, Direction::East, true).expect("12 blocks fit");
        assert_eq!(plan.to_push.len(), 12);
        assert_eq!(plan.to_push[0], origin.relative(Direction::East));
        assert_eq!(plan.to_push[11], origin.relative_n(Direction::East, 12));

        world.set(
            origin.relative_n(Direction::East, 13),
            state("minecraft:stone"),
        );
        assert!(resolve_plan(&world, origin, Direction::East, true).is_none());
    }

    #[test]
    fn sticky_retraction_pulls_one_block_and_honey_rejects_slime() {
        let origin = BlockPos::new(0, 0, 0);
        let mut world = TestWorld::new();
        world.set(origin, state(STICKY_PISTON));
        world.set(
            origin.relative_n(Direction::East, 2),
            state("minecraft:stone"),
        );
        let plan = resolve_plan(&world, origin, Direction::East, false).expect("stone pulls");
        assert_eq!(plan.to_push, vec![origin.relative_n(Direction::East, 2)]);

        let mut world = TestWorld::new();
        world.set(origin, state(PISTON));
        world.set(origin.relative(Direction::East), state(HONEY_BLOCK));
        world.set(
            origin.relative(Direction::East).relative(Direction::Up),
            state(SLIME_BLOCK),
        );
        let plan = resolve_plan(&world, origin, Direction::East, true).expect("honey moves");
        assert_eq!(plan.to_push, vec![origin.relative(Direction::East)]);

        let mut world = TestWorld::new();
        world.set(origin, state(PISTON));
        world.set(origin.relative(Direction::East), state(SLIME_BLOCK));
        world.set(
            origin.relative(Direction::East).relative(Direction::Up),
            state(SLIME_BLOCK),
        );
        let plan = resolve_plan(&world, origin, Direction::East, true).expect("slime chain");
        assert_eq!(
            plan.to_push,
            vec![
                origin.relative(Direction::East),
                origin.relative(Direction::East).relative(Direction::Up)
            ]
        );
    }

    #[test]
    fn resolver_rejects_immovable_blocks() {
        let origin = BlockPos::new(0, 0, 0);
        for name in [
            "minecraft:obsidian",
            "minecraft:crying_obsidian",
            "minecraft:bedrock",
            "minecraft:chest",
            "minecraft:barrier",
        ] {
            let mut world = TestWorld::new();
            world.set(origin, state(PISTON));
            world.set(origin.relative(Direction::East), state(name));
            assert!(
                resolve_plan(&world, origin, Direction::East, true).is_none(),
                "{name} must not be pushed"
            );
        }
    }

    #[test]
    fn resolver_destroys_plants_and_fluids_in_front() {
        let origin = BlockPos::new(0, 0, 0);
        for name in [
            "minecraft:torch",
            "minecraft:short_grass",
            "minecraft:water",
        ] {
            let mut world = TestWorld::new();
            world.set(origin, state(PISTON));
            world.set(origin.relative(Direction::East), state(name));
            let plan = resolve_plan(&world, origin, Direction::East, true)
                .unwrap_or_else(|| panic!("{name} should be destroyed"));
            assert!(plan.to_push.is_empty());
            assert_eq!(plan.to_destroy, vec![origin.relative(Direction::East)]);
        }
    }

    #[test]
    fn resolver_branch_order_is_deterministic() {
        let origin = BlockPos::new(0, 0, 0);
        let mut world = TestWorld::new();
        world.set(origin, state(PISTON));
        world.set(origin.relative(Direction::East), state(SLIME_BLOCK));
        world.set(
            origin.relative_n(Direction::East, 2),
            state("minecraft:stone"),
        );
        world.set(
            origin.relative(Direction::East).relative(Direction::Up),
            state(SLIME_BLOCK),
        );
        let plan = resolve_plan(&world, origin, Direction::East, true).expect("resolves");
        assert_eq!(
            plan.to_push,
            vec![
                origin.relative(Direction::East),
                origin.relative_n(Direction::East, 2),
                origin.relative(Direction::East).relative(Direction::Up),
            ]
        );
        let second = resolve_plan(&world, origin, Direction::East, true).expect("resolves twice");
        assert_eq!(plan, second);
    }

    #[test]
    fn moving_payload_progress_and_collision_state() {
        let payload = MovingPiston {
            moved_state: state("minecraft:stone"),
            direction: Direction::East,
            extending: true,
            is_source_piston: false,
            progress: 1,
        };
        assert_eq!(payload.progress(), 0.5);
        assert_eq!(payload.extended_progress(), -0.5);
        assert_eq!(payload.movement_direction(), Direction::East);

        let source = MovingPiston {
            moved_state: piston_base_state(true, Direction::East, true),
            direction: Direction::East,
            extending: false,
            is_source_piston: true,
            progress: 1,
        };
        let collision = source.collision_state();
        assert_eq!(collision.name, PISTON_HEAD);
        assert!(piston_type_is_sticky(&collision));
    }
}
