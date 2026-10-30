// Heat haze (ADR-0019): the push block shared by `vfx_distort.vert` and `vfx_distort.frag`.
// Must match `DistortionPush` in `crates/render/src/passes/vfx.rs` (48 bytes).
#ifndef MOBA_DISTORTION_GLSL
#define MOBA_DISTORTION_GLSL

layout(push_constant) uniform DistortionPush {
    vec4 rect;   // the region the copy holds this frame: first pixel (x, y), last pixel (z, w)
    vec4 fade;   // x: soft distance (m); y, z: view-distance fade start, end (m);
                 // w: offset cap (pixels)
    uvec4 list;  // x: first index of the distortion list in `alive`
} push;

#endif
