#version 450
#extension GL_GOOGLE_include_directive : require

// Skinned unit rendering (WP-4.1). Same camera and instance data as unit.vert, plus the
// per-frame bone buffer: three vec4 rows (a row-major affine 3x4 matrix) per joint. The push
// constant selects this draw's bone range. The renderer guarantees joint_count >= 1 and
// first_bone + joint_count <= the bones uploaded this frame; joint indices are clamped to
// the range, so no read can leave it. The output matches unit.vert, so unit.frag is reused.
//
// Permutations (crates/shaders/build.rs): `MATERIAL_TEXTURED` also passes the mesh UVs and
// tangents; `SHADOW_PASS` is the depth-only shadow caster (position, joints and weights).

#include "camera.glsl"

// The depth prepass (R6, ADR-0015) draws with this shader and no fragment stage; the scene
// pass then tests LESS_OR_EQUAL against that depth, so both must compute the same position.
invariant gl_Position;
#include "instance.glsl"

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
#ifndef SHADOW_PASS
layout(location = 1) in vec3 in_normal;
#endif
layout(location = 2) in uvec4 in_joints;  // R8G8B8A8_UINT
layout(location = 3) in vec4 in_weights;  // R8G8B8A8_UNORM, sum 1 or all zero
#ifdef MATERIAL_TEXTURED
layout(location = 4) in vec2 in_uv;
layout(location = 5) in vec4 in_tangent;
#endif

#ifndef SHADOW_PASS
layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;
#endif
#ifdef MATERIAL_TEXTURED
layout(location = 3) out vec2 out_uv;
layout(location = 4) out vec4 out_world_tangent;
#endif

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
#ifdef SHADOW_PASS
    gl_Position = camera.light_view_proj * world;
#else
    gl_Position = camera.view_proj * world;

    out_world_normal = normalize(transpose(inverse(mat3(model))) * in_normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
#endif
#ifdef MATERIAL_TEXTURED
    out_uv = in_uv;
    float mirror = determinant(mat3(model)) < 0.0 ? -1.0 : 1.0;
    out_world_tangent = vec4(mat3(model) * in_tangent.xyz, in_tangent.w * mirror);
#endif
}
