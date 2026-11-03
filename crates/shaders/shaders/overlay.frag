#version 450
#extension GL_GOOGLE_include_directive : require

// The post-upscale gameplay overlay (ADR-0021 §5): at output resolution, after the composite
// (and any FXAA and upscale), over the displayed LDR image into the swapchain. It never enters
// any history and reads only the unjittered overlay prepass, so its coverage is the same
// whichever upscaler runs, at every render scale and with temporal AA off.
//
// - Glow decals (telegraphs, zones, range indicators) on `GROUND` pixels only, in display
//   space (`apply_display_glow_decals`), at the ground position the overlay depth gives.
// - Outlines: R2b's dilation of the overlay IDs (`outline.frag`), widths in output pixels.
// - Pixels neither touches keep the LDR value exactly.
//
// IDs: 0 sky, 1..7 outline slots, 8 `OCCLUDER` (walls, props, units without a slot), 9
// `GROUND` (terrain, water).

#define MOBA_DECAL_SET 0
#define MOBA_DECAL_BINDING 4
#define MOBA_DECALS_DISPLAY
#include "decals.glsl"

#define OVERLAY_ID_OCCLUDER 8u
#define OVERLAY_ID_GROUND 9u

layout(set = 0, binding = 0) uniform OverlayBlock {
    mat4 inv_view_proj;  // the unjittered output-resolution camera, clip -> world
    vec4 viewport;       // xy output size, zw 1 / size
    vec4 params;         // x 1 = the target is *_SRGB, y widest outline (px, at most 4),
                         // z debug view (0 none, 1 overlay IDs), w 1 = decals on
    vec4 palette[8];     // outline slots 1..7: rgb linear display colour, w width (px)
} overlay;

layout(set = 0, binding = 1) uniform sampler2D ldr_image;
layout(set = 0, binding = 2) uniform sampler2D overlay_depth;
layout(set = 0, binding = 3) uniform usampler2D overlay_ids;

layout(location = 0) out vec4 out_color;

const vec2 DIRECTIONS[8] = vec2[8](
    vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0),
    vec2(0.7071, 0.7071), vec2(-0.7071, 0.7071), vec2(0.7071, -0.7071), vec2(-0.7071, -0.7071));

vec3 debug_color(uint id) {
    if (id == 0u) {
        return vec3(0.0);
    }
    if (id == OVERLAY_ID_GROUND) {
        return vec3(0.15, 0.3, 0.15);
    }
    if (id == OVERLAY_ID_OCCLUDER) {
        return vec3(0.45);
    }
    return vec3(float(id & 1u), float((id >> 1u) & 1u), float((id >> 2u) & 1u));
}

// The world position under the pixel centre (the overlay prepass draws through a flipped
// viewport: NDC +y is the top).
vec3 ground_position(ivec2 pixel, float depth) {
    vec2 uv = (vec2(pixel) + 0.5) * overlay.viewport.zw;
    vec4 world = overlay.inv_view_proj * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    return world.xyz / world.w;
}

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    ivec2 last = textureSize(overlay_ids, 0) - 1;
    uint own = texelFetch(overlay_ids, pixel, 0).r;
    vec4 encoded = texelFetch(ldr_image, pixel, 0);
    bool srgb = overlay.params.x > 0.5;
    if (overlay.params.z > 0.5) {
        vec3 debug = debug_color(own);
        out_color = vec4(srgb ? debug : pow(debug, vec3(1.0 / 2.2)), 1.0);
        return;
    }
    // AgX's output is a 2.2-gamma display encoding (composite.frag).
    vec3 color = pow(encoded.rgb, vec3(2.2));
    bool touched = false;

    // Telegraphs on the ground. The world-space pixel size for the edges' antialiasing comes
    // from the reconstructed positions (uniform control flow), capped where neighbouring
    // pixels lie on other surfaces.
    float depth = texelFetch(overlay_depth, pixel, 0).r;
    vec3 world = ground_position(pixel, depth);
    float aa = clamp(length(fwidth(world.xz)), 0.01, 0.5);
    if (own == OVERLAY_ID_GROUND && overlay.params.w > 0.5) {
        color = apply_display_glow_decals(color, world, aa, touched);
    }

    // Outlines: the exterior of each slot's silhouette, the highest slot winning.
    uint base = own >= OVERLAY_ID_OCCLUDER ? 0u : own;
    int reach = min(int(ceil(overlay.params.y)), 4);
    uint best = 0u;
    float coverage = 0.0;
    for (int distance = 1; distance <= 4; distance++) {
        if (distance > reach) {
            break;
        }
        for (int direction = 0; direction < 8; direction++) {
            ivec2 tap = clamp(pixel + ivec2(round(DIRECTIONS[direction] * float(distance))),
                              ivec2(0), last);
            uint id = texelFetch(overlay_ids, tap, 0).r;
            if (id <= base || id >= OVERLAY_ID_OCCLUDER) {
                continue;
            }
            float cover = clamp(overlay.palette[id].w - float(distance) + 1.0, 0.0, 1.0);
            if (cover <= 0.0) {
                continue;
            }
            if (id > best) {
                best = id;
                coverage = cover;
            } else if (id == best) {
                coverage = max(coverage, cover);
            }
        }
    }
    if (best != 0u) {
        touched = true;
        color = mix(color, overlay.palette[best].rgb, coverage);
    }

    if (!touched) {
        // Untouched pixels keep exactly what the post chain displayed.
        out_color = vec4(srgb ? pow(encoded.rgb, vec3(2.2)) : encoded.rgb, 1.0);
        return;
    }
    color = clamp(color, 0.0, 1.0);
    out_color = vec4(srgb ? color : pow(color, vec3(1.0 / 2.2)), 1.0);
}
