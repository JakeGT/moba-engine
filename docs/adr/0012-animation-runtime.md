# ADR-0012: Animation runtime

**Status:** accepted 2026-10-28 (R4a of the [AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r4--animation-runtime-ml-after-wp-41)).
Vertex-animation textures for minions and renderer-side bone LOD are R4b and are not decided
here beyond the direction in [Follow-ups](#follow-ups-r4b-and-later).
**Amended** 2026-10-28 by R4b-1: basic attacks are driven by the replicated `EVENT_ATTACK`
(protocol v41), no longer inferred from health drops ([below](#basic-attacks-event_attack-r4b-1)).
**Amended** 2026-10-29 by R4b: vertex-animation-texture minions, renderer bone-count LOD,
visible socket attachments, a 2D blend space and the local preview on the same runtime
([Amendment: R4b](#amendment-r4b-vat-minions-bone-lod-attachments)). No wire, replay,
simulation or content-hash change.

## Context
WP-4.1 ([ADR-0007](0007-mesh-format.md#skinning-wp-41)) made heroes GPU-skinned with one clip:
the client blends the rest pose to a walk clip by gait. R4 needs heroes that idle, run, attack
on the simulation's beat, cast, die, recall, react to hits and have secondary motion, plus
attach points for weapons and VFX (R5), at a cost that holds for 200 skinned units.

Constraints from the roadmap (§3) and the engine's invariants:

- **Never touch determinism.** Animation is client-side presentation driven by snapshots and
  presentation events. It never feeds the simulation, the wire, replays or the content hash.
  Root motion is visual only.
- **No wire change** in R4a. What the client already received had to be enough; the gap
  (basic attacks) was documented, and closed with its own protocol bump in R4b-1.
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
| facing while standing | the last snapshot movement step (≤ 3 m, so a respawn teleport is ignored), or towards the target of the last `EVENT_ATTACK` | the WP-4.1 pose snapped stationary heroes to +X |
| hit reaction | the hero's health dropping between snapshots | at most every 9 ticks (0.3 s), so damage over time does not jitter |
| basic attacks | `EVENT_ATTACK` (since v41: source, target, hit tick as `snapshot tick − aux`, attack interval in `value`) | release frame on the hit tick; the next swing is scheduled one attack interval ahead and withdrawn if no event confirms it |
| recall | none | the simulation has no recall; the state and `AnimationEvent::Recall` exist for when it does |

Snapshots are interpolated `SNAPSHOT_DELAY_TICKS` (4 ticks, 133 ms) behind the newest one, so
an event stamped with its tick is known before the view reaches it and the release frame can
land exactly on it; a wind-up longer than that is partly skipped unless scheduled ahead.

### Basic attacks: `EVENT_ATTACK` (R4b-1)
The simulation resolves a basic attack instantly in the tick the attacker's cooldown reaches
zero (`Simulation::resolve_attacks`); there is no wind-up in the simulation, so release and
hit are the same tick, for melee and ranged attackers alike (no basic attack spawns a
projectile). `SimEvent::BasicAttack { attacker, team, target, interval_ticks }` is emitted
with the hit. Events are outputs: they are not hashed or recorded, so the golden state hash
(`0x8db35a0f5aab5190`), the replay fixture and `SIM_LOGIC_VERSION` are unchanged. Attacks may
fill at most half of the simulation's 256-event queue (`MAX_ATTACK_EVENTS`), so they never
crowd out a death or a cast.

On the wire (protocol v41) `EVENT_ATTACK` = 8 is a compact 12-byte record: kind, team,
source and target net ids, their hero slots, `value` = the attack interval in ticks (the
attacker's timer: when it can hit again) and `aux` = the hit's age in ticks relative to the
carrying snapshot. It is filtered per viewer like the other events (both endpoints observed,
or a spectator), with one addition for attacks and deaths: an endpoint the viewer had in its
previous snapshot also counts, because a killing blow's victim is despawned (a minion) or out
of vision (a dead enemy hero) by the snapshot that carries it. Without that, last hits and
kill-feed deaths never reached the killer. It has the lowest priority under the
16-events-per-snapshot cap: deaths, structures and the match end, then the other events, then
hero attacks, then the rest. The server keeps attacks in their own bounded pending queue, so
a burst never evicts other events; when that queue is full a non-hero attack is evicted
first. The client delivers a snapshot's events only if it is newer than the last one whose
events it delivered (a duplicate or late datagram would replay them), and queues a
snapshot's events together with its `ClientEvent::Snapshot` marker or not at all, dropping
attacks first, so a stall never mis-stamps an event's tick.

`value` is the attacker's static attack interval today (`AttackStats::interval` never
changes during a match). If attack-speed modifiers are added it becomes cooldown
information about enemy units and needs a visibility review.

The client triggers `AnimationEvent::Attack` with its release on the hit tick and turns the
hero to its target. Because the event arrives only `SNAPSHOT_DELAY_TICKS` before the view
reaches the hit, shorter than the sample clip's 0.35 s wind-up, the next swing is scheduled
from the attack timer (hit + interval) so it gets its whole wind-up. An attack event within
two ticks of the scheduled hit confirms it (the animator ignores the repeat); one further
away replaces it; none by one tick after it withdraws it (`Animator::cancel`: dropped if
still queued, else faded out from the view's time). The swing after an attacker's last
hit is therefore shown as a wind-up that fades out; a lost snapshot can also withdraw a
swing that did happen. Bandwidth: [perf/0009](../perf/0009-presentation-events.md#protocol-v41-basic-attack-events).

### Gaps (documented, not patched)
- **Basic attacks** were inferred from health drops in R4a (a stationary hero with a
  damaged enemy in its catalogue range), which misattributed damage from abilities, burns or
  a second attacker. Closed by `EVENT_ATTACK` in R4b-1 (above); the inference is gone.
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
[perf/0012](../perf/0012-animation-runtime.md): one sample-shaped unit (10 joints, locomotion
blend, upper-body action, additive hit, 3 spring joints, a socket) evaluates in ~0.6 µs; 200
units take ~116 µs per frame, ~31 µs with a quarter of them evaluated per frame, against a
1 ms animation budget. The steady state allocates nothing (`moba-assets` test
`animation_allocations`, client test `steady_state_driver_frames_do_not_allocate`).

## Amendment: R4b (VAT minions, bone LOD, attachments)

Written before the code, 2026-10-29. Everything here is client-local presentation like the
rest of this ADR: nothing is hashed, signed, replicated or replayed; protocol v41, replay v5,
logic v8 and the golden state hash are unchanged. Every feature is tiered and has an off
switch; nothing allocates per frame.

### Vertex-animation textures for minions
Minions are many (up to ~150 alive in a late 5v5) and need few states, so they do not get an
`Animator` and a bone range each. Their clips are **baked offline** into a vertex-animation
texture (VAT) and played in the vertex shader from two numbers per instance.

- **Baker.** `moba-tools vat-bake <model.glb> <out>` samples every chosen clip of a skinned
  model at a fixed rate (default 20 fps; frames are interpolated in the shader) through the same `Skeleton::sample_clip` /
  `skinning_matrices` path as the runtime, skins every vertex on the CPU (4 influences, the
  shader's formula) and writes `<out>.ktx2` plus a `<out>.toml` sidecar descriptor.
- **Texture.** One `RGBA16F` KTX2 (`VK_FORMAT_R16G16B16A16_SFLOAT`, one level, linear) per
  model. Texel `i = frame × vertices + vertex` holds the skinned model-space position (`w = 1`);
  texel `frames × vertices + i` holds its normal. Rows wrap at the texture width (≤ 4,096), so
  the height is `ceil(2 × frames × vertices / width)` (≤ 4,096). Half floats give ~2 mm at
  4 m from the origin, below a pixel at gameplay zoom. Frames are fetched with `texelFetch`
  (no filtering), so wrapping is free; the shader interpolates between frames itself.
- **Descriptor** (`<asset root>/vat/<name>.toml`, ≤ 8 KiB, unknown keys rejected,
  range-checked like an animation set): the source `model` (whose static mesh is drawn),
  `vertex_count`, `frame_count`, `width`/`height`, `fps`, a 64-bit FNV-1a `mesh_hash` of the
  model's positions and indices (a re-exported model without a re-bake is detected and falls
  back), and ≤ 16 clips (`name`, `first`, `frames`, `loop`). An optional `[playback]` table holds
  the run speed (the locomotion clip's rate scales with ground speed), the attack release time
  and the crossfade time. `moba_assets::VatDescriptor` parses it and checks it against the
  loaded texture and mesh (`check_texture`, `check_mesh`); `VatAnimation::bind` then resolves
  the clips the player uses. The renderer re-checks the texel fit before upload. Texels are
  capped at `MAX_VAT_TEXELS` so the file fits `MAX_KTX2_BYTES`, and the baker range-checks
  `[playback]` so it never writes a descriptor the client rejects.
- **Draw path.** `Renderer::register_vat_mesh(mesh, texture, layout)` uploads the static
  mesh and the VAT image (vertex-stage combined image sampler, nearest) with its own
  descriptor set. VAT meshes are drawn **instanced, one draw per VAT mesh**, by `vat.vert`
  (which feeds the existing `unit.frag`), after the skinned draws; a `SHADOW_PASS`
  permutation (`vat_shadow.vert`) casts into the shadow map. No bone buffer is read. The VAT
  pipelines use their own layout whose set 2 is the VAT set (sets 0/1 stay compatible; set 3,
  lighting, is re-bound), because Vulkan only guarantees four bound sets. A VAT mesh does not
  take a textured material (the minion look is vertex colour and team tint).
- **Per instance.** `UnitInstance::with_vat(VatPose)` sets two frame samples (`frame`,
  `next`, `t`) and a crossfade weight between them (the previous clip fading into the current
  one). The instance block grows from 80 to 112 bytes (two `vec4`s that only `vat.vert`
  reads). The shader clamps both frames below `frame_count` and the vertex below
  `vertex_count` (the push constant), so no fetch leaves the texture.
- **What drives it.** `moba_assets::VatPlayer` (fixed size, no heap): idle ↔ run by the
  interpolated speed (the run clip's rate follows speed / `run_speed`), attack from
  `EVENT_ATTACK` (its release frame on the hit tick, as for heroes), death from
  `EVENT_UNIT_DIED`. The client's `MinionDriver` keeps a fixed table of 1,024 players indexed by net id and a
  64-entry corpse ring: a minion is despawned in the snapshot that reports its death, so
  the corpse is drawn at its last position for the death clip plus a 0.6 s linger. Facing is
  the last snapshot step, or towards the attack target.
- **Tiers.** `Low`: nearest frame, no crossfade (2 fetches per vertex). `Medium`: frames
  interpolated, no crossfade (4). `High`/`Ultra`: interpolated and crossfaded (8). All tiers
  cast shadows when shadows are on.
- **Fallback and off switch.** A minion kind without a usable VAT asset (melee tries
  `vat/minion-melee`, then `vat/minion`; ranged `vat/minion-ranged`, then `vat/minion`) is drawn
  as today's cube. A missing, oversized, malformed or mismatched asset is logged once and
  falls back. `MOBA_VAT=off` draws every minion as a cube.

### Bone-count LOD in the renderer
On top of `MOBA_ANIM_LOD` (update rate), distant and off-screen heroes draw **reduced
skins**: fewer joints in the bone buffer and fewer influences per vertex.

- `moba_assets::SkinLod::build(mesh, max_joints, max_influences)` keeps the joints carrying
  the most skinned weight (with every ancestor of a kept joint, so the hierarchy closes),
  re-points each vertex's influences at its nearest kept ancestor, merges duplicates, keeps the
  largest `max_influences` (sorted, renormalised to 255) and compacts the joint palette. It
  returns the reduced mesh and the palette map; `SkinLod::gather` copies the kept joints'
  skinning matrices out of the animator's full set. Collapsing a joint into its ancestor is
  exact for rigidly bound parts and makes secondary parts (the cape) follow their parent.
- Levels: LOD0 full; **LOD1** ≤ 2 influences and ≤ 75 % of the joints (spring-chain joints are
  dropped first); **LOD2** 1 influence and ≤ 4 joints. Each level is its own skinned mesh
  registration, made at load.
- Selection per hero per frame, by distance from the eye and visibility (the same
  `LodView` as the update-rate LOD). Off-screen heroes (still shadow casters) use LOD2. The
  distances are per tier: `Low` 40/80 m, `Medium` 60/110 m, `High` 80/140 m, `Ultra` 110/180 m.
  A pinned capture uses LOD0. `MOBA_BONE_LOD=off` keeps LOD0; `MOBA_BONE_LOD=<near>,<far>`
  overrides the distances.
- `skinned.vert` skips influences whose weight is zero (an exact no-op for the full mesh),
  so 1- and 2-influence skins do less GPU work.

### Visible socket attachments
A socket may name a client-local static `model` and a `scale`; the client loads it like a
hero model (`<asset root>/models/<model>.glb`, cached, logged once on failure) and draws one
extra instance per attached socket, at `instance transform × socket transform × scale`, in
a brightened tint of the hero's colour (so they read against the body). Attachments follow
the update-rate LOD (they use the animator's last sockets); on `Low` they are skipped for
heroes on bone LOD2, that is beyond the far distance or off screen. `MOBA_ATTACHMENTS=off`
hides them. The sample set attaches a small `crystal` to the sentinel's spear tip and one to
its head.

### 2D blend spaces
`BlendSpace2d` places ≤ 8 clips on a plane (strafe `x`, forward `y`, m/s in the unit's
facing frame) and weights them by gradient-band interpolation (Johansen 2009), which needs
no triangulation and gives exact weights at the sample points. An animation set uses it when
its `[[locomotion]]` points give `x`/`y` instead of `speed`. `Animator::evaluate_motion`
takes the local velocity; `Animator::evaluate` (speed only) passes `(0, speed)`. Heroes face
their movement, so the shipped sets stay 1D.

### The local preview on the same runtime
The local bot preview (no `MOBA_SERVER`) feeds the same `AnimationDriver` (and minion
driver) as a network match: each simulated tick becomes a `SnapshotState` of the units
(net id = entity index + 1, quantised exactly like the wire) and its `SimEvent`s
(`BasicAttack`, `AbilityCast`, `UnitDied`) become the `NetEvent`s the server would send, then
the driver's `on_snapshot` / `flush_events` run as in a match. This is a local adapter: the
wire, the server and the simulation are untouched. Anything that drives the local preview's
simulation (`MOBA_REPLAY`, R5) gets the same animation. The local VFX anchors use the same
net ids, so local effects attach to the animated heroes' sockets as in a match.

### Exit criterion
The simulation had no recall when R4b landed, so the recall state stayed bound but
untriggered; the amendment below adds it.

## Amendment: recall (R4 exit; protocol v42, simulation logic v9)
Recall is a simulation feature (an 8 s channel, `rules.recall_ticks`, that teleports the hero
to its spawn; cancelled by orders, casts, damage, stun and death) replicated two ways, both
read by the driver:

- **`EVENT_RECALL`** (9) marks the edges: `RECALL_STARTED` triggers
  `AnimationEvent::Recall` stamped with the carrying snapshot's tick (the `recall` state
  loops), and `RECALL_CANCELLED` / `RECALL_COMPLETED` withdraw that trigger with
  `Animator::cancel(.., at = start, stop_at = tick)`, so the loop fades out (`fade_out`) from
  the tick the channel ended. The driver keeps each hero's start tick for that.
- **`ENTITY_FLAG_RECALL`** in every snapshot of the channel. Events are not retransmitted, so
  a hero seen recalling with no start (a lost event, or one that walked into vision
  mid-channel) starts the loop on that snapshot's tick, and one no longer recalling ends it.
  Over the R4 exit run (5 % loss) the completion event of one recall was lost and the flag
  ended the loop on the same tick.

Fog of war comes from the server: the flag is part of the entity (only sent while the viewer
sees the hero), a start is sent only to viewers who see the hero, and a cancel or completion
only to those who saw it in their previous snapshot, standing on the channel spot the event
reports (seeing it now is not enough: at its spawn, or where a dash put it, it would reveal
a spot the viewer never saw).

The recall effect (ADR-0013) follows the same flag: it starts when the start event was lost,
and stops when the flag clears, the hero leaves the snapshot or dies (a killer never gets the
cancel of the hero it killed), as well as on the events.

**Teleports.** The interpolator used to lerp every step, so a recall (or a respawn) slid the
hero across the map for one tick and the driver saw a run speed and a facing towards the
spawn. `moba_net::interpolation::is_teleport` now treats the step between two snapshots as a
teleport when a living hero's recall flag cleared (a completion; a cancel moves it at most a
tick's walk or a dash) or a dead hero came back alive: the earlier position holds until the
later snapshot's tick, then the hero appears at the new one. It reads those replicated
signals rather than a distance (an earlier draft snapped steps over 10 units plus 0.5 per
tick), so a content pack's long dash or fast hero still interpolates. The network scene ignores such a step for the hero's speed and facing, so
the recall loop is not cut short by `interrupt_on_move` and the hero does not turn on
arrival. The own hero's prediction already snapped corrections over 8 units; it stops for a
recall only when the flag confirms the channel, so a recall that never starts does not
rubber-band it.

The local preview and replays map `SimEvent::RecallStarted` / `RecallEnded` to the same
events, and the flag comes from `ObservedUnit::recalling` through `NetEntity::from_observed`.
Evidence: [perf/0024](../perf/0024-recall.md).

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
- **Adding an attack event to the wire in R4a.** Exact, but a protocol bump with
  bandwidth/visibility review belonged in its own change; inference kept R4a wire-neutral.
  R4b-1 made that change.
- **Replicating the attack cooldown in `NetEntity`** instead of an event (R4b-1). It gives the
  timer but not the target, costs bytes on every changed hero update, and a hit would still
  have to be inferred from the cooldown resetting.
- **A wind-up event ahead of the hit** (R4b-1). The simulation has no wind-up phase; an event
  some ticks before the cooldown expires would be a forecast that can still fail (the target
  leaves range or dies), so the client would need a second, confirming event per attack.
- **Spring bones on the GPU or physics middleware.** A handful of joints per hero does not
  justify a compute pass or a dependency; the CPU cost is in the numbers above.
- **VAT for heroes too.** VAT suits crowds with few states (minions); heroes need blending,
  masks, springs and sockets, which VAT cannot express.

## Follow-ups (R4b and later)
- **VAT minions** (R4b): bake minion clips into vertex-animation textures and play them in the
  vertex shader; needs R1's texture and material support and a second skinned path.
- **Bone LOD in the renderer**: reduced skins (fewer joints/influences) for distant units.
- ~~**`EVENT_ATTACK`** for exact hit-frame sync~~: done in R4b-1 (protocol v41).
- Socket attachments drawn through the instanced mesh path; item visuals.
- Stride matching (locomotion playback rate from speed) and a 2D blend space if strafing is
  ever needed.

## Consequences
- `Skeleton` holds every clip of a file; memory is bounded by `MAX_SKELETON_KEYS`.
- Animation sets are per model and per client, like models: two clients can animate the same
  hero differently, by design.
- The sample `sentinel-skinned.glb` grew to 10 joints (a 3-joint cape) and 9 clips; the
  networked golden still matches (mean diff 0.014, max 7 on lavapipe, validation required).
