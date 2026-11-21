# World migration and formats

This page describes the world files this checkout reads and writes. **Migration
from other Minecraft versions is not implemented.** Worlds are generated and
saved in the 1.21.11 format only; do not point the server at a world from a
different DataVersion and expect an upgrade.

## Version guard

Target format: **Minecraft/Folia 1.21.11, DataVersion 4788**.

- `crates/folia-core/src/level.rs` defines `DATA_VERSION = 4788` and
  `TARGET_VERSION = "1.21.11"`. A world this server creates gets those values in
  `level.dat`.
- `crates/folia-core/src/asset_version.rs` validates the embedded generated
  assets (`assets/VERSION.txt` and the versioned JSON catalogs) at startup and
  in tests; a mismatch is a hard error, not a silent fallback.
- Generated assets are pinned by hash in `crates/folia-core/assets/ASSETS.md`
  and `tests/assets.rs`. See [OPERATOR_GUIDE.md](OPERATOR_GUIDE.md#oracle-regeneration-status-p05)
  for the P0.5 oracle situation.

`level.dat` parsing is tolerant of missing or mistyped fields (vanilla defaults
are used), so an incomplete file does not necessarily crash a load — but a
foreign DataVersion is not converted. The seed is taken from
`WorldGenSettings.seed`, falling back to `0` if the level file is missing or has
no seed.

## Level layout

With `level-name=world` (or `FOLIA_LEVEL_NAME=<name>`):

```text
<level-name>/
  level.dat                                   gzip NBT level header
  region/
    r.<x>.<z>.mca                             overworld Anvil regions
  dimensions/minecraft/the_nether/region/      Nether regions (partial support)
  dimensions/minecraft/the_end/region/         End regions (partial support)
run/
  playerdata/<uuid>.dat                        gzip NBT player state
  ops.txt                                      operator list
  world_state.txt                              time / weather / spawn
  <PluginName>/                                plugin data and config.yml
```

`level.dat` is a gzip-compressed NBT root containing a `Data` compound. The
subset this server reads/writes: `DataVersion`, `LevelName`, `GameType`, spawn
(`spawn.pos`, or legacy `SpawnX`/`SpawnY`/`SpawnZ`), `DayTime` (falling back to
`Time`), `LastPlayed`, `Version.Name`/`Version.Id`, `WorldGenSettings.seed`,
`hardcore`, `allowCommands`, plus the server's `folia:written` ownership marker.
On shutdown `record_session` updates day time and last played; the spawn is not
moved.

## Region files

Anvil `.mca` files are implemented in `crates/folia-core/src/anvil.rs`:

- 32x32 chunk slots, 4 KiB sectors, a 2-sector (8 KiB) location/timestamp
  header, and chunk payloads with a compression id (gzip, zlib or uncompressed;
  this server saves zlib by default).
- Chunks store block-state and biome palettes with packed indices, heightmaps,
  block entities, light data and the `folia:written` generation marker.
- `folia:written` carries `GENERATION_VERSION = 2`. Chunks stamped by an older
  generator capability are regenerated rather than served; this intentionally
  discards block edits in those chunks, so re-generating a level can lose edits
  made under an older build.
- `WorldStorage` caches up to 256 decoded chunks per dimension and writes region
  files as a whole, so exactly one `WorldStorage` exists per dimension per
  process (`shared_overworld`, `shared_dimension`). Running two servers against
  the same level directory at once is not safe.

## Loading foreign vs. self-created worlds

- A level this server created carries `folia:written` in `level.dat`: only chunks
  stamped `folia:written` are trusted; unstamped leftover chunks are regenerated
  instead of hiding generated terrain.
- A `level.dat` this server did not create has no marker, so every stored chunk
  is served as-is. `FOLIA_LOAD_STORED=1` forces that trust mode for any level.
  This is the closest thing to "opening an existing world" today, and it does
  **not** perform any version upgrade or block-ID remapping.
- Only the overworld `level.dat` is read/written; Nether/End sessions reuse its
  time, weather, spawn and seed, and no per-dimension level file is created.

## Backup / snapshot workflow

1. Stop the server with `stop`, `shutdown` or Ctrl+C so terrain and `level.dat`
   are flushed; confirm `shutdown complete`.
2. Take a filesystem snapshot of the level directory and `run/` (playerdata,
   ops, world state, plugin data). Copy them together — a partial copy can
   desynchronize level data and playerdata.
3. Ordinary risky-change snapshots use
   `.\scripts\snapshot.ps1 -Label "before-region-refactor"`, which writes
   `   snapshots/folia-rs-<timestamp>-<label>.zip`. That archive includes `world/`
   and `server.properties` when present, but not `run/` or `docs/`; add an
   explicit copy of `run/` for a full backup.
4. Restore by stopping the server and replacing the level directory and `run/`
   with the saved copies. Never restore while the process is running.

`server.properties`, `plugins/`, `run/`, `world/` and `snapshots/` are
git-ignored; snapshots are the recoverable copy, not Git history. Do not use Git
reset/rebase for recovery (see `AGENTS.md`).

## What is not implemented

- No DataVersion upgrade, block-ID or biome remapping between versions.
- No world conversion from Java/anvil worlds produced by other versions beyond
  serving stored chunks verbatim (`FOLIA_LOAD_STORED=1`); missing features and
  mismatched IDs are not translated.
- No per-dimension level data, no chunk compaction/garbage collection on
  rewrite, and no guarantee that a level from another server keeps all its
  content.
- Migration behavior is not covered by a differential test; do not treat it as
  supported.
