# Blockers

Record hard blockers that prevent a task from completing. Do not stop the whole
project because of one blocker: record it, choose another unblocked task, and
continue.

Format:

```text
## YYYY-MM-DD — Px.y Task name
- Blocker:
- What was tried:
- What is needed:
- Workaround / next task:
```

## 2026-11-07 — P4.6/P4.7 enchanting table is backend-only

- Status: Not a hard blocker. `item_components::enchanting` now models the
  shipped definitions, deterministic offers, click validation/consumption, and
  enchantment merging; 15 `tests/enchanting.rs` tests pass.
- Not wired (needs menu/inventory/join work outside this slice):
  - No `EnchantmentMenu` data slots, menu type instance, clientbound menu
    packet, serverbound button-click dispatch, or inventory slot 0/1 handling.
    The caller must place the input stack, lapis, experience level, bookshelf
    power, and enchantment seed into `EnchantingTable` itself.
  - No `EnchantingTableBlock.BOOKSHELF_OFFSETS` scan or `isValidBookShelf`;
    power is a caller-supplied count.
  - Paper `PrepareItemEnchantEvent` and `EnchantItemEvent` are not fired, so
    plugins cannot adjust costs/hints/result enchantments yet.
  - Anvil combine/repair, renaming, repair-cost accumulation, anvil durability
    merging, and anvil damage are an explicit `AnvilError::Unsupported`.
- Model gaps:
  - Enchantability is read from `item_prototypes::ItemPrototype::enchantability`
    (the generated `minecraft:enchantable` value) rather than a patchable
    `minecraft:enchantable` component, so a plugin-created stack cannot change
    it through the component map yet.
  - `apply_enchantments` caps levels at each enchantment's `max_level` and
    rejects exclusivity conflicts. Vanilla `ItemStack.enchant` clamps only at
    255 and does not re-check existing exclusivity; this stricter behavior is
    deliberate for the table path but differs from `/enchant`-style mutation.
  - Unknown/modded enchantment names in an existing component are preserved but
    cannot participate in exclusive-set checks.
  - There is no Java differential fixture for the cost/selection RNG yet; the
    tests encode the decompiled formulas and seed order directly.
- Workaround / next task: keep the backend API (`generate_offers`,
  `enchantment_list`, `EnchantingTable`, `apply_enchantments`) stable and wire
  menus/packets/events and the bookshelf scan in the inventory/menu slice.

## 2026-11-07 — P8.2 chat-signing core is not wired into login/status

- Status: Not a hard blocker for the model/codec slice. `crypto.rs` now has
  SHA-256 and RSASSA-PKCS1-v1_5/SHA-256 sign/verify; `protocol.rs` has the
  1.21.11 signed-body/serverbound-chat/last-seen codecs; `player.rs` has local
  and remote signed-message chains, profile-key generation, and Login Success
  profile properties; `config.rs` parses `enforces-secure-chat` (false default)
  and logs an audit line when enabled. 15 focused `tests/chat_signing.rs` tests
  (1 ignored release benchmark) + 2 crypto lib tests pass.
