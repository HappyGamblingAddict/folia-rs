package rs.folia.bridge;

import org.bukkit.configuration.ConfigurationSection;
import org.bukkit.configuration.InvalidConfigurationException;
import org.bukkit.configuration.file.FileConfiguration;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.File;
import java.io.IOException;
import java.lang.reflect.Field;
import java.util.Collection;
import java.util.Map;

/**
 * Flat YAML configuration backed by {@code run/<plugin>/config.yml}.
 *
 * <p>The Rust host performs the file I/O ({@link Bridge#nativeConfigRead} and
 * {@link Bridge#nativeConfigWrite}); this class only owns the scalar
 * parse/serialize subset. Dotted keys are stored as nested
 * {@link org.bukkit.configuration.MemorySection}s by Bukkit's own
 * {@code MemorySection}, so {@code getString("messages.welcome")} works.
 * Comments, list values, and map values are not preserved. The file format is
 * one {@code key: scalar} pair per line; nested sections are written as dotted
 * keys.</p>
 */
final class BridgeConfig extends FileConfiguration {
    private final String pluginName;
    private final File configFile;
    private final boolean nativeBacked;
    private boolean loaded;
    private boolean loadedWithFile;

    BridgeConfig(String pluginName, File configFile, boolean nativeBacked) {
        this.pluginName = pluginName;
        this.configFile = configFile == null ? null : configFile.getAbsoluteFile();
        this.nativeBacked = nativeBacked;
        this.loaded = !nativeBacked;
        this.loadedWithFile = !nativeBacked;
    }

    /**
     * Replaces {@code JavaPlugin#newConfig} so ordinary {@code plugin.getConfig()}
     * calls use the Rust-backed file instead of a second YamlConfiguration copy.
     */
    static void install(Plugin plugin) {
        if (!(plugin instanceof JavaPlugin javaPlugin)) {
            return;
        }
        try {
            Field field = JavaPlugin.class.getDeclaredField("newConfig");
            field.setAccessible(true);
            field.set(javaPlugin, new BridgeConfig(
                    plugin.getName(),
                    new File(plugin.getDataFolder(), "config.yml"),
                    true
            ));
        } catch (ReflectiveOperationException exception) {
            Bridge.logger().warning("Could not install the bridge config for "
                    + plugin.getName() + ": " + exception);
        }
    }

    void reload() {
        if (!nativeBacked) {
            return;
        }
        loaded = true;
        loadedWithFile = configFile != null && configFile.isFile();
        try {
            String contents = Bridge.nativeConfigRead(pluginName);
            loadFromString(contents == null ? "" : contents);
        } catch (Throwable throwable) {
            Bridge.logger().warning("Could not read config for " + pluginName + ": " + throwable);
        }
    }

    private void ensureLoaded() {
        if (!nativeBacked || loadedWithFile) {
            return;
        }
        if (!loaded || (configFile != null && configFile.isFile())) {
            reload();
        }
    }

    @Override
    public Object get(String path) {
        ensureLoaded();
        return super.get(path);
    }

    @Override
    public Object get(String path, Object def) {
        ensureLoaded();
        return super.get(path, def);
    }

    @Override
    public void set(String path, Object value) {
        ensureLoaded();
        super.set(path, value);
    }

    @Override
    public boolean contains(String path) {
        ensureLoaded();
        return super.contains(path);
    }

    @Override
    public boolean isSet(String path) {
        ensureLoaded();
        return super.isSet(path);
    }

    @Override
    public java.util.Set<String> getKeys(boolean deep) {
        ensureLoaded();
        return super.getKeys(deep);
    }

    @Override
    public java.util.Map<String, Object> getValues(boolean deep) {
        ensureLoaded();
        return super.getValues(deep);
    }

    @Override
    public String saveToString() {
        ensureLoaded();
        StringBuilder output = new StringBuilder();
        for (String key : getKeys(true)) {
            Object value = get(key);
            if (value == null || value instanceof ConfigurationSection
                    || value instanceof Map<?, ?> || value instanceof Collection<?>) {
                // Nested sections, maps, and lists are outside the flat scalar
                // subset; the key is skipped rather than silently serialized as
                // an object's toString().
                continue;
            }
            output.append(key).append(": ").append(formatScalar(value)).append('\n');
        }
        return output.toString();
    }

