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
// Set 3 is the scene lighting set, written at load/resize only (crates/render/src/lighting.rs).
#ifndef MOBA_LIGHTING_GLSL
#define MOBA_LIGHTING_GLSL

#include "camera.glsl"

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
};

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

// Shades `surface`: key light (shadowed), ambient, rim and emissive. Returns linear HDR.
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
    vec3 f0 = mix(vec3(0.04), surface.albedo, surface.metallic);
    vec3 sun = camera.sun_color.rgb * camera.light_direction.w;

    // Shadowing: strength < 1 keeps a floor of direct light (units in shadow stay readable).
    float shadow = shadow_visibility(surface.world_pos, surface.geometric_normal);
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

    vec3 direct = (diffuse + specular) * sun * lit;

    // Ambient: SH or hemispheric diffuse, prefiltered specular through the split-sum LUT.
    vec3 f_ambient = fresnel_schlick_roughness(f0, n_dot_v, roughness);
    vec3 kd_ambient = (vec3(1.0) - f_ambient) * (1.0 - surface.metallic);
    vec2 brdf = textureLod(brdf_lut, vec2(n_dot_v, roughness), 0.0).rg;
    vec3 r = reflect(-v, n);
    vec3 ambient_diffuse = kd_ambient * surface.albedo * ambient_irradiance(n);
    vec3 ambient_spec = ambient_specular(r, roughness) * (f0 * brdf.x + brdf.y)
        * surface.specular.rgb * surface.specular.w;

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
    return surface;
}

#endif
