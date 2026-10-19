# Audit remediation plan — 2026-10-18

Status: **actionable waves complete; blocked on external resources** (2026-10-19)
Tracking issue: [#46](https://github.com/JakeGT/moba-engine/issues/46)  
Scope: all audit findings #23–#45 from 2026-10-18.

## Execution result (2026-10-19)

W0–W3 and all actionable W4/W5 work landed and shipped in `v0.2.0`:
#7, #8, #10–#20, #22–#44 (excluding external-resource items) are closed with tests/evidence.
Current contract: protocol **v25**, replay **v5**, simulation logic **v6**, **248 tests**, golden
state hash `0x1923e3d7f3392604`, golden render workflow green.

Two exit items remain and are deferred rather than claimed:

- **#9 / #127 — real Windows/macOS GPU results.** Linux/NVIDIA is complete (RTX 5070 Ti,
  p95 4.197 ms, max 4.538 ms, validation clean). The hosted macOS runner records p95
  **16.945 ms** (misses the 6.9 ms target) and the hosted Windows runner has no Vulkan device
  (`VK_ERROR_INCOMPATIBLE_DRIVER`). This needs real platform hardware or a revised acceptance
  decision; see `docs/perf/0007-real-hardware.md`.
- **#21 — crates.io publication.** Metadata, package inspection, branch protection and tagged
  GitHub release artifacts are complete. `publish = false` is intentional and no registry
  token/ownership exists in this environment. Publication order/policy is in `RELEASE.md`.

Do not treat this plan as fully exit-complete until those two items are resolved.

This plan fixes the reported correctness, security, performance, compatibility and
extensibility problems in an order that avoids accumulating incompatible replay/protocol
changes. It does **not** treat the engine as complete until the exit gates below are met.

## Principles

1. **Correctness and determinism before refactors.** Fix replay/autority/UB bugs before
   changing the data model around them.
2. **Batch incompatible formats.** State-hash, replay and protocol changes are serialized into
   explicit release trains; don't bump them in every PR.
3. **One writer for wire/replay/content schemas at a time.** Protocol/replay/content PRs are
   merged sequentially, not in parallel, to avoid rebasing lossy field lists.
4. **Every fix gets a regression test and evidence.** Bugs get a test that failed before;
   perf work gets before/after numbers; renderer work gets validation/sync-validation evidence.
5. **No direct pushes once branch protection lands.** One branch/PR per issue or coherent wave;
   conventional commits; issue links; green required checks.
6. **Truthful milestones.** `docs/status.md` moves to ✅ only when every exit criterion has a
   linked artifact. Unfinished work stays in the tracker.
7. **No new public API promise until a wave is released.** Pre-1.0 breaking changes are allowed,
   but every wave updates `CHANGELOG.md`, protocol/replay/content docs and migration notes.

## Release trains and compatibility

| Wave | Theme | Format impact | Exit artifact |
|---|---|---|---|
| W0 | Stabilize | None (one possible behavior-only cast-range fix) | green CI + validation evidence |
| W1 | Determinism & replay | **Replay v5** + `SIM_LOGIC_VERSION`; state-hash changes | re-recorded golden replay; human-purchase replay |
| W2 | Contracts & catalogue | **Protocol v13**, content-hash/wire parity | parity tests and client range/tooltips data |
| W3 | Extensible simulation | **Protocol v15** composition, **v16** status/events | data-only skillshot-root/burn-zone examples |
| W4 | Client / renderer | UI/protocol changes as needed | golden screenshots + HUD tests |
| W5 | Hardening & release | Publishing metadata only | 0.2 tag/release candidate |

Any wave that changes a format must update **all** of: encoder/decoder, version constant,
fuzzers, parity/golden tests, replay fixtures, `CHANGELOG.md`, `AGENTS.md` and relevant docs.

---

## W0 — Stabilize main (no format churn)

**Goal:** remove reachable UB, server-authority/replay defects and renderer correctness bugs
before refactoring.

### W0.1 Safety / authority
- [x] **#33 ECS despawn panic safety.** Move/drop guard + `assert_eq!` + bounds-checked access
  and a Miri drop-panic test. *Blocks all trusted embedding.*