    @Override
    public void loadFromString(String contents) {
        loaded = true;
        for (String key : getKeys(false)) {
            set(key, null);
        }
        if (contents == null || contents.isEmpty()) {
            return;
        }
        for (String line : contents.split("\\R")) {
            String trimmed = line.trim();
            if (trimmed.isEmpty() || trimmed.startsWith("#")) {
                continue;
            }
            int separator = trimmed.indexOf(':');
            if (separator <= 0) {
                continue;
            }
            String key = trimmed.substring(0, separator).trim();
            String value = trimmed.substring(separator + 1).trim();
            if (key.isEmpty()) {
                continue;
            }
            set(key, parseScalar(value));
        }
    }

    @Override
    public void save(File file) throws IOException {
        if (isNativeFile(file)) {
            if (!Bridge.nativeConfigWrite(pluginName, saveToString())) {
                throw new IOException("Rust host rejected the config write for " + pluginName);
            }
            return;
        }
        super.save(file);
    }

    @Override
    public void load(File file) throws IOException {
        if (isNativeFile(file)) {
            reload();
            return;
        }
        try {
            super.load(file);
        } catch (InvalidConfigurationException exception) {
            throw new IOException(exception);
        }
    }

    @Override
    public void load(String file) throws IOException {
        if (isNativeFile(new File(file))) {
            reload();
            return;
        }
        try {
            super.load(file);
        } catch (InvalidConfigurationException exception) {
            throw new IOException(exception);
        }
    }

    private boolean isNativeFile(File file) {
        return nativeBacked && file != null && configFile != null
                && file.getAbsoluteFile().equals(configFile);
    }

    static Object parseScalar(String value) {
        if (value.length() >= 2 && value.startsWith("\"") && value.endsWith("\"")) {
            return unquote(value.substring(1, value.length() - 1));
        }
        if ("true".equalsIgnoreCase(value)) {
            return Boolean.TRUE;
        }
        if ("false".equalsIgnoreCase(value)) {
            return Boolean.FALSE;
        }
        try {
            long parsed = Long.parseLong(value);
            if (parsed >= Integer.MIN_VALUE && parsed <= Integer.MAX_VALUE) {
                return (int) parsed;
            }
            return parsed;
        } catch (NumberFormatException ignored) {
            // Not an integer; try a floating-point scalar next.
        }
        if (value.indexOf('.') >= 0 || value.indexOf('e') >= 0 || value.indexOf('E') >= 0) {
            try {
                return Double.parseDouble(value);
            } catch (NumberFormatException ignored) {
                // Plain string.
            }
        }
        return value;
    }

    static String formatScalar(Object value) {
        if (value instanceof String text) {
            return '"' + escape(text) + '"';
        }
        if (value instanceof Boolean || value instanceof Number) {
            return value.toString();
        }
        return '"' + escape(String.valueOf(value)) + '"';
    }

    private static String escape(String value) {
        StringBuilder escaped = new StringBuilder(value.length());
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            switch (character) {
                case '\\' -> escaped.append("\\\\");
                case '"' -> escaped.append("\\\"");
                case '\n' -> escaped.append("\\n");
                case '\r' -> escaped.append("\\r");
                case '\t' -> escaped.append("\\t");
                default -> escaped.append(character);
            }
        }
        return escaped.toString();
    }

    private static String unquote(String value) {
        StringBuilder output = new StringBuilder(value.length());
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            if (character != '\\' || index + 1 >= value.length()) {
                output.append(character);
                continue;
            }
            char escaped = value.charAt(++index);
            switch (escaped) {
                case 'n' -> output.append('\n');
                case 'r' -> output.append('\r');
                case 't' -> output.append('\t');
                default -> output.append(escaped);
            }
        }
        return output.toString();
    }

    /** Test hook: parse raw bridge text without touching the native host. */
    static BridgeConfig inMemory(String pluginName) {
        return new BridgeConfig(pluginName, null, false);
    }
}
