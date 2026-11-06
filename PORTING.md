# Porting Folia 1.21.11 to Rust

This document is the working plan for turning `../decompiled` into a Rust
server. It exists because "rewrite 744k lines 1:1" is not a single change; it is
a staged migration with measurable checkpoints.

## Source inventory

| Area | Files | LOC |
|---|---:|---:|
| `net/minecraft` | 4,650 | 546,137 |
| `org/bukkit` | 776 | 84,920 |
| `ca/spottedleaf` (Moonrise) | 473 | 59,257 |
| `io/papermc` (Paper/Folia) | 502 | 43,259 |
| Spigot, Mojang, misc. | 84 | 10,485 |
| **Total** | **6,485** | **744,058** |

Plus 114 dependency jars (Netty, Guava, Adventure, Log4J, ASM, etc.).

## Hard constraints

1. **JVM bytecode needs a JVM.** Bukkit/Paper plugins are compiled against Java
   classes and frequently use reflection. A Rust-only executable cannot load
   them. The only realistic way to keep existing `.jar` plugins working is an
   embedded JVM (this repo) or a separate JVM process with IPC.
2. **NMS plugins need Java classes.** A plugin calling
   `CraftWorld.getHandle()` or `net.minecraft.*` expects those classes to exist
   and to operate on live server state. Supporting that means writing Java shim
   classes that forward to Rust, one API at a time. Public-API plugins are the
   tractable target.
3. **Minecraft parity is behavioral, not just structural.** The port must match
   packet formats, world format, RNG, entity behavior, redstone, and region
   scheduling. This needs differential tests against the Java server.

## Architecture decision

Rust owns:

- process lifecycle and configuration
- the authoritative tick loop and region scheduler
- networking/protocol
- world storage, chunk system, lighting
- entity/gameplay systems as they are ported

The embedded JVM owns:

- plugin class loading and lifecycle
- the Bukkit/Paper API surface plugins compile against
- event listener registration and dispatch
- adapters that translate API calls into JNI calls on the Rust core

The boundary is a typed command/event protocol, not ad-hoc reflection. The
current proof uses `nativePing`; the next step is to formalize the ABI.

## Milestones

### M0 — Decompile and inventory (done)

- Reconstruct `folia-1.21.11.jar` from the Paperclip patch.
- Decompile with Vineflower into `../decompiled`.
- Record package/file/LOC inventory.
- Known decompiler failure: one method in
  `ChunkUpgradeGenericStatusTask.run` is emitted as bytecode comments.

### M1 — Rust host + plugin bridge (done)

- Embed the JVM via JNI.
- Load real `plugin.yml` + `JavaPlugin` jars.
- Paper-compatible `ConfiguredPluginClassLoader`.
- Lifecycle and logger.
- Scheduler driven by Rust ticks; the task table and cancellation live in Rust.
- `@EventHandler` dispatch for host-fired events.
- JNI round trip proven.

Acceptance: `cargo run` loads `HelloPlugin.jar`, enables it, receives a native
call, dispatches `PluginEnableEvent`, schedules a repeating task into the Rust
task table, ticks it, and disables it.

### M2 — Rust core primitives (done)

- VarInt/VarLong, packet framing, handshake/status/ping.
- Live status server with JSON, ping/pong, legacy ping and offline-mode login.
- NBT reader/writer with gzip/zlib.
- Anvil `.mca` parser/writer with gzip/zlib/uncompressed chunks.
- Chunk section decoder with block-state and biome palettes.
- Regionized scheduler with chunk→region isolation and Bukkit-style delays.
- Login/configuration/play handshake using captured 1.21.11 registry data, with
  live `ClientboundLevelChunkWithLight` streaming from `WorldStorage`/flat
  chunks and dynamically computed heightmaps/sky/block light; offline mode only.
- Live player session: movement parsing, gzip-NBT playerdata, keepalives and
  `PlayerJoinEvent` through the plugin bridge.
- Hotbar/full inventory: 36 slots + cursor, selected slot and stacks persisted,
  container content/slot packets, click and creative-slot updates, held block
  placement with consumption.
- Entity tracking: spawn/move/remove, AddEntity/RemoveEntities/PositionSync
  broadcasts.
- Entity physics: gravity/drag, axis-separated AABB collision against
  `WorldStorage`, ground detection and per-tick sync. Differential-tested
  against a captured Folia armor-stand fall (18 samples, all within `5e-4`,
  landing at `y=-60`).
