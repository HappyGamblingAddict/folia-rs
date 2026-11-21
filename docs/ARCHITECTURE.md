# Architecture

This page describes how Folia-RS is put together today. It is a description of
the current implementation, not a claim of feature parity with Folia 1.21.11.

## Ownership model

Rust is the authority for the process, configuration, world state, protocol,
tick loop and gameplay. The decompiled Java tree is a specification used to
implement those systems in Rust; it is not shipped or run as the server.

The embedded JVM is optional and lazy. It starts only when `plugins/` contains
at least one `.jar` and `FOLIA_DISABLE_PLUGINS=1` is not set
(`crates/folia-server/src/jvm.rs`). Its only job is to load unmodified
Bukkit/Paper plugin classes and translate API calls into Rust. No game or world
logic lives in Java; a plugin that calls `World.getBlockAt` ultimately reads
`WorldStorage` in Rust over JNI.

With no plugin jars the server is fully native and never loads Java at all.

## Crates

| Crate | Role |
|---|---|
| `crates/folia-core` | Authoritative core: protocol, NBT, Anvil/chunks, lighting, worldgen, region scheduler, sessions/join, config, status server, gameplay state (entities, inventories, redstone, crafting, ...). |
| `crates/folia-math` | Coordinates (`BlockPos`, `SectionPos`, directions), vectors, AABBs, `Mth` math helpers. |
| `crates/folia-random` | Legacy LCG and Xoroshiro128++ sources, MD5 positional seeds, Gaussian/support helpers. |
| `crates/folia-server` | The binary (`main.rs`): startup, console, tick loop, shutdown, and the optional JNI plugin runtime (`jvm.rs`). |

Java-side modules:

| Path | Role |
|---|---|
| `java/bridge/` | Bukkit/Paper/Folia API shim, plugin class loader, event bus, scheduler/config/permission/service adapters; the JVM boundary only. |
| `java/example-plugin/` | Demo plugin compiled against `folia-api`, used by the bridge self-test. |

See [PORT_MAP.md](../PORT_MAP.md) for the decompiled-Java → Rust-module mapping
and [PORTING.md](../PORTING.md) for the staged port plan.

## Region ownership and scheduler

`crates/folia-core/src/region.rs` models Folia's 8x8-chunk (and 4x4-section)
region grid:

- `RegionKey`, `ChunkPos`, `SectionPos`, `BlockPos` map world coordinates to
  regions; `RegionMap::vanilla()` uses the vanilla grid size.
- `RegionScheduler` owns per-region task queues with delays, periods and
  cancellation, plus a global queue (`GLOBAL_REGION_KEY`).
- `RegionOwned<T>`, `OwnedWorld`, `WorldSliceToken` and `ThreadingDetector`
  check that a caller acting on region state is actually in that region;
  violations return `OwnershipError` rather than silently crossing regions.
- `RegionTickDriver::vanilla()` groups cached chunks by region, ticks regions in
  sorted key order and calls `WorldStorage::tick_region`, so scheduled/random
  ticks, block entities, hoppers and neighbour drains only see that region's
  chunks. The per-region watchdog currently only reports slow regions; it does
  not interrupt them (TODO P10.8).

