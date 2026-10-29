#version 450
#extension GL_GOOGLE_include_directive : require

// Composite: HDR scene colour -> exposure -> AgX tonemap -> swapchain (ADR-0008). The tonemap
// lives in `tonemap.glsl`, shared with the UI's offscreen images (R7b).

#include "tonemap.glsl"

layout(set = 0, binding = 0) uniform sampler2D hdr_color;

layout(push_constant) uniform CompositePush {
    float exposure;
    // AgX look saturation around Rec.709 luma (1.0 = the base look).
    float saturation;
    // 1 when the swapchain is an *_SRGB format (hardware encodes on store, so the shader
    // outputs display-linear values); 0 for UNORM swapchains (output display-encoded).
    uint srgb_target;
    uint reserved;
} push;

layout(location = 0) out vec4 out_color;

void main() {
    // Same extent as the swapchain: fetch the texel under this fragment, no filtering.
    vec3 hdr = texelFetch(hdr_color, ivec2(gl_FragCoord.xy), 0).rgb * push.exposure;
    vec3 display = agx_tonemap(hdr, push.saturation);
    // AgX outputs a 2.2-gamma display encoding. An sRGB target re-encodes on store, so
    // linearise first; a UNORM target takes the encoded value as is.
    vec3 color = push.srgb_target != 0u ? pow(display, vec3(2.2)) : display;
    out_color = vec4(color, 1.0);
}
