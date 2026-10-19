# Changelog

All notable changes are documented here. Format loosely follows Keep a Changelog; the
project is pre-1.0, so minor versions may contain breaking API changes (see
[docs/API-STABILITY.md](docs/API-STABILITY.md)).

## [Unreleased]

### Added
- Rules `team_size`/`bot_fill` (protocol v20), 1v1-mid and ARAM example packs, and
  `match-soak --content` replay verification.
- Procedural animated hero rig (multi-part cubes; deterministic tick pose, cube fallback for
  minions/structures) and updated golden fingerprint/gallery capture.
- `MOBA_FRAME_LOG`/`MOBA_VSYNC` benchmark path and real NVIDIA 1080p p95/max timings.
- Built-in map visual themes (`[map] theme`) with protocol v25 wire id, terrain tint/wall colours,
  local `MOBA_CONTENT` preview and a snow ARAM gallery capture.
- Protocol v24 catalogue updates: connected clients adopt a reloaded pack between matches.
- Client identity allowlist rotation/revocation and ADR 0006 trust-boundary/threat-model docs.
- Map names and empty-lobby server map rotation (protocol v23).
- Client-only procedural audio: event mapping, volume/mute settings, graceful no-device fallback.
- Optional Ed25519 content-pack attestations with signing/verification CLI and server pinning.
- Bounded Loom model for the job-scope submit/wake protocol plus a weekly/manual CI job.
- cargo-fuzz targets (`wire`, `replay`, `content`) with seed corpus and a bounded
  ASan/UBSan nightly/manual workflow.
- Removed the ttf-parser advisory ignore by dropping winit's ab_glyph CSD feature (#22).
- Settings camera sensitivity/zoom, attack mouse button, spectator camera keys and reset.
- Client TOML settings: remappable movement/cast/stop/shop/scoreboard/ready/surrender keys,
  validation with safe fallbacks, save/load, reset defaults and documented example file.
- `MOBA_UI_SCALE` HUD scaling: logical layout + scaled vertex upload + scaled cursor hit-testing.
- Ability/inventory/shop icons from the baked UI atlas.
- Textured UI atlas: one glyph quad per character, baked 128×48 RGBA atlas and
  `moba-tools bake-ui-atlas`; sampler-backed UI pipeline (no TTF parser).
- Default bots now respect fog of war for target acquisition; `SIM_LOGIC_VERSION` 6.
- Runtime minion/tower combat stats in `[rules]` (protocol v22), used for simulated spawns.
- Rules now carry level cap, XP curve and minion/tower/nexus vision (protocol v21); client HUD
  no longer imports `LEVEL_CAP`, `xp_to_next` or minion/tower vision constants.
- Deterministic integration tests for the 1v1-mid and ARAM packs (hero counts + replay hashes).