- [x] **#25 point-AoE `cast_range`.** Validate Slow/Root/Silence/Burn; define `cast_range = 0`
  as self-centred; fix Burst behavior/docs; content-check warning; no mana/cooldown on reject.
- [x] **#24 Haste/Slow base bug.** Separate base speed from effective speed; tests for
  Haste→Slow, Slow→Haste, re-application and item purchase during effects.
- [x] **#23 human shop replay divergence.** Queue purchases through `TickInputs` (recommended:
  dedicated per-slot purchase field so move+shop in one tick remains possible), phase-gate to
  `Playing`, record it, and test a real-UDP human purchase replay.
- [x] **#28 decoder allocation bounds.** Replay frame/checkpoint counts and wire snapshot
  counts; caps by `remaining/MIN_RECORD`; regression tests + fuzz count mutations.
- [x] **#32 renderer audit.**
  - [x] macOS `VK_KHR_portability_enumeration` extension + runtime enable path.
  - [x] swapchain acquire barrier (`COLOR_ATTACHMENT_OUTPUT` source stage).
  - [x] capture buffer resize/format/TRANSFER_SRC validation.
  - [x] optional descriptor-indexing features; resize/fence/drop-order fixes.
  - [x] validation error count + sync validation in Required mode.

### W0.2 CI trust
- [x] **#34 duplicate example target name and clock/UDP flake.** Rename one example; timeout
  ≥3× ping or inject a clock; drain transient socket errors with `continue`; consider
  `SIO_UDP_CONNRESET = FALSE`.
- [x] **#35 CI gates:** cross-OS fixed state hash, release tests, nightly 1h fuzz + 5m soak +
  match-soak, MSRV, doctests, concurrency cancellation, bench gate (warn/fail thresholds).
- [x] **#31 strict content schema:** `deny_unknown_fields`, typed enums (no silent physical/
  blue/tower fallbacks), coordinate/stat bounds, saturating/i64 math, wire/content limit
  assertions, content-check rejection tests.
- [x] **#37 docs truth pass:** milestone evidence links, tooling/Miri wording, CHANGELOG,
  SECURITY, CONTRIBUTING, perf log, AGENTS repo map, API tiers, contributor extension
  checklists.

**W0 exit**
- [x] `cargo fmt`, clippy, docs, 178+ tests and release tests green on Linux/Windows/macOS.
- [x] Miri drop-panic test green; validation + sync validation zero messages for client/capture.
- [x] W0 closed before replay v5; purchases landed in the W1 train.
- [x] New regression tests for #23/#24/#25/#28/#33/#34.
- [x] Status docs no longer claim ✅ without evidence.

---

## W1 — Determinism and replay contract (one coordinated train)

**Goal:** make the authoritative state and replay format trustworthy before changing gameplay
data structures.

### W1.1 State hash contract
- [x] **#26 state hash coverage.** Hash every listed component/field (MoveSpeed, Resistances,
  Regen, VisionRange, LastHitBy, AttackDamageType, HeroSlot; Health/Mana max; AttackStats
  fields; projectile direction/speed/radius/type). Unique tags. Single registry or `HashState`
  trait; destructure structs; coverage test enumerates every registered component.

### W1.2 Replay format v5
- [x] **#27 replay version.** Set `VERSION = 4` (then publish v5 with this wave), add
  `ReplayError::UnsupportedVersion`, add `SIM_LOGIC_VERSION`; module docs match encoder.
- [x] **#23 purchase frames** (from W0) encoded in replay v5.
- [x] Re-record checked-in golden/custom replays and checkpoints; retain at least one legacy
  v1-header fixture to prove a clear version error.
- [x] Hash/order changes documented as expected divergence; add a human-purchase replay test.

**W1 exit**
- [x] A cross-OS golden state-hash test runs on Linux/Windows/macOS with a fixed seed/content/
  script.
- [x] Human-purchase replay verifies all checkpoints.
- [x] Replay `VERSION`/`SIM_LOGIC_VERSION`/docs/CHANGELOG agree.
- [x] A coverage test fails if a new gameplay component is added without hashing.
- [x] Re-baseline golden hashes deliberately and record the commit.

---

## W2 — Content/wire contract and catalogue parity

**Goal:** make the client catalogue and server content one source of truth; unblock tooltips,
range indicators and later content updates.

