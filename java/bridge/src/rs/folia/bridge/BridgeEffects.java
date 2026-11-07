package rs.folia.bridge;

import org.bukkit.Location;
import org.bukkit.NamespacedKey;
import org.bukkit.Particle;
import org.bukkit.Sound;
import org.bukkit.SoundCategory;
import org.bukkit.World;
import org.bukkit.entity.Entity;

import java.util.concurrent.ThreadLocalRandom;

/**
 * Argument parsing for {@code playSound}/{@code spawnParticle}.
 *
 * <p>Bukkit exposes many default overloads that all funnel into the abstract
 * category/doubles forms. A dynamic proxy sees the originally invoked method,
 * so this helper accepts every shape: {@link Location} or {@link Entity}
 * targets, {@link Sound} or {@link String} sounds, both coordinate forms for
 * particles, and the optional data/force tail.</p>
 */
final class BridgeEffects {
    private BridgeEffects() {
    }

    record PlaySound(Location location, String sound, int category,
                     float volume, float pitch, long seed) {
    }

    record SpawnParticle(Location location, String particle, int count,
                         double offsetX, double offsetY, double offsetZ,
                         double speed, boolean force) {
    }

    static String key(Object value) {
        if (value instanceof Sound sound) {
            return sound.getKey().toString();
        }
        if (value instanceof Particle particle) {
            return particle.getKey().toString();
        }
        if (value instanceof NamespacedKey key) {
            return key.toString();
        }
        if (value instanceof String text) {
            String trimmed = text.trim();
            if (trimmed.isEmpty()) {
                return null;
            }
            return trimmed.indexOf(':') >= 0 ? trimmed : "minecraft:" + trimmed;
        }
        return null;
    }

    static Location location(Object value) {
        if (value instanceof Location location) {
            return location;
        }
        if (value instanceof Entity entity) {
            return entity.getLocation();
        }
        return null;
    }

    static PlaySound playSound(Object[] args) {
        if (args == null || args.length < 2) {
            return null;
        }
        Location location = location(args[0]);
        String sound = key(args[1]);
        if (location == null || sound == null) {
            return null;
        }
        int index = 2;
        int category = 0;
        if (index < args.length && args[index] instanceof SoundCategory soundCategory) {
            category = soundCategory.ordinal();
            index++;
        }
        float volume = number(args, index, 1.0F).floatValue();
        float pitch = number(args, index + 1, 1.0F).floatValue();
        long seed = args.length > index + 2 && args[index + 2] instanceof Number number
                ? number.longValue()
                : ThreadLocalRandom.current().nextLong();
        return new PlaySound(location, sound, category, volume, pitch, seed);
    }

    static SpawnParticle spawnParticle(World world, Object[] args) {
        if (args == null || args.length < 3) {
            return null;
        }
        String particle = key(args[0]);
        if (particle == null) {
            return null;
        }

        Location location;
        int count;
        int index;
        if (args[1] instanceof Location target) {
            location = target;
            count = integer(args, 2, 0);
            index = 3;
        } else if (args[1] instanceof Number x
                && args.length > 3
                && args[2] instanceof Number y
                && args[3] instanceof Number z) {
            location = new Location(world, x.doubleValue(), y.doubleValue(), z.doubleValue());
            count = integer(args, 4, 0);
            index = 5;
        } else {
            return null;
        }

        double[] values = new double[4];
        int numeric = 0;
        while (index < args.length && args[index] instanceof Number number && numeric < values.length) {
            values[numeric++] = number.doubleValue();
            index++;
        }
        boolean force = args.length > index && Boolean.TRUE.equals(args[args.length - 1]);
        return new SpawnParticle(location, particle, count,
                values[0], values[1], values[2], values[3], force);
    }

    private static Number number(Object[] args, int index, Number fallback) {
        return index < args.length && args[index] instanceof Number number ? number : fallback;
    }

    private static int integer(Object[] args, int index, int fallback) {
        return index < args.length && args[index] instanceof Number number
                ? number.intValue() : fallback;
    }
}
