#version 450

// Composite: HDR scene colour -> exposure -> AgX tonemap -> swapchain (ADR-0008).
//
// AgX (Troy Sobotka) with the polynomial sigmoid fit published by Benjamin Wrensch
// ("Minimal AgX implementation", iolite engine, 2023): inset matrix, log2 encoding over
// [-12.47393, 4.026069] EV, a 6th-order polynomial for the default contrast curve, then the
// inverse (outset) matrix. It maps shadows and mid-tones almost unchanged and rolls highlights
// off without the hue skew of per-channel filmic curves. The base curve desaturates bright
// colours, so the AgX look stage is applied with identity slope/offset/power and CDL
// saturation (1.4, the "punchy" preset's saturation) to keep team colours as saturated as the
// pre-tonemap renderer. Grading looks and LUTs are R6 work, not this pass.

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

vec3 agx_contrast(vec3 x) {
    vec3 x2 = x * x;
    vec3 x4 = x2 * x2;
    return 15.5 * x4 * x2
         - 40.14 * x4 * x
         + 31.96 * x4
         - 6.868 * x2 * x
         + 0.4298 * x2
         + 0.1191 * x
         - 0.00232;
}

vec3 agx(vec3 color) {
    const mat3 inset = mat3(
        0.842479062253094, 0.0423282422610123, 0.0423756549057051,
        0.0784335999999992, 0.878468636469772, 0.0784336,
        0.0792237451477643, 0.0791661274605434, 0.879142973793104);
    const float min_ev = -12.47393;
    const float max_ev = 4.026069;
    color = inset * max(color, vec3(0.0));
    color = clamp(log2(max(color, vec3(1e-10))), min_ev, max_ev);
    color = (color - min_ev) / (max_ev - min_ev);
    return agx_contrast(color);
}

vec3 agx_look(vec3 color) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    return luma + push.saturation * (color - luma);
}

vec3 agx_outset(vec3 color) {
    const mat3 outset = mat3(
        1.19687900512017, -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433, 1.15107367264116);
    return clamp(outset * color, 0.0, 1.0);
}

void main() {
    // Same extent as the swapchain: fetch the texel under this fragment, no filtering.
    vec3 hdr = texelFetch(hdr_color, ivec2(gl_FragCoord.xy), 0).rgb * push.exposure;
    vec3 display = agx_outset(agx_look(agx(hdr)));
    // AgX outputs a 2.2-gamma display encoding. An sRGB target re-encodes on store, so
    // linearise first; a UNORM target takes the encoded value as is.
    vec3 color = push.srgb_target != 0u ? pow(display, vec3(2.2)) : display;
    out_color = vec4(color, 1.0);
}
