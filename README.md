# Folia-RS

A Rust server core with an embedded JVM plugin runtime. It boots a Rust process,
starts a JVM in-process, and loads ordinary Bukkit/Paper `.jar` plugins against
the real `folia-api` classes.

> **Status: working foundation, not a finished port.** The decompiled Folia
> server is 6,485 files / 744,058 lines of Java. A literal 1:1 rewrite is a
> multi-year project. The Rust core now owns the process, tick loop, task
> scheduler, protocol primitives, NBT, Anvil storage and chunk decoding; the
> embedded JVM exists only to run unmodified plugin jars.

## What works right now

### Rust core (`crates/folia-core`)

- VarInt/VarLong codecs with known-encoding tests.
- NBT reader/writer, including gzip and zlib containers.
- Minecraft packet framing, handshake/status/ping packets.
- Live server-list status server (status + ping/pong + legacy ping).
- Login → configuration → play handshake, with captured 1.21.11 registry data.
  With `online-mode=false`, a client reaches Join Game.
- Live chunk streaming: a 7×7 area is generated as
  `ClientboundLevelChunkWithLight` packets from `WorldStorage` chunks, centered
  on the player's chunk. Heightmaps and sky/block light are computed from the
  Rust chunk data.
- Overworld noise generation: the real `minecraft:overworld` noise router,
  splines and density functions are embedded and evaluated in Rust. Missing
  chunks use a 4x8x4 density grid, the vanilla noise aquifer, random
  bedrock/deepslate vertical gradients and simple surface blocks.
- Noise/density differential tests: `NormalNoise` and `BlendedNoise` match Java
  at 16 sample points within `1e-12`; all 1225 final-density grid values for
  chunk `(0,-1)` match Java within `1e-9`.
- Chunk differential: at 16 sampled Java chunk columns, bedrock layers match
  exactly, block-class agreement (solid/air/water/lava) is 96.3%, exact block
  names are 89.0%, and surface heights differ by 1.81 blocks on average.
- Multi-noise biome sampling: the real overworld parameter list and all 1536
  biome cells of chunk `(0,-1)` match the Java server exactly.
- Live player sessions: movement packets update position, gzip-NBT playerdata is
  saved under `run/playerdata`, and server keepalives are driven by the Rust
  session loop.
- Block update path: mutate a chunk block, recompute its heightmaps/light,
  persist through `WorldStorage`, and broadcast `ClientboundBlockUpdate`,
  `ClientboundSectionBlocksUpdate` and `ClientboundLightUpdate`.
- Hotbar/full player inventory: 36 inventory slots (9 hotbar + 27 main) plus
  cursor, selected slot and item stacks persist in gzip-NBT playerdata,
  `ClientboundContainerSetContent`/`SetSlot`/`SetHeldSlot` are sent, and
  `ServerboundContainerClick`/`SetCreativeModeSlot` update stacks.
- Player block break/place: `ServerboundPlayerAction` breaks blocks and
  `ServerboundUseItemOn` places the selected held block at the targeted face,
  consuming one item and updating the container state.
- Entity tracking: add/remove entity packets, entity position sync, broadcast to
  sessions, and entity ID/type/UUID/position state.
- Server-side entity physics: gravity, drag, axis-separated AABB collision with
  `WorldStorage` blocks, ground detection and per-tick position sync. The armor
  stand fall is differential-tested against a captured Folia trace (18 samples,
  all within 5e-4, landing at `y = -60`).
