# Folia-RS Agent State

> This file is the short-lived working memory for the next agent session.
> Update it after every completed task. Keep it short and factual.
> `TODO.md` is the full roadmap. `AGENTS.md` is the operating manual.

Last updated: 2026-11-06

## Current status

- Current commit (informational only): `6e0e5fe`
- Working tree: aquatic/vine/mushroom slice merged; docs being resolved.
- Tests: **426 passing** on the last full gate; the block-breaking slice adds
  13 lib + 9 `tests/block_breaking.rs` tests (full gate rerun by the
  coordinator).
- P4.3 block breaking is server-validated: `block_breaking.rs` ports the
  start/stop/abort dig machine, the 0.7 stop threshold and delayed destroy,
  `getDestroyProgress`/`getDestroySpeed`, tool tiers/harvest gating from the
  block tags, sword/shears rules, efficiency/haste/fatigue and water/air
  penalties, durability/item break, and a deterministic common-block drop
  table with a `LootProvider` hook. `join.rs` tracks dig state per session,
  breaks instantly in creative, applies drops to the inventory, and resyncs
  mismatched targets with the existing block-update packet. Real loot tables,
  item entities, block-damage overlay packets, Unbreaking/Fortune/Silk Touch,
  adventure `can_destroy`, and a Java break-time fixture remain open.
- P1.4 focused gate: 25 item-component tests passing (8 unit + 17
  integration), plus inventory, player, and the captured-world join test.
