# Operator guide

This page covers building, running and stopping the server, plus verification
and backups. The project is a partial server; see the root `README.md` for the
current feature list.

## Requirements

- Rust 1.85+ (edition 2024).
- Native server: no Java required.
- Plugin jars: JDK/JRE 21 with `libjvm.so`/`jvm.dll`, the reconstructed Folia
  dependency tree (`../libraries` on the Windows development host) and the
  bridge built via `scripts/build-java.ps1`.

## Build and run

From the repository root (Windows/PowerShell commands as in `AGENTS.md`):

```powershell
cargo fmt --all
cargo build --workspace
cargo run --release      # use --release for real play; debug chunkgen is much slower
cargo run                # fine for protocol tests
```

Native-only mode, no JVM and no plugin jars needed:

```powershell
$env:FOLIA_DISABLE_PLUGINS = "1"
cargo run
```

On Linux/macOS the equivalent is `FOLIA_DISABLE_PLUGINS=1 cargo run`.

Other useful invocations:

```powershell
$env:FOLIA_MAX_TICKS = "40"; cargo run                       # bounded demo
$env:FOLIA_BIND = "127.0.0.1:25566"; cargo run                # explicit bind
$env:FOLIA_LEVEL_NAME = "genlive"; cargo run                  # scratch world
cargo run -p folia-core --example status_probe -- 127.0.0.1:25565
cargo run -p folia-core --example join_probe -- 127.0.0.1:25565
```

`server.properties` controls bind address/port, MOTD, player limit, level name
and view distance; see [CONFIG_REFERENCE.md](CONFIG_REFERENCE.md). Copy
`server.properties.example` to `server.properties` first if you want offline
mode and the documented defaults. The default bind is IPv4 `0.0.0.0:25565`;
`server-ip=::` selects IPv6-only. A port conflict is a hard error. Allow TCP
25565 through the OS firewall for LAN clients.

## Deployment without a Rust toolchain

CI already runs `cargo build --workspace --release` on both `windows-latest`
and `ubuntu-latest` ([ci.yml](../.github/workflows/ci.yml)); those release
binaries can be copied straight to a host with no Rust installed. The workflow
does not publish GitHub Release assets yet.

- [Dockerfile](../Dockerfile) and [.dockerignore](../.dockerignore) — build and
  run the native server in a container:

  ```sh
  docker build -t folia-rs .
  docker run -d --name folia -p 25565:25565 -v folia-data:/data folia-rs
  ```

  The image is non-root, exposes `25565/TCP` and mounts `/data` for
  `server.properties`, the level (`region/*.mca`, `level.dat`) and
  `run/playerdata`. Keep `/data` as the mount point: the binary's data root is
  baked from its build-time crate path, and the image builds the source at
  `/data` to match. A fresh volume gets the offline-mode
  `server.properties.example`. Plugins are off by default
  (`FOLIA_DISABLE_PLUGINS=1`); `-e FOLIA_DISABLE_PLUGINS=0` enables the bridge
  only if you supply a JVM-enabled image. The `HEALTHCHECK` is a TCP connect to
  port 25565 (liveness only, not a full status ping); adjust it if you change
  the port.

- [deploy/folia-rs.service](../deploy/folia-rs.service) — install as
  `/etc/systemd/system/folia-rs.service`, point `WorkingDirectory` and
  `ExecStart` at your release binary, create the `folia` user and `chown -R
  folia:folia` the data directory. It restarts on failure, enables
  `NoNewPrivileges`/`PrivateTmp` with a read-only system, and uses
  `KillSignal=SIGINT` because graceful shutdown is wired to Ctrl+C (and the
  console `stop` command), **not** SIGTERM yet. Give it `TimeoutStopSec` time
  to flush terrain before exit.

- [scripts/backup-world.sh](../scripts/backup-world.sh) — stop the server, then
  run with optional `BACKUP_DIR` and `KEEP` overrides (defaults: `<root>/backups`
  and 5):

  ```sh
  scripts/backup-world.sh
  BACKUP_DIR=/var/backups/folia KEEP=10 scripts/backup-world.sh
  ```

  It refuses to run without `world/region/` and `run/playerdata/`, archives the
  level `region/`, `level.dat` and `playerdata` with a timestamp, and deletes
  only old archives inside `BACKUP_DIR`.

## Console, restart and shutdown

- Console commands: `help`, `list`, `ops`, `op`, `deop`, `gamemode`, `stop`.
- `stop` (or `shutdown`) in the console and Ctrl+C both stop the loop cleanly.
- On shutdown the server stops sessions with a 5-second timeout, flushes
  terrain to Anvil, updates `level.dat` (day time / last played; spawn is not
  moved), disables plugins, and prints `shutdown complete`.
