package rs.folia.bridge;

import com.destroystokyo.paper.SkinParts;
import io.papermc.paper.InternalAPIBridge;
import io.papermc.paper.command.brigadier.CommandSourceStack;
import io.papermc.paper.datacomponent.item.ResolvableProfile;
import io.papermc.paper.world.damagesource.CombatEntry;
import io.papermc.paper.world.damagesource.FallLocationType;
import net.kyori.adventure.text.Component;
import org.bukkit.GameRule;
import org.bukkit.block.Biome;
import org.bukkit.damage.DamageEffect;
import org.bukkit.damage.DamageSource;
import org.bukkit.entity.LivingEntity;
import org.bukkit.entity.Pose;

import java.util.Set;
import java.util.function.Function;
import java.util.function.Predicate;

/**
 * Paper service-loader shim for API surfaces that expect a server implementation.
 *
 * <p>{@code Biome}'s static initializer asks this bridge for its legacy custom
 * biome. The rest of the internal API is not used by the Rust host yet and
 * returns null/false so callers fail explicitly rather than on a missing
 * service.</p>
 */
public final class BridgeInternalApi implements InternalAPIBridge {
    @Override
    public DamageEffect getDamageEffect(String key) {
        return null;
    }

    @Override
    public Biome constructLegacyCustomBiome() {
        return new BridgeRegistryAccess.BridgeBiome("custom");
    }

    @Override
    public CombatEntry createCombatEntry(LivingEntity entity, DamageSource source, float damage) {
        return null;
    }

    @Override
    public CombatEntry createCombatEntry(DamageSource source, float damage,
                                        FallLocationType location, float fallDistance) {
        return null;
    }

    @Override
    public Predicate<CommandSourceStack> restricted(Predicate<CommandSourceStack> predicate) {
        return predicate;
    }

    @Override
    public ResolvableProfile defaultMannequinProfile() {
        return null;
    }

    @Override
    public SkinParts.Mutable allSkinParts() {
        return null;
    }

    @Override
    public Component defaultMannequinDescription() {
        return null;
    }

    @Override
    public <MODERN, LEGACY> GameRule<LEGACY> legacyGameRuleBridge(
            GameRule<MODERN> rule,
            Function<LEGACY, MODERN> toModern,
            Function<MODERN, LEGACY> toLegacy,
            Class<LEGACY> legacyType) {
        return null;
    }

    @Override
    public Set<Pose> validMannequinPoses() {
        return Set.of();
    }
}
