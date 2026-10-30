#version 450
#extension GL_GOOGLE_include_directive : require

// Heat haze (ADR-0019): refracts the copy of the HDR target taken after the water and before
// the particles. Per fragment:
//   1. the sprite is depth-tested (read-only) against the scene, so nothing in front is bent;
//   2. the offset is two octaves of rising value noise, times the strength, the soft edge,
//      the soft-depth and view-distance fades and the tier's cap, clamped to the cap;
//   3. a pixel where a unit is (nonzero in the prepass ID target) or a telegraph's rim or
//      halo band is (HDR alpha below 0.999, written by the decal receivers) keeps its colour;
//   4. a tap whose 2x2 footprint touches a unit, a telegraph band or geometry nearer than the
//      sprite is rejected (the fragment keeps its own pixel): nothing in front is pulled into
//      the haze, no unit bleeds out of its silhouette and no telegraph edge moves.
// `moba_render::distortion::refract_tap` is the CPU twin of steps 2-4 (unit-tested). The
// draw is scissored to the haze's projected bounds and the copy holds those bounds plus the
// cap, so every tap stays inside the copied region (clamped to it regardless).

#include "vfx.glsl"
#include "distortion.glsl"

layout(set = 1, binding = 0) uniform sampler2D scene_copy;
layout(set = 1, binding = 1) uniform sampler2D scene_depth;
layout(set = 1, binding = 2) uniform usampler2D unit_ids;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec3 in_params;
layout(location = 2) in float in_view_depth;
layout(location = 3) flat in float in_seed;
layout(location = 4) flat in float in_age;

layout(location = 0) out vec4 out_hdr;

// Integer hash of a lattice cell, in [0, 1].
float hash(vec2 cell) {
    uvec2 q = uvec2(ivec2(cell));
    uint n = (q.x * 1597334677u) ^ (q.y * 3812015801u);
    n = (n ^ (n >> 16u)) * 2246822519u;
    n ^= n >> 13u;
    return float(n & 0xffffu) * (1.0 / 65535.0);
}

float value_noise(vec2 p) {
    vec2 cell = floor(p);
    vec2 f = p - cell;
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash(cell);
    float b = hash(cell + vec2(1.0, 0.0));
    float c = hash(cell + vec2(0.0, 1.0));
    float d = hash(cell + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float octaves(vec2 p) {
    return value_noise(p) * 0.65 + value_noise(p * 2.03 + vec2(11.7, 5.3)) * 0.35;
}

float linear_depth(float depth) {
    return frame.depth.y / (depth + frame.depth.x);
}

const float MARK_BELOW = 0.999;

void main() {
    ivec2 first = ivec2(push.rect.xy);
    ivec2 last = ivec2(push.rect.zw);
    ivec2 pixel = clamp(ivec2(gl_FragCoord.xy), first, last);
    vec2 size = vec2(textureSize(scene_copy, 0));
    // 3. Units and telegraph bands keep their pixels (alpha 0 leaves the HDR target as it
    //    is; no `discard`, which a Vulkan 1.3 target compiles to a demote the device need not
    //    enable).
    if (texelFetch(unit_ids, pixel, 0).r != 0u
        || !(texelFetch(scene_copy, pixel, 0).a >= MARK_BELOW)) {
        out_hdr = vec4(0.0);
        return;
    }
    // 2. Strength with the soft edge, the soft-depth fade and the view-distance fade.
    vec2 q = in_uv * 2.0 - 1.0;
    float edge = 1.0 - smoothstep(0.45, 1.0, length(q));
    float scene = linear_depth(texelFetch(scene_depth, pixel, 0).r);
    float soft = clamp((scene - in_view_depth) / push.fade.x, 0.0, 1.0);
    float far = 1.0 - smoothstep(push.fade.y, push.fade.z, in_view_depth);
    float weight = clamp(in_params.x, 0.0, 1.0) * edge * soft * far;
    // Rising noise: `in_params.y` cells across the sprite, `in_params.z` sprite heights per
    // second (the particle's own age, so the phase never loses float precision).
    vec2 p = vec2(in_uv.x, in_uv.y - in_age * in_params.z) * in_params.y + vec2(in_seed);
    vec2 direction = vec2(octaves(p), octaves(p + vec2(37.2, 17.9))) * 2.0 - 1.0;
    direction = clamp(direction * 1.6, vec2(-1.0), vec2(1.0));
    float cap = push.fade.w;
    vec2 offset = direction * weight * cap;
    float length_px = length(offset);
    if (length_px > cap) {
        offset *= cap / length_px;
    }
    // The tap's centre in pixels, clamped so its whole bilinear (and gather) footprint stays
    // in the copied region: the rest of the copy holds whatever the aliased slot last held.
    vec2 centre = clamp(vec2(pixel) + 0.5 + offset, vec2(first) + 1.0, vec2(last));
    vec2 tap_uv = centre / size;
    // 4. Reject a footprint that touches a unit, a telegraph band or geometry in front of
    //    the haze.
    uvec4 ids = textureGather(unit_ids, tap_uv, 0);
    vec4 marks = textureGather(scene_copy, tap_uv, 3);
    bool rejected = any(notEqual(ids, uvec4(0u)))
        || !all(greaterThanEqual(marks, vec4(MARK_BELOW)));
    vec4 depths = textureGather(scene_depth, tap_uv, 0);
    float nearest = min(min(linear_depth(depths.x), linear_depth(depths.y)),
                        min(linear_depth(depths.z), linear_depth(depths.w)));
    if (nearest < in_view_depth) {
        rejected = true;
    }
    if (rejected || weight <= 0.0) {
        out_hdr = vec4(0.0);
        return;
    }
    vec3 color = textureLod(scene_copy, tap_uv, 0.0).rgb;
    // Source-alpha blend: overlapping sprites cross-fade at their soft edges.
    out_hdr = vec4(color, clamp(weight * 8.0, 0.0, 1.0));
}
