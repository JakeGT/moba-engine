# Handoff plan: finish the engine

Written 2026-10-20 and completed for handoff 2026-10-21. Owner of this plan: the orchestrating agent, which also **reviews every PR** produced from it (see [§3 Review protocol](#3-review-protocol)).

This document is meant to be enough for an agent with no prior context to pick up any work package (WP) safely. Read §1–§3 before touching code.

---

## 1. Rules for the implementing agent

The repo-wide rules in [AGENTS.md](../../AGENTS.md) and the runbook [docs/agents.md](../agents.md) apply. In addition:

1. **One WP = one branch = one PR.**
   - Branch from fresh `main`: `git switch main && git fetch origin && git reset --hard origin/main`.
   - Branch names: `feat/<issue>-<slug>`, and likewise `fix/`, `docs/`, `perf/`, `ci/`.
   - Claim the issue with a comment before starting.
2. **You may not merge.** Merging needs the maintainer, and agent tooling in this environment blocks `gh pr merge` and ruleset edits anyway.
   - Finish a WP by getting CI green and running the reviewers it lists.
   - Then add the label **`ready-for-review`** and post the handoff comment (§3.2). Stop there.
3. **Stacked PRs and squash merges.** `main` is squash-merge only, with linear history.
   - If your branch sits on top of another unmerged PR, the base PR's commits are rewritten when it squash-merges. After that, rebase only your own commits: `git rebase --onto origin/main <old-base-tip> <your-branch>`, then force-push with `--force-with-lease`.
   - Prefer branching from `main` and waiting over stacking.
4. **Contract numbers are shared, so serialise them.**
   - Before bumping `PROTOCOL_VERSION` (`crates/net/src/protocol.rs`), `SIM_LOGIC_VERSION` (`crates/sim/src/replay.rs`) or the replay `VERSION`, ask the maintainer which number to use. Write the number you were given in the PR.
   - Never reuse a number another open PR has claimed.
   - WPs that bump the protocol are marked **[proto]**, and those that bump sim logic **[logic]**. At most one of each kind may be open at a time.
5. **Security.**
   - If you find a vulnerability (auth bypass, crypto misuse, information leak across fog of war, remote crash), **do not** open a public issue or describe it in a public PR, commit message or doc. Report it to the maintainer, who files a private GitHub Security Advisory (SECURITY.md).
   - Fixes for advisories are developed in the advisory's **temporary private fork** (`gh api -X POST repos/JakeGT/moba-engine/security-advisories/<GHSA>/forks`).
   - The old `JakeGT/moba-engine-security` workspace is **archived and read-only**.
6. **Evidence.**
   - Quote only output you saw, and name the hardware or runner.
   - Never mark an acceptance box without linked evidence.
   - Hosted runners are regression gates, not performance evidence.
7. **Gates.**
   - `scripts/check-docs.sh` and `scripts/agent-verify.sh` on every PR.
   - `scripts/agent-verify.sh --full` for anything touching contracts, net, sim, render or performance.
   - The PR body follows `.github/PULL_REQUEST_TEMPLATE.md` (summary / area / contract impact / verification / evidence / notes) and is passed with `--body-file`.
8. **Test counts and versions** appear in README.md, AGENTS.md, CLAUDE.md, docs/status.md and docs/agents.md, and `check-docs.sh` enforces that they agree. Update all five when a count or version changes.
9. **Don't touch** another agent's worktree, the shared git stash, or the maintainer's main checkout. Work in `.claude/worktrees/<name>` (gitignored).

## 2. State at handoff (2026-10-21)

**Code state:**
- `main` is at `8ddc64a` (**v0.4.0** security release, #295). Workspace 0.4.0, protocol **v37**, replay v5, sim logic v7. Rust MSRV 1.88, Vulkan 1.3 minimum (ADR-0002). The **next free protocol number is v38**, but still ask the maintainer (rule 1.4).
- PR **#293** (this plan's branch, `feat/multi-match-host`, rebased onto v0.4.0): WP-1.1, multi-match hosting. It adds `MatchHost`, `moba-server --matches`, `load-test`, `ServerStats::snapshot_send_errors` and `ClientStats` datagram counters, and takes the test count to **322**.
- PR **#295**, the 0.4.0 security release, is **merged** and tagged `v0.4.0`.

**Maintainer decisions (2026-10-21):**
- **Mirror:** the side-swap mirror may be *asymmetric*. Exact symmetry is not required, so WP-2.2 is descoped (see there).
- **#9 / #127:** the maintainer will do the real Windows/macOS GPU run by hand.
- **#21:** crates.io publication will be done by hand by the maintainer.
- **Security:** security fixes are merged and advisories published. WP-6.4 is done.
- **Playtest:** the M6 playtest (WP-5.2) waits until phases 3–4 are complete.

**Milestone exit criteria still open:**

| Milestone | Open criterion | WP |
|---|---|---|
| M2 Platform | real Windows/macOS GPU runtime (#9) | 6.1 (external) |
| M3 Renderer | Windows/macOS frame budget (#127); skinned meshes | 4.x, 6.1 |
| M6 Vertical slice | recorded 10-client real-network playtest | 5.x |
| M7 Hardening | 50 concurrent matches (**done in #293**); independent external review | 6.3 |
| M8 Content | packs reference real assets | 3.x |

## 3. Review protocol

The orchestrating agent reviews every PR before the maintainer merges it. The ruleset requires all review threads to be resolved before merge, so unresolved review comments block the merge.

### 3.1 What the implementer does before asking for review
1. All required checks are green:
   - `gates (ubuntu/windows/macos)`
   - `tools + deterministic soak`
   - `deny`
   - `miri`
   - `release tests + doctests`
   - `MSRV`
   - `renderer-gate`
2. The specialist reviewers listed in the WP have run (`.claude/agents/*-reviewer.md`). Every CONFIRMED finding is fixed or explicitly argued in the PR.
3. Add the label `ready-for-review`.

### 3.2 Handoff comment (post on the PR, then stop)
```
Handoff: WP-<id> (#<issue>)
1. Branch/PR: <branch> / #<pr>; merged: no
2. Changed: <bullets>; verification: <commands + key output lines>
3. Open boxes and why: <list>
4. External requirement: <none | hardware | credential | maintainer decision>
5. Next safe command: <e.g. "rebase onto main after #NNN merges">
Contract impact: protocol vNN→vMM (number assigned by maintainer) | logic … | none
Reviewer runs: <reviewer>: <n CONFIRMED fixed, n PLAUSIBLE noted>
```

### 3.3 What the orchestrator checks (expect these questions)
- **Correctness:** does the code do what the WP says on every path, including error paths and the paths the WP calls out as pitfalls?
- **Contracts:**
  - Every hashed field must round-trip on the wire (see #294).
  - Any version bump must match the number the maintainer assigned.
  - Replay fixture and golden hash must be regenerated only when the logic version bumps, and the PR must say why.
- **Determinism:**
  - No `HashMap`/`HashSet`, wall clock or float in `sim`/`game`.
  - Iteration order must be explicit.
  - Tie-breaks must follow the rule in force (AGENTS.md invariant 3, or the WP-2.2 replacement).
- **Hot paths:** no per-tick or per-frame heap allocation (allocation guards: `crates/sim/tests/allocations.rs`, `crates/net/tests/snapshot_allocations.rs`, `crates/client/src/app.rs` `allocation_probe`).
- **Unsafe / GPU:** one unsafe op per `// SAFETY:` block, sync2 barriers covering every hazard, validation and sync validation clean (lavapipe golden).
- **Tests:** a regression test for every bug fixed, and it must fail without the fix. Say in the PR how you checked that.
- **Evidence:** the numbers in `docs/perf/` must match the raw output pasted with them.
- **Docs:** the contract table, task docs and status updated; no stale claims.

Outcome: the orchestrator either approves in a PR comment (the maintainer may then merge), or leaves review threads and the label `review-changes-requested`. Fix, re-run the gates, resolve threads, and re-apply `ready-for-review`.

---

## 4. Work packages

Order: **Phase 0** first (bugs and stale docs), then phases 1–5. Within a phase, WPs without a dependency can run in parallel, apart from the [proto]/[logic] serialisation (rule 1.4).

### Phase 0: correctness and stale-doc fixes

#### WP-0.1 [proto] Rules fields missing from the wire (#294, P1): **DONE in PR #297**
Protocol **v38**. Evidence: `wire_rules_parity_covers_every_hashed_field` (one mutation per hashed rules field, including every reward scalar) and `economy_override_pack_reaches_a_real_udp_client` (real UDP join against `content/economy-override.toml`). Exhaustive-destructure guards in `feed_catalogue` and `From<&GameContent> for WireCatalog`. `scripts/agent-verify.sh --full` passed; golden state hash unchanged.

- **Problem:** about 10 `RulesSpec` fields are hashed but not sent; `to_content` substitutes defaults. So a pack overriding them can't be joined: the client drops the Welcome on hash mismatch.
  - Fields: `starting_gold`, `gold_per_interval`, `gold_tick_interval`, `xp_radius`, `shop_radius`, `shared_xp_percent`, `rewards`, `structure_heroes_first`, `nexus_win`, `score_limit`.
  - Code: `crates/game/src/content/hash.rs` `feed_catalogue` rules destructure (feed; was `content.rs` ~1525–1547 before the module split); `crates/net/src/protocol.rs` ~1495–1504 (defaults); `client.rs` ~525 (drop).
  - Reproduced on `e31fee4` with a probe that sets `starting_gold += 100`.
- **Do:**
  1. Add the fields to `WireRules`, covering encode/decode/`From<&GameContent>`/`to_content`, with the same bounds `validate()` enforces.
  2. Bump the protocol (rule 1.4).
  3. Add a parity test that mutates every hashed `RulesSpec` field one at a time and asserts `WireCatalog::from(&c).content_hash() == c.hash()` and `to_content().rules == c.rules`.
  4. Add a structural guard so a future hashed field can't be forgotten, e.g. destructure `RulesSpec` exhaustively (`let RulesSpec { a, b, .. } = …` without `..`) in both the feed and the wire conversion.
  5. Add an economy-override pack under `content/`, plus a real-UDP join test against it.
  6. Update the wire fuzzers (`fuzz/`, `crates/tools/src/bin/fuzz-wire.rs`).
- **Accept:** see the #294 checkboxes.
- **Reviewers:** netcode-reviewer.
- **Pitfall:** `MAX_DATAGRAM`/Welcome size. Check that the Welcome still fragments correctly, since it already fragments.

#### WP-0.2 Stale docs (no code)
- README "Server hardening" says rate limiting happens "before decoding". The code limits **after** `recv` → header decode → AEAD open → reassembly (`crates/net/src/server.rs` `poll`, `transport.rs` `recv`). Correct the wording, and note that WP-1.2 changes this.
- `.claude/agents/gpu-safety-reviewer.md` still says "Vulkan 1.2 + descriptor indexing". Change it to Vulkan 1.3 core with dynamic rendering and sync2 (ADR-0002).
- `docs/dev/software-vulkan.md` ~84 says the golden workflow sets `MOBA_IGNORE_KNOWN_PRESENT_HAZARD=1`. No workflow or code reads it (grep on 2026-10-21). Remove the claim, or re-validate the hazard under lavapipe and document the result.
- `docs/perf/0008-balance.md` still says `slot % 5` and SIM_LOGIC v4. Fix these as part of WP-2.3 if that happens first.
- **Accept:** `scripts/check-docs.sh` passes, and every corrected claim cites the code it was checked against.

### Phase 1: multi-match hosting (M7)

#### WP-1.1 Multi-match host and load test (#291): **DONE in PR #293**
Evidence is in [docs/perf/0010-multi-match.md](../perf/0010-multi-match.md): 50 matches × 10 clients for 5 minutes, per-match tick p99 0.15 ms, 0 dropped frames, 500/500 clients connected, 8.4 / 2.3 KB/s per client.

The tightest margin is upstream under 5 % loss: 3.6 of the 4 KB/s budget.

#### WP-1.2 Host-wide budget, poll fairness, shared reload (#292, P2)
- **Facts:**
  - `RateLimiter` (`server.rs` ~159–247) is per `NetServer` and runs **after** `transport.recv()`.
  - `UdpTransport::recv` keeps draining undecodable datagrams for up to a 2 ms deadline per call, and `poll` calls it up to 512 times. So a flood on one port can stall every match on that worker (`host.rs` `run_worker`).
  - `watch_content`/`poll_content_reload` run separately in every match.
- **Do:**
  1. Count datagrams that fail to decode against the per-address limiter, by moving the cheap address check before decode in `transport.rs` `handle_datagram`.
  2. Cap each match's poll work per worker frame (for example, at most M datagrams or T µs), so one port can't starve its neighbours.
  3. Add an optional host-wide pps budget in `MatchHost`, shared by all matches on all workers. The server needs a `RateLimiter` handle; an `Arc<AtomicU32>` token bucket is fine.
  4. Load content once in the host and share the `Arc<GameContent>`. Document atomic replace (write a temp file, then `rename`).
  5. Either document `MatchHost::bind` as a test/tool helper, or give it a per-match configure callback.
  6. Add a load-test flood mode, e.g. `--flood-port K --flood-pps N` sending junk to one match. It must pass while the other matches keep tick p99 ≤ 2 ms and 0 dropped frames.
- **Accept:** the flood test passes, the README is corrected, and #292's boxes are ticked.
- **Reviewers:** netcode-reviewer, perf-reviewer.
- **Pitfall:** don't let the cap delay handshakes indefinitely. Keep handshakes fair, for example with round-robin across ports.

### Phase 2: determinism mirror and balance (#290)
**Background (verified 2026-10-21):** an exact side-swap mirror is impossible today.
- **Waves use point reflection, heroes use x-mirror.** `hero_spawn` is an x-mirror (red negates `dx`, `map.rs` ~100–107), but `spawn_wave` offsets formations with `direction.perpendicular()` (`sim.rs` ~1205–1217), which is a point reflection.
- **Tie-breaks use raw entity ids.** `candidate.entity < entity` (`sim.rs` ~1845, 1852, 1911, 1917, 2120, 2850), and ids interleave after the first deaths (LIFO free list, `crates/ecs/src/entity.rs` ~93).
- **ECS iteration isn't stable within a team.** It's archetype/row order with `swap_remove` and archetype moves (`world.rs` ~140).
- **Command and attack order flips the whole list.** `apply_inputs` and `resolve_attacks` reverse all ten slots on alternate ticks (~1474–1485, ~2025–2031).
- **Deaths and win checks favour blue.** Deaths are processed in ECS order (~2150), and the score and nexus checks look at blue first (~2193–2235).
- **Bot RNG differs per slot.** Seeds are `seed ^ (… | idx*φ)` (~479–483), and red's x-jitter isn't negated (~2744, 2756).
- **`Fix` rounding isn't odd-symmetric.** `mul` uses `>>` and `div` floors (`crates/core/src/fix.rs` ~308, 334–339), so floor(−x) ≠ −floor(x).

#### WP-2.1 Tolerance mirror test (no sim change)
- **Test:** add `crates/sim/tests/mirror.rs`.
  - Two simulations on the default map with waves disabled (`SimConfig { spawn_waves: false, .. }`, which is what `match-soak --no-waves` sets).
  - Run B uses `seed ^ 1`, so the scheduling phase is flipped.
  - Both runs use a **scripted** `BotPolicy` (`set_bot_policy`) that gives slot *i* in A and slot *i±5* in B x-mirrored commands (x → extent − x).
  - Every tick, compare by slot:
    - hero positions within ±4 ULP;
    - health and mana exactly;
    - kill and damage events as per-tick multisets keyed by slot.
  - Run N = 3,000 ticks.
- **Accept:** the test passes in CI. Record in the PR any field that needed a tolerance, and why.
- **Reviewers:** perf-reviewer, since `sim/` is touched by tests only. Also the orchestrator's determinism checklist.
- **Pitfall:** `Attack(Entity)` commands need an entity map between the runs. Build it from the hero slot → entity tables (`hero_entity`).

#### WP-2.2 [logic] Side-fairness fixes (descoped, conditional)
**Maintainer decision (2026-10-21): the mirror may be asymmetric.** Don't pursue exact symmetry:
- (a), (b), (c), (e) and the (f) rounding change are **out of scope**.
- Keep floor rounding (Option A) and the WP-2.1 tolerance mirror.

Do this WP **only if WP-2.3 shows a side bias** (a CI excluding 50 %). In that case fix only the items that can bias outcomes, in one logic bump:
- (d) same-tick double nexus or score-limit wins resolve as a draw;
- `MAX_UNITS` wave starvation of the team that spawns second;
- if needed, the command/attack processing order.

Re-run WP-2.3 afterwards.

The original full-symmetry list is kept below for reference only:
- (a) Flip red's formation perpendicular in `spawn_wave`, so waves are x-mirrored like heroes.
- (b) Add a hashed per-team `SpawnSerial` component (assigned at spawn, monotonically increasing per team) and use `(team-relative serial)` as the tie-break key in place of the raw entity id.
  - Update AGENTS.md invariant 3.
  - Add the component to `hash_state` and to the `every_registered_component_is_hashed` test.
- (c) Process blue and red as groups in serial order in `apply_inputs`, `resolve_attacks`, `resolve_deaths`, projectiles and zones. Alternate which team goes first on the existing `blue_first` phase.
- (d) Same-tick win resolution: if both nexuses (or both score limits) trigger in one tick, declare a draw rather than favouring blue.
- (e) Bots: derive red slot *i+5*'s RNG stream from the same seed as blue slot *i* and negate the x-jitter. Otherwise the mirror test keeps using scripted policies.
- (f) Rounding: **decided — keep floor rounding (Option A)**.
  - Option A: keep floor rounding (#266) and keep ULP tolerance on positions.
  - Option B: change `mul`/`div` to truncate toward zero, which is odd-symmetric. That reverses #266's choice, touches every gameplay path, and needs a broad re-baseline.
  - Default to **A** unless told otherwise.

Then:
- Extend `mirror.rs` to waves on, 6,000+ ticks, exact on everything except positions, which follow the (f) decision.
- Bump `SIM_LOGIC_VERSION` (rule 1.4).
- Regenerate `crates/sim/tests/fixtures/seed-v<N>.mbr` with `cargo run -p moba-sim --example generate_replay_fixture` and update `replay_fixture.rs`.
- Regenerate `tests/golden-state-hash.txt` with the `state-hash` tool.
- Update the contract table and CHANGELOG.

- **Accept:** the mirror test is exact (apart from (f)) with waves on, the replay fixture and golden hash are regenerated with the reason given, and the allocation guards pass.
- **Reviewers:** perf-reviewer, netcode-reviewer (snapshot determinism), orchestrator.
- **Pitfall:** `MAX_UNITS` cap starvation. Waves spawn blue first (`sim.rs` ~1133), so the cap starves red. Spawn interleaved by serial, or reserve capacity per team.

#### WP-2.3 Outcome-resolving balance sweep
- **Fix `match-soak`** (`crates/tools/src/bin/match-soak.rs`): `unique_hashes` uses `state_hash()`, which mixes in the seed, so it proves nothing. Use `state_hash_normalized()`.
- **Sweep:** 3,000-tick matches never resolve. Waves come at ticks 30/930/1,830/2,730, minions walk at 4 u/s, towers have 2,200 HP and the nexus 4,800 HP. Run with `--ticks 60000 --no-verify`, record resolution ticks, count unresolved matches as draws, and report blue/red with a **Wilson 95 % CI** over decided matches.
  - Roughly 1,000 s for 5,000 matches at ~10 µs/tick. Parallelising the soak across threads is allowed and welcome.
- **Write-up:** update `docs/perf/0008-balance.md`, including the stale `slot % 5`/v4 text. Run it again after WP-2.2.
- **Accept:** the CI excludes 50 %, or the bias is explained and an issue filed.

### Phase 3: assets (M8 remainder, epic #276)
**Key constraint:** network clients never see pack files, only `WireCatalog`. Visuals are therefore **names** resolved against a **client-local asset root**, with procedural fallback. Paths never go on the wire, and differing local assets are cosmetic only. Document that.

`moba-assets` (`crates/assets/src/lib.rs`) has `AssetManifest { units, maps }`, `AssetEntry { name, path, fallback }`, `validate()`, `resolve()` and path rules. **No other crate uses it yet.**

#### WP-3.1 [proto] Packs reference assets; content-check validates them (#285)
- **Schema:**
  - Optional `[assets]` in `ContentFile` (`crates/game/src/content/schema.rs`, `deny_unknown_fields`), with the same shape as `AssetManifest`.
  - `HeroEntry.model: Option<String>` (≤ 16 ASCII), stored in `HeroSpec`.
  - `MapEntry.visuals: Option<String>`, defaulting to `theme.asset_name()`.
  - `moba-game` depends on `moba-assets`.
- **Validation:**
  - Every `model`/`visuals` must name a manifest entry.
  - `validate_path` must also reject `\` and `:` (drive letters).
  - content-check checks the files exist under the pack directory and are `.glb`, and reports `"assets":N` plus errors in text and `--json`.
- **Hash and wire:**
  - Append a tagged visuals block to `feed_catalogue` **only when non-empty**, so existing packs, default content, replays and signatures stay byte-identical. Prove it with a test.
  - Add `WireHero.model`, round-tripped in `to_content`, and bump the protocol.
  - Add the parity test (as in WP-0.1).
- **Decide in the PR:** whether visual names are hashed and signed (recommended: yes, they're part of the pack's identity).
- **Accept:** the #285 boxes are ticked, `docs/content-authoring.md` is updated, and there's an example pack.
- **Reviewers:** netcode-reviewer.

#### WP-3.2 Mesh loading with procedural fallback (#286)
- **ADR:** write ADR-0007 on the mesh format.
  - Recommended: binary `.glb` only, through `gltf` with `default-features = false` (no `import`, image or base64). That handles static meshes with positions, normals and u32 indices, embedded buffers only.
  - Pin exact versions and run `cargo deny check` (the licences are expected to be MIT/Apache-2.0; verify them, don't assume).
  - Optional: a `moba-tools` baker to a simple `.mesh` format.
- **Loader:** `moba-assets` gets `load_glb(bytes) -> MeshData` behind a feature, with size, vertex and index caps (untrusted input) and a fuzz target.
- **Renderer:**
  - `Renderer::register_mesh(MeshData) -> MeshId`, uploaded with the existing `mesh::upload` (staging + sync2).
  - `UnitInstance.mesh` with id 0 = cube.
  - Per-mesh bucketed draws using `firstInstance` into the existing instance SSBO. The pipeline is unchanged; the vertex format stays `{pos, normal}`, stride 24.
  - Note that the 4,096-instance cap is shared across meshes.
- **Client:** resolve the name, then load from the asset root. On failure, log once and use the procedural rig or cube. A missing asset never fails a match.
- **Content:** ship one small, permissively licensed `.glb` under `content/`, recording the licence and source.
- **Accept:**
  - The custom model renders in a local match, with a validation-clean capture.
  - Allocation happens only at load time.
  - A gallery screenshot is added (docs/screenshots.md).
  - The golden fingerprint is regenerated **only** if the default scene changed, and from the CI artifact.
- **Reviewers:** gpu-safety-reviewer, perf-reviewer.

#### WP-3.3 Per-map visuals from manifests (#287)
- **Resolver:** `map_theme(u8)` (`crates/client/src/app/scene_build.rs` ~17) is hard-coded. Replace it with `resolve_map_visuals(theme/visuals, manifest)`, which reads a small descriptor (terrain tint, wall colour, optional wall/prop mesh names). Keep today's table as the fallback.
- **Local walls:** the local-sim path draws no walls today, so add them.
- **Wire:** no wire change while it stays keyed by theme id or name.
- **Accept:** two map packs render back to back in one session, and a validation-clean golden or capture of a custom-styled map is added.
- **Reviewers:** gpu-safety-reviewer.

### Phase 4: GPU skinning (M3 remainder)
**Facts:**
- `FRAMES_IN_FLIGHT = 2` (`renderer.rs` ~26). Descriptors are plain sets (not bindless):
  - set 0 is the camera UBO;
  - set 1 binding 0 is the instance SSBO;
  - there is one fixed pool, and sets are written once at init.
- Unit and terrain share a `pipeline_layout` with no push constants.
- Per-frame data goes straight into host-coherent `CpuToGpu` memory, after the frame fence wait.
- The hero rig is 6 cube instances posed on the CPU (`push_hero_rig`, `scene_build.rs` ~121). The network path derives facing and gait from interpolated positions.

#### WP-4.1 GPU skinning (#288; depends on WP-3.2)
- **Bone buffer:** a per-frame `bone_buffer` SSBO in `FrameData` (`CpuToGpu`), with 3 × `vec4` rows per joint (avoid std430 `mat3`). The default cap is 16 skinned instances × 64 joints, about 48 KiB.
- **Descriptor:** add binding 1 to the set-1 layout (vertex stage only), raise the pool's `STORAGE_BUFFER` count to `2 × FRAMES_IN_FLIGHT`, and write it at init.
- **Push constants:** `{first_bone, joint_count}` (16 B, vertex stage) on the **shared** `pipeline_layout`. A separate layout would break descriptor-set compatibility.
- **Vertex format:** `SkinnedVertex { pos, normal, joints: R8G8B8A8_UINT, weights: R8G8B8A8_UNORM }`. Make `pipeline::create` take a vertex layout, and add `skinned.vert` (register it in `crates/shaders/src/lib.rs` `MODULES`) while reusing `unit.frag`.
- **Barrier:** one `BufferMemoryBarrier2` from HOST/HOST_WRITE to VERTEX_SHADER/SHADER_STORAGE_READ before `cmd_begin_rendering`. The fence already covers write-after-read.
- **Data path:** the loader extends WP-3.2 to read `JOINTS_0`/`WEIGHTS_0` and one animation clip (walk or idle). The client samples the pose from the existing facing, gait and `render_tick`, with **no new wire fields**. Bone arrays are preallocated and cleared each frame; truncation is counted, not allocated.
- **Fallback:** keep `push_hero_rig` unchanged when there's no skinned asset, when the pipeline fails, or when an env opt-out is set.
- **Accept:** validation and sync validation are clean, there's no per-frame allocation (add an `allocation_probe` test around the skinning path), and the fallback still works.
- **Reviewers:** gpu-safety-reviewer (mandatory), perf-reviewer.

#### WP-4.2 Networked animated golden and frame cost (#289)
- **Deterministic capture:** `render_tick` is wall-clock driven (`crates/net/src/interpolation.rs` ~68–74), so a networked capture isn't repeatable.
  - Add `MOBA_CAPTURE_TICK=N`: capture once the phase is Playing and the tick is ≥ N, with animation time pinned to N.
  - Hide the FPS text in capture mode.
  - Add a second fingerprint, `tests/golden/render-network-fingerprint.rgb`, and a second step in `render-golden.yml` using `MOBA_SERVER=local`.
- **Measurement:**
  - Add a window-size env (the window is fixed at 1280×720, `launch.rs` ~167) and a network-mode warm-up (`MOBA_WARMUP_TICKS` only applies to the local sim).
  - Record `MOBA_VSYNC=0 MOBA_FRAME_LOG=…` at 1920×1080 on real hardware: GPU, driver, p50/p95/p99/max against the 6.9 ms / 16.6 ms budgets, with an A/B against the rig fallback.
  - Put the results in `docs/perf/0007-real-hardware.md`.
- **Accept:** see the #289 boxes. Numbers must be measured, not estimated.

### Phase 5: M6 playtest (needs people)

#### WP-5.1 Playtest runbook (`docs/playtest.md`)
Facts to use (all verified):
- **Host:**
  ```
  moba-server --listen 0.0.0.0:7777 --seconds 3600 --ready --record match.mbr --results-log matches.jsonl [--matches N] [--secret HEX | --allow-token HEX ...] [--server-key FILE]
  ```
  Or use `--config` (see `server.example.toml`). Each match has 10 player slots and 8 spectator slots.
- **Bots** fill empty slots (`bot_mask 0x03FF`, `rules.bot_fill` defaults to true). A dropped player keeps their slot for 60 s while a bot covers.
- **Join:** set these, then run `moba-client`:
  - `MOBA_SERVER=<ip:port>`;
  - optional `MOBA_SECRET`, `MOBA_SERVER_PUBKEY`, `MOBA_CLIENT_IDENTITY` (each must be exactly 64 hex characters, and the client fails closed if one is malformed);
  - `MOBA_SPECTATE=1`;
  - `MOBA_FRAME_LOG=<path>` with `MOBA_VSYNC=0`.
- **Outputs:**
  - one JSON line per finished match in the results log;
  - replays at `<stem>.matchN.mbr`, checked with `cargo run -p moba-tools --bin replay -- [--items PACK] match.mbr`;
  - the frame log, written on client exit.
- **Feedback:** a form with rubber-banding, input latency, desync/kicks and FPS per player, plus RTT/loss.

#### WP-5.2 Run it (maintainer plus players): **deferred until phases 3–4 are complete** (maintainer decision 2026-10-21)
- **Setup:** ≥ 2 hosts on a real network at about 100 ms RTT (WAN, or `netsim`/tc netem), 10 clients (humans plus bots allowed).
- **Record:**
  - the results log;
  - replays, which must pass `replay` verification;
  - per-client frame logs;
  - the feedback.
- **Wrap-up:** file each finding as an issue. Mark M6 done only with the recorded evidence (`docs/status.md` ~20, `docs/tasks/0006-vertical-slice.md` ~82).

### Phase 6: external or maintainer-only
These are never closed with simulated or hosted-runner evidence.

- **WP-6.1** (#9, #127):
  - **Status:** Linux/NVIDIA p95 is 4.197 ms. Hosted macOS p95 is 16.945 ms. Hosted Windows has no Vulkan device (`VK_ERROR_INCOMPATIBLE_DRIVER`), and `windows-probe` is `continue-on-error`.
  - **Decided (2026-10-21):** the maintainer will do the real Windows and macOS run by hand with `MOBA_FRAME_LOG`. Agents don't work on this; don't change the criteria.
  - **Note:** #9's comments suggest closing it as superseded by #127.
- **WP-6.2** (#21): **the maintainer does this by hand.** Agents must not change `publish = false` (`Cargo.toml:17`), add a registry token or publish crates. WP-7.1 (semver CI against a git tag) is still agent work.
- **WP-6.3:** an independent external security review (M7).
- **WP-6.4: DONE (2026-10-21).**
  - #295 merged and tagged `v0.4.0`; release run `39599870292` attached all artifacts.
  - GHSA-jw7c-q9x9-cgg3 is published (patched 0.4.0).
  - GHSA-23q3, GHSA-vpcq, GHSA-9j54 and GHSA-v6g8 are published (patched 0.3.0).

### Phase 7: 1.0 release
- **WP-7.1 Semver CI:** add `cargo-semver-checks` to CI/`release.yml` against the **git tag baseline** (`--baseline-rev v0.3.0`). This doesn't need crates.io, so it isn't blocked by #21.
- **WP-7.2 API review:** go through `docs/API-STABILITY.md`'s tiers. Mark public enums and structs `#[non_exhaustive]` where the API promises extensibility, and document the 1.0 surface.
- **WP-7.3 Release:** follow `RELEASE.md`: a final docs truth pass, CHANGELOG, `v1.0.0` tag, release artifacts (`release.yml`), and publish once WP-6.2 is resolved.
- **Gate:** phases 0–5 done, and phase 6 resolved or explicitly waived by the maintainer **in writing on the issue**.

---

## 5. Suggested assignment for parallel agents

| Lane | WPs, in order | Notes |
|---|---|---|
| A: net/content | 0.1 [proto] → 3.1 [proto] | Serial: both bump the protocol |
| B: net/host | 1.2 | Independent; coordinate with A on `server.rs` conflicts |
| C: sim | 2.1 → 2.3 → (2.2 [logic] only if 2.3 shows bias) → 2.3 rerun | Exact mirror not required (maintainer decision) |
| D: render | 0.2 → 3.2 → 3.3 → 4.1 → 4.2 | 3.2 depends on 3.1's schema only for names; it can start against a hard-coded test asset |
| E: docs/release | 5.1, 7.1, 7.2 | Any time |
| Maintainer | 6.1 and 6.2 by hand; 5.2 after phases 3–4; 7.3 | |
