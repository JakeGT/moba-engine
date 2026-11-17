// LTC area lights (N2d, ADR-0023 §4): the integrals of the GGX lobe (as a linearly
// transformed cosine) and of the clamped cosine over a sphere, a capsule and a rect. Tiered
// only: included by lighting.glsl under MOBA_TIERED_SHADING. The Rust twin is
// crates/render/src/ltc/twin.rs, which the gate-3 tests hold against Monte Carlo; keep the two in
// step.
//
// The table is `crates/render/assets/ltc-ggx.bin`, baked by `moba-tools ltc-bake` from a port
// of the LTC reference code's fitLTC.cpp, and the evaluation follows the reference's WebGL
// demo ltc_quad.fs (the clipless polygon) and ltc_disk.fs, adapted. That code is:
//
//   Copyright (c) 2017, Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//   All rights reserved.
//
//   Redistribution and use in source and binary forms, with or without
//   modification, are permitted provided that the following conditions are met:
//
//   * If you use (or adapt) the source code in your own work, please include a
//     reference to the paper:
//
//     Real-Time Polygonal-Light Shading with Linearly Transformed Cosines.
//     Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//     ACM Transactions on Graphics (Proceedings of ACM SIGGRAPH 2016) 35(4), 2016.
//     Project page: https://eheitzresearch.wordpress.com/415-2/
//
//   * Redistributions of source code must retain the above copyright notice, this
//     list of conditions and the following disclaimer.
//
//   * Redistributions in binary form must reproduce the above copyright notice,
//     this list of conditions and the following disclaimer in the documentation
//     and/or other materials provided with the distribution.
//
//   THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//   AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//   IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//   DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
//   FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
//   DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//   SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
//   CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
//   OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//   OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//
// and asks that its papers be cited: Heitz, Dupuy, Hill and Neubelt, "Real-Time
// Polygonal-Light Shading with Linearly Transformed Cosines", ACM SIGGRAPH 2016; Heitz and
// Hill, "Linear-Light Shading with Linearly Transformed Cosines" and "Real-Time Line- and
// Disk-Light Shading with Linearly Transformed Cosines", SIGGRAPH 2017 course. The sphere's
// lobe follows the demo ltc_disk.fs (the ellipse's form factor and SolveCubic, which it
// credits to Christoph Peters, "How to solve a cubic equation, revisited") over the disk
// spanning the sphere's cone; a rect is its own quad and a capsule the quad facing the point
// with its projected area (ours; the demo's line form, ltc_line.fs, was measured against).
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
vec3 ltc_transform(vec4 m, vec3 p) {
    return vec3(m.x * p.x + m.z * p.z, p.y, m.y * p.x + m.w * p.z);
}
vec3 ltc_apply(vec4 m, vec3 p) {
    return normalize(ltc_transform(m, p));
}
float ltc_quad(vec4 m, vec3 q[4]) {
    vec3 l0 = ltc_apply(m, q[0]);
    vec3 l1 = ltc_apply(m, q[1]);
    vec3 l2 = ltc_apply(m, q[2]);
    vec3 l3 = ltc_apply(m, q[3]);
    vec3 form = ltc_edge(l0, l1) + ltc_edge(l1, l2) + ltc_edge(l2, l3) + ltc_edge(l3, l0);
    return ltc_sphere_clip(dot(form, l0 + l1 + l2 + l3) < 0.0 ? -form : form);
}

