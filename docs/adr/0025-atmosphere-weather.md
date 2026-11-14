# ADR-0025: Atmosphere and weather: Hillaire sky, froxel volumetric fog, backdrop clouds, wetness and puddles (N6)

**Status:** proposed 2026-11-03. This is phase N6 of the
[next-gen graphics roadmap](../plans/next-gen-graphics-roadmap.md) (§3, §4, §6.5, §6.8, §8,
§10 and §12), written before the code. Docs only; nothing here is built yet.
- **Builds on** (each accepted; this ADR uses them and does not re-decide them):
  - [ADR-0020](0020-next-gen-foundations.md) (N0, accepted 2026-11-01, Amendments 1–6): the
    `Tiered`/`Compat` paths and the Compat freeze (§2), the bindless set and buffer device
    address (§3), Slang (§4), the tier budgets and budget-driven dynamic resolution (§5).
  - [ADR-0021](0021-temporal-upscaling-overlay.md) (N1, accepted 2026-11-02): Halton jitter
    and the frame index, motion vectors, the `reactive` and `tc_mask` targets, the history
    resets (§2c), the temporal back ends and the **post-upscale gameplay overlay layer** (§5).
  - [ADR-0023](0023-frame-architecture-materials.md) (N2, accepted 2026-11-02): the thin
    G-buffer, `light_classify` with its sky-tile list, `deferred_light` (fog and fog of war at
    step 6, team rims after fog at step 7), the forward `water` and `glass` passes, the shared
    Slang modules in `shaders/slang/lib/`, and the material model, which already names N6's
    **wetness** as a macro modifier evaluated when the G-buffer is written (§6a).
  - If any of these is amended in its implementation, this ADR follows it.
- **Amends, for the Tiered path only:**
  - [ADR-0014](0014-terrain-fog-of-war.md) §4: the sky and height fog are replaced on Tiered
    (§1, §2, §3); fog of war is unchanged;
  - [ADR-0011](0011-art-style-lighting.md) §3: `env-bake` gains a second sky model, the
    Hillaire atmosphere, next to today's analytic gradient (§1f);
  - [ADR-0013](0013-vfx-runtime.md): a new `ParticleRole::Weather` with its own budget share
    (§6g);
  - [ADR-0021](0021-temporal-upscaling-overlay.md) §2d: the `tc_mask` pass gains a fullscreen
    sub-draw for rippling puddles (§6f);
  - [ADR-0023](0023-frame-architecture-materials.md) §2a and §11: `sky` becomes a compute
    pass dispatched over `light_classify`'s sky tiles (§1d); §2c step 6 and §6a: fog becomes
    the froxel lookup on Medium and up (§3e), and wetness gets its concrete form (§6d).
- **Compat is unchanged by N6.** Every N6 PR leaves every Compat golden byte-identical. N6
  only *specifies and tests* Compat's share (the Hillaire sky and the improved height fog,
  §9); the C1 PR wires it in and rebaselines Compat's goldens deliberately, then Compat
  freezes (ADR-0020 §2d).
