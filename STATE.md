# Folia-RS Agent State

> This file is the short-lived working memory for the next agent session.
> Update it after every completed task. Keep it short and factual.
> `TODO.md` is the full roadmap. `AGENTS.md` is the operating manual.

Last updated: 2026-11-07

## Current status

- Current commit: see `git log`; all work is committed and pushed to `main`.
- Tests: **1052 passing** on the last full merged gate, 1 ignored
  (2048-bit secure-chat benchmark), plus 6 new `density_parity` tests in this
  worktree (focused runs green; full gate not re-run here). Parity is
  **98.75% block-class (6067/6144) / 95.88% exact-name (5891/6144)**; 28 of 33
  fixture ore blocks match.
- Formatting/Clippy: clean; no warnings.
- Worldgen: 218/224 configured feature types, 888/888 placement modifiers,
  483 shipped village templates, recursive jigsaw, Nether carvers, End
  spikes/gateways/chorus, dimension storage and portal math.
- Gameplay: brewing/potions/food, hunger, chat signing, armor/effects, crops,
  leaf decay, chest loot, item/XP entities, hoppers, pistons, block-entity
  tickers, region ownership, region tick driver, and live bridge
  chat/time/weather/sound/particles.
- Major gaps: feature-pool placement and non-village structure templates;
  live session dimension switching; effect/menu packets; shared per-viewer
  entity tracking and live AI goals; full plugin API; true parallel
  per-region world state (the region driver scopes chunk work, but
  `WorldStorage` is still one global lock).
- Gameplay: brewing/potions/food, hunger, chat signing, armor/effects,
  status-effect packets, shared per-viewer tracking, crops, leaf decay, chest
  loot, item/XP entities, hoppers, pistons, block-entity tickers, region
  ownership, and live bridge chat/time/weather/sound/particles.
- Major gaps: feature-pool placement and non-village structure templates;
  live session dimension switching; container menu packets; live per-viewer
  tracking wiring and live AI goals; full plugin API; real region tick
  threading.

## Current focus

1. P2.8 feature-pool placement and the remaining six configured feature types.
2. P7 portal-driven dimension switching and non-village structure templates.
3. P5 live AI goals and wiring the shared tracker into live session visibility.
4. P6 container menus/packets and deeper redstone/automation.
5. P9.3 remaining bridge API surface and secure-chat login wiring.
6. P2.3 ore parity — `NoiseChunk` vein interpolation is landed; remaining gaps
   are the 3x3-chunk feature write region, ellipsoid/vein edges, and the
   `disk_*`/`underwater_magma` entries in the `underground_ores` step.

## Next queue

- [x] P0.1-P0.4, P0.6 foundation, docs, assets, CI, commit policy.
- [ ] P0.5 reproducible oracle data (deferred).
- [~] P1.4 components/prototypes (network codecs, hashing, fixtures).
- [~] P1.5 shapes (support/occlusion/waterlogging/per-state random ticks).
- [~] P1.7 loot/recipes (block/chest drops; entity loot next).
- [~] P2.1/P2.2 218/224 feature types and all modifiers.
- [~] P2.8 recursive jigsaw + village templates (feature pools/non-village).
- [~] P3.5-P3.8 light (Java fixtures and exact occlusion columns).
- [~] P3.10 ticks/redstone (leaf/crop ticks; automation/menus remain).
- [~] P4.1-P4.4 physics/breaking/item entities (live hunger and special movement).
- [~] P4.8 crafting/brewing (menus, packets, recipe book).
- [~] P5.1/P5.2 metadata/AI/effects (effect packets + shared tracker core
  landed; live AI goals and live-session tracker wiring remain).
- [~] P6 block entities (chest/furnace/brewing/sign/spawner/hopper).
- [~] P7.1-P7.4 Nether/End generation/storage/portals (session switching).
- [~] P8.1 online/chat-signing core (TLS and login wiring).
- [~] P9.2/P9.3 bridge proxies/events/live effects (API depth).
- [~] P10 region ownership (per-region tasks/world tick slices; world
  storage still one global lock).

## Recently completed
- 2026-11-07: Unbreaking durability rolls; 4 integration tests.
- 2026-11-07: P10.4 region tick loop (first slice) — `RegionTickDriver` groups
  cached chunks into vanilla 8x8 regions, ticks the global queue first and each
  loaded/task region in sorted key order, and runs region tasks before that
  region's world slice. `WorldStorage::tick_region_with_table` hides other
  regions' chunks/block entities, moves only that region's due scheduled ticks
  and neighbour updates into the existing `tick_world` path, then restores
  them; cross-region neighbour updates are requeued instead of dropped.
  Scheduled/random ticks, block entities, hoppers, and neighbour drains are
  scoped per region. `main.rs` now ticks through the driver; the test-only
  parallel mode dispatches region task queues on scoped threads (world pass
  stays sequential) and returns ownership errors. Focused: 5
  `tests/region_tick_loop.rs`; `region_ownership`, lib `region`,
  `bulk_validation`, and `folia-server` stay green. Gaps: `WorldStorage` is
  still one global lock, so no parallel world tick; cross-region hopper
  transfers and border block edits remain queue/handoff work (P10.3/P10.5).
- 2026-11-07: P2.3 `NoiseChunk` vein interpolation — `density.rs` adds an
  `Interpolated` node that samples its argument on the vanilla 4x8x4 cell grid
  and trilinearly interpolates with `Mth.lerp3` (`compute_interpolated`),
  while direct `compute` keeps Java's unwrapped `DensityFunction` semantics.
  `OreVeinifier` now samples `vein_toggle` and both interpolated arguments
  inside `vein_ridged` through it; `vein_gap` stays per-block as shipped.
  `tests/density_parity.rs` pins the Java router dump, all 1225 density-oracle
  corners, trilinear values, 170 changed vein-edge positions, determinism, and
  the 16-column parity floor. Exact-name rose 5886 -> 5891/6144 (95.80% ->
  95.88%), matched ore blocks 26 -> 28/33; block-class stays 6067/6144.
- 2026-11-07: status-effect packets and shared tracking - `effects.rs` adds
  the 1.21.11 `ClientboundUpdateMobEffect` (130) / `ClientboundRemoveMobEffect`
  (76) bodies with registry ids 0-39, amplifier/duration/flags, merge/replace,
  and unknown-id decode errors. `LivingEntity` stores/tick effects, applies
  potions and instant health/damage once, and emits metadata/effect/remove
  packets. `entity.rs` adds a shared `EntityTracker` (id-keyed records,
  per-viewer range sets, enter/leave/update packets, deterministic order);
  `SessionEntityTracker` now uses it as its only entity storage. Focused: 15
  `effect_packets` + 8 `shared_tracking` tests; `entity_metadata`, `effects`,
  and `join_entities` stay green; fmt/clippy clean.
- 2026-11-07: brewing-stand block entity — persisted slots/brew time/fuel
  and live tick; 2 integration tests.
- 2026-11-07: player hunger — saturation-first exhaustion, eating caps,
  natural regeneration and starvation; 5 integration tests.
- 2026-11-07: chest loot roll — shipped tables fill chests once and clear the
  table id; 5 integration tests.
- 2026-11-07: crop growth — wheat/carrots/potatoes/beetroots age with a
  sky-light gate and deterministic random tick; 4 integration tests.
- 2026-11-07: brewing/consumables — `crafting.rs` adds `BrewingStandState`
  (3 bottle slots, ingredient, 20-charge blaze fuel, 400-tick brew, full vanilla
  potion mix table and container mixes), potion component helpers, and the 1.21.11
  potion effect table in `effects.rs`; `item_prototypes.rs` adds `FoodData`,
  consumable resolution (eat time/animation/use remainder), and complete
  food/potion use with creative no-consume. 16 `tests/brewing.rs` tests.
