package rs.folia.bridge;

import io.papermc.paper.plugin.lifecycle.event.LifecycleEventManager;
import org.bukkit.Bukkit;
import org.bukkit.Server;
import org.bukkit.UnsafeValues;
import org.bukkit.command.ConsoleCommandSender;
import org.bukkit.entity.Player;
import org.bukkit.event.Event;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.server.PluginDisableEvent;
import org.bukkit.event.server.PluginEnableEvent;
import org.bukkit.permissions.Permission;
import org.bukkit.plugin.EventExecutor;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.PluginManager;
import org.bukkit.plugin.java.JavaPlugin;
import org.bukkit.scheduler.BukkitScheduler;

import java.io.File;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.logging.Logger;

/**
 * The Rust-backed {@link Server} implementation exposed to plugins.
 *
 * <p>Most of Bukkit's surface is deliberately stubbed for now. The methods that
 * matter for plugin bootstrap, logging, scheduling and events are wired to the
 * Rust host through {@link BridgeScheduler} and {@link BridgeEventBus}.</p>
 */
final class BridgeServer {
    private final File pluginsDirectory;
    private final File dataDirectory;
    private final Logger log;
    private final BridgeScheduler scheduler;
    private final BridgeEventBus events;
    private final BridgeCommandRegistry commands;
    private final org.bukkit.command.CommandMap commandMap;
    private final Map<String, Plugin> plugins = new LinkedHashMap<>();
    private final PluginManager pluginManager;
    private final UnsafeValues unsafe;
    private final ConsoleCommandSender console;
    private final Server server;

