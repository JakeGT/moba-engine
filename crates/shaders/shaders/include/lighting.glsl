// Stylised PBR shared by every lit scene shader (ADR-0011).
//
// Physically based core: GGX distribution, height-correlated Smith-GGX visibility, Schlick
// Fresnel, an energy-conserving Lambert diffuse ((1 - F)(1 - metallic)), image-based ambient
// (SH-L2 irradiance + a prefiltered specular cube through the split-sum BRDF LUT) and one
// directional shadow map. Light intensities are pre-multiplied by pi, so an unshadowed white
// Lambert surface facing the sun shows `albedo * sun`.
//
// Stylisation layer (map and material parameters): wrap lighting, an optional ramp texture
// that remaps N.L, a painted-specular tint and scale, and a coloured rim term that keeps
// silhouettes readable against the terrain.
//
// R2b (ADR-0016) adds clustered dynamic point lights after the sun, an ambient-occlusion term
// on the ambient line (R6's min(relief, GTAO) x baked map AO) and screen-space contact
// shadows multiplying the shadow-map term.
//
// Set 3 is the scene lighting set, one per frame in flight (crates/render/src/lighting.rs):
// its descriptors are written at load/resize only; the light buffers' contents change per frame.
#ifndef MOBA_LIGHTING_GLSL
#define MOBA_LIGHTING_GLSL

#include "camera.glsl"

// The shaded pixel's centre in render pixels: `gl_FragCoord.xy` in the fragment stages. The
// deferred lighting pass (`deferred_light.comp`, ADR-0023 §2) is a compute shader and defines
// it as its own pixel before including this file, so both paths read the AO and the contact
// shadows at the same texel. The expansion is the original expression, so every fragment
// shader's SPIR-V is unchanged.
#ifndef MOBA_PIXEL_CENTER
#define MOBA_PIXEL_CENTER gl_FragCoord.xy
#endif

// Shadow map with hardware depth comparison (LESS_OR_EQUAL: 1 = lit) and bilinear PCF.
layout(set = 3, binding = 0) uniform sampler2DShadow shadow_map;
// The same depth image without comparison, for the PCSS blocker search.
layout(set = 3, binding = 1) uniform sampler2D shadow_depth;
// Prefiltered specular environment: mip m holds GGX roughness m / max_lod.
layout(set = 3, binding = 2) uniform samplerCube environment_specular;
// Split-sum BRDF LUT: (N.V, roughness) -> (scale, bias) of F0.
layout(set = 3, binding = 3) uniform sampler2D brdf_lut;
// Stylisation ramp: N.L remapped from [-1, 1] to u, rgb multiplier.
layout(set = 3, binding = 4) uniform sampler2D lighting_ramp;
// Half-resolution ambient occlusion (R6 GTAO, ADR-0015); white 1x1 without AO. Bindings 5-9
// are R3a's world bindings (atmosphere.glsl, terrain.frag; ADR-0014 §6).
layout(set = 3, binding = 10) uniform sampler2D ambient_occlusion;
// Dynamic point lights (R2b): two vec4 per light, xyz position + w radius, rgb colour.
layout(std430, set = 3, binding = 11) readonly buffer LightList {
    vec4 lights[];
} light_list;
// Light grid: per column `camera.light_info.z` words, a count then light indices.
layout(std430, set = 3, binding = 12) readonly buffer LightGrid {
    uint cells[];
} light_grid;
// Baked top-down ambient occlusion of the static map (R8, 1 = open).
layout(set = 3, binding = 13) uniform sampler2D baked_ao_map;
// Screen-space contact-shadow visibility of the main view (R8, 1 = lit).
layout(set = 3, binding = 14) uniform sampler2D contact_map;

const float MOBA_PI = 3.14159265;
const float MOBA_MIN_ROUGHNESS = 0.045;
// Upper bound of shade()'s output: finite in R16G16B16A16_SFLOAT and B10G11R11_UFLOAT.
const float MOBA_MAX_RADIANCE = 32768.0;

