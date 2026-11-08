# Folia-RS

A Rust server core with an optional embedded JVM plugin runtime. It boots a
Rust process and, only when `plugins/` contains `.jar` files, starts a JVM
in-process to load ordinary Bukkit/Paper plugins against the real `folia-api`
classes. With no plugin jars the server runs fully native and never loads Java.

> **Status: working foundation, not a finished port.** The decompiled Folia
> server is 6,485 files / 744,058 lines of Java. A literal 1:1 rewrite is a
> multi-year project. The Rust core now owns the process, tick loop, task
> scheduler, protocol primitives, NBT, Anvil storage, chunk decoding and the
> overworld generator; the optional embedded JVM exists only to run unmodified
> plugin jars.

## What works right now

### Rust core (`crates/folia-core`)

- VarInt/VarLong codecs with known-encoding tests.
- NBT reader/writer, including gzip and zlib containers.
- Minecraft packet framing, handshake/status/ping packets.
- Live server-list status server (status + ping/pong + legacy ping).
- Login → configuration → play handshake, with captured 1.21.11 registry data.
  With `online-mode=false`, a client reaches Join Game.
- Live chunk streaming: a view-distance-sized area is generated as
  `ClientboundLevelChunkWithLight` packets from `WorldStorage` chunks, centered
  on the player's chunk. Heightmaps and sky/block light are computed from the
  Rust chunk data.
- Real light storage (P3.5-P3.8 slice): vanilla 2048-byte nibble layers per
  section, `SkyLight`/`BlockLight` NBT round-trip, per-column sky sources with
  BFS under overhangs, block-light BFS over an explicit emitter table, stored
  layers preferred in chunk packets, and neighbour-aware relighting on edits.
  The captured `(0,-1)` packet fixture is still byte-identical.
- Overworld noise generation: the real `minecraft:overworld` noise router,
  splines, density functions, noise aquifer, surface-rule tree and cave/canyon
  carvers are embedded and evaluated in Rust. Missing chunks use a 4x8x4
  density grid and the full post-surface carving pass.
- Noise/density differential tests: `NormalNoise` and `BlendedNoise` match Java
  at 16 sample points within `1e-12`; all 1225 final-density grid values for
  chunk `(0,-1)` match Java within `1e-9`.
- Chunk differential: at 16 sampled Java chunk columns, bedrock layers match
  exactly, block-class agreement (solid/air/water/lava) is 98.75%, exact block
  names are 95.88%, and surface heights differ by 0.62 blocks on average. Exact
  names were 92.04% before ore/vein wiring.
- Multi-noise biome sampling: the real overworld parameter list and all 1536
  biome cells of chunk `(0,-1)` match the Java server exactly.
- Cave and canyon carvers: exact legacy seeding, the 17x17 neighbouring-start
  replay, shared carving masks and post-surface `topMaterial` replacement.
- Live player sessions: movement packets update position, gzip-NBT playerdata is
  saved under `run/playerdata`, and server keepalives are driven by the Rust
  session loop.
- Server-side player physics: vanilla walking/falling formulas, AABB collision
  against `WorldStorage`, jump/step-up, fall distance and damage, legal walking
  accepted without rubber-banding, and blocked/oversized moves corrected.
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
- Full 1.21.11 registries embedded from Java dumps: 29,671 block states
  (including properties) and 1,166 block defaults, item IDs, entity type IDs and
  biome IDs. Lookups resolve the exact property set and fall back to the
  block's default state, not to stone.
- Vanilla tag tables and all 23 synchronized dynamic registries are embedded:
  nine tag registries (602 non-empty tags / 7,208 flattened members) and
  compact id/name tables for biomes, damage types, enchantments, dimensions,
  paintings, trims, wolf variants, and the other configuration registries.
- Component-aware item stacks: `{item, count, components}` with NBT and network
  codecs for names, lore, damage, enchantments, attributes, containers,
  potions, food, tools, and opaque preservation of unknown components.
- Feature framework: `Feature`, `ConfiguredFeature`, `PlacedFeature`,
  `FeaturePlaceContext`, a chunk-bounded write region, and the 11 vanilla
  generation steps. `assets/feature_data.json` (224 configured features, 258
  placed features, 65 biome feature lists) is loaded and reference-validated;
  unported feature types fail loudly instead of no-oping. Placement modifiers
  are typed; all 888 execute in vanilla RNG order and none report an
  explicit failure. Ground-cover features include random patches/flowers,
  simple blocks, piles, columns, multiface growth, vegetation patches and
  waterlogged patches. Supported vegetation runs after carvers; supported trees
  include oak, birch, jungle, swamp oak, spruce, pine, dark oak, fancy oak,
  cherry, acacia, mega spruce/pine, mega jungle, jungle bush, azalea,
  mangrove, and fallen trees. Ore and scattered-ore configs (30 + 2) parse
  and place, but are not wired into chunk generation until feature-RNG parity.
  Attached-to-leaves decoration, randomized integer block states, weighted
  foliage providers, and waterlogged mangrove roots are supported.