- 2026-11-07: feature completion batch — `end.rs` (chorus plant, end island,
  end gateway x2 with exit block entity, end platform, void platform),
  `misc.rs` (bonus chest, desert well with suspicious-sand payloads, monster
  room with loot chests and spawner), and `sculk.rs` (SculkSpreader cursor
  loop, sculk/sculk-vein behavior, multiface vein spreading, catalyst/rare
  shriekers). 218/224 configured features; 6 explicit unsupported remain.
  Focused: 15 `feature_gaps`, 18 `vegetation`, 74 `worldgen::feature` lib.
- 2026-11-07: P8.2 chat signing core — `SignedMessageBody` 1.21.11 wire codec
  (content/timestamp/salt/packed last-seen), `ServerboundChat` parser/builder,
  fixed 20-bit `LastSeenUpdate` bitset, `SignedMessage` (sequence, salt,
  timestamp, SHA-256 content hash, 256-byte signature, last-seen tracking),
  `PlayerChatMessage.updateSignature` payload, local signing chain and remote
  chain verification with sequence/timestamp/expiry bounds. `crypto.rs` adds
  SHA-256 and RSASSA-PKCS1-v1_5/SHA-256 sign/verify plus parameterized RSA
  generation. Server-issued 2048-bit profile keys are encoded in the Login
  Success `profile_key` GameProfile property; a deterministic 256-byte test
  signer exists but enforced mode rejects it. `enforces-secure-chat` parses
  with a false default and logs an audit line when enabled. 15 focused
  `tests/chat_signing.rs` tests (1 ignored release benchmark) + 2 crypto lib
  tests. Gaps: join/status wiring and clientbound Player Chat packets.
- 2026-11-07: Nether/End content — `worldgen/nether.rs` replays
  `minecraft:nether_cave` from the 17x17 carver neighbourhood (lava through
  y=31, cave air above, protected roof), runs the local-modification and
  surface-structure feature steps, and exposes pinned `nether_complexes`
  selection plus explicit fortress/bastion start failures. `worldgen/end.rs`
  adds the deterministic 10-spike dragon ring, arena bounds, full platform
  construction, gateway block/exit data, chorus growth, and an explicit
  feature dispatch. Focused: 7 `tests/nether_content.rs` + 10
  `tests/end_content.rs`; `nether_gen`/`end_gen`/`worldgen_parity` stay green.
  Gaps: fortress/bastion/end-city templates, live dimension switching, dragon
  entity fight, and carve-before-surface ordering.
- 2026-11-07: P2.8/P2.9 structure-template asset — `assets/structure_templates.json`
  ships 483 village templates (776,539 gzip bytes; 1,063,517-byte catalog;
  SHA-256 `f82d96dd…3448d3`, FNV-1a-64 `0x0dde46f3a7863ed8`) generated by
  `scripts/extract_structure_templates.ps1`. `assets.rs` lazily base64-decodes,
  gunzips, parses, and caches templates with explicit unknown-id/corrupt-payload
  errors and a `TemplateProvider` impl. `StructureGenerator::from_catalog` /
  `shipped_catalog` use it; a real `plains_meeting_point_1` test asserts
  10x7x10 size, 153 blocks, palette, and a fixed-seed 48-piece village with
  deterministic bounds. Feature pool elements are skipped and recorded. 5
  `tests/structure_assets.rs` tests; jigsaw/starts/processors/template stay green.
- 2026-11-07: P9 bridge live packets — `join.rs` now has a UUID/name command
  registry (`ActiveSession`, `SessionCommandGuard`) and play-loop outbound
  variants for system chat, play-state disconnect, teleport, set-time, game
  events, sound, and simple particles. Packet bodies were encoded from the
  1.21.11 decompiled layouts (chat/disconnect network NBT components, set-time,
  inline direct sound events, particle registry ids). `jvm.rs` pushes chat,
  kick, teleport, time, weather, and effect calls into live sessions while the
  native `FOLIA_DISABLE_PLUGINS=1` path is unchanged. Added the missing
  `WorldStorage::tick_hoppers` downward-push pass required by the existing tick
  loop. Focused: 3 new lib `join` tests plus a hopper-push world test, the
  `join` integration test, 11 `folia-server` tests, and 8 new
  `BridgeApiTest` checks; fmt/clippy clean. A temporary wire probe against a
  live HelloPlugin session decoded system chat (119), sound (115), particle
  (46), set time (111), weather game events (38), and play-state disconnect
  (32) with the expected bodies.
- 2026-11-07: leaf decay — distance/persistent properties, BFS log support,
  random-tick decay; 4 integration tests.
- 2026-11-07: daylight detector — sky-light accessor plus 20-tick analog power
  sampling (inverted supported); 6 redstone-extra tests total.
- 2026-11-07: hopper slice — persisted 5-slot hopper state, transfer core with
  merge/empty/full/component semantics, cooldown tick; 5 integration tests.
- 2026-11-07: block-entity ticker wiring — furnaces/spawners advance through
  `tick_world` with the shipped recipe catalog; furnace cooking/fuel is tested
  across 220 ticks. 2 integration tests.
- 2026-11-07: redstone lamp/openable slice — lamp light/unlight, powered
  doors/trapdoors/fence gates, daylight signal, note pitch; 5 integration tests.
- 2026-11-07: dimension storage slice — Nether/End region files, generate-on-miss,
  portal coordinate scaling, End platform arrival, and frame detection; 9
  integration tests.
- 2026-11-07: effects/armor slice — `effects.rs` models armor points and the
  vanilla absorb curve, status effects (poison/wither/regen intervals,
  resistance, speed/slowness, strength/weakness, jump boost, fire resistance),
  typed damage bypass flags, and a void-damage invulnerability bypass on
  `LivingEntity`. 11 integration + 2 lib tests.
- 2026-11-06: P10.1/P10.2/P10.6 region ownership slice — `region.rs` now makes
  ownership real: public `Region` (owned task queue, `RegionWorldSlice` world
  token + sparse cells, monotonic tick counter), `RegionOwned<T>`/`OwnedWorld`
  with `OwnershipError` on cross-region access (explicit caller check plus a
  per-thread current-region map and `ThreadingDetector`), `RegionMap` chunk /
  section / block -> region math (vanilla 8x8 chunk, 4x4 section regions,
  floor division for negatives, deterministic `RegionId` packing), and the
  scheduler API (`run_for_region`, owned tasks, `tick_region_checked`).
  Legacy `schedule_at`/`tick`/`tick_parallel` semantics are preserved.
  Focused: 18 `tests/region_ownership.rs` + 3 new lib `region` tests;
  `bulk_validation` scheduler suites stay green; fmt/clippy clean. Gaps:
  live tick loop, world/entity/player state, and chunk tickets are not yet
  region-owned (see `BLOCKERS.md`).
- 2026-11-06: P4.3/P5.4 item entities + XP orbs — `item_entity.rs` models
  `ItemEntity` (stack, pickup delay 10, lifetime 6000, magnet 4 blocks,
  merge 3 blocks, gravity 0.04) and `ExperienceOrb` (value, attraction 8
  blocks, `getExperienceValue` splits, merge 0.5 blocks, lifetime 6000) with
  Add Entity (type 71/49), metadata at index 8 (item stack / value), position
  sync, and remove packets via the existing entity encoders. `spawn_drops_at`
  spawns items then split orbs at the block centre with Java initial
  velocities; `DigState::stop_with_entities` rolls and spawns in one step
  while `stop_with_loot` keeps its signature. Pickup collects through the
  `PickupTarget` trait (`inventory::add_stack` + XP pool), so no `join.rs`
  edit was needed. `entity.rs` gains item/orb type-id helpers and base
  constructors. Focused: 6 lib + 16 `tests/item_entities.rs`; `block_breaking`
  and `entity_physics` stay green; fmt/clippy clean. Gaps: live `join.rs`
  spawning/ticking, fluids/fire/damage, overlay packets, Java pickup
  fixture.
