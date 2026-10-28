// Per-material descriptor set (set 2), allocated at load time (ADR-0009, ADR-0010).
//
// Binding 0 is the parameter block (`MaterialParamsGpu` in crates/render/src/material.rs);
// 1-4 are the texture set. Missing textures are bound to 1x1 defaults (white base colour,
// flat normal, metallic 0 / roughness 1, full team-colour mask), so every binding is valid.
#ifndef MOBA_MATERIAL_GLSL
#define MOBA_MATERIAL_GLSL

layout(set = 2, binding = 0) uniform MaterialBlock {
    vec4 base_color_factor; // linear RGBA
    vec4 params;            // x metallic, y roughness, z normal scale, w team-tint strength
    vec4 emissive;          // rgb emissive factor (R2), w reserved
    uvec4 flags;            // reserved for R2
} material;

// sRGB colour (BC7_SRGB or R8G8B8A8_SRGB): the sampler decodes to linear.
layout(set = 2, binding = 1) uniform sampler2D base_color_map;
// Tangent-space normal, XY in RG (BC5_UNORM or R8G8_UNORM); Z is reconstructed.
layout(set = 2, binding = 2) uniform sampler2D normal_map;
// Linear metallic (R) and roughness (G) (BC5_UNORM or R8G8_UNORM), read by R2's PBR.
layout(set = 2, binding = 3) uniform sampler2D metallic_roughness_map;
// Linear team-colour mask in R (BC4_UNORM or R8_UNORM): 1 = tinted by the unit colour.
layout(set = 2, binding = 4) uniform sampler2D mask_map;

// Perturbs `normal` (normalised, world space) by the normal map. The tangent frame comes from
// screen-space derivatives, so meshes need no tangent attribute until R2.
vec3 material_normal(vec3 normal, vec3 world_pos, vec2 uv) {
    vec2 xy = (texture(normal_map, uv).rg * 2.0 - 1.0) * material.params.z;
    vec3 tangent_normal = vec3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    vec3 dp1 = dFdx(world_pos);
    vec3 dp2 = dFdy(world_pos);
    vec2 duv1 = dFdx(uv);
    vec2 duv2 = dFdy(uv);
    vec3 dp2perp = cross(dp2, normal);
    vec3 dp1perp = cross(normal, dp1);
    vec3 tangent = dp2perp * duv1.x + dp1perp * duv2.x;
    vec3 bitangent = dp2perp * duv1.y + dp1perp * duv2.y;
    float scale = max(dot(tangent, tangent), dot(bitangent, bitangent));
    if (scale <= 1e-20) {
        return normal; // Degenerate UVs: keep the geometric normal.
    }
    float inverse_length = inversesqrt(scale);
    mat3 frame = mat3(tangent * inverse_length, bitangent * inverse_length, normal);
    return normalize(frame * tangent_normal);
}

#endif
