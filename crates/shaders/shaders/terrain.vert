#version 450
#extension GL_GOOGLE_include_directive : require

// Terrain (ADR-0014): the map's heightfield mesh (or the R2a plane), already in world space.
// The static vertex layout's UV carries the baked relief ambient occlusion in `x`.

#include "camera.glsl"

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;
layout(location = 4) in vec2 in_uv;

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_world_pos;
layout(location = 2) out float out_occlusion;

void main() {
    gl_Position = camera.view_proj * vec4(in_position, 1.0);
    out_world_normal = in_normal;
    out_world_pos = in_position;
    out_occlusion = in_uv.x;
}
