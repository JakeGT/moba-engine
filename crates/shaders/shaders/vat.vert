#version 450
#extension GL_GOOGLE_include_directive : require

// Vertex-animation-texture units (R4b, ADR-0012 amendment): minions play clips baked into an
// RGBA16F texture instead of reading a bone buffer. Texel `frame * vertex_count + vertex`
// holds the skinned model-space position, texel `(frame_count + frame) * vertex_count +
// vertex` its normal; rows wrap at the texture width. The instance's `vat0`/`vat1` pick two
// frame samples and a crossfade between them (see `VatPose` in moba-assets). The output
// matches unit.vert, so unit.frag is reused.
//
// Every fetch stays inside the texture: frames are clamped below `frame_count`, the vertex
// below `vertex_count` (the renderer registers a VAT mesh only when its vertex count equals
// the descriptor's), and `2 * frame_count * vertex_count <= width * height`.
//
// Permutation (crates/shaders/build.rs): `SHADOW_PASS` is the depth-only shadow caster.

#include "camera.glsl"

// The depth prepass (R6, ADR-0015) draws with this shader and no fragment stage; the scene
// pass then tests LESS_OR_EQUAL against that depth, so both must compute the same position.
invariant gl_Position;
#include "instance.glsl"

layout(set = 2, binding = 0) uniform sampler2D vat_texture;

layout(push_constant) uniform VatBlock {
    uint vertex_count;
    uint frame_count;
    uint width;
    uint reserved;
} vat;

layout(location = 0) in vec3 in_position;
#ifndef SHADOW_PASS
layout(location = 1) in vec3 in_normal;

layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;
#endif

vec4 fetch(uint texel) {
    return texelFetch(vat_texture, ivec2(int(texel % vat.width), int(texel / vat.width)), 0);
}

// Position (xyz) and, unless SHADOW_PASS, normal of `vertex` at frame `frame`.
void frame_data(uint frame, uint vertex, out vec3 position, out vec3 normal) {
    uint clamped = min(frame, vat.frame_count - 1u);
    uint texel = clamped * vat.vertex_count + vertex;
    position = fetch(texel).xyz;
#ifdef SHADOW_PASS
    normal = vec3(0.0, 1.0, 0.0);
#else
    normal = fetch(vat.frame_count * vat.vertex_count + texel).xyz;
#endif
}

// One frame sample: `frame` blended `t` towards `next` (one fetch pair when `t` is 0).
void sample_clip(float frame, float next, float t, uint vertex, out vec3 position, out vec3 normal) {
    frame_data(uint(frame), vertex, position, normal);
    if (t > 0.0) {
        vec3 next_position;
        vec3 next_normal;
        frame_data(uint(next), vertex, next_position, next_normal);
        position = mix(position, next_position, t);
        normal = mix(normal, next_normal, t);
    }
}

void main() {
    Instance instance = instance_data.instances[gl_InstanceIndex];
    vec3 position = in_position;
#ifndef SHADOW_PASS
    vec3 normal = in_normal;
#else
    vec3 normal = vec3(0.0, 1.0, 0.0);
#endif
    if (vat.vertex_count > 0u && vat.frame_count > 0u) {
        uint vertex = min(uint(gl_VertexIndex), vat.vertex_count - 1u);
        sample_clip(instance.vat0.x, instance.vat0.y, instance.vat0.z, vertex, position, normal);
        float blend = instance.vat0.w;
        if (blend > 0.0) {
            vec3 to_position;
            vec3 to_normal;
            sample_clip(instance.vat1.x, instance.vat1.y, instance.vat1.z, vertex, to_position, to_normal);
            position = mix(position, to_position, blend);
            normal = mix(normal, to_normal, blend);
        }
    }
    vec4 world = instance.model * vec4(position, 1.0);
#ifdef SHADOW_PASS
    gl_Position = camera.light_view_proj * world;
#else
    gl_Position = camera.view_proj * world;
    out_world_normal = normalize(mat3(transpose(inverse(instance.model))) * normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
#endif
}
