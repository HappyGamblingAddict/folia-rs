package rs.folia.bridge;

import org.bukkit.permissions.Permissible;
import org.bukkit.permissions.Permission;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.permissions.PermissionAttachmentInfo;
import org.bukkit.plugin.Plugin;

import java.util.ArrayList;
import java.util.IdentityHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Bukkit permission facade backed by the Rust permission attachments.
 *
 * <p>Explicit attachment values win over the default. When a permission has no
 * attachment, the player's operator flag decides the value, matching
 * {@code PermissibleBase.hasPermission}: default-op permissions are granted to
 * operators. Attachment values are lower-cased like Bukkit's permission map.
 * Wildcard/permission-parent expansion from {@code plugin.yml} is not modelled
 * yet.</p>
 */
final class BridgePermissions {
    private final Permissible permissible;
    private final String uuid;
    private final Map<PermissionAttachment, Integer> attachmentIds = new IdentityHashMap<>();

    BridgePermissions(Permissible permissible, String uuid) {
        this.permissible = permissible;
        this.uuid = uuid;
    }

    boolean isOp() {
        return Bridge.nativePlayerIsOp(uuid);
    }

    void setOp(boolean value) {
        Bridge.nativePlayerSetOp(uuid, value);
    }

    boolean isPermissionSet(String permission) {
        if (permission == null) {
            throw new IllegalArgumentException("Permission name cannot be null");
        }
        return Bridge.nativePlayerPermissionIsSet(uuid, permission);
    }

    boolean isPermissionSet(Permission permission) {
        if (permission == null) {
            throw new IllegalArgumentException("Permission cannot be null");
        }
        return isPermissionSet(permission.getName());
    }

    boolean hasPermission(String permission) {
        if (permission == null) {
            throw new IllegalArgumentException("Permission name cannot be null");
        }
        return Bridge.nativePlayerPermissionHas(uuid, permission);
    }

    boolean hasPermission(Permission permission) {
        if (permission == null) {
            throw new IllegalArgumentException("Permission cannot be null");
        }
        return hasPermission(permission.getName());
    }

    PermissionAttachment addAttachment(Plugin plugin) {
        if (plugin == null) {
            throw new IllegalArgumentException("Plugin cannot be null");
        }
        int id = Bridge.nativePlayerPermissionAddAttachment(uuid, plugin.getName());
        if (id < 0) {
            throw new IllegalStateException("Rust host rejected the permission attachment");
        }
        BridgePermissionAttachment attachment =
                new BridgePermissionAttachment(plugin, permissible, this, id);
        attachmentIds.put(attachment, id);
        return attachment;
    }

    PermissionAttachment addAttachment(Plugin plugin, String permission, boolean value) {
        PermissionAttachment attachment = addAttachment(plugin);
        attachment.setPermission(permission, value);
        return attachment;
    }

    PermissionAttachment addAttachment(Plugin plugin, String permission, boolean value, int ticks) {
        PermissionAttachment attachment = addAttachment(plugin, permission, value);
        if (ticks > 0) {
            // Bukkit's timed attachment is a scheduler task. Reuse the Rust
            // task queue so plugin disable still cancels it.
            Bridge.nativeScheduleTask(plugin.getName(), () -> removeAttachment(attachment), ticks, 0L);
        }
        return attachment;
    }

    PermissionAttachment addAttachment(Plugin plugin, int ticks) {
        PermissionAttachment attachment = addAttachment(plugin);
        if (ticks > 0) {
            Bridge.nativeScheduleTask(plugin.getName(), () -> removeAttachment(attachment), ticks, 0L);
        }
        return attachment;
    }

    boolean removeAttachment(PermissionAttachment attachment) {
        if (attachment == null) {
            throw new IllegalArgumentException("Attachment cannot be null");
        }
        Integer id = attachmentIds.remove(attachment);
        if (id == null) {
            return false;
        }
        if (!Bridge.nativePlayerPermissionRemoveAttachment(uuid, id)) {
            return false;
        }
        if (attachment instanceof BridgePermissionAttachment bridge) {
            bridge.markRemoved();
        }
        if (attachment.getRemovalCallback() != null) {
            attachment.getRemovalCallback().attachmentRemoved(attachment);
        }
        return true;
    }

    void setAttachmentPermission(int id, String permission, boolean value) {
        Bridge.nativePlayerPermissionSet(uuid, id, permission, value);
    }

    void unsetAttachmentPermission(int id, String permission) {
        Bridge.nativePlayerPermissionUnset(uuid, id, permission);
    }

    Set<PermissionAttachmentInfo> getEffectivePermissions() {
        Set<PermissionAttachmentInfo> output = new LinkedHashSet<>();
        for (String line : Bridge.nativePlayerPermissionEffective(uuid).split("\n")) {
            if (line.isEmpty()) {
                continue;
            }
            String[] fields = line.split("\t", -1);
            if (fields.length < 2) {
                continue;
            }
            output.add(new PermissionAttachmentInfo(
                    permissible,
                    fields[0],
                    null,
                    "1".equals(fields[1])
            ));
        }
        return output;
    }

    Set<String> getPermissionNames() {
        Set<String> names = new LinkedHashSet<>();
        for (PermissionAttachmentInfo info : getEffectivePermissions()) {
            if (info.getValue()) {
                names.add(info.getPermission());
            }
        }
        return names;
    }

    List<PermissionAttachment> attachments() {
        return new ArrayList<>(attachmentIds.keySet());
    }
}