// Inputs of one shaded point. `normal` and `geometric_normal` are normalised world vectors.
struct Surface {
    vec3 albedo;
    vec3 normal;
    vec3 geometric_normal;
    vec3 world_pos;
    float metallic;
    float roughness;
    // Stylisation, already resolved from material and map: wrap 0..1, rim and ramp
    // multipliers, painted-specular tint (rgb) and scale (w).
    float wrap;
    float rim;
    float ramp;
    vec4 specular;
    vec3 emissive;
    // Baked ambient occlusion (1 = open) that joins the screen-space AO as min(baked, GTAO):
    // R3a's terrain relief while GTAO runs (ADR-0015 §5), 1 everywhere else. The map's baked
    // AO (R2b) multiplies the result (ADR-0016).
    float occlusion;
#ifdef MOBA_TIERED_SHADING
    // The dielectric F0 (ADR-0023 §6b: from the material's `ior` and `specular`, 0.04 by
    // default). Tiered only: Compat keeps its constant 0.04 until C1.
    float f0;
#endif
#ifdef MOBA_SHADING_MODELS
    // The G-buffer's model and its data in physical units (ADR-0023 §6c; extended tiles).
    uint model;
    vec3 model_data;
#endif
};

#ifdef MOBA_TIERED_SHADING
// Multiscatter energy compensation of the GGX lobes (ADR-0023 §6b; Kulla and Conty 2017,
// Filament's DFG form): the single-scatter specular times 1 + F0 (1 / E_ss - 1), where
// E_ss = A + B is the split-sum LUT's value at F0 = 1, so no new LUT is needed. Set by
// `shade()` before the light loop reads it. Tiered only (`MOBA_TIERED_SHADING`: the deferred
// lighting and the tiered forward permutations); Compat gains it with C1.
vec3 moba_energy_compensation = vec3(1.0);

vec3 multiscatter_compensation(vec3 f0, vec2 brdf) {
    return vec3(1.0) + f0 * (1.0 / max(brdf.x + brdf.y, 1e-4) - 1.0);
}

// The split-sum LUT at (N·V, roughness). Tiered's LUT (`brdf_lut_tiered` in lighting.rs)
// puts its texel centres on the end points, texel i at i / (size − 1) on both axes, so one
// bilinear fetch covers 0..=1 without clamping the roughest lobes' row (ADR-0023 amendment 3;
// the white furnace test in `lighting.rs` models this fetch). `MOBA_BRDF_LUT_SIZE` is
// `BRDF_LUT_SIZE`. Since N2d the LUT is the corner of an atlas that also holds the LTC table
// (`lut_atlas` in ltc.rs, `ltc.glsl`): `MOBA_LUT_ATLAS` is its size in texels.
#define MOBA_BRDF_LUT_SIZE 32.0
#define MOBA_LUT_ATLAS vec2(160.0, 64.0)
vec2 brdf_lut_sample(float n_dot_v, float roughness) {
    vec2 uv = (vec2(n_dot_v, roughness) * (MOBA_BRDF_LUT_SIZE - 1.0) + 0.5) / MOBA_LUT_ATLAS;
    return textureLod(brdf_lut, uv, 0.0).rg;
}
#endif

// SH-L2 irradiance / pi in direction `n` (crates/tools env-bake convention).
vec3 sh_irradiance(vec3 n) {
    vec3 result = camera.sh[0].rgb * 0.282095
        + camera.sh[1].rgb * (0.488603 * n.y)
        + camera.sh[2].rgb * (0.488603 * n.z)
        + camera.sh[3].rgb * (0.488603 * n.x)
        + camera.sh[4].rgb * (1.092548 * n.x * n.y)
        + camera.sh[5].rgb * (1.092548 * n.y * n.z)
        + camera.sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0))
        + camera.sh[7].rgb * (1.092548 * n.x * n.z)
        + camera.sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
    return max(result, vec3(0.0));
}

// Hemispheric ambient (the pre-R2 look): sky from above, ground from below.
vec3 hemispheric(vec3 n) {
    return mix(camera.ambient_ground.rgb, camera.ambient.rgb, n.y * 0.5 + 0.5);
}

// Diffuse irradiance / pi from the environment, or the hemispheric fallback.
vec3 ambient_irradiance(vec3 n) {
    float intensity = camera.ibl_params.x;
    return intensity > 0.0 ? sh_irradiance(n) * intensity : hemispheric(n);
}

// Prefiltered specular radiance in direction `r` for `roughness`.
vec3 ambient_specular(vec3 r, float roughness) {
    float intensity = camera.ibl_params.x;
    if (intensity <= 0.0) {
        return hemispheric(r);
    }
    if (camera.ibl_params.z > 0.5) {
        float lod = roughness * camera.ibl_params.y;
        return textureLod(environment_specular, r, lod).rgb * intensity;
    }
    // Low tier: no cube fetch; the irradiance in the reflection direction stands in.
    return sh_irradiance(r) * intensity;
}

