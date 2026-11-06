package rs.folia.bridge;

import java.io.File;
import java.util.logging.Logger;

/**
 * Entry point called from the Rust host through JNI.
 *
 * <p>The Rust process owns the game loop and calls {@link #tick()} once per server
 * tick. The JVM side only owns plugin lifecycle and the Bukkit API surface.</p>
 */
public final class Bridge {
    private static final Logger LOGGER = Logger.getLogger("FoliaRS");
    private static PluginHost host;

    private Bridge() {
    }

    /** Round-trip probe used by the example plugin to prove JNI works both ways. */
    public static native String nativePing(String message);

    /** Schedules a plugin task in the Rust tick loop. Returns a task id, or -1. */
    public static native int nativeScheduleTask(String pluginName, Runnable task, long delay, long period);

    public static native void nativeCancelTask(int taskId);

    public static native void nativeCancelTasks(String pluginName);

    // ---- World/block bridge -------------------------------------------------

    /** Block state name at a coordinate, or {@code minecraft:air}. */
    public static native String nativeWorldBlock(String world, int x, int y, int z);

    /** Sets a block by material key. Returns true when the Rust world accepted it. */
    public static native boolean nativeWorldSetBlock(String world, int x, int y, int z, String material);

    /** First empty Y above the highest non-air block, or the world's minimum Y. */
    public static native int nativeWorldHighestBlockY(String world, int x, int z);

    /** Namespaced biome key, e.g. {@code minecraft:plains}. */
    public static native String nativeWorldBiome(String world, int x, int y, int z);

    public static native int nativeWorldBlockLight(String world, int x, int y, int z);

    public static native int nativeWorldSkyLight(String world, int x, int y, int z);

    /** Spawn position encoded as {@code x\ty\tz}. */
    public static native String nativeWorldSpawn(String world);

    // ---- Player bridge ------------------------------------------------------

    public static native boolean nativePlayerExists(String uuid);

    /** Associates the Java player proxy's display name with the Rust record. */
    public static native void nativePlayerIdentify(String name, String uuid);

    public static native double nativePlayerHealth(String uuid);

    public static native void nativePlayerSetHealth(String uuid, double health);

    public static native int nativePlayerFood(String uuid);

    public static native void nativePlayerSetFood(String uuid, int food);

    public static native double nativePlayerSaturation(String uuid);

    public static native void nativePlayerSetSaturation(String uuid, double saturation);

    /** Position encoded as {@code x\ty\tz\tyaw\tpitch}. */
    public static native String nativePlayerLocation(String uuid);

    public static native boolean nativePlayerTeleport(
            String uuid, double x, double y, double z, float yaw, float pitch);

    public static native int nativePlayerGameMode(String uuid);

    public static native void nativePlayerSetGameMode(String uuid, int gameMode);

    public static native void nativePlayerSendMessage(String uuid, String message);

    public static native void nativePlayerSave(String uuid);

    /** Encoded inventory rows, one {@code slot\titem...} row per non-empty slot. */
    public static native String nativePlayerInventory(String uuid);

    /** Replaces all 36 storage slots from the encoded inventory. */
    public static native boolean nativePlayerSetInventory(String uuid, String encoded);

    /** One item encoded as {@code item\tcount\tdamage\thex(name)\tunbreakable[\thex(lore)]*}. */
    public static native String nativePlayerItem(String uuid, int slot);

    public static native boolean nativePlayerSetItem(String uuid, int slot, String encoded);

    public static native int nativePlayerSelectedSlot(String uuid);

    public static native boolean nativePlayerSetSelectedSlot(String uuid, int slot);

    public static void boot(String pluginsDirectory, String dataDirectory) throws Exception {
        File plugins = new File(pluginsDirectory);
        File data = new File(dataDirectory);
        if (!plugins.isDirectory() && !plugins.mkdirs()) {
            throw new IllegalStateException("Could not create plugins directory: " + plugins);
        }
        if (!data.isDirectory() && !data.mkdirs()) {
            throw new IllegalStateException("Could not create data directory: " + data);
        }

        host = new PluginHost(plugins, data);
        host.loadAll();
    }

    public static void tick() {
        PluginHost current = host;
        if (current != null) {
            current.tick();
        }
    }

    public static void shutdown() {
        PluginHost current = host;
        host = null;
        if (current != null) {
            current.disableAll();
        }
    }

    public static void dispatchCommand(String line) {
        PluginHost current = host;
        if (current == null) {
            LOGGER.warning("Cannot dispatch /" + line + " before the plugin host is ready");
            return;
        }
        boolean handled = current.dispatchCommand(line);
        LOGGER.info("Command /" + line + " handled=" + handled);
    }

    public static void firePlayerJoin(String name, String uuid) {
        PluginHost current = host;
        if (current != null) {
            current.firePlayerJoin(name, uuid);
        }
    }

    /** Fired by the Rust host before a player breaks a block. Returns cancelled. */
    public static boolean fireBlockBreak(String name, String uuid, int x, int y, int z) {
        PluginHost current = host;
        return current != null && current.fireBlockBreak(name, uuid, x, y, z);
    }

    /** Fired by the Rust host before a player places a block. Returns cancelled. */
    public static boolean fireBlockPlace(String name, String uuid, int x, int y, int z, String material) {
        PluginHost current = host;
        return current != null && current.fireBlockPlace(name, uuid, x, y, z, material);
    }

    /** Fired by the Rust host for a player interaction. Returns cancelled. */
    public static boolean firePlayerInteract(String name, String uuid, int x, int y, int z,
                                             String material, String action, String face) {
        PluginHost current = host;
        return current != null
                && current.firePlayerInteract(name, uuid, x, y, z, material, action, face);
    }

    public static Logger logger() {
        return LOGGER;
    }
}
