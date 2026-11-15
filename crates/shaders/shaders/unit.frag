#version 450
#extension GL_GOOGLE_include_directive : require

// Unit shading: stylised PBR (include/lighting.glsl, ADR-0011). Permutations
// (crates/shaders/build.rs):
// - default: the vertex/instance colour as albedo, dielectric, the map's vertex-colour
//   roughness and stylisation;
// - `MATERIAL_TEXTURED`: the material's base colour texture and factor, tinted by the unit
//   colour where the team mask is set (team readability), metallic/roughness from the map
//   times the factors, the normal map in the vertex tangent frame, emissive and the
//   material's stylisation overrides;
// - `GBUFFER` (ADR-0023 §1e): the G-buffer prepass's encode of the same surface (STANDARD,
//   F0 0.04, flags UNIT | NO_FOG_OF_WAR), with `PREPASS_IDS` also the outline slot.

#include "camera.glsl"
#include "lighting.glsl"
#include "atmosphere.glsl"
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

#ifdef GBUFFER
// ADR-0023 §1e: the tiered G-buffer prepass. The same surface as the forward shading,
// encoded (gbuffer.glsl) next to the motion; the forward `scene` still shades in N2a.
#include "gbuffer.glsl"
#define MOTION_VECTORS_FRAGMENT
#include "motion.glsl"
layout(location = 14) in vec4 in_motion_current;
layout(location = 15) in vec4 in_motion_previous;
layout(location = 0) out vec4 out_motion;
layout(location = 1) out vec4 out_gb0;
layout(location = 2) out vec4 out_gb1;
layout(location = 3) out vec4 out_gb2;
layout(location = 13) flat in uint in_outline_slot;
#ifdef PREPASS_IDS
layout(location = 4) out uint out_id;
#endif
#else
layout(location = 0) out vec4 out_color;
#endif

void main() {
    vec3 normal = normalize(in_normal);
    Surface surface = default_surface(in_color, normal, in_world_pos, camera.style.w);
#ifdef MATERIAL_TEXTURED
    vec4 base = texture(base_color_map, in_uv, camera.temporal.x) * material.base_color_factor;
    float tint = texture(mask_map, in_uv, camera.temporal.x).r * material.params.w;
    surface.albedo = base.rgb * mix(vec3(1.0), in_color, tint);
    vec2 metallic_roughness = texture(metallic_roughness_map, in_uv, camera.temporal.x).rg;
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
#ifdef GBUFFER
    out_motion = vec4(motion_pixels(in_motion_current, in_motion_previous), 0.0, 1.0);
    SurfaceParams params;
    params.base_color = surface.albedo;
    params.metallic = surface.metallic;
    params.normal = surface.normal;
    params.roughness = surface.roughness;
    params.model = GBUFFER_MODEL_STANDARD;
    params.model_data = vec3(0.04, 0.0, 0.0); // IOR 1.5, the default (ADR-0023 §6b)
    params.ao = surface.occlusion;
    // Units are never fogged by the fog of war (below).
    params.flags = GBUFFER_FLAG_UNIT | GBUFFER_FLAG_NO_FOG_OF_WAR;
    params.rim_slot = 0u; // Team rims arrive in N2e (ADR-0023 §8).
    encode_gbuffer(params, out_gb0, out_gb1, out_gb2);
#ifdef PREPASS_IDS
    out_id = in_outline_slot == 0u ? 8u : in_outline_slot;
#endif
#else
    // Height fog (R3a, ADR-0014). Units are never fogged by the fog of war: the server
    // decides what the client sees, so darkening a unit could only hide a visible one.
    out_color = vec4(apply_height_fog(shade(surface), in_world_pos), 1.0);
#endif
}
