use folia_core::inventory::{
    self, CLIENTBOUND_CONTAINER_SET_CONTENT, ContainerClick, ItemStack, apply_player_click,
    container_content_packet_with_armor,
};
use folia_core::protocol::PacketReader;

fn click(slot: i32, button: i32, click_type: i32) -> ContainerClick {
    ContainerClick {
        container_id: 0,
        state_id: 0,
        slot,
        button,
        click_type,
        carried: ItemStack::empty(),
    }
}

fn parse_content(packet: &[u8]) -> (Vec<ItemStack>, ItemStack) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(
        reader.read_varint().unwrap(),
        CLIENTBOUND_CONTAINER_SET_CONTENT
    );
    assert_eq!(reader.read_varint().unwrap(), 0); // container id
    assert_eq!(reader.read_varint().unwrap(), 0); // state id
    let count = reader.read_varint().unwrap() as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(inventory::read_item_stack(&mut reader).expect("menu item stack"));
    }
    let cursor = inventory::read_item_stack(&mut reader).expect("carried stack");
    (items, cursor)
}

#[test]
fn place_helmet_in_head_slot() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    let mut cursor = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut cursor);

    assert!(apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 0),
    ));
    assert_eq!(armor[3].item, "minecraft:diamond_helmet");
    assert!(cursor.is_empty());
}

#[test]
fn pick_up_and_reject_wrong_armor() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    let mut cursor = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut cursor);
    assert!(apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 0),
    ));

    let mut chestplate = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut chestplate);
    cursor = chestplate.clone();
    assert!(!apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 0),
    ));
    assert_eq!(armor[3].item, "minecraft:diamond_helmet");
    assert_eq!(cursor.item, "minecraft:diamond_chestplate");

    cursor = ItemStack::empty();
    assert!(apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 0),
    ));
    assert!(armor[3].is_empty());
    assert_eq!(cursor.item, "minecraft:diamond_helmet");
}

#[test]
fn quick_move_armor_into_inventory() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    let mut boots = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut boots);
    armor[0] = boots;
    let mut cursor = ItemStack::empty();

    assert!(apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(8, 0, 1),
    ));
    assert!(armor[0].is_empty());
    assert_eq!(inventory[0].item, "minecraft:diamond_boots");
}

#[test]
fn hotbar_swap_equips_armor() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    let mut helmet = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut helmet);
    inventory[0] = helmet;
    let mut cursor = ItemStack::empty();

    assert!(apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 2),
    ));
    assert_eq!(armor[3].item, "minecraft:diamond_helmet");
    assert!(inventory[0].is_empty());
}

#[test]
fn hotbar_swap_rejects_wrong_item() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    inventory[0] = ItemStack::new("minecraft:stone".to_string(), 1);
    let mut cursor = ItemStack::empty();

    assert!(!apply_player_click(
        &mut inventory,
        &mut armor,
        &mut cursor,
        click(5, 0, 2),
    ));
    assert!(armor[3].is_empty());
    assert_eq!(inventory[0].item, "minecraft:stone");
}

#[test]
fn armor_content_packet_places_slots_five_to_eight() {
    let inventory = vec![ItemStack::empty(); 36];
    let mut armor = std::array::from_fn(|_| ItemStack::empty());
    let mut helmet = ItemStack::new("minecraft:diamond_helmet".to_string(), 1);
    let mut chest = ItemStack::new("minecraft:diamond_chestplate".to_string(), 1);
    let mut legs = ItemStack::new("minecraft:diamond_leggings".to_string(), 1);
    let mut boots = ItemStack::new("minecraft:diamond_boots".to_string(), 1);
    folia_core::item_prototypes::apply_defaults(&mut helmet);
    folia_core::item_prototypes::apply_defaults(&mut chest);
    folia_core::item_prototypes::apply_defaults(&mut legs);
    folia_core::item_prototypes::apply_defaults(&mut boots);
    armor[3] = helmet;
    armor[2] = chest;
    armor[1] = legs;
    armor[0] = boots;

    let packet = container_content_packet_with_armor(&inventory, &armor, &ItemStack::empty());
    let (items, cursor) = parse_content(&packet);
    assert_eq!(items.len(), 46);
    assert_eq!(items[5].item, "minecraft:diamond_helmet");
    assert_eq!(items[6].item, "minecraft:diamond_chestplate");
    assert_eq!(items[7].item, "minecraft:diamond_leggings");
    assert_eq!(items[8].item, "minecraft:diamond_boots");
    assert!(cursor.is_empty());
}
