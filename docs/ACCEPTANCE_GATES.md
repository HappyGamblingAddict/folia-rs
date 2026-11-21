# Acceptance gates for "1.0" (P11.9)

**Audit date:** 2026-11-21
**Scope:** P11.9 in [TODO.md](../TODO.md) (lines 3528-3543), evaluated against the
current checkout (HEAD `6afc2bd`, clean tree). This is an **audit of evidence that
exists in the repository**, not a claim of parity and not a re-run of the full test
matrix. Where a gate has no test, fixture, benchmark, or implementation, it is
marked `UNKNOWN` or `FAIL` rather than assumed good.

## Status legend

| Status | Meaning |
|---|---|
| **PASS** | Current code/tests fully demonstrate the gate; evidence is cited. |
| **PARTIAL** | Real implementation/test coverage exists, but material gaps remain. |
| **FAIL** | Current evidence shows the gate is not met (below threshold, missing behavior, or explicit open TODO). |
| **UNKNOWN** | No fixture/benchmark/test exists to decide the gate either way. |

The authoritative checklist is P11.9 in [TODO.md](../TODO.md); the current
progress claims are in [STATE.md](../STATE.md); known gaps are in
[BLOCKERS.md](../BLOCKERS.md). Test counts in those two files disagree
(`1,530 passing / next gate 1,603` in STATE vs `1,600+ tests` in
[README.md](../README.md)); this document does not assert a verified count.

---

## 1. Functional parity gates

### 1.1 Survival loop — `FAIL`

P11.9 requires a vanilla client to complete: join → mine → craft → smelt → eat →
fight → die → respawn → use a bed → travel to Nether/End → save.

- **Present and tested:** join/configuration/play path and chunk streaming
  ([README.md](../README.md), "What works right now");
  `tests/block_breaking.rs` / `tests/loot_drops.rs` (mine/drops);
  `tests/crafting.rs` (craft + furnace smelt); `tests/food.rs` /
  `tests/brewing.rs` (eat/drink); `tests/combat.rs` / `tests/join_entities.rs`
  (fight); `tests/game_mode.rs` and join tests (death/respawn, keepInventory);
  portal-driven live dimension switching is recorded in
  [STATE.md](../STATE.md) ("portal-driven live dimension switching").
- **Missing:** sleeping / bed use has no handler in `join.rs` and is listed as
  missing in [BLOCKERS.md](../BLOCKERS.md) (2026-11-05 native-console/void-death
  entry: "beds/anchors ... still missing"); the End dragon fight is absent
  ([STATE.md](../STATE.md) "major gaps"). No single test drives the whole loop
  end to end.

### 1.2 Worldgen structure + feature parity — `FAIL`

Target: **99.9% block-class / 99% exact-name** on multi-seed fixtures.

- Measured on the only captured differential fixture (16 columns of chunk
  `(0,-1)`, seed 12345): **98.75% block-class (6067/6144)** and **95.88%
  exact-name (5891/6144)**; 28/33 Java ore blocks matched. Evidence:
  `tests/density_parity.rs::sixteen_column_parity_floor_after_interpolation`
  (asserts at [density_parity.rs:455-468](../crates/folia-core/tests/density_parity.rs))
  and the parity printout in `tests/worldgen_parity.rs`
  (asserts only `> 0.98` class / `> 0.915` exact at
  [worldgen_parity.rs:148-160](../crates/folia-core/tests/worldgen_parity.rs)).
- The implemented test floors are **lower than the P11.9 targets**, the fixture
  is single-seed/single-chunk, and P2.8 terrain adaptation and exact cross-chunk
  height/start caching remain open ([TODO.md](../TODO.md), [STATE.md](../STATE.md)).
- Context that is done: 224/224 configured feature types and 888/888 placement
  modifiers execute; 1,202 structure templates ship — but this is coverage,
  not the required fidelity threshold.

### 1.3 Lighting and block updates byte-identical on fixtures — `UNKNOWN`

- Byte-identical evidence exists only for the **join/chunk payload** path:
  `tests/worldgen_join.rs` and the captured `(0,-1)` chunk packet noted in
  [README.md](../README.md).