- 2026-11-06: P5 join-session entity wiring — `entity.rs` adds a
  `SessionEntityTracker` that owns three deterministic mobs (cow, pig, zombie)
  per viewer, built from `spawn::entity_type` dimensions and real entity ids.
  Join sends Add Entity id 1, full `ClientboundSetEntityData` id 97 (including
  health), and `ClientboundSetEquipment` id 100 before the chunk batch. The
  play loop ticks physics/gravity at 20 Hz, updates `entity_ai` environment
  state, flushes dirty metadata, emits death/poof events and remove-entities id
  75, and drops mobs on death, out-of-range, chunk unload, or disconnect.
  `join.rs` routes fall/void damage through the living metadata store (metadata
  health + `ClientboundSetHealth` id 102), resets it on respawn, and filters
  global broadcast entity packets to tracked ids. Focused: 6
  `tests/join_entities.rs`, plus `join`, `entity_metadata`, and
  `entity_physics` stay green; fmt/clippy clean.
- 2026-11-06: P4.7 block-entity persistence — `block_entity.rs` adds typed
  chest (27 slots, `LootTable`/`LootTableSeed`), furnace/blast furnace/smoker
  (3 slots, the four vanilla short timers, `RecipesUsed`, Paper cook-speed
  multiplier), sign (front/back four-line components, color, glowing, waxed),
  and spawner (delay/count/range fields plus raw `SpawnData`/`SpawnPotentials`).
  `decode_block_entities`/`store_block_entities` read the modern
  `block_entities` key and migrate the legacy `BlockEntities` key;
  unknown types are kept as raw compounds and typed entities retain their
  unknown top-level fields. `world.rs` caches the position-keyed map beside
  each chunk and exposes get/insert/remove; `update_chunk` drops entities at
  edited positions and keeps unrelated ones, while generated `save_chunk`
  writes an empty map and `save_chunk_with_block_entities` persists explicit
  edits. `FurnaceBlockEntity` copies through `crafting::FurnaceState` for
  smelting, and `SimpleBlockEntityTicker` is opt-in (not wired into `tick.rs`).
  Focused: 3 lib + 9 `tests/block_entity.rs` tests; `world_round_trip`,
  `bulk_validation`, fmt, and clippy clean. No menu/packet, placement, live
  ticker, or Java-fixture parity yet.
- 2026-11-06: P9 bridge depth slice — `jvm.rs` adds Rust-owned permission
  attachments (explicit values + operator default), Rust-backed flat-YAML
  plugin config I/O under `run/<plugin>/config.yml`, persisted world
  time/weather/spawn state in `run/world_state.txt`, and a v2 item codec with
  enchantments. Java adds `BridgeConfig`, `BridgeServicesManager`,
  `BridgePermissions`/`BridgePermissionAttachment`, world time/weather/spawn
  methods, a scheduler backend seam, registry shims for enchantment/sound
  constants, and explicit no-op sound/particle paths. `HelloPlugin` exercises
  config, services, time/weather/spawn, permissions, and enchantments;
  `BridgeApiTest` grows from 18 to 44 checks. Focused Rust: 11 `folia-server`
  tests pass; live `FOLIA_BRIDGE_SELFTEST=1` plugin startup passes. Live
  message/kick transport, weather/time packets, item flags/PDC persistence,
  plugin.yml permission expansion, and stock `reloadConfig` remain open.
- 2026-11-06: P6.5 pistons — `piston.rs` ports `PistonBaseBlock`,
  `PistonHeadBlock`, `MovingPistonBlock`, `PistonStructureResolver`, and the
  moving-block payload. Extension/retraction follow the facing direction,
  including the 12-block push limit, slime/honey chains (honey never sticks to
  slime), obsidian/bedrock/block-entity rejection, plant/fluid destruction,
  sticky one-block retraction, scheduled two-tick movement, shape/neighbour
  updates through `TickWorld::set_block`, and `block_shapes`-based head/moving
  collision boxes. Moving payloads are explicit in `PistonState` because
  `block_entity.rs` still has no typed API. Focused: 7 lib + 9
  `tests/pistons.rs`; full workspace gate 817 passing with the two known CRLF
  asset-hash guards skipped; fmt/clippy clean. Gaps: entity pushing, block
  entity contents in moved states, Java differential traces, and payload NBT
  persistence.
- 2026-11-06: P1.5/P3.6-P3.8 outline light occlusion - `light.rs` replaces
  the light-block-only approximation with cached per-state outline face masks
  (`face_occludes(state_id, direction)`), pairwise `shapeOccludes` union checks
  in both increase engines, `isEdgeOccluded` sky-column scans, and
  shape-change rechecks in `LightScene`. Slabs, stairs, snow layers, farmland,
  paths, and carpets block their full faces while fences, panes, walls,
  glass, water, leaves, and ice keep their attenuation. New
  `tests/light_occlusion.rs` has 10 focused tests (masks, opaque emitters,
  use-shape-disabled blocks, slab/stair/snow, transparency, torch-under-slab,
  decrease/re-light, bulk comparison, border).
  Remaining: Java light fixtures and exact `canOcclude` flags.
- 2026-11-06: P2.8/P2.9 recursive jigsaw + structure generator wiring -
  `worldgen/structure/jigsaw.rs` ports `JigsawPlacement.Placer` (start
  rotation/weighted element, optional named start jigsaw, `canAttach`
  front/top/target matching, rigid vs terrain-matching Y, expansion-hack
  max-size probing, block-aligned free-space collision with the max-distance
  window, junction records, depth limits, `SequencedPriorityIterator` order).
  `StructureStart` now carries jigsaw/desert payloads; jigsaw placement requires
  templates and refuses the template-only path. `StructureGenerator` embeds all
  34 structure biome tags, applies dimension gating (Nether/End sets excluded
  from Overworld), regenerates starts per candidate placement chunk, and runs
  before surface-step features in `WorldGenerator` only when a caller supplies
  templates. `DesertPyramidPiece` ports the pyramid body, four chest loot
  markers, cellar, collapsed roof, and suspicious-sand markers. Waterlogging is
  applied by a wrapper; `structure_void` stays dropped by `JigsawReplacement`.
  Tests: 5 `jigsaw` + 4 `structure_generation`; parity unchanged at 98.75%
  block-class / 95.80% exact-name. Gaps: shipped NBT template asset,
  feature-pool placement, terrain adaptation, serialization, block-entity
  persistence/entity spawning, cross-chunk height sampling, Java-exact desert
  random stream.
- 2026-11-06: P4.8 crafting/furnace model — `crafting.rs` implements 1x1–3x3
- 2026-11-06: P4.3 shipped block-drop tables - `block_breaking.rs` now rolls the
  real `minecraft:blocks/<block>` table for every break through
  `LootCatalog::roll_block`; the fallback `DROP_TABLE` is only reached when no
  shipped table exists or a known table hits an unsupported context (the report
  is preserved). `BlockLootContext` carries the tool, explicit
  `ToolContext { silk_touch, fortune }`, the correct-tool override, and an
  optional explosion radius; `set_count`, `set_damage`, `set_name`,
  `enchant_randomly`, `explosion_decay`, and `limit_count` execute through
  `item_components`. `DigState::stop_with_loot` rolls the table on a destroyed
  stop without changing durability, and `CatalogLootProvider` exposes the same
  path to callers. Focused: 13 `tests/loot_drops.rs`, 9 `tests/block_breaking.rs`,
  9 `tests/loot.rs`, and 13 lib `block_breaking` tests. The legacy `block_drops`
  signature still seeds a deterministic per-block RNG because `join.rs` has no
  `Level.random` stream yet; item entities, XP orbs, Unbreaking, and a Java
  fixture remain open.
