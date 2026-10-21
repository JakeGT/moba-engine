# Performance log: multi-match hosting (M7)

Measured 2026-10-20 on protocol v36, release build, branch `feat/multi-match-host` (#291).

**Hardware:** AMD Ryzen 7 9800X3D (8 cores / 16 threads), 32 GB, Linux 7.2.4 (CachyOS).

**Setup:**
- `load-test` runs a `MatchHost` with **50 matches**, each with **10 real `NetClient`s** over loopback UDP (500 clients in total), plus 8 host worker threads and 8 client-driver threads.
- Each client sends **10 commands/s**, alternating between pushing toward the enemy base and casting an ability, so the teams meet in lane fights.
- The clients run in the same process as the host and compete with it for CPU, so these numbers are a conservative bound for a dedicated box.

**What a "tick" measures:** one match's full cost per 30 Hz frame. That's its socket polling since the previous frame (receive, decrypt, handshakes, input handling) plus `NetServer::tick()` (simulation, visibility, snapshot delta encode and send to 10 clients).

**Bandwidth:** figures are decimal KB/s and include 28 B of IPv4 + UDP header per datagram.

## Budgets

| Budget | Target | 5 min, direct | 2 min, 150 ms RTT / 30 ms jitter / 5 % loss |
|---|---|---|---|
| Per-match tick p99 | ≤ 2 ms | **0.15 ms** | **0.15 ms** |
| Per-match tick p50 | | 0.09 ms | 0.10 ms |
| Frames dropped (30 Hz schedule missed) | ≤ 1 % | **0 / 72,010** | **0 / 29,099** |
| Clients connected throughout | 500 / 500 | **500 / 500** | **500 / 500** |
| Longest gap between applied snapshots | ≤ 1 s | 33 ms | 208 ms |
| Downstream per client, mean / max | ≤ 20 / 40 KB/s | 8.4 / 8.9 KB/s | 8.6 / 9.1 KB/s |
| Upstream per client, max | ≤ 4 KB/s | 2.3 KB/s | **3.6 KB/s** |
| Worker utilisation | | ≈ 2–3 % | ≈ 1–2 % |

Worst single ticks were 3.4 ms (direct) and 0.8 ms (lossy). p99 stays at 0.15 ms in both. The server's own simulation-only average (`avg_tick_us`) is about 17 µs per match; most of the per-match cost is snapshot encoding and sending.

**Upstream headroom is the tightest budget.** Under 5 % loss, at 10 commands/s, the upstream reaches 3.6 KB/s, 90 % of the 4 KB/s budget, because loss adds acknowledgements and resends. Faster clicking or worse links would exceed it. Coalescing inputs or acknowledgements is the lever if that becomes a problem.

**CPU headroom:** 50 matches use about 2–3 % of 8 worker threads, roughly 0.2 of one core in total. Per-match tick cost, not scheduling, is the limit. Extrapolating from about 0.1 ms per match per 33 ms frame (not measured), one worker thread could serve on the order of 100+ matches. Measure on the deployment hardware before relying on that.

## Raw output

### 50 × 10, 300 s, direct loopback
```
load-test: 50 matches x 10 clients for 300s at 10 commands/s, 8 host workers, 8 client threads, 16 logical cores, link direct loopback
connected 500 clients in 0.0s
host: matches=50 workers=8 frames=72010 overruns=0 (0.00%) match_ticks=450064 tick_p50_us=90 tick_p99_us=150 tick_max_us=3385
  worker 0: matches=7 utilisation=1.9% max_lateness_us=462 errors=0 p99_us=150
  worker 1: matches=7 utilisation=1.9% max_lateness_us=215 errors=0 p99_us=140
  worker 2: matches=6 utilisation=1.6% max_lateness_us=468 errors=0 p99_us=150
  worker 3: matches=6 utilisation=1.6% max_lateness_us=268 errors=0 p99_us=150
  worker 4: matches=6 utilisation=1.7% max_lateness_us=454 errors=0 p99_us=150
  worker 5: matches=6 utilisation=1.6% max_lateness_us=288 errors=0 p99_us=140
  worker 6: matches=6 utilisation=1.6% max_lateness_us=1248 errors=0 p99_us=150
  worker 7: matches=6 utilisation=1.6% max_lateness_us=317 errors=0 p99_us=140
clients: connected=500/500 down_mean=8.40 KB/s down_max=8.89 KB/s up_max=2.33 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=33
server: worst match avg_tick_us=17 rate_limited=0
load-test passed
```

### 50 × 10, 120 s, 150 ms RTT / 30 ms jitter / 5 % loss
```
load-test: 50 matches x 10 clients for 120s at 10 commands/s, 8 host workers, 8 client threads, 16 logical cores, link 150 ms RTT / 30 ms jitter / 5% loss
connected 500 clients in 1.2s
host: matches=50 workers=8 frames=29099 overruns=0 (0.00%) match_ticks=181870 tick_p50_us=100 tick_p99_us=150 tick_max_us=785
  worker 0: matches=7 utilisation=2.0% max_lateness_us=496 errors=0 p99_us=150
  worker 1: matches=7 utilisation=2.0% max_lateness_us=124 errors=0 p99_us=150
  worker 2: matches=6 utilisation=1.7% max_lateness_us=203 errors=0 p99_us=150
  worker 3: matches=6 utilisation=1.7% max_lateness_us=107 errors=0 p99_us=160
  worker 4: matches=6 utilisation=1.7% max_lateness_us=335 errors=0 p99_us=150
  worker 5: matches=6 utilisation=1.7% max_lateness_us=367 errors=0 p99_us=150
  worker 6: matches=6 utilisation=1.7% max_lateness_us=217 errors=0 p99_us=150
  worker 7: matches=6 utilisation=1.8% max_lateness_us=116 errors=0 p99_us=150
clients: connected=500/500 down_mean=8.55 KB/s down_max=9.12 KB/s up_max=3.60 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=208
server: worst match avg_tick_us=16 rate_limited=0
load-test passed
```

## Reproduce
```bash
cargo build --release -p moba-tools --bin load-test
./target/release/load-test --matches 50 --clients 10 --seconds 300
./target/release/load-test --matches 50 --clients 10 --seconds 120 --rtt 150 --jitter 30 --loss 5
```

## Hosted runner (regression gate)

Run [39504487980](https://github.com/JakeGT/moba-engine/actions/runs/39504487980) on a GitHub-hosted `ubuntu-latest` runner (4 vCPUs, virtualised and shared), 8 × 10 for 60 s:
```
load-test: 8 matches x 10 clients for 60s at 10 commands/s, 2 host workers, 2 client threads, 4 logical cores, link direct loopback
host: matches=8 workers=2 frames=3602 overruns=0 (0.00%) match_ticks=14408 tick_p50_us=210 tick_p99_us=320 tick_max_us=2779
clients: connected=80/80 down_mean=7.04 KB/s down_max=7.07 KB/s up_max=2.34 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=33
load-test passed
```
Per-match ticks cost about 2× the dev box on the shared runner, and p99 is still well inside the budget.

CI runs `load-test --matches 8 --clients 10 --seconds 60` in the `tools + deterministic soak` job on a GitHub-hosted `ubuntu-latest` runner. `scripts/agent-verify.sh --full` runs a 4-match, 10-second smoke test. Hosted-runner numbers are virtualised and shared, so they are a regression gate, not capacity evidence.
