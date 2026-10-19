# Performance log — M5 networking

Historical benchmark measured 2026-10-17 on protocol v12. Current protocol is v25; the extra
catalogue/rules/map/theme fields are small or handshake-only, but re-run the soak before
quoting absolute numbers for a current release.
Each client traverses an independent `moba-netsim` UDP proxy adding 75 ms one-way latency,
30 ms uniform jitter and 5% independent packet loss (≈150 ms RTT).

## Short soak (8 s, 10 clients)

```
client 0: slot=0 snapshots=176 down 4.5 KB/s up 1.10 KB/s rtt=Some(186)
client 1: slot=1 snapshots=163 down 4.6 KB/s up 1.13 KB/s rtt=Some(202)
...
client 9: slot=9 snapshots=126 down 4.4 KB/s up 1.10 KB/s rtt=Some(169)
server: ticks=240 snapshots=1693 sent=437 KiB recv=47 KiB
```

- Snapshot rate: every server tick (30 Hz), each client received ~25–39 snapshots/s
  depending on loss; the client requests a full resync if a delta baseline is missing.
  Protocol v2 adds scores/outcome/hero-pick, v3 progression, v4 match phase/countdown. 30 s
  reruns measured 4.0–5.2 KiB/s (v2), 4.2–5.7 KiB/s (v3), 4.2–5.7 KiB/s (v4), 4.2–5.8 KiB/s (v5)
  and 4.6–6.8 KiB/s (v6 plaintext, 20 s). Encrypted v6 measured 5.0–7.3 KiB/s down /
  1.8–1.9 KiB/s up.
- Downstream: **4.4–4.7 KB/s per client** (21% of the 20 KB/s typical budget).
- Upstream: **~1.1 KB/s per client** (28% of the 4 KB/s budget).
- RTT: 160–210 ms under 75+30 ms one-way shaping.
- Bandwidth includes fragmentation headers, input retransmits, acks and pings.

## Five-minute soak (M5 gate)

`./target/release/soak --seconds 300` — 10 clients, 150 ms RTT, 30 ms jitter, 5% loss:

```
client 0: slot=0 connected snapshots=8640 down=4.5 KiB/s up=1.16 KiB/s rtt=209
client 5: slot=5 connected snapshots=8573 down=4.2 KiB/s up=1.17 KiB/s rtt=177
client 9: slot=9 connected snapshots=8475 down=4.1 KiB/s up=1.16 KiB/s rtt=185
server: ticks=9120 snapshots=90291 sent=13745 KiB recv=3321 KiB
```

All ten clients remained connected for the full five minutes at ~28.7 snapshots/s, inside
both budgets every interval. Reproduce with:

```bash
cargo run --release -p moba-tools --bin soak -- --seconds 300
```

## What makes the bandwidth cheap

- Only units visible to the client's team are sent; structures are permanent map knowledge.
- Snapshots are deltas against the last snapshot the client acknowledged (64-snapshot
  server history), with per-entity field masks.
- Positions are quantised to 1/16 unit; static entity data is sent only on spawn.
- Net ids are per-connection `u16`, not full entity handles.
- Inputs are one-shot intents retransmitted until acked; movement orders then live in the
  simulation state.

## Next

- Prediction/reconciliation and interpolation add client CPU but no wire growth.
- Lag compensation needs a short server history of hitboxes (or positions) for rewind.


## Replay cost

Replays store resolved per-slot inputs, not snapshots: a live 194-tick match produced a
30 KB `.mbr` file (≈4.6 KB/s while playing). Offline verification re-simulates the match in
milliseconds.


Custom-content replay: 555 ticks with Example Pack 1.0.0 (8 items + Vanguard) produced a
`.mbr`; offline verification with `--items` matched the pack manifest, 3 checkpoints, content
content hash `0x717e1ddd76abfabe`; state hashes were re-baselined by the #58 hash contract.


## Tick telemetry

Release 10-client soak (12 s, 150 ms RTT, 5% loss, 30 ms jitter): average simulation tick
11 µs, max 54 µs (budget 33,333 µs at 30 Hz), zero rate-limited datagrams.


## Batch match harness

Release `match-soak --matches 50` with rotated team compositions and two verification passes
per seed: 509,820 ticks, unique state hash per seed, determinism ok, average tick 6 µs,
max 3,021 µs, outcomes blue 30 / red 20.