- 2026-11-06: P2.5/P2.6/P2.7 terrain, dripstone, and selector batch -
  `feature/support.rs` adds shared rule-based/rotated providers, float
  providers, and predicate reuse; `feature/terrain.rs` ports disks, springs,
  lava lake, icebergs, blue ice, ice spikes, netherrack blobs, basalt
  columns/pillar, and delta; `feature/caves.rs` ports pointed dripstone,
  dripstone clusters, and large dripstone. Selector branches now resolve
  through both the placed and configured registries with full placement
  pipelines, so all 25 selector configs execute. **207/224** configured
  features; 17 explicit unsupported. Focused: 9 `feature_gaps` + 18
  `vegetation` + 74 `worldgen::feature` lib tests; parity unchanged. Remaining
  blockers: root_system, sculk_patch, geode, freeze_top_layer, End/void and
  block-entity features.
- 2026-11-06: P4.8 crafting/furnace model - `crafting.rs` implements 1x1-3x3
  shaped matching with CraftingInput-style bounding-box cropping, mirrored
  patterns, ingredient item/tag/alternative matching, and vanilla's
  last-sorted-recipe-id priority; shapeless multiset matching with duplicate
  ingredients; transmute; one-item-per-slot consumption with component
  preservation; `CraftOutput::deposit` through `inventory::add_stack`;
  `FurnaceState` for furnace/blast furnace/smoker/campfire with the vanilla
  fuel table, recipe cook times, XP accumulation, output stacking, and
  extraction; and one-shot stonecutting/smithing transforms (base components
  copied by smithing). Special, decorated-pot, trim, and opaque recipes fail
  with `CraftingError::UnsupportedRecipe`; unknown ids and wrong-station use
  are explicit errors. Focused: 19 `tests/crafting.rs` + 5 lib tests. Gaps:
  menu packets/click handling, recipe book/advancement unlocks, brewing,
  cached-last-recipe lookup, campfire multi-slot/lit state, furnace fuel
  remainders other than lava buckets, and a Java recipe/furnace fixture.
- 2026-11-06: P1.4 item prototype catalog — `scripts/extract_item_prototypes.ps1`
  boots the local Folia 1.21.11 jar through `Bootstrap.bootStrap`, reads every
  non-air `BuiltInRegistries.ITEM` prototype component map, and writes
  `assets/item_prototypes.json` (1,504 items, 79,369 B, FNV-1a-64
  `0xf179dfed29cdcdad`). Only non-default fields are stored (`{}` = stack 64,
  no durability), with a recomputable `counts` index. `item_prototypes.rs`
  adds `ItemPrototype`, `ToolPrototype` (rules/tier/speed/attack damage/
  correct-for-drops), `FoodPrototype` (nutrition/saturation/eat time),
  `IngredientSet`, catalog lookups by name/id, `defaults_for`/`apply_defaults`
  component patches for `item_components`, and strict malformed-entry errors.
  3 unit + 13 integration tests cover diamond sword, netherite pickaxe, apple,
  potions, ender pearls, elytra, block-item fallback, patch idempotence,
  registry-id lookup, and a byte-identical extractor `-Check`.
- 2026-11-06: P2.8 processor-list and template-pool slice —
  `scripts/extract_structure_data.ps1` writes deterministic
  `assets/structure_processors.json` (46,334 bytes; 40 lists / 52 processors)
  and `assets/template_pools.json` (252,745 bytes; 188 pools / 1,204 element
  nodes / 4,880 slots). `processor.rs` implements `block_ignore`, `block_rot`,
  `gravity`, `jigsaw_replacement`, `protected_blocks`, `block_age`,
  `blackstone_replace`, `lava_submerged_block`, `nop`, `capped`, and ordered
  `rule` processors with all six rule tests, three position predicates, and
  four block-entity modifiers; unknown types fail explicitly. `pool.rs`
  implements single/legacy/feature/empty/list elements, projection, weights,
  fallbacks, and Java's weighted element sequence. The expansion API resolves
  a start pool and places one element through the template-piece path; recursive
  jigsaw connection remains open. 19 focused `tests/structure_processors.rs`
  tests; `structure_starts` 13 and lib structure tests stay green. Clippy and
  fmt are clean.
- 2026-11-06: P3.10 redstone components — `redstone.rs` extends the first
  slice with repeaters (facing/input side, delay 1–4, 2/4/6/8-tick scheduled
  toggles, live side locking, strong output, delay cycling), comparators
  (compare/subtract, rear and side inputs, `ContainerSignalSource` container
  strength, stored output, scheduled refresh), observers (front-change
  detection, one two-tick pulse), redstone torches (floor/wall attachment,
  inversion, vanilla 8-in-60-tick burn-out with a 160-tick restart), wooden/
  stone/weighted pressure plates, and tripwire (occupancy API, pressed state,
  ten-tick release, connection shape). Wire now recomputes its
  `north/east/south/west` connection shape through the same neighbour queue.
  `tick.rs` adds `ContainerSignals`, comparator-output/occupancy/torch-toggle
  state, `TickWorld` notification/state helpers, and registers every new
  behaviour. Focused: 10 lib, 2 tick-state, and 10 `tests/redstone2.rs` tests;
  `tests/fluids.rs` and `tests/ticks.rs` stay green; fmt/clippy clean.
- 2026-11-06: P6.2 fluid interactions — `fluid.rs` adds the exact 1.21.11
  `LiquidBlock.shouldSpreadLiquid`/`LavaFluid.spreadTo` contacts (source+water
  -> obsidian, flowing+water -> cobblestone, downward lava into water -> stone,
  correct up/horizontal faces only, soul-soil blue-ice basalt branch), lava
  random-tick ignition with the ported `FireBlock.bootStrap` odds and exact
  `BaseFireBlock.getState`/`FireBlock.tick` aging/burnout in `FireBehavior`,
  soul-sand/magma bubble columns with `BubbleColumnBlock.updateColumn`,
  `Entity.handleOnInsideBubbleColumn`/`handleOnAboveBubbleColumn` velocity
  updates, and fluid destruction of replaceable plants. `interaction_table()`
  exposes the fire scheduled-tick behaviour without editing `tick.rs`.
  Focused: 13 `--lib fluid` tests, 8 `tests/fluids.rs`, 16
  `tests/fluid_interactions.rs` (all pass; `cargo clippy --lib --tests -D
  warnings` clean). Production fire aging still needs the one-line
  `behavior_for` registration; see `BLOCKERS.md`.
- 2026-11-06: P1.5 per-state block collision/outline shapes — new
  `block_shapes.rs` embeds `assets/block_shapes.tsv.gz` (29,671 rows, 161,523
  B), generated by `scripts/extract_block_shapes.ps1` from the local Folia
  oracle. `collision_boxes`/`outline_boxes` return exact AABB lists for every
  state; passable states (air/fluids/plants/torches) are empty; unknown ids
  fall back to a full cube. `player_physics` now sweeps multi-box shapes with
  vanilla's ±1 block cursor and step-up; `entity.rs` uses the same lists via a
  `BlockCollider::collision_boxes` default. The player steps onto/off a bottom
  slab and an armor stand lands at 0.5. 4 lib + 15 `tests/block_shapes.rs`
  tests.
- 2026-11-06: P3.8/P3.9 incremental light — `light.rs` adds `LightScene`,
  `DirtySections`/`LightSectionPos`, deterministic deduplicated decrease and
  increase queues, sky column source updates, cross-chunk propagation into
  loaded neighbours, and `light_update_for_sections` for section-scoped
  `ClientboundLightUpdate` bodies. `block_update::set_block` now snapshots and
  restores the pre-edit layers around `Chunk::set_block_state`, runs the
  incremental scene, persists only changed neighbour chunks, and exposes
  `BlockChange.dirty_sections`/`neighbour_light`; `broadcast_change` sends the
  centre and neighbour payloads. 8 focused `tests/light_incremental.rs` tests
  cover torch removal, sky columns, borders, exact dirty
  sections, idempotence, storage persistence, and bulk-vs-incremental equality.
  Remaining: Java light fixture and P1.5 shape occlusion.
