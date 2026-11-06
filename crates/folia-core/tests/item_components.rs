use folia_core::inventory::{self, ComponentValue, ItemStack};
use folia_core::item_components::{
    AttributeDisplay, AttributeModifierEntry, AttributeOperation, BlockSet, ComponentType,
    EquipmentSlotGroup, FoodProperties, ItemContainer, MobEffectInstance, PotionContents, Rarity,
    Tool, ToolRule, component_from_nbt, component_to_nbt, item_stack_from_nbt, item_stack_to_nbt,
    plain_text, read_item_stack, write_item_stack_report,
};
use folia_core::nbt::{self, Tag};
use folia_core::player::PlayerSession;
use folia_core::protocol::{PacketReader, PacketWriter};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn encode(tag: &Tag) -> Vec<u8> {
    nbt::write_root_to_vec(tag).unwrap()
}

fn decode(bytes: &[u8]) -> Tag {
    let mut reader = bytes;
    nbt::read_root(&mut reader).unwrap()
}

fn round_trip_nbt(stack: &ItemStack) -> ItemStack {
    let tag = item_stack_to_nbt(stack);
    let bytes = encode(&tag);
    let decoded = item_stack_from_nbt(&decode(&bytes)).unwrap();
    assert_eq!(&decoded, stack);
    assert_eq!(
        encode(&item_stack_to_nbt(&decoded)),
        bytes,
        "NBT encoding is stable for {stack:?}"
    );
    decoded
}

fn stack_with(value: ComponentValue) -> ItemStack {
    let mut stack = ItemStack::new("minecraft:paper", 1);
    stack.set_component(value);
    stack
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-items-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn nbt_round_trips_every_ported_component() {
    round_trip_nbt(&stack_with(ComponentValue::CustomName(plain_text("Named"))));
    round_trip_nbt(&stack_with(ComponentValue::ItemName(Tag::Compound(vec![
        ("translate".into(), Tag::String("item.example".into())),
    ]))));
    round_trip_nbt(&stack_with(ComponentValue::Lore(vec![
        Tag::String("plain line".into()),
        Tag::Compound(vec![
            ("text".into(), Tag::String("styled line".into())),
            ("color".into(), Tag::String("gold".into())),
            ("italic".into(), Tag::Byte(1)),
        ]),
    ])));
    round_trip_nbt(&stack_with(ComponentValue::Damage(7)));
    round_trip_nbt(&stack_with(ComponentValue::MaxDamage(250)));
    round_trip_nbt(&stack_with(ComponentValue::MaxStackSize(16)));
    round_trip_nbt(&stack_with(ComponentValue::RepairCost(2)));
    round_trip_nbt(&stack_with(ComponentValue::Unbreakable));
    round_trip_nbt(&stack_with(ComponentValue::Rarity(Rarity::Rare)));
    round_trip_nbt(&stack_with(ComponentValue::CustomData(Tag::Compound(
        vec![
            ("plugin".into(), Tag::String("folia".into())),
            (
                "nested".into(),
                Tag::Compound(vec![("n".into(), Tag::Int(1))]),
            ),
        ],
    ))));
    round_trip_nbt(&stack_with(ComponentValue::Enchantments(BTreeMap::from([
        ("minecraft:efficiency".to_owned(), 4),
        ("minecraft:silk_touch".to_owned(), 1),
    ]))));
    round_trip_nbt(&stack_with(ComponentValue::StoredEnchantments(
        BTreeMap::from([("minecraft:mending".to_owned(), 1)]),
    )));
    round_trip_nbt(&stack_with(ComponentValue::AttributeModifiers(vec![
        AttributeModifierEntry {
            attribute: "minecraft:generic.attack_damage".into(),
            id: "minecraft:base_attack_damage".into(),
            amount: 3.0,
            operation: AttributeOperation::AddValue,
            slot: EquipmentSlotGroup::Mainhand,
            display: AttributeDisplay::Default,
        },
        AttributeModifierEntry {
            attribute: "minecraft:generic.movement_speed".into(),
            id: "example:slow".into(),
            amount: -0.1,
            operation: AttributeOperation::AddMultipliedTotal,
            slot: EquipmentSlotGroup::Any,
            display: AttributeDisplay::Hidden,
        },
    ])));
    round_trip_nbt(&stack_with(ComponentValue::Food(FoodProperties {
        nutrition: 4,
        saturation: 2.4,
        can_always_eat: true,
    })));
    round_trip_nbt(&stack_with(ComponentValue::Tool(Tool {
        rules: vec![ToolRule {
            blocks: BlockSet::Tag("minecraft:mineable/pickaxe".into()),
            speed: Some(8.0),
            correct_for_drops: Some(true),
        }],
        default_mining_speed: 1.0,
        damage_per_block: 2,
        can_destroy_blocks_in_creative: true,
    })));
    round_trip_nbt(&stack_with(ComponentValue::PotionContents(
        PotionContents {
            potion: Some("minecraft:strong_healing".into()),
            custom_color: Some(0x123456),
            custom_effects: vec![MobEffectInstance {
                id: "minecraft:speed".into(),
                amplifier: 1,
                duration: 200,
                ambient: false,
                show_particles: true,
                show_icon: true,
                hidden_effect: None,
            }],
            custom_name: Some("brew".into()),
        },
    )));
}

#[test]
fn container_component_nbt_round_trip() {
    let mut nested = ItemStack::new("minecraft:diamond_sword", 1);
    nested.set_component(ComponentValue::CustomName(plain_text("Nested")));
    nested.set_component(ComponentValue::Damage(3));
    nested.set_component(ComponentValue::MaxDamage(1561));
    let container = ItemContainer::new(vec![
        ItemStack::empty(),
        nested,
        ItemStack::new("minecraft:apple", 4),
    ]);
    round_trip_nbt(&stack_with(ComponentValue::Container(container)));
}

#[test]
fn unknown_component_is_preserved_exactly() {
    let root = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:paper".into())),
        ("count".into(), Tag::Int(3)),
        (
            "components".into(),
            Tag::Compound(vec![
                (
                    "example:mystery".into(),
                    Tag::Compound(vec![("value".into(), Tag::Long(42))]),
                ),
                ("minecraft:damage".into(), Tag::Int(1)),
            ]),
        ),
    ]);
    let stack = item_stack_from_nbt(&root).unwrap();
    assert!(matches!(
        stack.get_component("example:mystery"),
        Some(ComponentValue::Opaque(_))
    ));
    assert_eq!(encode(&item_stack_to_nbt(&stack)), encode(&root));
}

