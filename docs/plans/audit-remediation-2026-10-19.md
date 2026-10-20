# Second-audit remediation plan — 2026-10-19

Status: **draft plan, ready for execution**  
Baseline: `main` at `64951a0`, protocol **v26**, replay **v5**, simulation logic **v7**,
**289 tests**.
Sources: the 2026-10-19 re-audit issues **#135–#153**, reopened **#12/#16**, open dependency
upgrade **#49**, and **four private security advisories** (maintainer-visible only).

## How security work is handled

The advisories are private drafts. Their reproduction steps and exploit details must not be
copied into public issues, PRs, commit messages or this plan.

- Work in a **private security fork** associated with the advisory (or a private local
  branch) until the coordinated release. Do not open public PRs that reveal the issue.
- Every advisory gets a private remediation comment/checklist with the affected files,
  tests, reviewer sign-off and release version. The public plan only names the affected
  subsystem.
- Land fixes in one coordinated **security release**, publish the advisories with patched
  versions and credit reporters, then merge any public follow-up cleanup.
- If the advisory workflow cannot use a private fork, the release manager must approve an
  alternative non-public branch and merge strategy before code is pushed.
- Security fixes are not “done” when the code is green: they are done when the patched
  release exists, the advisory is published, and regression tests are in `main`.
- The four advisories split across S-1 (two: session crypto and keyed-peer packet
  acceptance), S-2 (content trust) and S-3 (catalogue update). S-4 is the public unsafe
  job-scope lifetime issue #143/#16, not a private advisory.

## Severity and order

| Priority | Work | Why first |
|---|---|---|
| S0 | Private advisories + **#143/#16** (unsafe scope lifetime) | confidentiality/integrity and UB; block a security release |
| P1 | #135, #136, #137, #138, #140, #141, #142 | correctness, leaks, replay truth, CI trust |
| P2 | #139, #144, #145/#12, #146, #147, #148, #149, #150, #151, #152 | completion and hardening |
| External | #9/#127, #21 | need hardware/credentials, not code alone |

