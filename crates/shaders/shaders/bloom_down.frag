#version 450
#extension GL_GOOGLE_include_directive : require

// Bloom down-sample (R6, ADR-0015): the 13-tap filter of Jimenez, "Next generation post
// processing in Call of Duty: Advanced Warfare" (SIGGRAPH 2014). Five overlapping 2x2 box
// groups (the centre group weighted 0.5, the four corner groups 0.125 each) cover a 4x4-texel
// footprint with bilinear taps. The first down-sample from the HDR target weights each group
// by 1 / (1 + luma) (the Karis average), so a single very bright pixel (a GGX peak) cannot
// produce a flickering bloom blob.

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D source;

layout(push_constant) uniform BloomDownPush {
    vec2 dst_inv;   // 1 / destination viewport size
    vec2 scale;     // the source's valid uv region
    vec2 texel;     // 1 / source allocation
    uint karis;     // 1 on the first down-sample
    uint reserved;
} push;

layout(location = 0) out vec4 out_color;

vec3 tap(vec2 uv, vec2 offset) {
    vec3 color = textureLod(source, post_clamp(uv + offset * push.texel, push.scale, push.texel), 0.0).rgb;
    // Finite, non-negative input even if a later pass ever writes something out of range.
    return clamp(color, vec3(0.0), vec3(32768.0));
}

float karis_weight(vec3 group) {
    return 1.0 / (1.0 + post_luma(group));
}

void main() {
    vec2 uv = post_source_uv(push.dst_inv, push.scale);
    vec3 a = tap(uv, vec2(-2.0, -2.0));
    vec3 b = tap(uv, vec2(0.0, -2.0));
    vec3 c = tap(uv, vec2(2.0, -2.0));
    vec3 d = tap(uv, vec2(-2.0, 0.0));
    vec3 e = tap(uv, vec2(0.0, 0.0));
    vec3 f = tap(uv, vec2(2.0, 0.0));
    vec3 g = tap(uv, vec2(-2.0, 2.0));
    vec3 h = tap(uv, vec2(0.0, 2.0));
    vec3 i = tap(uv, vec2(2.0, 2.0));
    vec3 j = tap(uv, vec2(-1.0, -1.0));
    vec3 k = tap(uv, vec2(1.0, -1.0));
    vec3 l = tap(uv, vec2(-1.0, 1.0));
    vec3 m = tap(uv, vec2(1.0, 1.0));

    vec3 centre = (j + k + l + m) * 0.25;
    vec3 top_left = (a + b + d + e) * 0.25;
    vec3 top_right = (b + c + e + f) * 0.25;
    vec3 bottom_left = (d + e + g + h) * 0.25;
    vec3 bottom_right = (e + f + h + i) * 0.25;
    vec3 result;
    if (push.karis != 0u) {
        float w0 = 0.5 * karis_weight(centre);
        float w1 = 0.125 * karis_weight(top_left);
        float w2 = 0.125 * karis_weight(top_right);
        float w3 = 0.125 * karis_weight(bottom_left);
        float w4 = 0.125 * karis_weight(bottom_right);
        result = (centre * w0 + top_left * w1 + top_right * w2 + bottom_left * w3
            + bottom_right * w4) / (w0 + w1 + w2 + w3 + w4);
    } else {
        result = centre * 0.5 + (top_left + top_right + bottom_left + bottom_right) * 0.125;
    }
    out_color = vec4(result, 1.0);
}
