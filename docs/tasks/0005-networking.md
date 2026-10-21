# 0005: Networking (M5)

**Milestone:** M5   **Crates touched:** net, netsim, server, tools   **Reviewers:** netcode + perf + /code-review

## Goal
A server-authoritative UDP stack: handshake, intent input, visibility-filtered and
delta-compressed snapshots, a reproducible lossy-link test harness, and measured bandwidth
inside the 20/40 KB/s downstream and 4 KB/s upstream budgets.

## Scope (landed)
- **Wire protocol** (`moba-net::protocol`): 26-byte packet header, versioning, packet kinds,
  handshake codecs, quantised commands (1/16 unit, map extent capped at 256), per-client network ids, full/delta
  snapshots with field masks, bounded decoders and a malformed-input fuzz test.
  - Since **v38** the handshake catalogue carries every hashed `RulesSpec` field (economy,
    rewards and win conditions), so any content pack can be joined and the client's rules match
    the server's; a per-field parity test and an exhaustiveness guard keep it complete.
- **Transport**: non-blocking UDP, application-level fragmentation/reassembly (1200-byte
  datagrams), ack tracker.
- **Server**: handshake and slot assignment, session tokens, input sequencing, bot fill for
  empty slots, per-client 64-snapshot history, delta baselines from client acks,
  visibility-filtered snapshots (teammates + known structures + enemies in team vision),
  timeouts, bandwidth counters.
- **Client**: handshake with retries, command retransmission until acked, snapshot history,
  delta application, full-resync request, keepalive/RTT, event queue.
- **prediction/reconciliation** (`moba-net::prediction`): the local hero responds to a
  click immediately; large errors snap (dash/respawn), lag-sized offsets are tolerated
  while moving, and idle positions converge smoothly to the authoritative one.
- **interpolation** (`moba-net::interpolation`): a 4-tick (133 ms) snapshot buffer lerps
  remote entity positions and deduplicates retransmitted/out-of-order snapshots.
- **playable network client**: `MOBA_SERVER=<addr|local>` renders snapshots with the local
  hero predicted; right-click move, left-click attack, Q/W/E cast, Space stop.
- **netsim**: per-client UDP link conditioner with base latency, jitter, seeded loss and
  reordering; optional one-way/round-trip shaping.
- **tools**: `soak` binary (10 clients, 150 ms RTT, 5% loss, 30 ms jitter, default 5 min).

## Non-goals / historical limits
- Server rewind compensation is not needed for the current kit: every aimed ability is a
  traveling projectile and basic attacks are server-targeted, so aim is not resolved
  against a stale snapshot. Revisit if hitscan abilities are added.
- Sanitizer-backed fuzzing landed in #117 (`fuzz/` + `fuzz.yml`); the deterministic fuzzer
  remains the fast local gate.
- HUD/fog/scoreboard and the wider match flow landed in M6; per-client content updates landed
  as the protocol-v24 `ContentUpdate` packet.

## Acceptance tests
- [x] Protocol round-trips, bounds checks and 5k-byte fragmented datagrams.
- [x] Snapshot full/delta build/apply, spawns/despawns, wrong-baseline rejection.
- [x] `client_connects_receives_snapshots_and_moves` over real UDP loopback.
- [x] Ten clients through per-client 150 ms/5%/30 ms links: all stay connected,
      130–178 snapshots each, RTT 160–210 ms.
- [x] Downstream 4.4–4.7 KB/s per client (budget 20 typical / 40 teamfight); upstream
      ~1.1 KB/s (budget 4).
- [x] Five-minute soak (`moba-tools soak --seconds 300`): all 10 clients stay connected,
      ~8,500 snapshots each, ~5.0 KB/s down, ~1.17 KB/s up, 169–201 ms RTT.
- [x] Prediction/reconciliation tests and interpolation tests.
- [x] Networked client renders server snapshots and predicts the local hero; readback
      capture with validation enabled.
- [x] Wire fuzzer: final v12 one-hour run, 2,384,035,000 decoder iterations clean (historical),
      plus current cargo-fuzz `wire`/`replay`/`content` targets and workflow.
- [~] Lag compensation: N/A for traveling projectiles/server-targeted attacks; documented.

## Netcode decisions (checked against prior art)
- **Interpolation delay**: the client buffers 4 ticks (133 ms) at 30 Hz. Glenn Fiedler's
  rule of thumb is ~3x the snapshot interval plus jitter to survive two consecutive lost
  packets; 4 ticks covers 2 x 33 ms loss + 30 ms jitter with margin. Linear interpolation is
  sufficient for ground-plane MOBA movement; Hermite splines are unnecessary until units
  have significant non-linear vertical motion.
- **Prediction**: movement is order-based, not physics replay. The client moves immediately,
  snaps on >8-unit errors (dash/respawn/clamp) and converges while idle; lag-sized errors
  are tolerated while moving instead of rubber-banding. Replaying unacknowledged inputs
  would require the server to timestamp orders by client tick; the current input model does
  not, so conservative dead-reckoning is the honest trade-off.
- **Lag compensation**: Source-style rewind compensates hitscan aim against stale targets.
  This kit has no hitscan player aim: every aimed ability is a travelling projectile and
  basic attacks are server-targeted, so the server resolves them against current state.
  Rewind becomes necessary if hitscan abilities are added.
- **Sequence wrap**: packet/input ordering uses RFC 1982 serial arithmetic, with a
  wrap-around test, so a long-lived session cannot misorder packets after 2^32 sends.
- **Fuzzing**: `just fuzz-wire` runs deterministic structured mutations through every
  decoder for one hour by default. This is a fast gate, not a replacement for sanitizer
  fuzzing, which joins M7.

References: [snapshot interpolation](https://www.gafferongames.com/post/snapshot_interpolation/),
[Valve lag compensation](https://developer.valvesoftware.com/wiki/Lag_Compensation),
[RFC 1982 serial arithmetic](https://www.rfc-editor.org/rfc/rfc1982).

## Protocol catalogue/event contract (v13–v26)
- `Welcome` carries one complete `WireCatalog` (manifest, items, heroes, abilities, map
  structures/walls) built only by `From<&GameContent>`; clients reconstruct it and verify
  `GameContent::hash`.
- v14 adds content-defined modifier definitions (name/kind) to the catalogue.
- v15 replaces ability kinds with targeting + effect lists (projectile on-hit, areas, zones).
- v16 adds a bounded per-snapshot presentation-event list and wide `status_bits`.
- v17 adds projectile arc height to wire effects and spawn events.
- v18 carries the map world extent in the handshake and derives simulation bounds from it.
- v19 carries tunable match rules (respawn, wave cadence) in the handshake.
- v20 adds team size / bot-fill policy to the rules.
- v21 adds level/XP curve and minion/tower vision to the rules for client-side HUD math.
- v22 carries runtime minion/tower combat stats.
- v23 carries the map display name for selection/rotation.
- v24 adds a full catalogue ContentUpdate packet for between-match hot reload.
- v26 carries the built-in map visual theme id.
- v24 catalogue updates replace the full catalogue in Lobby/Results; clients verify the hash,
  reset catalogue-dependent state and reconnect prediction.
- Abilities carry kind/cast range/radius/duration for HUD range and radius overlays.
- Stale protocol clients receive an explicit `Disconnect` reason instead of a silent drop.

## Budgets touched
Downstream ≤ 20 KB/s typical / 40 KB/s teamfight; upstream ≤ 4 KB/s. Measured far inside
both at 30 snapshots/s with server-side delta compression. See
`docs/perf/0005-networking.md`.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
