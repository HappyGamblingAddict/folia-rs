# Generated asset manifest

Target: **Minecraft/Folia 1.21.11, DataVersion 4788**. Do not mix assets from
another version.

`registry.rs` and `tags.rs` embed several of these files with
`include_str!`/`include_bytes!`, so a missing or truncated file is a build or
ID-corruption problem, not a runtime fallback. `tests/assets.rs` pins the byte
length, FNV-1a-64 checksum and line counts listed below; update that test and
this file together when an asset is legitimately regenerated.

## Registry tables

| File | Content | Lines | Bytes | FNV-1a-64 |
|---|---|---:|---:|---|
| `block_states.tsv` | state id, block name, sorted `key=value` properties | 29,671 | 2,595,686 | `0x3da1dcdd58b4f738` |
| `block_state_behaviour.tsv.gz` | per-state air/motion/solid/collision/light/random-tick/occlusion/replaceable/hardness/blast/friction/speed/jump/fluid | 29,671 | 284,415 (gz) | `0x11e9f4a01cc0afc7` |
| `block_defaults.tsv` | block name, default state id | 1,166 | 37,605 | `0x7c29fe7e3a7407a8` |
| `item_ids.tsv` | item id, item name | 1,505 | 45,873 | `0xb3fc594089ab5bf7` |
| `entity_type_ids.tsv` | entity type id, entity type name | 157 | 3,772 | `0x4d7c78e0dac39e50` |
| `biome_ids.txt` | biome protocol ids, one per line | 65 | 1,681 | `0x377b853ef1311f77` |
| `biome_climate.tsv` | biome, base temperature, `temperature_modifier` | 65 | 2,161 | `0x70a2dbb761408aaa` |

## Vanilla tag tables

`scripts/extract_tags.ps1` reads the decompiled `data/minecraft/tags` tree and
flattens every nested `#tag` reference into one sorted `tag<TAB>member` row per
member. Empty tags emit no row (1.21.11 has two empty block tags:
`incorrect_for_diamond_tool` and `incorrect_for_netherite_tool`). `src/tags.rs`
embeds these files with `include_str!`; `tests/tags.rs` pins the tag and member
totals below.

| File | Content | Lines | Bytes | FNV-1a-64 |
|---|---|---:|---:|---|
| `tags/block.tsv` | 202 non-empty block tags (204 files; 2 empty) | 3,655 | 195,706 | `0x1a256d60d13e9b9e` |
| `tags/item.tsv` | 198 item tags | 1,897 | 94,764 | `0x6a67eb5ab9ee97f6` |
| `tags/entity_type.tsv` | 46 entity type tags | 368 | 17,999 | `0xbd26ce82b892592e` |
| `tags/fluid.tsv` | 2 fluid tags | 4 | 139 | `0x1aa52d0422f15857` |
| `tags/game_event.tsv` | 5 game event tags | 119 | 5,652 | `0x483b5e4ec73fbd60` |
| `tags/damage_type.tsv` | 33 damage type tags | 196 | 9,146 | `0x2678e89314c3df1c` |
| `tags/enchantment.tsv` | 22 enchantment tags | 308 | 14,614 | `0x6ebb8076d622d277` |
| `tags/point_of_interest_type.tsv` | 3 point-of-interest type tags | 30 | 1,273 | `0x7808a6390eaa7e0d` |
| `tags/worldgen.tsv` | 91 tags: biome 68, structure 20, flat preset 1, world preset 2 | 631 | 35,799 | `0xf2e03c737a041e7c` |

## Dynamic registry tables

`scripts/dump_registries.ps1` replays the configuration packets embedded in
`join_config.bin` and writes one `id<TAB>name` table per synchronized dynamic
registry. `src/registries.rs` embeds them and `tests/registries.rs` pins each
entry count.

