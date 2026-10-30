#version 450
#extension GL_GOOGLE_include_directive : require

// Ground-truth ambient occlusion (R6, ADR-0015), after Jimenez, Wu, Pesce and Jarabo,
// "Practical real-time strategies for accurate indirect occlusion" (2016), at half render
// resolution. Positions come from the prepass depth: the pixel's view ray at unit view depth
// (affine in the screen position, from three corner vectors) times the linear view depth from
// the near/far planes. An inverse view-projection applied per pixel at depth ~1, or to a
// near-plane point minus the eye, cancels catastrophically in fp32 and leaves millimetre
// noise that tilts reconstructed normals on grazing faces.
// Normals are reconstructed from depth (per axis, the neighbour with
// the smaller depth step, so silhouettes do not smear), and each pixel integrates `slices`
// screen-space slices with `steps` horizon probes per side. The slice angle and step jitter
// follow a fixed 4x4 pattern that the blur pass averages out (no temporal noise without TAA).
// Output: (visibility, distance to the eye) for the depth-aware blur, which clamps.

#include "post.glsl"

layout(set = 0, binding = 0) uniform sampler2D depth_texture;

layout(push_constant) uniform GtaoPush {
    vec4 ray_origin;      // xyz view ray at unit view depth for viewport uv (0, 0)
    vec4 ray_du;          // xyz change of that ray across the viewport's width
    vec4 ray_dv;          // xyz change across its height (uv y down)
    vec4 eye_radius;      // xyz eye position, w world radius
    vec4 planes;          // x near, y far (view depth), z focal length in render pixels
    vec4 render;          // xy 1 / render viewport, zw last valid render pixel
    vec4 params;          // x slices, y steps per side, z falloff start (fraction of the
                          // radius), w reserved
} push;

layout(location = 0) out vec2 out_ao;

const float PI = 3.14159265;
const float HALF_PI = 1.57079633;

// Linear view depth of an NDC depth (perspective, depth 0 at the near plane).
float view_depth_of(float depth) {
    float near = push.planes.x;
    float far = push.planes.y;
    return near * far / max(far - depth * (far - near), 1e-6);
}

// World position of the render pixel centred at `pixel_centre` with NDC depth `depth`.
vec3 world_at(vec2 pixel_centre, float depth) {
    vec2 uv = pixel_centre * push.render.xy;
    vec3 ray = push.ray_origin.xyz + uv.x * push.ray_du.xyz + uv.y * push.ray_dv.xyz;
    return push.eye_radius.xyz + ray * view_depth_of(depth);
}

float depth_at(ivec2 pixel) {
    ivec2 clamped = clamp(pixel, ivec2(0), ivec2(push.render.zw));
    return texelFetch(depth_texture, clamped, 0).r;
}

vec3 position_at(ivec2 pixel) {
    ivec2 clamped = clamp(pixel, ivec2(0), ivec2(push.render.zw));
    return world_at(vec2(clamped) + 0.5, texelFetch(depth_texture, clamped, 0).r);
}