// Screen-space ambient occlusion under this fragment (1 = open), blended by the AO
// strength. Main view only: offscreen views set the strength to 0 and skip the fetch.
float ambient_occlusion_here() {
    float strength = camera.post_params.x;
    if (strength <= 0.0) {
        return 1.0;
    }
    vec2 uv = min(MOBA_PIXEL_CENTER * camera.post_params.yz, camera.post_clamp.xy);
    return mix(1.0, textureLod(ambient_occlusion, uv, 0.0).r, strength);
}

// Specular occlusion from ambient occlusion (Lagarde and de Rousiers 2014).
float specular_occlusion(float n_dot_v, float ao, float roughness) {
    return clamp(pow(n_dot_v + ao, exp2(-16.0 * roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
}

float distribution_ggx(float n_dot_h, float alpha) {
    float alpha2 = alpha * alpha;
    float d = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    return alpha2 / (MOBA_PI * d * d);
}

// Height-correlated Smith-GGX visibility G / (4 N.L N.V).
float visibility_smith_ggx(float n_dot_v, float n_dot_l, float alpha) {
    float alpha2 = alpha * alpha;
    float ggx_v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2);
    float ggx_l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2);
    return 0.5 / max(ggx_v + ggx_l, 1e-5);
}

vec3 fresnel_schlick(vec3 f0, float cosine) {
    float f = pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
    return f0 + (vec3(1.0) - f0) * f;
}

vec3 fresnel_schlick_roughness(vec3 f0, float cosine, float roughness) {
    float f = pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
    return f0 + (max(vec3(1.0 - roughness), f0) - f0) * f;
}

// Vogel (golden-angle) disk sample `index` of `count`, rotated by `phi`.
vec2 vogel_disk(int index, int count, float phi) {
    float radius = sqrt((float(index) + 0.5) / float(count));
    float theta = float(index) * 2.39996323 + phi;
    return vec2(cos(theta), sin(theta)) * radius;
}

// Fraction of the key light reaching `world_pos` (1 = lit).
float shadow_visibility(vec3 world_pos, vec3 geometric_normal) {
    float mode = camera.shadow_params.x;
    if (mode < 0.5) {
        return 1.0;
    }
    // Normal offset scaled by how grazing the light is, on top of the pass's slope bias.
    vec3 light = normalize(camera.light_direction.xyz);
    float grazing = 1.0 - clamp(dot(geometric_normal, light), 0.0, 1.0);
    vec3 offset_pos = world_pos + geometric_normal * (camera.shadow_params.w * (0.25 + grazing));
    vec4 clip = camera.light_view_proj * vec4(offset_pos, 1.0);
    vec3 coord = vec3(clip.xy * 0.5 + 0.5, clip.z);
    if (any(lessThan(coord, vec3(0.0))) || any(greaterThan(coord, vec3(1.0)))) {
        return 1.0; // Outside the fitted map: lit.
    }
    float texel = camera.shadow_params.y;
    if (mode < 1.5) {
        return textureLod(shadow_map, coord, 0.0);
    }
    int taps = int(camera.shadow_extra.x);
    if (mode < 2.5) {
        // 3x3 bilinear PCF: nine hardware taps, a 4x4-texel tent.
        float sum = 0.0;
        for (int y = -1; y <= 1; y++) {
            for (int x = -1; x <= 1; x++) {
                sum += textureLod(shadow_map, coord + vec3(vec2(x, y) * texel, 0.0), 0.0);
            }
        }
        return sum / 9.0;
    }
    // PCSS-lite: blocker search, penumbra from the blocker/receiver depth gap, variable PCF.
    // One fixed disk for every pixel: without TAA a per-pixel rotation shows as grain, and
    // bilinear compare taps keep the fixed pattern smooth (ADR-0011).
    const float phi = 0.0;
    float search = camera.shadow_extra.y * texel;
    float blocker_sum = 0.0;
    float blockers = 0.0;
    for (int i = 0; i < 16; i++) {
        vec2 uv = coord.xy + vogel_disk(i, 16, phi) * search;
        float depth = textureLod(shadow_depth, uv, 0.0).r;
        if (depth < coord.z) {
            blocker_sum += depth;
            blockers += 1.0;
        }
    }
    if (blockers < 0.5) {
        return 1.0;
    }
    float blocker = blocker_sum / blockers;
    float penumbra = clamp((coord.z - blocker) * camera.shadow_params.z, 1.0, camera.shadow_extra.z);
    float sum = 0.0;
    for (int i = 0; i < taps; i++) {
        vec2 uv = coord.xy + vogel_disk(i, taps, phi) * penumbra * texel;
        sum += textureLod(shadow_map, vec3(uv, coord.z), 0.0);
    }
    return sum / float(max(taps, 1));
}

// Baked map AO at `p`: the top-down map at p.xz, faded out over the height falloff.
float baked_occlusion(vec3 p) {
    float strength = camera.occlusion.x;
    if (strength <= 0.0) {
        return 1.0;
    }
    vec2 uv = (p.xz - camera.baked_ao.xy) * camera.baked_ao.z;
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) {
        return 1.0;
    }
    float ao = textureLod(baked_ao_map, uv, 0.0).r;
    float fade = clamp(p.y / max(camera.baked_ao.w, 1e-3), 0.0, 1.0);
    return mix(1.0, mix(ao, 1.0, fade), strength);
}

