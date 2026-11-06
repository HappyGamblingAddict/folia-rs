package rs.folia.bridge;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.inventory.ItemFlag;
import org.bukkit.inventory.meta.ItemMeta;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * ItemMeta implementation used by {@link BridgeItemStack}.
 *
 * <p>The Rust host only stores the meta subset its item component model
 * understands: display name, lore, unbreakable, and durability. Enchantments
 * and item flags live on this proxy for the lifetime of the stack but are not
 * part of the Rust round trip yet.</p>
 */
final class BridgeItemMeta implements InvocationHandler {
    private static final PlainTextComponentSerializer PLAIN = PlainTextComponentSerializer.plainText();

    private String displayName;
    private final List<String> lore = new ArrayList<>();
    private boolean unbreakable;
    private Integer customModelData;
    private Integer maxStackSize;
    private final Set<ItemFlag> itemFlags = EnumSet.noneOf(ItemFlag.class);
    private final Map<Enchantment, Integer> enchantments = new LinkedHashMap<>();

    private BridgeItemMeta() {
    }

    static BridgeItemMeta empty() {
        return new BridgeItemMeta();
    }

    static BridgeItemMeta of(String displayName, List<String> lore, boolean unbreakable) {
        BridgeItemMeta meta = new BridgeItemMeta();
        meta.displayName = displayName;
        if (lore != null) {
            meta.lore.addAll(lore);
        }
        meta.unbreakable = unbreakable;
        return meta;
    }

    static BridgeItemMeta copyOf(ItemMeta source) {
        BridgeItemMeta meta = new BridgeItemMeta();
        if (source == null) {
            return meta;
        }
        try {
            if (source.hasDisplayName()) {
                meta.displayName = source.getDisplayName();
            }
            if (source.hasLore()) {
                List<String> lines = source.getLore();
                if (lines != null) {
                    meta.lore.addAll(lines);
                }
            }
            meta.unbreakable = source.isUnbreakable();
            if (source.hasCustomModelData()) {
                meta.customModelData = source.getCustomModelData();
            }
            if (source.hasMaxStackSize()) {
                meta.maxStackSize = source.getMaxStackSize();
            }
            Set<ItemFlag> flags = source.getItemFlags();
            if (flags != null) {
                meta.itemFlags.addAll(flags);
            }
            Map<Enchantment, Integer> enchants = source.getEnchants();
            if (enchants != null) {
                meta.enchantments.putAll(enchants);
            }
        } catch (RuntimeException ignored) {
            // A foreign ItemMeta implementation may not implement every method.
        }
        return meta;
    }

    String displayName() {
        return displayName;
    }

    List<String> lore() {
        return List.copyOf(lore);
    }

    boolean unbreakable() {
        return unbreakable;
    }

    int maxStackSize() {
        return maxStackSize == null ? 64 : maxStackSize;
    }

    ItemMeta proxy() {
        return (ItemMeta) Proxy.newProxyInstance(
                BridgeItemMeta.class.getClassLoader(),
                new Class<?>[]{ItemMeta.class},
                this
        );
    }

    @Override
    public Object invoke(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return switch (method.getName()) {
                case "toString" -> "BridgeItemMeta{displayName=" + displayName + ", lore=" + lore
                        + ", unbreakable=" + unbreakable + "}";
                case "hashCode" -> System.identityHashCode(proxyObject);
                case "equals" -> proxyObject == args[0];
                default -> null;
            };
        }
        return switch (method.getName()) {
            case "clone" -> copyOf(proxy());
            case "hasCustomName", "hasDisplayName" -> displayName != null;
            case "getDisplayName" -> displayName;
            case "setDisplayName" -> {
                displayName = args == null || args.length == 0 ? null : (String) args[0];
                yield null;
            }
            case "customName", "displayName" -> {
                if (args != null && args.length == 1 && args[0] instanceof Component component) {
                    displayName = plain(component);
                    yield null;
                }
                yield displayName == null ? null : Component.text(displayName);
            }
            case "hasItemName", "hasLocalizedName" -> false;
            case "getItemName", "getLocalizedName" -> null;
            case "setItemName", "setLocalizedName" -> null;
            case "itemName" -> null;
            case "hasLore" -> !lore.isEmpty();
            case "getLore" -> lore.isEmpty() ? null : List.copyOf(lore);
            case "setLore" -> {
                lore.clear();
                if (args != null && args.length > 0 && args[0] instanceof List<?> lines) {
                    for (Object line : lines) {
                        lore.add(String.valueOf(line));
                    }
                }
                yield null;
            }
            case "lore" -> {
                if (args != null && args.length == 1 && args[0] instanceof List<?> components) {
                    lore.clear();
                    for (Object value : components) {
                        if (value instanceof Component component) {
                            lore.add(plain(component));
                        } else {
                            lore.add(String.valueOf(value));
                        }
                    }
                    yield null;
                }
                List<Component> output = new ArrayList<>(lore.size());
                for (String line : lore) {
                    output.add(Component.text(line));
                }
                yield output;
            }
            case "isUnbreakable" -> unbreakable;
            case "setUnbreakable" -> {
                unbreakable = args != null && Boolean.TRUE.equals(args[0]);
                yield null;
            }
            case "hasCustomModelData" -> customModelData != null;
            case "getCustomModelData" -> customModelData == null ? 0 : customModelData;
            case "setCustomModelData" -> {
                customModelData = args == null || args[0] == null ? null : (Integer) args[0];
                yield null;
            }
            case "hasMaxStackSize" -> maxStackSize != null;
            case "getMaxStackSize" -> maxStackSize == null ? 64 : maxStackSize;
            case "setMaxStackSize" -> {
                maxStackSize = args == null || args[0] == null ? null : (Integer) args[0];
                yield null;
            }
            case "hasEnchants" -> !enchantments.isEmpty();
            case "hasEnchant" -> args != null && enchantments.containsKey(args[0]);
            case "getEnchantLevel" -> args == null ? 0 : enchantments.getOrDefault(args[0], 0);
            case "getEnchants" -> Map.copyOf(enchantments);
            case "addEnchant" -> {
                if (args != null && args.length >= 2 && args[0] instanceof Enchantment enchantment) {
                    enchantments.put(enchantment, (Integer) args[1]);
                }
                yield true;
            }
            case "removeEnchant" -> {
                boolean removed = args != null && enchantments.remove(args[0]) != null;
                yield removed;
            }
            case "removeEnchantments" -> {
                enchantments.clear();
                yield null;
            }
            case "hasConflictingEnchant" -> false;
            case "getItemFlags" -> Set.copyOf(itemFlags);
            case "addItemFlags" -> {
                if (args != null && args.length > 0 && args[0] instanceof ItemFlag[] flags) {
                    itemFlags.addAll(List.of(flags));
                }
                yield null;
            }
            case "removeItemFlags" -> {
                if (args != null && args.length > 0 && args[0] instanceof ItemFlag[] flags) {
                    for (ItemFlag flag : flags) {
                        itemFlags.remove(flag);
                    }
                }
                yield null;
            }
            case "hasItemFlag" -> args != null && itemFlags.contains(args[0]);
            case "getAsString", "getAsComponentString" -> displayName == null ? "" : displayName;
            case "setVersion" -> null;
            case "toString" -> "BridgeItemMeta{" + displayName + "}";
            case "hashCode" -> System.identityHashCode(proxyObject);
            case "equals" -> proxyObject == args[0];
            default -> BridgeValues.defaultValue(method);
        };
    }

    private static String plain(Component component) {
        return component == null ? null : PLAIN.serialize(component);
    }
}
