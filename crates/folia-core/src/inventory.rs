use crate::chunk::BlockState;
use crate::protocol::{PacketReader, PacketWriter};

pub const CLIENTBOUND_CONTAINER_SET_CONTENT: i32 = 18;
pub const CLIENTBOUND_CONTAINER_SET_SLOT: i32 = 20;
pub const CLIENTBOUND_SET_HELD_SLOT: i32 = 103;
pub const CLIENTBOUND_SET_PLAYER_INVENTORY: i32 = 106;
pub const SERVERBOUND_CONTAINER_CLICK: i32 = 17;
pub const SERVERBOUND_SET_CREATIVE_MODE_SLOT: i32 = 55;
pub const SERVERBOUND_SET_CARRIED_ITEM: i32 = 52;

pub const INVENTORY_SLOTS: usize = 36;
pub const HOTBAR_SLOTS: usize = 9;
pub const CONTAINER_SLOTS: usize = 46;
pub const MAX_STACK: i32 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    pub item: String,
    pub count: i32,
}

impl ItemStack {
    pub fn new(item: impl Into<String>, count: i32) -> Self {
        Self {
            item: item.into(),
            count,
        }
    }

    pub fn empty() -> Self {
        Self {
            item: String::new(),
            count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count <= 0 || self.item.is_empty()
    }

    fn same_item(&self, other: &Self) -> bool {
        !self.is_empty() && !other.is_empty() && self.item == other.item
    }
}

pub fn item_id(item: &str) -> Option<i32> {
    crate::registry::item_id(item)
}

pub fn item_name(id: i32) -> Option<&'static str> {
    crate::registry::item_name(id)
}

pub fn block_for_item(item: &str) -> Option<BlockState> {
    crate::registry::default_block_state(item)
}

pub fn default_inventory() -> Vec<ItemStack> {
    let mut slots = vec![ItemStack::empty(); INVENTORY_SLOTS];
    slots[0] = ItemStack::new("minecraft:stone", MAX_STACK);
    slots[1] = ItemStack::new("minecraft:dirt", MAX_STACK);
    slots[2] = ItemStack::new("minecraft:grass_block", MAX_STACK);
    slots
}

pub fn held_slot_packet(slot: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_SET_HELD_SLOT).unwrap();
    writer.write_varint(slot).unwrap();
    writer.into_inner()
}

pub fn inventory_packet(slot: i32, stack: &ItemStack) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_SET_PLAYER_INVENTORY)
        .unwrap();
    writer.write_varint(slot).unwrap();
    write_item_stack(&mut writer, stack);
    writer.into_inner()
}

pub fn container_set_slot_packet(menu_slot: i32, stack: &ItemStack) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_CONTAINER_SET_SLOT).unwrap();
    writer.write_varint(0).unwrap(); // player inventory container
    writer.write_varint(0).unwrap(); // state id
    writer.write_i16(menu_slot as i16).unwrap();
    write_item_stack(&mut writer, stack);
    writer.into_inner()
}

pub fn container_content_packet(inventory: &[ItemStack], cursor: &ItemStack) -> Vec<u8> {
    let menu = container_items(inventory);
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_CONTAINER_SET_CONTENT)
        .unwrap();
    writer.write_varint(0).unwrap(); // player inventory container
    writer.write_varint(0).unwrap(); // state id
    writer.write_varint(menu.len() as i32).unwrap();
    for stack in menu {
        write_item_stack(&mut writer, &stack);
    }
    write_item_stack(&mut writer, cursor);
    writer.into_inner()
}

/// Convert the 36-slot `Inventory` model into the 46-slot vanilla menu layout.
pub fn container_items(inventory: &[ItemStack]) -> Vec<ItemStack> {
    let mut menu = vec![ItemStack::empty(); CONTAINER_SLOTS];
    for (index, stack) in inventory.iter().take(INVENTORY_SLOTS).enumerate() {
        let menu_slot = if index < HOTBAR_SLOTS {
            36 + index
        } else {
            index
        };
        if menu_slot < CONTAINER_SLOTS {
            menu[menu_slot] = stack.clone();
        }
    }
    menu
}

pub fn inventory_index(menu_slot: i32) -> Option<usize> {
    match menu_slot {
        9..=35 => Some(menu_slot as usize),
        36..=44 => Some((menu_slot - 36) as usize),
        _ => None,
    }
}

pub fn menu_slot(inventory_index: usize) -> Option<i32> {
    if inventory_index < HOTBAR_SLOTS {
        Some(36 + inventory_index as i32)
    } else if inventory_index < INVENTORY_SLOTS {
        Some(inventory_index as i32)
    } else {
        None
    }
}

