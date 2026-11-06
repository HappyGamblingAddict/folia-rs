//! Differential-style fixtures for shipped loot-table block drops.
//!
//! The expected values come from the 1.21.11 block loot tables in
//! `assets/loot_tables.json`, not from the old deterministic fallback:
//!
//! * `blocks/stone` -> cobblestone (Silk Touch -> stone).
//! * `blocks/diamond_ore` -> 1 diamond, `ore_drops` Fortune multiplier,
//!   Silk Touch -> diamond ore.
//! * `blocks/redstone_ore` -> uniform 4..=5 redstone, `uniform_bonus_count`
//!   Fortune formula.
//! * `blocks/short_grass` -> 12.5% wheat seeds, `uniform_bonus_count`.
//! * `blocks/oak_leaves` -> saplings/sticks/apples by `table_bonus`, shears or
//!   Silk Touch -> the leaf block.
//!
//! RNG is explicit (`LegacyRandomSource`) so the assertions are stable.

use folia_core::block_breaking::{
    self, BlockLootContext, CatalogLootProvider, DigLootContext, DigStart, DigState, DropSource,
    LootProvider, MiningContext, ToolContext,
};
use folia_core::chunk::BlockState;
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::loot::LootCatalog;
use folia_core::math::BlockPos;
use folia_core::random::LegacyRandomSource;
use std::collections::BTreeMap;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.to_owned(),
        properties: Vec::new(),
    }
}

fn tool(item: &str) -> ItemStack {
    ItemStack::new(item, 1)
}

fn enchanted(item: &str, enchantment: &str, level: i32) -> ItemStack {
    let mut stack = tool(item);
    stack.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        enchantment.to_owned(),
        level,
    )])));
    stack
}

fn drops_from_seed(
    block: &BlockState,
    context: &BlockLootContext<'_>,
    provider: Option<&dyn LootProvider>,
    seed: i64,
) -> block_breaking::BlockDrops {
    let mut random = LegacyRandomSource::new(seed);
    block_breaking::block_drops_with_context(block, context, provider, &mut random)
}

