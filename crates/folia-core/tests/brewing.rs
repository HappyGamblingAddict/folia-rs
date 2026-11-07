//! Brewing stand, potion component, and consumable behavior against the
//! shipped 1.21.11 data.

use folia_core::crafting::{
    BREW_TIME_TICKS, BREWING_FUEL_SLOT, BREWING_INGREDIENT_SLOT, BrewingStandState,
    add_potion_effect, brew_result, is_brewing_ingredient, potion_contents, potion_stack,
};
use folia_core::effects::{EffectTick, StatusEffectKind, StatusEffectSet, potion_effects};
use folia_core::item_components::{
    ComponentValue, ItemStack, MobEffectInstance, PotionContents, plain_text,
};
use folia_core::item_prototypes::{ConsumeAnimation, ConsumeError, FoodData, consumable, consume};

fn stand_with(
    potion: &str,
    bottle_item: &str,
    ingredient: &str,
    ingredient_count: i32,
) -> BrewingStandState {
    let mut stand = BrewingStandState::new();
    for slot in 0..3 {
        assert!(stand.set_slot(slot, potion_stack(bottle_item, potion)));
    }
    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new(ingredient, ingredient_count)
    ));
    assert!(stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 1)
    ));
    stand
}

fn run_brew(stand: &mut BrewingStandState) {
    stand.tick();
    assert!(stand.is_brewing(), "the first tick must start the brew");
    for _ in 0..BREW_TIME_TICKS {
        stand.tick();
    }
    assert_eq!(stand.brew_time, 0);
}

fn potion_of(stack: &ItemStack) -> &str {
    potion_contents(stack)
        .and_then(|contents| contents.potion.as_deref())
        .expect("a potion contents component")
}

#[test]
fn nether_wart_brews_awkward_after_four_hundred_ticks() {
    let mut stand = stand_with(
        "minecraft:water",
        "minecraft:potion",
        "minecraft:nether_wart",
        1,
    );

    stand.tick();
    assert_eq!(stand.fuel, 19, "starting a brew spends one fuel charge");
    assert_eq!(stand.brew_time, BREW_TIME_TICKS);
    assert_eq!(stand.recipe_brew_time, BREW_TIME_TICKS);
    assert_eq!(stand.brewing_ingredient(), Some("minecraft:nether_wart"));
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:water");

    for _ in 0..BREW_TIME_TICKS - 1 {
        stand.tick();
    }
    assert_eq!(stand.brew_time, 1);
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:water");

    stand.tick();
    assert_eq!(stand.brew_time, 0);
    for slot in 0..3 {
        assert_eq!(potion_of(stand.slot(slot).unwrap()), "minecraft:awkward");
    }
    assert!(stand.ingredient().is_empty());
    assert_eq!(stand.fuel, 19);
}

#[test]
fn sugar_turns_awkward_into_swiftness() {
    let mut stand = stand_with(
        "minecraft:awkward",
        "minecraft:potion",
        "minecraft:sugar",
        1,
    );
    run_brew(&mut stand);
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:swiftness");

    let effects = potion_effects("minecraft:swiftness").unwrap();
    assert_eq!(
        effects,
        &[folia_core::effects::StatusEffect::new(
            StatusEffectKind::Speed,
            3600,
            0
        )]
    );
}

#[test]
fn redstone_extends_and_glowstone_amplifies() {
    let mut extended = stand_with(
        "minecraft:swiftness",
        "minecraft:potion",
        "minecraft:redstone",
        1,
    );
    run_brew(&mut extended);
    assert_eq!(
        potion_of(extended.slot(0).unwrap()),
        "minecraft:long_swiftness"
    );
    assert_eq!(
        potion_effects("minecraft:long_swiftness").unwrap(),
        &[folia_core::effects::StatusEffect::new(
            StatusEffectKind::Speed,
            9600,
            0
        )]
    );

    let mut amplified = stand_with(
        "minecraft:swiftness",
        "minecraft:potion",
        "minecraft:glowstone_dust",
        1,
    );
    run_brew(&mut amplified);
    assert_eq!(
        potion_of(amplified.slot(0).unwrap()),
        "minecraft:strong_swiftness"
    );
    assert_eq!(
        potion_effects("minecraft:strong_swiftness").unwrap(),
        &[folia_core::effects::StatusEffect::new(
            StatusEffectKind::Speed,
            1800,
            1
        )]
    );
}

