package rs.folia.example;

import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.server.PluginEnableEvent;
import org.bukkit.plugin.java.JavaPlugin;
import rs.folia.bridge.Bridge;

/**
 * Demo plugin: uses only the normal Bukkit/Paper API for lifecycle, events and
 * scheduling. The single call into {@link Bridge#nativePing(String)} is only to
 * demonstrate the JNI round trip from the Rust host.
 */
public final class HelloPlugin extends JavaPlugin implements Listener {
    @Override
    public void onEnable() {
        getServer().getPluginManager().registerEvents(this, this);
        getLogger().info("HelloPlugin enabled on " + getServer().getName()
                + " " + getServer().getMinecraftVersion());
        getLogger().info(Bridge.nativePing("hello from a real .jar plugin"));

        if (getCommand("hello") != null) {
            getCommand("hello").setExecutor((sender, command, label, args) -> {
                String name = args.length > 0 ? args[0] : "world";
                sender.sendMessage("Hello, " + name + "! (Paper command executed by Rust)");
                return true;
            });
        } else {
            getLogger().warning("Command /hello was not registered");
        }

        getServer().getScheduler().runTaskTimer(this, () ->
                getLogger().info("scheduler tick from the embedded JVM"), 1L, 5L);
    }

    @EventHandler
    public void onPluginEnable(PluginEnableEvent event) {
        if (event.getPlugin() == this) {
            getLogger().info("event bus delivered PluginEnableEvent");
        }
    }

    @EventHandler
    public void onPlayerJoin(PlayerJoinEvent event) {
        getLogger().info("PlayerJoinEvent: " + event.getPlayer().getName()
                + " (" + event.getPlayer().getUniqueId() + ")");
    }

    @Override
    public void onDisable() {
        getLogger().info("HelloPlugin disabled");
    }
}
