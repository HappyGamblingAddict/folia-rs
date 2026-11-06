//! Vanilla block-breaking progress, tool rules, drops, and durability.
//!
//! This ports the gameplay half of breaking a block:
//!
//! * `ServerPlayerGameMode.handleBlockBreakAction` / `tick` for the
//!   start/stop/abort dig state machine and its 0.7 delayed-destroy rule.
//! * `Player.getDestroySpeed` and `Player.hasCorrectToolForDrops` for the
//!   per-tick progress formula.
//! * `BlockBehaviour.getDestroyProgress` for the `30`/`100` correct-tool
//!   divisor and the unbreakable (`hardness == -1`) case.
//! * `Tool`/`Item.getDestroySpeed`/`Item.isCorrectToolForDrops` for material
//!   speeds, tier gates, and the sword/shears special rules.
//! * `Item.mineBlock` / `ItemStack.hurtAndBreak` for durability.
//!
//! The per-state hardness comes from [`crate::block_behaviour`]. Tool tiers
//! come from the flattened vanilla block tags (`mineable/*`,
//! `incorrect_for_*_tool`, `needs_*_tool`, `sword_*`), so a tool is correct
//! exactly when its `mineable/*` tag contains the block and the material's
//! `incorrect_for_*_tool` tag does not.
//!
//! The wire protocol is unchanged: callers validate the client's dig packets
//! and broadcast the existing block-update packet when the block actually
//! breaks. Block-damage overlay packets are intentionally not produced here.

use crate::block_behaviour;
use crate::chunk::BlockState;
use crate::item_components::{ComponentValue, ItemStack, Tool};
use crate::loot::{LootCatalog, LootContext, LootReport};
use crate::math::BlockPos;
use crate::tags;
use folia_random::{LegacyRandomSource, RandomSource};
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

pub use crate::loot::ToolContext;

/// `ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK`.
pub const ACTION_START_DESTROY_BLOCK: i32 = 0;
/// `ServerboundPlayerActionPacket.Action.ABORT_DESTROY_BLOCK`.
pub const ACTION_ABORT_DESTROY_BLOCK: i32 = 1;
/// `ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK`.
pub const ACTION_STOP_DESTROY_BLOCK: i32 = 2;

/// Vanilla treats `air`, `cave_air`, and `void_air` as air.
pub fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// A vanilla tool family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Sword,
    Shears,
}

impl ToolKind {
    fn mineable_tag(self) -> Option<&'static str> {
        match self {
            Self::Pickaxe => Some("minecraft:mineable/pickaxe"),
            Self::Axe => Some("minecraft:mineable/axe"),
            Self::Shovel => Some("minecraft:mineable/shovel"),
            Self::Hoe => Some("minecraft:mineable/hoe"),
            Self::Sword | Self::Shears => None,
        }
    }
}

/// A vanilla tool material. The tag suffix matches `ToolMaterial`'s
/// `incorrect_for_*_tool` tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTier {
    Wooden,
    Stone,
    Copper,
    Iron,
    Gold,
    Diamond,
    Netherite,
}

impl ToolTier {
    fn tag_name(self) -> &'static str {
        match self {
            Self::Wooden => "wooden",
            Self::Stone => "stone",
            Self::Copper => "copper",
            Self::Iron => "iron",
            Self::Gold => "gold",
            Self::Diamond => "diamond",
            Self::Netherite => "netherite",
        }
    }

    /// `ToolMaterial.speed`.
    fn speed(self) -> f32 {
        match self {
            Self::Wooden => 2.0,
            Self::Stone => 4.0,
            Self::Copper => 5.0,
            Self::Iron => 6.0,
            Self::Gold => 12.0,
            Self::Diamond => 8.0,
            Self::Netherite => 9.0,
        }
    }

    /// `ToolMaterial.durability`.
    fn durability(self) -> i32 {
        match self {
            Self::Wooden => 59,
            Self::Stone => 131,
            Self::Copper => 190,
            Self::Iron => 250,
            Self::Gold => 32,
            Self::Diamond => 1_561,
            Self::Netherite => 2_031,
        }
    }
}

/// The subset of an item's `minecraft:tool` behaviour the breaking code uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolProfile {
    pub kind: ToolKind,
    pub tier: Option<ToolTier>,
    /// Mining speed used for blocks in the tool's `mineable/*` tag. The sword
    /// and shears special rules are applied in [`ToolProfile::mining_speed`].
    pub speed: f32,
    pub damage_per_block: i32,
    pub max_damage: i32,
    pub can_destroy_blocks_in_creative: bool,
}

impl ToolProfile {
    /// `Tool.getMiningSpeed`: the first rule with a speed for this state, or
    /// the tool's default mining speed (1.0 for every vanilla tool).
    pub fn mining_speed(self, state: &BlockState) -> f32 {
        match self.kind {
            ToolKind::Sword => {
                if state.name == "minecraft:cobweb" {
                    15.0
                } else if tags::block_has("minecraft:sword_instantly_mines", &state.name) {
                    // Vanilla uses `Float.MAX_VALUE`; the value only feeds the
                    // progress comparison, so any huge value is equivalent.
                    f32::MAX
                } else if tags::block_has("minecraft:sword_efficient", &state.name) {
                    1.5
                } else {
                    1.0
                }
            }
            ToolKind::Shears => {
                if state.name == "minecraft:cobweb"
                    || tags::block_has("minecraft:leaves", &state.name)
                {
                    15.0
                } else if tags::block_has("minecraft:wool", &state.name) {
                    5.0
                } else if matches!(
                    state.name.as_str(),
                    "minecraft:vine" | "minecraft:glow_lichen"
                ) {
                    2.0
                } else {
                    1.0
                }
            }
            kind => kind
                .mineable_tag()
                .filter(|tag| tags::block_has(tag, &state.name))
                .map_or(1.0, |_| self.speed),
        }
    }

    /// `Tool.isCorrectForDrops`: the first rule with `correct_for_drops` that
    /// matches the state. Vanilla sets this only for `mineable/*` (true) and
    /// `incorrect_for_*_tool` (false), plus cobweb for swords and shears.
    pub fn is_correct_for_drops(self, state: &BlockState) -> bool {
        match self.kind {
            ToolKind::Sword | ToolKind::Shears => state.name == "minecraft:cobweb",
            kind => {
                let Some(mineable) = kind.mineable_tag() else {
                    return false;
                };
                if !tags::block_has(mineable, &state.name) {
                    return false;
                }
                let Some(tier) = self.tier else {
                    return false;
                };
                !tags::block_has(
                    &format!("minecraft:incorrect_for_{}_tool", tier.tag_name()),
                    &state.name,
                )
            }
        }
    }
}

/// Vanilla tool material/kind lookup for the built-in tools.
///
/// The per-item `minecraft:tool` component is normally supplied by the item
/// prototype, which is not dumped yet (TODO P1.4). This table is the exact
/// `ToolMaterial` data for the vanilla tools, so breaking still behaves like
/// Java. An explicit `minecraft:tool` component always takes precedence.
pub fn builtin_tool_profile(item: &str) -> Option<ToolProfile> {
    let name = item.strip_prefix("minecraft:").unwrap_or(item);
    if name == "shears" {
        return Some(ToolProfile {
            kind: ToolKind::Shears,
            tier: None,
            speed: 1.0,
            damage_per_block: 1,
            max_damage: 238,
            can_destroy_blocks_in_creative: true,
        });
    }

    let (material, kind) = name.split_once('_')?;
    let tier = match material {
        "wooden" => ToolTier::Wooden,
        "stone" => ToolTier::Stone,
        "copper" => ToolTier::Copper,
        "iron" => ToolTier::Iron,
        "golden" => ToolTier::Gold,
        "diamond" => ToolTier::Diamond,
        "netherite" => ToolTier::Netherite,
        _ => return None,
    };
    let (kind, speed, damage_per_block, can_destroy_blocks_in_creative) = match kind {
        "pickaxe" => (ToolKind::Pickaxe, tier.speed(), 1, true),
        "axe" => (ToolKind::Axe, tier.speed(), 1, true),
        "shovel" => (ToolKind::Shovel, tier.speed(), 1, true),
        "hoe" => (ToolKind::Hoe, tier.speed(), 1, true),
        "sword" => (ToolKind::Sword, 1.0, 2, false),
        _ => return None,
    };
    Some(ToolProfile {
        kind,
        tier: Some(tier),
        speed,
        damage_per_block,
        max_damage: tier.durability(),
        can_destroy_blocks_in_creative,
    })
}

fn explicit_tool(held: Option<&ItemStack>) -> Option<&Tool> {
    match held?.get_component("minecraft:tool")? {
        ComponentValue::Tool(tool) => Some(tool),
        _ => None,
    }
}

fn rule_matches(blocks: &crate::item_components::BlockSet, state: &BlockState) -> bool {
    match blocks {
        crate::item_components::BlockSet::Tag(tag) => tags::block_has(tag, &state.name),
        crate::item_components::BlockSet::Blocks(names) => {
            names.iter().any(|name| name == &state.name)
        }
    }
}

fn explicit_mining_speed(tool: &Tool, state: &BlockState) -> f32 {
    for rule in &tool.rules {
        if rule.speed.is_some() && rule_matches(&rule.blocks, state) {
            return rule.speed.unwrap_or(1.0);
        }
    }
    tool.default_mining_speed
}

