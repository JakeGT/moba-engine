# Lighting mood: Ember Crossing (map 1)

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. Agents drafted the options as
concrete bake inputs so each one can be baked, rendered and compared.

**Fixed constraints (roadmap §1, §4, §6.1):**
- Lighting is **fixed per map and baked**: GPU path-traced GI on every tier. Ray tracing adds
  only the dynamic parts: characters, ability and VFX light, and reflections.
- **Exposure is locked or limited in gameplay**; full eye adaptation only in cinematics.
- Every mood must keep the board inside [palette-and-value.md](palette-and-value.md)
  (VAL-2 to VAL-4, PAL-1 to PAL-5) **after** lighting and grading, on every tier.

## How a mood is specified

Today (Compat renderer) a mood is three things:
1. an **environment bake** (`moba-tools env-bake`, analytic sky → SH-L2 + specular cube):
   `zenith`, `horizon`, `ground`, `sun_dir`, `sun_color`, `sun_glow`;
2. the map's **`[atmosphere]`** fog and **`[post]`** exposure and grading
   (`content/maps/ember.toml`);
3. a grading LUT (`content/grading/ember.cube`).

The photoreal renderer adds a **`light-bake`** step (roadmap §6.1, phase N-later). Its inputs
are listed per mood below as physical targets, so the mood carries over unchanged:
sun elevation and azimuth, sun illuminance, sky turbidity, and the gameplay EV100.

`sun_dir` points **towards** the sun (x east, y up, z towards the bottom of the screen at the
default camera). Elevation = asin(y / |v|).

**Baking one option** (Compat preview; the output stays outside `content/`):

```bash
cargo run -q --release -p moba-tools --bin env-bake -- bake --preset ember --size 64 --samples 256 \
  --zenith 0.16,0.22,0.36 --horizon 0.95,0.62,0.38 --ground 0.18,0.11,0.07 \
  --sun-dir -0.885,0.532,0.465 --sun-color 2.6,1.55,0.85 --sun-glow 6 ~/Projects/MOBA/art-review/env/golden
```

## The options

| | **A. Golden Crossing** (recommended) | **B. Ember Dusk** (today) | **C. Hazy Noon** | **D. Dust Veil** |
|---|---|---|---|---|
| Idea | Late afternoon. A low, warm but not red sun; long readable shadows; a dusty blue sky | Today's look: the sun near setting, a red-orange world | High sun through heat haze; bleached and harsh | A diffuse overcast of suspended dust and ash |
| Sun elevation | **28°** | 25° | 60° | ~35° (soft) |
| Sun azimuth | West-south-west (behind the Forge base, if the Forge holds the west) | West-south-west | West, high | West, diffuse |
| `sun_dir` | `-0.885, 0.532, 0.465` | `-0.6, 0.35, 0.45` | `-0.35, 0.87, 0.35` | `-0.70, 0.50, 0.50` |
| `sun_color` (linear) | `2.6, 1.55, 0.85` | `2.4, 0.9, 0.3` | `2.2, 2.0, 1.75` | `0.9, 0.75, 0.55` |
| `zenith` | `0.16, 0.22, 0.36` | `0.20, 0.07, 0.05` | `0.30, 0.40, 0.55` | `0.55, 0.45, 0.35` |
| `horizon` | `0.95, 0.62, 0.38` | `0.95, 0.38, 0.12` | `0.85, 0.78, 0.66` | `0.75, 0.58, 0.40` |
| `ground` | `0.18, 0.11, 0.07` | `0.14, 0.04, 0.02` | `0.22, 0.17, 0.12` | `0.20, 0.15, 0.10` |
| `sun_glow` | 6 | 6 | 10 | 3 |
| Fog `fog_density` / `fog_max` | 0.003 / 0.55 | 0.003 / 0.72 | 0.002 / 0.40 | 0.008 / 0.60 (casual), capped at 0.004 / 0.45 in ranked |
| Fog colour | `0.62, 0.45, 0.32` | `0.55, 0.30, 0.16` | `0.80, 0.76, 0.70` | `0.62, 0.50, 0.38` |
| Gameplay EV (`[post] exposure`) | 0.0 | 0.0 | −0.3 | +0.4 |
| Map `[lighting]` `sun_direction` | as `sun_dir` | as `sun_dir` | as `sun_dir` | as `sun_dir` |
| Map `[lighting]` `sun_color` / `sun_intensity` | `1.0, 0.80, 0.62` / 1.5 | `1.0, 0.84, 0.70` / 1.4 (today) | `1.0, 0.97, 0.92` / 1.8 | `0.95, 0.88, 0.78` / 0.7 |
| Map `[lighting]` `ibl_intensity` | 0.35 | 0.3 (today) | 0.4 | 0.6 |
| Hero shadow length (1.8 m) | 3.4 m | 3.9 m | 1.0 m | soft, ~2 m |
| Tower shadow (15 m) | 28 m | 32 m | 9 m | soft |
| **Physical targets for `light-bake`** | | | | |
| Sun illuminance | ~55 klx | ~45 klx | ~95 klx | ~15 klx direct + strong sky |
| Sky turbidity (Hillaire/Preetham) | 4 | 5 | 6 | 10+ |
| Gameplay EV100 | 13.5 | 13 | 14.7 | 12.5 |
| **Readability risk** | Low–medium: long tower shadows need the VAL-3 floor | **High:** the red-orange light pushes the board's chroma towards the enemy hues (PAL-1) and tints rims | **Low:** best contrast, least drama, flattest form | **High:** low contrast and fog; needs the ranked caps |
| **Photoreal payoff** | High: raking light shows material, GI bounce off sand | High but monotone | Medium: harsh speculars | Medium: soft, moody |

