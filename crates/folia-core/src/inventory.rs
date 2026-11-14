use crate::block_entity::BlockEntityPos;
use crate::chunk::BlockState;
use crate::crafting::FurnaceKind;
use crate::protocol::{PacketReader, PacketWriter};

pub use crate::item_components::{
    ComponentMap, ComponentType, ComponentValue, ItemStack, component_id, component_name,
    item_stack_from_nbt, item_stack_to_nbt, read_item_stack, write_item_stack,
    write_item_stack_report,
};

// Play packet ids from the 1.21.11 `GameProtocols` registration order. Play has
// a bundle packet at index 0, so these are one higher than the `addPacket`
// index. The decompiled registrations are the authority; an unknown menu id
// must fail instead of falling back to a guessed value.
pub const CLIENTBOUND_CONTAINER_CLOSE: i32 = 17;
pub const CLIENTBOUND_CONTAINER_SET_CONTENT: i32 = 18;
pub const CLIENTBOUND_CONTAINER_SET_DATA: i32 = 19;
pub const CLIENTBOUND_CONTAINER_SET_SLOT: i32 = 20;
pub const CLIENTBOUND_OPEN_SCREEN: i32 = 57;
pub const CLIENTBOUND_SET_HELD_SLOT: i32 = 103;
pub const CLIENTBOUND_SET_PLAYER_INVENTORY: i32 = 106;
pub const SERVERBOUND_CONTAINER_CLICK: i32 = 17;
pub const SERVERBOUND_CONTAINER_CLOSE: i32 = 18;
pub const SERVERBOUND_SET_CREATIVE_MODE_SLOT: i32 = 55;
pub const SERVERBOUND_SET_CARRIED_ITEM: i32 = 52;

/// `MenuType` registry ids, in `MenuType` static-field registration order.
pub const MENU_TYPE_GENERIC_9X3: i32 = 2;
pub const MENU_TYPE_BLAST_FURNACE: i32 = 10;
pub const MENU_TYPE_BREWING_STAND: i32 = 11;
pub const MENU_TYPE_FURNACE: i32 = 14;
pub const MENU_TYPE_SMOKER: i32 = 22;

pub const FURNACE_DATA_SLOTS: usize = 4;
pub const BREWING_DATA_SLOTS: usize = 2;

pub const INVENTORY_SLOTS: usize = 36;
pub const HOTBAR_SLOTS: usize = 9;
pub const CONTAINER_SLOTS: usize = 46;
pub const MAX_STACK: i32 = 64;

fn same_stack(a: &ItemStack, b: &ItemStack) -> bool {
    a.same_item_and_components(b)
}

/// Merge `source` into `target` when the item and full component patch match,
/// respecting the target's max stack size. Returns the number moved.
pub fn merge_stacks(target: &mut ItemStack, source: &mut ItemStack) -> i32 {
    if !same_stack(target, source) {
        return 0;
    }
    let space = target.max_stack_size() - target.count;
    if space <= 0 {
        return 0;
    }
    let moved = space.min(source.count);
    target.count += moved;
    source.shrink(moved);
    moved
}

/// Insert `stack` into the inventory, merging with matching stacks first and
/// then filling empty slots. Returns the part that did not fit (an empty stack
/// when everything was accepted).
pub fn add_stack(inventory: &mut [ItemStack], mut stack: ItemStack) -> ItemStack {
    if stack.is_empty() {
        return ItemStack::empty();
    }
    let max_stack = stack.max_stack_size();
    for slot in inventory.iter_mut() {
        if !same_stack(slot, &stack) {
            continue;
        }
        let space = slot.max_stack_size() - slot.count;
        if space > 0 {
            let moved = space.min(stack.count);
            slot.count += moved;
            stack.shrink(moved);
            if stack.is_empty() {
                return ItemStack::empty();
            }
        }
    }
    for slot in inventory.iter_mut() {
        if !slot.is_empty() {
            continue;
        }
        let moved = max_stack.min(stack.count);
        *slot = stack.copy_with_count(moved);
        stack.shrink(moved);
        if stack.is_empty() {
            return ItemStack::empty();
        }
    }
    stack
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

pub fn container_content_packet_with_armor(
    inventory: &[ItemStack],
    armor: &[ItemStack; 4],
    cursor: &ItemStack,
) -> Vec<u8> {
    let mut menu = container_items(inventory);
    // Menu slots: 5=head (armor[3]), 6=chest (armor[2]), 7=legs (armor[1]), 8=feet (armor[0]).
    if menu.len() > 8 {
        menu[5] = armor[3].clone();
        menu[6] = armor[2].clone();
        menu[7] = armor[1].clone();
        menu[8] = armor[0].clone();
    }
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

// ---------------------------------------------------------------------------
// Container menus
// ---------------------------------------------------------------------------

/// Furnace menu variants. A campfire has no menu, so it is excluded instead of
/// being mapped onto the ordinary furnace screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FurnaceMenuKind {
    Furnace,
    BlastFurnace,
    Smoker,
}

impl FurnaceMenuKind {
    pub const fn from_block_kind(kind: FurnaceKind) -> Option<Self> {
        match kind {
            FurnaceKind::Furnace => Some(Self::Furnace),
            FurnaceKind::BlastFurnace => Some(Self::BlastFurnace),
            FurnaceKind::Smoker => Some(Self::Smoker),
            FurnaceKind::Campfire => None,
        }
    }
}

/// The block-backed menus this port opens. The variant is part of the menu
/// type because a blast furnace and a smoker use different `MenuType` ids but
/// share the furnace slot layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Chest,
    Furnace(FurnaceMenuKind),
    BrewingStand,
}

