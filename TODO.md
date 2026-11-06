# Folia-RS Master TODO & Roadmap

> Last updated: 2026-11-06
> Current commit: see `git log` (all work is committed)
> Working tree: clean
> Test status: **611 passing** on the full merged gate (0 failed)
> Worldgen parity: **98.75% block-class / 95.80% exact-name** on the 16-column fixture
> Overall completion estimate: **~22%** (range 15–28% depending on metric)

This is the ordered plan from the completed foundation to a production-usable,
Folia-compatible Rust server that can run unmodified Bukkit/Paper plugins.

The important rule remains: **use the decompiled Java as a specification, not as
source to transliterate.** Every meaningful slice should get a Rust test and,
where possible, a differential test against the Java oracle.

## Status legend

- `[x]` done and verified
- `[~]` partial or landed but unverified end-to-end
- `[ ]` not started
- **S** = under 1 week, **M** = 1–3 weeks, **L** = 1–2 months,
  **XL** = 2–6 months for one experienced engineer. Sizes are relative only.

## Completion dashboard

| Area | Estimate | Notes |
|---|---:|---|
| Foundation (M0–M2) | 90–92% | Protocol, NBT, Anvil, chunks, login, inventory, physics, scheduler, registries, tags |
| Terrain generation | ~70% | Density, biomes, aquifers, surface rules, carvers, ore pass, vein filler |
| Full worldgen | ~60–65% | All 888 placement modifiers, 171/224 feature types, trees/ground cover/aquatic/vines/fungi; structures are template/start-only; Nether/End missing |
| Lighting | ~35% | Nibble storage/NBT, sky+block BFS, per-state behaviour table, neighbour-aware edits; incremental section updates and Java fixtures missing |
| Player simulation | ~30% | Movement physics, collision, fall damage, break progress/tools/drops/durability; special movement and full shapes missing |
| Entities | ~20% | Tracking, gravity/AABB, metadata packets, attributes, equipment, raw damage/death; AI, spawning, armor, per-viewer metadata missing |
| Blocks/redstone/fluids | ~25% | Break/place, neighbour/shape updates, scheduled/random ticks, falling blocks, grass spread, water/lava flow, wire/lever/button; pistons, block entities, full components missing |
| Networking | ~40% offline / 0% online | Join path works offline; no encryption, auth, or full packet coverage |
| Plugin compatibility | ~10–15% | Lifecycle, events, commands, scheduler, basic proxies; most gameplay API unfinished |
| Plugin compatibility | ~15–20% | Lifecycle, events, commands, scheduler, world/block read/write, player vitals/game mode, storage inventory, basic item meta, three cancellable events |
| Folia regionization | ~10% | Task queues exist; world state is still globally shared |
| Overall rewrite | **~22%** | Foundation-heavy, content-light |


---

# Baseline: what is already done

- 2026-11-06: P5.1 metadata/health/equipment core — `entity_metadata.rs` adds a
  typed synched-data store keyed by entity id with dirty-only
  `ClientboundSetEntityData` encoding, `LivingEntity` raw damage with the
  vanilla 20-tick invulnerability window, death event `3` plus poof event `60`
  after 20 ticks, `ClientboundSetHealth` for players, typed attributes, and the
  eight equipment slots with `ClientboundSetEquipment` round-trip. `entity.rs`
  exposes `spawn_living`/`hurt_entity`/`tick_living_entities`/
  `set_entity_equipment` bridges; join/player sessions are not wired. Focused:
  6 lib + 12 integration tests. Remaining: other serializers, initial
  non-default metadata at spawn, per-viewer tracking, armor calculation, and a
  Java packet fixture.
- 2026-11-06: Block-behaviour consumers for lighting and player physics —
  `light.rs` replaces its hand-written opacity/emission tables with
  `block_behaviour::light_block`/`light_emission`; `player_physics.rs` replaces
  its tag/name approximations with `has_collision`, `blocks_motion`, `is_solid`,
  `friction`, `speed_factor`, and `jump_factor`. Collision boxes stay full cubes
  because shape metadata is not ported, and unknown states fall back to the
  conservative registry behaviour (opaque, non-emitting, collidable). Focused:
  4 new `tests/block_behaviour_integration.rs` tests plus updated light/physics
  unit tests; the captured `world_chunk_packet` body is unchanged.
- 2026-11-06: P2.4/P2.5 feature-gap slice — `PaleMossDecorator` now embeds the
  parsed `minecraft:pale_moss_patch` and runs its ground/trunk/leaf passes in
  vanilla RNG order; `simple_random_selector` picks one inline branch with
  `nextInt(size)`; `simple_block.schedule_tick` queues through
  `crate::tick::ScheduledTickQueue`; beehives store two or three `bees`
  occupants with `nextInt(599)`/`min_ticks_in_hive: 600`. 171 of 224 configured
  features dispatch as supported (was 162). `tests/vegetation.rs` adds 3 shipped
  pale-forest/flower/dripleaf determinism tests.
- 2026-11-06: Block-breaking gameplay — `block_breaking.rs` ports the vanilla
  dig state machine, `getDestroyProgress`/`getDestroySpeed`, tier and harvest
  gating via the block tags, sword/shears rules, durability, and a
  deterministic common-block drop table. `join.rs` validates start/stop/abort
  against elapsed ticks, breaks instantly in creative, applies tool wear and
  item break, captures drops, and continues a delayed destroy on the idle
  tick. 13 lib + 9 `tests/block_breaking.rs` tests. Real loot tables, item
  entities, block-damage overlay packets, Unbreaking/Fortune/Silk Touch, and a
  Java break-time fixture remain open.
- 2026-11-06: P3.10 fluid/redstone slice — `fluid.rs` schedules water/lava
  flow (source/flowing/falling levels, 5/30-tick delays, 1/2 drop-off,
  downward priority, hole slope search, water-only source conversion,
  waterlogged placement, drain), and `redstone.rs` adds strong/weak
  `SignalGetter` queries, wire 0..15 propagation/dedup through the neighbour
  queue, and lever toggle plus button press with a scheduled release. Focused:
  8 fluid + 7 redstone + 1 block-update unit tests and 7 `tests/fluids.rs`
  integration tests. Workspace: 519 tests when the two pre-existing CRLF
  asset-hash guards are skipped.
- 2026-11-06: P1.7 loot/recipe catalogs — `scripts/extract_loot_recipes.ps1`
  normalizes all 1,324 loot tables and 1,470 recipes into compact sorted JSON
  with per-type counts. `src/loot.rs` adds typed tables/pools/entries/number
  providers/functions/conditions, a Java-order roll engine over
  `folia_random::RandomSource`, and typed recipe/ingredient/result lookups.
  Unknown payloads are preserved as `Opaque` and named in `LootRoll::report`.
  9 `tests/loot.rs` tests; both catalogs are pinned by `tests/assets.rs`.
- 2026-11-06: Plugin world/player/item/event bridge — `World`/`Block` read/
  write against the shared overworld storage, `Player` identity/location/game
  mode/vitals/teleport/messages backed by playerdata, `PlayerInventory` slots
  0-35, and `ItemStack` type/count/damage/name/lore/unbreakable. Paper
  `RegistryAccess`/`InternalAPIBridge` service shims make `new ItemStack(...)`
  and `Biome` initialization work without CraftBukkit. `BlockBreakEvent`,
  `BlockPlaceEvent`, and `PlayerInteractEvent` are cancellable and
  host-callable. 6 `folia-server` unit tests, 18 Java `BridgeApiTest` checks,
  and the gated `FOLIA_BRIDGE_SELFTEST=1` server run pass. Live `join.rs` event
  wiring, live player state sync, armor/off-hand, and enchantment/PDC
  persistence remain open.
- 2026-11-06: P2.5 aquatic/vine/mushroom batch — `worldgen/feature/aquatic.rs`
- 2026-11-06: P2.12 surface fidelity - `surface::build_surface` resolves
  surface biomes through `biome_zoom::BiomeZoomer` (lazily per block Y) when a
  configured generator seed exists, `Biome.getHeightAdjustedTemperature` uses
  `biome_climate` + `simplex::temperature_noise`, and Paper's
  `generate-flat-bedrock` is parsed from `server.properties`. The column start
  was corrected to Java's `Heightmap.getHeight + 1`. Tests cover the zoomer,
  flat-bedrock on/off, and fixed-coordinate frozen/deep-frozen melt.
- 2026-11-06: P2.5 aquatic/vine/mushroom batch - `worldgen/feature/aquatic.rs`
  ports `seagrass`, `kelp`, `sea_pickle`, `coral_tree`/`coral_claw`/
  `coral_mushroom`, `bamboo`, and `underwater_magma`; `vines.rs` ports `vines`,
  `twisting_vines`, and `weeping_vines`; `mushroom.rs` ports huge brown/red
  mushrooms, huge crimson/warped fungus (stem, hat, shroomlight decorator,
  hanging weeping vines), `glowstone_blob`, `forest_rock`, `fill_layer`,
  `nether_forest_vegetation`, and `random_boolean_selector`. 162 of 224
  configured features now dispatch as supported (was 134). `FeatureLevel` gained
  a vanilla-heightmap default and an absent section now reads as air.
  Generator decoration determinism is covered by `tests/aquatic.rs` (16 tests).
  Parity: 98.71% block-class / 92.02% exact-name (5654/6144), a one-block exact
  delta from 92.04% caused by a tall-seagrass pair on a column whose Rust
  terrain already differs from Java (water at `y=47` vs Java gravel).
  Superseded by ore/vein wiring on 2026-11-06: 98.75% / 95.80%.
- 2026-11-06: Server-side player physics — `player_physics.rs` ports the vanilla
  walking/falling formulas (AABB 0.6x1.8 standing, 0.6x1.5 sneaking, gravity
  0.08, drag 0.98, ground friction, jump 0.42, step 0.6, fall distance/damage),
  collides against `WorldStorage`, and reconciles movement packets with
  corrections. Live fall damage sends health updates and death. 12 focused
  tests.
- 2026-11-06: P2.5 ground-cover batch — `worldgen/feature/vegetation.rs`
  implements `random_patch`/`flower`, `simple_block`, `block_pile`,
  `block_column`, `multiface_growth`, `vegetation_patch`, and
  `waterlogged_vegetation_patch`, with inline nested placed features and all
  shipped state provider types. 134 of 224 configured features execute.
  Generation now runs `underground_decoration`, `vegetal_decoration`, and
  `top_layer_modification` (ores stay off). 8 unit + 15 integration tests;
  parity unchanged at 98.71% block-class / 92.04% exact-name.
- 2026-11-06: Structure template foundation — `worldgen/structure/template.rs`
  parity unchanged at 98.71% block-class / 92.04% exact-name. (Ores are on and
  the current fixture is 98.75%/95.80% as of the ore/vein wiring slice.)
- 2026-11-06: Structure-template foundation — `worldgen/structure/template.rs`
  parses gzip 1.21.11 structure NBT (DataVersion, size, palettes, blocks with
  optional block-entity NBT, entities), ports `StructureTemplate.transform`
  for block and vector positions with mirror/four rotations/pivot/origin, and
  writes blocks through `FeatureLevel`. `place_into` returns written/rejected
  counts, block-entity NBT, and transformed entity descriptors; it does not
  spawn entities. Structure starts, pieces, sets, references, processor lists,
  template pools, jigsaw expansion, and random palette selection were still
  unported at that point. 13 focused tests. 426 tests on the last full run.
- 2026-11-06: Structure start/piece/set foundation — `worldgen/structure/set.rs`
  parses the 20 shipped `structure_set` JSON documents and ports random-spread
  placement (spacing/separation/salt, linear/triangular spread, all four
  frequency reducers, exclusion zones), weighted selection, and
  `structure_at_chunk`. `piece.rs` adds `BoundingBox`, `StructurePiece`,
  template pieces, Java rotation-pivot bounds, chunk-box clipping, block filters,
  and data-marker reporting. `start.rs` adds deterministic generation contexts,
  `StructureStart` references and terrain inflation, the igloo structure end to
  end, and explicit `UnsupportedJigsaw` errors. 13 integration tests in
  `tests/structure_starts.rs`; full workspace gate pending the coordinator merge.
- 2026-11-06: Vanilla tag registries — `scripts/extract_tags.ps1` flattens nine
  registries into `assets/tags/*.tsv` (602 non-empty tags across 604 files,
  7,208 members) and
  `src/tags.rs` provides namespaced membership lookups with nested references
  resolved at extraction time. Focused tests cover block/item/fluid/entity
  membership, flattening, exact counts, and unknown keys. 426 tests passing.
- 2026-11-06: Ore generator wiring — `Decoration::UndergroundOres` now runs
  the 32 shipped `minecraft:ore`/`minecraft:scattered_ore` placed features and
  `OreVeinifier` executes in the density fill pass. The decoration pipeline now
  follows vanilla's lazy placement stream (feature RNG consumed between
  positions) and uses the `MultiNoiseBiomeSource` parameter order for
  `FeatureSorter` indices, which fixes mis-seeded multi-position features.
  Exact-name parity is 95.80% (5886/6144) and block-class 98.75% (6067/6144),
  up from 92.01%/98.71%; 26 of 33 Java ore blocks in the fixture match.
  Focused `tests/ore_wiring.rs` plus generator/ore-vein unit tests.
- 2026-11-06: Ore slice — `minecraft:ore` and `minecraft:scattered_ore`
  parse and place with the shipped target tags and states; all 30 ore + 2
  scattered-ore configs are supported. 341 tests.
- 2026-11-06: Creaking-heart decorator — parses `creaking_heart`, shuffles
  logs, requires six surrounding logs, and writes a dormant natural heart;
  focused test covers success and rejection. Pale moss stays unsupported at
  that point because its patch feature is not ported (the patch landed in the
  P2.5 batch on 2026-11-06; the tree decorator still needs wiring). 337 tests
  passing.
- 2026-11-05: Fallen-tree slice — a stump, sideways logs with axis tags,
  whole-log ground validation, and `attached_to_logs` mushrooms; shipped
  `fallen_oak_tree` fixture places 5 logs (4 sideways) and 4 mushrooms at seed
  7. 336 tests passing.
- 2026-11-05: Mangrove slice — upwards-branching trunk plus mangrove root
  placer with waterlogged and muddy roots; shipped fixture places 10 logs,
  157 leaves, 17 roots (7 waterlogged), 7 muddy roots, and 3 moss carpets at
  seed 7. 334 tests passing.
- 2026-11-05: Azalea slice — bending trunk plus random-spread foliage with a
  weighted azalea/flowering-azalea provider; shipped fixture places 8 logs,
  65 azalea leaves, and 24 flowering azalea leaves at seed 7. 333 tests
  passing.
- 2026-11-05: Jungle bush slice — straight trunk plus bush foliage; shipped
  fixture places 1 log/32 leaves at seed 7. 332 tests passing.
- 2026-11-05: Attached-to-leaves slice — shuffled leaves, direction and
  exclusion rules, and randomized integer block states. Synthetic oak fixture
  places 6 propagules at seed 7. 331 tests passing.
- 2026-11-05: Mega jungle slice — giant 2x2 trunk plus an angled branch
  walk, and jungle foliage. Fixture: 63 logs/287 leaves and 6 branch logs at
  seed 7. 330 tests passing.
