# Plugin compatibility

**Unmodified Bukkit/Paper `.jar` support is partial.** A jar that stays inside
the covered public API can load and run against the bridge; a plugin that uses
NMS/reflection, Folia's region schedulers, PDC persistence or rich chat will not
work yet. This is a compatibility layer in progress, not a complete Bukkit
implementation, and there is no curated plugin corpus passing yet (TODO P9.11).

## How plugin loading works

The JVM is lazy: it starts only when `plugins/*.jar` exists and
`FOLIA_DISABLE_PLUGINS=1` is not set. Plugins load with a Paper-compatible class
loader against the real `folia-api` classes, go through
`onLoad` → `onEnable` → ticks → `onDisable`, and get data folders under
`run/<PluginName>/`. All game state they touch lives in Rust; Java adapters
translate API calls into JNI calls on `folia-core`. See
[ARCHITECTURE.md](ARCHITECTURE.md#bridge-boundary) and
`java/bridge/src/rs/folia/bridge/`.

## API surface matrix

| Surface | Status | What is covered / what is missing |
|---|---|---|
| Lifecycle, logger, `plugin.yml` | Works | Load/enable/disable, descriptions, plugin data folders. |
| Events (host bus) | Partial | `PlayerJoinEvent` from live joins; `BlockBreakEvent`, `BlockPlaceEvent`, `PlayerInteractEvent` are constructible, dispatched and cancellable via `PluginRuntime::fire_*` and the bridge self-test. Live `join.rs` handlers still bypass the bus, so real gameplay does not fire them; quit, inventory, chat, command, death/respawn, entity and world events are missing (P9.2). |
| Commands | Partial | `plugin.yml` commands, `PluginCommand` executors and a dispatch hook work; tab completion, full command trees, Bukkit console dispatch and permission-gated chat commands are open (P9.5). |
| Scheduler | Partial | `runTask`, `runTaskLater`, `runTaskTimer`, legacy sync wrappers, cancellation by id/plugin and plugin-disable cleanup run through the Rust task table. Async variants are downgraded to the server tick thread (`isSync()` returns true). Folia region/entity schedulers are **not** implemented (P9.4). |
| Configuration / YAML | Partial | `plugin.getConfig()` is a Rust-backed flat scalar `config.yml` in `run/<PluginName>/`; `saveDefaultConfig`, `saveConfig`, dotted scalar get/set round-trip. Nested sections, lists, maps, comments and stock `reloadConfig()` are open (P9.6). |
| Permissions | Partial | `hasPermission`, `isPermissionSet`, add/remove/set/unset attachments, timed attachments and operator-default fallback are Rust-backed. Wildcard/parent expansion from `plugin.yml` and registration through `PluginManager.getPermission` are open (P9.5). |
| Services / plugin manager | Partial | Typed `ServicesManager` register/unregister/load with priorities and plugin-disable cleanup works. Dependency/load ordering (`depend`, `softdepend`, `loadbefore`) and plugin-manager queries are open (P9.7). |
| Inventories and items | Partial | Storage slots 0-35, item type/count/damage/name/lore/unbreakable/enchantments round-trip through the Rust component model and playerdata; `new ItemStack(...)` and registry constants work. Armor/off-hand, container contents, item flags, custom model data and live client inventory updates are missing (P9.8). |
| PDC (`PersistentDataContainer`) | Missing | Not exposed; no persistence across save/reload (P9.8). |
| World / block access | Partial | `World`/`Block`/`Chunk`/`BlockData` proxies read type, location, highest-Y, biome and sky/block light from `WorldStorage`; block edits persist and broadcast. Reads cover stored or cached columns only; no on-demand generation, full chunk snapshots or entity spawning (P9.9). |
| World time / weather | Partial | `getTime`/`setTime`/`getFullTime`/`isDayTime`, storm/thunder flags and spawn persist in `run/world_state.txt`; time and weather changes reach live sessions. Periodic synchronization is open (P9.9). |
| Player API | Partial | Identity, world/location, game mode, messages, kick, teleport, health/food/saturation and permissions. Game mode, chat, kick, teleport, sounds and simple particles reach a live session; health/food and inventory changes persist through playerdata only, not live packets (P9.3). |
| Entity API | Missing / minimal | Entity spawn/query, `LivingEntity`, metadata, AI, combat and entity events are not exposed to plugins; Rust-side entity tracking exists but is not wired as a Bukkit API (P9.9). |
| Chat | Missing for rich/signed | Plain `sendMessage` and live player chat work; signed/rich chat components, action-bar overlays and chat signing enforcement are not complete (P8.2/P9.3). |
| Sounds, particles, effects | Partial | `playSound` and simple `spawnParticle` send 1.21.11 packets to live sessions; option-carrying particles, effect distance filtering and per-viewer metadata dirtying are missing (P9.9). |
| NMS / `CraftBukkit` / reflection | Missing | No `CraftWorld`, `CraftPlayer`, `CraftEntity`, `CraftItemStack` or `net.minecraft.*` shims. Plugins that call `getHandle()` or reflect over NMS will fail (P9.10, long tail). |
| ABI stability | Partial | Native calls are grouped into world/player/inventory/item with explicit signatures in `Bridge.java`, and a Java API test pins the codec. The protocol is not versioned or exhaustively covered (P9.1). |
| Isolation and failure handling | Missing | Plugin exceptions are not fully attributed/contained; reload/disable behavior is not production-hardened (P9.12). |
| Compatibility corpus | Missing | No tiered jar test set or CI run yet; the only exercised plugin is `java/example-plugin` (P9.11). |

## Known to work end to end

The bundled `java/example-plugin` exercises lifecycle, world/block access,
player identity/vitals/game mode/messages, inventory, permissions, config,
services, time/weather, sounds/particles, enchantments and the three
cancellable events. The bridge self-test verifies them:

```powershell
$env:FOLIA_LIBRARIES = "C:\Users\austr\Downloads\rustfol\libraries"
$env:FOLIA_BRIDGE_SELFTEST = "1"
$env:FOLIA_MAX_TICKS = "20"
cargo run -p folia-server
```

Build the bridge and example plugin first with
`.\scripts\build-java.ps1 -LibrariesDirectory <dir>`. See
[OPERATOR_GUIDE.md](OPERATOR_GUIDE.md) and the root `README.md` for build and
run commands, and `TODO.md` P9 for the authoritative per-task progress.
