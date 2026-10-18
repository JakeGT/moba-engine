#version 450

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction;
    vec4 ambient;
    vec4 terrain;
    vec4 vision_circles[16];
    uvec4 vision_count;
    vec4 vision_walls[32];
    uvec4 wall_count;
} camera;

layout(location = 0) in vec3 in_normal;
layout(location = 1) in vec3 in_world_pos;

layout(location = 0) out vec4 out_color;

float grid_line(vec2 position, float spacing, float thickness) {
    vec2 coord = position / spacing;
    vec2 derivative = fwidth(coord);
    vec2 grid = abs(fract(coord - 0.5) - 0.5) / max(derivative, vec2(1e-5));
    return 1.0 - min(min(grid.x, grid.y) / thickness, 1.0);
}

void main() {
    float grid = max(grid_line(in_world_pos.xz, 4.0, 1.0), grid_line(in_world_pos.xz, 16.0, 1.4) * 0.6);

    vec3 grass = camera.terrain.rgb;
    vec3 line = camera.terrain.rgb * 2.0;
    vec3 base = mix(grass, line, grid);

    float diffuse = max(dot(normalize(in_normal), normalize(camera.light_direction.xyz)), 0.0)
        * camera.light_direction.w;
    vec3 lit = base * (camera.ambient.rgb + diffuse);

    // Fog of war: darken terrain outside the allied vision circles. Server snapshots
    // already withhold unseen units; this makes the visibility boundary legible.
    float visibility = 0.0;
    for (int i = 0; i < 16; i++) {
        if (i >= int(camera.vision_count.x)) {
            break;
        }
        vec4 circle = camera.vision_circles[i];
        float distance_to_vision = length(in_world_pos.xz - circle.xy);
        if (distance_to_vision > circle.z + 2.0) {
            continue;
        }
        bool blocked = false;
        for (int wall_index = 0; wall_index < 32; wall_index++) {
            if (wall_index >= int(camera.wall_count.x)) {
                break;
            }
            vec4 wall = camera.vision_walls[wall_index];
            vec2 from = circle.xy;
            vec2 to = in_world_pos.xz;
            vec2 ray = to - from;
            vec2 segment = wall.zw - wall.xy;
            float denominator = ray.x * segment.y - ray.y * segment.x;
            if (abs(denominator) < 1e-5) {
                continue;
            }
            vec2 offset = wall.xy - from;
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
    if (camera.vision_count.x > 0u) {
        vec3 fog_color = vec3(0.012, 0.020, 0.035);
        lit = mix(lit, fog_color, (1.0 - visibility) * 0.72);
    }

    // Distance fog hides the plane edge and keeps focus on the units.
    float distance = length(camera.camera_pos.xyz - in_world_pos);
    float fog = smoothstep(240.0, 520.0, distance);
    lit = mix(lit, camera.ambient.rgb * 1.2, fog);

    out_color = vec4(lit, 1.0);
}
