package rs.folia.bridge;

import org.bukkit.Material;
import org.bukkit.block.Block;
import org.bukkit.block.BlockFace;
import org.bukkit.block.BlockState;
import org.bukkit.enchantments.Enchantment;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.Action;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.block.BlockPlaceEvent;
import org.bukkit.event.player.PlayerInteractEvent;
import org.bukkit.inventory.ItemFlag;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.RegisteredServiceProvider;
import org.bukkit.plugin.ServicePriority;
import org.bukkit.scheduler.BukkitTask;

import java.lang.reflect.Proxy;
import java.util.List;
import java.util.UUID;
import java.util.logging.Logger;

/**
 * Standalone API checks that do not need the native Rust library.
 *
 * <p>Run by {@code scripts/build-java.ps1}. Covers the registry shim that makes
 * {@code new ItemStack(...)} work, the bridge item codec, and cancellable event
 * dispatch.</p>
 */
public final class BridgeApiTest {
    private static int failures;

    private BridgeApiTest() {
    }

    public static void main(String[] args) {
        testItemStackShim();
        testEventCancellation();
        testBridgeConfig();
        testServicesManager();
        testSchedulerContract();
        testItemEnchantCodec();
        if (failures == 0) {
            System.out.println("BridgeApiTest: all checks passed");
            return;
        }
        System.err.println("BridgeApiTest: " + failures + " check(s) failed");
        System.exit(1);
    }

    private static void testItemStackShim() {
        ItemStack stack = new ItemStack(Material.DIAMOND_SWORD, 1);
        check(stack.getType() == Material.DIAMOND_SWORD, "new ItemStack works through registry shim");
        check(stack.getAmount() == 1, "item amount survives construction");

        stack.setDurability((short) 7);
        ItemMeta meta = stack.getItemMeta();
        meta.setDisplayName("Bridge Blade");
        meta.setLore(List.of("line one", "第二行"));
        meta.setUnbreakable(true);
        stack.setItemMeta(meta);

        check(stack.getDurability() == 7, "durability set/get");
        check("Bridge Blade".equals(stack.getItemMeta().getDisplayName()), "display name");
        check(stack.getItemMeta().getLore() != null
                && stack.getItemMeta().getLore().size() == 2, "lore");
        check(stack.getItemMeta().isUnbreakable(), "unbreakable");

        String encoded = BridgeItemStack.encode(stack);
        ItemStack decoded = BridgeItemStack.decode(encoded);
        check(decoded.getType() == Material.DIAMOND_SWORD, "codec item type");
        check(decoded.getAmount() == 1, "codec amount");
        check(decoded.getDurability() == 7, "codec durability");
        check("Bridge Blade".equals(decoded.getItemMeta().getDisplayName()), "codec name");
        check(decoded.getItemMeta().isUnbreakable(), "codec unbreakable");
        check(decoded.getItemMeta().getLore().contains("第二行"), "codec unicode lore");

        check(Material.matchMaterial("minecraft:gold_block") == Material.GOLD_BLOCK,
                "namespaced material lookup");
        check(BridgeValues.unhex(BridgeValues.hex("Grüße")).equals("Grüße"), "hex codec");
    }

    private static void testEventCancellation() {
        Logger logger = Logger.getLogger("BridgeApiTest");
        BridgeEventBus bus = new BridgeEventBus(logger);
        CancellingListener listener = new CancellingListener();
        bus.register(listener, plugin());

        Block bedrock = block(Material.BEDROCK);
        Block stone = block(Material.STONE);
        Player player = player();

        BlockBreakEvent breakBedrock = new BlockBreakEvent(bedrock, player);
        bus.call(breakBedrock);
        check(breakBedrock.isCancelled(), "BlockBreakEvent cancellable");

        BlockBreakEvent breakStone = new BlockBreakEvent(stone, player);
        bus.call(breakStone);
        check(!breakStone.isCancelled(), "BlockBreakEvent not cancelled for stone");

        BlockPlaceEvent placeBedrock = new BlockPlaceEvent(
                stone,
                blockState(Material.AIR),
                stone,
                new BridgeItemStack(Material.BEDROCK, 1),
                player,
                true
        );
        bus.call(placeBedrock);
        check(placeBedrock.isCancelled(), "BlockPlaceEvent cancellable");

        PlayerInteractEvent interact = new PlayerInteractEvent(
                player,
                Action.LEFT_CLICK_BLOCK,
                new BridgeItemStack(Material.AIR, 0),
                bedrock,
                BlockFace.UP
        );
        bus.call(interact);
        check(interact.isCancelled(), "PlayerInteractEvent cancellable");
    }

