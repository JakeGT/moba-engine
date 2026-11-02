#version 460
#extension GL_GOOGLE_include_directive : require

// bindless_smoke (ADR-0020 gate 3), graphics half: a fullscreen triangle whose every pixel
// samples a bindless texture and sampler chosen per pixel (divergent, `nonuniformEXT`), with
// implicit LOD, at a coordinate where every address mode and filter gives a different exact
// value. The comparison samplers are checked by the compute half only: lavapipe (Mesa 26.2.5)
// crashes when a fragment shader takes a second sample after a `nonuniformEXT` one.

#include "bindless_smoke.glsl"

layout(location = 0) out vec4 out_color;

void main() {
    uvec2 pixel = uvec2(gl_FragCoord.xy);
    uint texture_handle = push.list.handles[smoke_slot(pixel)];
    uint sampler_handle = smoke_sampler(pixel);
    out_color = texture(BINDLESS_SAMPLER2D_NONUNIFORM(texture_handle, sampler_handle), SMOKE_UV);
}