// Screen-space contact-shadow visibility of this fragment (1 = lit); 1 when not bound.
float contact_visibility() {
    if (camera.occlusion.y < 0.5) {
        return 1.0;
    }
    return texelFetch(contact_map, ivec2(MOBA_PIXEL_CENTER), 0).r;
}

// Readability limits of the dynamic lights (ADR-0016, ADR-0011 rule 1). The inverse-square
// falloff is floored at 1 m, so a light inside a unit does not reach 4-40x its colour; and the
// lights' summed irradiance at a fragment is clamped to `MOBA_DYNAMIC_LIGHT_CAP` times the
// sun's (at least 1), so a pile of hit flashes brightens a unit without bleaching its team
// colour to white.
const float MOBA_LIGHT_NEAR_FIELD = 1.0;
const float MOBA_DYNAMIC_LIGHT_CAP = 1.5;

float moba_luminance(vec3 color) {
    return dot(color, vec3(0.2126, 0.7152, 0.0722));
}

#ifdef MOBA_SHADING_MODELS
#include "shading_models.glsl"
#endif

#ifdef MOBA_TIERED_SHADING
#include "ltc.glsl"

// N2d (ADR-0023 §4): how shaped lights are lit, `LightingConfig::area_lights`: 0 as points at
// their centres (`MOBA_AREA_LIGHTS=off`), 1 at the representative point (Low), 2 with LTC
// (Medium and up). The forward stages read the camera's `occlusion.z`; the deferred lighting
// defines it as its specialisation constant, set from the same value.
#ifndef MOBA_AREA_MODE
#define MOBA_AREA_MODE uint(camera.occlusion.z)
#endif
// Whether this stage has the LTC evaluation at all: the water defines it 0 and lights shaped
// lights at their representative point in mode 2 too (its registers cost the water pass
// 0.006 ms every frame, shaped light or not; ADR-0023 amendment 4).
#ifndef MOBA_AREA_LTC
#define MOBA_AREA_LTC 1
#endif
// Where the shapes start in the light list, in `vec4`s (`SHAPE_BASE` in lights.rs).
#define MOBA_LIGHT_SHAPE_BASE 512u

// A rect's half-extent directions from its shape `vec4` (`LightShape::words`): `xyz` the
// normal times the half-width, `w` the half-height; the width runs along the horizontal
// normalize(Y x normal) (+X when the normal is vertical).
void moba_rect_axes(vec4 shape, out vec3 normal, out vec3 ex, out vec3 ey) {
    float half_width = length(shape.xyz);
    normal = shape.xyz / max(half_width, 1e-6);
    vec3 across = cross(vec3(0.0, 1.0, 0.0), normal);
    float across2 = dot(across, across);
    vec3 width = across2 > 1e-8 ? across * inversesqrt(across2) : vec3(1.0, 0.0, 0.0);
    ex = width * half_width;
    ey = cross(normal, width) * shape.w;
}

// The representative point (Karis 2013): the point of the shape (`tag`, `shape`, centred at
// `to_light` from the shaded point) closest to the reflection ray `r`. Low lights every shape
// at it, and extended models' lobes too.
vec3 moba_representative_point(float tag, vec4 shape, vec3 to_light, vec3 r) {
    vec3 centre = to_light;
    float radius = shape.w;
    if (tag > 2.5) {
        vec3 normal;
        vec3 ex;
        vec3 ey;
        moba_rect_axes(shape, normal, ex, ey);
        float facing = dot(r, normal);
        float t = facing < 0.0 ? dot(to_light, normal) / facing : -1.0;
        vec3 local = (t > 0.0 ? r * t : to_light) - to_light;
        float half_width = length(ex);
        float half_height = length(ey);
        vec3 x = ex / max(half_width, 1e-6);
        vec3 y = ey / max(half_height, 1e-6);
        return to_light + x * clamp(dot(local, x), -half_width, half_width)
            + y * clamp(dot(local, y), -half_height, half_height);
    }
    if (tag > 1.5) {
        // The segment's point closest to the ray, then the sphere around it.
        vec3 p0 = to_light - shape.xyz;
        vec3 segment = 2.0 * shape.xyz;
        float r_dot_segment = dot(r, segment);
        float denominator = dot(segment, segment) - r_dot_segment * r_dot_segment;
        float t = denominator > 1e-8
            ? clamp((dot(r, p0) * r_dot_segment - dot(p0, segment)) / denominator, 0.0, 1.0)
            : 0.5;
        centre = p0 + segment * t;
    }
    vec3 centre_to_ray = dot(centre, r) * r - centre;
    float reach = length(centre_to_ray);
    return centre + centre_to_ray * clamp(radius / max(reach, 1e-6), 0.0, 1.0);
}