- Structure-template foundation: gzip 1.21.11 structure NBT loading (palettes,
  blocks, block-entity NBT, entities), Java mirror/rotation/pivot/origin
  transforms, and placement through the feature write API. Recursive jigsaw
  placement and feature pools are wired into chunk generation; the shipped
  catalogs cover 1,202 templates (483 village, 528 non-village, 191 trial
  chamber), and Nether fortress / End city procedural pieces are ported. Terrain
  adaptation and exact cross-chunk height sampling remain.
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

- The JVM starts lazily only when `plugins/*.jar` exists; the core never needs
  Java.
- Real `.jar` plugins load with a Paper-compatible class loader.
- Plugin lifecycle: `onLoad` → `onEnable` → ticks → `onDisable`.
- Bukkit `@EventHandler` annotations are dispatched through a bridge event bus.
- `plugin.yml` commands are registered; the bridge exposes a dispatch entry
  point and executors use the normal Bukkit `PluginCommand` API. The native
  terminal console now supports `help`, `list`, `ops`, `op`, `deop`,
  `gamemode`, and `stop`; full command trees and Bukkit console dispatch are
  still pending.
- Bukkit scheduler tasks are registered over JNI; the task table, delays,
  periods and tick advancement live in Rust.
- JNI round trip: Java plugin → Rust `nativePing` → Java.
- Java plugin data folders are created under `run/<PluginName>`.

The included `HelloPlugin.jar` is built from the `java/example-plugin` sources
and exercises every one of those paths.

## Quick start

```powershell
cd folia-rs
cargo run --release        # playable speed: release noise generation is far faster
cargo run                  # debug build, fine for protocol tests but slow to generate
```

World generation is CPU-bound. Use `--release` for real play; the debug build
is typically several times slower. `view-distance` in `server.properties`
controls how far chunks stream (the current hard cap is 16); a larger radius
trades join/generation time for a wider visible world.

To use `.jar` plugins, build the compatibility layer once and put plugin jars
in `plugins/`:

```powershell
# compiles the bridge + example plugin and runs the Java API test
.\scripts\build-java.ps1 -LibrariesDirectory C:\Users\austr\Downloads\rustfol\libraries
$env:FOLIA_LIBRARIES = "C:\Users\austr\Downloads\rustfol\libraries"
cargo run
```

For a bounded demo run:

```powershell
$env:FOLIA_MAX_TICKS = "40"
cargo run
```

To run the end-to-end bridge self-test (world edit, player/item round trip, and
the three cancellable events):

```powershell
$env:FOLIA_BRIDGE_SELFTEST = "1"
$env:FOLIA_MAX_TICKS = "20"
$env:FOLIA_LIBRARIES = "C:\Users\austr\Downloads\rustfol\libraries"
cargo run -p folia-server
```

Query the live status server from Rust while it is running:

```powershell
cargo run -p folia-core --example status_probe -- 127.0.0.1:25565
```

`server.properties` controls the bind address, port, MOTD and player limit. The
`FOLIA_BIND` environment variable overrides the address for tests.

The first join of a world without a `level.dat` writes one (DataVersion 4788,
seed, spawn, overworld generator settings) and marks it `folia:written`, so a
fresh world reloads with the same seed and spawn. Generated terrain is saved
into `<level>/region/*.mca` and stamped `folia:written`. A level this server
created serves only stamped chunks, so leftover chunks from another source are
regenerated instead of hiding the terrain; a `level.dat` this server did not
create, or `FOLIA_LOAD_STORED=1`, serves every chunk in the region files (for
example, an existing world).

### Server listing vs joining

The Rust server accepts logins when `server.properties` has
`online-mode=false`. It replays captured 1.21.11 registry/configuration packets
and streams live chunks from `WorldStorage`, so a protocol client reaches Join
Game and receives generated chunk data.

This is an initial join path, not full gameplay yet:

- Online-mode encryption is not implemented; keep `online-mode=false`.
- Chunks are generated from `WorldStorage` with dynamic heightmaps and
  sky/block light; when no region directory exists, chunks come from the Rust
  overworld noise generator with aquifers, surface rules, caves/canyons and
  exact biomes. Block break/place updates and basic tracked-entity gravity are
  implemented; ore/scattered-ore generation, the vein filler, and supported
  vegetation are wired, while structures, full light propagation, redstone and
  AI are not.
- Player movement is parsed and persisted, but player physics and movement
  corrections are not simulated; only tracked entities are server-ticked.
