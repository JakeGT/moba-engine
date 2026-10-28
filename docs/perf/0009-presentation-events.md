# Presentation events and status bits — protocol v16

Date: 2026-10-18  
Host: Linux x86-64, release build.

## Wire changes

- Snapshot deltas carry up to 16 `NetEvent` records (fixed 24-byte-ish layout): cast,
  projectile spawn/hit, unit death, structure destroyed, match ended, zone spawned.
- Projectile spawn events include origin, normalised direction, end point, speed and radius;
  clients extrapolate the projectile locally between snapshots.
- `NetEntity.status_bits: u16` widens status replication beyond the legacy u8 flags
  (dead/shield/slow/root/silence/burn/haste). The old flags remain for compatibility and now
  include burn/haste.
- `NetUpdate` mask bit 6 carries changed status bits.

## Visibility

Events are filtered per viewer before encoding:

- same-team events are always sent;
- enemy events require the source entity (or, for deaths/hits, target) to be present in that
  viewer's fog-of-war `ObservedUnit` list;
- match-ended is global.

A unit test verifies hidden enemy casts are dropped and visible/spectator events pass.

## Bandwidth

30-second 10-client lossy soak (150 ms RTT, 5% loss, 30 ms jitter):

```bash
cargo run --release -p moba-tools --bin soak -- --seconds 30
```

- server: 1,017 ticks, 9,371 snapshots, 1,877 KiB sent, 341 KiB received;
- per client: **4.7–6.3 KiB/s down**, **1.16–1.20 KiB/s up**;
- average tick 11 µs, worst 94 µs, no rate limiting.

This remains far under the ≤40 KB/s teamfight budget. Event records are sparse because only
validated casts/hits/deaths are emitted and capped at 16 per snapshot.

## Client

- Receives `ClientEvent::GameEvent` and drives a bounded kill feed and extrapolated projectile
  cubes.
- Uses `status_bits` for tint/icon state.
- Validation-required lavapipe capture succeeds.

## Protocol v41: basic attack events

Date: 2026-10-28 (R4b-1, [ADR-0012](../adr/0012-animation-runtime.md#basic-attacks-event_attack-r4b-1)).
Host: AMD Ryzen 7 9800X3D, Linux, release build, loopback through `moba-netsim`.

`EVENT_ATTACK` (8) is one 12-byte record per visible basic attack (kind, team, source and
target net ids, hero slots, attack interval, hit age); every other event is 26 bytes. It has
the lowest priority under the 16-events-per-snapshot cap (deaths/structures/match end, then
the other events, then hero attacks, then other attacks), and the server keeps attacks in their
own pending queue, so a burst cannot evict a death or a cast. Worst case, a snapshot full of
attacks adds 16 × 12 = 192 bytes, 5.6 KiB/s at 30 snapshots/s.

Same commands on `main` before the change (`6e809f0`, v40) and on this branch (v41), 150 ms
RTT, 30 ms jitter, 5% loss; each row is one run, so about ±0.1 KiB/s is run-to-run noise:

| Run | v40 down/client | v41 down/client | v41 events/client | of which attacks |
|---|---:|---:|---:|---:|
| `soak --seconds 60` (10 players, no bots; minions and towers fight) | 5.0–6.2 KiB/s | 5.0–6.2 KiB/s | 152–162 | 151–160 (~2.6/s) |
| `soak --seconds 120 --clients 2` (8 bot heroes fight, the teamfight case) | 7.0–7.1 KiB/s | 7.2–7.3 KiB/s | 1,587–1,590 | 1,357–1,362 (~11.3/s) |

Server totals: 3,640 → 3,684 KiB sent (+1.2%) in the 10-player run and 1,811 → 1,848 KiB
(+2.0%) in the bot-fight run. Upstream is unchanged (1.16–1.20 KiB/s). The teamfight case
stays far inside the ≤ 40 KB/s budget; even a permanently saturated event cap would add
5.6 KiB/s.

```bash
cargo run --release -p moba-tools --bin soak -- --seconds 60
cargo run --release -p moba-tools --bin soak -- --seconds 120 --clients 2
```

`soak` now prints each client's received `events` and `attacks`.
