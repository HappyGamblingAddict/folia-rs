//! Container-menu tests: Open Screen/Content/Data packet layouts, click
//! semantics, block-entity persistence, and close lifecycle.
//!
//! The packet ids here are the 1.21.11 `GameProtocols` registrations (play has
//! a bundle packet at index 0, so every id is one higher than the `addPacket`
//! index). An unknown id is a test failure, never a fallback.

use folia_core::block_entity::{
    BlockEntity, BlockEntityPos, BrewingStandBlockEntity, CHEST_SLOTS, ChestBlockEntity,
    FURNACE_SLOTS, FurnaceBlockEntity,
};
use folia_core::crafting::{BREWING_SLOT_COUNT, FurnaceKind};
use folia_core::inventory::{
    self, CLIENTBOUND_CONTAINER_CLOSE, CLIENTBOUND_CONTAINER_SET_CONTENT,
    CLIENTBOUND_CONTAINER_SET_DATA, CLIENTBOUND_OPEN_SCREEN, ContainerClick, FurnaceMenuKind,
    ItemStack, MENU_TYPE_BREWING_STAND, MENU_TYPE_FURNACE, MENU_TYPE_GENERIC_9X3, MenuError,
    MenuKind, SessionMenus,
};
use folia_core::item_components::ComponentValue;
use folia_core::nbt::Tag;
use folia_core::protocol::PacketReader;
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn pos() -> BlockEntityPos {
    BlockEntityPos::new(4, 70, -3)
}

fn other_pos() -> BlockEntityPos {
    BlockEntityPos::new(5, 70, -3)
}

fn chest_container() -> Vec<ItemStack> {
    vec![ItemStack::empty(); CHEST_SLOTS]
}

fn click(
    container_id: i32,
    state_id: i32,
    slot: i32,
    button: i32,
    click_type: i32,
) -> ContainerClick {
    ContainerClick {
        container_id,
        state_id,
        slot,
        button,
        click_type,
        carried: ItemStack::empty(),
    }
}

fn packet_id(packet: &[u8]) -> i32 {
    PacketReader::new(packet).read_varint().unwrap()
}

fn parse_content(packet: &[u8]) -> (i32, i32, Vec<ItemStack>, ItemStack) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(
        reader.read_varint().unwrap(),
        CLIENTBOUND_CONTAINER_SET_CONTENT
    );
    let container_id = reader.read_varint().unwrap();
    let state_id = reader.read_varint().unwrap();
    let count = reader.read_varint().unwrap();
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        items.push(inventory::read_item_stack(&mut reader).expect("menu item stack"));
    }
    let cursor = inventory::read_item_stack(&mut reader).expect("carried stack");
    assert!(reader.is_empty(), "trailing bytes in Set Content");
    (container_id, state_id, items, cursor)
}

fn parse_data(packet: &[u8]) -> (i32, i16, i16) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(
        reader.read_varint().unwrap(),
        CLIENTBOUND_CONTAINER_SET_DATA
    );
    let container_id = reader.read_varint().unwrap();
    let data_id = reader.read_i16().unwrap();
    let value = reader.read_i16().unwrap();
    assert!(reader.is_empty(), "trailing bytes in Set Data");
    (container_id, data_id, value)
}

fn open_chest(menus: &mut SessionMenus, container: Vec<ItemStack>) -> inventory::MenuOpenPackets {
    let inventory = inventory::default_inventory();
    let cursor = ItemStack::empty();
    menus
        .open_menu(MenuKind::Chest, pos(), container, &[], &inventory, &cursor)
        .expect("open chest menu")
}