- Restart by running the same `cargo run` command again with the same
  `level-name`; the world and `run/` state are read back from disk. Avoid
  killing the process: an unflushed edit can be lost.

## Verification

One-command verification (PowerShell):

```powershell
.\scripts\verify.ps1          # fast checks
.\scripts\verify.ps1 -Full    # also generated-world join test + release build
```

The script checks required assets (including 23 dynamic registry tables), then
runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo test --workspace --no-fail-fast` and the `worldgen_parity`
differential test; `-Full` adds `worldgen_join` and a release build. It requires
PowerShell and assumes the Java reference material when tests invoke extractor
`-Check` (those can be skipped with `FOLIA_SKIP_ITEM_ORACLE` /
`FOLIA_SKIP_STRUCTURE_ORACLE`).

For local slice work, `AGENTS.md` defines a cheaper test budget: run
`cargo check -p <crate>` (or `--workspace`), write focused tests, run
`cargo fmt --all`, and leave the full matrix to CI. Never run two `cargo`
commands against the same target directory concurrently.

## World and runtime files

| Path | Content |
|---|---|
| `<level-name>/level.dat` | Gzip NBT level header: DataVersion 4788, seed, spawn, game mode, version, time/last played, `folia:written` marker. |
| `<level-name>/region/*.mca` | Overworld Anvil region files (zlib-compressed chunks by default). |
| `<level-name>/dimensions/minecraft/the_nether/region/`, `.../the_end/region/` | Dimension region directories (partial generation support). |
| `run/playerdata/` | Gzip NBT playerdata, including inventory and selected hotbar slot. |
| `run/ops.txt` | Operator list. |
| `run/world_state.txt` | Persisted world time, weather and spawn. |
| `plugins/` | Drop plugin `.jar` files here (git-ignored). |
| `run/<PluginName>/` | Plugin data folders and Rust-backed `config.yml`. |

A level without `level.dat` gets one on first join (1.21.11 / DataVersion 4788,
seed, spawn). Chunks this server writes are stamped `folia:written`; a
self-created level serves only stamped chunks so leftover chunks from another
source are regenerated. A foreign `level.dat`, or `FOLIA_LOAD_STORED=1`, serves
stored chunks as-is. See [WORLD_MIGRATION.md](WORLD_MIGRATION.md).

## Backups and snapshots

Git checkpoints are not part of the normal workflow (`AGENTS.md`): do not
branch/reset/stash/rebase/push unless explicitly asked. Before risky changes,
take a filesystem snapshot:

```powershell
.\scripts\snapshot.ps1 -Label "before-region-refactor"
```

It writes `snapshots/folia-rs-<timestamp>-<label>.zip` containing the tracked
docs, `crates/`, `java/`, `plugins/`, `scripts/`, `server.properties` and
`world/` when present. Note that `docs/` is not currently in that archive list.
`snapshots/` and `plugins/` are git-ignored. For world-only safety, stop the
server first (so terrain and `level.dat` are flushed) and copy the level
directory plus `run/`.

## Oracle regeneration status (P0.5)

`scripts/dump-oracle.ps1` regenerates the Java-derived fixtures via eight
extractors, refuses to overwrite existing assets without `-Force`, and records
VERSION.txt plus a SHA-256 manifest. `-Check` validates the version and pinned
hashes through the Cargo asset guard **without Java**. `tests/assets.rs` and
`crates/folia-core/assets/ASSETS.md` pin the same bytes.

Full regeneration is **blocked in this Linux checkout**: the external Java
reference tree from `AGENTS.md` (`decompiled/`, `java-tools/`, `libraries/`,
`java-worldgen/`, `folia-1.21.11-14.jar`) and `pwsh` are absent, so neither
`-Force` nor `-Check` can be executed here. The script was only statically
validated. On the Windows host with that reference tree, run:

```powershell
.\scripts\dump-oracle.ps1 -Force -ReferenceRoot <dir>
.\scripts\dump-oracle.ps1 -Check
```

Then confirm the printed SHA-256 manifest and the Cargo asset guards. No fake
assets were written; do not claim regeneration parity until that run succeeds.
See `BLOCKERS.md` (2026-11-21 entry) and TODO P0.5.

## Troubleshooting

- "could not bind ... Is another server already using this port?" — stop the
  other process or change `server-port`/`FOLIA_BIND`.
- LAN clients time out — allow TCP 25565 in the OS and any third-party
  firewall; use the LAN address printed at startup or `127.0.0.1` locally.
- Debug build feels unusably slow — use `cargo run --release`; chunk generation
  is CPU-bound.
- Asset version guard failure at startup — assets do not match 1.21.11 /
  DataVersion 4788; re-run the oracle check above, do not silently upgrade.