/// Small integer seeds all share the same first LCG output bit pattern; the
/// shipped Java stream expects a proper `Level.random` value. Fan the fixture
/// indices out over the 64-bit space instead.
fn diverse_seed(index: i64) -> i64 {
    (index as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(17) as i64
}

#[test]
fn stone_requires_a_pickaxe_and_uses_the_shipped_table() {
    let stone = state("minecraft:stone");
    let pick = tool("minecraft:iron_pickaxe");
    let pick_context = BlockLootContext::from_tool(Some(&pick));

    let drops = drops_from_seed(&stone, &pick_context, None, 7);
    assert_eq!(drops.source, DropSource::Table);
    assert_eq!(drops.loot_table.as_deref(), Some("minecraft:blocks/stone"));
    assert_eq!(drops.items.len(), 1);
    assert_eq!(drops.items[0].item, "minecraft:cobblestone");
    assert_eq!(drops.items[0].count, 1);
    assert!(drops.report.is_empty());

    // Vanilla's harvest gate rejects a hand break before rolling the table.
    let hand = drops_from_seed(&stone, &BlockLootContext::default(), None, 7);
    assert!(hand.is_empty());
    assert!(hand.requires_correct_tool);
    assert!(!hand.had_correct_tool);
    assert_eq!(hand.source, DropSource::None);
    assert!(hand.loot_table.is_none());
}

#[test]
fn diamond_ore_silk_touch_and_fortune_come_from_the_table() {
    let ore = state("minecraft:diamond_ore");

    let plain_pick = tool("minecraft:diamond_pickaxe");
    let plain = drops_from_seed(
        &ore,
        &BlockLootContext::from_tool(Some(&plain_pick)),
        None,
        11,
    );
    assert_eq!(plain.items.len(), 1);
    assert_eq!(plain.items[0].item, "minecraft:diamond");
    assert_eq!(plain.items[0].count, 1);
    assert!(plain.report.is_empty());

    let silk_pick = enchanted("minecraft:diamond_pickaxe", "minecraft:silk_touch", 1);
    let silk = drops_from_seed(
        &ore,
        &BlockLootContext::from_tool(Some(&silk_pick)),
        None,
        11,
    );
    assert_eq!(silk.items.len(), 1);
    assert_eq!(silk.items[0].item, "minecraft:diamond_ore");
    assert_eq!(silk.items[0].count, 1);

    let fortune_pick = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    let mut saw_bonus = false;
    for index in 0..32 {
        let seed = diverse_seed(index);
        let drops = drops_from_seed(
            &ore,
            &BlockLootContext::from_tool(Some(&fortune_pick)),
            None,
            seed,
        );
        assert_eq!(drops.items.len(), 1);
        assert_eq!(drops.items[0].item, "minecraft:diamond");
        assert!((1..=4).contains(&drops.items[0].count));
        saw_bonus |= drops.items[0].count > 1;
    }
    assert!(saw_bonus, "Fortune III never produced a bonus diamond");

    // An explicit ToolContext works even when the stack has no enchantments.
    let bare_pick = tool("minecraft:diamond_pickaxe");
    let explicit = drops_from_seed(
        &ore,
        &BlockLootContext::from_tool(Some(&bare_pick)).with_tool_context(ToolContext::new(true, 0)),
        None,
        11,
    );
    assert_eq!(explicit.items[0].item, "minecraft:diamond_ore");
}

#[test]
fn redstone_ore_count_is_the_shipped_uniform_range() {
    let ore = state("minecraft:redstone_ore");
    let pick = tool("minecraft:iron_pickaxe");
    let context = BlockLootContext::from_tool(Some(&pick));

    let mut saw_four = false;
    let mut saw_five = false;
    for index in 0..64 {
        let seed = diverse_seed(index);
        let drops = drops_from_seed(&ore, &context, None, seed);
        assert_eq!(drops.items.len(), 1);
        assert_eq!(drops.items[0].item, "minecraft:redstone");
        assert!(
            (4..=5).contains(&drops.items[0].count),
            "seed {seed} produced {} redstone",
            drops.items[0].count
        );
        saw_four |= drops.items[0].count == 4;
        saw_five |= drops.items[0].count == 5;
    }
    assert!(
        saw_four && saw_five,
        "the 4..=5 uniform range was not sampled"
    );

    let fortune_pick = enchanted("minecraft:iron_pickaxe", "minecraft:fortune", 3);
    let fortune = drops_from_seed(
        &ore,
        &BlockLootContext::from_tool(Some(&fortune_pick)),
        None,
        2,
    );
    assert_eq!(fortune.items[0].item, "minecraft:redstone");
    assert!((4..=14).contains(&fortune.items[0].count));
}

#[test]
fn short_grass_seed_chance_scales_with_fortune() {
    let grass = state("minecraft:short_grass");
    let plain = BlockLootContext::default();
    let fortune = BlockLootContext::default().with_fortune(3);

    let mut plain_seeds = 0;
    let mut fortune_seeds = 0;
    for index in 0..200 {
        let seed = diverse_seed(index);
        let plain_drops = drops_from_seed(&grass, &plain, None, seed);
        let fortune_drops = drops_from_seed(&grass, &fortune, None, seed);
        for drops in [&plain_drops, &fortune_drops] {
            assert!(
                drops
                    .items
                    .iter()
                    .all(|stack| stack.item == "minecraft:wheat_seeds")
            );
        }
        plain_seeds += plain_drops.item_count();
        fortune_seeds += fortune_drops.item_count();
    }

    assert!(plain_seeds > 0, "short grass never dropped wheat seeds");
    assert!(
        fortune_seeds > plain_seeds,
        "Fortune III did not increase the seed count: plain={plain_seeds} fortune={fortune_seeds}"
    );

    // Shears skip the seed branch entirely and return the plant.
    let shears = tool("minecraft:shears");
    let sheared = drops_from_seed(&grass, &BlockLootContext::from_tool(Some(&shears)), None, 1);
    assert_eq!(sheared.items.len(), 1);
    assert_eq!(sheared.items[0].item, "minecraft:short_grass");
}

#[test]
fn leaves_sapling_chance_and_shear_self_drop() {
    let leaves = state("minecraft:oak_leaves");
    let plain = BlockLootContext::default();

    let mut saplings = 0;
    let mut sticks = 0;
    let mut apples = 0;
    for index in 0..400 {
        let seed = diverse_seed(index);
        let drops = drops_from_seed(&leaves, &plain, None, seed);
        for stack in &drops.items {
            match stack.item.as_str() {
                "minecraft:oak_sapling" => {
                    assert_eq!(stack.count, 1);
                    saplings += 1;
                }
                "minecraft:stick" => {
                    assert!((1..=2).contains(&stack.count));
                    sticks += 1;
                }
                "minecraft:apple" => {
                    assert_eq!(stack.count, 1);
                    apples += 1;
                }
                other => panic!("unexpected leaf drop {other}"),
            }
        }
    }
    assert!(saplings > 0, "400 leaf breaks never dropped a sapling");
    assert!(sticks > 0, "400 leaf breaks never dropped a stick");
    assert!(apples > 0, "400 leaf breaks never dropped an apple");

    let shears = tool("minecraft:shears");
    let sheared = drops_from_seed(
        &leaves,
        &BlockLootContext::from_tool(Some(&shears)),
        None,
        1,
    );
    assert_eq!(sheared.items.len(), 1);
    assert_eq!(sheared.items[0].item, "minecraft:oak_leaves");

    let silk = enchanted("minecraft:diamond_pickaxe", "minecraft:silk_touch", 1);
    let silk_drops = drops_from_seed(&leaves, &BlockLootContext::from_tool(Some(&silk)), None, 1);
    assert_eq!(silk_drops.items[0].item, "minecraft:oak_leaves");
}

#[test]
fn survives_explosion_and_explosion_decay_remove_drops_without_changing_items() {
    let stone = state("minecraft:stone");
    let pick = tool("minecraft:iron_pickaxe");
    let context = BlockLootContext::from_tool(Some(&pick)).with_explosion_radius(4.0);

    let mut kept = 0;
    let mut destroyed = 0;
    for index in 0..64 {
        let seed = diverse_seed(index);
        let drops = drops_from_seed(&stone, &context, None, seed);
        if drops.is_empty() {
            destroyed += 1;
        } else {
            assert_eq!(drops.items.len(), 1);
            assert_eq!(drops.items[0].item, "minecraft:cobblestone");
            assert_eq!(drops.items[0].count, 1);
            kept += 1;
        }
    }
    assert!(
        kept > 0 && destroyed > 0,
        "explosion survival was not sampled"
    );

    let ore = state("minecraft:diamond_ore");
    let fortune_pick = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    let exploded = BlockLootContext::from_tool(Some(&fortune_pick)).with_explosion_radius(2.0);
    for index in 0..64 {
        let seed = diverse_seed(index);
        let drops = drops_from_seed(&ore, &exploded, None, seed);
        assert!(
            drops
                .items
                .iter()
                .all(|stack| stack.item == "minecraft:diamond")
        );
        assert!(drops.items.iter().all(|stack| stack.count <= 4));
    }
}

#[test]
fn unknown_functions_are_preserved_and_reported() {
    let json = r#"{
        "version": "1.21.11",
        "data_version": 4788,
        "counts": { "total": 1, "by_type": { "minecraft:block": 1 } },
        "tables": {
            "minecraft:blocks/mystery_block": {
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1.0,
                    "bonus_rolls": 0.0,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:stone",
                        "functions": [{ "function": "minecraft:future_function" }],
                        "conditions": [{ "condition": "minecraft:future_condition" }]
                    }]
                }]
            }
        }
    }"#;
    let catalog = LootCatalog::from_json(json).expect("synthetic catalog parses");
    let provider = CatalogLootProvider::new(&catalog);
    let block = state("minecraft:mystery_block");

    let drops = drops_from_seed(&block, &BlockLootContext::default(), Some(&provider), 3);
    // The unknown condition fails closed, so the entry does not drop. The
    // important part is that both payloads are named instead of disappearing.
    assert!(drops.items.is_empty());
    assert_eq!(drops.source, DropSource::LootHook);
    assert_eq!(
        drops.loot_table.as_deref(),
        Some("minecraft:blocks/mystery_block")
    );
    assert!(
        drops
            .report
            .unsupported_conditions
            .contains("minecraft:future_condition")
    );
    assert!(
        !drops
            .report
            .unsupported_functions
            .contains("minecraft:future_function")
    );

    // Without the unsupported condition the same unknown function is applied
    // as a no-op and reported without suppressing the item.
    let json = json.replace(
        r#""conditions": [{ "condition": "minecraft:future_condition" }]"#,
        r#""conditions": []"#,
    );
    let catalog = LootCatalog::from_json(&json).expect("synthetic catalog parses");
    let provider = CatalogLootProvider::new(&catalog);
    let drops = drops_from_seed(&block, &BlockLootContext::default(), Some(&provider), 3);
    assert_eq!(drops.items.len(), 1);
    assert_eq!(drops.items[0].item, "minecraft:stone");
    assert!(
        drops
            .report
            .unsupported_functions
            .contains("minecraft:future_function")
    );
}

