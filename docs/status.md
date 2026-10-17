# Engine status

Last updated: 2026-10-17 (round 17 of the build).

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ | Workspace, lints, CI, justfile, agent definitions. |
| M1 | Core runtime | ✅ | Workspace test suite green, Miri clean on core/ecs (jobs: tree borrows), benches beat AoS. Loom model still pending. |
| M2 | Platform | ✅ (Linux run; Win/macOS CI pending) | winit window, input, 30 Hz fixed step, 60-frame smoke run exits 0. |
| M3 | Renderer | 🟡 | Device/swapchain/dynamic rendering + 500 instanced units and terrain validated on lavapipe (zero validation messages); skinning, perf and golden CI pending. |
| M4 | Simulation | ✅ (Linux-verified) | 5v5 + 3 lanes + towers/nexus + 15 abilities + bots; 12.9 µs/tick; 10k-tick determinism. Server-side vision filtering (M5), fog rendering and levels/items (M6) landed. |
| M5 | Networking | 🟡 | UDP transport, optional PSK AEAD encryption, reconnect, visibility/delta snapshots, prediction/interpolation, playable client and spectator mode (protocol v7); 5-minute plaintext soak, 20 s encrypted and 15 s v7 soaks pass; public identity/anti-DoS and one-hour sanitizer fuzzers pending. |
| M6 | Vertical slice | ✅ | Playable networked/local match with lobby/countdown/results/rematch, reconnect grace, read-only spectator mode, ready-up/surrender, HUD, levels/XP, gold/items/shop, minimap, health bars, scoreboard, fog-of-war and wall line-of-sight. Linux-verified; audio/settings deferred. |
| M7 | Hardening | 🟡 | PSK AEAD transport, reconnect, replay manifests, rate limiting/tick telemetry, 50-match harness and Criterion profiling captures landed; public identity/edge anti-DoS, one-hour fuzz and balanced demo bots remain. |
| M8 | Content pipeline | 🟡 | Runtime item/hero/ability/map packs load from TOML, validate, drive simulation and reach clients in the handshake; new ability behaviour kinds, hot reload and richer manifests remain. |

## What exists now

- **`moba-core`** — Q32.32 fixed-point (`Fix`, `FixVec2`) with deterministic integer
  `sqrt` and CORDIC `sin_cos`; xoshiro256++ RNG with streams/jump; generation-checked
  arena; 64-bit and growable bit sets; wrap-safe ticks and an integer `FixedStep`; FNV-1a
  stable hashing; `glam` re-exports plus AABB helpers.
- **`moba-ecs`** — generational entities; type-erased SoA archetype columns; insert,
  remove and despawn move components without cloning; typed queries up to 8 components
  with `With`/`Without` filters; per-row change ticks; zero-allocation item and chunk
  iteration.
- **`moba-jobs`** — scoped work-stealing pool with a global injector, per-worker deques,
  condvar idle sleep, caller participation, nested scopes, panic capture and
  `parallel_for` over disjoint chunks.
- **`moba-platform`** — the only crate that links `winit`: window config/error type,
  physical-key input snapshot with focus-loss safety, 30 Hz `FixedStep`, frame pacing,
  and the `App` loop. `moba-client` is a smoke-test app that exits after
  `MOBA_SMOKE_FRAMES` frames.
- **`moba-shaders`** — GLSL compiled to SPIR-V at build time and validated with
  `spirv-val`.
- **`moba-render`** — ash-based Vulkan renderer: validation layers, device scoring,
  Vulkan 1.2/1.3 feature negotiation, swapchain, sync2 barriers, dynamic rendering,
  instanced units via a storage buffer, procedural terrain with vision-circle fog, a
  screen-space bitmap-font UI batch (no texture/descriptor/pass), and CPU readback capture.
- **`moba-game`** — data tables and pure rules: 5 heroes, 15 abilities, minions, towers,
  nexuses, the three-lane map and integer mitigation.
- **`moba-nav`** — deterministic spatial grid with exact-radius queries.
- **`moba-sim`** — fixed-tick authoritative match simulation: commands, combat, abilities,
  waves, towers, respawns, bots, kill credit/team scores, events and a stable state hash.
