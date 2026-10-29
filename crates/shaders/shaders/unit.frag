#version 450
#extension GL_GOOGLE_include_directive : require

// Unit shading. Permutations (crates/shaders/build.rs):
// - default: the vertex/instance colour, lit (the pre-material look);
// - `MATERIAL_TEXTURED`: the material's base colour texture and factor, tinted by the unit
//   colour where the team mask is set, with the normal map applied. Metallic/roughness are
//   bound for R2's PBR; this lighting does not read them yet.

#include "camera.glsl"
#include "lighting.glsl"
#ifdef MATERIAL_TEXTURED
#include "material.glsl"
#endif

layout(location = 0) in vec3 in_normal;
layout(location = 1) in vec3 in_color;
layout(location = 2) in vec3 in_world_pos;
#ifdef MATERIAL_TEXTURED
layout(location = 3) in vec2 in_uv;
#endif

layout(location = 0) out vec4 out_color;

void main() {
    vec3 normal = normalize(in_normal);
    vec3 albedo = in_color;
#ifdef MATERIAL_TEXTURED
    vec4 base = texture(base_color_map, in_uv) * material.base_color_factor;
    float tint = texture(mask_map, in_uv).r * material.params.w;
    albedo = base.rgb * mix(vec3(1.0), in_color, tint);
    normal = material_normal(normal, in_world_pos, in_uv);
#endif
    out_color = vec4(unit_lighting(albedo, normal, in_world_pos), 1.0);
}