#[test]
fn repeated_seeds_are_deterministic_and_distinct_seeds_differ() {
    let ore = state("minecraft:redstone_ore");
    let pick = tool("minecraft:iron_pickaxe");
    let context = BlockLootContext::from_tool(Some(&pick));

    let left = drops_from_seed(&ore, &context, None, 2026);
    let right = drops_from_seed(&ore, &context, None, 2026);
    assert_eq!(left, right);

    let mut counts = std::collections::BTreeSet::new();
    for index in 0..32 {
        counts.insert(drops_from_seed(&ore, &context, None, diverse_seed(index)).item_count());
    }
    assert!(counts.len() > 1, "the RNG stream never varied");
}

#[test]
fn fallback_is_only_used_without_a_shipped_table() {
    // Stone has a shipped table; its drop must never come from DROP_TABLE.
    let stone = state("minecraft:stone");
    let pick = tool("minecraft:iron_pickaxe");
    let stone_drops = drops_from_seed(&stone, &BlockLootContext::from_tool(Some(&pick)), None, 0);
    assert_eq!(
        stone_drops.loot_table.as_deref(),
        Some("minecraft:blocks/stone")
    );

    // Wall signs have no `blocks/oak_wall_sign` table, so the documented
    // family fallback maps them to the standing-sign item.
    let wall_sign = state("minecraft:oak_wall_sign");
    let sign_drops = drops_from_seed(&wall_sign, &BlockLootContext::default(), None, 0);
    assert_eq!(sign_drops.source, DropSource::Family);
    assert_eq!(sign_drops.items[0].item, "minecraft:oak_sign");
    assert!(sign_drops.loot_table.is_none());

    // Nothing maps water to an item id and there is no block table.
    let water = state("minecraft:water");
    let water_drops = drops_from_seed(&water, &BlockLootContext::default(), None, 0);
    assert!(water_drops.is_empty());
    assert_eq!(water_drops.source, DropSource::None);
    assert!(water_drops.loot_table.is_none());
}

