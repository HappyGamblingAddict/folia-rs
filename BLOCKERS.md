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
  powder/anvils, plant support, and grass spread.
- Not implemented (needs P1.5 block metadata or a bigger tick model):
  leaf `distance` propagation and decay, crop/sapling growth, torch/rail/door/
  fence/piston/redstone neighbour reactions, fluid ticks and fluid flow, and
  block-entity ticks.
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
- No Java differential neighbor-update or scheduled-tick fixture exists yet,
  so this is a behavior-tested slice, not a parity claim.

## Known limitations (not hard blockers)

- `tests/assets.rs` is red on this branch before the ore work: commit `2ca3a1c`
  regenerated `crates/folia-core/assets/block_states.tsv` (now 2,625,357 bytes)
  without updating the manifest (still 2,595,686). The asset file and test are
  outside the ore slice's file scope; regenerate the manifest/hash in the same
  change that owns the per-state block-behaviour data.

These are expected gaps, not reasons to stop:

- Lighting is a first slice, not full vanilla light-engine parity. `light.rs`
  recomputes a whole chunk (plus a one-block neighbour margin) with an explicit
  hand-written opacity/emission table. Block behaviour metadata (P1.5) is still
  missing, so unknown blocks are opaque and unsupported emitters return 0.
  Incremental decrease/increase section bookkeeping, queued cross-chunk
  relighting, generation-time lighting, and a Java light-array/packet fixture
  remain open. `magma_block` deliberately emits 3 (vanilla) even though the
  task brief listed it as 15.
- `BiomeManager` has no Java fixture. Its seed obfuscation is pinned to NIST
  SHA-256 vectors and its quart selection to values derived from the Java
  source; a Java `getBiome` dump would lock it down.
- No Java fixture records placement positions, so P2.2's done-when is still
  open even though the pipeline runs.
- Tree fixtures are deterministic but not a Java block-for-block parity claim.
  `BlockStateProvider::simple` still samples through the weighted path and
  consumes a random draw that Java's `SimpleStateProvider` does not; exact
  tree RNG alignment needs a separate provider-representation fix.
- Structure templates load and place blocks, but structure starts, pieces,
  structure sets, references, processor lists, template pools, and jigsaw
  expansion are unported. There is no structure generation or Java
  block-for-block template fixture yet. Random multi-palette selection,
  entity spawning, waterlogging, shape updates, and per-block state
  derivatives (`shape`, `hinge`, `type`, …) are also open.
- Item components are a first slice: per-item prototype defaults
  (`max_stack_size`, `max_damage`, food/tool/enchantment defaults) are not
  dumped, so a plain damageable item without a damage/max_damage component
  still reports stack size 64. `attribute_modifiers`,
  `potion_contents`, and `tool` are NBT-only; their network codecs are omitted
  and reported by `NetworkEncodingReport`. `HashedStack` component hashes are
  parsed but not matched against a reconstructed stack, and there is no
  Java-captured component NBT/packet fixture yet.

- Some Java oracle data requires starting `java-worldgen`; if the server cannot
  start, use the existing dumps in `crates/folia-core/assets`.
- Plugin bridge work may require Java 21; native-only work does not.
- Structure NBT templates are available in the decompiled data tree; structure
  loot tables and other structure data may still need extraction from the local
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

## 2026-11-06 — P4.1/P4.2 special movement and block shapes remain unported

- Blocker: Full vanilla player movement needs block behavior metadata (shape,
  friction, jump factor, climbable, liquid, powder snow) that P1.5 has not
  produced, plus player-input packets and a standalone player tick scheduler.
- What was tried: Implemented walking/falling/jumping/step-up/fall damage and
  packet reconciliation against full-cube collision. Non-full shapes are
  approximated as full cubes; replaceable/plant tags are passable.
- What is needed: P1.5 block-shape/friction metadata, `ServerboundPlayerInput`/
  `ServerboundPlayerCommand` parsing, water/lava fluid state and movement,
  ladder/vine/scaffolding/cobweb/slime/elytra/powder-snow handling, and a
  per-region player tick.
- Workaround: Keep the unsupported paths explicit in `DECISIONS.md`; do not
  claim ladder, swimming, or elytra parity. The ten-block teleport net plus
  collider replay still rejects the common wall/teleport cases.
## 2026-11-06 — P2.8 structure template is loading/placement only

- Scope: `worldgen/structure/template.rs` loads gzip structure NBT, validates
  palettes/blocks/entities, transforms positions with mirror/rotation/pivot,
  and writes blocks through `FeatureLevel`. `place_into` returns block-entity
  NBT and entity descriptors rather than creating them.
- Not ported: structure starts, pieces, structure sets, references, biome
  checks, processor lists, template pools, jigsaw placement/junction expansion,
  terrain adaptation, random multi-palette selection, entity spawning,
  waterlogging, block-shape updates, and per-block state derivatives that need
  P1.5 block behaviour (stairs `shape`, door `hinge`, rail `shape`, …).
- Why it is not a blocker: This is a deliberately scoped foundation slice. The
  next P2.8 task is the start/piece/reference pipeline; template loading can be
  tested and used independently of it.
- Workaround: Call `StructureTemplate::placement_blocks`/`place_into` directly
  from focused code and tests. Do not claim structure generation parity until
  starts/references/pieces and a Java placement fixture exist.

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
