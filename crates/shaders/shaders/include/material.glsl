// Per-material descriptor set (set 2), allocated at load time (ADR-0009, ADR-0010, ADR-0011).
//
// Binding 0 is the parameter block (`MaterialParamsGpu` in crates/render/src/material.rs);
// 1-4 are the texture set. Missing textures are bound to 1x1 defaults (white base colour,
// flat normal, metallic 0 / roughness 1, full team-colour mask), so every binding is valid.
#ifndef MOBA_MATERIAL_GLSL
#define MOBA_MATERIAL_GLSL

layout(set = 2, binding = 0) uniform MaterialBlock {
    vec4 base_color_factor; // linear RGBA
    vec4 params;            // x metallic, y roughness, z normal scale, w team-tint strength
    vec4 emissive;          // rgb emissive factor, w specular scale
    vec4 style;             // x wrap (< 0: the map's), y rim, z ramp multiplier, w reserved
    vec4 specular_tint;     // rgb painted-specular tint, w reserved
} material;

// sRGB colour (BC7_SRGB or R8G8B8A8_SRGB): the sampler decodes to linear.
layout(set = 2, binding = 1) uniform sampler2D base_color_map;
// Tangent-space normal, XY in RG (BC5_UNORM or R8G8_UNORM); Z is reconstructed.
layout(set = 2, binding = 2) uniform sampler2D normal_map;
// Linear metallic (R) and roughness (G) (BC5_UNORM or R8G8_UNORM), times the factors.
layout(set = 2, binding = 3) uniform sampler2D metallic_roughness_map;
// Linear team-colour mask in R (BC4_UNORM or R8_UNORM): 1 = tinted by the unit colour.
layout(set = 2, binding = 4) uniform sampler2D mask_map;

// Perturbs `normal` (normalised, world space) by the normal map in the vertex tangent frame
// (glTF / MikkTSpace convention: bitangent = cross(normal, tangent.xyz) * tangent.w).
vec3 material_normal(vec3 normal, vec4 tangent, vec2 uv) {
    vec2 xy = (texture(normal_map, uv).rg * 2.0 - 1.0) * material.params.z;
    vec3 tangent_normal = vec3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    vec3 t = tangent.xyz - normal * dot(normal, tangent.xyz);
    float length_squared = dot(t, t);
    if (length_squared <= 1e-12) {
        return normal; // Degenerate tangent: keep the geometric normal.
    }
    t *= inversesqrt(length_squared);
    vec3 b = cross(normal, t) * (tangent.w < 0.0 ? -1.0 : 1.0);
    return normalize(mat3(t, b, normal) * tangent_normal);
}

#endif