// One shaped light by LTC: `(diffuse form factor, specular lobe integral)` and, in
// `radiance_scale`, the radiance per unit of colour that gives the shape the point light's
// total power: sphere 1 / R², capsule 1 / (R² + R h), one-sided rect pi / (w h) for the
// half-length h and half-extents w, h. A sphere is its own vector form factor (the diffuse)
// and the disk spanning its cone (the lobe); a capsule and a rect are one quad (`ltc.glsl`).
// Everything is evaluated in the shading frame (t1 toward v, t2, n).
vec2 moba_area_integrals(float tag, vec4 shape, vec3 to_light, vec3 n, vec3 v, float n_dot_v,
                         float roughness, out vec2 magnitude_fresnel, out float radiance_scale) {
    vec3 t1 = v - n * dot(v, n);
    float t1_length2 = dot(t1, t1);
    vec3 helper = abs(n.x) < 0.9 ? vec3(1.0, 0.0, 0.0) : vec3(0.0, 1.0, 0.0);
    t1 = t1_length2 > 1e-8 ? t1 * inversesqrt(t1_length2) : normalize(cross(n, helper));
    vec3 t2 = cross(n, t1);
    vec2 coord = vec2(roughness, sqrt(1.0 - clamp(n_dot_v, 0.0, 1.0)));
    magnitude_fresnel = ltc_fetch(1.0, coord).xy;
    vec4 m = ltc_fetch(0.0, coord);
    float radius = shape.w;
    float distance = length(to_light);
    vec3 centre = vec3(dot(to_light, t1), dot(to_light, t2), dot(to_light, n));
    if (tag < 1.5) {
        radiance_scale = 1.0 / max(radius * radius, 1e-6);
        float grow = ltc_grow(distance, radius);
        return vec2(ltc_sphere_form(centre, radius * grow), ltc_sphere(m, centre, radius * grow))
            / (grow * grow);
    }
    vec3 q[4];
    float grow;
    if (tag > 2.5) {
        vec3 normal;
        vec3 ex;
        vec3 ey;
        moba_rect_axes(shape, normal, ex, ey);
        radiance_scale = MOBA_PI / max(length(ex) * length(ey), 1e-6);
        if (dot(to_light, normal) >= 0.0) {
            return vec2(0.0); // Behind the emitting side.
        }
        grow = ltc_grow(distance, max(length(ex), length(ey)));
        ex *= grow;
        ey *= grow;
        q[0] = to_light - ex - ey;
        q[1] = to_light - ex + ey;
        q[2] = to_light + ex + ey;
        q[3] = to_light + ex - ey;
    } else {
        float half_length = length(shape.xyz);
        radiance_scale = 1.0 / max(radius * radius + radius * half_length, 1e-6);
        grow = ltc_grow(distance, radius + half_length);
        vec3 half_axis = shape.xyz * grow;
        radius *= grow;
        ltc_facing_quad(to_light - half_axis, to_light + half_axis, radius,
                        0.25 * MOBA_PI * radius, q);
    }
    for (int k = 0; k < 4; k++) {
        q[k] = vec3(dot(q[k], t1), dot(q[k], t2), dot(q[k], n));
    }
    // The clamped cosine (the diffuse), then the lobe, over the same corners.
    vec2 integral;
    for (int k = 0; k < 2; k++) {
        integral[k] = ltc_quad(k == 0 ? vec4(1.0, 0.0, 0.0, 1.0) : m, q);
    }
    return integral / (grow * grow);
}
#endif