## Bot balance check (round 44)

50 deterministic matches (509,820 ticks, rotated hero compositions) finished blue 30 / red 20
with worst tick 289 us and determinism ok. The 95% Wilson interval for blue's win rate is
46.2%–72.4%, which includes 50%; this sample does not establish a side bias. Larger samples
and fixed-composition sweeps should be used before changing balance.

## Wire-decoder fuzz gate

One-hour release runs of `fuzz-wire` completed with no panic or decoder hang:

- protocol v11 shape: **2,476,647,000 iterations clean**
- historical protocol v12 shape (client-identity fields + `Challenge`): **2,384,035,000 iterations
  clean**

## Final 10-client lossy soak (round 58)

300 s run with 10 clients, 150 ms RTT, 5% loss and 30 ms jitter:

- all 10 clients `Connected` at the end; 8,523–8,639 snapshots each
- server 9,117 ticks / 90,426 snapshots, 15,972 KiB sent / 3,327 KiB received
- about **5.0 KiB/s down** and **1.17 KiB/s up** per client
- avg tick **15 us**, max **304 us**, rate_limited 0, replayed_hellos 0
- final state hash from the #58 contract (see `tests/golden-state-hash.txt`)

This is well inside the 33 ms tick budget and the bandwidth budget for small dedicated
servers.

Bot fixed-composition mirror CIs are recorded in [0008-balance.md](0008-balance.md).

Presentation events/status bits and protocol v16 bandwidth are recorded in [0009-presentation-events.md](0009-presentation-events.md).

## Dense per-client entity IDs

`ClientConn` now maps entities through a dense table indexed by `Entity::index`, with a
generation check on lookup instead of a linear scan. Snapshot construction is O(1) per visible
unit regardless of match length; generation reuse after despawn is covered by a unit test.
Reverse lookup for rare client input commands still scans the dense table.

## Reused snapshot delta

`build_delta_into` clears and refills a server-owned `SnapshotDelta`, retaining spawn/update/
despawn/event vector capacity across clients and snapshots. The encode payload buffer was
already reused. A protocol test asserts the spawn capacity is stable across rebuilds.

## Snapshot history reuse

Snapshot history keeps up to 64 records per client. Popped records return their entity
vectors to a per-client pool; `record` refills a pooled buffer from the current view instead
of allocating a fresh clone every snapshot. Rematch/history clears also recycle buffers.

## Snapshot scaling with match age

`net/snapshot_scaling/{1,20,40}min` builds and encodes a snapshot from a simulated match kept
alive by restoring structure health. Latest criteria medians:

| Match age | Visible entities | Build+encode |
|---|---:|---:|
| 1 min | 58 | 215 ns |
| 20 min | 67 | 250 ns |
| 40 min | 93 | 332 ns |

That is ~3.7 / 3.7 / 3.6 ns per visible entity — cost tracks the view size, not match age.
The 40-minute view has more entities because later waves/skirmishes leave more units alive.
No super-linear growth was observed; the earlier “3 ms max tick” is not attributable to
snapshot construction in this benchmark.

## Server max-tick profile (5 minutes, 10 clients)

`cargo run --release -p moba-tools --bin soak -- --seconds 300` now reports both the
simulation tick and the whole frame (inputs + simulation + snapshots):

- 9,117 ticks / 90,310 snapshots, 16,061 KiB sent;
- average simulation tick **16 µs**, max **116 µs**;
- average whole frame bounded by tick rate; max frame **363 µs** at frame 3,053;
- frames ≥1 ms: **0**; simulation ticks ≥1 ms: **0**;
- no rate limiting, all clients connected.

The previously observed ~3 ms max tick did not reproduce; the instrumented run shows no
sustained or isolated >1 ms frame. Snapshot construction was separately measured at
~3.6–3.7 ns per visible entity (#87), so the old spike is most consistent with an OS/scheduler
or pre-optimisation transient rather than snapshot growth. The soak output now keeps the
location (`max_at_tick`, `max_wave`, `max_units`, frame index) for future regressions.

## Snapshot allocation guard

`cargo test -p moba-net --test snapshot_allocations` warms a delta and encode payload, then
builds/encodes 300 changed 100-entity snapshots through the global allocator. Result:
**0 allocations / 0 bytes**. Combined with pooled history buffers and the reused server delta,
the snapshot construction/encode path is allocation-free in steady state.
