# ADR-0016: Dynamic lights, team outlines, baked AO and contact shadows (R2b)

**Status:** accepted 2026-10-29 (AAA graphics roadmap R2b, the rest of
[R2](../plans/aaa-graphics-roadmap.md#r2--lighting-and-materials-l)). Builds on
[ADR-0011](0011-art-style-lighting.md) (the `shade()` model, set 3, the readability rules),
[ADR-0008](0008-render-graph.md) (the graph) and [ADR-0013](0013-vfx-runtime.md) (effects).
Written before the code; the numbers in [Consequences](#consequences) were filled in from
[perf/0020](../perf/0020-lights-outlines-ao.md) afterwards.

## Context
R2a left four roadmap R2 items open: many dynamic lights (item 5), team and selection
outlines (item 6), baked lighting for the static map (item 4) and screen-space contact
shadows (item 3, which needs scene depth before shading). ADR-0011's "extension points"
section sketched each one; ADR-0013 deferred effect lights to here.

Constraints, as for every R-phase: presentation only and client-local (no wire, replay, sim
or content-hash change), every feature tiered with an off switch, nothing allocated per
frame, Low must hold on min-spec, and three things specific to this work:

- **Lights and outlines must never reveal hidden information.** The server already withholds
  what a client may not see; nothing here may add a channel around that.
- **Readability first** (ADR-0011 §6): outlines must use the colour-blind team palette when
  it is selected, and nothing may wash out team colour.
- **R6 runs in parallel** and adds GTAO, which wants the same scene depth before shading
  that contact shadows need. The depth pre-pass must be reusable by it.

## Decision

### 1. Dynamic lights: clustered forward on a ground-aligned grid
- **API.** `PointLight { position, radius, color }` (linear RGB, intensity premultiplied;
  the same units as the sun), handed to the renderer per frame in `Scene::lights`. The
  renderer never invents lights.
- **Clusters.** A MOBA camera looks down at one board, so almost every shaded point is in a
  thin slab above the ground. View-space froxels would spend most of their depth slices on
  empty air; instead each frame gets a **32 × 32 grid of columns over the camera's ground
  footprint** (the frustum corner rays cut by the `0..10 m` receiver slab — the same cut as
  the shadow fit — at most 300 m from the eye, padded by 4 m). A light is binned into every
  column its sphere's xz bounds overlap, so binning is conservative for any height; the
  shader applies the exact attenuation. Fragments outside the grid clamp to its edge.
- **Caps.** At most [`MAX_LIGHTS`] (256) lights and `MAX_LIGHTS_PER_CELL` (16) per column
  exist at all; each tier sets a global and per-column cap
  (`LightingConfig::max_lights`/`lights_per_cell`). Lights past a cap are dropped **in list
  order**, and counted (`Renderer::light_stats`). The client lists enemy effects' lights
  first, like particle spawns, so a cap never drops an enemy's light before an ally's.
- **Data.** Per frame in flight, two host-visible storage buffers in set 3: the light list
  (`vec4 position/radius, vec4 colour`, 8 KiB) and the grid (per column a count and 16
  indices, 68 KiB). The CPU bins with fixed scratch, writing straight into the mapped
  buffers; the grid's origin, cell size and dimensions go in the camera uniform. Because the
  buffers change per frame, set 3 becomes **one set per frame in flight** (and the offscreen
  views' set likewise); every set is still written only at creation, resize and load-time
  lighting changes. Offscreen views (portraits, the minimap) get no dynamic lights.
- **Shading.** After the sun in `shade()`, a loop over the fragment's column: windowed
  inverse-square falloff (`(1 − (d/r)⁴)² / max(d², 0.25)`), the same wrap-lit diffuse and
  GGX specular as the sun, no shadows. Lights add light; they never replace albedo, so team
  colour survives (rule 1).
- **Effect lights.** An effect descriptor (`vfx/<name>.toml`) may have one optional
  `[light]` table (colour, intensity, radius, anchor, offset, delay, duration, fades,
  flicker, team tint), parsed with the same bounded, range-checked parser. The client's
  `VfxSystem` emits one light per live instance of such an effect, enemy/neutral instances
  first; allied instances are scaled by the allied-VFX opacity setting. **Lights come only
  from effect instances the client already plays**, and those come only from events the
  server sent; a light lights only geometry the client draws. So lights add no information.
- **Why not deferred or tiled-in-screen-space:** deferred would cost a G-buffer and lose
  the forward pass's transparency and MSAA path (ADR-0011 already chose forward); screen
  tiles with depth bounds need the depth pre-pass on every tier, including Low.

### 2. Team readability outlines
- **API.** `UnitInstance::outline` is a palette slot (`0` none, `1..=7`); `Scene::outlines`
  (`OutlinePalette`) holds per slot a linear display colour and a width in pixels. The
  renderer knows nothing about teams: the **client** fills the palette from its
  `team_palette` setting (standard or colour-blind Okabe–Ito, R7a) and assigns slots. A
  higher slot wins where outlines touch.
- **Slots the client uses:** 1 ally hero, 2 enemy hero, 3 own hero (team outlines on heroes
  only, so minion waves stay uncluttered), 4/5 hovered ally/enemy (any unit under the
  cursor, wider and lighter), 6 selected (the unit last ordered attacked, widest).
- **High and Ultra: screen space.** The depth pre-pass (below) also writes an `R8_UINT`
  **ID target** with each fragment's slot. An `outline` pass after the composite (on the
  tonemapped swapchain image, so the palette colour reaches the screen exactly, and before
  the UI) dilates each slot outward by its width with 8-direction taps and alpha-blends the
  colour, anti-aliased on the last pixel. Only the exterior of a silhouette is drawn; the
  unit's own pixels are untouched.
- **Low and Medium: inverted hull.** In the scene pass, after the units, each outlined unit
  is drawn again by a hull pipeline (static and skinned): the vertex is pushed 0.3 m back
  along the view ray and its clip position offset along the screen-space normal by the
  slot's width in pixels; depth-tested, not written, unlit palette colour. The object in
  front hides the hull except for the silhouette ring. The colour goes through the tonemap,
  so it shifts slightly (perf/0020 measures it). Instances with slot 0 are collapsed in the
  vertex shader, so every mesh's instanced draw is reused without CPU filtering.
- **Hidden information.** Both paths outline only units in `Scene::units` — which the client
  only has when the server sent them — and only where they are visible: the ID target is
  depth-tested and the hull is behind its own unit, so neither shows a unit through a wall,
  in fog or in brush beyond what the scene itself draws.

### 3. Baked ambient occlusion for the static map
- **Bake.** `moba-tools ao-bake` rasterises a map's static occluders — the pack's walls (as
  the boxes the client draws) and the visuals descriptor's props (their meshes' triangles,
  or the built-in block) — into a top-down height field at 0.5 m per texel, then computes
  **height-field horizon AO** per texel (16 directions, 12 m, cosine-weighted:
  `1 − mean(sin² horizon)`), written as an `R8_UNORM` KTX2 plus a TOML sidecar
  (`maps/ao/<name>.{ktx2,toml}`: extent, texel count, height falloff and an input
  **fingerprint**). Structures are not baked (they are destroyed in play).
- **Binding.** A map visuals descriptor names it (`[lighting] baked_ao = "ember-ao"`,
  `ao_strength`). The client recomputes the fingerprint (FNV-1a over the extent, wall
  segments, wall height and props) for the map it actually bound and ignores a stale bake
  with one log line, so one visuals name reused by a pack with other walls never gets wrong
  AO. `Renderer::set_baked_ao`/`clear_baked_ao` (load time) bind it to set 3.
- **Shading.** `ambient_occlusion()` in `shade()` multiplies **only the ambient line**
  (diffuse and specular ambient): `surface.occlusion × baked × ssao`. The baked term is the
  map at the fragment's xz, faded to 1 over the falloff height (3 m), so terrain darkens
  around walls and props and every unit's feet darken near them. `ssao` is 1 until R6's
  GTAO writes it (see §5); the product keeps the three composable. Every tier samples it
  (one fetch); it is the Low tier's main "lit" cue. Lightmaps were optional and are not
  done: the board has one sun and the shadow map is on every tier.

### 4. Depth pre-pass and contact shadows (High and up)
- **`depth_prepass`** (High, Ultra; or whenever screen outlines are on) draws terrain, static
  and skinned units depth-only into the frame's `depth` (with the ID target when screen
  outlines are on), with a small constant depth bias pushing it *back*. The scene pass then
  **loads** that depth (`Access::DepthReadWrite`, a new graph access: load, test
  `LESS_OR_EQUAL`, write) instead of clearing it, so its fragments always pass where the
  pre-pass drew them and the final depth is the scene's own, exactly as without a pre-pass.
  Vertex positions are `invariant` in every scene vertex shader.
- **`contact_shadows`** (fullscreen, between the pre-pass and the scene) reconstructs each
  pixel's world position from the depth, marches a short ray towards the sun (High 8 steps,
  Ultra 16, 0.8 m, thickness 0.6 m, distance-scaled bias, faded out by 120 m) against the
  depth buffer, and writes `R8_UNORM` visibility. `shade()` multiplies the shadow-map term by
  it before the shadow strength floor, so rule 2 (shadows never black out a unit) still
  holds. Offscreen views bind a 1×1 white default.
