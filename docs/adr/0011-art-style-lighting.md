# ADR-0011: Art style and lighting model (stylised PBR)

**Status:** accepted 2026-10-29 (AAA graphics roadmap R2a). Amends nothing; builds on
[ADR-0008](0008-render-graph.md) (render graph), [ADR-0009](0009-descriptor-model.md)
(descriptor sets) and [ADR-0010](0010-texture-pipeline.md) (materials and KTX2).

## Context
Until R2a every surface was lit by one Lambert term, a hemispheric ambient and a white rim
(`lighting.glsl`, unchanged since R1b). There were no shadows, no specular, no image-based
lighting and the material's metallic/roughness texture was bound but unread. The
[AAA graphics roadmap](../plans/aaa-graphics-roadmap.md) §2 sets the target: **readability
first**, a stylised PBR look ("hand-painted or painterly textures on physically based
lighting") that runs on min-spec hardware on Low. §3 requires every feature to be tiered with a
Low path or an off switch, and evidence per feature.

Constraints:
- **The camera.** A MOBA camera is high and angled down; the play space is one fixed board.
  Zoom ranges from the 18–46 m player camera to the whole-map spectator/overview camera
  (≈ 240 m orbit).
- **Readability beats realism.** Ten heroes, a hundred minions and overlapping abilities must
  read in a fraction of a second; team identity must be unmistakable.
- **Determinism is untouched.** Lighting is presentation only and client-local (map visuals,
  materials, environments), like every R1 asset.
- **Art direction must be able to push the look** without engine changes: the parameters
  live in client-local descriptors.

## Decision

### 1. Lighting model: a physically based core
Every lit scene shader (`unit.frag` in both permutations, the skinned path, which reuses it,
and `terrain.frag`) calls one function, `shade(Surface)` in `include/lighting.glsl`:

- **Specular:** GGX (Trowbridge–Reitz) distribution, **height-correlated Smith-GGX**
  visibility, **Schlick** Fresnel with `F0 = mix(0.04, albedo, metallic)`. Roughness is
  perceptual (`alpha = roughness²`) and clamped to ≥ 0.045 (no sub-pixel highlights without
  anti-aliasing). `shade()`'s output is clamped to 32768, finite in the fp16 and B10G11R11 HDR
  targets (a minimum-roughness GGX peak reaches ~7·10⁴), so R6's bloom and exposure never see
  an infinity; the half vector is normalised safely.
- **Diffuse:** Lambert scaled by `(1 - F)(1 - metallic)`, so diffuse plus specular never
  exceeds the incoming light (energy conserving for a single bounce).
- **Units:** light intensities are pre-multiplied by π, so an unshadowed white Lambert surface
  facing the sun shows `albedo × sun`; the pre-R2 exposure and `Camera::light_intensity`
  values therefore keep their meaning.
- **Inputs:** textured materials take base colour × factor (team-tinted through the mask,
  ADR-0010), metallic = texture R × factor, roughness = texture G × factor, the normal map in
  a **vertex tangent frame** (glTF `TANGENT` or a MikkTSpace-style generator; see below) and
  emissive. Units without a material are dielectric with the map's `unit_roughness`; the
  terrain uses `terrain_roughness`.

### 2. Stylisation layer
On top of the physical core, four art-direction controls, each a map parameter (the map's
`[lighting]` table) and a per-material multiplier/override (`materials/<model>.toml`):

| Control | What it does | Map key | Material key |
|---|---|---|---|
| Wrap lighting | `clamp((N·L + w) / (1 + w))`: light wraps past the terminator, softer forms | `wrap` (0.2) | `wrap` (overrides) |
| Ramp | remaps N·L through a 2D colour texture (`u` = N·L in −1..1), blended by strength — cel bands, warm/cool terminators | `ramp`, `ramp_strength` (0) | `ramp` (× strength) |
| Painted specular | specular (direct and ambient) × an RGB tint × a scale: warm or desaturated highlights instead of white | `specular_tint`, `specular` (1) | `specular_tint`, `specular` (×) |
| Rim | `(1 − N·V)³ × strength × colour`: a coloured silhouette | `rim` (0.25), `rim_color` | `rim` (×) |

The defaults keep the pre-R2 look recognisable (the same white rim strength, a slight wrap),
so maps without a `[lighting]` table only gain the physical core, IBL and shadows.

### 3. Image-based lighting
- **Diffuse:** SH-L2 (nine RGB coefficients) of the environment's irradiance, cosine-convolved
  and divided by π offline, evaluated per pixel (`sh_irradiance`), times `(1 − F_r)(1 − metallic)`
  with the roughness-aware Schlick term.
- **Specular:** a prefiltered GGX cube (mip `m` of `M` holds roughness `m / (M − 1)`) sampled at
  the reflection vector, times the **split-sum BRDF LUT** `F0 · A + B` (Karis 2013, with the
  height-correlated visibility, so the ambient matches the direct term). The LUT is 32²
  `R16G16_SFLOAT`, generated on the CPU at renderer creation (128 GGX samples per texel, well
  under a millisecond).
- **Baking:** `moba-tools env-bake` evaluates an analytic sky (zenith/horizon/ground gradient
  and a broad sun glow, **no sun disk** — the shaded sun provides direct light and shadows, so
  the disk would double-count) into a 64² `RGBA16F` cube with 7 levels (256 KiB) and the SH
  coefficients (a TOML sidecar). No external HDRIs; every shipped environment is generated, so
  `THIRD_PARTY.md` is unchanged. KTX2 cube support and the format are an
  [ADR-0010 amendment](0010-texture-pipeline.md#amendment-2026-10-29-cube-maps-and-rgba16f-r2a).
- **Per map:** the map visuals descriptor names an environment
  (`[lighting] environment = "ember"` → `environments/ember.{toml,ktx2}`). Without one (or if it
  fails to load) the **hemispheric ambient** (sky `Camera::ambient`, ground
  `SceneLighting::ambient_ground`) lights diffuse and specular ambient, as before R2.

### 4. Shadows: one fitted directional map
- **One map, no cascades.** The board is one plane seen from above, so one orthographic map
  fitted to the camera's view covers every zoom. The fit cuts the four frustum corner rays to
  a receiver slab (`0 ≤ y ≤ 10 m`: the ground and unit bodies), each at most 520 m from the eye
  (the terrain fog is opaque there), and covers the bounding **sphere** of those points, so
  rotating the camera never resizes texels. The radius is rounded up to a 2 m step (small zooms
  do not rescale texels), the map is pushed back along the light far enough to hold a 32 m
  caster (the tallest wall/prop) above any receiver, and the projection is **snapped to whole
  texels**, so panning never shimmers. Texel sizes at 2048² for the client's cameras
  (unit-tested): 0.039 m at an 18 m player camera, 0.092 m at the default 46 m player camera
  and 0.168 m at the 240 m whole-map overview (halved on Ultra's 4096²). A hero (≈ 1 m wide)
  spans about 11 texels at the default camera and 6 at the overview, so its shadow keeps its
  shape. A second cascade is not needed and would double the caster draws; revisit only if
  spectator art direction wants contact-sharp shadows at full zoom-out.
- **A render-graph pass** (`shadow`, ADR-0008) before `scene`: depth-only, a fixed-size
  transient (never aliasing swapchain-sized images), cleared to 1. Casters are every unit in
  the scene — minions, structures, walls and props (static meshes, instanced) and heroes
  (**skinned**: a `SHADOW_PASS` permutation of `skinned.vert` reads the same bone buffer, so
  animated heroes cast animated shadows). The terrain receives but does not cast. The graph
  derives the barriers: depth write (early/late fragment tests) → fragment-shader sampled read
  with `DEPTH_STENCIL_ATTACHMENT_OPTIMAL → SHADER_READ_ONLY_OPTIMAL`, and the bone buffer's
  host write → vertex read once for both passes.
- **Low suns and tall casters:** where the device supports `depthClamp` (enabled at device
  creation) the casters' pipelines clamp depth ("pancaking"), so a caster nearer the light than
  the map's near plane still writes it instead of being clipped. The fit's lift covers a 32 m
  caster down to a sun elevation of 0.05; map descriptors require at least 0.2.
- **Offscreen views (R7b portraits, the minimap) never sample the shadow map.** They render
  before the shadow pass, with their own cameras (a studio camera, a persistent top-down bake),
  so the main view's map would be wrong for them and on frame 0 not even written. Their camera
  uniform turns shadows off (`FrameLighting::offscreen`, unit-tested) and they bind a second
  lighting set whose shadow binding is a persistent 1×1 default image, so the graph never sees
  an unordered read of the transient. The environment is kept, so portraits match the map.
  The shadow pass draws only the main view's units and bones.
- **Format and sampler checks:** the map's format must be an attachment, sampled, usable with
  a comparison sampler (`FORMAT_FEATURE_2_SAMPLED_IMAGE_DEPTH_COMPARISON`) and, for hardware
  PCF, linearly filterable (else nearest taps); a `VK_KHR_portability_subset` device needs
  `mutableComparisonSamplers` (queried and enabled). Otherwise shadows turn off with one log
  line. The fixed-size map survives swapchain resizes (only swapchain-sized transients are
  reallocated).
- **Bias:** slope-scaled raster depth bias (1.25 constant, 1.75 slope) plus a normal offset of
  1.5 texels, scaled up at grazing light.
- **Filtering per tier** (`ShadowFilter`, in the shader by a uniform, not a permutation):
  - **Hard** — one hardware 2×2 PCF tap (`sampler2DShadow` with linear filtering);
  - **PCF** — 3×3 hardware taps (a 4×4-texel tent);
  - **PCSS-lite** — a 16-tap blocker search (taps keyed on the tier, `LightingConfig::pcss_taps`:
    16 on High, 32 on Ultra, independent of the map size), a penumbra from the receiver/blocker
    depth gap ×
    the sun's angular radius (`shadow_softness`), then a 16- or 32-tap PCF disk. The Vogel
    disk is **fixed** (not rotated per pixel): without TAA a per-pixel rotation shows as grain
    on contact shadows, and the bilinear compare taps keep the fixed pattern smooth.
- **Contact shadows:** screen-space contact shadows on High are **not** in R2a — they need
  the scene depth before shading (a depth pre-pass); R2b follow-up.

### 5. Tiers and switches

| | Low | Medium | High | Ultra |
|---|---|---|---|---|
| Shadow filter | hard (1 tap) | 3×3 PCF | PCSS-lite 16 + 16 | PCSS-lite 16 + 32 |
| Shadow map | 1024² | 2048² | 2048² | 4096² |
| Specular IBL | off: SH in the reflection direction | cube | cube | cube |
| Diffuse IBL, PBR core, stylisation | on | on | on | on |

**The Low path** costs, per pixel: one shadow tap, nine SH coefficients, one 2D LUT fetch and
the GGX terms; the shadow pass writes a 4 MiB D32 map (1024²; D16 on devices without a linearly filterable D32) once per frame. Low skips the
specular cube upload entirely (`set_environment` ignores the cube), saving 256 KiB of VRAM per
map and a cube fetch per pixel.

**Off switches** (`RenderConfig::lighting`, `LightingConfig::with_*`; the client reads them
from the environment): `MOBA_SHADOWS=off|hard|pcf|pcss` (off removes the shadow pass and its
pipelines from the frame), `MOBA_SHADOW_SIZE=<256..8192>`, `MOBA_IBL=0` (hemispheric ambient
everywhere) and `MOBA_SPECULAR_IBL=0`. A device without a sampled, attachable depth format gets
shadows off with one log line.

### 6. Readability rules (acceptance criteria)
These are the review criteria for any map, material or lighting change. Each has a check.

1. **Team identity survives lighting.** Team colour lives in albedo (vertex colour, or the
   material mask) and is never replaced by lighting: no coloured key light or ramp may shift
   the blue/red team hues by more than a readable margin. *Check:* in the per-tier captures,
   blue and red units stay distinguishable at a glance in lit, shadowed and fogged areas.
2. **Shadows never black out a unit.** `shadow_strength` ≤ 0.9 (enforced by the descriptor
   parser and clamped by the renderer) keeps a floor of direct light
   (default 0.78), and the ambient term is unshadowed. *Check:* a unit standing in a tower's
   shadow keeps its team colour readable.
3. **Silhouettes separate from the ground.** Units keep a rim (map `rim` > 0, enforced); the
   terrain has
   none and is rougher than units (`terrain_roughness` > `unit_roughness`, enforced), so units read as
   brighter, crisper shapes on a matte ground. *Check:* overview capture, units at 10–20 px.
4. **Highlights do not wash out colour.** Specular is tinted and scaled by art direction, and
   roughness is clamped to ≥ 0.045 (no sparkle). *Check:* no unit face goes to white in the
   captures.
5. **Shadows are stable.** No shimmering while panning (texel snapping, unit-tested) and no
   acne or detached ("peter-panning") shadows at the player and overview cameras. *Check:*
   player-camera capture of a skinned hero's contact shadow.
6. **Every tier keeps rules 1–5.** Low (hard shadows, no specular cube) must pass the same
   checks as Ultra.

Art direction signs off the rules on the reference maps (roadmap R2 exit); until an art
director exists the engine evidence in [perf/0015](../perf/0015-lighting.md) and the captures
in [screenshots.md](../screenshots.md#r2a-lighting) stand in.

### 7. Parameters
- **Map** (`maps/<name>.toml`, `[lighting]`, every key optional; `moba_assets::MapLighting`):
  `environment`, `sun_direction`, `sun_color`, `sun_intensity`, `ambient`, `ambient_ground`,
  `ibl_intensity`, `shadow_strength`, `shadow_softness`, `wrap`, `rim`, `rim_color`, `ramp`,
  `ramp_strength`, `specular`, `specular_tint`, `unit_roughness`, `terrain_roughness`, each
  range-checked ([content-authoring.md](../content-authoring.md#lighting)).
- **Material** (`materials/<model>.toml`): `metallic`, `roughness` (now read), `emissive`
  (now added), and `wrap`, `rim`, `ramp`, `specular`, `specular_tint`.
- **Renderer:** `Scene::lighting` (`SceneLighting`, per frame, copied into the camera uniform),
  `Camera::light_direction`/`light_intensity`/`ambient` (the sun and hemispheric sky, as
  before), `Renderer::set_environment`/`clear_environment`/`set_lighting_ramp` (load time)
  and `RenderConfig::lighting` (startup).

### 8. Tangents
Normal maps now use a per-vertex tangent (`vec4`, glTF convention: `xyz` along +u,
`bitangent = cross(normal, tangent.xyz) × w`, pointing up the image) instead of R1b's
screen-space derivative frame, which swims under animation and breaks on mirrored UVs. The
`.glb` loader reads `TANGENT` (when every primitive has it); otherwise the renderer generates
tangents at registration with a MikkTSpace-*style* generator (Lengyel per-triangle
accumulation, Gram–Schmidt, handedness from the bitangent; no vertex splitting at mirrored
seams, unlike full MikkTSpace). The sample-mesh tool writes generated tangents into the
static samples. Static vertices grow from 32 to 48 bytes and skinned ones from 40 to 56.

## Alternatives considered
- **Cascaded shadow maps (2–4 cascades).** The standard for open worlds; for a fixed top-down
  board the single sphere-fitted map is already 4–9 cm/texel at the player cameras, and
  cascades would double or quadruple caster draws. Kept as the documented escape hatch.
- **Variance / exponential shadow maps.** Cheap soft shadows with one filtered fetch, but light
  bleeding under stacked casters (walls behind units) is a readability hazard, and they need a
  colour target and a blur pass. PCF/PCSS on a depth map is simpler and has no bleeding.
- **A per-pixel rotated (noisy) PCSS disk.** The usual choice with TAA; without TAA (R6 keeps
  TAA optional because ghosting hurts readability) it reads as grain.
- **Full MikkTSpace port.** Exact baker parity needs the reference implementation (C, or a
  Rust port as a dependency); our assets are generated or exported with tangents, so the
  in-house generator covers missing ones. Revisit with production art.
- **Analytic environment BRDF (no LUT).** One less texture, but the split-sum LUT is 4 KiB,
  built at load, and matches the direct term exactly.
- **External CC0 HDRIs.** More natural skies, but a licence record per file, megabytes per map
  and a look the stylised maps do not want; the analytic sky is tunable per map.
- **Deferred shading.** Not needed for one directional light; clustered forward (R2b) keeps
  MSAA and transparency simple.

## Consequences
- Every scene pixel now runs the GGX/IBL/shadow shader. On the RTX 5070 Ti at 1080p (High) the
  scene pass goes from 0.027 to 0.052 ms at the overview camera and from 0.050 to 0.096 ms at
  the player camera, and the shadow pass adds 0.004–0.007 ms (0.012–0.045 ms for Ultra's
  4096² map) ([perf/0015](../perf/0015-lighting.md)). Both goldens were rebaselined
  deliberately.
- New public API (additive): in `moba-render` `LightingConfig`, `ShadowFilter`,
  `SceneLighting`, `Scene::lighting`/`with_lighting`, `RenderConfig::lighting`/
  `with_lighting`/`effective_lighting`, `Renderer::{lighting_config, set_environment,
  clear_environment, has_environment, set_lighting_ramp}`, `MaterialData`'s stylisation fields
  and builders, `RenderError::InvalidEnvironment`, the `EnvironmentDesc` re-export and
  `MIN_/MAX_SHADOW_RESOLUTION`; in `moba-assets` `MapLighting`/`MapVisuals::lighting`, the
  material stylisation fields, `EnvironmentDesc` and its errors, cube `TextureData`,
  `TextureFormat::Rgba16Float`, `f16_from_f32`/`f32_from_f16`, and `MeshData` tangents. New
  types are `#[non_exhaustive]` with constructors and `with_*` builders.
- No wire, replay, simulation-logic or content-hash change: environments, map lighting and
  materials are client-local and cosmetic.
- **Extension points for R2b** (recommendations): clustered forward lights add a light list to
  set 3 and a loop after the sun in `shade()`; team/selection outlines are a pass after
  `scene` reading `depth` (already sampled-capable) and a stencil/ID target; baked AO or
  lightmaps become a `Surface.occlusion` term multiplying the ambient (the hook is the ambient
  line in `shade()`); contact shadows need a depth pre-pass and a `contact` factor on `lit`.
