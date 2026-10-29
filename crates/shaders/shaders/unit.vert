#version 450
#extension GL_GOOGLE_include_directive : require

// Instanced unit rendering. One storage buffer holds every draw's model matrix and colour;
// the camera lives in a small per-frame uniform buffer.
//
// Permutations (crates/shaders/build.rs):
// - `MATERIAL_TEXTURED` also passes the mesh UVs and tangents to the textured fragment shader;
// - `SHADOW_PASS` is the depth-only shadow caster: position only, through the light's
//   view-projection (no fragment shader).

#include "camera.glsl"

// The depth prepass (R6, ADR-0015) draws with this shader and no fragment stage; the scene
// pass then tests LESS_OR_EQUAL against that depth, so both must compute the same position.
invariant gl_Position;
#include "instance.glsl"

layout(location = 0) in vec3 in_position;
#ifndef SHADOW_PASS
layout(location = 1) in vec3 in_normal;
#endif
#ifdef MATERIAL_TEXTURED
layout(location = 4) in vec2 in_uv;
layout(location = 5) in vec4 in_tangent;
#endif

#ifndef SHADOW_PASS
layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;
#endif
#ifdef MATERIAL_TEXTURED
layout(location = 3) out vec2 out_uv;
layout(location = 4) out vec4 out_world_tangent;
#endif

void main() {
    Instance instance = instance_data.instances[gl_InstanceIndex];
    vec4 world = instance.model * vec4(in_position, 1.0);
#ifdef SHADOW_PASS
    gl_Position = camera.light_view_proj * world;
#else
    gl_Position = camera.view_proj * world;

    // Rig parts and instanced units may be non-uniformly scaled.
    out_world_normal = normalize(mat3(transpose(inverse(instance.model))) * in_normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
#endif
#ifdef MATERIAL_TEXTURED
    out_uv = in_uv;
    // Tangents transform like positions (the model matrix); a mirrored model matrix flips
    // the handedness.
    float mirror = determinant(mat3(instance.model)) < 0.0 ? -1.0 : 1.0;
    out_world_tangent = vec4(mat3(instance.model) * in_tangent.xyz, in_tangent.w * mirror);
#endif
}