pub fn write_item_stack(writer: &mut PacketWriter, stack: &ItemStack) {
    if stack.is_empty() {
        writer.write_varint(0).unwrap();
        return;
    }
    let Some(id) = item_id(&stack.item) else {
        writer.write_varint(0).unwrap();
        return;
    };
    writer.write_varint(stack.count).unwrap();
    writer.write_varint(id).unwrap();
    writer.write_varint(0).unwrap(); // added components
    writer.write_varint(0).unwrap(); // removed components
}

pub fn read_item_stack(reader: &mut PacketReader<'_>) -> Option<ItemStack> {
    let count = reader.read_varint().ok()?;
    if count <= 0 {
        return Some(ItemStack::empty());
    }
    let id = reader.read_varint().ok()?;
    // We only support stacks without component patches in this model.
    let added = reader.read_varint().ok()?;
    if added != 0 {
        return None;
    }
    let removed = reader.read_varint().ok()?;
    if removed != 0 {
        return None;
    }
    Some(ItemStack::new(item_name(id)?, count))
}

pub fn read_hashed_stack(reader: &mut PacketReader<'_>) -> Option<ItemStack> {
    if !reader.read_bool().ok()? {
        return Some(ItemStack::empty());
    }
    let id = reader.read_varint().ok()?;
    let count = reader.read_varint().ok()?;
    let added = reader.read_varint().ok()?;
    for _ in 0..added {
        let _component = reader.read_varint().ok()?;
        let _hash = reader.read_i32().ok()?;
    }
    let removed = reader.read_varint().ok()?;
    for _ in 0..removed {
        let _component = reader.read_varint().ok()?;
    }
    Some(ItemStack::new(item_name(id)?, count))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerClick {
    pub container_id: i32,
    pub state_id: i32,
    pub slot: i32,
    pub button: i32,
    pub click_type: i32,
    pub carried: ItemStack,
}

pub fn parse_container_click(packet: &[u8]) -> Option<ContainerClick> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint().ok()? != SERVERBOUND_CONTAINER_CLICK {
        return None;
    }
    let container_id = reader.read_varint().ok()?;
    let state_id = reader.read_varint().ok()?;
    let slot = i32::from(reader.read_i16().ok()?);
    let button = i32::from(reader.read_u8().ok()? as i8);
    let click_type = reader.read_varint().ok()?;
    let changed = reader.read_varint().ok()?;
    for _ in 0..changed {
        let _slot = reader.read_i16().ok()?;
        let _stack = read_hashed_stack(&mut reader)?;
    }
    let carried = read_hashed_stack(&mut reader)?;
    Some(ContainerClick {
        container_id,
        state_id,
        slot,
        button,
        click_type,
        carried,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreativeSlot {
    pub slot: i32,
    pub stack: ItemStack,
}

pub fn parse_creative_slot(packet: &[u8]) -> Option<CreativeSlot> {
    let mut reader = PacketReader::new(packet);
    if reader.read_varint().ok()? != SERVERBOUND_SET_CREATIVE_MODE_SLOT {
        return None;
    }
    let slot = i32::from(reader.read_i16().ok()?);
    let stack = read_item_stack(&mut reader)?;
    Some(CreativeSlot { slot, stack })
}

/// Apply a player-inventory click. The packet's carried stack is used as a
/// consistency hint, but the authoritative cursor is kept in the session.
pub fn apply_click(
    inventory: &mut [ItemStack],
    cursor: &mut ItemStack,
    click: ContainerClick,
) -> bool {
    let Some(index) = inventory_index(click.slot) else {
        return false;
    };
    if index >= inventory.len() {
        return false;
    }

    match click.click_type {
        0 => {
            // PICKUP
            if click.button == 1 {
                if cursor.is_empty() {
                    let half = (inventory[index].count + 1) / 2;
                    if half <= 0 {
                        return false;
                    }
                    *cursor = ItemStack::new(inventory[index].item.clone(), half);
                    inventory[index].count -= half;
                    if inventory[index].count <= 0 {
                        inventory[index] = ItemStack::empty();
                    }
                } else if inventory[index].is_empty() || inventory[index].same_item(cursor) {
                    if inventory[index].is_empty() {
                        inventory[index] = ItemStack::new(cursor.item.clone(), 1);
                    } else if inventory[index].count < MAX_STACK {
                        inventory[index].count += 1;
                    } else {
                        return false;
                    }
                    cursor.count -= 1;
                    if cursor.count <= 0 {
                        *cursor = ItemStack::empty();
                    }
                }
            } else if cursor.is_empty() {
                *cursor = inventory[index].clone();
                inventory[index] = ItemStack::empty();
            } else if inventory[index].is_empty() {
                inventory[index] = cursor.clone();
                *cursor = ItemStack::empty();
            } else if inventory[index].same_item(cursor) {
                let space = MAX_STACK - inventory[index].count;
                let moved = space.min(cursor.count);
                inventory[index].count += moved;
                cursor.count -= moved;
                if cursor.count <= 0 {
                    *cursor = ItemStack::empty();
                }
            } else {
                std::mem::swap(&mut inventory[index], cursor);
            }
            true
        }
        1 => {
            // QUICK_MOVE: move to the other half of the inventory.
            if inventory[index].is_empty() {
                return false;
            }
            let range = if index < HOTBAR_SLOTS {
                9..INVENTORY_SLOTS
            } else {
                0..HOTBAR_SLOTS
            };
            for target in range {
                if inventory[target].is_empty() {
                    inventory[target] = inventory[index].clone();
                    inventory[index] = ItemStack::empty();
                    return true;
                }
                if inventory[target].same_item(&inventory[index])
                    && inventory[target].count < MAX_STACK
                {
                    let space = MAX_STACK - inventory[target].count;
                    let moved = space.min(inventory[index].count);
                    inventory[target].count += moved;
                    inventory[index].count -= moved;
                    if inventory[index].count <= 0 {
                        inventory[index] = ItemStack::empty();
                    }
                    return true;
                }
            }
            false
        }
        2 => {
            // SWAP with a hotbar slot.
            if !(0..HOTBAR_SLOTS as i32).contains(&click.button) {
                return false;
            }
            let hotbar = click.button as usize;
            if hotbar == index {
                return false;
            }
            inventory.swap(index, hotbar);
            true
        }
        3 => {
            // CLONE
            *cursor = inventory[index].clone();
            true
        }
        4 => {
            // THROW
            if !cursor.is_empty() {
                cursor.count -= 1;
                if cursor.count <= 0 {
                    *cursor = ItemStack::empty();
                }
            } else if !inventory[index].is_empty() {
                inventory[index].count -= 1;
                if inventory[index].count <= 0 {
                    inventory[index] = ItemStack::empty();
                }
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn held_slot_body_matches_java() {
        let packet = held_slot_packet(2);
        assert_eq!(hex(&packet[1..]), "02");
    }

    #[test]
    fn stone_stack_body_matches_java() {
        let packet = inventory_packet(0, &ItemStack::new("minecraft:stone", 1));
        assert_eq!(hex(&packet[1..]), "0001010000");
    }

    #[test]
    fn container_slot_body_matches_manual_java_encoding() {
        let packet = container_set_slot_packet(36, &ItemStack::new("minecraft:stone", 1));
        assert_eq!(hex(&packet[1..]), "0000002401010000");
    }

    #[test]
    fn container_layout_maps_hotbar_and_main_slots() {
        let inventory = default_inventory();
        let menu = container_items(&inventory);
        assert_eq!(menu.len(), CONTAINER_SLOTS);
        assert_eq!(menu[36].item, "minecraft:stone");
        assert_eq!(menu[37].item, "minecraft:dirt");
        assert_eq!(menu[9].item, "");
        assert_eq!(inventory_index(36), Some(0));
        assert_eq!(inventory_index(35), Some(35));
        assert_eq!(inventory_index(8), None);
        assert_eq!(menu_slot(0), Some(36));
        assert_eq!(menu_slot(35), Some(35));
    }

    #[test]
    fn pickup_click_moves_slot_to_cursor() {
        let mut inventory = default_inventory();
        let mut cursor = ItemStack::empty();
        let click = ContainerClick {
            container_id: 0,
            state_id: 0,
            slot: 36,
            button: 0,
            click_type: 0,
            carried: ItemStack::new("minecraft:stone", MAX_STACK),
        };
        assert!(apply_click(&mut inventory, &mut cursor, click));
        assert!(inventory[0].is_empty());
        assert_eq!(cursor.item, "minecraft:stone");
        assert_eq!(cursor.count, MAX_STACK);
    }

    #[test]
    fn place_click_moves_cursor_to_empty_slot() {
        let mut inventory = default_inventory();
        let mut cursor = ItemStack::new("minecraft:dirt", 5);
        let click = ContainerClick {
            container_id: 0,
            state_id: 0,
            slot: 40, // hotbar slot 4
            button: 0,
            click_type: 0,
            carried: ItemStack::empty(),
        };
        assert!(apply_click(&mut inventory, &mut cursor, click));
        assert!(cursor.is_empty());
        assert_eq!(inventory[4].item, "minecraft:dirt");
        assert_eq!(inventory[4].count, 5);
    }

    #[test]
    fn default_inventory_has_36_slots_and_three_blocks() {
        let inventory = default_inventory();
        assert_eq!(inventory.len(), INVENTORY_SLOTS);
        assert_eq!(inventory[0].count, MAX_STACK);
        assert_eq!(inventory[1].item, "minecraft:dirt");
        assert_eq!(inventory[2].item, "minecraft:grass_block");
        assert!(inventory[3..].iter().all(ItemStack::is_empty));
        assert!(block_for_item("minecraft:stick").is_none());
    }
}
