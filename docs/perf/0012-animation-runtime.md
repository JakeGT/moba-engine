# Animation runtime CPU cost (R4a, ADR-0012)

Date: 2026-10-28  
Host: AMD Ryzen 7 9800X3D (8 cores / 16 threads), Linux 7.3.0 (CachyOS), rustc 1.99.1,
`cargo bench` (release, criterion defaults), one thread.

## What is measured
`crates/assets/benches/skin_sample.rs`:

```bash
cargo bench -p moba-assets --bench skin_sample
```

- `skin_sample/*` — the WP-4.1 single-clip path, `Skeleton::sample` (unchanged code, re-run
  for comparison).
- `animator/*` — `Animator::evaluate` on a rig shaped like the shipped
  `sentinel-skinned.glb`: 10 joints, a locomotion blend space (idle ↔ run), an upper-body
  attack re-triggered every 120 frames, an additive hit reaction, a 3-joint spring chain and a
  socket. Half the units run (locomotion blend + springs reacting to motion), half stand.
  - `1_unit`: one animator per iteration.
  - `200_units`: the R4 stress case, 200 animators evaluated every frame.
  - `200_units_lod4`: the same 200 with a quarter evaluated per frame and the rest reusing
    their last matrices, as `MOBA_ANIM_LOD` does for heroes beyond 140 m or off-screen.

## Results

| Benchmark | Time (median) |
|---|---|
| `skin_sample/7_joints/rest` | 128 ns |
| `skin_sample/7_joints/walk` | 546 ns |
| `skin_sample/64_joints/rest` | 807 ns |
| `skin_sample/64_joints/walk` | 4.28 µs |
| `animator/1_unit` | **609 ns** |
| `animator/200_units` | **115.6 µs** per frame |
| `animator/200_units_lod4` | **31.4 µs** per frame |

**Budget.** The R4 exit criterion asks a 200-unit stress test to stay within the animation CPU
budget. This note sets that budget at **1 ms per frame** (15 % of the 6.9 ms p95 client frame
at 144 Hz). 200 fully evaluated units use 0.12 ms, 12 % of it; with the default LOD the far
quarter costs a quarter of that. A hero match evaluates at most 10 animators (`MAX_ANIMATED`
is 32), about 6 µs per frame.

The animator costs about the same as the old single-clip `walk` sample per unit even though it
blends two locomotion clips, an action layer through two masks, an additive layer and three
spring joints: the per-track work dominates and the sample rig has few tracks.

## Allocation
Steady state allocates nothing:

- `cargo test -p moba-assets --test animation_allocations` — 1,200 frames of 16 animators
  (attacks, casts, hits, deaths and respawns, springs, sockets, reports) after a 60-frame
  warm-up: 0 allocations, 0 bytes (`moba-test-support` counting allocator).
- `moba-client` test `app::animation::tests::steady_state_driver_frames_do_not_allocate` — the
  client driver's snapshot inference, event flush and per-hero `animate` for 10 heroes over
  56 snapshots: 0 allocations.

## Not measured here
- GPU skinning cost is unchanged (same bone SSBO and shader; see
  [perf/0007](0007-real-hardware.md#networked-match-with-skinned-heroes-wp-42-289)).
- Real-match animator counts on the reference GPUs of the roadmap (§7): the CPU numbers above
  are host-only and should be re-run on the min-spec machine when it exists.