#[test]
fn chest_open_screen_and_content_match_the_vanilla_layout() {
    let mut menus = SessionMenus::new();
    let mut container = chest_container();
    container[0] = ItemStack::new("minecraft:diamond", 3);
    let packets = open_chest(&mut menus, container);

    assert_eq!(packet_id(&packets.open_screen), CLIENTBOUND_OPEN_SCREEN);
    let mut reader = PacketReader::new(&packets.open_screen);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_OPEN_SCREEN);
    assert_eq!(reader.read_varint().unwrap(), 1, "first container id");
    assert_eq!(
        reader.read_varint().unwrap(),
        MENU_TYPE_GENERIC_9X3,
        "generic_9x3 menu id"
    );
    let title = reader.read_nbt_any().expect("title component");
    assert_eq!(
        title.get("translate").and_then(Tag::as_str),
        Some("container.chest")
    );
    assert!(reader.is_empty());

    let (container_id, state_id, items, cursor) = parse_content(&packets.content);
    assert_eq!(container_id, 1);
    assert_eq!(state_id, 0, "initial state id");
    assert_eq!(items.len(), 27 + 36, "chest slots + player inventory");
    assert_eq!(items[0].item, "minecraft:diamond");
    assert_eq!(items[0].count, 3);
    assert_eq!(
        items[54].item, "minecraft:stone",
        "hotbar 0 maps to slot 54"
    );
    assert_eq!(items[27].item, "", "main inventory 9 maps to slot 27");
    assert!(cursor.is_empty());
    assert!(packets.data.is_empty(), "chest has no data slots");
    assert!(menus.open().is_some_and(|menu| menu.container_id == 1));
}

#[test]
fn pickup_moves_between_cursor_and_container_slots() {
    let mut menus = SessionMenus::new();
    open_chest(&mut menus, chest_container());
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::new("minecraft:diamond", 9);

    // Cursor -> slot 0.
    let outcome = menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 0), false)
        .expect("place cursor stack");
    assert!(outcome.changed);
    assert!(!outcome.state_mismatch);
    assert!(cursor.is_empty());
    assert_eq!(menus.open().unwrap().container[0].item, "minecraft:diamond");
    assert_eq!(menus.open().unwrap().container[0].count, 9);
    assert_eq!(menus.open().unwrap().state_id, 1);

    // Slot -> cursor.
    let outcome = menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 0, 0, 0), false)
        .expect("take slot stack");
    assert!(outcome.changed);
    assert_eq!(cursor.item, "minecraft:diamond");
    assert_eq!(cursor.count, 9);
    assert!(menus.open().unwrap().container[0].is_empty());
    assert_eq!(menus.open().unwrap().state_id, 2);
}

#[test]
fn right_click_takes_half_and_places_one() {
    let mut menus = SessionMenus::new();
    let mut container = chest_container();
    container[0] = ItemStack::new("minecraft:apple", 7);
    open_chest(&mut menus, container);
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::empty();

    menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 0, 1, 0), false)
        .expect("right-click take half");
    assert_eq!(cursor.count, 4, "ceil(7/2)");
    assert_eq!(menus.open().unwrap().container[0].count, 3);

    menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 0, 1, 0), false)
        .expect("right-click place one");
    assert_eq!(cursor.count, 3);
    assert_eq!(menus.open().unwrap().container[0].count, 4);
}

#[test]
fn shift_move_transfers_between_chest_and_player_inventory() {
    let mut menus = SessionMenus::new();
    let mut container = chest_container();
    container[0] = ItemStack::new("minecraft:gold_ingot", 10);
    open_chest(&mut menus, container);
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::empty();

    // Chest -> inventory. Reverse scan fills the last hotbar slot first.
    let outcome = menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 1), false)
        .expect("shift chest to inventory");
    assert!(outcome.changed);
    assert!(menus.open().unwrap().container[0].is_empty());
    assert_eq!(inventory[8].item, "minecraft:gold_ingot");
    assert_eq!(inventory[8].count, 10);

    // Inventory -> chest. Slot 62 is hotbar index 8.
    let outcome = menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 62, 0, 1), false)
        .expect("shift inventory to chest");
    assert!(outcome.changed);
    assert!(inventory[8].is_empty());
    assert_eq!(
        menus.open().unwrap().container[0].item,
        "minecraft:gold_ingot"
    );
    assert_eq!(menus.open().unwrap().container[0].count, 10);
}

