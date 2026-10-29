#version 450
#extension GL_GOOGLE_include_directive : require

// Composite (ADR-0008, R6 ADR-0015): HDR scene colour + bloom -> exposure (the map's EV and,
// with auto exposure, the adapted EV) -> AgX tonemap -> colour-grading LUT -> gameplay screen
// effects (death desaturation, vignette) -> the LDR target or the swapchain. It runs at render
// resolution, so the HDR texel under the fragment is fetched directly. The tonemap lives in
// `tonemap.glsl`, shared with the UI's offscreen images (R7b).

#include "post.glsl"
#include "tonemap.glsl"

layout(set = 0, binding = 0) uniform sampler2D hdr_color;
layout(set = 0, binding = 1) uniform sampler2D bloom;
layout(set = 0, binding = 2) uniform sampler2D adapted_exposure;
layout(set = 0, binding = 3) uniform sampler3D grading_lut;

// Flags.
#define COMPOSITE_SRGB_TARGET 1u   // the target is *_SRGB: output display-linear values
#define COMPOSITE_AUTO_EXPOSURE 2u // add the adapted EV from `adapted_exposure`
#define COMPOSITE_LUMA_ALPHA 4u    // write perceptual luma to alpha (FXAA reads it)

layout(push_constant) uniform CompositePush {
    vec2 dst_inv;          // 1 / render viewport
    vec2 bloom_scale;      // bloom mip 0's valid uv region
    vec2 bloom_texel;      // 1 / bloom mip 0 allocation
    float exposure;        // linear: RenderConfig::exposure x 2^(map EV)
    float saturation;      // AgX look saturation (1.0 = the base look)
    float bloom_strength;  // mix of the (level-averaged) bloom, 0 = off
    float grading;         // LUT strength, 0 = off
    float lut_scale;       // (N - 1) / N
    float lut_offset;      // 0.5 / N
    float desaturation;    // death desaturation, 0..1
    float vignette;        // vignette strength, 0..1
    float aspect;          // viewport width / height
    uint flags;
    vec4 vignette_color;   // display-encoded rgb
} push;

layout(location = 0) out vec4 out_color;

void main() {
    vec3 hdr = texelFetch(hdr_color, ivec2(gl_FragCoord.xy), 0).rgb;
    if (push.bloom_strength > 0.0) {
        vec2 uv = post_clamp(post_source_uv(push.dst_inv, push.bloom_scale), push.bloom_scale,
            push.bloom_texel);
        // The up-chain adds five levels into mip 0; average them (energy conserving).
        vec3 glow = textureLod(bloom, uv, 0.0).rgb * 0.2;
        hdr = mix(hdr, glow, push.bloom_strength);
    }
    float exposure = push.exposure;
    if ((push.flags & COMPOSITE_AUTO_EXPOSURE) != 0u) {
        exposure *= exp2(texelFetch(adapted_exposure, ivec2(0), 0).r);
    }
    vec3 display = agx_tonemap(hdr * exposure, push.saturation);
    if (push.grading > 0.0) {
        vec3 graded = textureLod(grading_lut, display * push.lut_scale + push.lut_offset, 0.0).rgb;
        display = mix(display, graded, push.grading);
    }
    if (push.desaturation > 0.0) {
        display = mix(display, vec3(post_luma(display)), push.desaturation);
    }
    if (push.vignette > 0.0) {
        vec2 centred = (gl_FragCoord.xy * push.dst_inv - 0.5) * vec2(push.aspect, 1.0);
        float radius = length(centred) / length(vec2(push.aspect, 1.0) * 0.5);
        float edge = smoothstep(0.45, 1.0, radius) * push.vignette;
        display = mix(display, push.vignette_color.rgb, edge);
    }
    display = clamp(display, 0.0, 1.0);
    // AgX outputs a 2.2-gamma display encoding. An sRGB target re-encodes on store, so
    // linearise first; a UNORM target (or the LDR intermediate) takes the encoded value.
    vec3 color = (push.flags & COMPOSITE_SRGB_TARGET) != 0u ? pow(display, vec3(2.2)) : display;
    float alpha = (push.flags & COMPOSITE_LUMA_ALPHA) != 0u ? post_luma(display) : 1.0;
    out_color = vec4(color, alpha);
}
