# Bot balance sweep

## Outcome-resolving sweep (2026-10-27, WP-2.3, #290)

**Result: red is favoured.** Over 4,978 decided matches blue won **44.21 %**, with a Wilson
95 % interval of **42.84–45.60 %**. The interval excludes 50 %, so this is a measured side
bias of about 5.8 points in red's favour. The diagnosis below traces it to the minion-wave
formation, which is point-reflected rather than x-mirrored. Fixing it is WP-2.2 (a logic
bump, which needs a maintainer decision), so it is not done here.

### Method

- Default 5v5 content and map, sim logic **v7** (`SIM_LOGIC_VERSION` in
  `crates/sim/src/replay.rs`), all ten slots driven by `DefaultBotPolicy`.
- Match `i` uses seed `0x5EED_1234_ABCD_0001 + i`, so seed parity alternates the
  command/attack phase. Picks rotate with `i`, and both teams always field the same five
  heroes in the same slot order.
- Each match runs until a nexus falls or 60,000 ticks (33 minutes of game time) pass.
  Unresolved matches count as **draws**.
- The win rate and Wilson 95 % interval are computed over decided matches only.
- `unique_hashes` now counts `state_hash_normalized()`. It previously used `state_hash()`,
  which mixes in the seed and so was unique by construction.

Host: AMD Ryzen 7 9800X3D (8 cores / 16 threads), Linux 7.3.0-2-cachyos, rustc 1.99.1,
`--release`. This was a shared machine with other builds running, so timings are
indicative only. The outcomes are deterministic and independent of the host and thread
count.

```bash
cargo build --release -p moba-tools --bin match-soak
./target/release/match-soak --matches 5000 --ticks 60000 --no-verify --threads 8
```

### Raw output

```text
match-soak: 5000 matches, max 60000 ticks, bot_mask=0x3ff, verify=false, seed=0x5eed1234abcd0001, threads=8
  ...
  5000/5000 matches, 349 s
matches=5000 ticks=96685198 avg_tick_us=12 max_tick_us=6228 max_tick_at=5987 wall_s=348.91 unique_hashes=5000 outcome_none=22 blue=2201 red=2777 determinism=ok
balance: decided=4978 draws=22 blue_win_rate=44.21% wilson95=42.84%..45.60% resolve_tick_p10=8975 median=15153 p90=32897 max=59806
```

| N | Blue | Red | Draws (cap) | Blue win rate (decided) | Wilson 95 % | Resolution tick p10 / median / p90 / max |
|---:|---:|---:|---:|---:|---:|---:|
| 5,000 | 2,201 | 2,777 | 22 | 44.21 % | 42.84–45.60 % | 8,975 / 15,153 / 32,897 / 59,806 |

- The median match resolves at tick 15,153, about 8.4 minutes of game time.
- Every one of the 5,000 normalized end-state hashes is distinct, so no two seeds played
  out identically.
- 8 worker threads took 349 s of wall time. `avg_tick_us` of 12 µs is inflated by the
  shared cores.

### Diagnosis (local experiments, not committed)

Each row below is the same harness and seeds as the main sweep, with fewer matches. The
second and third rows needed a one-line local patch to `sim.rs`; both patches were
reverted, and neither is part of this change.

| Variant | N | Blue | Red | Draws | Blue win rate | Wilson 95 % |
|---|---:|---:|---:|---:|---:|---:|
| `--no-waves` (heroes only) | 1,000 | 329 | 347 | 324 | 48.67 % | 44.92–52.43 % |
| Deaths resolved in reverse ECS order on alternate phases | 2,000 | 874 | 1,117 | 9 | 43.90 % | 41.73–46.09 % |
| Red wave formation x-mirrored (`-direction.perpendicular()` for red in `spawn_wave`) | 2,000 | 999 | 993 | 8 | **50.15 %** | 47.96–52.34 % |

What the rows show:

- **Waves carry the bias.** Without minion waves the interval includes 50 %.
- **Death order is not the cause.** `resolve_deaths` walks deaths in ECS order, so a
  same-tick mutual hero kill gives experience to the hero listed second; the mirror test in
  #290 (WP-2.1) found this. Alternating that order leaves the bias unchanged.
- **The formation is the cause.** `spawn_wave` offsets each formation along
  `direction.perpendicular()`, which is a point reflection, while heroes and the map are
  x-mirrored. Making red's formation an x-mirror of blue's removes the bias.

Mechanism: the formation is not mirrored lane by lane.

- In the top lane, blue's ranged minion spawns about 6 units *ahead* of the lane start (+x,
  toward the enemy). Red's spawns about 6 units *behind* its lane start (+x, toward its own
  base).
- The bottom lane is the reverse.
- In the mid lane, the two formations sit on opposite sides of the lane (blue +y, red −y).
- Bots do not use the lanes evenly. With no visible target they walk straight at the enemy
  nexus through mid, and they engage whatever they see. The per-lane differences therefore
  do not cancel.
- Which lane contributes most has not been isolated. The experiment above shows only that
  mirroring the formation removes the bias.

The other asymmetries listed in the plan point the wrong way, or are too rare to explain
5.8 points:

- `MAX_UNITS` starvation of the team that spawns second would favour blue, and wave spawn
  order already alternates.
- The score check looks at blue first, but the default rules set no score limit.
- A same-tick double nexus is very rare. Deaths are processed in ECS order, so the blue
  nexus would fall first and red would win.

**Follow-up:** WP-2.2 item (a), x-mirroring the red formation in `spawn_wave`, needs a
`SIM_LOGIC_VERSION` bump, a regenerated replay fixture and golden hash, and a re-run of this
sweep.

### Bot seeding (current code)

- **Bot RNG:** each slot owns an independent stream,
  `Rng::from_seed(seed ^ (0xB0B0_0000_0000_0000 | slot * 0x9E37))` for slots 0–9.
  - The streams are not shared between paired roles.
  - Red's cast jitter is not negated. The jitter is uniform on −2..=2, so this changes
    individual matches but not the distribution.
- **Scheduling phase:** blue resolves first on ticks where `(tick + (seed & 1))` is even.
  - `apply_inputs` and `resolve_attacks` reverse the whole slot list on the other ticks.
  - Wave spawn order alternates by wave number.

## Earlier fixed-composition mirrors (2026-10-18)

These were measured when they were written (sim logic v4) and have not been re-run on v7.
They are kept as history.

```bash
cargo run --release -p moba-tools --bin match-soak -- \
  --matches 300 --pick N --fixed-picks --ticks 60000 --no-verify
```

`--pick N` puts the same hero in every slot.

**Seed fix.** Before that sweep, `SimConfig.seed` only entered the state hash: every match
was the same match. The fix seeded per-slot bot streams, seeded cast timing and shop
choice, and derived the scheduling phase from the seed bit.

| Composition | Blue | Red | Blue win rate | 95 % Wilson CI |
|---|---:|---:|---:|---:|
| Hero 0 mirror | 157 | 143 | 52.3 % | 46.7–57.9 % |
| Hero 1 mirror | 150 | 150 | 50.0 % | 44.4–55.6 % |
| Hero 2 mirror | 157 | 143 | 52.3 % | 46.7–57.9 % |
| Hero 3 mirror | 155 | 145 | 51.7 % | 46.0–57.3 % |
| Hero 4 mirror | 157 | 143 | 52.3 % | 46.7–57.9 % |
| Default role mirror | 156 | 144 | 52.0 % | 46.4–57.6 % |

At ±5.7 points, those 300-match intervals could not detect a bias of the size now measured.

## 5,000-match determinism sweep (2026-10-20)

`match-soak --matches 5000 --ticks 3000` completed **15,000,000 ticks** with
`determinism=ok`: average **7 µs/tick**, worst **481 µs**.

All 5,000 matches hit the 3,000-tick cap without an outcome, so that run measured
long-running determinism and not win rates. In the sweep above, only 10 % of matches
resolve by tick 8,975.
