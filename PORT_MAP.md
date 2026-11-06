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
| Player physics / movement validation | `folia-core/src/player_physics.rs` | AABB, gravity/drag, per-state friction/speed/jump factors, jump, step-up, fall distance/damage, collision reconciliation; full-cube collision only, and water/ladders/cobwebs/elytra and other special movement remain unported |
| `ClientboundBlockUpdatePacket`, `ClientboundSectionBlocksUpdatePacket` | `folia-core/src/block_update.rs` | Chunk mutation, heightmap/light recompute, persistence, broadcast, ServerboundPlayerAction/UseItemOn; edits now queue six neighbour updates and drain shape/support changes |
| `ServerPlayerGameMode`, `Player.getDestroySpeed`/`hasCorrectToolForDrops`, `BlockBehaviour.getDestroyProgress`, `Tool`/`ToolMaterial`, `Item.mineBlock`/`ItemStack.hurtAndBreak` | `folia-core/src/block_breaking.rs` | Partial: start/stop/abort dig state, 0.7 stop threshold + delayed destroy, hardness/tool/tier formula, efficiency/haste/fatigue and water/air penalties, sword/shears rules, harvest gating from block tags, deterministic common-block drops with a loot hook, durability/item break; loot tables, item entities, overlay packet, Unbreaking/Fortune/Silk Touch and adventure `can_destroy` remain |
| `ClientboundLightUpdatePacket` | `folia-core/src/block_update.rs` | Whole-chunk sky+block light data after a block edit; incremental section-only updates not ported |
| `Block.UPDATE_*`, `BlockBehaviour.UPDATE_SHAPE_ORDER`, `NeighborUpdater` | `folia-core/src/tick.rs` | Partial: vanilla flag values plus an internal shape bit, chunk-grouped deterministic neighbour queue, six-direction enqueue, shape/support checks, 512-chain limit |
| `ScheduledTick`, `TickPriority`, `LevelTicks`, `LevelChunkTicks` | `folia-core/src/tick.rs` | Partial: trigger/priority/sub-tick ordering, delay, dedup, cancellation between ticks; no NBT persistence, region ownership, or cancel-during-tick |
| `ServerLevel.tickChunk`, `LevelChunk.tickBlocks`, `RandomTick` | `folia-core/src/tick.rs` | Partial: 3 samples per 16x16x16 section from a deterministic seed/chunk/tick hash; Java level-RNG sequence and ticking-block lists not matched |
| `FallingBlock`, `VegetationBlock`, `GrassBlock`, `SpreadingSnowyDirtBlock` | `folia-core/src/tick.rs` | Partial: falling sand/gravel/concrete powder/anvils move one block per scheduled tick; plant support from `dirt`/farmland and double-plant halves; grass spread; leaf/crop/redstone/fluid/piston behavior remains explicit unsupported |
| Hotbar/full inventory and cursor | `folia-core/src/inventory.rs`, `player.rs` | 36 slots + cursor, container content/slot packets, clicks, creative slots, NBT persistence, placement consumption, inventory merge/overflow for block drops |
| `ItemStack`, `DataComponentMap`/`Patch`, `DataComponents`, `HashedStack` | `folia-core/src/item_components.rs` | Partial: insertion-ordered component patch, NBT + network subset (custom/item name, lore, damage/max damage/max stack size/repair cost, enchantments, unbreakable, custom data, container, food, rarity), opaque unknown NBT; attribute/potion/tool network and component hashing remain |
| Entity tracking (`ClientboundAddEntity`, `RemoveEntities`, `EntityPositionSync`) | `folia-core/src/entity.rs` | Spawn/move/remove store, broadcasts, Java-matched packets |
| Entity tick physics | `folia-core/src/entity.rs` | Gravity/drag, axis-separated AABB collision against `WorldStorage`, ground detection, per-tick sync |
| `synth/NormalNoise`, `PerlinNoise`, `ImprovedNoise`, `BlendedNoise` | `folia-core/src/worldgen/noise.rs` | Java-matched noise primitives, positional seed factories, MD5 name hashing |
| Overworld density functions / noise router | `folia-core/src/worldgen/density.rs` | Embedded `worldgen.json`; noise, splines, gradients, range/min/max, markers, weird sampler |
| `NoiseBasedAquifer` | `folia-core/src/worldgen/aquifer.rs` | Fluid/noise grid, pressure blending, water/lava selection, deep-dark rules |
| `MultiNoiseBiomeSource` | `folia-core/src/worldgen/biome.rs` | Biome parameter list, climate quantization, minimum-fitness search; 1536/1536 Java quart samples exact |
| `BiomeManager` fuzzy zoom | `folia-core/src/worldgen/biome_zoom.rs` | SHA-256 seed obfuscation, LCG fiddled-distance search over eight quart corners, `biome_at` against `BiomeSampler` |
| `NoiseBasedChunkGenerator` base terrain | `folia-core/src/worldgen/generator.rs` | 4x8x4 density grid, trilinear interpolation, aquifer substance, bedrock/deepslate gradients, simple surface, sea-level water |
| `BuiltInRegistries` / `RegistryDataLoader` block, item, entity type, biome registries | `folia-core/src/registry.rs` + `assets/*.tsv` | 29,671 block states, 1,166 block defaults, item/entity/biome IDs; name and property lookups |
| `BlockBehaviour`, `BlockStateBase` per-state metadata | `folia-core/src/block_behaviour.rs` + `assets/block_state_behaviour.tsv.gz` | 29,671 per-state rows (air/motion/solid/collision, light emission/block, random tick, occlusion, replaceable, hardness/blast, friction/speed/jump, fluid); consumed by lighting and player physics; ticking/breaking consumers still partial |
| `net/minecraft/tags/*`, `data/minecraft/tags/**` | `folia-core/src/tags.rs` + `assets/tags/*.tsv` | 9 registries, 602 non-empty tags (604 files) / 7,208 flattened members; nested `#tag` refs resolved by `scripts/extract_tags.ps1` |
| `world/level/storage/loot/*` (`LootTable`, `LootPool`, entries, providers, functions, conditions) | `folia-core/src/loot.rs` + `assets/loot_tables.json` | Partial: 1,324 tables typed and rollable in Java order, including weighted pools, alternatives/sequence/group, nested tables, item/tag/empty/dynamic entries, 20 function kinds, and all shipped conditions; unknown payloads fail closed and are reported. Block/entity drop wiring remains |
| `world/item/crafting/*` / `data/minecraft/recipe/**` | `folia-core/src/loot.rs` + `assets/recipes.json` | Partial: 1,470 recipes parsed (shaped, shapeless, four cooking types, stonecutting, smithing transform/trim, transmute, decorated pot, 13 special types), typed item/tag/alternatives ingredients and component-bearing results; grid/container matching and menus remain |
| `net/minecraft/world/level/levelgen/feature/*` definitions | `assets/feature_data.json` | Extracted (224 configured / 258 placed / 65 biome lists) |
| `Feature`, `ConfiguredFeature`, `PlacedFeature`, `FeaturePlaceContext`, `GenerationStep.Decoration` | `folia-core/src/worldgen/feature/mod.rs` | Framework, typed loading, 11-step ordering, deterministic replay, inline nested placed features, default `FeatureLevel::heightmap`, scheduled-tick and block-entity region sinks; 171/224 configured features dispatch as supported |
| `placement/*` modifiers, `IntProvider`, `HeightProvider`, `VerticalAnchor` | `folia-core/src/worldgen/feature/placement.rs` | Typed pipeline; all 888 shipped modifiers execute with vanilla draw order, including every heightmap, block predicate, environment scan, layer/noise count, surface-relative filter, and Gaussian offset |
| `RandomPatchFeature`, `SimpleBlockFeature`, `SimpleRandomSelectorFeature`, `BlockPileFeature`, `BlockColumnFeature`, `MultifaceGrowthFeature`, `VegetationPatchFeature`, `WaterloggedVegetationPatchFeature`, `stateproviders/*` | `folia-core/src/worldgen/feature/vegetation.rs` | Ported: random patch/flower, simple block (including `schedule_tick`), simple random selector, pile, column, multiface, vegetation and waterlogged patches; simple/weighted/randomized-int/noise/noise-threshold/dual-noise/rotated providers; predicate-only nested placements; Java HashSet order for patch vegetation |
| `SeagrassFeature`, `KelpFeature`, `SeaPickleFeature`, `CoralFeature`/`CoralTreeFeature`/`CoralClawFeature`/`CoralMushroomFeature`, `BambooFeature`, `UnderwaterMagmaFeature` | `folia-core/src/worldgen/feature/aquatic.rs` | Ported: short/tall seagrass, kelp head/body, pickles, three coral shapes with data-ordered tag picks/fans/wall fans, bamboo leaf stages, underwater magma column scan; block survival uses the shared full-cube approximation |
| `VinesFeature`, `TwistingVinesFeature`, `WeepingVinesFeature` | `folia-core/src/worldgen/feature/vines.rs` | Ported: face attachment in `Direction.values()` order, twisting/weeping spread scans, head/body columns and ages; the weeping column helper is reused by huge fungus |
| `HugeBrownMushroomFeature`, `HugeRedMushroomFeature`, `HugeFungusFeature`, `GlowstoneFeature`, `BlockBlobFeature`, `FillLayerFeature`, `NetherForestVegetationFeature`, `RandomBooleanSelectorFeature` | `folia-core/src/worldgen/feature/mushroom.rs` | Ported: huge mushroom caps/trunks with connection properties, huge crimson/warped fungus stem/hat/decor plus weeping vines, glowstone blob, `forest_rock`, fill layer, nether vegetation, mushroom-island boolean selector (empty nested placement only) |
| `TreeFeature`, trunk/foliage/root placers, tree decorators | `folia-core/src/worldgen/feature/tree.rs` | Ported straight/dark-oak/forking/fancy/bending/giant/mega-jungle/cherry/mangrove trunks and all shipped foliage; decorators include leaf litter, beehive occupants, vines, cocoa, alter-ground, attached leaves/logs, creaking hearts, and pale moss with the embedded `pale_moss_patch` |
| `structure/templatesystem/StructureTemplate`, `StructurePlaceSettings`, `Rotation`, `Mirror` | `folia-core/src/worldgen/structure/template.rs` | Partial: gzip structure NBT (palettes, blocks, block-entity NBT, entities), mirror/rotation/pivot/origin transforms, `FeatureLevel` block writing; starts, pieces, structure sets/references, processors, template pools, jigsaw, random palette selection, and entity spawning are not ported |
| `net/minecraft/world/level/lighting/*`, `chunk/DataLayer`, `ChunkSkyLightSources` | `folia-core/src/light.rs` | Partial: `DataLayer` nibble storage, per-column sky sources, sky/block BFS increase propagation, one-block neighbour border, and per-state emission/attenuation from `block_behaviour`; decrease queues, incremental updates, and Java light fixtures still open |
| `OreVeinifier` | `folia-core/src/worldgen/ore_vein.rs` | Standalone copper/iron vein pass over the noise router (toggle/ridged/gap, granite/tuff filler, raw-ore chance); generator wiring still pending |
| `SimplexNoise`, `PerlinSimplexNoise`, `Biome.BIOME_INFO_NOISE`/`FROZEN_TEMPERATURE_NOISE` | `folia-core/src/worldgen/simplex.rs` | Ported with Java-pinned reference values; used by noise-count placement and the frozen-ocean temperature modifier |
| `Biome.getTemperature`, `TemperatureModifier.FROZEN` | `folia-core/src/worldgen/biome_climate.rs` | 65 extracted biome base temperatures/modifiers; frozen-ocean iceberg melt now uses the exact noise branch |
| `net/minecraft/world/entity/*` | `folia-core/src/entity.rs` | Partial: base entity physics for tracked entities; AI/goals/damage not ported |
| `net/minecraft/world/level/block/*` redstone/gameplay | — | Not ported |