fn explicit_correct_for_drops(tool: &Tool, state: &BlockState) -> bool {
    for rule in &tool.rules {
        if rule.correct_for_drops.is_some() && rule_matches(&rule.blocks, state) {
            return rule.correct_for_drops.unwrap_or(false);
        }
    }
    false
}

/// `ItemStack.getDestroySpeed` / `Tool.getMiningSpeed` for the held stack.
pub fn mining_speed(state: &BlockState, held: Option<&ItemStack>) -> f32 {
    if let Some(tool) = explicit_tool(held) {
        return explicit_mining_speed(tool, state);
    }
    held.and_then(|stack| builtin_tool_profile(&stack.item))
        .map_or(1.0, |profile| profile.mining_speed(state))
}

/// `ItemStack.isCorrectToolForDrops`.
pub fn is_correct_tool_for_drops(state: &BlockState, held: Option<&ItemStack>) -> bool {
    if let Some(tool) = explicit_tool(held) {
        return explicit_correct_for_drops(tool, state);
    }
    held.and_then(|stack| builtin_tool_profile(&stack.item))
        .is_some_and(|profile| profile.is_correct_for_drops(state))
}

/// `BlockState.requiresCorrectToolForDrops`.
///
/// The 1.21.11 behaviour dump contains hardness and light/physics flags but
/// not this boolean. It is the set of blocks that call
/// `Properties.requiresCorrectToolForDrops()` in `Blocks.java` (including the
/// `ofLegacyCopy`/`registerLegacyStair` inheritances); blocks registered
/// through helper methods are covered by the three `needs_*_tool` tags.
pub fn requires_correct_tool_for_drops(state: &BlockState) -> bool {
    requires_tool_set().contains(state.name.as_str())
        || tags::block_has("minecraft:needs_stone_tool", &state.name)
        || tags::block_has("minecraft:needs_iron_tool", &state.name)
        || tags::block_has("minecraft:needs_diamond_tool", &state.name)
}

/// `Player.hasCorrectToolForDrops`: blocks that do not require a tool always
/// count as correctly harvested, even with an empty hand.
pub fn has_correct_tool_for_drops(state: &BlockState, held: Option<&ItemStack>) -> bool {
    !requires_correct_tool_for_drops(state) || is_correct_tool_for_drops(state, held)
}

/// The player-state inputs of `Player.getDestroySpeed`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiningContext {
    pub on_ground: bool,
    /// `isEyeInFluid(FluidTags.WATER)`.
    pub eye_in_water: bool,
    /// `MobEffects.HASTE` amplifier, if the player has it.
    pub haste_amplifier: Option<i32>,
    /// `MobEffects.MINING_FATIGUE` amplifier, if the player has it.
    pub mining_fatigue_amplifier: Option<i32>,
    /// `Attributes.BLOCK_BREAK_SPEED` (1.0 without modifiers).
    pub block_break_speed: f32,
    /// `Attributes.SUBMERGED_MINING_SPEED` (0.2 without modifiers).
    pub submerged_mining_speed: f32,
}

impl Default for MiningContext {
    fn default() -> Self {
        Self {
            on_ground: true,
            eye_in_water: false,
            haste_amplifier: None,
            mining_fatigue_amplifier: None,
            block_break_speed: 1.0,
            submerged_mining_speed: 0.2,
        }
    }
}

fn efficiency_level(held: Option<&ItemStack>) -> i32 {
    match held.and_then(|stack| stack.get_component("minecraft:enchantments")) {
        Some(ComponentValue::Enchantments(map)) => {
            map.get("minecraft:efficiency").copied().unwrap_or(0).max(0)
        }
        _ => 0,
    }
}

/// `Player.getDestroySpeed` before the hardness division.
pub fn player_destroy_speed(
    state: &BlockState,
    held: Option<&ItemStack>,
    context: &MiningContext,
) -> f32 {
    let mut speed = mining_speed(state, held);
    if speed > 1.0 {
        let efficiency = efficiency_level(held);
        if efficiency > 0 {
            speed += (efficiency * efficiency + 1) as f32;
        }
    }
    if let Some(amplifier) = context.haste_amplifier {
        speed *= 1.0 + (amplifier as f32 + 1.0) * 0.2;
    }
    if let Some(amplifier) = context.mining_fatigue_amplifier {
        speed *= match amplifier {
            0 => 0.3,
            1 => 0.09,
            2 => 0.0027,
            _ => 8.1e-4,
        };
    }
    speed *= context.block_break_speed;
    if context.eye_in_water {
        speed *= context.submerged_mining_speed;
    }
    if !context.on_ground {
        speed /= 5.0;
    }
    speed
}

/// `BlockBehaviour.getDestroyProgress`: the fraction of the block destroyed per
/// game tick.
pub fn destroy_progress_per_tick(
    state: &BlockState,
    held: Option<&ItemStack>,
    context: &MiningContext,
) -> f32 {
    let hardness = block_behaviour::hardness(state);
    if hardness < 0.0 {
        return 0.0;
    }
    let speed = player_destroy_speed(state, held, context);
    if hardness == 0.0 {
        return if speed > 0.0 { f32::INFINITY } else { 0.0 };
    }
    let divisor = if has_correct_tool_for_drops(state, held) {
        30.0
    } else {
        100.0
    };
    speed / hardness / divisor
}

/// Game ticks (at 20 per second) needed to finish the block.
pub fn break_ticks(state: &BlockState, held: Option<&ItemStack>, context: &MiningContext) -> f32 {
    let progress = destroy_progress_per_tick(state, held, context);
    if progress <= 0.0 {
        f32::INFINITY
    } else {
        1.0 / progress
    }
}

/// Result of a `START_DESTROY_BLOCK` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigStart {
    /// The state is air or unbreakable, or the tool cannot make progress.
    Rejected,
    /// Normal survival mining; callers keep the target in [`DigState`].
    Started,
    /// Creative mode or a zero-hardness block; destroy it immediately.
    Instant,
}

/// Loot inputs bundled for [`DigState::stop_with_loot`].
#[derive(Clone, Copy)]
pub struct DigLootContext<'a> {
    pub context: &'a BlockLootContext<'a>,
    pub provider: Option<&'a dyn LootProvider>,
}

/// Result of a `STOP_DESTROY_BLOCK` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigStop {
    /// No block was being mined, or it is no longer breakable.
    Ignored,
    /// The client stopped a block other than the server's tracked target.
    Mismatch { active: BlockPos },
    /// Vanilla's `hasDelayedDestroy`: the server keeps mining this target.
    Delayed,
    /// The accumulated progress reached vanilla's 0.7 stop threshold.
    Destroyed,
}

#[derive(Debug, Clone, Copy)]
struct ActiveDig {
    pos: BlockPos,
    start_tick: u64,
}

/// Per-session digging state, mirroring `ServerPlayerGameMode`'s
/// `isDestroyingBlock`/`destroyProgressStart`/`hasDelayedDestroy` fields.
///
/// Ticks are caller-supplied so the state machine is deterministic and can be
/// exercised without a live session. The join loop derives them from the
/// elapsed play time at 20 ticks per second.
#[derive(Debug, Clone, Default)]
pub struct DigState {
    active: Option<ActiveDig>,
    delayed: Option<ActiveDig>,
}

impl DigState {
    /// The block currently being mined, if any.
    pub fn active_target(&self) -> Option<BlockPos> {
        self.active.map(|dig| dig.pos)
    }

    /// The block vanilla would finish on a later tick
    /// (`hasDelayedDestroy`), if any.
    pub fn delayed_target(&self) -> Option<BlockPos> {
        self.delayed.map(|dig| dig.pos)
    }

    /// Drop a delayed destroy without changing the world.
    pub fn cancel_delayed(&mut self) {
        self.delayed = None;
    }

    /// Handle `START_DESTROY_BLOCK`.
    pub fn start(
        &mut self,
        pos: BlockPos,
        creative: bool,
        state: &BlockState,
        held: Option<&ItemStack>,
        context: &MiningContext,
        now_tick: u64,
    ) -> DigStart {
        self.active = None;
        self.delayed = None;
        if is_air(state) {
            return DigStart::Rejected;
        }
        let hardness = block_behaviour::hardness(state);
        if !creative && hardness < 0.0 {
            return DigStart::Rejected;
        }
        if creative || hardness == 0.0 {
            return DigStart::Instant;
        }
        if destroy_progress_per_tick(state, held, context) <= 0.0 {
            return DigStart::Rejected;
        }
        self.active = Some(ActiveDig {
            pos,
            start_tick: now_tick,
        });
        DigStart::Started
    }

    /// Handle `STOP_DESTROY_BLOCK`.
    pub fn stop(
        &mut self,
        pos: BlockPos,
        state: &BlockState,
        held: Option<&ItemStack>,
        context: &MiningContext,
        now_tick: u64,
    ) -> DigStop {
        let Some(dig) = self.active.or(self.delayed) else {
            return DigStop::Ignored;
        };
        if dig.pos != pos {
            return DigStop::Mismatch { active: dig.pos };
        }
        self.active = None;
        if is_air(state) {
            self.delayed = None;
            return DigStop::Ignored;
        }
        let elapsed = now_tick.saturating_sub(dig.start_tick) as f32 + 1.0;
        let progress = destroy_progress_per_tick(state, held, context) * elapsed;
        if progress >= 0.7 {
            self.delayed = None;
            DigStop::Destroyed
        } else {
            self.delayed = Some(dig);
            DigStop::Delayed
        }
    }

