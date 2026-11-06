package rs.folia.bridge;

import io.papermc.paper.registry.RegistryAccess;
import io.papermc.paper.registry.RegistryKey;
import io.papermc.paper.registry.tag.Tag;
import io.papermc.paper.registry.tag.TagKey;
import org.bukkit.Keyed;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.block.Biome;
import org.bukkit.block.BlockType;
import org.bukkit.inventory.ItemType;

import java.lang.reflect.Proxy;
import java.util.Iterator;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Function;
import java.util.stream.Stream;

/**
 * Minimal Paper {@link RegistryAccess} implementation for the embedded host.
 *
 * <p>Modern {@code ItemStack} construction goes through
 * {@code Material.asItemType()}, which resolves {@code Registry.ITEM}. Without a
 * registry access implementation the Bukkit API throws
 * {@code No RegistryAccess implementation found} before a plugin can create a
 * single item. This shim provides material, item, block, and biome lookups from
 * the API enums; the Rust registry remains authoritative for game data.</p>
 */
public final class BridgeRegistryAccess implements RegistryAccess {
    private static final Map<String, BridgeBiome> BIOMES = new ConcurrentHashMap<>();

    static BridgeBiome biomeValue(String path) {
        String normalized = path.toLowerCase(Locale.ROOT);
        BridgeBiome existing = BIOMES.get(normalized);
        if (existing != null) {
            return existing;
        }
        // computeIfAbsent is not re-entrant: constructing BridgeBiome
        // initializes the Biome interface, which asks this registry for more
        // biomes. A putIfAbsent avoids the recursive-update failure.
        BridgeBiome created = new BridgeBiome(normalized);
        BridgeBiome previous = BIOMES.putIfAbsent(normalized, created);
        return previous == null ? created : previous;
    }

    @Override
    @SuppressWarnings("removal")
    public <T extends Keyed> Registry<T> getRegistry(Class<T> type) {
        if (type == Material.class) {
            return cast(materialRegistry());
        }
        if (type == ItemType.class) {
            return cast(itemRegistry());
        }
        if (type == BlockType.class) {
            return cast(blockRegistry());
        }
        if (type == Biome.class) {
            return cast(biomeRegistry());
        }
        return cast(emptyRegistry());
    }

    @Override
    public <T extends Keyed> Registry<T> getRegistry(RegistryKey<T> key) {
        String name = key.key().value();
        if (name.equals("item") || name.endsWith("/item")) {
            return cast(itemRegistry());
        }
        if (name.equals("block") || name.endsWith("/block")) {
            return cast(blockRegistry());
        }
        if (name.equals("biome") || name.endsWith("/biome")) {
            return cast(biomeRegistry());
        }
        return cast(emptyRegistry());
    }

    @SuppressWarnings("unchecked")
    private static <T extends Keyed> Registry<T> cast(Registry<?> registry) {
        return (Registry<T>) registry;
    }

    private static Registry<Material> materialRegistry() {
        return new SimpleRegistry<>(
                Material.class,
                key -> Material.matchMaterial(key.toString()),
                () -> Stream.of(Material.values()),
                material -> material.getKey()
        );
    }

    private static Registry<ItemType> itemRegistry() {
        return new SimpleRegistry<>(
                ItemType.class,
                key -> {
                    Material material = Material.matchMaterial(key.toString());
                    return material == null ? null : itemType(material);
                },
                () -> Stream.of(Material.values()).map(BridgeRegistryAccess::itemType),
                ItemType::getKey
        );
    }

    private static Registry<BlockType> blockRegistry() {
        return new SimpleRegistry<>(
                BlockType.class,
                key -> {
                    Material material = Material.matchMaterial(key.toString());
                    return material == null ? null : blockType(material);
                },
                () -> Stream.of(Material.values()).map(BridgeRegistryAccess::blockType),
                BlockType::getKey
        );
    }

    private static Registry<Biome> biomeRegistry() {
        // Do not touch Biome.values()/valueOf() here: Bukkit's Biome interface
        // initializes its constants through Registry.BIOME, so the registry has
        // to exist before the interface is initialized.
        return new SimpleRegistry<>(
                Biome.class,
                key -> biomeValue(key.getKey()),
                () -> Stream.of(Biome.values()),
                biome -> NamespacedKey.minecraft(biome.name().toLowerCase(Locale.ROOT))
        );
    }

    private static Registry<Keyed> emptyRegistry() {
        return new SimpleRegistry<>(Keyed.class, key -> null, Stream::empty, value -> null);
    }