impl MenuKind {
    /// Number of block-entity slots before the player inventory starts.
    pub const fn container_slots(self) -> usize {
        match self {
            Self::Chest => crate::block_entity::CHEST_SLOTS,
            Self::Furnace(_) => crate::block_entity::FURNACE_SLOTS,
            Self::BrewingStand => crate::crafting::BREWING_SLOT_COUNT,
        }
    }

    /// `Registries.MENU` numeric id used by `ClientboundOpenScreenPacket`.
    pub const fn menu_type_id(self) -> i32 {
        match self {
            Self::Chest => MENU_TYPE_GENERIC_9X3,
            Self::Furnace(FurnaceMenuKind::BlastFurnace) => MENU_TYPE_BLAST_FURNACE,
            Self::Furnace(FurnaceMenuKind::Smoker) => MENU_TYPE_SMOKER,
            Self::Furnace(FurnaceMenuKind::Furnace) => MENU_TYPE_FURNACE,
            Self::BrewingStand => MENU_TYPE_BREWING_STAND,
        }
    }

    /// Translation key used as the screen title, matching the vanilla block
    /// entity's default name.
    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Chest => "container.chest",
            Self::Furnace(FurnaceMenuKind::BlastFurnace) => "container.blast_furnace",
            Self::Furnace(FurnaceMenuKind::Smoker) => "container.smoker",
            Self::Furnace(FurnaceMenuKind::Furnace) => "container.furnace",
            Self::BrewingStand => "container.brewing",
        }
    }

    /// Number of `ClientboundContainerSetData` slots this menu exposes.
    pub const fn data_slots(self) -> usize {
        match self {
            Self::Chest => 0,
            Self::Furnace(_) => FURNACE_DATA_SLOTS,
            Self::BrewingStand => BREWING_DATA_SLOTS,
        }
    }
}

fn furnace_can_smelt(kind: FurnaceMenuKind, item: &str) -> bool {
    use crate::loot::{CookingKind, RecipeCatalog, RecipeKind};
    let wanted = match kind {
        FurnaceMenuKind::Furnace => CookingKind::Smelting,
        FurnaceMenuKind::BlastFurnace => CookingKind::Blasting,
        FurnaceMenuKind::Smoker => CookingKind::Smoking,
    };
    RecipeCatalog::global()
        .recipes()
        .any(|(_, recipe)| match &recipe.kind {
            RecipeKind::Cooking(cooking) => {
                cooking.cooking == wanted && cooking.ingredient.matches(item)
            }
            _ => false,
        })
}

/// One open container menu. The player inventory is not copied here; menu
/// slots above the container view map onto `PlayerSession::inventory`, matching
/// vanilla's menu slots holding references to the player inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenMenu {
    pub kind: MenuKind,
    /// `AbstractContainerMenu.containerId`; 0 is reserved for the player's own
    /// inventory menu.
    pub container_id: i32,
    /// Monotonic (mod 32768) anti-desync state id.
    pub state_id: i32,
    pub pos: BlockEntityPos,
    pub container: Vec<ItemStack>,
    /// Wire values for `ClientboundContainerSetData`, in vanilla data-slot
    /// order (furnace: flame/progress; brewing: brew progress/fuel).
    pub data: Vec<i16>,
}

/// Packets needed to open a menu, in vanilla order: Open Screen, Set Content,
/// then every Set Data slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOpenPackets {
    pub open_screen: Vec<u8>,
    pub content: Vec<u8>,
    pub data: Vec<Vec<u8>>,
}

/// Result of applying one `ServerboundContainerClick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuClickOutcome {
    pub changed: bool,
    /// The packet's state id did not match the menu's. Vanilla still processes
    /// the click and then broadcasts the full menu state.
    pub state_mismatch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuError {
    NoOpenMenu,
    ContainerMismatch { expected: i32, got: i32 },
    InvalidSlot(i32),
    InvalidButton(i32),
    UnsupportedClickType(i32),
    NotCreative,
    WrongContainerSize { expected: usize, got: usize },
    SlotRejectsItem(i32),
    WrongDataCount { expected: usize, got: usize },
}

