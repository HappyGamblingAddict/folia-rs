package rs.folia.bridge;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer;
import org.bukkit.GameMode;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.Server;
import org.bukkit.entity.Entity;
import org.bukkit.entity.EntityType;
import org.bukkit.entity.Player;
import org.bukkit.inventory.InventoryView;
import org.bukkit.inventory.ItemStack;
import org.bukkit.inventory.PlayerInventory;
import org.bukkit.permissions.Permission;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.plugin.Plugin;
import org.bukkit.scoreboard.Scoreboard;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.List;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;

/**
 * Rust-backed {@link Player} proxy.
 *
 * <p>Identity, location, health, food, game mode, and inventory are stored by
 * the Rust bridge in the playerdata record. Game mode, messages, kick,
 * teleport, sounds, and particles are also forwarded to the live network
 * session when one is connected.</p>
 */
final class BridgePlayer implements InvocationHandler {
    private static final PlainTextComponentSerializer PLAIN = PlainTextComponentSerializer.plainText();

    private final String name;
    private final UUID uniqueId;
    private final String uuidString;
    private final Server server;
    private final BridgeWorld world;
    private final Player proxy;
    private final BridgeInventory inventory;
    private final BridgePermissions permissions;

    BridgePlayer(String name, UUID uniqueId, Server server, BridgeWorld world) {
        this.name = name == null || name.isEmpty() ? "Player" : name;
        this.uniqueId = uniqueId;
        this.uuidString = uniqueId.toString();
        this.server = server;
        this.world = world;
        this.proxy = (Player) Proxy.newProxyInstance(
                BridgePlayer.class.getClassLoader(),
                new Class<?>[]{Player.class},
                this
        );
        Bridge.nativePlayerIdentify(this.name, uuidString);
        this.inventory = new BridgeInventory(uuidString, proxy, this::location);
        this.permissions = new BridgePermissions(proxy, uuidString);
    }

    Player proxy() {
        return proxy;
    }

    String name() {
        return name;
    }

    UUID uniqueId() {
        return uniqueId;
    }

    private Location location() {
        String encoded = Bridge.nativePlayerLocation(uuidString);
        if (encoded == null || encoded.isEmpty()) {
            return world.spawnLocation();
        }
        String[] fields = encoded.split("\t", -1);
        if (fields.length < 5) {
            return world.spawnLocation();
        }
        try {
            return new Location(
                    world.proxy(),
                    Double.parseDouble(fields[0]),
                    Double.parseDouble(fields[1]),
                    Double.parseDouble(fields[2]),
                    Float.parseFloat(fields[3]),
                    Float.parseFloat(fields[4])
            );
        } catch (NumberFormatException exception) {
            return world.spawnLocation();
        }
    }

