// Shared by bindless_smoke.frag and bindless_smoke.comp (ADR-0020 gate 3). The CPU mirror,
// which the readback is compared with exactly, is `smoke::bindless` in moba-render: change
// both together.
#extension GL_EXT_buffer_reference : require

#include "bindless.glsl"

// The texture handles the smoke samples, read through a buffer device address (§3a: buffers
// are addresses in push constants, not descriptors).
layout(buffer_reference, std430, buffer_reference_align = 4) readonly buffer HandleList {
    uint handles[];
};

layout(push_constant) uniform Push {
    HandleList list;
    // List length - 1; the length is a power of two.
    uint list_mask;
    // Palette slots 0..sampler_count-1 are the non-comparison samplers.
    uint sampler_count;
    // The compute pass's output (binding 1).
    uint storage_image;
    uint size;
} push;

// Neighbouring pixels pick unrelated list entries, so the handles diverge within every
// subgroup and quad.
uint smoke_slot(uvec2 pixel) {
    return ((pixel.x * 93u) ^ (pixel.y * 159u)) & push.list_mask;
}

uint smoke_sampler(uvec2 pixel) {
    return (pixel.x + 3u * pixel.y) % push.sampler_count;
}