impl std::fmt::Display for MenuError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOpenMenu => write!(formatter, "no container menu is open"),
            Self::ContainerMismatch { expected, got } => {
                write!(
                    formatter,
                    "click targets container {got}, expected {expected}"
                )
            }
            Self::InvalidSlot(slot) => write!(formatter, "invalid menu slot {slot}"),
            Self::InvalidButton(button) => {
                write!(formatter, "invalid button {button} for this click type")
            }
            Self::UnsupportedClickType(click_type) => {
                write!(formatter, "unsupported container click type {click_type}")
            }
            Self::NotCreative => write!(formatter, "clone clicks require creative mode"),
            Self::WrongContainerSize { expected, got } => {
                write!(formatter, "container has {got} slots, expected {expected}")
            }
            Self::SlotRejectsItem(slot) => {
                write!(formatter, "menu slot {slot} does not accept that item")
            }
            Self::WrongDataCount { expected, got } => {
                write!(formatter, "menu has {expected} data slots, got {got}")
            }
        }
    }
}

impl std::error::Error for MenuError {}

impl OpenMenu {
    pub fn inventory_start(&self) -> usize {
        self.container.len()
    }

    pub fn menu_slots(&self) -> usize {
        self.container.len() + INVENTORY_SLOTS
    }

    /// Map a player-inventory index (0-8 hotbar, 9-35 main) to this menu's
    /// slot index. Mirrors `addStandardInventorySlots`: main inventory first,
    /// then the hotbar.
    pub fn menu_slot_for_inventory(&self, index: usize) -> Option<usize> {
        if index >= INVENTORY_SLOTS {
            return None;
        }
        let start = self.inventory_start();
        if index < HOTBAR_SLOTS {
            Some(start + 27 + index)
        } else {
            Some(start + index - 9)
        }
    }

    /// Inverse of [`Self::menu_slot_for_inventory`].
    pub fn inventory_index_for_slot(&self, slot: usize) -> Option<usize> {
        let start = self.inventory_start();
        if slot < start {
            return None;
        }
        let offset = slot - start;
        if offset < 27 {
            Some(offset + 9)
        } else if offset < 36 {
            Some(offset - 27)
        } else {
            None
        }
    }

    /// Full menu view: container slots followed by the 27 main inventory and
    /// 9 hotbar slots.
    pub fn view(&self, inventory: &[ItemStack]) -> Vec<ItemStack> {
        let mut view = Vec::with_capacity(self.menu_slots());
        view.extend(self.container.iter().cloned());
        for index in 9..INVENTORY_SLOTS {
            view.push(
                inventory
                    .get(index)
                    .cloned()
                    .unwrap_or_else(ItemStack::empty),
            );
        }
        for index in 0..HOTBAR_SLOTS {
            view.push(
                inventory
                    .get(index)
                    .cloned()
                    .unwrap_or_else(ItemStack::empty),
            );
        }
        view
    }

    pub fn increment_state_id(&mut self) -> i32 {
        self.state_id = (self.state_id + 1) & 32767;
        self.state_id
    }

    pub fn content_packet(&self, inventory: &[ItemStack], cursor: &ItemStack) -> Vec<u8> {
        let view = self.view(inventory);
        let mut writer = PacketWriter::new();
        writer
            .write_varint(CLIENTBOUND_CONTAINER_SET_CONTENT)
            .unwrap();
        writer.write_varint(self.container_id).unwrap();
        writer.write_varint(self.state_id).unwrap();
        writer.write_varint(view.len() as i32).unwrap();
        for stack in &view {
            write_item_stack(&mut writer, stack);
        }
        write_item_stack(&mut writer, cursor);
        writer.into_inner()
    }

    pub fn open_screen_packet(&self) -> Vec<u8> {
        let mut title = Vec::new();
        let component = crate::nbt::Tag::Compound(vec![(
            "translate".into(),
            crate::nbt::Tag::String(self.kind.title_key().into()),
        )]);
        crate::nbt::write_any_tag(&mut title, &component).expect("writing to Vec cannot fail");

        let mut writer = PacketWriter::new();
        writer.write_varint(CLIENTBOUND_OPEN_SCREEN).unwrap();
        writer.write_varint(self.container_id).unwrap();
        writer.write_varint(self.kind.menu_type_id()).unwrap();
        writer.write_bytes(&title);
        writer.into_inner()
    }

    pub fn close_packet(&self) -> Vec<u8> {
        let mut writer = PacketWriter::new();
        writer.write_varint(CLIENTBOUND_CONTAINER_CLOSE).unwrap();
        writer.write_varint(self.container_id).unwrap();
        writer.into_inner()
    }

    /// One `ClientboundContainerSetDataPacket` from the cached wire value.
    pub fn data_packet(&self, index: usize) -> Option<Vec<u8>> {
        let value = *self.data.get(index)?;
        let mut writer = PacketWriter::new();
        writer.write_varint(CLIENTBOUND_CONTAINER_SET_DATA).unwrap();
        writer.write_varint(self.container_id).unwrap();
        writer.write_i16(index as i16).unwrap();
        writer.write_i16(value).unwrap();
        Some(writer.into_inner())
    }

