//! Behavioral fixtures for vanilla block-breaking progress, tool rules,
//! drops, and durability.
//!
//! The expected numbers come from the Java formulas:
//!
//! * `BlockBehaviour.getDestroyProgress`:
//!   `player.getDestroySpeed(state) / state.getDestroySpeed(level, pos) / (hasCorrectToolForDrops ? 30 : 100)`
//! * `Player.getDestroySpeed` tool speed, efficiency `level^2 + 1`, haste
//!   `1 + (amplifier + 1) * 0.2`, mining fatigue `0.3/0.09/0.0027/0.00081`,
//!   water `*0.2`, airborne `/5`.
//! * `ToolMaterial` speeds and durabilities.

use folia_core::block_breaking::{self, DigStart, DigState, DigStop, DropSource, MiningContext};
use folia_core::chunk::BlockState;
use folia_core::inventory::ItemStack;
use folia_core::math::BlockPos;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn stack(item: &str) -> ItemStack {
    ItemStack::new(item, 1)
}

fn context() -> MiningContext {
    MiningContext::default()
}

#[test]
fn stone_is_slow_by_hand_and_fast_with_a_pickaxe() {
    let stone = state("minecraft:stone");
    let ctx = context();
    let hand = block_breaking::break_ticks(&stone, None, &ctx);
    let wooden = stack("minecraft:wooden_pickaxe");
    let pick_ticks = block_breaking::break_ticks(&stone, Some(&wooden), &ctx);

    // Hand: requires the correct tool, so the divisor is 100.
    assert!((hand - 150.0).abs() < 1e-3, "hand={hand}");
    // Wooden pickaxe: speed 2, correct tool, divisor 30.
    assert!((pick_ticks - 22.5).abs() < 1e-3, "pick={pick_ticks}");
    assert!(pick_ticks < hand);

    assert!(!block_breaking::has_correct_tool_for_drops(&stone, None));
    assert!(block_breaking::has_correct_tool_for_drops(
        &stone,
        Some(&wooden)
    ));
}

#[test]
fn dirt_drops_by_hand_and_a_shovel_is_faster() {
    let dirt = state("minecraft:dirt");
    let ctx = context();
    let hand = block_breaking::break_ticks(&dirt, None, &ctx);
    let shovel = stack("minecraft:iron_shovel");
    let shovel_ticks = block_breaking::break_ticks(&dirt, Some(&shovel), &ctx);

    // Dirt hardness 0.5, hand speed 1: 1 / 0.5 / 30 = 15 ticks.
    assert!((hand - 15.0).abs() < 1e-3, "hand={hand}");
    // Iron shovel speed 6: 6 / 0.5 / 30 = 2.5 ticks.
    assert!((shovel_ticks - 2.5).abs() < 1e-3, "shovel={shovel_ticks}");

    let drops = block_breaking::block_drops(&dirt, None, None);
    assert_eq!(drops.item_count(), 1);
    assert_eq!(drops.items[0].item, "minecraft:dirt");
    assert!(!drops.requires_correct_tool);
}

#[test]
fn obsidian_requires_a_diamond_tier_pickaxe() {
    let obsidian = state("minecraft:obsidian");
    let ctx = context();
    let stone = stack("minecraft:stone_pickaxe");
    let iron = stack("minecraft:iron_pickaxe");
    let diamond = stack("minecraft:diamond_pickaxe");

    assert!(!block_breaking::is_correct_tool_for_drops(
        &obsidian,
        Some(&stone)
    ));
    assert!(!block_breaking::is_correct_tool_for_drops(
        &obsidian,
        Some(&iron)
    ));
    assert!(block_breaking::is_correct_tool_for_drops(
        &obsidian,
        Some(&diamond)
    ));

    assert!(block_breaking::block_drops(&obsidian, Some(&stone), None).is_empty());
    assert!(!block_breaking::block_drops(&obsidian, Some(&diamond), None).is_empty());

    // Wrong tier uses the /100 divisor, correct tier /30.
    let wrong = block_breaking::break_ticks(&obsidian, Some(&stone), &ctx);
    let right = block_breaking::break_ticks(&obsidian, Some(&diamond), &ctx);
    assert!((wrong - 1_250.0).abs() < 1e-2, "wrong={wrong}");
    assert!((right - 187.5).abs() < 1e-2, "right={right}");
}

#[test]
fn creative_breaks_instantly_and_never_wears_tools() {
    let stone = state("minecraft:stone");
    let ctx = context();
    let mut dig = DigState::default();
    let pos = BlockPos::new(10, 64, -10);

    assert_eq!(
        dig.start(pos, true, &stone, None, &ctx, 0),
        DigStart::Instant
    );
    assert_eq!(dig.active_target(), None);

    let mut pick = stack("minecraft:diamond_pickaxe");
    let wear = block_breaking::apply_block_durability(&mut pick, true, 1.5);
    assert!(!wear.changed());
    assert_eq!(pick.damage(), 0);
    assert_eq!(pick.count, 1);
}