#[test]
fn gunpowder_makes_splash_and_dragon_breath_makes_lingering() {
    let mut stand = stand_with(
        "minecraft:swiftness",
        "minecraft:potion",
        "minecraft:gunpowder",
        1,
    );
    run_brew(&mut stand);
    let splash = stand.slot(0).unwrap();
    assert_eq!(splash.item, "minecraft:splash_potion");
    assert_eq!(potion_of(splash), "minecraft:swiftness");

    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:dragon_breath", 1)
    ));
    run_brew(&mut stand);
    let lingering = stand.slot(0).unwrap();
    assert_eq!(lingering.item, "minecraft:lingering_potion");
    assert_eq!(potion_of(lingering), "minecraft:swiftness");

    // Dragon breath has a glass-bottle crafting remainder; because the
    // ingredient slot was emptied, vanilla places it back in that slot.
    assert_eq!(stand.ingredient().item, "minecraft:glass_bottle");
    assert_eq!(stand.ingredient().count, 1);
    assert!(stand.take_drops().is_empty());
}

#[test]
fn fermented_spider_eye_corrupts_swiftness_into_slowness() {
    let mut stand = stand_with(
        "minecraft:swiftness",
        "minecraft:potion",
        "minecraft:fermented_spider_eye",
        1,
    );
    run_brew(&mut stand);
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:slowness");
    assert_eq!(
        potion_effects("minecraft:slowness").unwrap(),
        &[folia_core::effects::StatusEffect::new(
            StatusEffectKind::Slowness,
            1800,
            0
        )]
    );

    // The same ingredient corrupts water into weakness.
    let weak = brew_result(
        &ItemStack::new("minecraft:fermented_spider_eye", 1),
        &potion_stack("minecraft:potion", "minecraft:water"),
    )
    .unwrap();
    assert_eq!(potion_of(&weak), "minecraft:weakness");
}

#[test]
fn fuel_and_brew_time_match_the_vanilla_state_machine() {
    let mut stand = BrewingStandState::new();
    assert!(stand.set_slot(0, potion_stack("minecraft:potion", "minecraft:water")));
    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:nether_wart", 1)
    ));
    assert!(stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 2)
    ));

    // One blaze powder becomes 20 charges; starting the brew spends one.
    stand.tick();
    assert_eq!(stand.fuel, 19);
    assert_eq!(stand.fuel_item().count, 1);
    assert_eq!(stand.brew_time, BREW_TIME_TICKS);
    assert_eq!(stand.bottle_bits(), [true, false, false]);

    // A non-ingredient aborts a running brew.
    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:diamond", 1)
    ));
    stand.tick();
    assert_eq!(stand.brew_time, 0);
    assert_eq!(stand.brewing_ingredient(), None);
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:water");
    assert_eq!(stand.fuel, 19);
}

#[test]
fn a_stand_without_a_mix_never_starts_brewing() {
    let mut stand = BrewingStandState::new();
    assert!(stand.set_slot(0, potion_stack("minecraft:potion", "minecraft:water")));
    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:diamond", 1)
    ));
    assert!(stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 1)
    ));

    for _ in 0..100 {
        stand.tick();
    }
    // Vanilla still loads fuel before checking brewability.
    assert_eq!(stand.fuel, 20);
    assert_eq!(stand.brew_time, 0);
    assert!(!stand.is_brewing());
    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:water");
}

#[test]
fn brewing_slot_rules_follow_the_vanilla_container() {
    let stand = BrewingStandState::new();
    assert!(stand.can_place_in_slot(0, &potion_stack("minecraft:potion", "minecraft:water")));
    assert!(stand.can_place_in_slot(0, &ItemStack::new("minecraft:glass_bottle", 1)));
    assert!(!stand.can_place_in_slot(0, &ItemStack::new("minecraft:apple", 1)));
    assert!(stand.can_place_in_slot(
        BREWING_INGREDIENT_SLOT,
        &ItemStack::new("minecraft:gunpowder", 1)
    ));
    assert!(stand.can_place_in_slot(
        BREWING_FUEL_SLOT,
        &ItemStack::new("minecraft:blaze_powder", 1)
    ));
    assert!(!stand.can_place_in_slot(BREWING_FUEL_SLOT, &ItemStack::new("minecraft:coal", 1)));

    assert!(is_brewing_ingredient("minecraft:nether_wart"));
    assert!(is_brewing_ingredient("minecraft:dragon_breath"));
    assert!(!is_brewing_ingredient("minecraft:apple"));
}

