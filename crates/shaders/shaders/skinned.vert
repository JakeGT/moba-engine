#version 450

// Skinned unit rendering (WP-4.1). Same camera and instance data as unit.vert, plus the
// per-frame bone buffer: three vec4 rows (a row-major affine 3x4 matrix) per joint. The push
// constant selects this draw's bone range. The renderer guarantees joint_count >= 1 and
// first_bone + joint_count <= the bones uploaded this frame; joint indices are clamped to
// the range, so no read can leave it. The output matches unit.vert, so unit.frag is reused.

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

layout(set = 1, binding = 1) readonly buffer BoneBlock {
    vec4 rows[];
} bones;

layout(push_constant) uniform SkinBlock {
    uint first_bone;
    uint joint_count;
    uint reserved0;
    uint reserved1;
} skin;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in uvec4 in_joints;  // R8G8B8A8_UINT
layout(location = 3) in vec4 in_weights;  // R8G8B8A8_UNORM, sum 1 or all zero

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;

mat4 bone(uint joint) {
    uint base = (skin.first_bone + min(joint, skin.joint_count - 1u)) * 3u;
    vec4 r0 = bones.rows[base];
    vec4 r1 = bones.rows[base + 1u];
    vec4 r2 = bones.rows[base + 2u];
    // Rows back to a column-major matrix; the implicit fourth row is (0, 0, 0, 1).
    return mat4(
        vec4(r0.x, r1.x, r2.x, 0.0),
        vec4(r0.y, r1.y, r2.y, 0.0),
        vec4(r0.z, r1.z, r2.z, 0.0),
        vec4(r0.w, r1.w, r2.w, 1.0));
}

void main() {
    Instance instance = instance_data.instances[gl_InstanceIndex];
    float total = in_weights.x + in_weights.y + in_weights.z + in_weights.w;
    mat4 skinning = mat4(1.0);
    if (total > 0.0) {
        skinning = (in_weights.x * bone(in_joints.x)
                  + in_weights.y * bone(in_joints.y)
                  + in_weights.z * bone(in_joints.z)
                  + in_weights.w * bone(in_joints.w)) * (1.0 / total);
    }
    mat4 model = instance.model * skinning;
    vec4 world = model * vec4(in_position, 1.0);
    gl_Position = camera.view_proj * world;

    out_world_normal = normalize(transpose(inverse(mat3(model))) * in_normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
}
