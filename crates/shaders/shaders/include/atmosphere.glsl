// Fog of war and height fog shared by the lit scene shaders (R3a, ADR-0014).
//
// The fog-of-war visibility texture (set 3, binding 5) is built each frame by the
// `fow-raster` / `fow-resolve` passes from the team's own vision only; offscreen views bind a
// white image and turn the mode off. The legacy mode keeps the pre-R3a loop over the camera
// uniform's 16 circles and 32 walls for comparison.
#ifndef MOBA_ATMOSPHERE_GLSL
#define MOBA_ATMOSPHERE_GLSL

#include "camera.glsl"

layout(set = 3, binding = 5) uniform sampler2D fog_visibility;

// The pre-R3a fog of war: vision circles with line-of-sight walls, from the camera uniform.
float legacy_visibility(vec3 world_pos) {
    float visibility = 0.0;
    for (int i = 0; i < MOBA_VISION_CIRCLES; i++) {
        if (i >= int(camera.vision_count.x)) {
            break;
        }
        vec4 circle = camera.vision_circles[i];
        float distance_to_vision = length(world_pos.xz - circle.xy);
        if (distance_to_vision > circle.z + 2.0) {
            continue;
        }
        bool blocked = false;
        for (int wall_index = 0; wall_index < MOBA_VISION_WALLS; wall_index++) {
            if (wall_index >= int(camera.wall_count.x)) {
                break;
            }
            vec4 wall = camera.vision_walls[wall_index];
            vec2 ray = world_pos.xz - circle.xy;
            vec2 segment = wall.zw - wall.xy;
            float denominator = ray.x * segment.y - ray.y * segment.x;
            if (abs(denominator) < 1e-5) {
                continue;
            }
            vec2 offset = wall.xy - circle.xy;
            float along_ray = (offset.x * segment.y - offset.y * segment.x) / denominator;
            float along_wall = (offset.x * ray.y - offset.y * ray.x) / denominator;
            if (along_ray > 0.0 && along_ray < 1.0 && along_wall >= 0.0 && along_wall <= 1.0) {
                blocked = true;
                break;
            }
        }
        if (!blocked) {
            visibility = max(visibility, 1.0 - smoothstep(circle.z - 2.0, circle.z + 2.0, distance_to_vision));
        }
    }
    return visibility;
}

// Fog of war over `color` at `world_pos`: darkened, desaturated and misted where the team has
// no vision. Explicit-LOD fetch, so it is valid in any control flow.
vec3 apply_fog_of_war(vec3 color, vec3 world_pos) {
    float mode = camera.fog_rect.w;
    if (mode < MOBA_FOW_LEGACY - 0.5) {
        return color;
    }
    if (mode < MOBA_FOW_TEXTURE - 0.5) {
        // Legacy: exactly the pre-R3a darkening.
        float unseen = 1.0 - legacy_visibility(world_pos);
        return mix(color, vec3(0.012, 0.020, 0.035), unseen * 0.72);
    }
    vec2 uv = (world_pos.xz - camera.fog_rect.xy) * camera.fog_rect.z;
    float unseen = 1.0 - textureLod(fog_visibility, uv, 0.0).r;
    if (unseen <= 0.002) {
        return color;
    }
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(color, vec3(luma), unseen * camera.fog_params.x);
    float amount = unseen * camera.fog_color.w;
    if (camera.fog_params.y > 0.0) {
        // Ground mist: thickest on the ground, thinning with height, so the tops of walls
        // and cliffs stay readable through it.
        float mist = camera.fog_params.z * exp(-max(world_pos.y, 0.0) / camera.fog_params.y);
        amount += (1.0 - amount) * unseen * mist;
    }
    return mix(color, camera.fog_color.rgb, clamp(amount, 0.0, 0.95));
}

// Analytic exponential height fog along the ray from the eye to `world_pos` (density at the
// ground, falling by e every 1 / haze_params.x metres), after a start distance and capped
// at the maximum opacity. Returns `color` unchanged when the view has no height fog.
vec3 apply_height_fog(vec3 color, vec3 world_pos) {
    float density = camera.haze_color.w;
    if (density <= 0.0) {
        return color;
    }
    vec3 ray = world_pos - camera.camera_pos.xyz;
    float dist = length(ray);
    float start = camera.haze_params.y;
    if (dist <= start) {
        return color;
    }
    vec3 dir = ray / dist;
    float falloff = camera.haze_params.x;
    float travel = dist - start;
    // Heights where the fogged segment starts and ends (the ground is y = 0).
    float y0 = max(camera.camera_pos.y + dir.y * start, 0.0);
    float y1 = max(world_pos.y, 0.0);
    // integral of exp(-falloff * y(t)) dt over the segment, in a form that never overflows:
    // both exponents are <= 0 because both heights are clamped to the ground.
    float k = falloff * dir.y;
    float integral = abs(dir.y) > 1e-3
        ? (exp(-falloff * y0) - exp(-falloff * y1)) / k
        : travel * exp(-falloff * y0);
    float amount = density * max(integral, 0.0);
    float opacity = min(1.0 - exp(-amount), camera.haze_params.z);
    return mix(color, camera.haze_color.rgb, opacity);
}

#endif
