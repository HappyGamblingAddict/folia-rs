package rs.folia.bridge;

import org.bukkit.plugin.Plugin;
import org.bukkit.plugin.RegisteredServiceProvider;
import org.bukkit.plugin.ServicePriority;
import org.bukkit.plugin.ServicesManager;

import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.logging.Logger;

/**
 * In-memory Bukkit {@link ServicesManager}.
 *
 * <p>Registrations are typed by service interface and ordered by
 * {@link ServicePriority}; {@link #load(Class)} returns the highest-priority
 * provider or {@code null}. The manager is process-local, matching Paper's
 * default service registry for a single server instance.</p>
 */
final class BridgeServicesManager implements ServicesManager {
    private final Logger log;
    private final Map<Class<?>, List<RegisteredServiceProvider<?>>> registrations =
            new LinkedHashMap<>();

    BridgeServicesManager(Logger log) {
        this.log = log;
    }

    @Override
    public synchronized <T> void register(Class<T> service, T provider, Plugin plugin,
                                          ServicePriority priority) {
        if (service == null || provider == null || plugin == null) {
            throw new IllegalArgumentException("service, provider, and plugin must not be null");
        }
        RegisteredServiceProvider<T> registration =
                new RegisteredServiceProvider<>(service, provider, priority, plugin);
        List<RegisteredServiceProvider<?>> providers =
                registrations.computeIfAbsent(service, key -> new ArrayList<>());
        providers.add(registration);
        providers.sort((left, right) ->
                Integer.compare(right.getPriority().ordinal(), left.getPriority().ordinal()));
        log.fine(() -> "Registered service " + service.getName() + " from "
                + plugin.getName() + " at " + priority);
    }

    @Override
    public synchronized void unregisterAll(Plugin plugin) {
        for (List<RegisteredServiceProvider<?>> providers : registrations.values()) {
            providers.removeIf(registration -> registration.getPlugin() == plugin);
        }
        registrations.values().removeIf(List::isEmpty);
    }

    @Override
    public synchronized void unregister(Class<?> service, Object provider) {
        List<RegisteredServiceProvider<?>> providers = registrations.get(service);
        if (providers == null) {
            return;
        }
        providers.removeIf(registration -> registration.getProvider() == provider);
        if (providers.isEmpty()) {
            registrations.remove(service);
        }
    }

    @Override
    public synchronized void unregister(Object provider) {
        for (List<RegisteredServiceProvider<?>> providers : registrations.values()) {
            providers.removeIf(registration -> registration.getProvider() == provider);
        }
        registrations.values().removeIf(List::isEmpty);
    }

    @Override
    public synchronized <T> T load(Class<T> service) {
        RegisteredServiceProvider<T> registration = getRegistration(service);
        return registration == null ? null : registration.getProvider();
    }

    @Override
    public synchronized <T> RegisteredServiceProvider<T> getRegistration(Class<T> service) {
        List<RegisteredServiceProvider<?>> providers = registrations.get(service);
        if (providers == null || providers.isEmpty()) {
            return null;
        }
        @SuppressWarnings("unchecked")
        RegisteredServiceProvider<T> registration =
                (RegisteredServiceProvider<T>) providers.getFirst();
        return registration;
    }

    @Override
    public synchronized List<RegisteredServiceProvider<?>> getRegistrations(Plugin plugin) {
        List<RegisteredServiceProvider<?>> output = new ArrayList<>();
        for (List<RegisteredServiceProvider<?>> providers : registrations.values()) {
            for (RegisteredServiceProvider<?> registration : providers) {
                if (registration.getPlugin() == plugin) {
                    output.add(registration);
                }
            }
        }
        return Collections.unmodifiableList(output);
    }

    @Override
    public synchronized <T> Collection<RegisteredServiceProvider<T>> getRegistrations(
            Class<T> service) {
        List<RegisteredServiceProvider<?>> providers = registrations.get(service);
        if (providers == null) {
            return List.of();
        }
        List<RegisteredServiceProvider<T>> output = new ArrayList<>(providers.size());
        for (RegisteredServiceProvider<?> registration : providers) {
            @SuppressWarnings("unchecked")
            RegisteredServiceProvider<T> typed = (RegisteredServiceProvider<T>) registration;
            output.add(typed);
        }
        return Collections.unmodifiableList(output);
    }

    @Override
    public synchronized Set<Class<?>> getKnownServices() {
        return Collections.unmodifiableSet(Set.copyOf(registrations.keySet()));
    }

    @Override
    public synchronized <T> boolean isProvidedFor(Class<T> service) {
        List<RegisteredServiceProvider<?>> providers = registrations.get(service);
        return providers != null && !providers.isEmpty();
    }
}
