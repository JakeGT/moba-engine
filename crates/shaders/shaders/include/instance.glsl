// Per-frame instance storage buffer (set 1, binding 0): one model matrix, colour and VAT pose
// per draw (128 bytes). Layout must match `GpuInstance` in crates/render/src/scene.rs.
// `vat0` = (from frame, from next frame, from blend, crossfade weight), `vat1` = (to frame,
// to next frame, to blend, unused); only the VAT shaders read them. `params.x` is the
// outline palette slot (R2b).
#ifndef MOBA_INSTANCE_GLSL
#define MOBA_INSTANCE_GLSL

struct Instance {
    mat4 model;
    vec4 color;
    vec4 vat0;
    vec4 vat1;
    uvec4 params; // x outline palette slot (0 = none, ADR-0016), yzw reserved
};

layout(set = 1, binding = 0) readonly buffer InstanceBlock {
    Instance instances[];
} instance_data;

#endif
