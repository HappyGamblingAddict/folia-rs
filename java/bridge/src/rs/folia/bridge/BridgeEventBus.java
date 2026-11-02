package rs.folia.bridge;

import org.bukkit.event.Cancellable;
import org.bukkit.event.Event;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.plugin.EventExecutor;
import org.bukkit.plugin.Plugin;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Small event bus that understands Bukkit's {@link EventHandler} annotations.
 *
 * <p>This is intentionally independent of Bukkit's global {@code HandlerList}:
 * the Rust core fires events into this bus, and listeners are invoked directly.
 * That keeps event dispatch deterministic with the Rust tick loop.</p>
 */
final class BridgeEventBus {
    private final Logger log;
    private final List<Registration> registrations = new ArrayList<>();
    private final List<ExecutorRegistration> executors = new ArrayList<>();

    BridgeEventBus(Logger log) {
        this.log = log;
    }

    synchronized void register(Listener listener, Plugin plugin) {
        for (Method method : listener.getClass().getMethods()) {
            if (method.isBridge() || method.isSynthetic()) {
                continue;
            }
            EventHandler handler = method.getAnnotation(EventHandler.class);
            if (handler == null) {
                continue;
            }
            Class<?>[] parameters = method.getParameterTypes();
            if (parameters.length != 1 || !Event.class.isAssignableFrom(parameters[0])) {
                log.warning("Invalid @EventHandler method " + method + " in " + listener.getClass().getName());
                continue;
            }
            try {
                method.setAccessible(true);
            } catch (RuntimeException exception) {
                log.log(Level.WARNING, "Could not make " + method + " accessible", exception);
            }
            registrations.add(new Registration(
                    plugin,
                    listener,
                    method,
                    parameters[0].asSubclass(Event.class),
                    handler.priority(),
                    handler.ignoreCancelled()
            ));
        }
        registrations.sort(Comparator.comparingInt(registration -> registration.priority.ordinal()));
    }

    synchronized void register(Class<? extends Event> eventType, Listener listener, EventPriority priority,
                               EventExecutor executor, Plugin plugin, boolean ignoreCancelled) {
        executors.add(new ExecutorRegistration(plugin, listener, eventType, priority, executor, ignoreCancelled));
    }

    void call(Event event) {
        List<Registration> listenerSnapshot;
        List<ExecutorRegistration> executorSnapshot;
        synchronized (this) {
            listenerSnapshot = List.copyOf(registrations);
            executorSnapshot = List.copyOf(executors);
        }
        for (Registration registration : listenerSnapshot) {
            if (!registration.eventType.isInstance(event)) {
                continue;
            }
            if (registration.ignoreCancelled && event instanceof Cancellable cancellable && cancellable.isCancelled()) {
                continue;
            }
            try {
                registration.method.invoke(registration.listener, event);
            } catch (InvocationTargetException exception) {
                log.log(Level.SEVERE, "Exception in event handler " + registration.method,
                        exception.getCause());
            } catch (ReflectiveOperationException exception) {
                log.log(Level.SEVERE, "Could not invoke event handler " + registration.method, exception);
            }
        }
        for (ExecutorRegistration registration : executorSnapshot) {
            if (!registration.eventType.isInstance(event)) {
                continue;
            }
            if (registration.ignoreCancelled && event instanceof Cancellable cancellable && cancellable.isCancelled()) {
                continue;
            }
            try {
                registration.executor.execute(registration.listener, event);
            } catch (Throwable throwable) {
                log.log(Level.SEVERE, "Exception in registered event executor for "
                        + registration.eventType.getName(), throwable);
            }
        }
    }

    synchronized void unregister(Plugin plugin) {
        registrations.removeIf(registration -> registration.plugin == plugin);
        executors.removeIf(registration -> registration.plugin == plugin);
    }

    private record Registration(
            Plugin plugin,
            Listener listener,
            Method method,
            Class<? extends Event> eventType,
            EventPriority priority,
            boolean ignoreCancelled
    ) {
    }

    private record ExecutorRegistration(
            Plugin plugin,
            Listener listener,
            Class<? extends Event> eventType,
            EventPriority priority,
            EventExecutor executor,
            boolean ignoreCancelled
    ) {
    }
}