    /// Handle `STOP_DESTROY_BLOCK` and roll drops in one step.
    ///
    /// The drop roll only runs when the stop destroys the block. Counts and
    /// tool gates come from the supplied provider or the shipped
    /// [`LootCatalog`]; durability is left to the caller exactly as with
    /// [`DigState::stop`].
    pub fn stop_with_loot(
        &mut self,
        pos: BlockPos,
        state: &BlockState,
        context: &MiningContext,
        now_tick: u64,
        loot: &DigLootContext<'_>,
        random: &mut dyn RandomSource,
    ) -> (DigStop, Option<BlockDrops>) {
        let stop = self.stop(pos, state, loot.context.tool, context, now_tick);
        let drops = if stop == DigStop::Destroyed {
            Some(block_drops_with_context(
                state,
                loot.context,
                loot.provider,
                random,
            ))
        } else {
            None
        };
        (stop, drops)
    }

    /// Advance a vanilla `hasDelayedDestroy` target. Returns `true` when the
    /// block should be destroyed this tick (progress reached 1.0).
    pub fn tick_delayed(
        &mut self,
        state: &BlockState,
        held: Option<&ItemStack>,
        context: &MiningContext,
        now_tick: u64,
    ) -> bool {
        let Some(dig) = self.delayed else {
            return false;
        };
        if is_air(state) {
            self.delayed = None;
            return false;
        }
        let elapsed = now_tick.saturating_sub(dig.start_tick) as f32 + 1.0;
        if destroy_progress_per_tick(state, held, context) * elapsed >= 1.0 {
            self.delayed = None;
            true
        } else {
            false
        }
    }

    /// Handle `ABORT_DESTROY_BLOCK`. Clears both the active and delayed target
    /// and returns the previously tracked position when it differs from the
    /// client's abort position, so the caller can resynchronise the client.
    pub fn abort(&mut self, pos: BlockPos) -> Option<BlockPos> {
        let tracked = self.active.or(self.delayed).map(|dig| dig.pos);
        self.active = None;
        self.delayed = None;
        tracked.filter(|tracked| *tracked != pos)
    }
}

/// Vanilla `hurtAndBreak` outcome for one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolWear {
    pub damage: i32,
    pub max_damage: i32,
    pub broke: bool,
}

impl ToolWear {
    /// Whether anything changed and the client needs an inventory update.
    pub fn changed(self) -> bool {
        self.damage > 0 || self.broke
    }
}

/// Apply `Item.mineBlock`'s durability cost to the held stack.
///
/// Creative mode (`instabuild`) and zero-hardness blocks never wear a tool.
/// `Unbreaking` is not modelled yet (it needs a random draw); tools always wear
/// by `damage_per_block` like an unenchanted tool.
pub fn apply_block_durability(
    held: &mut ItemStack,
    creative: bool,
    block_hardness: f32,
) -> ToolWear {
    let none = ToolWear {
        damage: 0,
        max_damage: 0,
        broke: false,
    };
    if creative || held.is_empty() || block_hardness == 0.0 {
        return none;
    }
    if held.get_component("minecraft:unbreakable").is_some() {
        return none;
    }

    let (damage_per_block, profile_max_damage) = if let Some(tool) = explicit_tool(Some(held)) {
        (tool.damage_per_block, 0)
    } else if let Some(profile) = builtin_tool_profile(&held.item) {
        (profile.damage_per_block, profile.max_damage)
    } else {
        return none;
    };
    if damage_per_block <= 0 {
        return none;
    }

    let max_damage = {
        let explicit = held.max_damage();
        if explicit > 0 {
            explicit
        } else {
            profile_max_damage
        }
    };
    if max_damage <= 0 {
        return none;
    }

    let damage = held.damage().saturating_add(damage_per_block);
    if damage >= max_damage {
        // `hurtAndBreak` breaks the item instead of clamping the damage; the
        // held slot becomes empty.
        held.shrink(1);
        return ToolWear {
            damage: max_damage,
            max_damage,
            broke: true,
        };
    }
    held.set_component(ComponentValue::Damage(damage));
    if held.get_component("minecraft:max_damage").is_none() {
        // Prototype defaults are not dumped yet (TODO P1.4). Writing the
        // resolved maximum alongside the damage keeps the client bar and the
        // saved playerdata self-describing.
        held.set_component(ComponentValue::MaxDamage(max_damage));
    }
    ToolWear {
        damage,
        max_damage,
        broke: false,
    }
}

/// Inputs for one block-loot roll.
///
/// `tool_context` is explicit so callers that do not put enchantments on the
/// [`ItemStack`] can still supply Silk Touch and Fortune. Use
/// [`BlockLootContext::from_tool`] to derive both from a held stack, or
/// [`BlockLootContext::default`] for an unenchanted bare-hand break.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockLootContext<'a> {
    pub tool: Option<&'a ItemStack>,
    pub tool_context: ToolContext,
    /// Pre-computed `hasCorrectToolForDrops` result. `None` lets
    /// [`block_drops_with_context`] compute it from `tool`.
    pub correct_tool: Option<bool>,
    pub explosion_radius: Option<f32>,
}

impl<'a> BlockLootContext<'a> {
    /// Derives the tool context from the held stack's enchantment components.
    pub fn from_tool(tool: Option<&'a ItemStack>) -> Self {
        Self {
            tool,
            tool_context: ToolContext::from_stack(tool),
            ..Self::default()
        }
    }

    /// An unenchanted context with an explicit tool item.
    pub fn new(tool: Option<&'a ItemStack>) -> Self {
        Self {
            tool,
            ..Self::default()
        }
    }

    pub fn with_tool_context(mut self, tool_context: ToolContext) -> Self {
        self.tool_context = tool_context;
        self
    }

    pub fn with_silk_touch(mut self, silk_touch: bool) -> Self {
        self.tool_context.silk_touch = silk_touch;
        self
    }

    pub fn with_fortune(mut self, fortune: i32) -> Self {
        self.tool_context.fortune = fortune.max(0);
        self
    }

    pub fn with_correct_tool(mut self, correct_tool: bool) -> Self {
        self.correct_tool = Some(correct_tool);
        self
    }

    pub fn with_explosion_radius(mut self, radius: f32) -> Self {
        self.explosion_radius = Some(radius.max(0.0));
        self
    }
}

/// The result of a provider roll before the block-breaking gate is applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProvidedDrops {
    pub items: Vec<ItemStack>,
    pub report: LootReport,
    pub table_id: Option<String>,
}

/// A loot-table provider hook for `crate::loot`.
///
/// Providers should implement [`LootProvider::roll_block`]. The legacy
/// [`LootProvider::block_drops`] method is kept for simple callers and is
/// adapted by the default `roll_block` implementation.
pub trait LootProvider {
    fn block_drops(&self, state: &BlockState, tool: Option<&ItemStack>) -> Option<Vec<ItemStack>>;

    /// Rolls a block's drops with the full context and a caller-supplied
    /// [`RandomSource`]. Returns `None` to let the caller fall back.
    fn roll_block(
        &self,
        state: &BlockState,
        context: &BlockLootContext<'_>,
        random: &mut dyn RandomSource,
    ) -> Option<ProvidedDrops> {
        let _ = random;
        self.block_drops(state, context.tool)
            .map(|items| ProvidedDrops {
                items,
                ..ProvidedDrops::default()
            })
    }
}

/// A [`LootProvider`] backed by the shipped [`LootCatalog`].
#[derive(Debug, Clone, Copy)]
pub struct CatalogLootProvider<'a> {
    catalog: &'a LootCatalog,
}

impl<'a> CatalogLootProvider<'a> {
    pub fn new(catalog: &'a LootCatalog) -> Self {
        Self { catalog }
    }

    /// Resolves `minecraft:blocks/<block>` (or an exact table id) and rolls it.
    pub fn roll_block(
        &self,
        state: &BlockState,
        context: &BlockLootContext<'_>,
        random: &mut dyn RandomSource,
    ) -> Option<ProvidedDrops> {
        let table_id = self.catalog.block_table_id(&state.name)?;
        let properties: BTreeMap<String, String> = state.properties.iter().cloned().collect();
        let mut loot_context = LootContext::new()
            .with_tool_context(context.tool_context)
            .with_block_state(&state.name, &properties);
        if let Some(tool) = context.tool {
            loot_context = loot_context.with_tool(tool);
        }
        if let Some(radius) = context.explosion_radius {
            loot_context = loot_context.with_explosion_radius(radius);
        }
        let roll = self.catalog.roll(table_id, random, &loot_context)?;
        Some(ProvidedDrops {
            items: roll.stacks,
            report: roll.report,
            table_id: Some(table_id.to_owned()),
        })
    }
}

impl CatalogLootProvider<'static> {
    /// The process-wide shipped catalog.
    pub fn global() -> Self {
        Self::new(LootCatalog::global())
    }
}