- **`moba-server`** — headless 30 Hz match runner with tick statistics (`--ticks`, `--seed`);
  `--listen ADDR --seconds N` hosts a network match.
- **`moba-net`** — UDP wire protocol, fragmentation, handshake, session slots, intent
  input, NetId mapping, visibility-filtered delta snapshots, local-hero prediction,
  remote-entity interpolation, client snapshot history and RTT/bandwidth counters.
- **`moba-netsim`** — per-client UDP link conditioner (latency, jitter, seeded loss).
- **`moba-tools`** — `soak` for the five-minute network gate, `fuzz-wire` for decoder fuzzing,
  `replay` for deterministic replay verification, and embedding examples.
- **Replays** — `.mbr` files store config + resolved inputs + expected hash; server `--record`
  and `moba-tools replay` verify them.
- **`moba-engine`** — umbrella crate with `sim`, `net` and optional `render` features; the
  intended entry point for embedding the engine.
- **`moba-client`** — renders either a local bot preview or a live server match
  (`MOBA_SERVER`); predicted local hero, interpolated remote units, fog-of-war terrain,
  health/mana/cooldown/timer/score HUD, Tab scoreboard, mouse move/attack/cast controls,
  camera follow and headless capture modes.

## Verification actually run

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings` (with `missing_docs`)
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"`
- `cargo check -p moba-engine --no-default-features` and `--all-features`
- `cargo test --workspace --locked` — 161 tests
- `cargo bench -p moba-ecs --bench ecs_iter` — see `docs/perf/0001-core-runtime.md`
- `just layering` equivalent via `cargo tree` — server/sim/game/nav/net are clean of
  render/platform/winit/ash.
