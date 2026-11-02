#version 460
#extension GL_GOOGLE_include_directive : require

// bindless_smoke (ADR-0020 gate 3), graphics half: a fullscreen triangle whose every pixel
// samples a bindless texture and sampler chosen per pixel (divergent, `nonuniformEXT`), with
// implicit LOD. The textures are 1x1 solid colours, so every filter returns the texel exactly.

#include "bindless_smoke.glsl"

layout(location = 0) out vec4 out_color;

void main() {
    uvec2 pixel = uvec2(gl_FragCoord.xy);
    uint texture_handle = push.list.handles[smoke_slot(pixel)];
    uint sampler_handle = smoke_sampler(pixel);
    out_color = texture(BINDLESS_SAMPLER2D_NONUNIFORM(texture_handle, sampler_handle), vec2(0.5));
}
