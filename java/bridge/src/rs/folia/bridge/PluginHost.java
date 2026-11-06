package rs.folia.bridge;

import org.bukkit.plugin.InvalidDescriptionException;
import org.bukkit.plugin.PluginDescriptionFile;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Loads Paper/Bukkit plugin jars into the embedded JVM and drives their
 * lifecycle from the Rust host.
 */
final class PluginHost {
    private final File pluginsDirectory;
    private final File dataDirectory;
    private final Logger log = Bridge.logger();
    private final BridgeServer server;
    private final List<LoadedPlugin> loaded = new ArrayList<>();

    PluginHost(File pluginsDirectory, File dataDirectory) {
        this.pluginsDirectory = pluginsDirectory;
        this.dataDirectory = dataDirectory;
        this.server = new BridgeServer(pluginsDirectory, dataDirectory);
    }

    void loadAll() {
        File[] jars = pluginsDirectory.listFiles((directory, name) ->
                name.toLowerCase(Locale.ROOT).endsWith(".jar"));
        if (jars == null || jars.length == 0) {
            log.info("No plugins found in " + pluginsDirectory);
            return;
        }

        Arrays.sort(jars, Comparator.comparing(File::getName));
        for (File jar : jars) {
            try {
                load(jar);
            } catch (Throwable throwable) {
                log.log(Level.SEVERE, "Could not load plugin " + jar.getName(), throwable);
            }
        }
    }

    private void load(File jar) throws Exception {
        PluginDescriptionFile description = readDescription(jar);
        File dataFolder = new File(dataDirectory, description.getName());
        Files.createDirectories(dataFolder.toPath());

        PluginClassLoader loader = new PluginClassLoader(
                jar.toURI().toURL(),
                PluginHost.class.getClassLoader(),
                description
        );

        Class<?> mainClass = Class.forName(description.getMain(), true, loader);
        if (!JavaPlugin.class.isAssignableFrom(mainClass)) {
            loader.close();
            throw new InvalidDescriptionException(
                    "Main class " + description.getMain() + " does not extend JavaPlugin");
        }

        JavaPlugin plugin = (JavaPlugin) mainClass.getDeclaredConstructor().newInstance();
        Logger logger = Logger.getLogger("Minecraft." + description.getName());
        plugin.init(server.proxy(), description, dataFolder, jar, loader, description, logger);
        server.registerCommands(plugin, description);

        server.registerPlugin(plugin);
        loaded.add(new LoadedPlugin(plugin, loader));

        try {
            plugin.onLoad();
            plugin.setEnabled(true);
            server.callPluginEnable(plugin);
            log.info("Enabled " + description.getFullName());
        } catch (Throwable throwable) {
            loaded.removeIf(candidate -> candidate.plugin == plugin);
            server.unregisterPlugin(plugin);
            try {
                loader.close();
            } catch (IOException ignored) {
                // Already handling the plugin failure.
            }
            throw throwable;
        }
    }

    private PluginDescriptionFile readDescription(File jar) throws IOException, InvalidDescriptionException {
        try (JarFile jarFile = new JarFile(jar)) {
            JarEntry entry = jarFile.getJarEntry("plugin.yml");
            if (entry == null) {
                throw new InvalidDescriptionException("Jar " + jar.getName() + " does not contain plugin.yml");
            }
            try (InputStream input = jarFile.getInputStream(entry)) {
                return new PluginDescriptionFile(input);
            }
        }
    }

    void tick() {
        server.tick();
    }

    boolean dispatchCommand(String line) {
        return server.dispatchCommand(line);
    }

    void firePlayerJoin(String name, String uuid) {
        server.firePlayerJoin(name, uuid);
    }

    boolean fireBlockBreak(String name, String uuid, int x, int y, int z) {
        return server.fireBlockBreak(name, uuid, x, y, z);
    }

    boolean fireBlockPlace(String name, String uuid, int x, int y, int z, String material) {
        return server.fireBlockPlace(name, uuid, x, y, z, material);
    }

    boolean firePlayerInteract(String name, String uuid, int x, int y, int z,
                               String material, String action, String face) {
        return server.firePlayerInteract(name, uuid, x, y, z, material, action, face);
    }

    void disableAll() {
        List<LoadedPlugin> reversed = new ArrayList<>(loaded);
        Collections.reverse(reversed);
        for (LoadedPlugin candidate : reversed) {
            try {
                candidate.plugin.setEnabled(false);
            } catch (Throwable throwable) {
                log.log(Level.WARNING, "Plugin " + candidate.plugin.getName() + " failed to disable", throwable);
            }
            try {
                server.callPluginDisable(candidate.plugin);
            } catch (Throwable throwable) {
                log.log(Level.WARNING, "PluginDisableEvent failed for " + candidate.plugin.getName(), throwable);
            }
            server.unregisterPlugin(candidate.plugin);
            try {
                candidate.loader.close();
            } catch (IOException exception) {
                log.log(Level.WARNING, "Could not close class loader for " + candidate.plugin.getName(), exception);
            }
        }
        loaded.clear();
    }

    private record LoadedPlugin(JavaPlugin plugin, PluginClassLoader loader) {
    }
}