- 2026-11-06: P5.1 entity metadata/health/equipment core — `entity_metadata.rs`
- [~] P7.1 Dimension abstraction (`generator::Dimension` + `generate_dimension`
  dispatch done; fixed time, game rules, and plugin `World` dimension API remain)
- [~] P7.2 Nether generation (router, surface, lava/bedrock, five-biome preset,
  quartz/magma/glowstone features done; carvers, structures, and a Java fixture
  remain)
- [~] P7.3 End generation (exact island field, central/outer islands,
  `TheEndBiomeSource`, obsidian platform done; cities, gateways, dragon arena,
  and a Java fixture remain)

## Recently completed
- 2026-11-06: P7 dimension generation slice - `generator::Dimension` models
  the shipped Overworld/Nether/End type fields and
  `generate_dimension(dimension, x, z, seed)` dispatches without touching the
  Overworld generator. `worldgen/nether.rs` adds the Nether density router,
  bedrock y=0/127, lava below y=32, netherrack, the five-biome preset,
  soul-sand/basalt/nylium surfaces, and a Nether `FeatureSorter` order that
  places quartz/magma/glowstone. `worldgen/end.rs` ports
  `EndIslandDensityFunction`, central/outer islands, `TheEndBiomeSource`, and
  the cross-chunk `(100, 49, 0)` obsidian platform. Focused: 3 Nether lib +
  9 `tests/nether_gen.rs` + 3 End lib + 8 `tests/end_gen.rs`; full workspace
  634 passing; parity unchanged at 98.75% block-class / 95.80% exact-name.
- 2026-11-06: P5.1 entity metadata/health/equipment core - `entity_metadata.rs`
  adds `EntityMetadata`/`MetadataStore` with vanilla dirty semantics,
  typed `MetadataValue` serializers (byte/int/long/float/string/component/
  optional component/item stack/boolean/pose), `ClientboundSetEntityData`
  encode/decode, `ClientboundSetEquipment` with ordinal+continuation bits,
  `ClientboundEntityEvent` death events `3`/`60`, `ClientboundSetHealth`,
  `LivingEntity` raw damage with the 20-tick invulnerability window and death
  animation, typed attributes (max health, armor, toughness, attack
  damage/speed, movement speed, follow range), and eight equipment slots.
  `entity.rs` adds `spawn_living`, `hurt_entity`, `tick_living_entities`, and
  `set_entity_equipment` bridges plus cleanup on removal; join/player sessions
  are deliberately untouched. 6 lib + 12 integration tests.
- 2026-11-06: Block-behaviour consumers for lighting and physics —
  `light.rs` drops the hand-written opacity/emission tables and reads
  `block_behaviour::light_block`/`light_emission` per state (torch 14,
  glowstone/lava 15, water/leaves 1, glass 0, unknown states opaque and
  non-emitting). `player_physics.rs` replaces its tag/name approximations with
  `has_collision`, `blocks_motion`, `is_solid`, `friction`, `speed_factor`, and
  `jump_factor`; speed factor scales horizontal velocity after the collision
  move and jump factor multiplies the impulse. Collision boxes stay full cubes.
  Focused tests: 4 new `tests/block_behaviour_integration.rs` tests (emission,
  glass/water sky attenuation, glass-shaft overhang, ice/slime/blue-ice friction
  plus water/plant passability) and the existing light/physics/packet suites
  stay green; the captured `world_chunk_packet` body is unchanged.
- 2026-11-06: Structure start/piece/set foundation — `set.rs` parses all 20
  shipped structure-set documents (34 weighted entries) and implements
  random-spread placement, all four frequency reducers, exclusion zones,
  weighted selection, and `structure_at_chunk`; `piece.rs` adds `BoundingBox`,
  `StructurePiece`, template pieces, chunk-box clipping, block filters, and
  data markers; `start.rs` adds deterministic starts, references, terrain
  inflation, the igloo structure, and explicit jigsaw failures. 13 focused
  integration tests. Full workspace gate is the coordinator's merge run.
- 2026-11-06: P4.3 block-breaking slice — `block_breaking.rs` ports the
  start/stop/abort dig state machine, vanilla's 0.7 stop threshold and
  `hasDelayedDestroy`, `Player.getDestroySpeed`,
  `BlockBehaviour.getDestroyProgress`, tool material speeds/durabilities,
  efficiency `level^2 + 1`, haste/mining fatigue, water/air penalties, the
  sword/shears rules, and tier/harvest gates from the `mineable/*`,
  `incorrect_for_*_tool`, and `needs_*_tool` block tags. `join.rs` keeps dig
  state per session, destroys instantly in creative, wears/breaks tools,
  captures a deterministic common-block drop table (with a `LootProvider` hook
  for `crate::loot`), and continues delayed destroys on idle ticks. 13 lib + 9
  integration tests. Real loot tables, item entities, block-damage overlay
  packets, Unbreaking/Fortune/Silk Touch, adventure `can_destroy`, and a Java
  fixture remain open.

- 2026-11-06: P3.10 fluid/redstone slice — `fluid.rs` ports water/lava
  scheduled flow: source/flowing/falling state mapping, 5-tick water vs
  30-tick lava delay, 1/2 drop-off, downward priority, the 4/2-step slope
  search toward water holes, two-source conversion (water only), waterlogged
  placement, and source-removal drain. `redstone.rs` adds `SignalReader`
  strong/weak queries, `getDirectSignalTo`, `getBestNeighborSignal`, wire
  target strength (adjacent source or strongest wire minus one, 0..15), and
  lever toggling plus button press with the vanilla 20/30-tick scheduled
  release. Wire and fluid updates flow through the existing neighbour queue;
  `behavior_for` registers all four behaviours. Focused: 8 fluid + 7 redstone
  + 1 block-update lib tests and 7 `tests/fluids.rs` integration tests
  (hole flow, 7 levels, lava delay, drain, waterlogging, source conversion,
  determinism). Workspace run passes 519 tests when the two pre-existing CRLF
  asset-hash checks are skipped; those fail only in this checkout because
  `assets/*.tsv` were materialized with CRLF. See `BLOCKERS.md`.
- 2026-11-06: P1.7 loot and recipe catalogs — `scripts/extract_loot_recipes.ps1`
  normalizes 1,324 loot tables and 1,470 recipes into compact sorted JSON with
  per-type indexes (block 1,083, entity 114, chest 65; shaped 706, shapeless
  320, stonecutting 254, smelting 73). `src/loot.rs` adds typed pools, weighted
  entries, number providers, 20 function kinds, all shipped condition kinds,
  Java-order rolling through `folia_random::RandomSource`, and typed recipe
  ingredients/results with tag matching. Unsupported payloads are preserved
  opaquely and surface in `LootRoll::report`; `tests/loot.rs` covers the shipped
  diamond-ore, stone, and simple-dungeon tables, recipe serializers, opaque
  payloads, determinism, and component functions. 9 focused tests.
- 2026-11-06: P2.3 ore wiring — `Decoration::UndergroundOres` now dispatches
  the 32 shipped ore/scattered-ore placed features; `OreVeinifier` runs in the
  density fill pass before surface rules and carvers. The generator now applies
  vanilla's lazy placement stream (feature RNG consumed between positions) and
  computes `FeatureSorter` indices from the biome parameter order instead of
  the biome registry order; both fixes were required for multi-position ore
  placement. Exact-name parity: 95.80% (5886/6144), block-class 98.75%
  (6067/6144), up from 92.01%/98.71%. `tests/ore_wiring.rs` pins the default,
  determinism, vein fillers, and a >95.5% exact-name floor; generator unit
  tests pin the parameter-order indices and vein output.
