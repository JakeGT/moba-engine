#version 450
#extension GL_GOOGLE_include_directive : require

// HUD fragment shader (R7a; offscreen images R7b). Modes match `UiVertex::MODE_*` in crates/render/src/ui.rs.
//
// The font atlas is an MTSDF: RGB is the multi-channel field (sharp corners), alpha the true
// distance (soft shadows, wide outlines). Edges are anti-aliased from screen-space
// derivatives, so text stays crisp at any UI scale. Every derivative is taken in uniform
// control flow, before the mode branch.
//
// MODE_IMAGE samples one of the renderer's offscreen HDR images (binding 2, indexed by a
// constant in each branch, so no descriptor-indexing feature is needed) and tonemaps it like
// the scene, so a portrait or the minimap matches the world it shows.

#include "tonemap.glsl"

// Must equal `moba_render::UI_IMAGE_SLOTS` (crates/render/src/ui.rs asserts its value).
#define UI_IMAGE_SLOTS 2

layout(set = 0, binding = 0) uniform sampler2D ui_atlas;
layout(set = 0, binding = 1) uniform sampler2D font_atlas;
layout(set = 0, binding = 2) uniform sampler2D ui_images[UI_IMAGE_SLOTS];

layout(push_constant) uniform UiPush {
    vec2 viewport;
    float font_range_px;
    // The composite's exposure and AgX look saturation, for MODE_IMAGE.
    float exposure;
    float saturation;
    // 1 when the target is *_SRGB (outputs are display-linear).
    uint srgb_target;
    uint reserved0;
    uint reserved1;
} push;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec4 in_color;
layout(location = 2) flat in uint in_mode;
layout(location = 3) flat in vec4 in_outline;
layout(location = 4) flat in vec2 in_params;
layout(location = 5) flat in uint in_image;

layout(location = 0) out vec4 out_color;

const uint MODE_ATLAS = 0u;
const uint MODE_SOLID = 1u;
const uint MODE_TEXT = 2u;
const uint MODE_TEXT_SOFT = 3u;
const uint MODE_SWEEP = 4u;
const uint MODE_IMAGE = 5u;

const float TAU = 6.28318530718;

float median3(vec3 v) {
    return max(min(v.r, v.g), min(max(v.r, v.g), v.b));
}

// `top` over `bottom`, both non-premultiplied.
vec4 over(vec4 top, vec4 bottom) {
    float alpha = top.a + bottom.a * (1.0 - top.a);
    vec3 rgb = (top.rgb * top.a + bottom.rgb * bottom.a * (1.0 - top.a)) / max(alpha, 1e-5);
    return vec4(rgb, alpha);
}