## Verification evidence

<<<<<<< HEAD
- `cargo test --workspace` → 217 core unit + 45 core integration + 62 math +
  29 random tests (426 total), 0 failed (2026-11-06, last full gate before the
  block-behaviour slice). That slice adds 4 integration + 2 lib focused tests.
=======
- `cargo test --workspace` → 511 total, 0 failed (2026-11-06).
>>>>>>> task/loot
- P2.5 focused evidence: `worldgen::feature::vegetation` adds 8 unit tests and
  `tests/vegetation.rs` adds 15 integration tests covering shipped
  `patch_grass_plain`/`flower_plain`/`moss_patch`/`pale_moss_patch` parsing,
  random-patch success/failure, all provider draw orders, block pile/column,
  multiface attachment, vegetation and waterlogged patch behavior, Java
  HashSet ordering, and deterministic generation.
- P2.4/P2.5 feature-gap evidence: `tree.rs` unit tests cover the beehive
  occupant NBT (2–3 bees, `ticks_in_hive < 599`, `min_ticks_in_hive = 600`);
  `tests/vegetation.rs` adds 3 integration tests for pale oak pale-moss
  placement, pale forest flower tick scheduling, and deterministic dripleaf
  selector placement. 171/224 configured features dispatch as supported; 53
  remain explicit `Unsupported`.