- 2026-11-06: Plugin world/player/item/event bridge — `jvm.rs` adds a
  process-wide bridge state backed by `PlayerSession` playerdata and the shared
  overworld storage, 25 JNI getters/setters, and host-callable event methods.
  `java/bridge` adds `BridgeWorld`, `BridgePlayer`, `BridgeInventory`,
  `BridgeItemStack`, `BridgeItemMeta`, `BridgeRegistryAccess`, and
  `BridgeInternalApi`; `HelloPlugin` exercises the new API. 6 `folia-server`
  unit tests, 18 Java `BridgeApiTest` checks, and the gated live selftest pass.
  Gaps: live-session sync, armor/off-hand, enchant/PDC persistence, and live
  `join.rs` event wiring.
- 2026-11-06: Core item data components — `item_components.rs` replaces the
  `{item, count}` stack with an insertion-ordered component patch, typed values
  for names/lore/damage/enchantments/attributes/container/potion/food/tool/
  rarity, and opaque NBT for unknown components. Inventory/playerdata save and
  load preserve the exact tags; network encoding writes the ported subset and
  reports omitted components explicitly. 8 unit + 17 integration tests.
- 2026-11-06: P3.10 tick slice — `tick.rs` adds vanilla update-flag values, a
  chunk-grouped neighbour queue with dedup/flag merging, a scheduled-tick queue
  ordered by trigger/priority/sub-tick with delay and cancellation, and a
  deterministic 3-per-section random-tick sampler. `block_update::set_block`
  drains six neighbour updates; falling blocks, plant support, and grass spread
  execute. `tick::tick_world` is wired into the server loop. 11 unit + 10
  integration tests.

- 2026-11-06: P2.4/P2.5 feature-gap slice — `worldgen/feature/mod.rs`
  reorders configured-feature parsing (patches → simple selectors → vegetation
  patches → trees → selectors), adds `SimpleRandomSelectorFeature`, and gives
  `FeatureLevel` scheduled-tick/block-entity sinks backed by
  `WorldGenRegion`. `tree.rs` adds the pale-moss decorator (shuffled ground pass
  through the embedded `pale_moss_patch`, trunk/leaf hangers) and beehive
  occupant NBT. `vegetation.rs` accepts `schedule_tick` and ports
  `simple_random_selector`. Focused: 74 feature lib tests + 18 vegetation
  integration tests; 171/224 configured features supported.
- 2026-11-06: P2.5 aquatic/vine/mushroom batch - three new feature modules.
- 2026-11-06: P2.12 surface fidelity — `surface::build_surface` now resolves
  the pre-extension and per-block `isBiome`/temperature biome through
  `biome_zoom::BiomeZoomer`, memoized per block Y like Java, and falls back to
  the chunk-local quart only when no configured generator seed exists. Surface
  temperature uses `biome_climate` plus `simplex::temperature_noise` for
  `getHeightAdjustedTemperature`; frozen-ocean iceberg melt goes through the
  same helper. `config` gains Paper's `generate-flat-bedrock` and
  `paper:optionally_flat_bedrock_condition_source` collapses the floor gradient
  to one layer when enabled. The column start was corrected to Java's
  `Heightmap.getHeight + 1`. `tests/surface.rs` covers zoom selection,
  flat-bedrock on/off, and fixed-coordinate frozen/deep-frozen melt. Parity:
  98.71% block-class / 92.01% exact-name (5653/6144).
- 2026-11-06: P2.5 aquatic/vine/mushroom batch — three new feature modules.
  `aquatic.rs` ports `SeagrassFeature` (short/tall, half pairing),
  `KelpFeature` (head age and body fallback), `SeaPickleFeature`,
  `CoralFeature` plus tree/claw/mushroom shapes (tag-ordered coral picks,
  fans/pickles, wall-fan facing), `BambooFeature` (podzol ring, leaf stages),
  and `UnderwaterMagmaFeature` (column scan, per-position draw, enclosed
  placement). `vines.rs` ports `VinesFeature` (face attachment in
  `Direction.values()` order), `TwistingVinesFeature` and `WeepingVinesFeature`
  (spread scans, head/body columns, ages 17–25 and 23–25). `mushroom.rs` ports
  huge brown/red mushrooms (Java cap shapes, connection properties,
  trunk-after-cap order), huge crimson/warped fungus (stem/hat/decoration,
  planted branch, weeping vines), `GlowstoneFeature`, `BlockBlobFeature`
  (`forest_rock`), `FillLayerFeature`, `NetherForestVegetationFeature`, and
  `RandomBooleanSelectorFeature` (mushroom islands). `FeatureLevel` gains a
  default `heightmap()` and absent sections read as air. 171/224 configured
  features execute. Focused: 73 lib feature tests, 16 `tests/aquatic.rs`
  integration tests. Parity: 98.71% block-class / 92.02% exact-name
  (5654/6144); the one-block exact delta is a tall-seagrass pair on a column
  whose Rust terrain already differs from Java. Remaining: kelp/nether outer
  placement (`noise_based_count`/`count_on_every_layer`), scheduled ticks,
  `simple_random_selector`, pale moss wiring, and structures.
- 2026-11-06: First real lighting slice — `light.rs` adds vanilla-compatible
  `DataLayer` nibble storage, `SectionLight` layers on every `ChunkSection`,
  `SkyLight`/`BlockLight` NBT decode/encode, per-column sky sources with a
  six-direction BFS under overhangs, block-light BFS over an explicit emitter
  table, and one-block neighbour border propagation. `chunk_packet::light_bytes`
  now prefers stored/computed layers (including real block-light updates) and
  keeps the old height fallback only for chunks without light; the captured
  `(0,-1)` packet fixture is still byte-identical. `block_update` reloads
  existing neighbours, recomputes, persists, and broadcasts. `tests/lighting.rs`
  adds 7 tests (nibble/NBT round-trip, overhang, torch radius, unsupported
  emitter, packet masks, border torch, edit persistence). Emitter/opacity data
  is still hand-written; incremental section-only light packets and Java light
  fixtures remain open.
- 2026-11-06: Player physics and movement validation — `player_physics.rs`
  implements the vanilla walking/falling formulas, AABB collision against
  `WorldStorage`, step-up, fall distance/damage, and movement reconciliation.
  The live session keeps the ten-block safety net, accepts legal walking
  without corrections, sends position corrections for blocked claims, and
  sends health updates on fall damage. 12 focused `player_physics` tests plus
  updated playerdata/movement tests pass; clippy is clean.
- 2026-11-06: P2.5 ground-cover batch — `worldgen/feature/vegetation.rs`
  ports `RandomPatchFeature` (also `flower`), `SimpleBlockFeature`,
  `BlockPileFeature`, `BlockColumnFeature`, `MultifaceGrowthFeature`,
  `VegetationPatchFeature`, and `WaterloggedVegetationPatchFeature`, including
  inline nested placed features, the shipped state providers, and Java
  `HashSet` bucket order for patch positions. `FeatureData` now dispatches
  134/224 configured features. Generation runs `underground_decoration`,
  `vegetal_decoration`, and `top_layer_modification`; `underground_ores` stays
  off. Focused tests: 8 unit + 15 integration. Exact-name parity remains
  92.04% (5655/6144), block-class 98.71% (6065/6144).
- 2026-11-06: Structure-template foundation — `worldgen/structure/template.rs`
  parses gzip 1.21.11 structure NBT (DataVersion, size, palettes, blocks with
  optional block-entity NBT, entities), ports `StructureTemplate.transform`
  for block and vector positions with mirror/four rotations/pivot/origin, and
  writes blocks through `FeatureLevel`. `place_into` returns written/rejected
  counts, block-entity NBT, and transformed entity descriptors; it does not
  spawn entities. Structure starts, pieces, sets, references, processor lists,
  template pools, jigsaw expansion, and random palette selection are still
  unported. 13 focused tests.
