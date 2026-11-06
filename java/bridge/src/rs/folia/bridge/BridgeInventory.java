package rs.folia.bridge;

import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.entity.HumanEntity;
import org.bukkit.entity.Player;
import org.bukkit.event.inventory.InventoryType;
import org.bukkit.inventory.EquipmentSlot;
import org.bukkit.inventory.Inventory;
import org.bukkit.inventory.InventoryHolder;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.PlayerInventory;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.ListIterator;
import java.util.Map;
import java.util.function.Supplier;

/**
 * Rust-backed player inventory (slots 0-35 only).
 *
 * <p>Armor and off-hand slots are not part of the Rust inventory model yet;
 * reads return air and writes are ignored. Every mutation is persisted to the
 * playerdata file immediately so the next login observes it.</p>
 */
final class BridgeInventory implements InvocationHandler {
    static final int SIZE = 36;
    private static final int HOTBAR = 9;
    private static final int MAX_STACK = 64;

    private final String uuid;
    private final Player player;
    private final Supplier<Location> location;
    private final PlayerInventory proxy;

    BridgeInventory(String uuid, Player player, Supplier<Location> location) {
        this.uuid = uuid;
        this.player = player;
        this.location = location;
        this.proxy = (PlayerInventory) Proxy.newProxyInstance(
                BridgeInventory.class.getClassLoader(),
                new Class<?>[]{PlayerInventory.class},
                this
        );
    }

    PlayerInventory proxy() {
        return proxy;
    }