impl LootProvider for CatalogLootProvider<'_> {
    fn block_drops(&self, state: &BlockState, tool: Option<&ItemStack>) -> Option<Vec<ItemStack>> {
        let context = BlockLootContext::from_tool(tool);
        let mut random = LegacyRandomSource::new(default_drop_seed(state, tool));
        CatalogLootProvider::roll_block(self, state, &context, &mut random).map(|drops| drops.items)
    }

    fn roll_block(
        &self,
        state: &BlockState,
        context: &BlockLootContext<'_>,
        random: &mut dyn RandomSource,
    ) -> Option<ProvidedDrops> {
        CatalogLootProvider::roll_block(self, state, context, random)
    }
}

/// Where a drop list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSource {
    /// A shipped loot table or the built-in per-block fallback table.
    Table,
    /// A family rule (`*_leaves`, glass, ice, `*_wall_torch`, `*_wall_sign`).
    Family,
    /// A [`LootProvider`] supplied by the caller.
    LootHook,
    /// No table entry; the block's own item is the drop.
    SelfDrop,
    /// No item exists for this block, or the tool gate rejected the drop.
    None,
}

/// The drops from one block state.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDrops {
    pub items: Vec<ItemStack>,
    pub experience: i32,
    pub requires_correct_tool: bool,
    pub had_correct_tool: bool,
    pub source: DropSource,
    /// Diagnostics from the loot-table roll. Unknown functions and conditions
    /// are named here instead of being silently skipped.
    pub report: LootReport,
    /// Shipped table id when the drops came from a loot table.
    pub loot_table: Option<String>,
}

impl BlockDrops {
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            experience: 0,
            requires_correct_tool: false,
            had_correct_tool: false,
            source: DropSource::None,
            report: LootReport::default(),
            loot_table: None,
        }
    }

    /// Total number of dropped items (sum of stack counts).
    pub fn item_count(&self) -> i32 {
        self.items.iter().map(|stack| stack.count).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.items.iter().all(ItemStack::is_empty)
    }
}

/// One built-in drop table row. Counts of vanilla ranges are collapsed to a
/// deterministic representative until loot tables land (see `BLOCKERS.md`).
struct DropEntry {
    block: &'static str,
    item: &'static str,
    count: i32,
    experience: i32,
}

const fn drop_entry(
    block: &'static str,
    item: &'static str,
    count: i32,
    experience: i32,
) -> DropEntry {
    DropEntry {
        block,
        item,
        count,
        experience,
    }
}

#[rustfmt::skip]
const DROP_TABLE: &[DropEntry] = &[
    // Stone family.
    drop_entry("minecraft:stone", "minecraft:cobblestone", 1, 0),
    drop_entry("minecraft:cobblestone", "minecraft:cobblestone", 1, 0),
    drop_entry("minecraft:mossy_cobblestone", "minecraft:mossy_cobblestone", 1, 0),
    drop_entry("minecraft:granite", "minecraft:granite", 1, 0),
    drop_entry("minecraft:diorite", "minecraft:diorite", 1, 0),
    drop_entry("minecraft:andesite", "minecraft:andesite", 1, 0),
    drop_entry("minecraft:tuff", "minecraft:tuff", 1, 0),
    drop_entry("minecraft:calcite", "minecraft:calcite", 1, 0),
    drop_entry("minecraft:dripstone_block", "minecraft:dripstone_block", 1, 0),
    drop_entry("minecraft:deepslate", "minecraft:cobbled_deepslate", 1, 0),
    drop_entry("minecraft:cobbled_deepslate", "minecraft:cobbled_deepslate", 1, 0),
    drop_entry("minecraft:blackstone", "minecraft:blackstone", 1, 0),
    drop_entry("minecraft:basalt", "minecraft:basalt", 1, 0),
    drop_entry("minecraft:smooth_basalt", "minecraft:smooth_basalt", 1, 0),
    drop_entry("minecraft:netherrack", "minecraft:netherrack", 1, 0),
    drop_entry("minecraft:end_stone", "minecraft:end_stone", 1, 0),
    drop_entry("minecraft:obsidian", "minecraft:obsidian", 1, 0),
    drop_entry("minecraft:crying_obsidian", "minecraft:crying_obsidian", 1, 0),
    drop_entry("minecraft:ancient_debris", "minecraft:ancient_debris", 1, 0),
    drop_entry("minecraft:sandstone", "minecraft:sandstone", 1, 0),
    drop_entry("minecraft:red_sandstone", "minecraft:red_sandstone", 1, 0),
    drop_entry("minecraft:quartz_block", "minecraft:quartz_block", 1, 0),
    drop_entry("minecraft:amethyst_block", "minecraft:amethyst_block", 1, 0),
    drop_entry("minecraft:bone_block", "minecraft:bone_block", 1, 0),
    drop_entry("minecraft:magma_block", "minecraft:magma_block", 1, 0),
    drop_entry("minecraft:glowstone", "minecraft:glowstone_dust", 2, 0),
    drop_entry("minecraft:sea_lantern", "minecraft:prismarine_crystals", 2, 0),
    drop_entry("minecraft:terracotta", "minecraft:terracotta", 1, 0),
    drop_entry("minecraft:bricks", "minecraft:bricks", 1, 0),
    drop_entry("minecraft:stone_bricks", "minecraft:stone_bricks", 1, 0),
    drop_entry("minecraft:nether_bricks", "minecraft:nether_bricks", 1, 0),
    drop_entry("minecraft:bookshelf", "minecraft:book", 3, 0),
    // Ores.
    drop_entry("minecraft:coal_ore", "minecraft:coal", 1, 1),
    drop_entry("minecraft:deepslate_coal_ore", "minecraft:coal", 1, 1),
    drop_entry("minecraft:iron_ore", "minecraft:raw_iron", 1, 0),
    drop_entry("minecraft:deepslate_iron_ore", "minecraft:raw_iron", 1, 0),
    drop_entry("minecraft:copper_ore", "minecraft:raw_copper", 3, 0),
    drop_entry("minecraft:deepslate_copper_ore", "minecraft:raw_copper", 3, 0),
    drop_entry("minecraft:gold_ore", "minecraft:raw_gold", 1, 0),
    drop_entry("minecraft:deepslate_gold_ore", "minecraft:raw_gold", 1, 0),
    drop_entry("minecraft:nether_gold_ore", "minecraft:gold_nugget", 4, 0),
    drop_entry("minecraft:diamond_ore", "minecraft:diamond", 1, 5),
    drop_entry("minecraft:deepslate_diamond_ore", "minecraft:diamond", 1, 5),
    drop_entry("minecraft:emerald_ore", "minecraft:emerald", 1, 5),
    drop_entry("minecraft:deepslate_emerald_ore", "minecraft:emerald", 1, 5),
    drop_entry("minecraft:lapis_ore", "minecraft:lapis_lazuli", 6, 3),
    drop_entry("minecraft:deepslate_lapis_ore", "minecraft:lapis_lazuli", 6, 3),
    drop_entry("minecraft:redstone_ore", "minecraft:redstone", 4, 3),
    drop_entry("minecraft:deepslate_redstone_ore", "minecraft:redstone", 4, 3),
    drop_entry("minecraft:nether_quartz_ore", "minecraft:quartz", 1, 3),
    // Soil and soft blocks.
    drop_entry("minecraft:grass_block", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:dirt", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:coarse_dirt", "minecraft:coarse_dirt", 1, 0),
    drop_entry("minecraft:rooted_dirt", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:podzol", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:mycelium", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:farmland", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:dirt_path", "minecraft:dirt", 1, 0),
    drop_entry("minecraft:mud", "minecraft:mud", 1, 0),
    drop_entry("minecraft:clay", "minecraft:clay_ball", 4, 0),
    drop_entry("minecraft:sand", "minecraft:sand", 1, 0),
    drop_entry("minecraft:red_sand", "minecraft:red_sand", 1, 0),
    drop_entry("minecraft:gravel", "minecraft:gravel", 1, 0),
    drop_entry("minecraft:snow_block", "minecraft:snow_block", 1, 0),
    drop_entry("minecraft:snow", "minecraft:snowball", 1, 0),
    drop_entry("minecraft:soul_sand", "minecraft:soul_sand", 1, 0),
    drop_entry("minecraft:soul_soil", "minecraft:soul_soil", 1, 0),
    // Plants and misc.
    drop_entry("minecraft:cobweb", "minecraft:string", 1, 0),
    drop_entry("minecraft:melon", "minecraft:melon_slice", 3, 0),
    drop_entry("minecraft:pumpkin", "minecraft:pumpkin", 1, 0),
    drop_entry("minecraft:hay_block", "minecraft:hay_block", 1, 0),
    drop_entry("minecraft:sponge", "minecraft:sponge", 1, 0),
    drop_entry("minecraft:netherite_block", "minecraft:netherite_block", 1, 0),
    drop_entry("minecraft:iron_block", "minecraft:iron_block", 1, 0),
    drop_entry("minecraft:gold_block", "minecraft:gold_block", 1, 0),
    drop_entry("minecraft:diamond_block", "minecraft:diamond_block", 1, 0),
    drop_entry("minecraft:emerald_block", "minecraft:emerald_block", 1, 0),
    drop_entry("minecraft:lapis_block", "minecraft:lapis_block", 1, 0),
    drop_entry("minecraft:redstone_block", "minecraft:redstone_block", 1, 0),
    drop_entry("minecraft:coal_block", "minecraft:coal_block", 1, 0),
];

