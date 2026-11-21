# Configuration reference

This page lists the configuration this checkout actually reads. Unknown keys
are preserved in `ServerProperties::extra` but have no effect. Source of truth:
`crates/folia-core/src/config.rs` and `crates/folia-server/src/main.rs`.

`server.properties` uses Java `java.util.Properties` syntax: `=`, `:` or
whitespace separate key and value; `#`/`!` start comments; `\t`, `\n`, `\r`,
`\uXXXX`, escapes and backslash continuation lines are supported. The binary
looks for the file in the current directory, the repository root
(`CARGO_MANIFEST_DIR/../..`) and the executable directory, in that order.

## `server.properties` keys

| Key | Default | Effect in this checkout |
|---|---|---|
| `server-ip` | empty | Bind address. Empty or `0.0.0.0` = all IPv4 interfaces; `::` = IPv6-only; otherwise a specific address. `FOLIA_BIND` overrides the whole bind target. |
| `server-port` | `25565` | TCP port for the status/login server. A port conflict is a hard startup error. |
| `motd` | `A Minecraft Server` | Server-list MOTD (and legacy ping text). |
| `max-players` | `20` | Server-list limit and login cap; values `<= 0` are clamped to `0`, rejecting every login. |
| `online-mode` | `true` (struct default) | Forwarded to the session layer. Online-mode login/auth is not usable in the binary yet (HANDOFF/`BLOCKERS.md`); keep `false` for a working join. See the note below. |
| `prevent-proxy-connections` | `false` | Parsed; would pass the client IP to the session check. No practical effect until online-mode is wired. |
| `enforces-secure-chat` | `false` | Reject unsigned/invalid player chat when enabled; prints a startup audit line. `enforce-secure-profile` is accepted as an alias. Opt-in because profile-key provisioning is not complete. |
| `level-name` | `world` | World root directory (`<level-name>/level.dat`, `<level-name>/region/*.mca`). `FOLIA_LEVEL_NAME` overrides it. |
| `view-distance` | `10` | Live chunk-stream radius; clamped to a hard cap of 16 and to the client's requested radius (the smaller wins). |
| `simulation-distance` | `10` | Parsed and stored, but **not applied**: the Join Game simulation-distance field currently uses the effective view radius. |
| `gamemode` | `survival` | Default game mode for new players (`survival`/`creative`/`adventure`/`spectator`; unknown values map to survival). |
| `generate-flat-bedrock` | `false` | Paper-style surface toggle, accepted here directly from `server.properties`. `true` keeps bedrock flat; `false` preserves captured random bedrock depths. `FOLIA_GENERATE_FLAT_BEDROCK` overrides it. |
| `enable-command-block` | `false` | Parsed/stored only; no runtime effect yet. |
| `allow-flight` | `false` | Parsed/stored only; no runtime effect yet. |
| `hardcore` | `false` | Parsed/stored only; no runtime effect yet. |
| `pvp` | `true` | Parsed/stored only; no runtime effect yet. |
| `white-list` | `false` | Parsed/stored only; no whitelist enforcement yet. |
| `difficulty` | `easy` | Parsed/stored only; no runtime effect yet. |
| `spawn-protection` | `16` | Parsed/stored only; no protection enforcement yet. |

A ready-to-copy template is `server.properties.example` (git-ignored as
`server.properties`). Note its `motd` differs from the code default.

### `online-mode` caveat

`ServerProperties::default()` sets `online-mode=true`, and `main.rs` forwards
that value to `join::configure_online_mode` even when `server.properties` could
not be read. The separate fallback helper `config::online_mode()` (used only
when the session layer was never configured, e.g. tests/examples) treats a
missing file as offline and honors `FOLIA_ONLINE_MODE`. Because the binary does
not support online-mode logins yet, run with `online-mode=false`. This
inconsistency is worth checking by the coordinator (see the final report for
P11.6).

## Environment variables

| Variable | Effect |
|---|---|
| `FOLIA_BIND` | Overrides `server-ip`/`server-port` with a single bind target (e.g. `127.0.0.1:25566`). |
| `FOLIA_LEVEL_NAME` | Overrides `level-name`. |
| `FOLIA_MAX_TICKS` | Stops the native loop after N ticks (bounded demos/tests). |
| `FOLIA_DISABLE_PLUGINS=1` | Forces fully native startup; no JVM is loaded even if jars exist. |
| `FOLIA_LIBRARIES` | Dependency-jar root for `scripts/build-java.ps1` and the plugin class path in `jvm.rs`. |
| `JAVA_HOME` | JVM location; auto-detected on common Windows Java 21 paths, set explicitly elsewhere. |
| `FOLIA_ONLINE_MODE` | Overrides `online-mode` only for the fallback `config::online_mode()` path (see caveat above). |
| `FOLIA_ENFORCES_SECURE_CHAT` | Overrides `enforces-secure-chat`. |
| `FOLIA_GENERATE_FLAT_BEDROCK` | Overrides `generate-flat-bedrock`; useful to pin deterministic captures. |
| `FOLIA_LOAD_STORED=1` | Trust every stored chunk in region files, including unstamped ones (e.g. loading an existing world) instead of regenerating them. |
| `FOLIA_BRIDGE_SELFTEST=1` | Makes the example plugin exercise the bridge API, events and cancellation. |
| `FOLIA_BRIDGE_SELFTEST_PLAYER` | Selects the player name used by the bridge self-test. |
| `FOLIA_BRIDGE_KICK_TEST=1` | Extra kick-path check inside the example plugin self-test. |
| `FOLIA_SERVER_JAR`, `FOLIA_SERVER_LIBRARIES` | Inputs for the oracle extractor scripts (`scripts/extract_item_prototypes.ps1`, `scripts/dump-oracle.ps1`). |
| `FOLIA_SKIP_ITEM_ORACLE`, `FOLIA_SKIP_STRUCTURE_ORACLE` | Skip extractor `-Check` invocations in item/structure asset tests when the Java toolchain is unavailable. |

## Other server files

- `server-icon.png` — optional server-list favicon, loaded from the repository
  root at startup.
- `run/ops.txt` — operator list used by console commands and permission
  defaults.
- `run/playerdata/*.dat` — gzip NBT player state.
- `run/world_state.txt` — persisted world time/weather/spawn.
- `run/<PluginName>/` — plugin data folders and bridge-backed `config.yml`.

See [OPERATOR_GUIDE.md](OPERATOR_GUIDE.md) for how to run with these settings
and [WORLD_MIGRATION.md](WORLD_MIGRATION.md) for level data layout.