- [x] **#31 strict schema** (landed in W0).
- [x] **#40 single `GameContent -> WireCatalog` conversion.**
  - Move conversion into `moba-net`; only conversion path.
  - Ship ability kind, cast range, radius, duration and other display fields.
  - Send `GameContent::hash` (or derive catalogue hash from it); client verifies.
  - Compile-failing destructuring + content→wire→client parity test.
- [x] **Protocol v13** and parity tests; document that wire hashes are no longer a lossy subset.

**W2 exit**
- [x] Client can render range/radius indicators from wire data.
- [x] New content field cannot be silently omitted; parity test in CI.
- [x] Protocol bump documented; old clients rejected clearly.

*#10 (per-client updates) becomes implementable next; do not start it before #40.*

---

## W3 — Extensible simulation and modes

**Goal:** replace ad-hoc status/ability code with stable extension seams, as proposed by #38,
#39 and #45. This is the largest wave and may span multiple releases.

### W3.1 Modifiers and effect composition
- [x] **#38 data-driven ability effects + fixed-capacity `Modifiers`.**
  - Targeting (self/point/direction/unit/range) + effect lists + triggers.
  - Replace six status components with one modifier list; recompute effective stats from
    base + items + modifiers (removes the #24 class of bugs).
  - Fixed-capacity, no hot-path allocation; hash every modifier field.
  - Data-only examples: “skillshot that roots on hit”, “burn zone”.
  - Keep old implementation under `cfg(test)` for one release and diff per-tick `ObservedUnit`
    streams sorted by entity for a pack covering all 10 kinds.
  - Protocol bump for status/visual tags (coordinate with #41).
- [x] **#41 presentation event channel** (protocol v16):
  - [x] Bounded, team-filtered events (cast/projectile/hit/death/objective); visibility safe.
  - [x] `status_bits:u16`; projectiles as extrapolatable events.
  - [x] Client kill feed/projectile rendering; audio hook ready via `GameEvent`.
  - [x] Bandwidth evidence: 4.7–6.3 KiB/s down in 10-client soak.
- [x] **#13 scripting hook** built on #38; sandbox/threat model ADR; deterministic host calls.

### W3.2 Modes and policies
- [x] **#39 `GameRules` / `BotPolicy` seams** (acceptance met; #106 follows up stat blocks/vision bots).
  - Data-driven teams/economy/respawn/waves/minion/tower stats; map extent end-to-end.
  - `BotPolicy` trait + vision-respecting default; `NetServer` system hook with deterministic
    ordering.
  - 1v1-mid and ARAM-style packs in match-soak with replay verification.
  - Client stops importing gameplay constants.
- [x] **#17 balanced bots** seeded sweep landed early; re-run after #39.

**W3 exit**
- [x] All 10 existing kinds expressed as data; old-vs-new per-tick stream comparison matches.
- [x] Protocol v16 status/events documented and re-recorded; composition v15 is landed.
- [x] No hot-path status-effect allocation; allocation guard uses an all-kinds pack.
- [x] 1v1/ARAM packs + custom system embedding example.
- [x] Event/status bandwidth measured.

---

## W4 — Client, renderer and UI

Order matters: golden image first, then structural refactors it can protect.

- [x] **#8 golden-image CI** (landed with a 64×36 fingerprint and validation-required lavapipe job).
- [x] **#32 renderer correctness** from W0 (continues here for descriptor/resize niceties).
- [x] **#42 HUD split:** `HudModel`, `hud/*`, `theme`, zero per-frame `format!`, vertex
  truncation counter/tests; main.rs <600 lines.
- [x] **#43 UI hit-testing:** widget rect registry, focus-based input, mouse shop/minimap/
  tooltips, no HUD click becomes `MoveTo`.
- [x] **#44 textured UI:** UV/texture atlas, offline glyph/icon bake, no client ttf-parser,
  golden image and validation evidence.
- [x] **#7 skinning/animation** after asset/model work (procedural animated hero rig; GPU joint skinning remains a future optimization).
- [x] **#12 asset/model mappings + per-map rendering**, linked to #44/#7.
- [x] **#19 settings/remapping/UI scale** after #42/#44 layout seams.
- [x] **#18 audio** consuming #41 presentation events.
- [x] **#20 map selection** after #40/#10/#39.
- [x] **#10 per-client content updates** after #40/#20 (protocol bump documented).
- [x] **#11 manifest signing/attestation** (content/security).
- [ ] **#9 hardware/min-spec + Windows/macOS renderer validation** after #32 — deferred to
  #127/platform hardware; Linux/NVIDIA evidence complete.

**W4 exit**
- [x] Golden image stays unchanged through HUD refactors (or changes deliberately).
- [x] Zero validation/sync messages; HUD allocation test green.
- [x] UI clicks never leak to world commands.
- [x] Screenshot gallery updated with new UI/visuals.

---

## W5 — Hardening, performance and release

### Performance (can start after W0 tests)
- [x] **#29 allocation guard coverage:** status pack, wave window, 10-hero teamfight, respawn,
  shop, NetServer snapshot path; scratch vectors; copy specs after cooldown/mana checks;
  fixed-capacity modifiers; flat nav grid; record before/after.
  *Status-effect archetype churn is superseded by #38; land the low-risk fixes now.*
- [x] **#30 snapshot scaling:** dense net-id table, ring/pre-sized buffers, single-pass
  observe/grid vision, 1/20/40-minute bench, explain/fix 3 ms max tick, zero steady-state
  snapshot allocations.

### Hardening / security
- [x] **#28 decoder bounds** (W0) + **#15 sanitizer-backed cargo-fuzz/ASan** covering replay/
  wire/snapshot/content decoders.
- [x] **#16 Loom** job-scope model, bounded for CI/nightly.
- [x] **#14 account/reputation/key transparency/player-count attestation** with ADR and
  rotation/revocation tests.
- [x] **#22 replace unmaintained ttf-parser** (winit/ab_glyph upgrade or alternative).
- [x] **#37 docs truth** re-check after every wave; close only when ✅ evidence links hold.

### Repo/release
- [x] **#36 branch protection:** require PRs, `gates` (3 OS), `deny`, `miri`, `tools`; linear
  history; enable Dependabot security/groups; triage #1–#6 (rebase/hold per issue).
- [ ] **#21 crates.io packaging/API metadata/semver checks/first tag**; artifacts shipped
  (`v0.2.0`), registry publication deferred on credentials/ownership.
- [x] **#35 nightly/manual gates** (fuzz/soak/release/bench) with artifacts.
- [x] **#45 extensibility roadmap** kept as a map, not a work item; updated as waves land.

**W5 exit**
- [x] Branch protection rejects direct pushes/red PRs.
- [x] 1h fuzz + 5m soak + release tests run automatically (nightly) with artifacts.
- [x] Snapshot cost flat over 40 minutes; allocations at budget.
- [x] First tagged release with checksums/artifacts and full docs (v0.2.0).

---

## Dependency graph (critical path)

```text
#33 ECS safety ─┐
#24 speed bug ──┤
#25 cast range ─┤
#32 renderer ───┼─> W0 green ─> #26 hash ─> W1 replay v5 ─> #40 catalogue v13
#28 decoders ───┤                                  │
#23 purchases ──┤                                  ▼
#34/#35 CI ─────┘                          #38 modifiers + #41 events v14
                                                     │
                                      ┌──────────────┼──────────────┐
                                      ▼              ▼              ▼
                                   #39 modes       #13 scripts    #10 content updates
                                      │
                                      ▼
                                   #17 balance

#8 golden image ─> #42 HUD ─> #43 hit-test ─> #44 atlas ─> #7/#12/#19
#41 events ─> #18 audio
#40/#20/#39 ─> #10 updates
#28/#27 ─> #15 sanitizer fuzz
#16 Loom, #14 accounts can run in parallel after W1.
```

## Issue coverage map

| Issue | Wave | Depends on / notes |
|---|---|---|
| #7 skinning | W4 ✅ | animated rig landed; GPU joint-weight meshes future |
| #8 golden-image CI | W4 (start W1) | #32; protects #42/#44 |
| #9 hardware/OS renderer | W4 (deferred) | #127; needs Windows/macOS GPU hardware |
| #10 per-client content updates | W4 | #40, #20 |
| #11 manifest signing | W5 | #31 content schema |
| #12 assets/per-map rendering | W4 ✅ | themes landed; real meshes future |
| #13 scripting hooks | W3 | #38 |
| #14 account/key transparency | W5 | parallel after W1 |
| #15 sanitizer fuzzing | W5 | #28, #27 |
| #16 Loom | W5 | parallel after W1 |
| #17 bot balance | W3/W5 | #39 |
| #18 audio | W4/W5 | #41 |
| #19 settings/remapping | W4 ✅ | #113/#114/#115 |
| #20 map selection | W4 | #40, #10/#39 |
| #21 publishing | W5 (deferred) | artifacts ready; needs registry credentials/ownership |
| #22 ttf-parser | W5 ✅ | removed via winit no-title CSD feature |
| **#23 shop replay** | **W0/W1** | replay v5; #27 |
| **#24 haste/slow** | **W0** | regression tests |
| **#25 cast range** | **W0** | behavior/docs/content-check |
| **#26 state hash** | **W1** | blocks trustworthy replay |
| **#27 replay version** | **W1** | same train as #23/#26 |
| **#28 decoder bounds** | **W0** | #15 later |
| **#29 allocation guard** | **W5** | low-risk parts now; modifiers via #38 |
| **#30 snapshot scaling** | **W5** | benchmark first |
| **#31 strict schema** | **W0** | enables #40/#38 |
| **#32 renderer audit** | **W0/W4** | #8/#9 |
| **#33 ECS panic safety** | **W0** | Miri test |
| **#34 CI flakes/name** | **W0** | before #36 |
| **#35 CI gates** | **W0/W5** | golden hash, nightly, bench |
| **#36 branch protection** | **W5** | after #35/#34 |
| **#37 docs truth** | **W0 + each wave** | final close after W5 |
| **#38 modifiers/effects** | **W3** | protocol v15; #41 later |
| **#39 rules/modes** | **W3** | content schema v2 |
| **#40 catalogue parity** | **W2** | protocol v13 |
| **#41 presentation events** | **W3** | protocol v16 after #38 |
| **#42 HUD split** | **W4** | #8 |
| **#43 hit-testing** | **W4** | #42 |
| **#44 textured UI** | **W4** | #8, #42 |
| **#45 extensibility tracking** | map | update as waves land |

## Decisions to make before coding each wave

1. **W1 purchase semantics:** per-slot purchase field (allows move+buy same tick) vs a
   compound `PlayerCommand` variant (changes command encoding and bot code). Recommendation:
   per-slot purchase field in `TickInputs`/replay, because the current one-command-per-slot
   model would otherwise drop a move or a buy.
2. **W0 cast range:** `cast_range = 0` means self-centred for self AoE; point-targeted kinds
   must have a positive range to place at a point. Burst becomes target-centred to match docs,
   or docs are changed to caster-centred; recommendation: target-centred for consistency.
3. **W2 wire hash:** send full `GameContent::hash`; derive wire catalogue hash from it and
   fail the parity test on any omitted field. Recommendation: yes.
4. **W3 protocol batching:** combine #38 status/visual fields and #41 events into one protocol
   bump; do not merge #40 and #38 simultaneously.
5. **W5 branch protection:** require the five checks above; allow admin bypass only for
   documented emergencies.
6. **Release scope:** `0.2.0` was cut after the audit work landed; the next release should wait
   for the #9/#21 decisions rather than reopening closed behavior.

## Per-wave quality gates

Every wave must finish with:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --release --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo run --release -p moba-tools --bin match-soak -- --matches 10 --ticks 12000
```

Plus the wave-specific evidence: replay/content fixtures, validation/sync-validation captures,
fuzz iterations, soak metrics, benchmark deltas and screenshot updates. CI and branch protection
must not be weakened to make a wave pass.

## First execution batch (recommended)

**Batch A (W0, no format bumps):** #33, #24, #25, #28, #32, #34, #35, #31, #37.  
**Batch B (W1):** #26 + #27 + #23 in one replay-v5 train, re-record fixtures.  
**Batch C (W2):** #40 + protocol v13, parity test.  
Then reassess before starting #38/#41, which are the largest changes.

Do not start #38/#39/#41 or other protocol-changing work until Batch A/B gates are green.