#[test]
fn shift_move_routes_items_into_furnace_and_brewing_slots() {
    let mut inventory = vec![ItemStack::empty(); 36];
    let mut cursor = ItemStack::empty();

    let furnace = FurnaceBlockEntity::new(pos(), FurnaceKind::Furnace);
    let mut menus = SessionMenus::new();
    menus
        .open_menu(
            MenuKind::Furnace(FurnaceMenuKind::Furnace),
            pos(),
            furnace.items.clone(),
            &furnace.menu_data(),
            &inventory,
            &cursor,
        )
        .expect("open furnace");
    inventory[3] = ItemStack::new("minecraft:coal", 2);
    inventory[4] = ItemStack::new("minecraft:iron_ore", 3);
    // Hotbar slots 3 and 4 map to menu slots 33 and 34.
    menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 33, 0, 1), false)
        .expect("shift coal into fuel");
    menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 34, 0, 1), false)
        .expect("shift ore into input");
    let menu = menus.open().unwrap();
    assert_eq!(menu.container[1].item, "minecraft:coal");
    assert_eq!(menu.container[1].count, 2);
    assert_eq!(menu.container[0].item, "minecraft:iron_ore");
    assert_eq!(menu.container[0].count, 3);

    let stand = BrewingStandBlockEntity::new(pos());
    let mut menus = SessionMenus::new();
    let mut inventory = vec![ItemStack::empty(); 36];
    menus
        .open_menu(
            MenuKind::BrewingStand,
            pos(),
            stand.state.slots().to_vec(),
            &stand.menu_data(),
            &inventory,
            &cursor,
        )
        .expect("open brewing stand");
    inventory[5] = ItemStack::new("minecraft:blaze_powder", 1);
    inventory[6] = ItemStack::new("minecraft:nether_wart", 1);
    inventory[7] = ItemStack::new("minecraft:potion", 1);
    // Hotbar slots 5, 6, 7 map to menu slots 37, 38, 39.
    menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 37, 0, 1), false)
        .expect("shift blaze powder into fuel");
    menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 38, 0, 1), false)
        .expect("shift wart into ingredient");
    menus
        .click(&mut inventory, &mut cursor, &click(1, 2, 39, 0, 1), false)
        .expect("shift potion into bottle slot");
    let menu = menus.open().unwrap();
    assert_eq!(menu.container[4].item, "minecraft:blaze_powder");
    assert_eq!(menu.container[3].item, "minecraft:nether_wart");
    assert_eq!(menu.container[0].item, "minecraft:potion");
}

#[test]
fn furnace_open_and_data_packets_match_the_vanilla_slots() {
    let mut furnace = FurnaceBlockEntity::new(pos(), FurnaceKind::Furnace);
    assert!(furnace.set_slot(0, ItemStack::new("minecraft:iron_ore", 5)));
    assert!(furnace.set_slot(1, ItemStack::new("minecraft:coal", 2)));
    assert!(furnace.set_slot(2, ItemStack::new("minecraft:iron_ingot", 1)));
    furnace.lit_time_remaining = 123;
    furnace.lit_total_time = 200;
    furnace.cooking_time_spent = 50;
    furnace.cooking_total_time = 200;
    assert_eq!(furnace.menu_data(), [123, 200, 50, 200]);

    let mut menus = SessionMenus::new();
    let inventory = inventory::default_inventory();
    let cursor = ItemStack::empty();
    let packets = menus
        .open_menu(
            MenuKind::Furnace(FurnaceMenuKind::Furnace),
            pos(),
            furnace.items.clone(),
            &furnace.menu_data(),
            &inventory,
            &cursor,
        )
        .expect("open furnace menu");

    assert_eq!(packet_id(&packets.open_screen), CLIENTBOUND_OPEN_SCREEN);
    let mut reader = PacketReader::new(&packets.open_screen);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_OPEN_SCREEN);
    assert_eq!(reader.read_varint().unwrap(), 1);
    assert_eq!(reader.read_varint().unwrap(), MENU_TYPE_FURNACE);
    let title = reader.read_nbt_any().unwrap();
    assert_eq!(
        title.get("translate").and_then(Tag::as_str),
        Some("container.furnace")
    );

    let (_, state_id, items, _) = parse_content(&packets.content);
    assert_eq!(state_id, 0);
    assert_eq!(items.len(), FURNACE_SLOTS + 36);
    assert_eq!(items[0].item, "minecraft:iron_ore");
    assert_eq!(items[1].item, "minecraft:coal");
    assert_eq!(items[2].item, "minecraft:iron_ingot");
    assert_eq!(items[30].item, "minecraft:stone", "hotbar 0 -> slot 30");

    assert_eq!(packets.data.len(), 4);
    let values: Vec<(i16, i16)> = packets
        .data
        .iter()
        .map(|packet| {
            let (container_id, data_id, value) = parse_data(packet);
            assert_eq!(container_id, 1);
            (data_id, value)
        })
        .collect();
    assert_eq!(values, vec![(0, 123), (1, 200), (2, 50), (3, 200)]);

    // A world tick advances the furnace; only changed data slots are sent.
    furnace.lit_time_remaining = 100;
    furnace.cooking_time_spent = 73;
    let (contents_changed, data_packets) = menus
        .sync(furnace.items.clone(), &furnace.menu_data())
        .expect("sync furnace data");
    assert!(!contents_changed);
    let changed: Vec<(i16, i16)> = data_packets
        .iter()
        .map(|packet| {
            let (_, data_id, value) = parse_data(packet);
            (data_id, value)
        })
        .collect();
    assert_eq!(changed, vec![(0, 100), (2, 73)]);
}