| File | Entries | Bytes | FNV-1a-64 |
|---|---:|---:|---|
| `registries/banner_pattern.tsv` | 43 | 1,128 | `0x4b330b3ff4e508aa` |
| `registries/cat_variant.tsv` | 11 | 232 | `0x05cd22d8e03050d4` |
| `registries/chat_type.tsv` | 8 | 229 | `0xacb1f362d3ae4a4b` |
| `registries/chicken_variant.tsv` | 3 | 59 | `0x48029f4ad65a3a4b` |
| `registries/cow_variant.tsv` | 3 | 59 | `0x48029f4ad65a3a4b` |
| `registries/damage_type.tsv` | 50 | 1,232 | `0xde71cad60a14a36d` |
| `registries/dialog.tsv` | 3 | 81 | `0xc91a65335bb9ab3f` |
| `registries/dimension_type.tsv` | 4 | 97 | `0x858cec106a34bab5` |
| `registries/enchantment.tsv` | 43 | 1,063 | `0xcab664fa61919ac0` |
| `registries/frog_variant.tsv` | 3 | 59 | `0x48029f4ad65a3a4b` |
| `registries/instrument.tsv` | 8 | 230 | `0xbdf608bc5edb867a` |
| `registries/jukebox_song.tsv` | 21 | 426 | `0x08163fe859c036b0` |
| `registries/painting_variant.tsv` | 51 | 1,094 | `0x4965bd6bc4d59df3` |
| `registries/pig_variant.tsv` | 3 | 59 | `0x48029f4ad65a3a4b` |
| `registries/test_environment.tsv` | 1 | 21 | `0x0e4b680a9d036869` |
| `registries/test_instance.tsv` | 1 | 25 | `0x389b5dee903c0053` |
| `registries/timeline.tsv` | 4 | 90 | `0x8184152ad74b900a` |
| `registries/trim_material.tsv` | 11 | 224 | `0x57e7598597096a73` |
| `registries/trim_pattern.tsv` | 18 | 346 | `0x9df947cc5a98ff0c` |
| `registries/wolf_sound_variant.tsv` | 7 | 132 | `0x5118e2739e7863f0` |
| `registries/wolf_variant.tsv` | 9 | 177 | `0x54a3a5310b0c95d7` |
| `registries/worldgen_biome.tsv` | 65 | 1,746 | `0x6e4837e8fc4a38c0` |
| `registries/zombie_nautilus_variant.tsv` | 2 | 41 | `0x6188bf5d0dc16fd8` |

## Worldgen and math

| File | Content | Size | FNV-1a-64 |
|---|---|---:|---|
| `feature_data.json` | 224 configured features, 258 placed features, 65 biome feature lists, 2 tag groups | 295,053 B | `0xf68d8c86ee52ea65` |
| `mth_sin.bin` | 65,536 `f32` values of `Mth.SIN` | 262,144 B | `0x115b15b6ad9d2006` |

`feature_data.json` is loaded and validated by
`src/worldgen/feature/mod.rs`; execution during generation waits on placement
modifiers (`TODO.md` P2.2) and the chunk-status pipeline (P3.2).

## Java oracle fixtures

`java-density-grid.txt`, `java-biome-samples.txt`, `java-chunk-0--1.txt`,
`biome_params.txt`, `worldgen.json`, `join_config.bin` and `join_play.bin` are
captured from the Java capture server. They are used by differential tests and
are regenerated by the `java-worldgen` / `java-tools` dumps.

## How these files are produced

- Registry tables and `feature_data.json`: `cargo run -p folia-core --example
  dump_registries` against the Java capture server (see `PORTING.md`).
- Dynamic registries: `.\scripts\dump_registries.ps1` from the repo root (reads
  the embedded `join_config.bin`, writes `assets/registries/*.tsv`).
- Per-state block behaviour: `DumpBlockStateBehaviour.java` in the reference
  `java-tools` tree against the local `versions/1.21.11/folia-1.21.11.jar` plus
  `java-worldgen/libraries`; the 18 MB TSV is stored gzip-compressed as
  `assets/block_state_behaviour.tsv.gz`.
- Vanilla tag tables: `.\scripts\extract_tags.ps1` from the repo root (reads
  `../decompiled/data/minecraft/tags`, writes `assets/tags/*.tsv`).
- `biome_ids.txt`: `cargo run -q -p folia-core --example dump_biomes`.
- `biome_climate.tsv`: `.\scripts\extract_biome_climate.ps1` from the repo root
  (reads `../decompiled/data/minecraft/worldgen/biome`).
- `mth_sin.bin`: dumped from the Java `Mth.SIN` table.

## Recoverability

The assets are committed to Git in coherent slices (and
`scripts/snapshot.ps1` still produces local ZIP snapshots). Verify assets with:

```powershell
cargo test -p folia-core --test assets
```
