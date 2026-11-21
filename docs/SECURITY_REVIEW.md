# Security review (P11.8)

**Date:** 2026-11-21
**Scope:** the current Folia-RS implementation in this checkout, reviewed against
the P11.8 checklist in `TODO.md`. This is a factual release checklist of what the
code actually does today, not a claim that the server is production-hardened.
The project's 1:1 target is *behavioral parity with Folia 1.21.11*; where a
vanilla-compatible behavior is itself insecure (notably offline mode), this
document says so rather than claiming it was fixed.

**Method:** static reading of the listed sources plus the deterministic fuzz
suites. No live exploit, network scan, or JVM plugin corpus was exercised here.

## Release checklist at a glance

| Area | Status | Notes |
|---|---|---|
| 1. Authentication | Partial / by design | Real online-mode Mojang session check exists; offline mode is intentionally insecure; HTTPS session check is shelled out to `curl`. |
| 2. Packet handling | Good, with one fix landed | 5-byte VarInt, 2 MiB frames, bounded strings/collections, NBT depth cap added this review; fuzz suites pin the parsers. |
| 3. Path traversal | Config-trust, no remote vector | All data paths are operator/local config or integer-derived; no attacker-controlled path component found. |
| 4. Plugin classloading | Known gap | Any jar in `plugins/` executes with full JVM privileges; no signed-plugin model or sandbox. |
| 5. Permissions | Partial | Operator checks gate native commands; Bukkit permission bridge is partial with documented gaps. |
| 6. DoS limits | Partial | Max players, socket timeouts and a reporting watchdog exist; no connection/chat/login rate limits. |
| 7. Secrets | Clean | No credentials or tokens found in tracked source, config, logs or CI; ephemeral RSA keys stay in memory. |

---

## 1. Authentication

- **Mode selection.** Effective runtime mode comes from `crate::config::online_mode()`
  (`crates/folia-core/src/config.rs:142-150`). `Online-mode` is parsed at
  `config.rs:97`; `FOLIA_ONLINE_MODE` overrides the file. Crucially, a *missing*
  `server.properties` resolves to **offline** (`is_some_and`), so running from a
  bare checkout does not silently require Mojang auth. The server binary calls
  `join::configure_online_mode(properties.online_mode)` at startup
  (`crates/folia-server/src/main.rs:45`), which also eagerly generates the
  process RSA key pair (`crates/folia-core/src/join.rs:1180-1185`).
- **Online-mode session verification.** In `handle_login`
  (`join.rs:1486-1537`), when online mode is on, `authenticate_login` runs the
  encryption handshake and calls `MojangSessionAuthenticator`
  (`join.rs:1213-1308`) against
  `https://sessionserver.mojang.com/session/minecraft/hasJoined`
  (`join.rs:1240`). `Authenticated` uses the profile UUID/name returned by
  Mojang; `Unverified` and `Unavailable` kick with vanilla translatable reasons
  (`join.rs:1167-1170`, `join.rs:1523-1535`). The URL is built with
  percent-encoding and `curl` is invoked with argument arrays, never a shell
  (`join.rs:1252-1266`, `join.rs:1277-1291`). Process/response are bounded by
  `--connect-timeout 5` and `--max-time 10` (`join.rs:1282-1285`).
- **Encryption setup.** `complete_encryption` (`join.rs:1391-1437`) sends an
  `EncryptionRequest`, requires a 4-byte verify token, decrypts the client's
  token and shared secret with RSA/PKCS#1 v1.5, compares the token in constant
  time (`crypto::constant_time_eq`, `join.rs:1417`), enforces a 128-bit AES
  secret (`join.rs:1423-1425`), installs AES-128/CFB8 *before any further packet
  is written* (`join.rs:1426`), then computes the vanilla `server_hash`
  (`crypto.rs:462-468`, `join.rs:1428`) for the session check. RSA key generation
  and the encrypt/decrypt primitives live in `crates/folia-core/src/crypto.rs`.
- **Status/login separation.** `status::handle_connection`
  (`crates/folia-core/src/status.rs:190-220`) reads the handshake and dispatches
  on `NextState`: status pings are answered by `handle_status`, while login and
  transfer are either handed to `join::handle_login` (when `accept_logins` is
  set) or rejected after reading the Login Start packet. A status ping cannot
  advance a client into the play state through this path.