- P2.5 aquatic/vine/mushroom evidence: `aquatic.rs` adds coral tag-order and
  horizontal-rotation unit tests, `vines.rs` adds a twisting/weeping column
  unit test, and `tests/aquatic.rs` adds 16 integration tests covering all
  shipped config IDs, short/tall seagrass pairs, kelp heads/bodies, sea pickles,
  coral tree/claw/mushroom, vine face attachment, twisting/weeping columns,
  huge brown/red mushroom and crimson/warped fungus shapes, glowstone, forest
  rocks, fill layers, nether vegetation, underwater magma, bamboo leaf stages,
  and generator decoration determinism.
- Parity after the aquatic slice: 98.71% block-class (6065/6144) / 92.02%
  exact-name (5654/6144); the one-block exact delta from 92.04% is a
  tall-seagrass pair on a column whose Rust terrain already differs from Java.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`
  are clean (2026-11-06).
- Item components (`tests/item_components.rs`): 17 integration + 8 unit tests
  cover NBT round-trip for every ported component, mixed-style lore wrapping,
  interleaved `!removed`/added order, unknown and malformed component
  preservation, container contents, network encode/decode of the ported
  subset, explicit omission reports for unported network codecs, merge/split
  with component equality and max stack size, creative assignment, selected
  item consumption, playerdata load/save, legacy `Count`, and pinned component
  registry ids. Stone/dirt packet bodies and the captured-world join test are
  unchanged.
- Tag tables cover 602 non-empty tags (604 files) / 7,208 flattened members
  across 9 registries; `tests/tags.rs` checks membership, nested flattening,
  exact counts, and unknown keys.
- Loot/recipe catalogs (`tests/loot.rs`): 9 focused tests cover the pinned
  1,324/1,470 indexes, diamond-ore silk-touch alternatives, stone
  `survives_explosion`, weighted simple-dungeon counts, seeded determinism,
  opaque function/condition preservation, all shipped recipe serializers, and
  component-bearing `set_damage`/`set_name`; `tests/assets.rs` pins both
  catalogs' byte lengths and FNV hashes.
- Structure templates (`tests/structure_template.rs`): 13 focused tests load
  four shipped gzip templates (`empty.nbt`, igloo middle, desert camel spawn,
  nether fossil 8), pin sizes/palettes/block counts/block-entity and entity
  NBT, check all four rotations and both mirrors with hand-computed positions
  and non-square extent swaps, verify pivot/origin translation and
  facing/axis/jigsaw-orientation state transforms, place into a mock
  `FeatureLevel`, and reject malformed NBT.
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
- Block-behaviour integration (`tests/block_behaviour_integration.rs`): 4 tests
  cover generated torch/glowstone/lava emission, glass transmission vs water
  attenuation, sky light under a stone overhang lit through a glass shaft, and
  `StoragePhysicsWorld` ice/slime/blue-ice friction, soul-sand speed factor,
  honey jump factor, water/plant passability, and `blocks_motion`/`is_solid`
  flags. The captured `world_chunk_packet` body stays byte-identical.
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
- Player physics: focused tests cover the standing/sneaking/swimming hitboxes,
  gravity/drag, terminal velocity, the 20-tick walk displacement
  (`4.140388` blocks from rest within `1e-4`), wall collision, half-block step
  up, jump apex `1.252203`, fall damage (`10` blocks -> `7`), legal no-correction
  walking, wall correction, and burst destination settlement
  (`tests/player_physics.rs`, 12 tests).
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
- Generated chunk `(0,-1)` uses the noise aquifer, surface rules and carvers: block-class agreement with the Java chunk is 98.71% (6065/6144 sampled positions), exact block names are 92.02% (5654/6144), and surface heights differ by 0.62 blocks on average. Bedrock layers match Java exactly at all 16 sampled columns (`assets/java-chunk-0--1.txt`). Multi-noise biome sampling matches Java at all 1536 sampled quart cells (`assets/java-biome-samples.txt`).
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
- P3.10 tick slice: 11 `tick` unit tests cover vanilla flag values,
  neighbour dedup/flag merging/order, scheduled priority/sub-tick/delay/dedup/
  cancel, deterministic random positions, callback filtering, behavior
  classification, and priority clamping. 10 `tests/ticks.rs` tests cover the
  six-neighbour edit sequence, plant removal by support change, falling sand
  landing and support removal, scheduled delay/cancel, deterministic grass
  spread, unsupported leaf reporting, and deterministic repeated world ticks.
  `block_update` and `world` focused tests still pass; fmt and clippy are clean.
- Plugin bridge: `cargo run` loads `HelloPlugin.jar`, dispatches
  `PluginEnableEvent`, registers and executes the `/hello` command, schedules a
  repeating task into the Rust task table, and receives the JNI `nativePing`
  round trip.

## Porting rule

Use the decompiled code as a specification, not as source to transliterate.
Each module gets Rust tests first, then a differential check against the Java
behavior where practical. Java remains only in `java/bridge` for the plugin
compatibility boundary.