fn table_entry(block: &str) -> Option<&'static DropEntry> {
    DROP_TABLE.iter().find(|entry| entry.block == block)
}

/// Special families that are not one-to-one with their item id.
enum FamilyDrop {
    Nothing,
    SelfState,
    Item(&'static str),
}

fn family_drop(block: &str, held: Option<&ItemStack>) -> Option<FamilyDrop> {
    if block.ends_with("_leaves") {
        // Without silk touch, leaves drop saplings/sticks. The deterministic
        // table does not model those secondary drops, so only shears return
        // the leaf block itself.
        let shears = held.is_some_and(|stack| stack.item == "minecraft:shears");
        return Some(if shears {
            FamilyDrop::SelfState
        } else {
            FamilyDrop::Nothing
        });
    }
    if block == "minecraft:glass"
        || block.ends_with("_stained_glass")
        || block.ends_with("_glass_pane")
        || block.ends_with("_stained_glass_pane")
    {
        // Glass needs silk touch to drop itself; deterministic no-silk-touch.
        return Some(FamilyDrop::Nothing);
    }
    if matches!(
        block,
        "minecraft:ice" | "minecraft:blue_ice" | "minecraft:frosted_ice"
    ) {
        return Some(FamilyDrop::Nothing);
    }
    if block.ends_with("_wall_torch") {
        return Some(FamilyDrop::Item("minecraft:torch"));
    }
    if let Some(sign) = block.strip_suffix("_wall_sign") {
        // `oak_wall_sign` -> `oak_sign`; the item name is the block name with
        // the `_wall_` infix removed.
        let sign = sign.strip_prefix("minecraft:").unwrap_or(sign);
        return Some(match sign {
            "oak" => FamilyDrop::Item("minecraft:oak_sign"),
            "spruce" => FamilyDrop::Item("minecraft:spruce_sign"),
            "birch" => FamilyDrop::Item("minecraft:birch_sign"),
            "jungle" => FamilyDrop::Item("minecraft:jungle_sign"),
            "acacia" => FamilyDrop::Item("minecraft:acacia_sign"),
            "dark_oak" => FamilyDrop::Item("minecraft:dark_oak_sign"),
            "mangrove" => FamilyDrop::Item("minecraft:mangrove_sign"),
            "cherry" => FamilyDrop::Item("minecraft:cherry_sign"),
            "pale_oak" => FamilyDrop::Item("minecraft:pale_oak_sign"),
            "bamboo" => FamilyDrop::Item("minecraft:bamboo_sign"),
            "crimson" => FamilyDrop::Item("minecraft:crimson_sign"),
            "warped" => FamilyDrop::Item("minecraft:warped_sign"),
            _ => FamilyDrop::Nothing,
        });
    }
    None
}

/// Drops for a broken block, applying vanilla's harvest gate and the optional
/// loot-table hook.
///
/// With no explicit provider this rolls the shipped `LootCatalog` table for the
/// block. The built-in [`DROP_TABLE`] is only consulted when no shipped table
/// exists or a shipped table cannot be evaluated with the current context.
/// Because this legacy signature has no caller RNG stream, it seeds
/// `folia_random::LegacyRandomSource` deterministically from the block state
/// and held stack; use [`block_drops_with_context`] to supply the world's
/// random source.
pub fn block_drops(
    state: &BlockState,
    held: Option<&ItemStack>,
    loot: Option<&dyn LootProvider>,
) -> BlockDrops {
    let context = BlockLootContext::from_tool(held);
    let mut random = LegacyRandomSource::new(default_drop_seed(state, held));
    block_drops_with_context(state, &context, loot, &mut random)
}

/// Context-aware block drops with an explicit random source.
///
/// The harvest gate (`requiresCorrectToolForDrops` + `hasCorrectToolForDrops`)
/// is applied first, exactly like `Player.destroyBlock`. A caller-supplied
/// provider is tried before the shipped catalog. A known shipped table is
/// never silently discarded: if its roll reaches an unsupported condition with
/// no items, the fallback is used only after the report has recorded the
/// unsupported payload.
pub fn block_drops_with_context(
    state: &BlockState,
    context: &BlockLootContext<'_>,
    loot: Option<&dyn LootProvider>,
    random: &mut dyn RandomSource,
) -> BlockDrops {
    let requires_correct_tool = requires_correct_tool_for_drops(state);
    let had_correct_tool = context
        .correct_tool
        .unwrap_or_else(|| has_correct_tool_for_drops(state, context.tool));
    if requires_correct_tool && !had_correct_tool {
        return BlockDrops {
            items: Vec::new(),
            experience: 0,
            requires_correct_tool,
            had_correct_tool,
            source: DropSource::None,
            report: LootReport::default(),
            loot_table: None,
        };
    }

    let experience = table_entry(&state.name).map_or(0, |entry| entry.experience);

    if let Some(loot) = loot
        && let Some(provided) = loot.roll_block(state, context, random)
    {
        return BlockDrops {
            items: provided.items,
            experience,
            requires_correct_tool,
            had_correct_tool,
            source: DropSource::LootHook,
            report: provided.report,
            loot_table: provided.table_id,
        };
    }

    let mut table_report = LootReport::default();
    let mut table_id = None;
    if let Some(provided) = CatalogLootProvider::global().roll_block(state, context, random) {
        let unsupported = report_has_unsupported_payload(&provided.report);
        // A known table always wins unless it produced nothing and the empty
        // result is explained by an unsupported condition/entry. That is the
        // documented fallback case.
        if !unsupported || !provided.items.is_empty() {
            return BlockDrops {
                items: provided.items,
                experience,
                requires_correct_tool,
                had_correct_tool,
                source: DropSource::Table,
                report: provided.report,
                loot_table: provided.table_id,
            };
        }
        table_report = provided.report;
        table_id = provided.table_id;
    }

    if let Some(entry) = table_entry(&state.name) {
        return BlockDrops {
            items: vec![ItemStack::new(entry.item, entry.count)],
            experience: entry.experience,
            requires_correct_tool,
            had_correct_tool,
            source: DropSource::Table,
            report: table_report,
            loot_table: table_id,
        };
    }

    if let Some(family) = family_drop(&state.name, context.tool) {
        let (items, source) = match family {
            FamilyDrop::Nothing => (Vec::new(), DropSource::None),
            FamilyDrop::SelfState => (
                vec![ItemStack::new(state.name.clone(), 1)],
                DropSource::Family,
            ),
            FamilyDrop::Item(item) => (vec![ItemStack::new(item, 1)], DropSource::Family),
        };
        return BlockDrops {
            items,
            experience,
            requires_correct_tool,
            had_correct_tool,
            source,
            report: table_report,
            loot_table: table_id,
        };
    }

    if crate::registry::item_id(&state.name).is_some() {
        return BlockDrops {
            items: vec![ItemStack::new(state.name.clone(), 1)],
            experience,
            requires_correct_tool,
            had_correct_tool,
            source: DropSource::SelfDrop,
            report: table_report,
            loot_table: table_id,
        };
    }

    BlockDrops {
        items: Vec::new(),
        experience,
        requires_correct_tool,
        had_correct_tool,
        source: DropSource::None,
        report: table_report,
        loot_table: table_id,
    }
}

fn report_has_unsupported_payload(report: &LootReport) -> bool {
    !report.unsupported_conditions.is_empty()
        || !report.unsupported_functions.is_empty()
        || !report.unsupported_entries.is_empty()
        || !report.unsupported_number_providers.is_empty()
        || report.recursion_guard
}

/// Deterministic seed for the legacy `block_drops` signature. The live join
/// loop has no loot RNG stream yet; this keeps the legacy path reproducible
/// until it passes a `Level.random`-equivalent through
/// [`block_drops_with_context`].
fn default_drop_seed(state: &BlockState, held: Option<&ItemStack>) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    fn mix(mut hash: u64, bytes: &[u8]) -> u64 {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    let mut hash = FNV_OFFSET;
    hash = mix(hash, state.name.as_bytes());
    for (name, value) in &state.properties {
        hash = mix(hash, name.as_bytes());
        hash = mix(hash, b"=");
        hash = mix(hash, value.as_bytes());
    }
    if let Some(held) = held {
        hash = mix(hash, held.item.as_bytes());
        hash = mix(hash, &held.count.to_le_bytes());
        if let Some(ComponentValue::Enchantments(enchantments)) =
            held.get_component("minecraft:enchantments")
        {
            for (enchantment, level) in enchantments {
                hash = mix(hash, enchantment.as_bytes());
                hash = mix(hash, &level.to_le_bytes());
            }
        }
    }
    hash as i64
}

/// Vanilla `Properties.requiresCorrectToolForDrops()` blocks, extracted from
/// the 1.21.11 `Blocks.java` registrations (direct calls plus
/// `ofLegacyCopy`/`ofFullCopy`/`registerLegacyStair` inheritance). Blocks
/// registered through helper methods are additionally covered by the
/// `needs_*_tool` tags in [`requires_correct_tool_for_drops`].
#[rustfmt::skip]
const REQUIRES_CORRECT_TOOL: &str = "\
minecraft:amethyst_block minecraft:ancient_debris minecraft:andesite minecraft:andesite_slab minecraft:andesite_stairs minecraft:andesite_wall minecraft:anvil minecraft:basalt minecraft:black_concrete minecraft:black_glazed_terracotta minecraft:black_terracotta minecraft:blackstone minecraft:blackstone_slab minecraft:blackstone_stairs minecraft:blackstone_wall minecraft:blast_furnace minecraft:blue_concrete minecraft:blue_glazed_terracotta minecraft:blue_terracotta minecraft:bone_block minecraft:brain_coral_block minecraft:brick_slab minecraft:brick_stairs minecraft:brick_wall minecraft:bricks minecraft:brown_concrete minecraft:brown_glazed_terracotta minecraft:brown_terracotta minecraft:bubble_coral_block minecraft:budding_amethyst minecraft:calcite minecraft:cauldron minecraft:chain_command_block minecraft:chipped_anvil minecraft:chiseled_copper minecraft:chiseled_deepslate minecraft:chiseled_nether_bricks minecraft:chiseled_polished_blackstone minecraft:chiseled_quartz_block minecraft:chiseled_red_sandstone minecraft:chiseled_resin_bricks minecraft:chiseled_sandstone minecraft:chiseled_stone_bricks minecraft:chiseled_tuff minecraft:chiseled_tuff_bricks minecraft:coal_block minecraft:coal_ore minecraft:cobbled_deepslate minecraft:cobbled_deepslate_slab minecraft:cobbled_deepslate_stairs minecraft:cobbled_deepslate_wall minecraft:cobblestone minecraft:cobblestone_slab minecraft:cobblestone_stairs minecraft:cobblestone_wall minecraft:cobweb minecraft:command_block minecraft:copper_block minecraft:copper_bulb minecraft:copper_chest minecraft:copper_grate minecraft:copper_ore minecraft:copper_trapdoor minecraft:cracked_deepslate_bricks minecraft:cracked_deepslate_tiles minecraft:cracked_nether_bricks minecraft:cracked_polished_blackstone_bricks minecraft:cracked_stone_bricks minecraft:crimson_nylium minecraft:crying_obsidian minecraft:cut_copper minecraft:cut_copper_slab minecraft:cut_copper_stairs minecraft:cut_red_sandstone minecraft:cut_red_sandstone_slab minecraft:cut_sandstone minecraft:cut_sandstone_slab minecraft:cyan_concrete minecraft:cyan_glazed_terracotta minecraft:cyan_terracotta minecraft:damaged_anvil minecraft:dark_prismarine minecraft:dark_prismarine_slab minecraft:dark_prismarine_stairs minecraft:dead_brain_coral minecraft:dead_brain_coral_block minecraft:dead_brain_coral_fan minecraft:dead_brain_coral_wall_fan minecraft:dead_bubble_coral minecraft:dead_bubble_coral_block minecraft:dead_bubble_coral_fan minecraft:dead_bubble_coral_wall_fan minecraft:dead_fire_coral minecraft:dead_fire_coral_block minecraft:dead_fire_coral_fan minecraft:dead_fire_coral_wall_fan minecraft:dead_horn_coral minecraft:dead_horn_coral_block minecraft:dead_horn_coral_fan minecraft:dead_horn_coral_wall_fan minecraft:dead_tube_coral minecraft:dead_tube_coral_block minecraft:dead_tube_coral_fan minecraft:dead_tube_coral_wall_fan minecraft:deepslate minecraft:deepslate_brick_slab minecraft:deepslate_brick_stairs minecraft:deepslate_brick_wall minecraft:deepslate_bricks minecraft:deepslate_coal_ore minecraft:deepslate_copper_ore minecraft:deepslate_diamond_ore minecraft:deepslate_emerald_ore minecraft:deepslate_gold_ore minecraft:deepslate_iron_ore minecraft:deepslate_lapis_ore minecraft:deepslate_redstone_ore minecraft:deepslate_tile_slab minecraft:deepslate_tile_stairs minecraft:deepslate_tile_wall minecraft:deepslate_tiles minecraft:diamond_block minecraft:diamond_ore minecraft:diorite minecraft:diorite_slab minecraft:diorite_stairs minecraft:diorite_wall minecraft:dispenser minecraft:dripstone_block minecraft:dropper minecraft:emerald_block minecraft:emerald_ore minecraft:enchanting_table minecraft:end_stone minecraft:end_stone_brick_slab minecraft:end_stone_brick_stairs minecraft:end_stone_brick_wall minecraft:end_stone_bricks minecraft:exposed_chiseled_copper minecraft:exposed_copper minecraft:exposed_copper_bulb minecraft:exposed_copper_chest minecraft:exposed_copper_grate minecraft:exposed_copper_trapdoor minecraft:exposed_cut_copper minecraft:exposed_cut_copper_slab minecraft:exposed_cut_copper_stairs minecraft:exposed_lightning_rod minecraft:fire_coral_block minecraft:furnace minecraft:gilded_blackstone minecraft:gold_block minecraft:gold_ore minecraft:granite minecraft:granite_slab minecraft:granite_stairs minecraft:granite_wall minecraft:gray_concrete minecraft:gray_glazed_terracotta minecraft:gray_terracotta minecraft:green_concrete minecraft:green_glazed_terracotta minecraft:green_terracotta minecraft:grindstone minecraft:hopper minecraft:horn_coral_block minecraft:iron_bars minecraft:iron_block minecraft:iron_chain minecraft:iron_ore minecraft:iron_trapdoor minecraft:jigsaw minecraft:lapis_block minecraft:lapis_ore minecraft:lava_cauldron minecraft:light_blue_concrete minecraft:light_blue_glazed_terracotta minecraft:light_blue_terracotta minecraft:light_gray_concrete minecraft:light_gray_glazed_terracotta minecraft:light_gray_terracotta minecraft:lightning_rod minecraft:lime_concrete minecraft:lime_glazed_terracotta minecraft:lime_terracotta minecraft:lodestone minecraft:magenta_concrete minecraft:magenta_glazed_terracotta minecraft:magenta_terracotta minecraft:magma_block minecraft:mossy_cobblestone minecraft:mossy_cobblestone_slab minecraft:mossy_cobblestone_stairs minecraft:mossy_cobblestone_wall minecraft:mossy_stone_brick_slab minecraft:mossy_stone_brick_stairs minecraft:mossy_stone_brick_wall minecraft:mossy_stone_bricks minecraft:mud_brick_slab minecraft:mud_brick_stairs minecraft:mud_brick_wall minecraft:mud_bricks minecraft:nether_brick_fence minecraft:nether_brick_slab minecraft:nether_brick_stairs minecraft:nether_brick_wall minecraft:nether_bricks minecraft:nether_gold_ore minecraft:nether_quartz_ore minecraft:netherite_block minecraft:netherrack minecraft:observer minecraft:obsidian minecraft:orange_concrete minecraft:orange_glazed_terracotta minecraft:orange_terracotta minecraft:oxidized_chiseled_copper minecraft:oxidized_copper minecraft:oxidized_copper_bulb minecraft:oxidized_copper_chest minecraft:oxidized_copper_grate minecraft:oxidized_copper_trapdoor minecraft:oxidized_cut_copper minecraft:oxidized_cut_copper_slab minecraft:oxidized_cut_copper_stairs minecraft:oxidized_lightning_rod minecraft:petrified_oak_slab minecraft:pink_concrete minecraft:pink_glazed_terracotta minecraft:pink_terracotta minecraft:polished_andesite minecraft:polished_andesite_slab minecraft:polished_andesite_stairs minecraft:polished_basalt minecraft:polished_blackstone minecraft:polished_blackstone_brick_slab minecraft:polished_blackstone_brick_stairs minecraft:polished_blackstone_brick_wall minecraft:polished_blackstone_bricks minecraft:polished_blackstone_slab minecraft:polished_blackstone_stairs minecraft:polished_blackstone_wall minecraft:polished_deepslate minecraft:polished_deepslate_slab minecraft:polished_deepslate_stairs minecraft:polished_deepslate_wall minecraft:polished_diorite minecraft:polished_diorite_slab minecraft:polished_diorite_stairs minecraft:polished_granite minecraft:polished_granite_slab minecraft:polished_granite_stairs minecraft:polished_tuff minecraft:polished_tuff_slab minecraft:polished_tuff_stairs minecraft:polished_tuff_wall minecraft:powder_snow_cauldron minecraft:prismarine minecraft:prismarine_brick_slab minecraft:prismarine_brick_stairs minecraft:prismarine_bricks minecraft:prismarine_slab minecraft:prismarine_stairs minecraft:prismarine_wall minecraft:purple_concrete minecraft:purple_glazed_terracotta minecraft:purple_terracotta minecraft:purpur_block minecraft:purpur_pillar minecraft:purpur_slab minecraft:purpur_stairs minecraft:quartz_block minecraft:quartz_bricks minecraft:quartz_pillar minecraft:quartz_slab minecraft:quartz_stairs minecraft:raw_copper_block minecraft:raw_gold_block minecraft:raw_iron_block minecraft:red_concrete minecraft:red_glazed_terracotta minecraft:red_nether_brick_slab minecraft:red_nether_brick_stairs minecraft:red_nether_brick_wall minecraft:red_nether_bricks minecraft:red_sandstone minecraft:red_sandstone_slab minecraft:red_sandstone_stairs minecraft:red_sandstone_wall minecraft:red_terracotta minecraft:redstone_block minecraft:redstone_ore minecraft:repeating_command_block minecraft:resin_brick_slab minecraft:resin_brick_stairs minecraft:resin_brick_wall minecraft:resin_bricks minecraft:respawn_anchor minecraft:sandstone minecraft:sandstone_slab minecraft:sandstone_stairs minecraft:sandstone_wall minecraft:smoker minecraft:smooth_basalt minecraft:smooth_quartz minecraft:smooth_quartz_slab minecraft:smooth_quartz_stairs minecraft:smooth_red_sandstone minecraft:smooth_red_sandstone_slab minecraft:smooth_red_sandstone_stairs minecraft:smooth_sandstone minecraft:smooth_sandstone_slab minecraft:smooth_sandstone_stairs minecraft:smooth_stone minecraft:smooth_stone_slab minecraft:snow minecraft:snow_block minecraft:spawner minecraft:stone minecraft:stone_brick_slab minecraft:stone_brick_stairs minecraft:stone_brick_wall minecraft:stone_bricks minecraft:stone_slab minecraft:stone_stairs minecraft:stonecutter minecraft:structure_block minecraft:terracotta minecraft:tube_coral_block minecraft:tuff minecraft:tuff_brick_slab minecraft:tuff_brick_stairs minecraft:tuff_brick_wall minecraft:tuff_bricks minecraft:tuff_slab minecraft:tuff_stairs minecraft:tuff_wall minecraft:warped_nylium minecraft:water_cauldron minecraft:waxed_chiseled_copper minecraft:waxed_copper_block minecraft:waxed_copper_bulb minecraft:waxed_copper_chest minecraft:waxed_copper_grate minecraft:waxed_copper_trapdoor minecraft:waxed_cut_copper minecraft:waxed_cut_copper_slab minecraft:waxed_cut_copper_stairs minecraft:waxed_exposed_chiseled_copper minecraft:waxed_exposed_copper minecraft:waxed_exposed_copper_bulb minecraft:waxed_exposed_copper_chest minecraft:waxed_exposed_copper_grate minecraft:waxed_exposed_copper_trapdoor minecraft:waxed_exposed_cut_copper minecraft:waxed_exposed_cut_copper_slab minecraft:waxed_exposed_cut_copper_stairs minecraft:waxed_exposed_lightning_rod minecraft:waxed_lightning_rod minecraft:waxed_oxidized_chiseled_copper minecraft:waxed_oxidized_copper minecraft:waxed_oxidized_copper_bulb minecraft:waxed_oxidized_copper_chest minecraft:waxed_oxidized_copper_grate minecraft:waxed_oxidized_copper_trapdoor minecraft:waxed_oxidized_cut_copper minecraft:waxed_oxidized_cut_copper_slab minecraft:waxed_oxidized_cut_copper_stairs minecraft:waxed_oxidized_lightning_rod minecraft:waxed_weathered_chiseled_copper minecraft:waxed_weathered_copper minecraft:waxed_weathered_copper_bulb minecraft:waxed_weathered_copper_chest minecraft:waxed_weathered_copper_grate minecraft:waxed_weathered_copper_trapdoor minecraft:waxed_weathered_cut_copper minecraft:waxed_weathered_cut_copper_slab minecraft:waxed_weathered_cut_copper_stairs minecraft:waxed_weathered_lightning_rod minecraft:weathered_chiseled_copper minecraft:weathered_copper minecraft:weathered_copper_bulb minecraft:weathered_copper_chest minecraft:weathered_copper_grate minecraft:weathered_copper_trapdoor minecraft:weathered_cut_copper minecraft:weathered_cut_copper_slab minecraft:weathered_cut_copper_stairs minecraft:weathered_lightning_rod minecraft:white_concrete minecraft:white_glazed_terracotta minecraft:white_terracotta minecraft:yellow_concrete minecraft:yellow_glazed_terracotta minecraft:yellow_terracotta";

fn requires_tool_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| REQUIRES_CORRECT_TOOL.split_whitespace().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    #[test]
    fn tool_materials_match_vanilla() {
        let wooden_pickaxe = builtin_tool_profile("minecraft:wooden_pickaxe").unwrap();
        assert_eq!(wooden_pickaxe.kind, ToolKind::Pickaxe);
        assert_eq!(wooden_pickaxe.tier, Some(ToolTier::Wooden));
        assert_eq!(wooden_pickaxe.speed, 2.0);
        assert_eq!(wooden_pickaxe.max_damage, 59);

        let diamond_pickaxe = builtin_tool_profile("minecraft:diamond_pickaxe").unwrap();
        assert_eq!(diamond_pickaxe.speed, 8.0);
        assert_eq!(diamond_pickaxe.max_damage, 1_561);

        let netherite_axe = builtin_tool_profile("minecraft:netherite_axe").unwrap();
        assert_eq!(netherite_axe.kind, ToolKind::Axe);
        assert_eq!(netherite_axe.speed, 9.0);
        assert_eq!(netherite_axe.max_damage, 2_031);

        let shears = builtin_tool_profile("minecraft:shears").unwrap();
        assert_eq!(shears.kind, ToolKind::Shears);
        assert_eq!(shears.max_damage, 238);

        assert!(builtin_tool_profile("minecraft:stick").is_none());
        assert!(builtin_tool_profile("minecraft:wooden_sword").is_some());
    }

