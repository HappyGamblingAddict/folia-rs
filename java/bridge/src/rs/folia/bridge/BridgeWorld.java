package rs.folia.bridge;

import org.bukkit.Bukkit;
import org.bukkit.Chunk;
import org.bukkit.Difficulty;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.NamespacedKey;
import org.bukkit.Server;
import org.bukkit.World;
import org.bukkit.WorldType;
import org.bukkit.block.Biome;
import org.bukkit.block.Block;
import org.bukkit.block.BlockFace;
import org.bukkit.block.BlockState;
import org.bukkit.block.data.BlockData;
import org.bukkit.entity.Entity;
import org.bukkit.entity.LivingEntity;
import org.bukkit.entity.Player;
import org.bukkit.generator.BiomeProvider;
import org.bukkit.generator.WorldInfo;
import org.bukkit.inventory.ItemStack;
import org.bukkit.util.BoundingBox;
import org.bukkit.util.RayTraceResult;
import org.bukkit.util.Vector;

import java.io.File;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.Collection;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.function.Supplier;

/**
 * Rust-backed {@link World} and {@link Block} proxies.
 *
 * <p>Only the overworld is served. Block reads and writes go through the Rust
 * JNI bridge, which uses the same process-wide {@code WorldStorage} as the
 * native game loop. Chunks that are neither cached nor stored read as air; the
 * bridge does not generate terrain itself.</p>
 */
final class BridgeWorld implements InvocationHandler {
    private final String name;
    private final File folder;
    private final Server server;
    private final World proxy;
    private Supplier<List<Player>> players = List::of;

    BridgeWorld(String name, File folder, Server server) {
        this.name = name;
        this.folder = folder;
        this.server = server;
        this.proxy = (World) Proxy.newProxyInstance(
                BridgeWorld.class.getClassLoader(),
                new Class<?>[]{World.class},
                this
        );
    }

    World proxy() {
        return proxy;
    }

    String name() {
        return name;
    }

    void setPlayerSupplier(Supplier<List<Player>> players) {
        this.players = players;
    }

    static String key(Material material) {
        return material == null ? "minecraft:air" : material.getKey().toString();
    }

    static Material material(String key) {
        if (key == null || key.isEmpty()) {
            return Material.AIR;
        }
        Material material = Material.matchMaterial(key);
        return material == null ? Material.AIR : material;
    }

    static Biome biome(String key) {
        if (key == null || key.isEmpty()) {
            return BridgeRegistryAccess.biomeValue("plains");
        }
        String path = key.contains(":") ? key.substring(key.indexOf(':') + 1) : key;
        return BridgeRegistryAccess.biomeValue(path);
    }

    Block blockAt(int x, int y, int z) {
        return (Block) Proxy.newProxyInstance(
                BridgeWorld.class.getClassLoader(),
                new Class<?>[]{Block.class},
                new BlockHandler(x, y, z, false)
        );
    }

    BlockState blockStateAt(int x, int y, int z) {
        return (BlockState) Proxy.newProxyInstance(
                BridgeWorld.class.getClassLoader(),
                new Class<?>[]{BlockState.class},
                new BlockHandler(x, y, z, true)
        );
    }

    Chunk chunkAt(int chunkX, int chunkZ) {
        return (Chunk) Proxy.newProxyInstance(
                BridgeWorld.class.getClassLoader(),
                new Class<?>[]{Chunk.class},
                (proxyObject, method, args) -> {
                    if (method.getDeclaringClass() == Object.class) {
                        return switch (method.getName()) {
                            case "toString" -> "BridgeChunk{" + chunkX + "," + chunkZ + "}";
                            case "hashCode" -> 31 * chunkX + chunkZ;
                            case "equals" -> proxyObject == args[0];
                            default -> null;
                        };
                    }
                    return switch (method.getName()) {
                        case "getX" -> chunkX;
                        case "getZ" -> chunkZ;
                        case "getWorld" -> proxy;
                        case "getBlock" -> blockAt(
                                (chunkX << 4) + (Integer) args[0],
                                (Integer) args[1],
                                (chunkZ << 4) + (Integer) args[2]);
                        case "isGenerated", "isLoaded" -> true;
                        case "load" -> true;
                        case "unload", "isSlimeChunk", "isForceLoaded",
                             "addPluginChunkTicket", "removePluginChunkTicket" -> false;
                        case "getEntities" -> new Entity[0];
                        case "getTileEntities" -> args == null || args.length <= 1
                                ? new BlockState[0] : List.of();
                        case "getPlayersSeeingChunk", "getStructures",
                             "getPluginChunkTickets" -> List.of();
                        case "getInhabitedTime" -> 0L;
                        case "contains" -> false;
                        default -> BridgeValues.defaultValue(method);
                    };
                }
        );
    }