struct DecliningProvider;

impl LootProvider for DecliningProvider {
    fn block_drops(
        &self,
        _state: &BlockState,
        _tool: Option<&ItemStack>,
    ) -> Option<Vec<ItemStack>> {
        None
    }
}

#[test]
fn a_declining_provider_falls_through_to_the_shipped_table() {
    let stone = state("minecraft:stone");
    let pick = tool("minecraft:iron_pickaxe");
    let context = BlockLootContext::from_tool(Some(&pick));
    let drops = drops_from_seed(&stone, &context, Some(&DecliningProvider), 0);
    assert_eq!(drops.source, DropSource::Table);
    assert_eq!(drops.items[0].item, "minecraft:cobblestone");
}

#[test]
fn shipped_table_functions_apply_through_the_provider() {
    let json = r#"{
        "version": "1.21.11",
        "data_version": 4788,
        "counts": { "total": 1, "by_type": { "minecraft:block": 1 } },
        "tables": {
            "minecraft:blocks/function_block": {
                "type": "minecraft:block",
                "pools": [{
                    "rolls": 1.0,
                    "bonus_rolls": 0.0,
                    "entries": [{
                        "type": "minecraft:item",
                        "name": "minecraft:diamond_pickaxe",
                        "functions": [
                            { "function": "minecraft:set_components", "components": { "minecraft:max_damage": 100 } },
                            { "function": "minecraft:set_damage", "damage": 0.25 },
                            { "function": "minecraft:set_name", "name": { "text": "Miner" } },
                            { "function": "minecraft:set_count", "count": 10 },
                            { "function": "minecraft:limit_count", "limit": { "max": 3 } },
                            { "function": "minecraft:enchant_randomly", "options": ["minecraft:sharpness"] }
                        ]
                    }]
                }]
            }
        }
    }"#;
    let catalog = LootCatalog::from_json(json).expect("synthetic catalog parses");
    let provider = CatalogLootProvider::new(&catalog);
    let block = state("minecraft:function_block");

    let drops = drops_from_seed(&block, &BlockLootContext::default(), Some(&provider), 5);
    assert_eq!(drops.items.len(), 1);
    let stack = &drops.items[0];
    assert_eq!(stack.count, 3, "limit_count must clamp set_count");
    assert_eq!(stack.max_damage(), 100);
    assert_eq!(stack.damage(), 75);
    assert!(matches!(
        stack.get_component("minecraft:custom_name"),
        Some(ComponentValue::CustomName(_))
    ));
    assert!(matches!(
        stack.get_component("minecraft:enchantments"),
        Some(ComponentValue::Enchantments(map))
            if map.get("minecraft:sharpness") == Some(&1)
    ));
    assert!(
        drops
            .report
            .approximations
            .iter()
            .any(|message| message.contains("enchant_randomly"))
    );
}

