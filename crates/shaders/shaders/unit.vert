#version 450
#extension GL_GOOGLE_include_directive : require

// Instanced unit rendering. One storage buffer holds every draw's model matrix and colour;
// the camera lives in a small per-frame uniform buffer.
//
// Permutations (crates/shaders/build.rs): `MATERIAL_TEXTURED` also passes the mesh UVs to the
// textured fragment shader.

#include "camera.glsl"
#include "instance.glsl"

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;
#ifdef MATERIAL_TEXTURED
layout(location = 4) in vec2 in_uv;
#endif

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;
#ifdef MATERIAL_TEXTURED
layout(location = 3) out vec2 out_uv;
#endif

void main() {
    Instance instance = instance_data.instances[gl_InstanceIndex];
    vec4 world = instance.model * vec4(in_position, 1.0);
    gl_Position = camera.view_proj * world;

    // Rig parts and instanced units may be non-uniformly scaled.
    out_world_normal = normalize(mat3(transpose(inverse(instance.model))) * in_normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
#ifdef MATERIAL_TEXTURED
    out_uv = in_uv;
#endif
}