#[test]
fn eating_restores_food_and_saturation() {
    let apple = consumable("minecraft:apple").expect("apple is consumable");
    assert_eq!(apple.eat_ticks(), 32);
    assert_eq!(apple.animation, ConsumeAnimation::Eat);
    assert_eq!(apple.food.as_ref().unwrap().nutrition, 4);
    assert!((apple.food.as_ref().unwrap().saturation - 2.4).abs() < 1e-6);

    let mut food = FoodData {
        food_level: 10,
        saturation_level: 0.0,
        exhaustion_level: 0.0,
    };
    let mut effects = StatusEffectSet::new();
    let mut stack = ItemStack::new("minecraft:apple", 2);
    let outcome = consume(&mut stack, &mut food, &mut effects, false).unwrap();
    assert_eq!(outcome.nutrition, 4);
    assert!((outcome.saturation - 2.4).abs() < 1e-6);
    assert_eq!(food.food_level, 14);
    assert!((food.saturation_level - 2.4).abs() < 1e-6);
    assert_eq!(stack.count, 1);

    let outcome = consume(&mut stack, &mut food, &mut effects, false).unwrap();
    assert_eq!(outcome.nutrition, 4);
    assert_eq!(food.food_level, 18);
    assert!((food.saturation_level - 4.8).abs() < 1e-6);
    assert!(stack.is_empty());

    assert_eq!(consumable("minecraft:dried_kelp").unwrap().eat_ticks(), 16);
}

#[test]
fn full_hunger_blocks_food_unless_always_eatable_or_creative() {
    let mut food = FoodData::new();
    let mut effects = StatusEffectSet::new();
    let mut apple = ItemStack::new("minecraft:apple", 1);
    assert_eq!(
        consume(&mut apple, &mut food, &mut effects, false),
        Err(ConsumeError::CannotEat)
    );
    assert_eq!(apple.count, 1);
    assert_eq!(food.food_level, 20);

    // Golden apples set `can_always_eat`.
    let mut golden = ItemStack::new("minecraft:golden_apple", 1);
    let outcome = consume(&mut golden, &mut food, &mut effects, false).unwrap();
    assert_eq!(outcome.nutrition, 4);
    assert!(golden.is_empty());
    assert_eq!(food.food_level, 20);
    assert!((food.saturation_level - 14.6).abs() < 1e-6);

    // Creative (`hasInfiniteMaterials`) ignores the food gate and never
    // shrinks the stack.
    let mut creative_apple = ItemStack::new("minecraft:apple", 1);
    let outcome = consume(&mut creative_apple, &mut food, &mut effects, true).unwrap();
    assert_eq!(outcome.nutrition, 4);
    assert_eq!(creative_apple.count, 1);
    assert_eq!(creative_apple.item, "minecraft:apple");
}

#[test]
fn potion_drinking_applies_effects_and_leaves_a_glass_bottle() {
    let mut food = FoodData::new();
    let mut effects = StatusEffectSet::new();
    let mut stack = potion_stack("minecraft:potion", "minecraft:swiftness");
    let outcome = consume(&mut stack, &mut food, &mut effects, false).unwrap();

    assert!(outcome.effect_events.is_empty());
    let speed = effects.get(StatusEffectKind::Speed).expect("speed effect");
    assert_eq!(speed.duration, 3600);
    assert_eq!(speed.amplifier, 0);
    assert_eq!(stack.item, "minecraft:glass_bottle");
    assert_eq!(stack.count, 1);
    assert_eq!(food.food_level, 20);

    // Splash and lingering potions are thrown, not drunk.
    let mut splash = potion_stack("minecraft:splash_potion", "minecraft:swiftness");
    assert_eq!(
        consume(&mut splash, &mut food, &mut effects, false),
        Err(ConsumeError::NotConsumable(
            "minecraft:splash_potion".to_owned()
        ))
    );

    // Creative drinks without consuming or creating a bottle.
    let mut creative = potion_stack("minecraft:potion", "minecraft:strong_swiftness");
    consume(&mut creative, &mut food, &mut effects, true).unwrap();
    assert_eq!(creative.item, "minecraft:potion");
    assert_eq!(creative.count, 1);
    assert_eq!(
        effects.amplifier(StatusEffectKind::Speed),
        Some(1),
        "the creative drink still applies the potion"
    );
}

