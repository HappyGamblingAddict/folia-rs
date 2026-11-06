# Generated asset manifest

Target: **Minecraft/Folia 1.21.11, DataVersion 4788**. Do not mix assets from
another version.

`registry.rs`, `tags.rs`, and `loot.rs` embed several of these files with
`include_str!`/`include_bytes!`, so a missing or truncated file is a build or
ID-corruption problem, not a runtime fallback. `tests/assets.rs` pins the byte
length, FNV-1a-64 checksum and line counts listed below; update that test and
this file together when an asset is legitimately regenerated. Text assets are
hashed after folding CRLF to LF, so a Windows checkout with `core.autocrlf`
does not fail the manifest.

## Registry tables

| File | Content | Lines | Bytes | FNV-1a-64 |
|---|---|---:|---:|---|
| `block_states.tsv` | state id, block name, sorted `key=value` properties | 29,671 | 2,595,686 | `0x3da1dcdd58b4f738` |
| `block_state_behaviour.tsv.gz` | per-state air/motion/solid/collision/light/random-tick/occlusion/replaceable/hardness/blast/friction/speed/jump/fluid | 29,671 | 284,415 (gz) | `0x11e9f4a01cc0afc7` |
| `block_defaults.tsv` | block name, default state id | 1,166 | 37,605 | `0x7c29fe7e3a7407a8` |
| `item_ids.tsv` | item id, item name | 1,505 | 45,873 | `0xb3fc594089ab5bf7` |
| `entity_type_ids.tsv` | entity type id, entity type name | 157 | 3,772 | `0x4d7c78e0dac39e50` |
| `biome_ids.txt` | biome protocol ids, one per line | 65 | 1,681 | `0x377b853ef1311f77` |
| `biome_climate.tsv` | biome, base temperature, `temperature_modifier` | 65 | 2,096 | `0x41c1162ee67203b5` |

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
| `registries/banner_pattern.tsv` | 43 | 1,085 | `0xb28b4869b8a93aeb` |
| `registries/cat_variant.tsv` | 11 | 221 | `0x71d5f23602758e57` |
| `registries/chat_type.tsv` | 8 | 221 | `0x889ed8e3176583eb` |
| `registries/chicken_variant.tsv` | 3 | 56 | `0x91ed138a1da0880c` |
| `registries/cow_variant.tsv` | 3 | 56 | `0x91ed138a1da0880c` |
| `registries/damage_type.tsv` | 50 | 1,182 | `0x9ca3ed9996342bb1` |
| `registries/dialog.tsv` | 3 | 78 | `0x8706bf1479ffc1bc` |
| `registries/dimension_type.tsv` | 4 | 93 | `0xe19f26185e6afb71` |
| `registries/enchantment.tsv` | 43 | 1,020 | `0x431703eda3656761` |
| `registries/frog_variant.tsv` | 3 | 56 | `0x91ed138a1da0880c` |
| `registries/instrument.tsv` | 8 | 222 | `0xeae62ed2cafea822` |
| `registries/jukebox_song.tsv` | 21 | 405 | `0x3c92ea326c6200a7` |
| `registries/painting_variant.tsv` | 51 | 1,043 | `0x962b525bd33f8528` |
| `registries/pig_variant.tsv` | 3 | 56 | `0x91ed138a1da0880c` |
| `registries/test_environment.tsv` | 1 | 20 | `0x15a0acd1884e492c` |
| `registries/test_instance.tsv` | 1 | 24 | `0xb749aaa845c67f6a` |
| `registries/timeline.tsv` | 4 | 86 | `0x1be81df8928d6914` |
| `registries/trim_material.tsv` | 11 | 213 | `0x8101fd69b4ceda06` |
| `registries/trim_pattern.tsv` | 18 | 328 | `0x495c71f224487526` |
| `registries/wolf_sound_variant.tsv` | 7 | 125 | `0x4cd04d1716fb1647` |
| `registries/wolf_variant.tsv` | 9 | 168 | `0x4117e7f609594e04` |
| `registries/worldgen_biome.tsv` | 65 | 1,681 | `0x377b853ef1311f77` |
| `registries/zombie_nautilus_variant.tsv` | 2 | 39 | `0x05e2f622b4977cf0` |

## Loot tables and recipes

`scripts/extract_loot_recipes.ps1` normalizes the decompiled
`data/minecraft/loot_table` and `data/minecraft/recipe` trees into two compact,
deterministic catalogs. Every identifier field is namespaced, object keys are
sorted, insignificant whitespace is removed, and the top-level `counts` object
records the total plus a per-type index. `src/loot.rs` embeds and parses both
catalogs; unknown functions and conditions survive as typed `Opaque` variants.

| File | Content | Entries | Bytes | FNV-1a-64 |
|---|---|---:|---:|---|
| `loot_tables.json` | 11 table types: block 1,083, entity 114, chest 65, shearing 22, gift 21, archaeology 6, fishing 4, block_interact 4, equipment 3, entity_interact 1, barter 1 | 1,324 | 662,482 | `0x8c3b7afac27e3096` |
| `recipes.json` | 22 recipe types: shaped 706, shapeless 320, stonecutting 254, smelting 73, transmute 32, blasting 25, smithing_trim 18, smithing_transform 12, campfire 9, smoking 9, special/decorated 13 | 1,470 | 356,848 | `0x9b844794502a181f` |

## Item and structure catalogs

`scripts/extract_item_prototypes.ps1` dumps item default components from the
local 1.21.11 server jar. `scripts/extract_structure_data.ps1` normalizes the
`worldgen/processor_list` and `worldgen/template_pool` trees. Both emit sorted,
namespaced, whitespace-free JSON with a `counts` index.

| File | Content | Entries | Bytes | FNV-1a-64 |
|---|---|---:|---:|---|
| `item_prototypes.json` | 1,504 non-air items; 423 with overrides | 1,504 | 79,369 | `0xf179dfed29cdcdad` |
| `structure_processors.json` | 40 lists, 52 processors (block_rot 6, capped 4, protected_blocks 7, rule 35) | 40 | 46,334 | `0x446c0e7b069e66c4` |
| `template_pools.json` | 188 pools, 1,198 entries, 1,204 element nodes | 188 | 252,745 | `0xaa6434676fdae64c` |

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
- Loot tables and recipes: `.\scripts\extract_loot_recipes.ps1` from the repo
  root (reads `../decompiled/data/minecraft/{loot_table,recipe}`, writes
  `assets/loot_tables.json` and `assets/recipes.json`).
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
