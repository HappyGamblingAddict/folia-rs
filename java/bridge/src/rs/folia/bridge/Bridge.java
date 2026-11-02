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

    public static Logger logger() {
        return LOGGER;
    }
}