#[test]
fn brewing_open_and_data_packets_scale_brew_progress() {
    let mut stand = BrewingStandBlockEntity::new(pos());
    assert!(stand.set_slot(0, ItemStack::new("minecraft:potion", 1)));
    assert!(stand.set_slot(3, ItemStack::new("minecraft:nether_wart", 2)));
    assert!(stand.set_slot(4, ItemStack::new("minecraft:blaze_powder", 3)));
    stand.state.brew_time = 200;
    stand.state.recipe_brew_time = 400;
    stand.state.fuel = 7;
    assert_eq!(stand.menu_data(), [200, 7]);

    let mut menus = SessionMenus::new();
    let inventory = inventory::default_inventory();
    let cursor = ItemStack::empty();
    let packets = menus
        .open_menu(
            MenuKind::BrewingStand,
            pos(),
            stand.state.slots().to_vec(),
            &stand.menu_data(),
            &inventory,
            &cursor,
        )
        .expect("open brewing menu");

    let mut reader = PacketReader::new(&packets.open_screen);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_OPEN_SCREEN);
    assert_eq!(reader.read_varint().unwrap(), 1);
    assert_eq!(reader.read_varint().unwrap(), MENU_TYPE_BREWING_STAND);
    let title = reader.read_nbt_any().unwrap();
    assert_eq!(
        title.get("translate").and_then(Tag::as_str),
        Some("container.brewing")
    );

    let (_, _, items, _) = parse_content(&packets.content);
    assert_eq!(items.len(), BREWING_SLOT_COUNT + 36);
    assert_eq!(items[0].item, "minecraft:potion");
    assert_eq!(items[3].item, "minecraft:nether_wart");
    assert_eq!(items[4].item, "minecraft:blaze_powder");
    assert_eq!(items[32].item, "minecraft:stone", "hotbar 0 -> slot 32");

    assert_eq!(packets.data.len(), 2);
    assert_eq!(parse_data(&packets.data[0]), (1, 0, 200));
    assert_eq!(parse_data(&packets.data[1]), (1, 1, 7));

    stand.state.brew_time = 100;
    stand.state.fuel = 6;
    let (contents_changed, data_packets) = menus
        .sync(stand.state.slots().to_vec(), &stand.menu_data())
        .expect("sync brewing data");
    assert!(!contents_changed);
    assert_eq!(parse_data(&data_packets[0]), (1, 0, 100));
    assert_eq!(parse_data(&data_packets[1]), (1, 1, 6));
}

