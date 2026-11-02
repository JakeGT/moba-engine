// The tiered path's global bindless set (ADR-0020 §3a), bound at set 0 of every tiered
// pipeline layout. `BindlessTable` (crates/render/src/bindless.rs) is the only code that
// creates or writes it. Binding 1 (storage images) is declared by each shader with the image
// type it writes, because GLSL needs the sampled type (`image2D`, `uimage2D`, ...).
//
// Index with `nonuniformEXT` wherever the handle can diverge within a subgroup. A combined
// `sampler2D(...)` needs it too: the decoration must reach the `OpSampledImage` the sample
// reads (VUID-RuntimeSpirv-NonUniform-06274), not just the index and the descriptor loads, so
// sample through `BINDLESS_SAMPLER2D_NONUNIFORM` rather than writing the constructor by hand.
#extension GL_EXT_nonuniform_qualifier : require

// Binding 0: the fixed sampler palette (32 slots; slot 0 is linear/wrap, slots 6-11 compare).
layout(set = 0, binding = 0) uniform sampler bindless_samplers[];
// Binding 2: every texture (variable count, up to 16,384; slot 0 is the magenta fallback).
layout(set = 0, binding = 2) uniform texture2D bindless_textures[];

// The texture `t` with the palette sampler `s`, both possibly divergent.
#define BINDLESS_SAMPLER2D_NONUNIFORM(t, s) \
    nonuniformEXT(sampler2D(bindless_textures[nonuniformEXT(t)], \
                            bindless_samplers[nonuniformEXT(s)]))

// The depth texture `t` with the comparison palette sampler `s` (slots 6-11), for
// `texture(..., vec3(uv, reference))`. `t` must be a depth texture: a comparison through a
// colour texture, including the fallbacks, is invalid.
#define BINDLESS_SAMPLER2DSHADOW_NONUNIFORM(t, s) \
    nonuniformEXT(sampler2DShadow(bindless_textures[nonuniformEXT(t)], \
                                  bindless_samplers[nonuniformEXT(s)]))

// Fallback texture handles, written once when the table is created. Every fallback is a 2D
// float colour image, so a zero or bad index is only valid through `texture2D` with a
// non-comparison sampler (and, on binding 1, a float `image2D`); typed fallbacks come with N1.
const uint BINDLESS_TEXTURE_MISSING = 0u;     // 1x1 magenta
const uint BINDLESS_TEXTURE_WHITE = 1u;       // 1x1 white
const uint BINDLESS_TEXTURE_FLAT_NORMAL = 2u; // 1x1 (0.5, 0.5, 1)