#[test]
fn exact_shipped_table_ids_resolve_to_the_same_table() {
    let catalog = LootCatalog::global();
    assert_eq!(
        catalog.block_table_id("minecraft:stone"),
        Some("minecraft:blocks/stone")
    );
    assert_eq!(
        catalog.block_table_id("minecraft:blocks/stone"),
        Some("minecraft:blocks/stone")
    );
    assert_eq!(
        catalog.block_table_id("minecraft:oak_wall_sign"),
        None,
        "wall-sign states use the documented family fallback, not a table"
    );
}

#[test]
fn dig_stop_rolls_the_table_without_touching_durability() {
    let stone = state("minecraft:stone");
    let pick = tool("minecraft:wooden_pickaxe");
    let mining = MiningContext::default();
    let pos = BlockPos::new(0, 64, 0);
    let mut dig = DigState::default();
    assert_eq!(
        dig.start(pos, false, &stone, Some(&pick), &mining, 0),
        DigStart::Started
    );

    let mut random = LegacyRandomSource::new(4);
    let loot_context = BlockLootContext::from_tool(Some(&pick));
    let loot = DigLootContext {
        context: &loot_context,
        provider: None,
    };
    let (stop, drops) = dig.stop_with_loot(pos, &stone, &mining, 18, &loot, &mut random);
    assert_eq!(stop, block_breaking::DigStop::Destroyed);
    let drops = drops.expect("a destroyed block has a drop roll");
    assert_eq!(drops.items[0].item, "minecraft:cobblestone");
    assert_eq!(drops.loot_table.as_deref(), Some("minecraft:blocks/stone"));
    assert_eq!(pick.damage(), 0, "stop_with_loot must not wear the tool");
}