    @Override
    public Object invoke(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return switch (method.getName()) {
                case "toString" -> "BridgePlayer{" + name + "/" + uuidString + "}";
                case "hashCode" -> uniqueId.hashCode();
                case "equals" -> proxyObject == args[0];
                default -> null;
            };
        }
        return switch (method.getName()) {
            case "getName", "getDisplayName", "getPlayerListName", "getPlayerListHeader",
                 "getPlayerListFooter" -> name;
            case "setDisplayName", "setPlayerListName", "setPlayerListHeader",
                 "setPlayerListFooter" -> null;
            case "getUniqueId" -> uniqueId;
            case "isOnline" -> true;
            case "getServer" -> server;
            case "getWorld" -> world.proxy();
            case "getLocation" -> {
                Location location = location();
                if (args != null && args.length == 1 && args[0] instanceof Location target) {
                    target.setWorld(location.getWorld());
                    target.setX(location.getX());
                    target.setY(location.getY());
                    target.setZ(location.getZ());
                    target.setYaw(location.getYaw());
                    target.setPitch(location.getPitch());
                    yield target;
                }
                yield location;
            }
            case "getEyeLocation" -> location().add(0.0D, 1.62D, 0.0D);
            case "getInventory" -> inventory.proxy();
            case "getPlayer" -> proxy;
            case "getGameMode" -> gameMode(Bridge.nativePlayerGameMode(uuidString));
            case "setGameMode" -> {
                if (args != null && args.length == 1 && args[0] instanceof GameMode mode) {
                    Bridge.nativePlayerSetGameMode(uuidString, mode.getValue());
                }
                yield null;
            }
            case "getHealth" -> Bridge.nativePlayerHealth(uuidString);
            case "setHealth" -> {
                Bridge.nativePlayerSetHealth(uuidString, ((Number) args[0]).doubleValue());
                yield null;
            }
            case "getMaxHealth" -> 20.0D;
            case "getFoodLevel" -> Bridge.nativePlayerFood(uuidString);
            case "setFoodLevel" -> {
                Bridge.nativePlayerSetFood(uuidString, (Integer) args[0]);
                yield null;
            }
            case "getSaturation" -> (float) Bridge.nativePlayerSaturation(uuidString);
            case "setSaturation" -> {
                Bridge.nativePlayerSetSaturation(uuidString, ((Number) args[0]).doubleValue());
                yield null;
            }
            case "getExhaustion" -> 0.0F;
            case "setExhaustion" -> null;
            case "isDead" -> Bridge.nativePlayerHealth(uuidString) <= 0.0D;
            case "getRemainingAir", "getMaximumAir" -> 300;
            case "getFallDistance" -> 0.0F;
            case "isSneaking", "isSprinting", "isFlying", "getAllowFlight",
                 "isConversing", "isSleeping", "isInsideVehicle", "isGliding",
                 "isSwimming", "isCollidable", "isInvulnerable", "isSilent",
                 "isVisualFire", "isFrozen" -> false;
            case "getPing" -> 0;
            case "getLocale" -> "en_us";
            case "getClientBrandName" -> "Folia-RS";
            case "getEffectivePermissions" -> permissions.getEffectivePermissions();
            case "getPermissions" -> permissions.getPermissionNames();
            case "getPermissionAttachments" -> Set.copyOf(permissions.attachments());
            case "hasPermission" -> permission(args, true);
            case "isPermissionSet" -> permission(args, false);
            case "isOp" -> permissions.isOp();
            case "setOp" -> {
                permissions.setOp(args != null && Boolean.TRUE.equals(args[0]));
                yield null;
            }
            case "addAttachment" -> addAttachment(args);
            case "removeAttachment" -> {
                if (args != null && args.length == 1 && args[0] instanceof PermissionAttachment attachment) {
                    permissions.removeAttachment(attachment);
                }
                yield null;
            }
            case "recalculatePermissions" -> null;
            case "getType" -> EntityType.PLAYER;
            case "getEntityId" -> uniqueId.hashCode();
            case "getLastDamage" -> 0.0D;
            case "getLastDamageCause" -> null;
            case "getBedSpawnLocation" -> null;
            case "setBedSpawnLocation" -> null;
            case "getScoreboard" -> (Scoreboard) null;
            case "setScoreboard" -> null;
            case "getOpenInventory" -> (InventoryView) null;
            case "closeInventory", "openInventory", "openWorkbench", "openEnchanting",
                 "openMerchant" -> null;
            case "updateInventory", "saveData" -> {
                Bridge.nativePlayerSave(uuidString);
                yield null;
            }
            case "getAddress" -> null;
            case "getEquipment" -> null;
            case "teleport" -> teleport(args);
            case "teleportAsync" -> CompletableFuture.completedFuture(true);
            case "sendMessage", "sendRawMessage", "sendActionBar", "sendPlainMessage",
                 "sendPlayerListHeaderAndFooter" -> {
                Bridge.nativePlayerSendMessage(uuidString, message(args));
                yield null;
            }
            case "kickPlayer", "kick" -> {
                Bridge.nativePlayerKick(uuidString, message(args));
                yield null;
            }
            case "playSound" -> {
                BridgeEffects.PlaySound sound = BridgeEffects.playSound(args);
                if (sound != null) {
                    Bridge.nativePlayerPlaySound(
                            uuidString, sound.sound(), sound.category(),
                            sound.location().getX(), sound.location().getY(),
                            sound.location().getZ(), sound.volume(), sound.pitch(),
                            sound.seed());
                }
                yield null;
            }
            case "spawnParticle" -> {
                BridgeEffects.SpawnParticle particle = BridgeEffects.spawnParticle(world.proxy(), args);
                if (particle != null) {
                    Bridge.nativePlayerSpawnParticle(
                            uuidString, particle.particle(),
                            particle.location().getX(), particle.location().getY(),
                            particle.location().getZ(),
                            (float) particle.offsetX(), (float) particle.offsetY(),
                            (float) particle.offsetZ(), (float) particle.speed(),
                            particle.count(), particle.force());
                }
                yield null;
            }
            case "playEffect", "stopSound", "stopAllSounds" -> {
                Bridge.logger().fine(() -> "Ignoring unsupported effect " + method.getName()
                        + " from " + name + ": no matching client packet is modeled");
                yield null;
            }
            case "setVelocity", "setFireTicks", "setRemainingAir", "setMaximumAir" -> null;
            case "getVelocity" -> new org.bukkit.util.Vector();
            case "getVehicle", "getPassenger", "getEjectPassengers" -> null;
            case "getNearbyEntities", "getPassengers" -> List.of();
            case "getTrackedPlayers" -> Set.of();
            case "getItemOnCursor" -> new BridgeItemStack(Material.AIR, 0);
            case "setItemOnCursor" -> null;
            case "hasMetadata" -> false;
            case "getMetadata" -> List.of();
            case "setMetadata", "removeMetadata" -> null;
            case "getPersistentDataContainer" -> null;
            default -> BridgeValues.defaultValue(method);
        };
    }

    private Object permission(Object[] args, boolean checkValue) {
        if (args == null || args.length == 0 || args[0] == null) {
            return false;
        }
        if (args[0] instanceof Permission permission) {
            return checkValue ? permissions.hasPermission(permission)
                    : permissions.isPermissionSet(permission);
        }
        return checkValue ? permissions.hasPermission(String.valueOf(args[0]))
                : permissions.isPermissionSet(String.valueOf(args[0]));
    }

    private Object addAttachment(Object[] args) {
        if (args == null || args.length == 0 || !(args[0] instanceof Plugin plugin)) {
            return null;
        }
        return switch (args.length) {
            case 1 -> permissions.addAttachment(plugin);
            case 2 -> args[1] instanceof Number number
                    ? permissions.addAttachment(plugin, number.intValue())
                    : permissions.addAttachment(plugin, String.valueOf(args[1]), true);
            case 3 -> permissions.addAttachment(
                    plugin,
                    String.valueOf(args[1]),
                    Boolean.TRUE.equals(args[2])
            );
            default -> permissions.addAttachment(
                    plugin,
                    String.valueOf(args[1]),
                    Boolean.TRUE.equals(args[2]),
                    ((Number) args[3]).intValue()
            );
        };
    }

    private Object teleport(Object[] args) {
        if (args == null || args.length == 0) {
            return false;
        }
        Location target;
        if (args[0] instanceof Location location) {
            target = location;
        } else if (args[0] instanceof Entity entity) {
            target = entity.getLocation();
        } else {
            return false;
        }
        return Bridge.nativePlayerTeleport(
                uuidString,
                target.getX(), target.getY(), target.getZ(),
                target.getYaw(), target.getPitch()
        );
    }

    private static GameMode gameMode(int value) {
        GameMode mode = GameMode.getByValue(value);
        return mode == null ? GameMode.SURVIVAL : mode;
    }

    private static String message(Object[] args) {
        if (args == null || args.length == 0 || args[0] == null) {
            return "";
        }
        Object first = args.length > 1 && args[args.length - 1] instanceof Component component
                ? component : args[0];
        if (first instanceof Component component) {
            return PLAIN.serialize(component);
        }
        if (first instanceof String[] lines) {
            return String.join("\n", lines);
        }
        if (first instanceof String line) {
            return line;
        }
        if (first instanceof net.kyori.adventure.text.ComponentLike componentLike) {
            return PLAIN.serialize(componentLike.asComponent());
        }
        return String.valueOf(first);
    }

    static ItemStack item(Material material, int amount) {
        return new BridgeItemStack(material, amount);
    }

    static List<Player> none() {
        return List.of();
    }

    @SuppressWarnings("unused")
    private static Permission permission(String name) {
        return null;
    }

    @SuppressWarnings("unused")
    private static Plugin plugin() {
        return null;
    }
}