- Miri: `moba-core` + `moba-ecs` pass under Stacked Borrows with strict provenance
  (85 s); `moba-jobs` passes zero-worker and threaded tests under Tree Borrows with
  `-Zmiri-ignore-leaks` (crossbeam-epoch's deferred bags are the leak source).
- Client smoke: `MOBA_SMOKE_FRAMES=60 cargo run -p moba-client` exits 0 on Linux
  (Wayland session; the same path is exercised on X11).
- Online client smoke: external `moba-server --listen 127.0.0.1:7791 --seconds 8` plus
  `MOBA_SERVER=127.0.0.1:7791 MOBA_SMOKE_FRAMES=150` exits 0.
- Renderer: 500 units + terrain at 1920×1080 under lavapipe with
  `MOBA_VULKAN_VALIDATION=required` — zero validation messages; capture stored at
  `docs/images/m3-500-units.png` (see `docs/dev/software-vulkan.md`).
- Simulation: `cargo bench -p moba-sim --bench tick` — 12.9 µs/tick steady state;
  `cargo run --release -p moba-server -- --ticks 20000` — 0.25 s wall, 106 peak units,
  stable hash. Ten-thousand-tick determinism tests pass.
- Live slice: `MOBA_WARMUP_TICKS=3200 MOBA_CAPTURE=frame.ppm` renders the simulated match
  with zero validation messages; see `docs/images/m4-simulation-slice.png`.
- Network soak: five minutes (protocol v1) with ten clients through independent
  150 ms/5%/30 ms links — all connected, ~8,500 snapshots each, 4.1–4.5 KiB/s down and
  ~1.17 KiB/s up; server sent 13.4 MiB over 9,120 ticks. 30 s reruns measured 4.0–5.2 KiB/s
  (v2), 4.2–5.7 KiB/s (v3), 4.2–5.7 KiB/s (v4) and 4.2–5.8 KiB/s (v5); protocol-v6
  plaintext measured 4.6–6.8 KiB/s down (v6), 4.7–7.6 KiB/s (v7), 4.8–7.7 KiB/s (v8)
  5.2–8.8 KiB/s (v9) and 5.2–9.2 KiB/s (v10);
  encrypted v6 measured 5.0–7.3 KiB/s down / 1.8–1.9 KiB/s up.
  Protocol/delta/snapshot tests pass.
- Online client: `MOBA_SERVER=local MOBA_DEMO_MOVE=110,128` connected, rendered
  interpolated snapshots with the local hero predicted, and captured a validation-clean
  frame; see `docs/images/m5-network-client.png`.
- HUD/fog/scoreboard: captured under lavapipe with validation enabled — see
  `docs/images/m6-hud-fog.png` and `docs/images/m6-scoreboard.png`.
- Progression/shop: `MOBA_DEMO_BUY=0` and `MOBA_SHOP=1` captures show XP/gold/items and the
  catalogue; an over-UDP test buys an item and observes the spent gold — see
  `docs/images/m6-progression.png` and `docs/images/m6-shop.png`.
- Match flow/minimap: countdown and in-game captures with minimap and floating health bars —
  see `docs/images/m6-countdown.png` and `docs/images/m6-minimap-healthbars.png`.
- Match-flow test: countdown → nexus end → results → rematch with a derived seed and client
  baseline reset.
- Reconnect test: drop a connected client, wait out the short timeout/grace path, resume with
  the session token and verify the same slot and a fresh snapshot.
- Encrypted transport: PSK handshake, snapshot stream and encrypted reconnect tested; wrong
  key is rejected. 10-client encrypted soak stays at 5.0–7.3 KiB/s down / 1.8–1.9 KiB/s up.
- Spectator: `MOBA_SPECTATE=1` capture shows full-map visibility, both teams, minimap and free
  camera; a real-UDP test verifies 5 blue + 5 red heroes with no player slot.
- Ready/surrender: `--ready` lobby capture (`m6-ready-lobby.png`), real-UDP ready-gate test
  and a deterministic surrender replay test pass.
- Content: `--items content/example-pack.toml` client capture shows the custom Vanguard
  hero/name (`m8-hero-content.png`); UDP test verifies custom items, heroes and purchases.
- Map content: map-only pack capture (`m8-map-content.png`) shows the custom one-lane minimap;
  simulation tests verify custom spawns/nexuses and the wire map catalogue.
- Line-of-sight: wall geometry and shader fog capture (`m6-los-fog.png`); minimap walls and
  FPS/frame-time diagnostics (`m6-minimap-fps.png`); server test proves a
  wall hides an enemy hero, and walls align with centered world/vision coordinates.
- Replay manifest: custom-content 555-tick replay verified with `--items` at content
  `0x717e1ddd76abfabe` / state `0xd101300c1c113b98`, pack `Example Pack 1.0.0`, 3 state
  checkpoints; replaying without the pack is rejected.
- Rate limiting: valid-packet flood test drops excess at a 10 pps budget; release 10-client
  soak reports rate_limited=0, avg tick 11 µs / max 54 µs.
- Wire fuzzer: 73,072,000 structured/random decoder iterations in 60 s clean; one-hour gate
  via `just fuzz-wire`.
- Replay: live server recording (`--record`) and `moba-tools replay` reproduce
  `0x8fc6baefe95c96d6` for a 194-tick match; the server recording test and decoder noise test
  cover replay determinism and malformed input.

`just`, `cargo-nextest` and `cargo-deny` are not installed in this environment; their
commands were reproduced manually. Miri is green on core/ecs/jobs; the loom model is still
pending.

## Open-source surface

Dual MIT/Apache-2.0; README, architecture/content/API guides, contributing, security, code of
conduct, changelog, release checklist and GitHub templates. The umbrella crate keeps the
default embedding headless (`sim`+`net`) and puts Vulkan behind `render`. Runnable examples are
in `crates/tools/examples`. Publishing remains opt-in until crate names/ownership and the
repository URL are decided; see `docs/API-STABILITY.md` and `RELEASE.md`.

## Known gaps / next round

1. Loom model of the scope counter protocol; add `World`-level parallel iteration over
   chunk queries and bench it.
2. M6 remainder: ready-up lobby, surrender/vote, terrain line-of-sight fog and map selection.
3. M7 hardening: public identity/anti-DoS, one-hour sanitizer fuzzing, 50-match soak and
   profiling captures.
4. M3 remainder: skeletal skinning, frame-time measurement on min-spec hardware, and a
   golden-image test under lavapipe in CI.
5. The reference machine has no usable hardware Vulkan device; lavapipe is installed in
   `.tools/` for validation (see `docs/dev/software-vulkan.md`).
