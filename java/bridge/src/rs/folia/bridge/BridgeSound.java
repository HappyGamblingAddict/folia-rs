package rs.folia.bridge;

import org.bukkit.NamespacedKey;
import org.bukkit.Sound;

/**
 * Minimal {@link Sound} value for {@link BridgeRegistryAccess}. The Rust host
 * has no client audio queue, so only identity/key access is meaningful.
 */
final class BridgeSound implements Sound {
    private final NamespacedKey key;

    BridgeSound(NamespacedKey key) {
        this.key = key;
    }

    @Override
    public NamespacedKey getKey() {
        return key;
    }

    @Override
    public String name() {
        return key.getKey().toUpperCase(java.util.Locale.ROOT);
    }

    @Override
    public int ordinal() {
        return 0;
    }

    @Override
    public int compareTo(Sound other) {
        return name().compareTo(other.name());
    }

    @Override
    public String toString() {
        return key.toString();
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Sound sound && key.equals(sound.getKey());
    }

    @Override
    public int hashCode() {
        return key.hashCode();
    }
}