- 2026-11-05: Mega spruce/pine slice — giant 2x2 trunk, mega-pine crown
  foliage, and `alter_ground` podzol. Fixtures: spruce 109 logs/295 leaves,
  pine 109/75, 113 podzol each at seed 7. 329 tests passing.
- 2026-11-05: Cherry slice — cherry trunk branch sampling/walks and cherry
  foliage rows with hanging leaves. Fixture places 20 logs/291 leaves and 8
  horizontal logs at seed 7. 328 tests passing.
- 2026-11-05: Fancy oak slice — fancy limbs with axis-tagged logs, fancy foliage
  layers, and `min_clipped_height` support; clipped-height fixture passes.
  327 tests passing.
- 2026-11-05: Acacia slice — forking trunk plus acacia foliage now parse and
  place (11 logs/73 leaves at seed 7 on the flat fixture). 327 tests passing.
- 2026-11-05: Native console + void death — stdin reader, live session
  command channel, persisted `ops.txt`, operator gating, health packets, combat
  kill, and respawn. 325 tests passing.
These are the checkpoints that should be preserved while building forward.

## M0 — Decompile and inventory `[x]`

- [x] Reconstructed Folia 1.21.11 jar and decompiled it to `../decompiled`.
- [x] Recorded source inventory: 6,485 files / 744,058 lines.
- [x] Identified the one known decompiler failure in
  `ChunkUpgradeGenericStatusTask`.

## M1 — Rust host and JVM bridge `[x]`

- [x] Embedded JVM via JNI, now lazy and optional.
- [x] Real `plugin.yml` + `JavaPlugin` jar loading.
- [x] Paper-compatible configured plugin class loader.
- [x] Lifecycle: `onLoad` → `onEnable` → tick → `onDisable`.
- [x] Bukkit event bus for host-fired events.
- [x] Commands and a Rust-owned task table.
- [x] JNI round trip and example plugin.

## M2 — Core primitives `[x]`

- [x] VarInt/VarLong, packet framing, handshake/status/ping/legacy ping.
- [x] Offline-mode login → configuration → play.
- [x] Captured 1.21.11 configuration registries and join packets.
- [x] Live 7×7 chunk streaming from `WorldStorage` or generated chunks.
- [x] NBT reader/writer with gzip/zlib.
- [x] Anvil `.mca` reader/writer with gzip/zlib/uncompressed chunks.
- [x] Chunk block-state and biome palettes, decode and encode.
- [x] Dynamic chunk packets, heightmaps, and light payloads.
- [x] Player sessions, movement parsing, gzip-NBT playerdata, keepalives.
- [x] 36-slot inventory, cursor, selected slot, container clicks, item use.
- [x] Block break/place, persistence, and update packets.
- [x] Entity add/remove/position sync and gravity/AABB physics.
- [x] Regionized task queues with delays, periods, cancellation, parallel ticks.
- [x] `level.dat` and `server.properties` parsing.
- [x] Legacy and Xoroshiro random sources with Java-differential tests.
- [x] Differential armor-stand physics trace.
- [x] Overworld noise/density/aquifer/biome/surface/carver generation.
- [x] Exact 1225/1225 density grid, 1536/1536 biome cells, exact bedrock.
- [x] 98.75% block-class / 95.80% exact-name agreement on the 16-column fixture.
- [x] Full block/item/entity/biome registry tables (Nov 3, uncommitted).
- [x] Feature metadata extracted to `assets/feature_data.json` (supported types wired).

---

# P0 — Immediate cleanup and guardrails

**Why first:** The project has a large uncommitted working tree, a formatting
failure, dead-code warnings, and no CI. Adding more systems on top without a
clean, reproducible baseline makes every later regression harder to attribute.

### P0.1 Fix formatting and warnings `[x]` `S`

- **What:** Run `cargo fmt --all`; fix the four dead-code warnings in
  `chunk_packet.rs` (`GRASS`, `DIRT`, `BEDROCK`) and `registry.rs`
  (`state_count`), then clean the existing Clippy findings reported by
  `cargo clippy --workspace --all-targets -- -D warnings` (collapsible-if,
  unnecessary casts, too-many-arguments, needless-range-loop, and test-only
  lints).
- **Why:** A clean baseline is required before CI and before new work.
- **Done when:** `cargo fmt --check` passes and `cargo clippy --workspace
  --all-targets -- -D warnings` passes.
- **Done (2026-11-04):** `cargo fmt --all` applied; all 21 Clippy findings fixed
  by refactoring (let-chains, grouped carver origin args, `is_multiple_of`,
  literal grouping, needless-range-loop, removed bogus `% 1` assertion). Test
  constants moved into their test module; `block_state_count()` keeps the
  29,671-state assertion meaningful. Both commands are clean; the suite
  passed (240 tests at that point).

### P0.2 Refresh the handoff and port map `[x]` `S`

- **What:** Update `HANDOFF.md`, `PORT_MAP.md`, and `README.md` to say:
  - The current suite size (253), not 237.
  - Full registry tables and `registry.rs` exist.
  - `feature_data.json` exists but is not used.
  - Full block-ID mapping removed the old “fallback to stone” limitation.
  - `cargo fmt --check` state and known warnings.
- **Why:** The next session must not rediscover stale facts.
- **Done when:** Documents match the working tree and are dated.
- **Done (2026-11-04):** `HANDOFF.md` dated with the real working-tree list,
  240-test breakdown, clean fmt/clippy state, a new “Registries and generated
  assets” section, and refreshed limitations/next steps. `PORT_MAP.md` gained a
  registry/feature-data row and current parity numbers. `README.md` documents
  the registry tables, unused `feature_data.json`, and the clean baseline.

### P0.3 Track and validate generated assets `[x]` `S`

- **What:** Add the untracked assets to version control or an asset archive:
  `block_states.tsv`, `block_defaults.tsv`, `item_ids.tsv`,
  `entity_type_ids.tsv`, `feature_data.json`, `biome_ids.txt`,
  `mth_sin.bin`, and existing oracle fixtures.
  Add tests that assert expected line counts/checksums.
- **Why:** `registry.rs` uses `include_str!`, so a missing asset is a build
  failure. Silent truncation would corrupt IDs.
- **Done when:** A clean checkout builds and registry tests pass.
- **Done (2026-11-04):** Added `assets/ASSETS.md` as the versioned manifest
  (target 1.21.11 / DataVersion 4788, per-file line and byte counts, FNV-1a-64
  checksums) and `tests/assets.rs` with 4 guard tests: byte-exact manifest,
  table line counts, dense block-state ids from 0, and feature-data entry
  counts. Assets are recoverable through `scripts/snapshot.ps1` because Git
  remains off-limits for workflow.

### P0.4 Add CI `[x]` `S`

- **What:** Add a GitHub Actions (or equivalent) workflow that runs on Windows
  and Linux: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo test --workspace`, and a release build.
- **Why:** Differential tests are the only defense against “looks close,
  behaves differently” regressions.
- **Done when:** CI is green on a clean clone and blocks merges on failure.
- **Done (2026-11-04):** `.github/workflows/ci.yml` runs on `ubuntu-latest`
  and `windows-latest`: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`, and a release
  build, with `FOLIA_DISABLE_PLUGINS=1`. Not yet observed green on GitHub
  (this checkout cannot push); it also requires the generated assets and
  `world/region` fixture to be committed, which is still pending P0.6.

### P0.5 Make oracle data reproducible `[ ]` `M`

- **What:** Replace manual copy/paste of Java dump files with scripts that:
  start the capture server, run the dump plugin, normalize output, write into
  `crates/folia-core/assets`, and record the Java version/DataVersion.
- **Why:** Every parity claim depends on the oracle being reproducible.
- **Done when:** `scripts/dump-oracle.ps1` regenerates all fixtures and a test
  verifies their hashes.

### P0.6 Decide the commit/backup policy `[x]` `S`

- **What:** The handoff says the tree is uncommitted by request. Decide whether
  to commit the Nov 2–3 work, keep it as a backup branch, or export a patch.
- **Why:** Thirty paths, including all registry work, currently exist only in
  the working tree.
- **Done when:** There is a recoverable copy outside the working tree.
- **Done (2026-11-04):** Policy stays "no Git workflow" per `AGENTS.md` and
  `DECISIONS.md`; snapshots are the recoverable copy. A fresh snapshot
  (`scripts/snapshot.ps1`) was taken after the P0 cleanup work. If the user
  later wants history, exporting a patch or committing the tree is a separate
  explicit request.

---

# P1 — Data, registries, and item components

**Why this phase:** Almost every gameplay system reads registry IDs, tags,
block behavior flags, or item data components. Doing these first prevents
hard-coded IDs from spreading into lighting, redstone, inventories, and
networking.

**Current state:** Block states, block defaults, item IDs, entity type IDs, and
biome IDs are dumped. The registry module is stringly typed in places and item
stacks only carry `item + count`.

### P1.1 Complete all required registry dumps `[~]` `M`

- **What:** Extend `dump_registries` / a Java tool to dump every registry that
  appears on the wire or in saved data:
  - Built-in: block, item, entity type, block entity type, menu type, mob
    effect, particle type, sound event, potion, attribute, data component type,
    recipe type/serializer, stat type, command argument type, fluid, game event,
    creative mode tab.
  - Dynamic: biome, damage type, dimension type, enchantment, chat type,
    trim material/pattern, painting variant, jukebox song, banner pattern,
    instrument, cat/frog/wolf variant, villager profession, and all
    `worldgen/*` registries.
- **Why:** Packets, NBT, commands, and plugin APIs need stable numeric IDs and
  namespaced keys. Hard-coded IDs are already a recurring source of drift.
- **Reference:** `net/minecraft/core/registries/BuiltInRegistries.java`,
  `net/minecraft/resources/RegistryDataLoader.java`.
- **Done when:** Generated tables cover every registry with count assertions
  and lookups by name and ID.

### P1.2 Introduce typed registry keys and ID maps `[ ]` `M`

- **What:** Add `RegistryKey<T>`, `IdMap<T>`, and typed wrappers instead of
  passing raw strings or `i32` IDs through gameplay code.
- **Why:** Prevents mixing item IDs with block-state IDs, or biomes with
  dimensions, and makes invalid data fail at load time.
- **Done when:** New code uses typed keys; only serialization boundaries use
  raw IDs.

### P1.3 Port tags `[~]` `M`

- **What:** Extract and load block, item, entity type, fluid, game-event,
  biome, and damage-type tags. Start with:
  `stone_ore_replaceables`, `deepslate_ore_replaceables`, tool-mining tags,
  `dirt`, `sand`, `logs`, `leaves`, `planks`, `wool`, `flowers`, etc.
- **Why:** Ore generation, tool drops, block behavior, structure biomes, and
  plugin APIs all depend on tags.
- **Reference:** `net/minecraft/tags/*`, `data/minecraft/tags/**`.
- **Done when:** Tag membership matches Java for the shipped 1.21.11 data and
  feature_data’s ore tags are no longer special-cased.
- **Progress (2026-11-06):** `scripts/extract_tags.ps1` recursively flattens
  every nested `#tag` reference (cycle-checked, de-duplicated, ordinal-sorted)
  from `decompiled/data/minecraft/tags`. Nine registries are emitted into
  `assets/tags/*.tsv` and embedded by `src/tags.rs`:
  block (202 non-empty tags / 3,655 members), item (198/1,897), entity type
  (46/368), fluid (2/4), game event (5/119), damage type (33/196), enchantment
  (22/308), point-of-interest type (3/30), and worldgen (91/631, covering the
  biome, structure, flat-preset, and world-preset subregistries). Lookups accept
  bare or namespaced keys and unknown tags/members return empty/false.
  `tests/tags.rs` covers membership, nested flattening, exact per-registry
  totals, and unknown lookups; `tests/assets.rs` pins line counts, bytes, and
  FNV hashes. Remaining: cosmetic registries (`banner_pattern`, `dialog`,
  `instrument`, `painting_variant`, `timeline`), datapack `replace` overlays,
  and replacing the ore feature's hard-coded target-tag sets with
  `tags::has`.

### P1.4 Full item stack model and data components `[~]` `L`

- **What:** Replace `ItemStack { item, count }` with a component-aware model:
  item ID, count, added/removed component patches, custom name/lore, damage,
  enchantments, attribute modifiers, container contents, potion contents, food,
  tool, unbreakable, etc. Implement NBT and network codecs plus stack hashing.
- **Why:** The current packet writer emits zero added/removed components. That
  is enough for stone/dirt but breaks durability, enchants, chests, brewing,
  banners, books, and nearly every real plugin.
- **Reference:** `net/minecraft/world/item/ItemStack.java`,
  `net/minecraft/core/component/DataComponents.java`,
  `net/minecraft/network/HashedStack.java`.
- **Done when:** Java-captured item stacks round-trip byte-for-byte through
  NBT and the play-protocol codec, including component patches.
- **Progress (2026-11-06):** `src/item_components.rs` replaces the old
  `{item, count}` pair with `ItemStack { item, count, components }`, where
  `ComponentMap` keeps one insertion-ordered patch list of added values and
  `!` removal markers (matching `PatchedDataComponentMap`/`DataComponentPatch`
  order). NBT encode/decode covers `custom_name`, `item_name`, `lore`,
  `damage`, `max_damage`, `max_stack_size`, `enchantments`,
  `stored_enchantments`, `attribute_modifiers`, `unbreakable`, `custom_data`,
  `container`, `potion_contents`, `food`, `tool`, `repair_cost`, and `rarity`;
  unknown components and malformed known payloads survive as opaque NBT.
  Player inventory/cursor save/load now writes the vanilla `{Slot,id,count,
  components}` shape (legacy capital `Count` is still accepted on load).
  Network encoding writes added/removed component ids and payloads for the
  ported subset; `attribute_modifiers`, `potion_contents`, `tool`, and unknown
  components are omitted and returned in `NetworkEncodingReport` rather than
  emitted as malformed bytes. Inventory merge/split/click/creative-slot and
  selected-item consumption respect full component equality and
  `max_stack_size`. 8 unit + 17 integration tests; stone/dirt packet bodies and
  the captured Java join fixture stay green. Still open: item prototype
  component defaults (`max_stack_size`, `max_damage`, food/tool defaults) are
  not dumped, `attribute_modifiers`/`potion_contents`/`tool` network codecs,
  component hashing for `HashedStack`, and a Java-captured component NBT/packet
  fixture.

### P1.5 Block behavior metadata table `[ ]` `L`

- **What:** Generate per-block properties: hardness, blast resistance, sound
  type, map color, light emission, opacity, solidity, occlusion, air,
  replaceability, flammability, random-tick behavior, collision shape, support
  rules, waterlogging, and required tool/tier.
- **Why:** Physics, lighting, block breaking, placement, redstone, explosions,
  and drops cannot be correct without these flags. A name-only block state is
  not enough.
- **Reference:** `net/minecraft/world/level/block/state/BlockBehaviour.java`,
  `Block.java`, block subclasses.
- **Done when:** A generated table matches Java for all 1,166 blocks and is
  covered by spot-check fixtures.

### P1.6 Data-driven worldgen definitions `[~]` `M`

- **What:** Consolidate `worldgen.json`, `feature_data.json`, biome parameters,
  and future structure data into typed Rust structures loaded once and
  validated. Do not keep JSON as untyped `serde_json::Value` deep in hot code.
