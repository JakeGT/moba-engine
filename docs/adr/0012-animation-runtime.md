# ADR-0012: Animation runtime

**Status:** accepted 2026-10-28 (R4a of the [AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r4--animation-runtime-ml-after-wp-41)).
Vertex-animation textures for minions and renderer-side bone LOD are R4b and are not decided
here beyond the direction in [Follow-ups](#follow-ups-r4b-and-later).

## Context
WP-4.1 ([ADR-0007](0007-mesh-format.md#skinning-wp-41)) made heroes GPU-skinned with one clip:
the client blends the rest pose to a walk clip by gait. R4 needs heroes that idle, run, attack
on the simulation's beat, cast, die, recall, react to hits and have secondary motion, plus
attach points for weapons and VFX (R5), at a cost that holds for 200 skinned units.

Constraints from the roadmap (§3) and the engine's invariants:

- **Never touch determinism.** Animation is client-side presentation driven by snapshots and
  presentation events. It never feeds the simulation, the wire, replays or the content hash.
  Root motion is visual only.
- **No wire change.** What the client already receives has to be enough; a gap is documented,
  not patched with a protocol bump inside this phase.
- **No per-frame allocation**, and every feature has an off switch.
- Model and animation files are client-local and untrusted (a player or a mod controls them),
  like `.glb` models and map visuals.
- Another workstream is rebuilding the renderer (R1). R4a must not depend on it.

## Decision

### An in-house sampler and blender in `moba-assets`
Pure CPU code with no GPU dependency, next to the skeleton types it works on:

- **Clips.** The `.glb` loader keeps every animation that drives a joint, as one named
  `AnimationClip` each (`gltf`'s `names` feature, which adds no dependency). Caps, checked
  before reserving: ≤ 32 clips (`MAX_CLIPS`), ≤ 4,096 keys per track, ≤ 65,536 per clip and
  ≤ 262,144 per skeleton (`MAX_SKELETON_KEYS`), ≤ 600 s per clip, and ≤ 8,192 animation
  channels in the file (`MAX_ANIMATION_CHANNELS`: every one of 64 joints animated in
  translation, rotation and scale in all 32 clips, plus 2,048 skipped channels of slack;
  more is `SkinError::ClipTooLarge`). A rig at those caps needs about 350 KiB of compact
  channel JSON, so the JSON chunk cap rises from 256 KiB to 1 MiB (the transient
  parsed-document worst case scales with it, to the order of 100 MiB at load time); a unit
  test loads a rig at the caps. Clip and joint names are
  1–32 of `[A-Za-z0-9_-]`; other names are dropped (the clip stays reachable by index), not
  rejected. Duplicate names resolve to the first. The glb fuzz target now samples every clip
  and drives the animator over whatever loads.
- **Sampling.** `Skeleton::sample_clip(clip, time, Loop | Clamp, &mut Pose)`: keys interpolate
  linearly (translation, scale) or by slerp (rotation), or step. A `Pose` is an inline
  `[JointTransform; 64]`, so poses live in preallocated scratch.
- **Blending.** `Pose::blend` (lerp; rotation by shortest-path normalised lerp, cheaper than
  slerp and indistinguishable at blend weights), optionally through a `BoneMask` (a `u64`, one
  bit per joint; `BoneMask::subtree` builds the upper body from one joint). `Pose::add` layers
  an additive pose as its difference from a reference (the clip's first frame).
  `BlendSpace1d` places up to 8 clips along one parameter: locomotion uses idle at 0 m/s and
  run at the run speed.
- `Skeleton::model_transforms` and `skinning_matrices` replace the fused path for the runtime;
  `Skeleton::sample` (rest → locomotion clip by weight) stays as the WP-4.1 fallback. It now
  plays the clip named `run` or `walk` if there is one, else the first clip, so a single-clip
  WP-4.1 file behaves exactly as before.

### A content-defined state machine
A client-local TOML **animation set**, `<asset root>/animations/<model>.toml`, describes one
skinned model ([format](../content-authoring.md#animation-sets)). Like `MapVisuals` it is
bounded (≤ 16 KiB, unknown keys rejected, every number range-checked, ≤ 16 states, ≤ 16
ability bindings, ≤ 4 spring chains with ≤ 16 joints, ≤ 8 sockets) and cosmetic: it is not
hashed, signed, replicated or replayed. It names:

- the locomotion blend space and `move_speed`, above which upper-body actions leave the legs
  to locomotion;
- action **states**: a clip, a mode (`once`, `loop`, `hold`), a layer (`full`, `upper`,
  `additive`), fade-in/fade-out times, a playback speed, a **release** time (the frame the
  triggering event marks: the hit of an attack, the release of a cast) and whether moving
  interrupts it;
- **triggers**: attack, cast slots Q/W/E, ability names (which win over the slot), death,
  recall and hit reaction;
- spring chains and sockets (below).

`AnimationGraph::bind` resolves every name against the skeleton once at load, so the frame
path compares no strings except a cast's ability name. A missing, oversized, malformed or
unresolvable set is logged once and the model keeps the WP-4.1 pose; it never fails a match.
`MOBA_NO_ANIMATION_SETS=1` is the off switch.

`Animator` is the per-unit runtime, about 8 KiB of fixed-size state (8,240 bytes; the
shared `AnimationScratch` is 9,264):

- **Absolute clock.** `evaluate(now, speed, model_to_world)` takes the render tick in seconds,
  not a frame delta. Locomotion phase, action progress and fades are pure functions of that
  clock and the trigger stamps, so a pinned capture tick gives the same pose whatever the frame
  pacing before it (the networked golden is byte-repeatable).
- **Release-aligned triggers.** `trigger(event, at)` starts the bound state at
  `at − release / rate`, so its release frame lands on `at`. A start in the future waits in a
  4-entry queue; one in the past starts part-way. An attack faster than its clip speeds the
  clip up to fit the attack interval. Re-triggering the playing or queued state within 0.1 s is
  ignored, so a predicted swing is not restarted when the real hit is observed.
- **Crossfades.** One current and one previous action. The previous is dropped once the
  current has faded in if the current covers it (a `full` action, or the same layer);
  otherwise (an `upper` cast after a full-body slam while running) the joints the current
  does not drive fade the previous out over its own `fade_out`, so the legs do not snap. `once` fades out after its last frame, `hold` (death) stays until respawn,
  `loop` (recall) runs until replaced or interrupted by movement. One additive slot (hit
  reactions) layers on top. Death clears the queue and ignores everything but respawn.
- `report(now, speed)` exposes the chosen action, previous action, weights, clip time and
  locomotion blend for tests and tools.

### What drives it (the client)
`moba-client`'s `AnimationDriver` keeps up to 32 preallocated animators keyed by net id and
feeds them only what the client already has:

| Input | Source | Notes |
|---|---|---|
| idle ↔ run | interpolated position at the render tick and 2 ticks earlier | both ends interpolated, also for the own (predicted) hero, so the speed depends only on the render tick |
| casts | `EVENT_CAST` (`value` = ability slot; the catalogue maps slot → ability name) | stamped with the tick of the snapshot that carried the event (`ClientEvent::Snapshot` follows its events) |
| death | `EVENT_UNIT_DIED` (target net id), else the dead flag | the dead hero stays drawn while the death clip plus `death_linger` play |
| respawn | the dead flag clearing | `HeroRespawned` is not replicated; a hero first seen already dead (mid-match join, reconnect, reload) is marked dead without replaying its death |
| facing while standing | the last snapshot movement step (≤ 3 m, so a respawn teleport is ignored), or towards the target of the last inferred attack | the WP-4.1 pose snapped stationary heroes to +X |
| hit reaction | the hero's health dropping between snapshots | at most every 9 ticks (0.3 s), so damage over time does not jitter |
| basic attacks | **inferred** (see Gaps) | release frame on the inferred hit tick; predicted one interval ahead once two hits confirm the cadence |
| recall | none | the simulation has no recall; the state and `AnimationEvent::Recall` exist for when it does |

Snapshots are interpolated `SNAPSHOT_DELAY_TICKS` (4 ticks, 133 ms) behind the newest one, so
an event stamped with its tick is known before the view reaches it and the release frame can
land exactly on it; a wind-up longer than that is partly skipped unless predicted.

### Gaps (documented, not patched)
- **No attack event or attack timer on the wire.** Basic attacks resolve instantly in
  `Simulation::resolve_attacks` when `AttackStats::cooldown` reaches 0, and neither the hit nor
  the cooldown is replicated (`NetEntity` has ability cooldowns only). The client infers a hit
  when a stationary hero has an enemy within its catalogue attack range (+1.5 m) whose health
  dropped in this snapshot, at most once per catalogue attack interval. It misattributes damage
  from abilities, burns or a second attacker. A predicted swing whose hit has not been
  observed one tick after its release tick is withdrawn (`Animator::cancel`): dropped if
  still queued, else faded out from where the interpolated view is, which is usually still
  in the wind-up. Exact sync needs a presentation event
  (`EVENT_ATTACK` with source and target, filtered like the others) or the attack cooldown in
  `NetEntity`. Either is a protocol bump and is left as a follow-up (proposed in the R4a pull
  request, to be filed as an issue).
- **No respawn or recall events**, as above.
- **The local preview** (no `MOBA_SERVER`) keeps the WP-4.1 pose; the runtime is wired to the
  network view, which is what matches and the golden use.

### Secondary motion: spring bones
Chains of parent → child joints named in the set (≤ 4 chains, ≤ 8 joints each, ≤ 16 total).
Each joint's bone tip is a Verlet particle in world space (so moving the unit makes the cape
trail) with stiffness towards the animated tip, damping, gravity and a length constraint, at a
fixed 1/60 s step (≤ 8 steps per evaluation; a gap over 0.5 s restarts from the animated pose).
The particle bends the joint by the shortest arc, inline in the parents-first transform pass,
so each child follows its corrected parent. Deterministic given the sequence of evaluations;
`Animator::reset_springs` removes the history, which a pinned capture does every frame.

### Sockets
Named attach points on joints with an offset and rotation (≤ 8 per set).
`AnimationGraph::socket(name)` resolves one at load; `Animator::socket_transform(id)` returns
its model-space transform from the last evaluation, which the caller multiplies by the
instance transform. This is the query R5's VFX will use. Drawing attached meshes is left to a
follow-up: it needs a mesh name per socket and a second instance per hero, and the renderer
frame is being rebuilt in R1.

### LOD
`MOBA_ANIM_LOD` (default `80,140`): heroes within 80 m of the camera eye are evaluated every
frame, up to 140 m every 2nd frame, beyond or off-screen every 4th; skipped frames reuse the
animator's last matrices. `MOBA_ANIM_LOD=off` evaluates everything. A pinned capture bypasses
the LOD. Bone-count LOD needs per-LOD skins and is R4b.

### Cost
[perf/0011](../perf/0011-animation-runtime.md): one sample-shaped unit (10 joints, locomotion
blend, upper-body action, additive hit, 3 spring joints, a socket) evaluates in ~0.6 µs; 200
units take ~116 µs per frame, ~31 µs with a quarter of them evaluated per frame, against a
1 ms animation budget. The steady state allocates nothing (`moba-assets` test
`animation_allocations`, client test `steady_state_driver_frames_do_not_allocate`).

## Alternatives considered
- **A third-party animation runtime** (an engine's animation module such as Bevy's or
  Fyrox's, or bindings to the C++ ozz-animation). Each brings its own engine/ECS model or a
  C++ build, a second skeleton representation next to `Skeleton`, and its own file loading
  outside the bounded `.glb` loader; the allocation and determinism properties above would
  have to be re-proven against it. The sampler, blender and state machine here are about
  1,800 lines with their docs and reuse `moba-core::math`.
- **Accumulated frame deltas** (the usual `animator.update(dt)`). Simpler for one-shot
  playback but it makes the pose depend on frame pacing, which breaks the byte-repeatable
  networked golden and makes headless tests timing-sensitive. An absolute clock with stamped
  triggers costs nothing extra.
- **State machine in the pack** (`moba-game` content) instead of a client-local file. Network
  clients never see pack files, and anything in the pack is hashed: a cosmetic tweak would
  invalidate replays. Clips belong to a model, so the set is keyed by model name.
- **A graph of arbitrary blend nodes** (Unreal/Unity-style). More flexible, but the MOBA needs
  locomotion + one action layer + additive reactions; a fixed structure keeps the descriptor
  small, bounded and easy to validate.
- **Adding an attack event to the wire now.** Exact, but a protocol bump with
  bandwidth/visibility review belongs in its own change; inference keeps R4a wire-neutral.
- **Spring bones on the GPU or physics middleware.** A handful of joints per hero does not
  justify a compute pass or a dependency; the CPU cost is in the numbers above.
- **VAT for heroes too.** VAT suits crowds with few states (minions); heroes need blending,
  masks, springs and sockets, which VAT cannot express.

## Follow-ups (R4b and later)
- **VAT minions** (R4b): bake minion clips into vertex-animation textures and play them in the
  vertex shader; needs R1's texture and material support and a second skinned path.
- **Bone LOD in the renderer**: reduced skins (fewer joints/influences) for distant units.
- **`EVENT_ATTACK`** (or replicated attack cooldowns) for exact hit-frame sync.
- Socket attachments drawn through the instanced mesh path; item visuals.
- Stride matching (locomotion playback rate from speed) and a 2D blend space if strafing is
  ever needed.

## Consequences
- `Skeleton` holds every clip of a file; memory is bounded by `MAX_SKELETON_KEYS`.
- Animation sets are per model and per client, like models: two clients can animate the same
  hero differently, by design.
- The sample `sentinel-skinned.glb` grew to 10 joints (a 3-joint cape) and 9 clips; the
  networked golden still matches (mean diff 0.014, max 7 on lavapipe, validation required).
