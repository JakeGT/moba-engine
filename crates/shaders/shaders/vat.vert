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
// Permutations (crates/shaders/build.rs): `SHADOW_PASS` is the depth-only shadow caster;
// `DEPTH_PREPASS` (R2b) writes the main view's depth and the instance's outline slot before
// shading; `OUTLINE_HULL` (R2b) draws the inverted-hull outline of outlined instances.
// `gl_Position` is invariant in every permutation, as in unit.vert, so the pre-pass depth
// matches the scene pass's exactly (ADR-0016).

#include "camera.glsl"
#include "instance.glsl"
#ifdef OUTLINE_HULL
#include "outline.glsl"
#endif

#if defined(SHADOW_PASS) || defined(DEPTH_PREPASS)
#define VAT_POSITION_ONLY
#endif

invariant gl_Position;

layout(set = 2, binding = 0) uniform sampler2D vat_texture;

layout(push_constant) uniform VatBlock {
    uint vertex_count;
    uint frame_count;
    uint width;
    uint reserved;
} vat;

layout(location = 0) in vec3 in_position;
#ifndef VAT_POSITION_ONLY
layout(location = 1) in vec3 in_normal;
#endif

#if defined(DEPTH_PREPASS)
layout(location = 0) flat out uint out_outline;
#elif defined(OUTLINE_HULL)
layout(location = 0) flat out vec4 out_outline_color;
#elif !defined(SHADOW_PASS)
layout(location = 0) out vec3 out_world_normal;
layout(location = 1) out vec3 out_color;
layout(location = 2) out vec3 out_world_pos;
#endif

vec4 fetch(uint texel) {
    return texelFetch(vat_texture, ivec2(int(texel % vat.width), int(texel / vat.width)), 0);
}

// Position (xyz) and, unless position-only, normal of `vertex` at frame `frame`.
void frame_data(uint frame, uint vertex, out vec3 position, out vec3 normal) {
    uint clamped = min(frame, vat.frame_count - 1u);
    uint texel = clamped * vat.vertex_count + vertex;
    position = fetch(texel).xyz;
#ifdef VAT_POSITION_ONLY
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
#ifndef VAT_POSITION_ONLY
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
#if defined(SHADOW_PASS)
    gl_Position = camera.light_view_proj * world;
#elif defined(DEPTH_PREPASS)
    gl_Position = camera.view_proj * world;
    out_outline = instance.params.x;
#elif defined(OUTLINE_HULL)
    // VAT meshes are smooth-shaded skins: extrude along the animated normal.
    gl_Position = outline_hull_position(world.xyz, mat3(transpose(inverse(instance.model))) * normal,
        instance.params.x);
    out_outline_color = outline_color(instance.params.x);
#else
    gl_Position = camera.view_proj * world;
    out_world_normal = normalize(mat3(transpose(inverse(instance.model))) * normal);
    out_color = instance.color.rgb;
    out_world_pos = world.xyz;
#endif
}
