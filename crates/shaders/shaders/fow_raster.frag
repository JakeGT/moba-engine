#version 450

// Fog-of-war raster (ADR-0014): the visibility of this texel from one source, blended with
// MAX into the R8 target. Inside the disc and with no wall crossing the segment from the
// source to the texel centre: visible; else 0 (a no-op under MAX). The disc is inset by one
// texel (`push.rect.w`) and fades over the 1.5 texels inside that, so the resolve's one-texel
// blur keeps the visible edge within the vision radius the server uses. The CPU reference is
// `world::rasterize_reference` (unit-tested).

layout(set = 0, binding = 1) readonly buffer WallBlock {
    vec4 walls[]; // xy first endpoint, zw second (world x, z)
} wall_data;

layout(push_constant) uniform FogPush {
    vec4 rect;   // xy world corner, z 1 / size, w texel size (world units)
    uvec4 counts; // x walls, y sources
} push;

layout(location = 0) in vec2 in_world;
layout(location = 1) flat in vec3 in_source;

layout(location = 0) out float out_visibility;

void main() {
    float texel = push.rect.w;
    float outer = in_source.z - texel;
    float edge = 1.5 * texel;
    float coverage = 1.0 - smoothstep(outer - edge, outer, length(in_world - in_source.xy));
    // No `discard`: with MAX blending a 0 changes nothing, and it needs no
    // demote-to-helper feature (Vulkan 1.3 targets compile `discard` to a demote).
    out_visibility = 0.0;
    if (coverage <= 0.0) {
        return;
    }
    vec2 ray = in_world - in_source.xy;
    uint count = push.counts.x;
    for (uint index = 0u; index < count; index++) {
        vec4 wall = wall_data.walls[index];
        vec2 segment = wall.zw - wall.xy;
        float denominator = ray.x * segment.y - ray.y * segment.x;
        if (abs(denominator) < 1e-5) {
            continue;
        }
        vec2 offset = wall.xy - in_source.xy;
        float along_ray = (offset.x * segment.y - offset.y * segment.x) / denominator;
        float along_wall = (offset.x * ray.y - offset.y * ray.x) / denominator;
        if (along_ray > 0.0 && along_ray < 1.0 && along_wall >= 0.0 && along_wall <= 1.0) {
            return;
        }
    }
    out_visibility = coverage;
}