#[test]
fn invalid_clicks_and_open_arguments_are_rejected() {
    let mut menus = SessionMenus::new();
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::empty();
    assert_eq!(
        menus
            .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 0), false)
            .unwrap_err(),
        MenuError::NoOpenMenu
    );

    open_chest(&mut menus, chest_container());
    assert_eq!(
        menus
            .click(&mut inventory, &mut cursor, &click(99, 0, 0, 0, 0), false)
            .unwrap_err(),
        MenuError::ContainerMismatch {
            expected: 1,
            got: 99
        }
    );
    assert_eq!(
        menus
            .click(&mut inventory, &mut cursor, &click(1, 0, 63, 0, 0), false)
            .unwrap_err(),
        MenuError::InvalidSlot(63)
    );
    assert_eq!(
        menus
            .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 5), false)
            .unwrap_err(),
        MenuError::UnsupportedClickType(5)
    );
    assert_eq!(
        menus
            .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 3), false)
            .unwrap_err(),
        MenuError::NotCreative
    );

    // Wrong container size and wrong data-slot count fail at open time.
    assert_eq!(
        menus
            .open_menu(
                MenuKind::Furnace(FurnaceMenuKind::Furnace),
                pos(),
                vec![ItemStack::empty(); 3],
                &[],
                &inventory,
                &cursor,
            )
            .unwrap_err(),
        MenuError::WrongDataCount {
            expected: 4,
            got: 0
        }
    );

    // The result slot and a non-fuel item in the fuel slot are rejected.
    let furnace = FurnaceBlockEntity::new(pos(), FurnaceKind::Furnace);
    let mut menus = SessionMenus::new();
    menus
        .open_menu(
            MenuKind::Furnace(FurnaceMenuKind::Furnace),
            pos(),
            furnace.items.clone(),
            &furnace.menu_data(),
            &inventory,
            &cursor,
        )
        .unwrap();
    let mut placed = ItemStack::new("minecraft:stone", 1);
    assert_eq!(
        menus
            .click(&mut inventory, &mut placed, &click(1, 0, 2, 0, 0), false)
            .unwrap_err(),
        MenuError::SlotRejectsItem(2)
    );
    assert_eq!(
        menus
            .click(&mut inventory, &mut placed, &click(1, 0, 1, 0, 0), false)
            .unwrap_err(),
        MenuError::SlotRejectsItem(1)
    );

    // A state-id mismatch still applies the click but asks the caller to
    // resynchronise the full menu.
    let mut menus = SessionMenus::new();
    open_chest(&mut menus, chest_container());
    let mut cursor = ItemStack::new("minecraft:diamond", 1);
    let outcome = menus
        .click(&mut inventory, &mut cursor, &click(1, 99, 0, 0, 0), false)
        .expect("stale state id is applied");
    assert!(outcome.changed);
    assert!(outcome.state_mismatch);
}

#[test]
fn disconnect_and_block_removal_close_the_menu() {
    let mut menus = SessionMenus::new();
    let packets = open_chest(&mut menus, chest_container());
    let menu = menus.open().expect("open menu");
    let close = menu.close_packet();
    assert_eq!(packet_id(&close), CLIENTBOUND_CONTAINER_CLOSE);
    let mut reader = PacketReader::new(&close);
    assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_CONTAINER_CLOSE);
    assert_eq!(reader.read_varint().unwrap(), 1);
    assert!(reader.is_empty());
    assert!(!packets.open_screen.is_empty());

    menus.on_disconnect();
    assert!(!menus.is_open());
    assert!(menus.close().is_none());

    open_chest(&mut menus, chest_container());
    assert!(menus.on_block_removed(other_pos()).is_none());
    assert!(menus.is_open());
    let closed = menus
        .on_block_removed(pos())
        .expect("menu closes with its block");
    assert_eq!(
        packet_id(&closed.close_packet()),
        CLIENTBOUND_CONTAINER_CLOSE
    );
    assert!(!menus.is_open());
}

