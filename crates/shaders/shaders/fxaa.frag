#version 450
#extension GL_GOOGLE_include_directive : require

// FXAA (R6, ADR-0015): our implementation of the FXAA 3.11 "quality" algorithm (Timothy
// Lottes, NVIDIA 2011), on the tonemapped, graded image at render resolution, before the
// upscale and the HUD. The composite stores perceptual luma in alpha. Per pixel: a local
// contrast test, a sub-pixel blend factor from the 3x3 neighbourhood, the edge direction,
// a search along the edge for its ends (`steps` probes with growing strides), and one
// bilinear fetch offset across the edge.

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D source;

layout(push_constant) uniform FxaaPush {
    vec2 dst_inv;
    vec2 scale;
    vec2 texel;
    float subpixel;        // sub-pixel aliasing removal, 0..1 (0.75 default)
    float edge_threshold;  // minimum local contrast relative to the maximum luma
    float edge_min;        // minimum absolute local contrast (dark areas)
    uint steps;            // edge-search probes per side (fast preset: 5, quality: 10)
    uint srgb_target;      // linearise for an *_SRGB swapchain
    uint reserved;
} push;

layout(location = 0) out vec4 out_color;

const float STRIDES[12] = float[12](1.0, 1.0, 1.0, 1.0, 1.0, 1.5, 2.0, 2.0, 2.0, 2.0, 4.0, 8.0);

vec2 clamp_uv(vec2 uv) {
    return post_clamp(uv, push.scale, push.texel);
}

float luma_at(vec2 uv) {
    return textureLod(source, clamp_uv(uv), 0.0).a;
}

vec4 output_color(vec3 color) {
    vec3 encoded = push.srgb_target != 0u ? pow(color, vec3(2.2)) : color;
    return vec4(encoded, 1.0);
}

void main() {
    vec2 texel = push.texel;
    vec2 uv = post_clamp(post_source_uv(push.dst_inv, push.scale), push.scale, texel);
    vec4 centre = textureLod(source, uv, 0.0);
    float luma_m = centre.a;
    float luma_n = luma_at(uv + vec2(0.0, -texel.y));
    float luma_s = luma_at(uv + vec2(0.0, texel.y));
    float luma_e = luma_at(uv + vec2(texel.x, 0.0));
    float luma_w = luma_at(uv + vec2(-texel.x, 0.0));
    float luma_max = max(max(max(luma_n, luma_s), max(luma_e, luma_w)), luma_m);
    float luma_min = min(min(min(luma_n, luma_s), min(luma_e, luma_w)), luma_m);
    float range = luma_max - luma_min;
    if (range < max(push.edge_min, luma_max * push.edge_threshold)) {
        out_color = output_color(centre.rgb);
        return;
    }
    float luma_nw = luma_at(uv + vec2(-texel.x, -texel.y));
    float luma_ne = luma_at(uv + vec2(texel.x, -texel.y));
    float luma_sw = luma_at(uv + vec2(-texel.x, texel.y));
    float luma_se = luma_at(uv + vec2(texel.x, texel.y));

    // Sub-pixel blend: how far the centre is from its 3x3 low-pass.
    float low_pass = (2.0 * (luma_n + luma_s + luma_e + luma_w)
        + luma_nw + luma_ne + luma_sw + luma_se) / 12.0;
    float sub = clamp(abs(low_pass - luma_m) / range, 0.0, 1.0);
    sub = (-2.0 * sub + 3.0) * sub * sub;
    float sub_offset = sub * sub * push.subpixel;

    // Edge orientation from second derivatives.
    float horizontal = abs(luma_nw - 2.0 * luma_w + luma_sw) * 1.0
        + abs(luma_n - 2.0 * luma_m + luma_s) * 2.0
        + abs(luma_ne - 2.0 * luma_e + luma_se) * 1.0;
    float vertical = abs(luma_nw - 2.0 * luma_n + luma_ne) * 1.0
        + abs(luma_w - 2.0 * luma_m + luma_e) * 2.0
        + abs(luma_sw - 2.0 * luma_s + luma_se) * 1.0;
    // `horizontal_edge`: the edge runs along x, so the blend steps in y.
    bool horizontal_edge = vertical >= horizontal;
    float luma_neg = horizontal_edge ? luma_n : luma_w;
    float luma_pos = horizontal_edge ? luma_s : luma_e;
    float gradient_neg = abs(luma_neg - luma_m);
    float gradient_pos = abs(luma_pos - luma_m);
    float step_length = horizontal_edge ? texel.y : texel.x;
    float luma_local;
    float gradient;
    if (gradient_neg >= gradient_pos) {
        step_length = -step_length;
        luma_local = 0.5 * (luma_neg + luma_m);
        gradient = gradient_neg;
    } else {
        luma_local = 0.5 * (luma_pos + luma_m);
        gradient = gradient_pos;
    }
    // Start on the edge between the centre and its steepest neighbour.
    vec2 edge_uv = uv;
    if (horizontal_edge) {
        edge_uv.y += 0.5 * step_length;
    } else {
        edge_uv.x += 0.5 * step_length;
    }
    vec2 along = horizontal_edge ? vec2(texel.x, 0.0) : vec2(0.0, texel.y);
    float scaled_gradient = 0.25 * gradient;

    // Search both ways along the edge until the luma leaves the edge's average.
    vec2 uv_neg = edge_uv - along * STRIDES[0];
    vec2 uv_pos = edge_uv + along * STRIDES[0];
    float end_neg = luma_at(uv_neg) - luma_local;
    float end_pos = luma_at(uv_pos) - luma_local;
    bool done_neg = abs(end_neg) >= scaled_gradient;
    bool done_pos = abs(end_pos) >= scaled_gradient;
    uint steps = min(push.steps, 12u);
    for (uint i = 1u; i < steps && !(done_neg && done_pos); i++) {
        if (!done_neg) {
            uv_neg -= along * STRIDES[i];
            end_neg = luma_at(uv_neg) - luma_local;
            done_neg = abs(end_neg) >= scaled_gradient;
        }
        if (!done_pos) {
            uv_pos += along * STRIDES[i];
            end_pos = luma_at(uv_pos) - luma_local;
            done_pos = abs(end_pos) >= scaled_gradient;
        }
    }
    float distance_neg = horizontal_edge ? uv.x - uv_neg.x : uv.y - uv_neg.y;
    float distance_pos = horizontal_edge ? uv_pos.x - uv.x : uv_pos.y - uv.y;
    bool nearer_neg = distance_neg < distance_pos;
    float distance = min(distance_neg, distance_pos);
    float span = distance_neg + distance_pos;
    // Only blend when the nearer end's luma moves the way the centre's does.
    bool centre_smaller = luma_m < luma_local;
    bool correct = ((nearer_neg ? end_neg : end_pos) < 0.0) != centre_smaller;
    float edge_offset = correct ? 0.5 - distance / max(span, 1e-6) : 0.0;
    float offset = max(edge_offset, sub_offset);

    vec2 final_uv = uv;
    if (horizontal_edge) {
        final_uv.y += offset * step_length;
    } else {
        final_uv.x += offset * step_length;
    }
    out_color = output_color(textureLod(source, clamp_uv(final_uv), 0.0).rgb);
}
