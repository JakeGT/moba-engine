# Playtest runbook

How to run the M6 vertical-slice playtest: a host, up to ten players (bots fill the rest),
optional spectators, about 100 ms RTT, and the evidence to collect afterwards. The plan entry is
[WP-5.1/WP-5.2](plans/finish-plan-2026-10-20.md#phase-5-m6-playtest-needs-people); the
milestone criterion is in [status.md](status.md) (M6) and
[task 0006](tasks/0006-vertical-slice.md).

Every flag and environment variable below was checked against the code at protocol **v38**
(`main` @ `9a251a1`). The source of truth is cited next to each section; if a command here
disagrees with the code, the code wins and this file needs fixing.

## 1. Before the session

- **Same build everywhere.** The handshake rejects a different protocol version, and the
  v0.4.0 release archives speak protocol v37, not v38. Build every binary from the same commit
  or tag, or hand out archives from one `release.yml` run.
  ```bash
  cargo build --release --locked -p moba-server -p moba-client -p moba-tools --bins
  # target/release/{moba-server,moba-client,replay,...}
  ```
- **Clients need** a Vulkan 1.3 driver (see [AGENTS.md](../AGENTS.md#fresh-checkout--gpu)).
  They don't need a content file: the server sends its catalogue in the handshake.
- **Pick a host** with a public or VPN-reachable address. Only the host needs open ports.
- **Agree on a time limit.** `--seconds` is how long the host process runs before it shuts
  down, and the CLI default is only **60 s**. A full bot-filled 5v5 takes much longer, so set
  `--seconds 3600` or more.
- Send players [section 4](#4-join-as-a-player-or-spectator) and the
  [feedback form](#7-feedback-form) in advance.

## 2. Host the server

Source: `crates/server/src/main.rs` (flag parser and `ServerConfig`), [server.example.toml](../server.example.toml).

### Single match

```bash
moba-server --listen 0.0.0.0:7777 --seconds 3600 --ready \
  --record playtest/match.mbr --results-log playtest/matches.jsonl
```

The startup line shows the bound address, duration, countdown/results timings and the mode
(`plaintext LAN mode` or `encrypted`). Create the `playtest/` directory first; the server does
not create parent directories.

| Flag | Default | Notes |
|---|---|---|
| `--listen ADDR` | none | Required for network mode. Without it the binary runs a headless bot benchmark and exits. |
| `--seconds N` | 60 | Host lifetime; Ctrl-C also shuts down cleanly and flushes an in-progress replay. |
| `--ready` | off | The countdown waits until **every connected player** has pressed **R**. Without it, the countdown starts as soon as the first client connects. |
| `--countdown S` / `--results S` | 3 / 15 | Pre-match countdown and results screen. After results, the server starts a rematch with the next seed. |
| `--record FILE` | none | Replay per match (see [section 6](#6-collect-the-evidence)). |
| `--results-log FILE` | none | Appends one JSON line per *finished* match. |
| `--items FILE` | built-in pack | Runtime content pack; repeat the flag for a rotation. Verify replays with the same pack. |
| `--seed N` | `SimConfig` default | Seed of the first match. |
| `--max-pps N` / `--max-total-pps N` | 120 / 2000 | Per-address and global packet budgets. |
| `--config FILE` | none | TOML equivalent of the flags; CLI flags override it. |

Fixed per match: **10 player slots and 8 spectator slots**, a 5 s client timeout and a 60 s
reconnect grace (all hard-coded in `main.rs`). The `--ticks` flag only applies to the headless
benchmark, not to network mode.

### Several matches on one host (`--matches N`)

```bash
moba-server --listen 0.0.0.0:7777 --matches 2 --seconds 3600 --ready \
  --record playtest/match.mbr --results-log playtest/matches.jsonl
```

- Match *i* listens on port `7777 + i` (`7777`, `7778`, …), so players pick a match by port.
- Each match gets its own seed.
- With more than one match, the record and results paths get a `-mNN` suffix:
  `match-m00.mbr`, `matches-m01.jsonl` (`moba_net::host::suffixed_path`).
- `--workers K` sets the worker-thread count; the default is the number of logical cores.

### Plaintext or encrypted

Plaintext is the LAN default. **Don't run a plaintext server on the public internet**; see
[SECURITY.md](../SECURITY.md). For a WAN playtest, use one of these:

| Mode | Server | Client |
|---|---|---|
| Shared secret | `--secret HEX` (or `MOBA_SECRET` in the server's environment) | `MOBA_SECRET=HEX` |
| Per-player tokens (recommended) | `--allow-token HEX`, repeated once per player | `MOBA_SECRET=<that player's token>` |
| Pinned server identity (add to either) | `--server-key FILE` (file holds a 64-hex seed); prints `identity=<64 hex>` | `MOBA_SERVER_PUBKEY=<identity>` |
| Client identities (optional) | `--allow-client-key <public key>` | `MOBA_CLIENT_IDENTITY=<seed>`; see [README](../README.md#encrypted-private-servers) |

Every key is exactly 64 hex characters (32 bytes). Generate them like this:

```bash
mkdir -p playtest/keys && chmod 700 playtest/keys
openssl rand -hex 32 > playtest/keys/server.seed
for p in 1 2 3 4 5 6 7 8 9 10; do openssl rand -hex 32 > playtest/keys/token-p$p.txt; done
moba-server --listen 0.0.0.0:7777 --seconds 3600 --ready \
  --record playtest/match.mbr --results-log playtest/matches.jsonl \
  --server-key playtest/keys/server.seed \
  $(for f in playtest/keys/token-p*.txt; do printf -- '--allow-token %s ' "$(cat "$f")"; done)
```

Send each player their own token and the printed `identity=` value over a private channel.
Don't commit keys, tokens or seeds.

### Firewall and NAT

- The engine uses **UDP only**. Open the port range `7777` to `7777 + N - 1` inbound on the
  host, and forward the same UDP ports on the router if the host is behind NAT. Clients need no
  inbound rules.
  ```bash
  sudo ufw allow 7777:7778/udp                                  # Ubuntu/ufw
  sudo firewall-cmd --add-port=7777-7778/udp                    # firewalld (runtime only)
  ```
  ```powershell
  New-NetFirewallRule -DisplayName "MOBA playtest" -Direction Inbound -Protocol UDP -LocalPort 7777-7778 -Action Allow
  ```
- A mesh VPN (WireGuard, Tailscale) avoids port forwarding. Listen on `0.0.0.0` and have
  players join the host's VPN address.
- Several players behind one NAT are fine: the rate limiter is keyed per source address and
  port.

## 3. Bots, slots and spectators

Source: `crates/net/src/server.rs` (`refresh_bot_mask`, `free_slot`, `all_players_ready`).

- Bots fill every empty player slot: the server starts with `bot_mask 0x03FF`, and
  `rules.bot_fill` defaults to `true`. One human plus nine bots is a valid match.
- Joining players take the lowest free slot (0–9). The slot decides the team, and the HUD shows
  it (`SLOT n`).
- A player who goes silent for 5 s is dropped. Their slot stays reserved for 60 s, and a bot
  plays it in the meantime.
  - The shipped client **does not reconnect on its own**. It exits with `server disconnected`.
  - Relaunching it starts a new connection that takes a free slot. In a full 10-player lobby
    there is no free slot until the 60 s grace expires, so the relaunch is rejected until then.
  - Record every drop on the feedback form.
- Spectators use slots 10–17 (up to 8). They see both teams and don't count towards `--ready`.
- With `--ready`, the countdown starts once every connected player has pressed **R**. Bots and
  spectators don't count, and an empty lobby never starts. **F5** votes to surrender; the vote
  passes when every connected player on that team has voted.

## 4. Join as a player or spectator

Source: `crates/client/src/app/launch.rs` (`connect_network`, `hex32_env`),
`crates/client/src/app.rs` and `crates/client/src/settings.rs`.

```bash
MOBA_SERVER=203.0.113.10:7777 \
MOBA_SECRET=<your token> \
MOBA_SERVER_PUBKEY=<host identity> \
MOBA_VSYNC=0 MOBA_FRAME_LOG=frames-<name>.log \
  moba-client
```

```powershell
$env:MOBA_SERVER="203.0.113.10:7777"; $env:MOBA_SECRET="<your token>"
$env:MOBA_SERVER_PUBKEY="<host identity>"; $env:MOBA_VSYNC="0"
$env:MOBA_FRAME_LOG="frames-<name>.log"; .\moba-client.exe
```

| Variable | Meaning |
|---|---|
| `MOBA_SERVER=<ip:port>` | Dedicated server to join. Unset runs a local bot preview; `local` spawns an in-process server. |
| `MOBA_SECRET` | Shared secret or your personal token (64 hex). |
| `MOBA_SERVER_PUBKEY` | Pins the server identity (64 hex). An unsigned or wrongly signed server is rejected. |
| `MOBA_CLIENT_IDENTITY` | Your Ed25519 seed, only if the host requires client identities. |
| `MOBA_SPECTATE=1` | Join as a spectator. |
| `MOBA_FRAME_LOG=<path>` | Writes frame-time percentiles **when the client exits cleanly**. |
| `MOBA_VSYNC=0` | Uncapped present mode, so the frame log measures the frame cost rather than the display refresh. |
| `MOBA_SETTINGS=<path>` | Settings file (default `moba-settings.toml`): key bindings and UI scale. |

The three key variables **fail closed**. If one is set but isn't exactly 64 hex characters, or
the public key is malformed or weak, the client exits with an error instead of connecting
without protection.

Controls: right-click moves, left-click attacks, **Q/W/E** cast at the cursor, **Space**
stops, **B** opens the shop, **Tab** shows the scoreboard, **R** readies up, **F5** votes to
surrender and **Escape** quits. Quit with Escape, not by killing the process, or the frame log
isn't written. The HUD's top-left panel shows `PING n MS`; that is the RTT to report.

## 5. Simulate about 100 ms RTT

On a LAN, add latency on the **host** with `tc netem` (Linux, needs root). This delays only the
server's outgoing UDP game traffic, so SSH and everything else on the host are unaffected:

```bash
IF=eth0   # the host's interface towards the players: `ip route get <player ip>`
sudo tc qdisc add dev $IF root handle 1: prio bands 4
sudo tc qdisc add dev $IF parent 1:4 handle 40: netem delay 100ms 15ms distribution normal loss 1%
# One filter per match port (sport 7777, 7778, ...):
sudo tc filter add dev $IF protocol ip parent 1: prio 1 u32 \
  match ip protocol 17 0xff match ip sport 7777 0xffff flowid 1:4
tc -s qdisc show dev $IF            # the netem counters should rise during play
sudo tc qdisc del dev $IF root      # remove everything afterwards
```

- One-way delay on the server's egress adds the whole delay to every player's RTT. For 100 ms
  on a LAN with about 1 ms base RTT, use `delay 100ms`.
- The `prio bands 4` default priority map never selects band 4, so only the filtered game
  traffic is delayed.
- To shape a single machine's loopback instead, use `sudo tc qdisc add dev lo root netem delay
  50ms`. Loopback traffic crosses `lo` twice, so this gives about 100 ms RTT for **all** local
  traffic. Remove it with `sudo tc qdisc del dev lo root`.
- On a real WAN, don't add shaping; record the measured RTT instead.
- Check that the HUD's `PING` reads about 100 ms before starting the match.

The repo's own link conditioner (`crates/netsim`, `moba_netsim::LinkProxy`) is a library used by
the headless network tools, not a standalone proxy for the real client. Before the session, use
it as a quick check that the host machine and build hold the budget under the same conditions:

```bash
cargo run --release -p moba-tools --bin load-test -- \
  --matches 1 --clients 10 --seconds 60 --rtt 100 --jitter 15 --loss 1
cargo run --release -p moba-tools --bin soak -- --seconds 120 --rtt 100 --jitter 15 --loss 1
```

`load-test` exits non-zero if a client disconnects, misses a bandwidth budget or goes more than
1 s without a snapshot.

## 6. Collect the evidence

| Artifact | Where | Who |
|---|---|---|
| Server console log | `moba-server ... 2>&1 \| tee playtest/server.log` | host |
| Results log | `--results-log` file (`-mNN` suffix with `--matches`) | host |
| Replays | `--record` path (see the naming rules below) | host |
| Frame logs | each client's `MOBA_FRAME_LOG` file | every player |
| Feedback form | [section 7](#7-feedback-form) | every player |
| netem settings, or the measured WAN RTT | `tc -s qdisc show dev $IF` | host |

**Results log.** The server appends one line when a match *ends*, by nexus kill or surrender.
A match cut short by `--seconds` or Ctrl-C writes a replay but no results line. Each line looks
like this:

```json
{"match":1,"seed":…,"ticks":…,"outcome":"blue","blue_score":…,"red_score":…,"content":"…","content_version":"…","content_hash":"0x…","state_hash":"0x…","replay":"playtest/match.mbr"}
```

**Replays.** The first match in a server writes the `--record` path itself. Each rematch writes
`<stem>.match<N>.mbr`, for example `match.match1.mbr` and `match.match2.mbr`. With `--matches`,
the per-match suffix comes first: `match-m00.mbr`, then `match-m00.match1.mbr`. The results
line's `replay` field names the file for that match. Verify every replay:

```bash
for f in playtest/*.mbr; do
  cargo run -q --release -p moba-tools --bin replay -- "$f" || echo "FAILED: $f"
done
# If the host used --items PACK, pass the same pack:
cargo run -q --release -p moba-tools --bin replay -- --items content/example-pack.toml playtest/match.mbr
# Machine-readable output:
cargo run -q --release -p moba-tools --bin replay -- --json playtest/match.mbr
```

A good replay prints `verified   true`, and for a finished match its `state hash` equals the
results line's `state_hash`. A replay that doesn't verify is a determinism bug: file it with the
file attached.

**Frame logs.** On exit, the client drops the first 60 frames as warm-up and writes one line,
for example:

```text
frames=180 mean_ms=4.178 p50_ms=4.175 p95_ms=4.185 p99_ms=4.198 max_ms=4.496
```

The budget is a p95 of 6.9 ms or less and a maximum under 16.6 ms
([AGENTS.md](../AGENTS.md#performance-and-quality-budgets)). Ask players to send the file along
with their GPU, OS, driver and resolution. Software Vulkan (lavapipe) numbers are not hardware
evidence.

## 7. Feedback form

Copy this form for each player and each match.

```markdown
### Playtest feedback: <player name>, match <N>, <date>
- Build: <commit/tag>  OS/GPU/driver: <…>  Resolution/refresh: <…>
- Role: player slot <n> / spectator   Connection: LAN / WAN / VPN, netem <delay/jitter/loss or none>
- HUD ping (typical / worst): <ms> / <ms>   Perceived packet loss or stutter: none / rare / frequent
- Frame log line (from MOBA_FRAME_LOG): <frames=… p95_ms=… max_ms=…>
- Felt FPS / hitches: <smooth | occasional hitch | frequent hitches>, when: <teamfight, shop, …>

Rate 1 (bad) to 5 (good), with timestamps (match clock) for anything below 4:
- Rubber-banding (own hero snapping back / remote heroes jumping): <1-5> — <notes>
- Input latency (click/cast to visible response): <1-5> — <notes>
- Desync (state that looked wrong: health, gold, positions, kills): <yes/no> — <notes>
- Disconnects / kicks / "server disconnected" / rejected handshakes: <count> — <time, message>
- Audio/HUD/shop/minimap issues: <notes>
- Anything else: <notes>
```

The host adds one summary per match: the results line, the replay verification output, the
server's final `network done` line (`clients`, `rate_limited`, `avg_tick_us`, `max_tick_us`)
and the netem settings.

## 8. Turn findings into issues and update the docs

1. **One issue per finding** in <https://github.com/JakeGT/moba-engine/issues>, using the bug
   report template. Include the build commit, the match clock time, the matching feedback entries
   and the replay or frame log where relevant.
   - Labels: `networking` (rubber-banding, latency, drops), `renderer` (FPS), `simulation`
     (desync or a replay that fails to verify), `product` (game feel).
   - **Security findings** (handshake bypass, crash from a packet, information leaking through
     fog of war) are never filed publicly. Report them through a private security advisory per
     [SECURITY.md](../SECURITY.md).
2. **Record the evidence** in a new `docs/perf/` log, such as `docs/perf/0011-playtest.md`. Add
   the date, host hardware, network conditions, players (humans or bots), the results lines,
   replay verification output, per-client frame logs, a summary of the feedback and links to
   the filed issues. Quote only output you saw.
3. **Update the milestone.** Mark M6 done only with that recorded evidence:
   - [status.md](status.md): the M6 row, which says the 10-client real-network playtest has not
     been recorded.
   - [task 0006](tasks/0006-vertical-slice.md): Closeout, and add an acceptance box linking the
     playtest log.
   - The WP-5.2 entry in the [plan](plans/finish-plan-2026-10-20.md), and a CHANGELOG line.
   - If a finding blocks M6, leave it at 🟡 and link the blocking issues instead.

## Appendix: local smoke test (one machine)

The commands this runbook relies on were run on Linux with lavapipe (software Vulkan, so the
frame numbers are not hardware evidence) on 2026-10-27, `main` @ `9a251a1`:

```bash
cargo build --release --locked -p moba-server -p moba-client -p moba-tools --bins
# Plaintext host; player and spectator clients:
moba-server --listen 127.0.0.1:7777 --seconds 45 --countdown 1 --record s/match.mbr --results-log s/matches.jsonl
MOBA_SERVER=127.0.0.1:7777 MOBA_VSYNC=0 MOBA_FRAME_LOG=s/frames.log MOBA_SMOKE_FRAMES=300 moba-client
MOBA_SERVER=127.0.0.1:7777 MOBA_SPECTATE=1 MOBA_SMOKE_FRAMES=200 moba-client   # HUD: "MOBA SPECTATOR"
replay s/match.mbr                                                               # verified   true
# Two matches, per-player tokens, pinned identity, --ready:
moba-server --listen 127.0.0.1:7790 --matches 2 --seconds 40 --ready --server-key k/server.seed \
  --allow-token "$(cat k/token-p1.txt)" --allow-token "$(cat k/token-p2.txt)" ...
MOBA_SERVER=127.0.0.1:7790 MOBA_SECRET=$(cat k/token-p1.txt) MOBA_SERVER_PUBKEY=<identity> ... moba-client  # joins
MOBA_SERVER=127.0.0.1:7791 MOBA_SECRET=<wrong key> ... moba-client     # "server rejected the handshake"
# Two plaintext matches; one client on port 7791 -> replay written as match-m01.mbr, verified.
```

- `MOBA_SMOKE_FRAMES` makes the client exit after that many frames, so the frame log is written
  without a keypress.
- With `--ready` and no player pressing R, the match stayed in the lobby: `ticks=0` and no
  replay was written.
- A full match was **not** played to completion, so the results-log line wasn't produced
  end-to-end. Its format comes from `result_json` in `crates/net/src/server.rs` and its unit
  test.
- The `tc netem` commands were not run, because they need root. `load-test --rtt 100 --jitter 15
  --loss 1` with 10 clients passed (`max_snapshot_gap_ms=73`).
