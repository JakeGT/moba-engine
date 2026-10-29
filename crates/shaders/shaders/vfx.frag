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
#endif

void main() {
    vec2 texel = texture(atlas, in_uv).rg;
    float alpha = clamp(in_color.a * texel.r, 0.0, 1.0);
    // Clamped so the OIT accumulation (weights up to 3e3) stays inside half-float range.
    vec3 rgb = min(in_color.rgb * (0.6 + 0.8 * texel.g), vec3(64.0));
    float soft = frame.eye.w;
    if (soft > 0.0 && (in_flags & VFX_FLAG_HARD) == 0u) {
        float d = texelFetch(scene_depth, ivec2(gl_FragCoord.xy), 0).r;
        float scene = frame.depth.y / (d + frame.depth.x);
        alpha *= clamp((scene - in_view_depth) / soft, 0.0, 1.0);
    }
    bool additive = (in_flags & VFX_FLAG_ALPHA) == 0u;
#ifdef VFX_OIT
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