- **Why:** Features, structures, and dimensions are data-driven in vanilla.
  Typed definitions allow validation and fast repeated generation.
- **Done when:** All 224 configured features, 258 placed features, and 65 biome
  feature lists deserialize with typed configs and zero unknown types.

### P1.7 Extract remaining vanilla data assets `[~]` `M`

- **What:** Extract from the server jar into a versioned asset directory:
  structure NBT templates, loot tables, recipes, advancements, tags,
  damage types, enchantments, trim data, and worldgen JSON not yet captured.
- **Why:** Structures, loot, crafting, and enchanting cannot be ported without
  the actual data files.
- **Done when:** Asset extraction is scripted and checksums are recorded.
- **Progress (2026-11-06):** `scripts/extract_loot_recipes.ps1` reads
  `data/minecraft/loot_table` and `data/minecraft/recipe`, namespaces every id,
  sorts object keys, and writes `assets/loot_tables.json` (1,324 tables;
  662,482 B) and `assets/recipes.json` (1,470 recipes; 356,848 B), each with a
  total and per-type index. `src/loot.rs` embeds typed catalogs, and
  `tests/loot.rs` plus `tests/assets.rs` pin their behavior and hashes. Still
  open: structure NBT templates, advancements, trim/enchantment payloads, and
  the remaining worldgen JSON.

### P1.8 Registry/asset version guard `[ ]` `S`

- **What:** Record the target Minecraft version and DataVersion (1.21.11 /
  4788) next to every generated asset and fail startup/tests on mismatch.
- **Why:** Mixing data from two versions produces subtle, hard-to-find bugs.
- **Done when:** Build/test output identifies the exact data version used.

---

# P2 — World generation content: features, structures, dimensions

**Why this phase:** Terrain shape and biomes already match Java closely. The
remaining worldgen gap is almost entirely **what is placed on top of the
terrain**: ores, trees, plants, cave decoration, lakes, and structures. This is
also what makes worlds look and play like Minecraft rather than empty terrain.

**Current state:** 98.75% block-class and 95.80% exact-name agreement on a
16-column fixture. 171 of 224 configured features execute, including ores,
scattered ore, the vein filler, the aquatic/vine/mushroom batch, and the
pale-forest/dripleaf batch; remaining errors are concentrated in unported
lush-cave, special, and structure features. `feature_data.json` is consumed by
`worldgen/feature/mod.rs` and drives the decoration steps.

### P2.1 Feature framework core `[~]` `L`

- **What:** Implement the feature abstraction and execution context:
  `Feature` trait, `ConfiguredFeature`, `PlacedFeature`,
  `FeaturePlaceContext`, a `WorldGenLevel` writer, deterministic per-feature
  RNG, generation-step ordering, and feature counting/limits.
- **Why:** Every vanilla feature plugs into this framework. Without it, each
  feature becomes a one-off generator with inconsistent seeding.
- **Reference:** `net/minecraft/world/level/levelgen/feature/Feature.java`,
  `ConfiguredFeature.java`, `PlacedFeature.java`, `FeaturePlaceContext.java`.
- **Done when:** A trivial feature can be configured, placed, and replayed
  deterministically in a test.
- **Progress (2026-11-04):** `worldgen/feature/mod.rs` adds `Feature`,
  `FeaturePlaceContext`, `ConfiguredFeature` with explicit
  `Unsupported` types, `PlacedFeature` records, `FeatureLevel` and the
  chunk-bounded `WorldGenRegion`, `Decoration` step ordering, and a loader for
  the embedded `feature_data.json` that validates every configured/placed/
  biome reference (bare dump names normalized to `minecraft:` keys). Nine
  tests cover deterministic replay, region/vertical bounds, no-op success,
  224/258/65/2 definition counts, tag values, loud unsupported errors, and
  loader rejection of unknown refs. Placement modifiers landed with P2.2.
  Remaining: world-dependent modifiers (P2.2), debug feature-count tracking,
  and wiring into the chunk-status pipeline.

### P2.2 Placement modifier pipeline `[~]` `M`

- **What:** Port all placement modifiers used by the 258 placed features:
  `CountPlacement`, `CountOnEveryLayerPlacement`, `RarityFilter`,
  `InSquarePlacement`, `HeightRangePlacement`, `HeightmapPlacement`,
  `BlockPredicateFilter`, `BiomeFilter`, `SurfaceRelativeThresholdFilter`,
  `SurfaceWaterDepthFilter`, `RandomOffsetPlacement`, `NoiseBasedCount`,
  `NoiseThresholdCount`, `EnvironmentScanPlacement`, `FixedPlacement`, etc.
- **Why:** Placement modifiers determine the exact x/z/y coordinates and count
  of every feature. A wrong modifier gives plausible-looking but non-parity
  worlds.
- **Reference:** `net/minecraft/world/level/levelgen/placement/*`.
- **Done when:** Placed-feature positions match Java for the fixture chunk and
  multiple seeds.
- **Progress (2026-11-04):** `worldgen/feature/placement.rs` types the whole
  pipeline, executes the six world-free modifiers (`count`,
  `rarity_filter`, `in_square`, `fixed_placement`, `height_range`,
  `random_offset`) with vanilla draw order, and runs the `biome` filter
  through the new `PlacementWorld` trait. `worldgen/biome_zoom.rs` ports
  `BiomeManager` (SHA-256 seed obfuscation, the LCG fiddled-distance search
  over eight quart corners), and `BiomePlacementWorld` answers
  `BiomeGenerationSettings.hasFeature` from the loaded biome lists, so
  `end_platform` now yields `[(100, 49, 0)]` instead of failing.
  `IntProvider` samples constant,
  uniform, biased-to-bottom, clamped and weighted-list distributions;
  `HeightProvider` samples constant, uniform, trapezoid and
  very-biased-to-bottom, with `VerticalAnchor` resolved against the build
  limits.  `PlacedFeature::placement_positions` chains modifiers and reports
  `PlacementFailure` instead of dropping positions. Of the 888 shipped
  modifiers, 667 execute and 221 need more world access: `heightmap` 108,
  `block_predicate_filter` 57,
  `surface_water_depth_filter` 25, `environment_scan` 9,
  `count_on_every_layer` 8, noise counts 10, `surface_relative_threshold` 3,
  and one `random_offset` whose `clamped_normal` spread is unported. Those
  parse as explicit unsupported variants. Weighted lists with total weight
  ≥ 64 are refused because vanilla switches to its pre-sorted selector there;
  every shipped list totals 20 or less. Remaining: the tag-dependent
  modifiers above (see `BLOCKERS.md`), then the Java position fixture for the
  done-when. Every ore placement is `count, in_square, height_range, biome`,
  so P2.3 is unblocked.
- **Progress (2026-11-05):** Tree generation added `OCEAN_FLOOR` heightmaps
  (25), `surface_water_depth_filter` (25), and 45 sapling `would_survive`
  predicates, so **all 888** shipped modifiers now execute: every heightmap,
  block predicate, environment scan, layer count, noise count,
  surface-relative filter, and the Gaussian `clamped_normal` offset. Tree
  placement also proves the world-dependent pipeline runs end to end; a Java
  position fixture is still required for the P2.2 done-when.

### P2.3 Ore features and ore veins `[~]` `M`

- **What:** Port `OreFeature`, `ScatteredOreFeature`, ore target predicates, and
  the `OreVeinifier` pass used for copper/iron veins. Wire all 30 overworld ore
  placed features (dirt, gravel, granite, diorite, andesite, tuff, coal, iron,
  copper, gold, redstone, lapis, diamond, emerald, ancient debris, etc.).
- **Why:** Ores are the largest single block-level difference after vegetation,
  and they are required for survival progression.
- **Reference:** `net/minecraft/world/level/levelgen/feature/OreFeature.java`,
  `ScatteredOreFeature.java`,
  `net/minecraft/world/level/levelgen/OreVeinifier.java`.
- **Done when:** Ore block positions match Java block-for-block on several
  chunks/seeds, including air-exposure discard behavior.
- **Progress (2026-11-06):** `worldgen/feature/ore.rs` ports `OreFeature` and
  `ScatteredOreFeature`, including the size-derived ellipsoid chain, branch
  elimination, `discard_chance_on_air_exposure`, exact target states, and the
  `block_match` plus four shipped `tag_match` predicates. All 30 `minecraft:ore`
  and 2 `minecraft:scattered_ore` configs parse and have placement tests.
  `worldgen/ore_vein.rs` ports `OreVeinifier`, and the generator now runs it as
  a block-state filler for solid density positions before surface rules and
  carvers. `Decoration::UndergroundOres` is enabled and dispatches the ore types;
  the same step's disks and `underwater_magma` stay filtered out (they need
  their own surface/cave wiring). Two correctness fixes were required:
  `PlacedFeature::placeWithContext` is a lazy stream, so the configured feature
  must run (and consume RNG) before the next placement position is sampled; and
  `FeatureSorter` indices must use the biome parameter order from
  `assets/biome_params.txt`, not the biome registry order. With lazy placement,
  parameter-order indices, ore features, and the vein filler, exact-name parity
  is 95.80% (5886/6144) and block-class 98.75% (6067/6144), up from
  92.01%/98.71%; the no-ore-step baseline with veins is 92.37% (5675/6144).
  26 of the fixture's 33 Java ore blocks match exactly. Remaining: vein noise
  interpolation (`NoiseChunk` wraps `vein_toggle`/`vein_ridged` in
  `interpolated`, while the port evaluates them exactly), the 3x3-chunk biome
  union and out-of-chunk feature writes, a few ore ellipsoid/shape edge
  differences, and the block-for-block Java fixture.

### P2.4 Tree features and tree placers `[~]` `L`

- **What:** Port `TreeFeature`, all trunk placers, foliage placers, root
  placers, tree decorators, huge fungi, huge mushrooms, azalea trees, mangrove
  trees, and fallen trees.
- **Why:** Trees dominate forest/taiga/jungle visuals and are needed for wood,
  saplings, leaves, and structures.
- **Reference:** `.../feature/TreeFeature.java`,
  `.../feature/trunkplacers/*`, `.../feature/foliageplacers/*`,
  `.../feature/treedecorators/*`, `.../feature/rootplacers/*`.
- **Done when:** Every tree configured feature parses and tree block placement
  matches Java for representative biomes.
- **Progress (2026-11-05):** `worldgen/feature/tree.rs` ports
  `TreeFeature.doPlace` for `straight_trunk_placer` + `blob_foliage_placer`,
  including height RNG order, free-height scan, dirt replacement, layered leaf
  blob shape, and clipped out-of-chunk writes. `random_selector` resolves
  configured references; unsupported branches keep their chance draw so the
  RNG stays aligned but place nothing. `FeaturePlacementWorld` answers biome,
  `OCEAN_FLOOR`/`WORLD_SURFACE` heightmap, and block queries from the chunk
  being generated. Oak/birch trees now generate in forest, plains, and
  birch-forest columns; the generator test asserts logs, leaves, and
  determinism. `place_on_ground` leaf litter (weighted state providers) and
  `beehive` (vanilla probability, candidate shuffle, SOUTH-facing `bee_nest`)
  now execute. `leave_vine`, `trunk_vine`, and `cocoa` also execute in vanilla
  order, so the shipped `jungle_tree` and `swamp_oak` configs now load and
  place their decoration. The straight trunk now also supports
  `spruce_foliage_placer` and `pine_foliage_placer`; shipped fixtures place
  7 logs/54 leaves (spruce) and 7/14 (pine); dark oak now places its 2x2
  branching trunk and signed foliage (11 logs/126 leaves in the flat fixture).
  The forking trunk plus `acacia_foliage_placer` now place acacia (11 logs/73
  leaves in the flat fixture at seed 7). Fancy oak now ports its limb trunk,
  axis-tagged logs, fancy foliage rows, and minimum clipped height (8 logs/112
  leaves at seed 7, with 2 horizontal logs). Cherry now ports branch count,
  length, start/end sampling and the two-direction branch walk; its foliage
  rows place hanging leaves (20 logs/291 leaves, 8 horizontal logs at seed 7).
  Mega spruce/pine now use the giant 2x2 trunk, mega-pine crown foliage, and
  `alter_ground` podzol (109 logs/295 leaves for spruce, 109/75 for pine, 113
  podzol each at seed 7). Mega jungle now uses the giant trunk, an angled branch
  walk, and jungle foliage (63 logs/287 leaves, 6 branch logs at seed 7).
  `jungle_bush` now uses the straight trunk and bush foliage (1 log/32 leaves
  at seed 7). Azalea now pairs a bending trunk with random-spread weighted
  foliage (8 logs/65 azalea leaves/24 flowering azalea leaves at seed 7).
  `mangrove` and `tall_mangrove` now use the upwards-branching trunk and
  mangrove root placer (10 logs/157 leaves/17 roots, 7 waterlogged and 7 muddy,
  plus 3 moss carpets at seed 7). Fallen trees now place a stump, validate a
  sideways log with a two-block ground-gap limit, tag logs with their axis, and
  run `attached_to_logs` (shipped `fallen_oak_tree` fixture at seed 7: 5 logs,
  4 sideways, 4 mushrooms). Attached-to-leaves now shuffles leaves, applies
  direction/probability/exclusion/empty-block rules, and samples randomized
  integer states (6 propagules on the synthetic oak fixture at seed 7).
  Creaking hearts now shuffle logs, require six surrounding logs, and place a
  dormant natural heart. Pale moss now embeds the parsed `pale_moss_patch` and
  runs the shuffled-ground/trunk/leaf passes in vanilla order; `pale_oak` and
  `pale_oak_creaking` load. Beehives now record 2–3 bee occupants as block-entity
  NBT (`ticks_in_hive` from `nextInt(599)`, `min_ticks_in_hive` 600) in the
  feature region. An unsupported decorator type makes the whole tree config
  unsupported. Still open: the generator does not yet drain
  `WorldGenRegion::take_block_entities`, and there is no block-for-block Java
  tree fixture.

### P2.5 Vegetation and ground cover `[~]` `L`

- **What:** Port `RandomPatchFeature`, `SimpleBlockFeature`,
  `BlockPileFeature`, `BlockColumnFeature`, `MultifaceGrowthFeature`,
  `VegetationPatchFeature`, `WaterloggedVegetationPatchFeature`,
  `SeaPickleFeature`, `KelpFeature`, `SeagrassFeature`, coral features, vines,
  twisting/weeping vines, lily pads, sugar cane, cactus, bamboo, mushrooms,
  flowers, tall grass/ferns, leaf litter, wildflowers, firefly bushes, and dry
  grass.
- **Why:** These account for most of the remaining class mismatches in the
  current parity fixture, especially lush caves and oceans.
- **Reference:** `.../feature/*` and `.../feature/configurations/*`.
- **Done when:** Block-class agreement exceeds 99.9% on the fixture and water
  mismatches drop to zero.
