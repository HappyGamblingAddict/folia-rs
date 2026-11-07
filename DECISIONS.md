# Decisions Log

Record non-obvious technical decisions here so future agent sessions do not
re-litigate them. Keep entries short.

Format:

```text
## YYYY-MM-DD Ã¢â‚¬â€ Decision title
- Context:
- Decision:
- Why:
- Consequences:
```

## 2026-11-07 — Region tick loop scopes existing world logic instead of duplicating it
- Context: `region.rs` had region task queues and ownership checks, but the
  server still called `tick::tick_world` once for every cached chunk and no
  region owned its block-level work. Folia ticks a region's tasks and world
  slice together and treats another region's chunks as out of bounds.
- Decision: add `RegionTickDriver` (in `region.rs`) and
  `WorldStorage::tick_region[_with_table]` (in `world.rs`). The driver groups
  cached chunks with `RegionMap`, runs the global queue first, then each
  loaded/task region in sorted `RegionKey` order; each region runs its task
  queue and its scoped world tick. The world tick temporarily hides foreign
  chunks and block entities, moves only that region's due scheduled ticks and
  neighbour updates into the live queues, then calls the existing
  `tick::tick_world_with_table` so random/scheduled ticks, block-entity
  tickers, hoppers, and neighbour processing keep their existing semantics.
  Foreign queues are restored afterwards. Neighbour updates produced for
  another region are consumed as no-ops while its chunks are hidden, so they
  are re-enqueued for the owning region's later tick.
- Decision: `tick_parallel_for_test` is an explicitly test-only mode. It
  dispatches region *task queues* across scoped threads (preserving per-region
  task order and deterministic first-error reporting) but leaves the world
  pass sequential behind the one `WorldStorage` lock. Any ownership error is
  returned as `RegionTickError::Ownership`, never converted into a panic;
  `main.rs` logs it and keeps ticking.
- Why: this wires the real region loop in one slice without copying behaviour
  out of `tick.rs`, and it preserves the existing `tick_world` ordering and
  the shutdown flush. It makes cross-region mutation fail/queue instead of
  silently touching another region.
- Consequences: `WorldStorage`, entity/player state, and chunk tickets remain
  global, so regions are not yet ticked in parallel at the world level. The
  temporary hide/restore pass costs map moves per region tick. Cross-region
  hopper transfers and border edits that load the other region's block entity
  while hidden are dropped rather than transferred; queue/handoff support is
  P10.3/P10.5. `RegionScheduler`'s legacy `tick`/`tick_parallel` APIs are
  unchanged.
## 2026-11-07 — Interpolated density markers have direct and NoiseChunk modes
- Context: `NoiseChunk.wrap` turns every
  `DensityFunctions.Marker.Type.Interpolated` node into a `NoiseInterpolator`:
  the wrapped argument is evaluated on the 4x8x4 cell grid and `Mth.lerp3`
  interpolated per block. `OreVeinifier` receives those wrapped router
  functions, so Java never evaluates `vein_toggle` or the two noise arguments
  inside `vein_ridged` per block. Direct `DensityFunction.compute` (used by the
  `java-density-grid.txt` oracle and the pinned router dump) must stay
  unwrapped, because the oracle captures corner values through the direct API.
- Decision: `density::DensityFunction` gains an `Interpolated { id, origin_y,
  argument }` node and two entry points. `compute` keeps Java's direct
  semantics (marker pass-through); `compute_interpolated` samples the argument
  at the enclosing 4x8x4 cell corners, with the y grid anchored at the noise
  settings `min_y`, and trilinearly interpolates in Java's `Mth.lerp3` argument
  order. Corner values are cached per function id in a thread-local one-cell
  cache. `OreVeinifier::block_at` uses `compute_interpolated` for `vein_toggle`
  and the interpolated arguments inside `vein_ridged`; `vein_gap` stays direct
  because the shipped router does not mark it interpolated.
  `block_at_with(VeinSampling::Direct)` preserves the old per-block path for
  differential tests.
- Why: this is the smallest faithful port of `NoiseChunk` without moving the
  generator's existing 4x8x4 final-density grid, and it keeps the direct router
  oracle tests meaningful (corner values are identical in both modes).
- Consequences: the vein pass evaluates eight corners per cell instead of every
  block, and moves 170 positions in chunk `(0,-1)` relative to the old direct
  path (119 added, 51 removed, 0 type replacements). Exact-name fixture parity
  rises 5886 -> 5891/6144; block-class is unchanged. The remaining ore gaps are
  the 3x3-chunk feature write region, ellipsoid edges, and
  `disk_*`/`underwater_magma` in the `underground_ores` step.
## 2026-11-07 — Enchanting table is a deterministic backend slice

- Context: the shipped client menu uses `EnchantmentMenu`'s two-phase random
  order (costs first, then a reseed per slot), `EnchantmentHelper`'s weighted
  selection with enchantability variance, and `ItemStack.enchant` component
  writes. The slice may not edit `lib.rs`, `inventory.rs`, `join.rs`, or
  `Cargo.toml`.
- Decision: embed the 43-enchantment definition table directly in
  `src/enchanting.rs`, derived from `data/minecraft/enchantment/*.json`
  (DataVersion 4788), and cross-check its ids against the existing
  `assets/registries/enchantment.tsv` at test time. Applicability, exclusive
  sets, and the offer pool read the existing flattened tag assets
  (`enchantable/*`, `exclusive_set/*`, `in_enchanting_table`, `treasure`).
- Decision: declare the module with `#[path = "enchanting.rs"] pub mod
  enchanting;` inside `item_components.rs` because `lib.rs` is outside this
  slice's file scope. The public path is
  `folia_core::item_components::enchanting`; promote it to `pub mod
  enchanting;` in `lib.rs` at merge if desired.
- Decision: offer generation and click application use
  `LegacyRandomSource` in the exact vanilla order: one seed draws the three
  costs, then each slot reseeds to `seed + slot` for the list, the book
  single-entry removal, and the hint. A click re-rolls with `seed + slot` and
  the stored cost, so the applied list is identical to the offer's list.
  Applying `slot + 1` levels and `slot + 1` lapis matches
  `Player.onEnchantmentPerformed` and `ItemStack.consume`; creative bypasses
  the requirements but still debits experience levels (vanilla does), and only
  skips lapis consumption.
- Decision: `apply_enchantments` merges into `minecraft:enchantments` (or
  `minecraft:stored_enchantments` for books), upgrades to the higher level,
  clamps at `max_level`, and rejects a rolled enchantment that conflicts with
  an existing or other rolled enchantment. Vanilla's `ItemStack.enchant`
  clamps only at 255 and does not check existing exclusivity; the stricter
  behavior is intentional for this task and keeps incompatible components
  impossible. Unknown (modded) enchantment names are preserved but skipped in
  the compatibility check.
- Decision: anvil combine/repair, renaming, and repair-cost accumulation are
  an explicit `AnvilError::Unsupported`; no empty result is returned.
- Why: the backend is independently testable without menu packets, and the
  seed order and definition data are the parity-critical parts.
- Consequences: `join.rs`/menu state ids, bookshelf scanning, Paper
  `PrepareItemEnchantEvent`/`EnchantItemEvent`, the client clue display, and a
  Java RNG fixture remain open. Direct `apply_enchantments` calls clamp
  over-maximum levels instead of honoring vanilla's command path.
## 2026-11-07 - Non-village templates ship as per-family gzip streams

- Context: the decompiled tree has 1,202 structure NBT files. The village
  catalog covers 483; the remaining 719 would be a 3.88 MB base64+gzip file if
  each template kept its own payload, over the 2.5 MB asset budget. The largest
  family, `trial_chambers` (191 templates), is ~1.0 MB in that form by itself.
- Decision: `structure_templates_more.json` stores one base64+gzip stream per
  top-level family and an id -> `{blob, offset, length}` index. Every slice is a
  complete gzip-decoded NBT document. All families except `trial_chambers` are
  included: ancient cities, bastions, End cities, fossils, nether fossils,
  igloos, pillager outposts, ruined portals, shipwrecks, trail ruins,
  underwater ruins, woodland mansions, and `empty` (528 templates). Payloads
  are recompressed with Java zlib level 9 to fit the budget (2,358,479 bytes;
  1,729,270 compressed payload bytes).
- Decision: `StructureTemplateCatalog::shipped()` parses the village catalog
  and the indexed catalog and merges them. `len()`, `total_bytes()`, and
  `ids()` keep describing the primary village slice for the existing asset
  pin; `all_len()`, `all_total_bytes()`, and `all_ids()` report the merged
  view. `is_loaded` is a per-thread diagnostic so parallel tests cannot observe
  another thread's lazy load.
- Why: family streams preserve exact NBT payloads while exploiting cross-file
  redundancy that per-template gzip cannot reach. The Java level-9 recompression
  removes ~8% versus .NET's level-6 writer and avoids dropping another family.
- Consequences: the extractor now requires `java` for the second catalog (the
  village catalog stays pure PowerShell). `trial_chambers` remains a documented
  gap; adding it needs either a third asset or a stronger compressor. Bastion
  and End-city templates are data-ready, but the procedural
  `FortressPieces`/`EndCityPieces` ports still need to attach them to starts.

## 2026-11-07 — Chat signing is a verified model with server-issued RSA profile keys
## 2026-11-07 - Status-effect packets use registry ids and one shared tracker

- Context: 1.21.11 sends `ClientboundUpdateMobEffectPacket` (play id 130) and
  `ClientboundRemoveMobEffectPacket` (id 76) with a `MobEffect` holder registry
  id, not a name. Entity visibility was still split between a global broadcast
  map and `SessionEntityTracker`'s private vector.
- Decision: pin both packet ids to the decompiled `GameProtocols
  .CLIENTBOUND_TEMPLATE` order (`CLIENTBOUND_BUNDLE` is id 0) and pin effect
  ids 0-39 to the `MobEffects` static registration order. Effect decoders
  return an explicit error for unknown ids instead of guessing. The update
  body is entity id, effect registry id, amplifier, duration, then the
  ambient/visible/show-icon/blend flag byte.
- Decision: `LivingEntity` owns its `StatusEffectSet`. `apply_effect` merges
  by vanilla `MobEffectInstance.update` rules, `replace_effect` is the
  force-add path, instant health/damage applies once through the health
  metadata pipeline, and `tick` emits remove packets on expiry. Poison, wither,
  and regeneration damage/heal through `LivingEntity` so `DATA_HEALTH` stays
  synchronized.
- Decision: `entity::EntityTracker` stores each `TrackedEntity` once in a
  `BTreeMap<i32, _>`. Viewers hold only an ascending id `BTreeSet`, last-synced
  position snapshots, and pending removals. `update_viewer` emits batched
  removals, add+metadata+equipment+active-effect pairing, then position syncs,
  all in ascending id order; `update_all_viewers` iterates viewers ascending.
  `SessionEntityTracker` embeds this tracker as its only entity storage and
  exposes `shared`/`shared_mut`.
- Why: keeps packet bytes and ordering deterministic without a second entity
  list, and makes range sets testable before `join.rs` switches from global
  broadcasts to viewer ids.
- Consequences: effect particles/ambience metadata, hidden effects, per-type
  tracking ranges, and live `join.rs` viewer wiring remain open. Entity
  equipment is still sent as a full eight-slot snapshot on enter.

## 2026-11-07 - Chat signing is a verified model with server-issued RSA profile keys
## 2026-11-07 â€” Chat signing is a verified model with server-issued RSA profile keys
- Context: 1.21.11 clients sign chat with a profile private key and send a
  `SignedMessageBody` (content, timestamp, salt, last-seen signatures) plus a
  256-byte `MessageSignature`. The task also requires a SHA-256 content hash,
  sequence tracking, and server-issued profile keys in Login Success.
- Decision: model `SignedMessageBody` exactly as `SignedMessageBody.Packed`
  (UTF-8 string, epoch-millisecond `i64`, salt `i64`, VarInt-counted packed
  signatures with full/`MessageSignatureCache` variants). `SignedMessage` adds
  a local `content_hash` field (not on the vanilla wire) and signs exactly
  `PlayerChatMessage.updateSignature`: marker `1`, sender/session UUIDs, link
  index, salt, epoch **seconds**, content length/bytes, then the last-seen list.
  Sequences must be contiguous; timestamps must be nondecreasing, no older than
  five minutes, and no more than two minutes in the future.
- Decision: production profile keys are fresh 2048-bit RSA keys per login and
  use `SHA256withRSA`; a 256-byte message signature is mandatory, so a smaller
  RSA key returns `InsecureAlgorithm` rather than emitting non-wire-compatible
  bytes. The public key is published in the Login Success GameProfile property
  `profile_key` as
  `v1:<algorithm>:<expires_ms>:<key_hex>:<key_signature_hex>`. The server signs
  its own key record (self-signed), and `RemoteChatSession` can also verify a
  peer key against a supplied issuer key.
- Decision: provide an explicitly named `DeterministicTestKey` / `provision_test_chat_signing`
  for tests and offline captures. It produces deterministic 256-byte
  signatures but `enforces-secure-chat` rejects it with
  `InsecureAlgorithm`; production provisioning only uses `generate_secure`.
- Why: Keeps the wire body byte-compatible with the decompiled Java, makes
  tampering and replay testable without a live Mojang key service, and fails
  loudly when only an insecure signer is available.
- Consequences: 2048-bit generation takes about 1.4 s in a release build and
  ~49 s in an unoptimized debug build on this machine, so login wiring should
  generate off the tick thread or cache a startup key. `join.rs` and `status.rs`
  must call the provisioning/audit APIs before secure chat is user-visible;
  see `BLOCKERS.md`.
## 2026-11-07 â€” Nether carvers run after the surface pass
- Context: vanilla applies carvers before `SurfaceSystem.buildSurface`, but the
  existing Nether generator materializes surfaces from the pre-carve density
  column map. Moving the pass would either need a second full column scan or
  would place surface blocks into freshly carved air.
- Decision: keep the density fill + surface pass, then replay `nether_cave`
  carvers over the finished column. The carver replaces the
  `#minecraft:nether_carver_replaceables` blocks it hits, writes lava through
  y=31 and cave air above, and never touches the protected top seven layers.
- Why: deterministic, bounded, and testable without reworking the surface
  system; it is the smallest honest slice until the surface system can consume
  post-carve columns.
- Consequences: cave openings do not receive a second surface-rule pass, so
  netherrack/nylium walls stay exposed at cave mouths. A Java block-for-block
  carver fixture must decide whether the ordering delta is visible.

## 2026-11-07 â€” Nether structure selection succeeds; start generation fails loudly
- Context: `nether_complexes` is a shipped structure set, but the project has
  no fortress piece implementation and no bastion template/pool NBT assets.
- Decision: `NetherGenerator::structure_at_chunk` exposes the merged
  random-spread + weighted-selection + dimension/biome-filter result.
  `structure_start` parses the shipped fortress/bastion definition and then
  returns `UnsupportedStructureType` for fortresses or `UnknownTemplate` with
  the exact missing template id for bastions.
- Why: callers and tests can exercise placement selection now, while the
  failure path is explicit instead of an empty start or a fake block layout.
- Consequences: the generator does not write fortress/bastion terrain until
  the template assets and fortress piece port land.

## 2026-11-07 â€” End content is data-shaped and feature dispatch is id-keyed
- Context: `FeatureData` still classifies `chorus_plant`, `end_gateway`,
  `end_platform`, and `end_spike` as unsupported configured features, and End
  cities/dragon fight state are not implemented.
- Decision: `worldgen/end.rs` exposes `EndSpike`, `DragonArena`,
  `EndGateway`/`EndGatewayBlockEntity`, platform/spike/gateway construction,
  chorus growth, and `place_configured_feature`. The dispatch implements
  chorus, gateway variants, and the platform; everything else returns
  `EndFeatureError::Unsupported`. Spike values pin the vanilla seed-derived
  ring and the arena bounds are explicit constants.
- Why: block construction and growth are testable without the missing entity
  fight or shared feature registry support, and unsupported ids cannot be
  mistaken for a successful placement.
- Consequences: End cities, natural gateway placement, crystal entities, and
  the dragon fight still need separate slices; a future registry wiring should
  call this dispatch rather than duplicate the algorithms.
## 2026-11-07 â€” Structure-template asset is village-scoped and lazy
- Context: the decompiled `data/minecraft/structure` tree holds 1,202 gzip NBT
  files (~3.7 MB, ~40.7 MB raw NBT). Per-entry gzip+base64 across the whole tree
  is ~4.9 MB, over the 2.5 MB asset budget, and a solid gzip still exceeds it
  after base64.
- Decision: embed every `minecraft:village/**` template (483 files, 776,539
  gzip bytes, 1,063,517-byte JSON) as base64-of-gzip NBT, keyed by normalized
  structure id. `StructureTemplateCatalog` keeps payloads encoded and parses +
  caches each template on first `template()` call; `StructureGenerator` stores
  the catalog behind an `Arc` and exposes `from_catalog`/`shipped_catalog`.
  Non-village families are an explicit later asset expansion.
- Why: this is the smallest asset that makes the actual village jigsaw
  pipeline run end to end, and lazy parsing avoids paying the raw-NBT parse
  cost for templates a seed never touches. The extractor refuses catalogs over
  2.5 MB so a future all-structures run cannot silently blow the budget.
- Consequences: `assets/ASSETS.md` (out of scope for this slice) does not list
  the new file yet; `tests/structure_assets.rs` pins its bytes, FNV hash, and
  SHA-256. Selecting a non-village structure still fails with an explicit
  missing-template error.

## 2026-11-07 â€” Jigsaw feature pool elements are recorded skips
- Context: village pools contain `feature_pool_element` entries (trees, decor,
  piles). The configured/placed feature context needed to place them is not
  available inside jigsaw expansion, and failing the whole village made real
  generation fragile.
- Decision: during recursive child selection, record the feature id on
  `JigsawStart::skipped_feature_elements` and move to the next candidate. Keep
  the explicit `UnsupportedPoolElement` error for a feature element chosen as
  the start element.
- Why: a village without optional tree/decor features is still a valid
  structure, and the skip is observable rather than silent. Start elements must
  remain real templates.
- Consequences: generated villages currently omit their feature-pool decor
  until a later feature-context slice wires it in.

## 2026-11-07 â€” Stale `tick_hoppers` call removed to restore the build
- Context: `tick_world_with_table` called `WorldStorage::tick_hoppers`, but no
  such method exists anywhere in the tree; the call was added by the leaf-decay
  commit and made the baseline uncompilable. `SimpleBlockEntityTicker` already
  ticks hopper cooldowns in `tick_block_entities`.
- Decision: remove the stale call. This is a one-line build repair outside the
  structure slice; it does not add or remove hopper behavior.
- Consequences: hopper ticking continues through the existing block-entity
  ticker. A future dedicated hopper transfer tick should be added with its own
  `WorldStorage` method and tests rather than reintroducing this call.

## 2026-11-07 â€” `structure::assets` is a new public module
- Context: the task scope lists the new `assets.rs` but not `mod.rs`; Rust
  cannot compile a new child module without a declaration.
- Decision: add `pub mod assets;` and catalog re-exports to
  `worldgen/structure/mod.rs`.
- Consequences: `mod.rs` carries a few small lines outside the slice's file
  list; no behavior changes beyond exposing the catalog.

## 2026-11-06 â€” Item entities use a 4-block magnet and 3-block merge
- Context: vanilla `ItemEntity` has no in-tick magnet (pickup is a player-box
  inflate), and its merge radius is the spigot `itemMerge` value (default
  `1.0`), while the task requires magnet movement and merge within 3 blocks.
