#version 450
#extension GL_GOOGLE_include_directive : require

// Terrain: a single large plane. The grid and lane markers are procedural in the fragment
// shader, so the map needs no texture in the vertical-slice milestone.

#include "camera.glsl"

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_world_pos;

void main() {
    gl_Position = camera.view_proj * vec4(in_position, 1.0);
    out_world_normal = in_normal;
    out_world_pos = in_position;
}