    #[test]
    fn correct_tool_uses_mineable_and_incorrect_tags() {
        let stone = state("minecraft:stone");
        let obsidian = state("minecraft:obsidian");
        let dirt = state("minecraft:dirt");
        let wooden = ItemStack::new("minecraft:wooden_pickaxe", 1);
        let stone_pick = ItemStack::new("minecraft:stone_pickaxe", 1);
        let diamond_pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        let shovel = ItemStack::new("minecraft:iron_shovel", 1);

        assert!(has_correct_tool_for_drops(&stone, Some(&wooden)));
        assert!(!has_correct_tool_for_drops(&stone, None));
        assert!(!has_correct_tool_for_drops(&obsidian, Some(&stone_pick)));
        assert!(has_correct_tool_for_drops(&obsidian, Some(&diamond_pick)));
        assert!(has_correct_tool_for_drops(&dirt, None));
        assert!(has_correct_tool_for_drops(&dirt, Some(&shovel)));
        // Dirt is in mineable/shovel, so the shovel counts as a correct tool
        // for speed even though dirt does not require one.
        assert!(is_correct_tool_for_drops(&dirt, Some(&shovel)));
        assert!(
            break_ticks(&dirt, Some(&shovel), &MiningContext::default())
                < break_ticks(&dirt, None, &MiningContext::default())
        );
    }

