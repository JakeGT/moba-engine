# ADR-0013: VFX runtime

**Status:** accepted 2026-10-29 (R5 of the [AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r5--vfx-system-l)).
Distortion and heat haze (roadmap R5 item 2, High only) are not part of this decision: they
moved to R6 with the other full-screen post-processing ([Follow-ups](#follow-ups)).
**Amended** 2026-10-29 after review (same PR): stable event order, projectile exemptions and
fallback cubes, focus-based culling, a transactional slot allocator, telegraphs drawn hard,
an OIT range clamp and no depth sampling on portability-subset devices.

## Context
Until R5 the only effects were cube placeholders for projectiles and unit tinting. The
presentation events a VFX system needs already reach the client — casts, projectile spawns and
hits, deaths, structures, zones ([perf/0009](../perf/0009-presentation-events.md)) and basic
attacks (protocol v41, [ADR-0012](0012-animation-runtime.md#basic-attacks-event_attack-r4b-1))
— and R4a added animation sockets as attach points. R5 needs GPU particles with sprites,
flipbooks, trails, beams and mesh particles, data-driven from content, inside a hard per-tier
budget that holds in a 10-hero teamfight, without hiding what players must read.

Constraints from the roadmap (§2, §3) and the engine's invariants:

- **Never touch determinism.** Effects are client-side presentation driven by presentation
  events and snapshots. They never feed the simulation, the wire, replays or the content hash;
  their randomness is a client-only RNG stream.
- **No per-frame allocation** in the render loop; every buffer is sized at load.
- **Readability first:** VFX never hide enemies, ally VFX can be dimmed, enemy telegraphs
  stay legible.
- **Every feature is tiered** and has an off switch; Low must hold on the min-spec GPU.
- Effect files are client-local and untrusted, like models, map visuals and animation sets.
- R1's render graph ([ADR-0008](0008-render-graph.md)) owns ordering and barriers; the scene
  depth is already sampled-capable for this.

## Decision

### GPU particles: a compute simulation over a fixed-capacity buffer
The renderer owns one particle buffer of the tier's `VfxBudget::max_particles` (Low 4,096,
Medium 12,288, High 24,576, Ultra 49,152) 96-byte records, allocated at creation and zeroed
once. A particle is position, velocity, age and lifetime, start/end size, spin and drag,
start/end colour (packed half floats), gravity, a shape (billboard, segment, ground quad,
mesh), a blend mode, a readability role and an atlas cell range.

- **Spawning is CPU-described, GPU-applied.** The application hands the frame's
  `ParticleSpawn`s to `Scene::with_vfx`. The renderer gives each one a free slot from a CPU
  slot allocator — per-slot expiry times on a clock that sums the frames' `dt` exactly as the
  GPU ages particles, scanned from a ring cursor at most 64 slots per spawn — and uploads the
  records to a per-frame-in-flight host-visible buffer (`max_spawns_per_frame`, Low 512 …
  Ultra 4,096). A spawn without a free slot is dropped and counted: that is the hard cap, and
  live particles are never overwritten. Spawns past the per-frame cap drop in list order, so
  the client lists the ones that must not drop (enemy telegraphs and projectiles) first.
  Each frame's clock step and allocations are a transaction, committed when the frame is
  submitted and rolled back at the next `prepare` otherwise, so a frame that fails after
  `prepare` cannot push the CPU clock ahead of the GPU's or hold slots for particles that
  were never uploaded.
- **`vfx_simulate` (compute)** runs three dispatches with compute → compute barriers between
  them: copy spawns into their slots and reset the counters; age, integrate (gravity, drag)
  and kill every slot, appending live ones to compacted draw lists with atomics (sprites in
  one list, mesh particles in four per-mesh-slot lists of `capacity / 8`) and accumulating
  their projected pixel area; then write the indirect draw arguments, clamped to each list's
  capacity. It finally copies the 128-byte argument/statistics block to a host-readable
  buffer, read after the frame's fence (`Renderer::vfx_stats`: live particles, spawns, drops,
  the overdraw estimate).
- **`vfx_draw`** renders after the opaque `scene` pass into the HDR target: one
  `vkCmdDrawIndirect` for every sprite, flipbook, segment and ground quad (six vertices per
  live particle, expanded in the vertex shader), and one `vkCmdDrawIndexedIndirect` per bound
  mesh slot. The scene depth is attached **read-only** (`DEPTH_STENCIL_READ_ONLY_OPTIMAL`,
  `loadOp` LOAD, `storeOp` NONE) for early depth rejection and sampled in the same pass for
  **soft particles** (Medium and up): the fragment fades over 0.6 m of linear depth, the
  linear depth recovered from the projection's `P22`/`P23` terms. The scene pass stores depth
  only when this pass exists. Without soft particles (Low) the shader never samples it and the
  sampled slot names the atlas instead. **`MoltenVK`** (any `VK_KHR_portability_subset`
  device) gets no soft particles on any tier: Metal's support for sampling the read-only
  depth attachment of the same render pass is not something we can test, so the particles
  are depth-tested only there. Enemy telegraphs are drawn hard (no soft fade) everywhere, so
  the fade cannot take them below their opacity floor.
- **Sort-free blending.** Additive particles add to the HDR target. Alpha particles use
  **weighted blended order-independent transparency** (McGuire and Bavoil 2013) on Medium and
  up: an `RGBA16F` accumulation target and an `R16F` revealage target. Revealage is stored as
  the sum of `-ln(1 - α)`, so every attachment blends with the same `(ONE, ONE)` state and
  `independentBlend` is not required; `vfx_resolve` composites `exp(-sum)` over the HDR target.
  Colours are clamped to 64 and the weighted accumulation to 60,000, inside half-float range
  (weights reach 3,000).
  Low has no extra targets and no resolve: alpha particles blend unsorted and premultiplied,
  and additive ones write alpha 0, so one `(ONE, ONE_MINUS_SRC_ALPHA)` state draws both.
- **Synchronisation through the graph.** The graph gains four access kinds — compute storage
  read, compute storage read/write, indirect read, and read-only depth + fragment sampling —
  with headless barrier tests, and a `persistent_buffer` import state: the particle, list and argument
  buffers persist across frames on the one queue, so each frame's first barrier orders against
  every earlier frame's compute, vertex, indirect and copy work on them. The spawn upload is
  host-written per frame slot; the stats copy has a final host-read access. The graph's limits
  rise to 12 passes and 24 resources (a `u32` resource mask). The frame becomes
  `(portraits, minimap) → vfx_simulate → (shadow) → scene → vfx_draw → (vfx_resolve) →
  composite → ui` (the offscreen passes from R7b, the shadow pass from R2a), all before the
  composite in the batch that does not wait on the acquire semaphore.

### Primitives
Sprites (an atlas cell), flipbooks (consecutive cells over the lifetime or at a frame rate),
**trails** (one camera-facing segment per frame from the anchor's previous to its current
position, tapering by `size_end`; projectiles), **beams** (a segment from the source to the
target re-emitted every frame while the effect runs; dashes), **rings** (quads flat on the
ground; shockwaves, telegraphs) and **mesh particles** (a registered static mesh, fitted to a
unit cube at load, tumbling about a per-slot axis and lit by the key light; up to four mesh
slots, `Renderer::set_vfx_mesh`). The atlas is an 8×8 grid of RG cells (coverage, hot core);
the renderer generates a procedural 512×512 one at creation
(`moba_assets::procedural_atlas`), and the client replaces it with
`<root>/vfx/atlas.ktx2` through the KTX2 path ([ADR-0010](0010-texture-pipeline.md)), shipped
as BC5 from the same generator.

### Data-driven effects bound to events and sockets
Effects are client-local TOML in the asset root, resolved by name like map visuals and
animation sets; nothing is hashed, signed or sent, so there is **no wire, replay, content-hash
or simulation change**.

- `vfx/bindings.toml` maps catalogue ability names to `cast`, `projectile`, `hit` and `zone`
  effects (plus a cast socket and a tint), and unit events to `hero_death`, `minion_death`,
  `structure_destroyed` and `attack`, with defaults for unbound abilities. A mapping file was
  chosen over `vfx/<ability-name>.toml` because ability names contain spaces and several
  abilities share one effect with a tint.
- `vfx/<effect>.toml` holds up to eight emitters (primitive, blend, sprite, anchor, burst, rate,
  delay/stop, lifetime, speed and direction, spawn shape, size and colour over life, team tint,
  gravity, drag, spin, width, fps, mesh, telegraph). Parsers are bounded (16 KiB per effect,
  32 KiB of bindings, 128 bindings), reject unknown keys and range-check every number; a
  `vfx` cargo-fuzz target covers both. Missing or invalid files are logged once and play
  nothing — never a failed match.
- The client's `VfxSystem` keeps a fixed pool of 256 effect instances. Network events are
  stamped with the tick of the snapshot that carried them and released, in arrival order,
  when the interpolated view reaches it, so an effect plays with the units it belongs to and a
  projectile follows the cast that fired it. Projectile and zone events take their ability
  from the owner's most recent cast (they carry none on the wire); the projectile instance
  keeps it, and its hit plays that ability's hit effect even if the owner cast something else
  meanwhile. Projectile effects fly their own extrapolated anchor (with the visual arc) until
  the hit event lands. `source` anchors follow the caster, or the binding's **animation socket**
  (`Animator::socket_transform` × the hero's model-to-world transform). The local view
  (preview, `MOBA_REPLAY` playback) feeds the same system from `SimEvent`s.

### Budgets, LOD and readability
- **Hard caps** in the renderer: the particle capacity and the per-frame spawn cap.
- **Soft budgets** in the client: an expiry histogram (128 buckets of 0.1 s, no per-particle
  state) estimates live particles and their projected pixels. Spawns of `low` effects stop
  at 50 % of the particle or overdraw budget (Low 1.5 … Ultra 8 screens of overdraw), `normal`
  at 80 %, `high` at 95 %.
- **Distance and visibility culling:** an effect farther than the tier's `max_distance`
  (90–240 m) from the camera's focus on the ground — scaled up by the camera's distance past
  the default 46 m, so a zoomed-out spectator camera culls proportionally — or off screen,
  spawns nothing that frame but keeps its timeline.
- **Enemy telegraphs** (`telegraph` emitters of an enemy's effect) are exempt from every cull
  and budget and keep at least `telegraph_floor` (35 %) opacity on the GPU until the last 15 %
  of their life. **Enemy projectiles** (every emitter of a projectile effect that is not an
  ally's or the viewer's own; for spectators every projectile) are exempt the same way: a
  skillshot is gameplay information. Exempt spawns go first in the frame's list and may use
  the whole per-frame cap; the others stop 256 spawns (or a quarter of the cap) short of it.
- **Projectiles never vanish:** a projectile whose effect did not resolve, or that spawned no
  particle in the last 0.1 s (an ally's culled or over budget), draws the pre-VFX cube.
  A bindings file that is missing or invalid turns effects off entirely (logged once), and
  the client then draws the cubes as before R5.
- **Allied effects** (a teammate's, not the viewer's own) are scaled by the allied-VFX
  opacity setting on the GPU, and their alpha particles are capped at 75 % opacity so they
  never fully cover an enemy. Spectators get no ally/enemy rules.
- **Off switches:** `RenderConfig::vfx` / the `vfx` setting / `MOBA_VFX=off` remove the passes
  entirely; a device whose graphics queue has no compute gets none.

### Authoring
`MOBA_VFX_PREVIEW=<effect>` runs the client as an effect previewer: an empty arena, a stand-in
caster and target, the effect replayed whenever it ends, reloaded within half a second of a
change to any file in `vfx/`. `moba-tools teamfight` records the deterministic scripted 5v5
teamfight replay used as the evidence scene, and `MOBA_REPLAY=<file.mbr>` plays any replay
in the local view with every effect.

## Alternatives considered
- **CPU-simulated particles drawn as instances.** Simple and debuggable, but the CPU cost and
  upload bandwidth scale with live particles (50k × 96 bytes is ~4.6 MiB per frame) instead of
  spawns, and the instance cap is shared with units. Rejected: GPU simulation only uploads
  what is new.
- **A GPU free list (dead-list + atomics) instead of the CPU slot allocator.** The textbook
  design, with no CPU knowledge of slot state. It needs an extra counter round trip and makes
  the cap and drop counting asynchronous. Since spawns are CPU-described anyway, the CPU knows
  every lifetime exactly; the allocator is a few dozen instructions per spawn and drops are
  counted the frame they happen. A ring that overwrites the oldest slot was rejected because it
  can kill a long-lived telegraph.
- **Sorted alpha particles** (bitonic sort on the GPU). Correct for every blend, but a sort
  per frame of the live set costs more than the whole pass on min-spec, and still sorts per
  particle, not per pixel. WBOIT is order-independent at two cheap targets; Low drops even
  those.
- **Classic WBOIT revealage with `(ZERO, ONE_MINUS_SRC_COLOR)` blending.** Needs
  `independentBlend` (different blend state per attachment). Storing `-ln(1 - α)` additively
  gives the same product with one blend state.
- **Soft particles without a depth attachment** (sampling depth only, discarding behind
  geometry). Loses early depth rejection and costs fill rate; read-only depth gives both.
- **A third-party particle crate or middleware.** None targets raw `ash` with our graph and
  budgets; the system is about 5,900 lines, tests and shaders included.
- **Effects in the hashed content pack.** Server-validated and identical for everyone, but
  every cosmetic change would invalidate replays and need a protocol path for files the
  server never uses. Client-local like every other visual (finish-plan Phase 3).

## Consequences
- Evidence ([perf/0016](../perf/0016-vfx.md)): the scripted teamfight replay
  (`tests/replays/teamfight-5v5.mbr`, every built-in ability cast, verified by a test) renders
  validation- and sync-validation-clean on lavapipe and the RTX 5070 Ti on every tier, with
  per-pass GPU timings including the VFX passes and the overdraw p95.
- The frame has up to three more passes and two more screen-sized targets (Medium and up).
  The resolve runs every frame on those tiers even when no alpha particle is live (a
  fullscreen pass; measured in perf/0016).
- The goldens are unchanged: no effect is live at the pinned golden frames.
- Public API additions (all additive): `ParticleSpawn`, `ParticleShape`, `ParticleBlend`,
  `ParticleRole`, `VfxFrame`, `VfxBudget`, `VfxStats`, `VFX_MESH_SLOTS`, `Scene::vfx` /
  `with_vfx`, `RenderConfig::vfx` / `with_vfx`, `QualityTier::vfx_budget`,
  `Renderer::{vfx_available, vfx_budget, vfx_stats, set_vfx_atlas, set_vfx_mesh}`,
  `RenderError::Vfx`; in `moba-assets` the descriptor types (`VfxEffect`, `VfxEmitter`,
  `VfxBindings`, …) and `procedural_atlas`.

## Follow-ups
- **Distortion / heat haze** (High only): **moved to R6** with the rest of the full-screen
  post-processing. It needs a copy of the HDR target before the particle pass and a refraction
  permutation (another screen-sized target and pass), which belongs with R6's bloom chain and
  its own tier switch rather than inside R5's budget.
- Lights from effects (fireball glow on the ground) belong to R2's clustered lights.
- Projected decals for telegraphs (R3) would replace ground quads on uneven terrain.
- Per-viewer fog of war for effects: today the server already withholds hidden enemies'
  events, so an effect never reveals more than the event stream does.
- GPU-side culling of whole effects and a GPU free list if spawn rates ever exceed what the
  CPU allocator handles.