- [x] P3.5 Real light storage
- [~] P3.6 Sky-light engine (single-chunk, border margin, incremental source
  updates; Java fixture remains)
- [~] P3.7 Block-light engine (generated emission/opacity, decrease/increase
  queues; Java fixture remains)
- [~] P3.8 Incremental light updates (`DirtySections`, section-scoped
  payloads, neighbour payloads; Java packet-body comparison remains)
- [~] P3.10 Neighbor updates, scheduled ticks, random ticks (flags, queues,
  falling blocks, plant support, grass spread, server hook, water/lava flow,
  wire/lever/button/repeater/comparator/observer/torch/plate/tripwire redstone;
  pistons/sticky pistons; leaf decay, crops, lamps, tripwire hooks, and NBT tick
  persistence remain)
  falling blocks, plant support, grass spread, server hook, water/lava flow +
  lava/water quenching, lava ignition, bubble columns, wire/lever/button
  redstone; production fire aging, leaf decay, crops, remaining redstone, and
  NBT tick persistence remain)
- [~] P4.1 Player entity and physics (walk/fall/jump/step/fall damage done;
  ladders, water, cobwebs, slime, elytra, and a standalone tick remain)
- [~] P4.2 Movement validation (collision replay, 10-block net, corrections,
  and burst validation done; teleport ack and special movement remain)
- [~] P4.3 Block breaking progress (dig state, tool/tier checks, break times,
  creative instant break, drops, durability done; real loot tables, item
  entities, overlay packets, Unbreaking/Fortune/Silk Touch, adventure
  `can_destroy`, and a Java fixture remain)

## Recently completed

- 2026-11-06: First structure-template slice — `worldgen/structure/` loads
  gzip structure NBT (`empty.nbt`, igloo middle, desert camel spawn, nether
  fossil fixtures), validates size/palette indices/block bounds, ports
  `StructureTemplate.transform` for `BlockPos` and `Vec3` with mirror, four
  rotations, pivot and origin, and writes blocks through `FeatureLevel`.
  `place_into` returns block-entity NBT and transformed entity descriptors
  instead of spawning them. `tests/structure_template.rs` adds 13 tests
  covering real templates, all rotations/mirrors, pivot/offset, state
  properties, mock-level placement, and malformed NBT rejection. Focused test
  only; the coordinator runs the full workspace gate.

- 2026-11-06: First real lighting slice — `light.rs` adds vanilla-compatible
  `DataLayer` nibble storage, `SectionLight` layers on every `ChunkSection`,
  `SkyLight`/`BlockLight` NBT decode/encode, per-column sky sources with a
  six-direction BFS under overhangs, block-light BFS over an explicit emitter
  table, and one-block neighbour border propagation. `chunk_packet::light_bytes`
  now prefers stored/computed layers (including real block-light updates) and
  keeps the old height fallback only for chunks without light; the captured
  `(0,-1)` packet fixture is still byte-identical. `block_update` reloads
  existing neighbours, recomputes, persists, and broadcasts. `tests/lighting.rs`
  adds 7 tests (nibble/NBT round-trip, overhang, torch radius, unsupported
  emitter, packet masks, border torch, edit persistence). Emitter/opacity data
  is still hand-written; incremental section-only light packets and Java light
  fixtures remain open.
- 2026-11-06: Player physics and movement validation — `player_physics.rs`
  implements the vanilla walking/falling formulas, AABB collision against
  `WorldStorage`, step-up, fall distance/damage, and movement reconciliation.
  The live session keeps the ten-block safety net, accepts legal walking
  without corrections, sends position corrections for blocked claims, and
  sends health updates on fall damage. 12 focused `player_physics` tests plus
  updated playerdata/movement tests pass; clippy is clean.
- 2026-11-06: P2.5 ground-cover batch — `worldgen/feature/vegetation.rs`
  ports `RandomPatchFeature` (also `flower`), `SimpleBlockFeature`,
  `BlockPileFeature`, `BlockColumnFeature`, `MultifaceGrowthFeature`,
  `VegetationPatchFeature`, and `WaterloggedVegetationPatchFeature`, including
  inline nested placed features, the shipped state providers, and Java
  `HashSet` bucket order for patch positions. `FeatureData` now dispatches
  134/224 configured features. Generation runs `underground_decoration`,
  `vegetal_decoration`, and `top_layer_modification`; `underground_ores` stays
  off. Focused tests: 8 unit + 15 integration. After the later placement
  modifiers landed, exact-name parity is 92.04% (5655/6144) and block-class is
  98.71% (6065/6144).

- 2026-11-06: Vanilla tag registries — `scripts/extract_tags.ps1` walks the
  decompiled tag tree, resolves nested `#tag` references with cycle detection
  and de-duplication, and writes nine sorted `tag<TAB>member` assets under
  `crates/folia-core/assets/tags/`: block (202 non-empty tags/3,655 members),
  item (198/1,897), entity type (46/368), fluid (2/4), game event (5/119),
  damage type (33/196), enchantment (22/308), point-of-interest type (3/30),
  worldgen (91/631). `src/tags.rs` embeds them in `OnceLock`s, accepts bare or
  namespaced keys, and returns empty/false for unknown tags. `tests/tags.rs`
  adds 5 membership/count tests; `tests/assets.rs` pins the new assets. The
  merged workspace gate is 426 passing.

- 2026-11-06: Ore features — `worldgen/feature/ore.rs` ports `minecraft:ore`
  and `minecraft:scattered_ore`, including ellipsoid elimination, air-exposure
  discard, exact supplied target states, and the four shipped `RuleTest` tags.
  All 30 ore and 2 scattered-ore configs parse; focused placement tests cover
  stone and netherrack. Generator wiring is intentionally not enabled yet:
  doing so drops the exact-name fixture from 92.09% to 89.06% because feature
  RNG is still approximate. Full suite 341 passing.

- 2026-11-06: Creaking-heart decorator — `creaking_heart` parses, shuffles the
  log list, requires logs in all six directions, and writes a dormant natural
  creaking heart. Focused test covers both the six-neighbour success and the
  missing-neighbour rejection. Pale moss remains explicitly unsupported
  because its ground pass calls `pale_moss_patch`; full suite 337 passing.

- 2026-11-05: Fallen-tree slice — `FallenTreeFeature` places a stump, samples
  a horizontal direction/length/start, validates the whole log with
  `valid_tree_pos` and a two-block ground-gap limit, tags sideways logs with
  their axis, and runs `attached_to_logs`. Shipped `fallen_oak_tree` fixture at
  seed 7: 5 logs (4 sideways) and 4 mushrooms. Full suite 336 passing;
  clippy/fmt clean.

- 2026-11-05: Mangrove slice — `upwards_branching_trunk_placer` walks
  probability-gated side branches and `mangrove_root_placer` simulates roots
  through the mangrove root tag, placing waterlogged roots, muddy roots, and
  above-root moss. Shipped `mangrove`/`tall_mangrove` fixtures place 10 logs,
  157 leaves, 17 roots (7 waterlogged), 7 muddy roots, and 3 moss carpets at
  seed 7. Full suite 334 passing; clippy/fmt clean.

- 2026-11-05: Azalea slice — `bending_trunk_placer` grows a trunk with a
  random one-to-two-block horizontal bend and leaf attachments from
  `min_height_for_leaves`; `random_spread_foliage_placer` samples a weighted
  azalea/flowering-azalea provider. Shipped `azalea_tree` fixture places 8
  logs, 65 azalea leaves, and 24 flowering azalea leaves at seed 7. Full suite
  333 passing; clippy/fmt clean.

- 2026-11-05: Jungle bush slice — the straight trunk now pairs with
  `bush_foliage_placer`, and a shipped `jungle_bush` fixture places 1 log and
  32 leaves at seed 7. Full suite 332 passing; clippy/fmt clean.

