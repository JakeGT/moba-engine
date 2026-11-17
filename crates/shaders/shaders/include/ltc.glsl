// LTC area lights (N2d, ADR-0023 §4): the integrals of the GGX lobe (as a linearly
// transformed cosine) and of the clamped cosine over a sphere, a capsule and a rect. Tiered
// only: included by lighting.glsl under MOBA_TIERED_SHADING. The Rust twin is
// crates/render/src/ltc.rs, which the gate-3 tests hold against Monte Carlo; keep the two in
// step.
//
// The table is `crates/render/assets/ltc-ggx.bin`, baked by `moba-tools ltc-bake` from a port
// of the LTC reference code's fitLTC.cpp, and the evaluation follows the reference's WebGL
// demo ltc_quad.fs (the clipless polygon), adapted. That code is:
//
//   Copyright (c) 2017, Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//   Redistribution and use in source and binary forms, with or without modification, are
//   permitted provided that the conditions in THIRD_PARTY.md ("LTC reference code") are met;
//   the full notice and disclaimer are there.
//
// and asks that its papers be cited: Heitz, Dupuy, Hill and Neubelt, "Real-Time
// Polygonal-Light Shading with Linearly Transformed Cosines", ACM SIGGRAPH 2016; Heitz and
// Hill, "Linear-Light Shading with Linearly Transformed Cosines" and "Real-Time Line- and
// Disk-Light Shading with Linearly Transformed Cosines", SIGGRAPH 2017 course. Every shape
// is one quad: a rect as itself, a sphere or a capsule as the quad facing the point that
// covers its projected area (ours; measured against the demos' disk and line forms, which
// cost a cubic solve or a second integral and ~40 more registers in the deferred lighting).
#ifndef MOBA_LTC_GLSL
#define MOBA_LTC_GLSL

// The table's side (`LTC_SIZE` in ltc.rs).
#define MOBA_LTC_SIZE 64.0

// Tiered's binding 3 is an atlas (`lut_atlas` in ltc.rs): the split-sum LUT in texels
// 0..32 x 0..32, then the LTC table's two layers at x 32 and x 96, 64 x 64 each. Every
// fetch lands on texel centres inside its own rectangle, so bilinear filtering never mixes
// them.
vec4 ltc_fetch(float layer, vec2 coord) {
    vec2 texel = clamp(coord, 0.0, 1.0) * (MOBA_LTC_SIZE - 1.0)
        + vec2(MOBA_BRDF_LUT_SIZE + 0.5 + MOBA_LTC_SIZE * layer, 0.5);
    return textureLod(brdf_lut, texel / MOBA_LUT_ATLAS, 0.0);
}

// The horizon-clipped sphere for a vector form factor `form`: its length is the unclipped
// form factor, its direction the average; the table's layer 1 `w` clips it.
float ltc_sphere_clip(vec3 form) {
    float len = length(form);
    if (len <= 0.0) {
        return 0.0;
    }
    return len * ltc_fetch(1.0, vec2(form.z / len * 0.5 + 0.5, len)).w;
}

// IntegrateEdgeVec: the edge's vector form factor with the reference's fit of theta / sin.
vec3 ltc_edge(vec3 v1, vec3 v2) {
    float x = dot(v1, v2);
    float y = abs(x);
    float a = 0.8543985 + (0.4965155 + 0.0145206 * y) * y;
    float b = 3.4175940 + (4.1616724 + y) * y;
    float v = a / b;
    float theta_sintheta = x > 0.0 ? v : 0.5 * inversesqrt(max(1.0 - x * x, 1e-7)) - v;
    return cross(v1, v2) * theta_sintheta;
}

// How much to grow a shape whose largest half-extent `extent` spans less than
// MOBA_LTC_MIN_SPAN of its `distance` (the caller divides the integrals by its square).
// Below that the quad's corners are too close for fp32 (the edges' cross products cancel to
// the normalisation's noise: 10% at a span of 1e-3), while the integral is already the
// shape's area times the lobe's value, so the grown shape's, over the growth squared, is the
// small one's to O(span²). It is what makes a vanishing light its point light (gate 3).
#define MOBA_LTC_MIN_SPAN 0.01
float ltc_grow(float distance, float extent) {
    return max(MOBA_LTC_MIN_SPAN * distance / max(extent, 1e-6), 1.0);
}

// The clipless quad: `m` is the table's inverse matrix (a, b, c, d), the 3 x 3
// ((a, 0, c), (0, 1, 0), (b, 0, d)) by rows, taking the corners (offsets from the shaded
// point, in the shading frame) into the lobe's space; (1, 0, 0, 1) is the clamped cosine's.
// Either winding (the form factor is turned toward the quad).
vec3 ltc_apply(vec4 m, vec3 p) {
    return normalize(vec3(m.x * p.x + m.z * p.z, p.y, m.y * p.x + m.w * p.z));
}
float ltc_quad(vec4 m, vec3 q[4]) {
    vec3 l0 = ltc_apply(m, q[0]);
    vec3 l1 = ltc_apply(m, q[1]);
    vec3 l2 = ltc_apply(m, q[2]);
    vec3 l3 = ltc_apply(m, q[3]);
    vec3 form = ltc_edge(l0, l1) + ltc_edge(l1, l2) + ltc_edge(l2, l3) + ltc_edge(l3, l0);
    return ltc_sphere_clip(dot(form, l0 + l1 + l2 + l3) < 0.0 ? -form : form);
}

// The quad standing in for a sphere or a capsule (ours, ADR-0023 §4a and amendment 4), facing
// the shaded point: at each end (`p1`, `p2`, relative to the point) `half_width` either side
// across the view and `reach` beyond the end along the axis. A capsule of radius R takes R
// and pi R / 4, so a distant view sees 2 R x its projected length + pi R², its own projected
// area at every angle; a sphere is the square of its silhouette disk's area, both ends at its
// centre. End-on (or with no axis), any direction across the view.
void ltc_facing_quad(vec3 p1, vec3 p2, float half_width, float reach, out vec3 q[4]) {
    vec3 axis = p2 - p1;
    float axis_length2 = dot(axis, axis);
    axis = axis_length2 > 1e-12 ? axis * inversesqrt(axis_length2) : vec3(0.0);
    vec3 view1 = normalize(p1);
    vec3 view2 = normalize(p2);
    vec3 across1 = cross(view1, axis);
    vec3 across2 = cross(view2, axis);
    vec3 fallback1 = abs(view1.x) < 0.9 ? vec3(1.0, 0.0, 0.0) : vec3(0.0, 1.0, 0.0);
    vec3 fallback2 = abs(view2.x) < 0.9 ? vec3(1.0, 0.0, 0.0) : vec3(0.0, 1.0, 0.0);
    across1 = normalize(dot(across1, across1) > 1e-8 ? across1 : cross(view1, fallback1));
    across2 = normalize(dot(across2, across2) > 1e-8 ? across2 : cross(view2, fallback2));
    vec3 c1 = p1 - cross(across1, view1) * reach;
    vec3 c2 = p2 + cross(across2, view2) * reach;
    q[0] = c1 - across1 * half_width;
    q[1] = c1 + across1 * half_width;
    q[2] = c2 + across2 * half_width;
    q[3] = c2 - across2 * half_width;
}

#endif