- **Offline mode is intentionally insecure.** With online mode off, the server
  takes the username supplied in Login Start, derives a UUID from it
  (`join.rs:1539`), and skips both encryption and session verification
  (`join.rs:1511` is the only guard). Any client may claim any name/UUID. This
  matches vanilla offline behavior and is **not** safe for an exposed server;
  operators must set `online-mode=true` in `server.properties`.

## 2. Packet handling

- **VarInt / VarLong.** `crates/folia-core/src/varint.rs:3` caps encodings at 5
  and 10 bytes; `read_varint` rejects a sixth continuation byte
  (`varint.rs:47-61`) and the regression guard is in
  `tests/varint.rs`-equivalent `varint.rs:rejects_overlong_varint` and
  `protocol_fuzz.rs:75-101`.
- **Frame lengths.** `protocol.rs:15` defines `MAX_PACKET_SIZE = 2 MiB`;
  both the free `read_packet` (`protocol.rs:51-60`) and the encrypted
  `Connection` reader (`protocol.rs:123-160`) reject negative or oversized
  length prefixes before allocating.
- **String lengths.** `PacketReader::read_string` (`protocol.rs:1589-1597`)
  rejects negative lengths and any length larger than the bytes remaining in
  the frame, so a hostile prefix cannot force a large allocation; the writer
  caps at `i32::MAX` (`protocol.rs:1699-1704`). Protocol-level string bounds also
  exist (e.g. 16-char login name, 256-char chat/command at `protocol.rs:571-573`).
- **NBT depth and collection sizes.** `crates/folia-core/src/nbt.rs` bounds
  collection lengths to 16 MiB (`nbt.rs:6`, `checked_len` at `nbt.rs:380-386`),
  rejects negative byte-array/string lengths (`nbt.rs:360-369`), and — as of
  this review — caps nesting at `MAX_DEPTH = 512` (`nbt.rs:12`, enforced in
  `read_payload` at `nbt.rs:169-172` and on both recursive call sites at
  `nbt.rs:198`, `nbt.rs:210`). This matches the SNBT parser's existing 512 cap
  (`crates/folia-core/src/snbt.rs:26`, `snbt.rs:514-518`). Before the fix the
  binary NBT reader recursed without a limit, so a small network packet of
  nested lists could overflow the stack; see Finding 1 below.
- **Fuzz suites.** P8.9 `crates/folia-core/tests/protocol_fuzz.rs` (10 tests:
  VarInt boundaries/malformed values, random serverbound parsers, hostile frame
  lengths, strings, compression, NBT depth/truncation) and P11.3
  `crates/folia-core/tests/data_fuzz.rs` (5 tests: NBT/SNBT, chunk palettes,
  mutated asset JSON, item components). These are deterministic and in-process;
  parsers may return `io::Error` but must not panic or read outside the frame.

## 3. Path traversal

- **World/playerdata/config paths.** The world root is built as
  `root.join(&level_name)` (`crates/folia-server/src/main.rs:39-43`,
  `crates/folia-server/src/jvm.rs:298-300`) where `level_name` comes from the
  operator-owned `server.properties` (`config.rs:104`). Playerdata is fixed to
  `root/run/playerdata` (`main.rs:41`, `join.rs:1151-1156`) and keyed by a UUID,
  not a client-supplied filename. No remote packet supplies a path component.
  `level-name` is trusted operator configuration (vanilla also treats it as a
  path component); an operator can point it outside the root, so treat
  `server.properties` as sensitive configuration.
- **Plugin data folders.** `PluginHost.load` builds `new File(dataDirectory,
  description.getName())` from the plugin's own `plugin.yml`
  (`java/bridge/src/rs/folia/bridge/PluginHost.java:57-60`). The name comes from
  a locally installed jar, not a network peer; a malicious jar could choose a
  traversal-style name. Covered by the plugin trust note in section 4.
- **Snapshot/backup tooling.** `scripts/backup-world.sh` validates its inputs,
  canonicalises `WORLD_DIR`/`PLAYERDATA_DIR`/`BACKUP_DIR` with `cd ... && pwd`,
  refuses `BACKUP_DIR=/` and any backup location inside the data being backed
  up, and deletes only inside `BACKUP_DIR` (`backup-world.sh:24-42`, `70-79`).
  Paths come from operator-set environment variables.
- **Region file names.** `RegionPos::file_name` formats two integers from
  `chunk.div_euclid(32)` (`crates/folia-core/src/world.rs:23-37`), producing
  `r.<x>.<z>.mca`; no user string reaches a region path. Dimension subpaths are
  fixed constants (`world.rs:48-62`).

## 4. Plugin classloading

