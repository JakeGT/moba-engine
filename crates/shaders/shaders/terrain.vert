#version 450
#extension GL_GOOGLE_include_directive : require

// Terrain: a single large plane. The grid and lane markers are procedural in the fragment
// shader, so the map needs no texture in the vertical-slice milestone.

#include "camera.glsl"

// The depth prepass (R6, ADR-0015) draws with this shader and no fragment stage; the scene
// pass then tests LESS_OR_EQUAL against that depth, so both must compute the same position.
invariant gl_Position;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_world_pos;

void main() {
    gl_Position = camera.view_proj * vec4(in_position, 1.0);
    out_world_normal = in_normal;
    out_world_pos = in_position;
}
