#version 450
#extension GL_GOOGLE_include_directive : require

// Water surface (R3b, ADR-0017 §3): the map's water mesh, already in world space, on the
// terrain grid's vertices, so the bed depth interpolates to exactly the terrain drawn below.

#include "camera.glsl"

layout(location = 0) in vec3 in_position;
layout(location = 1) in float in_depth;
layout(location = 2) in vec2 in_flow;
layout(location = 3) in uint in_body;

layout(location = 0) out vec3 out_world_pos;
layout(location = 1) out float out_depth;
layout(location = 2) out vec2 out_flow;
layout(location = 3) flat out uint out_body;

void main() {
    out_world_pos = in_position;
    out_depth = in_depth;
    out_flow = in_flow;
    out_body = in_body;
    gl_Position = camera.view_proj * vec4(in_position, 1.0);
}
