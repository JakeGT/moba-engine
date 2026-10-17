# 0005: Networking (M5)

**Milestone:** M5   **Crates touched:** net, netsim, server, tools   **Reviewers:** netcode + perf + /code-review

## Goal
A server-authoritative UDP stack: handshake, intent input, visibility-filtered and
delta-compressed snapshots, a reproducible lossy-link test harness, and measured bandwidth
inside the 20/40 KB/s downstream and 4 KB/s upstream budgets.

## Scope (landed in this round)
- **Wire protocol** (`moba-net::protocol`): 26-byte packet header, versioning, packet kinds,
  handshake codecs, quantised commands (1/16 unit), per-client network ids, full/delta
  snapshots with field masks, bounded decoders and a malformed-input fuzz test.
- **Transport**: non-blocking UDP, application-level fragmentation/reassembly (1200-byte
  datagrams), ack tracker.
- **Server**: handshake and slot assignment, session tokens, input sequencing, bot fill for
  empty slots, per-client 64-snapshot history, delta baselines from client acks,
  visibility-filtered snapshots (teammates + known structures + enemies in team vision),
  timeouts, bandwidth counters.
- **Client**: handshake with retries, command retransmission until acked, snapshot history,
  delta application, full-resync request, keepalive/RTT, event queue.
- **netsim**: per-client UDP link conditioner with base latency, jitter, seeded loss and
  reordering; optional one-way/round-trip shaping.
- **tools**: `soak` binary (10 clients, 150 ms RTT, 5% loss, 30 ms jitter, default 5 min).

## Non-goals / still open
- Client-side prediction/reconciliation of the local hero and interpolation of remote
  entities (next round; the snapshot/ack/NetId foundation is ready).
- Server lag compensation for skillshots.
- Encryption/authentication: sessions are 32-bit tokens, not cryptographic; M7 hardening.
- Fuzzing harness runtime (short decoder fuzz lives in unit tests).

## Acceptance tests
- [x] Protocol round-trips, bounds checks and 5k-byte fragmented datagrams.
- [x] Snapshot full/delta build/apply, spawns/despawns, wrong-baseline rejection.
- [x] `client_connects_receives_snapshots_and_moves` over real UDP loopback.
- [x] Ten clients through per-client 150 ms/5%/30 ms links: all stay connected,
      130–178 snapshots each, RTT 160–210 ms.
- [x] Downstream 4.4–4.7 KB/s per client (budget 20 typical / 40 teamfight); upstream
      ~1.1 KB/s (budget 4).
- [x] Five-minute soak (`moba-tools soak --seconds 300`): all 10 clients stay connected,
      ~8,500 snapshots each, 4.1–4.5 KB/s down, ~1.17 KB/s up, 177–209 ms RTT.
- [ ] Prediction/reconciliation and lag compensation.

## Budgets touched
Downstream ≤ 20 KB/s typical / 40 KB/s teamfight; upstream ≤ 4 KB/s. Measured far inside
both at 30 snapshots/s with server-side delta compression. See
`docs/perf/0005-networking.md`.

## Review log
| Reviewer | Finding | Verdict | Resolution |
|---|---|---|---|