- **Progress (2026-11-06):** `worldgen/feature/vegetation.rs` ports all seven
  base feature types above with `simple`, `weighted`, `randomized_int`,
  `noise`, `noise_threshold`, `dual_noise`, and `rotated` state providers.
  Inline nested placed features are parsed for `random_patch`/`flower` and
  vegetation patches, including the `block_predicate_filter` predicates they
  use. The aquatic/vine/mushroom batch adds `aquatic.rs` (`seagrass`, `kelp`,
  `sea_pickle`, all three coral shapes, `bamboo`, `underwater_magma`),
  `vines.rs` (`vines`, `twisting_vines`, `weeping_vines`), and `mushroom.rs`
  (huge brown/red mushrooms, huge crimson/warped fungus, `glowstone_blob`,
  `forest_rock`, `fill_layer`, `nether_forest_vegetation`, and the boolean
  selector that drives mushroom islands). `simple_random_selector` picks one
  inline placed branch, and `simple_block.schedule_tick` queues a one-tick block
  tick through `crate::tick::ScheduledTickQueue`. 171/224 configured features
  now execute; 53 remain explicit `Unsupported`. Cactus and sugar cane already
  run through `block_column`, and lily pads through `simple_block`.
- **Still open:** `random_selector`/`simple_random_selector` branches that
  reference *placed* features (`pale_garden_vegetation`, `warm_ocean_vegetation`)
  need a two-registry resolver; the generator does not yet drain scheduled
  ticks or block entities out of `WorldGenRegion`; most structure features; and
  kelp/huge-fungus/nether-vegetation in-world wiring where their outer
  placement needs `noise_based_count`/`count_on_every_layer` (the features
  themselves are ported). Block-shape metadata would make
  `underwater_magma`'s face-occlusion test exact instead of the current
  full-cube approximation.
- **Parity note:** Enabling the decoration steps after the ground-cover batch
  left the fixture at 98.71% block-class / 92.04% exact-name. The aquatic
  slice moved it to 98.71% / 92.02% (5654/6144): one tall-seagrass pair lands
  on a column where Rust terrain already differs from Java, so the feature is
  faithful to the world this port generated but cannot match the Java block.
  The later ore/vein wiring raises the current fixture to 98.75% / 95.80%
  (6067/5886 of 6144); keep this paragraph as slice history, not current state.

### P2.6 Cave, dripstone, sculk, and special features `[ ]` `L`

- **What:** Port lush-cave features (root system, cave vines, spore blossom,
  moss/clay patches), dripstone cluster/pointed dripstone/large dripstone,
  sculk patch and sculk vein, geodes, monster rooms, fossils, desert wells,
  forest rocks, icebergs/blue ice, and amethyst geodes.
- **Why:** These are the remaining terrain-decoration systems and several are
  needed for a recognizable 1.18+ world.
- **Reference:** `.../feature/GeodeFeature.java`, `DripstoneClusterFeature.java`,
  `PointedDripstoneFeature.java`, `SculkPatchFeature.java`,
  `MonsterRoomFeature.java`, `FossilFeature.java`, `IcebergFeature.java`.
- **Done when:** Each feature has a fixture and contributes no unexplained
  parity differences.

### P2.7 Lakes, springs, disks, and freeze layer `[ ]` `M`

- **What:** Port `LakeFeature`, `SpringFeature`, `DiskFeature`, and
  `SnowAndFreezeFeature`, including water/lava lake placement and
  sand/clay/gravel/grass disks.
- **Why:** Lakes and disks change water boundaries and surface blocks; the
  freeze layer is required for snowy biomes.
- **Reference:** `.../feature/LakeFeature.java`, `SpringFeature.java`,
  `DiskFeature.java`, `SnowAndFreezeFeature.java`.
- **Done when:** Water/lava counts and frozen surfaces match Java on the
  fixture.

### P2.8 Structure framework and templates `[~]` `XL`

- **What:** Implement structure starts, pieces, structure types, structure
  placement, structure sets, references, and biome checks. Add NBT structure
  template loading, block rotation/mirroring, processors, and template pools.
- **Why:** Structures cannot be implemented as features; vanilla uses a
  separate start/reference/piece pipeline with its own chunk-status stages.
- **Reference:** `net/minecraft/world/level/levelgen/structure/*`,
  `.../structure/structures/*`, `.../structure/templatesystem/*`.
- **Done when:** A template-based structure generates at the same position and
  with the same blocks as Java.
- **Progress (2026-11-06):** `worldgen/structure/template.rs` loads the shipped
  gzip NBT format through `nbt::read_gzip_root`: `DataVersion` (default 500),
  `size[3]`, one `palette` or a `palettes` list (Name + sorted Properties),
  `blocks` (pos, state index, optional block-entity NBT), and `entities` (pos,
  blockPos, nbt). Malformed roots, non-positive or non-3 sizes, out-of-range
  palette indices, out-of-template block positions, and wrong tag types return
  `io::ErrorKind::InvalidData`. `StructurePlaceSettings` carries mirror (none,
  left-right, front-back), rotation (none, 90, 180, 270), rotation pivot,
  ignore-entities, and ignore-air (default false, like vanilla block
  placement). `transform`/`transform_vec3` port Java's mirror-then-rotation
  formulas exactly, including non-square extent swaps in `rotated_size`.
  `placement_blocks` yields transformed blocks (NBT-bearing blocks last, then
  y/x/z) and `place_into(&mut dyn FeatureLevel, origin, settings)` writes them,
  returning written/rejected counts plus block-entity NBT and transformed
  entity descriptors instead of spawning entities. `tests/structure_template.rs`
  covers `empty.nbt`, igloo middle, desert camel spawn (block entity + entity),
  and nether fossil templates, all rotations/mirrors, pivot/origin, state
  properties (facing/axis/jigsaw orientation), mock-level placement, and
  malformed NBT. The start/piece/set slice adds `set.rs` (all 20 shipped
  structure-set JSON documents, random-spread linear/triangular placement,
  legacy frequency reducers, exclusion zones, weighted selection, and
  `structure_at_chunk`), `piece.rs` (`BoundingBox`, `StructurePiece`, template
  pieces with Java rotation-pivot bounds, chunk-box clipping, structure-block
  filtering, and data markers), and `start.rs` (deterministic generation
  context, `StructureStart` with references/terrain inflation, the igloo
  structure end to end, and explicit jigsaw failures). `tests/structure_starts.rs`
  adds 13 integration tests for shipped set counts, fixed-seed placement chunk
  sets, weighted picks, igloo bounds for two rotations, fixed-origin template
  placement, an igloo placed at a fixed surface height, and jigsaw rejection.
  Still open: biome/dimension filtering in registry-level selection, processor
  lists, template pools, jigsaw expansion, random multi-palette selection,
  structure NBT serialization, entity spawning, loot-table/chest block
  entities, waterlogging, shape updates, and per-block state derivatives that
  need P1.5 block behaviour (stairs shape, door hinge, rail shape, …). Starts
  are not attached to the generator chunk-status pipeline yet.

### P2.9 Jigsaw structures `[ ]` `XL`

- **What:** Port jigsaw placement, junction expansion, template pools,
  processors, terrain adaptation, and structure bounding boxes.
- **Why:** Villages, ancient cities, trial chambers, bastions, and many other
  structures are jigsaw-based.
- **Reference:** `.../structure/structures/JigsawStructure.java`,
  `.../structure/pools/*`, `.../structure/templatesystem/*`.
- **Done when:** At least one jigsaw structure matches Java start position and
  piece layout on a fixture seed.

### P2.10 Concrete structure types `[ ]` `XL`

Implement in dependency order, each with loot/tags and a parity fixture:

- [ ] Mineshafts and strongholds.
- [ ] Villages, pillager outposts, woodland mansions.
- [ ] Ocean ruins, shipwrecks, buried treasure, ocean monuments.
- [~] Igloos (structure start, top/middle/bottom template pieces, laboratory
  branch, rotation, bounds, and surface snap done; loot-table block entities,
  biome filtering, and generator wiring remain).
- [ ] Desert temples, jungle temples, witch huts, ruined portals.
- [ ] Ancient cities, trial chambers.
- [ ] Nether fortresses, bastions, end cities (after P7 dimensions).
- **Why:** Structures are content-heavy but mostly data-driven once P2.8/P2.9
  exist.
- **Done when:** Structure starts/bounding boxes and sampled blocks match Java
  for each type.

### P2.11 Feature/structure parity harness `[ ]` `M`

- **What:** Expand `worldgen_parity.rs` from 16 columns to all 256 columns, add
  multiple chunks and seeds, report per-feature and per-structure differences,
  and add a Java dump mode for feature/structure starts.
- **Why:** The current fixture is good for terrain but too sparse to validate
  features and structures.
- **Done when:** CI runs at least 4 seeds × 4 chunks and fails on unexplained
  regressions.

### P2.12 Exact surface fixes `[~]` `M`

- **What:** Port `PerlinSimplexNoise` and the frozen-ocean
  `FROZEN_TEMPERATURE_NOISE` / `BIOME_INFO_NOISE` modifier. Add
  `BiomeManager`'s fuzzy zoomer for surface `isBiome` checks. Support Paper's
  `generate-flat-bedrock` option.
- **Why:** These are known approximations in the current surface pass and cause
  wrong frozen-ocean temperature/iceberg decisions.
- **Reference:** `net/minecraft/world/level/biome/BiomeManager.java`,
  `FuzzyOffsetConstantColumnBiomeZoomer`, `PerlinSimplexNoise`.
- **Done when:** Frozen/deep-frozen ocean surface and iceberg fixtures match.
- **Landed:** `surface::build_surface` now resolves biomes through
  `biome_zoom::BiomeZoomer` (lazily memoized per block Y like
  `SurfaceRules.Context.updateY`) whenever the active generator seed is
  available; explicit `*_with_seed` entry points cover direct harnesses.
  `Biome.getHeightAdjustedTemperature` now uses the extracted
  `biome_climate` + `simplex::temperature_noise` modules for the snow/ice and
  iceberg-melt branches, and Paper's `optionally_flat_bedrock_condition_source`
  collapses to one flat layer when `generate-flat-bedrock` is true in
  `server.properties`. The surface column start was corrected to Java's
  `Heightmap.getHeight + 1` (first available block).
- **Remaining:** no captured Java frozen-ocean/iceberg fixture yet, so the
  pinned temperature and melt tests are behavior tests rather than a
  differential fixture. Direct `WorldGenerator::overworld` harnesses that never
  call `generator::configure` still use the chunk-local quart fallback because
  the surface system does not receive the raw seed; the server path does.

### P2.13 Feature generation performance `[ ]` `M`

- **What:** Profile and optimize feature placement: avoid per-block
  allocation, cache tag/state lookups, batch chunk-local writes, and parallelize
  independent feature steps where safe.
- **Why:** Current debug generation is ~15.7 s for 49 chunks before features.
  Adding features without optimization would make login far too slow.
- **Done when:** Release-mode chunk generation has a measured budget and a
  regression benchmark.

### P2.14 Custom worldgen/datapack loading `[ ]` `L` optional

- **What:** Load worldgen JSON from datapacks (biomes, features, structures,
  dimension types) instead of only embedded vanilla data.
- **Why:** Many plugins and servers ship datapacks; this is a long-tail
  compatibility feature, not required for vanilla parity.
- **Done when:** A datapack can add a biome/feature without recompiling.

---

# P3 — Chunk lifecycle, lighting, and block updates

**Why this phase:** The server currently streams a fixed 7×7 grid of chunks and
computes a fake sky-light array from surface height. Real gameplay needs
ticket-based loading, incremental lighting, correct light packets, and neighbor
updates.

**Current state:** Joins stream a square chunk column sized by the negotiated
view distance (the shipped `server.properties` allows 10 and a default client
asks for 12, so a normal join streams radius 10 = 441 columns; the probe client
asks for 6 = 169). The first join of a world with no `level.dat` generates the spawn
chunk and writes `level.dat` (DataVersion 4788, `Version`, generator seed,
`spawn.pos`, overworld `WorldGenSettings`, and a `folia:written` marker).
Generated chunks are written to the region files with a `folia:written` NBT
stamp and loaded back on later joins (and after a restart), so terrain and
block edits persist. A level carrying the marker serves only stamped chunks
(leftover flat chunks beside it are regenerated); a foreign `level.dat`, no
level file plus no generator, or `FOLIA_LOAD_STORED=1` serves every stored
chunk. New players spawn at the saved `spawn.pos`. Newly visible columns are
sent when the player crosses a chunk border, columns more than one chunk
outside the radius are unloaded, and players who log in inside solid ground
are lifted to their column's surface.
The live radius is negotiated: the smaller of `server.properties`
`view-distance` and the client's `ClientInformation` view distance, clamped to
2–16 chunks. Cold batches generate their missing columns on up to eight worker
threads with send order preserved, and the level file records elapsed game
time and `LastPlayed` on shutdown. There is still no ticket system, spawn
chunks, in-flight dedupe across sessions, or cross-player sharing.
`light_bytes` prefers stored/computed sky and block layers from the new
`light` module and keeps the old `column_height` sky approximation only for
chunks with no light data yet. `block_update.rs` mutates + recomputes light
with existing neighbours + saves + broadcasts; it now also queues the six
neighbour updates, runs shape/support checks, schedules falling blocks, and the
server loop drives `tick_world` for scheduled and random ticks. Incremental
section-only light packets are still open.

### P3.1 Chunk ticket and load/unload manager `[~]` `L`

- **What:** Implement chunk tickets/load levels, player chunk loaders, view
  distance, spawn chunks, forced chunks, unload queues, and reference counting.
- **Why:** A fixed 7×7 stream cannot support movement, view distance changes,
  teleports, multiple players, or memory limits.
- **Reference:** `net/minecraft/server/level/ChunkMap.java`,
  `ChunkHolder.java`, `DistanceManager.java`,
  `ca/spottedleaf/moonrise/patches/chunk_system/scheduling/*`.
- **Done when:** Moving a player loads new chunks and unloads distant ones
  without leaking chunks or sending duplicate packets.
- **Progress (2026-11-04):** Crossing a chunk border streams the new column,
  re-centers the chunk cache, and unloads columns more than one chunk outside
  the live radius (`ClientboundForgetLevelChunk` id 37, `chunks_to_unload`), so
  a walking player's chunk set stays at 169 columns instead of growing without
  bound. The live radius is negotiated: the smaller of `server.properties`
  `view-distance` and the client's requested distance, clamped to 2–16. Tickets,
  spawn chunks, per-player load levels, and multi-player sharing are still open
  (each session still streams its own copy). A storage now keeps up to 256
  decoded chunks (FIFO eviction) so a second reader of the same column does not
  decompress and decode it again; edits and region unloads invalidate the entry.
  The generation queue is bounded at 16 columns; workers block on a full queue
  and stop when the receiver is dropped, so a disconnected client no longer
  pays for the rest of its batch.

### P3.2 Chunk status pipeline `[~]` `L`

- **What:** Implement the chunk-status chain:
  empty → structure starts → structure references → biomes → noise → surface →
  carvers → features → initialize light → light → spawn → full.
- **Why:** Feature/structure seeding and ordering depend on status stages.
  Current generation runs a fixed sequence without a general status pipeline.
- **Reference:** `net/minecraft/world/level/chunk/status/ChunkStatus.java`,
  `ChunkGenerator.java`, `ChunkStatusTasks.java`.
- **Done when:** Chunks can be upgraded one status at a time and the pipeline is
  resumable/cancellable.

### P3.3 Async chunk generation scheduling `[~]` `L`

- **What:** Add worker-thread generation, deduplication of in-flight chunks,
  priority by player distance, cancellation, and bounded queues.