// Clustered dynamic point lights (ADR-0016): the lights binned into this fragment's grid
// column, each with a windowed inverse-square falloff, wrap-lit diffuse and GGX specular,
// unshadowed. Lights only add light, so team colour in albedo survives (ADR-0011 rule 1).
vec3 dynamic_lights(Surface surface, vec3 n, vec3 v, float n_dot_v, vec3 f0, float alpha, vec3 sun) {
    uint total = camera.light_info.w;
    uint stride = camera.light_info.z;
    if (total == 0u || stride < 2u) {
        return vec3(0.0);
    }
    uvec2 dims = max(camera.light_info.xy, uvec2(1u));
    vec2 cell_f = (surface.world_pos.xz - camera.light_grid.xy) * camera.light_grid.zw;
    ivec2 cell = clamp(ivec2(floor(cell_f)), ivec2(0), ivec2(dims) - 1);
    uint base = (uint(cell.y) * dims.x + uint(cell.x)) * stride;
    uint count = min(light_grid.cells[base], stride - 1u);
    vec3 sum = vec3(0.0);
    float irradiance = 0.0;
    for (uint i = 0u; i < count; i++) {
        uint index = min(light_grid.cells[base + 1u + i], total - 1u);
        vec4 position_radius = light_list.lights[index * 2u];
        vec3 color = light_list.lights[index * 2u + 1u].rgb;
        vec3 to_light = position_radius.xyz - surface.world_pos;
        float d2 = dot(to_light, to_light);
        float radius2 = position_radius.w * position_radius.w;
        if (d2 >= radius2) {
            continue;
        }
        float ratio = d2 / radius2;
        float window = clamp(1.0 - ratio * ratio, 0.0, 1.0);
        float attenuation = window * window / max(d2, MOBA_LIGHT_NEAR_FIELD);
        vec3 l = to_light * inversesqrt(max(d2, 1e-8));
#ifdef MOBA_TIERED_SHADING
        // N2d: a shaped light (ADR-0023 §4a), LTC from Medium, else at its representative
        // point; Compat and `MOBA_AREA_MODE` 0 light it as the point at its centre.
        float shape_tag = light_list.lights[index * 2u + 1u].w;
        if (shape_tag > 0.5 && MOBA_AREA_MODE != 0u) {
            vec4 shape = light_list.lights[MOBA_LIGHT_SHAPE_BASE + index];
#if MOBA_AREA_LTC
#ifdef MOBA_SHADING_MODELS
            // The extended models' own lobes (coat, sheen, anisotropy, film) are lit at the
            // representative point, as §4d does for sheen.
            bool ltc = MOBA_AREA_MODE == 2u
                && (surface.model < MOBA_MODEL_CLEARCOAT || surface.model > MOBA_MODEL_THIN_FILM);
#else
            bool ltc = MOBA_AREA_MODE == 2u;
#endif
            if (ltc) {
                vec2 magnitude_fresnel;
                float radiance_scale;
                vec2 integral = moba_area_integrals(shape_tag, shape, to_light, n, v, n_dot_v,
                                                    sqrt(alpha), magnitude_fresnel,
                                                    radiance_scale);
                vec3 radiance = color * (radiance_scale * window * window);
                // The diffuse's Fresnel at the half vector toward the centre (the point
                // light's, exactly, as the shape shrinks); no wrap (ADR-0023 amendment 4).
                vec3 half_centre = v + l;
                float half_centre2 = dot(half_centre, half_centre);
                vec3 h_centre = half_centre2 > 1e-12 ? half_centre * inversesqrt(half_centre2) : n;
                vec3 kd_area = (vec3(1.0) - fresnel_schlick(f0, max(dot(v, h_centre), 0.0)))
                    * (1.0 - surface.metallic);
                vec3 specular_area = integral.y
                    * (f0 * magnitude_fresnel.x + (vec3(1.0) - f0) * magnitude_fresnel.y)
                    * surface.specular.rgb * surface.specular.w * moba_energy_compensation;
                sum += (kd_area * surface.albedo * integral.x + specular_area) * radiance;
                irradiance += moba_luminance(radiance) * integral.x;
                continue;
            }
#endif
            vec3 point = moba_representative_point(shape_tag, shape, to_light, reflect(-v, n));
            l = point * inversesqrt(max(dot(point, point), 1e-8));
            if (shape_tag > 2.5) {
                // A one-sided rect's intensity, 4 cos θ times the point's for the same
                // power, toward its front only.
                vec3 rect_normal = shape.xyz / max(length(shape.xyz), 1e-6);
                attenuation *= 4.0 * max(-dot(to_light * inversesqrt(max(d2, 1e-8)), rect_normal), 0.0);
            }
        }
#endif
        float n_dot_l_raw = dot(n, l);
        float n_dot_l = max(n_dot_l_raw, 0.0);
        float wrapped = clamp((n_dot_l_raw + surface.wrap) / (1.0 + surface.wrap), 0.0, 1.0);
        vec3 half_sum = v + l;
        float half_length2 = dot(half_sum, half_sum);
        vec3 h = half_length2 > 1e-12 ? half_sum * inversesqrt(half_length2) : n;
        vec3 fresnel = fresnel_schlick(f0, max(dot(v, h), 0.0));
        vec3 kd = (vec3(1.0) - fresnel) * (1.0 - surface.metallic);
        float d = distribution_ggx(max(dot(n, h), 0.0), alpha);
        float vis = visibility_smith_ggx(n_dot_v, n_dot_l, alpha);
        vec3 specular = MOBA_PI * d * vis * fresnel * n_dot_l
            * surface.specular.rgb * surface.specular.w;
#ifdef MOBA_TIERED_SHADING
        specular *= moba_energy_compensation;
#endif
#ifdef MOBA_SHADING_MODELS
        sum += model_direct(surface, n, v, l, h, n_dot_v, n_dot_l, alpha, f0,
                            kd * surface.albedo * wrapped, specular) * color * attenuation;
#else
        sum += (kd * surface.albedo * wrapped + specular) * color * attenuation;
#endif
        irradiance += moba_luminance(color) * attenuation * wrapped;
    }
    float cap = MOBA_DYNAMIC_LIGHT_CAP * max(moba_luminance(sun), 1.0);
    return irradiance > cap ? sum * (cap / irradiance) : sum;
}