- **Isolation.** `PluginClassLoader` extends `URLClassLoader` and implements
  Paper's `ConfiguredPluginClassLoader` only far enough for `JavaPlugin`
  construction (`java/bridge/src/rs/folia/bridge/PluginClassLoader.java:20-38`).
  Each jar gets its own loader whose parent is the *host* loader
  (`PluginHost.java:62-66`), so plugins see Folia API classes. `getGroup()`
  returns `null` and `loadClass` ignores the `checkGlobal`/`checkLibraries`
  flags (`PluginClassLoader.java:35-38, 51-53`); this is not Paper's full
  dependency isolation model.
- **Native bridge entry points.** Plugin calls cross JNI into Rust adapters in
  `crates/folia-server/src/jvm.rs` (world/player/inventory/item/permission etc.);
  the ABI is grouped in `Bridge.java` and partially pinned by a Java API test
  (see `docs/PLUGIN_COMPATIBILITY.md:40`). No game logic runs in the bridge.
- **Unknown/untrusted jars.** `PluginHost.loadAll` loads *every* `*.jar` in the
  plugins directory with a `plugin.yml` and a `JavaPlugin` main class, in name
  order (`PluginHost.java:39-77`). There is no signature check, allowlist,
  checksum, or permission prompt. A jar that reaches `plugins/` executes
  arbitrary JVM code with the server process's full privileges (filesystem,
  network, native bridge). Plugin exceptions are caught per load/tick/disable
  and logged (`PluginHost.java:48-54, 84-98, 138-159`), but there is no sandbox
  or `SecurityManager`; failure containment is explicitly listed as missing in
  `docs/PLUGIN_COMPATIBILITY.md:41` (P9.12).
- **No signed-plugin model.** None exists. Deploy only jars you trust, with
  filesystem permissions on `plugins/` restricted to the operator account.

## 5. Permissions

- **Operator checks.** Operators are stored in `run/ops.txt`, loaded and saved
  by `join::configure_operators`/`save_operators`
  (`crates/folia-core/src/join.rs:894-944`); `add_operator`/`remove_operator` and
  `is_operator` are at `join.rs:951-972`. Native console/chat commands
  (`/gamemode`, `/op`, `/deop`, `/xp`, `/time`, `/weather`, `/gamerule`, `/stop`,
  etc.) call `is_operator` at their dispatch sites (e.g. `join.rs:2914`,
  `join.rs:5432-5816`). Operator names are matched case-insensitively
  (`join.rs:904`).
- **Bukkit/Permissions bridge.** `crates/folia-server/src/jvm.rs:449-462`
  exposes Rust-backed operator state to `hasPermission`/`isPermissionSet`, with
  explicit permission attachments and an operator-default fallback. The matrix
  in `docs/PLUGIN_COMPATIBILITY.md:29` records the current partial state.
- **Known gaps.** Wildcard/parent permission expansion and registration via
  `PluginManager.getPermission` are open (P9.5); permission-gated chat commands
  and full command trees are open (P9.5); live gameplay handlers in `join.rs`
  still bypass the plugin event bus (`PLUGIN_COMPATIBILITY.md:25-26`). Treat
  plugin-facing permission checks as advisory, not a security boundary.

## 6. Denial-of-service limits

- **Max players.** `join.rs:194-216` defines a configurable limit (default 20,
  set from `server.properties` via `configure_max_players`); `handle_login`
  rejects a new session with "Server is full" once the live count reaches it
  (`join.rs:1499-1508`). Negative limits are treated as zero.
- **Timeouts.** Status connections have 5 s read/write timeouts
  (`status.rs:190-192`); login uses 15 s (`join.rs:1489-1490`); live sessions
  poll keepalives at 10 s (`join.rs:3678`) with 50 ms short socket timeouts for
  shutdown/save responsiveness (`join.rs:1855-1856`, `join.rs:10560`). The
  Mojang session check is bounded by curl's 5 s/10 s timeouts.
- **Tick watchdog.** `crates/folia-core/src/region.rs:59` sets a 250 ms per-region
  budget; slow regions are timed and reported after the tick
  (`region.rs:1813`, `crates/folia-server/src/main.rs:140-146`). It is
  **reporting-only** — it does not kill or isolate a runaway region.