void main() {
    // Derivatives in uniform control flow.
    vec2 uv_width = fwidth(in_uv);
    vec2 centered = in_uv * 2.0 - 1.0;
    vec2 centered_width = fwidth(centered);

    if (in_mode == MODE_SOLID) {
        out_color = in_color;
        return;
    }
    if (in_mode == MODE_IMAGE) {
        // A 2x2 box over the pixel footprint: the images have no mips and are usually drawn
        // smaller than their size (a 512-pixel minimap in a 168-pixel panel).
        vec2 offset = uv_width * 0.25;
        vec3 hdr = vec3(0.0);
        for (int tap = 0; tap < 4; tap++) {
            vec2 uv = in_uv + offset * vec2((tap & 1) != 0 ? 1.0 : -1.0, (tap & 2) != 0 ? 1.0 : -1.0);
            if (in_image == 0u) {
                hdr += textureLod(ui_images[0], uv, 0.0).rgb;
            } else {
                hdr += textureLod(ui_images[1], uv, 0.0).rgb;
            }
        }
        vec3 display = agx_tonemap(hdr * 0.25 * push.exposure, push.saturation);
        vec3 color = push.srgb_target != 0u ? pow(display, vec3(2.2)) : display;
        out_color = vec4(color * in_color.rgb, in_color.a);
        return;
    }
    if (in_mode == MODE_ATLAS) {
        float alpha = textureLod(ui_atlas, in_uv, 0.0).a;
        out_color = vec4(in_color.rgb, in_color.a * alpha);
        return;
    }
    if (in_mode == MODE_SWEEP) {
        // params.x: covered fraction, clockwise from 12 o'clock and ending there (the part
        // still cooling down); params.y: inner radius (0 = the whole quad, > 0 = a ring).
        float fraction = clamp(in_params.x, 0.0, 1.0);
        float radius = length(centered);
        float pixel = max(length(centered_width), 1e-4);
        float angle = atan(centered.x, -centered.y);
        if (angle < 0.0) {
            angle += TAU;
        }
        float start = (1.0 - fraction) * TAU;
        float angular_pixel = pixel / max(radius, 1e-3);
        // Anti-aliased on both edges: the sweep ray and the seam at 12 o'clock, where the
        // angle wraps from TAU back to 0.
        float coverage = clamp((angle - start) / angular_pixel + 0.5, 0.0, 1.0)
            * clamp((TAU - angle) / angular_pixel + 0.5, 0.0, 1.0);
        if (fraction >= 0.999) {
            coverage = 1.0;
        }
        if (fraction <= 0.001) {
            coverage = 0.0;
        }
        if (in_params.y > 0.0) {
            float outer = clamp((1.0 - radius) / pixel + 0.5, 0.0, 1.0);
            float inner = clamp((radius - in_params.y) / pixel + 0.5, 0.0, 1.0);
            coverage *= outer * inner;
        }
        // A bright leading edge along the sweep ray (outline colour; alpha 0 disables it).
        float edge = 0.0;
        if (fraction > 0.001 && fraction < 0.999 && in_params.y <= 0.0) {
            float distance_px = abs(angle - start) / angular_pixel;
            edge = clamp(1.25 - distance_px, 0.0, 1.0) * in_outline.a;
        }
        vec4 fill = vec4(in_color.rgb, in_color.a * coverage);
        out_color = over(vec4(in_outline.rgb, edge), fill);
        return;
    }

    vec4 field = textureLod(font_atlas, in_uv, 0.0);
    vec2 unit_range = vec2(push.font_range_px) / vec2(textureSize(font_atlas, 0));
    vec2 screen_texels = vec2(1.0) / max(uv_width, vec2(1e-6));
    float screen_px_range = max(0.5 * dot(unit_range, screen_texels), 1.0);

    if (in_mode == MODE_TEXT_SOFT) {
        // Soft shadow/glow from the true distance. params.x: dilation, params.y: softness,
        // both in normalised distance units (0.5 = half the baked range).
        float distance = field.a - 0.5 + in_params.x;
        float softness = max(in_params.y, 1.0 / screen_px_range);
        // The last texels of a cell are clamped to the range edge: fade them out so a
        // glyph quad never shows as a box.
        float alpha = smoothstep(-softness, softness, distance) * smoothstep(0.0, 0.06, field.a);
        out_color = vec4(in_color.rgb, in_color.a * alpha);
        return;
    }

    // MODE_TEXT: crisp MSDF fill, plus an outline when params.x > 0 (normalised width).
    float distance = median3(field.rgb) - 0.5;
    float fill = clamp(distance * screen_px_range + 0.5, 0.0, 1.0);
    vec4 color = vec4(in_color.rgb, in_color.a * fill);
    if (in_params.x > 0.0 && in_outline.a > 0.0) {
        float outline_distance = max(distance, field.a - 0.5) + in_params.x;
        // As with soft shadows: the cell margin reads 0 (the range edge), where a small
        // on-screen size would otherwise give the outline partial coverage and draw a box.
        float outer = clamp(outline_distance * screen_px_range + 0.5, 0.0, 1.0)
            * smoothstep(0.0, 0.06, field.a);
        color = over(color, vec4(in_outline.rgb, in_outline.a * outer));
    }
    out_color = color;
}