- Formatting: `cargo fmt --check` clean.
- Build warnings: none.
- Clippy: `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Notable delta: the aquatic slice moves exact-name parity from 92.04% to
  92.02% (5654/6144) while block-class stays 98.71% (6065/6144). The one block
  is a tall-seagrass pair on a column whose Rust terrain already differs from
  Java (Rust water at `y=47`, Java gravel); the feature is faithful to the
  generated world, not a Java block-position match yet.
- P0.1–P0.4 and P0.6 are complete. P0.5 is deferred (see `BLOCKERS.md`).
- P1.3 core tag registries are ported: `scripts/extract_tags.ps1` flattens
  block, item, entity type, fluid, game event, damage type, enchantment,
  point-of-interest type, and worldgen tags into
  `crates/folia-core/assets/tags/*.tsv` (602 non-empty tags across 604 files,
  7,208 members).
  `folia_core::tags` embeds them with `include_str!` and normalizes bare keys
  to `minecraft:`. Cosmetic registries (`banner_pattern`, `dialog`,
  `instrument`, `painting_variant`, `timeline`) and replacing the ore
  feature's hard-coded target-tag sets are still open.
- Feature generation is now wired into chunk generation. The vegetation slice
  places oak and birch trees (straight trunk + blob foliage, driven by the
  vanilla placed features and `random_selector`) in new forest, plains, and
  birch-forest chunks. Straight-trunk spruce/pine use layered foliage, dark
  oak uses its 2x2 branching trunk, forking acacia uses acacia foliage, fancy
  oak uses its custom limb trunk and clipped height, cherry uses its forked
  branches, giant mega spruce/pine use a 2x2 trunk and crown foliage, mega
  jungle adds its branch walk and jungle foliage, `jungle_bush` uses the
  straight trunk with bush foliage, azalea uses a bending trunk and
  random-spread weighted foliage, mangrove uses an upwards-branching trunk and
  waterlogged/muddy roots, fallen trees place a stump plus sideways logs, and
  `jungle_tree`/`swamp_oak` parse and place their vine/cocoa decoration.
  `place_on_ground` leaf litter,
  `beehive`, `leave_vine`, `trunk_vine`, `cocoa`, `attached_to_leaves`,
  `attached_to_logs`, and `creaking_heart` decorators execute; unsupported
  decorator types make the whole tree config unsupported. All 888 shipped
  placement modifiers execute; the remaining P2.2 gap is the Java position
  fixture. The P2.5 aquatic/vine/mushroom batch now places seagrass (short
  and tall), kelp, sea pickles, all three coral shapes, bamboo, vines,
  twisting and weeping vines, huge brown/red mushrooms, huge crimson/warped
  fungus with shroomlight and hanging vines, glowstone blobs, forest rocks,
  fill layers, nether forest vegetation, and underwater magma. 162 of 224
  configured features dispatch as supported. Beehive occupants and the
  chunk-status pipeline are still open; pale moss is blocked on wiring its
  now-supported `pale_moss_patch`.
- P2.1/P2.2 core is in `worldgen/feature/{mod,placement}.rs`: framework,
  typed loading, and a placement pipeline that executes all 888 shipped
  modifiers (world-free transforms, `biome`, every heightmap, block
  predicates, environment scans, layer/noise counts, surface-relative
  filters, water depth, sapling survival, and Gaussian offsets). `biome` reads
  world state through `PlacementWorld`; `BiomePlacementWorld` handles
  pre-generation queries and `FeaturePlacementWorld` handles the chunk being
  generated. The remaining 126 modifiers parse as explicit unsupported
  variants. Generation wiring is now complete for the supported subset.
- Native server: builds and runs; optional JVM only starts when plugin jars
  exist.
- Worldgen parity: 98.71% block-class / 92.02% exact-name on the 16-column
  fixture after the aquatic slice (was 92.04%).
- Lighting now has real 2048-byte nibble layers per section. `light.rs` ports
  `DataLayer` storage, per-column sky sources with a six-direction BFS, and
  block-light BFS for an explicit emitter table. Chunk NBT round-trips
  `SkyLight`/`BlockLight`; `light_bytes` prefers stored/computed layers and
  keeps the height fallback only for chunks without light. Block edits reload
  existing neighbours, recompute both layers, persist them, and broadcast real
  block-light updates. The emitter/opacity table is still hand-written, there
  is no Java light fixture, and light updates are whole-chunk rather than
  incremental sections.
- Joins stream a square chunk column sized by the negotiated view distance
  (the probe client asks for 6, so 13×13; the shipped server allows 10 and a
  default client asks for 12, so a normal join streams 441 columns) and
  re-stream new columns as
  the player crosses chunk borders. The first join of a world without
  `level.dat` writes one (DataVersion 4788, seed, `spawn.pos`, overworld
  generator settings) marked `folia:written`, so the level is a real saved
  world and later starts reuse its seed and spawn. Generated terrain is
  written back to the region files with a `folia:written` stamp, so
  exploration and block edits survive restarts. A level this server created
  serves only stamped chunks (leftover flat chunks are regenerated); a foreign
  `level.dat` or `FOLIA_LOAD_STORED=1` serves every stored chunk.
- A player whose feet are inside a solid block on login (the captured spawn is
  `y = -60`, four blocks above bedrock) is lifted to the first empty block of
  their column, so no one spawns buried in stone.
- Columns more than one chunk outside the live radius are unloaded on each
  streaming batch, so a session's chunk set stays bounded while walking. The
  radius is the smaller of `server.properties` `view-distance` and the client's
  `ClientInformation` request, clamped to 2–16 chunks. No tickets, spawn
  chunks, or cross-player dedupe yet, so each session still streams its own
  copy.
- Missing columns in a streaming batch are generated on up to eight worker
  threads; each column is sent as soon as it is ready, nearest-first, with
  stored columns taking their place in the same order and generated ones saved
  just before sending. A cold 169-column join on a fresh level measured 7.7 s
  versus 16.3 s with sequential generation; after palette-cached packet
  encoding and worker-side packet building it measured 1.9 s (release, two runs
  at 1.89-1.94 s).
- `level.dat` records elapsed `Time`/`DayTime` and `LastPlayed` on shutdown
  (`level::record_session`), rewriting the root atomically so fields this port
  does not model survive.
- Encoded chunk NBT carries `DataVersion` 4788 and `Status: "minecraft:full"`,
  so a stored column describes its own format and completion state.
- Region writes reuse a chunk's existing sectors when the new record fits;
  saving the same chunk again overwrites in place, and an oversized record
  appends a new copy as before.
- Dead sectors are tracked; once they exceed a quarter of the live allocation,
  `save_if_dirty` compacts the region (live records packed after the header)
  before writing it.
- `WorldStorage` caches up to 256 decoded chunks with FIFO eviction; `load_chunk`
  serves repeats from memory, while `save_chunk`, `update_chunk`, and
  `unload_region` replace or drop the cached copy so edits are never stale.
- Chunk generation uses a bounded 16-column queue; workers stop when the
  session drops the receiver, so a write failure cancels the rest of the batch
  instead of generating chunks nobody will receive.
- A movement packet moving more than ten blocks (squared distance > 100) is
  reverted and answered with a position packet, so a single-packet teleport
  cannot move a player through terrain or across the world.
- Stored chunks carry `Heightmaps` for the three vanilla types, packed at 9 bits
  per column; `stamp_chunk` recomputes them from the chunk so an edit cannot
  leave a stale heightmap in the file.
- A corrupt chunk record (invalid length, truncated payload, unknown
  compression) reads as a missing chunk and is logged; intact neighbours still
  load, so one damaged column cannot fail a join or a later save.
- Block edits save through `WorldStorage::update_chunk`, which copies top-level
  chunk fields the chunk model does not decode (`Status`, `Heightmaps`,
  `BlockEntities`, `Entities`, `structures`, …) from the stored root instead of
  dropping them on rewrite. Freshly generated columns still replace a stored
  column outright, so stale block entities never attach to new terrain. Block
  entities at positions an edit replaces are dropped, the way vanilla removes a
  block entity when its block changes; other stored entities survive.
- Overworld terrain is served from one process-wide `WorldStorage`
  (`world::shared_overworld`, locked with `world::lock`) used by all sessions
  and entity physics: a region save rewrites the whole `.mca`, so independent
  instances used to drop each other's chunk writes. Generated chunks are
  flushed every 5 s and on disconnect; block edits flush immediately.
- Playerdata parses and saves vanilla survival vitals (health, hunger, XP,
  score, air); new players start with vanilla's defaults, and a loaded file's
  values are kept instead of being reset.
- Playerdata is written on disconnect, after movement (throttled), and from the
  1 s read-timeout path, so an idle session also saves every 5 s
  (`PLAYER_SAVE_INTERVAL`) instead of only when the player moves. Saves stamp
  `DataVersion` 4788, so the file is self-describing like vanilla's.
- Shutdown asks sessions to close (`join::shutdown_sessions`, 5 s timeout): a
  session in its play loop saves playerdata, flushes terrain, and returns, so
  main's terrain flush and `level.dat` rewrite happen after players are done.
- On shutdown the server flushes the process-wide overworld storage before
  writing `level.dat`, so terrain still cached in region memory reaches disk on
  an orderly stop; level rewrites also keep the previous file as
  `level.dat_old`.
- The native server reads stdin. Console `help`, `list`, `ops`, `op`, `deop`,
  `gamemode`, and `stop` work; `run/ops.txt` persists operators. In-game
  `/op`/`/deop` and `/gamemode` are operator-gated.
- Void damage starts below `y = -128` (4 health per 500 ms), sends live health
  updates, shows the combat-kill death screen, and respawns at world spawn on
  the client's respawn command (auto-respawn after 2 s as a fallback). General
  fall/fire/hunger damage and inventory drops are still missing.
- Server-side player physics is live: `player_physics.rs` models the
  standing/sneaking/swimming hitboxes, vanilla gravity/drag/friction, jump,
  sprint jump, step height, axis-separated collision against the shared world
  storage, fall distance, and fall damage. Movement packets are reconciled
  instead of accepted verbatim: deltas up to `0.75` blocks are replayed through
  the collider, a reproducible claim is accepted exactly, and a blocked claim
  sends a position correction. Longer packets are lag bursts that validate
  their destination. Fall damage sends health updates and death; landing resets
  fall distance. Non-full block shapes, ladders, cobwebs, slime, elytra, water/
  lava currents, swimming, bubbles, powder snow, and sprint/sneak input packets
  are not ported yet.

- Structure templates have a first P2.8 slice in
  `worldgen/structure/template.rs`: gzip structure NBT parsing (DataVersion,
  size, single or multi palettes, blocks, block-entity NBT, entities), Java
  `StructureTemplate.transform`/`transform(Vec3)` coordinate math with mirror,
  four rotations, rotation pivot and origin, and block placement through
  `FeatureLevel`. `place_into` returns written/rejected counts, block-entity
  NBT, and transformed entity descriptors; it does not spawn entities.
  Structure starts, pieces, structure sets, references, processors, template
  pools, jigsaw expansion, random palette selection, and exact per-block state
  derivatives (stairs shape, door hinge, …) are still unported.

- Vegetation is partial: supported trees now place layered spruce/pine, dark
  oak/acacia/fancy/cherry foliage, mega-spruce/pine crown foliage, leaf litter,
  beehives, hanging/trunk vines, cocoa, altered podzol ground, and fallen logs
  with mushrooms. Beehive block entities have no stored bees yet. The P2.5
  ground-cover batch parses and places `random_patch`/`flower`, `simple_block`,
  `block_pile`, `block_column`, `multiface_growth`, `vegetation_patch`, and
  `waterlogged_vegetation_patch`; the aquatic/vine/mushroom batch adds
  `seagrass`, `kelp`, `sea_pickle`, coral tree/claw/mushroom, `bamboo`,
  `underwater_magma`, `vines`, `twisting_vines`, `weeping_vines`, huge
  brown/red mushrooms, huge crimson/warped fungus, `glowstone_blob`,
  `forest_rock`, `fill_layer`, `nether_forest_vegetation`, and
  `random_boolean_selector`. 162 of 224 configured features execute. The tree
  `pale_moss` decorator is still unsupported even though its `pale_moss_patch`
  dependency now exists, and kelp/nether vegetation/huge fungus still wait on
  `noise_based_count`/`count_on_every_layer` placement before they appear in
  generated chunks. Structures and most special features remain missing. Ore
  and scattered-ore features are implemented and all 32 shipped configs are
  covered, but generator wiring is still deferred because it lowers the
  exact-name parity fixture until WorldgenRNG parity lands. Existing stored
  chunks are not retroactively decorated, so features appear in newly generated
  columns only.

- P3.10 block simulation has a first vertical slice in `tick.rs`:
  vanilla `UPDATE_*` flags (`BLOCK_UPDATE`, `NEIGHBORS`, `SHAPE`,
  `SUPPRESS_NEIGHBORS`, …), a chunk-grouped neighbour queue with position
  deduplication and flag-union merging, a `ScheduledTickQueue` ordered by
  trigger/priority/sub-tick with delay, dedup, and cancellation, and a
  `RandomTickQueue` sampling 3 positions per 16x16x16 section from a
  deterministic per-chunk seed. `block_update::set_block` queues and drains the
  six neighbours; falling sand/gravel/concrete powder/anvils move one block per
  scheduled tick, plant support uses `dirt`/farmland tags (including double
  plants), and grass spreads to dirt. The server loop calls `tick::tick_world`
  every tick. Leaf decay, crop growth, redstone, fluids, pistons, and NBT tick
  persistence remain explicit gaps (see `BLOCKERS.md`).
- Folia region threading is not implemented beyond task queues.

## Current focus

- P4.3 now validates breaking. Next: port real block loot tables (ranges,
  Fortune/Silk Touch), create `ItemEntity` drops instead of inventory capture,
  send the block-damage overlay packet, model Unbreaking, parse adventure
  `can_destroy`, add status-effect/haste sources, and capture a Java
  break-time/drop fixture.
- P1.4 has a working component-aware stack model. Next: dump per-item prototype
  component defaults (`max_stack_size`, `max_damage`, food/tool/enchantment
  defaults), port the `attribute_modifiers`, `potion_contents`, and `tool`
  network codecs, implement `HashedStack` component hashing, and add a
  Java-captured component NBT/packet fixture.
- P3.10 now has flags/neighbour/scheduled/random tick queues and the server
  tick hook. Next: P1.5 metadata for leaf distance/decay, crop growth, torch
  support, redstone, fluids, and piston behaviour; persist `ScheduledTick`s in
  chunk NBT; replace the deterministic per-chunk random sampler with Java's
  level RNG once feature/level RNG parity lands; add a Java neighbor-update
  differential fixture.
- P4.1/P4.2 now have a live physics slice: finish the movement feature gaps
  (ladders/vines/scaffolding, water/lava/swimming, cobwebs, slime, elytra,
  powder snow, sprint/sneak input packets, real block shapes) and eventually
  move player simulation onto its own 20 Hz tick instead of packet-driven
  reconciliation.
- P1.3 tag data is available and the ore feature already resolves its four
  target tags through `folia_core::tags`; add the remaining cosmetic
  registries.
- P3.10 now has flags/neighbour/scheduled/random tick queues and the server
  tick hook. Next: P1.5 metadata for leaf distance/decay, crop growth,
  torch support, redstone, fluids, and piston behaviour; persist
  `ScheduledTick`s in chunk NBT; replace the deterministic per-chunk random
  sampler with Java's level RNG once parity lands; add a Java fixture.
- P4.1/P4.2 now have a live physics slice: finish the movement feature gaps
  (ladders/vines/scaffolding, water/lava/swimming, cobwebs, slime, elytra,
  powder snow, sprint/sneak input packets, real block shapes) and move
  player simulation onto its own 20 Hz tick.
- The P2.5 aquatic/vine/mushroom features are in place. Next: wire the tree
  `pale_moss` decorator, port `simple_random_selector` for the dripleaf
  pools, add `simple_block.schedule_tick`, and add beehive occupants.
- P2.8 structure starts/pieces are in progress; template loading and
  placement are already tested.

## Next queue

- [x] P0.1 Fix formatting and warnings
- [x] P0.2 Refresh documentation
- [x] P0.3 Track/validate generated assets
- [x] P0.4 Add CI
- [ ] P0.5 Make oracle data reproducible (deferred, see `BLOCKERS.md`)
- [x] P0.6 Decide commit/backup policy
- [ ] P1.1 Complete remaining registry dumps
- [~] P1.3 Port tags (nine core registries done; cosmetic registries and ore-tag wiring remain)
- [~] P1.4 Item component model (patch/NBT/network subset, inventory merge/split, playerdata round-trip; prototype defaults, three network codecs, hashing, and a Java fixture remain)
- [ ] P1.5 Block behavior metadata
- [~] P2.1 Feature framework core (framework + loader done)
- [~] P2.2 Placement modifiers (world-free, `biome`, ocean-floor heightmap, water depth, sapling predicates done)
- [~] P2.3 Ores and ore veins (`minecraft:ore` + `minecraft:scattered_ore` and all 32 shipped configs done; generator wiring and `OreVeinifier` pending feature-RNG parity)
- [~] P2.4 Tree features (oak/birch/jungle/swamp/spruce/pine/dark oak/acacia/fancy/cherry/mega spruce-pine/mega jungle/jungle bush/azalea/mangrove/fallen + litter/beehive/vine/cocoa/alter-ground/attached-to-leaves/attached-to-logs/creaking-heart done; pale moss decorator, beehive occupants, and a Java tree fixture remain)
- [~] P2.5 Vegetation and ground cover (`random_patch`/`flower`, `simple_block`, `block_pile`, `block_column`, `multiface_growth`, `vegetation_patch`, `waterlogged_vegetation_patch`, seagrass, kelp, sea pickle, coral, bamboo, underwater magma, vines, twisting/weeping vines, huge mushrooms/fungi, glowstone, forest rock, fill layer, nether forest vegetation, and the mushroom-island boolean selector done; `schedule_tick`, `simple_random_selector`, kelp/nether placement modifiers, pale moss, and structures remain)
- [~] P2.8 Structure templates (gzip NBT load, palettes/blocks/entities, rotation/mirror/pivot placement; starts, pieces, processors, pools, jigsaw, entity spawning still open)
- [x] P3.5 Real light storage
- [~] P3.6 Sky-light engine (single-chunk + one-block neighbour border)
- [~] P3.7 Block-light engine (hand-written emitter/opacity table)
- [~] P3.8 Incremental light updates (full-chunk recompute/replay; section-only still open)
- [~] P3.10 Neighbor updates, scheduled ticks, random ticks (flags, queues,
  falling blocks, plant support, grass spread, server hook; leaf decay, crops,
  redstone, fluids, pistons, and NBT tick persistence remain)
- [~] P4.1 Player entity and physics (walk/fall/jump/step/fall damage done;
  ladders, water, cobwebs, slime, elytra, and a standalone tick remain)
- [~] P4.2 Movement validation (collision replay, 10-block net, corrections,
  and burst validation done; teleport ack and special movement remain)
- [~] P4.3 Block breaking progress (dig state, tool/tier checks, break times,
  creative instant break, drops, durability done; real loot tables, item
  entities, overlay packets, Unbreaking/Fortune/Silk Touch, adventure
  `can_destroy`, and a Java fixture remain)

## Recently completed
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
  default `heightmap()` and absent sections read as air. 162/224 configured
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
- [~] P3.6 Sky-light engine (single-chunk + one-block neighbour border)
- [~] P3.7 Block-light engine (hand-written emitter/opacity table)
- [~] P3.8 Incremental light updates (full-chunk recompute/replay; section-only still open)
- [~] P3.10 Neighbor updates, scheduled ticks, random ticks (flags, queues,
  falling blocks, plant support, grass spread, server hook; leaf decay, crops,
  redstone, fluids, pistons, and NBT tick persistence remain)
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
