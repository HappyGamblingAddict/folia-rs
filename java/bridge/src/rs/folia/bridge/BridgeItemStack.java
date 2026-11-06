package rs.folia.bridge;

import org.bukkit.Material;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.material.MaterialData;

import java.util.List;
import java.util.Map;

/**
 * Standalone {@link ItemStack} used for every stack that crosses the Rust
 * bridge.
 *
 * <p>The Bukkit API's plain {@code ItemStack} delegates to a server-provided
 * craft delegate. The Rust host has no CraftBukkit, so this subclass carries
 * the fields itself. The bridge registry shim makes
 * {@code new ItemStack(Material, count)} return one of these through
 * {@code ItemType.createItemStack}.</p>
 */
public final class BridgeItemStack extends ItemStack {
    private Material type;
    private int amount;
    private short durability;
    private BridgeItemMeta meta;

    public BridgeItemStack(Material type, int amount) {
        super();
        this.type = type == null ? Material.AIR : type;
        this.amount = Math.max(0, amount);
    }

    public BridgeItemStack(Material type, int amount, short durability) {
        this(type, amount);
        this.durability = durability;
    }

    BridgeItemStack copy() {
        BridgeItemStack copy = new BridgeItemStack(type, amount, durability);
        if (meta != null) {
            copy.meta = BridgeItemMeta.copyOf(meta.proxy());
        }
        return copy;
    }

    @Override
    public Material getType() {
        return type;
    }

    @Override
    public void setType(Material type) {
        this.type = type == null ? Material.AIR : type;
    }

    @Override
    public int getAmount() {
        return amount;
    }

    @Override
    public void setAmount(int amount) {
        this.amount = Math.max(0, amount);
    }

    @Override
    public short getDurability() {
        return durability;
    }

    @Override
    public void setDurability(short durability) {
        this.durability = durability;
    }

    @Override
    public int getMaxStackSize() {
        return meta == null ? 64 : meta.maxStackSize();
    }

    @Override
    public ItemMeta getItemMeta() {
        return meta == null
                ? BridgeItemMeta.empty().proxy()
                : BridgeItemMeta.copyOf(meta.proxy()).proxy();
    }

    @Override
    public boolean hasItemMeta() {
        return meta != null;
    }

    @Override
    public boolean setItemMeta(ItemMeta itemMeta) {
        meta = itemMeta == null ? null : BridgeItemMeta.copyOf(itemMeta);
        return true;
    }

    @Override
    public ItemStack clone() {
        return copy();
    }

    @Override
    public boolean isEmpty() {
        return type == null || type == Material.AIR || amount <= 0;
    }

    @Override
    public boolean isSimilar(ItemStack other) {
        if (other == null) {
            return false;
        }
        if (other instanceof BridgeItemStack stack) {
            return type == stack.type
                    && durability == stack.durability
                    && metaString().equals(stack.metaString());
        }
        try {
            return type == other.getType() && durability == other.getDurability();
        } catch (RuntimeException exception) {
            return false;
        }
    }

    @Override
    public Map<Enchantment, Integer> getEnchantments() {
        return meta == null ? Map.of() : BridgeItemMeta.copyOf(meta.proxy()).proxy().getEnchants();
    }

    @Override
    public int getEnchantmentLevel(Enchantment enchantment) {
        return meta == null ? 0 : BridgeItemMeta.copyOf(meta.proxy()).proxy().getEnchantLevel(enchantment);
    }

    @Override
    public boolean containsEnchantment(Enchantment enchantment) {
        return getEnchantmentLevel(enchantment) > 0;
    }

    @Override
    public void addEnchantment(Enchantment enchantment, int level) {
        addUnsafeEnchantment(enchantment, level);
    }

    @Override
    public void addUnsafeEnchantment(Enchantment enchantment, int level) {
        ItemMeta itemMeta = getItemMeta();
        itemMeta.addEnchant(enchantment, level, true);
        setItemMeta(itemMeta);
    }