- Decision: model an item magnet within `4.0` blocks (pickup radius `1.5`,
  pull `(1-d/4)^2 * 0.2` toward the eye-half target) and merge within `3.0`
  blocks requiring same item + components and a combined count within the
  destination max stack. XP attraction keeps Java's `8`-block `(1-d/8)^2 *
  0.1` formula; XP merge is equal-value within `0.5` without Paper's
  `xpOrbGroupsPerArea` modulo. Metadata uses index `8` for both types
  (item-stack / int serializer), matching Java's first accessor after the
  base `Entity` indices.
- Why: deterministic, testable, and close enough to vanilla pickup feel
  without a player-session tick loop; the deviations are explicit.
- Consequences: pickup trajectories differ from a Java capture on edge cases
  (3-block merges, 4-block magnet). A Java pickup fixture is still required
  before claiming parity.

## 2026-11-06 â€” Pickup goes through a session-free trait; spawner keeps the loot signature backward-compatible
- Context: the task forbids `join.rs` edits and requires ground drops from
  block breaking without breaking the existing `stop_with_loot` caller.
- Decision: add `item_entity::PickupTarget` (`position`/`add_item`/
  `add_experience`) with a `SlicePickupTarget` adapter over a `36`-slot slice
  + XP pool that calls `inventory::add_stack`. Add `spawn_drops_at(pos,
  drops, rng)` (items first, then split orbs, block centre, Java velocity
  order) and `DigState::stop_with_entities` returning
  `DigStopEntities { stop, drops, entities }` that shares one `RandomSource`
  between the loot roll and the spawn velocities. `stop_with_loot` is
  unchanged. Drop ids come from a process-wide atomic counter starting at
  `20000` (distinct from session mob ids).
- Why: deterministic sequences for a fixed seed, no session writes, and no
  signature churn for inventory callers.
- Consequences: live spawning/ticking still needs a `join.rs` slice that
  implements `PickupTarget` for sessions and streams the packets; until then
  `finish_break` in `join.rs` still deposits drops directly.

## 2026-11-06 â€” Pistons use explicit moving-block payloads

- Context: `block_entity.rs` is still a stub with no typed moving-block API, but
  pistons must preserve the moved state across two movement ticks.
- Decision: `piston::PistonState` stores a `MovingPiston { moved_state,
  direction, extending, is_source_piston, progress }` per moving-block position
  on `TickState`. Movement is driven by scheduled `minecraft:moving_piston`
  ticks (0 -> 0.5 -> 1.0, then finalize), matching
  `PistonMovingBlockEntity.tick`. Extension/retraction trigger immediately from
  the neighbour update, like vanilla block events, so no artificial retraction
  delay is added.
- Why: The block-level behaviour is testable now, and the payload has the exact
  fields a future typed block entity needs. Leaving the moved state in a global
  side table without scheduling would lose blocks on chunk reload.
- Consequences: Moving-piston payloads are not persisted to chunk NBT yet, and
  block-entity contents are not part of the payload. Entity pushing/velocity
  and Java differential traces remain open.

## 2026-11-06 â€” Region ownership is enforced on tasks, not yet on world state

- Context: `region.rs` was a task-queue shim: region ids and per-region
  counters existed, but nothing owned world state and nothing rejected
  cross-region access, so "regionized" only meant "queued separately".
- Decision: Make `Region` public and owner-shaped: it holds its task queue,
  a `RegionWorldSlice` (world-slice token plus sparse block cells), and a
  monotonic tick counter. Owned tasks receive an `OwnedWorld` for their own
  region and fail with `OwnershipError` when they touch another region;
  `RegionOwned<T>` checks both an explicit caller region and the ticking
  region on the current thread (per-thread map + `ThreadingDetector`, the
  `TickThread` analogue). Access with no active region on the thread is
  denied, matching Folia's tick-thread-only rule. `RegionMap` uses vanilla
  8x8 chunk regions and 4x4 section regions with floor division, and
  `RegionId` packs key bits so equal regions always get equal ids.
- Why: This is the smallest slice that makes ownership a real, testable
  invariant instead of a scheduling hint, while keeping the legacy
  scheduler API (delay/period/cancel, `tick`/`tick_parallel`) intact.
- Consequences: The live server loop, `WorldStorage`, entities, players,
  and chunk tickets are still global; `Region` is not yet wired into them.
  Legacy (unowned) tasks still run, but any state they touch through
  `RegionOwned`/`OwnedWorld` is checked. Region merge/split, cross-region
  teleports, and entity transfer remain open (P10.2-P10.5, P10.7).

## 2026-11-04 Ã¢â‚¬â€ Git is not part of the workflow

- Context: The working tree contains large uncommitted changes and the user
  asked to ignore Git.
- Decision: Do not commit, branch, reset, or push. Treat the working tree as
  authoritative. Use `scripts/snapshot.ps1` for recoverable local snapshots.
- Why: Avoid blocking work on version-control policy and avoid accidental
  history operations.
- Consequences: There is no automatic diff history. Snapshots are the rollback
  mechanism, and the agent must avoid destructive edits.

## 2026-11-04 Ã¢â‚¬â€ Behavioral parity over literal transliteration

- Context: The decompiled Java tree is 6,485 files / 744,058 lines.
- Decision: Use Java as a specification and prove behavior with Rust tests and
  Java differential fixtures. Do not translate line-by-line.
- Why: A literal rewrite is not maintainable and does not guarantee parity.
- Consequences: Each subsystem needs a test harness; implementation may use
  different internal structure.

## 2026-11-04 Ã¢â‚¬â€ Placement modifiers split at the world boundary

- Context: 888 placement modifiers ship across 258 placed features. Most read
  the world (`heightmap`, `biome`, block predicates, surface depth, noise,
  environment scans); six are pure position/RNG transforms.
- Decision: Type every modifier and provider now, but implement only the
  world-free ones (`count`, `rarity_filter`, `in_square`, `fixed_placement`,
  `height_range`, `random_offset`) and their distributions. Every other type
  parses as an explicit unsupported variant and returns `PlacementFailure`
  when a placement runs; no modifier silently filters all positions.
- Decision: Refuse weighted lists whose total weight is 64 or more. Vanilla
  switches from an in-order scan to a pre-sorted selector at that threshold;
  the shipped maximum is 20, so the divergent case fails loudly instead of
  sampling differently.
- Decision: `clamped_normal` (Gaussian) stays unsupported because
  `folia-random` has no `nextGaussian` port yet; one `random_offset` uses it.
- Why: Modifier order and RNG draw order are parity-critical, and the
  world-free half can be pinned by scripted-random tests without a chunk
  generator.
- Consequences: 463 of 888 modifiers executed at that point; the `biome`
  entry below raises this to 667. The rest need heightmap access and block
  predicate tags.

## 2026-11-04 Ã¢â‚¬â€ Generated terrain instead of flat placeholders

- Context: joins only generated terrain when the level had no `region/`
  directory; with region files present, every stored-missing chunk was a flat
  placeholder, and nothing was sent when the player moved. In practice the
  whole spawn area of this repo's `world/` came from storage and was flat, so
  generated terrain only appeared ~500 blocks out.
- Decision: stream a 13Ãƒâ€”13 live column around the player (radius 6), resolving
  each chunk as `WorldStorage` Ã¢â€ â€™ noise generator Ã¢â€ â€™ flat, re-streaming when the
  player's chunk changes and tracking what was already sent. Stored chunks are
  served only when `world/level.dat` exists, or when `FOLIA_LOAD_STORED=1`
  is set.
- Why: A level without `level.dat` is not loadable Ã¢â‚¬â€ region files beside it are
  leftovers from an earlier flat run, and letting them win hides the generator
  across the entire spawn region. Players expect to walk into terrain, not a
  plane, so the leftovers lose unless explicitly requested.
- Consequences: generated chunks are not written back to disk, so leaving and
  re-entering an area regenerates it (deterministic for a fixed seed) and block
  edits on generated terrain are lost on restart. Generation is synchronous, so
  a radius-6 join costs ~5 s and each border crossing ~0.4 s; a vanilla-sized
  view distance needs async scheduling (TODO P3.3). No unloading or ticket
  system yet (P3.1), and each player still generates their own copy.

## 2026-11-04 Ã¢â‚¬â€ Client chunks are unloaded one chunk past the live radius

- Context: `sent_chunks` only grew, so a player walking across the map made the
  client hold every column it had ever seen; joins streamed a fixed 13Ãƒâ€”13 area.
- Decision: on every streaming batch, send `ClientboundForgetLevelChunk` (id 37)
  for columns outside `LIVE_CHUNK_RADIUS + UNLOAD_MARGIN` (margin 1) and drop
  them from `sent_chunks`.
- Why: the margin keeps the chunks at the edge of the client's render distance
  from popping the instant the player crosses a border, while the set stays
  bounded (13 columns leave and 13 arrive per border crossing).
- Packet id note: ids come from the registration order in
  `GameProtocols.CLIENTBOUND_TEMPLATE`; the derived table was checked against
  ids already captured from real traffic (block update 8, keep alive 43, level
  chunk 44, login 48, position 70, section blocks 82, chunk cache center 92).
- Consequences: chunks are re-streamed if a player walks back, which costs a
  region read (or generation only if the chunk was never saved Ã¢â‚¬â€ it always is).
  Ticket/reference-count sharing between players is still open (P3.1).

## 2026-11-04 Ã¢â‚¬â€ Generated terrain is written back and stamped

- Context: generated chunks were streamed but never saved, block edits on them
  were silently dropped (`storage` was `None`), and the whole spawn area of this
  repo's `world/` came from flat region leftovers, so the world looked flat.
- Decision: keep a `WorldStorage` for every session, save each generated chunk
  while streaming it, and stamp saved chunks with a `folia:written` NBT byte.
  Stored chunks are served only when the level has `level.dat` or the chunk is
  stamped, unless `FOLIA_LOAD_STORED=1` forces them.
- Why: generated terrain is deterministic per seed, so persisting it is safe,
  makes block edits work, and turns a cold join (~5 s of generation) into fast
  region reads. The stamp is what separates our terrain from flat leftovers in
  a level that has no `level.dat`; per-chunk trust avoids writing a half
  `level.dat` just to mark the world as ours.
- Consequences: region files grow as players explore, and the stamp is an extra
  (ignored by vanilla) chunk field. Unloading, dirty batching, and autosave are
  still open (TODO P3.1/P3.4).

## 2026-11-04 Ã¢â‚¬â€ Login lifts players out of solid ground

- Context: the captured play-position packet is `(0.5, -60.0, -2.5)` Ã¢â‚¬â€ four
  blocks above bedrock Ã¢â‚¬â€ so every fresh player spawned inside stone, and the
  playerdata written from that spawn kept them there on relogin.
- Decision: on login, if the block at the player's feet is solid, move them to
  the first empty block of their column before sending the position packet.
- Why: a player buried in stone sees a sealed, flat-looking world and cannot
  move; vanilla always spawns players on a surface.
- Consequences: a player who somehow stands inside a block is teleported to the
  surface instead of suffocating. Real spawn-point selection (bed, world spawn
  block) is still TODO P4.10.

## 2026-11-04 Ã¢â‚¬â€ `biome` filter reads world state through a trait

- Context: `BiomeFilter` is the most common modifier (204 of 888) and every
  ore placement ends with it. Vanilla's `BiomeFilter` calls
  `LevelReader.getBiome` then `BiomeGenerationSettings.hasFeature(topFeature)`;
  `getBiome` itself goes through `BiomeManager`'s eight-corner fuzzy search,
  not the exact `MultiNoiseBiomeSource` quart.
- Decision: Port `BiomeManager` in `worldgen/biome_zoom.rs` (SHA-256 of the
  little-endian seed for `obfuscateSeed`, the `LinearCongruentialGenerator`
  fiddled-distance search, tie keeps the first corner) and expose it as
  `BiomeZoomer`. Placement modifiers take `&dyn PlacementWorld`, whose two
  methods are exactly the queries ported filters need; `BiomePlacementWorld`
  implements them from `BiomeSampler` + `feature_data.json`.
- Decision: Recognize `biome` membership across all decoration steps, like
  `BiomeGenerationSettings.hasFeature`, rather than only the step being
  placed.
- Decision: Hand-roll SHA-256 instead of adding a dependency. It is ~90 lines,
  pinned to NIST vectors, and only used for seed obfuscation; the zoomer's
  expected `nearest_quart` values were derived from the Java source.
- Why: Ores cannot be placed without `biome`, and the zoomer is the only
  missing ingredient that needs no block-tag data. Keeping world access behind
  a trait lets placement tests stay generator-free.
- Consequences: At this point surface rules used the exact quart biome and
  features used the fuzzy one, matching vanilla's two different lookup paths;
  P2.12 later wired the same zoomer into the surface context as well.
  `biome_zoom.rs` has no Java fixture yet (the zoomer was checked against a
  Python reference of the Java code); `end_platform` now places instead of
  failing.

## 2026-11-04 Ã¢â‚¬â€ Feature framework scope and unsupported behavior

- Context: `feature_data.json` has 224 configured features across ~56 vanilla
  types. Porting `Feature`/`ConfiguredFeature`/`PlacedFeature` requires block
  survival checks and placement modifiers that do not exist yet.
- Decision: Load every definition into typed `ConfiguredFeature`/
  `PlacedFeature` records, implement the framework and the one vanilla type
  that needs no block behavior (`minecraft:no_op`), and represent every other
  type as `ConfiguredFeatureKind::Unsupported`. `place()` returns
  `UnsupportedFeature` instead of a silent `false`.
- Decision: Accept bare names from the dump as `minecraft:` resource
  locations, since all shipped entries are vanilla; validate every
  placedÃ¢â€ â€™configured and biomeÃ¢â€ â€™placed reference at load time.
- Decision: The feature write region is the chunk being generated. Vanilla
  uses a multi-chunk `WorldGenRegion`; widening this belongs with the
  chunk-status pipeline (P3.2), not with feature ports.
- Why: Keeps progress testable now, fails loudly for unported content, and
  avoids inventing block behavior before P1.5.
- Consequences: `PlacedFeature` modifiers are counted but not executed until
  P2.2; features are loaded but not yet run during generation.

## 2026-11-04 Ã¢â‚¬â€ Generated assets are guarded by a manifest, not Git

- Context: P0.3 asks for the generated registry/worldgen assets to be tracked,
  but Git is not used for workflow and must not be committed.
- Decision: Keep the assets in the working tree, document them in
  `assets/ASSETS.md` (version, DataVersion, counts, FNV-1a-64 checksums), and
  enforce that manifest with `tests/assets.rs`. FNV-1a is implemented in the
  test so no new dependency is added. `scripts/snapshot.ps1` remains the
  recoverable archive.
- Why: Detects truncation and regeneration drift without changing the
  version-control policy or adding a checksum crate.
- Consequences: Regenerating an asset requires updating the manifest and the
  test constants in the same change; the snapshot is the only off-tree copy.

## 2026-11-04 Ã¢â‚¬â€ Autonomy policy

- Context: The user wants the agent to continue without asking questions.
- Decision: Prefer the Folia 1.21.11 behavior, choose the simplest testable
  design, record decisions here, and move to another task if blocked.
- Why: Avoid stalls and preserve progress.
- Consequences: Some choices may need later review; the decision log is the
  audit trail.

## 2026-11-04 Ã¢â‚¬â€ Write `level.dat` for generated worlds

- Context: A world generated by this server had no `level.dat`, so nothing
  recorded the seed, spawn, or that the directory was a saved world; stored
  chunks were only trusted when a level file appeared from somewhere else.
- Decision: `level::write_new_world` writes a vanilla-shaped `level.dat`
  (DataVersion 4788, `Version`, `LevelName`, survival defaults, `spawn.pos`,
  overworld `WorldGenSettings` with the generator seed, `folia:written`) on the
  first join of a world without one, atomically via a temp file + rename. The
  marker splits trust: our own level serves only stamped chunks (leftover flat
  chunks are regenerated), while a level without the marker or
  `FOLIA_LOAD_STORED=1` serves every stored chunk. An unreadable `level.dat` is
  never overwritten; it falls back to serving stored chunks.
- Why: Makes generated terrain a real persisted world with a stable seed and
  spawn, and keeps the placeholder-era flat chunks from masking it.
- Consequences: `spawn.pos` is now the spawn for players without playerdata;
  `level.dat` state (time, spawn changes, game rules) is not rewritten yet, so
  it stays at the values from creation. A client that disconnects mid-stream
  now ends the session normally instead of surfacing an I/O error.

## 2026-11-04 Ã¢â‚¬â€ One shared overworld storage

- Context: Every login built its own `WorldStorage`, and `entity.rs` kept a
  third one, all over the same `region/*.mca` files. `RegionFile` caches chunks
  in memory and rewrites the whole file on save, so two instances silently
  overwrote each other's freshly generated chunks and block edits.
- Decision: Add `world::shared_overworld(root)` (a `OnceLock<Mutex<WorldStorage>>`)
  and `world::lock(...)`; sessions, entity physics, and level creation all go
  through that single instance. The first directory wins, matching a server
  that serves one level.
- Why: Removes a lost-update class of terrain corruption without changing the
  region format or adding a dependency.
- Consequences: Terrain access is serialized by one mutex; that is fine at the
  current scale (chunk generation dominates) but becomes a bottleneck when
  generation moves to async tasks (P3.3), which should revisit the locking
  granularity per region file.

## 2026-11-04 Ã¢â‚¬â€ Terrain flush on a timer instead of per batch

- Context: `stream_chunks_around` called `flush()` after every batch. A region
  save rewrites the whole `.mca`, so every chunk-border crossing rewrote a
  multi-megabyte file to persist 13 columns, and the cost grew with the explored
  area.
- Decision: Generated chunks go into the region cache and are flushed every
  `TERRAIN_FLUSH_INTERVAL` (5 s, checked on the 1 s read timeout) and when the
  session ends (disconnect, mid-stream disconnect, or socket error). Block
  edits keep their immediate `save_chunk` + `flush` because they are
  user-visible changes.
- Why: Fewer whole-file rewrites while a player explores, without changing the
  on-disk format.
- Consequences: A hard kill can lose up to 5 s of freshly generated terrain;
  that terrain regenerates deterministically from the saved seed. Per-chunk
  dirty batching inside a region file remains open work in P3.4.

## 2026-11-05 Ã¢â‚¬â€ `level.dat` records the session's time on shutdown

- Context: the level file wrote `Time = 0` and a fixed `DayTime` at creation and
  was never updated, so every restart reset the clock and `LastPlayed`.
- Decision: `level::record_session` adds the run's elapsed ticks to `Time` and
  `DayTime`, stamps `LastPlayed` with the current milliseconds, and can replace
  `spawn.pos` (currently always `None`). It rewrites the whole root read back
  from disk via a temp file + rename, preserving fields this port does not
  model.
- Why: the server loop is the only clock advance; persisting it means a saved
  world keeps its age across restarts and matches what vanilla writes. A
  whole-root rewrite avoids maintaining a partial field list that would silently
  drop data on save.
- Consequences: a level written by another server also gets its `Time` advanced
  by this server's ticks and `LastPlayed` refreshed on a clean shutdown. A hard
  kill loses the session's time (up to the whole run), same as the terrain
  flush. Game rules and spawn changes made while running are still not written
  back.

## 2026-11-05 Ã¢â‚¬â€ View distance is negotiated per session

- Context: joins always streamed a fixed 6-chunk radius regardless of
  `server.properties` `view-distance` or the client's request.
- Decision: use `min(server.view_distance, client.ClientInformation.viewDistance)`
  clamped to 2Ã¢â‚¬â€œ16 chunks, with 6 as the fallback when no value is configured or
  sent. The client's byte is parsed from `ClientInformation` during
  configuration.
- Why: vanilla clamps the client's request to the server's view distance (floor
  2), and this repo's default of 10 in `server.properties` was being ignored.
  The 16 cap keeps a cold batch's synchronous generation bounded until the
  async queue lands (P3.3).
- Consequences: a client asking for 2 now receives 25 columns instead of 169,
  a normal client (asks 12) is clamped to the shipped server's 10 and receives
  441 columns, and requests above 16 are capped at 1089 columns. Tests pin 6 in
  their `ClientInformation` packet to keep their column counts stable;
  `tests/view_distance_join.rs` covers a radius-4 session (81 columns, 9 new
  columns per border crossing).

## 2026-11-05 Ã¢â‚¬â€ Streaming batches generate columns in parallel

- Context: a cold 169-column join generated every column sequentially inside
  the session thread, taking 16.3 s on the probe machine.
- Decision: `generate_columns` splits the missing columns of a batch across up
  to `min(available_parallelism, 8)` scoped worker threads, writes results into
  per-index slots, and returns them in the batch's column order. Stored columns
  are still loaded first under the storage lock; generated chunks are saved
  back sequentially under the same lock.
- Why: the generator is read-only and deterministic, so column generation
  parallelizes without changing output; preserving order keeps the packet
  stream and the "13 new columns" border behaviour identical. A thread per
  batch (rather than a long-lived pool) avoids new shared state and matches the
  scoped session lifetime.
- Measured: the same fresh 169-column join dropped to 7.7 s (one run each;
  ~2.1Ãƒâ€”). The remaining cost is dominated by block-state generation and region
  writes.
- Consequences: up to 8 threads per joining session, so several simultaneous
  cold joins can oversubscribe the CPU until P3.3's bounded queue exists.
  In-flight dedupe and cancellation are still open; each session still
  generates its own copy of a column.

## 2026-11-05 Ã¢â‚¬â€ Block edits preserve chunk fields the model does not decode

- Context: `save_chunk` encoded only `xPos`, `zPos`, and `sections`, so any
  block edit in a stored chunk rewrote the column and silently dropped
  `Status`, `Heightmaps`, `BlockEntities`, `Entities`, `structures`, and
  `PostProcessing` Ã¢â‚¬â€ a vanilla-created or modded chunk lost that data.
- Decision: `WorldStorage::update_chunk` reads the stored root for the same
  column and copies every top-level field the encoder does not write before
  saving. Fields the server writes always win, and the `folia:written` stamp is
  re-applied. `save_chunk` stays a plain overwrite and is used for freshly
  generated terrain.
- Why: the chunk model only understands block states and biomes; re-encoding a
  loaded chunk without merging would destroy data the port cannot represent.
  Keeping generated saves unmerged avoids attaching stale block entities or
  heightmaps to terrain that just replaced the column.
- Consequences: an edited-chunk save costs one extra region read (a miss for
  fresh terrain, so streaming is unaffected), and a stored field whose name
  collides with an encoded field is ignored. Block edits still do not update
  `Heightmaps` or `BlockEntities` in place Ã¢â‚¬â€ they are preserved as-is, so a
  block entity whose block was removed remains until the port models block
  entities.

## 2026-11-05 Ã¢â‚¬â€ Edited blocks drop their block entities

- Context: preserving stored `BlockEntities` kept entries for blocks that no
  longer existed after an edit; vanilla removes the block entity when its block
  is replaced.
- Decision: `update_chunk` takes the world positions the edit changed and
  filters `BlockEntities` entries whose integer `x`/`y`/`z` match one of them;
  entries without integer coordinates are left untouched, and generated columns
  still overwrite the whole chunk.
- Why: block entity data (chest contents, furnace state) cannot outlive its
  block; keeping it would attach it to air or to a different block.
- Consequences: only positions the caller passes are dropped, so any future
  edit path must supply them. Other block entity state is preserved verbatim,
  not refreshed, and heightmaps are still not recomputed.

## 2026-11-05 Ã¢â‚¬â€ Streaming sends columns as they are generated

- Context: `generate_columns` waited for every missing column of a batch, then
  the session saved and sent them all, so a cold join delivered nothing for the
  whole generation time (~7 s at radius 6).
- Decision: worker threads report `(column index, chunk)` over an unbounded
  channel; the session walks the batch in nearest-first order, forwards stored
  columns in place, and waits only for the column at its cursor, saving each
  generated chunk just before its packet goes out. Packet order, counts, and
  the `sent_chunks` bookkeeping are unchanged.
- Why: the column under the player matters long before the last corner column,
  and keeping one receive cursor bounds buffering to out-of-order completions.
  Nearest-first also means the client can move without falling through unloaded
  terrain as early as possible.
- Consequences: a generation error aborts the batch after earlier columns were
  sent; the session disconnects anyway. In-flight dedupe, priority by distance,
  and cancellation are still open. A before/after latency comparison could not
  be trusted because leftover servers answered the probe, so no speedup number
  is claimed.

## 2026-11-05 Ã¢â‚¬â€ Playerdata autosaves while a session is idle

- Context: playerdata was written on disconnect and at most every 5 s inside the
  movement branch, so a player who stood still never had their position
  persisted before a crash.
- Decision: extract `autosave_playerdata`, gate it on `PLAYER_SAVE_INTERVAL`
  (5 s), and call it from both the movement path and the 1 s read-timeout arm.
- Why: the file must reflect the latest accepted position regardless of
  movement; the timeout arm runs at least once per second, so idle sessions save
  on schedule without a second timer thread.
- Consequences: idle sessions rewrite a small NBT file every 5 s. A hard crash
  can still lose up to 5 s of movement, and playerdata is still only saved by
  the session thread that owns it.

## 2026-11-05 Ã¢â‚¬â€ Shutdown writes cached terrain and keeps level.dat_old

- Context: region saves stay in the in-memory cache until a flush, and only the
  session's disconnect path flushed. A stop while a session was connected could
  exit with cached chunks unwritten. `level.dat` rewrites also replaced the only
  copy of the file in place (via rename).
- Decision: `world::flush_shared()` flushes the process-wide overworld storage
  and `main` calls it after the tick loop, before `level::record_session`.
  `level::write_root` copies the existing file to `level.dat_old` before the
  atomic rename, matching vanilla's backup slot.
- Why: an orderly shutdown should not lose terrain that is only waiting for the
  next 5 s flush, and a bad level write should be recoverable without a
  snapshot. Both operations are small and stay inside the existing locks.
- Consequences: shutdown now blocks on the storage mutex, so a session holding
  it can delay exit briefly. `level.dat_old` is only as fresh as the previous
  write; it is not written when no level file exists yet.

## 2026-11-05 Ã¢â‚¬â€ Shutdown waits for sessions to save

- Context: `main` stopped the tick loop and exited while session threads could
  still be connected; their playerdata was written only on disconnect or the
  5 s autosave, and their terrain only on the scheduled flush.
- Decision: `join::shutdown_sessions(timeout)` sets a process-wide flag and
  waits for an active-session counter (RAII `SessionGuard`) to reach zero, up
  to 5 s. The play loop checks the flag at the top of each iteration, saves
  playerdata, flushes terrain, and returns.
- Why: an orderly stop should not lose a player's last accepted position or
  cached terrain. The counter avoids a session registry, and the timeout bounds
  clients stuck in login/configuration, whose loops do not poll the flag.
- Consequences: shutdown can take up to one read timeout (~1 s) per idle
  session, and up to the timeout for clients still handshaking; after that,
  remaining sessions are abandoned and the process exits. The flag is one-shot
  and is never reset because the server exits.

## 2026-11-05 Ã¢â‚¬â€ Playerdata stamps the current DataVersion

- Context: saved playerdata had no `DataVersion`, while vanilla stamps it on
  every save; another server reading our file would treat it as unversioned.
- Decision: write `DataVersion = level::DATA_VERSION` (4788) as a known field,
  and list the key in `KNOWN_PLAYERDATA_FIELDS` so a loaded value is replaced by
  the current one instead of being duplicated through `extra`.
- Why: matches `NbtUtils.addCurrentDataVersion`, keeps the file self-describing,
  and makes an old file upgrade predictable on its next save.
- Consequences: loading an older playerdata drops its stored version and the
  next save writes 4788; unknown fields still round-trip unchanged.

## 2026-11-05 Ã¢â‚¬â€ A corrupt chunk reads as missing

- Context: `RegionFile::read_chunk` returned `InvalidData` for a bad length,
  truncated payload, or unknown compression, and `WorldStorage::load_chunk`
  propagated it, so a single damaged column aborted the join.
- Decision: `read_stored_chunk` treats invalid record data as `Ok(None)` and
  logs the chunk coordinates and region path; other I/O errors still propagate.
  `load_chunk`, `update_chunk`, and `is_written_chunk` all use it.
- Why: a damaged chunk should behave like an absent one, which vanilla already
  handles by regenerating it; failing the whole region (or the session) is worse
  than losing one column, and the log keeps the damage visible.
- Consequences: a corrupt chunk can be overwritten by regeneration on the next
  visit, losing whatever it held; structural `decode_chunk` errors are still
  reported rather than swallowed.

## 2026-11-05 Ã¢â‚¬â€ Chunk rewrites reuse their sectors

- Context: `RegionFile::write_chunk` always appended the new record at the end of
  the region and updated the header, so every save of an edited chunk added a
  full copy; a player editing one chunk repeatedly grew the file without bound.
- Decision: when the chunk's existing allocation (`location.sectors`) is large
  enough for the new record, write it at the same offset, zero the unused tail,
  and keep the existing sector count. Otherwise append as before. A location
  whose allocation extends past the in-memory data is not reused.
- Why: this is the common case for block edits and keeps region growth
  proportional to new chunks rather than edits. Keeping the original sector
  count means a later write can still reuse the allocation.
- Consequences: sectors left behind by previously appended copies are not
  reclaimed (no compaction or free list yet), so region files can still contain
  dead sectors; a record that outgrows its allocation appends a new copy.

## 2026-11-05 Ã¢â‚¬â€ Playerdata keeps survival vitals; chunks carry status

- Context: playerdata stored only position, rotation, inventory, slot, and
  dimension; health/hunger/XP stayed in the raw-extras bag, so a new player
  file omitted them, and generated chunks had neither `DataVersion` nor
  `Status`.
- Decision: model `Health`, `foodLevel`, `foodSaturationLevel`,
  `foodExhaustionLevel`, `XpLevel`, `XpP`, `XpTotal`, `Score`, and `Air` on
  `PlayerSession`, parse them with vanilla defaults, and write them on save.
  `encode_chunk` writes `DataVersion = 4788` and `Status = "minecraft:full"`.
- Why: a self-describing player file and chunk file are what vanilla and
  plugins expect; leaving vitals only in `extra` meant they were absent for new
  players and invisible to code that models them.
- Consequences: a loaded file's vitals are now parsed and re-written (still
  unchanged unless gameplay changes them); generated chunks always claim full
  status, which is accurate for this generator, and `Status` is no longer
  copied from an older stored root when a chunk is re-encoded.

## 2026-11-05 Ã¢â‚¬â€ Regions compact when edits leave too much garbage

- Context: in-place reuse keeps the common edit cheap, but a chunk that outgrows
  its allocation appends a new copy and abandons the old sectors forever; a
  long editing session could grow the file without bound.
- Decision: track `dead_sectors` in `RegionFile` (growing appends and removals
  add to it), and in `save_if_dirty` compact first when dead sectors exceed a
  quarter of the live allocation. `compact()` packs live records after the
  header in index order, drops records whose allocation or length is invalid,
  rewrites all header entries, and resets the counter.
- Why: compaction is amortized and only runs when the waste is material, so
  normal saves still just write the already-in-place data.
- Consequences: a compacting save rewrites the whole region in memory and on
  disk; records with damaged headers are dropped during compaction rather than
  copied, which matches the corrupt-chunk policy.

## 2026-11-05 Ã¢â‚¬â€ Decoded chunks are cached per storage

- Context: every `load_chunk` decompressed and decoded the same stored column
  again; with more than one reader (a second player, a border revisit, a block
  edit loading the chunk first) that work repeated.
- Decision: `WorldStorage` keeps a `HashMap<(x,z), Chunk>` of up to 256 decoded
  chunks with FIFO eviction and hit/miss counters. `load_chunk` checks it first;
  `save_chunk`/`update_chunk` replace the entry with the chunk just written, and
  `unload_region` drops every entry of that region.
- Why: the cache is bounded and keyed by the same coordinates the storage already
  uses, so it cannot serve another region's data; replacing on write means an
  edit is visible to the next read without a separate invalidation path.
- Consequences: cached chunks are clones, so memory is bounded by the limit but
  each hit copies the sections; the cache is per storage instance, which is the
  process-wide shared overworld in practice.

## 2026-11-05 Ã¢â‚¬â€ Stored chunks include recomputed heightmaps

- Context: generated and edited chunk NBT carried `Status`/`DataVersion` but no
  `Heightmaps`, so a stored column was incomplete for anything that reads the
  heightmap arrays from the file instead of the chunk packet.
- Decision: `chunk_packet::heightmap_nbt` emits `MOTION_BLOCKING`,
  `MOTION_BLOCKING_NO_LEAVES`, and `WORLD_SURFACE` from the same `column_height`
  values used by the network packet, packed at 9 bits per column (37 longs).
  `world::stamp_chunk` removes any stored `Heightmaps` and installs the
  recomputed compound, so edits cannot leave a stale array behind.
- Why: the packet and the file now describe the same heights, and recomputing on
  save avoids having to mutate the stored heightmap incrementally.
- Consequences: every server-written chunk carries three long arrays (roughly
  888 bytes) in addition to its sections; a chunk whose palette the height
  function cannot interpret still gets the height function's non-air result.

## 2026-11-05 Ã¢â‚¬â€ Generation queue is bounded and cancels on drop

- Context: `send_columns` used an unbounded channel, so workers could generate
  the whole batch even after the session had failed to write a chunk and
  returned; the scoped threads then had to finish before the join unwound.
- Decision: use `sync_channel(16)`. Workers block when the queue is full and
  break on a send error; the session's receiver is dropped on any early return,
  which turns pending/blocked sends into errors.
- Why: a disconnected client should not consume the rest of a cold batch's CPU,
  and bounded in-flight work keeps a large view distance from materializing
  every chunk immediately.
- Consequences: generation order is unchanged (indices are still claimed in
  nearest-first order), but workers can now block on backpressure; the session
  always drains toward the column it needs, so the queue cannot deadlock. The
  integration test covers correct 81-column streaming; cancellation itself is
  not separately timed in a test.


## 2026-11-05 Ã¢â‚¬â€ Basic movement correction and a legal-step border test

- Context: movement packets were applied verbatim, so one packet could place a
  player anywhere; adding the guard exposed that the view-distance test moved
  diagonally 10.4 blocks in one packet, which the guard correctly reverted.
- Decision: in the play loop, reject a movement whose squared displacement
  exceeds 100 (ten blocks), restore the previous position, and send a position
  packet with the last accepted place and a fresh teleport id. The border test
  now walks 8.5 blocks east from the joined position, a legal step that still
  crosses into chunk 1.
- Why: this is the same order of magnitude as vanilla's "moved too quickly"
  guard and prevents single-packet teleports without touching normal movement.
- Consequences: legitimate teleports (commands, respawn) will also be reverted
  until a real teleport path sets the position server-side; the client's accept
  teleport packet is already ignored, which is sufficient for the correction to
  take effect.

## 2026-11-05 Ã¢â‚¬â€ First tree slice: trunk/foliage now, decorators later

- Context: Forest and plains had no trees because `TreeFeature` and the tree
  placers were unported. A complete tree port needs every trunk/foliage/root
  placer and every decorator; the placed features used by forests also contain
  unsupported fancy-oak/fallen-tree selector branches.
- Decision: Port `TreeFeature.doPlace` for `straight_trunk_placer` +
  `blob_foliage_placer`, including vanilla RNG order, free-height scan, dirt
  replacement, and leaf-layer shape. Parse `random_selector` and preserve every
  branch's `nextFloat` draw; branches whose configured feature is unsupported
  place nothing. Parse tree decorators but do not execute them yet.
- Why: This is the smallest vertical slice that makes new forest/plains/birch
  chunks visibly wooded without inventing a separate tree generator. Keeping
  unsupported selector draws preserves the RNG position for supported branches.
- Consequences: Trunks and foliage are real; leaf litter, beehives, vines, and
  the fancy/spruce/fallen/mangrove placers remain open (P2.4 `[~]`). A tree
  with decorators is not full parity and must not be reported as such. Tree
  positions are deterministic per chunk but not yet the vanilla
  `WorldgenRandom` sequence, so block-for-block Java parity still needs a
  fixture.

## 2026-11-05 Ã¢â‚¬â€ Chunk-packet oracle uses an immutable fixture world

- Context: `tests/world_chunk_packet.rs` read `world/region/r.0.-1.mca`
  directly. A live server run regenerated that directory with noise terrain,
  replacing the captured flat chunk and failing the oracle even though the code
  was unchanged.
- Decision: Extract the captured region file from the pre-run snapshot
  `folia-rs-20261105-200311-p4-movement-correction.zip` into
  `crates/folia-core/tests/fixtures/world/region/r.0.-1.mca` and point the test
  at that fixture. Never read the live `world/` from a test.
- Why: The oracle must be immutable; runtime world data is mutable by design.
- Consequences: The test fixture is about 1.2 MB and travels with snapshots.
  Live server checks should continue to use a scratch level such as
  `FOLIA_LEVEL_NAME=target/probe-world`.

## 2026-11-05 Ã¢â‚¬â€ Shutdown aborts an in-progress chunk batch

- Context: Position is sent before the chunk batch so the client leaves the
  loading screen after the first column. A shutdown requested while the first
  169-column batch was still generating waited for the whole batch and tripped
  the 3 s shutdown test.
- Decision: Workers check `shutdown_requested()` before claiming a chunk, and
  the sending loop checks it before each column and when the generation channel
  closes. The batch returns `Ok(())` early; the play loop then saves and exits.
- Why: Shutdown should not generate or send columns no client will receive.
- Consequences: A shutdown during the initial stream leaves `sent_chunks`
  incomplete, but the process is exiting; no resume path is needed.

## 2026-11-05 Ã¢â‚¬â€ Chunk packet encoding caches palette state ids

- Context: `build_chunk_data` called `block_state_id` for every block. That
  function builds and sorts a property string, so a 64-column benchmark spent
  3.28 s encoding packets versus 1.64 s generating the same columns.
- Decision: resolve each section palette entry to its registry id once, then
  index that table while writing the 4096 block entries. Build generated
  columns' packets on the generation worker instead of the session thread.
- Why: the palette is small and stable for a section, so repeated registry work
  was pure overhead. Packet building is independent per chunk and safely
  parallel with generation. The wire format and palette order are unchanged.
- Measured (release, probe machine): 64 sequential packets 3.28 s -> 0.28 s;
  169-column cold join fixture 3.55 s -> 1.89-1.94 s. A regression test decodes
  the encoded palette and every block index.
- Consequences: generated workers now send `(chunk, packet)` pairs; the bounded
  16-column queue also bounds packet memory. Region saving remains on the
  session thread and is now the largest single-threaded part of a cold join.

## 2026-11-05 Ã¢â‚¬â€ Tree decorators are explicit or the tree is unsupported

- Context: forest/plains selectors default to beehive-decorated oaks, and the
  leaf-litter variants use weighted state providers. Parsing those decorators
  without executing them would silently generate undecorated trees; rejecting
  every decorated config would remove the forest trees entirely.
- Decision: implement `place_on_ground` and `beehive`, parse their providers
  and probabilities, and make any still-unknown decorator type reject the whole
  tree config. Beehive placement follows the vanilla probability, EAST/SOUTH/
  WEST candidate order, Fisher-Yates shuffle, and SOUTH-facing `bee_nest`
  state. Bee occupant NBT is not modeled yet.
- Why: the selector defaults become supported again, while unsupported trees
  fail closed instead of losing their decoration silently.
- Consequences: forest/plains/birch trees are back (forest fixture: 11 logs,
  109 leaves). Leaf litter and empty beehives are generated; beehives contain
  no bees, and vine/cocoa/alter-ground decorators plus other placers remain
  open. Full block-behavior metadata is still missing, so `is_solid_render`
  recognizes the common surface blocks used by the supported biomes.

## 2026-11-05 Ã¢â‚¬â€ Vine and cocoa decorators follow vanilla call order

- Context: `jungle_tree` and `swamp_oak` already use supported trunk/foliage
  placers, but decorator rejection made both configs unsupported. Their
  decorators are small and visible in generated jungles and swamps.
- Decision: implement `leave_vine`, `trunk_vine`, and `cocoa`. Preserve the
  sorted log/leaf order, four side draws per leaf/log, vanilla random gates
  before air checks, up to four hanging extensions, and cocoa's lowest-three-
  layer filter with its conditional age draw. Vine states include all five
  default side/up flags.
- Why: supports the shipped jungle/swamp configs without approximating their
  decoration; still-unknown decorators keep failing closed.
- Consequences: `jungle_tree` (3 decorators) and `swamp_oak` (1) parse and
  place; remaining attached-to-leaves/pale-moss/creaking-heart/alter-ground
  decorators and non-blob placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Spruce and pine foliage follow TreeFeature RNG order

- Context: `spruce` and `pine` use the already-supported straight trunk, but
  their foliage placers were rejected. Implementing them adds conifer shapes
  without another trunk port.
- Decision: add `spruce_foliage_placer` and `pine_foliage_placer`. `TreeFeature`
  samples foliage height first, computes `treeHeight - foliageHeight`, then
  samples the common radius; pine adds its height-derived radius draw. The
  offset is sampled inside `createFoliage`, after trunk writes, as in Java.
  Spruce uses its two-state row-radius progression; pine uses its tapered
  layer progression and only skips corners.
- Why: the shipped spruce/pine configs become supported without changing the
  straight-trunk behavior or approximating foliage shape.
- Consequences: shipped fixtures place 7 logs/54 leaves (spruce) and 7/14
  (pine); fancy/dark oak/cherry/mega/mangrove/fallen placers and the remaining
  decorators stay open.

## 2026-11-05 Ã¢â‚¬â€ Dark oak uses multi-attachment trunk placement

- Context: dark oak needs a 2x2 trunk, leaning/branch logs, and foliage at
  several attachment points; the existing single-attachment path could not
  represent it.
- Decision: make trunk placement return attachments and support dark oak's
  leaning 2x2 trunk, branches, three-layer minimum size, signed leaf clipping,
  and double-trunk foliage. Straight trunks still return one attachment.
- Consequences: `dark_oak` parses and places (11 logs/126 leaves fixture);
  fancy/cherry/mega/mangrove/fallen placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Console commands use a per-session channel

- Context: player sessions own their `TcpStream` and read loop, so the terminal
  thread cannot safely write to them directly.
- Decision: register a control channel per live session and have the session
  loop poll it between reads; console `gamemode` sends a command and waits for
  a bounded reply. Operators are a shared, file-backed set at `run/ops.txt`.
- Consequences: console commands work without a global stream lock, but only
  while the session is in its play loop; commands time out after 2 s.

## 2026-11-05 Ã¢â‚¬â€ Void death is the first damage slice

- Context: health was persisted but never changed or sent, so falling below the
  world was harmless.
- Decision: apply 4 void damage per 500 ms below `y = -128`, send the captured
  health packet, send combat-kill at zero health, and respawn on the client
  command (or after 2 s). Spectators are exempt; survival and creative are not.
- Consequences: void falls are now lethal and recoverable without a full damage
  system. Other damage sources, drops, and real respawn-point selection remain
  open.

## 2026-11-05 Ã¢â‚¬â€ Acacia uses a forking trunk and acacia foliage

- Context: acacia is the dominant savanna tree; its forking trunk differs from
  the straight and 2x2 dark oak placers, and its foliage has three explicit
  rows with signed-corner clipping.
- Decision: extend `FoliageAttachment` with the vanilla radius offset and port
  `ForkingTrunkPlacer` plus `AcaciaFoliagePlacer` in Java RNG order.
- Consequences: `acacia` parses and places (11 logs/73 leaves at seed 7 on the
  flat fixture); fancy/cherry/mega/mangrove/fallen placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Fancy oak ports limb placement and clipped height

- Context: fancy oak needs a limb-based trunk with axis-tagged logs and a foliage
  shape distinct from blob/dark oak; its config also declares a minimum clipped
  height so small obstacles can shorten the tree instead of rejecting it.
- Decision: add fancy trunk/foliage variants, a log-axis property setter, and
  parse/apply `min_clipped_height` only when the free-height scan reaches it.
- Consequences: `fancy_oak` parses and places (8 logs/112 leaves, 2 horizontal
  logs at seed 7); cherry/mega/mangrove/fallen placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Cherry uses sampled branch walks and hanging foliage

- Context: cherry trees branch in one or two horizontal directions, with
  branch start/end offsets and a foliage canopy that drops hanging leaves.
- Decision: store the four cherry integer providers on the trunk variant, port
  `generateBranch` with axis-tagged logs, and implement the hanging-leaf row
  helper against the same leaf-position set used by the foliage setter.
- Consequences: `cherry` parses and places (20 logs/291 leaves and 8 horizontal
  logs at seed 7); mega/mangrove/fallen placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Mega spruce/pine share the giant trunk and alter ground

- Context: mega spruce and mega pine both use a 2x2 giant trunk and the
  mega-pine crown foliage, but their only decorator is `alter_ground`, which
  was previously rejected.
- Decision: port `GiantTrunkPlacer`, `MegaPineFoliagePlacer`, and the
  `AlterGroundDecorator` ground scan/circle helper; keep mega jungle separate
  because it needs a different trunk and jungle foliage.
- Consequences: `mega_spruce` and `mega_pine` parse and place with podzol
  ground (109 logs/295 leaves and 109/75 at seed 7); mega jungle/mangrove/fallen
  placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Mega jungle extends the giant trunk with angled branches

- Context: mega jungle uses the same giant 2x2 trunk as mega spruce/pine but
  adds a trunk-level branch walk and the `jungle_foliage_placer` registered by
  `MegaJungleFoliagePlacer`.
- Decision: reuse `place_giant_trunk`, port the branch loop with the shared
  `Mth` sin/cos lookup table, and add a dedicated mega-jungle foliage variant.
- Consequences: `mega_jungle_tree` parses and places (63 logs/287 leaves and
  6 branch logs at seed 7); mangrove/fallen placers remain open.

## 2026-11-05 Ã¢â‚¬â€ Attached-to-leaves samples randomized integer states

- Context: mangrove's attached propagules use `attached_to_leaves` with a
  `randomized_int_state_provider`, and neither was parsed by the tree loader.
- Decision: preserve the Java order (shuffle leaves, draw a direction per
  leaf, test exclusion/probability/empty blocks, fill the exclusion box, then
  sample the provider) and model `randomized_int_state_provider` as a recursive
  provider that samples its source before its integer value.
- Consequences: attached leaves and randomized integer states are covered by a
  synthetic oak fixture (6 propagules at seed 7); mangrove still needs its
  upwards trunk, random-spread foliage, and root placer.

## 2026-11-05 Ã¢â‚¬â€ Jungle bush reuses the straight trunk

- Context: `jungle_bush` is a shipped tree with no decorators and uses the
  already-supported straight trunk, but its `bush_foliage_placer` was rejected.
- Decision: add a dedicated bush foliage variant with the inherited blob height
  and the signed positive-corner skip rule, rather than treating it as a blob.
- Consequences: `jungle_bush` now parses and places (1 log/32 leaves at seed 7);
  bending/random-spread placers and the remaining decorators stay open.

## 2026-11-05 Ã¢â‚¬â€ Azalea uses a bending trunk and weighted foliage

- Context: `azalea_tree` ships a `bending_trunk_placer` and
  `random_spread_foliage_placer` with a weighted azalea/flowering-azalea
  provider, none of which the tree loader supported.
- Decision: add a dedicated bending trunk variant (one horizontal direction,
  a `nextInt(2)` per trunk layer, attachments from `min_height_for_leaves`,
  then the bending log walk), generalize the foliage provider from a fixed
  block state to a `BlockStateProvider`, and sample it only after a leaf
  position is valid, matching Java draw order.
- Consequences: `azalea_tree` now parses and places (8 logs/65 azalea
  leaves/24 flowering azalea leaves at seed 7); pale moss, creaking heart,
  mangrove, and fallen trees remain open.

## 2026-11-05 Ã¢â‚¬â€ Mangrove uses the upwards-branching trunk and root placer

- Context: `mangrove` and `tall_mangrove` ship `upwards_branching_trunk_placer`
  and `mangrove_root_placer`, which need two block tags plus waterlogged and
  muddy root states.
- Decision: resolve the exact `mangrove_logs_can_grow_through` and
  `mangrove_roots_can_grow_through` 1.21.11 tag sets, sample the trunk origin
  after foliage radius, run root simulation before trunk placement, preserve
  the muddy-root bypass and above-root chance order, and treat water/leaves/
  seagrass as `replaceable_by_trees`.
- Consequences: `mangrove` and `tall_mangrove` parse and place (10 logs/157
  leaves/17 roots, 7 waterlogged, 7 muddy, 3 moss carpets at seed 7); only the
  fallen placer, pale moss, creaking heart, and beehive occupants remain open.
## 2026-11-05 Ã¢â‚¬â€ Fallen trees reuse the tree decorator context

- Context: `fallen_tree` is a separate configured feature but shares
  `BlockStateProvider`, `IntProvider`, `TreeDecorator`, `valid_tree_pos`, and
  log placement with normal trees. Five shipped configs use it.
- Decision: model a dedicated `FallenTreeFeature` rather than adding a shape to
  `TreeFeature`; parse the stump and log decorator lists through the shared
  decorator parser; preserve Java order (stump and stump decorators, direction,
  length, start offset, ground scan, whole-log validation, sideways log
  placement, log decorators). Add `attached_to_logs` with shuffled logs and
  per-log direction/probability/air rules.
- Consequences: `minecraft:fallen_oak_tree`, `fallen_birch_tree`,
  `fallen_jungle_tree`, `fallen_spruce_tree`, and `fallen_super_birch_tree`
  dispatch as supported. The seed-7 shipped oak fixture places 5 logs (4 with
  horizontal axis) and 4 mushrooms. This is a behavior fixture, not a Java
  block-for-block parity claim; the simple-provider RNG caveat remains.

## 2026-11-06 Ã¢â‚¬â€ Creaking heart is ported; pale moss remains unsupported

- Context: Both remaining tree decorators were investigated. `creaking_heart`
  is self-contained, while `pale_moss` conditionally delegates its ground pass
  to `minecraft:pale_moss_patch`, which depends on unported vegetation-patch and
  block-tag support.
- Decision: Implement `creaking_heart` with Java's probability check, shuffled
  log order, six-direction log requirement, and dormant/natural heart state.
  Keep `pale_moss` an explicit unsupported component rather than placing only
  the hangers and silently dropping the ground patch.
- Consequences: Synthetic creaking-heart trees are supported and tested; no
  shipped pale-oak config becomes supported yet. Pale-oak support is gated on
  P1.3 tags and P2.5 vegetation patches.

## 2026-11-06 Ã¢â‚¬â€ Ore features land, generator wiring waits for RNG parity

- Context: `OreFeature` and `ScatteredOreFeature` are self-contained once the
  four shipped target tags are known, but their positions flow through the
  project's approximate per-chunk feature RNG.
- Decision: Port both features and cover all 30 + 2 shipped configs, but do not
  enable the `underground_ores` generation step yet. The direct feature tests
  prove the algorithms; generator wiring is held until feature-RNG parity so
  the exact-name parity fixture is not weakened.
- Consequences: Ore configs are no longer unsupported, but generated worlds
  remain ore-free for now. `OreVeinifier` and block-for-block ore positions are
  still open; P2.3 stays `[~]`.
- Superseded (2026-11-06): the detector wiring landed with the lazy placement
  stream, parameter-order feature indices, and `OreVeinifier`; see the
  "Ore wiring uses a lazy placement stream" entry below.

## 2026-11-06 Ã¢â‚¬â€ Tags are flattened at extraction, not resolved at runtime

- Context: P1.3 needs block/item/entity/fluid/game-event/damage-type/
  enchantment/POI/worldgen tag membership. The reference tree stores one JSON
  file per tag with nested `#tag` references; parsing and recursively resolving
  them in Rust would duplicate a data-loading error surface and add a startup
  cost for data that never changes between builds.
- Decision: `scripts/extract_tags.ps1` resolves every tag once and emits one
  sorted `tag<TAB>member` row per member into `assets/tags/*.tsv`. `src/tags.rs`
  embeds the nine files with `include_str!` behind a single `OnceLock`, stores
  members in sorted `Vec<String>`s, and uses binary search for `has`. Bare keys
  are normalized to `minecraft:`; unknown tags return an empty slice and
  unknown members return `false`.
- Decision: Empty tags emit no rows, exactly as specified. Consequently
  `tags::tag_count` counts non-empty tags: 1.21.11 has two empty block tags
  (`incorrect_for_diamond_tool`, `incorrect_for_netherite_tool`), so block
  reports 202 tags from 204 files. The extractor prints the file count, empty
  count, and flattened member count separately so the distinction is visible.
- Decision: The four worldgen subregistries share one `worldgen.tsv`. Tag keys
  are relative to their subregistry directory (`biome/is_forest` ->
  `minecraft:is_forest`, `structure/village` -> `minecraft:village`,
  `biome/has_structure/village_plains` ->
  `minecraft:has_structure/village_plains`); those keys are disjoint in
  1.21.11, and the extractor fails loudly on a duplicate.
- Decision: `replace` is parsed by the schema but has no merge step because the
  decompiled tree is the complete vanilla set, not an overlay. Datapack tags
  and `replace` overlays remain future work (P1.7/P2.14).
- Why: Extraction makes nested-reference and cycle errors build-time data
  problems, keeps runtime I/O at zero, and gives tests one canonical sorted
  representation to pin.
- Consequences: Regenerating tags requires updating `ASSETS.md`,
  `tests/assets.rs`, and the exact counts in `tests/tags.rs`; the script prints
  all of those numbers. Cosmetic registries (`banner_pattern`, `dialog`,
  `instrument`, `painting_variant`, `timeline`) are deliberately not extracted
  yet, and `worldgen/feature/ore.rs` still hard-codes its four target-tag sets
  until it is switched to `tags::has`.

## 2026-11-06 - Lighting storage lives on each `ChunkSection`

- Context: P3.5 needs optional vanilla `SkyLight`/`BlockLight` arrays that
  survive `decode_chunk`/`encode_chunk`, packet building, block edits, and a
  region save. A separate `Chunk.light` map would avoid adding a field but
  would require a parallel lifetime/cache across every chunk clone; `Chunk` and
  `ChunkSection` struct literals already exist throughout worldgen/tests.
- Decision: Each `ChunkSection` owns
  `light: SectionLight { sky: Option<DataLayer>, block: Option<DataLayer> }`.
  `DataLayer` boxes its 2048-byte array so absent layers cost one niche-sized
  `Option`. `decode_section`/`encode_section` map those options to the vanilla
  section `SkyLight`/`BlockLight` byte arrays. `light::LightStorage` remains
  the engine's section-keyed intermediate result.
- Decision: Every block edit clears the chunk's cached layers. The engine then
  recomputes the whole chunk (and a one-block neighbour margin when neighbours
  are loaded). This is less incremental than vanilla's decrease/increase
  queues, but it cannot leave a stale light value and is deterministic.
- Decision: `magma_block` emits 3, following `Blocks.MAGMA_BLOCK` and the
  decompiled `lightLevel(blockState -> 3)`, not the task brief's example of 15.
- Decision: Opacity/emission are an explicit hand-written table until P1.5
  generates block behaviour: air/colored glass 0, water/leaves 1, tinted glass
  and unknown blocks 15. Unknown emitters return 0. This fails safe (dark rather
  than inventing light) and is documented in `BLOCKERS.md`.
- Why: This is the smallest correct vertical slice: vanilla NBT bytes round-trip,
  packet masks are driven by real arrays, and edits cannot persist stale light.
- Consequences: `worldgen/generator.rs` and two test fixtures needed a
  mechanical `light: SectionLight::default()` constructor line; no worldgen or
  feature logic changed. Full incremental light packets, queued cross-chunk
  relighting, generation-time initialization, and Java light fixtures remain
  P3.6-P3.9 work.


## 2026-11-06 - Player physics reconciles client movement against the world

- Context: Vanilla's server accepts the client's position with only coarse
  checks, but this port needs a simulated player for collision, fall damage, and
  future interaction. The client movement packet does not carry WASD input, so a
  pure server-side `travel` tick cannot reproduce a player's acceleration from a
  standstill without rubber-banding.
- Decision: `PlayerPhysics` implements the vanilla `LivingEntity.travelInAir`
  formulas (gravity 0.08, drag 0.98, ground friction, jump 0.42, step 0.6,
  0.91 horizontal drag) and a `tick` entry point for future server ticks, but
  live movement packets are reconciled by replaying their claimed delta through
  the axis-separated collider. A legal claim within 1e-3 blocks is accepted
  exactly; a blocked claim sends a correction and leaves the simulation at the
  collision limit. The existing ten-block teleport net remains as the first
  check.
- Decision: A single packet moving more than 0.75 blocks horizontally is
  treated as a client lag burst (vanilla's fastest grounded movement is well
  below that). Its destination is validated instead of replaying every tick, so
  the existing 8.5-block chunk-border tests do not rubber-band over natural
  terrain. An endpoint inside terrain is lifted by up to eight blocks to the
  first free position (`Adjusted`); if none exists it is rejected.
- Decision: Player collision uses full-cube shapes with vanilla friction values
  (ice 0.98, slime 0.8, otherwise 0.6), and treats the block tags
  `replaceable`/`flowers`/`saplings`/`crops`/`rails`/`buttons`/
  `pressure_plates`/`wall_post_override`/`all_signs`/`fire` plus air and fluids
  as passable. `minecraft:snow` is excluded because its thin layer has real
  collision.
- Why: This keeps normal walking stable, catches normal-speed wall clipping and
  teleports, and still lets fall distance come from the server simulation.
- Consequences: Non-full block shapes (slabs, stairs, fences, snow layers) are
  full cubes until P1.5 block-shape metadata lands. Burst moves validate the
  destination rather than the path; a lagging client is lifted out of terrain
  instead of being pushed back. Unported and therefore not simulated: ladders,
  vines/scaffolding, cobwebs, slime blocks, elytra, water/lava currents and
  swimming physics, bubble columns, powder snow, soulsand/magma jump factors,
  levitation/slow falling, sprint/sneak input packets, and real teleport
  acknowledgement. The player tick still follows movement packets rather than a
  standalone 20 Hz player scheduler.

## 2026-11-06 - Vegetation providers stay independent from the tree path

- Context: The tree module already has a private `BlockStateProvider` parser,
  but its `simple_state_provider` samples through the weighted path and draws a
  random value. Vanilla `SimpleStateProvider.getState` does not draw. Vegetation
  needs exact provider RNG order, and changing the shared parser would alter
  every existing tree fixture.
- Decision: Keep `vegetation.rs` state providers separate. `Simple` returns the
  state without a draw; `Weighted` draws one `nextInt(totalWeight)`;
  `RandomizedInt` samples its source before its property value; the noise
  providers index by position and only consume a draw where vanilla does
  (`NoiseThresholdProvider` low/high branch); `Rotated` consumes one draw for
  the axis.
- Consequences: Tree placement RNG is unchanged. Vegetation has its own tested
  provider behavior, and the documented simple-provider draw caveat still
  applies only to trees.

## 2026-11-06 - Nested vegetation placements are evaluated locally

- Context: `placement.rs` is intentionally outside this slice, and its
  `block_predicate_filter` only supports sapling `would_survive`. Every nested
  placement used by the shipped `random_patch`/`flower` configs is a
  `block_predicate_filter` with air/fluid/block predicates.
- Decision: Parse and execute nested placed features inside `vegetation.rs`
  with a predicate-only placement chain. Supported predicates are
  `matching_blocks`, `matching_fluids`, `would_survive`, `replaceable`, `solid`,
  `has_sturdy_face`, `all_of`, `any_of`, and `not`. Any other nested modifier
  makes the parent configured feature an explicit `Unsupported`, never a silent
  skip.
- Consequences: All 23 non-scheduled shipped `random_patch` configs and 8 of 9
  `flower` configs parse; the shipped `patch_grass_plain`, `flower_plain`,
  `moss_patch`, and `pale_moss_patch` definitions are executable. Top-level
  placement modifiers are still handled by `placement.rs`, so features whose
  outer placement uses an unported modifier remain wired but inactive.

## 2026-11-06 - Vegetation noise providers use the available NormalNoise path

- Context: Vanilla seeds `NoiseBasedStateProvider` with
  `WorldgenRandom(new LegacyRandomSource(seed))`. The current
  `folia_core::worldgen::noise::NormalNoise::create` constructor accepts only a
  `XoroshiroRandomSource`, and `noise.rs` is outside this task's file scope.
- Decision: Construct provider noise with `XoroshiroRandomSource::new(seed)`
  and keep the exact vanilla evaluation formula (scale, octaves, threshold,
  dual-noise mapping). This is deterministic and tested, but it is not a
  block-for-block Java noise-provider parity claim.
- Consequences: Flower/meadow provider state choices can differ from Java until
  `NormalNoise` accepts a generic `RandomSource` seeded from a
  `LegacyRandomSource`. The gap is recorded here and in `TODO.md`/`PORT_MAP.md`.

## 2026-11-06 - Vegetation patch sets follow Java HashSet iteration order

- Context: `VegetationPatchFeature` stores ground positions in a `HashSet` and
  then iterates it to place vegetation. `BlockPos.hashCode` and the default
  `HashSet` bucket order therefore influence RNG consumption.
- Decision: Collect positions once, emulate Java's `(y + z * 31) * 31 + x`
  hashCode and `h ^ (h >>> 16)` bucket spread, use the smallest power-of-two
  table that satisfies the 0.75 load factor (minimum 16), and iterate buckets
  in ascending order with insertion order inside a bucket.
- Consequences: Vegetation patch vegetation order is deterministic and matches
  the common Java `HashSet` case. Treeification of very large buckets is not
  modeled; shipped patches never reach that size.

## 2026-11-06 - `schedule_tick` and `simple_random_selector` stay explicit gaps

- Context: Two shipped simple-block configs (`pale_forest_flowers`,
  `flower_pale_garden`) request `schedule_tick: true`, and both dripleaf
  vegetation patches nest `minecraft:simple_random_selector`. The chunk
  generation boundary has no tick queue, and `simple_random_selector` is not
  ported.
- Decision: Treat `schedule_tick: true` simple blocks and vegetation patches
  whose nested feature is `simple_random_selector` as explicit `Unsupported`
  rather than placing the block and silently dropping the scheduled tick or the
  selector. The remaining 134 configured features keep executing.
- Consequences: `pale_forest_flowers`, `flower_pale_garden`,
  `clay_with_dripleaves`, and `clay_pool_with_dripleaves` remain unsupported.
## 2026-11-06 â€” Structure template foundation is loading/placement only

- Context: P2.8 needs the vanilla structure NBT model and the
  `StructureTemplate.placeInWorld` coordinate math before starts, pieces,
  processors, pools, or jigsaw expansion can be built. The shipped templates
  are gzip NBT under `data/minecraft/structure` (1,202 files), and the existing
  `nbt::read_gzip_root` already covers the container format.
- Decision: Load templates through `nbt::read_gzip_root`; parse `DataVersion`
  (default 500 like `NbtUtils.getDataVersion`), `size`, one `palette` or a
  `palettes` list, shared `blocks` entries, and `entities`. Reject malformed
  roots, non-3/non-positive sizes, negative or out-of-range palette indices,
  block positions outside `size`, and wrong tag types with
  `io::ErrorKind::InvalidData`. This is stricter than Java's permissive
  defaults (missing block `Name` becomes air, `SimplePalette.stateFor`
  saturates to air) because a corrupt template should fail loudly rather than
  silently lose blocks.
- Decision: Parse and validate every palette in a `palettes` list, but place
  with palette 0. Vanilla's `getRandomPalette` needs the structure piece's
  position-seeded RNG; that belongs to the start/piece pipeline, not template
  loading. `empty.nbt` and every template used by this slice have one palette.
- Decision: Keep the Java observable block ordering split: NBT-bearing blocks
  last, each group sorted y/x/z. Java also separates special-shape blocks from
  normal full-cube blocks before NBT blocks, but that needs P1.5 block
  behaviour; without it the port cannot classify ladders, torches, stairs, and
  similar. The limitation is explicit in the module docs and `BLOCKERS.md`.
- Decision: `StructurePlaceSettings` ports mirror, four rotations,
  `rotationPivot`, and `ignoreEntities`, and adds `ignoreAir` (default false,
  matching vanilla block placement). `place_into` takes the placement origin
  separately, matching Java's `placeInWorld(level, offset, pos, ...)` `offset`;
  `pos`/palette-seeded processors are not modeled yet.
- Decision: Transform block state `facing`, `axis`, `rotation` (0..15), and
  jigsaw `orientation` with mirror-then-rotation. Java's transforms are
  per-block; `shape`, `hinge`, `type`, and other companions need the P1.5
  block-behaviour table, so they are preserved unchanged for now rather than
  guessed.
- Decision: `place_into` writes only through `FeatureLevel::set_block`, counts
  rejected writes instead of dropping them, and returns block-entity NBT for
  successfully written blocks. Template entities are transformed (Vec3 with
  Java's 1.0-based mirror math, blockPos with the integer transform, pivot,
  then origin) and returned as descriptors; they are not spawned. No block
  entity or entity NBT is invented.
- Why: This is the smallest vertical slice that can load real vanilla
  templates and prove Java-exact coordinate behavior, while keeping every
  unported structure stage loud and documented.
- Consequences: Structure starts, pieces, structure sets, references,
  processor lists, template pools, jigsaw expansion, random palette selection,
  entity spawning, waterlogging, shape updates, and a Java block-for-block
  placement fixture remain open. The focused test embeds four shipped gzip
  files so it does not depend on the out-of-tree decompiled data.

## 2026-11-06 â€” Item component patch representation and network omissions

- Context: `ItemStack` only carried `{item, count}`. Vanilla 1.21.11 stores a
  stack as an item, a count, and a `DataComponentPatch` over the item
  prototype. The client and saved data expect the vanilla network/NBT codecs,
  but only part of the component registry's stream codecs is ported.
- Decision: Model the patch as one insertion-ordered list of
  `(ComponentType, Option<ComponentValue>)` entries. `None` is the vanilla
  `!component` removal marker. This preserves NBT/network byte order while
  allowing component equality to be order-insensitive, like Java's map
  equality. Decode unknown component names and malformed known payloads as
  opaque NBT tags, so an inventory save never silently loses them.
- Decision: Derive component registry ids from the field registration order in
  `net.minecraft.core.component.DataComponents` and pin the ones in use in
  `KNOWN_COMPONENT_TYPES` with a test. Enchantment stream ids come from the
  existing `assets/registries/enchantment.tsv` dump.
- Decision: Network encoding is all-or-nothing per component. Components with
  a ported stream codec are written normally; `attribute_modifiers`,
  `potion_contents`, `tool`, and unknown components are omitted and returned
  in `NetworkEncodingReport` (never written with partial payload bytes). The
  legacy `write_item_stack` wrapper keeps the existing call sites simple.
- Decision: Accept legacy capital `Count` when reading item NBT but always
  write lowercase `count`, matching the 1.21.11 `ItemStack.MAP_CODEC` and
  `ItemStackWithSlot.CODEC`.
- Decision: `max_stack_size` is taken from the component when present, then 1
  for stacks carrying `max_damage` or `damage`, otherwise 64. Per-item
  prototype defaults are not dumped yet, so a plain damageable item without a
  damage component still falls back to 64 until that table exists.
- Why: This keeps saved inventory data lossless and keeps the wire format
  honest while the remaining component codecs and item prototype data are
  ported.
- Consequences: The P1.4 done-when (Java-captured full patch byte parity) is
  still open. `HashedStack` component hashes are parsed but not reconstructed;
  serverbound click carried-item components remain hints, with the session
  cursor authoritative.
## 2026-11-06 â€” P3.10 keeps the vanilla flag word and adds one internal shape bit

- Context: Vanilla `Block.UPDATE_*` has no positive "shape update" bit; shape
  updates run unless `UPDATE_KNOWN_SHAPE` (16) is set. The task also asks for
  named `BLOCK_UPDATE`/`NEIGHBORS`/`SHAPE`/`SUPPRESS_NEIGHBORS` constants and
  for deduplicated neighbour positions that do not lose the strongest flags.
- Decision: `UpdateFlags` exposes the exact vanilla values plus aliases:
  `BLOCK_UPDATE = UPDATE_CLIENTS = 2`, `NEIGHBORS = UPDATE_NEIGHBORS = 1`,
  `SUPPRESS_NEIGHBORS = UPDATE_KNOWN_SHAPE = 16`, and a Rust-only
  `SHAPE = 1 << 10`. `normalize` adds `SHAPE` whenever the caller did not ask to
  suppress shapes, and merged queue entries OR their flags. Thus an explicit
  shape request survives a later suppression request, while a caller that only
  passes vanilla `UPDATE_ALL` still gets shape updates.
- Why: It preserves the vanilla flag values for any future code that passes
  them through while making dedup safe.
- Consequences: The queued flags can contain bit 1024, which is not a vanilla
  bit and must not be written to packet/NBT data.

## 2026-11-06 â€” Neighbour updates are chunk-grouped and insertion-ordered

- Context: Folia runs neighbour updates per region, and a world-global
  hash-map iteration would make the order depend on process hash seeds.
- Decision: `NeighborUpdateQueue` stores one insertion-ordered `Vec` per
  `(chunk_x, chunk_z)` in a `BTreeMap`. Chunks drain in ascending coordinate
  order and positions drain in first-enqueue order. Repeated positions merge
  flags in place. The six directions use
  `NeighborUpdater.UPDATE_ORDER` (`WEST,EAST,DOWN,UP,NORTH,SOUTH`), and the
  chain is capped at vanilla's 512 updates.
- Why: Deterministic, chunk-scoped storage is the smallest structure that can
  later move to a region thread without changing ordering semantics.
- Consequences: A position on a chunk border can be processed before a
  same-source position in a higher chunk, unlike Java's recursive order. The
  queue is world-global for now; `drain_chunk` exists for future region ticks.

## 2026-11-06 â€” Scheduled ticks are world-global but LevelTicks-ordered

- Context: Chunk storage cannot yet persist block ticks, and the port has no
  ticket-owned loaded-chunk container set. A world-level queue is enough to
  make behavior deterministic and testable.
- Decision: `ScheduledTickQueue` keeps entries in a `BTreeMap` keyed by
  `(trigger_tick, priority, sub_tick_order)` and deduplicates by
  `(position, block)`, matching `LevelChunkTicks.ticksPerPosition`. Priority is
  the vanilla `-3..=3` enum; sub-tick order is assigned on insertion. A due
  tick executes only while the block at its position still has the scheduled
  block name, matching `ServerLevel.tickBlock`.
- Consequences: Pending ticks are not saved to chunk NBT and a tick whose
  chunk is no longer cached is dropped. Cancellation between ticks works; a
  callback cannot cancel a sibling already collected for the same tick. These
  gaps are recorded in `BLOCKERS.md`.

## 2026-11-06 â€” Random ticks use a stable per-chunk hash, not Java Level.random

- Context: Java's random ticks draw from the level `RandomSource` sequence,
  which depends on every prior world RNG consumer. Feature/level RNG parity is
  not established yet, so reusing an approximate global sequence would make
  tests order-dependent and still fail to match Java.
- Decision: Sample `randomTickSpeed = 3` positions per 16x16x16 section from
  `XoroshiroRandomSource::new(mix64(seed, chunk_x, chunk_z, tick))`, drawing
  `nextInt(16)` for x, y, and z in that order. Sections are iterated by
  ascending `y`, so selection is independent of map insertion order and
  repeats exactly for the same seed/chunk/tick.
- Why: Deterministic positions make falling/grass tests reproducible and keep
  the door open to swapping in a Java-matching `Level.random` later.
- Consequences: Positions are not a Java parity claim; a Java random-tick
  fixture is still needed. Empty sections are sampled and filtered instead of
  using Folia's per-section ticking-block list, which is simpler but slower.

## 2026-11-06 â€” The P3.10 behaviour table is hard-coded and explicit

- Context: P1.5 block metadata does not exist, so only a small set of blocks
  can implement vanilla neighbour/random behaviour without guessing.
- Decision: A `BlockBehavior` trait with a `BehaviorTable` implements falling
  blocks (sand, red sand, gravel, suspicious variants, concrete powders,
  anvils, dragon egg), `VegetationBlock`-style support from the `dirt` tag and
  farmland, double-plant half checks, and grass spread. `BehaviorTable::with`
  is the callback interface for future blocks. Leaves, crops, and saplings keep
  their states but report unsupported random ticks; leaves/redstone/doors/rails/
  torches report unsupported neighbour reactions, and the tick state records
  every unsupported block a tick actually reached.
- Decision: A falling block moves one block per scheduled tick (delay 2)
  instead of creating a `FallingBlockEntity`. The landing block, its state, and
  the neighbour update below it are correct, but timing differs from vanilla.
- Why: This keeps unsupported behavior visible and gives a tested vertical
  slice without inventing P1.5 flags.
- Consequences: Real leaf decay, crop growth, redstone, fluid flow, pistons,
  block entities, and torch support remain open; see `BLOCKERS.md` and
  `TODO.md` P3.10.
## 2026-11-06 - Aquatic, vine, and mushroom features use the shared block approximations

- Context: The P2.5 aquatic/vine/mushroom batch needs `canSurvive`
  (seagrass/kelp/sea pickle/bamboo/nether plants), vine face attachment, huge
  mushroom grow blocks, and underwater-magma face occlusion. P1.5 block
  behaviour metadata (collision/support/occlusion shapes) is still missing, and
  the existing tree/vegetation modules already use a name/tag approximation.
- Decision: Add `aquatic.rs` block helpers (`face_sturdy`, `can_be_replaced`,
  nether-plant survival, seagrass/kelp/pickle survival) and share them with
  `vines.rs` and `mushroom.rs`. `face_sturdy` treats non-air, non-fluid blocks
  outside `replaceable`/`replaceable_by_trees`/leaves/saplings/flowers as full
  cubes. Underwater magma therefore asks "is the neighbour a full cube" rather
  than reading face-occlusion shapes. The same helpers make huge-fungus
  `canBeReplaced` follow the vanilla `replaceable` block tag.
- Why: One approximation shared by all three modules keeps existing tree and
  vegetation behaviour stable and avoids inventing per-feature shape rules.
  Every unsupported path stays explicit; no block is guessed as placeable when
  the feature needs a shape the port cannot answer.
- Consequences: A non-full-cube neighbour (slab, stair, fence, ...) is treated
  as a full cube until P1.5 lands. `underwater_magma` can therefore accept or
  reject a few positions Java would decide differently. The limitation is
  recorded in `TODO.md` and `BLOCKERS.md`; the focused tests use full blocks.

## 2026-11-06 - Coral picks keep the data-file tag order, not the sorted asset order

- Context: `CoralFeature` picks a random block from `minecraft:coral_blocks`,
  `minecraft:corals`, and `minecraft:wall_corals` with `nextInt(size)`. The
  generated `assets/tags/block.tsv` is sorted for binary-search lookup, but
  Java's `HolderSet.Named` keeps the resource-list order from the tag JSON, and
  the two orders differ (`tube_*` first in the data, `brain_*` first when
  sorted).
- Decision: Keep the three 1.21.11 data-file orders as constants in
  `aquatic.rs` and cross-check their membership against `crate::tags` in a unit
  test. Do not change the shared tag store.
- Why: Coral selection is an RNG-visible operation; using the sorted order
  would silently change which coral every placement chooses. The asset
  extractor and all other tag consumers stay unchanged.
- Consequences: A future data-version bump must update these three constants
  together with the extracted tag assets. The membership test fails loudly if
  the extractor sees a different coral set.

## 2026-11-06 - An absent chunk section reads as air inside the feature region

- Context: `WorldGenRegion::get_block` returned `None` for a section that had
  never been written. Generated chunks always materialize full sections, but
  focused tests and sparse feature worlds do not, so `isEmptyBlock`-style
  checks treated unset blocks as solid.
- Decision: Return a shared static `minecraft:air` state for in-region blocks
  whose section is absent; out-of-region and out-of-build-height positions still
  return `None`. Add a default `FeatureLevel::heightmap` that scans the column
  with the same motion-blocking approximation as the placement world.
- Why: A missing palette section encodes as air in vanilla chunk format, so
  reading it as air is the correct world view. The heightmap default lets the
  new seagrass/kelp/bamboo features query `OCEAN_FLOOR`/`WORLD_SURFACE` without
  widening the trait's implementors.
- Consequences: Sparse test chunks now behave like empty vanilla chunks.
  Heightmap queries outside the write region return the minimum Y (the scan
  skips unreadable positions), which folds into the same behaviour as a feature
  trying to write outside its region.

## 2026-11-06 - `random_boolean_selector` is supported only for empty nested placement

- Context: `mushroom_island_vegetation` (shipped) and `lush_caves_clay`
  (shipped) both use `random_boolean_selector`. Their branches are nested
  placed features; the mushroom branches have `placement: []`, while the
  dripleaf branches still depend on the unported `simple_random_selector`.
- Decision: Port the selector's single `nextBoolean` draw and both branch
  features. Reject the whole selector as explicit `Unsupported` when a branch
  has non-empty placement modifiers or its configured feature is unsupported.
  `mushroom_island_vegetation` therefore executes; `lush_caves_clay` stays
  unsupported with its unported dripleaf branches.
- Why: The shipped mushroom selector needs no placement execution, and silently
  dropping non-empty placement would change positions. Failing closed keeps
  the selector honest for future datapacks.
- Consequences: `supported_feature_count` rises by one for
  `mushroom_island_vegetation`; `lush_caves_clay` remains in
  `unsupported_type_counts`. A later `simple_random_selector` + placement
  implementation should reuse this container path.

## 2026-11-06 - Aquatic generator wiring costs one exact-name block on the fixture

- Context: Wiring the new features into `vegetal_decoration` /
  `underground_decoration` makes seagrass, sea pickles, forest rocks, vines,
  and underwater magma execute in newly generated chunks. The exact-name
  fixture moved from 92.04% (5655/6144) to 92.02% (5654/6144); block-class
  stayed at 98.71% (6065/6144).
- Decision: Keep the wiring. Keep ore/scattered-ore wiring deferred as before.
  Document the one-block delta instead of disabling a faithful feature.
- Why: The lost block is a tall-seagrass pair at `(4, 47..48, -12)`; Rust
  terrain already has water at `y=47` where Java has gravel, so the feature
  places on the port's own water and cannot match Java's block. The class
  fixture gains one match at `y=47` and loses one at `y=48`, netting zero.
  Feature positions remain approximate per the project's existing
  `WorldgenRandom` caveat, so a single block is within the known gap.
- Consequences: The exact-name number in `STATE.md`/`PORT_MAP.md` is 92.02%
  until feature-RNG/terrain parity improves. Do not claim the slice is neutral;
  the delta and its cause are recorded here.

## 2026-11-06 - Entity metadata and living state are separate from physics

- Context: The entity store is a global `HashMap<i32, Entity>` with physics
  and spawn packets. P5.1 needs typed synched data, health, attributes, and
  equipment while P5.2 will eventually move entities into per-chunk/per-region
  storage. Adding every gameplay field to `Entity` would make the physics type
  own state that must later move per region.
- Decision: Keep `Entity` as position/physics and add a separate
  `LivingEntity` record keyed by network id in `entity_metadata.rs`. It owns
  `EntityMetadata`, health, attributes, and equipment. `Entity::living()`
  constructs the record, and `entity.rs` exposes `spawn_living`/`hurt_entity`/
  `tick_living_entities`/`set_entity_equipment` bridges plus cleanup in
  `remove`. Join and player sessions are deliberately not wired.
- Decision: `MetadataValue` is a closed typed enum; its variant selects the
  serializer id, so a caller cannot write a byte under the float accessor.
  Setting an existing index with a different serializer returns
  `MetadataError::SerializerMismatch`. Unsupported serializer ids in an
  incoming packet return an explicit decode error instead of guessing a
  payload length.
- Decision: Dirty tracking uses a `BTreeSet<u8>`; `pack_dirty` returns values
  in ascending index order and clears the set, matching `SynchedEntityData`.
  Defaults are not dirty, so a new record has no dirty packet.
- Decision: Equipment packets use `EquipmentSlot.ordinal()` (0 mainhand, 1
  offhand, 2 feet, 3 legs, 4 chest, 5 head, 6 body, 7 saddle) because
  `ClientboundSetEquipmentPacket` writes the ordinal, not
  `EquipmentSlot.getId()` (whose values are 0, 5, 1, 2, 3, 4, 6, 7).
- Decision: Damage is the raw `LivingEntity.hurtServer` health subset:
  dead/removed entities reject it, negative damage clamps to zero, the hit is
  ignored while `invulnerableTime > invulnerableDuration / 2` and
  `amount <= lastHurt`, otherwise health is clamped to `[0, max_health]`.
  Death sends event `3` immediately, the dirty health/pose metadata packet,
  and `ClientboundSetHealth` for players. `tick` sends event `60` at death
  tick 20; non-players set `removed` but the model does not emit a remove
  packet.
- Decision: Packet builders return the raw payload including the packet id,
  matching existing `entity.rs` helpers, rather than framing the VarInt
  length. Callers use the existing `write_packet` path.
- Why: This keeps the metadata/damage model testable without a session, avoids
  committing the eventual region ownership design to the base physics type,
  and makes serializer/equipment mistakes fail at the model boundary.
- Consequences: Two stores keyed by entity id exist until the region entity
  store lands (`MetadataStore` and `LivingEntityStore`); `spawn` does not
  create a living record, so callers must use `spawn_living`. Initial
  non-default metadata, per-viewer tracking, armor calculation, other
  serializers, and a Java packet fixture remain open (see `BLOCKERS.md`).
## 2026-11-06 - Lighting and physics consume the generated block-behaviour table

- Context: `light.rs` still had a hand-written opacity/emission table, and
  `player_physics.rs` approximated collision/friction with block tags and short
  name lists. The new `block_behaviour` asset provides 29,671 per-state rows.
- Decision: Lighting reads `block_behaviour::light_block` and
  `block_behaviour::light_emission`. Player physics reads `has_collision`,
  `blocks_motion`, `is_solid`, `friction`, `speed_factor`, and `jump_factor`.
  Unknown states resolve through the registry fallback (stone), so they stay
  opaque, non-emitting, and collidable. Collision boxes remain full cubes
  because shape metadata is still missing.
- Why: One generated source of truth replaces two hand-maintained lists, and
  the values now match Java exactly where the old tables guessed (blue ice
  friction 0.989, ice light block 1, glass 0, water/leaves 1, soul sand speed
  0.4, honey jump 0.5).
- Consequences: Speed factor is applied to horizontal velocity after the
  collision move and jump factor multiplies the jump impulse, matching
  `Entity.move` / `LivingEntity.jumpFromGround`. The water/bubble-column
  speed-factor special case is not modeled, thin shapes are still full cubes,
  and the registry lookup per light BFS step is uncached until profiling says
  it matters.
## 2026-11-06 - Pale moss embeds the parsed patch instead of a runtime registry lookup

- Context: `PaleMossDecorator.place` asks the configured-feature registry for
  `minecraft:pale_moss_patch` at generation time. The Rust tree parser runs
  while configured features are still being built, and `WorldGenRegion` has no
  registry handle.
- Decision: Parse trees after vegetation patches and pass the parsed
  `pale_moss_patch` into `parse_tree_config_with_pale_moss`; the decorator owns
  a clone. The public test-only `parse_tree_config` keeps pale moss
  unsupported when no patch is supplied.
- Why: The embedded feature is exactly the shipped patch, needs no global
  lookup, and lets synthetic feature data provide its own patch. It keeps the
  dependency explicit instead of silently skipping the ground pass.
- Consequences: A datapack that replaces `minecraft:pale_moss_patch` without
  reparsing the tree still uses the embedded copy. The generator now parses
  trees after vegetation patches, which is safe for the shipped data because
  no patch branch references a tree.

## 2026-11-06 - Scheduled ticks and block entities are region sinks

- Context: `simple_block.schedule_tick` and beehive occupants need the
  `WorldGenLevel` tick queue and block-entity store. `generator.rs` creates a
  short-lived `WorldGenRegion` per placement and is outside this slice.
- Decision: Add default `FeatureLevel::schedule_tick`/`set_block_entity`
  methods. `WorldGenRegion` implements both through
  `crate::tick::ScheduledTickQueue` and a `Vec<FeatureBlockEntity>` exposed by
  `scheduled_ticks()`/`take_block_entities()`. Ticks are queued from tick zero
  because a generation-only region has no game time.
- Why: The feature layer now produces the real vanilla payloads and tick
  entries through the existing tick API instead of dropping them. The caller
  owns persistence, matching `StructurePlacement::block_entities`.
- Consequences: Generated chunks do not yet persist these sinks because
  `generator.rs` does not drain the region. That remains an explicit gap in
  `TODO.md`, not a silent no-op in the feature code.

## 2026-11-06 - Selector branches that reference placed features stay unsupported

- Context: Vanilla `random_selector`/`simple_random_selector` branches are
  `PlacedFeature` entries, so `oak_checked`/`pale_oak_checked` and
  `warm_ocean_vegetation`'s coral branches must resolve through the placed
  registry. The loader parses configured features before placed features and
  currently resolves branch references only against configured ids.
- Decision: Keep those selectors explicit `Unsupported`. `simple_random_selector`
  itself is ported and resolves inline or configured-feature branches, which
  covers `dripleaf` and `forest_flowers`; the pale garden's top-level selector
  remains a known gap until a two-registry resolver lands.
- Consequences: `pale_garden_vegetation` and `warm_ocean_vegetation` still
  count as unsupported, while the pale-oak tree configs, pale flowers, and both
  dripleaf pools execute. Do not hide the selector gap by treating
  `pale_oak_checked` as a configured feature.
## 2026-11-06 - Surface biome lookup uses `BiomeManager` when the generator seed is known

- Context: `SurfaceSystem.buildSurface` asks `BiomeManager.getBiome` at the
  pre-extension height and at every `updateY`, but `WorldGenerator::overworld`
  does not pass its raw seed into `SurfaceSystem::new`.
- Decision: `surface::build_surface` resolves the world seed from
  `generator::configured()` at generation time and installs
  `biome_zoom::BiomeZoomer`; the zoom is memoized per block Y like Java's
  `Suppliers.memoize`. Explicit `build_surface_with_seed`/
  `top_material_with_seed` entry points let harnesses pin the seed. If no
  configured generator exists, the earlier chunk-local quart lookup remains as
  an explicit fallback.
- Why: Editing `generator.rs` was outside this slice, and the server always
  calls `generator::configure` before streaming chunks, so the production path
  gets vanilla's fuzzy lookup. Direct `WorldGenerator::overworld` harnesses
  (including `worldgen_parity`) run without that global and keep the old
  fallback instead of guessing a seed.
- Consequences: Production generation matches Java's surface biome source;
  differential fixtures must call `*_with_seed` if they want zoom parity before
  `configure`. This is recorded as a residual gap in `TODO.md`/`BLOCKERS.md`.

## 2026-11-06 - Paper `generate-flat-bedrock` is read from `server.properties`

- Context: Paper stores `environment.generate-flat-bedrock` in
  `config/paper-world-defaults.yml`, but the native server already has one
  configuration file and the P2.12 task names `server.properties`.
- Decision: Accept `generate-flat-bedrock` as a `ServerProperties` field, read
  it once through `config::generate_flat_bedrock` (with
  `FOLIA_GENERATE_FLAT_BEDROCK` as an override), and use it while parsing
  `paper:optionally_flat_bedrock_condition_source`. Enabled collapses the
  floor gradient to the lower anchor (one flat layer); disabled keeps the
  vanilla random gradient.
- Why: It keeps the native server to one config file and makes the behavior
  testable without a YAML loader. The parsed condition source is otherwise
  identical to Paper's `OptionallyFlatBedrockConditionSource`.
- Consequences: A real Paper YAML is not consulted yet; a future Paper config
  loader should populate the same boolean. Default `false` preserves the
  captured oracle's random bedrock depths.

## 2026-11-06 - Surface column start is `Heightmap.getHeight + 1`

- Context: `Heightmap.getHeight` returns the highest taken block, while Rust's
  `column_first_air` returns the first available (air) Y. The surface driver
  was adding one more block on top of that value for the biome sample, the
  badlands/frozen-ocean extensions, and the downward walk.
- Decision: Treat `column_first_air` as Java's `i4` / `i5` directly. The
  downward walk starts at that first air block; extension heights receive the
  same value.
- Why: `ChunkAccess.getHeight` is `getFirstAvailable - 1`, so Java's
  `getHeight(...) + 1` is exactly the first available block.
- Consequences: The extra air iteration disappears and the extension boundary
  tests use Java's height. The fixture's exact-name count moves by one block
  (92.02% -> 92.01%, 5654 -> 5653); block-class stays 98.71%.
## 2026-11-06 - Structure-set JSON is embedded until a structure-set asset exists

- Context: The structure start/piece/set slice is limited to
  `worldgen/structure/{mod,start,piece,set}.rs`, one new integration test, and
  the five status documents. The shipped 1.21.11 `worldgen/structure_set`
  documents live only in the out-of-tree decompiled data pack, and the project
  has no structure-set asset under `crates/folia-core/assets`.
- Decision: Embed the 20 shipped documents as JSON string copies (insignificant
  whitespace normalized) in `set.rs` and parse them through the same
  `StructureSetRegistry::from_json_documents` entry point a future datapack or
  asset loader will use. Canonicalize bare file names to `minecraft:<name>` so
  exclusion zones (`minecraft:villages`) resolve against registry entries.
- Why: This keeps the slice testable and preserves the exact shipped data
  without adding an unplanned asset file. The JSON is source data, not game
  logic.
- Consequences: A later asset-extraction task should move the same documents
  under `assets/` and have tests pin their bytes/checksums. The parser and
  registry do not change when that happens.

## 2026-11-06 - Structure-set selection is placement-only for now

- Context: `ChunkGenerator.createStructures` filters structure sets by
  dimension and by whether the set's structures have valid biomes before it
  tests placement. This slice exposes `structure_at_chunk(chunk_x, chunk_z,
  seed)` without a dimension/biome source, because attaching starts to chunk
  generation is explicitly out of scope.
- Decision: `StructureSetRegistry::structure_at_chunk` evaluates every shipped
  set in resource-name order and returns the first placement match and weighted
  selection. Concentric-ring strongholds parse but are skipped by the registry
  method and report `PlacementError::RequiresBiomeSource` when queried directly.
  The returned `SelectedStructure` names the set and structure; callers that
  own a generator must still apply dimension and biome filtering.
- Why: The placement math, exclusion zones, and weighted draw are independent
  of biomes and can be pinned exactly now. Adding a fake biome predicate would
  hide the remaining gap.
- Consequences: `structure_at_chunk` can return a nether/end set for an
  overworld chunk in this slice. Tests call per-set placement when they need
  that distinction. The next generator slice must filter by dimension and
  structure biomes before generating a start.

## 2026-11-06 - Weighted selection uses the first vanilla draw

- Context: `ChunkGenerator.createStructures` removes a failed entry from a
  copy of the weighted list and draws again until one structure generates or
  the list is empty. This slice does not generate starts while selecting.
- Decision: `StructureSet::select_structure` performs exactly one
  `setLargeFeatureSeed(seed, chunkX, chunkZ)` + `nextInt(totalWeight)` draw and
  returns the selected entry. A single-entry set consumes no random values.
- Why: The first draw is the only one that can succeed when biome validation is
  absent; adding retries without knowing which start is valid would produce a
  different answer than Java for no testable benefit.
- Consequences: When start generation and biome checks are wired, the retry
  loop belongs with `tryGenerateStructure`, not in `select_structure`.

## 2026-11-06 - Jigsaw definitions parse but fail loudly

- Context: Villages, ancient cities, trial chambers, bastions, and many other
  shipped structures use `minecraft:jigsaw`. Template pools, junction
  expansion, processors, and terrain adaptation are not ported.
- Decision: Parse jigsaw definitions (including `start_pool` and `size`) and
  return `StructureError::UnsupportedJigsaw` from `generate_start`. The error
  names the structure and start pool and states that expansion is not
  implemented. Never return an empty start for a jigsaw structure.
- Why: An empty start is indistinguishable from a valid structure that happened
  not to generate, which would silently break structure parity.
- Consequences: `structure_at_chunk` can still select a jigsaw structure at a
  placement chunk; generation is the loud failure point. Jigsaw support adds a
  new `StructureKind` implementation rather than changing the error path.

## 2026-11-06 - Structure data markers are returned, not materialized

- Context: `TemplateStructurePiece.postProcess` turns structure-block
  `mode: DATA` markers into chests/loot tables, and `place_into` in the
  template slice already returns block-entity NBT rather than creating block
  entities. Loot tables are not ported.
- Decision: Template pieces skip `minecraft:structure_block` entries
  (`BlockIgnoreProcessor.STRUCTURE_BLOCK`) and return every marker in
  `PiecePlacement::data_markers` with its transformed position and NBT. The
  igloo chest is therefore reported, not given a loot table.
- Why: Returning the marker preserves the observable position/name without
  inventing a block entity or loot seed the rest of the server cannot persist.
- Consequences: The igloo test pins the marker at `(101, 66, 206)` for a fixed
  origin. A later block-entity/loot slice must consume these markers and place
  the chest/loot table exactly like `IglooPieces.handleDataMarker`.

## 2026-11-06 - Igloo templates are supplied through a provider

- Context: The slice may not add structure NBT assets. The igloo is the chosen
  non-jigsaw template structure end to end, and its `top`/`middle`/`bottom`
  templates live in the decompiled data tree.
- Decision: Define `TemplateProvider` and pass it through
  `StructureGenerationContext`. The igloo generator resolves the three
  template ids eagerly and fails with `UnknownTemplate` rather than writing a
  partial start. Tests embed byte-for-byte gzip copies of the three shipped
  templates.
- Why: The provider is the seam the asset/template-manager slice needs anyway,
  and eager resolution prevents a missing template from producing a silent
  empty structure.
- Consequences: The production generator has no template manager yet; the
  caller must supply one. Template loading is already tested independently in
  `tests/structure_template.rs`.
## 2026-11-06 - Block breaking uses wall-clock ticks and a deterministic drop table

- Context: The server loop is packet-driven with a 1 s read timeout, so there
  is no 20 Hz player tick to advance `ServerPlayerGameMode.gameTicks`. The
  behaviour dump also lacks `requiresCorrectToolForDrops`, and item prototypes
  (which carry `minecraft:tool`/`max_damage`) are not dumped yet. There are no
  item entities, and `crate::loot` is still a stub.
- Decision: `DigState` takes caller-supplied ticks; `join.rs` derives them as
  `elapsed / 50 ms` from the start of the play loop. Port vanilla's 0.7 STOP
  threshold and `hasDelayedDestroy` (continue from the original start tick).
  Extract the `requiresCorrectToolForDrops` block set from the 1.21.11
  `Blocks.java` registrations (including legacy-copy/stair inheritance) and
  supplement it with the `needs_*_tool` tags for helper-registered blocks.
  Use the exact `ToolMaterial` speeds/durabilities as the built-in tool table
  when a stack has no explicit `minecraft:tool` component. Use a
  deterministic common-block drop table whose vanilla count ranges are
  collapsed to representative values, and expose a `LootProvider` hook for the
  future loot module. Capture drops into the player inventory because item
  entities do not exist yet. A lone `STOP_DESTROY_BLOCK` (no START) starts a
  delayed destroy rather than insta-mining; this keeps the captured join
  fixture/probe clients working while still charging the computed break time.
- Why: Behaviour stays tied to the Java formulas and the block tags, while the
  missing prototype/loot/item-entity infrastructure does not force fake
  instant breaks or hard-coded guessed IDs. The 0.7 threshold and delayed path
  are vanilla's own client/server reconciliation.
- Consequences: Break timing is accurate to the tick for a live session but
  quantized by the 1 s idle timeout for delayed destroys. Drop counts are not
  yet vanilla RNG (redstone always 4, lapis always 6, ...), Fortune/Silk Touch
  and Unbreaking are not applied, and dropped items appear in the inventory
  instead of the world. `max_damage` is written onto a stack on first wear so
  the client durability bar and playerdata stay self-describing until item
  prototypes are dumped. A Java break-time/drop fixture is still needed before
  claiming parity.
## 2026-11-06 - Fluid flow uses the vanilla amount/falling model over block levels

- Context: The world model stores liquids as `minecraft:water`/`lava` with the
  vanilla `level` property, not as separate source/flowing fluid registry
  entries. Vanilla `FlowingFluid` operates on `FluidState` amount (1..8) plus a
  falling flag, and `LiquidBlock.getFluidState` maps block levels 1..7 to
  amounts 7..1 and level 8+ to falling amount 8.
- Decision: Add `fluid::FluidState { kind, amount, falling }` and convert at
  the block boundary. Level 0 is a source, 1..7 are flowing, 8+ are falling;
  waterlogged blocks read as full water sources. Scheduled ticks are keyed by
  the block name so `ScheduledTickQueue`'s existing `(pos, block)` check keeps
  working.
- Why: This preserves the vanilla algorithm (drop-off, falling priority, slope
  search, two-source conversion) without adding fluid registry types to the
  block model or changing `world.rs`.
- Consequences: Flowing amount 8 non-falling collapses to level 0/source, as it
  does in vanilla's `LiquidBlock` cache. Block-destruction drops and
  water/lava quenching are not modeled; they are explicit gaps.

## 2026-11-06 - Redstone updates ride the existing neighbour queue

- Context: Wire propagation is a graph update; a direct recursive evaluator
  would either recurse deeply or use an ad-hoc queue, and the task forbids
  changing `world.rs` storage hooks.
- Decision: `redstone::WireBehavior` recalculates a wire only when the normal
  `tick` neighbour queue delivers a `neighbor_changed`/`on_place` callback.
  `TickWorld::set_block` appends the six neighbours, and the queue's position
  dedup plus `(target == current) -> no write` termination makes chains
  deterministic without a second scheduler. `SignalReader` is a read-only view
  over `WorldStorage`; `should_signal = false` reproduces
  `RedStoneWireBlock.getBlockSignal`.
- Why: It reuses the tested ordering/dedup path and keeps block storage
  untouched. Identical worlds then produce identical `last_neighbor_updates`
  sequences, which the integration test pins.
- Consequences: Wire connection-shape properties are not recomputed yet, and
  only power is modeled. Repeaters/comparators/torches/pistons still need
  their own behaviour hooks.
## 2026-11-06 - Loot and recipe data use one normalized catalog per kind

- Context: 1.21.11 ships 1,324 loot-table JSON files and 1,470 recipe JSON
  files. The pretty-printed tree is 1.18 MB + 384 KB; a naive copy would keep
  per-file formatting and unversioned layout.
- Decision: `scripts/extract_loot_recipes.ps1` emits one compact, sorted
  catalog per kind with a `counts` index (total + per-type). Identifier fields
  (entry names, tags, function/condition types, enchantments, potions, recipe
  ingredients/results) are namespaced. Array order is preserved; object keys
  are sorted ordinally. The extractor uses the framework JSON serializer
  because PowerShell 5.1's `ConvertFrom-Json` rejects `anvil.json` (keys `I`
  and `i`).
- Why: The catalog is smaller than the source tree, deterministic across
  runs/platforms, and a single `include_str!` per kind avoids 2,794 tracked
  files. A per-kind index makes truncation obvious.
- Consequences: `src/loot.rs` parses catalogs rather than individual files.
  Regeneration requires `.\scripts\extract_loot_recipes.ps1` or
  `tests/assets.rs` fails on the pinned byte/hash values.

## 2026-11-06 - Unknown loot payloads fail closed and are reported

- Context: Vanilla has 20 loot-function types and 15 condition types; several
  need registries or world state this port does not model yet
  (`enchant_with_levels`, `copy_components`, `copy_state`, `set_stew_effect`,
  `set_instrument`, `entity_properties`, `location_check`,
  `damage_source_properties`).
- Decision: Parse every shipped type into a typed variant and preserve unknown
  functions/conditions/entries as `Opaque`. When an unsupported payload is
  reached, do not silently match: conditions return false and
  `LootRoll::report` names the condition/function/entry/provider. A separate
  `approximations` set marks the one deliberately approximate path
  (`enchant_randomly` picks a level-1 enchantment from the resolved options
  because min/max levels are not in the asset).
- Why: A skipped condition that returned true would inflate drops; returning
  false plus a report keeps the failure visible and testable.
- Consequences: Unsupported conditions can suppress a drop, but never invent
  one. Closing the gaps means adding the missing registries/context and
  promoting the report entries to exact execution.

## 2026-11-06 - Asset manifest hashes canonical LF text

- Context: Windows checkouts with `core.autocrlf=true` convert text assets to
  CRLF, while the recorded manifest mixed LF and CRLF hashes (the dynamic
  registry tables were generated on Windows).
- Decision: `tests/assets.rs` folds CRLF to LF before hashing text assets and
  the manifest records canonical LF lengths/hashes. Binary assets
  (`mth_sin.bin`, `block_state_behaviour.tsv.gz`) are hashed verbatim.
- Why: The guard must fail on content changes, not on the checkout platform.
  Regenerating the affected manifest values removed the last mixed line-ending
  entries.
- Consequences: A genuine CRLF-only content edit is not detected; any other
  byte change is. Regenerate the manifest with LF output when adding assets.
## 2026-11-06 - Ore wiring uses a lazy placement stream and parameter-order feature indices

- Context: The earlier ore wiring attempt lowered exact-name parity from 92.09%
  to 89.06%, which was attributed to approximate feature RNG. The actual causes
  were narrower: `apply_decorations` collected every placement position before
  running the configured feature, and `FeatureData::step_feature_order` built
  `FeatureSorter` indices from the biome registry order.
- Decision: In `worldgen/generator.rs`, walk placement modifiers depth-first
  and run the configured feature at each leaf before continuing the walk, so
  feature RNG is consumed between placement attempts exactly like vanilla's
  lazy `Stream.flatMap` (`PlacedFeature.placeWithContext`).
- Decision: Build the per-step feature order with vanilla's
  `FeatureSorter.buildFeaturesPerStep` over `MultiNoiseBiomeSource`'s
  `possibleBiomes()` order (`assets/biome_params.txt`), including edges across
  step boundaries. This is a local copy because `feature/mod.rs` is outside
  this slice's file scope; `FeatureData::step_feature_order` still returns the
  registry-order variant for its existing tests and should adopt the parameter
  order later so the two cannot drift.
- Decision: Enable `Decoration::UndergroundOres` by default but run only
  `minecraft:ore`/`minecraft:scattered_ore` types there. The same vanilla step
  also holds `disk_*` and `underwater_magma`, which are supported features but
  need separate surface/cave wiring; leaving them out keeps the change scoped
  to ores instead of silently changing disk placement.
- Decision: Run `OreVeinifier` as a solid-position block-state filler before
  surface rules and carvers, matching `NoiseChunk`'s filler chain. The vein
  constants are the Java `float` values promoted to `double` so boundary
  comparisons match.
- Why: These three changes turn the regression into a large gain and keep the
  RNG/order semantics in the generator where the parity test exercises them.
- Consequences: Exact-name parity is 95.80% (5886/6144) and block-class 98.75%
  (6067/6144); the vein-only, no-ore-step baseline is 92.37% (5675/6144).
  Remaining gaps: vein noise should be sampled through `NoiseChunk`'s
  interpolation grid, the active biome set should span the 3x3 chunk region,
  features should be allowed to write into that region, and 7 of the fixture's
  33 Java ore blocks still differ. Do not claim block-for-block ore parity.
## 2026-11-06 - Plugin player state mirrors playerdata, not the live session

- Context: `join.rs` owns each network session's `PlayerSession` on its own
  thread and exposes no registry. The bridge runs on the Rust main thread and
  must answer Bukkit `Player` calls without editing core files outside this
  slice.
- Decision: `BridgeState` keeps one `PlayerSession` per UUID, seeds it from
  `playerdata` on join, and persists every plugin mutation immediately. The
  bridge is authoritative for the next login (and for offline players). Only
  `setGameMode` is forwarded to the live session through the existing
  `execute_console_command("gamemode ...")` channel, because that path already
  exists.
- Why: Persisting through the real playerdata model is a genuine state change
  and keeps the bridge inside its allowed file boundary. Inventing a second
  live-session registry would require core changes and duplicate authority.
- Consequences: Live movement, fall damage, inventory clicks, and the 5 s
  autosave can overwrite bridge-side health/food/inventory/teleport changes
  until `join.rs` exposes a shared session handle. Teleports and messages are
  therefore partial: teleport persists and returns true, `sendMessage` logs to
  the server console, but neither reaches the connected client yet. These gaps
  are recorded in `TODO.md` (P9.3) and in the bridge self-test output.

## 2026-11-06 - Plugin world reads use stored/cached chunks only

- Context: `WorldStorage` can load saved chunks but generating on demand needs
  the chunk-status pipeline, which is still owned by the join streamer.
- Decision: `World.getBlockAt`, `getHighestBlockYAt`, biome, and light getters
  read the process-wide overworld storage and never generate. A missing chunk
  reads as air, the highest block returns the world minimum, and writes to a
  missing chunk return false.
- Why: Generating from a JNI callback would bypass the chunk-status pipeline
  and risk blocking plugin code on a full generation batch.
- Consequences: Plugins can read and edit any stored or currently cached
  column (the spawn area after a join), but cannot explore new terrain through
  the Bukkit API until ticket/generation APIs land. Stored chunks without light
  data are recomputed read-only for the light getters.

## 2026-11-06 - Bridge item meta is the component subset Rust already stores

- Context: The Rust item model has custom name, lore, damage, unbreakable,
  enchantments, attribute modifiers, containers, potions, food, and tool data.
  The Bukkit API needs `ItemStack`/`ItemMeta` objects before the JVM can expose
  any of it, and the plain API class delegates to a CraftBukkit object that does
  not exist here.
- Decision: Provide `BridgeItemStack` (a subclass with its own fields) and a
  `BridgeItemMeta` proxy. Round-trip type, count, damage, custom name, lore, and
  unbreakable through the Rust component model. Keep enchantments, item flags,
  and custom model data on the proxy for the object's lifetime but do not persist
  them yet.
- Decision: Install a `RegistryAccess` service provider so
  `Material.asItemType()` and `new ItemStack(Material, count)` work without
  CraftBukkit, plus an `InternalAPIBridge` service provider so `Biome` can
  initialize.
- Why: Without these services, a plugin fails before reading a single block:
  modern `ItemStack` construction throws `No RegistryAccess implementation
  found`, and `Biome` initialization throws from its static initializer.
- Consequences: Standard item creation and the common meta operations work.
  Enchantments and persistent data containers remain explicit gaps (P9.8).
  `PlayerInventory` covers storage slots 0-35; armor and off-hand slots read as
  air and ignore writes.

## 2026-11-06 - Block/place/interact events are host-callable, not yet live-wired

- Context: `join.rs` performs block break/place and item use directly through
  `block_update::set_block`; it has no plugin callback hook and is outside this
  slice's allowed files.
- Decision: Implement real Bukkit event construction, listener dispatch, and
  cancellation in the bridge, expose `PluginRuntime::fire_block_break`,
  `fire_block_place`, and `fire_player_interact` for the Rust host, and add a
  gated `FOLIA_BRIDGE_SELFTEST=1` end-to-end check. Do not fake a live hook by
  watching broadcast packets.
- Why: The event objects and cancellation semantics are the reusable part; a
  packet watcher would attribute someone else's edit to the wrong player.
- Consequences: Plugins that call the `Bridge.fire*` entry points (or the Rust
  host once join handlers are wired) observe and cancel these events. Vanilla
  break/place/interact still bypass the event bus until a follow-up changes
  `join.rs`. `BlockBreakEvent`, `BlockPlaceEvent`, and `PlayerInteractEvent` are
  covered; the rest of P9.2 remains open.

## 2026-11-06 - Crafting lookup crops the grid and prefers the last sorted recipe id

- Context: 1.21.11 `CraftingInput.ofPositioned` trims empty border rows/columns
  before `ShapedRecipePattern.matches` runs, and `RecipeManager.getRecipeFor`
  takes `list.getLast()` from a resource-location sorted stream. Older versions
  scanned offsets directly and returned the first match.
- Decision: `CraftingGrid` exposes its tight bounding box; shaped matching crops
  the input logically, compares the normalized 1-3 wide/tall pattern, then tries
  the mirror before the plain pattern. Every lookup (`CraftingGrid::craft`,
  furnace `find_recipe`, stonecutting, smithing) scans `RecipeCatalog` in
  `BTreeMap` order and keeps the last match. Shapeless matching uses one unit
  per occupied slot (not stack count) and solves the ingredient/item bipartite
  matching like `StackedContents.tryPick`, so duplicate ingredients need
  duplicate items and overlapping tags can fall back to a later assignment.
- Why: This reproduces vanilla's offset/translation behavior and its recipe
  conflict priority without maintaining a separate offset loop, and it matches
  the shipped sorted catalog layout.
- Consequences: A future recipe-reload path must preserve resource-location
  ordering, and a menu's cached `lastRecipe` fast path is still unported. A
  malformed shaped pattern is an explicit `InvalidPattern` error instead of a
  silent non-match.

## 2026-11-06 - Furnace XP is accumulated exactly; campfire is a single-slot model

- Context: `AbstractFurnaceBlockEntity` stores per-recipe counters and converts
  them to XP when a player extracts the result (`awardUsedRecipes` uses vanilla's
  experience rounding). Campfires have four independent cooking slots and a lit
  block state, while the current block-entity layer has no slots or campfire
  state yet.
- Decision: `FurnaceState` accumulates the recipe `experience` exactly as each
  item completes and `extract` returns that float with the output. Furnace,
  blast furnace, and smoker share one tick model with the vanilla fuel table and
  recipe cook times; `FurnaceKind::Campfire` uses the same model with a single
  input/result pair, no fuel, and the 600-tick campfire recipes.
- Why: Exact accumulation is deterministic and keeps the future menu packet
  fields (`litTimeRemaining`, `litTotalTime`, `cookingTimer`,
  `cookingTotalTime`) directly testable without a player/RNG dependency.
- Consequences: XP differs from vanilla only in the final random rounding at
  extraction, and campfire slot count/lit state must be widened when the
  campfire block entity lands. Furnace fuel crafting remainders are only handled
  for lava buckets (to empty buckets).
## 2026-11-06 â€” Item prototypes are a Java dump, not a hand-written table

- Context: Stack limits, durability, tool rules, food values, equippable slots,
  enchantability, repair ingredients, fire resistance, and rarity were guessed
  from item names or the presence of a damage component. The decompiled
  `Items.java` builds them through `ToolMaterial`, `ArmorMaterial`, `Foods`, and
  `Consumables`, so re-deriving the table in Rust would duplicate a lot of
  Mojang data and drift.
- Decision: `scripts/extract_item_prototypes.ps1` boots the local 1.21.11
  server jar (`Bootstrap.bootStrap`), reads `Item.components()` for every
  non-air registry item, and emits a compact JSON catalog keyed by namespaced
  id. Only non-default fields are stored, so a plain block item is `{}` and the
  Rust loader applies the explicit stack-64/no-durability fallback. Tool tier is
  not a 1.21.11 component; the extractor derives it from the repair tag
  (`*_tool_materials`) or the `incorrect_for_*_tool` rule and stores the
  canonical `wood`/`stone`/`copper`/`iron`/`gold`/`diamond`/`netherite` name.
  Attack damage/speed come from the prototype attribute modifiers and are cast
  to `f32` because the vanilla tool builder creates them from float constants.
- Decision: The loader rejects unknown fields, out-of-range values, damageable
  entries whose stack size is not 1, and a `counts` index that disagrees with
  the item entries. The extractor sorts item and object keys ordinally, writes
  one trailing LF, and supports `-Check` for byte comparison.
- Why: The data is exact and reproducible from the same jar, the asset stays
  small (79,369 bytes for 1,504 items), and malformed generator output fails at
  load instead of silently changing stack or durability behavior.
- Consequences: Regeneration needs Java 21 and the local server jar/dependency
  tree; without them the checked-in asset and its pinned FNV hash still load.
  `defaults_for` can only emit components `item_components::ComponentValue`
  models, so equippable/enchantability/repairable/fire resistance remain on the
  prototype rather than being invented as opaque tags. `block_breaking.rs` is
  not wired to the new catalog yet; it keeps its built-in `ToolMaterial` table
  until a follow-up owns that file.
## 2026-11-06 - Structure processor/pool catalogs are compact direct-codec data

- Context: `worldgen/processor_list` and `worldgen/template_pool` are registry
  data in 1.21.11, and the project has no datapack loader yet. Copying the
  decompiled JSON verbatim would be large, whitespace-dependent, and would make
  byte-identity regeneration hard to check.
- Decision: Add `scripts/extract_structure_data.ps1`, which normalizes every
  identifier, sorts object keys, removes insignificant whitespace, appends one
  newline, and writes `assets/structure_processors.json` and
  `assets/template_pools.json`. Processor lists keep the direct-codec
  `{ "processors": [...] }` shape so a datapack document can be passed to the
  same parser; template pools keep `fallback`/`elements`. Counts include the
  processor-list total/instances, pool/entry counts, every recursively nested
  element node, and Java's expanded template-slot count (`sum(weight)`).
- Why: This matches the existing loot/recipe catalog conventions, gives the
  Rust loaders one deterministic input, and lets tests compare the asset against
  a canonical JSON serialization. Running the extractor twice produces
  byte-identical files.
- Consequences: The assets are generated artifacts and must be regenerated from
  decompiled data when the target version changes. They are not in
  `tests/assets.rs` or `ASSETS.md` because those files are outside this slice;
  the focused `tests/structure_processors.rs` test checks their canonical shape,
  counts, and every parser family instead.

## 2026-11-06 - Processors run on raw states, then placement rotates the result

- Context: `StructureTemplate.placeInWorld` calls `processBlockInfos` on the raw
  palette blocks and only afterwards applies
  `state.mirror(settings.getMirror()).rotate(settings.getRotation())` while
  writing. Applying processors to already-rotated states would change rule and
  block-ignore predicates for rotated structures.
- Decision: `processor::template_block_infos` transforms positions but keeps
  template states raw. Every `StructureProcessor::process_block` sees Java's
  two arguments (`original` raw block and `current` absolute processed block).
  The pool placement path applies `transform_state` only after the processor
  chain and finalize pass. Per-block random values use
  `LegacyRandomSource::new(Mth.getSeed(absolutePos))`; `capped` uses
  `LegacyRandomSource::new(levelSeed).forkPositional().at(offset)` and Java's
  Fisher-Yates index shuffle.
- Why: This preserves the Java predicate/ordering semantics, including the
  ordered first-match `rule` processor, `capped`'s delegate calls, and the fact
  that `block_age` constructs both stair arrays (consuming both random
  facing/half draws) before choosing a mossy or normal result.
- Consequences: Rule tests and block-entity modifiers can be differential-tested
  on small synthetic templates. The first palette is still used for placement;
  Java's random multi-palette selection remains a documented `template.rs` gap.

## 2026-11-06 - Pool expansion places one element, not a recursive jigsaw

- Context: Full `JigsawPlacement.Placer` needs junction pieces, voxel free-space
  tracking, expansion-hack height selection, max-distance/height checks, and
  biome/dimension filtering. The current slice adds pools/processors without
  changing the generator's chunk-status pipeline.
- Decision: `PoolCatalog::expand_start` mirrors the opening of
  `JigsawPlacement.addPieces`: consume one `Rotation.getRandom` draw, then one
  `StructureTemplatePool.getRandomTemplate` draw, build a
  `TemplateStructurePiece`, and expose `PoolExpansion::place` for one element.
  Empty starts return `None`; `feature_pool_element` placement fails explicitly
  with `UnsupportedFeatureElement`. Recursive jigsaw connection is left as an
  explicit later slice rather than silently approximated.
- Why: This gives a real vertical slice through template pools, processors,
  bounds, data markers, and block writing while keeping the unsupported
  recursive path loud and testable.
- Consequences: A start pool element can be resolved and placed deterministically
  for a fixed seed, but jigsaw structures still return `UnsupportedJigsaw` from
  `StructureDefinition::generate_start`. `P2.9` is now partially complete.
## 2026-11-06 - Redstone components reuse the tick queues and keep comparator output on TickState

- Context: The first redstone slice covered wire, levers, and buttons only.
  Repeaters, comparators, observers, torches, pressure plates, and tripwire need
  scheduled ticks and entity-derived inputs, but the port has no block entities,
  no entity store on `WorldStorage`, and cannot edit `world.rs`.
- Decision: Repeaters use vanilla's `facing` convention (input side) and emit
  strong power toward `facing.opposite()`; locking is evaluated live from
  `getAlternateSignal`, while the `locked` property is only a cached shape/client
  value. Comparator output is kept in `TickState::comparator_outputs` keyed by
  position, matching `ComparatorBlockEntity` behaviour until block entities and
  NBT persistence land.
- Decision: Container inputs go through `tick::ContainerSignalSource` and the
  per-world `TickState::containers` registry. A chest/furnace implements the
  trait, registers at its block position, and calls
  `redstone::notify_container_changed` when its contents change. The comparator
  never needs to know inventory layouts, and tests can register a fake container.
- Decision: Pressure plates and tripwire take already-filtered
  `PlateOccupancy { entities, living }` counts from the entity layer instead of
  scanning an entity collection that `TickState` does not have. The block
  behaviours own the vanilla signal/schedule/release logic; the entity layer
  owns spectator/trigger-ignoring filtering.
- Decision: Redstone-torch burn-out uses vanilla's
  `RedstoneTorchBlock` constants: eight off toggles within 60 ticks, then a
  160-tick restart delay. The task brief's â€œ60 toggles in 60 ticksâ€ is a
  shorthand; the decompiled source is authoritative. Toggles are kept in
  `TickState::torch_toggles`.
- Decision: Wire connection shape is recomputed from `getConnectingSide` and
  `getConnectionState`, including the dot/cross stub rules, on placement and on
  neighbour changes. `isFaceSturdy` uses the shared full-cube solidity
  approximation until P1.5 shapes exist.
- Why: The existing neighbour queue plus scheduled queue already provide
  deterministic ordering and dedup, so a second redstone scheduler would
  duplicate tested behaviour. Per-world tick state keeps tests isolated and
  leaves comparator/torch/pressure state reusable when block entities arrive.
- Consequences: Comparator outputs, plate occupancy, torch toggles, and pending
  scheduled ticks are not persisted to chunk NBT; a restart resets them.
  Tripwire hooks, pistons, lamps, target blocks, and item-frame/analog block
  inputs remain explicit gaps. No Java differential redstone trace exists yet.
## 2026-11-06 - Lava/water contact follows the decompiled 1.21.11 branches

- Context: The slice brief said "source lava + water above -> stone", but the
  decompiled 1.21.11 `LiquidBlock.shouldSpreadLiquid` checks the block above
  (reversed `POSSIBLE_FLOW_DIRECTIONS` starts at `Direction.DOWN`) and converts
  a lava *source* to obsidian and a *flowing* lava block to cobblestone. Only
  `LavaFluid.spreadTo` produces stone, and only when lava flows downward into a
  water `LiquidBlock`.
- Decision: Follow the decompiled Java exactly: source lava with water above or
  on a horizontal face -> obsidian; flowing lava with water above or on a
  horizontal face -> cobblestone; lava flowing down into water -> stone; water
  below a lava source is not a contact face (it is handled by the downward
  flow branch). Diagonal water never converts. The blue-ice-over-soul-soil
  basalt branch is included.
- Why: AGENTS.md makes the decompiled Folia source the specification for
  vanilla behaviour, and the objective also says "per Java's exact branch".
- Consequences: `tests/fluid_interactions.rs` pins all of these outcomes at
  exact coordinates. If the brief intended a different rule, it conflicts with
  the 1.21.11 source and should be raised as a spec change, not implemented as
  a divergence.

## 2026-11-06 - Fire and bubble-column behaviour sit in fluid.rs behind a table

- Context: This slice may not edit `tick.rs`, whose `behavior_for` dispatches
  only fluids, redstone, falling blocks, plants, and grass. `minecraft:fire`,
  `minecraft:soul_sand`, and `minecraft:magma_block` therefore resolve to the
  no-op behaviour under the production `tick_world`.
- Decision: Implement `FireBehavior` and `interaction_table()` in `fluid.rs`,
  and use the table in tests/future callers. Lava fire ignition is live under
  the default table because lava itself is a fluid and routes to
  `FluidBehavior::random_tick`. Bubble columns are created without soul
  sand/magma callbacks: a water source over soul sand or magma performs
  `BubbleColumnBlock.updateColumn` on its own normal 5-tick fluid tick. That is
  effect-equivalent to `SoulSandBlock`/`MagmaBlock`'s 20-tick scheduled check,
  but earlier by up to 15 ticks.
- Why: It keeps the allowed file scope while making the algorithms real and
  testable instead of adding gameplay logic to `tick.rs` or faking callbacks.
- Consequences: Production `tick_world` ages fire only after a one-line
  registration in `tick.rs` (or a caller that uses `interaction_table`); fire
  blocks placed by lava will persist at age 0 until then. Bubble columns work
  under the default table but can appear sooner than Java's 20-tick check.
  Both gaps are recorded in `BLOCKERS.md`.

## 2026-11-06 - Fire flammability uses the bootstrapped FireBlock odds

- Context: The generated per-state `block_state_behaviour` table carries
  collision/light/tick data but not `BlockBehaviour.Properties.ignitedByLava`.
  `FireBlock.canBurn` and `LavaFluid.isFlammable` need that flag.
- Decision: Embed the exact 1.21.11 `FireBlock.bootStrap` `(block, ignite,
  burn)` registration list in `fluid.rs` and use `ignite > 0` as the
  `ignitedByLava` predicate. Waterlogged states report zero odds, matching
  `FireBlock.getIgniteOdds`/`getBurnOdds`.
- Why: The list is the authoritative source for every block FireBlock knows
  about, is fully deterministic, and can be replaced by a dumped per-state
  column later without changing the algorithms.
- Consequences: Datapack blocks added after 1.21.11 or blocks with
  `ignitedByLava` but no FireBlock registration would be treated as
  non-flammable until the behaviour dump grows that column.

## 2026-11-06 - Fire environment inputs are fixed defaults in the tick slice

- Context: `TickWorld` has no weather, difficulty, player-tracking, or
  `EnvironmentAttributes` state, and the slice may not edit `tick.rs`/`world.rs`.
- Decision: `canSpreadFireAround` is treated as the unlimited `-1` game rule;
  rain/extinguish checks are skipped; increased burnout is off; and the spread
  formula uses `server.properties`' default easy difficulty (id 1). Fire delay
  uses Paper's default `fireTickDelay = 1` plus `nextInt(10)`. TNT burnout
  consumes the same random draws but leaves the TNT block in place because
  primed-TNT entities do not exist yet.
- Why: These are the vanilla defaults for a fresh server, and inventing
  per-world state outside the allowed files would be a fake parity claim.
- Consequences: Fire behaves correctly for the default overworld rules, but
  rain, difficulty changes, `fireSpreadRadiusAroundPlayer`, TNT priming, and
  the increased-burnout attribute remain explicit gaps.

## 2026-11-06 - Fluid destruction replaces blocks but spawns no drops

- Context: `FlowingFluid.spreadTo` calls `beforeDestroyingBlock`, which drops
  the replaced block's loot unless it is a `LiquidBlockContainer`. This port has
  no item-entity/loot path in the allowed files.
- Decision: `destroyed_by_fluid` classifies replaceable container-less blocks
  (plants, torches, fire, ...) and `spread_to` still performs the replacement;
  `beforeDestroyingBlock` is an explicit documented no-op. Waterlogged
  containers are waterlogged instead of destroyed, and `LiquidBlock`s use the
  quench rules.
- Why: The observable block change (fluid replaces the plant/fire) is correct,
  and a fake item drop would be worse than an explicitly documented missing
  drop.
- Consequences: No item entities appear when a fluid breaks a plant/torch/fire.
  The block state and fluid flow are correct and tested; loot drops wait on the
  item-entity pipeline.
## 2026-11-06 - Dimension router/surface data lives in the dimension modules

- Context: `assets/worldgen.json` carries only the Overworld `noise_settings`,
  density-function graph, and noises. The Nether/End settings exist in the
  decompiled data tree, but the slice's allowed files exclude new assets and
  `assets/ASSETS.md`; the existing asset already contains every Nether normal
  noise (`patch`, `netherrack`, `soul_sand_layer`, ...).
- Decision: Encode the compact Nether router/surface sequence in
  `worldgen/nether.rs` and the End island field/router in `worldgen/end.rs`.
  Reuse `WorldgenData::overworld(seed)` for the normal-noise instances and
  `generator::sample_grid` for interpolation, and add
  `generate_dimension(dimension, x, z, seed)` in `generator.rs` rather than
  changing the Overworld generator or `worldgen.json`.
- Why: Keeps the Overworld parity fixture untouched, avoids a duplicate asset
  manifest entry, and still evaluates the shipped density functions.
- Consequences: A future dimension-data extraction can move these constants
  into a compact asset without changing the generators. The Nether/End routers
  are not loaded from `worldgen.json` today.

## 2026-11-06 - Legacy dimension noise uses the positional seed path

- Context: Both shipped dimension types set `legacy_random_source: true`.
  Java then builds `BlendedNoise` from a sequential `LegacyRandomSource(seed)`
  and the Nether temperature/vegetation noises from
  `NormalNoise.createLegacyNetherBiome`. The public Rust noise API exposes the
  positional Xoroshiro path (`BlendedNoise::create`,
  `NormalNoise::create_legacy_seed`), and `noise.rs` is outside this slice's
  allowed files.
- Decision: Seed `BlendedNoise::create` from `XoroshiroRandomSource(seed)` and
  build temperature/vegetation with `NormalNoise::create_legacy_seed(seed / seed
  + 1, -7, [1, 1])`. Implement the End island field exactly, because its
  `LegacyRandomSource` + `SimplexNoise` construction is available.
- Why: This is deterministic, uses the world seed, and keeps the large-scale
  Nether/End layout correct without adding an approximate sequential-noise port.
- Consequences: Nether/End terrain is structurally correct but not claimed
  block-identical to Java. Exact parity needs a public sequential
  legacy-blended-noise constructor (or moving that construction into
  `noise.rs`), then a Java dimension fixture.

## 2026-11-06 - The Nether uses its own FeatureSorter biome order

- Context: `FeatureData::step_feature_order` derives the biome iteration order
  from `assets/biome_params.txt`, which contains only Overworld biomes. The
  Nether's placed features therefore had no `featureIndex` entries and never
  ran.
- Decision: Build a Nether-specific feature order in `worldgen/nether.rs` from
  the shipped `MultiNoiseBiomeSourceParameterList.Preset.NETHER` biome order
  (wastes, soul sand valley, crimson forest, warped forest, basalt deltas) and
  reuse the same `FeatureSorter` dependency algorithm.
- Why: `WorldgenRandom.setFeatureSeed` consumes a global feature index derived
  from that preset order; using the registry or Overworld order would produce
  the wrong positions.
- Consequences: Supported Nether decoration (quartz, magma, glowstone,
  vegetation) now places in generated chunks. A shared dimension-aware
  `step_feature_order` would remove the duplicate algorithm when dimensions are
  wired into `world.rs`.
## 2026-11-06 - Block shapes are an oracle-generated AABB table

- Context: P1.5 asked for per-state collision/outline shapes. A hand-written
  property-family implementation (slab/stairs/fence/wall/...) would only cover a
  subset of the 1,166 blocks and would silently approximate chests, cauldrons,
  dripstone, scaffolding, shulker boxes, and every shape that is not a simple
  cube family.
- Decision: `scripts/extract_block_shapes.ps1` generates a temporary Java
  dumper, boots the local Folia 1.21.11 server classes, and writes
  `id<TAB>collision<TAB>outline` for all 29,671 states to
  `assets/block_shapes.tsv.gz`. Each shape is the `getCollisionShape` and
  `getShape` AABB list with `CollisionContext.empty()`; boxes are sorted
  lexicographically. The Rust server embeds and parses the table once and never
  starts a JVM for shapes.
- Decision: States whose block has `hasCollision == false` (air, fluids,
  plants, torches, signs, ...) store empty collision and outline lists. Vanilla
  still has a small selection shape for some of these, but this physics-focused
  slice deliberately exposes no outline for passable states.
- Decision: Unknown or out-of-range state ids fall back to a full cube, matching
  the previous conservative behaviour. `has_collision` is shape-based, so a
  snow layer with `layers=1` reports false even though the block's
  `hasCollision` flag is true.
- Decision: Player and entity collision iterate a block cursor widened by one
  block on every side, like `BlockCollisions`, so a fence/wall collision box
  that extends above its block is still found. The player keeps its
  axis-separated swept AABB move and `0.6` step height; entities use the same
  AABB lists but do not gain step-up.
- Why: The oracle table is exact, deterministic, reproducible from the local
  reference tree, and makes every block shape available without hand-porting
  1,166 subclasses. Embedding keeps the shipped server Java-free.
- Consequences: Regenerating the asset requires the local patched Folia jar (or
  Paperclip + `cache`/`libraries`), the same local-reference constraint as
  P0.5. Entity-specific collision contexts (`CollisionContext` with an entity)
  are not modeled; shapes are dumped with the empty context. Outline data for
  passable states, support/occlusion rules, and waterlogging remain open P1.5
  work.
## 2026-11-06 - Incremental light uses Java's decrease-then-increase queues

- Context: `Chunk::set_block_state` clears a chunk's cached light because the
  chunk model cannot know which sections an edit affects. Recomputing the whole
  chunk (and a one-block neighbour margin) on every edit was correct but far
  from vanilla's `LightEngine` and made every block change send a full-chunk
  light payload. P3.8/P3.9 need decrease propagation, sky column updates,
  cross-chunk borders, and dirty-section reporting.
- Decision: `light::LightScene` owns the edited chunk plus every loaded
  neighbour and runs vanilla's two phases after an edit: first `decrease`
  (zeroing every cell lit by the old source, re-seeding emitters, and pulling
  from unaffected borders), then `increase` (BFS from the surviving border).
  Block light ports `BlockLightEngine.checkNode`/`propagateDecrease`/
  `propagateIncrease`; sky light recomputes only the edited column's
  `ChunkSkyLightSources` entry and fills/removes the direct-sky run around it.
  Queues are FIFO over `Direction.values()` order with a pending-set dedup, and
  dirty sections are a `BTreeSet`, so ordering is reproducible.
- Decision: keep `Chunk::set_block_state`'s clear behavior. `LightScene`
  snapshots the pre-edit `SectionLight` layers, replays them after the block
  mutation, and then applies the incremental delta. This keeps the change
  inside `light.rs`/`block_update.rs` while preserving the existing chunk API.
- Decision: `DirtySections` is per section, not per layer. A dirty section's
  sky and block layers are both encoded, but sections that did not change are
  omitted. `light_update_for_sections` emits the exact
  `ClientboundLightUpdatePacketData` layout (four bitsets plus 2048-byte
  arrays), so the wire format is unchanged and a section-scoped update is a
  valid vanilla packet.
- Decision: chunks with no light data pay one whole-chunk initialization pass
  when first entering a scene; neighbouring chunks changed by border
  propagation are persisted via `WorldStorage::update_chunk` and broadcast as
  their own payloads. Generated chunks still start on the height fallback until
  their first edit (generation-time lighting remains open).
- Consequences: a single edit no longer recomputes an already-lit chunk, and
  `BlockChange` exposes `dirty_sections` plus `neighbour_light`. Remaining
  parity gaps are Java light-array/packet fixtures and shape-accurate occlusion
  (full cubes until P1.5). `materialize_sky_layers` fills sections above the
  highest stored sky layer with 15 on first use; this changes representation,
  not effective light, and those sections are not marked dirty on their own.
## 2026-11-06 - P5 AI foundation uses an injected WorldView
- Context: Folia-RS world storage is global and player sessions are not the
  right substrate for goal/pathfinding tests.
- Decision: `entity_ai.rs` defines `WorldView` (blocks, light, biome, surface,
  bounds) and drives goals/navigation through it. `MemoryWorld` is the
  in-memory implementation used by focused tests; the region tick loop can
  implement the same trait later.
- Why: Keeps AI out of `WorldStorage`/`join.rs`, makes tests deterministic,
  and leaves per-region ownership open without a global static.
- Consequences: AI cannot yet see player sessions or chunk tickets; the driver
  consumes `ActorView` snapshots and returns spawn/attack events for the
  caller to apply.

## 2026-11-06 - Bounded deterministic A* instead of the vanilla node evaluator
- Context: `block_shapes.rs` is still a stub, so exact vanilla node types
  (`WALKABLE`, `FENCE`, `DOOR_WOOD_CLOSED`, ...) cannot be computed.
- Decision: Search four cardinal directions over `block_behaviour` collision
  and fluid metadata. Allow one-block step-up, at most three blocks of fall,
  reject liquids/lava, cap nodes/range, and return explicit
  `InvalidStart`/`InvalidTarget`/`Unreachable`/`TooLarge` failures. Break cost
  ties by `(x, y, z)`.
- Why: It is testable now and behaviorally conservative on plain worlds.
  Inventing node types without shape metadata would look like parity without
  being it.
- Consequences: Diagonals, doors/gates, light cost malus, swim/fly nodes, and
  stuck detection remain open (P5.5).

## 2026-11-06 - Biome spawn lists embedded in `spawn.rs`
- Context: The captured `assets/` tree has entity-type ids and worldgen biomes
  but no biome `spawners` data.
- Decision: Embed the 1.21.11 vanilla data-pack spawner table (65 biomes, 785
  entries) as a compact string in `spawn.rs`, parsed once with `OnceLock`.
  Resolve entity ids through `registry::entity_type_id`, and keep only the 50
  entity types that actually appear in a biome spawn list.
- Why: The decompiled data pack is the reference specification; adding a new
  asset file was outside this worker's file allow-list.
- Consequences: Spawn data is real rather than guessed, but any future
  datapack-override support must replace the embedded table.

## 2026-11-06 - Light/height spawn rules are explicit approximations
- Context: `WorldView` exposes stored sky/block light and a surface height, not
  vanilla `Level.getMaxLocalRawBrightness` (which includes ambient darkening)
  and not `SpawnPlacements` registrations for every type.
- Decision: Implement `Monster.isDarkEnoughToSpawn` with sky > nextInt(32),
  block-light limit 0 for the overworld, and a uniform 0..=7 threshold;
  animals require grass below and raw light > 8. Encode the remaining placement
  families as `Water`, `Lava`, `AnyLightMonster`, `SurfaceMonster`, `Mob`.
  Pack placement walks a deterministic 3x3x3 cube around the candidate.
- Why: These are the parity-critical rules for the zombie/cow slice and are
  deterministic for a fixed seed.
- Consequences: Vanilla's random 6-block pack walk, structure-specific spawn
  overrides, and specialized predicates (ocelot/parrot/fox/etc.) remain open.
## 2026-11-06 - Online-mode crypto is dependency-free, with curl as the session client

- Context: The workspace has no crypto or HTTP/TLS crate, and this slice was not
  allowed to edit `Cargo.toml`. The login path needs RSA-1024, PKCS#1 v1.5,
  AES-128/CFB8, SHA-1, Java-style signed big-int hex, and Mojang
  `hasJoinedServer`.
- Decision: Implement AES-128, CFB8, SHA-1, DER SubjectPublicKeyInfo,
  PKCS#1 v1.5, a small `u32`-limb big integer, Montgomery modular exponentiation,
  Miller-Rabin prime generation, and binary modular inverses in
  `crates/folia-core/src/crypto.rs`. Every primitive is pinned to Java/JCE
  fixtures generated with the same algorithms vanilla uses. The server key pair
  is 1024-bit RSA with `e = 65537`, generated once per process from
  `BCryptGenRandom` on Windows (or `/dev/urandom` on Unix), like
  `Crypt.generateKeyPair`.
- Decision: `SessionAuthenticator` is a trait. Production uses
  `MojangSessionAuthenticator`, which invokes the system `curl` binary for the
  HTTPS `sessionserver.mojang.com/session/minecraft/hasJoined` request because
  no TLS stack is available. Arguments go straight to the process (no shell)
  and the username/hash/IP are percent-encoded. Tests inject a mock
  authenticator and never touch the network.
- Decision: Encryption is installed at the frame boundary in
  `protocol::Connection`; the cipher covers the VarInt length prefix and body,
  matching Netty's `prepender -> encrypt` ordering. Vanilla turns compression on
  only after the key exchange; this server still does not negotiate
  compression, so enabling encryption before `Login Success` is sufficient.
- Why: Correctness first and no new dependency in a slice that cannot touch
  `Cargo.toml`; Java/JCE remains the differential oracle.
- Consequences: PKCS#1 v1.5 decryption is not a constant-time bignum
  implementation. Protocol-level padding oracles are avoided by closing on any
  malformed response and by the random 4-byte challenge (Bleichenbacher cannot
  be driven without it), but timing side channels remain a known gap. The
  session lookup depends on `curl` being installed; without it online logins
  fail with the classic "authentication servers are down" disconnect. Login
  names longer than 16 characters are rejected at parse time, matching
  `ServerboundHelloPacket`'s `readUtf(16)`; legitimate offline clients are
  unaffected. The server binary still needs `accept_logins` and
  `configure_online_mode` wiring (see `BLOCKERS.md`).

## 2026-11-06 - Session mobs are viewer-owned, not process-wide entities

- Context: The existing `entity::ENTITIES` map plus `block_update::broadcast`
  sends every entity packet to every session. The join test relies on that
  legacy path for externally spawned armor stands, but a live population must
  not leak between players or keep ticking after disconnect.
- Decision: `SessionEntityTracker` owns the deterministic cow/pig/zombie set
  and its `LivingEntity` records. It emits packets straight to its own
  connection instead of broadcasting. Process-wide broadcasts are still
  forwarded, but only for ids in the session tracker or in the global entity
  map (`entity::is_tracked`); remove-entities packets are always forwarded
  because the global map forgets the id before broadcasting.
- Decision: Session mob ids come from a global atomic base (10,000 + 8 per
  session) rather than the player id, so concurrent sessions never share an id
  while the type/offset set remains deterministic.
- Decision: Join announces the mobs after the position packet and before the
  chunk batch. The client holds them until their columns arrive, and the
  existing captured-world join test's chunk loop ignores non-chunk packets, so
  its byte-level assertions stay unchanged.
- Decision: Fall/void damage goes through `LivingEntity::hurt` for the player
  id. Void damage clears the player invulnerability timer before each 500 ms
  hit to preserve the previous damage cadence (vanilla never decrements a
  server player's `invulnerableTime`, which would otherwise reject the repeat).
- Why: Keeps legacy single-player entity tests working while giving each live
  session a bounded, disconnect-safe entity view.
- Consequences: Session mobs are not shared, saved, or visible to other
  players; AI is limited to `AiMob::update_environment` because goal-driven
  movement would make unrelated join assertions race. Full tracking is P5.13.

## 2026-11-06 - Session tick cadence is the play-loop read timeout

- Context: The session loop blocks on a socket read with a one-second timeout,
  so entity gravity would otherwise tick at 1 Hz.
- Decision: Set the play read timeout to 50 ms and tick tracked entities at that
  cadence. Keepalive, autosave, delayed block breaking, and terrain flush keep
  their existing interval checks.
- Why: Gives tracked mobs a 20 Hz physics cadence without adding a timer thread
  or changing packet ids.
- Consequences: Idle sessions wake 20 times per second. Entity packets are
  written only when a position or metadata value changes.
## 2026-11-06 - Block drops use the shipped catalog, with an explicit tool context

- Context: The merged loot slice parses 1,324 tables (1,083 `minecraft:block`)
  and exposes `LootCatalog::roll`, but `block_breaking.rs` still had a
  deterministic fallback table that collapsed ranges (redstone always 4,
  lapis always 6) and could not apply Silk Touch/Fortune/explosion rules. The
  legacy `LootProvider::block_drops(state, tool)` signature also cannot carry
  an RNG stream or an explosion radius.
- Decision: `LootCatalog::block_table_id` resolves a block id to
  `minecraft:blocks/<path>` (and also accepts an exact table id).
  `CatalogLootProvider` rolls that table with a caller-supplied
  `folia_random::RandomSource` and returns `ProvidedDrops` with the table id
  and `LootReport`. `BlockLootContext` carries the tool `ItemStack`, an explicit
  `ToolContext { silk_touch, fortune }`, an optional pre-computed correct-tool
  flag, and an optional explosion radius. `ToolContext` values are derived from
  `ItemStack` enchantment components by default but can be supplied directly;
  `LootContext::tool_context` makes them authoritative for `match_tool`,
  `table_bonus`, and `apply_bonus`.
- Decision: `block_drops` keeps its old signature so `join.rs` is untouched,
  but it now rolls the shipped catalog with a deterministic
  `LegacyRandomSource` seeded from the block state and held stack. New callers
  that have a world/session RNG must use `block_drops_with_context`; a
  provider supplied to either function is tried before the global catalog.
  `DigState::stop_with_loot` bundles the context/provider/RNG and rolls only on
  `DigStop::Destroyed`; durability remains the caller's separate step.
- Decision: the built-in `DROP_TABLE` is only a fallback when the block has no
  shipped table or a known table rolls empty *and* the report names an
  unsupported condition/entry/number provider. Unknown functions never
  suppress a produced item; they are applied as no-ops and named in
  `BlockDrops::report`. A known empty table (for example leaves missing every
  secondary chance) stays a table result with `loot_table` set, so it is not
  silently replaced by the old collapsed values.
- Why: This keeps the vanilla data authoritative and makes the
  fallback/report boundaries observable. The explicit `ToolContext` also gives
  tests and future plugin/enchantment code a path when a stack does not carry
  the component patch.
- Consequences: `block_drops` is reproducible per block/tool but is **not**
  yet Java's `Level.random` sequence; `join.rs` must pass its session RNG to
  `block_drops_with_context` before claiming RNG parity. `item_entities`,
  experience orbs, Unbreaking, adventure `can_destroy`, and a Java break-time
  fixture remain open. Wall-sign family mapping now strips the `minecraft:`
  namespace (previously the namespaced suffix could never match).
## 2026-11-06 - Block entities live beside chunks, with modern NBT as canonical

- Context: `Chunk` is constructed with struct literals throughout worldgen,
  tests, the server, and the JNI bridge. Adding a `block_entities` field would
  have forced edits outside the P4.7 slice and made every future constructor
  carry a field that only persistence needs. The 1.21.11 chunk format also
  writes the list under `block_entities`, while stored worlds and the existing
  `world.rs` test used the older `BlockEntities` spelling.
- Decision: `block_entity.rs` owns `BlockEntityMap = BTreeMap<BlockEntityPos,
  BlockEntity>` and the chunk-level `decode_block_entities`/
  `store_block_entities` functions. `WorldStorage` caches the map beside its
  decoded chunk cache, keyed by chunk coordinates, and exposes
  `block_entities`/`load_block_entities`/`block_entity`/`get_block_entity`/
  `insert_block_entity`/`remove_block_entity`. `chunk.rs` provides
  `decode_chunk_with_block_entities` and
  `encode_chunk_with_block_entities` for callers that want both in one step.
- Decision: Decode both `block_entities` and legacy `BlockEntities`; a modern
  entry wins on a duplicate position. Every rewrite removes the legacy key and
  writes modern `block_entities`, including an empty list.
- Decision: Unknown block-entity types are stored as
  `UnknownBlockEntity { pos, id, data }` with the complete original compound and
  re-emitted unchanged. Known types keep every original field in a private
  `extras` vector and overwrite only the fields they model, so plugin data,
  custom names, Paper loot data, and future fields survive a save.
- Decision: `save_chunk` remains the generated-terrain overwrite path and
  writes an empty list; `update_chunk` carries and filters the live map on
  edits; `save_chunk_with_block_entities` persists explicit inserts/removals.
  An edit removes the entity at exactly the edited position, including an air
  replacement, and leaves entities elsewhere in the chunk untouched.
- Decision: Malformed block-entity NBT (missing `id`/position, wrong `Items`
  type, a sign without exactly four lines, ...) returns `InvalidData` instead
  of being skipped. A damaged chunk therefore fails its load rather than
  silently deleting data.
- Decision: The furnace timers use vanilla's short-shaped NBT fields and
  `RecipesUsed`; `cooking::FurnaceState` is copied in and out as a behaviour
  model, but its runtime `experience` accumulator is not persisted. Spawner
  `SpawnData`/`SpawnPotentials` and sign line components stay raw NBT.
- Why: This keeps `Chunk` construction unchanged, makes persistence lossless
  for unmodeled data, and matches the 1.21.11 chunk keys while still loading
  older stored worlds.
- Consequences: `save_chunk` intentionally drops block entities, so callers
  that inserted one must use `update_chunk` or
  `save_chunk_with_block_entities`. The ticker is explicit and is not called by
  `tick.rs` yet; chest/furnace menus, placement, spawner spawning, sign editing,
  and the remaining block-entity types are still open (see `BLOCKERS.md`).
## 2026-11-06 - Bridge config, permissions, scheduler, and world state are host-owned

- Context: Plugins expect `getConfig`, `hasPermission`/attachments, a scheduler,
  services, and mutable world time/weather. The bridge previously returned
  constants for all of these, and `JavaPlugin#getConfig` would otherwise create
  a second YamlConfiguration that the host never sees.
- Decision: `BridgeConfig` replaces `JavaPlugin#newConfig` at load time and
  reads/writes `run/<plugin>/config.yml` through native calls. The file format
  is a flat scalar YAML subset: dotted keys are serialized as dotted keys and
  reconstructed as MemorySections; comments, lists, maps, and sections are
  skipped explicitly.
- Decision: Permissions live in Rust (`BridgeState::permissions`) as ordered
  attachments keyed by a per-player id. Explicit attachment values win; an
  unset permission falls back to `folia_core::join::is_operator`. Java mirrors
  attachment mutations through native calls and returns a
  `PermissionAttachment` subclass so `setPermission`/`remove` stay in sync.
- Decision: Scheduler async variants are accepted but execute on the server
  thread in the same Rust tick queue; `BukkitTask.isSync()` always reports
  true. There is no bridge-owned worker pool yet, so this is an explicit
  downgrade rather than a fake async claim.
- Decision: World time, weather flags/durations, and spawn persist in
  `run/world_state.txt`, not `level.dat`, to avoid concurrent writes with the
  native level writer. `setTime` applies Bukkit's `(time - fullTime) % 24000`
  margin. `isDayTime` uses the 1.21.11 timeline thresholds 12,523 / 23,478.
- Decision: The item wire payload is versioned (`2<TAB>...`) and carries a
  lore count plus enchantment key/level pairs into
  `ComponentValue::Enchantments`. Legacy v1 payloads still decode. Item flags
  remain in the Java meta proxy because the component model has no hide-flag
  component; they are documented as non-persistent.
- Why: These are the highest-value compatibility surfaces and all can be
  tested without a live client socket.
- Consequences: Config reload through the stock `reloadConfig()` replaces the
  bridge object with YamlConfiguration; plugins must call `getConfig()` (which
  lazily reloads the bridge file on first access). Weather/time changes are
  persisted but not sent in play packets. `sendMessage`/`kickPlayer` still only
  log because the join session exposes no outbound queue. `playSound` and
  `spawnParticle` are logged no-ops. Enchantment/sound registry shims provide
  identity only, not gameplay effects or audio.
## 2026-11-06 - Light occlusion uses the generated outline face projections
- Context: `light.rs` previously treated light occlusion as the per-state
  `light_block` value alone. That is correct for full cubes but wrong for
  slabs, stairs, snow layers, farmland, and paths: Java's
  `LightEngine.shapeOccludes` merges the two adjacent `getFaceOcclusionShape`
  projections and only blocks light when the union covers the shared face.
  `block_state_behaviour.tsv.gz` has no `canOcclude` or
  `useShapeForLightOcclusion` columns, but `block_shapes.tsv.gz` stores the
  exact `getShape`/outline AABBs.
- Decision: `light::face_occludes(state_id, direction)` projects the extreme
  slice of the state's outline onto the face square and tests whether the union
  of projected boxes covers it. `pair_shape_occludes` unions both states'
  opposing faces, mirroring `Shapes.faceShapeOccludes`. The geometry and light
  masks are cached in two dense 29,671-byte tables (`FACE_MASKS`), so the hot
  path is a branch plus a byte lookup; only the rare partial/partial pair
  rebuilds rectangles.
- Decision: A full cube with `light_block < 15` is treated as shape-transparent.
  This reproduces the effect of `canOcclude=false`/`noOcclusion()` for glass,
  stained glass, ice, frosted ice, leaves, water, honey, slime, beacon,
  shulker boxes, copper grates, and spawners without adding columns to the
  generated assets. Empty outlines (air, fluids, plants, torches) never
  contribute a face. Attenuation still comes from `light_block`, so water and
  leaves stay at 1 and glass stays at 0.
- Decision: The engine's `light_face_mask` also excludes ordinary full cubes
  (stone, glowstone, sea lantern, magma). Vanilla leaves
  `useShapeForLightOcclusion` false for them and relies on `light_block == 15`;
  including the full shape would incorrectly stop an opaque emitter such as
  glowstone from lighting its neighbours.
- Decision: Because the asset lacks `useShapeForLightOcclusion`, the light
  mask uses a small static allowlist transcribed from the decompiled block
  classes: slabs, stairs, shelves, carpets (intentional task deviation), snow,
  farmland, dirt paths, daylight detectors, enchanting tables, lecterns,
  pistons, sculk shriekers/sensors, stonecutters, and end portal frames. Doors,
  trapdoors, fence gates, fences, walls, panes, campfires, beds, bells,
  composters, and other `useShapeForLightOcclusion=false` partial shapes keep
  their `light_block` attenuation without occluding. This is what lets a
  campfire light the block above it and a closed trapdoor pass direct sky.
  The public `face_occludes` still reports the geometric face, so callers can
  use it for hit/visibility tests without inheriting the engine's policy.
- Decision: Both engines use the same check: the whole-chunk sky source scan
  uses `isEdgeOccluded` (lower `light_block != 0` or the pairwise face test),
  and sky/block increase propagation checks the face before writing a level.
  The decrease queues deliberately do not shape-check, matching
  `BlockLightEngine.propagateDecrease`/`SkyLightEngine.propagateDecrease`: they
  over-clear the old region and the increase phase re-lights it through the new
  shape. `LightScene::update_block` recomputes the sky column when either the
  light-block value or the per-state face mask changes, and re-runs a direct-sky
  cell when only the shape changed.
- Why: This keeps the asset format stable, uses the oracle-exact outline data
  already embedded, and matches the documented task contract.
- Consequences: Cover the common shape-occluding blocks (slabs, stairs, snow,
  farmland, paths, carpets) and confirm fences/panes/walls and other
  `useShapeForLightOcclusion=false` shapes stay transparent, without Java
  light fixtures yet. Remaining approximation: blocks that override
  `getOcclusionShape` independently of `getShape` (sculk shrieker, lectern,
  fence gate) use their outline, and carpet shape-occludes its top/bottom
  although vanilla `CarpetBlock` leaves `useShapeForLightOcclusion` false.
  Exact `canOcclude`/`useShapeForLightOcclusion` columns and Java light-array
  fixtures are the next parity step.
- Decision: selector branches resolve through **both** registries. Vanilla
  `random_selector` / `simple_random_selector` hold `Holder<PlacedFeature>`, so
  a bare branch id is looked up in the placed registry first, then in the
  configured registry for the dump's inline forms. A placed branch keeps its
  full `PlacementModifier` list and runs it before the target feature; branch
  modifiers that are unported or that are `minecraft:biome` (the branch world
  adapter has no biome sampler) make the selector unsupported instead of
  silently dropping the branch. This is what makes `pale_garden_vegetation`,
  `warm_ocean_vegetation`, and `pointed_dripstone` execute.
- Decision: `FeatureLevel::sea_level` defaults to 63. The region has no
  `ChunkGenerator` handle, and every shipped config that reads the sea level
  (icebergs, blue ice, basalt columns) is Overworld or Nether lava-level
  compatible. A future dimension-aware region should override this method.
- Decision: `LakeFeature` accepts the shipped lava configuration and rejects a
  water configuration at runtime. Water lakes require
  `Biome.shouldFreeze` for their surface pass, which the current
  `FeatureLevel` cannot observe; failing is preferable to silently skipping
  the freeze pass. `freeze_top_layer` remains unsupported for the same reason.
- Decision: rule-based state providers are parsed at both the record form used
  by `DiskConfiguration` (no `type` discriminant) and the normal dispatched
  form. The parser reuses `BlockPredicate` from the placement module rather
  than duplicating predicate semantics.
- Decision: `root_system` is **not** marked supported yet. The 1.21.11
  bytecode moves the working position up before testing
  `allowed_tree_position`, while the shipped predicate requires the block below
  that candidate to be `#minecraft:azalea_grows_on`; the initial air check
  makes that unsatisfiable for a normal surface origin in this region. The
  feature needs a Java placement oracle / behaviour clarification before we
  claim parity. Details are in `BLOCKERS.md`.
- Consequence: `sculk_patch` stays unsupported until `SculkSpreader`,
  `SculkBehaviour`, and the multiface spreader are ported; `geode` stays
  unsupported until a world seed reaches `FeatureLevel` (the geode noise is
  seeded from `worldGenLevel.getSeed()`). Both failures are explicit
  `UnsupportedFeature` errors, not silent no-ops.
## 2026-11-06 - Recursive jigsaw uses block-aligned AABB occupancy

- Context: Java `JigsawPlacement` uses `VoxelShape`/`AABB` with a 0.25 deflate
  and `BooleanOp` unions. This crate has no generic voxel-shape boolean engine,
  and the existing `block_shapes` metadata is for blocks, not arbitrary AABBs.
- Decision: model the allowed max-distance window and every placed start/piece
  as inclusive `BoundingBox`es. Reject a candidate when its integer box leaves
  the allowed window or intersects an occupied box. A connector that points
  back into its own piece starts a fresh occupancy list containing only that
  piece, matching Java's branch; otherwise the shared per-start occupancy list
  is consulted. Chains that would place a child nearly entirely inside the
  parent are rejected rather than approximated with a fractional deflate.
- Why: block-aligned structure windows and templates are integer grids; this is
  deterministic, testable, and conservative. It never accepts an overlapping
  rigid piece.
- Consequences: exact edge-touching parity for fractional/rotated shapes and
  terrain-adaptive pieces is not guaranteed. Terrain adaptation itself
  (`beard_thin`, `beard_box`, `bury`, `encapsulate`) is still not applied to
  block placement; only the 12-block start bounding-box inflation is modeled.

## 2026-11-06 - Jigsaw starts take the level through the generation context

- Context: `StructureDefinition::generate_start` previously had no level, while
  `JigsawPlacement` needs `getFirstFreeHeight`, `getMaxY`, and dimension-padding
  checks. Adding a mutable world to the start API would break the data-only
  start contract.
- Decision: `StructureGenerationContext` carries
  `Option<&dyn FeatureLevel>` via `with_level`. Jigsaw generation fails with
  `MissingHeightSource` when it is absent. The generator passes the chunk's
  `WorldGenRegion`, so height sampling is chunk-local and reads terrain after
  surface/carvers but before decoration features.
- Why: keeps starts deterministic and testable with `MockLevel`, while making
  the missing-world failure explicit instead of silently using y=0.
- Consequences: starts that span chunks sample only the chunk currently being
  generated. The next structure manager slice should cache one start and expose
  a region height source so cross-chunk pieces use the same ground height.

## 2026-11-06 - Structure generation is opt-in and template data is caller-supplied

- Context: the repository has no shipped structure-template NBT asset (the
  decompiled tree is reference material and must not be published). Adding one
  was outside this worker's file budget. Making structures always run would
  also change the default worldgen/parity path.
- Decision: `WorldGenerator::overworld` leaves `structures: None`. Callers add a
  `StructureGenerator` through `with_structure_generator` or a
  `HashMap<String, StructureTemplate>` through `with_shipped_structures`. The
  manager regenerates starts deterministically per candidate placement chunk
  (radius 8), filters by the 34 shipped biome tags and the Overworld dimension,
  and runs before the first decoration feature step at/after
  `surface_structures`. Tests embed the shipped igloo NBT and enable only
  `minecraft:igloo`.
- Why: parity fixtures stay byte-for-byte unchanged, and a missing template is a
  loud `UnknownTemplate`/`MissingHeightSource` error rather than an empty start.
- Consequences: a real village currently fails loudly because its NBT templates
  are not embedded. Feature-pool elements also remain an explicit
  `UnsupportedPoolElement`; many village decor pools contain them. A future
  asset extractor should populate the template provider and add a start cache.

## 2026-11-06 - Waterlogging wraps the feature level; markers stay data

- Context: the shared `PoolExpansion::place` path cannot be edited in this
  slice, and the chunk has no typed block-entity storage yet.
- Decision: `JigsawPiece` wraps the caller's `FeatureLevel` in a
  `WaterloggingLevel` that reads the existing fluid at the target and flips a
  `waterlogged=false` property to true when `apply_waterlogging` is selected.
  `structure_void` replacement continues to drop blocks through the existing
  `JigsawReplacement` processor. Data markers, chest/brushable block-entity NBT,
  and template entities are returned on `StartPlacement`/`StructureChunkPlacement`;
  no entity is spawned and no loot table is rolled.
- Why: explicit, local behavior without changing the pool API or pretending
  block entities exist.
- Consequences: non-source fluid propagation, fluid containers beyond the
  waterlogged property, loot seeding, chest block-entity creation, and entity
  spawning remain open. `StructureGenerator` currently discards the returned
  marker/entity payload after placement; callers of the start API can consume it.

## 2026-11-06 - Desert pyramid random stream is deterministic but not Java-exact

- Context: `DesertPyramidPiece.postProcess` consumes the chunk generator's
  decoration random, which this port does not expose to structure starts.
- Decision: derive a local `LegacyRandomSource` from the world seed and the
  chunk's minimum X/Z. Use it for the height offset, cellar stairs boolean,
  collapsed-roof chance, and suspicious-sand subset. The piece body, chest
  positions, cellar geometry, and loot-table markers are direct ports.
- Why: keeps generation deterministic and testable without adding a chunk RNG
  plumbing slice.
- Consequences: collapsed-roof/suspicious-sand choices can differ from Java for
  the same seed. The four chests and main pyramid body are position-stable.

## 2026-11-07 - Armor and effect math is formula-based, not entity-differential

- Armor uses the vanilla `CombatRules.getDamageAfterAbsorb` curve with the
  per-material point table (leather/chain/iron/gold/diamond/netherite/turtle)
  and diamond/netherite toughness. Enchantment Protection and armor durability
  are not part of the reduction yet.
- Status-effect intervals use `25 >> amplifier`, `40 >> amplifier`, and
  `50 >> amplifier` for poison, wither, and regeneration, clamped to one tick.
  Effects are ticked by callers; instant effects are added but not scheduled.
- `LivingEntity::hurt_with` adds the invulnerability bypass used by void damage;
  `hurt` delegates to it with the default rules so existing behavior is
  unchanged.

## 2026-11-07 - Dimension caches are per-instance, not process-wide

- `DimensionStorages` owns one `WorldStorage` per dimension so region files are
  never shared across dimensions. The overworld still uses the process-wide
  `shared_overworld` instance elsewhere; this type is the portal/dimension
  manager and is intentionally not a second overworld singleton.
- Generate-on-miss persists the generated chunk immediately, mirroring the
  join path's stamped overworld chunks. Portals are not wired into sessions yet.
- Coordinate scaling floors toward negative infinity for Overworld -> Nether,
  matching Java's `Mth.floor`; the End transition always uses the fixed
  obsidian platform.

## 2026-11-07 - Lamps and openables use immediate neighbour power

- `neighbor_power` queries all six directions through `SignalReader`, which
  preserves the existing weak/strong power rules. Lamps and doors update on the
  neighbour/shape update already queued by `block_update::set_block`.
- Double-door halves are updated independently; linking both halves belongs to
  the shape-update slice.
- Daylight detectors expose the signal math but are not registered as a
  behaviour because chunk light has no public getter yet. Note blocks expose
  pitch frequency without audio.

## 2026-11-07 - Block entities tick after random ticks

- `tick_world_with_table` runs the persisted furnace/spawner ticker after the
  random-tick pass and before the final neighbour drain. This keeps any block
  changes queued by a ticker in the same tick and makes the order deterministic.
- The ticker uses `RecipeCatalog::global()` rather than allocating a catalog per
  world, and it only sees block entities that have been loaded into storage.

## 2026-11-07 - Hopper transfer is a pure slot core

- `move_one_item` takes source and target slot slices, so container automation can
  be tested without live world access. It merges into compatible stacks first,
  then uses an empty slot, moves exactly one item, and respects the item
  prototype's `max_stack_size`.
- `HopperBlockEntity` persists its five slots and `TransferCooldown`; world-level
  pull/push wiring (block above/below container lookup) remains a later slice.

## 2026-11-07 - Daylight detectors sample the cell above

- Stored sky-light nibbles for a non-air block cell are zero in this engine, so
  the detector samples `pos.y + 1` (the open cell that actually carries sky
  light) to match the vanilla brightness the player sees.
- The detector schedules itself every 20 ticks; a block placed above it triggers
  an immediate light recompute via the existing block-update path.

## 2026-11-07 - Leaf distance uses a bounded BFS

- `natural_distance` walks outward through leaves to the nearest log instead of
  trusting stored `distance` values. This makes placement/neighbor updates
  correct even when a leaf chain is built out of order.
- Placement and neighbour updates only recalculate distance; only the random
  tick may destroy a distance-7 non-persistent leaf, matching vanilla's decay
  gate.

## 2026-11-07 - Brewing preserves the bottle component patch

- Context: vanilla `PotionBrewing.mix` builds the result through
  `PotionContents.createItemStack`, which starts from a fresh stack and therefore
  drops a bottle's custom name, color, and custom effects. The slice acceptance
  criteria require component preservation, and `PotionContents` already models
  those fields.
- Decision: `crafting::brew_result` copies the input bottle stack, changes only
  the item id and the `potion` field of its `PotionContents`, and keeps every
  other component (including `custom_effects`). Both potion mixes and container
  mixes (gunpowder/dragon breath) use this rule. This is an explicit,
  documented extension over `PotionContents.createItemStack`.
- Why: preserves the typed potion component the task requires and keeps the
  existing crafting/smithing component-preservation contract.
- Consequences: a Java differential fixture will show a component-patch
  difference when a decorated bottle is brewed. If strict Java parity wins later,
  change `potion_result` to build a fresh `PotionContents::new` stack.

## 2026-11-07 - Brewing and consumables are state models, not menus

- Brewing state lives in `crafting::BrewingStandState` rather than
  `block_entity.rs` because the brewing stand block entity/menu is still open.
  It owns five slots, `brew_time`, `recipe_brew_time`, `fuel`, the
  ingredient-change guard, and dropped remainders, but no block-entity NBT or
  menu packets yet.
- `item_prototypes::consume` mutates a hand `ItemStack` and a `FoodData` and
  returns instant effect events plus an overflow remainder. It deliberately does
  not own player health: the caller applies instant healing/damage and inserts
  the overflow remainder. Creative is modeled by the caller flag, matching
  `LivingEntity.hasInfiniteMaterials`.
- The `#minecraft:brewing_fuel` tag table is used instead of hardcoding blaze
  powder so datapack additions work through the shipped tag asset.

## 2026-11-07 - Crop growth is a documented light-gated approximation

- `crops.rs` advances age on a random tick when sky light at the cell above is
  >= 9, with a deterministic 1-in-4 roll. Vanilla's farmland/neighbour growth
  speed points are not modeled yet.
- Hydration, stem/fruit crops, bone-meal particles, and crop drops remain open.
## 2026-11-07 - Feature block-entity payloads use the region sink

- `bonus_chest`, `desert_well`, and `monster_room` attach their vanilla
  `LootTable`/`LootTableSeed`, brushable loot table, and spawner `SpawnData`
  compounds through `FeatureLevel::set_block_entity` instead of failing the
  feature. This is the existing worldgen block-entity contract (trees already
  use it for beehives), so the feature is supported even though the generator
  currently discards the returned list.
- `end_gateway` always attaches an `Age`, and attaches `exit_portal`/
  `ExactTeleport` only when the shipped config supplies the exit, matching
  `TheEndGatewayBlockEntity.saveAdditional`.

## 2026-11-07 - Sculk patch ports the worldgen spreader

- `sculk.rs` ports `SculkSpreader.createWorldGenSpreader` values (growth cost
  50, no-growth radius 1, charge decay 5, additional decay 10, replaceable tag
  `sculk_replaceable_world_gen`) and the cursor update/charge loop.
- The multiface spreader only implements the three `SpreadType` variants and
  the sculk-vein `SpreadConfig`; face-sturdiness uses the shared solid-state
  approximation because `FeatureLevel` has no collision-context query.
- Entity pushing on sculk conversion (`Block.pushEntitiesUp`) is not modelled;
  there is no entity layer during feature placement.

## 2026-11-07 - Hopper tick shim lives outside world.rs

- The tree at `d3e2b53` calls `WorldStorage::tick_hoppers` without defining it.
  Because `world.rs` was outside this worker's file set, the inherent impl was
  placed in `worldgen/feature/mod.rs` so the workspace compiles. It performs a
  cached-chunk push/pull pass; move it to `world.rs` when that file is in scope.

## 2026-11-07 - Chest loot uses first-fit slot placement

- `fill_loot` rolls the stored table with a caller-supplied RNG, preserves
  existing slots, and fills the first empty slots. Vanilla distributes stacks
  into random empty slots; the deterministic first-fit order is documented as an
  approximation until a Java fixture pins the placement.
- The loot id and seed are cleared after a successful fill, so opening the chest
  again does not reroll it.

## 2026-11-07 - Hunger ticks are a model, not yet wired to the session loop

- `tick_food` uses the vanilla 10-tick saturated regen / 80-tick unsaturated
  regen / 80-tick starvation cadence and burns exhaustion in 4-point units.
- It is not called by `join.rs` yet; eating item use still needs the bridge/
  session wiring. Sprint/jump exhaustion sources and difficulty scaling remain
  open.

## 2026-11-07 - Brewing stands persist the vanilla fields

- `BrewingStandBlockEntity` stores `Items`, `BrewTime` (short) and `Fuel` (byte)
  and delegates behaviour to `crafting::BrewingStandState`.
- The recipe brew time is not persisted because vanilla resets it to 400; the
  runtime state keeps it for tests/menus.
## 2026-11-07 - Bridge outbound traffic uses a play-session command registry

- `join::SessionCommand` is the single outbound queue. Each play connection
  registers an `ActiveSession` keyed by UUID with its name and a monotonic
  connection id in `SESSIONS`; `SessionCommandGuard` removes only its own
  connection on drop, so a reconnect cannot lose the new registration. Console
  `gamemode` and the JNI bridge both use the same channel.
- Packet ids come from the 1.21.11 `GameProtocols` clientbound order: disconnect
  32, level particles 46, set time 111, sound 115, and system chat 119 (the
  bundle packet occupies id 0). Component payloads use unnamed network NBT
  (`nbt::write_any_tag`) rather than the named-root form used by chunk NBT.
- Chat sends `ClientboundSystemChat` with `overlay=false`; kick sends the
  play-state common disconnect packet and then saves playerdata and flushes
  terrain before the session returns. Teleport reuses
  `ClientboundPlayerPosition`, sets the live session position and physics yaw/
  pitch, and re-streams chunks when the column changes.
- World time uses `ClientboundSetTime` with the persisted day time for both
  game time and day time because the bridge still has no separate game-time
  clock. Storm/thunder changes emit the vanilla start/stop rain events plus the
  rain/thunder level events; the flags remain persisted in `world_state.txt`.
- Sound uses a direct `SoundEvent` holder (registry id 0, identifier, no fixed
  range), so the bridge does not need a sound registry table. Simple particles
  use the built-in `particle_type` registration order; option-carrying
  particles are rejected with an explicit log instead of sending a truncated
  packet. This slice broadcasts effects to every live session and does not yet
  apply the vanilla distance filter.
- `WorldStorage::tick_hoppers` was added to satisfy the existing
  `tick_world_with_table` call. It pushes one ready hopper item into the
  container below after the block-entity ticker has advanced the cooldown;
  hopper pull, locking, and menu packets remain open.

## 2026-11-07 - Unbreaking needs a level RNG

- `apply_block_durability_with_rng` takes an explicit `RandomSource` and skips
  wear when `nextInt(level + 1) != 0`, matching `ItemStack.hurtAndBreak`.
- The old `apply_block_durability` wrapper uses a fixed legacy RNG so existing
  no-enchant callers and tests stay stable; production should pass the level RNG.

## 2026-11-07 - Oxygen is a player model, not yet wired to sessions

- `tick_air` follows the vanilla `-20` drowning threshold, two damage per
  20-tick cycle, and four-per-tick refill out of water.
- `join.rs` does not call it yet; water/eye-submersion detection, water-breathing
  and respiration enchantments, and bubble columns remain open.
## 2026-11-07 - Final configured features and the level capability hooks

- `FeatureLevel` now exposes three explicit defaults:
  `seed()` (the globally configured generator's seed, else `0`),
  `biome_at(pos)` (`None` when the level cannot resolve biomes), and
  `spawn_entity(FeatureEntity)` (`false` when the level has no entity sink).
  `WorldGenRegion` overrides all three: `with_seed` pins the geode/end-spike
  seed, `biome_at` reads the generated chunk's 4x4x4 biome palette, and
  `spawn_entity` records descriptors next to block-entity payloads.
  Production startup already installs the generator through
  `worldgen::generator::configure`, so the seed default reaches the real level
  seed; direct `WorldGenerator::overworld(seed)` harnesses use `with_seed` when
  they need a different one.
- `root_system` is supported. The earlier blocker was a misread of the shipped
  placement: `rooted_azalea_tree` uses `environment_scan` + `random_offset
  y=-1`, so the feature origin is the air block *below* a solid support, not
  the surface air block. Moving up then reaches the real candidate whose
  `allowed_tree_position` `[0,-1,0]` arm sees that support, exactly as
  `RootSystemFeature.placeDirtAndTree` does. `tree.rs` preserves the Java draw
  order for rooted-dirt and hanging-root attempts.
- `freeze_top_layer` reads `WorldGenRegion`'s chunk biome cells, not a guessed
  climate. `misc.rs` implements `shouldFreeze`/`shouldSnow` through
  `height_adjusted_temperature`, the shipped `has_precipitation=false` biome
  list (the extractor does not carry that flag yet), the four-side
  `mustBeAtEdge` water rule, and the `snow_layer_can_survive_on` tags.
  Generation-time block light is `0`; the `<10` gate is documented in the
  feature.
- `end_spike` uses `worldgen::end::end_spikes_for_seed(level.seed())` when the
  config's `spikes` list is empty, writes the same obsidian pillar/cage/fire
  blocks as `worldgen::end::place_spike`, and queues an `EndCrystal`
  `FeatureEntity` with `ShowBottom`, `Paper.GeneratedByDragonFight`, optional
  `beam_target`, and optional `Invulnerable` NBT. The entity itself is not
  spawned by the generation layer, matching how structure templates return
  entity descriptors.
- `fossil` remains explicitly unsupported. `assets/structure_templates.json`
  contains only `minecraft:village/**`; `minecraft:fossil/spine_*` and
  `minecraft:fossil/skull_*` are absent, and `FeatureLevel` has no template
  provider. The error names both missing pieces.
- Consequence: 222/224 configured features dispatch as supported. The two
  remaining unsupported configs are `fossil_coal` and `fossil_diamonds`.
## 2026-11-07 - Container menus are session-owned and sync full content

- Menu state lives in `inventory::SessionMenus`, not `PlayerSession`, because
  `player.rs` was outside this worker's file set. A session has at most one open
  menu; container ids follow `ServerPlayer.nextContainerCounter` (1..100) and
  state ids follow `AbstractContainerMenu.incrementStateId` (15-bit wrap).
- Click handling keeps the session cursor authoritative. The packet's hashed
  carried stack is parsed only for framing and is never used to reconstruct
  components, so component patches survive every move. Menu slots map to the
  player inventory in vanilla `addStandardInventorySlots` order: main inventory
  9..35 first, then hotbar 0..8.
- Play packet ids come from `GameProtocols` (bundle packet occupies id 0):
  Open Screen 57, Set Content 18, Set Data 19, Container Close 17, Container
  Click 17. Menu type ids come from `MenuType` registration order: generic_9x3
  2, blast furnace 10, brewing stand 11, furnace 14, smoker 22.
- Furnace data slots are `litTimeRemaining`, `litTotalTime`, `cookingTimer`,
  `cookingTotalTime`. Brewing slot 0 is the menu wrapper's
  `400 * brewTime / recipeBrewTime`; slot 1 is the fuel charge.
- Menu edits are written through `WorldStorage::remove_block_entity`/
  `insert_block_entity` and then `update_chunk`, which preserves top-level
  chunk fields this port does not decode. Menus close on block break or
  disconnect. Unknown block-entity/menu ids are rejected rather than guessed.
- `QUICK_CRAFT` drag and generalized double-click collection are not modelled;
  only chest, furnace/blast furnace/smoker, and brewing stand menus are
  supported in this slice.