#[test]
fn menu_edits_survive_a_chunk_reload() {
    use folia_core::anvil::Compression;
    use folia_core::chunk::BlockState;
    use folia_core::chunk_packet::flat_chunk;

    let chest_pos = BlockEntityPos::new(1, -60, 1);
    let temp = TempDir::new("menu-reload");
    let region = temp.path().join("region");
    std::fs::create_dir_all(&region).unwrap();
    let mut chunk = flat_chunk(0, 0);
    assert!(chunk.set_block_state(
        1,
        -60,
        1,
        BlockState {
            name: "minecraft:chest".into(),
            properties: vec![],
        }
    ));
    {
        let mut storage = WorldStorage::new(&region);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        let mut chest = ChestBlockEntity::new(chest_pos);
        chest.set_slot(0, ItemStack::new("minecraft:emerald", 4));
        storage
            .insert_block_entity(BlockEntity::Chest(chest))
            .unwrap();
        storage
            .save_chunk_with_block_entities(&chunk, Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();
    }

    let mut storage = WorldStorage::new(&region);
    let chest = match storage
        .block_entity(chest_pos)
        .unwrap()
        .expect("stored chest")
        .clone()
    {
        BlockEntity::Chest(chest) => chest,
        _ => panic!("stored entity is a chest"),
    };
    let mut menus = SessionMenus::new();
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::empty();
    menus
        .open_menu(
            MenuKind::Chest,
            chest_pos,
            chest.items.clone(),
            &[],
            &inventory,
            &cursor,
        )
        .expect("open stored chest");
    menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 0), false)
        .expect("take emeralds");
    menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 26, 0, 0), false)
        .expect("move emeralds to the last chest slot");
    let menu = menus.open().unwrap().clone();
    assert!(folia_core::join::persist_menu_container(&mut storage, &menu).unwrap());
    storage.flush().unwrap();

    let mut reopened = WorldStorage::new(&region);
    let loaded = reopened.load_chunk(0, 0).unwrap().expect("chunk reloads");
    assert_eq!(
        loaded
            .block_state(1, -60, 1)
            .map(|state| state.name.as_str()),
        Some("minecraft:chest")
    );
    let stored = reopened
        .block_entity(chest_pos)
        .unwrap()
        .expect("chest reloads")
        .clone();
    let BlockEntity::Chest(stored) = stored else {
        panic!("reloaded entity is a chest");
    };
    assert!(stored.items[0].is_empty());
    assert_eq!(stored.items[26].item, "minecraft:emerald");
    assert_eq!(stored.items[26].count, 4);
}

#[test]
fn components_survive_clicks_and_block_entity_persistence() {
    let mut named = ItemStack::new("minecraft:diamond_pickaxe", 1);
    named.set_component(ComponentValue::CustomName(Tag::String("Miner".into())));
    let mut chest = ChestBlockEntity::new(pos());
    assert!(chest.set_slot(0, named.clone()));

    let temp = TempDir::new("menu-components");
    let mut storage = WorldStorage::new(temp.path());
    storage
        .insert_block_entity(BlockEntity::Chest(chest.clone()))
        .expect("insert chest");

    let mut menus = SessionMenus::new();
    let mut inventory = inventory::default_inventory();
    let mut cursor = ItemStack::empty();
    let chest_items = BlockEntity::Chest(chest.clone())
        .container_items()
        .expect("chest container items");
    menus
        .open_menu(
            MenuKind::Chest,
            pos(),
            chest_items,
            &[],
            &inventory,
            &cursor,
        )
        .expect("open chest");

    menus
        .click(&mut inventory, &mut cursor, &click(1, 0, 0, 0, 0), false)
        .expect("pick up named stack");
    assert_eq!(cursor.item, "minecraft:diamond_pickaxe");
    assert_eq!(
        cursor.get_component("minecraft:custom_name"),
        Some(&ComponentValue::CustomName(Tag::String("Miner".into())))
    );

    menus
        .click(&mut inventory, &mut cursor, &click(1, 1, 5, 0, 0), false)
        .expect("place named stack in another chest slot");
    assert!(cursor.is_empty());
    assert_eq!(
        menus.open().unwrap().container[5].components,
        named.components
    );

    let menu = menus.open().unwrap().clone();
    assert!(
        folia_core::join::persist_menu_container(&mut storage, &menu).expect("persist container")
    );
    let stored = storage
        .block_entity(pos())
        .expect("read block entity")
        .expect("chest still exists")
        .clone();
    let BlockEntity::Chest(stored_chest) = stored else {
        panic!("stored entity is a chest");
    };
    assert_eq!(stored_chest.items[5], named);

    // The same component patch survives a chunk-NBT round trip.
    let tag = BlockEntity::Chest(stored_chest)
        .to_nbt()
        .expect("encode chest NBT");
    let BlockEntity::Chest(decoded) = BlockEntity::from_nbt(&tag).expect("decode chest NBT") else {
        panic!("decoded entity is a chest");
    };
    assert_eq!(decoded.items[5], named);
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "folia-rs-menu-{}-{}-{label}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
