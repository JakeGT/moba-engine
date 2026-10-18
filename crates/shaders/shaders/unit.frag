#version 450

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction;
    vec4 ambient;
    vec4 terrain;
} camera;

layout(location = 0) in vec3 in_normal;
layout(location = 1) in vec3 in_color;
layout(location = 2) in vec3 in_world_pos;

layout(location = 0) out vec4 out_color;

void main() {
    vec3 normal = normalize(in_normal);
    vec3 light = normalize(camera.light_direction.xyz);
    float diffuse = max(dot(normal, light), 0.0) * camera.light_direction.w;

    // Hemispheric ambient: warmer from above, cooler from below. Cheap and stable.
    vec3 sky = camera.ambient.rgb;
    vec3 ground = camera.ambient.rgb * vec3(0.55, 0.6, 0.7);
    vec3 ambient = mix(ground, sky, normal.y * 0.5 + 0.5);

    // A touch of rim light separates silhouettes from the terrain.
    vec3 view_dir = normalize(camera.camera_pos.xyz - in_world_pos);
    float rim = pow(1.0 - max(dot(normal, view_dir), 0.0), 3.0) * 0.25;

    vec3 lit = in_color * (ambient + diffuse) + vec3(rim);
    out_color = vec4(lit, 1.0);
}