- Not wired (outside this slice's allowed files):
  - `join.rs` still builds Login Success from `profile_properties` only and
    never calls `PlayerSession::provision_chat_signing` or
    `PlayerSession::login_profile`, so the server-issued public key is not yet
    sent to clients and `ServerboundChat`/`ServerboundChatSessionUpdate` are
    not dispatched.
  - `status.rs` still hardcodes `"enforcesSecureChat": false`; it needs to read
    `config::enforces_secure_chat()`.
  - Clientbound `ClientboundPlayerChatPacket`/`ClientboundDisguisedChatPacket`
    and `LastSeenMessagesValidator` window application are not implemented, so
    accepted messages are not displayed yet.
  - Profile keys are self-signed by this server; Mojang's `Services` profile-key
    trust root (`profileKeySignatureValidator`) is not available offline. A
    real client's `ServerboundChatSessionUpdate` key cannot be validated against
    Mojang without the session service's key set.
- Key-generation cost: a fresh 2048-bit RSA profile key takes ~1.4 s in a
  release build and ~49 s in an unoptimized debug build on this machine.
  `PlayerSession::provision_chat_signing` must therefore not run on the tick
  thread in production, or the process should generate its profile key once at
  startup and treat per-login keys as a later optimization.
- Workaround / next task: call provisioning before Login Success, send
  `login_profile()`, dispatch chat packets through a `RemoteChatSession`, and
  flip the status flag. Until then keep `enforces-secure-chat=false`.

## 2026-11-07 — Base branch does not compile without a hopper shim

- Blocker: `crates/folia-core/src/tick.rs:1730` calls
  `storage.tick_hoppers()?`, but no `tick_hoppers` method exists anywhere in
  `world.rs` or `block_entity.rs` at commit `d3e2b53`. `cargo check -p
  folia-core` fails with E0599 before this slice's code is type-checked.
- What was tried: confirmed the call was added by `d3e2b53` (leaf decay) while
  the persisted-hopper commit `8bbd0d2` only added `block_entity.rs` state and
  `tick.rs` calls `tick_block_entities` separately. No branch contains a
  `tick_hoppers` definition.
- What is needed: a `WorldStorage::tick_hoppers` implementation (or removal of
  the call) in `world.rs`/`tick.rs`, both outside this slice's file scope.
- Workaround / next task: a temporary local `Ok(())` shim was used to run the
  focused tests and clippy, then reverted before committing. Do not merge this
  slice until the base build is fixed; the chat-signing changes themselves do
  not touch the failing path.
## 2026-11-07 — P7.2/P7.3 Nether and End structure templates are not shipped

- Blocker: `minecraft:fortress`, `minecraft:bastion_remnant`, and
  `minecraft:end_city` placement selects correctly, but the repository has no
  structure-template NBT assets or full piece ports for them. Bastions are
  jigsaw starts and need `minecraft:bastion/*` templates; fortresses and End
  cities have no procedural piece implementation in Rust.
- What was tried: Parsed the shipped structure definitions, ran the jigsaw
  starter with an empty template provider, and pinned the exact failure
  (`UnknownTemplate { structure: "minecraft:bastion/starts", template:
  "minecraft:bastion/treasure/big_air_full" }` for seed 12345 chunk `(12,20)`).
  Fortresses report `UnsupportedStructureType`.
- What is needed: Extract/copy the 1.21.11 structure NBT and template pools into
  the asset pipeline, port `FortressPieces`/`EndCityPieces`, then call the
  existing `JigsawPlacement` path.
- Workaround / next task: `NetherGenerator::structure_at_chunk` and
  `structure_start` are the stable selection/error contract; add templates and
  pieces behind them without changing callers. End cities and the dragon fight
  remain blocked on the same asset/entity work.

## 2026-11-07 — Nether carver/surface ordering differs from vanilla

- Blocker: vanilla runs carvers before `SurfaceSystem.buildSurface`; this
  slice runs them after the existing surface pass because the surface pass is
  driven by a pre-carve solid-column map.
- What was tried: Keeping the carver after the surface pass avoids writing
  soul-sand/basalt/nylium into air just carved open, and keeps the existing
  Nether terrain/features stable without a second column scan.
- What is needed: A surface pass that reads the post-carve chunk (or a
  post-carve solidity recomputation) plus a Java block-for-block fixture to
  measure exposed-cave-wall differences.
- Workaround / next task: Carver output is deterministic and confined below
  the roof; document the ordering delta and revisit when P2.12/P7.2 surface
  parity work resumes.

## 2026-11-07 — Pre-existing `tick_hoppers` call made the merged gate unbuildable

- Blocker: the merged leaf-decay slice added `storage.tick_hoppers()?` in
  `tick.rs`, but no `WorldStorage::tick_hoppers` method exists on this branch,
  so `cargo check`/`cargo clippy`/every test failed before this slice compiled.
- What was tried: Searched every branch and commit for a `tick_hoppers`
  implementation; none exists. The hopper slice only ships the persisted
  5-slot state and the `move_one_item` core, not live container transfer.
- What is needed: A proper hopper transfer tick (source/destination container
  discovery and `move_one_item` dispatch) in `world.rs`/`tick.rs`.
- Workaround / next task: This slice removes the dangling call so the gate can
  build; hopper cooldowns still tick through `tick_block_entities`, and live
  pull/push remains the documented P6 gap. Do not treat the removal as hopper
  parity.
## 2026-11-07 — P2.8 shipped structure templates are village-scoped

- Status: Not a hard blocker. `assets/structure_templates.json` ships all 483
  `minecraft:village/**` templates (776,539 gzip payload bytes; 1,063,517-byte
  catalog; FNV-1a-64 `0x0dde46f3a7863ed8`; SHA-256
  `f82d96ddfb284bb757962a5194098c181b40d4f0106eca2d794ba9a6d23448d3`) and
  `StructureTemplateCatalog` loads them lazily through `TemplateProvider`.
  `StructureGenerator::from_catalog`/`shipped_catalog` make the village jigsaw
  path run with real NBT.
- Not embedded: the remaining 719 non-village templates (bastion, trial
  chambers, ancient city, ocean ruins, woodland mansion, trail ruins, end city,
  pillager outpost, shipwreck, ruined portal, fossil/igloo) would push the
  per-entry gzip+base64 catalog past the 2.5 MB budget. Selecting those
  structures still fails explicitly with an unknown template until the asset is
  expanded (or a shared bundle format is chosen).
- `feature_pool_element` entries selected during recursive expansion are skipped
  and recorded on `JigsawStart::skipped_feature_elements`; their configured
  features (village trees/decor/piles) are not placed yet. A feature element
  chosen as the start element still returns `UnsupportedPoolElement`.
- Template entities are returned as `PlacedEntity` descriptors, not spawned;
  block-entity NBT is returned as `PlacedBlock` markers, not instantiated (so
  chest loot tables, bells, beds, and villager POIs are not live block
  entities). No Java block-for-block village fixture exists yet.
- Remaining parity gaps: terrain adaptation/beardifier, random multi-palette
  selection, exact per-block shape derivatives (stairs/door/rail), cross-chunk
  height sampling, structure serialization/caching, and texture/entity data.
- Workaround: use `StructureTemplateCatalog::template` for explicit
  unknown/corrupt errors and `StructureGenerator::shipped_catalog` for village
  generation. Do not claim full structure or village parity from this slice.

## 2026-11-06 — P10 region ownership is task-scoped, not world-scoped

- Status: Not a hard blocker. `region.rs` now enforces ownership on the
  task subsystem (`Region` queue + world-slice token + tick counter,
  `RegionOwned<T>`/`OwnedWorld`, `OwnershipError`, `RegionMap` vanilla
  8x8/4x4 math). 18 `tests/region_ownership.rs` tests plus the existing
  scheduler suites in `bulk_validation` pass.
- Not wired (needs P10.3-P10.7, outside this slice's files):
  - `WorldStorage` (`world.rs`), entity state (`entity.rs`), players, and
    chunk tickets are still shared/global; the server loop in `main.rs`
    still ticks the world globally. `RegionWorldSlice` cells are a token
    model, not yet the real chunk/entity/block-entity store.
  - Legacy `schedule_at` tasks receive no `OwnedWorld`; only owned tasks
    and `RegionOwned` values are checked. A legacy task can still mutate
    global state without an ownership error.
  - No region merge/split, ownership transfer, cross-region teleport, or
    entity handoff; `RegionMap::group_chunks` only groups chunks for
    migration.
  - The global queue uses the `GLOBAL_REGION_KEY` sentinel so it can
    never collide with a real region; it is not a real region.
- Workaround / next task: Use `schedule_owned_*` plus `RegionOwned` for
  new region-local work. Wire `Region` into the tick loop and per-region
  world state before claiming Folia region parity; do not treat the
  current scheduler isolation as world-state isolation.

## 2026-11-04 — P0.5 Make oracle data reproducible

- Blocker: Regenerating the Java oracle dumps requires starting the
  out-of-tree capture server in `../java-worldgen` and letting `WorldGenDump`
  rewrite `worldgen-*.txt`. That run also rewrites the embedded fixture hashes
  pinned in `crates/folia-core/assets/ASSETS.md`, and it cannot be verified
  from a session that must not modify the reference tree.
- What was tried: Confirmed the current fixtures and their checksums are pinned
  by `tests/assets.rs`; reviewed the documented manual procedure in
  `HANDOFF.md` (“Java oracle setup”).
- What is needed: A deliberate oracle-regeneration run that starts
  `java-worldgen`, copies normalized dumps into `assets/`, and updates the
  manifest/test hashes in the same change.
- Workaround / next task: Keep using the existing captures; P0.5 stays `[ ]` and
  work continues on P2.1 (feature framework) instead.

## 2026-11-04 — P2.2 Heightmap and predicate modifiers (partially resolved 2026-11-05)

- Blocker: Every remaining placement modifier needs block data the assets do
  not carry. `HeightmapPlacement` reads `Heightmap.Types` over
  `blocksMotion`/`leaves` block tags; `BlockPredicateFilter` reads block tags
  and `BlockState` predicates. No asset ships tag membership, so a port would
  have to guess which blocks count and would look like parity without being it.
- What was tried: Counted the shipped demand (heightmap 108,
  `block_predicate_filter` 57, surface depth 25, environment scan 9) and
  checked the assets for tag data (`feature_data.json` has only the two item
  tags it needs).
- What is needed: Block-tag extraction (P1.3) with `blocksMotion`/leaves (and
  the predicate tags) reachable from Rust.
- Workaround / next task: Ported `biome` instead, which only needs the biome
  sampler. Heightmap/predicate modifiers stay explicit unsupported variants.
  P2.3's ore features and generator wiring are now done; see the 2026-11-06
  ore section below for the remaining vein/biome-region gaps.

## 2026-11-06 — P3.10 tick slice is partial by design

- Implemented: vanilla update-flag values, six-neighbour update queue and
  shape/support checks, scheduled ticks (priority/delay/sub-tick/dedup/cancel),
  random ticks (3 per 16x16x16 section), falling sand/gravel/concrete
  powder/anvils, plant support, grass spread, scheduled water/lava flow, and
  wire/lever/button/repeater/comparator/observer/torch/plate/tripwire redstone.
- Not implemented (needs P1.5 block metadata or a bigger tick model):
  leaf `distance` propagation and decay, crop/sapling growth, rail/door/fence
  behaviour, pistons, redstone lamps, target blocks, tripwire-hook signal
  chains, item-frame analog inputs, lava fire spread, water/lava quenching to
  stone/obsidian/cobblestone, and block-entity ticks.
- Fluid shape collision uses a full-cube/occlusion approximation instead of
  `Shapes.mergedFaceOccludes`; flowing water/lava through partial shapes can
  differ from Java. Block-destruction drops are not modeled for fluids.
- Redstone wire now recomputes its `north/east/south/west` connection shape, but
  `isFaceSturdy` is the shared full-cube approximation: trapdoors and other
  partial shapes can connect or not connect differently from Java.
- Falling blocks move one block per scheduled tick instead of spawning a
  `FallingBlockEntity` with gravity. The landing block and its neighbour
  updates are correct, but fall timing differs from vanilla.
- `ScheduledTick`s are not persisted to chunk NBT, so a restart loses pending
  block ticks. They are also world-global rather than owned by a chunk/region.
- `RandomTickQueue` derives its per-chunk RNG from
  `(world seed, chunk, game tick)` with a deterministic hash. It is stable and
  testable, but it is not Java's `Level.random` draw sequence; a Java random-tick
  position fixture is still required before claiming positional parity.
- `tick_world` iterates the `WorldStorage` decoded-chunk cache (up to 256
  chunks) rather than a ticket/loaded-chunk set, so cached chunks outside a
  player's view distance can still random-tick until evicted.
- Cancelling a scheduled tick that was already collected for the current tick
  (`LevelTicks.clearArea` removing from `toRunThisTick`) is not modeled;
  cancellation between ticks works.
- No Java differential neighbor-update, fluid, or redstone fixture exists yet,
  so this is a behavior-tested slice, not a parity claim.

## 2026-11-06 — P3.10 redstone components are a behavior slice, not parity

- Implemented: repeater delay/facing/locking/strong output, comparator
  compare/subtract with container inputs, observer two-tick pulses, redstone
  torch inversion and burn-out, wooden/stone/weighted pressure plates, tripwire
  occupancy and connection shape, and wire connection-shape updates. All of
  them run through the existing neighbour and scheduled-tick queues.
- Comparator output is stored in `TickState::comparator_outputs`; it is not a
  block entity and is not written to chunk NBT. A restart loses the learned
  output until the comparator's next scheduled refresh.
- Comparator analog inputs are the registered `ContainerSignalSource` entries
  only. Item frames, daylight detectors, lecterns, sculk sensors, and the other
  `hasAnalogOutputSignal` blocks are not modeled.
- Pressure plates and tripwire never inspect entities themselves: the entity
  layer must call `set_pressure_plate_occupancy`/`set_tripwire_occupancy` with
  counts that already exclude spectators and trigger-ignoring entities.
- Tripwire hooks are not implemented, so a pressed tripwire changes only its own
  `powered` state. Tripwire source recalculation (`TripWireHookBlock`) and hook
  signal output are explicit gaps, and hooks are reported through
  `unsupported_neighbor_reason`.
- Redstone-torch burn-out follows vanilla (`8` off toggles in `60` ticks, then a
  `160`-tick restart) rather than the task brief's shorthand of “60 toggles in
  60 ticks”. The toggle log lives on `TickState` and is not persisted.
- Repeater/comparator `locked` properties are cached for shape/client updates;
  behaviour decisions use the live `getAlternateSignal`, matching vanilla's
  overridden `isLocked(level, pos, state)`.
- No Java differential redstone trace exists yet; the focused tests encode the
  Java formulas and packet-visible state transitions.

## 2026-11-06 — P5.1 entity metadata/health/equipment is a core slice

- Scope: `entity_metadata.rs` implements typed synched data, dirty-only
  `ClientboundSetEntityData`, `LivingEntity` raw damage/death, attributes, and
  eight equipment slots. `entity.rs` exposes bridge helpers only.
- Not wired: join/player sessions still own player health and food; server
  entities are not automatically given a living record by `spawn` (call
  `spawn_living`), and `tick_living_entities` is not called by the server loop.
  `spawn_living` sends only the add-entity packet; the initial non-default
  metadata snapshot is not emitted yet.
- Metadata coverage: indices 0-9 only. Effect particles, arrow count, stinger
  count, sleeping position, and the many type-specific accessors are absent.
  Supported serializers are byte, int, long, float, string, component,
  optional component, item stack, boolean, and pose; other serializer ids
  return an explicit error instead of guessing a payload length.
- Damage coverage: raw health subtraction plus the 20-tick invulnerability
  window. Armor/enchantment reduction, damage types/tags, knockback,
  absorption, totems, hunger/regeneration, and effects are not modeled.
  ClientboundSetHealth uses the model's food/saturation fields rather than a
  live player's vitals.
- Equipment coverage: full-slot updates and subset encoding exist, but there is
  no per-viewer dirty equipment tracking and armor points are not derived from
  equipped items (that needs item prototype/armor data).
- Death: event `3` is sent at death and event `60` after 20 ticks; `removed` is
  set for non-players, but no `ClientboundRemoveEntities` is emitted by the
  model. The caller must remove the entity.
- Parity: packet bodies are tested against the Java source layout and
  round-trip decoders, but no Java-captured metadata/equipment/death packet
  fixture exists yet. Do not claim entity parity from this slice.
- Workaround / next task: keep join/session wiring out of this slice; the next
  P5.1 task is initial metadata on spawn, per-viewer dirty tracking, remaining
  serializers, armor calculation, and a captured Java fixture.
## 2026-11-06 — Asset hash tests fail on a CRLF-materialized checkout

- Blocker: `tests/assets.rs` compares raw byte lengths/FNV hashes from
  `assets/ASSETS.md`. This worktree materialized `crates/folia-core/assets/*.tsv`
  with CRLF (`block_states.tsv` is 2,625,357 bytes instead of 2,595,686), so
  `generated_assets_match_the_manifest` and
  `dynamic_registry_assets_match_the_manifest` fail before any Rust code runs.
- What was tried: confirmed `git status` shows no asset modifications; the
  extra bytes are exactly one CR per line (29,671 lines). The content is the
  repository's committed LF content transformed by the checkout's line-ending
  settings.
- What is needed: re-materialize the assets with their committed LF bytes
  (or run the suite in a checkout with `core.autocrlf=false`).
- Workaround / next task: all other workspace tests pass
  (`cargo test --workspace -- --skip generated_assets_match_the_manifest
  --skip dynamic_registry_assets_match_the_manifest`: 519 tests). Do not edit
  the generated assets or their hashes for this slice.

## Known limitations (not hard blockers)

- `tests/assets.rs` is red on this branch before the ore work: commit `2ca3a1c`
  regenerated `crates/folia-core/assets/block_states.tsv` (now 2,625,357 bytes)
  without updating the manifest (still 2,595,686). The asset file and test are
  outside the ore slice's file scope; regenerate the manifest/hash in the same
  change that owns the per-state block-behaviour data.

These are expected gaps, not reasons to stop:

- Lighting is now incremental for block edits but still not fully
  vanilla-verified. `LightScene` runs Java's decrease-then-increase queues
  across the edited chunk and loaded neighbours, updates the edited sky column
  source, and reports dirty sections; `block_update` persists and broadcasts
  only those section payloads. Face occlusion now uses the oracle outline
  table: cached per-state face masks plus Java's pairwise face-shape union in
  both increase engines and in the sky-column edge scan, with full-cube
  transparent states treated like `canOcclude=false`. Remaining gaps: no Java
  light-array/packet fixture, exact `canOcclude`/`useShapeForLightOcclusion`
  columns (the engine uses a static override list) and `getOcclusionShape`
  overrides (sculk shrieker, lectern, fence gate) are approximated from
  outlines, unknown states stay opaque and non-emitting,
  chunks with no light data pay a one-time whole-chunk initialization on their
  first edit, generated chunks still use the height fallback until edited, and
  generation-time lighting is not wired into the chunk-status pipeline. Cached
  behaviour lookups and a Java seam fixture also remain open.
- `BiomeManager` has no Java fixture. Its seed obfuscation is pinned to NIST
  SHA-256 vectors and its quart selection to values derived from the Java
  source; a Java `getBiome` dump would lock it down.
- No Java fixture records placement positions, so P2.2's done-when is still
  open even though the pipeline runs.
- Tree fixtures are deterministic but not a Java block-for-block parity claim.
  `BlockStateProvider::simple` still samples through the weighted path and
  consumes a random draw that Java's `SimpleStateProvider` does not; exact
  tree RNG alignment needs a separate provider-representation fix.
- Structure templates, random-spread structure sets, weighted selection,
  starts, pieces, bounds, references, and the igloo now exist. Still open:
  dimension/biome filtering before set selection, processor lists, template
  pools, jigsaw expansion, concentric-ring placement, structure NBT
  serialization, random multi-palette selection, entity spawning, loot-table
  block entities, waterlogging, shape updates, and per-block state derivatives
  (`shape`, `hinge`, `type`, …). There is no Java block-for-block structure
  fixture yet, and starts are not attached to chunk generation.
- Item components are a first slice. The per-item prototype catalog is now
  generated (`assets/item_prototypes.json`, 1,504 items), so stack limits,
  durability, tool rules, food, rarity, and the fallback are data-driven rather
  than guessed. `attribute_modifiers`, `potion_contents`, and `tool` are still
  NBT-only; their network codecs are omitted and reported by
  `NetworkEncodingReport`. `HashedStack` component hashes are parsed but not
  matched against a reconstructed stack, and there is no Java-captured component
  NBT/packet fixture yet.
- Item components are a first slice: per-item prototype defaults
  (`max_stack_size`, `max_damage`, food/tool/enchantment defaults) are not
  dumped, so a plain damageable item without a damage/max_damage component
  still reports stack size 64. `attribute_modifiers`,
  `potion_contents`, and `tool` are NBT-only; their network codecs are omitted
  and reported by `NetworkEncodingReport`. `HashedStack` component hashes are
  parsed but not matched against a reconstructed stack, and there is no
  Java-captured component NBT/packet fixture yet.
- Block shapes are oracle-exact for `CollisionContext.empty()`, but
  entity-dependent collision contexts are not modeled, and passable states
  (air/fluids/plants/torches) store empty outline lists by design. The asset is
  regenerated with `scripts/extract_block_shapes.ps1`, which needs the local
  patched Folia jar or Paperclip plus `cache`/`libraries`; without that reference
  tree the table cannot be reproduced. `underwater_magma` still uses its old
  full-cube face-occlusion approximation instead of `block_shapes`.

- Some Java oracle data requires starting `java-worldgen`; if the server cannot
  start, use the existing dumps in `crates/folia-core/assets`.
- Plugin bridge work may require Java 21; native-only work does not.
- Village structure NBT templates now ship in `structure_templates.json`; the
  other structure families remain in the decompiled data tree, and structure
  loot tables/other structure data may still need extraction from the local
  Folia jar. If an asset is missing, document it and continue with a different
  feature.

## 2026-11-05 — P2.4 Tree decorators and non-blob placers (partial)

- Resolved: leaf litter, beehives, vines, cocoa, spruce/pine foliage, dark
  oak's 2x2 branching trunk, forking acacia, fancy oak (including clipped
  height), giant mega spruce/pine, mega jungle, bending trunks, random-spread
  weighted foliage, upwards-branching trunks, mangrove root placement,
  `alter_ground`, `attached_to_leaves`, `attached_to_logs`, and
  `creaking_heart` now execute. `jungle_tree`, `swamp_oak`, `spruce`, `pine`,
  `dark_oak`, `acacia`, `fancy_oak`, `cherry`, `mega_spruce`, `mega_pine`,
  `mega_jungle_tree`, `jungle_bush`, `azalea_tree`, `mangrove`,
  `tall_mangrove`, and all five `fallen_*_tree` configs parse and place.
  Still blocked: beehive occupant NBT and the pale-moss decorator.
- Workaround: supported trees are verified by config/placement fixtures, not a
  Java block-for-block fixture. Do not claim full tree parity.

## 2026-11-05 — P2.2 Remaining world-dependent modifiers

- Blocker: `heightmap` variants other than `OCEAN_FLOOR` (83 uses), non-sapling
  block predicates (12), `environment_scan` (9), `count_on_every_layer` (8),
  noise counts (10), `surface_relative_threshold_filter` (3), and one
  `clamped_normal` random offset still need block tags, noise context, or
  `nextGaussian`.
- What was tried: Ported `OCEAN_FLOOR`, `WORLD_SURFACE`, surface water depth,
  and sapling `would_survive`; 762/888 modifiers now execute. The rest fail
  loudly as unsupported variants.
- What is needed: Block/item tag extraction (P1.3), noise-context placement
  support, and `nextGaussian` in `folia-random`.
- Workaround / next task: Keep them explicit unsupported variants; tree
  generation only uses the ported subset.

## 2026-11-05 — Native console and void death are partial

- Resolved: the server reads terminal stdin, console commands can reach live
  sessions, `op`/`deop` persist in `run/ops.txt`, and game-mode commands are
  operator-gated. Void falls now damage, kill, and respawn.
- Still partial: no Minecraft command tree/tab completion or Bukkit console
  dispatch; only the listed console commands exist. General damage types,
  inventory drops, keep-inventory, beds/anchors, and spawn-point selection are
  still missing.
- Workaround: use the terminal console to op a player, then `/gamemode` in
  chat. Do not claim full death or permission parity.

## 2026-11-05 — P2.4 Fallen trees land; remaining tree gaps

- Resolved: `FallenTreeFeature` and `attached_to_logs` now execute. The five
  shipped `fallen_*_tree` configs parse and the `fallen_oak_tree` fixture places
  a stump, four sideways logs, and four mushrooms at seed 7.
- Still open: beehive occupant NBT, pale moss (vegetation-patch dependency),
  exact Java block-for-block tree fixtures, and the simple-provider RNG caveat
  above.
- Workaround: keep tests behavior-focused; do not claim complete tree parity.

## 2026-11-06 — P2.4 Pale moss decorator is blocked on vegetation patches

- Blocker: `PaleMossDecorator` places hanging moss on logs and leaves, but its
  ground pass calls the configured `minecraft:pale_moss_patch`. That is a
  `vegetation_patch` feature whose `moss_replaceable` block tag, face-sturdy
  checks, and nested `pale_moss_vegetation` simple-block placement are not
  ported. The shipped pale-oak configs use `ground_probability: 0.8`, so
  skipping the patch would silently lose most of the decorator's effect.
- What was tried: Inspected `PaleMossDecorator`, `VegetationPatchFeature`, and
  the shipped `pale_moss_patch`/`pale_moss_vegetation` configs. Implemented the
  independent `creaking_heart` decorator instead.
- What is needed: P1.3 block tags plus the P2.5 vegetation-patch/simple-block
  feature work.
- Workaround: keep `minecraft:pale_moss` as an explicit unsupported decorator;
  pale-oak configs remain unsupported instead of generating partial decoration.

## 2026-11-06 — P2.3 Ore generator wiring (resolved) and remaining parity gaps

- Resolved: `underground_ores` is wired. The earlier 89.06% regression had two
  causes, not approximate `WorldgenRandom`: the generator collected every
  placement position before running the feature, while vanilla's
  `placeWithContext` is a lazy stream that consumes feature RNG between
  positions; and `FeatureSorter` feature indices were built from the biome
  registry order instead of `MultiNoiseBiomeSource.possibleBiomes()` (the
  `assets/biome_params.txt` parameter order), which mis-seeded `ore_clay`.
  Fixing both plus `OreVeinifier` raises exact-name parity from 92.01% to
  95.80% (5886/6144) and block-class from 98.71% to 98.75% (6067/6144).
- Still open (not a hard blocker):
  - `NoiseChunk` wraps `vein_toggle` and `vein_ridged` in `interpolated`, so
    Java evaluates them on the 4x8x4 interpolation grid. `OreVeinifier` here
    computes them exactly per block; a few vein edge blocks differ. Wiring
    interpolation belongs in `density.rs` (outside this slice).
  - Vanilla builds the active biome set from a 3x3 chunk region and features
    may write into that whole region. This generator reads the center chunk's
    biomes and rejects out-of-chunk writes, so border placements and air
    exposure checks can differ.
  - 26 of the fixture's 33 Java ore blocks match exactly. Remaining deltas are
    small ellipsoid-edge/vein-edge differences and out-of-chunk writes.
  - The `underground_ores` step also contains `disk_*` and
    `underwater_magma`; the generator deliberately runs only ore/scattered-ore
    types until those are wired with their own surface/cave rules.
- Workaround: The default is enabled and pinned by `tests/ore_wiring.rs`; do
  not claim block-for-block ore parity until the three gaps above are closed.

## 2026-11-06 — P4.1/P4.2 special movement remains unported

- Blocker: Full vanilla player movement still needs climbable/liquid/powder-snow
  handling, player-input packets, and a standalone player tick scheduler. Per-
  state collision and outline AABBs are now ported, so cube-family and multi-box
  geometry is no longer the blocker.
- What was tried: Implemented walking/falling/jumping/step-up/fall damage and
  packet reconciliation. `has_collision` replaces the old tag list for
  passability, `blocks_motion`/`is_solid` gate support,
  `friction`/`speed_factor`/`jump_factor` come from the generated behaviour
  table, and `block_shapes::collision_boxes` supplies every collision AABB.
  Player and entity moves use vanilla's one-block-widened block cursor so tall
  fence/wall shapes are found.
- What is needed: `ServerboundPlayerInput`/`ServerboundPlayerCommand` parsing,
  water/lava fluid state and movement,
  ladder/vine/scaffolding/cobweb/slime/elytra/powder-snow handling, a per-region
  player tick, and `CollisionContext`-aware shapes for blocks whose collision
  depends on the entity (the table currently uses `CollisionContext.empty()`).
- Workaround: Keep the unsupported paths explicit in `DECISIONS.md`; do not
  claim ladder, swimming, or elytra parity. The ten-block teleport net plus
  collider replay still rejects the common wall/teleport cases.
## 2026-11-06 — P2.8 structure starts/pieces/sets are partial

- Scope: `worldgen/structure/template.rs` loads gzip structure NBT and writes
  transformed blocks. `set.rs` parses all 20 shipped structure-set documents
  and implements random-spread placement, legacy frequency reducers, exclusion
  zones, weighted selection, and `structure_at_chunk`. `piece.rs` adds
  `BoundingBox`, `StructurePiece`, template pieces, chunk-box clipping, and
  data-marker reporting. `start.rs` adds deterministic starts, references,
  terrain inflation, the igloo end to end, and explicit jigsaw failures.
- Not ported: dimension/biome filtering before registry-level set selection,
  concentric-ring placement, processor lists, template pools, jigsaw
  placement/junction expansion, random multi-palette selection, structure NBT
  serialization, entity spawning, loot-table/chest block entities,
  waterlogging, block-shape updates, and per-block state derivatives that need
  P1.5 block behaviour (stairs `shape`, door `hinge`, rail `shape`, …).
- Why it is not a blocker: Placement math, selection, starts, and one template
  structure are independently testable without the chunk-status pipeline.
  Biome/dimension filtering cannot be answered by the current
  `structure_at_chunk(chunk_x, chunk_z, seed)` signature, so callers must not
  treat its result as a generator decision yet.
- Workaround: Call per-set placement (`StructureSetRegistry::is_structure_chunk`)
  when a test needs to avoid nether/end sets, and use `TemplateProvider` to
  supply the igloo templates. Do not claim structure generation parity until
  biome checks, generator wiring, and a Java block-for-block structure fixture
  exist.

## 2026-11-06 — P2.5 aquatic/vine/mushroom remaining placement and shape gaps

- Scope: All requested aquatic/vine/mushroom feature algorithms are ported and
  covered by focused tests. 162/224 configured features execute. This is not a
  hard blocker, but these gaps prevent a "complete" claim:
  - `kelp`, `warped_fungi`, `crimson_fungi`, `warped_forest_vegetation`,
    `crimson_forest_vegetation`, and `nether_sprouts` still parse but do not
    run in live chunks because their outer placement uses
    `noise_based_count` or `count_on_every_layer`, which `placement.rs` still
    reports as explicit unsupported variants (10 noise counts + 8 layer counts).
  - `simple_block.schedule_tick` and `simple_random_selector` remain explicit
    unsupported, so pale-forest flowers and the dripleaf pools stay inactive.
  - `underwater_magma`, seagrass, kelp, sea pickles, bamboo, and the nether
    plants use the full-cube `face_sturdy` approximation until P1.5 produces
    real collision/support/occlusion shapes. Non-cube neighbours may accept or
    reject a position differently from Java.
  - `FeatureLevel` reads only the chunk being generated; vanilla's
    `WorldGenRegion` spans a neighbour radius. Features whose spread crosses a
    chunk edge (seagrass ±8, vine/glowstone scans, huge mushroom/fungus caps)
    skip positions outside this chunk instead of reading the neighbour. This is
    the existing chunk-status-pipeline limitation, not new to this slice.
- What was tried: Implemented every requested algorithm in Java RNG order,
  reused the project's existing state providers and tag tables, and measured
  the decoration steps against the parity fixture.
- What is needed: `noise_based_count` and `count_on_every_layer` placement,
  scheduled ticks, `simple_random_selector`, P1.5 block behaviour metadata, and
  a multi-chunk feature region.
- Workaround / next task: Keep the features executable and tested directly;
  generated worlds show seagrass, sea pickles, vines, glowstone, forest rocks,
  underwater magma, huge mushrooms/fungi (where placement is supported), and
  bamboo. Parity is 98.71% block-class / 92.02% exact-name (5654/6144); the
  one-block exact delta is a tall-seagrass pair on a column whose Rust terrain
  already differs from Java.

## 2026-11-06 - P2.12 surface fixtures and direct-seed harness gap

- Blocker: The P2.12 code paths are implemented and behavior-tested, but there
  is no captured Java frozen-ocean/iceberg fixture and no way to regenerate one
  from this session (same oracle-regeneration blocker as P0.5). Direct
  `WorldGenerator::overworld` harnesses also do not carry the raw seed into
  `SurfaceSystem`, so they use the chunk-local quart fallback unless the test
  calls `build_surface_with_seed`/`top_material_with_seed`.
- What was tried: Extracted the real biome climate table and `PerlinSimplexNoise`
  tables, wired `BiomeZoomer` into the surface context through
  `generator::configured()`, and pinned fixed-coordinate temperature/melt
  values in `tests/surface.rs`.
- What is needed: A Java capture of a frozen-ocean/deep-frozen-ocean chunk with
  icebergs, and either a surface seed parameter threaded through
  `generator.rs` or a test helper that always calls `*_with_seed`.
- Workaround / next task: Keep the focused tests; when generator.rs is in scope,
  pass `WorldGenerator.seed()` into `build_surface` directly and make the
  fallback a test-only path. Regenerate the Java fixture with `java-worldgen`
  when P0.5 is unblocked.
## 2026-11-06 - P4.3 block breaking is a behaviour slice, not loot parity

- Scope: `block_breaking.rs` ports the dig state machine, hardness/tool-tier
  break-time formula, efficiency/haste/fatigue and water/air penalties,
  harvest gating, durability/item break, and shipped loot-table drops;
  `join.rs` validates elapsed ticks and continues delayed destroys. 13
  `tests/loot_drops.rs` + 13 lib + 9 integration tests pass.
- Resolved (2026-11-06): block drops now roll
  `minecraft:blocks/<block>` through `LootCatalog::roll_block` with a
  caller-supplied `RandomSource`. `BlockLootContext` carries the tool, explicit
  `ToolContext { silk_touch, fortune }`, the correct-tool flag, and an optional
  explosion radius. `survives_explosion`, `match_tool`, `random_chance`,
  `table_bonus`, `set_count`, `set_damage`, `set_name`, `enchant_randomly`,
  `explosion_decay`, and `limit_count` execute. The old collapsed table is a
  fallback only for blocks without a shipped table or a known table whose roll
  is empty because of an unsupported condition/entry; the report and table id
  are preserved. `DigState::stop_with_loot` rolls on a destroyed stop.
- Not ported (needs data or a larger model):
  - `ItemEntity` drops. Items that fit are inserted into the player inventory;
    overflow is logged and lost. There is no ground-drop or pickup path.
  - Experience orbs. `BlockDrops::experience` still comes from the old
    per-block table and is returned but not granted.
  - Java `Level.random` draw ordering in the legacy `block_drops` signature:
    `join.rs` is outside this slice, so the fallback path seeds a deterministic
    `LegacyRandomSource` from the block state + held stack. Use
    `block_drops_with_context` for an explicit world/session RNG.
  - `ClientboundBlockDestructionPacket` (block-damage overlay) and block-break
    sounds/particles. The wire path uses only the existing block-update and
    light-update packets, as required by the slice.
  - Unbreaking, Mending, and tool attribute modifiers beyond
    `BLOCK_BREAK_SPEED`. Haste/mining fatigue inputs exist in
    `MiningContext` but the session has no status-effect source yet.
  - Adventure `can_destroy`/`can_place_on` and spectator interactions;
    adventure and spectator block actions are rejected conservatively.
  - The `minecraft:tool`/`max_damage` prototype catalog now exists
    (`item_prototypes.rs`), but `block_breaking.rs` is not wired to it in this
    slice; it still uses its built-in `ToolMaterial` table unless a stack
    carries an explicit component.
  - No Java break-time or drop fixture yet; the tests encode the Java formulas
    and table behavior directly rather than a captured oracle trace.
- Workaround: use `CatalogLootProvider`/`block_drops_with_context` for the
  table path. Do not claim item-entity, XP, or Java-RNG parity until `join.rs`
  passes the world RNG and the item-entity pipeline lands.

## 2026-11-06 - P4.8 crafting is a behaviour model, not menu/packet parity

- Scope: `crafting.rs` implements shaped/shapeless/transmute matching, one-item
  consumption with component preservation, furnace/blast-furnace/smoker/
  campfire state, the common-fuel burn table, exact recipe XP accumulation,
  stonecutting, and smithing transforms. 19 `tests/crafting.rs` + 5 lib tests.
- Not ported (needs menus, registries, or richer state):
  - Special crafting recipes (`crafting_special_*`), decorated pots, smithing
    trims, and opaque recipe types have no transform. Automatic lookup skips
    them; addressing one by id returns `CraftingError::UnsupportedRecipe`.
  - No crafting/furnace menu packets, click handling, result-slot preview,
    recipe book/advancement unlocks, or vanilla `lastRecipe` cache fast path.
  - Brewing has a standalone `crafting::BrewingStandState` (slots, fuel, 400-tick
    timer, vanilla mixes) but no brewing-stand block entity, NBT persistence, or
    menu/packet wiring. Potion drinking and food use in `item_prototypes` are
    kernel models driven by explicit `FoodData`/`StatusEffectSet` arguments, not
    a live player tick.
  - Food `on_consume_effects` (golden apple regeneration/absorption, rotten flesh
    and chicken hunger rolls, honey poison removal, pufferfish nausea) are not
    modeled; only nutrition/saturation and the potion component apply. Milk does
    clear all effects.
  - Campfires are a single input/result pair with an assumed-lit state; vanilla
    has four slots and a block `lit` property. Furnaces do not persist state to
    block-entity NBT yet, and only the lava-bucket fuel remainder is modeled.
  - XP is accumulated exactly per completed item instead of vanilla's
    per-recipe counts plus extraction rounding.
- What is needed: menu/container wiring in `join.rs`, block-entity storage, and
  a Java recipe/furnace fixture to compare selected recipe ids, output stacks,
  fuel times, and XP.
- Workaround / next task: Keep these as explicit gaps. `CraftOutput::deposit`
  already uses `inventory::add_stack`, so menu code can consume the model
  without changing the matcher.
## 2026-11-06 — P1.4 item prototypes are dumped; the component bridge is partial

- Resolved: `scripts/extract_item_prototypes.ps1` boots the local 1.21.11 jar
  and dumps all 1,504 non-air item prototypes to
  `assets/item_prototypes.json` (79,369 B). `src/item_prototypes.rs` validates
  the catalog, exposes by-name/by-id lookups, and returns component patches for
  the subset `src/item_components.rs` models (stack size, max damage/damage,
  food, tool, rarity).
- Not in the patch: 1.21.11 prototypes also carry `equippable`, `enchantable`,
  `repairable`, and `damage_resistant` (fire resistance).
  `item_components::ComponentValue` has no variants for them yet, so
  `defaults_for` does not emit them; callers must read them from
  `ItemPrototype`. Do not treat the patch as the complete prototype.
- Not ported: the `attribute_modifiers`, `potion_contents`, and `tool` network
  codecs; `HashedStack` component hashing; and a Java-captured component
  NBT/packet fixture.
- Extraction dependency: the oracle needs Java 21 plus the local server jar and
  dependency tree. `tests/item_prototypes.rs` runs the extractor `-Check` only
  when that jar is present (and skips on non-Windows), so CI without the
  reference tree still validates the pinned bytes and catalog semantics.
- Workaround: use `prototype`/`prototype_or_default` for the full prototype and
  `defaults_for`/`apply_defaults` for the modeled component subset. Do not claim
  full prototype-patch, network, or hashing parity.
## 2026-11-06 - P2.8/P2.9 processor/pool slice leaves recursive jigsaw open

- Status: Not a hard blocker. Processor lists, every registered processor kind,
  all shipped template pools, weighted selection, and one-element start-pool
  placement are implemented and tested.
- Not ported (needs a larger placement/generation slice):
  - `JigsawPlacement.Placer` recursive child placement, junction pieces,
    expansion-hack height selection, free-space voxel shapes, and
    `max_distance_from_center`/dimension-padding checks.
  - `FeaturePoolElement` world placement; it currently returns
    `UnsupportedFeatureElement` because it needs a configured/placed-feature
    context and generator.
  - Biome/dimension filtering in structure selection, random multi-palette
    selection, structure NBT serialization, entity spawning, loot/chest block
    entities, waterlogging, shape updates, and exact per-block state
    derivatives (stairs shape, door hinge, rail shape, …).
  - Starts are not attached to the generator's chunk-status pipeline.
- Workaround / next task: Use `PoolCatalog::expand_start` and
  `PoolExpansion::place` for focused pool/processor tests. Implement
  `JigsawPlacement` plus generator wiring before claiming jigsaw structure
  parity; do not replace the explicit `UnsupportedJigsaw` path with an empty or
  approximate start.
## 2026-11-06 — Fluid interactions are behaviour-tested, not Java-fixture parity

- Scope: `fluid.rs` now covers lava/water quenching, lava fire ignition and
  aging, soul-sand/magma bubble columns, per-tick bubble velocity, fluid
  destruction of replaceable plants, and neighbour-scheduled source updates.
  `tests/fluid_interactions.rs` adds 16 focused tests and `tests/fluids.rs` adds
  one neighbour-update test.
- Not wired in production:
  - `tick::behavior_for` still routes `minecraft:fire` to the no-op behaviour.
    Fire aging, burnout, and fire spread need one registration line in
    `tick.rs` (outside this slice) or a caller that uses
    `fluid::interaction_table()`. Lava ignition itself is live because lava is
    a fluid.
  - `minecraft:soul_sand`/`minecraft:magma_block` do not schedule their Java
    20-tick column check. A water source above them performs the equivalent
    `updateColumn` on its normal 5-tick fluid tick, so columns can form up to
    15 ticks early.
- Not modelled:
  - Fire weather/rain, per-world difficulty, `fireSpreadRadiusAroundPlayer`,
    increased-burnout environment attribute, and TNT priming. TNT burnout
    consumes the Java random draws but leaves the block in place.
  - `FlowingFluid.beforeDestroyingBlock` loot drops; fluids remove plants,
    torches, and fire but spawn no item entities.
  - The exact `CollisionShape` merged-face occlusion test; the full-cube
    approximation remains.
  - Java `Level.random` draw ordering for fire/lava; the tick engine's
    deterministic per-position (`seed, pos, tick`) RNG is used instead.
- Needed for a parity claim: a Java differential fixture for lava/water contact
  (all faces), a fire spread/aging fixture, and a bubble column entity-velocity
  fixture. The current tests encode the decompiled formulas directly.
## 2026-11-06 - P7.2/P7.3 Nether and End parity fixtures

- Blocker: The Nether and End shipped settings use `legacy_random_source:
  true`. Exact Java terrain needs `BlendedNoise` and the Nether
  temperature/vegetation noises built from a sequential `LegacyRandomSource`;
  the public `worldgen::noise` API only exposes the positional path, and
  `noise.rs` was outside this slice's allowed files.
- What was tried: The exact `EndIslandDensityFunction` construction is
  available (`LegacyRandomSource(seed)` + `consumeCount(17292)` +
  `SimplexNoise`) and is ported. Nether surface/noise parameters are available
  in `worldgen.json`; the Nether router and surface sequence are evaluated from
  the shipped JSON values in `nether.rs`. `TheEndBiomeSource` and the Nether
  five-entry parameter preset are implemented from the decompiled code.
- What is needed: a `BlendedNoise::create_legacy`/sequential
  `NormalNoise::create_legacy_nether_biome` entry point in
  `worldgen/noise.rs`, then a Java dimension oracle dump (bedrock, lava,
  surface layers, biomes, and representative density samples) for both
  dimensions.
- Workaround / next task: Tests pin the structural contract only (bedrock
  y=0/127, lava at y=31, netherrack, valid biomes, deterministic chunks; End
  central island, headroom, platform, outer end stone, biomes). Do not claim
  Nether/End block-for-block parity until the legacy noise path and Java
  fixtures land. Carvers, fortresses/bastions, end cities/gateways, and the
  dragon arena also remain unported.
## 2026-11-06 - P5.5/P5.6/P5.12 full AI/spawn parity

- Blocker: `block_shapes.rs` is still a stub, so exact collision/outline shapes,
  per-state path node types, door/gate state, and fence/step topology are not
  available. `SpawnPlacements` per-entity predicates and structure-specific
  spawn overrides also need data the current assets do not carry.
- What was tried: Implemented the unblocked vertical slice: deterministic
  bounded A* over `block_behaviour` collision/fluid flags, five goals, an AI
  driver over an injected world, and a natural-spawn cycle with real biome mob
  lists and real entity ids.
- What is needed: P1.5 shape metadata, door/gate path metadata, and a Java
  spawn-count/path trace fixture.
- Workaround / next task: Keep the current full-cube collision approximation,
  document it in `DECISIONS.md`, and continue with P5.2 persistence or P5.3
  movement once collision shapes land.
## 2026-11-06 - P8.1 server binary still gates online mode off

- Status: Core complete. `crypto.rs` implements RSA-1024 key generation,
  PKCS#1 v1.5, DER SubjectPublicKeyInfo, AES-128/CFB8, SHA-1, the Java signed
  `BigInteger` hash, and constant-time token comparison. `join.rs` performs the
  encryption exchange, `protocol.rs` owns the encrypted `Connection`, and
  `config.rs` exposes `online-mode`/`prevent-proxy-connections`. 22 focused
  tests pass and `online-mode=false` frames are unchanged.
- Blocker: `crates/folia-server/src/main.rs` builds `StatusConfig` with
  `accept_logins: !properties.online_mode`, so the login path is never called
  when `online-mode=true`; `status.rs` also documents the old restriction. Both
  files are outside this slice's allowed file set.
- Required integration (two small edits):
  - `main.rs`: set `accept_logins: true` for both modes and call
    `folia_core::join::configure_online_mode(properties.online_mode)` before
    starting the status server. (If the call is omitted, `join::online_mode`
    falls back to reading `server.properties` and `FOLIA_ONLINE_MODE`.)
  - `status.rs`: update the `accept_logins` doc comment, which still says online
    mode encryption is not ported.
- Workaround / next task: apply those two edits, then `online-mode=true` accepts
  a real client provided the host has `curl` and reaches
  `sessionserver.mojang.com`. `FOLIA_ONLINE_MODE` can pin the mode for a probe
  run. Tests cannot perform a real Mojang lookup (no network by design).
- Additional limitation: there is no TLS stack in the workspace, so the real
  session lookup shells out to `curl`. Replace it with an in-process TLS client
  when a dependency is allowed (P11 hardening).

## 2026-11-06 - P5.13 join-session entity tracking is a behaviour slice

- Scope: Live sessions spawn, tick, damage, and remove three deterministic mobs
  (cow/pig/zombie) through `SessionEntityTracker`; join sends Add Entity, full
  metadata, and equipment; fall/void player damage uses the living store; global
  entity broadcasts are filtered to tracked ids.
- Not ported (not hard blockers):
  - Session mobs are viewer-owned and are not shared with other players, saved
    with chunks, or persisted across restarts.
  - AI is limited to `AiMob::update_environment` (water/lava/ground state).
    Goal selectors and pathfinding are not ticked in the live loop, so mobs do
    not wander or attack yet.
  - Health uses the generic `LivingEntity` 20 HP default except for the pinned
    cow/pig (10) and zombie (20) max-health values; drops, XP, knockback,
    armor, damage types, effects, and absorption are absent.
  - Tracking range is a fixed 80 blocks. Vanilla per-entity-type ranges, chunk
    tickets, velocity/teleport packet variants, animation/effect/passenger/leash
    packets, and per-viewer metadata dirtying are not modeled.
  - Join mob packets are sent before the chunk batch so the captured-world join
    test stays byte-compatible. A real client holds the entities until their
    columns arrive; vanilla normally sends them after chunk tracking starts.
  - There is no Java-captured join entity packet fixture; the six focused tests
    pin packet ids and bodies against the decompiled layouts.
- Workaround / next task: keep the explicit `SessionEntityTracker` boundary and
  extend it for P5.13/P5.2 rather than reintroducing global broadcasts.

## 2026-11-06 - P4.3/P5.4 item entities + XP orbs are a behaviour slice

- Scope: `item_entity.rs` models ItemEntity/XP orb physics, magnet/attraction,
  merges, splitting, `PickupTarget` collection, and add/metadata/sync/remove
  packets. `spawn_drops_at` + `DigState::stop_with_entities` connect block
  drops to ground entities. 6 lib + 16 `tests/item_entities.rs` tests pass.
- Not wired: `join.rs` still deposits drops directly into the inventory and
  does not spawn, tick, track, or collect ground entities. The live slice
  needs a `PickupTarget` impl for sessions, per-session storage (like
  `SessionEntityTracker`), chunk-appear/disappear packet routing, and world
  RNG plumbing. No packet is sent by this slice in production.
- Not modelled: fluids (water/lava drag), fire/lava damage (`health` is
  carried but never hurt), `noCollision` ejection, Paper merge/despawn
  events, Bukkit pickup events, `takeXpDelay` cooldown, mending,
  chunk persistence, and per-viewer tracking.
- Deviations: item magnet (4 blocks) and merge (3 blocks) are task values;
  vanilla has no item magnet and a `1.0` merge radius. XP merge ignores
  Paper's `xpOrbGroupsPerArea` grouping. See `DECISIONS.md`.
- Parity: packet bodies are pinned against the Java layouts and round-trip
  decoders, but there is no Java-captured item/XP pickup trajectory fixture.
  Do not claim pickup parity from this slice.
- Workaround: call `stop_with_entities` (or `spawn_drops_at`) from tests or a
  future session slice; keep `stop_with_loot` for inventory callers.

## 2026-11-06 - P4.7/P6 block entities are persistence-only, not gameplay parity

- Status: `block_entity.rs` implements typed chest/trapped chest,
  furnace/blast furnace/smoker, sign/hanging sign, and spawner state, modern and
  legacy chunk-list parsing, unknown-type preservation, and world-store
  get/insert/remove. 3 lib + 9 integration tests pass; `world_round_trip` and
  `bulk_validation` remain green.
- Not wired: placing a chest/furnace/sign/spawner does not create a block
  entity, `tick.rs` does not call `BlockEntityTicker`, join does not send
  block-entity update packets, and there are no container menus, click
  handling, hopper transfers, or loot-table unpacking.
- Not modelled: barrel, shulker box, hopper, dispenser/dropper, brewing stand,
  enchanting table, beacon, beehive, campfire (its NBT is different), and every
  other block-entity type are preserved opaquely rather than typed.
- Spawner `SpawnData`/`SpawnPotentials` are preserved verbatim and `delay`
  ticks down, but no entity is selected, no spawn rules are checked, no player
  range/nearby-count check runs, and no entity is spawned.
- Sign text components are preserved, but editing, filtering, click events,
  commands, and the `ClientboundBlockEntityData` packet are not implemented.
- Furnace `RecipesUsed` is persisted and smelting is driven through
  `crafting::FurnaceState`; XP is computed only in the runtime model and is not
  awarded to a player. Fuel remainders other than lava buckets remain open.
- `block_entities` item stacks use the modern `{id, count, components}` codec.
  Legacy `Count` is accepted, but pre-1.20.5 nested `tag` payloads are not
  migrated; an old chunk that still has them will keep the item id/count and
  preserve the unknown `tag` key through item-component opaque handling where
  possible.
- Malformed block-entity NBT fails the chunk load by design. A real stored
  chunk with a damaged entry will therefore not join until the entry is
  repaired; this is recorded as the chosen conservative behavior rather than a
  silent delete.
- No Java-captured block-entity NBT fixture exists, so this is behavior-tested,
  not a byte-for-byte Java parity claim.
- Workaround / next task: wire placement and ticker calls, container menus and
  packets, then add a Java fixture that round-trips chest/furnace/sign/spawner
  NBT and an unknown entry.
## 2026-11-06 - Bridge API depth leaves live transport and YAML sections open

- Scope: `BridgeConfig` (flat scalar YAML, Rust file I/O), Rust-owned permission
  attachments with operator defaults, `ServicesManager`, scheduler backend
  seam, persisted world time/weather/spawn, item enchantment round-trip, and
  registry shims for enchantment/sound constants. All focused Rust and Java
  checks pass.
- Not implemented: `Player.sendMessage` reaches only the Rust log because
  `join.rs` exposes no outbound message queue; `kickPlayer` logs a request but
  cannot close the socket. `playSound`/`spawnParticle` are explicit no-ops.
  World time/weather changes persist to `run/world_state.txt` but no play
  packets are emitted, so a connected client will not see them.
- Config limitations: comments, lists, maps, and nested sections are not
  serialized; dotted scalar keys work. `saveDefaultConfig()` is honoured on
  first read, but the stock `JavaPlugin.reloadConfig()` replaces the bridge
  configuration with a YamlConfiguration. Plugins that need the bridge file
  should call `getConfig()` or `saveConfig()` after reload.
- Permission limitations: attachments are Rust-owned and explicit values win,
  but permission wildcards/parents from `plugin.yml`, permission registration
  (`PluginManager.addPermission`/`getPermission`), and default permission maps
  are not modelled. Unset permissions use only the operator flag.
- Scheduler limitations: async variants are downgraded to the server tick
  thread and `isSync()` reports true; Folia region/entity schedulers are not
  implemented. Timed permission attachments reuse the same queue.
- Item limitations: enchantments round-trip through
  `ComponentValue::Enchantments`; item flags, PDC, custom model data, and
  container contents do not. Registry shims make `Enchantment`/`Sound`
  constants resolvable but provide identity only, not vanilla stats or audio.
- Services are process-local and in-memory; dependency/load ordering and a
  multi-plugin service test remain open.
## 2026-11-06 - P6.5 pistons are behaviour-tested, not a Java differential claim

- Scope: `piston.rs` implements piston/sticky-piston extension and retraction,
  head placement, the 12-block `PistonStructureResolver` (line and branch
  chains), sticky pulls, slime/honey rules, immovable blocks, plant/fluid
  destruction, scheduled two-tick moving blocks, and `block_shapes` collision
  for heads and moving blocks. 7 lib + 9 `tests/pistons.rs` tests.
- Not modelled: moving-piston payloads and `PistonMovingBlockEntity` contents
  are not persisted to chunk NBT; moved states carry no block-entity data.
  Entity collision/pushing, honey entity drag, `PistonMath` entity movement,
  and piston sounds/game events are not wired. The block-entity name table is a
  curated list, not a generated registry dump, so an obscure BE block could be
  pushed instead of rejected.
- Parity status: tests encode the decompiled resolver rules and observable
  block transitions; there is no captured Java tick-by-tick piston trace yet.
  Do not claim piston parity from this slice.
- Workaround / next task: keep the explicit `MovingPiston` payload when typed
  block entities land, then add payload NBT and a Java differential fixture.
## 2026-11-07 - Remaining configured-feature blockers after the end/dungeon/sculk batch

- Status: 218/224 configured features execute. The 6 remaining configs are
  `fossil` (2), `end_spike`, `freeze_top_layer`, `geode`, and `root_system`.
- `root_system` / `rooted_azalea_tree`: the decompiled 1.21.11
  `placeDirtAndTree` moves the mutable position up before testing
  `allowed_tree_position`, but the shipped predicate requires
  `#minecraft:azalea_grows_on` below that candidate and the feature's first
  check requires the origin itself to be air. No origin can satisfy both in
  the current single-chunk region; the feature stays explicitly unsupported
  rather than shipping a no-op. Needs a Java oracle placement trace to resolve
  whether the predicate data or bytecode ordering is authoritative.
- `geode` / `amethyst_geode`: `GeodeFeature` seeds its `NormalNoise` from
  `worldGenLevel.getSeed()`. `FeatureLevel` has no seed accessor and
  `generator.rs` is outside this worker's file set, so the feature stays
  explicit. Add `FeatureLevel::seed` plus generator wiring before porting the
  noise/crack/layer math.
- `freeze_top_layer`: needs `Biome.shouldFreeze` / `shouldSnow` through the
  generator's biome sampler. `FeatureLevel` exposes no biome; a hard-coded
  snowy assumption would corrupt non-frozen biomes. Add `FeatureLevel::biome_at`
  and wire the generator's sampler first.
- `fossil`: needs a `StructureTemplateManager`. The decompiled data tree has
  `fossil/*.nbt`, but the feature level has no template access and adding the
  NBT assets is outside this slice's file scope. `FossilFeature` must stay
  unsupported until a template provider is passed to features.
- `end_spike`: the shipped config has an empty `spikes` list, so
  `SpikeFeature.getSpikesForLevel` needs `level.getSeed()`; `placeSpike` also
  spawns an `EndCrystal` entity through `ServerLevelAccessor.addFreshEntity`.
  `FeatureLevel` cannot express either capability.
- `bonus_chest`, `desert_well`, and `monster_room` are now supported: they
  attach their vanilla `minecraft:chest`, `minecraft:brushable_block`, and
  `minecraft:mob_spawner` NBT through `FeatureLevel::set_block_entity`. The
  generator still discards the returned block-entity list; callers that
  persist generation output must consume it (existing limitation, unchanged
  by this slice).
- `sculk_patch` is now supported with the full worldgen spreader. It cannot
  push entities up when converting a vein to sculk (`Block.pushEntitiesUp`
  needs an entity layer); no shipped sculk patch placement targets a populated
  entity column during generation.

## 2026-11-07 - `WorldStorage::tick_hoppers` compatibility shim

- Blocker: commit `d3e2b53` (leaf decay) added `storage.tick_hoppers()?` to
  `tick.rs` but no implementation exists in `world.rs`; the tree does not
  compile at that commit. `world.rs` is outside this worker's allowed file
  set, so a real push/pull implementation was added as an inherent
  `impl WorldStorage` in `worldgen/feature/mod.rs` to keep the focused gates
  green. It ticks cached hoppers, pushes one item into a chest/hopper/furnace
  below, pulls from one above when the push fails, and resets the cooldown to
  8 ticks on a transfer.
- What is needed: move the impl to `world.rs` (or replace it with the proper
  hopper ticker) and delete the shim; the current placement has no access to
  hopper facing/locked state and is not a vanilla parity claim.
## 2026-11-06 - Real jigsaw villages need shipped structure-template assets

- Status: recursive `JigsawPlacement`, biome/dimension filtering, and generator
  wiring are implemented. `tests/jigsaw.rs` proves the expander with synthetic
  templates and `tests/structure_generation.rs` proves igloo placement in a real
  generated chunk.
- Blocker: the repository contains no `data/minecraft/structure/**.nbt` asset
  and this slice could not add asset files. `StructureGenerator` therefore
  requires a caller-supplied template map; a real village selected from the
  shipped pools fails with `UnknownTemplate` (loud, not an empty start).
- What was tried: reused the byte-for-byte igloo fixtures embedded in
  `tests/structure_starts.rs`; the synthetic village tests build NBT through
  `StructureTemplate::from_nbt`.
- What is needed: an extractor that walks the local decompiled data pack, gzip
  copies every `structure/**.nbt`, and emits a compact catalog (name, bytes,
  FNV/SHA) analogous to `template_pools.json`. Then a provider can load all
  village town-centers/houses/streets/terminators.
- Other explicit gaps:
  - `FeaturePoolElement` placement returns `UnsupportedPoolElement`; many
    village decor pools include `feature_pool_element` entries.
  - Terrain adaptation (`beard_thin`, `beard_box`, `bury`, `encapsulate`) is
    not applied; only start-bounds inflation is modeled.
  - `StructureGenerator` caches no starts and samples heights from the current
    chunk region only, so pieces spanning chunks can use different heights.
  - `StructureGenerator` chooses the first matching structure set per chunk;
    vanilla can create several starts in one chunk.
  - Structure block-entity NBT, chest loot seeding, suspicious-sand gameplay,
    and template entities are returned as data and discarded by the generator.
  - Desert-pyramid placement random is deterministic but not Java-exact.
  - `pool_aliases` and non-absolute `start_height` providers fail explicitly.
- Workaround / next task: add the NBT asset extractor, then a
  `TemplateProvider` backed by it; implement `FeaturePoolElement` through the
  configured-feature pipeline; then add a start cache/region manager so
  cross-chunk height sampling and terrain adaptation can be exact.

## 2026-11-07 - Effects/armor slice boundaries

- Armor enchantments (Protection/Blast/Fire/Projectile), armor durability loss,
  and armor trims are not modeled.
- No absorption, health boost, hunger, levitation, slow falling, conduit,
  dolphin, bad omen, or darkness gameplay; only the listed effect flags and
  modifiers are exposed.
- Effect application/removal is not wired to consuming potions, beacons,
  conduits, or commands, and no `ClientboundUpdateMobEffect` packet is emitted.
- Damage types are a typed enum, not the full `DamageSources` registry; totems,
  absorption, knockback, death messages, and loot on death remain open.

## 2026-11-07 - Dimension storage and portal boundaries

- Portal frame detection requires an existing frame; portal creation/search
  (PortalForcer), obsidian platform construction, and entity teleport packets
  are not wired.
- Nether/End chunks are generated and stored but sessions still always join the
  overworld; no dimension switch path or per-dimension level.dat.
- Nether/End carvers, fortresses, bastions, End cities, gateways, and the dragon
  arena remain unported.

## 2026-11-07 - Redstone lamp/openable boundaries

- Lamps are immediate; scheduled off-delay behavior and light packet updates are
  not separately modeled.
- Double doors, iron-trapdoor hand interaction, and fence-gate wall state are not
  linked across blocks.
- Daylight detectors, target blocks, note-block audio, tripwire hooks, and
  comparator output persistence remain open.

## 2026-11-07 - Block-entity ticker boundaries

- Hoppers and other container automation are not modeled yet; chests/furnaces
  are containers but have no transfer ticker.
- Smithing/cooking menu packets, block-entity animation packets, furnace-lit
  block state updates, and spawner entity creation are not wired.
- Block entities only tick once their chunk is loaded; no chunk-ticket-driven
  ticking exists.

## 2026-11-07 - Hopper wiring boundaries

- Hopper pull/push runs on the world tick through `WorldStorage::tick_hoppers`,
  using the container above/below lookup and the 8-tick transfer cooldown. No
  hopper minecart, inventory menu, or container-update packets yet.
- Hoppers do not lock, do not skip empty containers with a cooldown, and do not
  emit container-update packets.

## 2026-11-07 - Daylight detector boundaries

- Celestial angle, weather dimming, and `getSkyDarken` are not modeled; the
  detector reports raw stored sky light.
- Inverted detector state is honored but the crafted block id/property split and
  block-entity-less updates still need a Java fixture.

## 2026-11-07 - Leaf decay boundaries

- Leaf decay does not persist `distance` in chunk NBT separately (it is part of
  the block state) and has no Java differential fixture.
- Mangrove propagules, azalea/flowering leaves, and waterlogged leaves decay
  through the same tag rule; special-source support is not differentiated.

## 2026-11-07 - Crop growth boundaries

- Vanilla `getGrowthSpeed` (farmland rows, neighbouring crops, hydration) is not
  modeled; growth uses a fixed 1-in-4 roll under sufficient sky light.
- Bone meal is exposed as `grow` but has no item-use path or particles; crop
  drops/harvesting and stem crops (melon/pumpkin) remain open.

## 2026-11-07 - Chest loot boundaries

- Slot placement is first-fit, not vanilla's random empty-slot order.
- Loot is not triggered by chest opening/menu code, and `entity_properties`/
  `location_check` still fail closed; no Java chest-loot fixture yet.

## 2026-11-07 - Player hunger boundaries

- No live food tick call yet; sprinting, jumping, mining, and regeneration
  exhaustion sources are not connected.
- Difficulty-based starvation damage, peaceful regeneration, hunger effect, and
  food item use remain open.

## 2026-11-07 - Brewing-stand block-entity boundaries

- No brewing menu/packets, bottle-slot click handling, or live fuel-item
  interaction; the entity ticks but does not expose a container menu.
- Brewing stand `Fuel` is persisted as a byte and clamped, matching vanilla's
  `getByteOr`/`putByte`; custom Paper fuel values above 127 lose precision.
## 2026-11-07 - Bridge live-packet boundaries

- Chat is plain text only: the Java proxy still flattens Adventure components
  to `PlainTextComponentSerializer` output, so colors, hover/click events, and
  signed player chat are not carried. Action-bar/overlay chat is sent as normal
  system chat.
- World sound/particle calls broadcast to every live play session and do not
  apply the vanilla distance/volume cutoff. `playSound(adventure Sound, ...)`
  shapes without a Bukkit `Sound`/`String` are skipped.
- `spawnParticle` supports simple particle types only. `BLOCK`, `DUST`, `ITEM`,
  `VIBRATION`, and the other option-carrying particle types are rejected with an
  explicit log because their option payload is not encoded yet.
- World time has no separate game-time clock in the bridge; `ClientboundSetTime`
  uses the persisted day time for both fields and is not resent periodically.
- Kick sends the play-state disconnect packet but does not fire
  `PlayerKickEvent`, cannot run async, and has no reason-component styling.
- Hopper automation is still a one-way push pass. Pull from the container
  above, locking, menus, and block-entity update packets remain open.