void main() {
    ivec2 ao_pixel = ivec2(gl_FragCoord.xy);
    ivec2 pixel = ao_pixel * 2;
    float depth = depth_at(pixel);
    if (depth >= 1.0) {
        out_ao = vec2(1.0, 65000.0);
        return;
    }
    vec3 eye = push.eye_radius.xyz;
    vec3 position = world_at(vec2(pixel) + 0.5, depth);
    vec3 to_eye = eye - position;
    float distance_to_eye = length(to_eye);
    vec3 view = to_eye / max(distance_to_eye, 1e-4);

    // Reconstructed normal: on each axis take the neighbour on the same surface.
    float depth_l = depth_at(pixel + ivec2(-1, 0));
    float depth_r = depth_at(pixel + ivec2(1, 0));
    float depth_u = depth_at(pixel + ivec2(0, -1));
    float depth_d = depth_at(pixel + ivec2(0, 1));
    vec3 dx = abs(depth_r - depth) < abs(depth - depth_l)
        ? position_at(pixel + ivec2(1, 0)) - position
        : position - position_at(pixel + ivec2(-1, 0));
    vec3 dy = abs(depth_d - depth) < abs(depth - depth_u)
        ? position_at(pixel + ivec2(0, 1)) - position
        : position - position_at(pixel + ivec2(0, -1));
    vec3 normal = normalize(cross(dx, dy));
    if (dot(normal, view) < 0.0) {
        normal = -normal;
    }

    float radius = push.eye_radius.w;
    float view_depth = max(view_depth_of(depth), 1e-3);
    // Projected radius in render pixels, bounded so far pixels still probe a few texels and
    // near ones do not thrash the cache.
    float radius_px = clamp(radius * push.planes.z / view_depth, 2.0, 128.0);
    float falloff_start = radius * push.params.z;
    float falloff_range = max(radius - falloff_start, 1e-3);

    float pattern = post_bayer4(ao_pixel);
    float rotation = pattern / 16.0;
    float jitter = fract(pattern * 0.618034 + 0.5);
    int slices = int(push.params.x);
    int steps = int(push.params.y);
    float visibility = 0.0;
    int slices_done = 0;
    for (int slice = 0; slice < slices; slice++) {
        float phi = (float(slice) + rotation) * PI / float(slices);
        vec2 direction = vec2(cos(phi), sin(phi));
        // The slice's world direction: a point a few pixels along `direction` at the same
        // depth, made orthogonal to the view vector.
        vec3 along = world_at(vec2(pixel) + 0.5 + direction * 4.0, depth) - position;
        vec3 tangent = along - view * dot(along, view);
        float tangent_length = length(tangent);
        if (tangent_length < 1e-6) {
            continue;
        }
        tangent /= tangent_length;
        vec3 axis = cross(tangent, view);
        // The normal projected onto the slice plane, and its angle from the view vector.
        vec3 projected = normal - axis * dot(normal, axis);
        float projected_length = length(projected);
        if (projected_length < 1e-4) {
            continue;
        }
        float cos_n = clamp(dot(projected, view) / projected_length, -1.0, 1.0);
        float n = sign(dot(projected, tangent)) * acos(cos_n);

        float horizon_pos = -1.0;
        float horizon_neg = -1.0;
        for (int i = 0; i < steps; i++) {
            float t = (float(i) + jitter) / float(steps);
            // Quadratic spacing: more probes near the pixel, where occluders matter most.
            vec2 offset = direction * max(radius_px * t * t, 1.0 + float(i));
            for (int side = 0; side < 2; side++) {
                vec2 sample_pixel = vec2(pixel) + (side == 0 ? offset : -offset);
                vec3 delta = position_at(ivec2(sample_pixel)) - position;
                float length_delta = length(delta);
                float cos_h = dot(delta, view) / max(length_delta, 1e-4);
                // Fade occluders past the radius back to the open horizon.
                float fade = clamp((length_delta - falloff_start) / falloff_range, 0.0, 1.0);
                cos_h = mix(cos_h, -1.0, fade);
                if (side == 0) {
                    horizon_pos = max(horizon_pos, cos_h);
                } else {
                    horizon_neg = max(horizon_neg, cos_h);
                }
            }
        }
        float h1 = acos(clamp(horizon_pos, -1.0, 1.0));
        float h0 = -acos(clamp(horizon_neg, -1.0, 1.0));
        // Clamp the horizons to the hemisphere around the projected normal.
        h0 = n + max(h0 - n, -HALF_PI);
        h1 = n + min(h1 - n, HALF_PI);
        float sin_n = sin(n);
        float arc0 = -cos(2.0 * h0 - n) + cos_n + 2.0 * h0 * sin_n;
        float arc1 = -cos(2.0 * h1 - n) + cos_n + 2.0 * h1 * sin_n;
        // Weighted by the projected normal's length and averaged over the slices (not
        // normalised by the summed lengths): over all slice directions an open hemisphere
        // integrates to exactly 1 for any normal.
        visibility += projected_length * 0.25 * (arc0 + arc1);
        slices_done++;
    }
    // Not clamped to 1 here: with few slices one pixel's estimate scatters around the true
    // value (above 1 on open surfaces at some slice angles), and clamping before the blur
    // would bias open grazing faces dark. The blur clamps.
    float ao = slices_done > 0 ? clamp(visibility / float(slices_done), 0.0, 2.0) : 1.0;
    out_ao = vec2(ao, distance_to_eye);
}
