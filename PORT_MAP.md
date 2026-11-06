# Decompiled Java → Rust port map

Reference tree: `../decompiled` (6,485 files, 744,058 lines).
Rust tree: `crates/folia-core`.

| Decompiled Java | Rust module | Status |
|---|---|---|
| `net/minecraft/nbt/*` | `folia-core/src/nbt.rs` | Ported: all tag types, big-endian, gzip/zlib |
| `net/minecraft/nbt/SnbtParser`, `TagParser` | `folia-core/src/snbt.rs` | Ported subset: compounds/lists/typed arrays/escapes |
| `net/minecraft/network/Varint21FrameDecoder`, VarInt helpers | `folia-core/src/varint.rs`, `protocol.rs` | Ported: VarInt/VarLong, packet framing, handshake/status/ping |
| `net/minecraft/world/level/chunk/storage/RegionFile` | `folia-core/src/anvil.rs` | Ported: location/timestamp tables, chunk records, compression |
| `net/minecraft/world/level/chunk/PalettedContainer`, `LevelChunkSection` | `folia-core/src/chunk.rs` | Ported: block/biome palettes, packed indices, section lookup; optional `SkyLight`/`BlockLight` layers now round-trip through NBT |
| `net/minecraft/core/BlockPos`, `Direction`, `SectionPos`, `world/phys/Vec3`, `AABB`, `util/Mth` | `folia-math` | Ported by math task |
| `net/minecraft/util/RandomSource`, `LegacyRandomSource`, `XoroshiroRandomSource` | `folia-random` | Ported, Java-differential sequences |
| `net/minecraft/world/level/storage/LevelData`, `PrimaryLevelData` | `folia-core/src/level.rs` | Ported subset: seed, spawn, version, game type |
| Anvil world directory + dimension layout (`RegionFileStorage`) | `folia-core/src/world.rs` | Ported: chunk load/save, region cache, overworld/nether/end paths |
| `server.properties` / `java.util.Properties` | `folia-core/src/config.rs` | Ported |
| Login packet compression | `folia-core/src/compression.rs`, `protocol.rs` | Ported primitives |
| `io/papermc/paper/threadedregions/ThreadedRegionizer`, `RegionizedTaskQueue` | `folia-core/src/region.rs` | Ported: chunk→region mapping, per-region queues, delays/periods/cancel |
| Server list ping / status | `folia-core/src/status.rs` | Ported: status JSON, ping/pong, live listener |
| `net/minecraft/server/MinecraftServer` tick loop | `crates/folia-server/src/jvm.rs` | Partial: Rust loop drives plugins and region tasks |
| Folia plugin schedulers | `java/bridge` + Rust `TASKS` table | Partial: sync tasks are Rust-owned; region/async/entity schedulers not yet |
| `net/minecraft/network/protocol/login/*`, configuration/play initial packets | `folia-core/src/join.rs` + `assets/join_*.bin` | Login → configuration → Join Game; captured registries, live chunk stream |
| `net/minecraft/network/protocol/game/ClientboundLevelChunkWithLightPacket` | `folia-core/src/chunk_packet.rs` | Live chunk packets from `WorldStorage`/flat chunks with dynamic heightmaps and light; stored/computed sky+block layers take precedence over the height fallback |
| Player session / movement / playerdata | `folia-core/src/player.rs` | Movement packet parsing, gzip-NBT persistence, join session |
| `ClientboundBlockUpdatePacket`, `ClientboundSectionBlocksUpdatePacket` | `folia-core/src/block_update.rs` | Chunk mutation, heightmap/light recompute, persistence, broadcast, ServerboundPlayerAction/UseItemOn |
| `ClientboundLightUpdatePacket` | `folia-core/src/block_update.rs` | Whole-chunk sky+block light data after a block edit; incremental section-only updates not ported |
| Hotbar/full inventory and cursor | `folia-core/src/inventory.rs`, `player.rs` | 36 slots + cursor, container content/slot packets, clicks, creative slots, NBT persistence, placement consumption |
| Entity tracking (`ClientboundAddEntity`, `RemoveEntities`, `EntityPositionSync`) | `folia-core/src/entity.rs` | Spawn/move/remove store, broadcasts, Java-matched packets |
| Entity tick physics | `folia-core/src/entity.rs` | Gravity/drag, axis-separated AABB collision against `WorldStorage`, ground detection, per-tick sync |
| `synth/NormalNoise`, `PerlinNoise`, `ImprovedNoise`, `BlendedNoise` | `folia-core/src/worldgen/noise.rs` | Java-matched noise primitives, positional seed factories, MD5 name hashing |
| Overworld density functions / noise router | `folia-core/src/worldgen/density.rs` | Embedded `worldgen.json`; noise, splines, gradients, range/min/max, markers, weird sampler |
| `NoiseBasedAquifer` | `folia-core/src/worldgen/aquifer.rs` | Fluid/noise grid, pressure blending, water/lava selection, deep-dark rules |
| `MultiNoiseBiomeSource` | `folia-core/src/worldgen/biome.rs` | Biome parameter list, climate quantization, minimum-fitness search; 1536/1536 Java quart samples exact |
| `BiomeManager` fuzzy zoom | `folia-core/src/worldgen/biome_zoom.rs` | SHA-256 seed obfuscation, LCG fiddled-distance search over eight quart corners, `biome_at` against `BiomeSampler` |
| `NoiseBasedChunkGenerator` base terrain | `folia-core/src/worldgen/generator.rs` | 4x8x4 density grid, trilinear interpolation, aquifer substance, bedrock/deepslate gradients, simple surface, sea-level water |
| `BuiltInRegistries` / `RegistryDataLoader` block, item, entity type, biome registries | `folia-core/src/registry.rs` + `assets/*.tsv` | 29,671 block states, 1,166 block defaults, item/entity/biome IDs; name and property lookups |
| `net/minecraft/tags/*`, `data/minecraft/tags/**` | `folia-core/src/tags.rs` + `assets/tags/*.tsv` | 9 registries, 602 non-empty tags (604 files) / 7,208 flattened members; nested `#tag` refs resolved by `scripts/extract_tags.ps1` |
| `net/minecraft/world/level/levelgen/feature/*` definitions | `assets/feature_data.json` | Extracted (224 configured / 258 placed / 65 biome lists) |
| `Feature`, `ConfiguredFeature`, `PlacedFeature`, `FeaturePlaceContext`, `GenerationStep.Decoration` | `folia-core/src/worldgen/feature/mod.rs` | Framework, typed loading, 11-step ordering, deterministic replay; `no_op` implemented, other feature types explicitly unsupported |
| `placement/*` modifiers, `IntProvider`, `HeightProvider`, `VerticalAnchor` | `folia-core/src/worldgen/feature/placement.rs` | Typed pipeline; 6 world-free modifiers plus `BiomeFilter` (via `PlacementWorld`) execute with vanilla draw order (667/888 shipped), the rest report `PlacementFailure` |
| `net/minecraft/world/level/lighting/*`, `chunk/DataLayer`, `ChunkSkyLightSources` | `folia-core/src/light.rs` | Partial: `DataLayer` nibble storage, per-column sky sources, sky/block BFS increase propagation, one-block neighbour border; decrease queues, generated opacity/emission tables, and Java light fixtures still open |
| `net/minecraft/world/entity/*` | `folia-core/src/entity.rs` | Partial: base entity physics for tracked entities; AI/goals/damage not ported |
| `net/minecraft/world/level/block/*` redstone/gameplay | — | Not ported |

