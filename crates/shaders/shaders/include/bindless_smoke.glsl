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
    // The 2x2 depth texture the comparison samplers read.
    uint depth_texture;
    // Comparison palette slots sampler_count..sampler_count+compare_count-1; 0 skips them.
    uint compare_count;
} push;

// Every pixel samples here. The textures are 2x2: u lies in the texel left of the image, so
// wrap and clamp differ; v lies a quarter texel below the centre line, so point and linear
// differ. The CPU mirror (`smoke::bindless::sample`) gives the exact result per sampler.
const vec2 SMOKE_UV = vec2(-0.125, 0.625);
// The depth comparison's reference (LESS_OR_EQUAL) and the alpha the result is written as.
const float SMOKE_REFERENCE = 0.5;
const float SMOKE_COMPARE_SCALE = 240.0 / 255.0;

// Neighbouring pixels pick unrelated list entries, so the handles diverge within every
// subgroup and quad.
uint smoke_slot(uvec2 pixel) {
    return ((pixel.x * 93u) ^ (pixel.y * 159u)) & push.list_mask;
}

uint smoke_sampler(uvec2 pixel) {
    return (pixel.x + 3u * pixel.y) % push.sampler_count;
}

uint smoke_compare_sampler(uvec2 pixel) {
    return push.sampler_count + (2u * pixel.x + pixel.y) % push.compare_count;
}