    @Override
    public int removeEnchantment(Enchantment enchantment) {
        int level = getEnchantmentLevel(enchantment);
        if (level != 0) {
            ItemMeta itemMeta = getItemMeta();
            itemMeta.removeEnchant(enchantment);
            setItemMeta(itemMeta);
        }
        return level;
    }

    @Override
    public void removeEnchantments() {
        ItemMeta itemMeta = getItemMeta();
        itemMeta.removeEnchantments();
        setItemMeta(itemMeta);
    }

    @Override
    public MaterialData getData() {
        return null;
    }

    @Override
    public void setData(MaterialData data) {
        // Legacy material data is not modelled by the component bridge.
    }

    @Override
    public String toString() {
        return "ItemStack{" + type + " x " + amount + (meta == null ? "" : " " + meta) + "}";
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof BridgeItemStack stack)) {
            return false;
        }
        return amount == stack.amount && isSimilar(stack);
    }

    @Override
    public int hashCode() {
        return 31 * (31 * type.hashCode() + amount) + metaString().hashCode();
    }

    private String metaString() {
        if (meta == null) {
            return "";
        }
        return meta.displayName() + "\u0000" + String.join("\u0001", meta.lore()) + "\u0000" + meta.unbreakable();
    }

    static String encode(ItemStack stack) {
        if (stack == null || stack.getType() == null || stack.getType() == Material.AIR
                || stack.getAmount() <= 0) {
            return "";
        }
        String name = stack.getType().getKey().toString();
        int count = stack.getAmount();
        int damage = stack.getDurability() & 0xffff;
        String displayName = "";
        boolean unbreakable = false;
        List<String> lore = List.of();
        ItemMeta itemMeta = stack.getItemMeta();
        if (itemMeta != null) {
            try {
                if (itemMeta.hasDisplayName()) {
                    displayName = itemMeta.getDisplayName();
                }
                unbreakable = itemMeta.isUnbreakable();
                if (itemMeta.hasLore()) {
                    List<String> lines = itemMeta.getLore();
                    if (lines != null) {
                        lore = List.copyOf(lines);
                    }
                }
            } catch (RuntimeException ignored) {
                // Foreign ItemMeta implementations may not support every getter.
            }
        }
        StringBuilder encoded = new StringBuilder();
        encoded.append(name).append('\t')
                .append(count).append('\t')
                .append(damage).append('\t')
                .append(BridgeValues.hex(displayName)).append('\t')
                .append(unbreakable ? '1' : '0');
        for (String line : lore) {
            encoded.append('\t').append(BridgeValues.hex(line));
        }
        return encoded.toString();
    }

    static BridgeItemStack decode(String encoded) {
        if (encoded == null || encoded.isEmpty()) {
            return new BridgeItemStack(Material.AIR, 0);
        }
        String[] fields = encoded.split("\t", -1);
        if (fields.length < 5) {
            return new BridgeItemStack(Material.AIR, 0);
        }
        Material material = Material.matchMaterial(fields[0]);
        if (material == null) {
            material = Material.AIR;
        }
        int count;
        int damage;
        try {
            count = Integer.parseInt(fields[1]);
            damage = Integer.parseInt(fields[2]);
        } catch (NumberFormatException exception) {
            return new BridgeItemStack(Material.AIR, 0);
        }
        BridgeItemStack stack = new BridgeItemStack(material, count, (short) Math.max(0, damage));
        String displayName = BridgeValues.unhex(fields[3]);
        boolean unbreakable = "1".equals(fields[4]);
        List<String> lore = new java.util.ArrayList<>();
        for (int index = 5; index < fields.length; index++) {
            if (!fields[index].isEmpty()) {
                lore.add(BridgeValues.unhex(fields[index]));
            }
        }
        if (!displayName.isEmpty() || !lore.isEmpty() || unbreakable) {
            stack.setItemMeta(BridgeItemMeta.of(displayName, lore, unbreakable).proxy());
        }
        return stack;
    }
}