- **Missing rate limits (known gaps).** No per-IP connection rate limiting, no
  handshake/login flood throttle, no chat/command rate limiting, and no cap on
  concurrent status-handshake sockets were found. The status listener handles
  connections synchronously (`status.rs:153-169`), and each online-mode login
  spawns a `curl` process (`join.rs:1277`), so a flood can consume file
  descriptors/processes before the player limit is reached. Tracked as a known
  gap (P11.8 / P11.9 acceptance); no fix was attempted in this review to avoid
  changing network behavior without a differential test.

## 7. Secrets

- **No credentials found.** No password, API key, access token, bearer token or
  private-key material was found in tracked Rust/Java source, tracked config,
  or the reviewed CI workflow. RSA keys for online mode are generated in memory
  per process and never written to disk (`join.rs:1447-1457`).
- **No secret logging.** Login/crypto logs print the player name, UUID and
  generic error reasons, not the shared secret, verify token, or private key
  (`join.rs:1494-1497`, `join.rs:1412-1426`, `join.rs:1432-1435`).
- **Ignored/generated files.** `server.properties`, `world/`, `run/`, `logs/`,
  `plugins/` and `*.jar` are git-ignored (`.gitignore`), so operator secrets and
  runtime state are not committed. `server.properties.example` is a placeholder.
- **GitHub Actions.** The checked-in workflow
  (`.github/workflows/ci.yml`) uses no `secrets.*` references and no token
  variables; it sets only `FOLIA_DISABLE_PLUGINS=1`. Verified from the file;
  no repository/organization-level secret configuration was inspected.

---

## Findings and fixes in this review

### Finding 1 — fixed: unbounded binary-NBT recursion (stack-overflow DoS)

- **Impact:** `nbt::read_payload` recursed for every nested list/compound with no
  depth bound, while `snbt::MAX_DEPTH = 512` existed for the text parser. NBT is
  read from client-controlled packets via `PacketReader::read_nbt_any` /
  `read_nbt_root` (`protocol.rs:1603-1614`), used by item components, entity
  metadata components, chat and container titles. A few kilobytes of nested
  list headers (well under the 2 MiB frame cap) could exhaust the thread stack
  and abort the process — a remotely triggerable DoS with no authentication
  needed beyond reaching the login/config/play path.
- **Fix:** added `nbt::MAX_DEPTH = 512` and a depth guard in `read_payload`,
  threaded `depth + 1` through both recursive cases. Valid NBT at or below the
  cap is unchanged.
- **Tests:** `nbt::tests::rejects_excessive_nesting_depth`,
  `nbt::tests::accepts_nesting_below_the_limit`
  (`crates/folia-core/src/nbt.rs:507-529`) and
  `protocol_fuzz::nbt_network_deep_nesting_is_rejected`
  (`crates/folia-core/tests/protocol_fuzz.rs:311-325`).

### Known gaps (not fixed here)

- **Bounded NBT allocation amplification.** `nbt.rs` collection/byte-array
  lengths are capped at 16 MiB (`nbt.rs:6`), but a ~5-byte list/byte-array header
  in a small packet can still trigger a scratch allocation up to that cap before
  `read_exact` fails. It is bounded and transient, not unbounded, so it is
  documented rather than changed. TODO reference: P11.8 / P11.9.
- **Plugin trust and no signed-plugin model** — section 4. TODO P9.12.
- **Missing rate limits and reporting-only watchdog** — section 6. TODO P11.9.
- **Permission/event breadth** — section 5. TODO P9.5 / P9.3.
- **Online-mode depends on an external `curl` binary and Mojang's HTTPS
  endpoint** — section 1. A missing/failing `curl` yields `AuthError::Unavailable`
  and a clean kick, but online-mode servers must have it available.

## Verification performed

Run in this checkout after the source change:

- `git diff --check` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo test -p folia-core --test protocol_fuzz --test data_fuzz` — 10 + 5
  passed, 0 failed.
- `cargo test -p folia-core --lib nbt::tests` — 21 passed, 0 failed.
- `cargo test -p folia-core --test anvil --test world_round_trip --test
  structure_template --test assets` — all passed (NBT consumer regression
  check).

No Docker, network, or Java toolchain was needed. No commit, branch, or history
operation was performed.

## Release recommendation

Do not expose this build directly to the public Internet yet. Minimum before a
public release: (1) require `online-mode=true` and a TLS-capable session-check
path that does not depend on an external `curl` process, (2) add connection and
chat/login rate limits and make the watchdog actionable, (3) restrict/allowlist
the `plugins/` directory or add signed-plugin verification, and (4) close the
permission/event gaps so plugins cannot be treated as a trust boundary. The NBT
depth fix above removes one concrete remote crash and should be kept.