    /// Replace the menu data values and return packets for the slots that
    /// changed. Values are wire shorts, exactly like
    /// `ClientboundContainerSetDataPacket`.
    pub fn update_data(&mut self, values: &[i16]) -> Result<Vec<Vec<u8>>, MenuError> {
        if values.len() != self.data.len() {
            return Err(MenuError::WrongDataCount {
                expected: self.data.len(),
                got: values.len(),
            });
        }
        let mut packets = Vec::new();
        for (index, value) in values.iter().copied().enumerate() {
            if self.data[index] != value {
                self.data[index] = value;
                if let Some(packet) = self.data_packet(index) {
                    packets.push(packet);
                }
            }
        }
        Ok(packets)
    }

    /// Replace server-side container contents after a tick. Returns whether
    /// anything changed and bumps the state id like `broadcastChanges`.
    pub fn replace_container(&mut self, container: Vec<ItemStack>) -> Result<bool, MenuError> {
        if container.len() != self.container.len() {
            return Err(MenuError::WrongContainerSize {
                expected: self.container.len(),
                got: container.len(),
            });
        }
        if self.container == container {
            return Ok(false);
        }
        self.container = container;
        self.increment_state_id();
        Ok(true)
    }

    fn slot_stack(&self, inventory: &[ItemStack], slot: usize) -> Option<ItemStack> {
        if slot < self.container.len() {
            return self.container.get(slot).cloned();
        }
        let index = self.inventory_index_for_slot(slot)?;
        inventory.get(index).cloned()
    }

    fn set_slot_stack(
        &mut self,
        inventory: &mut [ItemStack],
        slot: usize,
        stack: ItemStack,
    ) -> bool {
        if slot < self.container.len() {
            let Some(target) = self.container.get_mut(slot) else {
                return false;
            };
            *target = stack;
            return true;
        }
        let Some(index) = self.inventory_index_for_slot(slot) else {
            return false;
        };
        let Some(target) = inventory.get_mut(index) else {
            return false;
        };
        *target = stack;
        true
    }

    fn slot_max_stack(&self, slot: usize, stack: &ItemStack) -> i32 {
        if matches!(self.kind, MenuKind::BrewingStand) && slot < 3 {
            return 1;
        }
        stack.max_stack_size().max(1)
    }

