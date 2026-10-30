// Heat haze (ADR-0019): the push block shared by `vfx_distort.vert` and `vfx_distort.frag`.
// Must match `DistortionPush` in `crates/render/src/passes/vfx.rs` (48 bytes).
#ifndef MOBA_DISTORTION_GLSL
#define MOBA_DISTORTION_GLSL

layout(push_constant) uniform DistortionPush {
    vec4 rect;   // x, y: last pixel of the render sub-rectangle; z: offset cap (pixels);
                 // w: 1 = mask units with the prepass ID target
    vec4 fade;   // x: soft distance (m); y, z: view-distance fade start, end (m); w: unused
    uvec4 list;  // x: first index of the distortion list in `alive`
} push;

#endif
