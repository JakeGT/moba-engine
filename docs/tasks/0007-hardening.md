# 0007: Hardening (M7)

**Milestone:** M7   **Crates touched:** net, sim, server, tools   **Reviewers:** netcode + security + perf

## Goal
Make the engine safe and operable outside a trusted LAN, and make failures reproducible:
authenticated transport, reconnect, deterministic replays, load/soak gates and profiling.

## Landed so far
- **Ed25519 client identity**: server challenge packet, public-key allowlist
  (`--allow-client-key`), challenge/signature verification and rejection of missing/unlisted
  keys.
- **Per-client token ring**: multiple Hello HMAC keys, per-player session encryption and
  allowlist/rotation (`--allow-token`); wrong/unlisted tokens rejected.
- **Handshake replay window**: authenticated key/nonce pairs are address-bound for 60 s;
  cross-address replays are counted/rejected while retransmits still work.
- **Pinned server identity**: Ed25519-signed Welcome over nonces/session/catalogue; clients
  pin the public key via `MOBA_SERVER_PUBKEY`; server loads `--server-key FILE`.
- **Optional PSK AEAD** (protocol v7): HMAC-SHA256 handshake proofs, server nonce, HKDF-
  SHA256 session keys, ChaCha20-Poly1305 payloads authenticated with header AAD, per-peer
  transport keys, encrypted fragments/reconnect, redacted key debug output.
- **Reconnect** with a configurable grace period and slot/hero/network-id resumption.
- **Deterministic replays**: server `--record FILE`, per-tick resolved inputs, expected state
  hash, `moba-tools replay FILE` verification, recording test and decoder noise test.
- **Operational tools**: 5-minute plaintext soak, encrypted 10-client soak, deterministic
  wire fuzzer, replay verifier.
- **Application-layer DoS controls**: bounded per-address token buckets (configurable
  `--max-pps`), a global packet budget (`--max-total-pps`), drop counters and avg/max tick
  telemetry; both limits can be disabled with 0. Release soak measures
  avg 9 µs / max 55 µs per simulation tick with 10 clients.

- **Graceful shutdown**: SIGINT/SIGTERM flag stops the loop and flushes the live replay.
- **Match results log**: optional JSONL append with seed/ticks/score/content/state/replay for
  operators and analytics.

- **Fuzzing**: historical one-hour wire-decoder runs completed **2,476,647,000 iterations
  clean** on v11 and **2,384,035,000 iterations clean** on v12. Current protocol v40 uses the
  deterministic fuzzer plus cargo-fuzz `wire`/`replay`/`content` targets.

## CI gates
- Cross-platform fmt/clippy/tests/docs/features/layering and a checked golden state hash.
- Release tests/doctests and MSRV (1.88) jobs.
- `ci-tools`: content-check fixtures, 10-match deterministic soak, 30-second wire fuzz and
  bench compilation.
- Nightly/manual: one-hour fuzz, five-minute 10-client soak, 50-match soak and benchmark log;
  cargo-deny and Miri jobs on every CI run.
- Benchmark thresholds are a documented manual gate because hosted-runner timing is noisy.

## Landed evidence
- 50-match deterministic harness (509,820 ticks, rotated compositions) and per-address/global
  rate limits.
- Allocation regression guard: 300 steady ticks at 1 allocation / 192 bytes; isolated all-modifier path 10 allocations / 1,920 bytes after removing
  a per-tick command-order `Vec` (previously 312 / 24,490).
- Criterion captures: snapshot codec and simulation ticks in `docs/perf/0007-profiling.md`.
- Final one-hour v12 fuzz and 10-client lossy soak.

