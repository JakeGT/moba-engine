#version 450
#extension GL_GOOGLE_include_directive : require

// Heat haze (ADR-0019): refracts the copy of the HDR target taken after the water and before
// the particles. Per fragment:
//   1. the sprite is depth-tested (read-only) against the scene, so nothing in front is bent;
//   2. the offset is two octaves of rising value noise, times the strength, the soft edge,
//      the soft-depth and view-distance fades and the tier's cap, clamped to the cap;
//   3. a pixel where a unit is (nonzero in the prepass ID target) keeps its colour;
//   4. a tap whose 2x2 footprint touches a unit or geometry nearer than the sprite is
//      rejected (the fragment keeps its own pixel): nothing in front is pulled into the haze
//      and no unit bleeds out of its silhouette.
// `moba_render::distortion::refract_tap` is the CPU twin of steps 2-4 (unit-tested). Every
// tap stays inside the render sub-rectangle (render scale, dynamic resolution).

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

void main() {
    ivec2 last = ivec2(push.rect.xy);
    ivec2 pixel = min(ivec2(gl_FragCoord.xy), last);
    vec2 size = vec2(textureSize(scene_copy, 0));
    bool mask = push.rect.w > 0.5;
    // 3. Units keep their pixels (alpha 0 leaves the HDR target as it is; no `discard`,
    //    which a Vulkan 1.3 target compiles to a demote the device need not enable).
    if (mask && texelFetch(unit_ids, pixel, 0).r != 0u) {
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
    float cap = push.rect.z;
    vec2 offset = direction * weight * cap;
    float length_px = length(offset);
    if (length_px > cap) {
        offset *= cap / length_px;
    }
    // The tap's centre in pixels, clamped so its bilinear footprint stays in the region.
    vec2 centre = clamp(vec2(pixel) + 0.5 + offset, vec2(0.5), vec2(last) + 0.5);
    vec2 tap_uv = centre / size;
    // 4. Reject a footprint that touches a unit or geometry in front of the haze.
    bool rejected = false;
    if (mask) {
        uvec4 ids = textureGather(unit_ids, tap_uv, 0);
        rejected = any(notEqual(ids, uvec4(0u)));
    }
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
