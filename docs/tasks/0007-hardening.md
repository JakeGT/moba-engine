# 0007: Hardening (M7)

**Milestone:** M7   **Crates touched:** net, sim, server, tools   **Reviewers:** netcode + security + perf

## Goal
Make the engine safe and operable outside a trusted LAN, and make failures reproducible:
authenticated transport, reconnect, deterministic replays, load/soak gates and profiling.

## Landed so far
- **Optional PSK AEAD** (protocol v7): HMAC-SHA256 handshake proofs, server nonce, HKDF-
  SHA256 session keys, ChaCha20-Poly1305 payloads authenticated with header AAD, per-peer
  transport keys, encrypted fragments/reconnect, redacted key debug output.
- **Reconnect** with a configurable grace period and slot/hero/network-id resumption.
- **Deterministic replays**: server `--record FILE`, per-tick resolved inputs, expected state
  hash, `moba-tools replay FILE` verification, recording test and decoder noise test.
- **Operational tools**: 5-minute plaintext soak, encrypted 10-client soak, deterministic
  wire fuzzer, replay verifier.
- **Application-layer DoS controls**: bounded per-address token buckets (configurable
  `--max-pps`, 0 disables), drop counters and avg/max tick telemetry. Release soak measures
  avg 9 µs / max 55 µs per simulation tick with 10 clients.

## Still open
- Public identity/key exchange/revocation and a formal replay window.
- One-hour sanitizer-backed fuzzing run (the deterministic fuzzer is available).
- 50 consecutive matches per box under the soak harness is implemented (`match-soak`) with
  per-match composition rotation: 509,820 ticks, deterministic, unique hashes per seed,
  avg 6 µs / max 3,021 µs, outcomes blue 30 / red 20. Profiling capture review remains.
- Allocation-counting hot-path tests; loom model of the job-scope counter.
- Windows/macOS renderer validation and min-spec frame timings.

## Acceptance
- [x] Wrong-key encrypted handshake is rejected; encrypted reconnect works.
- [x] Encrypted 10-client soak at 150 ms/5%/30 ms stays inside budgets.
- [x] Replay file reproduces the final state hash; malformed input never panics.
- [x] 50-match deterministic harness, content-check tool, per-address rate limiting and tick
  telemetry.
- [ ] One-hour fuzz, balanced demo bots and profiling captures.

## Security references
- RustCrypto [ChaCha20-Poly1305](https://docs.rs/chacha20poly1305) and
  [HKDF](https://docs.rs/hkdf).
- [hexgate](https://github.com/alexdesander/hexgate) for UDP encryption/auth structure.
- [SECURITY.md](../../SECURITY.md) for the documented threat model and limitations.
- [lockstep-tactics](https://github.com/RizgarOzan/lockstep-tactics) for bit-identical
  lockstep/replay verification across platforms.
- [AstraWeave deterministic architecture](https://github.com/lazyxeon/AstraWeave/blob/main/docs/src/architecture/deterministic.md)
  for deterministic simulation/testing practices.
