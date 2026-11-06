package rs.folia.bridge;

import io.papermc.paper.enchantments.EnchantmentRarity;
import io.papermc.paper.registry.set.RegistryKeySet;
import net.kyori.adventure.text.Component;
import org.bukkit.NamespacedKey;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.enchantments.EnchantmentTarget;
import org.bukkit.entity.EntityCategory;
import org.bukkit.entity.EntityType;
import org.bukkit.inventory.EquipmentSlotGroup;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.ItemType;

import java.util.Set;

/**
 * Minimal {@link Enchantment} value used by {@link BridgeRegistryAccess}.
 *
 * <p>Bukkit's static enchantment constants resolve through
 * {@code Registry.ENCHANTMENT} during class initialization. The Rust host has
 * no CraftEnchantment, so the bridge supplies a named value object. Gameplay
 * effects (damage, protection, efficiency) are not modelled here; the Rust
 * component map stores only the id/level pair.</p>
 */
final class BridgeEnchantment extends Enchantment {
    private final NamespacedKey key;

    BridgeEnchantment(NamespacedKey key) {
        this.key = key;
    }

    @Override
    public NamespacedKey getKey() {
        return key;
    }

    @Override
    public String getName() {
        return key.getKey();
    }

    @Override
    public int getMaxLevel() {
        return 255;
    }

    @Override
    public int getStartLevel() {
        return 1;
    }

    @Override
    public EnchantmentTarget getItemTarget() {
        return EnchantmentTarget.BREAKABLE;
    }

    @Override
    public boolean isTreasure() {
        return false;
    }

    @Override
    public boolean isCursed() {
        return false;
    }

    @Override
    public boolean conflictsWith(Enchantment other) {
        return false;
    }

    @Override
    public boolean canEnchantItem(ItemStack item) {
        return item != null && !item.isEmpty();
    }

    @Override
    public Component displayName(int level) {
        return Component.text(getName() + " " + level);
    }

    @Override
    public boolean isTradeable() {
        return true;
    }

    @Override
    public boolean isDiscoverable() {
        return true;
    }

    @Override
    public int getMinModifiedCost(int level) {
        return level;
    }

    @Override
    public int getMaxModifiedCost(int level) {
        return level + 10;
    }

    @Override
    public int getAnvilCost() {
        return 1;
    }

    @Override
    public EnchantmentRarity getRarity() {
        return EnchantmentRarity.COMMON;
    }

    @Override
    public float getDamageIncrease(int level, EntityCategory entityCategory) {
        return 0.0F;
    }

    @Override
    public float getDamageIncrease(int level, EntityType entityType) {
        return 0.0F;
    }

    @Override
    public Set<EquipmentSlotGroup> getActiveSlotGroups() {
        return Set.of(EquipmentSlotGroup.ANY);
    }

    @Override
    public Component description() {
        return Component.text(getName());
    }

    @Override
    public RegistryKeySet<ItemType> getSupportedItems() {
        return null;
    }

    @Override
    public RegistryKeySet<ItemType> getPrimaryItems() {
        return null;
    }

    @Override
    public int getWeight() {
        return 1;
    }

    @Override
    public RegistryKeySet<Enchantment> getExclusiveWith() {
        return null;
    }

    @Override
    public String translationKey() {
        return "enchantment.minecraft." + key.getKey();
    }

    @Override
    public String getTranslationKey() {
        return translationKey();
    }

    @Override
    public String toString() {
        return key.toString();
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Enchantment enchantment && key.equals(enchantment.getKey());
    }

    @Override
    public int hashCode() {
        return key.hashCode();
    }
}
