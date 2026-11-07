# AGENTS.md — Folia-RS Autonomous Work Guide

This file is the operating manual for any agent continuing the Rust rewrite of
Folia 1.21.11. Read this first, then `TODO.md`, then `STATE.md`.

## Mission

Build a behaviorally 1:1 Rust server matching Folia 1.21.11, while keeping real,
unmodified Bukkit/Paper `.jar` plugins working.

“1:1” means **behavioral parity**, not line-by-line transliteration:

- Same generated worlds for the same seed.
- Same block/item/entity IDs and packet bytes.
- Same physics, damage, redstone, inventory, and tick ordering.
- Same public plugin API behavior.
- Same Folia region/threading model.

The decompiled Java is the specification. Rust is the implementation. Java is
only allowed at the plugin-compatibility boundary.

## Non-negotiable rules

1. **Git checkpoints only on explicit request.** Do not branch, reset, stash,
   rebase, or push unless the user asks. Ordinary work leaves the tree
   uncommitted and uses `scripts/snapshot.ps1` before risky changes. When the
   user asks to commit, commit the working tree in coherent slices and never
   rewrite existing history.
2. **Do not ask the user questions.** Make the most conservative choice that
   matches vanilla/Folia behavior, record it in `DECISIONS.md`, and continue.
   If blocked, write the blocker in `BLOCKERS.md`, pick another unblocked task,
   and keep working.
3. **Never claim parity without a test.** Every behavior change needs a Rust
   test; packet/worldgen/physics changes need a differential test against the
   Java oracle where practical.
4. **Never delete user work.** If a file looks unrelated or modified, inspect it
   before editing. Do not revert or overwrite changes you did not make.
5. **Keep the build green.** Run the verification script after each task. Fix
   regressions before starting the next task.
6. **Use the decompiled source as a spec.** Do not copy decompiled Java
   wholesale into Rust or add game logic to the Java bridge.
7. **Do not publish the decompiled Mojang/Paper tree.** It is local reference
   material, not part of the Rust project.
8. **One vertical slice at a time.** Prefer a small end-to-end working feature
   with tests over a large half-finished subsystem.

## Current environment

Project root:

```text
C:\Users\austr\Downloads\rustfol\folia-rs
```

External reference material (do not edit):

```text
C:\Users\austr\Downloads\rustfol\decompiled          # decompiled Folia 1.21.11
C:\Users\austr\Downloads\rustfol\java-worldgen       # Java capture server + world
C:\Users\austr\Downloads\rustfol\java-tools          # dump plugins/tools
C:\Users\austr\Downloads\rustfol\libraries           # reconstructed dependency tree
C:\Users\austr\Downloads\rustfol\folia-1.21.11-14.jar
```

Target version: **Minecraft/Folia 1.21.11, DataVersion 4788**. Do not silently
upgrade.

## Required commands

```powershell
cd C:\Users\austr\Downloads\rustfol\folia-rs

cargo fmt --all
cargo build --workspace
cargo test --workspace
cargo test -p folia-core --test worldgen_parity -- --nocapture
cargo test -p folia-core --test worldgen_join -- --nocapture

# Native server, no Java required
$env:FOLIA_DISABLE_PLUGINS = "1"
cargo run
```

One-command verification:

```powershell
.\scripts\verify.ps1          # fast checks
.\scripts\verify.ps1 -Full    # includes generated-world join test
```

## Test budget (avoid useless runs)

Do not run the whole suite after every edit. Use this budget:

1. During a slice, run only the focused target(s): `cargo test -p folia-core
   --lib <module>` and/or `cargo test -p folia-core --test <slice>`.
2. Run `cargo fmt --all` and one `cargo clippy --workspace --all-targets -- -D
   warnings` before handing the slice back.
3. Run the full `cargo test --workspace` and `worldgen_parity` exactly once
   when merging the slice into the integration branch.
4. Run `worldgen_join`, `view_distance_join`, and `scripts/verify.ps1 -Full`
   only for changes that touch generation, join streaming, or shutdown. They
   take tens of seconds and are not needed for unrelated edits.
5. Never run two `cargo` commands against the same target directory
   concurrently; it only serializes behind the lock and can confuse timing
   tests.

Snapshot before risky work:

```powershell
.\scripts\snapshot.ps1 -Label "before-region-refactor"
```

## Required files

Before starting, verify these exist:

- `crates/folia-core/assets/block_states.tsv`
- `crates/folia-core/assets/block_defaults.tsv`
- `crates/folia-core/assets/block_state_behaviour.tsv.gz`
- `crates/folia-core/assets/block_shapes.tsv.gz`
- `crates/folia-core/assets/item_ids.tsv`
- `crates/folia-core/assets/entity_type_ids.tsv`
- `crates/folia-core/assets/feature_data.json`
- `crates/folia-core/assets/biome_params.txt`
- `crates/folia-core/assets/biome_ids.txt`
- `crates/folia-core/assets/mth_sin.bin`
- `crates/folia-core/assets/worldgen.json`
- `crates/folia-core/assets/structure_templates.json` (483 village templates)
- Java oracle dumps in `assets/` and/or `../java-worldgen`