- 2026-11-05: Attached-to-leaves slice — leaf shuffle, per-leaf direction
  choice, probability/exclusion/empty-block rules, and the randomized integer
  block-state provider. A synthetic oak fixture places 6 mangrove propagules
  at seed 7. Full suite 331 passing; clippy/fmt clean.

- 2026-11-05: Mega jungle slice — `mega_jungle_trunk_placer` builds a giant
  2x2 trunk and walks angled branch logs; `jungle_foliage_placer` places the
  crown/branch foliage. Fixture at seed 7: 63 logs, 6 branch logs, 287 leaves.
  Full suite 330 passing; clippy/fmt clean.

- 2026-11-05: Mega spruce/pine slice — `giant_trunk_placer` now builds a
  2x2 trunk, `mega_pine_foliage_placer` places the crown shape, and
  `alter_ground` converts grass/dirt to podzol. Fixtures place 109 logs/295
  leaves (spruce) and 109/75 (pine), with 113 podzol each. Full suite 329
  passing; clippy/fmt clean.

- 2026-11-05: Cherry slice — `cherry_trunk_placer` samples branch count,
  length, starts and ends, then walks one or two axis-tagged branches;
  `cherry_foliage_placer` places five leaf rows with hanging leaves. Fixture
  places 20 logs/291 leaves and 8 horizontal logs. Full suite 328 passing;
  clippy/fmt clean.

- 2026-11-05: Fancy oak slice — `fancy_trunk_placer` limbs with axis-tagged
  branch logs, `fancy_foliage_placer` layers, and `min_clipped_height` support.
  Shipped `fancy_oak` fixture places 8 logs/112 leaves with 2 horizontal logs;
  a capped fixture verifies clipping to the minimum height of four. Full suite
  327 passing; clippy/fmt clean.

- 2026-11-05: Acacia slice — `forking_trunk_placer` now leans the trunk and
  adds a second branch in Java RNG order, with `acacia_foliage_placer` rows and
  signed corner clipping. Shipped `acacia` fixture places 11 logs/73 leaves.
  Full suite 325 passing; clippy/fmt clean.

- 2026-11-05: Native console and void-death slice — stdin commands reach live
  sessions through a per-session channel, operators persist in `run/ops.txt`,
  game-mode commands are operator-gated, and void fall damage now sends health,
  death, and respawn packets. Full suite 324 passing; clippy/fmt clean.

- 2026-11-05: Dark oak slice — the first multi-attachment trunk, 2x2 leaning
  trunk, dark-oak branch attachments, signed foliage clipping, and three-layer
  minimum-size support. Shipped `dark_oak` fixture places 11 logs/126 leaves.
  Full suite 320 passing; clippy/fmt clean.

- 2026-11-05: Spruce/pine foliage slice — `spruce_foliage_placer` and
  `pine_foliage_placer` use the straight trunk with Java's height/radius RNG
  order. Shipped fixtures place 7 logs/54 leaves (spruce) and 7/14 (pine).
  Full suite 319 passing; clippy/fmt clean.

- 2026-11-05: Vine/cocoa decorator slice — `leave_vine`, `trunk_vine`, and
  `cocoa` execute in vanilla order; the shipped `jungle_tree` and `swamp_oak`
  configs now parse and place. The jungle fixture produced 146 vines and 2
  cocoa blocks. Full suite 318 passing; clippy/fmt clean.

- 2026-11-05: Join/worldgen/game-mode slice — Join Game rebuilt from the live
  session (game mode, view distance, no flat flags), position sent before the
  chunk batch, floating survival players settled to the surface, game-mode
  changes via packet and `/gamemode`, oversized movement reverted, and a new
  end-to-end game-mode test. Full suite 312 passing; clippy/fmt clean;
  `dist/folia-server.exe` rebuilt.
- 2026-11-05: Chunk-packet hot path — each section palette entry is resolved
  once instead of hashing every block's name and properties (64-column packet
  encoding 3.28 s -> 0.28 s), packet building moved onto generation workers,
  and a palette-map regression test was added. The cold 169-column join test
  dropped from 3.55 s to 1.9 s. Full suite 313 passing; clippy/fmt clean.

- 2026-11-05: Tree decorator slice — `place_on_ground` leaf litter and
  `beehive` placement execute after trunk/foliage, using vanilla weighted-state
  draws and candidate shuffle. Unsupported decorator types now make the tree
  config unsupported instead of silently skipping them. Decorator regression
  tests cover leaf litter, beehive state/facing, and rejection. Full suite 316
  passing; clippy/fmt clean.

- 2026-11-05: First tree slice — `worldgen/feature/tree.rs` ports
  straight-trunk/blob-foliage oaks and birches, `random_selector` resolution,
  `heightmap(OCEAN_FLOOR)`, `surface_water_depth_filter`, and sapling
  `would_survive` predicates. Forest chunk `(0,0)` now contains logs/leaves
  and regenerates deterministically. `world_chunk_packet` uses an immutable
  fixture under `tests/fixtures/world`; shutdown aborts a chunk batch promptly
  when requested.

- 2026-11-05: View-distance negotiation (server `view-distance` ∧ client
  request, 2–16), parallel chunk generation in streaming batches, per-session
  `level.dat` time persistence on shutdown, and
  `tests/view_distance_join.rs`; full workspace suite green.
- 2026-11-03: Full block/item/entity/biome registry tables and `registry.rs`.
- 2026-11-03: `feature_data.json` extraction (224 configured, 258 placed
  features, 65 biome feature lists).
- 2026-11-02: Surface rules, carvers, aquifers, biomes, entity physics, and
  optional lazy JVM.
- 2026-11-04: `TODO.md` master roadmap, `AGENTS.md` autonomous guide,
  `STATE.md`, `DECISIONS.md`, `BLOCKERS.md`, `scripts/verify.ps1`, and
  `scripts/snapshot.ps1` created.
- 2026-11-04: Snapshot `snapshots/folia-rs-20261104-171409-before-p0-cleanup.zip`
  created before any cleanup work.
- 2026-11-04: P0.1 done — fmt and all 21 Clippy findings fixed by refactoring
  (no suppressions). P0.2 done — `HANDOFF.md`, `PORT_MAP.md`, and `README.md`
  refreshed to match the tree. P0.3 done — `assets/ASSETS.md` manifest and
  `tests/assets.rs` guard tests. P0.4 done — GitHub Actions CI for Windows and
  Linux. P0.6 done — snapshot `folia-rs-20261104-173335-p0-cleanup-done.zip`.
- 2026-11-04: Join streaming — missing chunks now come from the noise
  generator even when region files exist (flat only with no generator), and
  crossing a chunk border streams the newly visible column and re-centers the
  chunk cache. Covered by the extended `worldgen_join` test.
- 2026-11-04: P2.2 second slice — `worldgen/biome_zoom.rs` (`BiomeManager`:
  SHA-256 seed obfuscation, LCG fiddled distance, eight-corner quart
  search), `PlacementWorld` trait, `BiomePlacementWorld` adapter, and the
  `biome` placement filter. 667/888 shipped modifiers now execute; 6 new
  tests; 278 total; `verify.ps1` green; snapshot
  `folia-rs-20261104-193508-p2-biome-filter.zip`.
- 2026-11-04: P2.1 first slice — feature framework, typed `feature_data.json`
  loading, chunk-bounded `WorldGenRegion`, `Decoration` ordering, and 9 tests.
- 2026-11-04: P2.2 first slice — typed `PlacementModifier`/`IntProvider`/
  `HeightProvider` with vanilla sampling and draw order; 19 tests; 272 tests
  pass and `scripts/verify.ps1` is green.

## Blockers

- None currently recorded. See `BLOCKERS.md`.

## Notes for the next session

- Do not use Git.
- Do not ask the user; use `AGENTS.md` decision policy.
- Always run `.\scripts\verify.ps1` before and after a task.
- If you change generated assets, update the expected counts/checksums in
  tests.