    @Override
    public Object invoke(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return switch (method.getName()) {
                case "toString" -> "BridgeInventory{" + uuid + "}";
                case "hashCode" -> uuid.hashCode();
                case "equals" -> proxyObject == args[0];
                default -> null;
            };
        }
        String name = method.getName();
        return switch (name) {
            case "getSize" -> SIZE;
            case "getMaxStackSize" -> MAX_STACK;
            case "setMaxStackSize" -> null;
            case "getItem" -> {
                if (args != null && args.length == 1 && args[0] instanceof EquipmentSlot slot) {
                    yield slot == EquipmentSlot.HAND
                            ? getSlot(Bridge.nativePlayerSelectedSlot(uuid))
                            : getSlot(equipmentSlot(slot));
                }
                yield getSlot((Integer) args[0]);
            }
            case "setItem" -> {
                if (args != null && args.length == 2 && args[0] instanceof EquipmentSlot slot) {
                    setSlot(slot == EquipmentSlot.HAND
                            ? Bridge.nativePlayerSelectedSlot(uuid)
                            : equipmentSlot(slot), (ItemStack) args[1]);
                } else {
                    setSlot((Integer) args[0], (ItemStack) args[1]);
                }
                yield null;
            }
            case "getContents", "getStorageContents" -> contents();
            case "setContents", "setStorageContents" -> {
                setContents((ItemStack[]) args[0]);
                yield null;
            }
            case "getHeldItemSlot" -> Bridge.nativePlayerSelectedSlot(uuid);
            case "setHeldItemSlot" -> {
                Bridge.nativePlayerSetSelectedSlot(uuid, (Integer) args[0]);
                yield null;
            }
            case "getItemInMainHand", "getItemInHand" ->
                    getSlot(Bridge.nativePlayerSelectedSlot(uuid));
            case "setItemInMainHand", "setItemInHand" ->
                    {
                        setSlot(Bridge.nativePlayerSelectedSlot(uuid), (ItemStack) args[0]);
                        yield null;
                    }
            case "getItemInOffHand" -> empty();
            case "setItemInOffHand" -> null;
            case "getArmorContents" -> new ItemStack[]{
                    empty(), empty(), empty(), empty()
            };
            case "setArmorContents", "setExtraContents" -> null;
            case "getExtraContents" -> new ItemStack[]{empty()};
            case "getHelmet", "getChestplate", "getLeggings", "getBoots" -> empty();
            case "setHelmet", "setChestplate", "setLeggings", "setBoots" -> null;
            case "addItem" -> addItems((ItemStack[]) args[0]);
            case "removeItem", "removeItemAnySlot" -> removeItems((ItemStack[]) args[0]);
            case "contains" -> contains(args);
            case "containsAtLeast" -> containsAtLeast((ItemStack) args[0], (Integer) args[1]);
            case "all" -> all(args[0]);
            case "first" -> first(args[0]);
            case "firstEmpty" -> firstEmpty();
            case "isEmpty" -> isEmpty();
            case "remove" -> {
                removeMatching(args[0]);
                yield null;
            }
            case "clear" -> {
                if (args != null && args.length == 1) {
                    setSlot((Integer) args[0], null);
                } else {
                    Bridge.nativePlayerSetInventory(uuid, "");
                }
                yield null;
            }
            case "close" -> 0;
            case "getViewers" -> List.of(player);
            case "getType" -> InventoryType.PLAYER;
            case "getHolder" -> player;
            case "getLocation" -> location.get();
            case "iterator" -> {
                int index = args != null && args.length == 1 ? (Integer) args[0] : 0;
                yield List.of(contents()).listIterator(index);
            }
            default -> BridgeValues.defaultValue(method);
        };
    }

    private ItemStack getSlot(int slot) {
        if (slot < 0 || slot >= SIZE) {
            return empty();
        }
        return BridgeItemStack.decode(Bridge.nativePlayerItem(uuid, slot));
    }

    private void setSlot(int slot, ItemStack stack) {
        if (slot < 0 || slot >= SIZE) {
            return;
        }
        Bridge.nativePlayerSetItem(uuid, slot, BridgeItemStack.encode(stack));
    }

    private ItemStack[] contents() {
        ItemStack[] contents = new ItemStack[SIZE];
        for (int index = 0; index < contents.length; index++) {
            contents[index] = empty();
        }
        String encoded = Bridge.nativePlayerInventory(uuid);
        if (encoded == null || encoded.isEmpty()) {
            return contents;
        }
        for (String line : encoded.split("\n", -1)) {
            if (line.isEmpty()) {
                continue;
            }
            String[] fields = line.split("\t", 2);
            if (fields.length != 2) {
                continue;
            }
            try {
                int slot = Integer.parseInt(fields[0]);
                if (slot >= 0 && slot < SIZE) {
                    contents[slot] = BridgeItemStack.decode(fields[1]);
                }
            } catch (NumberFormatException ignored) {
                // Ignore malformed native inventory rows.
            }
        }
        return contents;
    }

    private void setContents(ItemStack[] contents) {
        StringBuilder encoded = new StringBuilder();
        for (int slot = 0; slot < SIZE; slot++) {
            ItemStack stack = contents != null && slot < contents.length ? contents[slot] : null;
            if (slot > 0) {
                encoded.append('\n');
            }
            encoded.append(slot).append('\t').append(BridgeItemStack.encode(stack));
        }
        Bridge.nativePlayerSetInventory(uuid, encoded.toString());
    }

    private Map<Integer, ItemStack> addItems(ItemStack[] stacks) {
        ItemStack[] contents = contents();
        Map<Integer, ItemStack> leftover = new HashMap<>();
        if (stacks == null) {
            return leftover;
        }
        for (ItemStack stack : stacks) {
            if (stack == null || stack.getType() == Material.AIR || stack.getAmount() <= 0) {
                continue;
            }
            int remaining = stack.getAmount();
            for (int slot = 0; slot < SIZE && remaining > 0; slot++) {
                ItemStack current = contents[slot];
                if (!current.isEmpty() && current.isSimilar(stack) && current.getAmount() < MAX_STACK) {
                    int moved = Math.min(remaining, MAX_STACK - current.getAmount());
                    current.setAmount(current.getAmount() + moved);
                    remaining -= moved;
                }
            }
            for (int slot = 0; slot < SIZE && remaining > 0; slot++) {
                if (contents[slot].isEmpty()) {
                    BridgeItemStack placed = stack instanceof BridgeItemStack bridge
                            ? bridge.copy() : new BridgeItemStack(stack.getType(), 0);
                    int moved = Math.min(remaining, MAX_STACK);
                    placed.setAmount(moved);
                    contents[slot] = placed;
                    remaining -= moved;
                }
            }
            if (remaining > 0) {
                leftover.put(-1, new BridgeItemStack(stack.getType(), remaining));
            }
        }
        setContents(contents);
        return leftover;
    }

    private Map<Integer, ItemStack> removeItems(ItemStack[] stacks) {
        ItemStack[] contents = contents();
        Map<Integer, ItemStack> removed = new HashMap<>();
        if (stacks == null) {
            return removed;
        }
        for (ItemStack stack : stacks) {
            if (stack == null || stack.isEmpty()) {
                continue;
            }
            int remaining = stack.getAmount();
            for (int slot = 0; slot < SIZE && remaining > 0; slot++) {
                ItemStack current = contents[slot];
                if (current.isEmpty() || !current.isSimilar(stack)) {
                    continue;
                }
                int moved = Math.min(remaining, current.getAmount());
                current.setAmount(current.getAmount() - moved);
                removed.put(slot, new BridgeItemStack(stack.getType(), moved));
                remaining -= moved;
                if (current.getAmount() <= 0) {
                    contents[slot] = empty();
                }
            }
        }
        setContents(contents);
        return removed;
    }

    private boolean contains(Object[] args) {
        if (args[0] instanceof Material material) {
            int amount = args.length > 1 ? (Integer) args[1] : 1;
            return count(material) >= amount;
        }
        ItemStack stack = (ItemStack) args[0];
        int amount = args.length > 1 ? (Integer) args[1] : 1;
        return count(stack.getType()) >= amount;
    }

    private boolean containsAtLeast(ItemStack stack, int amount) {
        return count(stack.getType()) >= amount;
    }

    private int count(Material material) {
        int total = 0;
        for (ItemStack stack : contents()) {
            if (!stack.isEmpty() && stack.getType() == material) {
                total += stack.getAmount();
            }
        }
        return total;
    }

    private Map<Integer, ? extends ItemStack> all(Object value) {
        Map<Integer, ItemStack> matches = new HashMap<>();
        ItemStack[] contents = contents();
        for (int slot = 0; slot < contents.length; slot++) {
            ItemStack stack = contents[slot];
            boolean match = value instanceof Material material
                    ? !stack.isEmpty() && stack.getType() == material
                    : stack.isSimilar((ItemStack) value);
            if (match) {
                matches.put(slot, stack);
            }
        }
        return matches;
    }

    private int first(Object value) {
        ItemStack[] contents = contents();
        for (int slot = 0; slot < contents.length; slot++) {
            ItemStack stack = contents[slot];
            boolean match = value instanceof Material material
                    ? !stack.isEmpty() && stack.getType() == material
                    : stack.isSimilar((ItemStack) value);
            if (match) {
                return slot;
            }
        }
        return -1;
    }

    private int firstEmpty() {
        ItemStack[] contents = contents();
        for (int slot = 0; slot < contents.length; slot++) {
            if (contents[slot].isEmpty()) {
                return slot;
            }
        }
        return -1;
    }

    private boolean isEmpty() {
        for (ItemStack stack : contents()) {
            if (!stack.isEmpty()) {
                return false;
            }
        }
        return true;
    }

    private void removeMatching(Object value) {
        ItemStack[] contents = contents();
        for (int slot = 0; slot < contents.length; slot++) {
            ItemStack stack = contents[slot];
            boolean match = value instanceof Material material
                    ? !stack.isEmpty() && stack.getType() == material
                    : stack.isSimilar((ItemStack) value);
            if (match) {
                contents[slot] = empty();
            }
        }
        setContents(contents);
    }

    private static int equipmentSlot(EquipmentSlot slot) {
        return switch (slot) {
            case HAND -> -1;
            case OFF_HAND -> 40;
            case HEAD -> 39;
            case CHEST -> 38;
            case LEGS -> 37;
            case FEET -> 36;
            default -> -1;
        };
    }

    private static ItemStack empty() {
        return new BridgeItemStack(Material.AIR, 0);
    }
}