#[test]
fn known_component_with_malformed_payload_is_kept_opaque() {
    let root = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:paper".into())),
        ("count".into(), Tag::Int(1)),
        (
            "components".into(),
            Tag::Compound(vec![(
                "minecraft:damage".into(),
                Tag::String("oops".into()),
            )]),
        ),
    ]);
    let stack = item_stack_from_nbt(&root).unwrap();
    assert!(matches!(
        stack.get_component("minecraft:damage"),
        Some(ComponentValue::Opaque(Tag::String(_)))
    ));
    assert_eq!(encode(&item_stack_to_nbt(&stack)), encode(&root));
}

fn all_network_supported_stack() -> ItemStack {
    let mut nested = ItemStack::new("minecraft:apple", 2);
    nested.set_component(ComponentValue::CustomName(plain_text("Snack")));
    let mut stack = ItemStack::new("minecraft:shulker_box", 1);
    stack.set_component(ComponentValue::CustomName(plain_text("Box")));
    stack.set_component(ComponentValue::ItemName(Tag::String("Box item".into())));
    stack.set_component(ComponentValue::Lore(vec![
        Tag::String("line one".into()),
        Tag::Compound(vec![("text".into(), Tag::String("line two".into()))]),
    ]));
    stack.set_component(ComponentValue::Damage(2));
    stack.set_component(ComponentValue::MaxDamage(100));
    stack.set_component(ComponentValue::RepairCost(1));
    stack.set_component(ComponentValue::Unbreakable);
    stack.set_component(ComponentValue::Rarity(Rarity::Uncommon));
    stack.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:unbreaking".to_owned(),
        2,
    )])));
    stack.set_component(ComponentValue::StoredEnchantments(BTreeMap::from([(
        "minecraft:mending".to_owned(),
        1,
    )])));
    stack.set_component(ComponentValue::CustomData(Tag::Compound(vec![(
        "plugin".into(),
        Tag::String("test".into()),
    )])));
    stack.set_component(ComponentValue::Container(ItemContainer::new(vec![nested])));
    stack.set_component(ComponentValue::Food(FoodProperties {
        nutrition: 3,
        saturation: 1.5,
        can_always_eat: false,
    }));
    stack.set_component(ComponentValue::MaxStackSize(64));
    stack
}