- Overworld noise generation: Java-compatible `ImprovedNoise`/`PerlinNoise`/
  `NormalNoise`/`BlendedNoise`, MD5 positional seeds, the embedded
  `minecraft:overworld` density-function graph (splines, gradients, weird
  samplers), a full `NoiseBasedAquifer` port, exact multi-noise biome sampling,
  bedrock/deepslate surface
  gradients, and a 4x8x4 grid chunk generator. Differential-tested against a
  real Java oracle: 1225/1225 final-density grid values within `1e-9`, exact
  bedrock layers, 98.76% block-class agreement and 92.09% exact block names on
  sampled chunk columns.
- Block-update path: chunk mutation, heightmap/light recomputation, Anvil
  persistence, `ClientboundBlockUpdate`/`SectionBlocksUpdate` broadcast.
- Validated against real world data: `level.dat` parsed; nether region parsed
  242 chunks / 3,872 sections with zero errors; end region parsed 144 chunks /
  2,305 sections with zero errors.

Acceptance: `cargo test --workspace` passes 133 core unit tests + 35 core
integration tests + 62 math tests + 23 random tests (253 total), the status
probe returns the 1.21.11 JSON/pong, the join probe reaches Join Game and
streams 49 live chunks, a world without region data streams 49 generated noise
chunks, block mutation persists and broadcasts, hotbar selection changes the
placed block, inventory clicks/cursor sync, entity spawn/move/remove is
visible, entity gravity/AABB matches the Java trace, generated terrain matches
the Java density grid and bedrock layers, movement is persisted, and the read
examples parse real `.mca` / `level.dat` files.

### M3 — API surface breadth (next)

Add, in this order, with tests for each:

1. plugin commands (`plugin.yml` commands, `CommandMap`, `PluginCommand`) — done
2. `ServicesManager` registration/lookup
3. console/player command senders
4. `PlayerJoinEvent` with a minimal `Player` implementation — done; `PlayerQuitEvent` and richer player/world APIs still pending
5. `Server`/`World`/`Entity` read-only views backed by Rust state
6. inventory and item metadata — 36 slots + cursor, clicks/creative slots done; armor/offhand/containers/item components pending
7. configuration (`getConfig`, `saveDefaultConfig`)
8. permissions

Acceptance: a public-API plugin that registers a command, listens to join/quit,
reads config, and schedules tasks runs unchanged.

### M4 — Protocol and world vertical slices

Port bounded systems first, keeping the Java original as the oracle:

1. login/configuration/play protocol (encryption, compression, registries)
2. chunk ticket propagation and Moonrise chunk system I/O
3. world format read/write against the Rust Anvil/chunk modules
4. block/light updates — mutation, persistence, broadcast, break/place wired; inventory/redstone triggers pending
5. entity tracking - spawn/move/remove and gravity/AABB physics done; AI/damage/range logic pending
6. world generation - noise primitives, density router, aquifers, biomes, surface rules and cave/canyon carvers done; ores/features/structures pending
7. gameplay systems

Each slice gets a differential harness: run the same input against the Java
implementation and the Rust implementation, compare serialized output.

### M5 — NMS shim layer

Generate Java shims for the NMS classes most used by plugins (`CraftWorld`,
`CraftPlayer`, `CraftEntity`, `CraftItemStack`, ...). Each shim forwards to Rust
and must fail loudly for unimplemented operations rather than silently lie.

Plugins that use arbitrary reflection over `net.minecraft.*` will still require
the corresponding classes to exist. This is the long tail and should be driven
by an actual plugin corpus, not by guessing.

### M6 — Parity and performance

- differential test suite over protocol/world/gameplay
- region-thread stress tests
- plugin compatibility harness against a curated plugin set
- profiling and allocation budgets

## Non-goals for the near term

- claiming 1:1 behavior before differential tests exist
- supporting every NMS/reflection plugin
- replacing the JVM for plugin execution

## Rule for porting code

Do not translate decompiled Java line-by-line. Use it as a specification:

1. identify the behavior and edge cases
2. write Rust tests that encode those cases
3. implement the Rust version
4. run the differential test against the Java oracle
5. delete the Java-side code only after the Rust path is authoritative

This keeps the port reviewable and prevents "looks the same, behaves
differently" regressions.