- 50 concurrent matches per box: `MatchHost` + `moba-server --matches N`, verified by
  `load-test` (see [`docs/perf/0010-multi-match.md`](../perf/0010-multi-match.md), #291).
- Host-wide hardening (#292): datagrams are charged against the per-address/per-match/host-wide
  budgets before decode; per-match poll caps with round-robin polling isolate a flooded port
  (`load-test --flood-port K --flood-pps N`); `--reload-content` loads once per host and shares
  the pack across matches.

## Still open
- Independent external review.
- Account/reputation/key-transparency services remain external integrations (ADR 0006);
  player-count attestation is designed, not implemented.
- [x] Sanitizer-backed fuzzing (cargo-fuzz/ASan) in addition to the deterministic fuzzer:
  `fuzz/` targets `wire`, `replay`, `content`; `.github/workflows/fuzz.yml` runs bounded
  nightly/manual sessions and uploads artifacts. Seed corpus lives in `fuzz/corpus/`.
- [x] Loom model of the job-scope counter (`crates/jobs/src/loom_tests.rs`, `loom.yml`).
- [x] Job-scope state is `Arc`-owned; no raw `ScopeShared` pointer survives the scope frame.
- [ ] Loom model drives the real pool and catches an injected early-decrement bug (#143).
- [x] Pool/Loom wait uses blocking condvar wait; lost wake-ups now fail the model.
- Balanced demo bots across fixed compositions (current 50-match sample's CI includes even
  odds; larger sweeps are needed).
- Windows/macOS renderer validation and min-spec frame timings (renderer task #9/#127).

## Acceptance
- [x] Wrong-key encrypted handshake is rejected; encrypted reconnect works.
- [x] Final 10-client soak (plaintext + encrypted tests) at 150 ms/5%/30 ms stays inside budgets.
- [x] Replay file reproduces the final state hash; malformed input never panics.
- [x] 50-match deterministic harness, content-check tool, per-address rate limiting and tick
  telemetry.
- [x] Profiling captures: Criterion snapshot codec + simulation benches recorded in
  `docs/perf/0007-profiling.md`.
- [x] One-hour wire fuzz on the historical final v12 shape; current cargo-fuzz targets are in `fuzz/`.
- [ ] Balanced demo bots across larger fixed-composition sweeps.

## Security references
- RustCrypto [ChaCha20-Poly1305](https://docs.rs/chacha20poly1305) and
  [HKDF](https://docs.rs/hkdf).
- [hexgate](https://github.com/alexdesander/hexgate) for UDP encryption/auth structure.
- [SECURITY.md](../../SECURITY.md) for the documented threat model and limitations.
- [lockstep-tactics](https://github.com/RizgarOzan/lockstep-tactics) for bit-identical
  lockstep/replay verification across platforms.
- [AstraWeave deterministic architecture](https://github.com/lazyxeon/AstraWeave/blob/main/docs/src/architecture/deterministic.md)
  for deterministic simulation/testing practices.

## Identity trust (ADR 0006)

- [x] Local allowlist key rotation/revocation (`--rotate-client-key`, `--revoke-client-key`).
- [x] Trust-boundary ADR covering accounts, transparency/revocation feeds and optional
  player-count attestation.
- [x] SECURITY threat-model update; plaintext/token-only modes unchanged.

## Perf closure (#140)

Bot masks, allocation guards, grid gathering and slow-tick tracing landed; #140 closed.

## CI trust (#142)

- [x] Nightly uses `shell: bash` (pipefail) and a failure job opens an issue with the run link
  (exercised by #160; short green run 39239367089).
- [x] Sim and snapshot allocation tests use per-thread counters; 20 consecutive release
  repeats of each pass.
- [x] `release tests + doctests`, `MSRV` and `renderer-gate` are required checks.
- [x] Full nightly **39295169028** green with logs; #142 closed.
- [x] `renderer-gate` runs on every PR (no-op for non-renderer paths) and fully on renderer
  changes/dispatch; macOS MoltenVK probe runs automatically for renderer paths.
- [x] Network test client timeouts raised to at least 3 s; Dependabot ignores the toolchain pin.
