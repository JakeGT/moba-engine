#version 450
#extension GL_GOOGLE_include_directive : require

// Water shading (R3b, ADR-0017 §3).
//
// - Normals: value-noise ripples advected along the baked flow at two phases and
//   cross-faded, so the pattern moves downstream without stretching (a flow map without a
//   texture); one octave on Low, two above.
// - Depth fade: `1 - exp(-depth / clarity)` between the body's shallow and deep colours, from
//   the bed depth baked per vertex (exact: the mesh shares the terrain grid's vertices).
// - `shade()` for the sun's specular, IBL reflection, shadows; shoreline foam where the water
//   thins out, broken by noise and pushed by the flow; the fog of war, height fog and glow
//   decals like the terrain. Fragments over dry ground (negative depth) write nothing new:
//   transparent black (Simple) or the unchanged copied pixel (Full). No `discard`, which a
//   Vulkan 1.3 target compiles to a demote-to-helper the device need not enable.
// - Default (Simple): premultiplied alpha over the carved bed in the scene pass.
// - `WATER_REFRACTION` (Full): an opaque pass after the scene that samples a copy of the HDR
//   target offset by the normal (rejecting offsets onto geometry in front of the water) and
//   the scene depth, so units standing in the water get contact foam and the water column's
//   real thickness sets the fade.

#include "camera.glsl"
#include "lighting.glsl"
#include "atmosphere.glsl"
#include "decals.glsl"
#include "environment.glsl"

#ifdef WATER_REFRACTION
layout(set = 2, binding = 1) uniform sampler2D refraction;
layout(set = 2, binding = 2) uniform sampler2D scene_depth;
#endif

layout(location = 0) in vec3 in_world_pos;
layout(location = 1) in float in_depth;
layout(location = 2) in vec2 in_flow;
layout(location = 3) flat in uint in_body;

layout(location = 0) out vec4 out_color;

float water_hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float value_noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = water_hash(i);
    float b = water_hash(i + vec2(1.0, 0.0));
    float c = water_hash(i + vec2(0.0, 1.0));
    float d = water_hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float ripples(vec2 p, int octaves) {
    float h = value_noise(p * 0.9);
    if (octaves > 1) {
        h += 0.45 * value_noise(p * 2.3 + vec2(17.0, 5.0));
    }
    return h;
}

// Ripple slope at `p` (finite differences of the height).
vec2 ripple_slope(vec2 p, int octaves) {
    const float e = 0.08;
    float h = ripples(p, octaves);
    return vec2(ripples(p + vec2(e, 0.0), octaves) - h, ripples(p + vec2(0.0, e), octaves) - h) / e;
}

// Fog of war and height fog, applied to the water's own colours only: whatever is seen
// through it was fogged when it was drawn.
vec3 atmosphere(vec3 color) {
    color = apply_fog_of_war(color, in_world_pos);
    if (camera.haze_params.w > 0.5) {
        float distance = length(camera.camera_pos.xyz - in_world_pos);
        return mix(color, camera.ambient.rgb * 1.2, smoothstep(240.0, 520.0, distance));
    }
    return apply_height_fog(color, in_world_pos);
}