- **For R6.** The pre-pass depth is complete and sampled-capable (`SHADER_READ_ONLY_OPTIMAL`
  through `Access::SampledFragment`) between `depth_prepass` and `scene`. GTAO should run
  there (after or beside `contact_shadows`), write its own screen-space AO target, and
  multiply it in through the `ssao` hook of `ambient_occlusion()`; with GTAO on, the
  pre-pass condition becomes "contact shadows, screen outlines or SSAO".

### 5. Frame, tiers and switches
```text
(portraits, minimap) → vfx_simulate → (shadow) → (depth_prepass) → (contact_shadows) → scene
  → vfx_draw → (vfx_resolve) → composite → (outline) → ui → (capture)
```
The graph's limits rise from 12 passes and 24 resources to 32 and 48 (a `u64` resource mask; the
profiler's query pools follow the pass limit), the headroom R3a and R6 need as well.

| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| Lights: global / per column | 16 / 4 | 64 / 8 | 128 / 16 | 256 / 16 |
| Outlines | hull | hull | screen | screen |
| Baked AO | on | on | on | on |
| Depth pre-pass | — | — | on | on |
| Contact shadows | off | off | 8 steps | 16 steps |

Switches (`LightingConfig::with_*`; the client's environment variables):
`MOBA_LIGHTS=off|<global cap>`, `MOBA_OUTLINES=off|hull|screen`, `MOBA_BAKED_AO=off` and
`MOBA_CONTACT_SHADOWS=off|<steps>`. `screen` on Low/Medium adds the pre-pass; contact
shadows on Low/Medium likewise.

### 6. Integration with R3a and R6 (merge order R4b, R3a, R6, then R2b)
This branch is written against `main` at `74f3087`; R3a (terrain, FoW) and R6 (post,
ADR-0015, PR #334) land first and touch the same frame. On the rebase:
- **One pre-pass.** R6 adds a depth `prepass` before `scene` (when its AO is on, High and
  up) that writes the depth GTAO reads and the scene loads. R2b's `depth_prepass` folds into
  it: R6's pass gains the optional outline-ID attachment, and its condition becomes "AO,
  contact shadows or screen outlines". `contact_shadows` then runs beside R6's `ao` on the
  same depth.
- **MSAA 4× (Ultra).** R6's pre-pass renders a multisampled depth and resolves sample 0
  into `depth`. The ID target follows the same rule: a 4× `R8_UINT` attachment with a
  `SAMPLE_ZERO` resolve (the only resolve integer formats allow) into the single-sampled ID
  target, so the outline pass runs on resolved data, after the composite, as now; contact
  shadows read the resolved depth. Clustered lights are part of `shade()` and are unchanged
  by MSAA. The hull pipelines (Low/Medium, or `MOBA_OUTLINES=hull` on Ultra) take the scene
  pass's sample count like the other scene pipelines.
- **AO composition.** `ambient × surface occlusion × baked × GTAO`: R6's GTAO term becomes
  the `ssao` factor of `ambient_occlusion()`.
- **Set 3.** R3a uses bindings 5–9; R2b's light list, light grid, baked AO and contact
  shadows move after R3a's and R6's. The light list and grid are storage buffers, so R2b adds
  two combined image samplers to set 3 (baked AO, contact shadows), through the existing
  linear-clamp and plain shadow samplers (no new sampler object), against the 16-sampler `maxPerStageDescriptorSamplers` floor. If that
  budget is short, contact shadows can share R6's AO target (a second channel) instead.
- **Graph limits and frame log.** This branch raises the graph to 32 passes / 48 resources
  with a `u64` mask, the same limits as R6, so the rebase keeps one copy; the frame-log lanes
  take R6's layout, with `contact_shadows` and `outline` appended and R2b's
  `depth_prepass` lane merged into R6's `prepass`.

## Alternatives considered
- **View-space froxel clusters (16×9×24).** The textbook choice; on this camera most slices
  hold nothing, and the binning needs per-light depth ranges. The ground grid is simpler,
  conservative and cheaper to bin; revisit if a map gets tall interiors.
- **GPU (compute) light binning.** Removes the CPU loop but adds a pass and a barrier for at
  most 256 lights; the CPU bins them in microseconds.
- **Stencil outlines.** The renderer's depth formats have no stencil (`D32_SFLOAT`/`D16`),
  and a stencil adds a format query and loses the per-slot colour; an ID target carries
  both. A post-tonemap pass also keeps the palette exact.
- **Outlining through walls ("x-ray").** Common in MOBAs for the *own* hero behind walls,
  but it is exactly the information leak the brief forbids for anyone else; not done.
- **Per-vertex AO baked into meshes.** Needs every prop mesh re-exported and does not
  darken the terrain; the top-down height field covers both, and units walking past walls.
- **SSAO-only AO.** Low has no depth pre-pass; baked AO is what makes Low look lit.
- **Contact shadows inline in the scene shader.** Would sample the depth attachment being
  written (a feedback loop) or force a read-only depth scene pass; a separate pass into a
  small target is simpler and is the slot R6's GTAO needs anyway.

## Consequences
- New public API (additive, `#[non_exhaustive]` with builders): `PointLight`, `MAX_LIGHTS`,
  `MAX_LIGHTS_PER_CELL`, `OutlineMode`, `OutlinePalette`, `OUTLINE_SLOTS`, `BakedAo`,
  `LightStats`, `Scene::lights`/`with_lights`/`outlines`/`with_outlines`,
  `UnitInstance::outline`/`with_outline`, `LightingConfig::{max_lights, lights_per_cell,
  outlines, baked_ao, contact_shadow_steps}` and builders, `Renderer::{set_baked_ao,
  clear_baked_ao, has_baked_ao, light_stats}`; in `moba-assets` `VfxLight`,
  `VfxEffect::light`, `BakedAoDesc`, `ao_fingerprint`, `MapLighting::{baked_ao,
  ao_strength}`.
- The instance record grows from 80 to 96 bytes (the outline slot); the camera uniform from
  1,264 to 1,552 bytes.
- No wire, replay, simulation-logic or content-hash change: effects, map visuals and bakes
  are client-local and cosmetic.
- Cost, validation and goldens: [perf/0020](../perf/0020-lights-outlines-ao.md). Both
  goldens were rebaselined (the own and allied hero outlines, and the pre-pass and contact
  shadows on High; the default grass map has no static occluders, so no AO bake).
- The R2 exit's art-direction readability review is prepared (captures per tier, palette
  checks) but needs an art director: a maintainer item.
