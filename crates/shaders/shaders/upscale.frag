#version 450
#extension GL_GOOGLE_include_directive : require

// Upscale (R6, ADR-0015): the render-resolution LDR image to the swapchain, before the HUD.
// Mode 0 is one bilinear tap (Low); mode 1 is Catmull-Rom bicubic from nine bilinear taps
// (the two middle weights of each axis folded into one filtered fetch), clamped to the
// image's valid sub-rectangle and to [0, 1] (the negative lobes cannot ring past black or
// white).

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D source;

layout(push_constant) uniform UpscalePush {
    vec2 dst_inv;
    vec2 scale;
    vec2 texel;
    uint mode;
    uint srgb_target;
} push;

layout(location = 0) out vec4 out_color;

vec3 fetch(vec2 uv) {
    return textureLod(source, post_clamp(uv, push.scale, push.texel), 0.0).rgb;
}

vec3 catmull_rom(vec2 uv) {
    vec2 position = uv / push.texel;
    vec2 centre = floor(position - 0.5) + 0.5;
    vec2 f = position - centre;
    vec2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    vec2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    vec2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    vec2 w3 = f * f * (-0.5 + 0.5 * f);
    vec2 w12 = w1 + w2;
    vec2 offset12 = w2 / w12;
    vec2 uv0 = (centre - 1.0) * push.texel;
    vec2 uv3 = (centre + 2.0) * push.texel;
    vec2 uv12 = (centre + offset12) * push.texel;
    vec3 result = vec3(0.0);
    result += fetch(vec2(uv0.x, uv0.y)) * w0.x * w0.y;
    result += fetch(vec2(uv12.x, uv0.y)) * w12.x * w0.y;
    result += fetch(vec2(uv3.x, uv0.y)) * w3.x * w0.y;
    result += fetch(vec2(uv0.x, uv12.y)) * w0.x * w12.y;
    result += fetch(vec2(uv12.x, uv12.y)) * w12.x * w12.y;
    result += fetch(vec2(uv3.x, uv12.y)) * w3.x * w12.y;
    result += fetch(vec2(uv0.x, uv3.y)) * w0.x * w3.y;
    result += fetch(vec2(uv12.x, uv3.y)) * w12.x * w3.y;
    result += fetch(vec2(uv3.x, uv3.y)) * w3.x * w3.y;
    return clamp(result, 0.0, 1.0);
}

void main() {
    vec2 uv = post_source_uv(push.dst_inv, push.scale);
    vec3 color = push.mode == 1u ? catmull_rom(uv) : fetch(uv);
    vec3 encoded = push.srgb_target != 0u ? pow(color, vec3(2.2)) : color;
    out_color = vec4(encoded, 1.0);
}
