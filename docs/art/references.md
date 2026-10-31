# References: named CC0 candidates

**Version:** v0 draft, 2026-10-31. **Status:** for review. Agent-researched; the maintainer
picks. Part of the [art bible](README.md).

**What this is:** a list of **named candidate assets per need**: the HDRIs, textures, models
and museum objects that match Ember Crossing and the two factions. **It is not a download
list.** Nothing here is fetched into the repository. When an asset is approved, the content
pipeline (roadmap §9.4) fetches it, records its provenance and puts it through
`content-check`.

**Licence rule** ([roadmap §9.2](../plans/next-gen-graphics-roadmap.md#92-sources-cc0-and-cc-by-plus-the-listed-exceptions)):
**CC0 and CC-BY only.** No NC, ND or SA licences, and no engine-restricted terms. Every entry
below is **CC0**; there are no CC-BY entries in v0. Rejected sources stay rejected:
Megascans/Fab, Textures.com and Share Textures.

**How each ID was checked (2026-10-31):**
- **Poly Haven:** `api.polyhaven.com/info/<id>` and `/files/<id>`. The HDRI resolution
  column comes from `/files`. Everything on Poly Haven is CC0.
- **ambientCG:** the API v2 catalogue (`/api/v2/full_json?type=Material`, three paged
  requests in total). All ambientCG assets are CC0.
- **CGBookcase and TextureCan:** each asset's page, which must state CC0.
- **The Met:** `collectionapi.metmuseum.org/public/collection/v1/objects/<id>` must return
  `isPublicDomain: true` (Met Open Access, CC0).
- **Smithsonian:** the Open Access API record must show `metadata_usage.access = CC0` and the
  image's `usage.access = CC0`.

Anything that failed these checks isn't listed. Five candidates passed but were **dropped after
a visual check** of their thumbnails (see the end of this file).

**Thumbnails are cached outside the repository**, in
`~/Projects/MOBA/art-review/refs/` (`thumbs/` plus `refs.json`, one record per entry with
its source, licence, page URL and reason). They are previews only: ≤ 600 px, JPEG, about
4.4 MB for all 110. They stay out of git because the bible's image budget (≤ 400 KB per
image, ≤ 25 MB for `docs/art/`, see the [README](README.md#licences-for-what-lives-here)) is
for curated boards, not for every candidate. Most thumbnails are the sources' own preview
renders (material spheres, sky previews), useful for choosing but not as board images. They
are there for the review app to show beside each table. Requests used the User-Agent
`moba-art-bible/0.1`, throttled to about 2 per second.

**At a 46 m camera**, a texture is judged by its large-scale value and colour breakup, not
its pores. Every colour here is a look reference: approved assets still have to pass the
palette clamps (PAL-1, PAL-2, VAL-1) and PBR ranges in [checks.md](checks.md). Several of the
saturated reds (`red_sand`, `cracked_red_ground`, `Gravel025`) are likely to need their chroma
pulled down to meet PAL-1 (ground C ≤ 0.06); none has been measured yet.

Every table has these columns: ID (linked to its page) | source | licence | what it's for |
thumbnail cached (y/n). HDRI tables add the maximum resolution; the artefact table adds the
object's title, culture and date.

---

## 1. HDRIs for Ember Crossing's lighting moods

All from Poly Haven, all CC0, unclipped. "Max res." is the largest file `/files` offers.
They are **references** for the analytic sky and the future `light-bake`
([lighting-mood.md](lighting-mood.md#reference-hdris)). None ships unless the maintainer
approves it. The moods map to the options in [lighting-mood.md](lighting-mood.md#the-options).

Several picks come in series shot at one location (Rogland, Kiara 1–9, Quarry 01–04), so a
mood can be compared across times of day over the same terrain.

### 1.1 Golden hour, low sun (A. Golden Crossing; B. Ember Dusk)

| ID | Source | Licence | Max res. | What it's for | Thumb |
|---|---|---|---|---|---|
| [`rogland_sunset`](https://polyhaven.com/a/rogland_sunset) | Poly Haven | CC0 | 24K | Rocky arid Namaqualand desert at sunset, clear sky, low warm sun: the closest match to Golden Crossing (mood A, recommended). | y |
| [`the_lost_city`](https://polyhaven.com/a/the_lost_city) | Poly Haven | CC0 | 24K | Low golden-hour sun raking across rocky formations, high contrast, partly cloudy; strong long shadows for cliff terraces. | y |
| [`quarry_01`](https://polyhaven.com/a/quarry_01) | Poly Haven | CC0 | 16K | Sandy quarry at golden hour, warm low sun, long shadows; bare ground with no vegetation to colour the bounce light. | y |
| [`kiara_8_sunset`](https://polyhaven.com/a/kiara_8_sunset) | Poly Haven | CC0 | 16K | Clear, high-contrast golden sunset over rocky ridges; useful for a hotter, more saturated variant. | y |
| [`valley_of_desolation`](https://polyhaven.com/a/valley_of_desolation) | Poly Haven | CC0 | 24K | Rocky Karoo valley at golden hour, sun near the horizon; the name and terrain suit the weathered tone. | y |
| [`kiara_7_late-afternoon`](https://polyhaven.com/a/kiara_7_late-afternoon) | Poly Haven | CC0 | 16K | Late-afternoon sun just clear of the hills over dry, rocky ground, clear sky: a late-afternoon grade rather than a sunset one. | y |

### 1.2 Harsh, hazy midday (C. Hazy Noon)

| ID | Source | Licence | Max res. | What it's for | Thumb |
|---|---|---|---|---|---|
| [`goegap`](https://polyhaven.com/a/goegap) | Poly Haven | CC0 | 16K | Clear midday desert sun, hard shadows, warm scrubland: the harsh-midday baseline. | y |
| [`kiara_5_noon`](https://polyhaven.com/a/kiara_5_noon) | Poly Haven | CC0 | 16K | High-noon sun over an arid rocky valley; deep blue sky, strongest contrast of the set. | y |
| [`quarry_02`](https://polyhaven.com/a/quarry_02) | Poly Haven | CC0 | 16K | Bright midday sun over a sandy quarry, partly cloudy; bare-earth surroundings. | y |
| [`dry_cracked_lake`](https://polyhaven.com/a/dry_cracked_lake) | Poly Haven | CC0 | 24K | Bright sun over a cracked dry lakebed; matches the cracked-earth ground and gives a pale, hazy horizon. | y |
| [`veld_fire`](https://polyhaven.com/a/veld_fire) | Poly Haven | CC0 | 16K | Midday sun over scorched veld; burnt ground and warm highlights fit the ash/scorch palette. | y |

### 1.3 Dusk (B. Ember Dusk, and the blue hour after it)

| ID | Source | Licence | Max res. | What it's for | Thumb |
|---|---|---|---|---|---|
| [`kiara_9_dusk`](https://polyhaven.com/a/kiara_9_dusk) | Poly Haven | CC0 | 16K | Clear dusk over an arid rocky valley: cool blue sky with a warm horizon glow, good for ember VFX contrast. | y |
| [`aarfontein_dusk`](https://polyhaven.com/a/aarfontein_dusk) | Poly Haven | CC0 | 24K | Rocky scrub plains at dusk under broad cloud; soft, cool ambient light. | y |
| [`qwantani_dusk_1_puresky`](https://polyhaven.com/a/qwantani_dusk_1_puresky) | Poly Haven | CC0 | 24K | Pure-sky dusk (no ground) so it can sit behind our own terrain without a mismatched horizon. | y |
| [`quarry_04`](https://polyhaven.com/a/quarry_04) | Poly Haven | CC0 | 16K | Soft, low-contrast sunset over a cracked sandy quarry; a gentler end-of-day option. | y |

### 1.4 Overcast and dusty (D. Dust Veil)

| ID | Source | Licence | Max res. | What it's for | Thumb |
|---|---|---|---|---|---|
| [`rogland_overcast`](https://polyhaven.com/a/rogland_overcast) | Poly Haven | CC0 | 24K | Overcast light over rocky arid desert: same region as rogland_sunset, so the two can be compared directly. | y |
| [`waterbuck_trail`](https://polyhaven.com/a/waterbuck_trail) | Poly Haven | CC0 | 16K | Overcast midday over an arid rocky valley; soft, low-contrast base for a dust-storm grade. | y |
| [`kloofendal_overcast_puresky`](https://polyhaven.com/a/kloofendal_overcast_puresky) | Poly Haven | CC0 | 24K | Pure overcast sky (no ground); a neutral dome to tint brown/orange for a dust-storm variant. | y |
| [`kloofendal_28d_misty_puresky`](https://polyhaven.com/a/kloofendal_28d_misty_puresky) | Poly Haven | CC0 | 24K | Misty pure sky with a diffused sun; closest thing to haze in the library, a starting point for suspended dust. | y |

**Gap:** Poly Haven has **no dust-storm or sand-haze HDRI**. Its "haze" skies are humid mist
over grassland. D (Dust Veil) is best built from an overcast pure sky plus the engine's own
fog colour and density (see the D column in lighting-mood.md), not found as a capture.

---

## 2. Ground and cliff textures

Basalt cliffs, scorched ash and brown ground, the rust-red trodden lane, cracked earth,
sandstone and the ember crystals.

| ID | Source | Licence | What it's for | Thumb |
|---|---|---|---|---|
| [`burned_ground_01`](https://polyhaven.com/a/burned_ground_01) | Poly Haven | CC0 | Scorched gravelly ground with charred soil and ash: the scorched/ash plain. | y |
| [`cracked_red_ground`](https://polyhaven.com/a/cracked_red_ground) | Poly Haven | CC0 | Cracked red clay ground; ties the rust-red lane colour to cracked earth. | y |
| [`red_sand`](https://polyhaven.com/a/red_sand) | Poly Haven | CC0 | Dusty red sand with a trampled surface and shallow footprints: candidate for the trodden lane. | y |
| [`red_laterite_soil_stones`](https://polyhaven.com/a/red_laterite_soil_stones) | Poly Haven | CC0 | Red laterite soil with compacted gravel: alternative lane surface with more breakup. | y |
| [`sandy_gravel_02`](https://polyhaven.com/a/sandy_gravel_02) | Poly Haven | CC0 | Dusty reddish-brown sandy gravel: transition between lane and ash ground. | y |
| [`mud_cracked_dry_03`](https://polyhaven.com/a/mud_cracked_dry_03) | Poly Haven | CC0 | Sun-baked cracked mud with deep fissures: cracked-earth basin floor. | y |
| [`moon_dusted_01`](https://polyhaven.com/a/moon_dusted_01) | Poly Haven | CC0 | Fine grey regolith with low reflectance: stand-in for volcanic ash fields. | y |
| [`dark_rock`](https://polyhaven.com/a/dark_rock) | Poly Haven | CC0 | Dark charcoal stratified rock with fractures: the nearest Poly Haven match to basalt. | y |
| [`dark_rock_02`](https://polyhaven.com/a/dark_rock_02) | Poly Haven | CC0 | Dark stratified rock with sharp fractures: second basalt-like cliff candidate. | y |
| [`rock_face`](https://polyhaven.com/a/rock_face) | Poly Haven | CC0 | Reddish-brown cliff stone: warm cliff tiers between the basalt bands. | y |
| [`cliff_side`](https://polyhaven.com/a/cliff_side) | Poly Haven | CC0 | Eroded orange layered sedimentary cliff: reference for terrace strata. | y |
| [`rocky_trail_02`](https://polyhaven.com/a/rocky_trail_02) | Poly Haven | CC0 | Dusty rocky trail with gravel and compacted dirt: jungle-path equivalent between lanes. | y |
| [`sandstone_cracks`](https://polyhaven.com/a/sandstone_cracks) | Poly Haven | CC0 | Cracked sandstone slab: natural sandstone outcrop. | y |
| [`Rock035`](https://ambientcg.com/a/Rock035) | ambientCG | CC0 | Black cliff rock (tagged black, cave, cliff): basalt cliff candidate. | y |
| [`Rock031`](https://ambientcg.com/a/Rock031) | ambientCG | CC0 | Dark grey-black cliff rock: basalt cliff candidate. | y |
| [`Rock033`](https://ambientcg.com/a/Rock033) | ambientCG | CC0 | Black, dirty cliff rock: weathered basalt candidate. | y |
| [`Rock029`](https://ambientcg.com/a/Rock029) | ambientCG | CC0 | Orange-red desert cliff rock: warm cliff tier. | y |
| [`Ground031`](https://ambientcg.com/a/Ground031) | ambientCG | CC0 | Grey cracked dry ground: ash-coloured cracked earth. | y |
| [`Gravel025`](https://ambientcg.com/a/Gravel025) | ambientCG | CC0 | Orange-red gravel with pebbles: lane-edge breakup. | y |
| [`Gravel024`](https://ambientcg.com/a/Gravel024) | ambientCG | CC0 | Black/dark grey gravel: basalt scree and ash gravel at cliff feet. | y |
| [`Rocks015`](https://ambientcg.com/a/Rocks015) | ambientCG | CC0 | Fine grey pebble grit: ash grit and scree between lanes. | y |
| [`Lava004`](https://ambientcg.com/a/Lava004) | ambientCG | CC0 | Glowing lava/magma: emissive reference for ember seams and crystal glow. | y |
| [`239 (ground_0027)`](https://www.texturecan.com/details/239/) | TextureCan | CC0 | Black lava crust with glowing cracks: basalt flow ground and ember seams. | y |
| [`228 (ground_0025)`](https://www.texturecan.com/details/228/) | TextureCan | CC0 | Red crystal ground: direct reference for ember crystal clusters. | y |
| [`253 (rock_0007)`](https://www.texturecan.com/details/253/) | TextureCan | CC0 | Red sandstones/mudstones: warm layered cliff tier. | y |

**Gaps:**
- **No true basalt** (columnar or vesicular) on Poly Haven or ambientCG. The dark stratified
  rocks (`dark_rock`, `dark_rock_02`, `Rock031`, `Rock033`, `Rock035`) and TextureCan's
  lava crust (`239`) are the nearest matches. Columnar basalt cliffs are a candidate for
  the AI and procedural route (roadmap §9.3).
- **Ember crystals:** TextureCan's red crystal ground (`228`) and the lava references give the
  colour and emission idea only. The crystals themselves are a modelled and shaded asset.

---

## 3. Faction materials

Material looks to anchor the vocabulary in [material-vocabulary.md](material-vocabulary.md).
The physical ranges come from physicallybased.info there, not from these scans.

### 3.1 The Solar Forge

Brass, bronze, copper, forged iron, gold leaf, kiln ceramics, sandstone and basalt masonry,
amber sun-glass, slag.

| ID | Source | Licence | What it's for | Thumb |
|---|---|---|---|---|
| [`rusty_metal_03`](https://polyhaven.com/a/rusty_metal_03) | Poly Haven | CC0 | Heavily rusted flat steel: aged forged iron and slag-stained plates. | y |
| [`rust_coarse_01`](https://polyhaven.com/a/rust_coarse_01) | Poly Haven | CC0 | Coarse rust: breakup layer for forged iron. | y |
| [`terracotta_floor_tiles`](https://polyhaven.com/a/terracotta_floor_tiles) | Poly Haven | CC0 | Warm red-brown terracotta tiles with soft gloss: kiln-ceramic floors. | y |
| [`patterned_terracotta_tiling`](https://polyhaven.com/a/patterned_terracotta_tiling) | Poly Haven | CC0 | Octagon-and-dot terracotta with black inserts: patterned kiln ceramics. | y |
| [`red_sandstone_wall`](https://polyhaven.com/a/red_sandstone_wall) | Poly Haven | CC0 | Red sandstone wall: Forge masonry. | y |
| [`sandstone_blocks_08`](https://polyhaven.com/a/sandstone_blocks_08) | Poly Haven | CC0 | Large, uneven sandstone blocks: monumental Forge masonry. | y |
| [`volcanic_rock_tiles`](https://polyhaven.com/a/volcanic_rock_tiles) | Poly Haven | CC0 | Dark pitted volcanic stone mosaic: basalt masonry for foundations and plinths. | y |
| [`Metal015`](https://ambientcg.com/a/Metal015) | ambientCG | CC0 | Pale, eroded bronze with a grey-green patina: long-weathered bronze statuary. | y |
| [`Metal018`](https://ambientcg.com/a/Metal018) | ambientCG | CC0 | Dark, mottled old bronze with rust and scratches: battle-worn bronze. | y |
| [`Metal035`](https://ambientcg.com/a/Metal035) | ambientCG | CC0 | Smooth orange copper: clean copper inlay. | y |
| [`Metal058B`](https://ambientcg.com/a/Metal058B) | ambientCG | CC0 | Oxidised copper: verdigris on exposed copper. | y |
| [`Metal048C`](https://ambientcg.com/a/Metal048C) | ambientCG | CC0 | Rough, impure gold: gilded trim that has seen weather. | y |
| [`Foil002`](https://ambientcg.com/a/Foil002) | ambientCG | CC0 | Gold foil: gold-leaf reference (wrinkled leaf over carving). | y |
| [`Metal052C`](https://ambientcg.com/a/Metal052C) | ambientCG | CC0 | Rough impure iron, bright steel tone: hammered, forged iron before soot and heat tint. | y |
| [`Metal055B`](https://ambientcg.com/a/Metal055B) | ambientCG | CC0 | Rusty iron: exposed forged iron. | y |
| [`GlazedTerracotta001`](https://ambientcg.com/a/GlazedTerracotta001) | ambientCG | CC0 | Glazed terracotta: glazed kiln ceramics. | y |
| [`Tiles144`](https://ambientcg.com/a/Tiles144) | ambientCG | CC0 | Aged golden sandstone tiles: Forge plaza paving. | y |
| [`Onyx007`](https://ambientcg.com/a/Onyx007) | ambientCG | CC0 | Honey-orange onyx: translucent stand-in for amber sun-glass. | y |
| [`545 (metal_0073)`](https://www.texturecan.com/details/545/) | TextureCan | CC0 | Oxidised brass wall: aged brass panels. | y |
| [`bronze-01`](https://www.cgbookcase.com/textures/bronze-01) | CGBookcase | CC0 | Bronze: clean bronze reference to set the base colour range. | y |
| [`dirty-gold-01`](https://www.cgbookcase.com/textures/dirty-gold-01) | CGBookcase | CC0 | Dirty gold: worn gilding. | y |
| [`old-iron-01`](https://www.cgbookcase.com/textures/old-iron-01) | CGBookcase | CC0 | Old iron: forged iron. | y |

### 3.2 The Void

Dark ceramics, obsidian, polished black stone, tarnished silver, cool void-glass.

| ID | Source | Licence | What it's for | Thumb |
|---|---|---|---|---|
| [`slate_floor`](https://polyhaven.com/a/slate_floor) | Poly Haven | CC0 | Polished dark slate with a glossy coat: polished black stone floors. | y |
| [`granite_tile`](https://polyhaven.com/a/granite_tile) | Poly Haven | CC0 | Dark grey granite tiles with low sheen: Void paving. | y |
| [`patterned_slate_tiles`](https://polyhaven.com/a/patterned_slate_tiles) | Poly Haven | CC0 | Dark, rough patterned slate: weathered Void masonry. | y |
| [`slab_tiles`](https://polyhaven.com/a/slab_tiles) | Poly Haven | CC0 | Dark slate slabs with rust-orange veining: Void paving where it borders Forge ground. | y |
| [`Marble016`](https://ambientcg.com/a/Marble016) | ambientCG | CC0 | Black, reflective, shiny marble: polished black stone. | y |
| [`Marble017`](https://ambientcg.com/a/Marble017) | ambientCG | CC0 | Black reflective marble with a rough finish: weathered polished stone. | y |
| [`Onyx013`](https://ambientcg.com/a/Onyx013) | ambientCG | CC0 | Banded black-and-grey onyx: dark polished stone with movement. | y |
| [`Onyx014`](https://ambientcg.com/a/Onyx014) | ambientCG | CC0 | Banded dark onyx, lighter than Onyx013: secondary polished stone. | y |
| [`Tiles072`](https://ambientcg.com/a/Tiles072) | ambientCG | CC0 | Black hexagonal mosaic: dark ceramic tiling. | y |
| [`Porcelain003`](https://ambientcg.com/a/Porcelain003) | ambientCG | CC0 | Black porcelain: dark glazed ceramics. | y |
| [`Metal049C`](https://ambientcg.com/a/Metal049C) | ambientCG | CC0 | Dented, rough silver: tarnished silver base. | y |
| [`Metal049B`](https://ambientcg.com/a/Metal049B) | ambientCG | CC0 | Smeared, dirty silver: tarnished silver. | y |
| [`Metal046B`](https://ambientcg.com/a/Metal046B) | ambientCG | CC0 | Dirty black metal: dark metal trim. | y |
| [`630 (metal_0080)`](https://www.texturecan.com/details/630/) | TextureCan | CC0 | Black wall with golden metallic veins: Void stone where Forge gold has been absorbed (contested sites). | y |
| [`93 (marble_0007)`](https://www.texturecan.com/details/93/) | TextureCan | CC0 | Black marble with white cracks: polished Void stone. | y |
| [`550 (rooftop_0005)`](https://www.texturecan.com/details/550/) | TextureCan | CC0 | Black clay roof tiles: dark ceramics. | y |
| [`coal-matte-tiles-01`](https://www.cgbookcase.com/textures/coal-matte-tiles-01) | CGBookcase | CC0 | Coal-black matte tiles: dark ceramic tiling. | y |

**Gaps:**
- **Glass** (amber sun-glass and cool void-glass): no CC0 scanned glass in any of the four
  texture sources. Glass is a shader (IOR, transmission, tint) from
  [material-vocabulary.md](material-vocabulary.md). `Onyx007` and the brass lantern are look
  references only.
- **Obsidian:** no texture is truly obsidian. The black onyx and marble entries plus the two
  obsidian mirrors in §5 are the references.
- **Brass:** Poly Haven has no brass texture. The brass references are TextureCan `545`,
  the Poly Haven brass models in §4 and the astrolabes in §5.
- **Sandstone carving:** no carved CC0 scan was found. Carving language comes from the
  artefacts in §5 (the winged sun disc panel, the sun disk).
- **Slag** has no direct scan. `rusty_metal_03`, `Lava004` and TextureCan `239` together
  cover its look.

---

## 4. Props and models (Poly Haven, CC0)

Scale and kitbash references, not shipping meshes: every model still goes through the
remesh and bake pipeline and the triangle budgets (roadmap §9.1). Poly Haven supplies glTF
and Blend files; texture sets go up to the resolution shown on each page (8K for most, 16K
for `moon_rock_03`).

| ID | Source | Licence | What it's for | Thumb |
|---|---|---|---|---|
| [`namaqualand_cliff_01`](https://polyhaven.com/a/namaqualand_cliff_01) | Poly Haven | CC0 | Weathered Karoo cliff with stacked boulders: scale and kitbash reference for cliff terraces. | y |
| [`namaqualand_cliff_02`](https://polyhaven.com/a/namaqualand_cliff_02) | Poly Haven | CC0 | Stacked boulders and cracked slabs: second cliff piece. | y |
| [`mountainside`](https://polyhaven.com/a/mountainside) | Poly Haven | CC0 | Layered, jagged cliff face (greenish in the scan, so it needs a colour pass): large backdrop rock. | y |
| [`namaqualand_boulders_01`](https://polyhaven.com/a/namaqualand_boulders_01) | Poly Haven | CC0 | Desert boulder cluster: lane-side cover. | y |
| [`namaqualand_boulder_04`](https://polyhaven.com/a/namaqualand_boulder_04) | Poly Haven | CC0 | Single rugged desert boulder. | y |
| [`rock_09`](https://polyhaven.com/a/rock_09) | Poly Haven | CC0 | Rust-brown weathered boulder: fits the warm palette. | y |
| [`moon_rock_03`](https://polyhaven.com/a/moon_rock_03) | Poly Haven | CC0 | Dusty regolith rock: reference for the Void's levitating rocks. | y |
| [`dead_quiver_trunk`](https://polyhaven.com/a/dead_quiver_trunk) | Poly Haven | CC0 | Dried, decayed desert tree trunk. | y |
| [`dead_tree_trunk`](https://polyhaven.com/a/dead_tree_trunk) | Poly Haven | CC0 | Weathered dead log with cracked bark. | y |
| [`quiver_tree_02`](https://polyhaven.com/a/quiver_tree_02) | Poly Haven | CC0 | Desert quiver tree: a distinctive arid silhouette for the basin edge. | y |
| [`modular_fort_01`](https://polyhaven.com/a/modular_fort_01) | Poly Haven | CC0 | Modular weathered stone fort: ruin and wall kitbash, scale reference for structures. | y |
| [`gothic_statue`](https://polyhaven.com/a/gothic_statue) | Poly Haven | CC0 | Weathered robed statue: scale reference for monuments and ruins. | y |
| [`bull_head`](https://polyhaven.com/a/bull_head) | Poly Haven | CC0 | Dark bronze bull head with golden horns: Forge bronze-and-gold material reference. | y |
| [`brass_diya_lantern`](https://polyhaven.com/a/brass_diya_lantern) | Poly Haven | CC0 | Aged brass lantern with glass panels: Forge brass and sun-glass prop. | y |
| [`seadogs_compass`](https://polyhaven.com/a/seadogs_compass) | Poly Haven | CC0 | Aged brass compass: instrument reference for the Void's celestial devices. | y |

---

## 5. Artefact references (museum Open Access, CC0)

Shape language, ornament and material references for the factions. They are photos of real
objects: use them for **motifs and wear**, not for tracing, and don't present a real object
as a faction artefact in marketing. The Smithsonian object's photo is CC0 by the API record;
check each image's usage flag again before it goes into a board in the repository.

**Forge:** solar discs, bronze armour, Egyptian, Mesopotamian and Etruscan solar motifs.
**Void:** astrolabes and a compendium (celestial instruments), obsidian mirrors, black-figure
ceramics.

| ID | Source | Licence | Object | What it's for | Thumb |
|---|---|---|---|---|---|
| [`309286`](https://www.metmuseum.org/art/collection/search/309286) | The Met | CC0 | Sun Disk (Colonial, after 16th century) | Hammered, gilt silver sun disk: sun-disc emblem for the Forge. | y |
| [`324925`](https://www.metmuseum.org/art/collection/search/324925) | The Met | CC0 | Panel with a youth grasping a tree; winged sun disc above (Assyrian, ca. 8th century BCE) | Neo-Assyrian ivory panel with a winged sun disc: Mesopotamian solar motif. | y |
| [`249096`](https://www.metmuseum.org/art/collection/search/249096) | The Met | CC0 | Bronze statuette of a solar deity (Etruscan, 3rd–2nd century BCE) | Etruscan bronze statuette of a solar deity: bronze figure work. | y |
| [`248133`](https://www.metmuseum.org/art/collection/search/248133) | The Met | CC0 | Bronze cuirass (breastplate) (Hallstatt (Western European), 12th-10th century BCE) | Late Bronze Age (Hallstatt) bronze cuirass: hammered bronze armour. | y |
| [`243765`](https://www.metmuseum.org/art/collection/search/243765) | The Met | CC0 | Faience amulet of Ra Horakhty (Egyptian, 664–334 BCE) | Faience amulet of Ra-Horakhty (sun disc on falcon head): Egyptian solar iconography. | y |
| [`256133`](https://www.metmuseum.org/art/collection/search/256133) | The Met | CC0 | Bronze helmet of Corinthian type (Greek, late 6th century BCE) | Greek bronze helmet of Corinthian type: bronze armour silhouette. | y |
| [`923639`](https://www.metmuseum.org/art/collection/search/923639) | The Met | CC0 | Astrolabe (German or Italian, 1462) | Copper-alloy astrolabe (1462): celestial instrument for the Void. | y |
| [`451699`](https://www.metmuseum.org/art/collection/search/451699) | The Met | CC0 | Planispheric Astrolabe (dated 1065 AH/1654–55 CE) | Planispheric astrolabe in brass and steel: pierced rete patterns. | y |
| [`314189`](https://www.metmuseum.org/art/collection/search/314189) | The Met | CC0 | Obsidian Mirror (Ecuador, before 15th century) | Obsidian mirror from Ecuador: polished obsidian surface. | y |
| [`317411`](https://www.metmuseum.org/art/collection/search/317411) | The Met | CC0 | Mirror (Mexican, before 16th century) | Mexican obsidian mirror: polished black stone. | y |
| [`254345`](https://www.metmuseum.org/art/collection/search/254345) | The Met | CC0 | Terracotta amphora (jar) (Greek, Attic, ca. 510 BCE) | Attic black-figure amphora: black-on-terracotta ceramic language. | y |
| [`nmah_997126`](https://collections.si.edu/search/detail/edanmdm:nmah_997126) | Smithsonian | CC0 | Vopel Compendium (ca 1540) | Vopel astronomical compendium (c. 1540): brass celestial instrument with engraved dials. | y |

**Not found as CC0 with an image:** an armillary sphere (the Met's are prints; the
NMAH armillary-sphere records with images aren't flagged CC0). Smithsonian searching was
cut short by the shared `DEMO_KEY` rate limit; a registered api.data.gov key would allow a
fuller pass (Freer bronze mirrors, NMAH astrolabes).

---

## 6. CC-BY attribution handling

v0 lists **only CC0** assets, so nothing here needs attribution, though crediting the
sources is still good practice. When a CC-BY asset is approved later (for example a
Sketchfab `by` model, roadmap §9.2):

1. **Record it at fetch time.** The content pipeline stores, next to the asset, a provenance
   record: source URL, author, licence and its version (for example CC-BY 4.0), the date, a
   snapshot of the licence page, and any changes we made.
2. **The licence gate will generate the notices.** `content-check`'s licence and provenance gate
   (A0, [checks.md](checks.md)) will reject any asset whose licence is not CC0 or CC-BY, or
   whose record is missing, and **will generate the CC-BY section of `THIRD_PARTY.md`** and
   the in-game credits from those records. Nobody edits that section by hand.
3. **What a notice holds:** title, author, source link, licence name and link, and "modified"
   if it was (remeshed, rebaked or recoloured all count).
4. **CC0 entries** are listed in a short "CC0 sources" credit line (Poly Haven, ambientCG,
   CGBookcase, TextureCan, the Met, the Smithsonian), not per asset.

---

## 7. Dropped after the visual check

These passed the ID and licence checks but their previews don't fit the brief:

| ID | Source | Why dropped |
|---|---|---|
| `Ground093C` | ambientCG | Too pale (cream) for the ash and rust palette |
| `Rock009` | ambientCG | Tagged "red" but olive-green in the preview |
| `aerial_rocks_02` | Poly Haven | Visible moss and lichen; wrong climate |
| `red-rock-01` | CGBookcase | Pale pink-white, not the red cliff wanted |
| `Rock041` | ambientCG | Grey-olive, not dark enough to stand in for obsidian |

## 8. Counts

| Section | Entries | Sources |
|---|---|---|
| 1.1 HDRI, golden hour | 6 | Poly Haven 6 |
| 1.2 HDRI, midday | 5 | Poly Haven 5 |
| 1.3 HDRI, dusk | 4 | Poly Haven 4 |
| 1.4 HDRI, overcast/dusty | 4 | Poly Haven 4 |
| 2. Ground and cliff | 25 | Poly Haven 13, ambientCG 9, TextureCan 3 |
| 3.1 Forge materials | 22 | Poly Haven 7, ambientCG 11, TextureCan 1, CGBookcase 3 |
| 3.2 Void materials | 17 | Poly Haven 4, ambientCG 9, TextureCan 3, CGBookcase 1 |
| 4. Props and models | 15 | Poly Haven 15 |
| 5. Artefacts | 12 | The Met 11, Smithsonian 1 |
| **Total** | **110** | |
