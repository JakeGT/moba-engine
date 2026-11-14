#version 450
#extension GL_GOOGLE_include_directive : require

// `gbuffer_smoke` (ADR-0023 verification gate 1): every pixel encodes the surface its index
// names with `encode_gbuffer` (include/gbuffer.glsl) into the three G-buffer targets in their
// real formats; the smoke reads them back and compares every texel with the Rust twin byte
// for byte (crates/render/src/smoke/gbuffer.rs). Paired with `fullscreen.vert`.

#include "gbuffer.glsl"

// `SmokeSurface` in crates/render/src/smoke/gbuffer.rs (64 bytes).
struct SmokeSurface {
    vec4 base_metallic;      // rgb linear base colour, a metallic
    vec4 normal_roughness;   // xyz normal, w perceptual roughness
    vec4 model_data_ao;      // xyz model data (physical units), w material AO
    uvec4 model_flags_rim;   // x model, y flags, z rim slot, w unused
};

layout(std430, set = 0, binding = 0) readonly buffer SurfaceBlock {
    SmokeSurface surfaces[];
} smoke;

layout(push_constant) uniform SmokePush {
    uint width;
} push;

layout(location = 0) out vec4 out_gb0;
layout(location = 1) out vec4 out_gb1;
layout(location = 2) out vec4 out_gb2;

void main() {
    uvec2 pixel = uvec2(gl_FragCoord.xy);
    SmokeSurface surface = smoke.surfaces[pixel.y * push.width + pixel.x];
    SurfaceParams s;
    s.base_color = surface.base_metallic.rgb;
    s.metallic = surface.base_metallic.a;
    s.normal = surface.normal_roughness.xyz;
    s.roughness = surface.normal_roughness.w;
    s.model = surface.model_flags_rim.x;
    s.model_data = surface.model_data_ao.xyz;
    s.ao = surface.model_data_ao.w;
    s.flags = surface.model_flags_rim.y;
    s.rim_slot = surface.model_flags_rim.z;
    encode_gbuffer(s, out_gb0, out_gb1, out_gb2);
}