    private static void testBridgeConfig() {
        BridgeConfig config = BridgeConfig.inMemory("BridgeApiTest");
        config.loadFromString("""
                # comment
                motd: "Hello: world"
                max-homes: 3
                debug: false
                nested.value: 9
                """);
        check("Hello: world".equals(config.getString("motd")), "config string scalar");
        check(config.getInt("max-homes") == 3, "config int scalar");
        check(!config.getBoolean("debug"), "config boolean scalar");
        check(config.getInt("nested.value") == 9, "config dotted scalar path");

        config.set("max-homes", 5);
        config.set("new-key", true);
        String serialized = config.saveToString();
        BridgeConfig roundTripped = BridgeConfig.inMemory("BridgeApiTest");
        roundTripped.loadFromString(serialized);
        check(roundTripped.getInt("max-homes") == 5, "config set survives serialization");
        check(roundTripped.getInt("nested.value") == 9, "config nested round-trip");
        check(roundTripped.getBoolean("new-key"), "config boolean round-trip");
        check(roundTripped.getString("motd").equals("Hello: world"),
                "config quoted scalar round-trip");
    }

    private static void testServicesManager() {
        BridgeServicesManager services = new BridgeServicesManager(Logger.getLogger("BridgeApiTest"));
        Plugin owner = plugin();
        GreetingService low = name -> "low:" + name;
        GreetingService high = name -> "high:" + name;

        services.register(GreetingService.class, low, owner, ServicePriority.Low);
        services.register(GreetingService.class, high, owner, ServicePriority.High);
        check(services.load(GreetingService.class) == high, "services load highest priority");
        RegisteredServiceProvider<GreetingService> registration =
                services.getRegistration(GreetingService.class);
        check(registration != null && registration.getPriority() == ServicePriority.High,
                "services registration priority");
        check(services.isProvidedFor(GreetingService.class), "services isProvidedFor");
        check(services.getRegistrations(owner).size() == 2, "services registrations by plugin");
        check(services.getKnownServices().contains(GreetingService.class),
                "services known services");

        services.unregister(high);
        check(services.load(GreetingService.class) == low, "services fallback after unregister");
        services.unregisterAll(owner);
        check(!services.isProvidedFor(GreetingService.class), "services unregisterAll");
    }

    private static void testSchedulerContract() {
        RecordingBackend backend = new RecordingBackend();
        BridgeScheduler scheduler = new BridgeScheduler(
                Logger.getLogger("BridgeApiTest"),
                BridgeApiTest.class.getClassLoader(),
                backend
        );
        Plugin owner = plugin();
        BukkitTask delayed = scheduler.proxy().runTaskLater(owner, () -> {}, 5L);
        check(backend.lastDelay == 5L && backend.lastPeriod == 0L,
                "scheduler delays one-shot task");
        check(delayed.isSync(), "scheduler task is synchronous");
        delayed.cancel();
        check(backend.cancelledId == delayed.getTaskId(), "scheduler cancel forwards task id");

        BukkitTask repeating = scheduler.proxy().runTaskTimer(owner, () -> {}, 2L, 7L);
        check(backend.lastDelay == 2L && backend.lastPeriod == 7L,
                "scheduler timer passes period");
        BukkitTask async = scheduler.proxy().runTaskTimerAsynchronously(owner, () -> {}, 1L, 3L);
        check(async.isSync(), "async variant downgrades to the sync tick queue");
        scheduler.proxy().cancelTasks(owner);
        check(backend.cancelledPluginName.equals(owner.getName()),
                "scheduler cancelTasks forwards plugin");
    }

    private static void testItemEnchantCodec() {
        ItemStack stack = new ItemStack(Material.DIAMOND_SWORD, 1);
        stack.addUnsafeEnchantment(Enchantment.SHARPNESS, 4);
        stack.addUnsafeEnchantment(Enchantment.UNBREAKING, 2);
        ItemMeta meta = stack.getItemMeta();
        meta.addItemFlags(ItemFlag.HIDE_ENCHANTS);
        stack.setItemMeta(meta);
        check(stack.getItemMeta().getItemFlags().contains(ItemFlag.HIDE_ENCHANTS),
                "item flags live on the meta proxy");

        String encoded = BridgeItemStack.encode(stack);
        check(encoded.startsWith("2\t"), "item codec uses the v2 component payload");
        ItemStack decoded = BridgeItemStack.decode(encoded);
        check(decoded.getEnchantmentLevel(Enchantment.SHARPNESS) == 4,
                "enchantment round-trips through Rust");
        check(decoded.getEnchantmentLevel(Enchantment.UNBREAKING) == 2,
                "second enchantment round-trips");
        ItemStack legacy = BridgeItemStack.decode("minecraft:stone\t2\t0\t\t0");
        check(legacy.getType() == Material.STONE && legacy.getAmount() == 2,
                "legacy v1 item payload still decodes");
    }