#[test]
fn network_round_trips_all_supported_components() {
    let stack = all_network_supported_stack();
    let mut writer = PacketWriter::new();
    let report = write_item_stack_report(&mut writer, &stack).unwrap();
    assert!(report.is_complete(), "{report:?}");
    let bytes = writer.into_inner();
    let mut reader = PacketReader::new(&bytes);
    let decoded = read_item_stack(&mut reader).unwrap();
    assert_eq!(decoded, stack);
    assert!(reader.is_empty(), "decoder consumed the whole payload");
}

#[test]
fn network_omits_unported_components_with_an_explicit_report() {
    for (value, name) in [
        (ComponentValue::Tool(Tool::new(vec![])), "minecraft:tool"),
        (
            ComponentValue::PotionContents(PotionContents::new("minecraft:healing")),
            "minecraft:potion_contents",
        ),
        (
            ComponentValue::AttributeModifiers(vec![AttributeModifierEntry {
                attribute: "minecraft:generic.attack_damage".into(),
                id: "example:test".into(),
                amount: 1.0,
                operation: AttributeOperation::AddValue,
                slot: EquipmentSlotGroup::Any,
                display: AttributeDisplay::Default,
            }]),
            "minecraft:attribute_modifiers",
        ),
    ] {
        let stack = stack_with(value);
        let mut writer = PacketWriter::new();
        let report = write_item_stack_report(&mut writer, &stack).unwrap();
        assert_eq!(report.omitted_added, vec![name]);
        // The payload is byte-identical to the same item with no components:
        // no malformed payload bytes are written for the omitted component.
        let mut baseline = PacketWriter::new();
        write_item_stack_report(&mut baseline, &ItemStack::new("minecraft:paper", 1)).unwrap();
        assert_eq!(writer.into_inner(), baseline.into_inner());
    }
}

#[test]
fn removed_component_network_round_trip() {
    let mut stack = ItemStack::new("minecraft:stone", 1);
    stack.mark_component_removed("minecraft:damage");
    let mut writer = PacketWriter::new();
    let report = write_item_stack_report(&mut writer, &stack).unwrap();
    assert!(report.is_complete());
    let bytes = writer.into_inner();
    // count=1, item=stone, added=0, removed=1, component id for damage=3
    assert_eq!(bytes, vec![1, 1, 0, 1, 3]);
    let mut reader = PacketReader::new(&bytes);
    let decoded = read_item_stack(&mut reader).unwrap();
    assert!(decoded.components.is_removed("minecraft:damage"));
}

#[test]
fn stone_and_dirt_network_bodies_are_unchanged() {
    let mut writer = PacketWriter::new();
    inventory::write_item_stack(&mut writer, &ItemStack::new("minecraft:stone", 1));
    assert_eq!(writer.into_inner(), vec![1, 1, 0, 0]);

    let mut writer = PacketWriter::new();
    inventory::write_item_stack(&mut writer, &ItemStack::new("minecraft:dirt", 64));
    assert_eq!(
        writer.into_inner(),
        vec![64, item_id("minecraft:dirt") as u8, 0, 0]
    );
}

