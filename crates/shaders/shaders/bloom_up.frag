#version 450
#extension GL_GOOGLE_include_directive : require

// Bloom up-sample (R6, ADR-0015): a 3x3 tent filter of the smaller mip, added into the
// larger one by the pipeline's additive blend (Jimenez 2014). The composite divides the
// accumulated level 0 by the number of levels.

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D source;

layout(push_constant) uniform BloomUpPush {
    vec2 dst_inv;
    vec2 scale;
    vec2 texel;
    float radius;   // tent radius in source texels
    float reserved;
} push;

layout(location = 0) out vec4 out_color;

vec3 tap(vec2 uv, vec2 offset) {
    return textureLod(source, post_clamp(uv + offset * push.texel * push.radius, push.scale, push.texel), 0.0).rgb;
}

void main() {
    vec2 uv = post_source_uv(push.dst_inv, push.scale);
    vec3 sum = tap(uv, vec2(0.0)) * 4.0;
    sum += (tap(uv, vec2(-1.0, 0.0)) + tap(uv, vec2(1.0, 0.0))
        + tap(uv, vec2(0.0, -1.0)) + tap(uv, vec2(0.0, 1.0))) * 2.0;
    sum += tap(uv, vec2(-1.0, -1.0)) + tap(uv, vec2(1.0, -1.0))
        + tap(uv, vec2(-1.0, 1.0)) + tap(uv, vec2(1.0, 1.0));
    out_color = vec4(sum / 16.0, 1.0);
}