- Void damage, death, and respawn are implemented; other damage sources,
  inventory drops, and full respawn-point rules are not.
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
cargo test --workspace    # 1194 tests
```

The baseline is clean: `cargo fmt --check` and
`cargo clippy --workspace --all-targets -- -D warnings` both pass with no
warnings.

Requirements:

- Rust 1.85+ (edition 2024)
- For the native server: nothing else.
- For `.jar` plugins only: JDK/JRE 21 with `jvm.dll` / `libjvm.so`, plus the
  reconstructed Folia dependency tree in `..\libraries`.

`JAVA_HOME` is auto-detected on Windows from common Java 21 install paths. Set
it explicitly for other layouts. If no Java is installed, remove the jars from
`plugins/` (or set `FOLIA_DISABLE_PLUGINS=1`) and the native server still runs.

## Architecture

```text
┌──────────────────────────── Rust process ────────────────────────────┐
│                                                                      │
│  folia-server (Rust)                                                 │
│    ├─ region scheduler ───────┐                                      │
│    ├─ status server ──────────┤                                      │
│    ├─ core: NBT/Anvil/chunks  │                                      │
│    └─ optional JNI bridge ────┼──────────────┐ (only with jars)      │
│                               │              │                       │
│  ┌────────────────────────────▼──────────────▼───────────────────┐  │
│  │ lazy embedded JVM (libjvm), started only for plugin jars      │  │
│  │   Bridge / PluginHost                                         │  │
│  │   PluginClassLoader (Paper ConfiguredPluginClassLoader)        │  │
│  │   BridgeServer / BridgeScheduler / BridgeEventBus              │  │
│  │   real folia-api classes + plugin jars                         │  │
│  └───────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

The server core is Rust. Java is confined to `java/bridge`, the compatibility
surface that plugins compile against, and is not started unless plugin jars are
present. No game/world logic is implemented in Java; those modules are ported
into `crates/folia-core`.

## Repository layout

| Path | Purpose |
|---|---|
| `crates/folia-core/` | Rust core: NBT, Anvil, chunks, protocol, region scheduler, config |
| `crates/folia-math/` | Coordinates, directions, vectors, AABBs, math helpers |
| `crates/folia-random/` | Legacy LCG and Xoroshiro128++ random sources |
| `crates/folia-server/` | Rust binary, native tick loop, status server, optional JVM bootstrap |
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
| `BlockBreakEvent` / `BlockPlaceEvent` / `PlayerInteractEvent` | Constructed, dispatched, cancellable; Rust host entry points exist, live `join.rs` wiring pending |
| `plugin.yml` commands + `PluginCommand` executors | Registered; dispatch hook available |
| `runTask*` sync scheduling | Works (Rust task table) |
| `World` / `Block` reads and edits | Partial: stored/cached overworld columns, highest-Y, biome, light; no on-demand generation |
| `Player` identity/vitals/game mode/teleport/messages | Partial: persisted through playerdata; game mode reaches the live session; other live sync pending |
| `PlayerInventory` / `ItemStack` (type, count, damage, name, lore, unbreakable) | Partial: slots 0-35; `new ItemStack(...)` works; armor/off-hand and enchant/PDC persistence pending |
| Services, entities, full `PluginManager` | Not yet |
| NMS / `CraftBukkit` / reflection plugins | Not yet; requires Java shims for those classes |

**Important:** a pure Rust binary cannot load JVM bytecode by itself. Keeping
`.jar` plugins working requires a JVM in the process. The embedded JVM is that
JVM; the Rust core is still the owner of the server.

## What is not done

- Live world simulation: the Rust noise generator produces overworld base
  terrain with aquifers, surface rules, cave/canyon carvers and exact
  multi-noise biomes. Ore/scattered ore, the copper/iron vein filler, and
  supported tree/vegetation features run; most other vegetation, structures and
  full light propagation are not ported; other feature types fail loudly, and
  some world-dependent placement modifiers are stubs. Entities are
  spawned/moved/removed, visible to clients and ticked with gravity/AABB
  collision, but there is no AI, damage or entity-tracking range logic.
  Redstone is not ported.
- Online-mode encryption and Mojang session authentication.
- Full player/world/entity API objects. The bridge now covers world/block
  access, player identity/vitals/game mode/teleport, storage-slot inventories,
  and basic item meta, but live-session sync, entities, armor/off-hand,
  enchantment/PDC persistence, services, and NMS shims remain open.
- The current API shim covers bootstrap, events, commands, scheduling, world
  blocks, players, and items; it is not yet a complete Bukkit surface.
- There is no full 1:1 parity test suite yet. See `PORTING.md` for the staged
  plan and `PORT_MAP.md` for what is already ported.
