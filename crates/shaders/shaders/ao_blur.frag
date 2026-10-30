#version 450
#extension GL_GOOGLE_include_directive : require

// AO denoise (R6, ADR-0015): a 4x4 box over the GTAO output, which averages the 4x4
// slice-rotation pattern exactly, weighted by the relative difference of each tap's eye
// distance from the centre's so occlusion never bleeds across silhouettes.

layout(set = 0, binding = 0) uniform sampler2D raw_ao;

layout(push_constant) uniform BlurPush {
    ivec2 last;        // last valid texel of this frame's AO sub-rectangle
    float sharpness;   // depth weight: exp(-sharpness * relative distance difference)
    float reserved;
} push;

layout(location = 0) out float out_ao;

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    float centre_distance = texelFetch(raw_ao, clamp(pixel, ivec2(0), push.last), 0).g;
    float sum = 0.0;
    float weights = 0.0;
    for (int y = -1; y <= 2; y++) {
        for (int x = -1; x <= 2; x++) {
            vec2 value = texelFetch(raw_ao, clamp(pixel + ivec2(x, y), ivec2(0), push.last), 0).rg;
            float difference = abs(value.g - centre_distance) / max(centre_distance, 1e-3);
            float weight = exp(-push.sharpness * difference);
            sum += value.r * weight;
            weights += weight;
        }
    }
    out_ao = clamp(sum / max(weights, 1e-4), 0.0, 1.0);
}