- **Why:** Generation is CPU-heavy and must not block the tick loop or network
  thread.
- **Done when:** Login with a cold world does not stall the server loop; chunk
  requests are deduplicated and prioritized.
- **Progress (2026-11-05):** Missing columns of a streaming batch are generated
  on up to eight worker threads (`send_columns`); each column is packet-encoded
  on its worker and sent as soon as it is ready, in nearest-first order, with
  stored columns taking their place in the same order and generated ones saved
  just before sending. A 16-column bounded queue applies backpressure. A cold
  169-column join on a fresh level dropped from 16.3 s sequentially to 7.7 s
  with parallel generation, then to 1.9 s after resolving each section palette
  once and moving packet encoding off the session thread. Still open: in-flight
  dedupe across sessions, distance priority, and cancellation beyond dropping
  the receiver. Generation runs in the joining session's thread, so it does not
  block the tick loop but does hold that client's packet flow until its batch is
  ready.

### P3.4 Dirty tracking, autosave, and full chunk sections `[~]` `M`

- **What:** Track dirty chunks, autosave periodically, save entities/POI/block
  entities/light/heightmaps, and add corruption recovery.
- **Why:** Current saves happen immediately on block change; there is no
  entities/POI section support and no periodic autosave.
- **Done when:** A crash/restart preserves blocks, entities, block entities,
  light, and player data.
- **Progress (2026-11-04):** Generated chunks are saved into the region cache
  as they are streamed and flushed on a 5-second timer and on disconnect
  (`TERRAIN_FLUSH_INTERVAL`), so a region file is not rewritten on every chunk
  batch; block edits save and flush immediately. Overworld storage is one
  process-wide instance (`world::shared_overworld`) shared by sessions and
  entity physics, because a region save rewrites the whole file and separate
  instances dropped each other's chunks. A
  fresh world writes `level.dat` (data version, version, seed, spawn, overworld
  generator settings, ownership marker) before the first session, so the world
  is a saved level from the start and later starts read the same seed and
  spawn. On shutdown `level::record_session` adds the run's ticks to `Time`
  and `DayTime`, stamps `LastPlayed`, and can replace `spawn.pos`, rewriting the
  whole root atomically so fields this port does not model survive. Still
  missing: game-rule and spawn changes while running, entity/POI/block-entity
  sections, light persistence, per-chunk (rather than per-region) dirty
  batching, and corruption recovery. Level rewrites keep the previous file as
  `level.dat_old`, and shutdown flushes the shared overworld storage before
  writing `level.dat`, so cached terrain reaches disk on an orderly stop. Block edits save through
  `WorldStorage::update_chunk`, which keeps top-level chunk fields the model
  does not decode (`Status`, `Heightmaps`, `BlockEntities`, `Entities`,
  `structures`, …) from the stored root; generated terrain still overwrites a
  column outright, so stale block entities never attach to new terrain. Block
  entities whose position an edit replaced are dropped, matching vanilla; other
  stored block entity state is kept as-is. A corrupt chunk record (bad length,
  truncated payload, unknown compression) now reads as a missing chunk and is
  logged, so one damaged column no longer fails the whole join; intact
  neighbours in the same region still load. A rewritten chunk whose new record
  fits its existing sectors is written in place, so repeatedly editing one
  chunk no longer grows the region file by a full copy each time; a record that
  outgrows its sectors still appends. When abandoned sectors pass a quarter of
  the live allocation, `save_if_dirty` compacts the region first: live records
  are packed after the header and the file shrinks, so long edit sessions do
  not leave an ever-growing tail. Server-written chunks also carry a
  `Heightmaps` compound (`MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES`,
  `WORLD_SURFACE`) computed from the same column heights as the chunk packet,
  and a block edit recomputes it instead of keeping the old array.

### P3.5 Light storage and NBT format `[x]` `M`

- **What:** Store sky-light and block-light nibble arrays per section, serialize
  them in the Anvil chunk format, and expose them through the chunk packet
  builder.
- **Why:** Lighting is currently recomputed as a fake height-based array and
  never persisted.
- **Reference:** `net/minecraft/world/level/chunk/DataLayer.java`,
  `LevelChunkSection.java`, `SerializableLightMetadata`.
- **Done when:** A chunk with known light data round-trips through save/load.
- **Done (2026-11-06):** `light::DataLayer` is a 2048-byte nibble array with
  vanilla `y << 8 | z << 4 | x` indexing. Each `ChunkSection` carries
  `light: SectionLight { sky, block }`, `decode_section`/`encode_section`
  read/write the vanilla `SkyLight`/`BlockLight` byte arrays (rejecting wrong
  lengths), and `tests/lighting.rs` round-trips layers through
  `encode_chunk` → `decode_chunk` plus a region save/load.

### P3.6 Sky-light engine `[~]` `L`

- **What:** Port sky-light sources/heightmaps, 15→0 propagation, opacity
  handling, and border propagation.
- **Why:** Caves, overhangs, water, leaves, and roofs need real sky light.
- **Reference:** `net/minecraft/world/level/lighting/SkyLightEngine.java`,
  `ChunkSkyLightSources.java`, `LayerLightSectionStorage.java`.
- **Done when:** Sky-light arrays match Java for cave/overhang fixtures.
- **Progress (2026-11-06):** `light.rs` computes the per-column lowest sky
  source Y from `ChunkSkyLightSources` semantics, fills 15 downward, and runs
  a six-direction BFS with opacity so light spreads under overhangs. A
  one-block neighbour margin imports border sources when neighbour chunks are
  supplied; missing neighbours are not seeded, so they cannot invent a bright
  seam. Tests cover a roofed shaft (15/14/… falloff), border propagation, and
  packet bytes. Full `LayerLightSectionStorage` decrease/increase bookkeeping
  and a Java light-array fixture are still open.

### P3.7 Block-light engine `[~]` `L`

- **What:** Port emission tables and block-light increase/decrease propagation,
  including transparent/opaque block rules.
- **Why:** Torches, lava, glowstone, portals, and lit caves are currently
  completely dark/incorrect on the wire.
- **Reference:** `net/minecraft/world/level/lighting/BlockLightEngine.java`,
  `LightEngine.java`.
- **Done when:** Block-light arrays match Java for lava/torch/glowstone
  fixtures.
- **Progress (2026-11-06):** `light.rs` scans emitters and runs a BFS increase
  pass. Emission and opacity now come from the generated per-state
  `block_behaviour` table (29,671 rows), not a hand-written list. That covers
  torch 14, soul torch 10, lantern 15, soul lantern 10,
  glowstone/sea lantern/shroomlight/lava 15, magma 3 (vanilla; the task brief’s
  “magma 15” was wrong), campfire 15, fire 15, redstone lamp and furnace lit
  states, respawn anchor charges, sea pickle, cave vines, froglight, amethyst
  buds, and every other shipped emitter. Opacity comes from `light_block`:
  air/glass 0, water/leaves/ice 1, tinted glass and unknown states 15. Tests
  cover torch radius, glass/water sky attenuation, a glass-shaft overhang,
  unsupported emitters, and border light. No Java block-light array fixture yet.

### P3.8 Incremental light updates and packets `[~]` `M`

- **What:** On block changes, compute only affected light sections and emit
  `ClientboundLightUpdate` / section updates with correct masks.
- **Why:** Re-sending a full chunk’s light on every block change is wasteful and
  still wrong for block light.
- **Done when:** A torch placement produces the same light packet body as Java
  and does not resend unrelated chunks.
- **Progress (2026-11-06):** `block_update::set_block` invalidates cached light,
  reloads existing neighbours, recomputes the edited chunk, persists it through
  `WorldStorage::update_chunk`, and broadcasts the block + light payloads.
  `chunk_packet::light_bytes` now uses stored/computed layers (sky and block)
  with correct masks and real block-light updates; chunks without any light
  still use the old height-based fallback, preserving the byte-identical
  `(0,-1)` Java packet fixture. Incremental section-only relighting, Java
  packet-body comparison, and avoiding whole-chunk broadcasts are still open.

### P3.9 Cross-chunk light propagation `[~]` `M`

- **What:** Handle light crossing chunk borders, queue relighting, and
  generation-time light initialization.
- **Why:** Light at chunk edges is a common source of seams and client
  lighting bugs.
- **Done when:** Border fixtures have no seams and match Java.
- **Progress (2026-11-06):** the edit path loads existing 8-neighbour chunks
  and passes them to the light engine; a neighbour torch lights the border
  (tested 13 at distance one). Generated chunks are still sent with the
  height fallback until they are edited/recomputed, and there is no queued
  cross-chunk relight or Java seam fixture yet.

### P3.10 Neighbor updates, scheduled ticks, random ticks `[~]` `L`

- **What:** Port block update flags, shape updates, neighbor updates, scheduled
  ticks, and random ticks. Add a tick scheduler with deterministic ordering.
- **Why:** Redstone, fluids, crop growth, leaf decay, gravity blocks, and
  pistons all depend on these.
- **Reference:** `net/minecraft/world/level/Level.java`,
  `BlockState.java`, `ScheduledTick.java`, `RandomTick`. 
- **Done when:** A placed block triggers the same neighbor updates as Java in a
  fixture.
- **Progress (2026-11-06):** `src/tick.rs` adds the vanilla `UPDATE_*` flag
  values plus aliases (`BLOCK_UPDATE`, `NEIGHBORS`, `SHAPE`,
  `SUPPRESS_NEIGHBORS`), a chunk-grouped `NeighborUpdateQueue` with
  position deduplication and flag-union merging, a `ScheduledTickQueue` ordered
  by trigger tick, priority, then sub-tick order with delay/cancel/dedup, and a
  `RandomTickQueue` that draws `randomTickSpeed = 3` positions per 16x16x16
  section from a deterministic per-chunk seed. `block_update::set_block` now
  enqueues the six neighbours in `NeighborUpdater.UPDATE_ORDER`, drains them
  (512-update chain limit), and applies shape/support updates. The hook table
  implements falling sand/gravel/concrete-powder/anvil movement, plant support
  from the `dirt`/farmland tags (including double-plant halves), and grass
  spread; `BehaviorTable::with` is the callback interface for future blocks.
  The server loop calls `tick::tick_world` after entity ticks and seeds the
  shared storage from `level.dat`. `src/fluid.rs` adds scheduled water/lava
  flow: source/flowing/falling state mapping, water 5 vs lava 30 tick delay,
  1/2 level drop-off, downward priority, the 4/2-step slope search toward
  holes, two-source conversion (water only), waterlogged placement, and
  source-removal drain. `src/redstone.rs` adds `SignalReader` strong/weak
  queries, `getDirectSignalTo`/`getBestNeighborSignal`, wire target strength
  (adjacent source or strongest wire minus one, 0..15), and lever toggling plus
  button press with the vanilla 20/30-tick release. Wire and fluid updates flow
  through the existing neighbour queue; `behavior_for` registers all four
  behaviours. Focused tests: 8 fluid + 7 redstone + 1 block-update unit tests
  and 7 `tests/fluids.rs` integration tests (hole flow, 7 levels, lava delay,
  drain, waterlogging, source conversion, determinism).
- **Still open:** leaf distance/decay updates, crop/sapling growth, pistons,
  doors, rails, torch support, lava fire spread/quenching, and the remaining
  redstone components (repeater, comparator, torch, observer, pressure plates,
  lamp). Falling blocks move one block per scheduled tick instead of using a
  `FallingBlockEntity`; scheduled ticks are not persisted to chunk NBT; the
  random-tick RNG is a deterministic hash of seed/chunk/tick rather than
  Java's `Level.random` sequence; `tick_world` runs over the storage cache
  rather than a ticket/loaded-chunk set; cancellation during a running tick
  (`clearArea` of `toRunThisTick`) is not modeled. No Java differential
  neighbor-update or fluid/redstone fixture yet.

### P3.11 Heightmap and chunk-packet exactness `[~]` `M`

- **What:** Verify all heightmap types, negative-Y handling, biome sections,
  block-state palette encoding, and light masks against Java packets.
- **Why:** The existing byte-identical test covers one real chunk; features,
  negative Y, and multiple sections need broader coverage.
- **Done when:** Captured Java packets for several chunks (including generated
  chunks) match byte-for-byte.

### P3.12 Lighting/chunk lifecycle differential harness `[ ]` `M`

- **What:** Add Java dumps for light arrays, heightmaps, loaded chunk sets, and
  block-update sequences; compare with Rust.
- **Why:** These are binary systems where “looks right in-game” is not enough.
- **Done when:** CI covers light, heightmaps, chunk tickets, and update packets.

---

# P4 — Player simulation and interaction

**Why this phase:** The player can join and send movement packets, but the
server does not simulate player physics or validate movement. Block breaking is
instant, and only a 36-slot inventory exists. This phase turns the join demo
into actual gameplay.

**Current state:** `player.rs` parses movement and persists it;
`player_physics.rs` now simulates walking, jumping, falling, step-up, collision,
and fall damage and reconciles movement packets. `block_update.rs` breaks
instantly on start-destroy and places on use-item-on.

### P4.1 Player entity and physics `[~]` `L`

- **What:** Give each player a real entity with pose, dimensions, collision,
  gravity, drag, step height, jump, sprint, sneak, swim, fly, ladder, water,
  lava, cobweb, slime, and fall handling.
- **Why:** Without server-side player physics, movement corrections, fall
  damage, anti-cheat, and interactions are impossible.
- **Reference:** `net/minecraft/world/entity/player/Player.java`,
  `ServerPlayer.java`, `LivingEntity.travel`.
- **Done when:** Movement traces match Java for walking, jumping, falling,
  swimming, and ladders within `1e-4`.
- **Progress (2026-11-06):** `player_physics.rs` ports the standing/sneaking/
  swimming AABBs, `travelInAir` gravity/drag/friction, jump impulse, sprint
  jump, step height, axis-separated collision, fall-distance accumulation, and
  fall damage. Collision, friction, speed and jump metadata now come from the
  generated per-state `block_behaviour` table (`has_collision`, `blocks_motion`,
  `is_solid`, `friction`, `speed_factor`, `jump_factor`); ice is 0.98, blue ice
  0.989, slime 0.8, soul sand slows to 0.4, and honey halves the jump impulse.
  Focused tests pin a 20-tick walk displacement within `1e-4`, the jump apex
  `1.252203`, ten-block fall damage `7`, wall collision, half-slab step-up,
  factor scaling, and water/plant passability. The live session now simulates
  movement instead of accepting packets verbatim and sends health updates on
  fall damage. Still open: ladder/vine/scaffolding, cobweb, slime bounce,
  elytra, water/lava/swimming physics, bubbles, powder snow,
  levitation/slow falling, sprint/sneak input packets, and a standalone 20 Hz
  player tick; non-full block shapes are full cubes until shape metadata is
  ported.

### P4.2 Movement validation, corrections, and teleports `[~]` `M`

- **Progress (2026-11-05):** a movement packet whose squared displacement
  exceeds 100 (ten blocks) is reverted and answered with a position packet
  carrying the last accepted position; the integration test joins, sends a
  500-block jump, and asserts the correction restores the joined position.