#[test]
fn merge_and_split_respect_components_and_max_stack_size() {
    let mut base = ItemStack::new("minecraft:potion", 10);
    base.set_component(ComponentValue::CustomName(plain_text("Healing")));
    let mut same = base.copy_with_count(8);

    let mut target = base.copy_with_count(60);
    let moved = inventory::merge_stacks(&mut target, &mut same);
    assert_eq!(moved, 4);
    assert_eq!(target.count, 64);
    assert_eq!(same.count, 4);
    assert_eq!(
        target.get_component("minecraft:custom_name"),
        base.get_component("minecraft:custom_name")
    );

    let mut other = base.copy_with_count(2);
    other.set_component(ComponentValue::CustomName(plain_text("Other")));
    let mut target = base.copy_with_count(1);
    assert_eq!(inventory::merge_stacks(&mut target, &mut other), 0);

    let mut small = ItemStack::new("minecraft:ender_pearl", 5);
    small.set_component(ComponentValue::MaxStackSize(16));
    let mut target = small.copy_with_count(15);
    let mut source = small.copy_with_count(5);
    assert_eq!(inventory::merge_stacks(&mut target, &mut source), 1);
    assert_eq!(target.count, 16);

    let mut stack = base.copy_with_count(5);
    let split = stack.split(2);
    assert_eq!(split.count, 2);
    assert_eq!(stack.count, 3);
    assert_eq!(
        split.get_component("minecraft:custom_name"),
        stack.get_component("minecraft:custom_name")
    );
    assert_eq!(
        split.get_component("minecraft:custom_name"),
        base.get_component("minecraft:custom_name")
    );

    let mut sword = ItemStack::new("minecraft:iron_sword", 1);
    sword.set_component(ComponentValue::MaxDamage(250));
    let mut sword_source = sword.clone();
    assert_eq!(sword.max_stack_size(), 1);
    assert_eq!(inventory::merge_stacks(&mut sword, &mut sword_source), 0);
}

#[test]
fn creative_slot_assignment_preserves_components() {
    let mut stack = ItemStack::new("minecraft:diamond_sword", 1);
    stack.set_component(ComponentValue::CustomName(plain_text("Creative")));
    stack.set_component(ComponentValue::Damage(5));
    stack.set_component(ComponentValue::MaxDamage(1561));

    let mut writer = PacketWriter::new();
    writer
        .write_varint(inventory::SERVERBOUND_SET_CREATIVE_MODE_SLOT)
        .unwrap();
    writer.write_i16(36).unwrap();
    inventory::write_item_stack(&mut writer, &stack);
    let slot = inventory::parse_creative_slot(&writer.into_inner()).expect("creative slot");
    assert_eq!(slot.slot, 36);
    assert_eq!(slot.stack, stack);

    let mut session = PlayerSession::new("Creative".into(), [0x21; 16], 0.0, 64.0, 0.0);
    session.inventory[0] = slot.stack.clone();
    assert_eq!(session.inventory[0], stack);
}

#[test]
fn interleaved_added_and_removed_components_keep_their_order() {
    let root = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:paper".into())),
        ("count".into(), Tag::Int(1)),
        (
            "components".into(),
            Tag::Compound(vec![
                ("!minecraft:max_damage".into(), Tag::Compound(Vec::new())),
                ("minecraft:damage".into(), Tag::Int(5)),
                (
                    "example:extra".into(),
                    Tag::Compound(vec![("k".into(), Tag::Int(1))]),
                ),
                ("!minecraft:repair_cost".into(), Tag::Compound(Vec::new())),
            ]),
        ),
    ]);
    let stack = item_stack_from_nbt(&root).unwrap();
    assert_eq!(encode(&item_stack_to_nbt(&stack)), encode(&root));
}

#[test]
fn component_equality_ignores_patch_insertion_order() {
    let mut a = ItemStack::new("minecraft:stone", 1);
    a.set_component(ComponentValue::Rarity(Rarity::Rare));
    a.set_component(ComponentValue::CustomName(plain_text("A")));
    let mut b = ItemStack::new("minecraft:stone", 1);
    b.set_component(ComponentValue::CustomName(plain_text("A")));
    b.set_component(ComponentValue::Rarity(Rarity::Rare));
    assert_ne!(
        item_stack_to_nbt(&a),
        item_stack_to_nbt(&b),
        "wire/NBT order still differs"
    );
    assert!(a.same_item_and_components(&b));

    let mut c = a.copy_with_count(60);
    let mut d = b.copy_with_count(5);
    assert_eq!(inventory::merge_stacks(&mut c, &mut d), 4);
    assert_eq!(c.count, 64);
    assert_eq!(d.count, 1);
}

#[test]
fn selected_item_consumption_keeps_components() {
    let mut session = PlayerSession::new("Survival".into(), [0x22; 16], 0.0, 64.0, 0.0);
    let mut stack = ItemStack::new("minecraft:golden_apple", 2);
    stack.set_component(ComponentValue::CustomName(plain_text("Lunch")));
    session.inventory[0] = stack;
    session.selected_slot = 0;
    assert!(session.consume_selected());
    assert_eq!(session.selected_item().unwrap().count, 1);
    assert!(
        session
            .selected_item()
            .unwrap()
            .get_component("minecraft:custom_name")
            .is_some()
    );
    assert!(session.consume_selected());
    assert!(session.selected_item().is_none());
}