    private interface GreetingService {
        String greet(String name);
    }

    private static final class RecordingBackend implements BridgeScheduler.Backend {
        private long lastDelay;
        private long lastPeriod;
        private int nextId = 1;
        private int cancelledId = -1;
        private String cancelledPluginName = "";

        @Override
        public int schedule(String pluginName, Runnable task, long delay, long period) {
            lastDelay = delay;
            lastPeriod = period;
            return nextId++;
        }

        @Override
        public void cancel(int taskId) {
            cancelledId = taskId;
        }

        @Override
        public void cancelTasks(String pluginName) {
            cancelledPluginName = pluginName;
        }
    }

    private static final class CancellingListener implements Listener {
        @EventHandler
        public void onBreak(BlockBreakEvent event) {
            if (event.getBlock().getType() == Material.BEDROCK) {
                event.setCancelled(true);
            }
        }

        @EventHandler
        public void onPlace(BlockPlaceEvent event) {
            if (event.getItemInHand() != null
                    && event.getItemInHand().getType() == Material.BEDROCK) {
                event.setCancelled(true);
            }
        }

        @EventHandler
        public void onInteract(PlayerInteractEvent event) {
            if (event.getClickedBlock() != null
                    && event.getClickedBlock().getType() == Material.BEDROCK) {
                event.setCancelled(true);
            }
        }
    }

    private static Plugin plugin() {
        return (Plugin) Proxy.newProxyInstance(
                BridgeApiTest.class.getClassLoader(),
                new Class<?>[]{Plugin.class},
                (proxy, method, args) -> switch (method.getName()) {
                    case "getName" -> "BridgeApiTestPlugin";
                    case "toString" -> "BridgeApiTestPlugin";
                    case "hashCode" -> System.identityHashCode(proxy);
                    case "equals" -> proxy == args[0];
                    default -> BridgeValues.defaultValue(method);
                }
        );
    }

    private static Player player() {
        return (Player) Proxy.newProxyInstance(
                BridgeApiTest.class.getClassLoader(),
                new Class<?>[]{Player.class},
                (proxy, method, args) -> switch (method.getName()) {
                    case "getName", "getDisplayName", "getPlayerListName" -> "BridgeTester";
                    case "getUniqueId" -> UUID.fromString(
                            "00000000-0000-0000-0000-0000000000bb");
                    case "toString" -> "BridgeTester";
                    case "hashCode" -> 0xbb;
                    case "equals" -> proxy == args[0];
                    default -> BridgeValues.defaultValue(method);
                }
        );
    }

    private static Block block(Material material) {
        return (Block) Proxy.newProxyInstance(
                BridgeApiTest.class.getClassLoader(),
                new Class<?>[]{Block.class},
                (proxy, method, args) -> switch (method.getName()) {
                    case "getType" -> material;
                    case "getX", "getY", "getZ" -> 0;
                    case "toString" -> "TestBlock{" + material + "}";
                    case "hashCode" -> material.hashCode();
                    case "equals" -> proxy == args[0];
                    default -> BridgeValues.defaultValue(method);
                }
        );
    }

    private static BlockState blockState(Material material) {
        return (BlockState) Proxy.newProxyInstance(
                BridgeApiTest.class.getClassLoader(),
                new Class<?>[]{BlockState.class},
                (proxy, method, args) -> switch (method.getName()) {
                    case "getType" -> material;
                    case "toString" -> "TestBlockState{" + material + "}";
                    case "hashCode" -> material.hashCode();
                    case "equals" -> proxy == args[0];
                    default -> BridgeValues.defaultValue(method);
                }
        );
    }

    private static void check(boolean condition, String label) {
        if (condition) {
            System.out.println("  ok   " + label);
        } else {
            failures++;
            System.out.println("  FAIL " + label);
        }
    }
}
