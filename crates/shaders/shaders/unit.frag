#version 450
#extension GL_GOOGLE_include_directive : require

// Unit shading: stylised PBR (include/lighting.glsl, ADR-0011). Permutations
// (crates/shaders/build.rs):
// - default: the vertex/instance colour as albedo, dielectric, the map's vertex-colour
//   roughness and stylisation;
// - `MATERIAL_TEXTURED`: the material's base colour texture and factor, tinted by the unit
//   colour where the team mask is set (team readability), metallic/roughness from the map
//   times the factors, the normal map in the vertex tangent frame, emissive and the
//   material's stylisation overrides.

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
layout(location = 4) in vec4 in_tangent;
#endif

layout(location = 0) out vec4 out_color;

void main() {
    vec3 normal = normalize(in_normal);
    Surface surface = default_surface(in_color, normal, in_world_pos, camera.style.w);
#ifdef MATERIAL_TEXTURED
    vec4 base = texture(base_color_map, in_uv) * material.base_color_factor;
    float tint = texture(mask_map, in_uv).r * material.params.w;
    surface.albedo = base.rgb * mix(vec3(1.0), in_color, tint);
    vec2 metallic_roughness = texture(metallic_roughness_map, in_uv).rg;
    surface.metallic = clamp(metallic_roughness.r * material.params.x, 0.0, 1.0);
    surface.roughness = clamp(metallic_roughness.g * material.params.y, 0.0, 1.0);
    surface.normal = material_normal(normal, in_tangent, in_uv);
    surface.emissive = material.emissive.rgb;
    if (material.style.x >= 0.0) {
        surface.wrap = material.style.x;
    }
    surface.rim = material.style.y;
    surface.ramp = camera.style.z * material.style.z;
    surface.specular = vec4(camera.specular_tint.rgb * material.specular_tint.rgb,
                            camera.specular_tint.w * material.emissive.w);
#endif
    out_color = vec4(shade(surface), 1.0);
}
