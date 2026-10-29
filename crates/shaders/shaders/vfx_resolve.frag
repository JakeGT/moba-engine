#version 450

// Weighted blended OIT resolve (ADR-0013): the average colour of the alpha particles over the
// HDR scene, weighted by their total coverage. Blend: (SRC_ALPHA, ONE_MINUS_SRC_ALPHA).

layout(set = 0, binding = 0) uniform sampler2D accum_texture;
layout(set = 0, binding = 1) uniform sampler2D reveal_texture;

layout(location = 0) out vec4 out_color;

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    vec4 accum = texelFetch(accum_texture, pixel, 0);
    float reveal = exp(-texelFetch(reveal_texture, pixel, 0).r);
    float coverage = 1.0 - reveal;
    out_color = vec4(accum.rgb / max(accum.a, 1e-5), coverage);
}