// The quad standing in for a capsule (ours, ADR-0023 §4a and amendment 4), facing the shaded
// point: at each end (`p1`, `p2`, relative to the point) `half_width` either side across the
// view and `reach` beyond the end along the axis. A capsule of radius R takes R and pi R / 4,
// so a distant view sees 2 R x its projected length + pi R², its own projected area at every
// angle. End-on (or with no axis), any direction across the view.
void ltc_facing_quad(vec3 p1, vec3 p2, float half_width, float reach, out vec3 q[4]) {
    vec3 axis = p2 - p1;
    float axis_length2 = dot(axis, axis);
    axis = axis_length2 > 1e-12 ? axis * inversesqrt(axis_length2) : vec3(0.0);
    vec3 view1 = p1 / max(length(p1), 1e-6);
    vec3 view2 = p2 / max(length(p2), 1e-6);
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

// The three real roots of c.w x^3 + c.z x^2 + c.y x + c.x (c.w = 1 here), the demo's order.
vec3 ltc_solve_cubic(vec4 c) {
    c.xyz /= c.w;
    c.yz /= 3.0;
    float b = c.z;
    float cc = c.y;
    float d = c.x;
    vec3 delta = vec3(-c.z * c.z + c.y, -c.y * c.z + c.x, dot(vec2(c.z, -c.y), c.xy));
    float discriminant = dot(vec2(4.0 * delta.x, -delta.y), delta.zy);
    vec2 xlc;
    vec2 xsc;
    {
        float c_a = delta.x;
        float d_a = -2.0 * b * delta.x + delta.y;
        float theta = atan(sqrt(max(discriminant, 0.0)), -d_a) / 3.0;
        float x_1a = 2.0 * sqrt(max(-c_a, 0.0)) * cos(theta);
        float x_3a = 2.0 * sqrt(max(-c_a, 0.0)) * cos(theta + (2.0 / 3.0) * MOBA_PI);
        float xl = (x_1a + x_3a) > 2.0 * b ? x_1a : x_3a;
        xlc = vec2(xl - b, c.w);
    }
    {
        float c_d = delta.z;
        float d_d = -d * delta.y + 2.0 * cc * delta.z;
        float theta = atan(d * sqrt(max(discriminant, 0.0)), -d_d) / 3.0;
        float x_1d = 2.0 * sqrt(max(-c_d, 0.0)) * cos(theta);
        float x_3d = 2.0 * sqrt(max(-c_d, 0.0)) * cos(theta + (2.0 / 3.0) * MOBA_PI);
        float xs = x_1d + x_3d < 2.0 * cc ? x_1d : x_3d;
        xsc = vec2(-d, xs + cc);
    }
    float e = xlc.y * xsc.y;
    float f = -xlc.x * xsc.y - xlc.y * xsc.x;
    float g = xlc.x * xsc.x;
    vec2 xmc = vec2(cc * f - b * g, -b * f + cc * e);
    vec3 root = vec3(xsc.x / xsc.y, xmc.x / xmc.y, xlc.x / xlc.y);
    if (root.x < root.y && root.x < root.z) {
        root.xyz = root.yxz;
    } else if (root.z < root.x && root.z < root.y) {
        root.xyz = root.xzy;
    }
    return root;
}

// A sphere's diffuse: its own vector form factor, R² / d² toward its centre (in the shading
// frame), horizon-clipped; inside it, the whole hemisphere.
float ltc_sphere_form(vec3 centre, float radius) {
    float distance2 = dot(centre, centre);
    float radius2 = radius * radius;
    return distance2 <= radius2 * 1.0001
        ? 1.0
        : ltc_sphere_clip(centre * (radius2 / (distance2 * sqrt(distance2))));
}

// A sphere's lobe (`centre` in the shading frame, `m` as for `ltc_quad`): the disk spanning
// its cone of directions (exactly its silhouette), facing the point, through `m`, then the
// ellipse's form factor (ltc_disk.fs).
float ltc_sphere(vec4 m, vec3 centre, float radius) {
    float distance2 = dot(centre, centre);
    float radius2 = radius * radius;
    if (distance2 <= radius2 * 1.0001) {
        return 1.0;
    }
    float disk = radius * sqrt(distance2 / (distance2 - radius2));
    vec3 axis = centre * inversesqrt(distance2);
    vec3 helper = abs(axis.x) < 0.9 ? vec3(1.0, 0.0, 0.0) : vec3(0.0, 1.0, 0.0);
    vec3 e1 = normalize(cross(axis, helper));
    vec3 e2 = cross(axis, e1);
    vec3 c = ltc_transform(m, centre);
    vec3 v1 = ltc_transform(m, e1 * disk);
    vec3 v2 = ltc_transform(m, e2 * disk);
    float d11 = dot(v1, v1);
    float d22 = dot(v2, v2);
    float d12 = dot(v1, v2);
    // The ellipse's axes: the eigenvectors of the symmetric [d11 d12; d12 d22], by one
    // rotation, stable where the disk is close to a circle (atan(0, 0) guarded; ADR-0023
    // amendment 4 item 3).
    float off = 2.0 * d12;
    float diff = d11 - d22;
    float theta = abs(off) + abs(diff) > 1e-7 * (d11 + d22) ? 0.5 * atan(off, diff) : 0.0;
    float cos_theta = cos(theta);
    float sin_theta = sin(theta);
    vec3 w1 = v1 * cos_theta + v2 * sin_theta;
    vec3 w2 = v2 * cos_theta - v1 * sin_theta;
    float a = 1.0 / dot(w1, w1);
    float b = 1.0 / dot(w2, w2);
    v1 = w1 * sqrt(a);
    v2 = w2 * sqrt(b);
    vec3 v3 = cross(v1, v2);
    if (dot(c, v3) < 0.0) {
        v3 = -v3;
    }
    float l = dot(v3, c);
    float x0 = dot(v1, c) / l;
    float y0 = dot(v2, c) / l;
    a *= l * l;
    b *= l * l;
    float c0 = a * b;
    float c1 = a * b * (1.0 + x0 * x0 + y0 * y0) - a - b;
    float c2 = 1.0 - a * (1.0 + x0 * x0) - b * (1.0 + y0 * y0);
    vec3 roots = ltc_solve_cubic(vec4(c0, c1, c2, 1.0));
    vec3 average = vec3(a * x0 / (a - roots.y), b * y0 / (b - roots.y), 1.0);
    average = normalize(mat3(v1, v2, v3) * average);
    float l1 = sqrt(max(-roots.y / roots.z, 0.0));
    float l2 = sqrt(max(-roots.y / roots.x, 0.0));
    float form_factor = l1 * l2 * inversesqrt((1.0 + l1 * l1) * (1.0 + l2 * l2));
    // A degenerate ellipse (the roots coincide) adds nothing rather than a NaN.
    return form_factor > 0.0 ? ltc_sphere_clip(average * form_factor) : 0.0;
}


#endif
