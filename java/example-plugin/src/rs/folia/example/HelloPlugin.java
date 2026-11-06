package rs.folia.example;

import org.bukkit.GameMode;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.Particle;
import org.bukkit.Sound;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.block.BlockPlaceEvent;
import org.bukkit.event.player.PlayerInteractEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.server.PluginEnableEvent;
import org.bukkit.inventory.ItemFlag;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.PlayerInventory;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.plugin.RegisteredServiceProvider;
import org.bukkit.plugin.ServicePriority;
import org.bukkit.plugin.java.JavaPlugin;
import rs.folia.bridge.Bridge;

import java.util.List;
import java.util.UUID;

/**
 * Demo plugin: uses only the normal Bukkit/Paper API for lifecycle, events,
 * world/block access, player state, inventory items, and scheduling.
 *
 * <p>Set {@code FOLIA_BRIDGE_SELFTEST=1} to make the plugin exercise every
 * bridge primitive on enable with a synthetic player. Without that variable the
 * plugin only reads and logs the spawn surface.</p>
 */
public final class HelloPlugin extends JavaPlugin implements Listener {
    private static final String PROBE_NAME = "BridgeProbe";
    private static final UUID PROBE_ID = UUID.fromString("00000000-0000-0000-0000-0000000000aa");

    /** Tiny service exercised through the Bukkit services manager. */
    public interface GreetingService {
        String greet(String name);
    }

    private boolean selfTest;

    @Override
    public void onEnable() {
        selfTest = "1".equals(System.getenv("FOLIA_BRIDGE_SELFTEST"));
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

        readSpawnSurface();
        exerciseConfig();
        exerciseServices();
        exerciseWorldDepth();
        if (selfTest) {
            runSelfTest();
        }

        getServer().getScheduler().runTaskLater(this, () ->
                getLogger().info("scheduler one-shot task from the embedded JVM"), 1L);
        getServer().getScheduler().runTaskTimer(this, () ->
                getLogger().info("scheduler tick from the embedded JVM"), 1L, 20L);
    }

    @EventHandler
    public void onPluginEnable(PluginEnableEvent event) {
        if (event.getPlugin() == this) {
            getLogger().info("event bus delivered PluginEnableEvent");
        }
    }

    @EventHandler
    public void onPlayerJoin(PlayerJoinEvent event) {
        Player player = event.getPlayer();
        getLogger().info("PlayerJoinEvent: " + player.getName()
                + " (" + player.getUniqueId() + ") at " + player.getLocation().getBlockX()
                + "," + player.getLocation().getBlockY() + ","
                + player.getLocation().getBlockZ());
        if (selfTest && PROBE_ID.equals(player.getUniqueId())) {
            exercisePlayer(player);
        }
    }

    @EventHandler
    public void onBlockBreak(BlockBreakEvent event) {
        Material type = event.getBlock().getType();
        getLogger().info("BlockBreakEvent: " + type + " at " + event.getBlock().getLocation());
        if (type == Material.BEDROCK) {
            event.setCancelled(true);
            getLogger().info("cancelled bedrock break through the Rust event bridge");
        }
    }

    @EventHandler
    public void onBlockPlace(BlockPlaceEvent event) {
        Material placed = event.getBlockPlaced().getType();
        Material held = event.getItemInHand() == null
                ? Material.AIR : event.getItemInHand().getType();
        getLogger().info("BlockPlaceEvent: placed=" + placed + " held=" + held);
        if (placed == Material.BEDROCK || held == Material.BEDROCK) {
            event.setCancelled(true);
            getLogger().info("cancelled bedrock place through the Rust event bridge");
        }
    }

    @EventHandler
    public void onPlayerInteract(PlayerInteractEvent event) {
        if (event.getClickedBlock() == null) {
            return;
        }
        Material type = event.getClickedBlock().getType();
        if (type == Material.BEDROCK) {
            event.setCancelled(true);
            getLogger().info("cancelled bedrock interact through the Rust event bridge");
        }
    }