- **ADR numbers.** ADR-0022 is the A0 provenance gate (PR #349) and ADR-0024 is being drafted
  for N7 in parallel, so N6 is ADR-0025.

External claims (papers, talks, reference code and its licences, engine documentation and
Vulkan limits) were checked on 2026-11-03 against the sources in [Sources](#sources). Facts
that ADR-0020, ADR-0021 or ADR-0023 already verified are cited as theirs. Anything that could
not be confirmed is marked **unverified**.

## Context

Roadmap §8 gives N6 four items: the Hillaire sky, froxel volumetrics, backdrop clouds, and
wetness and puddles. It depends on N2 and runs in parallel with N4 and N5. Roadmap §6.5 adds
the rule that weather is client-side, gameplay-neutral and capped by the readability tests,
and the C1 row says Compat takes "Hillaire sky and improved height fog from N6".

What the code does today (`main` at `e9d5f91`):

- **Sky** (`sky.vert`, `sky.frag`, ADR-0014 §4): a fullscreen triangle at the far plane, drawn
  after the opaque scene. On Medium and up it samples the map's environment cube at a blurred
  mip; on Low it draws a zenith/horizon gradient with a sun glow. Near and below the horizon it
  blends into the height-fog colour. There is no sun disk: "the sun itself is the shaded key
  light".
- **Height fog** (`include/atmosphere.glsl`, `apply_height_fog`): one analytic exponential
  layer integrated along the view ray, with density, height falloff, a start distance, a
  maximum opacity and **one constant colour**. `terrain.frag`, `foliage.frag` and `water.frag`
  apply fog of war first and height fog second; `unit.frag` applies height fog only;
  **particles are not fogged.**
- **Image-based lighting** (ADR-0011 §3): `moba-tools env-bake` evaluates an analytic
  zenith/horizon/ground gradient with a sun glow into a 64² `RGBA16F` cube with 7 mips and an
  SH-L2 sidecar. Maps name it in `[lighting] environment`.
- **Per-map visuals** (`content/maps/<map>.toml`, client-local, never hashed or sent):
  `[lighting]` (sun direction, colour and intensity, `ibl_intensity`), `[atmosphere]`
  (`fog_color`, `fog_density`, `fog_height`, `fog_start`, `fog_max`, `sky_zenith`,
  `sky_horizon`), `[fog_of_war]`, `[post]` (EV compensation, auto exposure and its clamp, the
  grading LUT) and `[foliage]` (wind). Ember's fog starts at **80 m**.
- **Exposure** (ADR-0015 §2): map EV plus optional auto exposure, clamped; captures pin it.
  The art bible proposes EXP-1 to EXP-4 (fixed EV in ranked, ±0.5 EV in casual).
- **The gameplay camera never sees the sky.** `network_camera` orbits at a pitch of
  0.95 rad (54.4°) with a 60° **vertical** field of view (`Camera::orbit`), so the top of the
  frame looks 24.4° below the horizon. The spectator (0.92 rad) and the overview camera do not
  see it either. Only the local preview's low orbit (pitch down to 0.08 rad), the showcase and
  cinematics do. At gameplay the sky matters only through **reflections and the IBL**.
- **The board's depth range is narrow.** At the default 46 m follow distance the eye is about
  37 m above the ground, and the ground spans about **38–91 m** of view depth from the bottom of
  the frame to the top; the far plane is 1,000 m. With `fog_start = 80` the board is almost
  unfogged today: the haze exists for the horizon and the map edge.
- **Nothing is wet.** There is no wetness, puddle, rain or weather state anywhere.

The art direction (the art bible v0 draft on PR #347, `docs/art/`, read-only here) shapes
what N6 has to deliver:
- Map 1 is **Ember Crossing**, a desert basin of basalt terraces, ash and sand, in a
  science-fantasy setting (the Solar Forge against the Void) with an "epic, weathered" tone.
- `lighting-mood.md` proposes four moods: **A Golden Crossing** (recommended: late afternoon,
  sun at 28°), B Ember Dusk (today), C Hazy Noon (the readability baseline in tests), and
  **D Dust Veil**, an overcast of suspended dust and ash offered as a **casual-only weather
  variant with ranked caps** (fog 0.008 / 0.60 casual, 0.004 / 0.45 ranked). It lists "sky
  turbidity (Hillaire/Preetham)" among each mood's physical targets.
- Its open decision on weather: "lighting is baked and fixed per map, so a variant is a second
  bake of the same map, not a dynamic cycle", with the recommendation **none for the slice**.
- `material-vocabulary.md` has **wet basalt** ("seeps and the rare rain variant": dry albedo
  × 0.7, roughness 0.10–0.30 where wet, puddle and streak masks, ripples and streaks from
  Medium), and a **dust** weathering layer (up to 0.85–0.95 roughness, ≤ 20 % on units).
- `palette-and-value.md` sets VAL-2 (rendered ground mean L 0.30–0.58), VAL-3 (shadow floor
  L ≥ 0.18) and CON-1 to CON-5, including **CON-5: a higher tier never lowers CON-1 to CON-4
  below the Low result (≥ Low − 0.02)**. `silhouettes-and-scale.md` RIM-2: the rim survives
  fog.

So the honest picture: at the 46 m camera, N6's visible payoff is **light shafts and local
fog volumes near the ground, VFX light in the air, wet and puddled ground, and the sky's
colour in every reflection and in the IBL.** The full sky, aerial perspective and clouds pay
off only for the showcase, cinematics and low-orbit views.

Constraints:
- **Presentation only.** No change to `PROTOCOL_VERSION` (v42), replay `VERSION` (v5),
  `SIM_LOGIC_VERSION` (v9), content hashes or `tests/golden-state-hash.txt` (§11). **Weather is
  never gameplay.** If weather were ever to affect vision, movement or anything the server
  decides, that is a separate decision with its own ADR, a simulation-logic bump, wire and
  replay changes, and server authority over the weather state. Nothing in this ADR prepares
  for that or depends on it.
- **Readability is non-negotiable** (roadmap §4): fog and weather must never hide fog of war,
  telegraphs or units, and a higher tier may never hide what Low shows.
- **The R8 rules hold:** no per-frame allocation, `locked_uploads=0`, `locked_pipelines=0`.
  Everything N6 creates is created with the renderer or at map load.
- **Compat is byte-identical** until C1.
- **Lighting is fixed per map and baked** (roadmap §1, N3). N6 must not introduce a moving sun
  or a time-of-day cycle in gameplay.

## Decision

### 1. The atmosphere model and the sky LUTs (Hillaire 2020)

**1a. The model.** Hillaire's EGSR 2020 technique [S1][S2]: a planet with an atmosphere of
Rayleigh and Mie scatterers and an absorbing ozone-like layer, rendered through a small set of
look-up tables, with a multiple-scattering approximation that stores the isotropic
contribution of all higher orders in a 2D LUT indexed by altitude and sun zenith angle [S2].
The default parameters are the reference implementation's Earth [S3]:

| Parameter | Earth default [S3] | Per-map key (`[atmosphere.sky]`) |
|---|---|---|
| Ground radius / top radius | 6,360 km / 6,460 km | fixed |
| Rayleigh scattering at sea level | (5.802, 13.558, 33.1) × 10⁻³ km⁻¹, scale height 8 km | `rayleigh` (multiplier), `rayleigh_tint` (rgb multiplier, for non-Earth skies) |
| Mie scattering / extinction | 3.996 × 10⁻³ / 4.440 × 10⁻³ km⁻¹, scale height 1.2 km | `mie` (multiplier: the dust and haze dial), `mie_absorption` |
| Mie phase | Cornette–Shanks, g = 0.8 | `mie_g` |
| Ozone absorption | (0.650, 1.881, 0.085) × 10⁻³ km⁻¹, a tent around 25 km | `ozone` (multiplier) |
| Ground albedo | — | `ground_albedo` (rgb, default from the map's dominant terrain layer) |
| Sun disk | — | `sun_disk_deg` (angular diameter, default 0.53°) |

- **No turbidity.** The model has no turbidity parameter; that belongs to the Preetham and
  Hosek–Wilkie sky models. The art bible's "sky turbidity" targets become `mie` values, chosen
  by eye against the mood captures (see [Corrections](#corrections-to-the-roadmap) and Q11).
- **Science fantasy is in range.** Smoke-dark or ash-laden skies (moods B and D) are higher
  `mie` and `mie_absorption`; a stranger sky is `rayleigh_tint`. All values are validated by
  `content-check` against ranges that keep the LUTs finite (the reference code warns that
  extreme coefficients can break its multiple-scattering series [S3]).

**1b. Units: the authored sun stays the ground truth.** The engine is not in photometric units
(`[lighting] sun_intensity` is 1.4 on Ember), and maps tune the sun's colour for readability
(ADR-0011 rule 1; the bible's moods). So:
- `[lighting] sun_color × sun_intensity` keeps meaning **the illuminance that reaches the
  board.** Nothing in N6 changes how the board is lit by the sun.
- The sky is computed for a top-of-atmosphere sun of that value divided, per channel, by the
  transmittance LUT at the sun's zenith angle (clamped to ≥ 0.05 so a sun at the horizon does
  not explode). The sky and the sun then agree: a warm low sun gets a sky that would have
  reddened it.
- This is what Hillaire's ground-view mode already does: one transmittance value from the top
  of the atmosphere to the ground, applied to everything [S2].
- When roadmap §6.7's physical camera (EV100) lands, one scale factor converts both; that is
  that phase's change, not N6's.

**1c. The LUTs.** Sizes and step counts from the reference implementation [S3] and Hillaire's
GTX 1080 timings [S2]:

| LUT | Size, format | Depends on | Computed | Cost on GTX 1080 [S2] |
|---|---|---|---|---|
| `atmo_transmittance` | 256 × 64 `RGBA16F` (128 KiB) | atmosphere parameters | **CPU at map load** (Rust model, §1e), uploaded, both paths | 0.01 ms (40 steps) |
| `atmo_multiscatter` | 32 × 32 `RGBA16F` (8 KiB) | atmosphere parameters | **CPU at map load**, uploaded, both paths | 0.07 ms (20 steps) |
| `atmo_sky_view` | 192 × 108 `RGBA16F` (162 KiB); latitude mapped non-linearly to keep the horizon sharp [S2][S3] | sun direction, camera altitude | Tiered: Slang compute, **only when the sun or the camera altitude changes** (§1d); Compat (C1): CPU at load | 0.05 ms (Hillaire's table uses 200 × 100, 30 steps) |
| `atmo_aerial` | 32 × 32 × 32 `RGBA16F` (256 KiB), froxels over the frustum, 4 km per slice [S3] | camera | Tiered only, Slang compute, **only on cameras that see past 1 km** (§1d) | 0.04 ms (30 steps) |

- **Why the two static LUTs are CPU-made.** They depend only on the map's parameters, which
  are fixed for the match. A CPU build at load is an upload like any texture (no per-frame
  work), it is the same code that `env-bake`, `light-bake` and the tests use (§1e), and Compat
  (GLSL only, ADR-0020 §2d) gets it without a compute pass. A CPU build of 256 × 64 × 40 and
  32 × 32 × 64 directions × 20 steps is a few million medium samples, about milliseconds on one
  core (to be measured, verification gate 1).
- **The sky-view LUT is effectively static in a match.** The sun is fixed per map (roadmap §1)
  and the camera's altitude changes by tens of metres against a 100 km atmosphere. It is
  rebuilt when the sun direction changes or the camera altitude changes by more than 100 m,
  which in practice is map load, the showcase and cinematics.
- **Aerial perspective is not a gameplay effect.** Over the board's 38–91 m of view depth, sea-level
  Rayleigh extinction (0.033 km⁻¹ in blue [S3]) attenuates by about 0.3 %. The AP LUT is built
  and applied only when the camera's configuration sees past 1 km (the low orbit, showcase and
  cinematics; roadmap §2 gives those cameras their own render configuration). At the gameplay
  camera the height fog (§2) and the froxels (§3) are the only aerial effects.

**1d. The sky pass on Tiered.**
- `sky` stays where ADR-0023 §11 puts it, after `deferred_light`, but it is now **drawn only
  over N2's sky tiles**: a compute pass dispatched indirectly from `light_classify`'s sky list.
  At the gameplay camera that list is empty, so the sky costs nothing.
- Per sky pixel: the sky-view LUT, plus the sun disk (Hillaire composites it at this stage
  [S2]) with its radiance clamped to keep bloom inside the post budget, plus the clouds where
  they exist (§5), then the froxel volume's far slice (§3e) or aerial perspective (§1c).
- The far-plane background-motion draw of ADR-0021 §2b is unchanged.

**1e. One Rust implementation, three consumers.** `moba_render::atmosphere` (pure Rust, no
Vulkan) implements the medium, the phase functions, the LUT parameterisations and the four
LUT builders. It is:
- the **oracle** for the Slang sky-view and AP passes (verification gate 1);
- what builds the two static LUTs at load on both paths and Compat's sky-view LUT (§9);
- what `env-bake` and N3's `light-bake` evaluate as the sky (§1f).

The Slang side is `shaders/slang/lib/atmosphere.slang` (medium, phase, LUT mappings) plus
`atmo_sky_view.slang` and `atmo_aerial.slang`.

**1f. The IBL comes from the same sky.** A sky that disagrees with its own reflections reads
as fake, and at the gameplay camera the reflections *are* the sky.
- `env-bake` gains `--sky hillaire --map <map>`: it evaluates the Rust model from the map's
  `[atmosphere.sky]` table (and the static cloud layer, §5c) into the same 64² cube and SH-L2
  as today. The output records a fingerprint of its inputs, and `content-check` fails if the
  map's parameters no longer match it, as it does for the AO bake.
- The map names it in `[atmosphere.sky] environment = "<name>"`. **Tiered uses it from N6;
  Compat keeps `[lighting] environment` until C1** (§9), so Compat's pixels do not move.
- `ibl_intensity` remains an art override on both paths (the bible keeps it below 1 so a warm
  sky does not grey blue units). A physically exact sky and a readability override coexist;
  the override wins.
- **N3 builds on this.** `light-bake` uses the same Rust sky as its sky light, so baked GI,
  the probes, the IBL and the visible sky all agree. That is a constraint on N3's ADR.

**1g. Reference code.** Hillaire's reference implementation, `sebh/UnrealEngineSkyAtmosphere`,
is **MIT-licensed** (`LICENSE`: "MIT License", "Copyright (c) 2020 Epic Games, Inc."; GitHub
detects MIT) [S3]. Its source files also carry an "All Rights Reserved" header line, which the
repository's MIT grant overrides for this repository's contents; that reading is ours, not a
legal opinion. The repository also vendors Bruneton's 2017 code under a **BSD-3-Clause**
notice (`Resources/Bruneton17/`) [S3], which N6 does not need. Decision (Q2): port what we use
from Hillaire's MIT files, with the MIT notice in `THIRD_PARTY.md`; do not port the Bruneton
files. Unreal Engine's own implementation is under Epic's EULA and is not used.

### 2. Analytic height fog v2 (Low, the far field, and Compat at C1)

Today's function has one layer and one constant colour, so fog looks the same towards and away
from the sun and never matches the sky. v2 keeps the closed-form integral and adds:
- **A second layer** (`fog_layers`): each layer has a density, a height falloff and a height
  offset. Typical use: a thin ground mist plus a high haze. Unreal's exponential height fog
  has the same structure ("Second Fog Data") [S9].
- **In-scattered colour from the light, not a constant.**
  - An **ambient** term: the sky's irradiance from the environment's SH-L2 (already in the
    lighting block), times `fog_albedo`.
  - A **directional** term: the authored sun (§1b) times a Cornette–Shanks phase lobe of
    `fog_anisotropy` (default 0.6) towards the sun. Unreal exposes the same idea as
    "Directional Inscattering" [S9].
  - `fog_color` remains as an optional tint, and a map that sets only today's keys gets
    today's look (a single layer, constant colour) on every path.
- **Two caps.** Today's `fog_max` (off-board), plus **`fog_max_playable`**: the maximum fog
  opacity inside the board rectangle (the fog-of-war rectangle). Its default is the opacity
  today's map settings produce at the far edge of the default gameplay view, so v2 never fogs
  the board more than today without an explicit, tested change.
- **Its jobs.** It is the fog of Tiered **Low**; the **far field** beyond the froxel range on
  Medium and up (§3b), so the two meet without a seam; and the fog of **Compat from C1** (§9).
- A Rust twin (`moba_render::atmosphere::height_fog`) is the oracle for the Slang version now
  and the GLSL version in C1.

### 3. Froxel volumetric fog (Medium and up)

**3a. The approach** is Wronski's frustum-aligned volume [S5] with Hillaire's 2015
energy-conserving integration and temporal reprojection [S6]: inject media and light into a
low-resolution volume aligned with the view frustum, integrate front to back in one compute
pass, and let every opaque and transparent surface read in-scattering and transmittance with a
single filtered 3D fetch.

**3b. The grid, fitted to the board.**
- **160 × 90 × 64**, the roadmap's figure and Wronski's (who used 160 × 90 × 64 or × 128 by
  platform) [S5]. X follows the aspect ratio (`round(90 × aspect)`), so 21:9 is 210 × 90. One
  size on every froxel tier; Ultra may raise Z after measurement (Q10).
- **The Z range is fitted to the ground, not to the near and far planes.** Wronski's volume
  covered 50–128 m [S5]; our camera's ground spans 38–91 m. Each frame the renderer computes
  the view depth of the plane `y = H_top` (the tallest structure on the board, from the map
  bounds; the cores are 18–24 m in the bible) along the bottom frame edge, and of `y = 0` along
  the top edge, clamped to `[1 m, 1,000 m]`, and spreads the 64 slices **exponentially**
  between them. At the default camera that is about 13–91 m, or about 2 m per slice in the
  middle of the board.
- Beyond the far slice, height fog v2 (§2) continues the same media analytically, blended
  over the last slice.
- **Memory** (fixed, at creation): `froxel_scatter` (in-scattered light × density and
  extinction, `RGBA16F`) twice, for the history ping-pong, and `froxel_integrated`
  (accumulated in-scattering and transmittance, `RGBA16F`): 3 × 160 × 90 × 64 × 8 B =
  **22.1 MB**. `maxImageDimension3D` is at least 256 on every Vulkan device [S12]; 3D
  `RGBA16F` storage support is checked with `vkGetPhysicalDeviceImageFormatProperties` at
  admission and recorded in the capability fixtures.

**3c. Passes.**

```text
froxel_scatter     compute, one thread per froxel → froxel_scatter[cur]
                   media (layers + local volumes + weather) × light (sun shadow map, ambient SH,
                   column-grid lights) at a jittered position, blended with the reprojected history
froxel_integrate   compute, one thread per (x, y) column, marching the 64 slices → froxel_integrated
```

- **Neither pass reads the G-buffer.** They need only the shadow map, the light list and the
  previous volume, so they may run right after `shadow`, and are candidates for the async
  compute queue (roadmap §6.8), as Wronski noted [S5].
- **Media** (density and albedo per froxel):
  - the two height-fog layers of §2, evaluated at the froxel;
  - **local fog volumes**: up to 32 boxes or ellipsoids from the map's `[[atmosphere.volumes]]`
    (density, albedo, edge falloff, optional wind-scrolled noise), for the Forge vents, the
    Still Well's dust spiral or the fallen engine's steam. Frostbite has the same "local fog
    volumes" source [S6];
  - the weather state's density multiplier (§6).
  - One octave of wind-scrolled noise is optional per layer, as Wronski used [S5].
- **Light:**
  - **the sun**, through the existing sun shadow map with the compare sampler (ADR-0011 §4), and
    the Cornette–Shanks phase;
  - **ambient**: the sky's SH-L2 (until N3's probes, which then replace it);
  - **local lights** from N2's ground-column grid (same lists, same caps as `deferred_light`,
    ADR-0023 §2b), **unshadowed** on Medium and High;
  - **VFX and ability lights only when flagged** `volumetric` (Q5). Unreal warns that
    fast-changing lights "leave lighting trails" in its reprojected fog [S10]; most VFX lights
    are short flashes, so by default they light surfaces but not the air. At most 16
    volumetric lights per frame.
- **Integration** uses Hillaire's energy-conserving form over each froxel's depth `D`:
  `(S − S·e^(−σt·D)) / σt` for scattered light `S` and extinction `σt` [S6], with the
  transmittance accumulated alongside.

**3d. Ultra: volumetric shadows from local lights.** The roadmap's Ultra cell. N6 does **not**
build per-light shadow maps for it: every tier has ray query (roadmap §1) and N4 builds the
TLAS, as ADR-0023 §3 decided for surface shadows. With N4 present, `froxel_scatter` on Ultra
traces one ray-query shadow ray per froxel for the top 4 volumetric lights by contribution,
with the temporal filter (§4) absorbing the noise. Until N4 merges, **Ultra is High** for
volumetrics, and the preset cell stays `Planned(N6)` with a note (Q9).

**3e. Application.**
- **`deferred_light`** (ADR-0023 §2c step 6): fog of war first, as today, then
  `color × T + S` from `froxel_integrated` at the pixel's view depth (one trilinear fetch),
  then the team rim, still **after** fog (step 7). On Low, the same step calls height fog v2.
- **`sky`**: the volume's far slice along the ray, then AP where enabled (§1c).
- **`water` and `glass`**: the same fetch at their fragment depth, through the shared Slang
  `fog.slang` that ADR-0023 §5b already lists.
- **Particles stay unfogged**, as today (Q4). Fogging them would dim enemy ability effects in
  haze, which the R5 VFX rules (telegraph floors, ally dimming) were tuned without. The one
  exception is `ParticleRole::Weather` (§6g), which sits in the atmosphere and is fogged.
- **The playable cap.** Inside the board rectangle, the applied fog is clamped so opacity
  `1 − T` never exceeds `fog_max_playable` (§2) and the added in-scattering never lifts a pixel
  by more than `fog_lift_playable` in luminance (default 0.05 of the exposure's middle grey,
  tuned by the readability gates). This is ours to enforce: Unreal's volumetric fog, for
  example, does not honour its height fog's "Fog Max Opacity" [S10].

### 4. The temporal answer for volumetrics

**4a. Froxel reprojection, in volume space** (Hillaire 2015 [S6]; Wronski's suggested
extension [S5]):
- Each frame, `froxel_scatter` samples a jittered position inside each froxel. The offset is
  a Halton sequence driven by **ADR-0021's frame index**, with the **same offset for every
  froxel along a view ray**, as Frostbite does [S6].
- The previous `froxel_scatter` is sampled at the froxel's world position projected through the
  previous frame's view-projection and the previous frame's slice mapping (§3b keeps both).
  The current sample is blended in at **5 %** (an exponential moving average, Frostbite's
  value [S6]).
- History outside the previous volume is discarded and the current sample used alone [S6].
- **Resets** follow ADR-0021 §2c exactly: `Scene::history_reset`, and the automatic reset when
  the eye moves more than 8 m or turns more than 15° in a frame. A reset also happens when the
  fitted Z range (§3b) changes by more than 25 % in one frame (a zoom jump).
- **Flagged volumetric lights** use a 25 % current weight in the froxels they touch, which
  trades some noise for shorter trails; whether that is enough is measured (verification
  gate 4, ghost energy).

**4b. With the upscaler.**
- The fog is composited into `hdr` **before** `temporal`, like the rest of the lighting, so
  TAAU, FSR 3.1, DLSS, FSR 4 and XeSS see a fog that is already stable. The volume is smooth
  and world-anchored, so it adds no disocclusion problem of its own.
- **No reactive mask by default.** The volume is already temporally filtered; marking it
  reactive would only make the upscaler discard good history.
- **The fallback** if the ghost-energy gate shows smearing where wind-scrolled fog moves over
  static ground: the `tc_mask` pass (ADR-0021 §2d) writes `min(1, k × fog lift)` there, which
  keeps history but drops locks. Built behind `MOBA_FOG_TCMASK`, off unless the gate needs it.
- **The overlay is untouched.** Outlines, telegraphs, ranges, health bars and the HUD are drawn
  after the upscaler on `ldr` (ADR-0021 §5), so no fog or weather ever covers them, and
  telegraph contrast is evaluated against the fogged background that is actually displayed.

### 5. Clouds as a backdrop

**5a. Where they exist.** Only where the sky is on screen: the low orbit, the showcase and
cinematics (§1d). They are drawn over N2's sky tiles, so at the gameplay and spectator
cameras they cost **nothing**. On Low they are off.

**5b. The technique.** A Nubis-style 2.5D cloud layer [S7]: a slab between two altitudes,
shaped by a coverage-and-type weather map and Perlin–Worley and Worley noise, ray-marched
with the atmosphere's transmittance and sun colour.
- **Resolution and temporal:** the march runs at quarter resolution (half width × half
  height) with reprojection and temporal accumulation, then is upsampled into the sky pass
  with the sky mask. That is the RDR2 approach as Hillaire summarises it ("quarter
  resolution with TAA upsampling ⇒ instant result but looks blurrier"); Nubis 2015/2017
  instead updated one pixel per 4 × 4 tile, with a 16-frame latency [S2]. A backdrop wants
  the instant one. History resets with ADR-0021's resets.
- **Noise volumes** (128³ and 32³ `RGBA8`, 8.1 MiB) are generated at load by a compute pass
  with fixed seeds: no asset, no licence question and no content-format change.
- **Cost:** Nubis rendered "in under 2 milliseconds on the PlayStation 4" in its 2015
  prototype [S7]. Our share is a fraction of the screen at quarter resolution; it is measured
  against the showcase budget, not a gameplay tier.
- Guerrilla has since moved Nubis from 2.5D to voxel clouds (Nubis³, 2023) [S7]. That pays for
  flying through clouds, which a MOBA backdrop never does.

**5c. Clouds at the gameplay camera: in the IBL only.** `env-bake --sky hillaire` can add a
static cloud layer (the same noise, frozen) to the baked cube, so puddles, metals and water
reflect a cloudy sky consistently. RT reflection rays that miss (N5) sample that cube.

**5d. No cloud shadows in gameplay** (Q8). The lighting is baked and fixed (N3), a moving
shadow over the board changes VAL-2 and VAL-3 locally from second to second, and a darkened
patch can read as fog of war. The showcase and cinematics may use them.

### 6. Wetness, puddles, rain and weather

**6a. The weather model: a variant per match, an intensity over match time.**
- **A weather variant** is a named set of overrides on the map's atmosphere, sky, wetness,
  wind and particles, declared in `[[weather.variants]]`. A variant that changes the sun or
  sky (the bible's Dust Veil) **also names its own bakes** (environment, N3 lightmaps), since
  lighting is baked: the bible's own conclusion. The variant is chosen **once, at map load**.
- **The intensity** within a variant (rain or dust strength, wet level, fog density
  multiplier) is a **pure function of the map, the variant and the match time** (from the
  client's render tick, not the wall clock): `weather_at(map, variant, match_time) ->
  WeatherState`, unit-tested. Every client, spectator and replay of a match with the same
  setting sees the same weather at the same moment, and captures are deterministic. Wetness
  dries with a lag after rain stops (Lagarde's drying behaviour [S8]), inside the same
  function.
- **Who chooses the variant:** a client setting, `weather = map | off | <variant>`, in casual
  and custom games. **Ranked and tournament** use the map's `ranked` variant at its fixed
  ranked intensity, with no schedule (§7d). Nothing goes on the wire (Q6).
- **For the vertical slice** the maps ship one variant with constant intensity, as the bible
  recommends; the schedule is built and tested but unused until the art review asks for it
  (Q12).

**6b. What the weather drives:** the fog density multiplier and the local fog volumes' noise
(§3c), the wet level (§6d), the dust amount (ADR-0023 §6a's dust modifier, capped by the
bible's ≤ 20 % on units), rain and dust particles (§6g), ripples (§6e) and the foliage wind
(ADR-0017's `wind`, which the weather state now sets; foliage motion vectors already use the
previous frame's wind, ADR-0021 §2b).

**6c. Inputs on the ground.**
- **A wetness map** per map, in world XZ over the board like the fog-of-war texture:
  `maps/wet/<map>-wet.{toml,ktx2}`, `R8G8`: R the **puddle potential** (where water collects),
  G **static wetness** (seeps and river banks, wet in every weather). It is authored or derived
  from the terrain heightfield's cavities by a `moba-tools wet-bake`, with the same
  fingerprint check as the AO bake. Lagarde's puddles are artist-placed with flood levels
  [S8]; a top-down board makes a 2D map the natural place for them.
- **Rain occlusion.** An orthographic top-down depth of the static occluders (terrain, walls,
  props, structures), 1,024² over the board (about 25 cm per texel on a 256 m board), rendered
  **once at map load**. A surface below an occluder does not get rain-wet. Lagarde describes
  the same depth-map approach and its limits [S8].
- **Porosity**: a new optional material key `porosity` (0–1). When absent it is derived from
  roughness with Lagarde's remap, and metals are not darkened [S8].

**6d. How wetness reaches the N2 material model.** ADR-0023 §6a already places wetness among
the **macro modifiers evaluated when the G-buffer is written**, costing no G-buffer bits. N6
fixes the form:
- **Where:** in the Slang G-buffer fragment stage, before `encode_gbuffer`, for receivers
  (terrain, foliage) and the rest (walls, props, structures, units), and in `gbuffer_decals`
  on the blended values so a scorch mark on wet ground is wet too.
- **Not from compute.** ADR-0023 §1a left room for "N6's wetness pass" to rewrite `gb0` from
  compute, but `gb1` (roughness) cannot be written from compute on the 1.3 floor (ADR-0023
  §7c). Wetness changes both, so it lives in the fragment stage.
- **The wet level** `w = max(G, weather_wet × rain_exposed)`, where `rain_exposed` comes from
  the rain occlusion test. **Units** take at most half the world's wet level and never
  puddles: units carry less weathering than the world (bible §4 rule 4).
- **Darkening and gloss** follow Lagarde [S8]:
  `factor = mix(1, 0.2, (1 − metallic) × porosity)`; `base_color *= mix(1, factor, w)`;
  roughness moves towards water's smoothness by the same rule. The darkening is clamped by the
  map's `wet_darkening_max` so VAL-2 and VAL-3 hold (§7b).
- **Puddles** (receivers only), where the accumulated water `a = R × weather_wet` passes a
  threshold with a soft band (Lagarde's 0.4 transition [S8]): the normal bends towards up, the
  roughness drops to 0.02–0.05, and the surface is encoded as `STANDARD` with the water F0
  of 0.02 (IOR 1.33) [S8]: `gb2.r = sqrt(0.02 / 0.16) ≈ 0.354` in ADR-0023 §6c's `0.16 x²`
  encoding. Basalt's "dry albedo × 0.7, roughness 0.10–0.30 where wet" from the bible is the
  test case.
- **Reflections in puddles** come from whatever the tier has: the Hillaire IBL now, N3's probes,
  SSR on Medium and RT on High and up from N5 (roadmap §6.4).
- **Low:** static wetness (G) and puddles, no ripples: the roadmap's "puddle masks".

**6e. Ripples (Medium and up).** A procedural, animated ripple normal map: rings baked into a
tiling texture and animated in layers that switch on as rain intensity rises, as Lagarde does
[S8]. One 256² `RGBA8` texture is regenerated per frame by a tiny compute pass while it rains
(Lagarde measured 0.14–0.15 ms for 256² on PS3/Xbox 360 [S8]), and sampled in world XZ where
puddles are.

**6f. Ripples and the upscaler.** Ripples are animated shading on static geometry: exactly what
ADR-0021 §2d's `tc_mask` is for. The `tc_mask` pass gains a fullscreen sub-draw that
reconstructs each pixel's world XZ from depth, skips `UNIT` pixels (`gb1.a`), and writes the
puddle ripple strength with `MAX` blending. Puddles keep history and drop locks, as water does.

**6g. Rain and dust particles.**
- A new `ParticleRole::Weather`: camera-relative emitters around the camera focus, density
  proportional to the intensity, killed below the rain-occlusion height in `vfx_simulate`.
- **A separate budget share**, so weather never eats into the ability VFX budgets of R5 and
  ADR-0013. The ranked lock caps it (§7d).
- They write `reactive` like every particle (ADR-0021 §2d); they are lit by the key light and
  ambient like particles today; and they are the only particles that take fog (§3e).
- They are never compacted into the overlay and never drawn post-upscale.

**6h. Streaks are not built** (Q7). The roadmap cites Lagarde 2013 for "ripples and streaks",
but Lagarde's rain article argues that water streams on walls are unrealistic and does not use
them [S8]. Vertical surfaces get wetness by exposure (§6d). Streaks can come back if the art
review asks for them, as a content mask, not a new shader model.

### 7. Readability: fog and weather never hide gameplay

**7a. What is safe by construction.**
- **Telegraphs, outlines, range indicators, selection, health bars and the HUD** are drawn
  after the upscaler on the displayed image (ADR-0021 §5). No fog, cloud, rain or wetness
  reaches them, and the telegraph rim rule is evaluated against the fogged `ldr` it actually
  sits on.
- **Team rims** are added after fog with an exposure-independent floor (ADR-0023 §8a, RIM-2).
- **Fog of war** is game information drawn by its own rule (ADR-0014 §3), applied before the
  atmospheric fog as today.
- **Ability VFX** stay unfogged (§3e), and weather particles have their own budget (§6g).
- **Hidden information:** the froxel volume reads only the shadow map, the light list and the
  map's media. All of them are built from `Scene`, which holds only what the server sent
  (ADR-0016 §2). Weather depends only on the map, the variant and the match time. N6 adds no
  new channel through which a client could learn anything.

**7b. What is capped and tested.**
- The **playable fog cap** (§3e) and `fog_max_playable` (§2).
- The **wet darkening cap** (§6d), so VAL-2 (mean L 0.30–0.58) and VAL-3 (shadow floor
  L ≥ 0.18) hold on wet ground.
- **CON-5 decides the caps.** A higher tier may never lower CON-1 to CON-4 below the Low
  result minus 0.02. Low runs analytic fog that today barely touches the board (`fog_start`
  80 m), so the froxel fog over playable space must stay light: a light shaft or a vent
  plume, not a blanket. That is a design consequence, not an accident.
- **A new proposed rule, FOG-1** (Q3): the luminance difference between seen and unseen ground
  across the fog-of-war edge, in every tier and weather state, stays at or above the Low,
  clear-weather value minus 0.02. Atmospheric fog lifts dark areas, and fog of war is a
  darkening, so haze is the one effect that can erase the fog-of-war edge.
- **No weather may look like fog of war:** a weather variant's fog colour and the fog-of-war
  colour must differ by ΔE_OK ≥ 0.10, checked by `content-check`.

**7c. Debug views.** `MOBA_DEBUG_VIEW` gains `froxel-density|froxel-scatter|transmittance|
fog-cap|wetness|puddles|rain-occlusion|sky-luts`. `fog-cap` paints the pixels where the
playable cap clamped, and joins roadmap §4's readability heatmap.

**7d. The competitive profile** (roadmap §4 rule 6: "weather and volumetric density are
capped"). When the client's match is ranked or tournament it sets
`AtmosphereConfig::competitive` (the same match-mode switch the frame-generation and DLSS 5
locks use; if that switch does not exist yet, N6 adds the render-side flag and the client sets
it from the match mode):
- the map's `ranked` variant at its fixed ranked intensity, no schedule;
- fog density and local-volume density capped at the map's `[weather.ranked]` values (the
  bible's Dust Veil gives 0.004 / 0.45);
- weather particle density capped;
- the setting cannot be changed during the match.

### 8. Per-tier switches and costs

**8a. The matrix** (roadmap §3, `preset.rs` rows "Atmosphere" and "Weather and wetness"):

| | Low | Medium | High | Ultra | Overdrive |
|---|---|---|---|---|---|
| Sky | Hillaire LUTs | same | same | same | same |
| Aerial perspective | only on cameras that see past 1 km | same | same | same | same |
| Fog | height fog v2 | froxels + v2 far field | same | + local-light volumetric shadows (from N4) | as Ultra |
| Volumetric VFX lights | — | flagged, ≤ 16 | same | same | same |
| Clouds | off | backdrop only (sky tiles) | same | same | same |
| Wetness | static wetness and puddles | + rain wetness, ripples | same | same | same |
| Weather particles | off | on (capped) | same | same | same |

Overdrive keeps the froxels until N9 decides whether its path tracer takes over participating
media; that is N9's ADR.

**8b. N6's share of ADR-0020 §5b.** Planning estimates on each tier's reference GPU at its
internal resolution, at the **gameplay camera**. They are to be replaced by measurements;
estimates never close a target.

| Pass (ms) | Low (RTX 2060, 720p) | Medium (RTX 3060, 720p) | High (RTX 4070, 960p) | Ultra (RTX 5070 Ti, 960p / 1080p) |
|---|---|---|---|---|
| Sky LUTs, sky, AP, clouds | ≈ 0 (static LUTs, no sky tiles) | ≈ 0 | ≈ 0 | ≈ 0 |
| Fog: v2 in `deferred_light` / froxels | 0.05 | 0.45 | 0.45 | 0.4 (+ 0.25 local-light shadows from N4) |
| Fog lookups (`deferred_light`, `water`, `glass`) | in the above | 0.05 | 0.05 | 0.05 |
| Wetness in the G-buffer, ripples, `tc_mask` sub-draw | 0.05 | 0.1 | 0.1 | 0.1 |
| **N6 total** | **0.1 of 12.0** | **0.6 of 6.0** | **0.6 of 7.5** | **0.8 of 9.0** |

- **Where the numbers come from.** Wronski's 160 × 90 × 64 volume cost 1.1 ms in total on Xbox
  One, 0.43 ms of it lighting the volume [S5]. Frostbite's 8 × 8-pixel froxels with 64
  slices cost 2.95 ms on PS4 at 900p, and 0.83 ms with 16 × 16 tiles [S6]. Unreal quotes about
  1 ms on PS4 at High and about 3 ms on a GTX 970 at Epic with eight times the voxels [S10].
  A 3060-class GPU is several times those consoles; our volume is Wronski's size.
- **Medium stays the tight tier:** N2's 2.7 ms plus N6's 0.6 leaves 2.7 ms of the 6.0 ms
  for shadows, SSGI, SSR, the upscaler, post, the overlay and the UI.
- **The showcase, cinematics and low orbit** add the sky (Hillaire: 0.14 ms to render the sky
  and atmosphere at 720p on a GTX 1080, 0.31 ms with all four LUTs rebuilt [S2]), aerial
  perspective and clouds. They are measured against the showcase configuration's own budget.
- **Roadmap §6.8's High example** ("Fog and sky 0.5") is within 0.1 ms of this.

**8c. VRAM.** Froxels 22.1 MB, cloud noise 8.1 MiB and cloud targets (quarter resolution,
two `RGBA16F` for history: 6.6 MB at High's 960p ceiling), on Medium and up. On every tier:
the LUTs 0.55 MiB, the wetness map and rain occlusion about 2.5 MiB, ripples 0.25 MiB. About
**40 MB on Medium and up, 3.3 MB on Low**, inside ADR-0020 §5e's budgets, and reported in
`docs/perf/`.

### 9. Compat's share (C1)

The C1 row takes "Hillaire sky and improved height fog from N6". N6 delivers the
specification, the Rust oracle and the content; **the C1 PR wires them into Compat**, and
that PR rebaselines Compat's goldens deliberately.
- **Sky.** The CPU-built sky-view LUT (§1c), uploaded at load, sampled by `sky.frag`. The sun
  is fixed per map, so a static LUT is exact for gameplay.
  - **The sampler budget.** Compat's scene layout is full (16 of 16, ADR-0016 §6). The limit is
    per pipeline layout, so the sky pipeline gets its own layout with one extra binding; the
    scene pipelines are unaffected. If that is not possible, C1 falls back to sampling the
    Hillaire-baked environment cube at its sharpest mip, as today's Medium-and-up sky does.
- **Height fog v2** (§2), ported to GLSL in `atmosphere.glsl`, checked against the Rust twin.
  It needs uniforms only (the second layer, anisotropy, the playable cap); the ambient term
  uses the SH already in the lighting block. **No new sampler.**
- **The environment.** Compat switches from `[lighting] environment` to the
  `[atmosphere.sky] environment` bake (§1f), so its IBL and its sky agree.
- **Not in Compat:** froxels, clouds, aerial perspective, ripples, rain wetness, weather
  particles and weather variants. Compat renders a map's default variant, dry.
- **Content-format rule** (ADR-0020 §2d): every N6 content addition has a Compat fallback in
  its own PR. The new `[atmosphere]` keys are ignored by Compat until C1 (today's keys keep
  today's look), `[atmosphere.sky]`, `[[atmosphere.volumes]]`, `[weather]` and the wetness map
  are ignored, and `porosity` is ignored. Static seeps that must read as wet on Compat use a
  `[compat]` material override (ADR-0023 §10), as the bible's wet basalt allows.

### 10. The tiered frame from N6

```text
(load: atmo_transmittance, atmo_multiscatter (CPU); cloud_noise; rain_occlusion)
(portraits, minimap) → (fow) → vfx_simulate → (shadow)
→ (froxel_scatter → froxel_integrate)         Medium+; no G-buffer dependency
→ (atmo_sky_view, only when the sun or altitude changed) → (atmo_aerial, far-seeing cameras only)
→ (ripples, Medium+ while raining)
→ gbuffer_receivers → gbuffer_decals → gbuffer_rest   (+ wetness, puddles)
→ (contact_shadows, ao, ao_blur)
→ light_classify
→ (clouds → clouds_resolve, only when sky tiles exist)
→ deferred_light   … fog of war, fog (v2 on Low, froxel lookup Medium+, playable cap), team rims → hdr
→ sky              sky tiles only: sky-view LUT, sun disk, clouds, far fog / AP
→ (water_copy → water) → (glass) → vfx_draw (weather particles fogged) → (vfx_resolve)
→ (distortion_copy → distortion) → tc_mask (+ puddle ripples)
→ temporal → bloom, exposure → composite → overlay_prepass → overlay → overlay_vfx → ui
```

- **Graph limits.** N6 adds up to ten passes and about a dozen resources over N2's topology.
  ADR-0023 §11 already provides for widening the resource mask from `u64` to `u128` in the
  same PR if the largest tiered topology needs it; the topology test compiles every variant.
- **Bindless.** Every N6 compute pass binds only the global set and a push block. The 3D
  volumes are storage-image and sampled-image handles; 3D views use a typed alias of the
  binding, with the typed 3D fallback that ADR-0020 Amendment 5 anticipates.
- **Off switches.** `MOBA_SKY=off|gradient|hillaire`, `MOBA_FOG=off|analytic|froxel` (today's
  `off` keeps its meaning), `MOBA_VOLUMETRIC_LIGHTS=off`, `MOBA_CLOUDS=off`,
  `MOBA_WEATHER=off|map|<variant>`, `MOBA_WETNESS=off`, `MOBA_FROXEL_HISTORY=off` (for
  debugging) and `MOBA_FOG_TCMASK` (§4b).

### 11. Determinism and the invariant

- Rendering still never touches the simulation, the wire, replays or the state hash.
  `PROTOCOL_VERSION` v42, replay v5, `SIM_LOGIC_VERSION` v9, content hashes and
  `tests/golden-state-hash.txt` are unchanged.
- The atmosphere, the weather variants, the wetness map and the material `porosity` key are
  map and material visuals: client-local, never hashed, signed or sent
  ([content-authoring.md](../content-authoring.md)).
- The weather state is a pure function of data the client already has (map, variant setting,
  match time). Nothing reads back from the GPU into gameplay. Picking intersects the ground
  plane through the client's own camera (ADR-0021 §8) and is unaffected by fog, clouds,
  puddles or particles.
- **If weather is ever made gameplay** (vision ranges, movement, anything the server decides),
  that is a new ADR with server-authoritative weather, a simulation-logic bump and wire and
  replay changes. This ADR does not cover it.

## Departures from the roadmap

| Roadmap says | This ADR decides | Why |
|---|---|---|
| §6.5: Hillaire LUT sky, "~0.1–0.2 ms" | The static LUTs are built on the CPU at load; the sky-view LUT only when the sun or altitude changes; the sky is drawn only over sky tiles (§1c, §1d) | The gameplay camera never sees the sky, and the sun is fixed per map; per-frame cost at gameplay is about zero |
| §6.5: froxel fog "~160×90×64" | 160 × 90 × 64, with Z fitted each frame to the ground's depth range, exponential within (§3b) | The board spans 38–91 m of view depth; slices spent on 0–38 m or on the sky are wasted |
| §3 matrix, Ultra: "+ volumetric shadows from local lights" | From ray-query shadow rays once N4's TLAS exists; Ultra is High for volumetrics until then (§3d) | The same reasoning ADR-0023 §3 applied to surface shadows; no shadow-map atlas to throw away |
| §6.5: clouds "Nubis-style, quarter-res" | Nubis-style shapes, quarter-resolution march with RDR2-style reprojection, only over sky tiles; a static layer in the IBL; no cloud shadows in gameplay (§5) | A backdrop wants an instant result; the 1-in-16 update has a 16-frame latency [S2]. Moving shadows on a baked, readability-budgeted board are a risk (Q8) |
| §6.5: "streaks (Lagarde 2013)" | No streaks in N6 (§6h) | Lagarde argues against them [S8]; vertical surfaces get exposure wetness |
| §3 matrix: weather per tier, with no model of what "weather" is | Variants chosen at load, intensity as a deterministic function of match time; a competitive profile (§6a, §7d) | Lighting is baked per map (the bible's conclusion); ranked must be fixed (roadmap §4 rule 6) |
| §6.5: froxels "shadowed by the sun map and clustered lights" | VFX and ability lights light the air only when flagged `volumetric` (§3c, Q5) | Reprojected fog leaves trails behind fast-changing lights [S10] |
| Implicit: fog applies to every transparent | Particles stay unfogged, except weather particles (§3e, Q4) | Ability VFX readability (R5 rules) is tuned unfogged |

## Risks

| Risk | Mitigation |
|---|---|
| Fog over the board lowers unit or fog-of-war contrast | The playable cap and lift cap (§3e); CON-5 against Low and the proposed FOG-1 (§7b); the competitive profile; captures in every weather variant |
| Froxel history trails behind moving or flashing lights | Lights flagged `volumetric` only, a 25 % current weight for them, resets from ADR-0021, the ghost-energy gate; trails would look like ability residue, so the default leaves VFX lights out (Q5) |
| Double temporal filtering (froxel EMA, then the upscaler) makes fog changes laggy | Fog changes are slow by design (weather schedules over minutes); `MOBA_FOG_TCMASK` if the gate shows smearing (§4b) |
| Light leaking through thin walls in a 1.3 m froxel | The sun shadow is sampled at the jittered froxel position and filtered over time; local fog volumes keep density off thin structures; reviewed on the wall-heavy overview capture |
| The Z fit changes during a zoom and smears history | Reset when the range changes by more than 25 % in a frame (§4a) |
| The sky and IBL drift apart, or N3's bake uses a different sky | One Rust model for `env-bake`, `light-bake`, Compat and the oracle (§1e); `content-check` fingerprints (§1f) |
| Wet ground pushes VAL-2/VAL-3 or makes dark Void units vanish on dark wet basalt | `wet_darkening_max`, units at half wetness and no puddles, CON-1/CON-2 captures in the wettest ranked state |
| Ripples ghost under FSR or DLSS | `tc_mask` sub-draw (§6f), as for water; ghost-energy gate with rain on |
| Weather particles clutter the frame and compete with ability VFX | Their own budget share and the competitive cap (§6g, §7d) |
| A weather variant without its own bakes looks wrong (a sunny bake under an overcast sky) | `content-check` requires a variant that changes the sun or sky to name its environment and, from N3, its lightmaps |
| The reference code's per-file "All Rights Reserved" headers contradict its MIT `LICENSE` | We rely on the repository's MIT licence (Q2) and keep the notice; if the maintainer prefers, a clean implementation from the paper avoids the question |
| N5 removes the sun shadow map on High and up (RT sun shadows) | The froxels need it: N5 must keep a sun shadow map (possibly 1024²) for the volume on tiers where RT replaces it for surfaces; recorded here for N5's ADR |
| 3D `RGBA16F` storage unsupported on some admitted device | Checked at admission (§3b); a device without it runs height fog v2 on every tier, logged |
| Graph resource count exceeds the `u64` mask | Widened to `u128` as ADR-0023 §11 provides |

## Alternatives considered

- **Bruneton's precomputed scattering** [S4] (BSD-3-Clause code). High-dimensional 4D LUTs
  with horizon artefacts and a cost linear in the number of scattering orders, which Hillaire's
  method was designed to avoid [S1][S2]. Rejected.
- **An analytic sky model (Preetham, Hosek–Wilkie).** Cheap, and it has the turbidity the
  bible names, but it covers only ground views, a limited range of appearances, and no aerial
  perspective [S2]; and it cannot match a non-Earth or smoke-laden sky. Rejected.
- **Keep the gradient sky and bake the IBL from an HDRI.** The bible lists CC0 Poly Haven
  HDRIs as references, but none is a dust storm, an HDRI fixes the sun's position and colour
  per capture, and every map's mood would depend on finding one. The analytic model is
  parameterised per map. HDRIs stay references only.
- **Port Bevy's atmosphere** (WGSL, Bevy 0.16 credits Hillaire's EGSR 2020 paper; the
  repository is `MIT OR Apache-2.0`) [S11]. Permissive and in Rust's ecosystem, but WGSL and
  tied to Bevy's renderer; Hillaire's MIT HLSL is the closer source for Slang. Kept as a
  cross-check.
- **Ray-march the fog per pixel** (no froxels). Cost scales with resolution and every
  transparent layer needs its own march; froxels are resolution-independent and serve opaque
  and transparent surfaces from one fetch [S5]. Rejected.
- **Froxels on Low.** The Low budget could afford them (12 ms), but Low is the readability
  baseline for CON-5 and the roadmap's matrix gives Low analytic fog. Rejected.
- **Unreal-style exponential height fog plus volumetric fog as is.** The ideas are taken (§2,
  §3), the code is not (Epic's EULA), and its volumetric fog ignores the height fog's maximum
  opacity [S10], which our readability cap needs. Rejected as a dependency.
- **Wetness as a compute pass rewriting the G-buffer.** `gb1` storage is not mandatory on the
  1.3 floor (ADR-0023 §7c). Rejected (§6d).
- **Screen-space rain occlusion from the sun shadow map.** It sees the sun's direction, not
  the rain's, and it moves with the shadow frustum. A load-time top-down depth is exact for a
  static board. Rejected.
- **Voxel clouds (Nubis³).** For flying through clouds [S7]; a MOBA backdrop never does.
  Rejected.
- **A dynamic time of day.** The lighting is baked and fixed per map (roadmap §1, N3), so a
  moving sun would contradict the GI. Weather variants with their own bakes cover the need.
  Rejected for gameplay; the showcase may animate its own scene.

## Consequences

- **Wire, replays and determinism: no change** (§11).
- **Frame shape.** The tiered frame gains `froxel_scatter`, `froxel_integrate`,
  `atmo_sky_view`, `atmo_aerial`, `ripples`, `clouds`, `clouds_resolve`, plus load-time
  `cloud_noise` and `rain_occlusion`; `sky` becomes a sky-tile compute pass. Compat's frame is
  unchanged until C1.
- **Shaders.** New Slang: `atmosphere.slang`, the LUT and froxel passes, `fog.slang`'s froxel
  and v2 paths, `wetness.slang` (used by the G-buffer fragments and decals), ripples, clouds
  and the cloud noise generator. C1 adds the GLSL height fog v2 and the sky-view sampling.
- **Content** (map visuals, client-local): `[atmosphere]` gains `fog_layers`, `fog_albedo`,
  `fog_anisotropy`, `fog_ambient`, `fog_max_playable` and `fog_lift_playable`;
  `[atmosphere.sky]` (§1a, `environment`); `[[atmosphere.volumes]]`; `[weather]` with
  `[[weather.variants]]` and `[weather.ranked]`; `maps/wet/<map>-wet.{toml,ktx2}`. Materials
  gain `porosity`. Every key is optional; maps without them render as today on Compat and
  with Earth defaults on Tiered. `content-check` gains the ranges, the bake fingerprints, the
  variant-bake rule and the fog-colour vs fog-of-war rule (§7b).
- **Tools.** `env-bake --sky hillaire` (§1f) and `wet-bake` (§6c).
- **Public API** (additive, `#[non_exhaustive]`): `AtmosphereConfig` (with `competitive`),
  `SkyModel`, `AtmosphereParams`, `FogLayer`, `FogVolume`, `WeatherState`, `WeatherVariant`,
  `moba_render::atmosphere::{weather_at, height_fog, lut builders}`, `Scene::weather`,
  `VfxLight::volumetric`, `ParticleRole::Weather`; in `moba_assets`, `MapAtmosphere`'s new
  fields, `MapSky`, `MapWeather`, `WetnessMap` and `MaterialDesc::porosity`.
- **Licences.** `THIRD_PARTY.md` gains the MIT notice of Hillaire's reference implementation
  (Q2). No vendor SDK is involved in N6.
- **Docs.** `docs/content-authoring.md` (the new keys and bakes), `docs/status.md`,
  `docs/architecture.md`'s renderer section, `docs/screenshots.md` (debug views) and
  `preset.rs`'s cells move with the code.
- **Cross-phase constraints recorded here:** N3's `light-bake` uses the Rust sky model (§1f);
  N5 keeps a sun shadow map for the froxels (Risks); N9 decides whether the path tracer
  replaces the froxels on Overdrive (§8a).
- **Work estimate.** The roadmap's **M**, in six PRs, off the critical path:
  1. **N6a:** the Rust atmosphere model and LUT builders; the static LUTs at load; the Slang
     sky-view and AP passes; the sky over sky tiles on Tiered; `env-bake --sky hillaire` and its
     `content-check` fingerprint; `[atmosphere.sky]`. **Unblocks C1's sky.**
  2. **N6b:** height fog v2 (Rust twin and Slang) on Tiered Low and as the far field; the
     playable caps. **Unblocks C1's fog.**
  3. **N6c:** the froxel volume (scatter, integrate, apply), temporal reprojection, local fog
     volumes, flagged volumetric lights, Medium and High; the readability gates for fog.
  4. **N6d:** the weather model and schedule, the wetness map and `wet-bake`, rain occlusion,
     wetness and puddles in the G-buffer, ripples and the `tc_mask` sub-draw, weather
     particles, the competitive profile.
  5. **N6e:** backdrop clouds and the static IBL cloud layer.
  6. **N6f** (after N4): Ultra's local-light volumetric shadows by ray query.

  N6a and N6b are what C1 needs; N6c closes the Medium "froxel volumetrics" cell; N6d closes
  the weather row.

## Verification and evidence gates

These follow roadmap §10 and the gates of ADR-0020, ADR-0021 and ADR-0023, applied to N6.

1. **The model and its twins.**
   - The Rust LUT builders against reference values: transmittance at the zenith and the
     horizon for the Earth defaults, the multiple-scattering LUT's energy for an
     all-scattering, non-absorbing medium (bounded and monotonic in density), and a sky-view
     sample against a brute-force CPU march of the same medium.
   - The Slang sky-view and AP LUTs on lavapipe match the Rust builders within a recorded
     tolerance (relative error ≤ 1 % where the value exceeds 1 % of its maximum).
   - Height fog v2's Slang output matches its Rust twin within 1/255 over a grid of view
     rays, heights and layers; with today's keys only, it reproduces today's
     `apply_height_fog` within 1/255.
   - The froxel integration of a homogeneous, constant medium matches Beer–Lambert's closed
     form within 1 %.
   - The CPU LUT build time at load is recorded.
2. **Determinism.** `weather_at` is pure: the same inputs give the same state, and two
   lavapipe runs of every N6 golden are byte-identical with validation and sync validation
   clean. A replay run with weather `off`, `map` and each variant gives the same state-hash
   stream and the same `golden-state-hash.txt`; a client test gives byte-identical
   `WireCommand`s from the same cursor with fog, clouds, wetness and weather on and off.
3. **Goldens.**
   - Every Compat golden is byte-identical (`MOBA_RENDER_PATH=compat`) in every N6 PR.
   - New tiered goldens at small resolution with ADR-0021's warm-up: Low sky and fog v2;
     Medium froxels with a light shaft and a local fog volume; a low-orbit sky with aerial
     perspective and clouds; a wet scene (static seep, puddles, frozen ripples); and the
     competitive Dust Veil-style variant.
   - Tiered goldens that change (the Low fog, the sky in reflections) are rebaselined in the
     PR that changes them, with the reason.
4. **Readability (merge-gating).**
   - **CON-1, CON-2, CON-4 and CON-5** on the standard captures at every tier, both team
     palettes, rims on, in **every weather variant a map ships** at its casual maximum and its
     ranked intensity; no tier below Low − 0.02 in the same variant.
   - **CON-3** (telegraph contrast) in the densest ranked state.
   - **VAL-2 and VAL-3** on the rendered ground, dry and at the wettest state.
   - **FOG-1** (proposed, §7b): the fog-of-war edge contrast at every tier and weather state
     ≥ the Low clear-weather value − 0.02.
   - **RIM-2** re-run with froxels and the densest ranked fog.
   - **Ghost energy** (ADR-0021 gate 3) re-run with froxels on, a flagged volumetric light on a
     dashing hero, rain ripples on, and weather particles on.
   - **The competitive profile**: a unit test that ranked clamps density, intensity and
     particles regardless of the user's setting.
   - **Hidden information:** `outline-probe.sh` re-run with N6 on, unchanged.
5. **Debug views** ship with the passes they show, in `docs/screenshots.md`.
6. **Performance.**
   - Per-pass lanes for every N6 pass in the frame log; medians of 3 interleaved rounds
     against a detached build of `origin/main`.
   - On the RTX 5070 Ti, on the teamfight replay at each preset's output resolution: each
     lane under its §8b column scaled by the 5070 Ti / reference-GPU ratio from the R8 perf
     logs, labelled as an estimate, and the froxel lanes together **≤ 0.2 ms at Medium and
     High** (a proposed absolute gate: today's whole 1080p frame on the 5070 Ti is about
     0.5–0.7 ms, so a relative gate would be meaningless).
   - The low-orbit and showcase cameras: sky, AP and clouds lanes recorded.
   - The Radeon 610M (RADV) runs the froxel and LUT passes with validation clean, as an AMD
     correctness smoke for 3D storage images; not a performance reference.
   - Tiered budgets keep failing `perf-gate`. `locked_uploads=0` and `locked_pipelines=0`
     hold, including across a weather schedule change and a camera that moves from gameplay to
     low orbit.
   - VRAM against §8c.
7. **Compat contract (for C1).** A test builds Compat's CPU sky-view LUT and height fog v2
   inputs for every shipped map, and `content-check` passes every map's new keys with Compat
   fallbacks.

## Open questions for the maintainer

1. **Keep the authored `[lighting] sun_color × sun_intensity` as the light reaching the board,
   and derive the sky's top-of-atmosphere sun from it (§1b)?** *Recommendation:* yes. The board
   lighting stays exactly what the art bible's moods tune, and the sky agrees with it.
2. **Port the parts of Hillaire's MIT reference implementation we use (with its notice), or
   write a clean implementation from the paper?** *Recommendation:* port, from the MIT files
   only, with the notice in `THIRD_PARTY.md`; skip the vendored Bruneton files. The per-file
   "All Rights Reserved" headers sit under a repository MIT licence; if you would rather not
   rely on that, the clean route costs about a week.
3. **Add FOG-1 (fog-of-war edge contrast) to the art bible's readability rules?**
   *Recommendation:* yes. Haze is the one effect that can erase the fog-of-war edge, and the
   bible has no rule for it yet. It is your bible; N6 can run it as a gate either way.
4. **Particles stay unfogged, except weather particles?** *Recommendation:* yes, as today.
   Fogging ability VFX would weaken R5's telegraph floors in haze.
5. **VFX and ability lights light the fog only when flagged `volumetric` (≤ 16 per frame)?**
   *Recommendation:* yes, opt-in, so flashes do not leave trails in the air. Persistent
   zones and map lights are the natural candidates.
6. **The weather variant is a client setting in casual and custom games, and locked to the
   map's ranked variant in ranked and tournament, with nothing on the wire?**
   *Recommendation:* yes. Weather is presentation; players choosing differently see different
   skies, as with any graphics setting.
7. **Skip water streaks on walls in N6?** *Recommendation:* yes; Lagarde argues against them,
   and vertical surfaces still get wet. Revisit if the art review wants them.
8. **No cloud shadows in gameplay (showcase and cinematics only)?** *Recommendation:* yes.
   The lighting is baked and fixed, and a moving dark patch can read as fog of war.
9. **Ultra's local-light volumetric shadows wait for N4's ray query, with Ultra equal to High
   for volumetrics until then?** *Recommendation:* yes, as ADR-0023 Q1 did for surface
   shadows.
10. **One froxel grid (160 × 90 × 64, Z fitted to the ground) on every froxel tier, tuned only
    after measurement?** *Recommendation:* yes.
11. **Replace the art bible's "sky turbidity" targets with the model's `mie` (aerosol)
    multiplier?** *Recommendation:* yes; Hillaire's model has no turbidity parameter. The
    moods' other physical targets (sun elevation, illuminance, EV) carry over unchanged.
12. **For the vertical slice, ship one weather variant per map with constant intensity, and
    leave the in-match schedule built but unused until the art review?** *Recommendation:*
    yes, as the bible recommends ("none for the slice").

## Corrections to the roadmap

These were re-checked on 2026-11-03. This PR only adds a link to this ADR from roadmap §12;
the maintainer may fold the corrections in when this ADR is accepted.

- **§6.5, "Hillaire 2020 LUT sky (~0.1–0.2 ms)".** Hillaire's own figures on a GTX 1080 are
  0.01 (transmittance), 0.05 (sky-view), 0.04 (aerial perspective) and 0.07 ms
  (multiple scattering) for the LUTs, plus 0.14 ms to render the sky and atmosphere at 720p:
  **0.31 ms in total** [S2]. With a fixed sun and a camera that never sees the sky, our
  per-frame cost at gameplay is about zero (§1c, §1d).
- **§6.5, "~160×90×64".** Wronski used 160 × 90 × 64 **or 160 × 90 × 128** depending on the
  platform, over 50–128 m, at **1.1 ms on Xbox One** [S5]. Frostbite's default is 8 × 8-pixel
  froxels with 64 slices (2.95 ms on PS4 at 900p; 0.83 ms with 16 × 16) [S6].
- **§6.5, "Nubis-style, quarter-res".** Nubis 2015/2017 updated one pixel per 4 × 4 tile per
  frame (16-frame latency); quarter resolution with TAA is the RDR2 approach (Bauer 2019),
  both as summarised by Hillaire [S2]. Guerrilla moved from 2.5D to voxel clouds in 2023
  (Nubis³) [S7].
- **§6.5, "ripples and streaks (Lagarde 2013)".** Lagarde's ripples are a procedural animated
  normal map, his rain occlusion is a depth map from the rain direction, and he explicitly
  does **not** do water streaks on walls, which he finds unrealistic ("Water drop 2b", 2013-01-03)
  [S8]. The wet-surface model (porosity darkening, gloss, puddles with water's F0 0.02) is
  "Water drop 3b" (2013-04-14) [S8].
- **§3 matrix, Hillaire sky on Low.** Correct, but note that the gameplay and spectator
  cameras never see the sky (pitch 0.95 and 0.92 rad, 60° vertical FOV). At gameplay the sky
  reaches the screen only through reflections and the IBL.
- **Art bible (`lighting-mood.md`, not the roadmap), "sky turbidity (Hillaire/Preetham)".**
  Turbidity is a Preetham/Hosek–Wilkie parameter; Hillaire's model uses Rayleigh, Mie and
  ozone coefficients [S3]. See Q11.
- **Confirmed unchanged:** Hillaire's paper is "A Scalable and Production Ready Sky and
  Atmosphere Rendering Technique", Computer Graphics Forum 39(4):13–22, EGSR 2020 [S1]; its
  reference code is MIT [S3]; Unreal's volumetric fog uses heavy temporal reprojection with
  per-frame sub-voxel jitter [S10].

## Sources

All accessed 2026-11-03.

- [S1] S. Hillaire, "A Scalable and Production Ready Sky and Atmosphere Rendering Technique",
  Computer Graphics Forum 39(4):13–22, EGSR 2020, DOI 10.1111/cgf.14050:
  <https://api.crossref.org/works/10.1111/cgf.14050>, author's copy listed at
  <https://sebh.github.io/publications/> (<https://sebh.github.io/publications/egsr2020.pdf>).
  The paper's PDF was too large to read through the fetch tool and the publisher's copy
  returned 403, so the paper's **internal details** (its exact parameterisations and tables)
  are taken from [S2] and [S3], not from the paper itself; anything attributed to the paper
  alone is **unverified**.
- [S2] S. Hillaire, SIGGRAPH 2020 course "Physically Based Shading in Theory and Practice",
  Unreal Engine atmosphere talk, slides with notes (LUTs, the non-linear sky-view latitude
  mapping, the aerial-perspective froxel volume, the GTX 1080 and iPhone 6s timing table on
  slide 22, and the cloud previous-work summary of Schneider 2015/2017 and Bauer 2019 on
  slide 25): <https://blog.selfshadow.com/publications/s2020-shading-course/hillaire/s2020_pbs_hillaire_slides.pdf>.
  The talk's exact title is **unverified**. Bauer 2019 (RDR2) is cited only through this
  summary.
- [S3] `sebh/UnrealEngineSkyAtmosphere` (reference implementation; repository `LICENSE` MIT,
  "Copyright (c) 2020 Epic Games, Inc."; `Application/SkyAtmosphereCommon.{h,cpp}` for the
  Earth parameters and the 256 × 64 transmittance LUT, `Application/Game.{h,cpp}` for the
  32² multiple-scattering, 192 × 108 sky-view and 32³ `RGBA16F` aerial-perspective
  resources, `Resources/RenderSkyRayMarching.hlsl` for 32 slices at 4 km;
  `Resources/Bruneton17/` under Bruneton's BSD-3-Clause notice; last pushed 2022-09-11):
  <https://github.com/sebh/UnrealEngineSkyAtmosphere>,
  <https://raw.githubusercontent.com/sebh/UnrealEngineSkyAtmosphere/master/LICENSE>
- [S4] E. Bruneton, "Precomputed Atmospheric Scattering: a New Implementation" (2017), as
  vendored in [S3] under BSD-3-Clause; the 2008 paper's LUT approach and its limits as
  summarised in [S2].
- [S5] B. Wronski (Ubisoft Montreal), "Volumetric Fog: Unified Compute Shader-Based Solution to
  Atmospheric Scattering", SIGGRAPH 2014 Advances in Real-Time Rendering (160 × 90 × 64 or
  × 128 volumes, exponential depth, 50–128 m range, Xbox One performance table: total 1.1 ms,
  lighting the volume 0.43 ms; temporal reprojection and jitter proposed as future work):
  <https://advances.realtimerendering.com/s2014/index.html>,
  <https://advances.realtimerendering.com/s2014/wronski/bwronski_volumetric_fog_siggraph2014.pdf>
- [S6] S. Hillaire (EA Frostbite), "Physically-based & Unified Volumetric Rendering in
  Frostbite", SIGGRAPH 2015 Advances in Real-Time Rendering (8 × 8 tiles and 64 slices by
  default; `RGBA16F` volumes; Halton jitter with one offset per view ray; 5 % blend of the
  current frame; out-of-frustum history skipped; the energy-conserving integration; local fog
  volumes; PS4 900p timings): course page <https://advances.realtimerendering.com/s2015/index.html>
  (the PPTX is 158 MB and was not read); slide transcript read from
  <https://www.slideshare.net/DICEStudio/physically-based-and-unified-volumetric-rendering-in-frostbite>
  (a re-host of the talk; the figures above come from that transcript).
- [S7] Guerrilla Games clouds: A. Schneider, "The Real-time Volumetric Cloudscapes of Horizon:
  Zero Dawn", SIGGRAPH 2015 Advances (<https://advances.realtimerendering.com/s2015/index.html>);
  A. Schneider, "Nubis: Authoring Real-Time Volumetric Cloudscapes with the Decima Engine",
  SIGGRAPH 2017 Advances (abstract: the 2015 prototype "rendered in under 2 milliseconds on the
  PlayStation 4"): <https://advances.realtimerendering.com/s2017/index.html>;
  "Nubis, Evolved" (SIGGRAPH 2022 Advances):
  <https://www.guerrilla-games.com/read/nubis-evolved>; "Nubis³" (2023-08-05, leaving 2.5D for
  voxel clouds): <https://www.guerrilla-games.com/read/nubis-cubed>
- [S8] S. Lagarde, "Water drop" series: "2b – Dynamic rain and its effects" (2013-01-03:
  procedural ripple normal maps, rain occlusion depth map, puddles with a 0.4 transition band
  and F0 0.02, no wall streaks, PS3/Xbox 360 timings)
  <https://seblagarde.wordpress.com/2013/01/03/water-drop-2b-dynamic-rain-and-its-effects/>;
  "3b – Physically based wet surfaces" (2013-04-14: porosity darkening
  `lerp(1, 0.2, (1 − metalness) × porosity)`, gloss change, accumulated water and puddles)
  <https://seblagarde.wordpress.com/2013/04/14/water-drop-3b-physically-based-wet-surfaces/>
- [S9] Epic Games, "Exponential Height Fog" (Unreal Engine documentation: second fog data,
  directional inscattering, fog max opacity, sky atmosphere ambient contribution):
  <https://dev.epicgames.com/documentation/en-us/unreal-engine/exponential-height-fog-in-unreal-engine>
- [S10] Epic Games, "Volumetric Fog" (Unreal Engine documentation: heavy temporal reprojection
  with per-frame sub-voxel jitter; fast-changing lights leave trails; Fog Max Opacity not
  supported; about 1 ms on PS4 at High and about 3 ms on a GTX 970 at Epic):
  <https://dev.epicgames.com/documentation/en-us/unreal-engine/volumetric-fog-in-unreal-engine>
- [S11] Bevy 0.16 release notes (2025-04-24: procedural atmosphere using "optimizations described
  in Sébastien Hillaire's EGSR 2020 paper") <https://bevy.org/news/bevy-0-16/>; licence
  `MIT OR Apache-2.0` (workspace `Cargo.toml`) and the WESL LUT modules in
  `crates/bevy_pbr/src/atmosphere/`: <https://github.com/bevyengine/bevy>
- [S12] Vulkan specification, limits (`maxImageDimension3D` minimum 256):
  <https://docs.vulkan.org/spec/latest/chapters/limits.html>; required format support is
  ADR-0023 [S12].
- **Project inputs:** the art bible v0 draft on PR #347 (`docs/art/lighting-mood.md` moods A–D,
  the weather-variant decision and EXP-1 to EXP-4; `palette-and-value.md` VAL-2, VAL-3,
  CON-1 to CON-5; `material-vocabulary.md` wet basalt, dust and §4 weathering;
  `silhouettes-and-scale.md` RIM-2), read-only. The camera figures come from
  `crates/client/src/app/scene_build.rs` (`network_camera`) and `crates/render/src/scene.rs`
  (`Camera::orbit`) at `e9d5f91`.
