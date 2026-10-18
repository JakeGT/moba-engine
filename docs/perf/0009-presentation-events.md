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
