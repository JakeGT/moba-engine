#version 450
#extension GL_GOOGLE_include_directive : require

// Screen-space contact shadows (High and up, ADR-0016). Reconstructs each pixel's world
// position from the depth pre-pass, marches a short ray towards the sun and reports whether
// the depth buffer has a surface in front of the ray within `thickness`: the small contact
// shadows (feet, wall bases) the fitted shadow map is too coarse for. The scene multiplies
// its shadow-map term by the result (1 = lit).

#include "camera.glsl"

layout(set = 1, binding = 0) uniform sampler2D scene_depth;

layout(location = 0) out float out_visibility;

vec3 world_at(vec2 uv, float depth) {
    // The scene renders with a flipped viewport: NDC +Y is the top of the image.
    vec4 world = camera.inv_view_proj * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    return world.xyz / world.w;
}

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    float depth = texelFetch(scene_depth, pixel, 0).r;
    out_visibility = 1.0;
    if (depth >= 1.0) {
        return; // Background.
    }
    vec2 uv = (vec2(pixel) + 0.5) * camera.viewport.zw;
    vec3 eye = camera.camera_pos.xyz;
    vec3 origin = world_at(uv, depth);
    float distance_to_eye = distance(eye, origin);
    float fade_end = camera.contact.w;
    if (distance_to_eye >= fade_end) {
        return;
    }
    vec3 light = normalize(camera.light_direction.xyz);
    int steps = clamp(int(camera.contact.x), 1, 32);
    float ray_length = camera.contact.y;
    float thickness = camera.contact.z;
    // Grows with distance: one depth texel covers more of the world farther away.
    float bias = 0.03 + distance_to_eye * 0.002;
    ivec2 last = ivec2(camera.viewport.xy) - 1;
    float occlusion = 0.0;
    for (int i = 1; i <= steps; i++) {
        float t = bias + ray_length * float(i) / float(steps);
        vec3 point = origin + light * t;
        vec4 clip = camera.view_proj * vec4(point, 1.0);
        if (clip.w <= 1e-4) {
            break;
        }
        vec3 ndc = clip.xyz / clip.w;
        vec2 point_uv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (any(lessThan(point_uv, vec2(0.0))) || any(greaterThan(point_uv, vec2(1.0)))) {
            break;
        }
        ivec2 tap = clamp(ivec2(point_uv * camera.viewport.xy), ivec2(0), last);
        float scene = texelFetch(scene_depth, tap, 0).r;
        if (scene >= ndc.z) {
            continue; // Nothing in front of the ray point.
        }
        float gap = distance(eye, point) - distance(eye, world_at(point_uv, scene));
        if (gap > bias && gap < thickness) {
            // Nearer occluders cast darker contact shadows.
            occlusion = 1.0 - 0.5 * float(i - 1) / float(steps);
            break;
        }
    }
    float fade = 1.0 - smoothstep(fade_end * 0.6, fade_end, distance_to_eye);
    out_visibility = 1.0 - occlusion * fade;
}