    /// Slot-level placement rule, mirroring the vanilla slot subclasses.
    fn can_place(&self, slot: usize, stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return true;
        }
        if slot >= self.container.len() {
            return true;
        }
        match self.kind {
            MenuKind::Chest => true,
            MenuKind::Furnace(_) => match slot {
                2 => false,
                1 => crate::crafting::fuel_burn_time(&stack.item).is_some(),
                _ => true,
            },
            MenuKind::BrewingStand => match slot {
                0..=2 => matches!(
                    stack.item.as_str(),
                    "minecraft:potion"
                        | "minecraft:splash_potion"
                        | "minecraft:lingering_potion"
                        | "minecraft:glass_bottle"
                ),
                3 => crate::crafting::is_brewing_ingredient(&stack.item),
                4 => crate::crafting::is_brewing_fuel(stack),
                _ => false,
            },
        }
    }

    fn set_checked(
        &mut self,
        inventory: &mut [ItemStack],
        slot: usize,
        stack: ItemStack,
    ) -> Result<bool, MenuError> {
        if !self.can_place(slot, &stack) {
            return Err(MenuError::SlotRejectsItem(slot as i32));
        }
        if !self.set_slot_stack(inventory, slot, stack) {
            return Err(MenuError::InvalidSlot(slot as i32));
        }
        Ok(true)
    }

    /// `AbstractContainerMenu.moveItemStackTo`: merge into compatible stacks
    /// first, then fill empty slots. `reverse` scans from the end of the range,
    /// as vanilla does for container -> player moves.
    fn move_stack_to_range(
        &mut self,
        inventory: &mut [ItemStack],
        source_slot: usize,
        start: usize,
        end: usize,
        reverse: bool,
    ) -> Result<bool, MenuError> {
        let mut stack = self
            .slot_stack(inventory, source_slot)
            .ok_or(MenuError::InvalidSlot(source_slot as i32))?;
        if stack.is_empty() {
            return Ok(false);
        }
        let original_count = stack.count;
        let mut order: Vec<usize> = (start..end).collect();
        if reverse {
            order.reverse();
        }
        for &target in &order {
            if target == source_slot || !self.can_place(target, &stack) {
                continue;
            }
            let Some(mut target_stack) = self.slot_stack(inventory, target) else {
                continue;
            };
            if target_stack.same_item_and_components(&stack) {
                let space = self.slot_max_stack(target, &stack) - target_stack.count;
                if space > 0 {
                    let moved = space.min(stack.count);
                    target_stack.count += moved;
                    stack.shrink(moved);
                    self.set_slot_stack(inventory, target, target_stack);
                    if stack.is_empty() {
                        break;
                    }
                }
            }
        }
        if !stack.is_empty() {
            for &target in &order {
                if target == source_slot || !self.can_place(target, &stack) {
                    continue;
                }
                if self
                    .slot_stack(inventory, target)
                    .is_some_and(|target_stack| target_stack.is_empty())
                {
                    let moved = self.slot_max_stack(target, &stack).min(stack.count);
                    let placed = stack.split(moved);
                    self.set_slot_stack(inventory, target, placed);
                    if stack.is_empty() {
                        break;
                    }
                }
            }
        }
        let moved = original_count - stack.count;
        if moved <= 0 {
            return Ok(false);
        }
        self.set_slot_stack(inventory, source_slot, stack);
        Ok(true)
    }

    fn quick_move(&mut self, inventory: &mut [ItemStack], slot: usize) -> Result<bool, MenuError> {
        let start = self.inventory_start();
        let menu_slots = self.menu_slots();
        if slot < start {
            // Furnace input/fuel move forward through the inventory; the
            // result slot and the other menu types scan from the hotbar back.
            let reverse = !matches!(self.kind, MenuKind::Furnace(_)) || slot == 2;
            return self.move_stack_to_range(inventory, slot, start, menu_slots, reverse);
        }
        let (main_start, main_end, hotbar_start, hotbar_end) =
            (start, start + 27, start + 27, menu_slots);
        let in_main = (main_start..main_end).contains(&slot);
        let in_hotbar = (hotbar_start..hotbar_end).contains(&slot);
        match self.kind {
            MenuKind::Chest => self.move_stack_to_range(inventory, slot, 0, start, false),
            MenuKind::Furnace(kind) => {
                let stack = self
                    .slot_stack(inventory, slot)
                    .ok_or(MenuError::InvalidSlot(slot as i32))?;
                if furnace_can_smelt(kind, &stack.item) {
                    return self.move_stack_to_range(inventory, slot, 0, 1, false);
                }
                if crate::crafting::fuel_burn_time(&stack.item).is_some() {
                    return self.move_stack_to_range(inventory, slot, 1, 2, false);
                }
                if in_main {
                    self.move_stack_to_range(inventory, slot, hotbar_start, hotbar_end, false)
                } else if in_hotbar {
                    self.move_stack_to_range(inventory, slot, main_start, main_end, false)
                } else {
                    Ok(false)
                }
            }
            MenuKind::BrewingStand => {
                let stack = self
                    .slot_stack(inventory, slot)
                    .ok_or(MenuError::InvalidSlot(slot as i32))?;
                if crate::crafting::is_brewing_fuel(&stack)
                    && self.move_stack_to_range(inventory, slot, 4, 5, false)?
                {
                    return Ok(true);
                }
                if crate::crafting::is_brewing_ingredient(&stack.item)
                    && self.move_stack_to_range(inventory, slot, 3, 4, false)?
                {
                    return Ok(true);
                }
                if matches!(
                    stack.item.as_str(),
                    "minecraft:potion"
                        | "minecraft:splash_potion"
                        | "minecraft:lingering_potion"
                        | "minecraft:glass_bottle"
                ) && self.move_stack_to_range(inventory, slot, 0, 3, false)?
                {
                    return Ok(true);
                }
                if in_main {
                    self.move_stack_to_range(inventory, slot, hotbar_start, hotbar_end, false)
                } else if in_hotbar {
                    self.move_stack_to_range(inventory, slot, main_start, main_end, false)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Apply one click. The carried stack from the packet is deliberately not
    /// trusted: `SessionMenus` keeps the authoritative cursor, like
    /// `AbstractContainerMenu.getCarried`.
    pub fn apply_click(
        &mut self,
        inventory: &mut [ItemStack],
        cursor: &mut ItemStack,
        click: &ContainerClick,
        creative: bool,
    ) -> Result<MenuClickOutcome, MenuError> {
        if click.container_id != self.container_id {
            return Err(MenuError::ContainerMismatch {
                expected: self.container_id,
                got: click.container_id,
            });
        }
        let state_mismatch = click.state_id != self.state_id;
        let slot = click.slot;
        if slot != -999 && (slot < 0 || slot as usize >= self.menu_slots()) {
            return Err(MenuError::InvalidSlot(slot));
        }
        let mut changed = false;

        match click.click_type {
            0 => {
                if !(0..=1).contains(&click.button) {
                    return Err(MenuError::InvalidButton(click.button));
                }
                if slot == -999 {
                    if !cursor.is_empty() {
                        if click.button == 0 {
                            *cursor = ItemStack::empty();
                            changed = true;
                        } else {
                            cursor.shrink(1);
                            changed = true;
                        }
                    }
                } else {
                    let slot_index = slot as usize;
                    let mut slot_stack = self
                        .slot_stack(inventory, slot_index)
                        .ok_or(MenuError::InvalidSlot(slot))?;
                    if click.button == 0 {
                        if cursor.is_empty() {
                            if !slot_stack.is_empty() {
                                *cursor = slot_stack;
                                self.set_slot_stack(inventory, slot_index, ItemStack::empty());
                                changed = true;
                            }
                        } else if slot_stack.is_empty() {
                            let placed = cursor.clone();
                            self.set_checked(inventory, slot_index, placed)?;
                            *cursor = ItemStack::empty();
                            changed = true;
                        } else if slot_stack.same_item_and_components(cursor) {
                            let space =
                                self.slot_max_stack(slot_index, &slot_stack) - slot_stack.count;
                            if space > 0 {
                                let moved = space.min(cursor.count);
                                slot_stack.count += moved;
                                cursor.shrink(moved);
                                self.set_slot_stack(inventory, slot_index, slot_stack);
                                changed = true;
                            }
                        } else if self.can_place(slot_index, cursor) {
                            std::mem::swap(&mut slot_stack, cursor);
                            self.set_slot_stack(inventory, slot_index, slot_stack);
                            changed = true;
                        }
                    } else {
                        // Right click: take half / place one, or merge one when
                        // both stacks match.
                        if cursor.is_empty() {
                            if !slot_stack.is_empty() {
                                let half = (slot_stack.count + 1) / 2;
                                *cursor = slot_stack.split(half);
                                self.set_slot_stack(inventory, slot_index, slot_stack);
                                changed = true;
                            }
                        } else if slot_stack.is_empty() {
                            let placed = cursor.copy_with_count(1);
                            self.set_checked(inventory, slot_index, placed)?;
                            cursor.shrink(1);
                            changed = true;
                        } else if slot_stack.same_item_and_components(cursor) {
                            let space =
                                self.slot_max_stack(slot_index, &slot_stack) - slot_stack.count;
                            if space > 0 {
                                slot_stack.count += 1;
                                cursor.shrink(1);
                                self.set_slot_stack(inventory, slot_index, slot_stack);
                                changed = true;
                            }
                        }
                    }
                }
            }
            1 => {
                if click.button != 0 {
                    return Err(MenuError::InvalidButton(click.button));
                }
                if slot >= 0 && self.quick_move(inventory, slot as usize)? {
                    changed = true;
                }
            }
            2 => {
                if !(0..HOTBAR_SLOTS as i32).contains(&click.button) {
                    return Err(MenuError::InvalidButton(click.button));
                }
                if slot >= 0 {
                    let slot_index = slot as usize;
                    let hotbar_slot = self
                        .menu_slot_for_inventory(click.button as usize)
                        .ok_or(MenuError::InvalidSlot(click.button))?;
                    if slot_index != hotbar_slot {
                        let slot_stack = self
                            .slot_stack(inventory, slot_index)
                            .ok_or(MenuError::InvalidSlot(slot))?;
                        let hotbar_stack = self
                            .slot_stack(inventory, hotbar_slot)
                            .ok_or(MenuError::InvalidSlot(hotbar_slot as i32))?;
                        if self.can_place(slot_index, &hotbar_stack) {
                            let mut slot_target = hotbar_stack;
                            if slot_target.count > self.slot_max_stack(slot_index, &slot_target) {
                                let overflow = slot_target
                                    .split(self.slot_max_stack(slot_index, &slot_target));
                                let leftover = add_stack(inventory, overflow);
                                if !leftover.is_empty() {
                                    return Err(MenuError::SlotRejectsItem(slot));
                                }
                            }
                            self.set_slot_stack(inventory, slot_index, slot_target);
                            self.set_slot_stack(inventory, hotbar_slot, slot_stack);
                            changed = true;
                        }
                    }
                }
            }
            3 => {
                if !creative {
                    return Err(MenuError::NotCreative);
                }
                if cursor.is_empty()
                    && slot >= 0
                    && let Some(stack) = self.slot_stack(inventory, slot as usize)
                    && !stack.is_empty()
                {
                    *cursor = stack.copy_with_count(stack.max_stack_size());
                    changed = true;
                }
            }
            4 => {
                if !cursor.is_empty() {
                    return Ok(MenuClickOutcome {
                        changed: false,
                        state_mismatch,
                    });
                }
                if !(0..=1).contains(&click.button) {
                    return Err(MenuError::InvalidButton(click.button));
                }
                if slot >= 0
                    && let Some(mut stack) = self.slot_stack(inventory, slot as usize)
                    && !stack.is_empty()
                {
                    if click.button == 0 {
                        stack.shrink(1);
                    } else {
                        stack = ItemStack::empty();
                    }
                    self.set_slot_stack(inventory, slot as usize, stack);
                    changed = true;
                }
            }
            6 => {
                // Double-click collect-to-cursor. Vanilla scans the menu twice
                // in the button-selected direction; one full scan is enough for
                // the stacks this port models.
                if !cursor.is_empty() {
                    for target in 0..self.menu_slots() {
                        let Some(mut stack) = self.slot_stack(inventory, target) else {
                            continue;
                        };
                        if stack.same_item_and_components(cursor) {
                            let space = cursor.max_stack_size() - cursor.count;
                            if space > 0 {
                                let moved = space.min(stack.count);
                                cursor.count += moved;
                                stack.shrink(moved);
                                self.set_slot_stack(inventory, target, stack);
                                changed = true;
                            }
                        }
                    }
                }
            }
            other => return Err(MenuError::UnsupportedClickType(other)),
        }

        if changed {
            self.increment_state_id();
        }
        Ok(MenuClickOutcome {
            changed,
            state_mismatch,
        })
    }
}

/// Per-session menu bookkeeping: at most one open container menu and the
/// player's rolling container id counter (`nextContainerCounter`).
#[derive(Debug, Clone, Default)]
pub struct SessionMenus {
    next_container_id: i32,
    open: Option<OpenMenu>,
}

impl SessionMenus {
    pub fn new() -> Self {
        Self {
            next_container_id: 0,
            open: None,
        }
    }

    pub fn open(&self) -> Option<&OpenMenu> {
        self.open.as_ref()
    }

    pub fn open_mut(&mut self) -> Option<&mut OpenMenu> {
        self.open.as_mut()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Open a block-backed menu. `container` must be exactly the variant's
    /// block-entity slot count; a wrong size is a caller bug and is rejected
    /// instead of being padded.
    #[allow(clippy::too_many_arguments)]
    pub fn open_menu(
        &mut self,
        kind: MenuKind,
        pos: BlockEntityPos,
        container: Vec<ItemStack>,
        data: &[i16],
        inventory: &[ItemStack],
        cursor: &ItemStack,
    ) -> Result<MenuOpenPackets, MenuError> {
        let expected = kind.container_slots();
        if container.len() != expected {
            return Err(MenuError::WrongContainerSize {
                expected,
                got: container.len(),
            });
        }
        let expected_data = kind.data_slots();
        if data.len() != expected_data {
            return Err(MenuError::WrongDataCount {
                expected: expected_data,
                got: data.len(),
            });
        }
        if inventory.len() < INVENTORY_SLOTS {
            return Err(MenuError::WrongContainerSize {
                expected: INVENTORY_SLOTS,
                got: inventory.len(),
            });
        }
        self.next_container_id = self.next_container_id % 100 + 1;
        let menu = OpenMenu {
            kind,
            container_id: self.next_container_id,
            state_id: 0,
            pos,
            container,
            data: data.to_vec(),
        };
        let open_screen = menu.open_screen_packet();
        let content = menu.content_packet(inventory, cursor);
        let data = (0..menu.data.len())
            .filter_map(|index| menu.data_packet(index))
            .collect();
        self.open = Some(menu);
        Ok(MenuOpenPackets {
            open_screen,
            content,
            data,
        })
    }

    /// Close the menu and return it, so the caller can send a close packet.
    pub fn close(&mut self) -> Option<OpenMenu> {
        self.open.take()
    }

    /// Disconnect path: the socket is gone, so the state is simply dropped.
    pub fn on_disconnect(&mut self) {
        self.open = None;
    }

    /// Close when the backing block is removed. Returns the closed menu.
    pub fn on_block_removed(&mut self, pos: BlockEntityPos) -> Option<OpenMenu> {
        if self.open.as_ref().is_some_and(|menu| menu.pos == pos) {
            self.open.take()
        } else {
            None
        }
    }

    pub fn click(
        &mut self,
        inventory: &mut [ItemStack],
        cursor: &mut ItemStack,
        click: &ContainerClick,
        creative: bool,
    ) -> Result<MenuClickOutcome, MenuError> {
        let menu = self.open.as_mut().ok_or(MenuError::NoOpenMenu)?;
        menu.apply_click(inventory, cursor, click, creative)
    }

    /// Push new block-entity values into the open menu. Returns whether the
    /// container contents changed (the caller must resend content) plus any
    /// changed data packets.
    pub fn sync(
        &mut self,
        container: Vec<ItemStack>,
        data: &[i16],
    ) -> Result<(bool, Vec<Vec<u8>>), MenuError> {
        let menu = self.open.as_mut().ok_or(MenuError::NoOpenMenu)?;
        let changed = menu.replace_container(container)?;
        let data_packets = menu.update_data(data)?;
        Ok((changed, data_packets))
    }
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

pub fn read_hashed_stack(reader: &mut PacketReader<'_>) -> Option<ItemStack> {
    if !reader.read_bool().ok()? {
        return Some(ItemStack::empty());
    }
    let id = reader.read_varint().ok()?;
    let count = reader.read_varint().ok()?;
    // The hashed form only carries component ids and hashes, not payloads, so
    // the component values cannot be reconstructed. The click's carried stack
    // is a consistency hint; the session cursor remains authoritative.
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

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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
                    *cursor = inventory[index].split(half);
                } else if inventory[index].is_empty() || same_stack(&inventory[index], cursor) {
                    if inventory[index].is_empty() {
                        inventory[index] = cursor.copy_with_count(1);
                    } else if inventory[index].count < inventory[index].max_stack_size() {
                        inventory[index].count += 1;
                    } else {
                        return false;
                    }
                    cursor.shrink(1);
                }
            } else if cursor.is_empty() {
                *cursor = inventory[index].clone();
                inventory[index] = ItemStack::empty();
            } else if inventory[index].is_empty() {
                inventory[index] = cursor.clone();
                *cursor = ItemStack::empty();
            } else if same_stack(&inventory[index], cursor) {
                merge_stacks(&mut inventory[index], cursor);
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
                if same_stack(&inventory[target], &inventory[index])
                    && inventory[target].count < inventory[target].max_stack_size()
                {
                    let space = inventory[target].max_stack_size() - inventory[target].count;
                    let moved = space.min(inventory[index].count);
                    inventory[target].count += moved;
                    inventory[index].shrink(moved);
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
                cursor.shrink(1);
            } else if !inventory[index].is_empty() {
                inventory[index].shrink(1);
            }
            true
        }
        _ => false,
    }
}

fn armor_slot_info(menu_slot: i32) -> Option<(usize, &'static str)> {
    match menu_slot {
        5 => Some((3, "head")),
        6 => Some((2, "chest")),
        7 => Some((1, "legs")),
        8 => Some((0, "feet")),
        _ => None,
    }
}

fn can_equip_in_armor_slot(stack: &ItemStack, required: &str) -> bool {
    if stack.is_empty() {
        return true;
    }
    crate::item_prototypes::prototype(&stack.item)
        .and_then(|prototype| prototype.equippable_slot.as_deref())
        == Some(required)
}

/// Apply a click in the player inventory menu (container 0), including armor
/// slots 5..=8 (head/chest/legs/feet mapped to `armor[3]/[2]/[1]/[0]`).
/// Hotbar/main slots 9..=44 keep the free `apply_click` behavior.
pub fn apply_player_click(
    inventory: &mut [ItemStack],
    armor: &mut [ItemStack; 4],
    cursor: &mut ItemStack,
    click: ContainerClick,
) -> bool {
    if !(5..=44).contains(&click.slot) {
        return false;
    }

    if let Some((armor_index, required)) = armor_slot_info(click.slot) {
        match click.click_type {
            0 => {
                if click.button == 0 {
                    if cursor.is_empty() {
                        if armor[armor_index].is_empty() {
                            return false;
                        }
                        *cursor = armor[armor_index].clone();
                        armor[armor_index] = ItemStack::empty();
                        true
                    } else if armor[armor_index].is_empty() {
                        if !can_equip_in_armor_slot(cursor, required) {
                            return false;
                        }
                        armor[armor_index] = cursor.clone();
                        *cursor = ItemStack::empty();
                        true
                    } else if can_equip_in_armor_slot(cursor, required) {
                        std::mem::swap(&mut armor[armor_index], cursor);
                        true
                    } else {
                        false
                    }
                } else if click.button == 1 {
                    if cursor.is_empty() {
                        if armor[armor_index].is_empty() {
                            return false;
                        }
                        *cursor = armor[armor_index].clone();
                        armor[armor_index] = ItemStack::empty();
                        true
                    } else if armor[armor_index].is_empty() {
                        if !can_equip_in_armor_slot(cursor, required) {
                            return false;
                        }
                        armor[armor_index] = cursor.copy_with_count(1);
                        cursor.shrink(1);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            1 => {
                if armor[armor_index].is_empty() {
                    return false;
                }
                let mut stack = armor[armor_index].clone();
                for slot in inventory.iter_mut().take(INVENTORY_SLOTS) {
                    if slot.is_empty() {
                        *slot = stack;
                        armor[armor_index] = ItemStack::empty();
                        return true;
                    }
                    if same_stack(slot, &stack) && slot.count < slot.max_stack_size() {
                        let space = slot.max_stack_size() - slot.count;
                        let moved = space.min(stack.count);
                        slot.count += moved;
                        stack.shrink(moved);
                        if stack.is_empty() {
                            armor[armor_index] = ItemStack::empty();
                            return true;
                        }
                    }
                }
                if stack != armor[armor_index] {
                    armor[armor_index] = stack;
                    true
                } else {
                    false
                }
            }
            2 => {
                if !(0..HOTBAR_SLOTS as i32).contains(&click.button) {
                    return false;
                }
                let hotbar = click.button as usize;
                if hotbar >= inventory.len() {
                    return false;
                }
                if !can_equip_in_armor_slot(&inventory[hotbar], required) {
                    return false;
                }
                std::mem::swap(&mut inventory[hotbar], &mut armor[armor_index]);
                true
            }
            _ => false,
        }
    } else {
        apply_click(inventory, cursor, click)
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

    #[test]
    fn add_stack_merges_then_fills_and_returns_overflow() {
        let mut inventory = vec![ItemStack::empty(); 2];
        inventory[0] = ItemStack::new("minecraft:stone", 60);

        let leftover = add_stack(&mut inventory, ItemStack::new("minecraft:stone", 10));
        assert!(leftover.is_empty());
        assert_eq!(inventory[0].count, 64);
        assert_eq!(inventory[1].item, "minecraft:stone");
        assert_eq!(inventory[1].count, 6);

        let full = vec![ItemStack::new("minecraft:dirt", 64); 2];
        let mut full = full;
        let leftover = add_stack(&mut full, ItemStack::new("minecraft:dirt", 3));
        assert_eq!(leftover.item, "minecraft:dirt");
        assert_eq!(leftover.count, 3);
    }
}
