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

- **Fuzzing**: one-hour wire-decoder runs completed **2,476,647,000 iterations clean** on the
  v11 shape and **2,384,035,000 iterations clean** on the final v12 shape.

## CI gates
- Cross-platform fmt/clippy/tests/docs/features/layering.
- `ci-tools`: content-check fixtures, 10-match deterministic soak, 30-second wire fuzz and
  bench compilation.
- cargo-deny and Miri jobs.

## Still open
- Account/reputation/key-transparency layer and player-count attestation.
- One-hour sanitizer-backed fuzzing run (the deterministic fuzzer is available).
- 50 consecutive matches per box under the soak harness is implemented (`match-soak`) with
  per-match composition rotation: 509,820 ticks, deterministic, unique hashes per seed,
  avg 6 µs / max 3,021 µs, outcomes blue 30 / red 20.
- Allocation regression guard: 300 steady ticks at 12 allocations / 490 bytes after removing
  a per-tick command-order `Vec` (previously 312 / 24,490).
- Criterion captures: snapshot codec (full encode 206 ns / decode 264 ns, steady delta
  encode 35 ns / decode 92 ns) and simulation ticks (10.7–12.9 µs steady). Profiling log:
  `docs/perf/0007-profiling.md`.
- Allocation-counting hot-path tests; loom model of the job-scope counter.
- Windows/macOS renderer validation and min-spec frame timings.

## Acceptance
- [x] Wrong-key encrypted handshake is rejected; encrypted reconnect works.
- [x] Encrypted 10-client soak at 150 ms/5%/30 ms stays inside budgets.
- [x] Replay file reproduces the final state hash; malformed input never panics.
- [x] 50-match deterministic harness, content-check tool, per-address rate limiting and tick
  telemetry.
- [x] Profiling captures: Criterion snapshot codec + simulation benches recorded in
  `docs/perf/0007-profiling.md`.
- [ ] One-hour fuzz and balanced demo bots.

## Security references
- RustCrypto [ChaCha20-Poly1305](https://docs.rs/chacha20poly1305) and
  [HKDF](https://docs.rs/hkdf).
- [hexgate](https://github.com/alexdesander/hexgate) for UDP encryption/auth structure.
- [SECURITY.md](../../SECURITY.md) for the documented threat model and limitations.
- [lockstep-tactics](https://github.com/RizgarOzan/lockstep-tactics) for bit-identical
  lockstep/replay verification across platforms.
- [AstraWeave deterministic architecture](https://github.com/lazyxeon/AstraWeave/blob/main/docs/src/architecture/deterministic.md)
  for deterministic simulation/testing practices.
