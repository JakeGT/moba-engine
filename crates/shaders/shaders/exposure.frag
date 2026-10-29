#version 450
#extension GL_GOOGLE_include_directive : require

// Auto exposure (R6, ADR-0015): meters the last bloom mip (1/32 of the render resolution)
// with an 8x8 grid of bilinear taps, centre-weighted, as a log-average luminance; the target
// exposure log2(key / average) is clamped to the map's EV range. The 1x1 R16F target holds the
// adapted EV: the pipeline blends this output into it with a constant factor
// k = 1 - exp(-dt * adaptation) (k = 1 on the first frame), so adaptation needs no read-back.

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D source;

layout(push_constant) uniform ExposurePush {
    vec2 scale;    // the mip's valid uv region
    vec2 texel;    // 1 / mip allocation
    float min_ev;
    float max_ev;
    float key;     // mid-grey target (0.18)
    float reserved;
} push;

layout(location = 0) out vec4 out_color;

void main() {
    float sum = 0.0;
    float weights = 0.0;
    for (int y = 0; y < 8; y++) {
        for (int x = 0; x < 8; x++) {
            vec2 cell = (vec2(x, y) + 0.5) / 8.0;
            vec2 uv = post_clamp(cell * push.scale, push.scale, push.texel);
            float luma = post_luma(textureLod(source, uv, 0.0).rgb);
            // Centre-weighted: the middle of the view counts up to twice the edges.
            float weight = 2.0 - length(cell - 0.5) * 1.414;
            sum += log2(max(luma, 1e-4)) * weight;
            weights += weight;
        }
    }
    float average = sum / weights;
    float ev = clamp(log2(push.key) - average, push.min_ev, push.max_ev);
    out_color = vec4(ev, 0.0, 0.0, 1.0);
}