- **Progress (2026-11-06):** movement deltas up to `0.75` blocks are replayed
  through the same collision code used by the simulation; a claim that cannot
  reach the requested position beyond `1e-3` gets a position correction. Longer
  packets are treated as lag bursts, validate their destination, and lift out
  of terrain by up to eight blocks. Normal walking is accepted exactly and is
  covered by a no-rubber-band test. Teleport acknowledgement is still ignored,
  and block-shape metadata limits collision exactness.

- **What:** Validate movement packets, handle on-ground/sprint/sneak flags,
  send position corrections, implement teleport/ack, and reject impossible
  movement.
- **Why:** Clients can send arbitrary positions; the server must be
  authoritative.
- **Done when:** A simulated client cannot teleport through blocks and a legal
  walk is not rubber-banded.

### P4.3 Block breaking progress, tools, and drops `[~]` `L`

- **What:** Implement start/stop/abort digging, hardness/tool tiers, efficiency,
  instant-break blocks, block drops, tool durability, incorrect-tool behavior,
  and creative mode.
- **Why:** Current break was instantaneous for every block and item.
- **Reference:** `ServerPlayerGameMode.java`, `Player.java`,
  `BlockBehaviour.getDestroySpeed`, loot tables.
- **Progress (2026-11-06):** block loot tables are loaded and rollable through
  `LootCatalog::roll`, including tool, silk-touch, fortune, and explosion
  conditions. The break path is still instant and does not call the roll engine
  yet.
- **Done when:** Break times and drops match Java for representative blocks and
  tools.
- **Progress (2026-11-06):** `block_breaking.rs` ports
  `ServerPlayerGameMode.handleBlockBreakAction`/`tick` (start, stop, abort,
  the 0.7 stop threshold, and `hasDelayedDestroy`), `Player.getDestroySpeed`,
  `Player.hasCorrectToolForDrops`, and
  `BlockBehaviour.getDestroyProgress`. Tool material speeds/durabilities,
  efficiency (`level^2 + 1`), haste/mining fatigue, water/air penalties,
  sword/shears special rules, and the `mineable/*` + `incorrect_for_*_tool` +
  `needs_*_tool` tags drive tier and harvest gates. `join.rs` keeps per-session
  dig state, validates elapsed ticks, breaks instantly in creative, applies
  tool durability (item break included), and captures drops into the
  inventory. A deterministic drop table covers the common blocks and ores,
  with a `LootProvider` hook for `crate::loot`; leaves/glass/ice no-self-drop
  families are handled. 13 unit + 9 integration tests cover stone/wood,
  dirt/shovel, obsidian tier gating, creative, durability, drop counts, and the
  delayed-destroy path. Still open: real loot tables (ranges/Fortune/Silk
  Touch), item entities, block-damage overlay packets, Unbreaking, adventure
  `can_destroy`, and a Java break-time fixture.

### P4.4 Block placement rules `[ ]` `M`

- **What:** Implement face/cursor placement, collision checks, replaceable
  blocks, waterlogging, block-entity creation, support checks, sound/particle
  events, consumption, and creative behavior.
- **Why:** Current placement places one held block at the target face without
  most vanilla rules.
- **Done when:** Placement fixtures match Java for slabs, stairs, waterlogged
  blocks, plants, and block entities.

### P4.5 Item use behaviors `[ ]` `L`

- **What:** Port item use for food/drink, potions, buckets, bow/crossbow,
  trident, shield, ender pearl, snowball/egg, firework, maps, compasses, and
  spawn eggs.
- **Why:** These are core interactions and many plugins depend on them.
- **Reference:** `net/minecraft/world/item/*`, `ItemStack.use`,
  `UseOnContext`.
- **Done when:** Each implemented item has a behavior test and the correct
  client packets.

### P4.6 Full inventory and menu framework `[ ]` `XL`

- **What:** Add armor/offhand, crafting grid, chest/furnace/hopper/etc. menus,
  click modes (pickup, quick move, swap, throw, quick craft), drag handling,
  state IDs, creative slots, item components, and drop/pickup.
- **Why:** The current 36-slot inventory is a demo; real gameplay and plugins
  need containers and metadata.
- **Reference:** `net/minecraft/world/inventory/*`,
  `AbstractContainerMenu.java`, `ServerboundContainerClickPacket`.
- **Done when:** Inventory click sequences match Java and items persist with
  components.

### P4.7 Block entities `[ ]` `XL`

- **What:** Implement block-entity types and NBT: chest, trapped chest, barrel,
  furnace/blast furnace/smoker, crafting table, hopper, dispenser/dropper,
  sign, spawner, brewing stand, enchanting table, beacon, shulker box, etc.
- **Why:** Containers, automation, loot, and many plugins require block
  entities.
- **Reference:** `net/minecraft/world/level/block/entity/*`.
- **Done when:** Placing/opening/reloading each implemented block entity
  preserves contents and ticks correctly.

### P4.8 Crafting and recipe system `[~]` `L`

- **What:** Load recipe JSON and implement shaped/shapeless crafting, smelting,
  blasting, smoking, campfire cooking, stonecutting, smithing, and brewing.
- **Why:** Survival progression and most server plugins expect recipes.
- **Reference:** `net/minecraft/world/item/crafting/*`,
  `RecipeManager.java`.
- **Done when:** Recipe lookup and output match Java for the vanilla recipe
  set.
- **Progress (2026-11-06):** `src/loot.rs` parses all 1,470 shipped recipes:
  shaped (pattern/key), shapeless, smelting, blasting, smoking, campfire,
  stonecutting, smithing transform/trim, transmute, decorated pot, and the
  special crafting recipes. Ingredients are typed item/tag/alternatives and
  match through `folia_core::tags`; results carry item components. Still open:
  grid/container matching, recipe book/advancement unlocks, crafting menus,
  brewing, and exact recipe-selection order.

### P4.9 Health, damage, hunger, XP, and effects `[~]` `L`

- **What:** Port attributes, armor, enchantment protection, damage types,
  knockback, hunger/saturation, regeneration, XP orbs/levels, status effects,
  and totems.
- **Why:** Combat, death, food, and potions are fundamental.
- **Reference:** `LivingEntity.java`, `Player.java`,
  `net/minecraft/world/effect/*`, `AttributeMap`.
- **Done when:** Damage traces and effect durations match Java.
- **Progress (2026-11-06):** The P5.1 core supplies raw health subtraction,
  the 20-tick invulnerability window, death events, a typed attribute map, and
  `ClientboundSetHealth` for players. Armor/enchantment reduction, damage
  types, knockback, absorption, totems, hunger/regeneration, and effects remain
  unported; player sessions still own their own health/food fields.

### P4.10 Death, drops, respawn, and game modes `[~]` `M`

- **What:** Implement death screen, inventory drops, keep-inventory rules,
  respawn point selection, bed/anchor rules, spectator/creative/adventure
  behavior.
- **Why:** A server cannot be playable without death/respawn.
- **Done when:** Death and respawn round-trip player data and inventory.

### P4.11 Player data, stats, advancements, recipes `[~]` `M`

- **What:** Persist stats, advancements, unlocked recipes, ender chest, spawn
  point, game mode, and selected slot in playerdata NBT.
- **Why:** Plugins and vanilla progression read these; losing them on relog is
  unacceptable.
- **Done when:** All fields round-trip and unknown fields are preserved.
- **Progress (2026-11-05):** playerdata autosaves every 5 s while
  connected, from both the movement path and the 1 s read-timeout path,
  so an idle session survives a crash; unknown fields stay intact.
- **Progress (2026-11-05):** shutdown asks connected sessions to close and
  save before the main thread flushes terrain and rewrites `level.dat`; the
  wait is bounded at 5 s for clients still in login or configuration.
- **Progress (2026-11-05):** saves stamp `DataVersion` 4788
  (`level::DATA_VERSION`), matching vanilla's `NbtUtils.addCurrentDataVersion`;
  an older stored value is replaced on the next save.
- **Progress (2026-11-05):** playerdata models `Health`, `foodLevel`,
  `foodSaturationLevel`, `foodExhaustionLevel`, `XpLevel`, `XpP`, `XpTotal`,
  `Score`, and `Air`; an existing file's values are parsed and written back,
  and new players get vanilla's survival defaults.
- **Progress (2026-11-04):** Unknown fields are preserved: `PlayerSession`
  keeps every tag it does not model in `extra` and writes them back, so health,
  food, XP, ender chest, and modded data are not lost on save. Position,
  rotation, on-ground, dimension, selected slot, inventory, and cursor
  round-trip. Still missing: stats/advancements/recipes files, known vanilla
  fields the server could write itself, and multi-dimension positions.

### P4.12 Chat, commands, and permissions `[~]` `L`

- **What:** Implement chat (including 1.21 signing rules), command tree,
  command dispatch, tab completion, console sender, and permission checks.
- **Why:** Commands and chat are the primary admin/player interface.
- **Reference:** `net/minecraft/commands/*`, `PlayerChatMessage`,
  `ChatType`.
- **Done when:** Vanilla commands and plugin commands work from chat/console.

### P4.13 Tab list, scoreboard, teams, titles, boss bars `[ ]` `M`

- **What:** Implement player info, scoreboard objectives/teams, titles/action
  bar, boss bars, and sidebar display.
- **Why:** Plugins use these constantly for UI.
- **Done when:** A test plugin can create a scoreboard/title and the client
  receives the correct packets.

### P4.14 Sounds, particles, and world events `[ ]` `M`

- **What:** Add sound/particle/world-event packet helpers and trigger them from
  block/item/entity behavior.
- **Why:** Feedback is part of parity and plugin APIs.
- **Done when:** Representative actions emit Java-matching packets.

### P4.15 Multiplayer visibility and tracking `[ ]` `L`

- **What:** Track which players can see which entities/chunks; send spawn,
  metadata, equipment, animation, and removal packets per viewer.
- **Why:** Entity tracking is currently global broadcast, which does not scale
  and leaks entities across the world.
- **Done when:** Two players in distant chunks do not receive each other’s
  entities.

### P4.16 Player parity harness `[ ]` `M`

- **What:** Capture Java traces for movement, break times, placement, inventory
  clicks, damage, and respawn; replay in Rust.
- **Why:** Player behavior is full of edge cases that unit tests miss.
- **Done when:** CI runs the traces and reports drift.

---

# P5 — Entities, AI, combat, and spawning

**Why this phase:** The current entity store only supports generic position
sync and armor-stand gravity. Minecraft’s world is defined by mobs, item
entities, projectiles, and combat.

**Current state:** Global `HashMap<i32, Entity>`, basic type/UUID/position,
gravity/AABB, add/remove/sync packets. A separate typed metadata/living-entity
store now covers health, flags, pose, custom names, attributes, and equipment
packets, but it is not yet wired into sessions or per-viewer tracking. No
per-chunk storage, AI, spawning, or armor calculation.

### P5.1 Entity registry and metadata `[~]` `L`

- **What:** Add entity dimensions, data watchers, metadata serializers, flags,
  attributes, equipment, effects, and type-specific spawn data.
- **Why:** Clients need metadata for almost every visible behavior; plugins need
  typed entity objects.
- **Reference:** `net/minecraft/world/entity/Entity.java`,
  `SynchedEntityData.java`, `EntityType.java`.
- **Done when:** Metadata packets match Java for representative entities.
- **Progress (2026-11-06):** `entity_metadata.rs` implements a typed
  `MetadataValue`/`DataValue` store keyed by entity id with vanilla dirty
  semantics and `ClientboundSetEntityData` encode/decode. Living-entity indices
  0-9 are modeled (shared flags, air supply, custom name/visibility, silent, no
  gravity, pose, ticks frozen, living flags, health). `LivingEntity` adds raw
  damage with the 20-tick invulnerability window, death event `3`, poof event
  `60` after 20 death ticks, and `ClientboundSetHealth` for players. Typed
  attributes cover max health, armor, armor toughness, attack damage/speed,
  movement speed, knockback resistance, and follow range. The eight equipment
  slots encode/decode through `ClientboundSetEquipment` (ordinal + continuation
  bits) and are readable from the store. `entity.rs` adds
  `spawn_living`/`hurt_entity`/`tick_living_entities`/`set_entity_equipment`
  bridges; join/player sessions are not wired. Still open: effect particles,
  arrow/stinger counts, sleeping pos, remaining serializers, initial
  non-default metadata at spawn, per-viewer dirty tracking, armor points from
  equipment, effects/absorption/damage types, and a Java packet fixture.

### P5.2 Entity lifecycle and per-chunk storage `[ ]` `L`

- **What:** Replace the global entity map with per-chunk/per-region storage,
  UUID lookup, persistence, passengers/vehicles, leashes, and despawn rules.
- **Why:** Needed for chunk unload, region threading, and multiplayer tracking.
- **Done when:** Entities save/load with chunks and move between chunk stores
  correctly.

### P5.3 Extended movement and physics `[ ]` `L`

- **What:** Add fluid movement, ladders, cobwebs, honey/slime, ice, explosions,
  piston pushes, minecarts, boats, and entity pushing.
- **Why:** Armor-stand gravity is only the first physics case.
- **Done when:** Differential traces cover water, ladders, and vehicles.

### P5.4 Item, projectile, and falling entities `[ ]` `M`

- **What:** Implement item entities, XP orbs, arrows, tridents, snowballs, eggs,
  fireballs, fireworks, falling blocks, TNT, and pickup/despawn behavior.
- **Why:** These connect combat, mining, farming, and redstone.
- **Done when:** Projectile trajectories and item pickup match Java traces.

### P5.5 Pathfinding and navigation `[ ]` `XL`

- **What:** Port node evaluators (walk, swim, fly, amphibious), A* pathfinding,
  path following, stuck detection, and door/gate interaction.
- **Why:** No mob AI is credible without pathfinding.
- **Reference:** `net/minecraft/world/level/pathfinder/*`,
  `net/minecraft/world/entity/ai/navigation/*`.
- **Done when:** A zombie can path around obstacles and through doors like Java.

### P5.6 AI goals, sensors, and brains `[ ]` `XL`

- **What:** Implement goal selectors, priorities, sensors, memories, and brain
  activities for the mobs that need them.
- **Why:** Different mobs use different AI architectures; a single generic
  “wander” goal is not parity.
- **Reference:** `net/minecraft/world/entity/ai/goal/*`,
  `.../ai/sensing/*`, `.../ai/brain/*`.
- **Done when:** Representative passive/hostile mobs behave comparably in
  scripted scenarios.

### P5.7 Passive mobs `[ ]` `L`

- **What:** Implement cows, pigs, sheep, chickens, rabbits, horses, wolves,
  cats, foxes, bees, pandas, etc., including breeding, tempt/follow, panic,
  and drops.
- **Why:** Passive mobs are the baseline world population and food/wool source.
- **Done when:** Spawn/breed/drop behavior matches Java.

### P5.8 Hostile mobs `[ ]` `XL`

- **What:** Implement zombies, skeletons, creepers, spiders, endermen, witches,
  pillagers, drowned, husks, etc., including targeting, attacks, and drops.
- **Why:** Survival gameplay and many plugins depend on hostile mobs.
- **Done when:** Combat scenarios match Java damage/timing.

### P5.9 Villagers, trading, and raids `[ ]` `XL`

- **What:** Professions, workstations, trading offers, restocking, breeding,
  iron golems, raids, and village mechanics.
- **Why:** Villagers are central to progression and a common plugin target.
- **Done when:** Trades and villager schedules match Java.

### P5.10 Bosses `[ ]` `XL` later

