# Changelog

All notable changes are recorded here. The workspace is pre-1.0; minor releases may include
breaking protocol, replay or content-schema changes.

## [Unreleased]

### Breaking changes
`scripts/semver-check.sh v0.4.0` reports that the next release must be **0.5.0**. The WP-7.2 API
review ([docs/api-1.0.md](docs/api-1.0.md)) made the public surface extensible before 1.0: error,
event, outcome and key enums, read-only stats/results and configs are now `#[non_exhaustive]`.
Configs get `Default` plus one `with_*` builder per field. Behaviour, wire format, hashes and
replays are unchanged. Breaking changes versus v0.4.0, by crate (`moba-assets` is now part of
the semver check):

- **`moba-game`**
  - New public fields on exhaustively constructible structs (#296, #317): `HeroSpec::model`,
    `MapSpec::visuals`, `GameContent::assets`, and `RulesSpec::recall_ticks` (recall). Struct
    literals must set them;
    `..Default::default()` works where `Default` exists. The content schema stays exhaustive by
    design ([api-1.0.md](docs/api-1.0.md#why-the-content-schema-stays-exhaustive)).
  - `#[non_exhaustive]`: `ContentError` (which also gained `BadMapCoordinate` and
    `BadVisualName`) and `ContentLoadError` (gained `Assets` and `UnknownAsset`). Add a `_` arm
    to matches.
- **`moba-sim`**
  - `#[non_exhaustive]` enums: `SimEvent`, `PlayerCommand`, `UnitKind`, `ReplayError`; also
    `MatchOutcome` (new since v0.4.0). Add a `_` arm to matches.
  - `#[non_exhaustive]` structs: `SimConfig`. Replace `SimConfig { seed, ..SimConfig::default() }`
    with `SimConfig::default().with_seed(seed)`; there are also `with_bot_mask`, `with_hero_picks`
    and `with_spawn_waves`. Also `TickInputs` (use `TickInputs::new()`/`set`/`set_purchase`),
    `Replay` (use `Replay::new(config)` and assign the public fields), `ReplayResult` and
    `ObservedUnit` (new `ObservedUnit::new(entity, kind, team, position)`, then assign fields).
- **`moba-net`**
  - `#[non_exhaustive]` enums: `WireError`, `CryptoError`, `PacketKind`, `WireCommand`,
    `MatchPhase`, `ClientEvent`, `ClientState`, `ServerPhase`. Add a `_` arm to matches.
  - `#[non_exhaustive]` structs: `ServerConfig` and `ClientAuth`. Use
    `ServerConfig::default().with_bind(addr).with_max_clients(4)` and
    `ClientAuth::default().with_pre_shared_key(Some(key))`; every field has a `with_*` builder.
    Read-only: `ServerStats` (gained `poll_budget_exhausted`/`snapshot_send_errors`),
    `ClientStats` (gained `datagrams_received`/`datagrams_sent`), `SnapshotState`, `NetUpdate`,
    `ReceivedMessage` and the wire catalogue, which only `moba-net` builds: `WireCatalog`,
    `WireItem`, `WireHero` (gained `model`/`model_len`), `WireAbility`, `WireEffect`,
    `WireAreaEffect`, `WireModifier`, `WireMap` (gained `visuals`/`visuals_len`), `WireLane`,
    `WireStructure`, `WireWall`, `WireRules` (gained the economy/score fields, `rewards`,
    `nexus_win` and `structure_heroes_first`), `WireMinionStats`, `WireStructureStats`.
    `HostReport`, `WorkerStats`, `WireRewards` and `WireUnitRewards` (new since v0.4.0) are
    `#[non_exhaustive]` as well.
  - Removed accidental public helpers that wrapped `protocol` functions and were unused:
    `client::{encode_raw, decode_raw, decode_welcome_payload, decode_hello_payload,
    decode_snapshot_payload}` and `server::{decode_input_message, apply_delta}`. Use
    `protocol::{encode_packet, decode_packet, decode_welcome, decode_hello, decode_snapshot,
    decode_input}` and `SnapshotState::apply`. Also removed the unused
    `transport::EventQueue` alias (`VecDeque`).
  - `crypto::{random_session, random_nonce}` are now `pub(crate)`. They are handshake
    internals; use `getrandom` for your own nonces.
- **`moba-render`**
  - `#[non_exhaustive]`: `RenderError` (which also gained `TooManyMeshes`, `InvalidMesh` and
    `SkinningUnavailable`). Add a `_` arm to matches.
  - `#[non_exhaustive]` structs: `RenderConfig` (gained `max_bones`/`skinning`; use
    `RenderConfig::default().with_vsync(false)` etc.), `Scene` (gained `bones`; use
    `Scene::new`/`Scene::empty` and the `with_*` methods), `UnitInstance` (gained `mesh`/`bones`;
    use `UnitInstance::new`/`at` with `with_mesh`/`with_bones`), `Camera` (use `perspective`/
    `orbit`), `CapturedFrame`, `VulkanProbe`.
  - `UiVertex` (R7a): `atlas` renamed to `mode`, new `outline` and `params` fields (48-byte
    layout). `UiDrawList::text` now uses the MSDF font, so `text_width` values change; the old
    5×7 font is `text_bitmap`/`bitmap_text_width`.
  - `UiVertex` (R7b): new `image` field (the offscreen image slot of `MODE_IMAGE`; `0` for
    every other mode), 52-byte layout. Struct literals must set it. The alpha channel of
    `Scene::terrain_tint` now scales the terrain grid lines (`1.0` keeps the old look, and is
    what every built-in tint and `Scene::new` use).
- **`moba-platform`**
  - `#[non_exhaustive]`: `PlatformError`, `Key`, `MouseButton` (add a `_` arm) and
    `WindowConfig` (use `WindowConfig::new(title, w, h)` or `Default` with `with_*` builders).
- **`moba-assets`**
  - `#[non_exhaustive]`: `AssetError`, `AssetManifestError`, `AssetManifest` and `AssetEntry`
    (new `AssetEntry::new(name, path)` + `with_fallback`). Types added since v0.4.0 are
    `#[non_exhaustive]` from the start: `AssetFileProblem`, `AssetList`, `AssetFileIssue`,
    `MeshError`, `GlbError`, `SkinError`, `MapVisualsError`, `TrackPath`, `Interpolation`,
    `MapVisuals`, `MapProp`, and the animation runtime's `AnimationSetError`,
    `AnimationEvent`, `AnimatorReport`, `PlayMode`, `Layer`, `ClipWrap` and `BlendSpaceError`
    (ADR-0012). `Skeleton::sample` now prefers a clip named `run` or `walk` over the first.
- No breaking change in `moba-core`, `moba-ecs`, `moba-jobs`, `moba-nav` or `moba-engine`.

### Security
- `ctutils` 0.4.3 → 0.4.6 (RUSTSEC-2026-0385: constant-time mask generation could compile to
  branches). It sits under `sha2`/`hmac`/`hkdf`, `ed25519-dalek` and `poly1305`, i.e. the
  session AEAD, token HMAC and identity signatures in `moba-net`. Also updated in `fuzz/Cargo.lock`.
- `tokio` 1.54.0 → 1.54.1 (RUSTSEC-2026-0384, yanked; only reached via `winit`'s wasm path).

### Build
- Removed unused internal dependency edges (`sim`/`server`/`client` → `moba-jobs`; `server` → `moba-core`/`moba-ecs`; `client`/`game` → `moba-ecs`; `jobs` → `moba-core`). `moba-test-support` (a counting allocator) is now a dev-dependency only of `moba-net`. `cpal` and `loom` moved to workspace dependencies.

### Documentation
- Corrected stale claims: README rate-limit placement (applied after decode, not before), the
  GPU reviewer's Vulkan target (1.3 core), a validation-hazard suppression variable that nothing
  reads, and `docs/status.md` still listing the 50-match load test as open (done in #293).
- Added the [playtest runbook](docs/playtest.md) (WP-5.1): hosting, tokens/identity, firewall, bots and spectators, client join variables, ~100 ms RTT with `tc netem`, replay/results/frame-log collection and a feedback form.
- Truth pass after the finish-plan work landed: test count 324 → 416 in the front-door docs,
  finished work packages marked done with their PRs (finish plan §2a), and `docs/status.md`
  and `AGENTS.md` no longer describe #276 work as pending.
- Recorded maintainer decisions (2026-10-28): the content schema stays exhaustive (breaking
  changes are fine until 1.0), and the playtest, external review, Windows/macOS GPU runs and
  crates.io publication happen just before the 1.0 release.
- RTX 5070 Ti evidence for R3b, R8 and distortion, re-run on `main` once the GPU was free
  (perf/0021, perf/0022, perf/0023): the validation matrix (37 configurations, 0 messages),
  the 10-minute `perf-replay.sh` on every tier with `tests/perf/baseline.toml` refreshed from
  it, `terrain-decal-check.sh`, the R3b per-feature costs and the distortion on/no-draw
  timings (median of three interleaved rounds), and RTX captures of the R3b screenshots.
  Recorded the maintainer decision (2026-10-30) that the art-direction sign-offs, the §7
  reference-GPU matrix and the self-hosted GPU runner are deferred to the pre-1.0 milestone.

### Planning
- Added `docs/plans/aaa-graphics-roadmap.md`: graphics roadmap (R0–R8), quality tiers,
  performance targets, ADRs to write and art-production requirements.

### Rendering and assets
- **Recall in the client (AAA roadmap R4 exit, ADR-0012 amendment;
  [perf/0024](docs/perf/0024-recall.md)):**
  - **Controls:** **B** recalls (`controls.recall`); the shop moves to **P** (`controls.shop`).
    A settings file saved before recall (no `recall` key) with the old `shop = "B"` is
    migrated to shop P, recall B, with a note on stderr; one with another shop key keeps it.
    A recall key another action (or the camera, item, shop or page keys) uses leaves recall
    unbound, with a warning. New `Key::P`.
  - **Prediction:** pressing recall sends the order only (none when the pack disables recall
    or the hero is dead); the predicted hero stops when a snapshot's recall flag confirms the
    channel, unless another order was given since. A recall that never starts (a stun the
    client is not told about, a lost packet) no longer stops the drawn hero while the
    server's walks on.
  - **Animation:** `EVENT_RECALL` starts the set's `recall` loop on its snapshot tick and a
    cancel or completion fades it out; the recall flag starts or ends it when those events
    are lost. Interpolation no longer slides a hero across the map on a teleport: between two
    snapshots where a living hero's recall flag cleared or a dead hero came back alive
    (`moba_net::interpolation::is_teleport`, `Interpolator::teleported_between`) it holds the
    old position until the later snapshot's tick and then snaps. It decides from those flags,
    not a distance, so a pack's long dashes or fast heroes still interpolate. The hero's
    facing and run speed ignore that step. The local preview and replays map the
    recall events too.
  - **VFX:** a `recall` default binding (`[defaults] recall = "recall"` in
    `vfx/bindings.toml`) plays `content/vfx/recall.toml` at the hero for the channel
    (`event_duration`): a contracting ground ring, a rising column of motes and a soft light,
    with no telegraph styling. A cancel or completion stops it emitting, and so do the hero's
    death and its recall flag clearing (or the hero leaving the snapshot); the flag also
    starts it when the start event was lost. One channel plays one effect.
  - **HUD:** a "RECALLING" channel bar above the hero panel while the local hero channels.
  - **Evidence runs:** `MOBA_DEMO_ORDERS=<order>@<tick>,...` (`recall`, `stop`, `attack`,
    `q`/`w`/`e`, `move:<x>:<y>`) scripts the local player; such runs log every recall event
    they receive (`[recall] ...`).
- **Distortion / heat haze (AAA roadmap R6 item 7, ADR-0019;
  [perf/0023](docs/perf/0023-distortion.md)):**
  - **Content:** `blend = "distortion"` emitters (`VfxBlend::Distortion`, `VfxDistortion`:
    `distortion`, `distortion_end`, `distortion_scale`, `distortion_speed`, optional
    `fallback_color`; sprite emitters only, no colour, team tint or telegraph). Map visuals
    gain up to 16 looping `[[effects]]` (`MapAmbientEffect`). Heat haze ships on
    `cinder-zone`, `ember-curse` and a new `heat-vent` effect at the Ember map's five
    crystals. `content-check` (`moba_assets::check_vfx_dir`) fails on a missing or invalid
    effect a binding or a map names and on a map effect with a telegraph; an unreferenced
    file that does not load, and a map effect that would pulse, are warnings. A map effect
    holds at most two live plays (`AMBIENT_PLAYS`) and never one of the last 64 free effect
    slots, so looping effects never starve gameplay events.
  - **Renderer:** `ParticleBlend::Distortion` particles go to a list of their own and are
    drawn by a `distortion` pass after the full water and before `vfx_draw`, refracting a
    `distortion_copy` of the haze's projected region plus the cap (CPU-tracked haze bounds;
    a transient sharing the water's refraction copy or the OIT accumulation target, so no
    extra VRAM): rising procedural noise, soft edge, birth, depth and view-distance fades, an
    offset capped at 6 px (High) / 8 px (Ultra) per 1080 render pixels. Units are masked: the
    prepass now writes ID 8 (`distortion::UNIT_ID_PLAIN`) for units without an outline, the
    outline pass ignores it, and taps landing on a unit or nearer geometry are rejected.
    Telegraph decals' rim and halo bands are masked too: the terrain, foliage and water write
    `1 - mark` into the HDR alpha (`telegraph_mark` is the CPU twin). Own pipeline layout (3
    fragment samplers; the scene layout stays at 16 of 16). Frames without haze in view run
    a frame-graph variant in which both passes are only *reserved* (new
    `PassBuilder::reserved`: same transients, no commands, no barriers). `MAX_PASSES`
    32 → 40 (the largest frame is now 33 passes).
  - **Tiers and switches:** High and Ultra (`VfxBudget::{distortion, distortion_pixels}`);
    off on Low and Medium, with `MOBA_DISTORTION=off` (`RenderConfig::distortion`), on
    portability-subset devices and without screen-space outlines (one `[render] distortion
    off: ...` line); `MOBA_DISTORTION=nodraw` (`RenderConfig::distortion_draw`) simulates
    the haze but never draws it. `Renderer::distortion_available`; the frame log gains
    `distortion_copy` and `distortion` lanes and the `vfx_distortion` statistic. New
    `render-distortion` golden (the `cinder-zone` preview on Ultra), plus a bloom-off pair
    of the same frame, drawn and no-draw, that must differ by at least 5000 pixels with the
    stand-in units' pixels identical (`image-check --against --min-changed --same-box`); the
    `renderer-gate` job's timeout is 45 minutes.
- **Culling, mesh LODs, VRAM budgets and frame pacing (AAA roadmap R8, ADR-0018;
  [perf/0022](docs/perf/0022-r8-scaling-pacing.md)):**
  - **Frustum culling:** every view culls its units against its camera (bounding spheres per
    mesh: VAT spheres over every baked frame, skinned bind-pose spheres doubled), and the main
    view builds a separate shadow caster list culled against the fitted shadow box's four side
    planes. Outlined units are never culled (the hull outline reaches past the silhouette).
    The terrain heightfield is split into 32 × 32-cell chunks with bounds, and each view draws
    the runs of its visible chunks. Culled frames are byte-identical to unculled ones
    (`scripts/cull-probe.sh`, a new render-golden step). `RenderConfig::culling`
    (`CullMode::{Off, Cpu}`), `MOBA_GPU_CULL=off` in the client. GPU-driven culling with
    indirect-count draws is designed but deferred: at today's instance counts the CPU path
    costs microseconds (ADR-0018 §2).
  - **Mesh LODs:** `moba-tools mesh-bake lod` simplifies a `.glb` with meshoptimizer (through
    the `meshopt` crate, behind the optional `mesh-bake` feature) into `models/<model>.lod`
    (`MLOD` v1: up to three index-only levels over the LOD0 vertices, each with its
    object-space error, tied to the mesh by `mesh_hash`); `scripts/bake-mesh-lods.sh` bakes
    every sample model. Collapses are weighted by normals and UVs (`--protect-uv-seams` locks
    UV seams instead), and each level's error is measured against LOD0 over the bind pose and
    sampled clip poses. A `fuzz/fuzz_targets/mesh_lod.rs` target fuzzes the `.lod` parser,
    seeded with the committed files; content-check warns on a stale or malformed `.lod`.
    `moba_assets::{MeshLods, MeshLod, load_lods_for, load_glb_file_with_lods}`,
    `MeshData::{with_lods, lods, bounding_sphere}`. The renderer appends the levels to the
    mesh's index buffer and picks, per unit and view, the coarsest level whose projected
    error is within the tier's tolerance (Low 2 px, Medium 1, High 0.75, Ultra 0.5); one
    level file serves static, skinned (every bone-LOD skin) and VAT meshes.
    `RenderConfig::mesh_lod` (`LodConfig`), `MOBA_MESH_LOD=off|<px>`.
  - **VRAM budgets and load-time-only uploads:** per-tier budgets in
    `moba_assets::TIER_BUDGETS` (`QualityTier::budget`): total and texture VRAM, per-model
    triangles, joints and texture bytes, the LOD tolerance. Every image and buffer is counted
    by class (`Renderer::memory_stats`, `MemoryStats`); a texture over the texture budget
    uploads without its largest mips, one that fits at no level is refused with
    `RenderError::OverBudget`. The swapchain images are counted too, the allocator's reserved
    block bytes are reported (`MemoryStats::{swapchain_bytes, reserved_bytes, over_budget}`),
    and the first allocation over the tier's total logs a warning.
    `Renderer::lock_uploads`/`unlock_uploads` mark the match: any upload or pipeline build
    while locked is counted (`locked_uploads`, `locked_pipelines`) and logged. The client
    locks on the first measured frame while the match is `Playing` and unlocks when it leaves
    `Playing`; `Renderer::new` starts unlocked and a dropped renderer unlocks
    (`Renderer::uploads_locked`). It prints a `[vram]` line at start.
  - **Culling bounds and cap:** a skinned mesh culls with a sphere measured over its clips'
    sampled poses (at least twice the bind sphere; `MeshData::cull_sphere`,
    `moba_assets::for_each_sampled_pose`); the instance cap applies after culling
    (`CullStats::dropped`, `cull_dropped`); the LOD metric uses the configured render height,
    so dynamic resolution never flips levels.
  - **Frame log:** `MOBA_FRAME_LOG_FRAMES` (default 10,000) sizes the log for 10-minute runs;
    the line adds `frames_dropped`, `gpu_total_p99_ms`, the VRAM counters, the locked counts
    and the main view's culling (`cull_drawn`, `cull_casters`, `cull_dropped`,
    `terrain_chunks`, `lod_instances`).
    `MOBA_REPLAY_EXIT=1` exits when a replay ends. `Renderer::cull_stats` (`CullStats`).
  - **Perf gate:** `moba-tools perf-gate` checks frame logs against the roadmap's §7 GPU
    budget per tier, frame pacing (p99 ≤ 1.5 × p50), zero mid-match uploads and pipeline
    builds, no dropped frames, the VRAM peak within the tier's budget, and
    `tests/perf/baseline.toml` (strict: bound to the GPU it was recorded on, a table per
    logged tier, no unknown keys); `scripts/perf-replay.sh` records the 10-minute
    teamfight (`teamfight --ticks 18000`) and runs it per tier; `.github/workflows/nightly-perf.yml`
    runs both on a `[self-hosted, gpu]` runner when `vars.HAS_GPU_RUNNER` is `true`.
  - **content-check** reports every unit model's triangles (LOD0 and baked levels), joints and
    texture bytes against each tier (`fits`, `lod<N>`, `over`; `budgets` in `--json`): over a
    tier warns, over Ultra is an error; a stale or malformed `.lod` warns.
- **Decals, foliage and water (AAA roadmap R3b, ADR-0017; completes R3):**
  - **Decals:** effect descriptors gain up to four `[[decals]]` (`moba_assets::VfxDecal`:
    ring, disc or splat; `paint` tints the ground's albedo before lighting, `glow` adds light
    after it; telegraph decals sweep with the effect's progress; `stamp` leaves a scorch or
    splat behind for its fade-out). The renderer projects up to 128 decals a frame in the
    terrain shader (and on the water surface) from one host-visible storage buffer per frame
    (set-3 binding 20, 24,608 bytes): the decals plus a 32×32 world grid of 128-bit masks over
    their bounding box. No extra pass and no sampler. **Enemy telegraphs** (`DecalRole`) are
    listed first and never dropped, their alpha keeps R5's 35 % floor (only the last 15 % of
    the telegraph fades), and they get a two-tone rim (a band brightened to at least 1.8× the
    ground's luminance plus a dark halo just inside it; rings included, whose cast timer
    fills their inside) whose contrast `glow_reference`, the CPU twin of the shader's
    per-shape coverage, and unit tests hold for every shape from black to 16× white ground;
    over water the telegraph is composited over the refracted bed (or drawn opaque in the
    simple water's blend). Decals outside the main camera's ground footprint are skipped, so
    a far-off one never coarsens the grid. Ally decals follow the allied-VFX opacity. Decals come
    only from the client's own effects, which follow what it is shown. The shipped
    telegraphs, impacts and deaths gain rings, scorch and splats; R5's particle rings stay.
    An effect lives `STAMP_LATCH_SECONDS` past a stamp's delay, so a late stamp still fires.
    Tiers cap the other decals at 48 / 96 / 128 / 128.
  - **Foliage:** a per-map `[foliage]` table (`MapFoliage`: density, height range, seed, kind
    and up to 32 `grass`/`brush`/`reeds`/`flowers` patches; reeds along the banks) scattered
    deterministically by `moba_assets::generate_foliage` (never on lanes, near walls,
    structures or spawns, or in water). The renderer draws it GPU-instanced (16-byte instances, procedural blades in the vertex
    shader, no vertex buffer), culled by 16 m chunk and draw distance (from the eye or the
    camera's focus on the ground, whichever is nearer), wind-animated, and pushed aside by
    up to 32 displacers on every tier that the client builds from the units it draws: hidden
    and long-dead units never bend anything (unit-tested through
    `rebuild_network_units`). The simulation has no brush, so foliage is decorative. Tiers:
    density 0.3 / 0.6 / 1 / 1, blades per clump 3–6, 60–180 m.
  - **Water:** up to eight `[[water]]` bodies per map (`MapWater`: a centre polyline or a
    pool, width, level, depth, flow, colours, clarity, foam). `generate_terrain_with_water`
    carves the beds into the heightfield and `generate_water` builds the surface on the
    terrain grid with the bed depth per vertex. The shader has flow-map normals (one octave
    on Low), depth fade, shoreline and contact foam and glow decals on the surface; Low and
    Medium
    blend it at the end of the scene pass (`Simple`), High and Ultra add **screen-space
    refraction** (`Full`: a `water_copy` pass copies the HDR region and a `water` pass reads
    it and the scene depth). `content-check` rejects water deeper than a unit can visibly
    wade through (0.8 m), levels outside 0..0.3 m (never below the ground units stand on;
    the parser rejects them too), water off the board and a water mesh over the renderer's
    `MAX_WATER_MESH_VERTICES` on any terrain grid, and warns about
    water near structures or spawns, overlapping bodies and brush over lanes or structures.
  - **Showcase:** `map-grass` gains a river with fords down the middle, reeds, brush and
    flower patches and a meadow; ember and frost get sparse, tinted grass in their own wind
    (no water).
  - **Tiers and switches:** `EnvironmentConfig::for_tier` (`RenderConfig::environment`),
    `MOBA_DECALS=off`, `MOBA_FOLIAGE=off`, `MOBA_WATER=off|simple|full`; the frame log gains
    `water_copy` and `water` lanes and `decals=`, `foliage=`, `water=` fields.
    `MOBA_PREVIEW_CAMERA` takes an optional focus (`yaw,pitch,distance,x,z`).
  - **Composition:** R3b adds no sampler to the scene pipelines (16 of 16 with R6's AO and
    R2b's baked AO and contact shadows; the decals are a third fragment storage buffer beside
    R2b's light list and grid, and the `descriptor_budget` test now also counts storage
    buffers per stage). Foliage and the simple water get MSAA twins; the full water runs
    after the resolve; neither is in the depth prepass nor writes outline IDs, so a unit in
    brush keeps its screen-space outline; with MSAA the scene resolves its depth again after
    the prepass whenever the water or the particles read it, so they stay behind the brush
    in front of them; refraction reads by texel, so render scale works.
    R2b's effect lights, baked AO and contact shadows light foliage and water.
  - **Evidence** ([perf/0021](docs/perf/0021-decals-foliage-water.md)): validation and
    synchronisation validation report 0 messages on lavapipe and the RTX 5070 Ti for every
    tier and switch, MSAA, render scale, the 5v5 teamfight at `MOBA_VFX_SCALE=8` and 64 stress
    minions. The decal shader reads each record in place: copying the record into a local
    pushed the terrain shader past a register step on NVIDIA and doubled its cost on Ultra.
    `scripts/terrain-decal-check.sh` guards it: it times the Ultra overview with decals on
    and off (`MOBA_DECALS=off` now builds the terrain from `terrain_nodecals.frag`, the decal
    code compiled out) and fails above 1.15×, and fails above 64 registers in the terrain
    statistics that `MOBA_PIPELINE_STATS=1` (`RenderConfig::pipeline_stats`) logs through
    `VK_KHR_pipeline_executable_properties`. Every golden was rebaselined
    deliberately (the showcase map changes every view) and four are new: the river on the
    default tier, on Low and through the bank's reeds on Ultra, and the teamfight's decals. Art sign-off of the showcase is a maintainer item.
  - No wire, replay, simulation-logic or content-hash change: decals, foliage and water are
    client-local presentation.
- **Terrain, fog of war v2 and atmosphere (AAA roadmap R3a, ADR-0014):**
  - **Terrain:** a map visuals `[terrain]` table (`moba_assets::MapTerrain`, in
    `MapVisuals::ground`; the old `terrain = [r, g, b]` tint still parses and moves into the
    table as `tint`) gives 1–8 layers (`textures/<albedo>.ktx2`, BC7 sRGB with cavity AO in
    alpha, optional BC5 normals; a tint and tiling each), an optional KTX2 splat map, the
    board relief, the map-edge style (`none`/`hills`/`cliffs`, `edge_height`) and optional
    edge props (`edge_mesh`). `moba_assets::generate_terrain` builds the heightfield (board
    relief ≤ 0.5 m, the margin only rising) and the splat (base, paths along lanes and around
    structures and spawns, rock on slopes and at wall feet, variation patches) from the
    simulation's map, deterministically. The renderer draws it as one grid mesh
    (`Renderer::set_terrain`, `TerrainData`) with the layers in two texture arrays and the
    splat in a two-layer array (set 3), triplanar on slopes (High+), normal maps (Medium+),
    baked relief AO; Low samples 4 layers top-down. Walls are placed by the shared
    `moba_assets::wall_box`. The sample maps (meadow, ember, frost) have terrain,
    atmosphere and fog-of-war tables; ember's centre crystal moved off the mid lane.
  - **Self-generated layer textures:** `texture-bake generate terrain` makes five tileable
    256² layers (grass, dirt, rock, moss, pebbles) with normals and AO, committed under
    `content/textures/` and checked against the generator; BC7 keeps the AO alpha.
  - **Visual-versus-simulation checks:** `content-check` runs
    `moba_assets::check_map_consistency` for the pack's map descriptor: errors for board
    relief over 0.5 m (on every tier's grid, including the interpolated surface of the
    cells straddling the board edge: `board_edge_relief`), edges dipping below the board,
    simulation walls not covered by a visual wall at least 2 m tall or outside the board,
    edge props whose real mesh footprint reaches the board (`edge_footprint_radius`,
    `check_map_consistency_with_edge_footprint`; the client places them by the same
    footprint) and bad layer or splat textures; warnings for tall props on lane corridors or
    next to structures. It checks the listed `[[assets.maps]]` descriptors and
    `maps/<visuals>.toml` whenever it exists, since the client loads it by name. The edge
    rise starts one Low-tier cell outside the board. The JSON output gains
    `map_errors`/`map_warnings`.
  - **Fog of war v2:** a GPU visibility texture replaces the 16-circle uniform loop:
    `fow-raster` draws one quad per own-team vision source (up to 1,024; walls up to 256)
    into a fixed-size `R8` transient with line-of-sight walls and `MAX` blending, and
    `fow-resolve` blurs it and eases it from the previous frame (reveal 0.08 s; lost vision
    drops to half at once, then fades in 0.3 s) into ping-pong persistent images. The disc
    is inset one texel so the blurred edge stays within half a texel of the vision radius,
    and a property test checks the wall test blocks whatever the simulation blocks. The
    scene's terrain samples it (darkened, desaturated, ground mist, styled per map by
    `[fog_of_war]`), and the minimap masks with it (`UiDrawList::fog_mask`; the 24×24 grid
    that ignored walls remains for `MOBA_FOW=legacy`, and `MOBA_FOW=off` turns the minimap's
    fog off too). The client builds
    the input from its own team only (`hud::FogInput`, unit-tested with enemies in the
    snapshot); spectators get none. 128² (Low) to 512² (Ultra). `MOBA_FOW=legacy` keeps the
    old loop, `MOBA_FOW=off` none, `MOBA_FOW_SIZE` overrides the size. With both offscreen
    passes, VFX, shadows and a capture the frame uses all 12 passes of the graph limits
    (22 of 24 resources). `RenderConfig::max_vision_sources` is clamped to 65,536
    (`MAX_VISION_SOURCES`) and the device's storage-buffer range.
  - **Descriptor budget:** renderer creation sums the scene layout's per-stage samplers and
    sampled images and fails with `RenderError::DescriptorLimit` over the device limits;
    the scene's fragment stage uses 13 of the 16 guaranteed (set-3 map in ADR-0014 §6).
  - **Atmosphere:** analytic height fog on the terrain and units (`[atmosphere]`,
    `SceneAtmosphere`), a far-plane sky (environment cube on Medium+, else a gradient) whose
    horizon meets the fog, and hills or cliffs past the board, so no board edge shows.
    `MOBA_FOG=off` keeps the R2a distance fade; `MOBA_SKY=off` the clear colour.
  - **Tiers and switches:** `WorldConfig::for_tier`, `RenderConfig::with_world`;
    `MOBA_TERRAIN=flat|splat`, `MOBA_FOW`, `MOBA_FOW_SIZE`, `MOBA_FOG=off`, `MOBA_SKY=off`;
    `MOBA_PREVIEW_CAMERA=<yaw>,<pitch>,<distance>` for the local preview. The frame log gains
    `gpu_fow_raster_*`/`gpu_fow_resolve_*` columns and `terrain=`, `fow=`, `fow_size=`,
    `fog=`, `sky=`.
  - **Evidence** ([perf/0017](docs/perf/0017-terrain-fow.md)): validation and synchronisation
    validation report 0 messages on lavapipe and on the RTX 5070 Ti on every tier and with
    every switch; per-pass GPU timings before and after; all goldens rebaselined
    deliberately, plus two new map goldens (networked overview, low horizon orbit).
  - No wire, replay, simulation-logic or content-hash change: terrain, atmosphere and fog
    styling are client-local map visuals. Decals, foliage and water are R3b.
- **Lighting: stylised PBR, image-based lighting and shadows (AAA roadmap R2a, ADR-0011):**
  - **PBR:** every lit surface (units, GPU-skinned heroes, terrain) goes through one
    `shade()` in `include/lighting.glsl`: GGX, height-correlated Smith-GGX visibility,
    Schlick Fresnel and an energy-conserving Lambert, with metallic/roughness from the
    material texture × factors (now read) and emissive (now added). A **stylisation layer**
    — wrap lighting, an optional N·L ramp texture, painted-specular tint and scale, a coloured
    rim — is set per map (`[lighting]` in `maps/<name>.toml`, `moba_assets::MapLighting`) and
    refined per material (`wrap`, `rim`, `ramp`, `specular`, `specular_tint`). ADR-0011
    records the model and the readability rules as acceptance criteria.
  - **Tangents:** normal maps use a vertex tangent frame instead of screen-space derivatives:
    the `.glb` loader reads `TANGENT` (`MeshData::with_tangents`/`tangents`), and a
    MikkTSpace-style generator (`MeshData::generate_tangents`) fills them in at registration
    and in `sample-mesh` (the static samples now ship tangents; the skinned sample is
    unchanged). Static vertices are 48 bytes, skinned ones 56.
  - **Image-based lighting:** per-map environments (`[lighting] environment = "<name>"` →
    `environments/<name>.{toml,ktx2}`, `moba_assets::EnvironmentDesc`, ≤ 4 KiB) give SH-L2
    diffuse irradiance and a prefiltered GGX specular cube, combined through a split-sum BRDF
    LUT generated at renderer creation. `moba-tools env-bake` bakes them from an analytic sky
    (presets `meadow`, `ember`, `frost`, every parameter overridable;
    `scripts/bake-sample-environments.sh`); no external HDRIs. The default grass map now has a
    descriptor (`content/maps/map-grass.toml`, the built-in colours plus `meadow`), and ember
    and frost got lighting tables. Without an environment the hemispheric ambient stays.
  - **KTX2 cube maps and RGBA16F** (ADR-0010 amendment): `faceCount` 6 (square, ≤ 1024²),
    `TextureFormat::Rgba16Float`, `TextureData::new_cube`/`is_cube`/`face`,
    `f16_from_f32`/`f32_from_f16`; same untrusted-input caps, a cube seed in the `ktx2` fuzz
    corpus. BC6H is deferred (documented).
  - **Shadows:** a new `shadow` render-graph pass before `scene` renders every unit (static
    meshes instanced; skinned heroes through a `SHADOW_PASS` permutation of the skinned
    vertex shader) into one directional map fitted to the camera's view of the board (sphere
    fit, 2 m radius steps, texel snapping: no shimmer while panning; no cascades needed at
    MOBA zoom, see ADR-0011), with slope-scaled bias and a normal offset. The graph derives
    the depth-write → fragment-sampled barrier. Filtering by tier: hard (1 tap, 1024²) on
    Low, 3×3 PCF (2048²) on Medium, PCSS-lite (16 + 16 taps, 2048²) on High and (16 + 32,
    4096²) on Ultra (PCSS taps keyed on the tier). Depth clamping ("pancaking") where
    `depthClamp` is supported; the map's format must support depth-comparison sampling
    (`mutableComparisonSamplers` on portability devices), else shadows turn off; the map
    survives resizes. R7b's offscreen views (portraits, minimap) render with shadows off and
    a lighting set naming a default shadow image, and the shadow pass draws only the main
    view. `shade()` output is clamped to 32768 (fp16-safe). Map descriptors enforce
    readability rules 2–3 (`shadow_strength` ≤ 0.9, `rim` > 0, terrain rougher than units,
    some ambient) and a sun elevation ≥ 0.2. A zero-length file tangent no longer rejects a
    `.glb`. Screen-space contact shadows are an R2b follow-up.
  - **Tiers and switches:** `LightingConfig::for_tier`, `RenderConfig::with_lighting`;
    `MOBA_SHADOWS=off|hard|pcf|pcss`, `MOBA_SHADOW_SIZE`, `MOBA_IBL=0` and
    `MOBA_SPECULAR_IBL=0` in the client. Low skips the specular cube (SH ambient only). The
    frame log gains `gpu_shadow_*` columns and `shadows=`, `shadow_size=`, `ibl=`,
    `specular_ibl=`.
  - **Evidence** ([perf/0015](docs/perf/0015-lighting.md)): validation and synchronisation
    validation report 0 messages on lavapipe and on the RTX 5070 Ti for the default,
    networked and textured captures on every tier. Both golden fingerprints were rebaselined
    deliberately for the new lighting.
  - No wire, replay, simulation-logic or content-hash change: environments, map lighting and
    materials are client-local.
- **Dynamic lights, team outlines, baked AO and contact shadows (AAA roadmap R2b, ADR-0016;
  R2 is done):**
  - **Dynamic lights:** clustered forward shading on a 32×32 grid of columns over the
    camera's ground footprint. `Scene::with_lights(&[PointLight])` (linear colour with the
    intensity premultiplied, windowed inverse-square falloff to `radius`); the CPU bins at
    most `MAX_LIGHTS` (256) lights, `MAX_LIGHTS_PER_CELL` (16) per column, into two
    host-visible storage buffers per frame in flight (set 3 bindings 11 and 12, after R3a's and R6's, so the lighting
    set is now one per frame in flight), and `shade()` loops over the fragment's column after
    the sun (wrap diffuse + GGX, no shadows; falloff floored at 1 m and the summed irradiance
    clamped to 1.5× the sun's, so stacked flashes never bleach a team colour). The footprint
    covers a shallow camera's ground out to the horizon; per-column counts stay on the CPU
    (the write-combined grid is never read back); a criterion bench (`light_binning`) times
    256 lights. Per-tier caps (Low 16/8, Medium 64/12, High
    128/16, Ultra 256/16) drop lights in list order and are counted (`Renderer::light_stats`,
    the frame log's `lights_*` columns). Offscreen views get none.
  - **Effect lights:** an effect descriptor may have one `[light]` table (`moba_assets::VfxLight`:
    colour, intensity, radius, target/source anchor, offset, delay, duration, fades, flicker,
    team tint, `scale_with_radius`; bounded and range-checked like the rest, covered by the
    `vfx` fuzz target); `VfxEffect::lifespan` includes it. The client emits one light per live
    instance, enemy and neutral first, allies scaled by `ally_vfx_opacity` (the viewer's own
    not, as for particles); 25 shipped effects
    got lights. Lights come only from effects the client already plays, so they reveal nothing.
  - **Team outlines:** `UnitInstance::outline` (a palette slot) and `Scene::with_outlines`
    (`OutlinePalette`: colour and pixel width per slot). The client outlines allied, enemy and
    its own heroes, the hovered enemy (what a click would attack) and the unit it last
    ordered attacked, in the HUD's team
    palette (the colour-blind palette applies). High/Ultra draw them in screen space: the new
    depth pre-pass writes an `R8_UINT` slot ID and an `outline` pass after the composite
    dilates it (8 directions, up to 4 px, higher slot wins, exterior only). Low/Medium draw an
    inverted hull in the scene pass (static, skinned and VAT permutations, pushed back 0.3 m
    and extruded in screen space, drawn only for each mesh's outlined instances, which lead
    its bucket). Both are depth-tested and only ever see units the client draws: never
    through walls, fog or brush; `scripts/outline-probe.sh` (CI) checks that a unit fully
    behind a wall changes no pixel on either path.
  - **Baked AO:** `moba-tools ao-bake` rasterises a map's walls and props into a 0.5 m height
    field and bakes horizon AO (16 directions, 12 m) into an `r8` KTX2 plus a sidecar
    (`moba_assets::BakedAoDesc`, ≤ 2 KiB) with an input fingerprint (`ao_fingerprint`, which covers each prop mesh's bounds and
    vertex and index counts, `PropMeshShape`);
    `[lighting] baked_ao`/`ao_strength` (`MapLighting`) name it, `maps/ao/<name>.{toml,ktx2}`.
    The client ignores a stale bake (fingerprint mismatch) with one log line;
    `Renderer::set_baked_ao`/`clear_baked_ao`/`has_baked_ao` bind it. `shade()` multiplies
    only the ambient line by `min(material AO, R6's GTAO) × baked`, faded out 3 m above the
    ground; every tier. Ships `content/maps/ao/ember-ao` and
    `frost-ao` (`scripts/bake-map-ao.sh`).
  - **Depth pre-pass and contact shadows:** R6's depth `prepass` also runs for contact
    shadows and screen outlines (High/Ultra, or when forced on) and draws terrain and static,
    skinned and VAT units depth-only (no depth bias, `invariant` positions) plus, with screen
    outlines, the `R8_UINT` outline-slot IDs (4× on Ultra, resolved from sample zero, or hull
    outlines on a device without 4× integer colour); the scene pass loads that depth
    (`LESS_OR_EQUAL`) instead of clearing it. `contact_shadows` ray-marches the pre-pass
    depth towards the sun (High 8 steps, Ultra 16, 0.8 m) into an `R8_UNORM` visibility that
    multiplies the shadow-map term, beside R6's GTAO on the same depth. Both, and the
    outlines, work in the frame's render-scale sub-rectangle.
  - **Frame and API:** the frame is `… → (shadow) → (prepass) → (contact_shadows) →
    (ao → ao_blur) → scene → vfx_draw → (vfx_resolve) → (bloom, exposure) → composite →
    (outline) → (fxaa) → (upscale) → ui`: the outlines draw on R6's LDR image before FXAA and
    the upscale (widths scaled by the render scale), or on the swapchain without them; the
    graph's limits are 32 passes and 48 resources (a `u64` mask, as in R6). The instance
    record is 128 bytes (R4b's 112 plus the outline slot), the camera uniform 1,696 (R6's
    1,408 plus 288). Set 3 gains bindings 11–14 (light list, light grid, baked AO, contact
    shadows, after R6's AO at 10), and the scene's fragment stage uses all 16 samplers
    Vulkan guarantees (R3a's `descriptor_budget`; the VAT layout 12). New (additive) API: `PointLight`, `LightStats`,
    `MAX_LIGHTS`, `MAX_LIGHTS_PER_CELL`, `OutlineMode`, `OutlinePalette`, `OUTLINE_SLOTS`,
    `MAX_OUTLINE_WIDTH`, `BakedAo`, `MAX_CONTACT_SHADOW_STEPS`, `LightingConfig::{max_lights,
    lights_per_cell, outlines, baked_ao, contact_shadow_steps}` and their builders,
    `Scene::{lights, outlines}`; in `moba-assets` `VfxLight`, `BakedAoDesc`, `BakedAoError`,
    `ao_fingerprint`, `PropMeshShape`, `baked_ao_paths`, `MapLighting::{baked_ao, ao_strength}`.
  - **Switches:** `MOBA_LIGHTS=off|<cap>`, `MOBA_OUTLINES=off|hull|screen`,
    `MOBA_BAKED_AO=off`, `MOBA_CONTACT_SHADOWS=off|<steps>`; the frame log gains
    `gpu_contact_shadows_*`, `gpu_outline_*` and `lights_*` (R6's `gpu_prepass_*` covers the
    pre-pass).
  - **Evidence** ([perf/0020](docs/perf/0020-lights-outlines-ao.md)): validation and
    synchronisation validation report 0 messages on lavapipe and the RTX 5070 Ti on every
    tier and every switch, with animated portraits, the minimap bake, `MOBA_VFX_SCALE=8` and
    the teamfight replay; per-pass GPU timings before/after at 1080p (median of 3 interleaved
    rounds on `main` with R6: Low +0.02, Medium +0.01, High +0.10, Ultra +0.15 ms GPU;
    contact shadows 0.05–0.09 ms the largest pass, the outlines 0.03 ms). The
    golden fingerprints (all seven with R3a's and R6's) were rebaselined deliberately (hero
    outlines and contact shadows on High and Ultra), two lavapipe runs byte-identical, and a
    new ember golden covers walls, baked AO
    and outlines beside walls. R3a's per-stage descriptor budget covers R2b's set-3
    bindings (16 of 16 fragment samplers with R6's AO).
  - No wire, replay, simulation-logic or content-hash change (protocol v41, replay v5, logic
    v8): effects, map visuals and bakes are client-local.
- **Animation runtime R4b: VAT minions, bone LOD, attachments (AAA roadmap R4, ADR-0012
  amendment):**
  - **Vertex-animation-texture minions:** `moba-tools vat-bake <model.glb> <out>` skins a
    model's clips on the CPU (the shader's formula) at 20 fps into one `RGBA16F` KTX2
    (positions then normals, rows ≤ 4,096 texels) plus a `<out>.toml` descriptor
    (`moba_assets::VatDescriptor`: vertex/frame counts, clips, a `mesh_hash` that detects a
    stale bake, `[playback]` run speed / attack release / crossfade). The sample
    `content/vat/minion.{toml,ktx2}` is baked from `sentinel-skinned` (974 vertices, 88 frames,
    1.4 MB). `Renderer::register_vat_mesh` registers a static mesh with its VAT (≤ 8,
    `MAX_VAT_MESHES`); VAT units are drawn instanced by a new `vat.vert` (one draw per VAT
    mesh, no bone buffer, frames interpolated and two samples crossfaded in the shader) and
    cast shadows through a `vat_shadow.vert` permutation. `UnitInstance::with_vat(VatPose)`
    sets the per-instance clip times; the instance block grows from 80 to 112 bytes.
    `VatPlayer` (fixed size) drives idle ↔ run by interpolated speed, attacks from
    `EVENT_ATTACK` (release on the hit tick) and deaths from `EVENT_UNIT_DIED` on a corpse at
    the death position. Melee/ranged minions use `vat/minion-melee`/`vat/minion-ranged`, else
    `vat/minion`; a missing or mismatched asset falls back to the cube (logged once).
    Tiers: Low nearest frame, Medium interpolated, High/Ultra interpolated + crossfaded.
    `MOBA_VAT=off` draws cubes.
  - **Bone-count LOD:** `moba_assets::SkinLod` builds reduced skins at load (LOD1 ≤ 2
    influences and 75 % of the joints, spring joints first; LOD2 1 influence, ≤ 4 joints;
    dropped joints collapse onto their nearest kept ancestor) and gathers their palettes from
    the animator's matrices. Heroes switch by distance (Low 40/80 m … Ultra 110/180 m) and draw
    LOD2 off screen; pinned captures keep LOD0. `skinned.vert` skips zero-weight influences.
    `MOBA_BONE_LOD=off|<near>,<far>`.
  - **Visible socket attachments:** `[[sockets]]` take an optional static `model` and `scale`
    (`AnimationGraph::attachment(s)`, `AnimationSet::attachment_models`); the client draws them
    at the socket in a brightened hero colour, skipping far and off-screen ones on Low. The sample set puts a
    `crystal` on the sentinel's spear tip and head. `MOBA_ATTACHMENTS=off`.
  - **2D blend spaces:** `BlendSpace2d` (gradient-band weights, ≤ 8 points) used when
    `[[locomotion]]` points give `x`/`y`; `Animator::evaluate_motion` takes a local velocity.
  - **Local preview and replays on the same runtime:** the bot preview and `MOBA_REPLAY`
    turn each simulated tick into the snapshot and events a match delivers (`LocalFeed`), so
    heroes play the same attack/cast/death animations and minions their VATs as in a match;
    local VFX anchors use the same ids, so local effects attach to sockets.
  - **Evidence tooling:** `MOBA_STRESS_MINIONS=<n>` (≤ 512 client-only VAT minions in the
    local preview, CPU per frame in the frame log), `MOBA_LOCAL_CAMERA_DISTANCE` for the local
    overview, `MOBA_ANIM_LOG`, `MOBA_CAPTURE_SERIES`/`MOBA_CAPTURE_SERIES_TICKS`,
    `MOBA_SPECTATE_FOLLOW=<team>:<slot>`, and `moba-tools link-proxy` (a UDP proxy with RTT,
    jitter and loss around `moba_netsim::LinkProxy`). The frame log gains `vat_meshes=` and the
    `stress_*` columns.
  - **Evidence** ([perf/0018](docs/perf/0018-vat-anim-lod.md)): 200 VAT minions cost
    ≈ 8 µs of CPU per frame, p95 ≤ 17 µs (budget 0.5 ms), and 0.04 ms of GPU over cubes on
    the RTX 5070 Ti; validation and synchronisation validation
    report 0 messages on lavapipe and the RTX 5070 Ti on every tier and with every off
    switch; at 150 ms RTT a skinned hero played idle, run, attack, three casts and death
    (the simulation has no recall). Both golden fingerprints were rebaselined (VAT minions,
    attachments, animated local preview), and a third golden checks a VAT close-up
    (`tests/golden/render-vat-fingerprint.rgb`). `Renderer::mesh_capacity` reports the mesh
    slots; heroes register their base meshes before any bone-LOD skin, and LOD skins are
    skipped (logged once) when fewer than 8 slots would remain. `vat-bake` refuses playback
    values or texture sizes (over `MAX_VAT_TEXELS`, the KTX2 limit) the client would reject.
  - No wire, replay, simulation-logic or content-hash change: VATs, attachments and LOD are
    client-local.
- **VFX system (AAA roadmap R5, ADR-0013):**
  - **GPU particles:** a fixed per-tier particle buffer (`QualityTier::vfx_budget`: Low 4,096
    … Ultra 49,152 particles, 512 … 4,096 spawns per frame) simulated by a new `vfx_simulate`
    compute pass (spawn, age, integrate, compact into indirect draw lists, projected-pixel
    overdraw estimate) and drawn by `vfx_draw` after the opaque scene into the HDR target:
    sprites, flipbooks, trails, beams, ground rings and mesh particles (four mesh slots), one
    indirect draw for all quads plus one per mesh. Additive blending plus weighted blended
    OIT for alpha particles (Medium and up; revealage stored as `Σ -ln(1 - α)` so one blend
    state serves every attachment, resolved by `vfx_resolve`), unsorted premultiplied on Low.
    Soft particles (Medium and up) sample the scene depth, which is now attached read-only and
    stored only when VFX are on. The CPU gives each spawn a free slot (exact expiry times, at
    most 64 slots scanned), so the cap never overwrites a live particle; drops are counted.
    API (additive): `ParticleSpawn` and its `ParticleShape`/`ParticleBlend`/`ParticleRole`,
    `VfxFrame` via `Scene::with_vfx`, `VfxBudget`, `VfxStats`, `RenderConfig::vfx`,
    `Renderer::{vfx_available, vfx_budget, vfx_stats, set_vfx_atlas, set_vfx_mesh}`,
    `RenderError::Vfx`.
  - **Render graph:** compute-storage, indirect and read-only-depth-plus-sampled accesses
    with headless barrier tests, a `persistent_buffer` import state for buffers that live
    across frames, and limits of 12 passes / 24 resources. The frame is `(portraits, minimap) →
    vfx_simulate → (shadow) → scene → vfx_draw → vfx_resolve → composite → ui`; `MOBA_FRAME_LOG` gains the three VFX pass times
    and `vfx_alive`/`vfx_overdraw` percentiles.
  - **Effects as content:** client-local, bounded TOML under `<MOBA_ASSET_ROOT>/vfx/`
    (`moba_assets::VfxEffect`, ≤ 8 emitters, 16 KiB; `VfxBindings`, `bindings.toml`, ≤ 128
    abilities) map catalogue ability names to cast/projectile/hit/zone effects (with an
    animation-socket attach point and a tint) and unit events to death, structure and attack
    effects. The client plays them from presentation events (network events released at their
    snapshot's tick, so effects line up with interpolated units) and from local `SimEvent`s.
    Missing or invalid files are logged once and play nothing. A new `vfx` cargo-fuzz target
    covers both parsers. Ships `content/vfx/` with 30 effects covering all 15 built-in and 8
    Nova Pack abilities and every unit event, and `content/vfx/atlas.ktx2` (BC5, generated by
    `texture-bake generate vfx` from `moba_assets::procedural_atlas`, which is also the
    renderer's built-in atlas; no external art).
  - **Budgets and readability:** per-tier particle and overdraw budgets drop `low`, then
    `normal`, then `high` effects as they fill; effects far from the camera's focus (scaled
    with zoom) or off screen spawn nothing; enemy telegraphs and enemy projectiles are never
    culled and go first in the spawn list, and telegraphs keep ≥ 35 % opacity (drawn without
    the soft fade); allied effects follow the new `ally_vfx_opacity` setting and their alpha
    particles are capped at 75 %. A projectile whose effect does not resolve or spawns
    nothing keeps the pre-VFX cube. A missing or invalid `vfx/bindings.toml`, `vfx = false`
    or `MOBA_VFX=off` turns effects off (and the passes with them). Portability-subset devices
    (`MoltenVK`) get no soft particles.
  - **Authoring and evidence:** `MOBA_VFX_PREVIEW=<effect>` is a live-reload effect preview;
    `MOBA_REPLAY=<file.mbr>` plays a replay in the local view with a camera on the fight (the
    local HUD now titles these `VFX PREVIEW` and `REPLAY`);
    `moba-tools teamfight` records the scripted, deterministic 5v5 teamfight
    `tests/replays/teamfight-5v5.mbr` (verified by a new `moba-sim` test). Per-tier RTX 5070 Ti
    timings and the overdraw p95 are in `docs/perf/0016-vfx.md`; validation and sync
    validation are clean on lavapipe and the RTX. No wire, replay, logic or content-hash
    change; the goldens are unchanged.
- **Post-processing and image quality (AAA roadmap R6, ADR-0015):**
  - **Post chain:** the frame gains a five-level physically based bloom chain
    (`bloom_d0`…`bloom_d4`, 13-tap down-sample with a Karis average on the first, tent
    up-sample `bloom_u3`…`bloom_u0`; Medium and up), optional eye adaptation (`exposure`: the
    1/32 bloom mip metered into a persistent 1×1 `R32_SFLOAT` image (`R16_SFLOAT` where that
    cannot blend, with steps deferred to ≥ 1/32 so slow adaptation never stalls), clamped per
    map, adapted with a constant blend and cleared on a reset, non-finite input metered as
    black; no compute or read-back), and a composite that mixes the bloom, applies
    exposure, AgX, a per-map **3D colour-grading LUT** and the gameplay screen effects. Per-map
    settings live in the client-local `[post]` table of `maps/<name>.toml`
    (`moba_assets::MapPost`: exposure EV, `auto_exposure` with its EV clamp and rate, bloom
    strength, `grading = "<name>"` → `grading/<name>.cube`, grading strength). The `.cube`
    parser (`GradingLut`, 2³…64³, ≤ 8 MiB, display-referred `0..1` domain only) has a new
    `grading` fuzz target; `moba-tools grade-bake` bakes LUTs from documented
    lift/gamma/gain, contrast, saturation and white-balance parameters (presets `ember`,
    `frost`, `meadow`, shipped as 17³ LUTs and regenerated by
    `scripts/bake-sample-textures.sh`), and prints or applies them to a PNG. Ember opts into
    auto exposure (±1 EV). Map bloom is capped at 0.1. `GradingLut::check_team_readability`
    (ADR-0011 rule 1) holds a grade to the standard and colour-blind team colours at the map's
    strength and EV ± 1: `grade-bake` refuses a failing LUT and the client draws ungraded.
  - **Anti-aliasing:** FXAA (own implementation of the FXAA 3.11 edge search, fast preset on
    Low) on the tonemapped LDR image on Low–High, and **MSAA 4×** on Ultra: the scene (and the
    AO prepass) render into 4× attachments resolved by dynamic rendering (colour average,
    depth sample 0), so R5's particles, AO and every post pass stay single-sampled; the
    multisampled colour is a transient attachment. A device whose HDR or depth format has no
    4× sample count falls back to FXAA. R4b's VAT minions draw through the multisampled
    pipelines and the prepass like every scene mesh. The HUD is never filtered. TAA and SMAA are not implemented (ADR-0015 §4).
  - **Ambient occlusion:** GTAO at half render resolution (2 slices on High, 3 on Ultra;
    world-space, depth-reconstructed normals, 4×4 interleaved rotation and a depth-aware 4×4
    blur) from a new depth **prepass** of the main view (every scene vertex shader declares
    `invariant gl_Position`, and the scene loads that depth). It occludes only the ambient
    diffuse and specular terms (specular occlusion after Lagarde & de Rousiers), never direct
    light, rim, emissive or particles. Offscreen views bind a white AO image. AO is set-3
    binding 10, after R3a's world bindings (the scene's fragment stage uses 14 of 16
    samplers). On High and Ultra, R3a's baked terrain relief AO joins GTAO as
    `min(baked, gtao)` on the ambient term instead of darkening the albedo, so crevices are
    not darkened twice; Low and Medium keep R3a's look.
  - **Resolution scaling:** `render_scale` (0.5–1, setting and `MOBA_RENDER_SCALE`) renders
    the scene and post chain below native and an `upscale` pass (bilinear on Low, Catmull-Rom
    bicubic above) writes the swapchain before the native-resolution UI. Dynamic resolution
    (Low by default, 14 ms GPU target, 0.6–1.0) adjusts the scale from the GPU timings with a
    hold and hysteresis; images are allocated at the maximum scale and each frame renders a
    sub-rectangle, so a scale change never allocates. FSR is not included.
  - **Gameplay screen effects:** death desaturation, a damage vignette and a low-health pulse
    from the local hero's snapshot health (`ScreenEffects` via `Scene::with_effects`), each
    with an accessibility setting (`death_desaturation`, `damage_vignette`,
    `low_health_pulse`); `MOBA_SCREEN_EFFECTS=off` turns them all off and
    `MOBA_SCREEN_EFFECTS_PREVIEW=death|damage|lowhealth` holds one for captures. They keep
    team colours readable: desaturation stops at 0.6 and the vignette is a neutral dark,
    corners only, at most 0.45.
  - **Render graph (ADR-0008 amendment):** `ImageSize::Render { shift }` (render-resolution
    images; HDR, depth and R5's OIT targets use it), per-image sample counts, the
    `DepthReadWrite`, `DepthWriteResolveSource` and `DepthResolveWrite` accesses, and limits
    raised to 32 passes and 48 resources (a `u64` use mask). `FrameSpec::prepass` asks for the
    depth prepass without AO (for R2b's contact shadows).
  - **Tiers and switches:** `PostConfig::for_tier` (Low: FXAA fast, no bloom/AO, dynres;
    Medium: + bloom, FXAA; High: + GTAO; Ultra: MSAA 4×, GTAO 3 slices), `RenderConfig::post`;
    `MOBA_BLOOM`, `MOBA_AA=off|fxaa|msaa4`, `MOBA_AO`, `MOBA_RENDER_SCALE`,
    `MOBA_DYNRES=off|on|<ms>`, `MOBA_UPSCALE`, `MOBA_GRADING=off`,
    `MOBA_EXPOSURE=auto|fixed|<EV>`, `MOBA_SCREEN_EFFECTS=off`. Captures pin auto exposure,
    dynamic resolution and the screen effects off unless set explicitly. The frame log gains `prepass`, `ao`,
    `bloom`, `exposure`, `fxaa` and `upscale` lanes (printed when the pass exists) and `aa=`,
    `bloom=`, `ao=`, `auto_exposure=`, `grading=`, `render=`, `dynres=` fields.
  - **Evidence** ([perf/0019](docs/perf/0019-post.md)): validation and synchronisation
    validation report 0 messages on lavapipe and on the RTX 5070 Ti for every tier and every
    off switch, with and without particles; per-pass GPU timings before and after on the R5
    teamfight replay are within every §7 budget. The default, networked and VAT golden
    fingerprints were rebaselined deliberately (bloom, grading and FXAA change every scene
    pixel; the HUD is unchanged), and two goldens are new: Ultra (MSAA 4× + GTAO with VAT
    minions) and Low at render scale 0.75 (FXAA fast + the upscale).
    Distortion / heat haze (roadmap R6 item 7, moved from R5) is not implemented yet.
  - No wire, replay, simulation-logic or content-hash change: post settings and LUTs are
    client-local map visuals.
- **Materials, textures, pipeline cache and shader tooling (AAA roadmap R1b, ADR-0010):**
  - **Materials:** a model named `<m>` picks up a client-local descriptor
    `<MOBA_ASSET_ROOT>/materials/<m>.toml` (`moba_assets::MaterialDesc`, ≤ 4 KiB, range-checked):
    a shader permutation (`vertex-color` or `textured`), a parameter block (base colour factor,
    metallic, roughness, normal scale, team tint, emissive) and a texture set (base colour,
    normal, metallic-roughness, team-colour mask) at `textures/<name>.ktx2`. Each textured
    material gets its own descriptor set (set 2), from one pool sized for exactly the materials
    of a content load (ADR-0009); nothing is allocated per frame. The `MATERIAL_TEXTURED`
    permutation samples base colour, tints it by the unit colour through the mask and applies
    the normal map (derivative tangent frame); metallic/roughness are bound for R2. Any problem
    is logged once and keeps the vertex-colour look. Ships `content/materials/sentinel.toml`
    with self-generated textures on the static `sentinel` model. Not hashed, not on the wire.
  - **Textures:** a bounded KTX 2.0 loader (`moba_assets::load_ktx2`: ≤ 4096², ≤ 13 levels,
    ≤ 64 MiB, checked before allocating; data-format-descriptor transfer function must match
    the format's sRGB-ness; fuzz target `ktx2` with a corpus) and writer; BC7/BC5/BC4 upload
    as is where `textureCompressionBC` is supported, and are decoded on the CPU at load
    otherwise (`bc-decode` feature, `bcdec_rs`) or with `MOBA_COMPRESSED_TEXTURES=0`. Each slot
    accepts only its formats (sRGB colour, linear data).
  - **`moba-tools texture-bake`:** PNG → KTX2 with mips filtered per kind (linear-light colour,
    renormalised normals), an in-house BC7 (mode 6), BC5 and BC4 encoder (51–54 dB on the
    sample set), `info` (with the CPU-decode cost) and `generate sentinel` (the procedural
    sample set; `scripts/bake-sample-textures.sh`).
  - **UVs:** `MeshData::with_uvs`/`uvs`, `TEXCOORD_0` in the `.glb` loader, box-projected UVs on
    the generated static samples (`sentinel`, `crystal`, `rampart`; the skinned sample is
    unchanged). Static vertices are 32 bytes and skinned ones 40.
  - **Pipeline cache:** every pipeline and permutation is built at renderer creation through a
    persistent `VkPipelineCache` (`MOBA_PIPELINE_CACHE`, default a per-user cache file; `off`
    disables). The file is checksummed and its vendor, device and `pipelineCacheUUID` are
    checked before the driver sees it; a corrupt or foreign file is a cold start (a corrupt
    one is deleted, a foreign one kept), a driver blob that would not be reusable is not
    saved, and saves go through a fresh `create_new` temporary, `sync_all` and a rename. RTX 5070 Ti: 7 pipelines in 90–98 ms
    cold, 0.5 ms warm (driver disk cache off); renderer ready in about 175 ms vs 84 ms. The
    startup log and `MOBA_FRAME_LOG` report `pipelines`, `pipeline_build_ms`,
    `pipeline_cache` and `startup_ms`.
  - **Shader tooling:** GLSL `#include` files (`crates/shaders/shaders/include/`: the camera
    block that was duplicated in five shaders, the instance buffer, lighting and the material
    set), a build-time permutation table (`MATERIAL_TEXTURED` → `unit_textured.frag.spv`,
    `unit_textured.vert.spv`, `skinned_textured.vert.spv`), `spirv-val` on every module as
    before, and hot reload in debug builds (release builds ignore it, since a mismatched module
    is invalid Vulkan usage): `MOBA_SHADER_DIR` holds SPIR-V overrides that are re-read when a
    `.spv` changes (scene and composite pipelines; `Renderer::reload_shaders`).
  - **R1a review fixes:** the normal and capture graph variants must agree on each transient
    slot's cross-frame start `(stage, access)`, not only the slot list; read-before-write is
    also enforced for imports with discarded contents (the swapchain), final accesses
    included; `QualityTier::ALL` is a `&'static [QualityTier]` (semver-safe for the
    `#[non_exhaustive]` enum); `GpuTimings::frame` numbers each sample and the frame log
    records each GPU sample once instead of repeating stale ones; and the scene pass is
    submitted in a batch that does not wait on the acquire semaphore, so its GPU time cannot
    include that wait.
  - **New public API (additive):** see ADR-0010 "Consequences" and
    [docs/api-1.0.md](docs/api-1.0.md). Golden captures are byte-identical to `main`
    (lavapipe, validation + sync validation, 0 messages); evidence in
    `docs/perf/0013-materials.md`. No wire, replay, logic or content-hash change.
- **Render-to-texture HUD images: 3D hero portraits and a baked minimap (AAA roadmap R7b):**
  - **Offscreen targets through the graph:** the renderer owns fixed offscreen images, one
    per `UiImage` slot (a 2048×128 portrait strip and a 512×512 minimap by default,
    `RenderConfig::with_ui_image_size`; HDR format, each with a depth buffer), created with
    the renderer and never resized or reallocated. A frame that re-renders one
    (`Renderer::render_with_offscreen` with an `OffscreenRender`: a `Scene`, optional
    terrain, a clear colour and backdrop rectangles) gets that image's graph pass ahead of
    the scene, drawn with the scene pipelines, meshes and materials into per-image per-frame
    buffers (`OFFSCREEN_MAX_INSTANCES` units, `OFFSCREEN_MAX_BONES` joints). The images are
    persistent graph imports (`ImportState::persistent`): they start and end every frame in
    `SHADER_READ_ONLY_OPTIMAL`, the UI pass declares a fragment sampled read of each, and the
    graph derives the write-after-read, write-after-write and read-after-write barriers. The
    eight capture × offscreen variants are compiled at creation and share the transients;
    headless graph tests cover the barrier plan of every variant.
  - **UI images:** `UiVertex::MODE_IMAGE` and `UiDrawList::image` sample an offscreen image
    (binding 2 of the UI set: one combined image sampler per slot, indexed by a constant in
    each shader branch, so no descriptor-indexing feature is needed) with a 2×2 box over the
    pixel footprint, then apply the composite's exposure and AgX curve (now shared through
    `include/tonemap.glsl`). The pixel-art atlas, MSDF text and sweep modes are unchanged.
  - **Hero portraits:** the client renders one 128-pixel cell per `(team, hero slot)` into
    the strip: an orthographic "studio" view, simple key and rim light, a team-coloured
    backdrop per cell, each hero three-quarter turned. Skinned heroes with an animation set
    play their idle clip through a per-cell R4a animator; others use the rest pose, the static
    mesh or the procedural rig. Tiered: `Low` renders when the heroes change (static),
    `Medium` 5 Hz, `High`/`Ultra` 10 Hz; `MOBA_PORTRAITS=off|static|animated|<hz>` overrides
    it (`off` restores the text-only layout and allocates a 1×1 image). Shown in the hero
    panel (a 104-pixel portrait column) and leading each scoreboard row; dead heroes are
    dimmed.
  - **Baked minimap:** at every map bind (startup, content update, map rotation) the client
    renders the map's terrain, walls and props (`MapVisuals`) straight down, orthographic,
    into the minimap image once (with a faint terrain grid); the HUD draws it under the
    lanes, unit dots and the own-hero frame. Outside allied vision the image is darkened by a
    24×24 grid computed from the units the client was sent (presentation only: it never
    reveals a hidden unit, and like the markers it ignores vision-blocking walls).
    `MOBA_MINIMAP_BAKE=off|once|<frames>` (`off`: the flat minimap; `<frames>` re-bakes
    periodically to measure it).
  - `render_with_offscreen` returns whether the frame was submitted, and
    `Renderer::last_frame_submitted` also answers after an error (presentation can fail
    after the submit), so a caller re-sends exactly the renders that did not land. A map
    with more walls and props than one offscreen render takes (`OFFSCREEN_MAX_INSTANCES`)
    bakes the first ones and logs once.
  - **Frame log:** `MOBA_FRAME_LOG` gains `gpu_portraits_*`/`gpu_minimap_*` columns over the
    frames those passes ran in, with a `_runs` count.
  - **Cost:** RTX 5070 Ti at 1920×1080: the portrait pass takes 8–9 µs and a minimap bake
    8–9 µs (in the frames that run them); the `ui` pass goes from 6 µs to 9 µs p50 with the
    portrait, minimap image and fog. CPU portrait updates and the HUD stay allocation-free
    (`docs/perf/0014-ui.md`).
  - **Golden images:** the networked fingerprint was **deliberately rebaselined** (hero
    panel portrait column and the baked minimap); the default-scene capture, whose local HUD
    has neither, is byte-identical. Lavapipe, validation and sync validation required, 0
    messages.
  - No wire, replay, logic or content-hash change.
- **UI, HUD and presentation polish (AAA roadmap R7a):**
  - **MSDF text:** HUD text is drawn from a multi-channel signed-distance-field atlas (an
    MTSDF: RGB for sharp corners, alpha for the true distance) of Open Sans Condensed SemiBold
    (SIL OFL 1.1, `THIRD_PARTY.md`). `ui.frag` anti-aliases it from screen-space derivatives,
    so text stays crisp at any UI scale and at 4K; outlines and soft shadows come from the
    same atlas. The 512×208 atlas and its metrics table are baked offline by
    `crates/tools/font-bake` and checked in. The baker uses `fdsm` and `skrifa`, both
    permissive, and is a standalone crate with its own lockfile, like `fuzz/`. Through
    `nalgebra` it pulls in the unmaintained build-time `paste` macro (RUSTSEC-2024-0436), so it
    stays out of the workspace and the shipped dependency graph. `ci-tools` re-bakes the font
    and fails on any difference. Capital letters keep the old `7 × scale` height, so
    layouts keep their proportions. The RGBA atlas keeps the pixel-art icons and the legacy
    5×7 font (`UiDrawList::text_bitmap`).
  - **Health bars:** palette team colours (own/ally/enemy), a damage "ghost" that holds 9
    ticks and then drains, a shield segment, tick marks every 100 HP (heroes, stronger every
    1,000) or 500 HP (structures), hero level badge and mana strip. Bars of units far off
    screen are culled instead of piling up at the edge.
  - **Colour-blind palette:** new `team_palette` setting (`standard`/`colorblind`, default
    `standard`) and `MOBA_TEAM_PALETTE` override. `colorblind` uses Okabe-Ito yellow,
    sky-blue and orange for self/ally/enemy on health bars and the minimap.
  - **Floating combat text:** damage, heal and own-gold numbers rise and fade, with a pop
    and `!` for big hits. They come from snapshot health and gold deltas (the wire has no
    per-hit damage, attacker or crit flag; "crit" means one change of at least 12 % of max
    health), are timed by the render tick so they appear with the interpolated hit, and live
    in a fixed 360-slot ring. The ring fits 8 spawns per snapshot across the render lag plus
    the longest life. Each snapshot keeps its 8 most important numbers: the own hero first,
    then the viewer's team, then the biggest changes. The newest 48 are drawn, and off-screen
    numbers are culled. A unit that was fogged for a snapshot, or unseen for more than
    15 ticks, is re-initialised silently instead of reporting the gap as one number. Health
    bars also follow the render tick, so the bar, ghost and number change together.
  - **Ability slots:** a radial cooldown sweep with a bright leading edge, remaining seconds
    (tenths under one second), a blue tint when mana is short, mana cost and a gold frame when
    ready. Item cooldowns use the sweep too.
  - **Screens:** a procedural loading screen (gradient, light bands, glowing title, spinner,
    map name, tip) for at least 1.2 s after connecting or a map change, then a cross-fade;
    a dip-to-dark fade between lobby, match and results; lobby and results panels fade and
    slide in. `MOBA_CAPTURE_TICK` captures settle all fades.
  - **Also:** the kill feed moved under the controls line (it overlapped it), the map name
    sits under the timer (it was under the status panel), and `MOBA_DEMO_CAST=<slot>@<tick>`
    casts once for capture demos.
  - **Public API:** `UiVertex` (GPU layout, exhaustive) grew from 36 to 48 bytes: `atlas` is
    now `mode` (`UiVertex::MODE_*`), plus `outline` and `params`. `UiDrawList::text` draws
    the MSDF font with new advances; new `text_styled`, `text_outlined`, `text_bitmap`,
    `bitmap_text_width`, `rect_gradient`, `sweep` and `ring`; new `TextStyle`
    (`#[non_exhaustive]`, `new` + `with_outline`/`with_shadow`) and `pack_rgba8`.
  - **Golden images:** both fingerprints were **deliberately rebaselined** (HUD text and
    health bars) on lavapipe with validation and sync validation required, 0 messages.
  - **Cost:** RTX 5070 Ti at 1920×1080: the `ui` pass stays at 5–6 µs for the normal HUD
    (5 µs before). A 2× HUD with the scoreboard and shop open goes from 38–41 µs to 49–52 µs
    p50. Steady-state HUD frames still allocate nothing (`docs/perf/0014-ui.md`).
  - No wire, replay, logic or content-hash change.
- **Render graph, HDR and tonemap (AAA roadmap R1a, ADR-0008/ADR-0009):** the frame is now a
  small in-house render graph (`moba-render` `graph`). Passes declare image and buffer
  accesses, and compiling the graph derives `vkCmdPipelineBarrier2` barriers and layout
  transitions, pools transients with disjoint lifetimes, and culls passes whose outputs are
  unused. The logic is pure and unit-tested headless. The graph is compiled once at creation
  and recorded without allocating. The frame runs `scene` (terrain, units and skinned meshes
  into an HDR target plus a sampled-capable depth buffer), then `composite` (exposure and an
  AgX tonemap with a saturation look, into the swapchain), then `ui` (the HUD, not tonemapped),
  plus `capture` for readbacks. The HDR target is `R16G16B16A16_SFLOAT`, or
  `B10G11R11_UFLOAT_PACK32` on Low. Per-pass GPU timestamps come from one query pool per frame
  in flight, created at init; devices without timestamps report `None`.
  - **New public API (additive):** `QualityTier` (`Low`/`Medium`/`High`/`Ultra`,
    `#[non_exhaustive]`), `RenderConfig::with_quality_tier` and `with_exposure`,
    `Renderer::gpu_timings` (returns `GpuTimings`/`PassTiming`), `gpu_timing_supported`,
    `quality_tier` and `hdr_format_name`, and `RenderError::RenderGraph`.
  - **Client:** a `quality` setting (`low`/`medium`/`high`/`ultra`, default `high`) with a
    `MOBA_QUALITY` override. `MOBA_FRAME_LOG` gains `gpu_scene`, `gpu_composite`, `gpu_ui`,
    `gpu_total` and `gpu_span` p50/p95 columns, plus `quality=`/`hdr=`.
  - **Golden images:** both fingerprints (`render-fingerprint.rgb`,
    `render-network-fingerprint.rgb`) were **deliberately rebaselined** for the tonemap on
    lavapipe, with validation and sync validation reporting 0 messages.
  - **Cost:** RTX 5070 Ti at 1920×1080: +16–21 µs of GPU per frame, almost all of it the
    fullscreen composite. Barriers between passes add under 1 µs. See
    `docs/perf/0011-render-graph.md` for why the roadmap's ≤ 5 % relative criterion cannot hold
    on a 13–30 µs frame.
  - `renderer.rs` shrinks from 1,872 to about 900 lines; the pass code moves to
    `crates/render/src/passes/`.
  - No wire, replay, logic or content-hash change.
- **Mesh loading (#286, ADR-0007):** bounded `.glb` loader (`moba-assets` feature `glb`, fuzzed), `Renderer::register_mesh` with per-mesh instanced draws (`UnitInstance.mesh`, `MeshId::CUBE` default), and client hero models from `MOBA_ASSET_ROOT`/`MOBA_HERO_MODELS` that fall back to the procedural rig on any failure; ships the generated `content/models/sentinel.glb`.
- **GPU skinning (#288, ADR-0007):** the `.glb` loader reads `JOINTS_0`/`WEIGHTS_0`, the skin (≤ 64 joints, inverse bind matrices) and one animation clip under the same untrusted-input caps (fuzzed); `Renderer::register_skinned_mesh` draws skinned instances with `skinned.vert`, a per-frame bone SSBO (set 1 binding 1, default 16 × 64 joints) and a `{first_bone, joint_count}` push constant on the shared pipeline layout. The client poses heroes from facing/gait/render tick with a preallocated bone pool (no new wire fields; over-budget heroes draw static); `MOBA_NO_SKINNING=1`, a missing skin or a failed pipeline fall back to the static mesh or the procedural rig. Ships the generated `content/models/sentinel-skinned.glb` and `content/sentinel-skinned-pack.toml`, which names it for Blade and Guardian through the pack's `model` field.
- **Networked animated golden and frame cost (#289):** `MOBA_CAPTURE_TICK` captures a networked frame at a pinned match tick, and a `MOBA_SERVER=local` server lock-steps to the client, so the frame is byte-repeatable. The HUD hides FPS, ping and packet counters in capture mode. `render-golden.yml` gains a second fingerprint, `tests/golden/render-network-fingerprint.rgb`, with skinned heroes. Also adds `MOBA_WINDOW_SIZE`, a network-mode `MOBA_WARMUP_TICKS`, and frame-log `cpu_*`/`render_*`/`extent`/`gpu` fields. `MOBA_CAMERA` is now read once. Fixes the UI vertex upload reading back from write-combined memory, which cost about 7 ms per frame with the networked HUD. RTX 5070 Ti at 1920×1080 with 10 skinned heroes: p95 4.192 ms (display-paced), against 8.3–8.5 ms before.
- **Per-map visuals (#287, WP-3.3):** a map's `visuals` name resolves on each client to a
  bounded TOML descriptor, `<MOBA_ASSET_ROOT>/maps/<name>.toml` (`moba_assets::MapVisuals`:
  terrain tint, wall colour and height, an optional wall mesh and up to 32 props with optional
  `.glb` meshes). It replaces the client's hard-coded `map_theme` table, which stays as the
  fallback for a missing or invalid descriptor (a missing mesh falls back to the built-in block
  for that part); each problem is logged once and never fails a match. The local preview now
  draws the map's walls, from the same wall data the network path uses. Visuals rebind on
  connect, `ContentUpdate` and map rotation without restarting the client.
  `MOBA_CAPTURE_MAPS=<prefix>` writes one capture per map in a network session. Ships
  `content/ember-map.toml` and `content/frost-map.toml` with descriptors in `content/maps/` and
  the generated `content/models/crystal.glb` and `content/models/rampart.glb` (`sample-mesh`).
  `AssetManifest::check_files` (content-check) now requires map entries to be `.toml`
  descriptors and validates them and the meshes they name; unit entries stay `.glb` only.
  No wire, hash, replay or logic change: only the visuals *name* is hashed and sent (#285);
  descriptors are client-local and cosmetic. The default and networked golden scenes are
  unchanged (no visuals name, no shipped `map-*` descriptor).
- **Animation runtime (R4a, ADR-0012):** the `.glb` loader keeps every animation as a named
  clip (≤ 32 clips, ≤ 262,144 keys per skeleton, ≤ 8,192 animation channels per file, joint
  names; `gltf`'s `names` feature, no new dependency; the glb fuzz target samples every clip
  and drives the animator). `MAX_JSON_BYTES` rises from 256 KiB to 1 MiB so a rig at those
  caps fits. New in
  `moba-assets`, pure CPU and allocation-free: `Pose`, `BoneMask`, `Skeleton::sample_clip`
  (loop/clamp; linear, slerp or step keys), `Pose::blend` (crossfade, shortest-path nlerp,
  optional mask) and `Pose::add` (additive layers), `BlendSpace1d`, and a content-defined state
  machine: the bounded client-local descriptor `<MOBA_ASSET_ROOT>/animations/<model>.toml`
  (`AnimationSet`, bound to a skeleton as `AnimationGraph`) and the per-unit `Animator`
  (release-aligned triggers on an absolute clock, crossfades, upper-body actions while moving,
  additive hit reactions, death hold/respawn, fixed-step spring bones, sockets with
  `Animator::socket_transform`). The network client drives one animator per hero from the
  interpolated speed, `EVENT_CAST` (slot or catalogue ability name), `EVENT_UNIT_DIED`/the dead
  flag, health drops (hit reactions) and inferred basic attacks (the wire has no attack event; documented gap), draws dead
  heroes while their death clip plays, and reduces far heroes' update rate
  (`MOBA_ANIM_LOD=off|<near>,<far>`, default `80,140`). A missing or invalid set keeps the
  WP-4.1 pose, logged once; `MOBA_NO_ANIMATION_SETS=1` is the off switch. `Skeleton::sample`
  now plays the clip named `run` or `walk` (else the first, as before). The regenerated
  `content/models/sentinel-skinned.glb` has 10 joints (a 3-joint spring cape) and idle, run,
  attack, cast1–3, death, recall and hit clips, with `content/animations/sentinel-skinned.toml`.
  200 animated units cost 0.12 ms of CPU per frame (`docs/perf/0012-animation-runtime.md`). No
  wire, hash, replay or logic change; the networked golden still matches (mean diff 0.014).
- **Attack hit-frame sync (R4b-1, ADR-0012):** the network client drives basic-attack swings
  from the new `EVENT_ATTACK` (protocol v41, below) instead of inferring them from health
  drops: the attack state's release frame lands on the tick the hit landed and the hero turns
  to its target, for melee and ranged attackers alike (the simulation resolves both
  instantly). The next swing is scheduled one attack interval ahead from the replicated attack
  timer, so it plays its whole wind-up, and is withdrawn if no attack event confirms it. The
  health-drop inference, the catalogue-range heuristic and its two-hit cadence check are
  gone; damage from abilities, burns or another attacker no longer starts a swing.

### Determinism and balance
- Added `crates/sim/tests/mirror.rs`, a side-swap tolerance mirror (WP-2.1, #290): five scripted 1v1 duels run on `seed` and, x-mirrored with slots swapped, on `seed ^ 1`; per tick, hero positions match within 2,048 ULPs and health, mana, cooldowns, xp, gold, score and slot-keyed events match exactly.
- `match-soak` now counts `unique_hashes` with `state_hash_normalized()`, and adds `--threads N` and a `balance:` line (draws, Wilson 95 % blue win rate over decided matches, resolution-tick percentiles). A 5,000-match, 60,000-tick sweep measured blue at 44.21 % (95 % CI 42.84–45.60 %), a red bias traced to the point-reflected wave formation (`docs/perf/0008-balance.md`, #290).
- After the v8 fixes (below), the same sweep measures blue at **49.81 %** (95 % CI 48.42–51.20 %), so the bias is gone. `match-soak` now reports `cap_draws` and `same_tick_draws` separately (2 of 5,000 matches were same-tick draws) and `peak_units`. Peak units were 183 of `MAX_UNITS` 512, so wave starvation never occurs.

### Simulation
- **Recall** (AAA roadmap R4 exit; **simulation logic v9**). `PlayerCommand::Recall` starts an
  8 s channel (`rules.recall_ticks`, default 240 ticks; `0` disables recall) that teleports
  the hero to its team's spawn (`MapSpec::hero_spawn` for its slot) and clears its orders.
  The channel is cancelled by an obeyed move order (not while rooted), an attack order, stop,
  a cast that goes off, any damage left after armour or magic resist (a shield absorbing
  it still cancels), a stun, and death. Slows, roots, silences, buying, using an item and
  surrendering do not cancel it. Recalling again while channelling has no effect (it does not
  restart); dead or stunned heroes cannot start one. New `SimEvent::RecallStarted { hero,
  team, position, duration_ticks }` and `SimEvent::RecallEnded { hero, team, position, end }`
  with `RecallEnd::{Cancelled(RecallCancel), Completed { destination }}` and
  `RecallCancel::{Order, Cast, Damage, Stun, Death}` (all `#[non_exhaustive]`),
  `ObservedUnit::recalling`, `Simulation::recall_remaining(slot)`, and a hashed `Recall`
  component (state hash tag 31). Replays keep format **v5** (the command is replay tag 7, as
  `UseItem` was added as tag 6 without a version bump), but logic-v8 replays are rejected by
  this build. The golden state hash is now `0x59e062c6037e76ac` (every hero carries the new
  hashed component); the fixture is `crates/sim/tests/fixtures/seed-v9.mbr` (state hash
  `0x88a26326d20d5c41`), and the teamfight replay was re-recorded (same events, state hash
  `0x8945433b3adbd4ce`). Bots do not recall.
- **`SimEvent::BasicAttack { attacker, team, target, interval_ticks }`** is emitted when a
  basic attack lands (R4b-1). Events are outputs: no state, hash, replay or logic change
  (golden state hash `0x8db35a0f5aab5190` and `seed-v8.mbr` unchanged). Attacks fill at most
  `MAX_ATTACK_EVENTS` (half of `MAX_EVENTS`) of the event queue, so they never crowd out other
  events. `SimEvent` is `#[non_exhaustive]`, so the variant is additive; matches need a `_`
  arm already.
- **Simulation logic v8** (#290). Replays recorded with logic v7 are rejected by this build;
  play them back with a logic-v7 build (0.4.0 or `main` before this change). The checked-in
  replay fixture is now `crates/sim/tests/fixtures/seed-v8.mbr`. The golden state hash is
  unchanged (`0x8db35a0f5aab5190`): its scenario runs without waves and never ends the match.
  - Minion wave formations are x-mirrored between the teams, like hero spawns. Red's
    formation offset used the same `perpendicular()` as blue's, which made the two formations
    point reflections: in the top lane blue's ranged minion spawned ahead of the lane start and
    red's behind it (the reverse in the bottom lane). This biased bot matches towards red.
  - Win conditions met in the same tick (nexus destroyed, score limit reached, surrender) are
    resolved together at the end of the tick. If both teams meet one, the match is a **draw**
    instead of being decided by processing order (a same-tick double nexus used to go to red).
  - API: new `moba_sim::MatchOutcome { Winner(Team), Draw }`. `Simulation::outcome()` and
    `ReplayResult::outcome` return `Option<MatchOutcome>`, and `SimEvent::MatchEnded::winner`
    is `Option<Team>` (`None` for a draw).

### Content and networking
- **Protocol v42** (not wire-compatible with v41): recall.
  - `WireCommand::Recall` (input tag 8). The input decoder also gained the missing arm for
    `WireCommand::UseItem` (tag 7): before v42 the server rejected every item active a
    client sent as a bad value.
  - `ENTITY_FLAG_RECALL` (bit 7 of the entity flags, `NetEntity::is_recalling`) is set while a
    hero channels. It travels with the entity, so fog of war filters it like the rest of the
    entity, and it survives lost events.
  - `EVENT_RECALL` (9), a full 26-byte event: `value` is `RECALL_STARTED` (`aux` = the channel
    length in ticks), `RECALL_CANCELLED` or `RECALL_COMPLETED` (`x2`/`y2` = the spawn it
    arrived at); `x`/`y` is where the hero channelled. A start is sent only to viewers who see
    the hero in that tick; a cancel or completion only to viewers who saw it in their previous
    snapshot (where it stood still on `x`/`y`), whether or not they see it now: it may have
    teleported to its spawn, or a dash that cancelled it may have carried it out of the fog,
    in that tick. An enemy recalling out of vision is never reported, not even to a viewer
    watching its spawn. Priority: with casts and zones.
  - `WireRules::recall_ticks` (u32) carries the content rule in the catalogue.
- **Content:** `[rules] recall_ticks` (0..=900, i.e. up to 30 s, the longest effect the
  client plays; default 240). It is hashed and signed only
  when it is not the default, so every existing pack keeps its content hash and attestation.
- **Protocol v41** (not wire-compatible with v40). Snapshots carry **`EVENT_ATTACK`** (8), one
  per visible basic attack (R4b-1): `source`/`target` net ids and hero slots, `team`,
  `value` = the attacker's attack interval in ticks and `aux` = the hit's age, so the hit
  landed on snapshot tick `tick - aux`. It is a compact 12-byte record (no position,
  direction or arc; those decode as zero) next to the 26-byte form of the other events,
  filtered per viewer like them, and has the lowest priority under the 16-event cap (deaths,
  structures and the match end; other events; hero attacks; other attacks). The server keeps
  attacks in their own pending queue so a burst never evicts a death or a cast; when full, a
  non-hero attack is evicted first. Attack and death events also reach a viewer whose
  previous snapshot had the endpoint, so a killing blow on a minion (despawned that tick) or
  an enemy hero (dead, so out of vision) is sent to the killer; before, last hits and
  kill-feed deaths of those units were filtered out. The client no longer re-delivers the
  events of a duplicate or late snapshot, and queues a snapshot's events with its
  `ClientEvent::Snapshot` marker or not at all (attacks dropped first; the event queue grows
  from 32 to 68 entries). `value` is static per unit today; attack-speed modifiers would make
  it cooldown information needing a visibility review. Measured cost:
  +0.2 KiB/s per client in a bot teamfight soak (7.0–7.1 → 7.2–7.3 KiB/s) and within noise in
  the 10-player soak; worst case 5.6 KiB/s with every snapshot's cap full of attacks
  (`docs/perf/0009-presentation-events.md`). `fuzz-wire` and the cargo-fuzz `wire` target now
  generate events of every kind and check that whatever decodes re-encodes identically;
  `soak` prints received events and attacks per client.
- **Protocol v40** (not wire-compatible with v39, which #314 took). Packs can reference
  visual assets (#285, WP-3.1):
  - Optional `[assets]` manifest (`[[assets.units]]` / `[[assets.maps]]`, the
    `moba_assets::AssetManifest` shape) in the pack, plus `heroes.model` and `map.visuals`
    names (1..=16 of `[A-Za-z0-9_-]`; `visuals` defaults to the theme's `asset_name()`). New
    `HeroSpec::model`, `MapSpec::visuals` / `visuals_name()` and `GameContent::assets`.
    `moba-game` now depends on `moba-assets` (no `glb` feature, render-free). The new public
    fields (also `WireHero::model`/`model_len`, `WireMap::visuals`/`visuals_len`) break
    exhaustive struct literals and patterns.
  - Loading rejects names with no manifest entry (`ContentLoadError::UnknownAsset`), invalid
    manifests (`ContentLoadError::Assets`) and malformed names
    (`ContentError::BadVisualName`). Asset paths now also reject `\`, `:` and control bytes.
    These are new enum variants, which breaks exhaustive matches.
  - `content-check` checks every manifest file (`.glb`, exists under the pack directory, no
    symlink escape, unit models parse) and reports `assets` and `errors` in text and `--json`.
    Load failures are now JSON too with `--json`. Any error exits with status 1.
  - The names are hashed, signed and carried: `WireHero::model` and `WireMap::visuals`
    (16 bytes + length each). The `[assets]` paths are pack-local and never hashed or sent.
    The hash appends a tagged visuals block only when a name is set, so every shipped pack,
    the default content, replays and signatures are byte-identical (pinned by a test).
  - The client draws `HeroSpec::model` / `WireHero::model` from its local asset root.
    `MOBA_HERO_MODELS` is now a per-hero dev override. `MOBA_SERVER=local` plays
    `MOBA_CONTENT` when set. Example pack: `content/sentinel-pack.toml` (Guardian uses
    `models/sentinel.glb`).
- **Protocol v39** (not wire-compatible with v38): the snapshot `outcome` byte gains
  `3` = draw (`OUTCOME_DRAW`; `OUTCOME_NONE/BLUE/RED` name the existing codes) and
  `EVENT_MATCH_ENDED` carries `value = MATCH_ENDED_DRAW` (2) for a draw. The byte layout is
  unchanged, but a v38 client would have shown a draw as a defeat. The HUD shows **DRAW**, the
  kill feed "Match drawn", and the results log, `replay --json` and `moba-server` report
  `"draw"`.
- **Protocol v38** (not wire-compatible with v37). The remaining hashed `[rules]` fields are now
  carried in the handshake: `starting_gold`, `gold_per_interval`, `gold_tick_interval`,
  `xp_radius`, `shop_radius`, `shared_xp_percent`, `[rules.rewards.*]`, `structure_heroes_first`,
  `nexus_win` and `score_limit` (#294).
  - Previously `WireCatalog::to_content` substituted `RulesSpec::default()` for them, so a pack
    that overrode any of them produced a different client-side content hash and the client
    dropped the Welcome.
- Added `content/economy-override.toml`; `cargo test -p moba-net --test content` joins a real
  UDP client against it and compares the client's rules and content hash with the server's.
- Added a per-field wire parity test covering every hashed `RulesSpec` field, plus exhaustive
  `RulesSpec` destructures in `feed_catalogue` and `From<&GameContent> for WireCatalog` so a
  future hashed field cannot be added without being hashed and carried.
- The wire fuzzers now round-trip a full built-in catalogue and Welcome, covering every rules
  field's encoder/decoder path.
- Multi-match flood isolation (#292): datagrams are charged against the per-address, per-match and new optional host-wide (`--host-max-pps`, `HostBudget`) budgets before decoding, so undecodable junk counts too; `MatchHost` polls round-robin with a per-match cap of 64 datagrams / 500 µs per pass; `--reload-content` loads the pack once per host (`ContentWatch`) and shares it across matches; `load-test --flood-port K --flood-pps N` verifies the isolation. `--max-pps 0` now disables only the per-address budget, not the per-match one.
- `GameContent::validate` now rejects map geometry the wire catalogue would silently truncate
  (#296): a fractional or out-of-`i16`/`u16` extent, spawn, lane waypoint, structure position
  or wall endpoint (new `ContentError::BadMapCoordinate(field)`; adding a variant is a breaking
  change for exhaustive matches), more than 4 lanes / 16 waypoints / 32 structures, empty lane
  paths (which panicked the wave spawner), and over-long or non-ASCII map/lane names.
  `NetServer::bind_with_content` now validates its pack (`ErrorKind::InvalidInput`), and the
  client validates the Welcome catalogue as it already did for `ContentUpdate`. No wire-format
  change (still protocol v38); shipped pack hashes are unchanged. New `MapSpec::wire_point` /
  `MapSpec::wire_extent` and `MAX_LANE_NAME` are shared by validation and the wire conversion.

### Internal
- Split `crates/game/src/content.rs` into `content/{spec,schema,load,validate,hash,manifest,error}.rs`
  (pure move; public paths, content hashes, signatures and the golden state hash are unchanged).
- Split `moba-net`'s `protocol.rs` and `server.rs` into `protocol/` and `server/` module directories (pure move; public paths, wire format v38 and behaviour unchanged).
- Split `crates/sim/src/sim.rs` (4,846 lines) into a `sim/` module directory (spawn, status,
  inputs, abilities, movement, combat, deaths, items, bots, vision, hashing, tests); pure move,
  no behaviour, API or state-hash change.

### CI
- Added the informational `semver (informational)` CI job and `scripts/semver-check.sh` (`just semver`) (WP-7.1): `cargo-semver-checks` diffs the library crates against the latest release tag. It currently reports the `moba-net` struct-field additions since v0.4.0, so the next release must be 0.5.0.

## [0.4.1] — 2026-10-27

Security patch release cut from `v0.4.0` on the `release/0.4.x` branch. Protocol **v37**, replay
**v5**, simulation logic **v7**: wire-, replay- and content-compatible with 0.4.0, and no public
API change. Lockfile-only dependency updates.

### Security
- `ctutils` 0.4.3 → 0.4.6 ([RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385)).
  Before 0.4.4, `ctutils::Choice` mask generation could compile to a branch on common targets,
  and `u128` comparisons branched on 32-bit targets, which can leak secret values through timing.
  `ctutils` is used by the RustCrypto stack under `moba-net`: `sha2`/`hmac`/`hkdf` (per-client
  tokens, key derivation), `curve25519-dalek`/`ed25519-dalek` (server/client identity) and
  `poly1305`/`chacha20poly1305` (session AEAD). Also updated in `fuzz/Cargo.lock`.
- `tokio` 1.54.0 → 1.54.1 ([RUSTSEC-2026-0384](https://rustsec.org/advisories/RUSTSEC-2026-0384);
  1.54.0 was yanked). Only reached through `winit`'s wasm path; native builds do not run it.

## [0.4.0] — 2026-10-21

Security release. Protocol **v37** (not wire-compatible with 0.3.x), replay **v5**, simulation logic **v7**. Content hashes and replays are unchanged.

### Security
- **GHSA-jw7c-q9x9-cgg3:**
  - Reconnects and re-handshakes are bound to the credential that opened the slot: the same token/PSK, and in identity mode the same client key.
  - Session ids are random.
  - Keyed handshakes answer an address-bound server challenge, which the Hello authenticator (`moba-hello-v2`) covers, so a captured Hello can't be replayed to another server or match.
  - Hello rejects are sent unsealed, and connected clients only act on a `Disconnect` for their own session.
- The client fails closed on malformed `MOBA_SERVER_PUBKEY`, `MOBA_SECRET` and `MOBA_CLIENT_IDENTITY`.
- The server validates allow-listed and rotated identity keys at startup.
- Server and client identity signatures use `verify_strict`.
- AEAD keys are zeroized again.

### Platform and toolchain
- Vulkan **1.3** is the minimum. The 1.2 extension fallback is removed (ADR-0002 amended), and shaders target `vulkan1.3`.
- Rust MSRV **1.88**. The CI check is now named `MSRV`.
- Dependencies are at their latest releases:
  - RustCrypto/dalek: chacha20poly1305 0.11, hmac/hkdf 0.13, sha2 0.11, ed25519-dalek 3, getrandom 0.4. Wire and signature compatibility is proven by known-answer tests.
  - glam 0.34, toml 1.1, gpu-allocator 0.28, cpal 0.18, criterion 0.8.
  - GitHub Actions bumped to their current majors.

### Client
- Audio fixes:
  - Tones play to completion across device buffers.
  - Voices mix instead of queueing.
  - The audio callback no longer allocates.
  - Only the stream for the device's sample format is opened.

### Content
- The built-in content's manifest version is now `BUILTIN_CONTENT_VERSION` ("0.3.0") instead of the crate version. Engine releases no longer change the default content hash, so existing default-content replays stay valid.

### Tooling
- `just bench-baseline` / `bench-compare` now pass criterion flags only to bench targets.
- The docs consistency check also covers CLAUDE.md and docs/agents.md.

## [0.3.0] — 2026-10-20

Preview release after the 2026-10-19 audit remediation. Protocol **v36**, replay **v5**,
simulation logic **v7**.

### Security
- Hardened session transport (independent AEAD counters, replayed-packet rejection, keyed peers
  reject unsealed application packets/fragments).
- Replaced the weak pack digest with domain-separated SHA-256 attestation, strict signature
  verification and a unified trusted loader.
- Hardened content parsing/hashing, catalogue updates and client validation.
- Closed the public unsafe job-scope lifetime issue with scoped ownership, blocking waits and a
  Loom model.

### Simulation
- Deterministic targeting tie-breaks, symmetric fixed-point division, team-size-agnostic bot
  seeds and fast-projectile swept collision.
- Rules-driven starting/passive gold, radii, shared XP, unit rewards, aggro and win conditions.
- Item actives/cooldowns, on-hit/on-damaged passives, HUD cooldowns and bot item use.
- Generic modifier stat bonuses, replicated status bits, data-driven expiry/on-tick/on-damaged
  triggers and a Stun modifier kind.
- Replay decode memory cap and seed-normalized state hash for cross-seed comparisons.

### Networking
- Reliable catalogue delivery with generation checks, acknowledgements, retries and
  connected-client map rotation.
- Snapshot/event allocation guards, transport send-buffer reuse and per-thread allocation
  accounting.

### Renderer
- Capture extent/format snapshots, corrected present/atlas barriers and validation suppression
  removal.
- Setup-failure RAII for buffers, images, samplers and swapchains; resize skip on out-of-date
  rebuilds.
- Correct inverse-transpose normals and explicit texture LOD.
- Downloadable/known-issue validation now counts every error.

### Client
- HUD split into scoreboard/shop/abilities modules; centralized theme colors.
- Input routing, NetworkView, scene-build and launch modules extracted; `app.rs` under 800 lines.
- Procedural audio formats, mute, device recovery, bounded lock-free scheduling and offline
  sound.
- Networked hero rig with interpolated facing/gait; zero-allocation HUD frame and UI-click
  routing tests.

### Content and tooling
- Validated asset manifests with safe paths, duplicate checks and fallback chains.
- Stable map-theme asset keys and map-rotation coverage.
- Fuzzed catalogue decoding; checked replay/fixture and golden-state rebaselines.

### Documentation
- Completed the audit remediation plan and closed tracking issues.
- Replaced the duplicated plan text in this changelog; refreshed status, release and API docs.
- Recorded a 15,000,000-tick determinism sweep.

### Deferred
- Asset-pipeline integration, GPU skinning and remaining mirror/balance coverage are tracked
  post-1.0 in [#276](https://github.com/JakeGT/moba-engine/issues/276).

## [0.2.0] — 2026-10-18

Preview release with runtime content packs and protocol/content hardening.

- Runtime TOML item/hero/ability/map packs, manifests, signed pack attestation and catalogue
  updates.
- Map-driven world extent, arcing projectiles, rules for respawn/wave cadence and per-map
  themes.
- Prediction/interpolation, reconnect, spectators, bot policy seam and dense net-id tables.
- Golden-image renderer gate, replay v5, allocation/soak/fuzz evidence and macOS/Linux CI
  coverage.
- Protocol v16–v25; see the `v0.2.0` tag for generated PR notes.

## [0.1.0] — 2026-10-18

First preview tag.

- Deterministic 30 Hz simulation, ECS/jobs runtime, Vulkan renderer, UDP netcode, server and
  client binaries.
- Golden state hash, replay format v5 and initial release workflow.
- Protocol v13–v15; see the `v0.1.0` tag for generated PR notes.

[0.3.0]: https://github.com/JakeGT/moba-engine/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/JakeGT/moba-engine/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/JakeGT/moba-engine/releases/tag/v0.1.0
