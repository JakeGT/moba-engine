# Bot balance sweep — fixed-composition mirrors

Date: 2026-10-18  
Host: Linux x86-64, `--release`, 30 Hz, 60,000-tick cap.

## Harness

```bash
cargo run --release -p moba-tools --bin match-soak -- \
  --matches 300 --pick N --fixed-picks --ticks 60000 --no-verify
```

`--pick N` puts the same hero in every slot, so both teams field an identical mirrored
composition. Each match uses a different seed. Every match produced a unique state hash and
the harness reported `determinism=ok`.

## Finding: seed did not previously affect play

Before this sweep, `SimConfig.seed` only entered the state hash; bot decisions were entirely
constant. Five `--pick` runs of 500 seeds were therefore **the same match repeated 500 times**,
which is why every run reported 100% one side. The apparent bias was not a sample of matches.

## Fix

- Each bot slot owns a `Rng` stream derived from `seed ^ (slot % 5)`, so paired roles share a
  stream while independent matches differ.
- Bot casts use seeded timing/jitter; shop choice is seeded.
- The command/attack scheduling phase is derived from the seed bit, balancing first-mover
  advantage across matches instead of always resolving blue first.
- `SIM_LOGIC_VERSION` is 4; same seeds remain fully deterministic.

## Results (300 matches per composition)

| Composition | Blue | Red | Blue win rate | 95% Wilson CI |
|---|---:|---:|---:|---:|
| Hero 0 mirror | 157 | 143 | 52.3% | 46.7–57.9% |
| Hero 1 mirror | 150 | 150 | 50.0% | 44.4–55.6% |
| Hero 2 mirror | 157 | 143 | 52.3% | 46.7–57.9% |
| Hero 3 mirror | 155 | 145 | 51.7% | 46.0–57.3% |
| Hero 4 mirror | 157 | 143 | 52.3% | 46.7–57.9% |
| Default role mirror | 156 | 144 | 52.0% | 46.4–57.6% |

A larger 600-match hero-2 check produced **307 blue / 293 red = 51.2%**
(95% CI 47.2–55.1%). Every composition CI includes 50%; at this sample size there is no
statistical evidence of a side bias. The 300-match intervals are about ±5.7 points, so a
sub-1% bias would need a much larger sweep.

## Performance

The fixed sweeps averaged 7–11 µs/tick with worst ticks up to 1.6 ms; all matches finished
under the 60,000-tick cap. Allocation guard: **1 allocation / 192 B** per 300 steady bot ticks;
the isolated all-modifier path uses **10 allocations / 1,920 B** (under the 30 / 4 KiB budget).

## Follow-up

- Re-run after #39's `BotPolicy` seam and after every bot/ability change.
- 300 matches detect only large side biases; use ≥5,000 for a 1% resolution.

## 5,000-match determinism sweep (2026-10-20)

`match-soak --matches 5000 --ticks 3000` completed **15,000,000 ticks** with
`determinism=ok`; avg **7 µs/tick**, worst **481 µs**. All 5,000 matches hit the
3,000-tick cap without an outcome, so this run does not measure side win rates — it
only exercises long-running determinism. A balance-resolution sweep needs a much
larger tick budget or a shorter win condition.