- **What:** Ender dragon fight, wither, boss bars, arena behavior, drops.
- **Why:** End-game content; can be deferred until dimensions and combat work.
- **Done when:** Boss fights can be completed with parity behavior.

### P5.11 Combat system `[ ]` `L`

- **What:** Attack cooldown, critical hits, knockback, armor, enchantments,
  damage types, death loot, XP, shields, and projectile damage.
- **Why:** Combat ties together items, entities, attributes, and effects.
- **Done when:** Damage traces match Java for melee/ranged/enchanted cases.

### P5.12 Natural spawning and spawners `[ ]` `L`

- **What:** Spawn cycles, light/biome/height checks, mob caps, persistence,
  despawn, spawn eggs, monster spawners, breeding, and village spawning.
- **Why:** Without spawning the world is empty.
- **Done when:** Spawn rates/caps roughly match Java and configured spawners
  work.

### P5.13 Entity tracking and packets `[ ]` `M`

- **What:** Per-viewer spawn/remove, position/velocity updates, metadata,
  equipment, animation, effects, passengers, and leashes.
- **Why:** Needed for multiplayer and bandwidth control.
- **Done when:** Distant entities are not sent; metadata changes reach only
  tracking players.

### P5.14 Entity parity and performance harness `[ ]` `M`

- **What:** Add scripted entity scenarios and tick-level comparisons; profile
  entity ticking and pathfinding.
- **Why:** Entity behavior is timing-sensitive; performance regressions are
  easy to hide.
- **Done when:** CI covers representative mobs and a tick budget benchmark.

---

# P6 — Block behavior, redstone, fluids, and ticks

**Why this phase:** Blocks are currently inert data. Real Minecraft requires
neighbor updates, scheduled ticks, fluid flow, redstone, explosions, and
block-specific behavior.

**Current state:** The P3.10 tick slice provides update flags, neighbour and
scheduled/random tick queues, falling blocks, plant support, grass spread,
water/lava flow, and wire/lever/button redstone. Pistons, explosions, block
entities, the remaining redstone components, and full P1.5 metadata are open.

### P6.1 Block behavior framework `[~]` `L`

- **What:** Define a block behavior interface/table with neighbor changed,
  scheduled tick, random tick, use, place, break, entity interaction, shape,
  support, and comparator output.
- **Why:** Redstone, fluids, crops, doors, and pistons all need block-specific
  callbacks.
- **Done when:** At least 20 vanilla block types have behavior tests.
- **Progress (2026-11-06):** `tick::BlockBehavior` and `tick::BehaviorTable`
  provide random tick, scheduled tick, neighbor changed, shape update, and
  place hooks; falling blocks, plants, grass, fluids, wire, lever, and buttons
  are registered. Use/break/entity interaction and comparator output remain
  open.

### P6.2 Fluid system `[~]` `L`

- **What:** Implement water/lava levels, source creation, flow direction,
  spread distance, waterlogging, infinite sources, lava/water interactions,
  bubble columns, and fluid rendering data.
- **Why:** Fluids are foundational and interact with nearly every system.
- **Reference:** `net/minecraft/world/level/material/*`,
  `LiquidBlock.java`, `FlowingFluid.java`.
- **Done when:** Flow traces match Java for slopes, sources, and waterlogging.
- **Progress (2026-11-06):** `fluid.rs` covers source/flowing/falling levels,
  the 5/30-tick delays, 1/2 drop-off, downward priority, slope search toward
  holes, water source conversion, waterlogged placement, and source-removal
  drain, with focused flow tests. Lava/water quenching, lava fire, bubble
  columns, block-destruction drops, and a Java flow trace fixture remain.

### P6.3 Redstone core `[~]` `L`

- **What:** Implement signal strength, power propagation, weak/strong power,
  update ordering, and redstone components' interfaces.
- **Why:** Redstone is a large but self-contained parity domain.
- **Reference:** `net/minecraft/world/level/redstone/*`,
  `SignalGetter.java`, `RedstoneWireBlock.java`.
- **Done when:** Basic wire/torch/lever circuits match Java tick-for-tick.
- **Progress (2026-11-06):** `redstone.rs` implements `SignalGetter`-style
  strong/weak queries (`getSignal`, `getDirectSignal`, `getDirectSignalTo`,
  `getControlInputSignal`, `hasNeighborSignal`, `getBestNeighborSignal`), wire
  target strength and 0..15 decay through the neighbour queue, lever toggling,
  and button press with a scheduled release. Redstone torch/repeater/
  comparator/observer/pressure-plate/door sources and a Java timing fixture are
  still open.

### P6.4 Redstone components `[~]` `XL`

- **What:** Port wire, torch, repeater, comparator, observer, piston/sticky
  piston, rails, buttons, levers, pressure plates, doors/trapdoors, dispensers,
  droppers, hoppers, daylight detectors, target blocks, note blocks, bells,
  copper bulbs, and crafters.
- **Why:** These are the visible redstone surface players and plugins use.
- **Done when:** Each component has a timing fixture against Java.
- **Progress (2026-11-06):** Wire, lever, and button are implemented; the
  remaining components are still explicit gaps.

### P6.5 Pistons and moving blocks `[ ]` `L`

- **What:** Implement piston push/pull, block-moving entities, slime/honey
  interactions, and entity pushing.
- **Why:** Pistons are a common redstone/automation and plugin dependency.
- **Done when:** Piston sequences match Java including block entities.

### P6.6 Explosions `[ ]` `M`

- **What:** Implement explosion ray casting, block destruction, drops,
  entity damage/knockback, TNT, creepers, beds, respawn anchors, and end
  crystals.
- **Why:** Explosions affect terrain, combat, and redstone.
- **Done when:** Explosion block/damage results match Java fixtures.

### P6.7 Random-tick behaviors `[ ]` `L`

- **What:** Crop growth, sapling growth, leaf decay, grass/mycelium spread,
  fire spread, snow/ice formation/melt, vine growth, and bone meal.
- **Why:** The world needs to change over time without player input.
- **Done when:** Growth timings and block results match Java.

### P6.8 Sculk sensors, vibrations, and game events `[ ]` `M`

- **What:** Implement game event emission/listening, vibration frequencies,
  sculk sensor, calibrated sensor, and warden-related events (if mobs exist).
- **Why:** 1.19+ mechanics and a dependency for deep dark content.
- **Done when:** Sensor activation fixtures match Java.

### P6.9 Automation/block-entity interactions `[ ]` `M`

- **What:** Hopper transfer, furnace smelting, dispenser use, dropper, brewing,
  enchanting, beacon effects, and comparator outputs.
- **Why:** These connect containers, items, and redstone.
- **Done when:** Item transfer/smelting sequences match Java.

### P6.10 Redstone/block parity harness `[ ]` `M`

- **What:** Build deterministic circuit tests and block-update sequence captures
  from Java; compare tick-by-tick.
- **Why:** Redstone bugs are often one-tick ordering issues that unit tests miss.
- **Done when:** CI runs a circuit suite with zero unexplained drift.

---

# P7 — Dimensions and world systems

**Why this phase:** The server currently generates only the overworld and has
no portals, weather, time, or multi-world lifecycle. Folia is a multi-dimension
server.

**Current state:** `WorldStorage` has dimension paths; generation and gameplay
are overworld-only. No time/weather/portals.

### P7.1 Dimension abstraction `[ ]` `M`

- **What:** Model dimension type, min/max Y, height, logical height, coordinate
  scale, ambient light, ceiling/skylight, fixed time, and per-dimension
  generator.
- **Why:** Nether/End and plugins need a real dimension API.
- **Done when:** The overworld path uses the abstraction with no behavior
  change.

### P7.2 Nether generation `[ ]` `L`

- **What:** Port `minecraft:nether` noise settings, density functions, surface
  rules, carvers, features, structures, lava sea, and bedrock.
- **Why:** A complete server needs the Nether.
- **Done when:** Nether chunk parity fixtures pass.

### P7.3 End generation `[ ]` `L`

- **What:** Port the End central island, outer islands, end features, gateways,
  and dragon arena.
- **Why:** End progression and end-game content.
- **Done when:** End chunk parity fixtures pass.

### P7.4 Portals, travel, and respawn `[ ]` `M`

- **What:** Nether portal search/linking/cooldowns, end portal, gateway,
  dimension change packets, respawn anchors/beds, and cross-dimension
  teleports.
- **Why:** Dimensions are only usable with travel.
- **Done when:** A player can travel between dimensions and respawn correctly.

### P7.5 Time, weather, and lightning `[ ]` `M`

- **What:** Day/night cycle, moon phases, rain/thunder, lightning, snow, and
  client time/weather packets.
- **Why:** World atmosphere and mechanics (sleep, mobs, crops).
- **Done when:** Time/weather packets match Java and affect gameplay.

### P7.6 World border, spawn, and game rules `[ ]` `M`

- **What:** World border, spawn chunks, forced chunks, difficulty, game rules,
  spawn protection, and player spawn selection.
- **Why:** Server administration and vanilla behavior.
- **Done when:** Commands/config changes take effect and persist.

### P7.7 Save, autosave, backup, and migration `[ ]` `M`

- **What:** Full world save (level.dat, playerdata, chunks, entities, POI,
  advancements, stats), autosave, backup, and version migration.
- **Why:** Data loss is unacceptable; current saves are minimal.
- **Done when:** A world round-trips through shutdown/startup with no loss.

### P7.8 Multiple worlds `[ ]` `M`

- **What:** Support multiple world directories and per-world generators,
  entities, players, and settings.
- **Why:** Common server setup and plugin requirement.
- **Done when:** Two worlds can run simultaneously with separate state.

### P7.9 Custom world settings and flat worlds `[ ]` `M`

- **What:** Parse `level.dat` generator settings, support flat worlds, custom
  seeds, and Paper `generate-flat-bedrock`.
- **Why:** Existing worlds must load correctly; current fallback is seed 0.
- **Done when:** A real world with custom settings loads and generates
  correctly.

---

# P8 — Networking, authentication, and multiplayer

**Why this phase:** The current path is an offline-mode join demo with a small
subset of play packets. A public server needs encryption, Mojang auth, full
packet coverage, multiple players, and robustness.

**Current state:** Offline mode only; captured configuration registries; 7×7
chunk stream; limited play packets; no online-mode encryption or chat signing.

### P8.1 Online-mode encryption `[ ]` `L`

- **What:** Implement RSA key exchange, AES/CFB8 encryption, compression
  negotiation, Mojang session server authentication, and profile properties.
- **Why:** Public servers require authenticated accounts.
- **Reference:** `net/minecraft/server/network/ServerLoginPacketListenerImpl.java`,
  `Connection.java`, `Crypt.java`.
- **Done when:** A real online-mode client can join with a valid session.

### P8.2 Chat signing and profile keys `[ ]` `L`

- **What:** Implement 1.21.11 profile keys, signed chat, message chains, and
  the enforcement/report path required by online mode.
- **Why:** Modern clients expect signed chat; offline mode can bypass this.
- **Reference:** `net/minecraft/network/chat/*`, `PlayerChatMessage`,
  `SignedMessageChain`.
- **Done when:** Signed chat messages are accepted/validated and displayed.

### P8.3 Full play-packet coverage `[ ]` `XL`

- **What:** Enumerate every clientbound/serverbound play packet for 1.21.11,
  implement handlers for supported features, and explicitly reject/log
  unsupported ones instead of silently ignoring them.
- **Why:** Silent ignores cause desyncs; full coverage is required for parity.
- **Done when:** A packet coverage report shows each ID mapped to a handler or
  an explicit unsupported path.

### P8.4 Connection handling and limits `[ ]` `M`

- **What:** Add per-connection threads/tasks, read/write timeouts, keepalive
  timing, packet size limits, rate limits, backpressure, and slow-client
  handling.
- **Why:** Prevents lag/DoS from a single connection.
- **Done when:** Malformed/hostile clients cannot stall the server.

### P8.5 Configuration-phase completeness `[ ]` `M`

- **What:** Generate registries/tags/feature flags/known packs from Rust rather
  than replaying captured bytes; support configuration re-entry and transfer.
- **Why:** Captured bytes only work for one version/world configuration.
- **Done when:** Join works with a freshly generated configuration phase.

### P8.6 Play join/leave/respawn/teleport `[ ]` `L`

- **What:** Complete join sequence, player info, chunk batch, spawn position,
  respawn, dimension change, teleport, and quit cleanup.
- **Why:** These are the core lifecycle packets.
- **Done when:** Multiple clients can join, respawn, teleport, and leave
  cleanly.

### P8.7 Status/favicon/version `[ ]` `S`

- **What:** Add favicon, player sample, version name/protocol, secure-chat flag,
  and correct player counts.
- **Why:** Server list presentation and compatibility.
- **Done when:** Status JSON matches Java for the same config.

### P8.8 Proxy forwarding `[ ]` `M` optional

- **What:** Support BungeeCord/Velocity modern forwarding and proxy-aware
  addresses.
- **Why:** Common production deployment.
- **Done when:** A proxy can pass players with UUID/skin data.

### P8.9 Protocol robustness and fuzzing `[ ]` `M`

- **What:** Fuzz VarInts, NBT depth, strings, compression, packet lengths, and
  malformed handshakes; add limits and tests.
- **Why:** Network input is untrusted.
- **Done when:** Fuzz targets run in CI with no panics/hangs.

---

# P9 — Plugin compatibility

**Why this phase:** Keeping unmodified jars working is a core project goal. The
bridge now covers lifecycle, a host event bus, commands, a task table, basic
world/block access, player identity/vitals/game mode, storage-slot inventories,
and basic item meta.

**Current state:** Lazy JVM, real class loading, events, commands, scheduler,
JNI ping, `World`/`Block` read/write over stored/cached chunks, `Player`
playerdata-backed state, `PlayerInventory` slots 0-35, and `ItemStack` type/
count/damage/name/lore/unbreakable. Three block/interact events are
host-callable with cancellation. Still missing: live-session sync for
health/food/inventory/teleport/messages, live event dispatch from `join.rs`,
armor/off-hand slots, enchantment/PDC persistence, entities, services, config,
permissions, and NMS shims.

### P9.1 Formalize the Rust↔Java ABI `[~]` `M`

- **What:** Replace ad-hoc JNI methods with a typed command/event protocol:
  versioned calls, error codes, ownership/lifetime rules, and thread-affinity
  rules.
- **Why:** The API surface will grow to hundreds of calls; ad-hoc JNI becomes
  unmaintainable.
- **Done when:** A documented ABI covers existing bridge calls and has a
  compatibility test.
- **Progress (2026-11-06):** The native surface is now grouped into world,
  player, inventory, and item calls with explicit signatures in `Bridge.java`
  and a field-by-field encoded item/inventory payload. `BridgeApiTest` pins the
  codec and registry shims; the Rust and Java tests do not yet version the
  protocol or test every signature.

### P9.2 Complete event coverage `[~]` `L`

- **What:** Fire events for join/quit, block break/place, interact, inventory
  click/open/close, chat, command, death/respawn, entity spawn/death, world
  load/unload, weather/time, and plugin lifecycle.
- **Why:** Plugins hook events for nearly all behavior.
- **Done when:** A test plugin can observe each event with cancellable results
  respected by the server.
