#version 450
#extension GL_GOOGLE_include_directive : require

// GPU particle shading: atlas coverage (R) and hot core (G), soft fade against the scene
// depth, then either
//   default build:  one HDR output, premultiplied. Additive particles write alpha 0, so one
//                   (ONE, ONE_MINUS_SRC_ALPHA) blend serves both modes (unsorted alpha, Low);
//   VFX_OIT build:  HDR (additive particles) + weighted blended OIT accumulation and
//                   revealage (sum of -ln(1 - alpha)), all with (ONE, ONE) blending, so no
//                   independentBlend is needed; `vfx_resolve.frag` composites them.

#include "vfx.glsl"
#ifdef VFX_OVERLAY
#include "tonemap.glsl"
#endif

// Tiered builds (ADR-0021):
//   VFX_TEMPORAL: two more outputs after the colour ones: motion (`R16G16_SFLOAT`, blended
//                 SRC_ALPHA / ONE_MINUS_SRC_ALPHA with alpha `step(0.5, opacity)`, so a
//                 dominant fragment replaces the prepass's motion and any other keeps it,
//                 exactly) and the reactive mask (`R8_UNORM`, MAX blended: alpha particles
//                 their opacity, additive ones their luminance, both clamped to 0.9);
//   VFX_OVERLAY:  enemy telegraphs at output resolution over the displayed image: the
//                 composite's exposure and AgX in-shader (as the UI's offscreen images),
//                 premultiplied into the swapchain encoding.

layout(set = 1, binding = 0) uniform sampler2D atlas;
layout(set = 1, binding = 1) uniform sampler2D scene_depth;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec4 in_color;
layout(location = 2) in float in_view_depth;
layout(location = 3) flat in uint in_flags;

layout(location = 0) out vec4 out_hdr;
#ifdef VFX_OIT
layout(location = 1) out vec4 out_accum;
layout(location = 2) out vec4 out_reveal;
#define VFX_MOTION_LOCATION 3
#define VFX_REACTIVE_LOCATION 4
#else
#define VFX_MOTION_LOCATION 1
#define VFX_REACTIVE_LOCATION 2
#endif
#ifdef VFX_TEMPORAL
layout(location = 14) in vec4 in_motion_current;
layout(location = 15) in vec4 in_motion_previous;
layout(location = VFX_MOTION_LOCATION) out vec4 out_motion;
layout(location = VFX_REACTIVE_LOCATION) out vec4 out_reactive;
#endif

void main() {
    vec2 texel = texture(atlas, in_uv).rg;
    float alpha = clamp(in_color.a * texel.r, 0.0, 1.0);
    // Clamped so the OIT accumulation (weights up to 3e3) stays inside half-float range.
    vec3 rgb = min(in_color.rgb * (0.6 + 0.8 * texel.g), vec3(64.0));
#ifndef VFX_OVERLAY
    // (The overlay's telegraphs are drawn hard at output resolution: no soft fade, and the
    // render-resolution depth is not read.)
    float soft = frame.eye.w;
    if (soft > 0.0 && (in_flags & VFX_FLAG_HARD) == 0u) {
        float d = texelFetch(scene_depth, ivec2(gl_FragCoord.xy), 0).r;
        float scene = frame.depth.y / (d + frame.depth.x);
        alpha *= clamp((scene - in_view_depth) / soft, 0.0, 1.0);
    }
#endif
    bool additive = (in_flags & VFX_FLAG_ALPHA) == 0u;
#ifdef VFX_TEMPORAL
    vec2 ndc_current = in_motion_current.xy / in_motion_current.w;
    vec2 ndc_previous = in_motion_previous.xy / in_motion_previous.w;
    vec2 motion = (ndc_previous - ndc_current) * vec2(0.5, -0.5) * frame.temporal.xy;
    out_motion = vec4(motion, 0.0, step(0.5, alpha));
    float reactive = additive ? dot(rgb * alpha, vec3(0.2126, 0.7152, 0.0722)) : alpha;
    out_reactive = vec4(min(reactive, 0.9), 0.0, 0.0, 0.0);
#endif
#ifdef VFX_OVERLAY
    vec3 display = agx_tonemap(rgb * frame.overlay.x, frame.overlay.y);
    vec3 encoded = frame.overlay.z > 0.5 ? pow(display, vec3(2.2)) : display;
    out_hdr = additive ? vec4(encoded * alpha, 0.0) : vec4(encoded * alpha, alpha);
#elif defined(VFX_OIT)
    if (additive) {
        out_hdr = vec4(rgb * alpha, 0.0);
        out_accum = vec4(0.0);
        out_reveal = vec4(0.0);
    } else {
        // McGuire & Bavoil 2013, equation (10), view depth in metres.
        float z = in_view_depth;
        float weight = alpha * clamp(10.0 / (1e-5 + pow(z / 5.0, 2.0) + pow(z / 200.0, 6.0)),
                                     1e-2, 3e3);
        out_hdr = vec4(0.0);
        out_accum = min(vec4(rgb * alpha, alpha) * weight, vec4(60000.0));
        out_reveal = vec4(-log(1.0 - min(alpha, 0.995)), 0.0, 0.0, 0.0);
    }
#else
    out_hdr = additive ? vec4(rgb * alpha, 0.0) : vec4(rgb * alpha, alpha);
#endif
}
