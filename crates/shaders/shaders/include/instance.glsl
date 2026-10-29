// Per-frame instance storage buffer (set 1, binding 0): one model matrix and colour per draw.
// Layout must match `GpuInstance` in crates/render/src/scene.rs.
#ifndef MOBA_INSTANCE_GLSL
#define MOBA_INSTANCE_GLSL

struct Instance {
    mat4 model;
    vec4 color;
    uvec4 params; // x outline palette slot (0 = none, ADR-0016), yzw reserved
};

layout(set = 1, binding = 0) readonly buffer InstanceBlock {
    Instance instances[];
} instance_data;

#endif