- **Progress (2026-11-06):** `BlockBreakEvent`, `BlockPlaceEvent`, and
  `PlayerInteractEvent` are constructed by the bridge, dispatched through the
  Bukkit listener bus, and return cancellation to Rust through
  `PluginRuntime::fire_block_break`/`fire_block_place`/`fire_player_interact`.
  `FOLIA_BRIDGE_SELFTEST=1` verifies all three end to end. The vanilla
  `join.rs` break/place/interact handlers still call `block_update` directly, so
  real gameplay does not yet fire these events; join/quit beyond
  `PlayerJoinEvent`, inventory, chat, death/respawn, entity, and world events
  remain unimplemented.

### P9.3 Core API objects `[~]` `XL`

- **What:** Implement `Server`, `World`, `Player`, `Entity`, `LivingEntity`,
  `Block`, `Location`, `Chunk`, `Inventory`, `ItemStack`, `ItemMeta`, and
  related interfaces backed by Rust state.
- **Why:** Public-API plugins cannot function without these.
- **Done when:** A representative plugin can read/modify blocks, players, and
  entities through normal Bukkit APIs.
- **Progress (2026-11-06):** `World`/`Block`/`BlockData`/`BlockState`/`Chunk`
  proxies cover block type, location, highest-Y, biome, sky/block light, and
  stored/cached block edits. `Player` covers world/location/inventory/game
  mode/messages/teleport/health/food/saturation; `setGameMode` forwards to a
  live session, while the rest persists through `PlayerSession` playerdata.
  `PlayerInventory` covers slots 0-35; `ItemStack`/`ItemMeta` cover
  type/count/damage/name/lore/unbreakable. Live-session sync, armor/off-hand,
  entities, `LivingEntity`, full chunk snapshots, and persistent-data containers
  remain open.

### P9.4 Scheduler parity `[ ]` `M`

- **What:** Implement sync, async, region, entity, delayed, repeating, and
  cancellation semantics, including task IDs and owner lifecycle.
- **Why:** Folia plugins specifically use region/entity schedulers.
- **Done when:** Scheduler tests match Bukkit/Folia behavior and tasks stop on
  plugin disable.

### P9.5 Commands and permissions `[ ]` `M`

- **What:** Complete `plugin.yml` commands, aliases, `CommandMap`,
  `PluginCommand`, tab completion, console senders, and permissions.
- **Why:** Admin/player commands are a primary plugin surface.
- **Done when:** A plugin command runs from console/chat with correct
  permission checks.

### P9.6 Configuration and YAML `[ ]` `M`

- **What:** Implement `getConfig`, `saveDefaultConfig`, `reloadConfig`,
  `saveConfig`, YAML parsing/writing, and file encoding behavior.
- **Why:** Almost every plugin reads config on enable.
- **Done when:** A config round-trips and comments/defaults behave like Bukkit.

### P9.7 Services, plugin manager, and dependencies `[ ]` `M`

- **What:** Implement `ServicesManager`, plugin descriptions, dependency/load
  order (`depend`, `softdepend`, `loadbefore`), `PluginManager` queries, and
  plugin data folders.
- **Why:** Plugin ecosystems rely on service lookup and load ordering.
- **Done when:** A multi-plugin test set loads in the correct order and shares
  a service.

### P9.8 Inventory, ItemStack, and PDC APIs `[~]` `L`

- **What:** Expose inventory contents, item metadata, enchantments,
  durability, and `PersistentDataContainer`.
- **Why:** Common plugin operations and cross-plugin data sharing.
- **Done when:** A plugin can create/modify items and have changes persist and
  appear on the client.
- **Progress (2026-11-06):** Storage slots 0-35, item type/count, damage,
  display name, lore, and unbreakable round-trip through the Rust component
  model and playerdata. `new ItemStack(...)` works through the registry shim.
  Enchantments, item flags, custom model data, PDC, armor/off-hand, and client
  inventory updates remain open.

### P9.9 World/entity APIs `[~]` `L`

- **What:** Block get/set, biome, time/weather, spawn entity, teleport, sounds,
  particles, explosions, and entity queries.
- **Why:** These are the most common plugin actions after events/commands.
- **Done when:** A plugin can build a small structure and spawn a mob through
  the API.
- **Progress (2026-11-06):** Block get/set, highest-Y, biome, and light read
  from `WorldStorage`; block edits persist and broadcast. Time/weather are
  static defaults, and entity spawn/query, sounds, particles, explosions,
  weather, and on-demand chunk generation remain open. World reads/writes only
  cover stored or currently cached columns.

### P9.10 NMS/CraftBukkit shims `[ ]` `XL` long tail

- **What:** Generate Java shim classes for the most-used `CraftWorld`,
  `CraftPlayer`, `CraftEntity`, `CraftItemStack`, and NMS access points, each
  forwarding to Rust or failing loudly.
- **Why:** Many plugins use NMS/reflection; supporting them requires classes to
  exist.
- **Done when:** A curated NMS plugin corpus loads and basic operations work.

### P9.11 Plugin compatibility corpus `[ ]` `L`

- **What:** Build a tiered test set: hello/lifecycle, public API plugins,
  scheduler-heavy plugins, inventory plugins, NMS plugins. Add CI runs.
- **Why:** Compatibility is the product; it must be measured against real jars,
  not assumptions.
- **Done when:** Tier 1–2 plugins run unchanged and failures are tracked.

### P9.12 Isolation, logging, and failure handling `[ ]` `M`

- **What:** Catch plugin exceptions, attribute logs to plugins, prevent one
  plugin from killing the server, and define reload/disable behavior.
- **Why:** Production servers run untrusted/imperfect plugins.
- **Done when:** A throwing plugin is disabled/reported without crashing the
  process.

---

# P10 — Folia regionization and concurrency

**Why this phase:** Folia’s defining feature is regionized multithreading. The
current `region.rs` has task queues and parallel tick calls, but world, entity,
and player state are still global statics. True Folia parity requires ownership
and scheduling at the region level.

**Current state:** `tick_parallel(4)` runs independent region queues, but no
world data is isolated per region and no cross-region safety exists.

> **Start early:** P10.1 should be designed before P4/P5 store lots of mutable
> gameplay state. Ideally introduce ownership handles during P5/P6 even if the
> actual parallel tick loop comes later.

### P10.1 Ownership model design `[ ]` `M`

- **What:** Define region ownership for chunks, entities, block entities,
  players, POI, and tasks. Decide which data is region-local, shared/read-only,
  or global. Define cross-region message types.
- **Why:** This is the hardest architectural decision; retrofitting it later
  means rewriting gameplay state.
- **Done when:** An ADR documents the model and a prototype enforces ownership
  on one subsystem.

### P10.2 Regionizer `[ ]` `L`

- **What:** Implement chunk→region mapping, region merging/splitting, region
  lifecycle, and ownership transfer.
- **Why:** Folia schedules ticks per region, not per chunk.
- **Reference:** `io/papermc/paper/threadedregions/ThreadedRegionizer.java`,
  `TickRegions.java`, `RegionizedWorldData.java`.
- **Done when:** A region can grow/shrink and transfer chunks without data
  races.

### P10.3 Per-region world state `[ ]` `XL`

- **What:** Replace global `OnceLock`/`Mutex` stores with region-owned world
  data: entity lists, block entities, POI, scheduled ticks, players, and
  chunk maps.
- **Why:** Shared global state cannot be ticked in parallel.
- **Done when:** Two regions can tick simultaneously without sharing mutable
  gameplay state.

### P10.4 Region tick loop `[ ]` `L`

- **What:** Tick regions on worker threads with 20 TPS, per-region budgets,
  catch-up/backlog policy, and safe shutdown.
- **Why:** This is Folia’s core execution model.
- **Done when:** Multiple regions tick in parallel under load while preserving
  per-region ordering.

### P10.5 Cross-region tasks and teleports `[ ]` `L`

- **What:** Implement delayed cross-region scheduling, entity transfer, player
  teleport, block updates at region borders, and ownership handoff.
- **Why:** Players and entities routinely cross region boundaries.
- **Done when:** A player can walk/teleport across regions without losing
  state or deadlocking.

### P10.6 Folia scheduler APIs `[ ]` `M`

- **What:** Implement RegionScheduler, EntityScheduler, AsyncScheduler,
  GlobalRegionScheduler, and their Bukkit bridge equivalents.
- **Why:** Folia plugins depend on these instead of the legacy scheduler.
- **Done when:** A Folia-style plugin schedules work on the correct region and
  observes cancellation on unload.

### P10.7 Chunk system integration `[ ]` `L`

- **What:** Integrate tickets/loaders with regions and player chunk loaders;
  ensure generation/lighting tasks run on the correct threads.
- **Why:** Chunk ownership is the foundation of region safety.
- **Done when:** Chunk load/unload/gen/light never touches another region’s
  state.

### P10.8 Watchdog, shutdown, and failure handling `[ ]` `M`

- **What:** Add region tick watchdog, stuck-region reporting, graceful shutdown,
  region crash isolation, and data flush.
- **Why:** A stuck region must not silently freeze the whole server.
- **Done when:** A simulated stuck region is reported and the server can shut
  down cleanly.

### P10.9 Concurrency testing `[ ]` `L`

- **What:** Add stress tests for region merge/split, cross-region teleports,
  entity transfer, chunk ownership, and task ordering; run with sanitizers
  where available.
- **Why:** Data races are the highest-risk part of Folia parity.
- **Done when:** Stress suite passes repeatedly with no races/deadlocks.

### P10.10 Folia API parity `[ ]` `M`

- **What:** Implement Folia-specific APIs: current region, region ownership
  queries, region/entity scheduler access, and regionized server state.
- **Why:** Folia plugins compile against these APIs.
- **Done when:** A Folia API plugin can query region state and schedule work.

### P10.11 Parallel performance `[ ]` `M`

- **What:** Measure scaling from 1→N regions, tune tick budgets, reduce
  contention, and parallelize generation/lighting.
- **Why:** Folia’s value is performance; correctness without scaling is not
  enough.
- **Done when:** Benchmarks show near-linear scaling for independent regions
  up to available cores.

---

# P11 — Parity, performance, and release hardening

**Why this phase:** A feature-complete server is not a usable server until it is
tested against Java, performs at 20 TPS, survives hostile input, and can be
deployed.

### P11.1 Expand the differential harness `[ ]` `L`

- **What:** Cover protocol, worldgen, lighting, physics, redstone, entities,
  inventory, chat, and plugin events. Capture Java packets/ticks and replay
  them against Rust.
- **Why:** Parity claims need evidence, not “looks right.”
- **Done when:** Every major subsystem has at least one differential suite.

### P11.2 Automate oracle capture and version pinning `[ ]` `M`

- **What:** One command regenerates all Java fixtures, records hashes and
  versions, and fails tests on mismatch.
- **Why:** Manual fixtures rot silently.
- **Done when:** CI can validate/reproduce the oracle.

### P11.3 Fuzzing and property tests `[ ]` `M`

- **What:** Fuzz NBT, SNBT, VarInt, packet framing, chunk palettes, worldgen
  JSON, and plugin-provided data.
- **Why:** Untrusted data is everywhere; panics are unacceptable.
- **Done when:** Fuzz targets run continuously and no known crashes remain.

### P11.4 Benchmark suite `[ ]` `M`

- **What:** Add benchmarks for chunk generation, lighting, ticking, entity AI,
  pathfinding, network throughput, and memory per player/chunk.
- **Why:** Performance regressions are otherwise invisible.
- **Done when:** CI tracks budgets and flags regressions.

### P11.5 Performance optimization `[ ]` `L`

- **What:** Optimize hot paths: worldgen allocation, chunk I/O, lighting,
  entity ticking, packet encoding, and lock contention.
- **Why:** The current debug login generation is far too slow for production.
- **Done when:** Release targets are met (see acceptance gates).

### P11.6 Documentation `[ ]` `M`

- **What:** Write architecture docs, config reference, operator guide, plugin
  compatibility matrix, world migration notes, and contributor porting guide.
- **Why:** The project spans many systems and will outlive any one session.
- **Done when:** A new contributor can build, test, and add a slice without
  reading the whole tree.

### P11.7 Packaging and deployment `[ ]` `M`

- **What:** Release binaries for Windows/Linux, Docker image, systemd/service
  scripts, graceful shutdown, backup tooling, and versioned releases.
- **Why:** Production usability.
- **Done when:** A user can download, configure, and run a server without a
  Rust toolchain.

### P11.8 Security review `[ ]` `M`

- **What:** Review authentication, packet handling, path traversal, plugin
  classloading, permission checks, and denial-of-service limits.
- **Why:** A public server is an exposed service.
- **Done when:** Known issues are fixed and a checklist is part of release.

### P11.9 Acceptance gates for “1.0” `[ ]`

Define and verify:

- [ ] A vanilla client can complete a survival loop: join, mine, craft, smelt,
  eat, fight, die, respawn, use a bed, travel to the Nether/End, and save.
- [ ] Generated overworld/Nether/End structure + feature parity exceeds agreed
  thresholds (start: 99.9% block-class, 99% exact-name on multi-seed fixtures).
- [ ] Lighting and block updates are byte-identical on captured fixtures.
- [ ] Redstone circuits are tick-exact on a curated suite.
- [ ] Entity physics/combat traces match Java within tolerance.
- [ ] A tiered plugin corpus runs unchanged.
- [ ] 20 TPS with 20+ players and a defined render/simulation distance.
- [ ] Cold-world login reaches play within 2 seconds on the target machine.
- [ ] No crashes/panics under fuzzing and a 24-hour soak test.
- [ ] World save/restart/backup causes no data loss.

---

# Critical path: suggested execution order

This is the shortest path to a genuinely playable server, not the full parity
long tail.

1. **P0 cleanup** — make the baseline green and recoverable.
2. **P1.1–P1.5** — complete registries, tags, item components, block metadata.
3. **P2.1–P2.3 + P2.7** — feature framework, placement, ores, lakes/disks.
4. **P3.1–P3.10** — chunk tickets/status pipeline and real lighting.
5. **P4.1–P4.6** — player physics, breaking/placing, item use, menus.
6. **P5.1–P5.4 + P5.11–P5.12** — entity metadata, projectiles, combat, spawning.
7. **P6.1–P6.4** — block behavior, fluids, redstone core/components.
8. **P7.1–P7.4** — dimensions and portals.
9. **P8.1–P8.6** — online mode and full join/play lifecycle.
10. **P9.1–P9.9** — plugin API breadth.
11. **P10.1–P10.7** — region ownership and parallel ticking.
12. **P11.1–P11.9** — parity, performance, hardening, release.

## What not to do

- Do not port every Java class literally. Port behavior and keep differential
  tests.
- Do not build gameplay systems on global mutable statics if region threading
  is the goal.
- Do not add features that write directly to chunk data without going through
  the chunk-status pipeline; it breaks seeding and light updates.
- Do not claim parity from visual inspection. Capture Java output.
- Do not treat plugin API breadth as a single task; it is an incremental corpus
  effort.

## Current next three actions

1. Complete **P0.1–P0.3** (formatting, warnings, asset tracking, docs).
2. Start **P2.3** (ores) on top of **P2.1–P2.2** (feature framework and
   placement), using `feature_data.json`.
3. In parallel, implement **P3.5–P3.8** (real light storage and engines), since
   the current light payload is the largest correctness gap for gameplay.
