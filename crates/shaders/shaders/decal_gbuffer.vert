#version 450
#extension GL_GOOGLE_include_directive : require

// `Paint` decals in the G-buffer (ADR-0023 §7a, N2c): one box per decal (instanced), drawn
// between the G-buffer pass's receivers (the terrain) and the rest. Instance `i` draws decal
// `count - 1 - i`, so the hardware blend composites them in `apply_paint_decals`'s order
// (highest index first, the enemy telegraphs last). The box is the decal's footprint, a
// square about its centre that bounds every shape (a splat's droplets reach 1.5 radii; a
// rectangle runs one length along its yaw), and +/-4 m about its height, the projection's
// reach (`MOBA_DECAL_REACH`). A glow decal's box collapses to one point outside the clip
// volume, so it rasterises nothing (glow decals are the overlay's, ADR-0021 §5). The pipeline
// culls front faces (the fragment stage shades back faces only) and tests them against the
// read-only depth.

#include "camera.glsl"

struct DecalBox {
    vec4 placement; // xz centre, cos and sin of the yaw
    vec4 size;      // radius (length), width, half cone angle, height
    vec4 color;
    vec4 params;
};

// `DecalBuffer` in include/decals.glsl, without its grid (the vertex stage needs the records).
layout(std430, set = 1, binding = 1) readonly buffer DecalBoxes {
    vec4 rect;
    uvec4 counts; // decals, grid edge, enemy telegraphs, reserved
    DecalBox decals[128];
} boxes;

layout(location = 0) flat out uint out_index;

// The 36 corners of a box's 12 triangles, counter-clockwise seen from outside.
const vec3 CORNERS[8] = vec3[8](
    vec3(-1.0, -1.0, -1.0), vec3(1.0, -1.0, -1.0), vec3(1.0, 1.0, -1.0), vec3(-1.0, 1.0, -1.0),
    vec3(-1.0, -1.0, 1.0), vec3(1.0, -1.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(-1.0, 1.0, 1.0));
const int TRIANGLES[36] = int[36](
    0, 2, 1, 0, 3, 2,  4, 5, 6, 4, 6, 7,  0, 1, 5, 0, 5, 4,
    3, 7, 6, 3, 6, 2,  0, 4, 7, 0, 7, 3,  1, 2, 6, 1, 6, 5);

// `MOBA_DECAL_GLOW` in include/decals.glsl.
const uint DECAL_GLOW = 1u;

void main() {
    uint count = min(boxes.counts.x, 128u);
    uint index = count - 1u - min(uint(gl_InstanceIndex), count - 1u);
    out_index = index;
    if ((uint(boxes.decals[index].params.y) & DECAL_GLOW) != 0u) {
        gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
        return;
    }
    float extent = 1.5 * boxes.decals[index].size.x + boxes.decals[index].size.y + 0.5;
    vec3 corner = CORNERS[TRIANGLES[gl_VertexIndex % 36]];
    vec3 world = vec3(boxes.decals[index].placement.x + corner.x * extent,
                      boxes.decals[index].size.w + corner.y * 4.0,
                      boxes.decals[index].placement.y + corner.z * extent);
    gl_Position = camera.view_proj * vec4(world, 1.0);
}