The task-level ownership checks are real, but they do **not** yet mean full
Folia region parallelism; see [Known global-state limitations](#known-global-state-limitations).

## Chunk generation pipeline

1. `main.rs` reads the level seed from `level.dat` (falling back to `0`) and
   calls `worldgen::generator::configure(seed)`.
2. `WorldGenerator::generate_chunk` evaluates the overworld density router,
   surface rules, biomes, aquifers and cave/canyon carvers, then runs the
   configured/placed features and structure jigsaw pools. Features write only
   through a chunk-bounded write region; unported feature types fail loudly
   instead of no-oping.
3. `join::load_or_generate_chunk` prefers a stored chunk that carries the
   `folia:written` marker (or a foreign world when `FOLIA_LOAD_STORED=1`),
   otherwise generates and saves one through `WorldStorage` before it is
   streamed. The design rule is that generation must flow through the
   chunk-status pipeline and features must not write directly into arbitrary
   chunks; features currently write through the bounded write region, and the
   full status pipeline is not implemented yet.
4. Packet chunks are built from `WorldStorage` (`chunk_packet.rs`) with dynamic
   heightmaps and sky/block light.

Nether and End generators exist (`worldgen/nether.rs`, `worldgen/end.rs`) but
full non-overworld generation is not finished. The embedded generated assets are
version-guarded at startup (`asset_version.rs`: 1.21.11 / DataVersion 4788).

## Storage

`crates/folia-core/src/world.rs` defines `WorldStorage`, backed by
`crates/folia-core/src/anvil.rs`:

- Anvil `.mca` region files (32x32 chunks, 4 KiB sectors), gzip/zlib/uncompressed
  chunk compression, chunk palettes, heightmaps, block entities and NBT.
- Light uses vanilla 2048-byte nibble layers per section, with column sky
  sources and emitter-based block-light BFS; stored layers are preferred in
  chunk packets and neighbours are relit on edits.
- `folia:written` marks chunks this server generated; `GENERATION_VERSION` (2)
  marks the generation capability. A level this server created serves only
  stamped chunks so leftovers cannot hide generated terrain.
- Dimension roots: `region/` (overworld),
  `dimensions/minecraft/the_nether/region/`, `dimensions/minecraft/the_end/region/`.
- `shared_overworld()` / `flush_shared()` expose one process-wide
  `Mutex<WorldStorage>` used by the session and region tick paths.

Playerdata (`run/playerdata/`), operators (`run/ops.txt`), plugin config/data
(`run/<PluginName>/`) and persisted world time/weather (`run/world_state.txt`)
live outside the level directory.

## Session loop

`crates/folia-server/src/main.rs::run_server`:

1. Pending joins from `join.rs` are drained and forwarded as `PlayerJoinEvent`
   to the plugin bridge (when present).
2. Plugin tasks tick.
3. `RegionTickDriver::tick_outcome` ticks region queues and world block ticks.
4. `entity::tick_all()` ticks entities.
5. The loop sleeps 50 ms, unless `FOLIA_MAX_TICKS` has been reached.

Startup verifies assets, reads config, sets world/player/view-distance/game-mode
state, seeds the generator, starts the status server and installs a Ctrl+C
handler. Console commands (`help`, `list`, `ops`, `op`, `deop`, `gamemode`,
`stop`) run on a separate thread. Shutdown stops sessions with a timeout,
flushes terrain, records day time/last played in `level.dat`, disables plugins
and exits. `stop` or `shutdown` in the console and Ctrl+C are the supported
ways to stop the server.

## Bridge boundary

`java/bridge` exposes Bukkit/Paper API objects backed by Rust state through JNI.
Native calls are grouped into world, player, inventory and item calls with
explicit signatures in `Bridge.java` and encoded item/inventory payloads. The
Rust task table owns scheduler delays/periods; YAML config, permissions and
services are Rust-backed. Events such as `BlockBreakEvent`, `BlockPlaceEvent`
and `PlayerInteractEvent` can be fired from Rust and return cancellation, but
the live `join.rs` handlers do not all dispatch through the event bus yet. The
ABI is not versioned exhaustively (TODO P9.1). Details and gaps are in
[PLUGIN_COMPATIBILITY.md](PLUGIN_COMPATIBILITY.md).

## Known global-state limitations

Do not describe this server as having parallel-region world isolation:

- There is one `WorldStorage` behind a single process-wide `Mutex`; sessions and
  the region tick loop take the same lock.
- Entity, player/session and POI/registry state are global statics, not owned by
  regions.
- Regionized task queues and ownership checks are enforced, but true Folia
  per-region world ownership, entity scheduler and multi-threaded ticking are
  still TODOs (P10). See `BLOCKERS.md`.

## Status

This is a working foundation, not a finished port. Worldgen/packet/physics
parity is measured by differential tests against a Java oracle; many gameplay
systems and most of the Bukkit API remain unimplemented. See the
[operator guide](OPERATOR_GUIDE.md) and the root `README.md` for what currently
works.