- No captured Java **light-array/light-packet** fixture and no Java
  **neighbor-update** fixture exist; [BLOCKERS.md](../BLOCKERS.md) records this
  for P1.5/P3.6-P3.8 and P3.10 ("no Java light-array/packet fixture", "No Java
  differential neighbor-update, fluid, or redstone fixture exists yet").
  `tests/lighting.rs`, `tests/light_incremental.rs`, and
  `tests/light_occlusion.rs` are behavior tests against decoded Java formulas,
  not byte comparisons.

### 1.4 Redstone tick-exact on a curated suite — `UNKNOWN`

- `tests/redstone2.rs`, `tests/redstone_extra.rs`, and `tests/automation.rs`
  encode the Java update/signal formulas and packet-visible transitions.
- There is **no Java differential redstone trace and no curated tick-exact
  suite**; [BLOCKERS.md](../BLOCKERS.md) (P3.10 redstone entry) states this
  explicitly, as do the unported gap lists (tripwire hooks, analog inputs,
  persisted comparator output/torch toggle log, no persisted scheduled ticks).

### 1.5 Entity physics/combat traces within tolerance — `PARTIAL`

- One genuine captured trace exists: the armor-stand fall (18 samples, all
  within `5e-4`, landing at `y = -60`), cited in
  [README.md](../README.md) and exercised via `tests/entity_physics.rs`.
- `tests/combat.rs`, `tests/entity_ai.rs`, and `tests/projectile.rs` pin Java
  formulas and deterministic behavior, but there is no broad captured
  physics/combat trace corpus, and live AI goals plus per-type tracking ranges
  are open ([STATE.md](../STATE.md) major gaps, P5).

### 1.6 Tiered plugin corpus runs unchanged — `FAIL`

- The only exercised plugin is `java/example-plugin`, via `BridgeApiTest` (44
  checks) and the `FOLIA_BRIDGE_SELFTEST=1` run.
- [PLUGIN_COMPATIBILITY.md](../docs/PLUGIN_COMPATIBILITY.md) (lines 7 and 42)
  states: "there is no curated plugin corpus passing yet" and "Compatibility
  corpus | Missing | No tiered jar test set or CI run yet" — TODO **P9.11**.
  NMS/CraftBukkit/reflection plugins are explicitly unsupported.

### 1.7 Multiplayer behavior — `PARTIAL`

- Cross-session player entities, chat broadcast, tab-list player info,
  max-player guard, and shared entity tracking have focused tests
  (`tests/join_entities.rs`, `tests/shared_tracking.rs`, `tests/status.rs`,
  `tests/game_mode.rs`; see [STATE.md](../STATE.md) "Recently completed").
- No 20+ player concurrency/load test exists, and real parallel-region world
  isolation is absent: `WorldStorage` remains one global lock and task handoff
  is unimplemented ([BLOCKERS.md](../BLOCKERS.md) P10 entry; P10.3/P10.5 in
  [TODO.md](../TODO.md)). Performance/scale is therefore `UNKNOWN`, not proven.

---

## 2. Correctness gates

### 2.1 Differential fixtures — `PARTIAL`

- Available: `tests/worldgen_parity.rs`, `tests/density_parity.rs` (pinned parity
  floors), `tests/worldgen_join.rs`, `tests/join.rs` captured join sequence, and
  the armor-stand physics trace.
- Missing Java differential fixtures are catalogued in
  [BLOCKERS.md](../BLOCKERS.md) for lighting, redstone, fluids, structures,
  item components, tree features, recipes/furnace, break times/drops, entity
  metadata, and BiomeManager. P0.5 full oracle regeneration is blocked by the
  absent external Java toolchain ([OPERATOR_GUIDE.md](../docs/OPERATOR_GUIDE.md),
  "Oracle regeneration status"; [BLOCKERS.md](../BLOCKERS.md) 2026-11-04 P0.5).

### 2.2 No panics on fuzz input — `PARTIAL`

- `tests/protocol_fuzz.rs` (10 deterministic tests: VarInt boundaries and
  malformed values, random serverbound parsers, hostile frame lengths, strings,
  compression, truncated/random NBT, deep NBT rejection) and
  `tests/data_fuzz.rs` (5 deterministic tests: NBT/SNBT, chunk palettes, mutated
  JSON, item data) assert parsers return errors rather than panicking. The NBT
  depth cap (512) was added after review with three regression tests
  ([SECURITY_REVIEW.md](../docs/SECURITY_REVIEW.md), "Finding 1").
- These are **seeded, deterministic in-process tests**, not a continuous fuzzer
  (no `cargo-fuzz`/`arbitrary` harness), and no 24-hour soak test exists (see
  3.4). Command: `cargo test -p folia-core --test protocol_fuzz --test data_fuzz`.

### 2.3 Version guards — `PASS`

- `crates/folia-core/assets/VERSION.txt` pins `minecraft_version=1.21.11` and
  `data_version=4788`; `src/asset_version.rs` validates the manifest and eight
  versioned JSON catalogs at startup and in tests
  ([asset_version.rs:8-46](../crates/folia-core/src/asset_version.rs)).
- Test evidence: `tests/assets.rs::asset_version_matches_the_target_data_version`
  plus manifest/hash/line-count guards
  ([assets.rs:56](../crates/folia-core/tests/assets.rs)); startup validation is
  wired in `crates/folia-server/src/main.rs`. Caveat from
  [BLOCKERS.md](../BLOCKERS.md): two hash guards were documented as red on a
  CRLF-materialized checkout; they must be clean in the release checkout.

### 2.4 Save/load round-trips — `PARTIAL`

- `tests/world_round_trip.rs::generated_chunk_round_trips_through_anvil`,
  `tests/anvil.rs` (item/component round-trips), `tests/shutdown_flush.rs`,
  `tests/shutdown_sessions.rs` (playerdata saved on shutdown), plus spawner and
  block-entity persistence tests cover the main path.
- Data-loss gaps remain: scheduled ticks, comparator outputs, and redstone
  toggle logs are not persisted ([BLOCKERS.md](../BLOCKERS.md) P3.10 entries);
  world-state persistence and `/time`/`/weather` transitions are open
  ([STATE.md](../STATE.md)); a full stop → restart → backup integrity test is
  not present.

---

## 3. Performance gates

There is no Criterion-style benchmark suite: no `benches/` directories, no
Criterion dependency, and no dedicated performance job in CI (checked across
`crates/*/Cargo.toml` and `.github/workflows/`). A dependency-free timing smoke
suite does run as part of the ordinary test job:
[tests/perf_smoke.rs](../crates/folia-core/tests/perf_smoke.rs) times chunk
generation, lighting, the world tick loop, entity AI, pathfinding, and packet
encoding, printing elapsed time and throughput under `--nocapture`. Its only
time assertion is a deliberately loose 120 s per-phase hang guard; there are no
regression budgets, no statistical sampling, and no per-player/chunk memory
numbers, so it cannot flag a small slowdown and does not answer the gates below.
The remaining timing code is an `#[ignore]`d diagnostic that prints the cost of
2048-bit RSA profile-key generation
(`tests/chat_signing.rs::benchmark_secure_profile_key_generation`,
[chat_signing.rs:444-459](../crates/folia-core/tests/chat_signing.rs));
[BLOCKERS.md](../BLOCKERS.md) records ~1.4 s release / ~49 s debug for that
operation and warns it must not run on the tick thread. That is a warning, not
a login-time budget measurement.

### 3.1 20 TPS with 20+ players at defined render/simulation distance — `UNKNOWN`
No load-tester, tick-time budget test, or sustained-TPS measurement. The
watchdog only *reports* regions over 250 ms and is explicitly "reporting-only"
([SECURITY_REVIEW.md](../docs/SECURITY_REVIEW.md), section 6).
`tests/view_distance_join.rs` verifies view-distance bounds, not throughput.

### 3.2 Cold-world login reaches play within 2 seconds — `UNKNOWN`
No timed cold-login benchmark. `tests/worldgen_join.rs` asserts packet
correctness with a 30 s socket read timeout, not a latency budget.

### 3.3 Release targets and scaling — `UNKNOWN`
CI builds `--release` on both target OSes but does not measure performance
([ci.yml:48-49](../.github/workflows/ci.yml)). Debug chunk generation is
documented as much slower ([README.md](../README.md)). Per-region parallelism is
not real (global `WorldStorage` lock; P10.3/P10.5), so there is no scaling curve.

### 3.4 No crashes/panics under fuzzing and a 24-hour soak — `PARTIAL` / `UNKNOWN`
Fuzz-shaped deterministic tests pass for the parser surface (2.2), but there is
no continuous fuzzer and **no 24-hour soak test** anywhere in the tree. The
"no crashes" half is partially evidenced; the soak half is `UNKNOWN`.

---

## 4. Operations gates

### 4.1 CI green on Windows/Linux — `PASS` (definition present; last confirmed per STATE)
[ci.yml](../.github/workflows/ci.yml) runs `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, and `cargo build --workspace --release` on
`ubuntu-latest` and `windows-latest`; [codeql.yml](../.github/workflows/codeql.yml)
adds scheduled Java/Rust analysis. [STATE.md](../STATE.md) records "CI is green
at `1bb0e39`"; this audit did **not** re-run CI or query GitHub, and local HEAD
is `6afc2bd`, so green status must be re-confirmed on the release commit.
No secrets are referenced in the workflow
([SECURITY_REVIEW.md](../docs/SECURITY_REVIEW.md), section 7).

### 4.2 Packaging — `PARTIAL`
Dockerfile, `.dockerignore`, systemd unit, and a tested backup script exist; CI
produces release binaries ([OPERATOR_GUIDE.md](../docs/OPERATOR_GUIDE.md),
"Deployment without a Rust toolchain"). Gaps: no published GitHub Release
assets; Docker/shellcheck could not be validated locally (P11.7 note in
[STATE.md](../STATE.md)); systemd uses SIGINT because SIGTERM graceful shutdown
is not implemented.

### 4.3 Documentation — `PARTIAL` → `PASS` with this document
[README.md](../README.md) links Architecture, Configuration, Operator, Plugin
compatibility, World migration, and Contributing docs. The required release
docs are present; adding this acceptance-gates document and its README link
closes the documentation portion of P11.9.

### 4.4 Security review — `PARTIAL` (review done, release recommendation negative)
[SECURITY_REVIEW.md](../docs/SECURITY_REVIEW.md) contains the required release
checklist and one fixed remote DoS (NBT depth cap), but its own verdict is
"Do not expose this build directly to the public Internet yet" (line 270).
Open items: external `curl` for online-mode session checks, no connection/chat
rate limits, reporting-only watchdog, unsigned/untrusted plugin jars, partial
permission/event breadth. No secrets found in tracked source or CI.

### 4.5 Backups and shutdown — `PARTIAL`
`scripts/backup-world.sh` canonicalizes paths and prunes only inside
`BACKUP_DIR`; graceful shutdown flushes terrain/playerdata on Ctrl+C and console
`stop`, with `tests/shutdown_flush.rs` and `tests/shutdown_sessions.rs`
([OPERATOR_GUIDE.md](../docs/OPERATOR_GUIDE.md), "Backups and snapshots";
[SECURITY_REVIEW.md](../docs/SECURITY_REVIEW.md), section 3). Gaps: SIGTERM is not
handled; [OPERATOR_GUIDE.md](../docs/OPERATOR_GUIDE.md) notes `docs/` is missing
from the `snapshot.ps1` archive list; no automated backup-restore integrity test.

---

## 5. Summary table

| # | Gate (P11.9 item) | Status | Evidence / exact remaining blocker |
|---|---|---|---|
| 1.1 | Survival loop end to end | **FAIL** | No bed/sleep handler, no End dragon fight, no full-loop test. BLOCKERS 2026-11-05; P5/P7. |
| 1.2 | Worldgen parity ≥99.9% class / ≥99% exact | **FAIL** | 98.75% / 95.88%, single 16-column fixture; test floors only >98% / >91.5%. `worldgen_parity.rs`, `density_parity.rs`; P2.8 terrain adaptation/cross-chunk caching. |
| 1.3 | Lighting/updates byte-identical | **UNKNOWN** | No Java light-array or neighbor-update fixture. BLOCKERS P1.5/P3.10. |
| 1.4 | Redstone tick-exact suite | **UNKNOWN** | No Java redstone trace; tripwire hooks/analog inputs/persistence open. BLOCKERS P3.10. |
| 1.5 | Physics/combat traces within tolerance | **PARTIAL** | One armor-stand fall trace (18 samples, 5e-4); no broad corpus; live AI goals/ranges open (P5). |
| 1.6 | Tiered plugin corpus | **FAIL** | Only `example-plugin` + `BridgeApiTest`; P9.11 open. PLUGIN_COMPATIBILITY.md:7,42. |
| 1.7 | Multiplayer behavior/scale | **PARTIAL** | Cross-session/chat/tab/tracking tests; no 20-player load test; global world lock (P10.3/P10.5). |
| 2.1 | Differential fixtures | **PARTIAL** | Worldgen/join/physics fixtures only; long list of missing Java fixtures; P0.5 blocked. |
| 2.2 | No panics on fuzz input | **PARTIAL** | 10 `protocol_fuzz` + 5 `data_fuzz` deterministic tests; no continuous fuzzer, no soak. |
| 2.3 | Version guards | **PASS** | `VERSION.txt` + `asset_version.rs` (8 catalogs) + `assets.rs` guards; CRLF checkout caveat. |
| 2.4 | Save/load round-trips | **PARTIAL** | Anvil/playerdata/shutdown tests; pending ticks/comparator state not persisted. |
| 3.1 | 20 TPS, 20+ players | **UNKNOWN** | No benchmark or load test; watchdog is reporting-only. |
| 3.2 | Cold login < 2 s | **UNKNOWN** | No timing benchmark (30 s socket timeout is not a budget). |
| 3.3 | Release targets / scaling | **UNKNOWN** | Release build in CI but no perf job; no per-region world parallelism. |
| 3.4 | No crashes + 24 h soak | **PARTIAL / UNKNOWN** | Fuzz-shaped tests exist; no soak test. |
| 4.1 | CI green on Win/Linux | **PASS** (as recorded) | `ci.yml` matrix fmt/clippy/test/release; STATE claims green at `1bb0e39`; re-confirm on release commit. |
| 4.2 | Packaging | **PARTIAL** | Docker/systemd/backup exist; no Release assets; Docker/shellcheck unverified. |
| 4.3 | Documentation | **PASS** | Six docs linked from README plus this file. |
| 4.4 | Security review | **PARTIAL** | Review + NBT fix; "do not expose publicly yet" (SECURITY_REVIEW.md:270); P11.8/P11.9 gaps. |
| 4.5 | Backups / graceful shutdown | **PARTIAL** | Script + flush tests; SIGTERM unhandled; docs missing from snapshot archive. |

**Totals:** 3 PASS, 8 PARTIAL, 3 FAIL, 5 UNKNOWN, 1 mixed (3.4 PARTIAL/UNKNOWN)
across 20 rows; 4.1's PASS is conditional on re-confirming CI at the release commit.

---

## Verdict: 1.0 is NOT met

The acceptance gates for "1.0" are **not satisfied**. Three gates are outright
failures (survival loop, worldgen fidelity vs the 99.9%/99% thresholds, plugin
corpus), four performance/latency questions have no measurement at all, and the
security review explicitly advises against public exposure.

### Minimum remaining work to reach 1.0

1. **Close the three FAIL gates:** bed/sleep + End dragon fight and an
   end-to-end survival-loop test (P5/P7); raise worldgen fidelity to ≥99.9%
   class / ≥99% exact on a multi-seed fixture and raise the test floors to match
   P11.9 (P2.8, P0.5 oracle regeneration); assemble and run a tiered plugin
   corpus in CI (P9.11).
2. **Make the UNKNOWN gates measurable:** add Java differential fixtures for
   lighting and neighbor updates and a curated tick-exact redstone suite; add
   benchmarks/load tests for 20 TPS at 20+ players and cold-login < 2 s; add a
   24-hour soak test and a continuous fuzz harness.
3. **Finish correctness/operations gaps:** persist scheduled ticks and redstone
   state; add a stop → restart → backup-restore integrity test; re-confirm CI
   green at the release commit; publish release assets; add SIGTERM handling,
   package `docs/` in snapshots, and validate Docker/shellcheck.
4. **Act on the security review:** replace the `curl`-dependent online-mode
   check, add connection/chat rate limits, make the watchdog actionable, and
   constrain the `plugins/` trust boundary before any public exposure.