#[test]
fn playerdata_round_trips_inventory_components() {
    let directory = temp_dir("playerdata");
    let mut session = PlayerSession::new("Steve".into(), [0x33; 16], 12.5, 64.0, -3.25);
    let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
    sword.set_component(ComponentValue::CustomName(plain_text("Excalibur")));
    sword.set_component(ComponentValue::Damage(12));
    sword.set_component(ComponentValue::MaxDamage(1561));
    sword.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:sharpness".to_owned(),
        5,
    )])));
    session.inventory[0] = sword;
    let mut cursor = ItemStack::new("minecraft:paper", 2);
    cursor.set_component(ComponentValue::CustomData(Tag::Compound(vec![(
        "tag".into(),
        Tag::Int(9),
    )])));
    session.cursor = cursor;

    let path = session.save(&directory).unwrap();
    let root = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
    let inventory = root.get("Inventory").and_then(Tag::as_list).unwrap();
    assert_eq!(
        inventory[0].get("count").and_then(Tag::as_int),
        Some(1),
        "vanilla writes lowercase count"
    );
    assert!(inventory[0].get("components").is_some());
    assert!(root.get("Cursor").unwrap().get("components").is_some());

    let loaded = PlayerSession::load(&directory, [0x33; 16])
        .unwrap()
        .expect("playerdata");
    assert_eq!(loaded.inventory[0], session.inventory[0]);
    assert_eq!(loaded.cursor, session.cursor);

    // Rewriting the loaded session keeps the component NBT byte-identical.
    loaded.save(&directory).unwrap();
    let rewritten = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
    let rewritten_inventory = rewritten.get("Inventory").and_then(Tag::as_list).unwrap();
    assert_eq!(
        encode(&rewritten_inventory[0]),
        encode(&inventory[0]),
        "inventory entry NBT survives a load/save cycle"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn legacy_capital_count_still_loads() {
    let root = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:stone".into())),
        ("Count".into(), Tag::Int(7)),
    ]);
    let stack = item_stack_from_nbt(&root).unwrap();
    assert_eq!(stack.item, "minecraft:stone");
    assert_eq!(stack.count, 7);
}

#[test]
fn component_type_ids_match_java_registration_order() {
    let expected = [
        ("minecraft:custom_data", 0),
        ("minecraft:max_stack_size", 1),
        ("minecraft:max_damage", 2),
        ("minecraft:damage", 3),
        ("minecraft:unbreakable", 4),
        ("minecraft:custom_name", 6),
        ("minecraft:item_name", 9),
        ("minecraft:lore", 11),
        ("minecraft:rarity", 12),
        ("minecraft:enchantments", 13),
        ("minecraft:attribute_modifiers", 16),
        ("minecraft:repair_cost", 19),
        ("minecraft:food", 23),
        ("minecraft:tool", 28),
        ("minecraft:stored_enchantments", 41),
        ("minecraft:potion_contents", 49),
        ("minecraft:container", 73),
    ];
    for (name, id) in expected {
        assert_eq!(inventory::component_id(name), Some(id), "{name}");
        assert_eq!(inventory::component_name(id), Some(name));
        assert_eq!(ComponentType::named(name).id(), Some(id));
    }
}

#[test]
fn component_helpers_keep_custom_data_and_potion_types() {
    let tag = Tag::Compound(vec![("a".into(), Tag::Int(1))]);
    let component = ComponentType::named("minecraft:custom_data");
    let value = component_from_nbt(&component, &tag).unwrap();
    assert_eq!(component_to_nbt(&value), tag);

    let potion = ComponentType::named("minecraft:potion_contents");
    let tag = Tag::String("minecraft:healing".into());
    let value = component_from_nbt(&potion, &tag).unwrap();
    assert_eq!(
        value,
        ComponentValue::PotionContents(PotionContents::new("minecraft:healing"))
    );
}

fn item_id(name: &str) -> i32 {
    inventory::item_id(name).expect("known item")
}
