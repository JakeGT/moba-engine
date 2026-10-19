#version 450

// Instanced unit rendering. One storage buffer holds every draw's model matrix and colour;
// the camera lives in a small per-frame uniform buffer.

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction; // xyz direction *towards* the light, w = intensity
    vec4 ambient;
    vec4 terrain;
} camera;

struct Instance {
    mat4 model;
    vec4 color;
};

layout(set = 1, binding = 0) readonly buffer InstanceBlock {
    Instance instances[];
} instance_data;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;

void main() {
    Instance instance = instance_data.instances[gl_InstanceIndex];
    vec4 world = instance.model * vec4(in_position, 1.0);
    gl_Position = camera.view_proj * world;

    // Rig parts and instanced units may be non-uniformly scaled.
    out_world_normal = normalize(mat3(transpose(inverse(instance.model))) * in_normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
}