    #[test]
    fn break_times_match_java_formula() {
        let stone = state("minecraft:stone");
        let dirt = state("minecraft:dirt");
        let obsidian = state("minecraft:obsidian");
        let ctx = MiningContext::default();

        // Stone, hand: 1 / 1.5 / 100.
        let hand = break_ticks(&stone, None, &ctx);
        assert!((hand - 150.0).abs() < 1e-3, "stone hand {hand}");

        // Stone, wooden pickaxe: 2 / 1.5 / 30.
        let wooden = ItemStack::new("minecraft:wooden_pickaxe", 1);
        let wood_ticks = break_ticks(&stone, Some(&wooden), &ctx);
        assert!((wood_ticks - 22.5).abs() < 1e-3, "stone wood {wood_ticks}");

        // Dirt, hand: 1 / 0.5 / 30.
        assert!((break_ticks(&dirt, None, &ctx) - 15.0).abs() < 1e-3);

        // Obsidian, stone pickaxe: wrong tier -> /100; diamond -> /30.
        let stone_pick = ItemStack::new("minecraft:stone_pickaxe", 1);
        let diamond_pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        let wrong = break_ticks(&obsidian, Some(&stone_pick), &ctx);
        let right = break_ticks(&obsidian, Some(&diamond_pick), &ctx);
        assert!((wrong - 1_250.0).abs() < 1e-2, "obsidian stone {wrong}");
        assert!((right - 187.5).abs() < 1e-2, "obsidian diamond {right}");
    }

    #[test]
    fn efficiency_haste_fatigue_and_air_penalties_apply() {
        let stone = state("minecraft:stone");
        let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        pick.set_component(ComponentValue::Enchantments(
            std::collections::BTreeMap::from([("minecraft:efficiency".to_owned(), 3)]),
        ));
        let base = player_destroy_speed(&stone, Some(&pick), &MiningContext::default());
        assert!((base - 18.0).abs() < 1e-3); // 8 + 3^2 + 1

        let mut ctx = MiningContext {
            haste_amplifier: Some(1),
            ..MiningContext::default()
        };
        let hasted = player_destroy_speed(&stone, Some(&pick), &ctx);
        assert!((hasted - 18.0 * 1.4).abs() < 1e-3);

        ctx = MiningContext {
            mining_fatigue_amplifier: Some(0),
            ..MiningContext::default()
        };
        let fatigued = player_destroy_speed(&stone, Some(&pick), &ctx);
        assert!((fatigued - 18.0 * 0.3).abs() < 1e-3);

        ctx = MiningContext {
            on_ground: false,
            ..MiningContext::default()
        };
        assert!((player_destroy_speed(&stone, Some(&pick), &ctx) - 18.0 / 5.0).abs() < 1e-3);

        ctx = MiningContext {
            eye_in_water: true,
            ..MiningContext::default()
        };
        assert!((player_destroy_speed(&stone, Some(&pick), &ctx) - 18.0 * 0.2).abs() < 1e-3);
    }