void main() {
    // Before the early out: derivatives need the whole quad.
    float aa = decal_pixel(in_world_pos);
    if (in_depth <= 0.0) {
#ifdef WATER_REFRACTION
        out_color = vec4(texelFetch(refraction, ivec2(gl_FragCoord.xy), 0).rgb, 1.0);
#else
        out_color = vec4(0.0);
#endif
        return;
    }
    uint body = min(in_body, uint(MOBA_WATER_BODIES - 1));
    vec4 deep = environment.water_deep[body];
    vec4 shallow = environment.water_shallow[body];
    int octaves = int(environment.wind.w);
    float time = environment.time.x;

    // Two-phase flow advection (period 2 s); still water drifts slowly.
    vec2 flow = in_flow + vec2(0.05, 0.03);
    float phase0 = fract(time * 0.5);
    float phase1 = fract(time * 0.5 + 0.5);
    float blend = abs(1.0 - 2.0 * phase0);
    vec2 base = in_world_pos.xz * 1.6;
    vec2 slope0 = ripple_slope(base - flow * (phase0 * 2.0) * 1.6, octaves);
    vec2 slope1 = ripple_slope(base - flow * (phase1 * 2.0) * 1.6 + vec2(0.37, 0.71), octaves);
    vec2 slope = mix(slope0, slope1, blend);
    float strength = 0.10 + 0.05 * min(length(in_flow), 2.0);
    vec3 normal = normalize(vec3(-slope.x * strength, 1.0, -slope.y * strength));

    float thickness = in_depth;
    vec3 behind = vec3(0.0);
#ifdef WATER_REFRACTION
    // Viewport-relative uv for the reconstruction; the targets are read by texel, since the
    // render region may be a corner of them (render scale, dynamic resolution).
    vec2 uv = gl_FragCoord.xy * environment.screen.xy;
    float here = texelFetch(scene_depth, ivec2(gl_FragCoord.xy), 0).r;
    vec4 scene = environment.inverse_view_proj * vec4(uv.x * 2.0 - 1.0, 1.0 - 2.0 * uv.y, here, 1.0);
    vec3 scene_pos = scene.xyz / scene.w;
    // The water column straight above whatever is behind this pixel: the bed, or the legs
    // of a unit wading through.
    thickness = clamp(in_world_pos.y - scene_pos.y, 0.0, in_depth + 0.5);
    vec2 offset = normal.xz * environment.screen.z * clamp(thickness * 2.0, 0.0, 1.0);
    vec2 region = 1.0 / environment.screen.xy;
    ivec2 refracted = ivec2(clamp(gl_FragCoord.xy + offset * region, vec2(0.5), region - 0.5));
    // Rejected where the offset lands on geometry in front of the water.
    if (texelFetch(scene_depth, refracted, 0).r < gl_FragCoord.z) {
        refracted = ivec2(gl_FragCoord.xy);
    }
    behind = texelFetch(refraction, refracted, 0).rgb;
#endif

    float opacity = 1.0 - exp(-thickness / max(deep.w, 0.02));
    vec3 body_color = mix(shallow.rgb, deep.rgb, opacity);
    Surface surface = default_surface(body_color, normal, in_world_pos, 0.06);
    surface.geometric_normal = vec3(0.0, 1.0, 0.0);
    surface.rim = 0.0;
    vec3 lit = atmosphere(shade(surface));

    // Shoreline and contact foam: where the column thins out, broken up and drifting.
    float foam_noise = value_noise(in_world_pos.xz * 2.2 - in_flow * time * 0.8);
    float edge = 1.0 - smoothstep(0.0, 0.03 + 0.04 * foam_noise, thickness);
    float foam = clamp(edge * shallow.w * (0.35 + 0.6 * foam_noise), 0.0, 1.0);
    Surface foam_surface = default_surface(vec3(0.82), vec3(0.0, 1.0, 0.0), in_world_pos, 1.0);
    foam_surface.rim = 0.0;
    vec3 foam_lit = atmosphere(shade(foam_surface));

    // The surface (water and foam) and how much of the pixel it covers. Glow decals other
    // than enemy telegraphs go on the surface only: what shows through (the bed, in the copy
    // or under the blend) already has them from the scene pass. Enemy telegraphs are
    // composited over everything the pixel shows (the full water) or made opaque in their
    // rim and halo (the simple water), so the bed's refracted copy of the ring never doubles
    // or weakens it (ADR-0017 §1).
    float rim_cover = 0.0;
#ifdef WATER_REFRACTION
    float cover = 1.0 - (1.0 - opacity) * (1.0 - foam);
    vec3 top = (lit * (opacity * (1.0 - foam)) + foam_lit * foam) / max(cover, 1e-4);
#else
    float cover = clamp(mix(0.18, 0.95, opacity) + foam, 0.0, 1.0);
    vec3 top = mix(lit, foam_lit, foam / max(cover, 1e-3));
#endif
    top = apply_glow_decals_role(top, in_world_pos, aa, false, rim_cover);
#ifdef WATER_REFRACTION
    // The bed seen through the water, tinted by the shallow colour as it deepens.
    vec3 transmitted = behind * mix(vec3(1.0), shallow.rgb * 2.0, opacity * 0.6);
    vec3 shown = apply_glow_decals_role(mix(transmitted, top, cover), in_world_pos, aa, true,
        rim_cover);
    out_color = vec4(shown, 1.0);
#else
    top = apply_glow_decals_role(top, in_world_pos, aa, true, rim_cover);
    cover = max(cover, rim_cover);
    // Premultiplied: ONE, ONE_MINUS_SRC_ALPHA.
    out_color = vec4(top * cover, cover);
#endif
}