    private void exerciseConfig() {
        saveDefaultConfig();
        String motd = getConfig().getString("motd", "missing");
        int maxHomes = getConfig().getInt("max-homes", -1);
        boolean debug = getConfig().getBoolean("debug", false);
        getConfig().set("runtime-check", "ok");
        getConfig().set("nested.value", 42);
        saveConfig();
        boolean configOk = !"missing".equals(motd);
        getLogger().info("config self-test: motd=" + motd + " max-homes=" + maxHomes
                + " debug=" + debug + " persisted=" + configOk);
        if (!configOk) {
            getLogger().severe("bridge config self-test FAILED: no bundled config value");
        }
    }

    private void exerciseServices() {
        GreetingService service = name -> "hello, " + name;
        getServer().getServicesManager().register(
                GreetingService.class, service, this, ServicePriority.Normal);
        RegisteredServiceProvider<GreetingService> registration =
                getServer().getServicesManager().getRegistration(GreetingService.class);
        GreetingService loaded = getServer().getServicesManager().load(GreetingService.class);
        boolean servicesOk = registration != null
                && registration.getProvider() == service
                && loaded == service
                && getServer().getServicesManager().isProvidedFor(GreetingService.class);
        getServer().getServicesManager().unregister(service);
        getLogger().info("services self-test: registered=" + servicesOk
                + " afterUnregister="
                + getServer().getServicesManager().isProvidedFor(GreetingService.class));
        if (!servicesOk) {
            getLogger().severe("bridge services self-test FAILED");
        }
    }

    private void exerciseWorldDepth() {
        World world = getServer().getWorld("world");
        if (world == null) {
            return;
        }
        long previousFullTime = world.getFullTime();
        long previousTime = world.getTime();
        world.setTime(6_000L);
        boolean timeOk = world.getTime() == 6_000L && world.isDayTime();
        world.setTime(previousTime);
        world.setFullTime(previousFullTime);

        Location spawn = world.getSpawnLocation();
        boolean spawnOk = spawn != null && spawn.getWorld() == world;
        world.setStorm(true);
        world.setThundering(true);
        world.setWeatherDuration(100);
        world.setThunderDuration(80);
        world.setClearWeatherDuration(40);
        boolean weatherOk = world.hasStorm() && world.isThundering()
                && world.getWeatherDuration() == 100
                && world.getThunderDuration() == 80
                && world.getClearWeatherDuration() == 40;
        world.setStorm(false);
        world.setThundering(false);
        world.setWeatherDuration(0);
        world.setThunderDuration(0);
        world.setClearWeatherDuration(0);

        world.playSound(spawn, Sound.BLOCK_NOTE_BLOCK_PLING, 1.0F, 1.0F);
        world.spawnParticle(Particle.END_ROD, spawn, 1);
        getLogger().info("world depth self-test: time=" + timeOk + " weather=" + weatherOk
                + " spawn=" + spawnOk + " effects are explicit no-ops");
        if (!timeOk || !weatherOk || !spawnOk) {
            getLogger().severe("bridge world depth self-test FAILED");
        }
    }

    private void readSpawnSurface() {
        World world = getServer().getWorld("world");
        if (world == null) {
            getLogger().warning("world 'world' was not exposed by the bridge");
            return;
        }
        int y = world.getHighestBlockYAt(0, 0);
        Block block = world.getBlockAt(0, y, 0);
        getLogger().info("spawn surface: " + block.getType() + " at y=" + y
                + ", biome=" + world.getBiome(0, y, 0)
                + ", light=" + block.getLightLevel()
                + " (sky=" + block.getLightFromSky()
                + ", block=" + block.getLightFromBlocks() + ")");
    }

    private void runSelfTest() {
        World world = getServer().getWorld("world");
        if (world == null) {
            getLogger().warning("bridge self-test skipped: no world");
            return;
        }
        int y = world.getHighestBlockYAt(0, 0);
        Block target = world.getBlockAt(0, y + 2, 0);
        Material previous = target.getType();
        target.setType(Material.GLOWSTONE);
        Material after = target.getType();
        target.setType(previous);
        if (after != Material.GLOWSTONE) {
            getLogger().severe("bridge self-test failed: block write did not stick (" + after + ")");
        } else {
            getLogger().info("world block self-test passed: " + previous + " -> GLOWSTONE -> "
                    + target.getType());
        }
        // The Rust host fires the block/interact events after plugin boot; this
        // synthetic join exercises the player and inventory proxies now.
        Bridge.firePlayerJoin(PROBE_NAME, PROBE_ID.toString());
    }

