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
  P2.3's ore features are now ported; generator wiring waits on feature-RNG
  parity (see the 2026-11-06 ore blocker below).

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

## 2026-11-06 — P2.3 Ore generator wiring deferred on feature-RNG parity

- Blocker: Wiring the newly ported `minecraft:ore` and
  `minecraft:scattered_ore` features into `underground_ores` lowers the
  established exact-name parity fixture from 92.09% to 89.06%. The ore
  algorithms are ported, but feature positions still use the project's
  approximate per-chunk RNG rather than Java's `WorldgenRandom` sequence.
- What was tried: Ran both ore features directly and through the generator;
  all 30 ore + 2 scattered-ore configs parse and place. The generator run
  produced 344 ore blocks in a forest chunk, while the parity fixture regressed.
- What is needed: Feature-RNG parity (`WorldgenRandom` sequencing) before ore
  placement can be compared block-for-block; `OreVeinifier` also remains open.
- Workaround: Keep ore features executable but do not add `underground_ores` to
  generation yet. Preserve the 92.09% fixture rather than weakening it.

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
