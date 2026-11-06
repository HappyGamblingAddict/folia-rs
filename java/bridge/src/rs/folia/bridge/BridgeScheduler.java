package rs.folia.bridge;

import org.bukkit.plugin.Plugin;
import org.bukkit.scheduler.BukkitScheduler;
import org.bukkit.scheduler.BukkitTask;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Future;
import java.util.logging.Level;
import java.util.logging.Logger;

/**
 * Bukkit scheduler facade.
 *
 * <p>The actual task table lives in Rust ({@code TASKS} in {@code jvm.rs}).
 * Scheduling, cancellation and tick advancement all happen in the Rust server
 * loop; Java only keeps the {@link BukkitTask} handle that plugins expect.</p>
 *
 * <p><strong>Asynchronous semantics:</strong> the bridge has no worker pool, so
 * the asynchronous variants are accepted but run on the server thread in the
 * same Rust tick queue. Their {@link BukkitTask#isSync()} intentionally reports
 * {@code true}; a plugin must not treat them as off-thread work. This is an
 * explicit limitation until a bridge-owned async executor exists.</p>
 */
final class BridgeScheduler {
    /** Test seam so the deferral/cancellation protocol can be checked without JNI. */
    interface Backend {
        int schedule(String pluginName, Runnable task, long delay, long period);

        void cancel(int taskId);

        void cancelTasks(String pluginName);
    }

    private final Logger log;
    private final Backend backend;
    private final BukkitScheduler proxy;

    BridgeScheduler(Logger log, ClassLoader classLoader) {
        this(log, classLoader, new NativeBackend());
    }

    BridgeScheduler(Logger log, ClassLoader classLoader, Backend backend) {
        this.log = log;
        this.backend = backend;
        this.proxy = (BukkitScheduler) Proxy.newProxyInstance(
                classLoader,
                new Class<?>[]{BukkitScheduler.class},
                this::handle
        );
    }

    BukkitScheduler proxy() {
        return proxy;
    }

    /** Tasks are advanced by Rust; this exists so the host tick stays symmetric. */
    void tick() {
    }

    private Object handle(Object proxyObject, Method method, Object[] args) {
        if (method.getDeclaringClass() == Object.class) {
            return objectMethod(proxyObject, method, args);
        }
        String name = method.getName();
        try {
            return switch (name) {
                case "runTaskTimer" -> schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), asLong(args, 3));
                case "runTaskTimerAsynchronously" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), asLong(args, 3), false);
                case "runTaskLater" -> schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), 0L);
                case "runTaskLaterAsynchronously" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), 0L, false);
                case "runTask" -> schedule((Plugin) args[0], (Runnable) args[1], 0L, 0L);
                case "runTaskAsynchronously" -> schedule((Plugin) args[0], (Runnable) args[1], 0L, 0L, false);
                case "scheduleSyncDelayedTask" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), 0L);
                case "scheduleSyncRepeatingTask" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), asLong(args, 3));
                case "scheduleAsyncDelayedTask" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), 0L, false);
                case "scheduleAsyncRepeatingTask" ->
                        schedule((Plugin) args[0], (Runnable) args[1], asLong(args, 2), asLong(args, 3), false);
                case "callSyncMethod" -> callSync((Callable<?>) args[1]);
                case "cancelTask" -> {
                    backend.cancel(asInt(args, 0));
                    yield null;
                }
                case "cancelTasks" -> {
                    backend.cancelTasks(((Plugin) args[0]).getName());
                    yield null;
                }
                case "isCurrentlyRunning", "isQueued" -> false;
                case "getActiveWorkers", "getPendingTasks" -> List.of();
                default -> BridgeValues.defaultValue(method);
            };
        } catch (Throwable throwable) {
            log.log(Level.SEVERE, "Scheduler method " + name + " failed", throwable);
            return BridgeValues.defaultValue(method);
        }
    }

    private BukkitTask schedule(Plugin plugin, Runnable runnable, long delay, long period) {
        return schedule(plugin, runnable, delay, period, true);
    }

    private BukkitTask schedule(Plugin plugin, Runnable runnable, long delay, long period,
                                boolean asyncRequested) {
        if (!asyncRequested) {
            log.fine(() -> "Async task from " + plugin.getName()
                    + " will run on the server tick thread; no async executor exists");
        }
        int id = backend.schedule(
                plugin.getName(),
                runnable,
                Math.max(0L, delay),
                Math.max(0L, period)
        );
        if (id < 0) {
            throw new IllegalStateException("Rust host rejected the scheduled task");
        }
        // isSync is always true: the Rust queue runs tasks on the server thread.
        return new Task(id, plugin, true).proxy;
    }

    private Future<Object> callSync(Callable<?> callable) {
        try {
            return CompletableFuture.completedFuture(callable.call());
        } catch (Throwable throwable) {
            return CompletableFuture.failedFuture(throwable);
        }
    }

    private static long asLong(Object[] args, int index) {
        if (args == null || args.length <= index || args[index] == null) {
            return 0L;
        }
        return ((Number) args[index]).longValue();
    }

    private static int asInt(Object[] args, int index) {
        if (args == null || args.length <= index || args[index] == null) {
            return -1;
        }
        return ((Number) args[index]).intValue();
    }

    private static Object objectMethod(Object proxyObject, Method method, Object[] args) {
        return switch (method.getName()) {
            case "toString" -> "BridgeScheduler";
            case "hashCode" -> System.identityHashCode(proxyObject);
            case "equals" -> proxyObject == args[0];
            default -> null;
        };
    }

    private static final class NativeBackend implements Backend {
        @Override
        public int schedule(String pluginName, Runnable task, long delay, long period) {
            return Bridge.nativeScheduleTask(pluginName, task, delay, period);
        }

        @Override
        public void cancel(int taskId) {
            Bridge.nativeCancelTask(taskId);
        }

        @Override
        public void cancelTasks(String pluginName) {
            Bridge.nativeCancelTasks(pluginName);
        }
    }

    private final class Task implements InvocationHandler {
        private final int id;
        private final Plugin owner;
        private final boolean sync;
        private final BukkitTask proxy;
        private volatile boolean cancelled;

        private Task(int id, Plugin owner, boolean sync) {
            this.id = id;
            this.owner = owner;
            this.sync = sync;
            this.proxy = (BukkitTask) Proxy.newProxyInstance(
                    BridgeScheduler.class.getClassLoader(),
                    new Class<?>[]{BukkitTask.class},
                    this
            );
        }

        @Override
        public Object invoke(Object proxyObject, Method method, Object[] args) {
            if (method.getDeclaringClass() == Object.class) {
                return switch (method.getName()) {
                    case "toString" -> "BridgeTask#" + id;
                    case "hashCode" -> System.identityHashCode(proxyObject);
                    case "equals" -> proxyObject == args[0];
                    default -> null;
                };
            }
            return switch (method.getName()) {
                case "getTaskId" -> id;
                case "getOwner" -> owner;
                case "isSync" -> sync;
                case "isCancelled" -> cancelled;
                case "cancel" -> {
                    if (!cancelled) {
                        cancelled = true;
                        backend.cancel(id);
                    }
                    yield null;
                }
                default -> BridgeValues.defaultValue(method);
            };
        }
    }
}
