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

## Flood isolation

Measured 2026-10-27 on protocol v38, release build, branch `feat/292-host-budget` (#292), same box as above. Other agents' builds were running on the machine (load average 3–9), and the run was limited to 8 load-test threads: 4 host workers, 3 client drivers and 1 flood thread.

**What changed (#292):**
- Every datagram is charged against the per-address, per-match and optional host-wide budgets straight after `recv_from`, before header decode, AEAD open or reassembly. Undecodable junk now counts too.
- Each worker polls its sockets round-robin and caps every match at 64 datagrams or 500 µs per pass. Anything unread stays in that socket's kernel queue, and the kernel sheds the excess.

**Setup:** `load-test --flood-port 0 --flood-pps 300000` sends 300,000 undecodable datagrams/s (32–231 B, bad magic) at match 0. The flood starts 0.5 s before the 500 clients connect, so every handshake happens under it. Match 0 shares worker 0 with matches 1–12.

The gate in flood mode:
- every *other* match's tick p99 must be ≤ 2 ms (the worst one is gated);
- **zero** dropped frames on any worker;
- every other client must stay connected, within bandwidth and with no snapshot gap over 1 s.

The flooded match is reported but not gated.

| | Baseline (no flood, 60 s) | 1 junk source, 120 s | 64 junk sources + `--host-max-pps 30000`, 120 s |
|---|---|---|---|
| Worst per-match tick p99, other matches | — (all-match p99 0.17 ms) | **0.23 ms** | **0.15 ms** |
| Other matches' tick p99 / p50 | 0.17 / 0.11 ms | 0.19 / 0.13 ms | 0.13 / 0.08 ms |
| Same-worker neighbours (m1–m12), worst p99 | — | 0.23 ms | 0.13 ms |
| Frames dropped | 0 / 7,205 | **0 / 14,704** | **0 / 14,822** |
| Other clients connected | 500 / 500 | **490 / 490** | **490 / 490** |
| Slowest handshake, other matches | — | 22 ms | 22 ms |
| Slowest handshake, flooded match | — | 2,013 ms (10/10 connected) | 3,002 ms timeout (4/10 connected) |
| Flooded match tick p99 (incl. its capped polling) | — | 0.66 ms | 0.51 ms |
| Junk read and charged / sent | — | 6.96 M / 36.8 M | 6.85 M / 37.1 M |

**Reading the numbers:**
- **Neighbours are isolated.** With 300 k junk datagrams/s on one port, the other 49 matches stay within 0.1 ms of baseline p99, with no dropped frames and no client losses. Only about 19 % of the junk is ever read (≈ 57 k/s, i.e. 64 datagrams per ≈ 1.1 ms pass); the kernel drops the rest. Worker 0 spends about 1–2 percentage points more time busy than the other workers.
- **Single-source flood: the flooded match keeps working.** The per-address bucket stops the junk without touching the per-match budget, so all 10 of the flooded match's clients connect (the slowest needed a few 250 ms Hello retries because the kernel queue overflowed) and play with snapshot gaps of 33 ms or less.
- **Multi-source (spoofed) flood: new handshakes on the flooded port starve.** 64 sources each get their own 120 pps, which exhausts the 4,000 pps per-match budget, so most Hellos to *that* port are refused. 6 of 10 clients did not connect within the 3 s the tool allows. Clients that did connect kept playing (gap ≤ 33 ms). This is the documented limit of application-layer budgets: put public servers behind a DDoS/connection gateway.
- **The host-wide budget was not exercised.** Each match's own 4,000 pps budget kept admitted junk below 30,000 pps host-wide, so `host_budget_rejected=0`. Its behaviour (shared across matches and workers, charged last, never more than the burst under 4-thread contention) is covered by unit tests in `crates/net/src/host.rs` and `server/rate_limit.rs`.
- **One outlier run.** An earlier 64-source run, with load average ≈ 7–9 from concurrent builds, passed but had one neighbour (m12) at p99 1.79 ms. Two reruns at lower load gave m12 0.11–0.12 ms. That run's raw output is kept below.

### Raw output: baseline, 50 × 10, 60 s
```
load-test: 50 matches x 10 clients for 60s at 10 commands/s, 4 host workers, 3 client threads, 16 logical cores, link direct loopback
connected 500 clients in 0.0s
host: matches=50 workers=4 frames=7205 overruns=0 (0.00%) match_ticks=90063 tick_p50_us=110 tick_p99_us=170 tick_max_us=3567
  worker 0: matches=13 utilisation=4.4% max_lateness_us=6566 errors=0 p99_us=170
  worker 1: matches=13 utilisation=4.4% max_lateness_us=2789 errors=0 p99_us=180
  worker 2: matches=12 utilisation=4.0% max_lateness_us=4388 errors=0 p99_us=170
  worker 3: matches=12 utilisation=4.0% max_lateness_us=4818 errors=0 p99_us=170
clients: connected=500/500 down_mean=7.05 KB/s down_max=7.22 KB/s up_max=2.35 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=34
server: worst match avg_tick_us=20 rate_limited=0
load-test passed
```

### Raw output: 1 junk source at 300 k/s on match 0, 120 s
```
load-test: 50 matches x 10 clients for 120s at 10 commands/s, 4 host workers, 3 client threads, 16 logical cores, link direct loopback
flood: 300000 junk datagrams/s to match 0 from 1 source(s); host budget off
connected 500 clients in 2.0s
handshake under flood: slowest connect flooded_match=2013 ms other_matches=22 ms flooded_match_connect_failures=0
host: matches=50 workers=4 frames=14704 overruns=0 (0.00%) match_ticks=183800 tick_p50_us=130 tick_p99_us=510 tick_max_us=5340
  worker 0: matches=13 utilisation=6.0% max_lateness_us=1639 errors=0 p99_us=540
  worker 1: matches=13 utilisation=5.0% max_lateness_us=2633 errors=0 p99_us=200
  worker 2: matches=12 utilisation=4.6% max_lateness_us=3699 errors=0 p99_us=190
  worker 3: matches=12 utilisation=4.6% max_lateness_us=8504 errors=0 p99_us=200
flood: flooded match clients connected=10/10 (plus 0 that never connected) max_snapshot_gap_ms=33
clients: connected=490/490 down_mean=8.51 KB/s down_max=8.77 KB/s up_max=2.38 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=36
server: worst match avg_tick_us=25 rate_limited=6959584
flood: other matches tick_p50_us=130 tick_p99_us=190 worst_match_p99_us=230 (m12) same_worker_neighbours_p99_us=230
flood: flooded match m0 (worker 0) tick_p99_us=660 tick_max_us=1893 rate_limited=6959584 poll_capped=109273 recv=896211 KiB host_budget_rejected=0
flood: sent=36758305 datagrams in 122.5s (299996/s) send_errors=0
load-test passed
```

### Raw output: 64 junk sources at 300 k/s on match 0, host budget 30,000 pps, 120 s
```
load-test: 50 matches x 10 clients for 120s at 10 commands/s, 4 host workers, 3 client threads, 16 logical cores, link direct loopback
flood: 300000 junk datagrams/s to match 0 from 64 source(s); host budget 30000 pps
connected 494 clients in 3.0s
handshake under flood: slowest connect flooded_match=3002 ms other_matches=22 ms flooded_match_connect_failures=6
host: matches=50 workers=4 frames=14822 overruns=0 (0.00%) match_ticks=185276 tick_p50_us=80 tick_p99_us=380 tick_max_us=1222
  worker 0: matches=13 utilisation=3.9% max_lateness_us=320 errors=0 p99_us=400
  worker 1: matches=13 utilisation=3.0% max_lateness_us=295 errors=0 p99_us=130
  worker 2: matches=12 utilisation=2.8% max_lateness_us=359 errors=0 p99_us=130
  worker 3: matches=12 utilisation=2.8% max_lateness_us=315 errors=0 p99_us=130
flood: flooded match clients connected=4/4 (plus 6 that never connected) max_snapshot_gap_ms=33
clients: connected=490/490 down_mean=8.69 KB/s down_max=9.03 KB/s up_max=2.38 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=33
server: worst match avg_tick_us=15 rate_limited=6848864
flood: other matches tick_p50_us=80 tick_p99_us=130 worst_match_p99_us=150 (m13)
flood: same-worker neighbours p99_us m1:130 m2:130 m3:130 m4:130 m5:130 m6:130 m7:130 m8:130 m9:130 m10:120 m11:120 m12:120
flood: flooded match m0 (worker 0) tick_p99_us=510 tick_max_us=626 rate_limited=6848864 poll_capped=114795 recv=942207 KiB host_budget_rejected=0
flood: sent=37051673 datagrams in 123.5s (299996/s) send_errors=0
load-test passed
```

### Raw output: the outlier run (64 sources, load average ≈ 7–9)
```
flood: 300000 junk datagrams/s to match 0 from 64 source(s); host budget 30000 pps
connected 491 clients in 3.0s
handshake under flood: slowest connect flooded_match=3001 ms other_matches=14 ms flooded_match_connect_failures=9
host: matches=50 workers=4 frames=14822 overruns=0 (0.00%) match_ticks=185276 tick_p50_us=120 tick_p99_us=510 tick_max_us=12524
flood: flooded match clients connected=1/1 (plus 9 that never connected) max_snapshot_gap_ms=34
clients: connected=490/490 down_mean=8.75 KB/s down_max=9.21 KB/s up_max=2.38 KB/s (incl. IP/UDP headers) max_snapshot_gap_ms=45
flood: other matches tick_p50_us=120 tick_p99_us=190 worst_match_p99_us=1790 (m12) same_worker_neighbours_p99_us=1790
flood: flooded match m0 (worker 0) tick_p99_us=580 tick_max_us=2880 rate_limited=6582191 poll_capped=110606 recv=909001 KiB host_budget_rejected=0
load-test passed
```

The single-source and outlier runs predate the final report format: the per-neighbour list replaced `same_worker_neighbours_p99_us` (the worst of them).

Reproduce:
```bash
./target/release/load-test --matches 50 --clients 10 --workers 4 --client-threads 3 --seconds 120 --flood-port 0 --flood-pps 300000
./target/release/load-test --matches 50 --clients 10 --workers 4 --client-threads 3 --seconds 120 --flood-port 0 --flood-pps 300000 --flood-sources 64 --host-max-pps 30000
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