### Fixed
- Client ignores duplicate ContentUpdate hashes and rejects decoded catalogues that fail validation before swapping state (#139).
- Job workers wait indefinitely on the generation condvar instead of polling with a timeout; the Loom model uses the same blocking wait so lost wake-ups fail the model (#143).
- Snapshot allocation guard uses per-thread counters, eliminating the release-test flake from unrelated harness threads (#142).
- Own-team events no longer bypass endpoint visibility; hidden hit/death endpoints are filtered and covered by a NetServer unit test (#138).
- Added an allocation guard for ability-pack zone/on-hit ticks (no steady-state allocations) (#140).
- Snapshot events prioritise deaths, structure kills and match end when the 16-event cap is hit (#136).
- Content parsing rejects missing modifier kinds and on-hit kinds before ability construction; `modifier_id` no longer silently falls back to id 0. Example ability-pack areas were reduced from near-map-wide radii to tactical radii (#135).
- Bot fog masks are built in one world/candidate pass (benchmark −14.3% on `tick_full_5v5`), with a mask/visibility equivalence test (#140).
- Replays no longer diverge when a mid-match disconnect changes `bot_mask`; the mask is an input source, not state, and `SIM_LOGIC_VERSION` is 7 (#137).
- Fog-of-war: hidden event endpoints are redacted by dropping the event, attack orders on unseen targets are rejected, and teams are reassigned after catalogue reloads (reload is rejected if occupied slots exceed the new team size) (#138).
- Client snapshots are cloned only for a new sequence number; minimap clicks use the handshake map extent; the unwired ability-button hit region was removed (#141).
- Every cast emits exactly one `AbilityCast`; server event snapshots reuse capacity and pending events use a bounded `VecDeque` (#136).
- Rules now hash minion/structure `vision_range`, reject non-finite/out-of-range stats with a field-specific error, keep per-unit-kind defaults, and derive the legacy vision fields from the unit stats (#135).
- Job scope state is reference-counted; `JobRef`/`PendingGuard`/`Scope` no longer hold a
  stack `ScopeShared` pointer (#143).

### Changed
- Added the second-audit remediation plan (security-first #135–#153 + private advisories).
- Documentation/agentic-coding refresh: current contract versions, status/plan/task evidence, README, perf logs, release/contributing guidance and an operational `docs/agents.md` runbook with `scripts/agent-verify.sh`.

## [0.2.0] — 2026-10-18

### Added
- `[rules]` content section for hero respawn (base + per-level) and wave cadence; hashed,
  protocol v19 and wired into simulation bounds.
- Map `extent` in content, hash/wire (protocol v18) and simulation bounds; client camera,
  minimap, fog and healthbars scale from the handshake map.
- Data-only arcing projectiles: TOML `arc`, hashed fixed-point effect field, protocol v17
  wire/event support and client sinusoidal arc rendering. ADR 0005 selects typed composition
  over embedded scripting.
- Nightly macOS MoltenVK probe job; green dispatch enumerated 1 device with portability enabled.
- Headless `vulkan-probe` tool plus a macOS MoltenVK workflow for instance/device enumeration
  coverage (#32).
- HUD network-type boundary: `hud/model.rs` is now the only HUD module naming `moba_net`;
  drawing modules use `HudUnit`/map wrappers.
- `BotPolicy` trait with the built-in lane/farm/fight policy as default and
  `Simulation::set_bot_policy`.
- Grid-accelerated line-of-sight source queries with early exit in the observe pass;
  three-view observe benchmark ~13.1 µs.
- Global-allocator snapshot guard: steady-state build/encode of a 100-entity delta performs
  zero allocations.
- Soak instrumentation for max simulation tick and whole-frame timing, plus 5-minute profile
  showing zero >1 ms frames (max 363 µs).
- 1/20/40-minute snapshot scaling benchmark; build+encode stays ~3.6–3.7 ns per visible
  entity, i.e. cost tracks view size rather than match age.
- Snapshot history recycles per-client entity buffers instead of cloning each snapshot.
- `build_delta_into` reuses a server-owned snapshot delta and retains vector capacity across
  snapshots and clients.
- Dense per-client net-id table indexed by entity slot + generation, removing the snapshot
  hot-path linear scan.
- Single-query observe candidate pass with a reused scratch vector; observe benchmark
  improves from 19.2 µs to 13.2 µs for three views.
- Flat deterministic spatial grid with reusable counting-sort buffers; steady/teamfight ticks
  now perform zero heap allocations.
- Allocation guard matrix for wave spawns, 10-hero teamfights, respawns and shop purchases;
  tests are serialised and budgets tightened to ≤8 allocations / ≤1 KiB per window.
- `NetServer::with_system` deterministic extension hook with ordered `ServerPhase` callbacks
  and an embedding example.
- Focus-based network input routing: shop focus consumes gameplay input through a dedicated
  handler instead of a modal early-return branch.
- Ability boxes register hover rects and show name, targeting, range, cooldown, mana and
  effect summary tooltips from the wire catalogue.
- Minimap clicks issue a world move; shop rows show a hover tooltip with item stats.
- HUD hit rectangles are registered each frame; mouse clicks on panels no longer issue
  `MoveTo`, and shop items can be bought by clicking a row (keyboard 1–8 still works).
- Shared `HudModel` frame state used by both network and local-preview HUDs.
- A test-only allocation counter proves a warmed local-HUD frame performs **zero heap
  allocations** on the calling thread (`forbid(unsafe_code)` remains in non-test builds).
- HUD text now formats into fixed 256-byte stack buffers (`hud_fmt!`/`hud_static!`); item
  summaries, timers, scoreboard and shop labels no longer allocate per frame.
- Client HUD split into `hud/{theme,geometry,healthbars,minimap,status,model}` with shared
  palette constants, a UI vertex-truncation counter in the debug overlay, and a local-HUD
  draw-list test. `main.rs` is an 8-line entry point.
- Golden-image fingerprint tool/check and a manual or `render-golden`-label lavapipe workflow
  with validation + sync validation required; capture artifacts uploaded on failure.
- Protocol v16 presentation events (cast/projectile/hit/death/objective/zone) with per-viewer
  fog filtering, projectile extrapolation, client kill feed/projectile rendering and a wide
  `status_bits` field. 30 s 10-client soak measured 4.7–6.3 KiB/s down per client.
- Protocol v15 data-driven ability composition: targeting + fixed-capacity effect lists,
  projectile on-hit effects, periodic zones, all 10 legacy kinds expressed as data, and
  `AbilityKind` removed. Data-only root-on-hit and burn-zone examples ship in
  `content/ability-pack.toml`; `SIM_LOGIC_VERSION` is 5.
- Release metadata and a tag-triggered workflow that runs release tests, inspects package file
  lists and attaches Linux/Windows/macOS server/client/tools archives; publish policy and
  dependency order documented in `RELEASE.md`.
- Seed-driven bot play and fair scheduling phase; fixed-composition mirror sweeps now vary per
  seed. Balance harness/results documented in `docs/perf/0008-balance.md`.
- Protocol v14 modifier catalogue: content-defined `ModifierDef` names/kinds are validated,
  hashed, shipped in `WireCatalog`, and recorded on every modifier instance. `SIM_LOGIC_VERSION`
  is now 3.
- Fixed-capacity always-present `Modifiers` component replaces six ad-hoc status components:
  no status archetype churn, allocation-free expiry/burn passes, derived status flags, and
  one hashed modifier list. `SIM_LOGIC_VERSION` is now 2.
- Protocol v13 complete `WireCatalog` handshake projection: full hero/ability/map/structure
  fields, exact `GameContent::hash` verification on the client, destructured parity conversion,
  and client range/radius overlays from wire data.
- Documentation truth pass: milestone grading/evidence links, security/contributing/API/perf
  corrections, runtime-support wording, and contributor checklists for item stats, statuses
  and ability kinds.
- Replay format v5 with an explicit simulation-logic version, per-slot purchase inputs and
  rejection of unknown layout/logic versions; human purchases are queued in `Playing` and
  recorded even when a movement command arrives in the same tick.
- State hash now covers every registered component/field with unique tags, config fields and
  full projectile data; a coverage test fails when a new component is not hashed.
- CI: cross-platform golden state hash, release/doctest and MSRV jobs, plus scheduled
  one-hour fuzz, five-minute soak, 50-match soak and benchmark runs.
- Strict content schema: unknown fields and enum values are rejected, stats/coordinates are
  bounded, mitigation uses i64 intermediates, and wire/content limits are compile-time checked.
- Audit remediation W0: panic-safe ECS despawn (deferred Drop + hard bounds checks), shared
  base/effective move-speed model, point-ability cast-range validation, bounded decoder
  counts, renderer GPU/unsafe correctness fixes, and deterministic CI example/UDP tests.
- Final 300 s / 10-client lossy soak evidence (all connected, avg tick 15 us).
- Release-audit evidence for optimized tests, feature matrix, layering, docs links and packs.
- `haste` self movement-buff ability kind.
- Rooted units can no longer bypass control with a dash.
- Burn state hashing now includes applying entity and damage type.
- Free-pan/rotate/zoom spectator camera (WASD, arrows, Shift boost).
- Refreshed sim/snapshot benchmark capture after the status-effect work.
- Shop pagination (arrows) so packs with more than eight items are purchasable.
- Health-bar icons for shield/slow/root/silence/burn statuses.
- One-hour wire fuzz completed 2,476,647,000 iterations clean (protocol v11).
- Optional Ed25519 client identities with address-bound server challenges and public-key
  allowlists (`--allow-client-key`, `MOBA_CLIENT_IDENTITY`).
- Graceful SIGINT/SIGTERM shutdown that flushes an in-progress replay.
- Documented 50-match bot-balance confidence interval.
- CI tools gate now runs a 30-second wire-decoder fuzz pass.
- `--json` summaries for `replay` and `content-check` tools.
- `burn` damage-over-time ability kind.
- `silence` ability kind blocking casts until expiry.
- `root` ability kind blocking movement until expiry.
- Optional JSONL match-results log (`--results-log`).
- `slow` ability kind with movement multiplier, expiry/respawn restore and client tint.
- Client FPS/frame-time HUD and wall segments on the minimap.
- Per-hero level-growth fields in content packs.
- Bounded handshake nonce replay window (same-address retransmits allowed).
- Replay format v4 periodic state checkpoints and first-divergence tick reporting.
- Per-client token key ring for authenticated/encrypted per-player sessions and rotation.
- Content pack manifests (name/version/author) and replay format v3 carrying them.
- Allocation-regression test and hot-path fix: command ordering no longer allocates a `Vec`
  per tick (312→12 allocations per 300 ticks).
- Global UDP packet budget (`--max-total-pps`) complementing per-address limits.
- TOML dedicated-server configuration (`--config server.example.toml`) with CLI override.
- `shield` ability kind with timed damage absorption, snapshot flag and client tint.
- Content hot reload (`--reload-content`) applied only in an empty lobby, with a server test.
- CI release gate for content validation, deterministic 10-match soak and benchmark
  compilation.
- Optional pinned Ed25519 server identity (`--server-key`, `MOBA_SERVER_PUBKEY`) with a
  signed Welcome challenge.
- Criterion snapshot-codec benchmarks and a profiling capture log
  (`docs/perf/0007-profiling.md`).
- Runtime map vision blockers: TOML `[[map.walls]]`, server LOS filtering, shader
  per-pixel wall shadows, aligned wall geometry and wall data in the handshake map.
- `match-soak` 50-match deterministic harness with composition rotation and `content-check`
  validator; mirrored hero spawn/pick fairness fixes and alternating wave/command order.
- Per-address UDP token-bucket rate limiting (`--max-pps`) with bounded/LRU buckets, drop
  counters and average/max simulation tick telemetry.
- Replay format v2 with content-hash manifests; `moba-tools replay --items`; shared
  `moba-game` TOML loader used by server and tools.
- Runtime item, hero, ability and map content packs (`--items FILE`), validated and published to clients in
  the handshake; fixed progress-only delta encoding bug found by the content integration test.
- Deterministic replay recording (`moba-server --record`) and verification
  (`moba-tools replay`), including a server recording test and decoder fuzz coverage.
- Optional pre-shared-key authenticated encryption: HMAC handshake proofs, HKDF session keys,
  ChaCha20-Poly1305 payloads, encrypted reconnect and privacy-preserving debug output.
- Open-source project surface: dual MIT/Apache-2.0 license, README, architecture,
  content-authoring, API-stability and security docs, contributions guide and templates.
- `cargo doc` quality gate and CI documentation job.
- `moba-engine` umbrella crate with `sim`, `net` and optional `render` features.
- Match flow: lobby/countdown/playing/results/rematch state machine, match overlays, minimap,
  floating health bars and protocol v4 (phase/countdown).
- Runnable embedding examples (`headless_match`, `scripted_orders`), release/security/contributing
  guidance, third-party notices and issue/PR templates.
- `missing_docs` lint for the public API.

## [0.1.0] — preview

### Added
- Deterministic 30 Hz 5v5 MOBA simulation: three lanes, 5 heroes × 3 abilities, minions,
  towers, nexus, respawns, bots, events and state hashing.
- Hero levels/XP with shared experience, gold bounties, passive gold, eight items, six-slot
  inventories, validated shop and bot purchases.
- Custom UDP stack: handshake, session slots, intent input, fragmentation, visibility-filtered
  delta snapshots, client prediction, interpolation, link conditioner and soak/fuzz tools.
- Server-authoritative dedicated server with bot fill, timeouts and bandwidth counters.
- Vulkan 1.2 client: instanced units, procedural terrain, vision-circle fog of war, bitmap
  HUD (health/mana/XP/cooldowns/items/gold), Tab scoreboard, shop, VICTORY/DEFEAT and capture.
- ECS, job system, core fixed-point primitives, spatial grid and shader build validation.

### Protocol
- Version 12: Ed25519 client identity fields and server `Challenge` packet.
- Version 11: Ed25519 server identity public key/signature in `Welcome`; pinned clients.
- Version 10: handshake carries validated runtime item, hero, ability and map catalogues,
  with a catalogue hash bound into the Welcome HMAC.
- Version 9: handshake carries a validated runtime item catalogue.
- Version 8: ready-up and surrender vote commands, lobby ready/player masks.
- Version 7: read-only spectator handshake/slots and full-map observation; spectator welcome
  flag.
- Version 6: optional authenticated, encrypted transport (PSK handshake + ChaCha20-Poly1305
  payloads); handshake auth fields; reconnect token; snapshots carry phase/countdown,
  team scores, outcome, hero pick and per-hero progression; shop intents are one-shot
  commands applied immediately by the server.

### Known limitations at the 0.1.0 preview tag
- At this tag there was no encryption/authentication and content was compile-time; the
  Unreleased section records the later AEAD/identity and runtime-pack additions.
- Lobby/rematch/reconnect, minimap, floating health bars and line-of-sight fog were not yet
  present at the tag.
- Windows/macOS builds are CI-checked but rendering has been validated primarily on Linux
  (lavapipe); min-spec hardware timings are not yet recorded.
