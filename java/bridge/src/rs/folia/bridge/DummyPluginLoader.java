package rs.folia.bridge;

import org.bukkit.event.Event;
import org.bukkit.event.Listener;
import org.bukkit.plugin.InvalidDescriptionException;
import org.bukkit.plugin.InvalidPluginException;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.PluginDescriptionFile;
import org.bukkit.plugin.PluginLoader;
import org.bukkit.plugin.RegisteredListener;
import org.bukkit.plugin.UnknownDependencyException;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.util.Map;
import java.util.Set;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.regex.Pattern;

/**
 * Service-loader shim required by {@code JavaPlugin}'s static initializer.
 *
 * <p>Paper's {@link JavaPlugin} looks up a {@link PluginLoader} through
 * {@code ServiceLoader}. On a normal Paper server that service is provided by
 * the server implementation. Our Rust host performs loading itself, so this
 * class only has to satisfy the contract; the real work stays in
 * {@link PluginHost}.</p>
 */
public final class DummyPluginLoader implements PluginLoader {
    private static final Pattern[] FILTERS = {Pattern.compile("\\.jar$")};

    public DummyPluginLoader() {
    }

    @Override
    public Plugin loadPlugin(File file) throws InvalidPluginException, UnknownDependencyException {
        throw new UnsupportedOperationException("Plugins are loaded by the Rust host");
    }

    @Override
    public PluginDescriptionFile getPluginDescription(File file) throws InvalidDescriptionException {
        try (JarFile jar = new JarFile(file)) {
            JarEntry entry = jar.getJarEntry("plugin.yml");
            if (entry == null) {
                throw new InvalidDescriptionException("Jar " + file.getName() + " does not contain plugin.yml");
            }
            try (InputStream input = jar.getInputStream(entry)) {
                return new PluginDescriptionFile(input);
            }
        } catch (IOException exception) {
            throw new InvalidDescriptionException(exception, "Could not read " + file.getName());
        }
    }

    @Override
    public Pattern[] getPluginFileFilters() {
        return FILTERS.clone();
    }

    @Override
    public Map<Class<? extends Event>, Set<RegisteredListener>> createRegisteredListeners(
            Listener listener, Plugin plugin) {
        return Map.of();
    }

    @Override
    public void enablePlugin(Plugin plugin) {
        if (plugin instanceof JavaPlugin javaPlugin) {
            javaPlugin.setEnabled(true);
        }
    }

    @Override
    public void disablePlugin(Plugin plugin) {
        if (plugin instanceof JavaPlugin javaPlugin) {
            javaPlugin.setEnabled(false);
        }
    }
}