    private static ItemType itemType(Material material) {
        NamespacedKey key = material.getKey();
        return (ItemType) Proxy.newProxyInstance(
                BridgeRegistryAccess.class.getClassLoader(),
                new Class<?>[]{ItemType.class},
                (proxy, method, args) -> {
                    if (method.getDeclaringClass() == Object.class) {
                        return switch (method.getName()) {
                            case "toString" -> "BridgeItemType{" + key + "}";
                            case "hashCode" -> key.hashCode();
                            case "equals" -> proxy == args[0];
                            default -> null;
                        };
                    }
                    return switch (method.getName()) {
                        case "createItemStack" -> new BridgeItemStack(
                                material,
                                args == null || args.length == 0 ? 1 : (Integer) args[0]);
                        case "asMaterial" -> material;
                        case "getKey" -> key;
                        case "getMaxStackSize" -> 64;
                        case "getMaxDurability" -> (short) 0;
                        case "isAir" -> material == Material.AIR;
                        case "getTranslationKey", "translationKey" -> "item.minecraft."
                                + key.getKey();
                        default -> BridgeValues.defaultValue(method);
                    };
                }
        );
    }

    private static BlockType blockType(Material material) {
        NamespacedKey key = material.getKey();
        return (BlockType) Proxy.newProxyInstance(
                BridgeRegistryAccess.class.getClassLoader(),
                new Class<?>[]{BlockType.class},
                (proxy, method, args) -> {
                    if (method.getDeclaringClass() == Object.class) {
                        return switch (method.getName()) {
                            case "toString" -> "BridgeBlockType{" + key + "}";
                            case "hashCode" -> key.hashCode();
                            case "equals" -> proxy == args[0];
                            default -> null;
                        };
                    }
                    return switch (method.getName()) {
                        case "asMaterial" -> material;
                        case "getKey" -> key;
                        case "isAir" -> material == Material.AIR;
                        case "getTranslationKey", "translationKey" -> "block.minecraft."
                                + key.getKey();
                        default -> BridgeValues.defaultValue(method);
                    };
                }
        );
    }

    /**
     * A biome value that does not depend on Bukkit's enum-style static fields.
     *
     * <p>Paper's {@code Biome} is an interface whose static constants resolve
     * through {@code Registry.BIOME}. The server normally supplies
     * {@code CraftBiome} instances, so the bridge supplies its own value object
     * for the same keys.</p>
     */
    static final class BridgeBiome implements Biome {
        private final String path;
        private final NamespacedKey key;

        BridgeBiome(String path) {
            this.path = path.toLowerCase(Locale.ROOT);
            this.key = NamespacedKey.minecraft(this.path);
        }

        @Override
        public NamespacedKey getKey() {
            return key;
        }

        @Override
        public String name() {
            return path.toUpperCase(Locale.ROOT);
        }

        @Override
        public int ordinal() {
            return 0;
        }

        @Override
        public int compareTo(Biome other) {
            return name().compareTo(other.name());
        }

        @Override
        public String translationKey() {
            return "biome.minecraft." + path;
        }

        @Override
        public String toString() {
            return key.toString();
        }

        @Override
        public boolean equals(Object other) {
            return other instanceof Biome biome && key.equals(biome.getKey());
        }

        @Override
        public int hashCode() {
            return key.hashCode();
        }
    }

    private static final class SimpleRegistry<T extends Keyed> implements Registry<T> {
        private final Class<T> type;
        private final Function<NamespacedKey, T> lookup;
        private final java.util.function.Supplier<Stream<T>> values;
        private final Function<T, NamespacedKey> keyOf;

        private SimpleRegistry(Class<T> type, Function<NamespacedKey, T> lookup,
                               java.util.function.Supplier<Stream<T>> values,
                               Function<T, NamespacedKey> keyOf) {
            this.type = type;
            this.lookup = lookup;
            this.values = values;
            this.keyOf = keyOf;
        }

        @Override
        public T get(NamespacedKey key) {
            return lookup.apply(key);
        }

        @Override
        public NamespacedKey getKey(T value) {
            return keyOf.apply(value);
        }

        @Override
        public boolean hasTag(TagKey<T> key) {
            return false;
        }

        @Override
        public Tag<T> getTag(TagKey<T> key) {
            return null;
        }

        @Override
        public java.util.Collection<Tag<T>> getTags() {
            return List.of();
        }

        @Override
        public Stream<T> stream() {
            return values.get();
        }

        @Override
        public Stream<NamespacedKey> keyStream() {
            return stream().map(keyOf);
        }

        @Override
        public int size() {
            return (int) stream().count();
        }

        @Override
        public Iterator<T> iterator() {
            return stream().iterator();
        }

        @Override
        public String toString() {
            return "BridgeRegistry{" + type.getSimpleName() + "}";
        }
    }
}
