#version 450
#extension GL_GOOGLE_include_directive : require
#extension GL_EXT_samplerless_texture_functions : require

// `Paint` decals in the G-buffer (ADR-0023 §7a, N2c): each box's pixels reconstruct the
// ground from the receivers' depth (sampled, not attached: the pass tests nothing), evaluate
// the decal's shape with the shared `decals.glsl` and blend its colour into `gb0`'s sRGB view
// with (SRC_ALPHA, ONE_MINUS_SRC_ALPHA), so the blend is linear, as the forward terrain's
// `apply_paint_decals` mixes; gb0's alpha (metallic) is kept by the blend and the mask.
// Pixels a decal does not cover are discarded, so they keep their bytes exactly. Only the
// terrain is drawn before this pass; the units, walls, props, the VAT minions and the foliage
// are drawn after it and overwrite it (units never take decals, by order, §7a).

#include "camera.glsl"
#define MOBA_DECAL_SET 1
#define MOBA_DECAL_BINDING 1
#include "decals.glsl"
#define MOBA_REVERSED_Z
#include "depth.glsl"

layout(set = 1, binding = 0) uniform texture2D scene_depth;

layout(location = 0) flat in uint in_index;
layout(location = 0) out vec4 out_gb0;

void main() {
    float depth = texelFetch(scene_depth, ivec2(gl_FragCoord.xy), 0).r;
    vec2 uv = gl_FragCoord.xy * camera.viewport.zw;
    vec4 world_h = camera.inv_view_proj * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    vec3 world = world_h.xyz / world_h.w;
    // The world size of a pixel, in uniform control flow (derivatives).
    float aa = decal_pixel(world);
    uint index = in_index;
    // The box is drawn without culling or a depth test: of its two layers over a pixel, only
    // the back faces shade, so each covered pixel blends once.
    if (gl_FrontFacing || DEPTH_IS_FAR(depth)
        || (uint(MOBA_DECAL(index).params.y) & MOBA_DECAL_GLOW) != 0u) {
        discard;
    }
    vec4 coverage = decal_coverage(index, world, aa);
    float alpha = MOBA_DECAL(index).color.a * MOBA_DECAL(index).params.w * coverage.x;
    // A slightly lighter edge, so paint reads as soaked in rather than stamped.
    alpha = clamp(alpha * mix(1.0, 0.7, coverage.y), 0.0, 1.0);
    if (alpha <= 0.0) {
        discard;
    }
    out_gb0 = vec4(MOBA_DECAL(index).color.rgb, alpha);
}