    BridgeServer(File pluginsDirectory, File dataDirectory) {
        this.pluginsDirectory = pluginsDirectory;
        this.dataDirectory = dataDirectory;
        this.log = Bridge.logger();
        this.scheduler = new BridgeScheduler(log, BridgeServer.class.getClassLoader());
        this.events = new BridgeEventBus(log);
        this.commands = new BridgeCommandRegistry();
        this.commandMap = (org.bukkit.command.CommandMap) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{org.bukkit.command.CommandMap.class},
                this::handleCommandMap
        );
        this.pluginManager = (PluginManager) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{PluginManager.class},
                this::handlePluginManager
        );
        this.unsafe = (UnsafeValues) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{UnsafeValues.class},
                this::handleUnsafe
        );
        this.console = (ConsoleCommandSender) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{ConsoleCommandSender.class},
                this::handleConsole
        );
        this.server = (Server) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{Server.class},
                this::handleServer
        );
        install();
    }

    Server proxy() {
        return server;
    }

    synchronized void registerPlugin(Plugin plugin) {
        plugins.put(plugin.getName().toLowerCase(), plugin);
        plugins.put(plugin.getName(), plugin);
    }

    synchronized void unregisterPlugin(Plugin plugin) {
        plugins.values().removeIf(candidate -> candidate == plugin);
        commands.remove(plugin);
    }

    void registerCommands(Plugin plugin, org.bukkit.plugin.PluginDescriptionFile description) {
        commands.register(plugin, description);
    }

    boolean dispatchCommand(String line) {
        return commands.dispatch(line, console);
    }

    Map<String, org.bukkit.command.Command> knownCommands() {
        return commands.knownCommands();
    }

    void firePlayerJoin(String name, String uuid) {
        UUID id;
        try {
            id = UUID.fromString(uuid);
        } catch (IllegalArgumentException exception) {
            id = UUID.nameUUIDFromBytes(uuid.getBytes());
        }
        final UUID playerId = id;

        Player player = (Player) Proxy.newProxyInstance(
                BridgeServer.class.getClassLoader(),
                new Class<?>[]{Player.class},
                (proxy, method, args) -> handlePlayer(proxy, method, args, name, playerId)
        );
        events.call(new PlayerJoinEvent(
                player,
                net.kyori.adventure.text.Component.text(name + " joined the game")
        ));
        log.info("PlayerJoinEvent fired for " + name);
    }

    private Object handlePlayer(Object proxyObject, Method method, Object[] args, String name, UUID uuid) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        return switch (method.getName()) {
            case "getName", "getDisplayName", "getPlayerListName", "getPlayerListHeader",
                 "getPlayerListFooter" -> name;
            case "getUniqueId" -> uuid;
            case "isOnline" -> true;
            case "getServer" -> server;
            case "sendMessage", "sendRawMessage" -> {
                logConsoleMessage(args);
                yield null;
            }
            case "hasPermission", "isPermissionSet", "isOp" -> true;
            case "getPing" -> 0;
            case "getLocale" -> "en_us";
            case "getClientBrandName" -> "Folia-RS";
            case "getEffectivePermissions" -> Set.of();
            case "getHealth", "getMaxHealth" -> 20.0D;
            case "getFoodLevel" -> 20;
            case "getSaturation" -> 5.0F;
            case "getRemainingAir", "getMaximumAir" -> 300;
            case "getFallDistance" -> 0.0F;
            case "isSneaking", "isSprinting", "isFlying", "getAllowFlight",
                 "isConversing", "isSleeping" -> false;
            default -> BridgeValues.defaultValue(method);
        };
    }

    void callPluginEnable(Plugin plugin) {
        events.call(new PluginEnableEvent(plugin));
    }

    void callPluginDisable(Plugin plugin) {
        events.call(new PluginDisableEvent(plugin));
    }

    void tick() {
        scheduler.tick();
    }

    private void install() {
        try {
            Field field = Bukkit.class.getDeclaredField("server");
            field.setAccessible(true);
            field.set(null, server);
        } catch (ReflectiveOperationException exception) {
            throw new IllegalStateException("Could not install the Bukkit server singleton", exception);
        }
    }

    private Object handleServer(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        String name = method.getName();
        return switch (name) {
            case "getPluginsFolder" -> pluginsDirectory;
            case "getUpdateFolder" -> "update";
            case "getUpdateFolderFile" -> new File(pluginsDirectory, "update");
            case "getName" -> "Folia-RS";
            case "getVersion" -> "1.21.11-RS";
            case "getBukkitVersion" -> "1.21.11-R0.1-SNAPSHOT";
            case "getMinecraftVersion" -> "1.21.11";
            case "getLogger" -> log;
            case "getPluginManager" -> pluginManager;
            case "getScheduler" -> scheduler.proxy();
            case "getUnsafe" -> unsafe;
            case "getConsoleSender" -> console;
            case "getOnlinePlayers" -> List.of();
            case "getWorlds" -> List.of();
            case "getMaxPlayers" -> 20;
            case "getPort" -> 25565;
            case "getIp" -> "0.0.0.0";
            case "getViewDistance", "getSimulationDistance" -> 10;
            case "getSpawnRadius" -> 0;
            case "getConnectionThrottle" -> 0L;
            case "isPrimaryThread" -> true;
            case "getWorldContainer" -> dataDirectory;
            case "getTPS" -> new double[]{20.0D, 20.0D, 20.0D};
            case "getTickTimes" -> new long[]{0L, 0L, 0L, 0L, 0L};
            case "getAverageTickTime" -> 0.0D;
            case "getServerIcon" -> null;
            case "getServerConfig" -> null;
            case "getServerResourcePack" -> null;
            case "getServerTickManager" -> null;
            case "getServerLinks" -> null;
            case "getMessenger" -> null;
            case "getServicesManager" -> null;
            case "getItemFactory" -> null;
            case "getCommandMap" -> commandMap;
            case "getPluginCommand" -> commands.get((String) args[0]);
            case "getPlayer", "getPlayerExact", "getOfflinePlayer", "getOfflinePlayerIfCached",
                 "getEntity", "getWorld", "getMap", "getBossBar", "getAdvancement",
                 "getLootTable", "getStructureManager", "getRespawnWorld" -> null;
            case "matchPlayer" -> List.of();
            case "getOfflinePlayers" -> new org.bukkit.OfflinePlayer[0];
            case "getBannedPlayers", "getOperators", "getWhitelistedPlayers" -> Set.of();
            case "getIPBans" -> Set.of();
            case "getCommandAliases" -> Map.of();
            case "getRecipesFor" -> List.of();
            case "getBossBars" -> java.util.Collections.emptyIterator();
            case "advancementIterator" -> java.util.Collections.emptyIterator();
            case "recipeIterator" -> java.util.Collections.emptyIterator();
            default -> BridgeValues.defaultValue(method);
        };
    }

    private Object handleCommandMap(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        return switch (method.getName()) {
            case "getCommand" -> commands.get((String) args[0]);
            case "dispatch" -> commands.dispatch(
                    (String) args[1],
                    (org.bukkit.command.CommandSender) args[0]
            );
            case "getKnownCommands" -> commands.knownCommands();
            case "clearCommands" -> {
                commands.clear();
                yield null;
            }
            case "tabComplete" -> List.of();
            case "registerAll" -> null;
            case "register" -> false;
            default -> BridgeValues.defaultValue(method);
        };
    }

    private Object handlePluginManager(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        String name = method.getName();
        return switch (name) {
            case "getPlugin" -> plugin(args, 0);
            case "getPlugins" -> plugins.values().stream().distinct().toArray(Plugin[]::new);
            case "isPluginEnabled" -> {
                Plugin plugin = plugin(args, 0);
                yield plugin != null && plugin.isEnabled();
            }
            case "registerEvents" -> {
                events.register((Listener) args[0], (Plugin) args[1]);
                yield null;
            }
            case "registerEvent" -> {
                @SuppressWarnings("unchecked")
                Class<? extends Event> eventType = (Class<? extends Event>) args[0];
                events.register(eventType, (Listener) args[1],
                        (org.bukkit.event.EventPriority) args[2],
                        (EventExecutor) args[3],
                        (Plugin) args[4],
                        args.length > 5 && Boolean.TRUE.equals(args[5]));
                yield null;
            }
            case "callEvent" -> {
                events.call((Event) args[0]);
                yield null;
            }
            case "enablePlugin" -> {
                if (args[0] instanceof JavaPlugin plugin) {
                    plugin.setEnabled(true);
                }
                yield null;
            }
            case "disablePlugin" -> {
                if (args[0] instanceof JavaPlugin plugin) {
                    plugin.setEnabled(false);
                }
                yield null;
            }
            case "getPermission" -> null;
            case "getPermissions", "getDefaultPermissions", "getPermissionSubscriptions",
                 "getDefaultPermSubscriptions" -> Set.of();
            case "useTimings" -> false;
            case "loadPlugins" -> new Plugin[0];
            case "loadPlugin" -> null;
            case "disablePlugins", "clearPlugins", "recalculatePermissionDefaults",
                 "subscribeToPermission", "unsubscribeFromPermission", "subscribeToDefaultPerms",
                 "unsubscribeFromDefaultPerms", "removePermission", "addPermission",
                 "overridePermissionManager", "registerInterface" -> null;
            default -> BridgeValues.defaultValue(method);
        };
    }

    private Object handleUnsafe(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        if (method.getName().equals("createPluginLifecycleEventManager")) {
            return Proxy.newProxyInstance(
                    BridgeServer.class.getClassLoader(),
                    new Class<?>[]{LifecycleEventManager.class},
                    (managerProxy, managerMethod, managerArgs) -> {
                        if (managerMethod.getDeclaringClass() == Object.class) {
                            return objectMethod(managerProxy, managerMethod, managerArgs);
                        }
                        return BridgeValues.defaultValue(managerMethod);
                    }
            );
        }
        return BridgeValues.defaultValue(method);
    }

    private Object handleConsole(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        return switch (method.getName()) {
            case "sendMessage" -> {
                logConsoleMessage(args);
                yield null;
            }
            case "getName" -> "CONSOLE";
            case "getServer" -> server;
            case "isOp" -> true;
            case "hasPermission", "isPermissionSet" -> true;
            case "getEffectivePermissions", "getPermissions" -> Set.of();
            case "getUniqueId" -> UUID.nameUUIDFromBytes("CONSOLE".getBytes());
            default -> BridgeValues.defaultValue(method);
        };
    }

    private void logConsoleMessage(Object[] args) {
        if (args == null || args.length == 0 || args[0] == null) {
            return;
        }
        Object first = args[0];
        if (first instanceof String[] lines) {
            for (String line : lines) {
                log.info(line);
            }
        } else if (first instanceof String line) {
            log.info(line);
        } else {
            log.info(String.valueOf(first));
        }
    }

    private Plugin plugin(Object[] args, int index) {
        if (args == null || args.length <= index || args[index] == null) {
            return null;
        }
        Object value = args[index];
        if (value instanceof Plugin plugin) {
            return plugin;
        }
        if (value instanceof String name) {
            Plugin plugin = plugins.get(name);
            if (plugin != null) {
                return plugin;
            }
            return plugins.get(name.toLowerCase());
        }
        return null;
    }

    private static Object objectMethod(Object proxyObject, Method method, Object[] args) {
        return switch (method.getName()) {
            case "toString" -> "BridgeServer";
            case "hashCode" -> System.identityHashCode(proxyObject);
            case "equals" -> proxyObject == args[0];
            default -> null;
        };
    }
}
