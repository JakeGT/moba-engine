// AgX tonemap shared by the composite pass and the UI's offscreen images (R7b).
//
// AgX (Troy Sobotka) with the polynomial sigmoid fit published by Benjamin Wrensch
// ("Minimal AgX implementation", iolite engine, 2023): inset matrix, log2 encoding over
// [-12.47393, 4.026069] EV, a 6th-order polynomial for the default contrast curve, then the
// inverse (outset) matrix. It maps shadows and mid-tones almost unchanged and rolls highlights
// off without the hue skew of per-channel filmic curves. The base curve desaturates bright
// colours, so the AgX look stage is applied with identity slope/offset/power and a CDL
// saturation (the composite passes 1.4, the "punchy" preset's saturation). Grading looks and
// LUTs are R6 work.
#ifndef MOBA_TONEMAP_GLSL
#define MOBA_TONEMAP_GLSL

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

// AgX look saturation around Rec.709 luma (1.0 = the base look).
vec3 agx_look(vec3 color, float saturation) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    return luma + saturation * (color - luma);
}

vec3 agx_outset(vec3 color) {
    const mat3 outset = mat3(
        1.19687900512017, -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433, 1.15107367264116);
    return clamp(outset * color, 0.0, 1.0);
}

// Exposed HDR colour -> AgX display value (a 2.2-gamma encoding). For an *_SRGB target, which
// re-encodes on store, linearise with `pow(display, vec3(2.2))` first.
vec3 agx_tonemap(vec3 exposed, float saturation) {
    return agx_outset(agx_look(agx(exposed), saturation));
}

#endif
