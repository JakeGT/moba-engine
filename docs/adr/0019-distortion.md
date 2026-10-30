# ADR-0019: Distortion and heat haze

**Status:** accepted 2026-10-30 (R6 item 7 of the
[AAA graphics roadmap](../plans/aaa-graphics-roadmap.md#r6--post-processing-and-image-quality-m),
moved there from R5). Written before the code; the costs in [Consequences](#consequences) come
from [perf/0023](../perf/0023-distortion.md). Builds on [ADR-0013](0013-vfx-runtime.md) (GPU
particles, effect descriptors, budgets and readability rules; this is its first follow-up),
[ADR-0015](0015-post-processing.md) (MSAA resolve, render scale and the sub-rectangle),
[ADR-0016](0016-lights-outlines-ao.md) (the prepass's outline ID target) and
[ADR-0017](0017-decals-foliage-water.md) §3, §5–§7 (the `water_copy` → `water` refraction
passes, the closest existing pattern).

## Context
R5 shipped every particle primitive except distortion: a heat shimmer over a burning zone, a
fire ability's haze. It needs the scene behind the particle, so it is a refraction problem,
not a blend: the scene is copied before the effect and the effect samples the copy with an
offset. R3b's full water already does exactly that for the river (`water_copy`, then a
`water` pass that reads the copy by texel at `gl_FragCoord`, rejects offsets that land on
nearer geometry and honours the render sub-rectangle).

Constraints:
- **Presentation only, client-local** (roadmap §3): effects never feed the simulation, the
  wire, replays or a content hash (ADR-0013). No protocol, replay, simulation-logic or golden
  state-hash change.
- **Readability beats realism** (ADR-0011 rules, ADR-0013's legibility rules): distortion
  must never hide or meaningfully displace an enemy unit, a telegraph or a decal.
- **Every feature is tiered** with an off switch; Low must hold on min-spec.
- **The scene's fragment stage is at 16 of 16 samplers** (ADR-0017 §6): distortion may not
  add a sampler to the scene layout.
- **No per-frame allocation and no mid-match pipeline build** (R8's `locked_uploads` and
  `locked_pipelines` stay 0).
- The graph's limits are 32 passes and 48 resources; the largest frame had 29 passes.

## Decision

### 1. Distortion particles: a blend mode of the existing particles
A distortion particle is an ordinary GPU particle (ADR-0013) with a new blend,
`ParticleBlend::Distortion` (`blend = "distortion"` in an effect's emitter). It is spawned,
budgeted, aged and compacted like every other particle — it takes a slot of the tier's
particle buffer, a spawn of the per-frame cap, and its projected area counts in the overdraw
estimate on the CPU and the GPU — but it writes a **screen-space offset, not colour**.

- **Shape.** Billboards only (`primitive = "sprite"`): a heat shimmer faces the camera. Rings,
  segments and meshes are rejected for distortion emitters by the parser.
- **Authored fields** (`moba_assets::VfxDistortion`, all range-checked): `distortion`, the
  strength at birth as a fraction of the tier's pixel cap (`0.05..=1`, required);
  `distortion_end`, the strength at the end of life (`0..=1`, default 0, so the haze fades
  out); `distortion_scale`, noise cells across the sprite (`0.5..=8`, default 2);
  `distortion_speed`, how fast the noise rises, sprite heights per second (`0..=8`, default
  1); `fallback_color`, an optional faint additive sprite for tiers without distortion
  (RGB `0..=4`, alpha `0..=0.5`). A distortion emitter takes no `color`, `color_end`,
  `team_tint`, `mesh`, `width`, `fps` or `telegraph`; those are rejected, so the schema
  stays exhaustive. Unknown keys fail as everywhere else.
- **The offset is procedural.** Two octaves of value noise over the sprite, scrolled upwards
  by `distortion_speed` and seeded by the particle's slot, give a 2D offset in `[-1, 1]²`,
  like the normal of a rising heat column; no texture (and so no sampler) is needed. A baked
  normal texture would be one more sampler and one more asset for a pattern the noise draws
  well at MOBA zoom.
- **Soft edges.** The strength fades (a) radially to zero at the sprite's edge
  (`1 − smoothstep(0.45, 1, r)`), (b) over the first 15 % of the particle's life, (c) against
  the scene depth over 1 m (soft particles, so the haze never shows a hard line where it
  meets the ground), and (d) with view distance from 70 m to 110 m (beyond the default 46 m
  camera's focus; a zoomed-out spectator camera sees no shimmer far away).
- **Encoding.** The particle's start and end colours carry `(strength, scale, speed, 1)`, so
  the GPU interpolates the strength over the life with no new field; bit 8 of the flags word
  marks the particle. The 96-byte record is unchanged.
- **Allied effects** scale the strength by the allied-VFX opacity (as allied colours are);
  distortion emitters are never telegraphs and never exempt from culling or budgets, even
  in an enemy projectile's effect (they carry no gameplay information).

### 2. Passes: copy after the water, refract before the particles
```text
… → scene → (water_copy → water) → (distortion_copy → distortion) → vfx_draw → (vfx_resolve) → …
```
- **`vfx_simulate`** appends distortion particles to a list of their own (a fourth region of
  the `alive` buffer, `capacity / 8` entries, after the four mesh regions) with its own
  `VkDrawIndirectCommand` and counter (the args block grows from 32 to 40 words); they are
  not in the sprite list, so `vfx_draw` never draws them.
- **`distortion_copy`** copies the HDR target — resolved, single-sample (MSAA on Ultra is
  already resolved by the scene pass), the **render sub-rectangle only** — into a
  render-sized transient of the HDR format (`ImageSize::Render { shift: 0 }`).
- **`distortion`** draws the distortion list (one `vkCmdDrawIndirect`) into the HDR target
  with the scene depth attached **read-only** and sampled in the same pass (as the particles
  and the water do), sampling the copy and the outline ID target by texel at `gl_FragCoord`.
  Its viewport and scissor are the render sub-rectangle, and every tap is clamped to it.
  Each fragment writes the copy's texel at `gl_FragCoord + offset`, blended with source
  alpha so overlapping sprites cross-fade, and keeps the destination alpha.

**Why the copy is after the water and before the particles.** The water's copy (taken
before the water pass) would show the river bed without the river under a haze. A copy
after the water and before the particle draw bends the opaque scene, the water and the
foliage, and leaves every particle — and so every telegraph ring and projectile — crisp on
top: the haze bends what is behind the fire, not the fire's own sparks or an enemy's
telegraph. The roadmap's "copy before the particle pass" is this order. The two copies are
separate graph resources; the graph aliases transients with equal descriptions whose
lifetimes do not overlap, so the distortion copy shares the water's refraction image when
there is one (their lifetimes are `water_copy..water` and `distortion_copy..distortion`) or
the OIT accumulation target on tiers without water (`vfx_draw..vfx_resolve`, the same
`R16G16B16A16_SFLOAT` render-sized description). A unit test pins both aliasings.

**Why a direct refraction draw (b) and not an offset target (a).** An offset target (an
`RG16F` screen-sized image the particles add their offsets into, then a fullscreen apply
pass resampling the copy) costs one more screen-sized target, one more pass and a
fullscreen fragment invocation for every pixel every frame, even where no haze is. The
direct draw shades only the haze's own pixels, once per overlapping sprite (cheap: a few
taps each), adds no target beyond the copy and needs no apply pass. Offsets do not sum
across overlapping sprites in (b); with soft edges and the blend that is invisible, and a
cap per fragment is simpler to hold than a cap on a sum. The water chose (b) for the same
reasons.

**Idle frames cost nothing.** The CPU knows every distortion spawn's lifetime (the slot
allocator's clock, ADR-0013): when no distortion particle can be live, the renderer records
neither the copy nor the draw (the graph's barriers still run, so the transient's layout
history is the same every frame). The passes exist in the graph on High and Ultra
regardless, so nothing is compiled or allocated when a haze first appears.

### 3. Depth awareness and readability (the mask)
Per fragment, in `vfx_distort.frag` and its CPU twin `moba_render::distortion::refract_tap`
(unit-tested):
1. **Depth test.** The sprite is depth-tested against the scene depth (read-only), so
   anything in front of the haze is never touched.
2. **Cap.** The offset is `noise × strength × fades × cap`, then clamped to `cap` pixels in
   length. `cap = distortion_pixels × render_height / 1080`, clamped to
   `[0.5, 2] × distortion_pixels`: **High 6 px, Ultra 8 px at 1080p** (12 and 16 at 4K, 4.5
   and 6 at 0.75 render scale). Dynamic resolution shrinks it with the sub-rectangle, so the
   displacement is the same fraction of the screen at every scale.
3. **Unit mask.** The prepass's `R8_UINT` ID target (ADR-0016) is where units are: slots
   1–7 are outlined units, and the prepass now writes **8 for every unit without an
   outline** (minions, structures, walls, props; the terrain writes nothing). The outline
   pass treats 8 as "no outline" (it was 0 before), so the outlines are unchanged. A
   fragment whose own pixel has a nonzero ID keeps its pixel (offset 0): **no unit is ever
   displaced or smeared**.
4. **Rejection.** A tap that lands on a unit (nonzero ID) or on geometry nearer the camera
   than the sprite (scene depth in front of the particle) is rejected and the fragment keeps
   its own pixel, so nothing in front is ever pulled into the haze and no unit's colour
   bleeds out of its silhouette. This is the water's rule plus the ID test.

So **units are never displaced** (they are masked), **telegraphs and projectiles are never
displaced** (the particles draw after the distortion), **the outlines are never displaced**
(drawn after the composite from the unmasked ID target) and **the UI and text are never
touched** (drawn after the post chain). **Decals and the ground** are bent by at most the
cap: 6 px at 1080p on High is 0.6 % of the screen height, under a quarter of a telegraph
ring's 0.35 m band at the default camera, and the two-tone contrast rim (ADR-0017 §1)
survives a shift of that size. Distortion emitters cannot be telegraphs (parser), and a
telegraph effect's own rings draw on top. The mask needs the ID target, which High and
Ultra have with their default screen-space outlines; with `MOBA_OUTLINES=hull|off` there is
no ID target and **distortion is off** (one log line), rather than unmasked.

**Hidden information.** Distortion particles exist only for effect instances the client
plays, which come only from events the server sent; the passes only move pixels the frame
already shows, by a few pixels. Nothing hidden is drawn, so nothing hidden can be revealed.

### 4. Tiers and switches
| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| Distortion | off | off | on, 6 px at 1080p | on, 8 px at 1080p |

- **Low and Medium: distortion particles are skipped**, or, when the emitter has a
  `fallback_color`, drawn as a faint additive sprite (`sprite`, default `soft`) with that
  colour fading to zero alpha. The shipped hazes define no fallback: a shimmer has no
  colour to fall back to. The skipped spawns cost nothing (no slot, no upload).
- **`MOBA_DISTORTION=off`** (client) → `RenderConfig::distortion = false`.
- **Portability-subset devices (`MoltenVK`): off**, as the full water and soft particles are:
  the pass samples the depth image that is its own read-only depth attachment.
- **Without VFX** (`MOBA_VFX=off`, no compute) or **without the ID target**: off.
- `VfxBudget::distortion` and `VfxBudget::distortion_pixels` carry the tier's values; the
  renderer's `vfx_budget()` reports the effective one (off where any rule above turns it
  off), and the client decides skip-or-fallback from it. `Renderer::distortion_available`.

### 5. Descriptors, graph and memory
- **Its own pipeline layout** `[vfx frame, distortion]` with a 48-byte push block
  (vertex + fragment: the render sub-rectangle's last pixel, the cap, the fades, the
  list's base index). Set 0 is the VFX frame set (uniform, particles, `alive`), set 1 holds
  three combined image samplers: the copy, the scene depth and the outline IDs. Its fragment
  stage sees 3 samplers (and its vertex stage 2 storage buffers); `descriptor_budget`
  checks the layout at creation against the device and a unit test pins it at the Vulkan
  minimum. **No sampler is added to the scene layout** (still 16 of 16).
- **Graph.** Two passes (`distortion_copy`, `distortion`) and one transient. The largest
  frame (both offscreen passes, fog of war, shadow, the prepass with contact shadows and
  AO, the scene, the two water passes, the two distortion passes, the three VFX passes, the
  bloom chain, exposure, composite, outlines, FXAA, upscale, UI and capture) has **31
  passes**, inside `MAX_PASSES` 32; resources stay under `MAX_RESOURCES` 48. The limits do
  not change. The frame-graph topology test compiles every topology with distortion.
- **Memory.** The copy is a graph transient, so R8's VRAM accounting counts it as a target
  automatically (ADR-0018 §4). It aliases the water's refraction copy or the OIT
  accumulation target, so on High and Ultra it adds **no VRAM** in every shipped
  configuration; without either (no OIT is impossible on High+), it would be one
  `R16G16B16A16_SFLOAT` render-sized image (16.6 MB at 1080p). The `alive` buffer grows by
  `capacity / 8` entries (12 KB on High, 24 KB on Ultra) and the args block by 32 bytes.
- **Pipelines** are built at renderer creation through the pipeline cache, with the other
  VFX pipelines, only when distortion is on; nothing compiles mid-match.

### 6. Content
- `cinder-zone` (a burning zone) and `ember-curse` (a fire ability's burning area) gain a
  heat-haze emitter.
- Map visuals gain optional `[[effects]]` (up to 16 looping, client-local ambient effects
  at board positions: `effect`, `position`, `radius`), played by the client like any other
  effect instance (culled by distance, budgeted, `low` importance by default). The Ember map
  gets heat vents: a `heat-vent` effect (haze, a few embers, a dim light) at five spots
  beside its crystals, off the lanes. `content-check` validates every effect file under
  the pack's `vfx/` directory, the bindings, and that each map ambient effect names an
  effect that exists.

## Alternatives considered
- **(a) An offset target and an apply pass** (above): one more screen-sized target and a
  fullscreen pass every frame; rejected on cost.
- **Distortion after the particles** (copy the final HDR, sparks included). The haze would
  bend telegraph rings and enemy projectiles in it, against the readability rules; and it
  needs the same copy.
- **Reusing the water's copy.** It is taken before the water, so a haze over the river would
  show the dry bed; and on maps without water it does not exist. Aliasing gives the memory
  saving without the wrong image.
- **Masking only enemy heroes** (the outline slots alone). Minions and structures are
  gameplay information too; one more ID value covers every unit for free.
- **Running unmasked when the ID target is absent** (hull outlines on High). The cap alone
  would still move a unit by a few pixels; turning distortion off is the readable choice.
- **A distortion normal-map texture.** A sampler and an asset for what two noise octaves
  draw; can be added to set 1 later if art wants authored shapes.

## Consequences
- New public API (additive, `#[non_exhaustive]`): `ParticleBlend::Distortion`,
  `VfxBudget::{distortion, distortion_pixels}`, `VfxStats::distortion`,
  `RenderConfig::distortion`/`with_distortion`, `Renderer::distortion_available`, the
  `distortion` module (`max_offset_pixels`, `refract_tap`, `DistortionTap`,
  `UNIT_ID_PLAIN`); in `moba-assets` `VfxBlend::Distortion`, `VfxDistortion`,
  `VfxEmitter::distortion`, `MapAmbientEffect`, `MapVisuals::effects`,
  `MAX_MAP_EFFECTS`.
- No wire, replay, simulation-logic, golden-state-hash or content-hash change: effects and
  map visuals are client-local, and the pack hash covers only the pack TOML (checked).
- `MOBA_FRAME_LOG` gains `distortion_copy` and `distortion` lanes.
- Goldens: a new distortion golden (the `cinder-zone` preview on Ultra: MSAA, the prepass
  IDs, the stand-in units inside the haze). Existing goldens change only where a haze is in
  view (perf/0023 lists them).
- Costs and validation: [perf/0023](../perf/0023-distortion.md).