#[test]
fn durability_decrements_and_a_tool_breaks_at_its_limit() {
    let mut pick = stack("minecraft:diamond_pickaxe");
    let wear = block_breaking::apply_block_durability(&mut pick, false, 1.5);
    assert!(wear.changed());
    assert_eq!(wear.damage, 1);
    assert_eq!(pick.damage(), 1);
    // Prototype defaults are not dumped yet, so the resolved maximum is
    // written with the first damage point.
    assert_eq!(pick.max_damage(), 1_561);
    assert_eq!(pick.count, 1);

    pick.set_component(folia_core::item_components::ComponentValue::Damage(1_560));
    let wear = block_breaking::apply_block_durability(&mut pick, false, 1.5);
    assert!(wear.broke);
    assert!(pick.is_empty());

    // Zero-hardness blocks (flowers, torches, ...) never wear a tool.
    let mut pick = stack("minecraft:diamond_pickaxe");
    assert!(!block_breaking::apply_block_durability(&mut pick, false, 0.0).changed());
}

#[test]
fn drop_counts_are_deterministic_and_tool_gated() {
    let stone = state("minecraft:stone");
    let redstone = state("minecraft:redstone_ore");
    let clay = state("minecraft:clay");
    let diamond_ore = state("minecraft:diamond_ore");
    let iron_pick = stack("minecraft:iron_pickaxe");
    let stone_pick = stack("minecraft:stone_pickaxe");

    let cobble = block_breaking::block_drops(&stone, Some(&iron_pick), None);
    assert_eq!(cobble.source, DropSource::Table);
    assert_eq!(cobble.item_count(), 1);
    assert_eq!(cobble.items[0].item, "minecraft:cobblestone");

    assert_eq!(
        block_breaking::block_drops(&redstone, Some(&iron_pick), None).item_count(),
        4
    );
    assert_eq!(
        block_breaking::block_drops(&clay, Some(&iron_pick), None).item_count(),
        4
    );
    assert!(block_breaking::block_drops(&diamond_ore, Some(&stone_pick), None).is_empty());

    // The formula's tool gate is part of the drop result.
    let hand = block_breaking::block_drops(&stone, None, None);
    assert!(hand.is_empty());
    assert!(hand.requires_correct_tool);
    assert!(!hand.had_correct_tool);
}

#[test]
fn dig_state_reaches_destroyed_and_abort_clears_it() {
    let stone = state("minecraft:stone");
    let ctx = context();
    let pick = stack("minecraft:wooden_pickaxe");
    let pos = BlockPos::new(3, 70, 3);
    let mut dig = DigState::default();

    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &ctx, 100),
        DigStart::Started
    );
    assert_eq!(dig.active_target(), Some(pos));

    // 22 elapsed ticks -> 0.0444 * 23 = 1.02, above the 0.7 stop threshold.
    assert_eq!(
        dig.stop(pos, &stone, Some(&pick), &ctx, 122),
        DigStop::Destroyed
    );
    assert_eq!(dig.active_target(), None);

    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &ctx, 130),
        DigStart::Started
    );
    assert_eq!(dig.abort(BlockPos::new(9, 9, 9)), Some(pos));
    assert_eq!(dig.active_target(), None);
}

#[test]
fn early_stop_uses_vanilla_delayed_destroy() {
    let stone = state("minecraft:stone");
    let ctx = context();
    let pick = stack("minecraft:wooden_pickaxe");
    let pos = BlockPos::new(0, 64, 0);
    let mut dig = DigState::default();

    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &ctx, 0),
        DigStart::Started
    );
    // 8 ticks -> 0.4 progress, below the 0.7 threshold.
    assert_eq!(
        dig.stop(pos, &stone, Some(&pick), &ctx, 8),
        DigStop::Delayed
    );
    assert_eq!(dig.delayed_target(), Some(pos));
    assert!(!dig.tick_delayed(&stone, Some(&pick), &ctx, 10));
    // 22 ticks -> progress >= 1.0.
    assert!(dig.tick_delayed(&stone, Some(&pick), &ctx, 22));
    assert_eq!(dig.delayed_target(), None);
}

#[test]
fn unbreakable_blocks_are_rejected_in_survival() {
    let bedrock = state("minecraft:bedrock");
    let pick = stack("minecraft:netherite_pickaxe");
    let mut dig = DigState::default();
    assert_eq!(
        dig.start(
            BlockPos::new(0, -64, 0),
            false,
            &bedrock,
            Some(&pick),
            &context(),
            0
        ),
        DigStart::Rejected
    );
    assert_eq!(
        block_breaking::break_ticks(&bedrock, Some(&pick), &context()),
        f32::INFINITY
    );
}
