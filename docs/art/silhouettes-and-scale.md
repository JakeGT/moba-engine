# Silhouettes and scale

**Status:** v0 draft, 2026-10-31. **Lead:** maintainer. Agents drafted the size classes and
numeric rules; the review renders (roadmap §9.4) will check them.

## The camera sets the rules

From roadmap §2: default camera 46 m away, 60° FOV, about 54° pitch. A 1.8 m hero is about
**40 px tall at 1440p**. That gives these **pixels per metre of height** at the focus:

| Output | px per metre of height | 1.8 m hero |
|---|---|---|
| 1080p | ~16.5 | ~30 px |
| 1440p | ~22 | ~40 px |
| 4K | ~33.5 | ~60 px |

**1080p is the design floor.** If a silhouette rule holds at 30 px, it holds everywhere.

At 54° pitch the camera sees **heads, shoulders, backs and the top of weapons** more than
legs. So identity lives in the **top third** of a unit and in its outline against the ground,
not in faces or belts (those are for the 8 m zoom and showcase cameras).

## Size classes

| Class | Height (m) | 1080p px | 1440p px | Footprint (m, diameter) | Notes |
|---|---|---|---|---|---|
| Minion, ranged | 1.2–1.4 | 20–23 | 26–31 | 0.9 | |
| Minion, melee | 1.3–1.5 | 21–25 | 29–33 | 1.0 | |
| Minion, siege | 2.2–2.6 | 36–43 | 48–57 | 2.2 | Wider than tall |
| **Hero S** (assassin, marksman) | 1.6–1.8 | 26–30 | 35–40 | 1.0 | |
| **Hero M** (bruiser, caster) | 1.8–2.0 | 30–33 | 40–44 | 1.1 | |
| **Hero L** (tank, guardian) | 2.1–2.6 | 35–43 | 46–57 | 1.4 | Bulk, not just height |
| Large monster / objective | 4–7 | 66–115 | 88–154 | 4–6 | |
| Tower | 12–16 | — | — | 5–6 | The crown must stay readable from 60 m |
| Core | 18–24 | — | — | 10–14 | |
| Tall prop | ≤ 11 | — | — | — | Off the lane corridor (content-check warns today) |

[boards/scale.jpg](boards/scale.jpg) draws every class at its true pixel size (view it at 100 %).

> **Decision needed (maintainer): one hero scale or role-scaled heroes?** (a) **Role-scaled
> S/M/L (recommended):** the size itself tells the role; (b) one height for all heroes, with
> role carried by shape only. (b) is fairer for hit-box perception; (a) reads faster.

## Silhouette rules

Tested by the review renders on a **flat single-value silhouette** at 1080p and 1440p, from
the gameplay camera, at eight yaw angles.

| Rule | What | Proposed threshold |
|---|---|---|
| **SIL-1** | Height is within the unit's class range | as the table |
| **SIL-2** | Minions are clearly smaller than heroes | Minion height ≤ **0.8×** the smallest hero; no capes, halos or orbiting parts on minions |
| **SIL-3** | Heroes differ from each other | Silhouette IoU < **0.80** between any two heroes (best-aligned, 8 yaws), at 1080p |
| **SIL-4** | Role reads in the top third | The top third of each hero's silhouette contains its role tell: a weapon head, shoulder mass, a staff tip or a bow arc |
| **SIL-5** | The Forge tell | One radial element (crest, back-halo or shield boss) ≥ **8 %** of the silhouette area at 1440p |
| **SIL-6** | The Void tell | ≥ **2** negative-space gaps or floating parts, each ≥ **2 px** wide at 1440p |
| **SIL-7** | Weapon and cast reads | Melee weapon extends the bounding box by ≥ **15 %**; ranged weapons are visible from ≥ 6 of 8 yaws |
| **SIL-8** | Clean outline | No hair-thin parts below 1 px at 1080p in the outline (they shimmer under TAA and DLSS); thin parts get a minimum thickness or go to the zoom LOD |
| **SIL-9** | Animation keeps the silhouette | The idle, run and cast key poses each pass SIL-3 against the other heroes' same poses |

## Team rims (the team layer on units)

**Decided:** team identity on units is rim plus outline, never the unit's own colours.

| Rule | What | Proposed |
|---|---|---|
| **RIM-1** | Rim in the lighting pass (roadmap §4.2), tinted by the player-relative team colour | Fresnel-based, width ≈ **1.5–2.5 px** at output resolution |
| **RIM-2** | The rim survives darkness, fog and GI | Added after fog; doesn't scale with scene exposure below a floor |
| **RIM-3** | Enemy rims are stronger than ally rims | Enemy ≥ **1.25×** ally intensity; self uses the self colour |
| **RIM-4** | No material, emissive or VFX makes a rim-like bright edge in a team hue | PAL-4 and PAL-5, checked on review renders |
| **RIM-5** | The outline (overlay layer, after upscaling) follows the same colours | as `crates/client/src/app/outlines.rs` today |
| **RIM-6** | It passes CON-2 in [palette-and-value.md](palette-and-value.md) | |

> **Decision needed (maintainer + designer): rim vs outline on allies.** Today outlines carry
> the team colour. Options: (a) **rim on everyone, outline only on enemies and the hovered
> unit (recommended):** less clutter in teamfights; (b) both on everyone; (c) outline only.

## Scale reference

The review app's **actual-size** toggle shows every concept at 30 px and 40 px, next to a
1.8 m reference figure and a 12 m lane width, so scale is judged at the size players see.
