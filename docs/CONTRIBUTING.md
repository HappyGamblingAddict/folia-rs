# Contributing

Read `AGENTS.md` first, then `TODO.md` and `STATE.md`. Those files are the
authoritative task list and current focus; this page is a short workflow
summary. See [ARCHITECTURE.md](ARCHITECTURE.md) for how the code is organized.

## One-slice workflow

1. Read `STATE.md` and the next unfinished task in `TODO.md`.
2. Run `.\scripts\verify.ps1` to establish a baseline.
3. Pick the highest-priority unblocked task. If blocked, record the blocker in
   `BLOCKERS.md`, pick another task and keep working.
4. Read the relevant decompiled Java spec (local `../decompiled`) and the
   existing Rust code; do not translate Java line-by-line.
5. Write a failing Rust test that encodes the expected behavior.
6. Implement the smallest vertical slice that passes it.
7. Run the focused test, then `.\scripts\verify.ps1`.
8. Add/extend a Java-oracle differential fixture if the behavior needs one.
9. Update `TODO.md` status and `STATE.md` focus, record non-obvious choices in
   `DECISIONS.md`, and snapshot if the change was risky.

A task is done only when the code builds without warnings, a focused test
covers it, differential tests pass where an oracle exists, verification passes
(or failures are documented as pre-existing/unrelated), and docs/status are
updated. Never leave a fake implementation silently pretending to work; make
unsupported paths fail loudly and add a TODO.

## Required commands

```powershell
cargo fmt --all
cargo build --workspace
cargo test --workspace
cargo test -p folia-core --test worldgen_parity -- --nocapture
cargo test -p folia-core --test worldgen_join -- --nocapture
.\scripts\verify.ps1          # fast checks
.\scripts\verify.ps1 -Full    # includes generated-world join test + release build
```

## Test budget

Local work is **check-only** by default; CI owns the full matrix.

1. During a slice, run `cargo check -p <crate>` (or `cargo check --workspace`)
   and write focused tests; do not run the full suite locally.
2. Run `cargo fmt --all` before handing a slice back. Run clippy only for
   lint-sensitive changes; CI runs
   `cargo clippy --workspace --all-targets -- -D warnings`.
3. A focused test (`cargo test -p folia-core --test <slice>`) is optional for a
   high-risk change or when debugging a specific failure.
4. Never run two `cargo` commands against the same target directory
   concurrently; they only serialize behind the lock and can confuse timing
   tests.
5. Snapshot before risky work:
   `.\scripts\snapshot.ps1 -Label "before-<change>"`.

## Subagent expectations

- Work autonomously; `AGENTS.md` says not to ask the user questions. Pick the
  conservative vanilla/Folia behavior, record the choice in `DECISIONS.md`, and
  continue. If truly blocked, write the exact error in `BLOCKERS.md` and move to
  another task.
- Do not commit, branch, reset, stash, rebase or push unless explicitly asked.
  Git checkpoints are not part of the normal workflow.
- Never delete or revert work you did not make; inspect unfamiliar files first.
- Do not add gameplay logic to the Java bridge, and do not publish the
  decompiled reference tree.
- Keep changes to one vertical slice. After finishing, report exactly which
  files changed, any verification results, and any inaccuracies found in
  `TODO.md`/`STATE.md` for the coordinator to fix.
- Prefer a focused test with the change; differential-test packet/worldgen/
  physics changes against the Java oracle where practical.

## Recording decisions and blockers

- `DECISIONS.md` — non-obvious choices (e.g. defaults, RNG path, when to fail
  loudly). Written once, not rewritten to hide a change of course.
- `BLOCKERS.md` — exact blocker text, what is needed and what still works. The
  P0.5 entry (2026-11-21) records why oracle regeneration cannot run in this
  Linux checkout.
- `TODO.md` / `STATE.md` — task status and next focus, updated by the
  coordinator/loop, not by documentation-only slices.
- `crates/folia-core/assets/ASSETS.md` — pinned asset hashes and line counts;
  update together with `tests/assets.rs` when an asset is legitimately
  regenerated.

## Adding a differential fixture

1. Capture the same input from the Java oracle (the 1.21.11 Folia server in
   `../java-worldgen`, dump plugin in `../java-tools`, seed `12345`). Prefer the
   `scripts/dump-oracle.ps1` orchestration over manual copy/paste; `-Check`
   validates the current version/hashes without Java. In this Linux checkout
   the Java reference tree and `pwsh` are absent (P0.5 blocker), so capture on
   the Windows host and copy the normalized dump into
   `crates/folia-core/assets/`.
2. Pin it: add its byte length / FNV-1a hash / line count to
   `tests/assets.rs` and `ASSETS.md`, and add any new versioned JSON catalog to
   `asset_version.rs`.
3. Load it from Rust with `include_str!`/`include_bytes!` and write a focused
   test that compares Rust output with the captured Java output
   (`tests/worldgen_parity.rs` and `tests/density_parity.rs` are the models:
   `java-chunk-0--1.txt` lists `chunk`/`column` lines of `y=block` entries).
4. Report agreement thresholds honestly; do not claim parity for systems the
   fixture does not cover. If the extractor needs the Java jar, tests can skip
   its `-Check` with `FOLIA_SKIP_ITEM_ORACLE` / `FOLIA_SKIP_STRUCTURE_ORACLE`
   while still validating pinned bytes.

## Documentation

Docs live in `docs/`; this slice's pages are linked from `README.md` and
`HANDOFF.md`. Keep them factual and update them when behavior changes:

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [CONFIG_REFERENCE.md](CONFIG_REFERENCE.md)
- [OPERATOR_GUIDE.md](OPERATOR_GUIDE.md)
- [PLUGIN_COMPATIBILITY.md](PLUGIN_COMPATIBILITY.md)
- [WORLD_MIGRATION.md](WORLD_MIGRATION.md)
