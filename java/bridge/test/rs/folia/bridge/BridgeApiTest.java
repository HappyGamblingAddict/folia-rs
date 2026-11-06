package rs.folia.bridge;

import org.bukkit.Material;
import org.bukkit.block.Block;
import org.bukkit.block.BlockFace;
import org.bukkit.block.BlockState;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.Action;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.block.BlockPlaceEvent;
import org.bukkit.event.player.PlayerInteractEvent;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.meta.ItemMeta;
import org.bukkit.plugin.Plugin;

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