#[test]
fn custom_potion_effects_keep_amplifier_and_duration() {
    let mut stack = potion_stack("minecraft:potion", "minecraft:water");
    assert!(add_potion_effect(
        &mut stack,
        MobEffectInstance::new("minecraft:absorption", 1234, 2)
    ));

    let contents = potion_contents(&stack).unwrap();
    assert_eq!(contents.custom_effects.len(), 1);
    assert_eq!(contents.custom_effects[0].duration, 1234);
    assert_eq!(contents.custom_effects[0].amplifier, 2);

    let mut food = FoodData::new();
    let mut effects = StatusEffectSet::new();
    consume(&mut stack, &mut food, &mut effects, false).unwrap();
    let absorption = effects
        .get(StatusEffectKind::Absorption)
        .expect("custom absorption effect");
    assert_eq!(absorption.duration, 1234);
    assert_eq!(absorption.amplifier, 2);
    assert_eq!(effects.iter().count(), 1, "water has no base effects");
}

#[test]
fn potion_components_survive_brewing() {
    let mut bottle = potion_stack("minecraft:potion", "minecraft:swiftness");
    bottle.set_component(ComponentValue::CustomName(plain_text("Runner")));
    assert!(add_potion_effect(
        &mut bottle,
        MobEffectInstance::new("minecraft:absorption", 200, 1)
    ));

    let mut stand = BrewingStandState::new();
    assert!(stand.set_slot(0, bottle));
    assert!(stand.set_slot(
        BREWING_INGREDIENT_SLOT,
        ItemStack::new("minecraft:redstone", 1)
    ));
    assert!(stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 1)
    ));
    run_brew(&mut stand);

    let result = stand.slot(0).unwrap();
    assert_eq!(potion_of(result), "minecraft:long_swiftness");
    assert_eq!(
        result.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Runner")))
    );
    let contents = potion_contents(result).unwrap();
    assert_eq!(contents.custom_effects.len(), 1);
    assert_eq!(contents.custom_effects[0].duration, 200);
    assert_eq!(contents.custom_effects[0].amplifier, 1);
}

#[test]
fn brewing_preserves_the_ingredient_stack_components() {
    let mut wart = ItemStack::new("minecraft:nether_wart", 2);
    wart.set_component(ComponentValue::CustomName(plain_text("Seed Stock")));

    let mut stand = BrewingStandState::new();
    assert!(stand.set_slot(0, potion_stack("minecraft:potion", "minecraft:water")));
    assert!(stand.set_slot(BREWING_INGREDIENT_SLOT, wart));
    assert!(stand.set_slot(
        BREWING_FUEL_SLOT,
        ItemStack::new("minecraft:blaze_powder", 1)
    ));
    run_brew(&mut stand);

    assert_eq!(potion_of(stand.slot(0).unwrap()), "minecraft:awkward");
    let remaining = stand.ingredient();
    assert_eq!(remaining.item, "minecraft:nether_wart");
    assert_eq!(remaining.count, 1);
    assert_eq!(
        remaining.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(plain_text("Seed Stock")))
    );
}

#[test]
fn potion_effect_table_handles_multi_effect_and_unknown_potions() {
    assert!(potion_effects("minecraft:awkward").unwrap().is_empty());
    assert_eq!(
        potion_effects("minecraft:turtle_master").unwrap(),
        &[
            folia_core::effects::StatusEffect::new(StatusEffectKind::Slowness, 400, 3),
            folia_core::effects::StatusEffect::new(StatusEffectKind::Resistance, 400, 2),
        ]
    );
    assert!(potion_effects("minecraft:not_a_potion").is_none());

    // Custom effects are appended after the base potion's effects.
    let mut contents = PotionContents::new("minecraft:swiftness");
    contents
        .custom_effects
        .push(MobEffectInstance::new("minecraft:poison", 80, 1));
    let mut effects = StatusEffectSet::new();
    let events = folia_core::effects::apply_potion_contents(&mut effects, &contents);
    assert!(events.is_empty());
    assert_eq!(effects.iter().count(), 2);
    assert_eq!(effects.get(StatusEffectKind::Poison).unwrap().duration, 80);
}

#[test]
fn instant_potion_payload_is_reported_to_the_caller() {
    let mut effects = StatusEffectSet::new();
    let events = folia_core::effects::apply_potion_contents(
        &mut effects,
        &PotionContents::new("minecraft:healing"),
    );
    assert_eq!(events, vec![EffectTick::None]);
    assert!(
        !effects.has(StatusEffectKind::InstantHealth),
        "instant effects are applied once and never stored"
    );
}