Do not start broad content features (#147/#148) until the hash/validation work (#135/#151)
and the security release train are in place.

## Wave S — security release train

One release train avoids repeated protocol/replay/content bumps. Target release **0.3.0**
(the transport/content changes are not patch-compatible under pre-1.0 semver); the release
manager may choose a different version only with an explicit compatibility rationale.

### S-1. Session transport cryptography
Private advisory: session AEAD nonce/counter handling.

- Replace application/echoed sequence numbers in AEAD nonce inputs with a dedicated,
  strictly increasing per-direction send counter; never seal two packets with the same
  (key, nonce). Rekey or disconnect before counter exhaustion.
- Add a sliding receive replay window for sealed packets and drop duplicates.
- Ensure keyed peers reject unsealed application packets and unsealed reassembled fragments;
  handshake kinds remain the only pre-key exception.
- Seal `Disconnect`; check session ids on teardown; replace sequential session ids with
  random ids.
- Upgrade the RustCrypto stack (**#49**: chacha20poly1305/hkdf/hmac/sha2/ed25519-dalek/
  getrandom) or document why a partial upgrade cannot land; add known-answer and
  no-nonce-reuse tests.
- Bump **PROTOCOL_VERSION** once for this train and document old-client rejection.

**Acceptance:** no duplicate (direction, nonce) across a loopback session with pings,
snapshots, disconnects and catalogue updates; plaintext application packets from keyed peers
are dropped (including fragments); replayed sealed packets are dropped; crypto tests and
real-UDP integration pass.

### S-2. Content trust and deterministic schema coverage
Private advisory: content-pack attestation bypass; **#135**, **#151**.

- Replace the weak 64-bit signing digest with a canonical, length-prefixed digest (SHA-256)
  over an explicit domain-separated format; bump the attestation format version and use
  strict signature verification.
- Apply **one loader** for startup, map rotation and `--reload-content`; verify attestation
  and the pinned publisher consistently, and require a publisher key when attestation is
  mandatory. No unverified pack may reach the simulation.
- Make `content-check --publisher` imply signature verification; add CLI/JSON tests and a
  checked-in signed fixture.
- Fix hex parsing to reject non-ASCII/invalid digits without panicking.
- **#135:** hash `vision_range`; remove or derive the duplicate `*_vision` fields; validate
  ranges/velocities/damage/armour and finite floats; give each unit table its own defaults;
  bound haste; turn the modifier fallback into a validation error; review `ability-pack.toml`
  radii. Add a hash-change test for every field and a partial-table defaults test.
- **#151:** destructure every hashed component/content struct (or add a hash trait/derive),
  add a v5 replay fixture, run parity on fully non-default content, and cap replay decode
  allocation with the counting allocator.

Status (2026-10-19): #139 client-side catalogue validation and duplicate-hash suppression
landed in #180; generation numbers/retry delivery and connected-player rotation landed in #188; explicit ack and advisory regression tests remain open.

Status (2026-10-19): #136 duplicate cast events, per-snapshot capacity reuse, O(1) pending
queue eviction (#169) and high-priority event selection (#175) landed. Event-buffer and transport send-buffer reuse landed; a NetServer allocation guard now asserts zero with events (#223). Fragmented sends still allocate per fragment.

Status (2026-10-19): #135 is complete — rules hash/validation/per-kind defaults, finite
parsing, client/sim vision agreement (#168), explicit modifier validation instead of the
silent fallback, and reviewed example radii (#174).

**Acceptance:** tampered/unsigned/untrusted packs fail at startup, rotation and reload;
two packs differing in a stat never share a signing digest; an unhashed new field fails
compile/test; client and sim agree on vision; replay/content-check fixtures pass.

### S-3. Catalogue-update delivery and rotation
Private advisory: client accepts replayed/out-of-phase catalogue updates; **#139**.

- Add a monotonically increasing content generation; client accepts only a newer generation,
  only in Lobby/Results, and validates the decoded catalogue before swapping.
- Deliver updates reliably: ack the content hash and retransmit with backoff, or repeat in
  snapshots until acked. Bound the queue and generation values.
- Rotate at rematch with connected clients (server policy/rotation or lobby vote), keep the
  Results scoreboard until the next match, and broadcast the new catalogue to every client.
- Add `decode_content_update` to the deterministic fuzzer and cargo-fuzz `wire` target.
- Fix the stale `watch_content` docs.
- Tests: under netsim 5% loss, a connected client adopts a new pack and plays the next match;
  two maps rotate in sequence; replay verifies with each pack; replayed/mid-match updates
  are ignored.

**Acceptance:** a connected client plays two different maps in sequence under loss and both
replays verify; a stale or mid-match update cannot change the catalogue.

### S-4. Job-scope lifetime and real Loom model
**#143/#16**, follow-up to the closed but not delivered #16.

- [x] Make `ScopeShared` owned per job (`Arc<ScopeData>`, as `std::thread::scope` does); no
  raw scope pointer remains in `JobRef`, `PendingGuard` or `Scope`.
- [ ] Under `cfg(loom)`, drive the real `JobSystem::scope`/`pool.rs` with loom atomics, mutex
  and condvar; remove `wait_timeout` so lost wake-ups can fail the model; add an injected
  decrement-before-body bug and show the model fails.
- Simplify `parallel_for` with `chunks_mut()` and remove unsafe blocks.
- Correct M1 status (do not call it green until the model drives the real code).

**Acceptance:** Loom drives `pool.rs`, the injected bug fails, and no reference to the
scope counter can outlive the scope.

Status (2026-10-19): **#143 closed** — Arc ownership (#161), blocking wait (#179), real-type Loom (#182) and early-decrement regression (#183) all landed.

### S-5. Fog-of-war/visibility leaks
**#138** (security-adjacent information leak).

- Recompute team on server and client after reload/rotation/reconnect; reject rotations that
  change `team_size` with occupied slots or disconnect affected slots.
- Redact hidden target/killer position/net id in own-team hit/death events; reject attack
  orders on targets not currently visible to the attacker.
- Add reload-team-size, fogged-skillshot and hidden-net-id attack tests.

**Acceptance:** no snapshot/event carries hidden-enemy data; all three regression tests pass.

### S-6. CI trust gate for the security release
**#142** is a prerequisite, not an afterthought.

- [x] Fix `nightly.yml` shell/pipefail; add failure notification (dispatch 39235240360).
- [x] Make allocation tests per-thread/serialized; 20 consecutive local release runs passed.
- [x] Add `release tests + doctests`, `MSRV (1.85)` and `renderer-gate` to the required
  ruleset; docs updated.
- [x] Run `renderer-gate` on every PR with a no-op path filter; run macOS MoltenVK
  automatically for renderer paths.
- [x] Raise net test client timeouts to ≥3 s; ignore the toolchain pin in Dependabot; remove
  Windows-probe smoke suppression.

**Acceptance:** one green scheduled nightly with artifacts and a deliberately broken step
turns red; 20 consecutive green release-allocation runs; a renderer PR cannot merge without
golden; required-check list matches docs.

Status (2026-10-19): #156/#162/#164/#166/#178 implemented; the net snapshot allocation guard
is now per-thread and passed 20 consecutive release runs. Failure notification was exercised by
the pre-fix shader/ALSA/criterion failures and created #160 (closed). 20 allocation runs passed;
the ruleset has release/MSRV/renderer-gate. Short post-fix nightly **39239367089 is green**;
#142 is closed: full nightly **39295169028 is green** with attached logs.

## Wave C — correctness and replay truth (P1)

### C-1. #136 duplicate cast events and event-path allocations
- Remove the duplicate `AbilityCast` push; test exactly one event per cast.
- Reuse `delta.events`; use a ring/VecDeque for pending events; prioritise deaths/objectives
  within the event cap.
- Add a NetServer snapshot allocation guard with events present; measure teamfight bandwidth.

### C-2. #137 replay divergence after disconnect
- Remove `bot_mask` from `state_hash` (preferred; replay inputs are resolved after bots) or
  record mask changes per frame with a replay layout change.
- Add a real-UDP disconnect+reconnect match and verify every checkpoint; bump
  replay/SIM_LOGIC if the hash/layout changes.

### C-3. #140 bot-vision cost and guard gaps
- Compute both team masks in one pass over vision sources (or reuse server observation), not
  two full `observe()` passes.
- Size masks from `MAX_UNITS`; fix out-of-bounds/bit-leak.
- Give the teamfight guard a real teamfight; add ability-pack (zones/on-hit/area), bot-shop
  and wave guards; remove per-purchase/lane `collect()` allocations.
- Use the grid in `gather_enemies`; skip dead units; trace match-soak tail spikes and refresh
  AGENTS/perf figures.

### C-4. #141 client snapshot/minimap/UI-scale bugs
- Sequence-check before cloning; recycle the snapshot vector; add a steady network-frame
  allocation guard.
- Use map extent in `minimap_world`; test extents 128/200/256.
- Implement or remove `HudAction::Ability`; reject non-finite UI scale.

### C-5. #135/#151 integration checks
- Keep the content-hash/validation and hash-coverage work from S-2 as the single source of
  truth; no separate ad-hoc fix.

Status (2026-10-19): #140 single-pass bot fog masks landed in #173; `tick_full_5v5`
measured 5.566 ms, −14.3% vs the prior baseline. Ability-pack allocation guard added in #176.
**#140 closed** — single-pass masks (#173), MAX_UNITS bounds (#190), ability/bot-shop guards (#176/#191), grid-based dead-skipping gathering (#189/#194) and match-soak slow-tick indexing (#195) landed; current benchmarks documented.

Status (2026-10-19): #137 removes `bot_mask` from the state hash, bumps `SIM_LOGIC_VERSION`
to 7, updates the golden hash and adds a mask-independence test (#172). A real-UDP
disconnect/reconnect checkpoint test remains open.

Status (2026-10-19): #138 hidden event endpoints (including own-team events), unseen attack
orders, stale teams after reload and reload/occupied-slot rejection landed in #171/#177. Slot/team mapping is now covered for every mode size; a reload
team-size integration test remains open.

Status (2026-10-19): client snapshot sequencing (clone only on a new seq), map-extent minimap
clicks, removal of the unwired ability hit region and UI-scale finiteness landed in #170.
Snapshot sequence gating and a zero-allocation unchanged-frame guard are covered (#199/#224).

## Wave D — completion and hardening (P2)

### D-1. #144 renderer hardening
Status (2026-10-19): inverse-transpose normals, UI `textureLod`, DeviceLost/InvalidTexture, checked texture size, atlas round-trip, D24S8 layout and capture extent/format snapshot and present barrier, suppression removal and allocation-error cleanup landed in #210–#216; **#144 closed** — capture/render/leak/resize/barrier/normal/atlas/error-client changes landed in #210–#220; the client test allocator is a documented test exception. Capture extent/format snapshot across swapchain rebuild; D24S8 layout; RAII/error-path
leaks; replace golden suppression with a correct barrier (or narrowly scope it); inverse-
transpose normals; `textureLod`; 0×0 resize skip; `DeviceLost`; checked texture size; atlas
round-trip test; queue-family ignored; client unsafe policy exception/test-support crate.
Acceptance: golden with no suppression; forced rebuild/injected failure tests where practical.

### D-2. #145/#12 networked animation and assets
Use the hero rig in networked matches with replicated facing/gait/idle; add joint/weight
layout + bone-matrix storage buffer with sync2 barriers (after #12 asset schema); measure
10-hero + minion frame cost; capture a networked animated match. #12 must define a validated
asset manifest, missing-asset fallback and per-map visuals before the GPU-skinning path.

### D-3. #146 determinism hygiene
Entity tie-breaks in every targeting path; one rounding mode for `Fix::mul/div`; side-swap
mirror test; team-size-agnostic bot RNG; distinct-outcome match-soak reporting and ≥5k sweep;
swept projectile segment-circle hits before removal, skip dead; bump SIM_LOGIC (batch with
#137/#135 hash train).

### D-4. #147 generic modifiers
`ModifierDef` with stat mods, status flags and `on_tick`/`on_damaged`/`on_expire` effects;
drop the all-kinds-present rule; generic replicated status flags; stun/armour-shred data
packs; zero-allocation guard; content/protocol/SIM_LOGIC bump.

### D-5. #148 economy and item actives/passives
Starting/passive gold, radii, shared-XP, unit rewards, win conditions and the item-active schema and `UseItem` behavior/cooldowns landed (#244–#249); cooldown replication/HUD, Ogre Heart active, bot use, Ironblade on-hit and Chain Vest on-damaged hooks landed (#250–#254); aggro rules remain.

### D-6. #149 NetServer extension contract
Status (2026-10-19): every ServerPhase value is delivered to systems (#201); **#149 closed** — every phase delivered, replay contract documented, bot-policy factory + id registry survive rematches, CI embedding example and 256 cap documented. Systems: either registered/replayed deterministically or explicitly invalidate/mark replays;
deliver all `ServerPhase` values or trim; add `with_bot_policy` surviving rematches and an
id registry; CI-built embedding example; `WORLD_EXTENT` removed and the 256 map cap documented.

### D-7. #150 HUD/audio completion
Split `shop`/`scoreboard`/`abilities` out of `build_network_hud`; move colors to `theme`;
keep `moba_net` inside `hud/model.rs`; network HUD allocation/fixture tests and input-router
test; audio local-sim events, I16/U16 conversion, device-loss reopen, mute key, fixed voice
pool/lock-free queue. Acceptance: `app.rs` <800 lines, HUD tests exist, offline sound and
device recovery work.

### D-8. #151 remaining hash/replay enforcement

Status: checked-in v7 fixture verifies in tests; checked-in fixture and fully non-default parity are covered; **#151 closed** — decode memory cap (#225) and exhaustive component-field destructuring guard (#243).
If not fully landed in S-2: compile-time hash coverage, checked-in replay fixture, fully
non-default parity pack, decode amplification cap/allocation assertion.

### D-9. #152 docs truth pass 2
Link every status ✅ to evidence; correct nightly/validation/Loom/claims; content-authoring
composition/kind/rule claims; building-on-moba v24/replay-system claims; SECURITY attestation
status; perf allocation wording; AGENTS tick figure; fuzz job name/corpus (add replay/#28
seeds). Must follow the final implementation, not this plan.

## Release and compatibility plan

1. **Protocol:** one bump to **v26** for S-1/S-3 (and any event fields that require it).
   Reject old clients explicitly; document migration in `docs/tasks/0005-networking.md`.
2. **Replay/logic:** batch hash-affecting fixes (#135 content hash, #137 bot mask, #146
   determinism/projectiles) into **replay v6 / SIM_LOGIC v7**, re-record fixtures and
   regenerate the golden state hash; document the intentional divergence.
3. **Content:** attestation format v2; content-hash changes for rules/vision fields; signed
   fixture; all example packs revalidated/re-signed; replay compatibility notes.
4. **Golden:** renderer changes (#144/#145) may require a deliberate fingerprint rebaseline;
   rebaseline only from CI's 1280×720 capture and link the run.
5. **Release:** cut **v0.3.0** only after S0–S2 and the CI gate are green; publish the four
   advisories as patched in `>=0.3.0`, then close the private records. Publish platform
   artifacts with the existing release workflow. #21 registry publishing remains external.
6. **No public disclosure** of advisory details before the coordinated release. The release
   notes describe fixed classes and patched versions without exploit recipes.

## Work batches

| Batch | Contents | Exit |
|---|---|---|
| 0 (now) | #142 CI trust, private advisory forks, RustCrypto upgrade spike (#49), #143 design | gates reproducible; private branch exists |
| 1 | S-1 transport, S-4 jobs, S-5 visibility | protocol v26 draft; Loom real; leak tests |
| 2 | S-2 content trust + #135/#151, S-3 update delivery + #139 | signed fixture; loss/rotation tests |
| 3 | C-1..C-4 (#136/#137/#140/#141) | P1 acceptance; replay v6/logic v7 |
| 4 | D-1..D-9 | P2 acceptance; docs truth pass |
| 5 | Release v0.3.0 + advisories | patched release published; advisories closed |

Each batch lands as small PRs (security fixes in private branches) and must not weaken required
checks. Update `docs/status.md`, task logs and `CHANGELOG.md` as each accepts.

## Coverage table

| Item | Wave | Primary work | Depends |
|---|---|---|---|
| Private advisory S-1 | S | transport AEAD counter/anti-replay/encrypted-only | #49, protocol v26 |
| Private advisory S-2 | S | strong attestation digest + unified loader | #135, #151, content v2 |
| Private advisory S-3 | S | reliable generation/phase-checked updates | #139, protocol v26 |
| #143/#16 job-scope lifetime | S | scope lifetime + real Loom | #16 |
| PR #49 RustCrypto upgrade | S | crypto dependency refresh | advisory S-1 |
| #153 tracking | S | this plan, issue closure/verification | all |
| #135 rules stats/hash | S | validation, per-table defaults, vision hash | #151 |
| #136 duplicate events/alloc | C | one cast event; reuse event buffers | — |
| #137 replay disconnect | C | bot_mask hash/replay frame | replay v6 |
| #138 fog leaks | S | team recompute, redaction, attack visibility | — |
| #139 reliable ContentUpdate/rotation | S | ack/generation/rotation/tests | protocol v26 |
| #140 bot vision/guards | C | single-pass masks, guards, grid gather | — |
| #141 client snapshot/minimap/scale | C | seq check/recycle, extent, ability click/scale | — |
| #142 CI trust | S | nightly, required checks, renderer gates, timeouts | — |
| #143 jobs lifetime | S | Arc scope shared/real Loom | #16 |
| #144 renderer hardening | D | capture/layout/leaks/barrier/shader/policy | golden |
| #145 GPU skinning/network rig | D | networked rig, joints/weights, asset schema | #12 |
| #12 asset pipeline (reopened) | D | validated asset manifest/fallback/per-map visuals | #145 |
| #146 determinism hygiene | D | tie-breaks/rounding/side-swap/projectiles | replay v6/logic v7 |
| #147 generic ModifierDef | D | stat mods/flags/triggers/data packs | #135/#151 |
| #148 economy/items | D | rules economy, UseItem/cooldowns/hooks | #147/#151 |
| #149 NetServer extension | D | systems/replay contract, phases, bot policy, example | #137 |
| #150 HUD/audio | D | module split/tests/audio robustness | — |
| #151 hash/replay enforcement | S/D | compile-time coverage, fixture, parity, decode cap | #135 |
| #152 docs truth pass 2 | D | final docs after code | all |
| #9/#127 platform GPU | External | hardware/revised acceptance | external |
| #21 crates.io | External | registry ownership/token | external |

## Risks and controls

- **Disclosure risk:** private advisory work only; no details in public plan/issues; release
  notes reviewed by the security lead.
- **Format churn:** one protocol v26 and one replay v6/logic v7 train; no mixing with P2
  feature bumps. Use the plan's batching table.
- **Golden/replay invalidation:** rebaseline from CI artifacts, not local windows; link every
  rebaseline commit/run; re-record fixtures in the same PR as the logic change.
- **RustCrypto migration:** API/behavior changes in #49; land behind tests and don't combine
  with unrelated feature work.
- **Hidden dependencies:** #135/#151 must precede #147/#148 and attestation v2; #12 must
  precede #145 GPU skinning; #142 must gate all security merges.
- **Capacity:** security review needs a second maintainer; if unavailable, do not publish
  fixes as “verified”—leave advisories private and report the blocker.
- **External blockers:** #9/#127 and #21 remain; do not claim them as part of this audit.

## Exit criteria

- Four advisories fixed, privately reviewed, patched release published, advisories published
  and closed; regression tests in `main`.
- All P1 acceptance criteria and P2 implementation issues accepted/closed.
- Protocol v26, replay v6/SIM_LOGIC v7 (if applicable), content/attestation v2 documented;
  old clients/replays rejected clearly.
- Required checks: gates ×3, tools, deny, miri, release/doctests, MSRV, docs, renderer-gate;
  one scheduled nightly green with artifacts; 20 consecutive release-allocation runs.
- Golden render/state hash rebaselined deliberately with linked CI runs; all shipped packs
  validate and signed fixtures verify.
- `docs/status.md`, task logs, AGENTS/agent runbook, SECURITY, content-authoring, perf and
  CHANGELOG match the implementation; `scripts/check-docs.sh` and `scripts/agent-verify.sh
  --full` pass.