    #[test]
    fn sword_and_shears_special_rules() {
        let cobweb = state("minecraft:cobweb");
        let leaves = state("minecraft:oak_leaves");
        let wool = state("minecraft:white_wool");
        let shears = ItemStack::new("minecraft:shears", 1);
        let sword = ItemStack::new("minecraft:iron_sword", 1);

        assert!(is_correct_tool_for_drops(&cobweb, Some(&shears)));
        assert!(is_correct_tool_for_drops(&cobweb, Some(&sword)));
        assert!(!is_correct_tool_for_drops(&cobweb, None));
        assert!(requires_correct_tool_for_drops(&cobweb));

        assert!((mining_speed(&leaves, Some(&shears)) - 15.0).abs() < 1e-3);
        assert!((mining_speed(&wool, Some(&shears)) - 5.0).abs() < 1e-3);
        // Leaves do not require a tool, so shears still harvest them; a sword
        // does not (its only correct-for-drops rule is cobweb).
        assert!(has_correct_tool_for_drops(&leaves, Some(&shears)));
        assert!(has_correct_tool_for_drops(&leaves, Some(&sword)));
    }

    #[test]
    fn dig_state_uses_start_elapsed_and_the_stop_threshold() {
        let stone = state("minecraft:stone");
        let pick = ItemStack::new("minecraft:wooden_pickaxe", 1);
        let ctx = MiningContext::default();
        let pos = BlockPos::new(1, 2, 3);
        let mut dig = DigState::default();

        assert_eq!(
            dig.start(pos, false, &stone, Some(&pick), &ctx, 100),
            DigStart::Started
        );
        assert_eq!(dig.active_target(), Some(pos));

        // 10 ticks = 0.0444 * 11 = 0.489 < 0.7 -> delayed.
        assert_eq!(
            dig.stop(pos, &stone, Some(&pick), &ctx, 110),
            DigStop::Delayed
        );
        assert_eq!(dig.active_target(), None);
        assert_eq!(dig.delayed_target(), Some(pos));

        // 22 ticks = 0.0444 * 23 = 1.02 >= 1.0 -> destroyed on the next tick.
        assert!(dig.tick_delayed(&stone, Some(&pick), &ctx, 122));
        assert_eq!(dig.delayed_target(), None);
    }

    #[test]
    fn dig_state_stop_destroys_at_vanilla_seven_tenths() {
        let stone = state("minecraft:stone");
        let pick = ItemStack::new("minecraft:wooden_pickaxe", 1);
        let ctx = MiningContext::default();
        let pos = BlockPos::new(0, 64, 0);
        let mut dig = DigState::default();
        assert_eq!(
            dig.start(pos, false, &stone, Some(&pick), &ctx, 0),
            DigStart::Started
        );
        // 18 ticks -> 0.0444 * 19 = 0.844 >= 0.7.
        assert_eq!(
            dig.stop(pos, &stone, Some(&pick), &ctx, 18),
            DigStop::Destroyed
        );
    }

    #[test]
    fn instant_break_in_creative_and_zero_hardness() {
        let stone = state("minecraft:stone");
        let flower = state("minecraft:dandelion");
        let ctx = MiningContext::default();
        let mut dig = DigState::default();
        assert_eq!(
            dig.start(BlockPos::new(0, 64, 0), true, &stone, None, &ctx, 0),
            DigStart::Instant
        );
        assert_eq!(dig.active_target(), None);
        assert_eq!(
            dig.start(BlockPos::new(0, 64, 0), false, &flower, None, &ctx, 0),
            DigStart::Instant
        );
        assert_eq!(
            dig.start(
                BlockPos::new(0, 64, 0),
                false,
                &state("minecraft:bedrock"),
                Some(&ItemStack::new("minecraft:netherite_pickaxe", 1)),
                &ctx,
                0
            ),
            DigStart::Rejected
        );
    }

    #[test]
    fn abort_clears_and_reports_a_mismatched_target() {
        let stone = state("minecraft:stone");
        let ctx = MiningContext::default();
        let pos = BlockPos::new(4, 5, 6);
        let mut dig = DigState::default();
        assert_eq!(
            dig.start(pos, false, &stone, None, &ctx, 0),
            DigStart::Started
        );
        assert_eq!(dig.abort(BlockPos::new(9, 9, 9)), Some(pos));
        assert_eq!(dig.active_target(), None);
        assert_eq!(dig.abort(pos), None);
    }

    #[test]
    fn drops_are_gated_by_tool_and_count_is_deterministic() {
        let stone = state("minecraft:stone");
        let diamond_ore = state("minecraft:diamond_ore");
        let redstone_ore = state("minecraft:redstone_ore");
        let clay = state("minecraft:clay");
        let dirt = state("minecraft:dirt");
        let hand_stone = block_drops(&stone, None, None);
        assert!(hand_stone.is_empty());
        assert!(hand_stone.requires_correct_tool);
        assert!(!hand_stone.had_correct_tool);

        let pick = ItemStack::new("minecraft:iron_pickaxe", 1);
        let stone_drops = block_drops(&stone, Some(&pick), None);
        assert_eq!(stone_drops.item_count(), 1);
        assert_eq!(stone_drops.items[0].item, "minecraft:cobblestone");
        assert_eq!(stone_drops.source, DropSource::Table);

        let stone_pick = ItemStack::new("minecraft:stone_pickaxe", 1);
        let diamond_pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        assert!(block_drops(&diamond_ore, Some(&stone_pick), None).is_empty());
        assert_eq!(
            block_drops(&diamond_ore, Some(&diamond_pick), None).items[0].item,
            "minecraft:diamond"
        );
        assert_eq!(
            block_drops(&redstone_ore, Some(&pick), None).item_count(),
            4
        );
        assert_eq!(block_drops(&clay, Some(&pick), None).item_count(), 4);
        // Dirt does not require a tool; bare hand still drops it.
        assert_eq!(block_drops(&dirt, None, None).item_count(), 1);
    }

    #[test]
    fn leaves_glass_and_ice_do_not_self_drop() {
        let leaves = state("minecraft:oak_leaves");
        let glass = state("minecraft:glass");
        let ice = state("minecraft:ice");
        assert!(block_drops(&leaves, None, None).is_empty());
        assert!(block_drops(&glass, None, None).is_empty());
        assert!(block_drops(&ice, None, None).is_empty());
        let shears = ItemStack::new("minecraft:shears", 1);
        assert_eq!(
            block_drops(&leaves, Some(&shears), None).items[0].item,
            "minecraft:oak_leaves"
        );
    }

    #[test]
    fn durability_wears_and_breaks_tools() {
        let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        let wear = apply_block_durability(&mut pick, false, 1.5);
        assert!(wear.changed());
        assert_eq!(wear.damage, 1);
        assert_eq!(pick.damage(), 1);
        assert_eq!(pick.max_damage(), 1_561);

        // Creative and instant-break blocks do not wear.
        let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        assert!(!apply_block_durability(&mut pick, true, 1.5).changed());
        assert!(!apply_block_durability(&mut pick, false, 0.0).changed());

        // A nearly-broken tool breaks on the next use.
        let mut pick = ItemStack::new("minecraft:diamond_pickaxe", 1);
        pick.set_component(ComponentValue::Damage(1_560));
        pick.set_component(ComponentValue::MaxDamage(1_561));
        let wear = apply_block_durability(&mut pick, false, 1.5);
        assert!(wear.broke);
        assert!(pick.is_empty());

        // Non-tools never wear.
        let mut stick = ItemStack::new("minecraft:stick", 64);
        assert!(!apply_block_durability(&mut stick, false, 1.5).changed());
        assert_eq!(stick.count, 64);
    }

    #[test]
    fn requires_correct_tool_set_matches_known_blocks() {
        assert!(requires_correct_tool_for_drops(&state("minecraft:stone")));
        assert!(requires_correct_tool_for_drops(&state(
            "minecraft:obsidian"
        )));
        assert!(requires_correct_tool_for_drops(&state(
            "minecraft:diamond_ore"
        )));
        assert!(requires_correct_tool_for_drops(&state("minecraft:cobweb")));
        assert!(requires_correct_tool_for_drops(&state(
            "minecraft:stone_stairs"
        )));
        assert!(!requires_correct_tool_for_drops(&state("minecraft:dirt")));
        assert!(!requires_correct_tool_for_drops(&state(
            "minecraft:oak_planks"
        )));
        assert!(!requires_correct_tool_for_drops(&state(
            "minecraft:stone_button"
        )));
    }
}