// Shades `surface`: key light (shadowed), dynamic lights, ambient, rim and emissive.
// Returns linear HDR.
vec3 shade(Surface surface) {
    vec3 n = surface.normal;
    vec3 v = normalize(camera.camera_pos.xyz - surface.world_pos);
    vec3 l = normalize(camera.light_direction.xyz);
    // Safe half vector: v + l vanishes when the eye looks straight back at the sun.
    vec3 half_sum = v + l;
    float half_length2 = dot(half_sum, half_sum);
    vec3 h = half_length2 > 1e-12 ? half_sum * inversesqrt(half_length2) : n;
    float n_dot_v = max(dot(n, v), 1e-4);
    float n_dot_l_raw = dot(n, l);
    float n_dot_l = max(n_dot_l_raw, 0.0);
    float n_dot_h = max(dot(n, h), 0.0);
    float v_dot_h = max(dot(v, h), 0.0);

    float roughness = clamp(surface.roughness, MOBA_MIN_ROUGHNESS, 1.0);
    float alpha = roughness * roughness;
#ifdef MOBA_TIERED_SHADING
    vec3 f0 = mix(vec3(surface.f0), surface.albedo, surface.metallic);
    vec2 brdf = brdf_lut_sample(n_dot_v, roughness);
    moba_energy_compensation = multiscatter_compensation(f0, brdf);
#ifdef MOBA_SHADING_MODELS
    model_prepare(surface, n, v, n_dot_v, alpha, f0);
#endif
#else
    vec3 f0 = mix(vec3(0.04), surface.albedo, surface.metallic);
#endif
    vec3 sun = camera.sun_color.rgb * camera.light_direction.w;

    // Shadowing: strength < 1 keeps a floor of direct light (units in shadow stay readable).
    // Contact shadows (High+) tighten the map's shadow at contact points; the strength
    // floor below still applies to their product.
    float shadow = shadow_visibility(surface.world_pos, surface.geometric_normal)
        * contact_visibility();
    float lit = mix(1.0, shadow, camera.sun_color.w);

    // Diffuse: wrap lighting, then the optional ramp remap of N.L.
    float wrap = surface.wrap;
    float wrapped = clamp((n_dot_l_raw + wrap) / (1.0 + wrap), 0.0, 1.0);
    vec3 diffuse_term = vec3(wrapped);
    if (surface.ramp > 0.0) {
        vec3 ramp = textureLod(lighting_ramp, vec2(n_dot_l_raw * 0.5 + 0.5, 0.5), 0.0).rgb;
        diffuse_term = mix(diffuse_term, ramp * wrapped, surface.ramp);
    }
    vec3 fresnel = fresnel_schlick(f0, v_dot_h);
    vec3 kd = (vec3(1.0) - fresnel) * (1.0 - surface.metallic);
    vec3 diffuse = kd * surface.albedo * diffuse_term;

    // Specular: GGX x Smith x Schlick, times the painted tint and scale (pi folded into sun).
    float d = distribution_ggx(n_dot_h, alpha);
    float vis = visibility_smith_ggx(n_dot_v, n_dot_l, alpha);
    vec3 specular = MOBA_PI * d * vis * fresnel * n_dot_l * surface.specular.rgb * surface.specular.w;
#ifdef MOBA_TIERED_SHADING
    specular *= moba_energy_compensation;
#endif

#ifdef MOBA_SHADING_MODELS
    vec3 direct = model_direct(surface, n, v, l, h, n_dot_v, n_dot_l, alpha, f0, diffuse,
                               specular) * sun * lit
        + dynamic_lights(surface, n, v, n_dot_v, f0, alpha, sun);
#else
    vec3 direct = (diffuse + specular) * sun * lit
        + dynamic_lights(surface, n, v, n_dot_v, f0, alpha, sun);
#endif

    // Ambient: SH or hemispheric diffuse, prefiltered specular through the split-sum LUT.
    vec3 f_ambient = fresnel_schlick_roughness(f0, n_dot_v, roughness);
    vec3 kd_ambient = (vec3(1.0) - f_ambient) * (1.0 - surface.metallic);
#ifndef MOBA_TIERED_SHADING
    vec2 brdf = textureLod(brdf_lut, vec2(n_dot_v, roughness), 0.0).rg;
#endif
    vec3 r = reflect(-v, n);
    vec3 ambient_diffuse = kd_ambient * surface.albedo * ambient_irradiance(n);
#ifdef MOBA_SHADING_MODELS
    vec3 ambient_spec = ambient_specular(model_ambient_direction(surface, n, v, r, roughness),
                                         roughness)
        * (model_ambient_f0(surface, f0) * brdf.x + brdf.y)
        * surface.specular.rgb * surface.specular.w;
#else
    vec3 ambient_spec = ambient_specular(r, roughness) * (f0 * brdf.x + brdf.y)
        * surface.specular.rgb * surface.specular.w;
#endif
#ifdef MOBA_TIERED_SHADING
    ambient_spec *= moba_energy_compensation;
#endif
    // R6: AO occludes the ambient (indirect) terms only; direct light, rim and emissive
    // keep their readability (ADR-0015). R2b's baked map AO multiplies it: the map's static
    // occluders, which GTAO sees only on screen and at half resolution (ADR-0016).
    float ao = min(surface.occlusion, ambient_occlusion_here()) * baked_occlusion(surface.world_pos);
    ambient_diffuse *= ao;
#ifdef MOBA_SHADING_MODELS
    float specular_ao = specular_occlusion(n_dot_v, ao, roughness);
    ambient_spec *= specular_ao;
    vec3 ambient_models = model_ambient(surface, n, v, n_dot_v, ambient_diffuse + ambient_spec,
                                        ao, specular_ao);
    ambient_diffuse = ambient_models;
    ambient_spec = vec3(0.0);
#else
    ambient_spec *= specular_occlusion(n_dot_v, ao, roughness);
#endif

    // Rim: a coloured silhouette term, strongest where the surface turns away from the eye.
    // Half of it takes the surface's own hue (albedo normalised to its brightest channel), so
    // a map's warm or cold rim colour never repaints a unit's team colour (ADR-0011 rule 1).
    float rim = pow(1.0 - n_dot_v, 3.0) * camera.style.y * surface.rim;
    float peak = max(max(surface.albedo.r, surface.albedo.g), max(surface.albedo.b, 1e-3));
    vec3 hue = mix(vec3(1.0), surface.albedo / peak, 0.5);
    vec3 rim_light = rim * camera.rim_color.rgb * hue;

    // A GGX peak at the minimum roughness reaches ~7e4, past the fp16 target's 65504 (an
    // inf would poison R6's bloom and exposure); 32768 is far above anything tonemapped.
    vec3 result = direct + ambient_diffuse + ambient_spec + rim_light + surface.emissive;
    return min(result, vec3(MOBA_MAX_RADIANCE));
}

// A surface with the map's stylisation and no emissive.
Surface default_surface(vec3 albedo, vec3 normal, vec3 world_pos, float roughness) {
    Surface surface;
    surface.albedo = albedo;
    surface.normal = normal;
    surface.geometric_normal = normal;
    surface.world_pos = world_pos;
    surface.metallic = 0.0;
    surface.roughness = roughness;
    surface.wrap = camera.style.x;
    surface.rim = 1.0;
    surface.ramp = camera.style.z;
    surface.specular = camera.specular_tint;
    surface.emissive = vec3(0.0);
    surface.occlusion = 1.0;
#ifdef MOBA_TIERED_SHADING
    surface.f0 = 0.04; // IOR 1.5 (ADR-0023 §6b)
#endif
#ifdef MOBA_SHADING_MODELS
    surface.model = 1u; // STANDARD
    surface.model_data = vec3(0.0);
#endif
    return surface;
}

#endif
