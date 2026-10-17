#version 450

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction;
    vec4 ambient;
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

    vec3 grass = vec3(0.10, 0.16, 0.11);
    vec3 line = vec3(0.20, 0.30, 0.22);
    vec3 base = mix(grass, line, grid);

    float diffuse = max(dot(normalize(in_normal), normalize(camera.light_direction.xyz)), 0.0)
        * camera.light_direction.w;
    vec3 lit = base * (camera.ambient.rgb + diffuse);

    // Distance fog hides the plane edge and keeps focus on the units.
    float distance = length(camera.camera_pos.xyz - in_world_pos);
    float fog = smoothstep(120.0, 260.0, distance);
    lit = mix(lit, camera.ambient.rgb * 1.2, fog);

    out_color = vec4(lit, 1.0);
}