**Recommendation: A, Golden Crossing,** as map 1's gameplay mood. Keep B as a reference and
cinematic grade, C as the readability baseline in tests, and D as an optional casual weather
variant (roadmap §6.5) with ranked caps.

> **Decision needed (maintainer): map 1's gameplay mood** — A (recommended), B, C or D.

> **Decision needed (maintainer): is there a time-of-day or weather variant?** Lighting is
> baked and fixed per map, so a variant is a second bake of the same map, not a dynamic
> cycle. **Recommendation:** none for the slice; consider D as a casual-only variant after A4.

## First measurements (Compat renderer, 2026-10-31)

Each mood was baked into a private copy of `content/` and captured in the real engine
(Compat renderer, Ultra, 1920×1080, gameplay camera, `MOBA_SERVER=local`). The script is
`tools/moods.sh` in the review app folder, and the captures are in the app's
*Ember Crossing moods* section and in [boards/ember-moods.jpg](boards/ember-moods.jpg).
Values are OKLab over the playable ground at the screen centre (HUD and tower excluded):

| Mood | Mean L | L 5th–95th pct | Spread | Median C | p99 C |
|---|---|---|---|---|---|
| A. Golden Crossing | 0.50 | 0.41–0.69 | 0.28 | 0.066 | 0.104 |
| B. Ember Dusk (today) | 0.50 | 0.40–0.68 | 0.27 | 0.068 | 0.119 |
| C. Hazy Noon | 0.56 | 0.43–0.76 | 0.34 | 0.057 | 0.084 |
| D. Dust Veil | 0.54 | 0.45–0.71 | 0.27 | 0.067 | 0.101 |

What this says:
- **Value (VAL-2):** every mood's mean sits in the proposed 0.30–0.58 band. Hazy Noon's
  spread (0.34) is over the proposed 0.30, from its bright sunlit patches.
- **Chroma (PAL-1, C ≤ 0.06 on the ground):** **today's Ember ground fails in every mood**
  (p99 0.08–0.12). The cause is the stylised content (an orange-tinted terrain, the rust lane
  layer and an orange grading LUT), not the sky. Mood A is no worse than today (B), and it
  improves the chroma tail. The photoreal terrain (CC0 basalt, ash and sand from
  [references.md](references.md)) and a neutral grade are what bring the board under the cap.
- These are Compat-renderer numbers: the photoreal renderer's baked GI changes the absolute
  values, but the ranking between moods should hold.

**Concept images agree:** AI aerials of the same four moods
([boards/ember-concepts.jpg](boards/ember-concepts.jpg)) all paint the lane a saturated rust
red. Measured on three of them, the lane's median chroma is **C 0.10–0.11 at hue 15–55°**:
inside the enemy-red zone and about twice the proposed lane grit (`#7E5A47`, C 0.056). The
ground p99 is 0.115–0.132. The lane is the part of the board most likely to break PAL-1, in
concept art and in today's content alike, so the photoreal lane material must be specified
desaturated from the start. **Round 3** prompted the same mood with "a muted grey-brown
road, not red". The ground's median chroma fell to **0.027** and its p99 to **0.070–0.098**.
That is close to the cap, and it shows a calm lane still reads as a lane.

> **Decision needed (maintainer): the ground chroma cap.** Keep PAL-1 at 0.06 and treat
> today's Ember content as needing the photoreal rework (recommended); or set the cap from
> these measurements (≈ 0.10 p99) and accept a warmer, more saturated board.

## Exposure lock

| Rule | Proposed |
|---|---|
| **EXP-1** Ranked and tournament | `auto_exposure = false`; fixed EV per mood |
| **EXP-2** Casual | Auto exposure allowed, clamped to **±0.5 EV** (today ±1.0) |
| **EXP-3** Cinematic and showcase | Free adaptation |
| **EXP-4** VFX may not drive the meter | Bright effects don't darken the board (meter on pre-VFX HDR, or clamp per frame) |

## Shadows and GI rules

- **SHD-1:** in playable space no fully unlit area: bounce from the sand and the VAL-3 floor
  (L ≥ 0.18) hold in every structure's shadow.
- **SHD-2:** a unit standing in a structure's shadow still passes CON-1 (that is what the
  rims are for, but the ground must not go black).
- **SHD-3:** the sun azimuth never puts long shadows straight up the screen along the main
  lane, because they read as lines pointing at the enemy base. Azimuth west-south-west keeps
  shadows diagonal.

## Grading intent

- Golden Crossing: lift shadows slightly warm, keep midtones neutral, **cap saturation in the
  mids** so PAL-1 holds; a gentle filmic shoulder, with AgX as today.
- No teal-and-orange split-toning: orange highlights collide with the enemy colours.

## Reference HDRIs

Candidate Poly Haven HDRIs (CC0) for each mood are listed and previewed in
[references.md](references.md) and the review app. They are **references** for the analytic
sky and the future `light-bake`. They don't ship with the map unless the maintainer approves
one.
