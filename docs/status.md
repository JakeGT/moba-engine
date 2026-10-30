# Engine status

Last updated: **2026-10-29**. Workspace **0.4.0** preview; protocol **v41**; replay format **v5**;
simulation logic **v8**; content packs are validated runtime TOML.

The engine is playable end-to-end and has a stable internal contract, but it is pre-1.0.
Two external-resource items remain open: real Windows/macOS GPU frame results (#9/#127) and
crates.io publication enablement (#21). The 2026-10-19 audit remediation is complete, and every agent work package in the
[finish plan](plans/finish-plan-2026-10-20.md) has landed (#297–#320), including the #276
asset, skinning and mirror work. What is left needs people or hardware: the 10-client playtest,
an external security review, real Windows/macOS GPU runs and crates.io ownership.

## Milestones

| # | Milestone | State | Notes |
|---|---|---|---|
| M0 | Bootstrap | ✅ | Workspace, lints, CI, justfile, [agent instructions](../AGENTS.md), issue/PR templates. |
| M1 | Core runtime | ✅ | Fixed-point core, SoA ECS, work stealing, arenas/bitsets; Miri and the [Loom job-scope model](../.github/workflows/loom.yml) are green in CI. |
| M2 | Platform | 🟡 | Linux runtime verified; Windows/macOS compile and CI-checked, runtime GPU validation still open (#9). winit/input, 30 Hz step, settings and headless modes present. |
| M3 | Renderer | 🟡 | Vulkan 1.3 device/swapchain/dynamic rendering, a render graph with HDR target, AgX tonemap composite, quality tiers and per-pass GPU timestamps ([ADR-0008](adr/0008-render-graph.md), [perf/0011](perf/0011-render-graph.md)), client-local materials with KTX2 textures (BC7/BC5/BC4 with mips, CPU-decode fallback), a `MATERIAL_TEXTURED` shader permutation, GLSL includes, a persistent pipeline cache and debug-build shader hot reload (R1b, [ADR-0010](adr/0010-texture-pipeline.md), [perf/0013](perf/0013-materials.md)), compute-simulated GPU-particle VFX (R5: sprites, flipbooks, trails, beams, rings and mesh particles, additive + weighted blended OIT, soft particles, client-local effects bound to presentation events and sockets, per-tier budgets and readability rules, a live-reload preview; [ADR-0013](adr/0013-vfx-runtime.md), [perf/0016](perf/0016-vfx.md)), stylised PBR (GGX, wrap/ramp/painted specular/rim), per-map image-based lighting baked by `env-bake` and one fitted directional shadow map with hard/PCF/PCSS-lite filtering by tier, skinned heroes casting (R2a, [ADR-0011](adr/0011-art-style-lighting.md), [perf/0015](perf/0015-lighting.md)), per-map heightfield terrain with 1–8 splat-textured layers (triplanar on slopes, normal maps, cavity and relief AO) generated from the simulation's map and checked against it by content-check, a GPU fog-of-war visibility texture from own-team vision with line-of-sight walls (no 16/32 caps, blurred and faded, also masking the minimap), height fog, a sky and hills or cliffs past the board edge (R3a, [ADR-0014](adr/0014-terrain-fog-of-war.md), [perf/0017](perf/0017-terrain-fow.md)), a tiered post chain (R6: physically based bloom, per-map exposure with optional eye adaptation, per-map 3D colour-grading LUTs baked by `grade-bake`, FXAA on Low–High and MSAA 4× on Ultra, half-resolution GTAO on High/Ultra from a depth prepass, render scale with dynamic resolution and a native-resolution HUD, accessibility-toggleable death desaturation, damage vignette and low-health pulse; [ADR-0015](adr/0015-post-processing.md), [perf/0019](perf/0019-post.md)), clustered forward dynamic lights from effect `[light]` blocks, team/hover/target outlines in the team palette (screen-space ID buffer on High+, inverted hull on Low/Medium), baked map AO from `ao-bake` and a depth pre-pass with contact shadows on High+ (R2b, R2 done; [ADR-0016](adr/0016-lights-outlines-ao.md), [perf/0020](perf/0020-lights-outlines-ao.md)), ground decals for ability telegraphs, scorch and splats with guaranteed enemy-telegraph contrast, GPU-instanced wind-animated foliage pushed aside only by drawn units, and rivers and pools with flow, depth fade, shoreline foam and screen-space refraction on High, checked against walkability by content-check (R3b, [ADR-0017](adr/0017-decals-foliage-water.md), [perf/0021](perf/0021-decals-foliage-water.md)), instanced units, textured UI atlas, animated hero rig, GPU skinning of `.glb` hero models (rig fallback; #288, ADR-0007), an [animation runtime](adr/0012-animation-runtime.md) (R4a: named clips, idle↔run blend space, crossfades, upper-body masks, additive hit reactions, a client-local state machine driven by snapshots and cast/death/attack events (basic attacks from `EVENT_ATTACK`, protocol v41, R4b-1), spring bones, sockets and a distance LOD; 200 units in 0.12 ms of CPU per frame, [perf/0012](perf/0012-animation-runtime.md); R4b: vertex-animation-texture minions, tiered bone-count LOD, visible socket attachments, 2D blend spaces and the local preview/replays on the same runtime, 200 VAT minions in ≈ 0.01 ms of CPU per frame, [perf/0018](perf/0018-vat-anim-lod.md)) with a [networked skinned-match frame cost](perf/0007-real-hardware.md#networked-match-with-skinned-heroes-wp-42-289) (RTX 5070 Ti, 1920×1080, p95 4.192 ms) and a deterministic networked golden (#289), map themes, [golden-image CI](../.github/workflows/render-golden.yml) and [NVIDIA timings](perf/0007-real-hardware.md). Windows/macOS real-GPU timing remains open (#127). |
| M4 | Simulation | ✅ | [Deterministic 5v5](perf/0004-simulation.md), 3 lanes, towers/nexuses, minions, vision-respecting bots, respawns, XP/gold/items, modes/rules; measured ~26.3 µs/tick steady benchmark (7.876 ms/300 ticks, 2026-10-19). |
| M5 | Networking | ✅ | [UDP, fragmentation](perf/0005-networking.md), delta snapshots, prediction/interpolation, reconnect, spectators, protocol v41, content updates, identity/attestation. Deterministic and sanitizer fuzz gates exist. |
| M6 | Vertical slice | 🟡 | Lobby/countdown/results/rematch, ready/surrender, reconnect, spectator, HUD, shop, minimap, scoreboard, fog/LOS, status icons, procedural audio, persistent settings/remapping/UI scale. R7a HUD polish: MSDF text, styled health bars (damage ghost, shield, ticks, colour-blind palette), floating combat text, cooldown sweeps, screen fades and a loading screen. R7b: 3D hero portraits (hero panel, scoreboard) and a baked top-down minimap, rendered to texture through the render graph. A 10-client playtest on a real network has not been recorded. |
| M7 | Hardening | 🟡 | AEAD, per-client tokens, Ed25519 server/client identity with rotation/revocation, handshake replay window, per-address/per-match/host-wide datagram budgets charged before decode, graceful shutdown, results log, soak/fuzz/bench evidence, Loom model, [50 concurrent matches per box](perf/0010-multi-match.md) (`MatchHost`, `moba-server --matches N`, #293) with per-port poll caps and [flood isolation](perf/0010-multi-match.md#flood-isolation) (#292). Independent external review remains open. |
| M8 | Content pipeline | ✅ | [Runtime](tasks/0008-content.md) item/hero/ability/map packs, manifests, per-client catalogue updates (v36), manifest attestation, map names/rotation, visual themes and modes. Static `.glb` hero models load from a client-local asset root with procedural fallback (#286, ADR-0007) and are GPU-skinned when the file has a skin (#288). Packs reference visuals by name through an `[assets]` manifest that content-check validates, and the names are hashed and carried in the catalogue (protocol v40, #285). Map visuals names resolve to client-local descriptors (terrain tint, walls, props, optional meshes) with the built-in themes as fallback, and switch on content updates and rotation without a restart (#287). |

## Core capabilities

- **Deterministic simulation**: Q32.32 fixed point, integer math, no wall clocks/hash maps in
  gameplay, seeded config, stable state hashing and replay verification.
- **Server authority**: clients send intent only; cooldowns, mana, range, vision, purchases and
  movement are validated by the server.
- **Netcode**: delta snapshots against acked baselines, team visibility filtering, local-hero
  prediction, remote interpolation, reconnect grace, spectators and between-match catalogue
  updates.
- **Security**: optional ChaCha20-Poly1305 sessions, per-client token ring, pinned Ed25519
  server identity, Ed25519 client identity with rotation/revocation, content-pack attestation,
  replay window and application-layer DoS budgets.
- **Content**: validated runtime TOML packs with hashes, signed manifests, hot reload in
  Lobby/Results, map names/themes, empty-lobby rotation and 1v1/ARAM modes.
- **Client**: MSDF HUD text (R7a) plus the pixel-art icon atlas, styled health bars, combat
  text, cooldown sweeps, screen transitions, 3D hero portraits and a baked minimap (R7b,
  render-to-texture), settings/key remapping/UI scale/team palette,
  procedural audio,
  animated hero rig, map themes, spectator camera, capture/frame-log modes.
- **Tooling**: replay verifier, match/network soak, wire fuzzers (deterministic + cargo-fuzz),
  content checker/signer, state hash, image fingerprint, atlas and MSDF font bake, Vulkan probe
  and Loom.

## Verification evidence

- `cargo test --workspace --locked`: **780 passed**, 0 failed, 3 ignored.
- `cargo test --workspace --release --locked`: passes (same suite; CI job `release tests + doctests`).
- `cargo fmt --all --check`; clippy `--workspace --all-targets -- -D warnings`; rustdoc with
  `RUSTDOCFLAGS="-D warnings"` are clean.
- Feature matrix (`moba-engine` no-default, `render`, all-features) and crate layering checks
  pass. `scripts/agent-verify.sh --full` runs these plus Loom/bench compile/soak/fuzz.
- Deterministic cross-OS golden state hash: `0x8db35a0f5aab5190`.
- Golden render workflow: on lavapipe with validation + sync validation, the 64×36 default-scene
  fingerprint, a networked fingerprint with skinned heroes at a pinned tick (#318) and a VAT
  minion close-up (R4b) are green.
  Both were rebaselined deliberately for the R1a HDR + AgX tonemap frame
  ([ADR-0008](adr/0008-render-graph.md)) and again for the R7a HUD (MSDF text, health bars;
  [perf/0014](perf/0014-ui.md)); the networked one again for R7b (hero portrait and baked
  minimap; the default scene's local HUD has neither and is byte-identical), and both for the
  R2a lighting (PBR, IBL, shadows; [ADR-0011](adr/0011-art-style-lighting.md)), and all three
  (with the VAT close-up, whose local preview gains the terrain) for R3a (terrain, fog-of-war
  texture, height fog and sky; [ADR-0014](adr/0014-terrain-fog-of-war.md)),
  which also added two map goldens: the networked whole-map overview and the local
  preview's low horizon orbit, and for R2b (hero outlines, the High-tier pre-pass and contact
  shadows; [ADR-0016](adr/0016-lights-outlines-ao.md)), which added the ember golden (walls,
  baked AO and outlines beside walls) and the outline occlusion probe. R3b
  ([ADR-0017](adr/0017-decals-foliage-water.md)) made the grass map the showcase (a river with
  fords, reeds, brush, flowers and grass), rebaselined every golden, and added three: the
  river close up on the default tier and on Low, and the 5v5 teamfight replay's ground decals. The graph's barrier plan, offscreen, shadow and fog-of-war
  variants included, is unit-tested headless.
  R1b's materials leave both captures byte-identical to the then `main` (the textured material is on
  the static `sentinel` model, which neither golden uses).
- Deterministic wire fuzzer historical evidence: v11 **2,476,647,000** iterations clean and
  final v12 **2,384,035,000** clean. Current cargo-fuzz targets `wire`, `replay`, `content`,
  `glb`, `ktx2` (R1b: the KTX2 loader and material descriptors; R2a: cube maps and
  environment descriptors) and `vfx` (R5: effect descriptors and bindings; R3b: decals) run in the manual/nightly
  `fuzz` workflow; a 30 s-per-target run passed for the first three.
- Loom model: `loom.yml` weekly/manual run passes.
- 10-client soak (historical, v12): all connected, 9,117 ticks, avg tick 15 µs, max 304 µs,
  ~5.0 KiB/s down / 1.17 KiB/s up per client, no rate-limit drops.
- 50-match soak (historical): 509,820 ticks, unique hashes per seed, avg 6 µs/max 3,021 µs;
  outcomes blue 30 / red 20 (Wilson interval includes even odds).
- Allocation guards: 6/6 simulation budget tests pass (steady/teamfight/respawn/shop/status/
  wave); snapshot build+encode allocates **0** in steady state.
- Replay v5: round-trip, checkpoint, human-purchase, content-hash and unsupported-version
  tests pass; golden replay fixtures are checked in, including the scripted 5v5 teamfight
  `tests/replays/teamfight-5v5.mbr` (the R5 VFX evidence scene, verified by a test).
- Content: all eleven `content/*.toml` packs validate; 1v1/ARAM packs replay deterministically;
  signed-pack verify accepts valid signatures and rejects tampering/untrusted publishers.
- Renderer: validation-required lavapipe capture passes with zero messages. Real NVIDIA
  RTX 5070 Ti at 1920×1080: p50 4.181 ms, p95 4.197 ms, p99 4.212 ms, max 4.538 ms (vsync off).
  macOS hosted runner recorded p95 16.945 ms; Windows hosted runner has no Vulkan GPU.
- Release: `v0.1.0`–`v0.4.1` workflows build Linux/Windows/macOS archives;
  [v0.4.1](https://github.com/JakeGT/moba-engine/releases/tag/v0.4.1) (security patch from `release/0.4.x`) is the current preview; `main` targets 0.5.0.
  crates.io publication remains opt-in (`publish = false`, #21).
- CI: Linux/Windows/macOS gates, Miri, cargo-deny, release tests/doctests, MSRV, no-op/real
  `renderer-gate`, docs link/contract check, golden/fuzz/Loom/platform workflows. Nightly
  failures open an issue automatically. Hosted-runner capability limits are documented, not treated as passes.

Screenshots are in [the gallery](screenshots.md). Renderer/network captures use lavapipe and
headless readback; real-GPU evidence is in [perf/0007](perf/0007-real-hardware.md).

## Tooling availability

The development container may omit `just`, `cargo-nextest`, `cargo-deny`, `cargo-fuzz` or
Miri. Prefer `scripts/agent-verify.sh` for portable gates; CI runs Miri, cargo-deny, cargo-fuzz,
Loom and the nightly soak. Record which optional tools were available when quoting local
evidence.

## Known gaps and tracked work

Open issues: <https://github.com/JakeGT/moba-engine/issues>
Second-audit plan: [`docs/plans/audit-remediation-2026-10-19.md`](plans/audit-remediation-2026-10-19.md)

- **#9 / #127** — real Windows/macOS GPU frame results and min-spec validation. Linux/NVIDIA
  evidence is complete; the hosted macOS result misses the frame target and Windows hosted
  runners expose no Vulkan device. Requires real platform hardware or a revised acceptance
  decision.
- **#21** — crates.io publication. Metadata, release artifacts and policy are ready; registry
  ownership plus `CARGO_REGISTRY_TOKEN` are required. `publish = false` remains intentional.
- **#45** — extensibility roadmap map, not unfinished implementation work.
- **#276** — asset pipeline, GPU skinning and mirror/balance: done (#308, #309, #311, #314,
  #316–#319). The epic and #286/#290 are still open on GitHub. Items have no asset references yet.
- Multi-match flood isolation (#292) is application-layer only: a spoofed multi-source flood
  on one port can still starve new handshakes on that port (other matches are unaffected, see
  [perf/0010](perf/0010-multi-match.md#flood-isolation)); public servers still need a
  DDoS/connection gateway.
- Future product work (not blocking the current preview): independent external review, larger
  balance sweeps.

## Open-source surface

Dual MIT/Apache-2.0. Repository: <https://github.com/JakeGT/moba-engine>.
README, architecture, content authoring, API stability, security, contributing, code of
conduct, release checklist, screenshot gallery, agent guide/runbook and GitHub issue/PR
templates are included. `publish = false` remains intentional until crate names, ownership and
per-crate metadata are finalized; see [RELEASE.md](../RELEASE.md) and [docs/agents.md](agents.md).