## Verification evidence

- `cargo test --workspace` → 217 core unit + 45 core integration + 62 math +
  27 random tests (351 total), 0 failed (2026-11-06).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`
  are clean (2026-11-06).
- Tag tables cover 602 non-empty tags (604 files) / 7,208 flattened members
  across 9 registries; `tests/tags.rs` checks membership, nested flattening,
  exact counts, and unknown keys.
- Status server responds to status/ping, legacy ping, and login.
- End-to-end login probe: `cargo run -p folia-core --example join_probe -- 127.0.0.1:25565`
  prints `login accepted`, `configuration complete`, `Join Game received`,
  49 live chunk packets, a live position packet, and sends a movement packet.
- Server log: `streamed 49 live chunks around (0, -1)` and
  `chunk source: 49 from WorldStorage, 0 generated flat chunks`.
- Differential chunk test: generated heightmaps and light bytes for the real
  chunk `(0,-1)` are byte-identical to the captured Java packet
  (`tests/world_chunk_packet.rs`), including when the light payload is built
  from the decoded `SkyLight`/`BlockLight` NBT layers.
- Lighting tests (`tests/lighting.rs`): nibble/NBT round-trip, sky falloff
  under a roofed shaft, torch block-light radius, unsupported emitter, packet
  masks vs stored arrays, neighbour torch across a chunk border, and a block
  edit that persists/reloads block light.
- `read_player` prints the persisted position after the probe:
  `pos=(12.50, -60.00, 8.25) on_ground=true`.
- `PlayerJoinEvent` is delivered to the Java bridge:
  `PlayerJoinEvent: RustProbe (...)` in the plugin log.
- Block-update packet bodies match Java 1.21.11 codecs:
  `000000400000300201` (block update) and `00000000000fffff01b222`
  (section update), covered by `block_update::tests`.
- Inventory bodies match Java: held slot `02`, stone stack `0001010000`,
  container slot body `0000002401010000`; item IDs are stone 1, grass 27,
  dirt 28.
- Entity packet bodies match Java 1.21.11:
  add `2a000000...0000`, remove `012a`, sync `2a401000...0001`;
  armor stand entity type ID is 5.
- End-to-end entity test: spawn armor stand 9001, receive add id 1, move it and
  receive `EntityPositionSync` id 35, then remove it and receive
  `RemoveEntities` id 75.
- Differential entity physics: a real Folia armor-stand fall from `y=50` was
  captured (`java-tools/EntitySpawn`, `capture/play-051-*`). All 18 Java
  position samples (ticks 8..59) match Rust `tick_physics` within `5e-4`, and
  both land at `y=-60` on the grass block at `y=-61`
  (`tests/entity_physics.rs`).
- `cargo run -p folia-core --example physics_trace` prints the Rust trace:
  `tick 8: y=47.305439` … `tick 59: y=-47.520416`, then
  `landed: y=-60.000000 on_ground=true`.
- The join test also spawns a stand at `y=50`, ticks `entity::tick_all()` 90
  times through `WorldStorage`, and receives an on-ground position sync at
  `y=-60`.
- Java worldgen oracle (`java-tools/WorldGenDump`, normal overworld seed 12345):
  `NormalNoise`/`BlendedNoise` match Rust at 16 sample points within `1e-12`.
- The full `minecraft:overworld` density router matches Java at temperature,
  vegetation, continents, erosion, ridges and final-density sample points
  within `1e-9` (`worldgen/density.rs` tests).
- All 1225 `final_density` values on the chunk `(0,-1)` 4x8x4 interpolation grid
  match Java within `1e-9` (`worldgen/generator.rs` test,
  `assets/java-density-grid.txt`).
- Generated chunk `(0,-1)` uses the noise aquifer, surface rules and carvers: block-class agreement with the Java chunk is 98.76% (6068/6144 sampled positions), exact block names are 92.09% (5658/6144), and surface heights differ by 0.62 blocks on average. Bedrock layers match Java exactly at all 16 sampled columns (`assets/java-chunk-0--1.txt`). Multi-noise biome sampling matches Java at all 1536 sampled quart cells (`assets/java-biome-samples.txt`).
- `tests/worldgen_join.rs` logs in with no region directory, streams 49
  generated terrain chunks, and checks the palette contains bedrock.
- Live server startup logs `[rust] noise world generator ready (seed=0)`.
- Join test broadcasts a `ClientboundBlockUpdate` and the client receives id 8.
- End-to-end break/place test: the client sends `ServerboundPlayerAction` and
  `ServerboundUseItemOn`; it receives block id 8 and light id 47, and the
  persisted chunk is air at the broken position and stone at the placed
  position.
- Block mutation test persists a stone block through `WorldStorage`, reloads it,
  and verifies heightmap/light bytes changed.
- Legacy pre-1.7 `0xFE` ping returns a UTF-16BE server list response.
- Parallel region ticking (`tick_parallel`) runs independent region queues on
  worker threads and is covered by a concurrency test.
- Generated chunk → `encode_chunk` → Anvil write → Anvil read → `decode_chunk`
  round trip preserves blocks and biomes (`tests/world_round_trip.rs`).
- Real world validation:
  - `read_level` parses a real `level.dat` (DataVersion 4788).
  - `read_region` parsed a nether region with 242 chunks / 3,872 sections and
    zero errors, plus an end region with 144 chunks / 2,305 sections and zero
    errors after handling sections that omit `block_states`.
- Plugin bridge: `cargo run` loads `HelloPlugin.jar`, dispatches
  `PluginEnableEvent`, registers and executes the `/hello` command, schedules a
  repeating task into the Rust task table, and receives the JNI `nativePing`
  round trip.

## Porting rule

Use the decompiled code as a specification, not as source to transliterate.
Each module gets Rust tests first, then a differential check against the Java
behavior where practical. Java remains only in `java/bridge` for the plugin
compatibility boundary.