    Location spawnLocation() {
        String encoded = Bridge.nativeWorldSpawn(name);
        if (encoded == null || encoded.isEmpty()) {
            return new Location(proxy, 0.5D, 64.0D, 0.5D);
        }
        String[] fields = encoded.split("\t", -1);
        if (fields.length < 3) {
            return new Location(proxy, 0.5D, 64.0D, 0.5D);
        }
        try {
            return new Location(
                    proxy,
                    Double.parseDouble(fields[0]),
                    Double.parseDouble(fields[1]),
                    Double.parseDouble(fields[2])
            );
        } catch (NumberFormatException exception) {
            return new Location(proxy, 0.5D, 64.0D, 0.5D);
        }
    }

    @Override
    public Object invoke(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return switch (method.getName()) {
                case "toString" -> "BridgeWorld{" + name + "}";
                case "hashCode" -> name.hashCode();
                case "equals" -> proxyObject == args[0];
                default -> null;
            };
        }
        return switch (method.getName()) {
            case "getName" -> name;
            case "getUID" -> UUID.nameUUIDFromBytes(("folia-world:" + name).getBytes());
            case "getKey" -> NamespacedKey.minecraft("overworld");
            case "getEnvironment" -> World.Environment.NORMAL;
            case "getSeed" -> 0L;
            case "getMinHeight" -> -64;
            case "getMaxHeight" -> 320;
            case "getSeaLevel" -> 63;
            case "getWorldFolder" -> folder;
            case "getWorldType" -> WorldType.NORMAL;
            case "getDifficulty" -> Difficulty.NORMAL;
            case "getSpawnLocation" -> spawnLocation();
            case "setSpawnLocation" -> false;
            case "getTime", "getFullTime" -> 0L;
            case "setTime", "setFullTime" -> null;
            case "isDayTime" -> true;
            case "hasStorm", "isThundering" -> false;
            case "getWeatherDuration", "getThunderDuration", "getClearWeatherDuration" -> 0;
            case "getKeepSpawnInMemory" -> true;
            case "setKeepSpawnInMemory" -> null;
            case "getBlockAt" -> switch (args.length) {
                case 1 -> {
                    Location location = (Location) args[0];
                    yield blockAt(location.getBlockX(), location.getBlockY(), location.getBlockZ());
                }
                default -> blockAt((Integer) args[0], (Integer) args[1], (Integer) args[2]);
            };
            case "getHighestBlockYAt" -> {
                if (args[0] instanceof Location location) {
                    yield Bridge.nativeWorldHighestBlockY(
                            name, location.getBlockX(), location.getBlockZ());
                }
                yield Bridge.nativeWorldHighestBlockY(name, (Integer) args[0], (Integer) args[1]);
            }
            case "getHighestBlockAt" -> {
                if (args[0] instanceof Location location) {
                    yield blockAt(location.getBlockX(),
                            Bridge.nativeWorldHighestBlockY(name,
                                    location.getBlockX(), location.getBlockZ()),
                            location.getBlockZ());
                }
                yield blockAt((Integer) args[0],
                        Bridge.nativeWorldHighestBlockY(name, (Integer) args[0], (Integer) args[1]),
                        (Integer) args[1]);
            }
            case "getBiome", "getComputedBiome" -> switch (args.length) {
                case 1 -> {
                    Location location = (Location) args[0];
                    yield biome(Bridge.nativeWorldBiome(name,
                            location.getBlockX(), location.getBlockY(), location.getBlockZ()));
                }
                default -> biome(Bridge.nativeWorldBiome(
                        name, (Integer) args[0], (Integer) args[1], (Integer) args[2]));
            };
            case "setBiome" -> null;
            case "getBlockLightAt" -> Bridge.nativeWorldBlockLight(
                    name, (Integer) args[0], (Integer) args[1], (Integer) args[2]);
            case "getSkyLightAt" -> Bridge.nativeWorldSkyLight(
                    name, (Integer) args[0], (Integer) args[1], (Integer) args[2]);
            case "getType" -> switch (args.length) {
                case 1 -> {
                    Location location = (Location) args[0];
                    yield material(Bridge.nativeWorldBlock(name,
                            location.getBlockX(), location.getBlockY(), location.getBlockZ()));
                }
                default -> material(Bridge.nativeWorldBlock(
                        name, (Integer) args[0], (Integer) args[1], (Integer) args[2]));
            };
            case "setType" -> {
                if (args.length == 1) {
                    Location location = (Location) args[0];
                    Bridge.nativeWorldSetBlock(name,
                            location.getBlockX(), location.getBlockY(), location.getBlockZ(),
                            key((Material) args[1]));
                } else {
                    Bridge.nativeWorldSetBlock(name,
                            (Integer) args[0], (Integer) args[1], (Integer) args[2],
                            key((Material) args[3]));
                }
                yield null;
            }
            case "getBlockData" -> switch (args.length) {
                case 1 -> {
                    Location location = (Location) args[0];
                    yield blockData(location.getBlockX(), location.getBlockY(), location.getBlockZ());
                }
                default -> blockData((Integer) args[0], (Integer) args[1], (Integer) args[2]);
            };
            case "setBlockData" -> {
                BlockData data = (BlockData) (args.length == 1 ? args[1] : args[3]);
                if (args.length == 1) {
                    Location location = (Location) args[0];
                    Bridge.nativeWorldSetBlock(name,
                            location.getBlockX(), location.getBlockY(), location.getBlockZ(),
                            key(data.getMaterial()));
                } else {
                    Bridge.nativeWorldSetBlock(name,
                            (Integer) args[0], (Integer) args[1], (Integer) args[2],
                            key(data.getMaterial()));
                }
                yield null;
            }
            case "getBlockState" -> switch (args.length) {
                case 1 -> {
                    Location location = (Location) args[0];
                    yield blockStateAt(location.getBlockX(), location.getBlockY(), location.getBlockZ());
                }
                default -> blockStateAt((Integer) args[0], (Integer) args[1], (Integer) args[2]);
            };
            case "getChunkAt" -> switch (args.length) {
                case 1 -> {
                    Object value = args[0];
                    if (value instanceof Location location) {
                        yield chunkAt(location.getBlockX() >> 4, location.getBlockZ() >> 4);
                    }
                    if (value instanceof Block block) {
                        yield chunkAt(block.getX() >> 4, block.getZ() >> 4);
                    }
                    yield chunkAt(0, 0);
                }
                default -> chunkAt((Integer) args[0], (Integer) args[1]);
            };
            case "isChunkGenerated", "isChunkLoaded", "isChunkInUse" -> false;
            case "loadChunk", "unloadChunk", "unloadChunkRequest", "refreshChunk",
                 "regenerateChunk" -> false;
            case "getLoadedChunks" -> new Chunk[0];
            case "getForceLoadedChunks" -> List.of();
            case "getForceLoadedChunkCount" -> 0;
            case "getPlayers" -> List.copyOf(players.get());
            case "getPlayerCount" -> players.get().size();
            case "getEntities", "getLivingEntities" -> List.of();
            case "getEntitiesByClass", "getEntitiesByClasses", "getNearbyEntities",
                 "getIntersectingChunks", "getPlayersSeeingChunk",
                 "getNearbyPlayers", "getNearbyLivingEntities" -> List.of();
            case "getPluginChunkTickets" -> args == null || args.length == 0
                    ? Map.of() : List.of();
            case "getEntity" -> null;
            case "getEntityCount", "getTileEntityCount", "getTickableTileEntityCount",
                 "getChunkCount" -> 0;
            case "spawnEntity", "spawn", "createEntity", "dropItem", "dropItemNaturally",
                 "strikeLightning", "strikeLightningEffect", "spawnArrow" -> null;
            case "generateTree", "hasStructureAt", "lineOfSightExists", "hasCollisionsIn",
                 "addPluginChunkTicket", "removePluginChunkTicket", "setChunkForceLoaded",
                 "isChunkForceLoaded" -> false;
            case "removePluginChunkTickets" -> null;
            case "rayTrace", "rayTraceBlocks", "rayTraceEntities" -> (RayTraceResult) null;
            case "getGameRules" -> null;
            case "getBiomeProvider", "vanillaBiomeProvider" -> (BiomeProvider) null;
            case "getMoonPhase" -> null;
            case "getStructures" -> List.of();
            case "isVoidDamageEnabled" -> false;
            case "setVoidDamageEnabled" -> null;
            case "getVoidDamageAmount" -> 0.0F;
            case "setVoidDamageAmount" -> null;
            case "getVoidDamageMinBuildHeightOffset" -> 0.0D;
            case "setVoidDamageMinBuildHeightOffset" -> null;
            case "isPositionLoaded" -> true;
            case "getLocationAtKey" -> new Location(proxy, 0.5D, 64.0D, 0.5D);
            case "getBlockAtKey" -> blockAt(0, 64, 0);
            case "getPersistentDataContainer" -> null;
            case "getFeatureFlags" -> null;
            default -> BridgeValues.defaultValue(method);
        };
    }

    private BlockData blockData(int x, int y, int z) {
        Material material = material(Bridge.nativeWorldBlock(name, x, y, z));
        return (BlockData) Proxy.newProxyInstance(
                BridgeWorld.class.getClassLoader(),
                new Class<?>[]{BlockData.class},
                (proxyObject, method, args) -> {
                    if (method.getDeclaringClass() == Object.class) {
                        return switch (method.getName()) {
                            case "toString" -> material.getKey().toString();
                            case "hashCode" -> material.hashCode();
                            case "equals" -> proxyObject == args[0];
                            default -> null;
                        };
                    }
                    return switch (method.getName()) {
                        case "getMaterial" -> material;
                        case "getAsString" -> material.getKey().toString();
                        case "matches" -> args != null && args[0] instanceof BlockData other
                                && other.getMaterial() == material;
                        case "clone" -> blockData(x, y, z);
                        case "isAir" -> material == Material.AIR;
                        case "isSolid", "isOccluding", "isBurnable", "isFlammable",
                             "isLiquid", "isPassable" -> false;
                        case "getHardness", "getBlastResistance" -> 0.0F;
                        case "getLightEmission" -> 0;
                        default -> BridgeValues.defaultValue(method);
                    };
                }
        );
    }

    private final class BlockHandler implements InvocationHandler {
        private final int x;
        private final int y;
        private final int z;
        private final boolean state;

        private BlockHandler(int x, int y, int z, boolean state) {
            this.x = x;
            this.y = y;
            this.z = z;
            this.state = state;
        }

        @Override
        public Object invoke(Object proxyObject, Method method, Object[] args) {
            if (method.getDeclaringClass() == Object.class) {
                return switch (method.getName()) {
                    case "toString" -> (state ? "BridgeBlockState{" : "BridgeBlock{")
                            + x + "," + y + "," + z + "}";
                    case "hashCode" -> (x * 31 + y) * 31 + z;
                    case "equals" -> proxyObject == args[0];
                    default -> null;
                };
            }
            String nameOfMethod = method.getName();
            if (nameOfMethod.equals("getBlock") && state) {
                return blockAt(x, y, z);
            }
            if (nameOfMethod.equals("getWorld")) {
                return proxy;
            }
            if (nameOfMethod.equals("getLocation")) {
                Location location = new Location(proxy, x, y, z);
                if (args != null && args.length == 1 && args[0] instanceof Location target) {
                    target.setWorld(proxy);
                    target.setX(x);
                    target.setY(y);
                    target.setZ(z);
                    return target;
                }
                return location;
            }
            return switch (nameOfMethod) {
                case "getX" -> x;
                case "getY" -> y;
                case "getZ" -> z;
                case "getType", "getMaterial" -> material(Bridge.nativeWorldBlock(name, x, y, z));
                case "setType", "setMaterial" -> {
                    if (args != null && args.length > 0) {
                        Bridge.nativeWorldSetBlock(name, x, y, z, key((Material) args[0]));
                    }
                    yield null;
                }
                case "getBlockData" -> blockData(x, y, z);
                case "setBlockData" -> {
                    if (args != null && args.length > 0 && args[0] instanceof BlockData data) {
                        Bridge.nativeWorldSetBlock(name, x, y, z, key(data.getMaterial()));
                    }
                    yield null;
                }
                case "getState" -> blockStateAt(x, y, z);
                case "setState" -> {
                    if (args != null && args.length > 0 && args[0] instanceof BlockState blockState) {
                        Bridge.nativeWorldSetBlock(name, x, y, z, key(blockState.getType()));
                    }
                    yield null;
                }
                case "getBiome" -> biome(Bridge.nativeWorldBiome(name, x, y, z));
                case "getLightLevel" -> (byte) Math.max(
                        Bridge.nativeWorldBlockLight(name, x, y, z),
                        Bridge.nativeWorldSkyLight(name, x, y, z));
                case "getLightFromSky" -> (byte) Bridge.nativeWorldSkyLight(name, x, y, z);
                case "getLightFromBlocks" -> (byte) Bridge.nativeWorldBlockLight(name, x, y, z);
                case "isEmpty" -> material(Bridge.nativeWorldBlock(name, x, y, z)) == Material.AIR;
                case "isLiquid" -> {
                    Material current = material(Bridge.nativeWorldBlock(name, x, y, z));
                    yield current == Material.WATER || current == Material.LAVA;
                }
                case "getRelative" -> {
                    if (args != null && args.length == 1 && args[0] instanceof BlockFace face) {
                        yield blockAt(x + face.getModX(), y + face.getModY(), z + face.getModZ());
                    }
                    if (args != null && args.length == 3) {
                        yield blockAt(x + (Integer) args[0],
                                y + (Integer) args[1], z + (Integer) args[2]);
                    }
                    yield blockAt(x, y, z);
                }
                case "getChunk" -> chunkAt(x >> 4, z >> 4);
                case "update" -> false;
                case "isPlaced" -> state;
                case "breakNaturally" -> false;
                case "getDrops" -> List.of();
                case "getTemperature", "getHumidity" -> 0.5D;
                case "isBlockPowered", "isBlockIndirectlyPowered" -> false;
                case "getFace" -> BlockFace.SELF;
                case "getBlockKey" -> 0L;
                default -> BridgeValues.defaultValue(method);
            };
        }
    }
}