- `PlayerJoinEvent` is forwarded from the Rust session to the Java plugin bridge.
- Anvil `.mca` region reader/writer with gzip/zlib/uncompressed chunks.
- Chunk section decoder: block-state and biome palettes, packed indices.
- Chunk encoder: generated chunks round-trip through Anvil storage.
- `level.dat` parser: data version, level name, spawn, game type, seed, version.
- `server.properties` parser: Java properties escapes/continuations + bind/MOTD.
- Zlib packet compression primitives with a decompressed-size limit.
- Regionized task scheduler with chunk→region mapping, delays, periods,
  cancellation and parallel region ticks (Folia's core isolation model).
- Legacy LCG and Xoroshiro128++ random sources with Java-differential sequences.
- Verified against real world files: `level.dat` and `.mca` regions parse with
  zero read/decode errors (including empty sections that omit `block_states`).

### Plugin translation layer (`java/bridge`, only where a JVM is required)

- Real `.jar` plugins load with a Paper-compatible class loader.
- Plugin lifecycle: `onLoad` → `onEnable` → ticks → `onDisable`.
- Bukkit `@EventHandler` annotations are dispatched through a bridge event bus.
- `plugin.yml` commands are registered and dispatched from Rust; executors use
  the normal Bukkit `PluginCommand` API.
- Bukkit scheduler tasks are registered over JNI; the task table, delays,
  periods and tick advancement live in Rust.
- JNI round trip: Java plugin → Rust `nativePing` → Java.
- Java plugin data folders are created under `run/<PluginName>`.

The included `HelloPlugin.jar` is built from the `java/example-plugin` sources
and exercises every one of those paths.

## Quick start

```powershell
cd folia-rs
.\scripts\build-java.ps1   # compiles the bridge + example plugin
cargo run                  # runs until Ctrl+C
```

For a bounded demo run:

```powershell
$env:FOLIA_MAX_TICKS = "40"
cargo run
```

Query the live status server from Rust while it is running:

```powershell
cargo run -p folia-core --example status_probe -- 127.0.0.1:25565
```

`server.properties` controls the bind address, port, MOTD and player limit. The
`FOLIA_BIND` environment variable overrides the address for tests.

### Server listing vs joining

The Rust server accepts logins when `server.properties` has
`online-mode=false`. It replays captured 1.21.11 registry/configuration packets
and streams live chunks from `WorldStorage`, so a protocol client reaches Join
Game and receives generated chunk data.

This is an initial join path, not full gameplay yet:

- Online-mode encryption is not implemented; keep `online-mode=false`.
- Chunks are generated from `WorldStorage` with dynamic heightmaps and
  sky/block light; when no region directory exists, chunks come from the Rust
  overworld noise generator with aquifers and bedrock/deepslate gradients.
  Block break/place updates and basic tracked-entity gravity are implemented,
  but the full vanilla surface-rule tree, carvers/caves, ores, features,
  biomes, light propagation, redstone and AI are not.
- Player movement is parsed and persisted, but player physics and movement
  corrections are not simulated; only tracked entities are server-ticked.
- `PlayerJoinEvent` is wired through a minimal player proxy; player/world/entity
  API objects beyond that are not complete.

Verify the full path with the Rust protocol client:

```powershell
cargo run -p folia-core --example join_probe -- 127.0.0.1:25565
```

It prints `login accepted`, `configuration complete`, `Join Game received`, and
the number of chunk packets received.

- Default bind: `0.0.0.0:25565` (IPv4 only).
- `server-ip=::` switches to IPv6-only. `localhost` resolves to `::1` first on
  this machine, so use `127.0.0.1:25565` or the printed LAN IP.
- A port conflict is now a hard error instead of silently switching to a random
  port.
- From another machine, allow the port through Windows Firewall (admin shell):

```powershell
New-NetFirewallRule -DisplayName "Folia-RS" -Direction Inbound -Protocol TCP -LocalPort 25565 -Action Allow
```

AVG Firewall is also installed and running here; if LAN clients still time out,
add the same TCP 25565 allow rule in AVG's firewall settings.

- Use the LAN address printed at startup, or `127.0.0.1:25565` on the same
  machine.

Run the core test suite:

```powershell
cargo test -p folia-core
```

Requirements:

- Rust 1.85+ (edition 2024)
- JDK 21 with `jvm.dll` / `libjvm.so`
- The reconstructed Folia dependency tree in `..\libraries` (created by the
  decompile/bootstrap step)

`JAVA_HOME` is auto-detected on Windows from common JDK 21 install paths. Set it
explicitly for other layouts.

## Architecture

```text
┌──────────────────────────── Rust process ────────────────────────────┐
│                                                                      │
│  folia-server (Rust)                                                 │
│    ├─ region scheduler ───────┐                                      │
│    ├─ status server ──────────┤                                      │
│    ├─ core: NBT/Anvil/chunks  │                                      │
│    └─ JNI bridge ─────────────┼──────────────┐                       │
│                               │              │                       │
│  ┌────────────────────────────▼──────────────▼───────────────────┐  │
│  │ embedded JVM (libjvm)                                         │  │
│  │   Bridge / PluginHost                                         │  │
│  │   PluginClassLoader (Paper ConfiguredPluginClassLoader)        │  │
│  │   BridgeServer / BridgeScheduler / BridgeEventBus              │  │
│  │   real folia-api classes + plugin jars                         │  │
│  └───────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

The server core is Rust. Java is confined to `java/bridge`, the compatibility
surface that plugins compile against. No game/world logic is implemented in
Java; those modules are ported into `crates/folia-core`.

## Repository layout

| Path | Purpose |
|---|---|
| `crates/folia-core/` | Rust core: NBT, Anvil, chunks, protocol, region scheduler, config |
| `crates/folia-math/` | Coordinates, directions, vectors, AABBs, math helpers |
| `crates/folia-random/` | Legacy LCG and Xoroshiro128++ random sources |
| `crates/folia-server/` | Rust binary, JVM bootstrap, tick loop, status server |
| `java/bridge/` | Bukkit/Paper API shim and plugin host (JVM boundary only) |
| `java/example-plugin/` | Demo plugin compiled against `folia-api` |
| `scripts/build-java.ps1` | javac/jar build for the bridge and demo plugin |
| `plugins/` | Drop plugin `.jar` files here |
| `run/` | Plugin data folders |
| `../decompiled/` | Full decompiled Folia 1.21.11 source (reference) |

See `PORT_MAP.md` for the decompiled-Java → Rust-module mapping.

## Plugin compatibility

| Plugin type | Status |
|---|---|
| Lifecycle + logger + `plugin.yml` | Works |
| Bukkit `@EventHandler` events fired by the host | Works (host bus) |
| `plugin.yml` commands + `PluginCommand` executors | Works (dispatched from Rust) |
| `runTask*` sync scheduling | Works (Rust task table) |
| Public API-only plugins | Partial; API surface grows incrementally |
| Services, inventories, players, worlds | Not yet |
| NMS / `CraftBukkit` / reflection plugins | Not yet; requires Java shims for those classes |

**Important:** a pure Rust binary cannot load JVM bytecode by itself. Keeping
`.jar` plugins working requires a JVM in the process. The embedded JVM is that
JVM; the Rust core is still the owner of the server.

## What is not done

- Live world simulation: the Rust noise generator produces overworld base
  terrain with aquifers, bedrock/deepslate gradients and exact multi-noise
  biomes, but the full vanilla surface-rule tree, carvers/caves, ores, features
  and structures are not ported. Entities are spawned/moved/removed, visible to
  clients and ticked with gravity/AABB collision, but there is no AI, damage or
  entity-tracking range logic. Redstone is not ported.
- Online-mode encryption and Mojang session authentication.
- Full player/world/entity API objects; only the minimal `PlayerJoinEvent`
  player proxy exists.
- The current API shim covers bootstrap, events, commands and scheduling only.
- There is no full 1:1 parity test suite yet. See `PORTING.md` for the staged
  plan and `PORT_MAP.md` for what is already ported.
