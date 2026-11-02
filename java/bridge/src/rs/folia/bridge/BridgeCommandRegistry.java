package rs.folia.bridge;

import org.bukkit.command.Command;
import org.bukkit.command.CommandException;
import org.bukkit.command.CommandSender;
import org.bukkit.command.PluginCommand;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.PluginDescriptionFile;

import java.lang.reflect.Constructor;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Mutable command registry populated from each plugin's {@code plugin.yml}.
 *
 * <p>{@link PluginCommand}'s constructor is package-private, so the bridge
 * creates it reflectively. Everything else (metadata, executor, tab
 * completion) is the normal Bukkit API.</p>
 */
final class BridgeCommandRegistry {
    private final Logger log = Bridge.logger();
    private final Map<String, PluginCommand> commands = new LinkedHashMap<>();

    void register(Plugin plugin, PluginDescriptionFile description) {
        Map<String, Map<String, Object>> declared = description.getCommands();
        for (Map.Entry<String, Map<String, Object>> entry : declared.entrySet()) {
            String name = entry.getKey();
            try {
                PluginCommand command = create(name, plugin);
                applyMetadata(command, entry.getValue());

                commands.put(name.toLowerCase(Locale.ROOT), command);
                commands.put(
                        (plugin.getName() + ":" + name).toLowerCase(Locale.ROOT),
                        command
                );
                for (String alias : command.getAliases()) {
                    commands.putIfAbsent(alias.toLowerCase(Locale.ROOT), command);
                    commands.putIfAbsent(
                            (plugin.getName() + ":" + alias).toLowerCase(Locale.ROOT),
                            command
                    );
                }
            } catch (ReflectiveOperationException exception) {
                log.log(Level.WARNING, "Could not register command " + name
                        + " for " + plugin.getName(), exception);
            }
        }
    }

    PluginCommand get(String name) {
        if (name == null) {
            return null;
        }
        return commands.get(name.toLowerCase(Locale.ROOT));
    }

    Map<String, Command> knownCommands() {
        return new LinkedHashMap<>(commands);
    }

    boolean dispatch(String line, CommandSender sender) {
        String trimmed = line.startsWith("/") ? line.substring(1) : line;
        String[] parts = trimmed.trim().split("\\s+");
        if (parts.length == 0 || parts[0].isEmpty()) {
            return false;
        }

        PluginCommand command = get(parts[0]);
        if (command == null) {
            return false;
        }

        String[] args = Arrays.copyOfRange(parts, 1, parts.length);
        try {
            return command.execute(sender, parts[0], args);
        } catch (CommandException exception) {
            log.log(Level.WARNING, "Command /" + parts[0] + " failed", exception);
            return false;
        }
    }

    void remove(Plugin plugin) {
        commands.values().removeIf(command -> command.getPlugin() == plugin);
    }

    void clear() {
        commands.clear();
    }

    private PluginCommand create(String name, Plugin plugin) throws ReflectiveOperationException {
        Constructor<PluginCommand> constructor =
                PluginCommand.class.getDeclaredConstructor(String.class, Plugin.class);
        constructor.setAccessible(true);
        return constructor.newInstance(name, plugin);
    }

    private void applyMetadata(PluginCommand command, Map<String, Object> metadata) {
        if (metadata == null) {
            return;
        }
        if (metadata.get("description") instanceof String value) {
            command.setDescription(value);
        }
        if (metadata.get("usage") instanceof String value) {
            command.setUsage(value);
        }
        if (metadata.get("permission") instanceof String value) {
            command.setPermission(value);
        }

        List<String> aliases = new ArrayList<>();
        Object rawAliases = metadata.get("aliases");
        if (rawAliases instanceof String value) {
            aliases.add(value);
        } else if (rawAliases instanceof List<?> values) {
            for (Object value : values) {
                if (value instanceof String alias) {
                    aliases.add(alias);
                }
            }
        }
        if (!aliases.isEmpty()) {
            command.setAliases(aliases);
        }
    }
}
