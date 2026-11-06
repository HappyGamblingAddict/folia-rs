package rs.folia.bridge;

import org.bukkit.permissions.Permissible;
import org.bukkit.permissions.PermissionAttachment;
import org.bukkit.plugin.Plugin;

/**
 * {@link PermissionAttachment} whose values are mirrored into the Rust
 * permission map. Bukkit's parent map keeps the API-visible view; the native
 * copy makes the values visible to the host and to other bridge players.
 */
final class BridgePermissionAttachment extends PermissionAttachment {
    private final BridgePermissions permissions;
    private final int id;
    private boolean removed;

    BridgePermissionAttachment(Plugin plugin, Permissible permissible,
                               BridgePermissions permissions, int id) {
        super(plugin, permissible);
        this.permissions = permissions;
        this.id = id;
    }

    int id() {
        return id;
    }

    boolean isRemoved() {
        return removed;
    }

    void markRemoved() {
        removed = true;
    }

    @Override
    public void setPermission(String name, boolean value) {
        if (removed) {
            return;
        }
        super.setPermission(name, value);
        permissions.setAttachmentPermission(id, name, value);
    }

    @Override
    public void unsetPermission(String name) {
        if (removed) {
            return;
        }
        super.unsetPermission(name);
        permissions.unsetAttachmentPermission(id, name);
    }

    @Override
    public boolean remove() {
        if (removed) {
            return false;
        }
        return permissions.removeAttachment(this);
    }
}
