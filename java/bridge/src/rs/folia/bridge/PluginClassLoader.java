package rs.folia.bridge;

import io.papermc.paper.plugin.configuration.PluginMeta;
import io.papermc.paper.plugin.provider.classloader.ConfiguredPluginClassLoader;
import io.papermc.paper.plugin.provider.classloader.PluginClassLoaderGroup;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.IOException;
import java.net.URL;
import java.net.URLClassLoader;

/**
 * Minimal Paper-compatible plugin class loader.
 *
 * <p>Paper's {@link JavaPlugin} constructor refuses to initialise unless its
 * class loader implements {@link ConfiguredPluginClassLoader}, so this wrapper
 * provides just enough of that contract while delegating real class loading to
 * a normal {@link URLClassLoader}.</p>
 */
final class PluginClassLoader extends URLClassLoader implements ConfiguredPluginClassLoader {
    private final PluginMeta configuration;
    private volatile JavaPlugin plugin;

    PluginClassLoader(URL jar, ClassLoader parent, PluginMeta configuration) {
        super(new URL[]{jar}, parent);
        this.configuration = configuration;
    }

    @Override
    public PluginMeta getConfiguration() {
        return configuration;
    }

    @Override
    public Class<?> loadClass(String name, boolean resolve, boolean checkGlobal, boolean checkLibraries)
            throws ClassNotFoundException {
        return super.loadClass(name, resolve);
    }

    @Override
    public void init(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    @Override
    public JavaPlugin getPlugin() {
        return plugin;
    }

    @Override
    public PluginClassLoaderGroup getGroup() {
        return null;
    }

    @Override
    public void close() throws IOException {
        super.close();
    }
}