If an asset is missing, regenerate it from the local jar/decompiled data or the
Java oracle. Do not ask the user; document the workaround in `DECISIONS.md`.

## The autonomous work loop

Repeat forever:

1. Read `STATE.md` and the next unfinished task in `TODO.md`.
2. Run `.\scripts\verify.ps1` to establish the current baseline.
3. Choose the highest-priority **unblocked** task. If the current task is
   blocked, record the blocker and move to the next one.
4. Read the relevant decompiled Java classes and existing Rust code.
5. Write a failing Rust test that encodes the expected behavior.
6. Implement the smallest vertical slice that passes.
7. Run the focused test, then `.\scripts\verify.ps1`.
8. If a differential fixture is needed, add it or extend the Java oracle dump.
9. Update the task status in `TODO.md` and the current focus in `STATE.md`.
10. Record any non-obvious decision in `DECISIONS.md`.
11. If the change was risky or touched many files, run `scripts/snapshot.ps1`.
12. Move immediately to the next unblocked task.

Do not stop after one task. Continue until:

- the user interrupts,
- a hard blocker requires credentials/hardware/network access that is not
  locally available,
- a destructive action outside the workspace is required, or
- the acceptance gates for “1.0” are actually met.

## Decision policy (avoid asking)

When a choice is ambiguous:

| Situation | Default choice |
|---|---|
| Vanilla vs Paper behavior | Follow Folia/Paper 1.21.11 |
| Multiple implementation designs | Choose the simplest one with a differential test |
| Unsure about an ID/order | Read `decompiled` and/or dump from the Java server |
| Missing optional data | Generate it locally; if impossible, stub explicitly and continue another task |
| Performance vs correctness | Correctness first; optimize after parity |
| Plugin API ambiguity | Follow Bukkit/Paper Javadocs and real plugin behavior |
| Network/download needed | Use local copies; do not add a dependency unless necessary |
| Large task | Split it into smaller tasks in `TODO.md` and complete one slice |
| Tests failing in unrelated code | Fix the regression or document why it is pre-existing |

Never leave a fake implementation silently pretending to work. Either implement
it, or fail loudly with an explicit unsupported path and a TODO entry.

## Definition of done for every task

A task is done only when:

- The Rust code compiles with no warnings.
- A focused test covers the behavior.
- Differential tests pass where a Java oracle exists.
- `.\scripts\verify.ps1` passes (or the only failures are documented,
  pre-existing, and unrelated).
- `TODO.md` is updated.
- `STATE.md` points to the next task.
- No unsupported behavior is silently ignored.

## What to work on first

The current active queue is:

1. `P2.8` - recursive jigsaw connection and structure generator wiring.
2. `P6` - typed block entities (chest/furnace/sign/spawner) and pistons.
3. `P5` - join-session entity spawning/tracking and AI tick integration.
4. `P1.5` - shape-aware light occlusion and remaining support/waterlogging.
5. `P9.3` - bridge live-session state and gameplay event dispatch.

Item entities/XP orbs from the table-driven block drops are now modeled
(`item_entity.rs`, `spawn_drops_at` + `DigState::stop_with_entities`, 16
`tests/item_entities.rs` + 6 lib tests; `join.rs` wiring remains open).
`TODO.md` is authoritative if this list is stale.

## Architecture reminders

- Rust owns the process, world, protocol, tick loop, and gameplay.
- The embedded JVM is lazy and exists only for plugin compatibility.
- Do not add gameplay logic to the Java bridge.
- Chunk generation must flow through the chunk-status pipeline; features must
  not write directly into arbitrary chunks.
- Design per-region ownership before spreading gameplay state across global
  statics. Folia threading is not a bolt-on.
- Region ownership is enforced in `region.rs` for tasks: `Region` owns its
  queue, world-slice token, and tick counter; `RegionOwned<T>`/`OwnedWorld`
  reject cross-region access with `OwnershipError`; `RegionMap` uses vanilla
  8x8 chunk / 4x4 section regions. World/entity/player state is not yet
  region-owned, so do not claim region isolation for it (see `BLOCKERS.md`).

## Recovery

If the build is broken:

1. Read the first error, not the last.
2. Run the focused test that failed.
3. Use `scripts/snapshot.ps1` snapshots to compare recent changes if needed.
4. Do not use Git to reset.
5. If an asset is corrupt, regenerate it from the oracle.
6. If still blocked, record the exact error in `BLOCKERS.md` and move to another
   task.

## Honest expectation

A complete behavioral 1:1 Folia port is a multi-year, multi-agent effort. The
agent should optimize for **steady, tested progress**, not for declaring victory.
The TODO’s acceptance gates, not a single session, define completion.
