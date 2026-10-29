// Helpers shared by the fullscreen post-processing passes (R6, ADR-0015).
//
// Dynamic resolution renders a sub-rectangle of every render-resolution image: a pass
// writes a viewport of `dst` pixels and reads a source whose valid region is `scale` of its
// allocation (uv 0..scale), with `texel` = 1 / allocation. Every tap is clamped half a texel
// inside that region, so bilinear filtering never reads stale texels outside it.
#ifndef MOBA_POST_GLSL
#define MOBA_POST_GLSL

// The source uv under this fragment: the destination's normalised position mapped onto
// the source's valid region.
vec2 post_source_uv(vec2 dst_inv, vec2 scale) {
    return gl_FragCoord.xy * dst_inv * scale;
}

// `uv` clamped half a texel inside the valid region [0, scale].
vec2 post_clamp(vec2 uv, vec2 scale, vec2 texel) {
    return clamp(uv, 0.5 * texel, scale - 0.5 * texel);
}

// Rec.709 luma.
float post_luma(vec3 color) {
    return dot(color, vec3(0.2126, 0.7152, 0.0722));
}

// 4x4 Bayer index in 0..15, a fixed interleaved pattern (no temporal noise without TAA).
float post_bayer4(ivec2 pixel) {
    const float bayer[16] = float[16](0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0,
                                      3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return bayer[(pixel.x & 3) + 4 * (pixel.y & 3)];
}

#endif