    private void exercisePlayer(Player player) {
        getLogger().info("player self-test start: gamemode=" + player.getGameMode()
                + " health=" + player.getHealth() + " food=" + player.getFoodLevel()
                + " saturation=" + player.getSaturation());

        Location location = player.getLocation();
        boolean teleported = player.teleport(location);
        player.sendMessage("Hello from the Rust player bridge at " + location);

        PlayerInventory inventory = player.getInventory();
        ItemStack previous = inventory.getItem(0);
        ItemStack sword = new ItemStack(Material.DIAMOND_SWORD, 1);
        sword.setDurability((short) 5);
        ItemMeta meta = sword.getItemMeta();
        meta.setDisplayName("Bridge Blade");
        meta.setLore(List.of("created by HelloPlugin", "stored by the Rust component model"));
        meta.setUnbreakable(true);
        meta.addEnchant(Enchantment.SHARPNESS, 3, true);
        meta.addItemFlags(ItemFlag.HIDE_ENCHANTS);
        boolean flagsInMemory = meta.getItemFlags().contains(ItemFlag.HIDE_ENCHANTS);
        sword.setItemMeta(meta);
        inventory.setItem(0, sword);
        ItemStack read = inventory.getItem(0);
        boolean itemOk = read.getType() == Material.DIAMOND_SWORD
                && read.getAmount() == 1
                && read.getDurability() == 5
                && read.getItemMeta() != null
                && "Bridge Blade".equals(read.getItemMeta().getDisplayName())
                && read.getItemMeta().isUnbreakable()
                && read.getEnchantmentLevel(Enchantment.SHARPNESS) == 3
                && flagsInMemory;
        inventory.setItem(0, previous);

        GameMode previousMode = player.getGameMode();
        player.setGameMode(GameMode.CREATIVE);
        GameMode creative = player.getGameMode();
        player.setGameMode(previousMode);

        player.setHealth(18.0D);
        player.setFoodLevel(17);
        player.setSaturation(3.5F);
        boolean vitalsOk = Math.abs(player.getHealth() - 18.0D) < 0.001D
                && player.getFoodLevel() == 17
                && Math.abs(player.getSaturation() - 3.5F) < 0.001F;
        player.setHealth(20.0D);
        player.setFoodLevel(20);
        player.setSaturation(5.0F);

        boolean initiallySet = player.isPermissionSet("bridge.selftest");
        PermissionAttachment attachment =
                player.addAttachment(this, "bridge.selftest", true);
        boolean granted = player.hasPermission("bridge.selftest");
        attachment.setPermission("bridge.selftest", false);
        boolean revoked = !player.hasPermission("bridge.selftest")
                && player.isPermissionSet("bridge.selftest");
        attachment.remove();
        boolean permissionOk = !initiallySet && granted && revoked
                && !player.isPermissionSet("bridge.selftest");

        getLogger().info("player self-test: teleport=" + teleported
                + " item=" + itemOk + " itemType=" + read.getType()
                + " itemDamage=" + read.getDurability()
                + " enchant=" + read.getEnchantmentLevel(Enchantment.SHARPNESS)
                + " creative=" + creative
                + " vitals=" + vitalsOk
                + " permissions=" + permissionOk
                + " op=" + player.isOp());
        if (!teleported || !itemOk || creative != GameMode.CREATIVE || !vitalsOk
                || !permissionOk) {
            getLogger().severe("bridge player/inventory self-test FAILED");
        } else {
            getLogger().info("bridge player/inventory self-test passed");
        }
    }

    @Override
    public void onDisable() {
        getLogger().info("HelloPlugin disabled");
    }
}
