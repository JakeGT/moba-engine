#![allow(clippy::print_stdout, clippy::print_stderr)]
//! Fit the GGX linearly transformed cosine (LTC) tables of the Tiered area lights (N2d,
//! ADR-0023 §4b).
//!
//! ```text
//! ltc-bake <out.bin>        # crates/render/assets/ltc-ggx.bin
//! ```
//!
//! A port of `fit/fitLTC.cpp` (with `fit/LTC.h`, `fit/brdf_ggx.h` and `fit/nelder_mead.h`) from
//! the LTC reference code, <https://github.com/selfshadow/ltc_code> at commit
//! `31e5e96b54f98f33098f8503003119ba2231a1c6` (`THIRD_PARTY.md` has the notice and the files'
//! SHA-256). The fit, its first guesses, its iteration order and its `f32` arithmetic are the
//! upstream's; only the threading is new, and it does not change a value (below). Upstream's
//! notice, which its licence asks to be kept with the source:
//!
//! ```text
//! Copyright (c) 2017, Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//! All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are met:
//!
//! * If you use (or adapt) the source code in your own work, please include a
//!   reference to the paper:
//!
//!   Real-Time Polygonal-Light Shading with Linearly Transformed Cosines.
//!   Eric Heitz, Jonathan Dupuy, Stephen Hill and David Neubelt.
//!   ACM Transactions on Graphics (Proceedings of ACM SIGGRAPH 2016) 35(4), 2016.
//!   Project page: https://eheitzresearch.wordpress.com/415-2/
//!
//! * Redistributions of source code must retain the above copyright notice, this
//!   list of conditions and the following disclaimer.
//!
//! * Redistributions in binary form must reproduce the above copyright notice,
//!   this list of conditions and the following disclaimer in the documentation
//!   and/or other materials provided with the distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//! AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//! IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//! DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
//! FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
//! DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//! SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
//! CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
//! OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//! ```
//!
//! **Output.** Two layers of `N` × `N` (64 × 64) `RGBA16F` texels, little-endian, 64 KiB, the
//! upstream's `packTab` layout (texel `x + y·N`):
//! - layer 0, `ltc_1`: the inverse of the fitted matrix divided by its middle element, as
//!   `(m00, m02, m20, m22)` (glm's `m[column][row]`), at `x` = roughness `x / (N − 1)` and
//!   `y` = `sqrt(1 − cos θ)` = `y / (N − 1)`;
//! - layer 1, `ltc_2`: the lobe's magnitude (the BRDF's directional albedo) and its average
//!   Schlick term `(1 − V·H)⁵` in `x` and `y` (same axes), `z` unused (0), and in `w` the
//!   horizon-clipped sphere's projected solid angle over its unclipped one, at `x` =
//!   `(z + 1) / 2` (the average direction's elevation cosine) and `y` = the vector form
//!   factor's length (`genSphereTab`).
//!
//! **Threading.** Upstream fits roughness columns from rough to smooth and, within a column,
//! view angles from normal to grazing; each fit starts from the previous one. The first fit
//! of a column (normal incidence) starts from the next-rougher column's first fit only, so
//! the bake fits those 64 in order and then the columns' remaining fits in parallel: every
//! fit sees the same first guess as upstream's single thread, and the bytes do not depend on
//! the thread count. They may depend on the platform's `libm` (`acos`, `tan`, ...), so the
//! table is baked once and committed, never baked at build time, and nothing compares a
//! re-bake byte for byte.
//!
//! The report printed at the end is the gate-3 precision check of the `RGBA16F` storage: each
//! channel's largest absolute and relative change when rounded to half precision.

use std::path::PathBuf;
use std::process::ExitCode;

use moba_core::math::{Mat3, Vec3};

/// Table size (theta, alpha): upstream's `N`.
const N: usize = 64;
/// Samples per axis for the fitting error and the averages: upstream's `Nsample`.
const N_SAMPLE: usize = 32;
/// Smallest GGX alpha (avoids singularities): upstream's `MIN_ALPHA`.
const MIN_ALPHA: f32 = 0.00001;
/// Upstream's literal `3.14159f`, used where it writes it instead of `pi`: the table matches
/// upstream's only with its rounding.
#[allow(clippy::approx_constant)]
const PI_LITERAL: f32 = 3.14159;

/// The GGX BRDF times the cosine, with its sampling (`brdf_ggx.h`).
struct BrdfGgx;

impl BrdfGgx {
    // Upstream's arithmetic, kept so that the table is upstream's bit for bit.
    #[allow(clippy::manual_midpoint)]
    fn lambda(alpha: f32, cos_theta: f32) -> f32 {
        let a = 1.0 / alpha / cos_theta.acos().tan();
        if cos_theta < 1.0 {
            0.5 * (-1.0 + (1.0 + 1.0 / a / a).sqrt())
        } else {
            0.0
        }
    }

    /// The cosine-weighted BRDF and the PDF of sampling `l`.
    fn eval(v: Vec3, l: Vec3, alpha: f32) -> (f32, f32) {
        if v.z <= 0.0 {
            return (0.0, 0.0);
        }
        let lambda_v = Self::lambda(alpha, v.z);
        let g2 = if l.z <= 0.0 {
            0.0
        } else {
            let lambda_l = Self::lambda(alpha, l.z);
            1.0 / (1.0 + lambda_v + lambda_l)
        };
        let h = (v + l).normalize();
        let slope_x = h.x / h.z;
        let slope_y = h.y / h.z;
        let mut d = 1.0 / (1.0 + (slope_x * slope_x + slope_y * slope_y) / alpha / alpha);
        d *= d;
        d /= PI_LITERAL * alpha * alpha * h.z * h.z * h.z * h.z;
        let pdf = (d * h.z / 4.0 / v.dot(h)).abs();
        (d * g2 / 4.0 / v.z, pdf)
    }

    fn sample(v: Vec3, alpha: f32, u1: f32, u2: f32) -> Vec3 {
        let phi = 2.0 * PI_LITERAL * u1;
        let r = alpha * (u2 / (1.0 - u2)).sqrt();
        let n = Vec3::new(r * phi.cos(), r * phi.sin(), 1.0).normalize();
        -v + 2.0 * n * n.dot(v)
    }
}

/// One linearly transformed cosine (`LTC.h`).
#[derive(Clone, Copy)]
struct Ltc {
    magnitude: f32,
    fresnel: f32,
    m11: f32,
    m22: f32,
    m13: f32,
    x: Vec3,
    y: Vec3,
    z: Vec3,
    m: Mat3,
    inv_m: Mat3,
    det_m: f32,
}

impl Ltc {
    fn new() -> Self {
        let mut ltc = Self {
            magnitude: 1.0,
            fresnel: 1.0,
            m11: 1.0,
            m22: 1.0,
            m13: 0.0,
            x: Vec3::X,
            y: Vec3::Y,
            z: Vec3::Z,
            m: Mat3::IDENTITY,
            inv_m: Mat3::IDENTITY,
            det_m: 1.0,
        };
        ltc.update();
        ltc
    }

    /// The matrix from the parameters: `mat3(X, Y, Z) * mat3(m11,0,0, 0,m22,0, m13,0,1)` in
    /// glm's column order.
    fn update(&mut self) {
        let frame = Mat3::from_cols(self.x, self.y, self.z);
        let shape = Mat3::from_cols(
            Vec3::new(self.m11, 0.0, 0.0),
            Vec3::new(0.0, self.m22, 0.0),
            Vec3::new(self.m13, 0.0, 1.0),
        );
        self.m = frame * shape;
        self.inv_m = self.m.inverse();
        self.det_m = self.m.determinant().abs();
    }

    fn eval(&self, l: Vec3) -> f32 {
        let original = (self.inv_m * l).normalize();
        let transformed = self.m * original;
        let length = transformed.length();
        let jacobian = self.det_m / (length * length * length);
        let d = 1.0 / PI_LITERAL * original.z.max(0.0);
        self.magnitude * d / jacobian
    }

    fn sample(&self, u1: f32, u2: f32) -> Vec3 {
        let theta = u1.sqrt().acos();
        let phi = 2.0 * PI_LITERAL * u2;
        (self.m
            * Vec3::new(
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ))
        .normalize()
    }
}

/// The lobe's norm (albedo), average Schlick term and average direction (`computeAvgTerms`).
fn average_terms(v: Vec3, alpha: f32) -> (f32, f32, Vec3) {
    let (mut norm, mut fresnel, mut direction) = (0.0f32, 0.0f32, Vec3::ZERO);
    for j in 0..N_SAMPLE {
        for i in 0..N_SAMPLE {
            let u1 = (i as f32 + 0.5) / N_SAMPLE as f32;
            let u2 = (j as f32 + 0.5) / N_SAMPLE as f32;
            let l = BrdfGgx::sample(v, alpha, u1, u2);
            let (eval, pdf) = BrdfGgx::eval(v, l, alpha);
            if pdf > 0.0 {
                let weight = eval / pdf;
                let h = (v + l).normalize();
                norm += weight;
                fresnel += weight * (1.0 - v.dot(h).max(0.0)).powf(5.0);
                direction += weight * l;
            }
        }
    }
    let samples = (N_SAMPLE * N_SAMPLE) as f32;
    direction.y = 0.0;
    (norm / samples, fresnel / samples, direction.normalize())
}

/// The fitting error between the BRDF and the LTC, by multiple importance sampling
/// (`computeError`).
fn error(ltc: &Ltc, v: Vec3, alpha: f32) -> f32 {
    let mut error = 0.0f64;
    for j in 0..N_SAMPLE {
        for i in 0..N_SAMPLE {
            let u1 = (i as f32 + 0.5) / N_SAMPLE as f32;
            let u2 = (j as f32 + 0.5) / N_SAMPLE as f32;
            for l in [ltc.sample(u1, u2), BrdfGgx::sample(v, alpha, u1, u2)] {
                let (eval_brdf, pdf_brdf) = BrdfGgx::eval(v, l, alpha);
                let eval_ltc = ltc.eval(l);
                let pdf_ltc = eval_ltc / ltc.magnitude;
                let difference = f64::from((eval_brdf - eval_ltc).abs());
                let cubed = difference * difference * difference;
                error += cubed / f64::from(pdf_ltc + pdf_brdf);
            }
        }
    }
    error as f32 / (N_SAMPLE * N_SAMPLE) as f32
}

/// Sets the LTC's parameters from the simplex point (`FitLTC::update`).
fn apply(ltc: &mut Ltc, params: [f32; 3], isotropic: bool) {
    let m11 = params[0].max(1e-7);
    let m22 = params[1].max(1e-7);
    let m13 = params[2];
    if isotropic {
        ltc.m11 = m11;
        ltc.m22 = m11;
        ltc.m13 = 0.0;
    } else {
        ltc.m11 = m11;
        ltc.m22 = m22;
        ltc.m13 = m13;
    }
    ltc.update();
}

/// Downhill simplex (`nelder_mead.h`, `NelderMead<3>`): Nelder-Mead with the Numerical
/// Recipes termination test.
#[allow(clippy::many_single_char_names)] // The reference's names, kept for side-by-side reading.
fn nelder_mead(
    start: [f32; 3],
    delta: f32,
    tolerance: f32,
    max_iterations: usize,
    mut objective: impl FnMut([f32; 3]) -> f32,
) -> [f32; 3] {
    const DIM: usize = 3;
    const REFLECT: f32 = 1.0;
    const EXPAND: f32 = 2.0;
    const CONTRACT: f32 = 0.5;
    const SHRINK: f32 = 0.5;
    let mut s = [start; DIM + 1];
    for (i, point) in s.iter_mut().enumerate().skip(1) {
        point[i - 1] += delta;
    }
    let mut f = [0.0f32; DIM + 1];
    for (value, point) in f.iter_mut().zip(&s) {
        *value = objective(*point);
    }
    let mut lo = 0;
    for _ in 0..max_iterations {
        lo = 0;
        let (mut hi, mut nh) = (0, 0);
        for i in 1..=DIM {
            if f[i] < f[lo] {
                lo = i;
            }
            if f[i] > f[hi] {
                nh = hi;
                hi = i;
            } else if f[i] > f[nh] {
                nh = i;
            }
        }
        let (a, b) = (f[lo].abs(), f[hi].abs());
        if 2.0 * (a - b).abs() < (a + b) * tolerance {
            break;
        }
        let mut o = [0.0f32; DIM];
        for (i, point) in s.iter().enumerate() {
            if i == hi {
                continue;
            }
            for k in 0..DIM {
                o[k] += point[k];
            }
        }
        for value in &mut o {
            *value /= DIM as f32;
        }
        let towards = |scale: f32, worst: [f32; DIM]| -> [f32; DIM] {
            core::array::from_fn(|k| o[k] + scale * (o[k] - worst[k]))
        };
        let r = towards(REFLECT, s[hi]);
        let fr = objective(r);
        if fr < f[nh] {
            if fr < f[lo] {
                let e = towards(EXPAND, s[hi]);
                let fe = objective(e);
                if fe < fr {
                    s[hi] = e;
                    f[hi] = fe;
                    continue;
                }
            }
            s[hi] = r;
            f[hi] = fr;
            continue;
        }
        let c = towards(-CONTRACT, s[hi]);
        let fc = objective(c);
        if fc < f[hi] {
            s[hi] = c;
            f[hi] = fc;
            continue;
        }
        let best = s[lo];
        for k in 0..=DIM {
            if k == lo {
                continue;
            }
            for i in 0..DIM {
                s[k][i] = best[i] + SHRINK * (s[k][i] - best[i]);
            }
            f[k] = objective(s[k]);
        }
    }
    s[lo]
}

/// One fitted texel: the matrix (with the unused terms cleared), magnitude and Fresnel.
#[derive(Clone, Copy)]
struct Texel {
    m: Mat3,
    magnitude: f32,
    fresnel: f32,
}

/// The view direction and alpha of texel (`a`, `t`) (`fitTab`).
fn configuration(a: usize, t: usize) -> (Vec3, f32) {
    let x = t as f32 / (N - 1) as f32;
    let ct = 1.0 - x * x;
    let theta = ct.acos().min(1.57);
    let v = Vec3::new(theta.sin(), 0.0, theta.cos());
    let roughness = a as f32 / (N - 1) as f32;
    (v, (roughness * roughness).max(MIN_ALPHA))
}

/// Fits texel (`a`, `t`) from `ltc`'s current state, as one iteration of `fitTab`'s loop.
/// `rougher` is the fitted matrix of (`a + 1`, 0), needed when `t` is 0 and `a < N − 1`.
fn fit_texel(ltc: &mut Ltc, a: usize, t: usize, rougher: Option<Mat3>) -> Texel {
    let (v, alpha) = configuration(a, t);
    let (magnitude, fresnel, average) = average_terms(v, alpha);
    ltc.magnitude = magnitude;
    ltc.fresnel = fresnel;
    let isotropic = if t == 0 {
        ltc.x = Vec3::X;
        ltc.y = Vec3::Y;
        ltc.z = Vec3::Z;
        match rougher {
            None => {
                ltc.m11 = 1.0;
                ltc.m22 = 1.0;
            }
            Some(m) => {
                ltc.m11 = m.x_axis.x;
                ltc.m22 = m.y_axis.y;
            }
        }
        ltc.m13 = 0.0;
        ltc.update();
        true
    } else {
        let l = average;
        ltc.x = Vec3::new(l.z, 0.0, -l.x);
        ltc.y = Vec3::Y;
        ltc.z = l;
        ltc.update();
        false
    };
    // `fit`: Nelder-Mead from the first guess, epsilon 0.05, tolerance 1e-5, 100 iterations.
    let start = [ltc.m11, ltc.m22, ltc.m13];
    let mut trial = *ltc;
    let best = nelder_mead(start, 0.05, 1e-5, 100, |params| {
        apply(&mut trial, params, isotropic);
        error(&trial, v, alpha)
    });
    apply(ltc, best, isotropic);
    let mut m = ltc.m;
    // "kill useless coefs in matrix": glm's m[0][1], m[1][0], m[2][1], m[1][2].
    m.x_axis.y = 0.0;
    m.y_axis.x = 0.0;
    m.z_axis.y = 0.0;
    m.y_axis.z = 0.0;
    Texel {
        m,
        magnitude: ltc.magnitude,
        fresnel: ltc.fresnel,
    }
}

/// Every texel, indexed `a + t·N` (`fitTab`).
fn fit_table() -> Vec<Texel> {
    let empty = Texel {
        m: Mat3::IDENTITY,
        magnitude: 0.0,
        fresnel: 0.0,
    };
    let mut table = vec![empty; N * N];
    // Normal incidence, rough to smooth: each starts from the next-rougher one.
    let mut firsts = vec![Ltc::new(); N];
    for a in (0..N).rev() {
        let rougher = (a + 1 < N).then(|| table[a + 1].m);
        let mut ltc = if a + 1 < N { firsts[a + 1] } else { Ltc::new() };
        table[a] = fit_texel(&mut ltc, a, 0, rougher);
        firsts[a] = ltc;
    }
    // The rest of each column from its first fit, columns in parallel.
    let columns = std::sync::Mutex::new((0..N).rev());
    let results = std::sync::Mutex::new(Vec::with_capacity(N));
    let work = || {
        loop {
            let next = columns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next();
            let Some(a) = next else { break };
            let mut ltc = firsts[a];
            let column: Vec<Texel> = (1..N).map(|t| fit_texel(&mut ltc, a, t, None)).collect();
            results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((a, column));
            eprint!(".");
        }
    };
    let threads = std::thread::available_parallelism().map_or(1, core::num::NonZeroUsize::get);
    std::thread::scope(|scope| {
        for _ in 1..threads {
            if std::thread::Builder::new()
                .name(String::from("ltc-fit"))
                .spawn_scoped(scope, work)
                .is_err()
            {
                break;
            }
        }
        work();
    });
    eprintln!();
    for (a, column) in results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        for (t, texel) in column.into_iter().enumerate() {
            table[a + (t + 1) * N] = texel;
        }
    }
    table
}

fn sqr(x: f32) -> f32 {
    x * x
}

/// `G` of `ihemi` (fitLTC.cpp).
fn g_term(w: f32, s: f32, g: f32) -> f32 {
    -2.0 * w.sin() * s.cos() * g.cos() + core::f32::consts::FRAC_PI_2 - g + g.sin() * g.cos()
}

/// `H` of `ihemi` (fitLTC.cpp).
fn h_term(w: f32, s: f32, g: f32) -> f32 {
    let sin_s2 = sqr(s.sin());
    let cos_g2 = sqr(g.cos());
    w.cos() * (g.cos() * (sin_s2 - cos_g2).sqrt() + sin_s2 * (g.cos() / s.sin()).asin())
}

/// The projected solid angle of a spherical cap of half-angle `s` at elevation `w`, clipped
/// to the horizon (`ihemi`).
fn ihemi(w: f32, s: f32) -> f32 {
    let pi = core::f32::consts::PI;
    let g = (s.cos() / w.sin()).asin();
    let sin_s2 = sqr(s.sin());
    if (0.0..=(pi / 2.0 - s)).contains(&w) {
        return pi * w.cos() * sin_s2;
    }
    if w >= pi / 2.0 - s && w < pi / 2.0 {
        return pi * w.cos() * sin_s2 + g_term(w, s, g) - h_term(w, s, g);
    }
    if w >= pi / 2.0 && w < pi / 2.0 + s {
        return g_term(w, s, g) + h_term(w, s, g);
    }
    0.0
}

/// The horizon-clipped sphere table, indexed `i + j·N` (`genSphereTab`).
fn sphere_table() -> Vec<f32> {
    let pi = core::f32::consts::PI;
    let mut table = vec![0.0; N * N];
    for j in 0..N {
        for i in 0..N {
            let u1 = i as f32 / (N - 1) as f32;
            let u2 = j as f32 / (N - 1) as f32;
            let z = 2.0 * u1 - 1.0;
            let len = u2;
            let sigma = len.sqrt().asin();
            let omega = z.acos();
            let value = if sigma > 0.0 {
                ihemi(omega, sigma) / (pi * len)
            } else {
                z.max(0.0)
            };
            if value.is_nan() {
                eprintln!("nan in the sphere table at ({i}, {j})");
            }
            table[i + j * N] = value;
        }
    }
    table
}

/// The two texture layers (`packTab`), as `f32`.
fn pack(table: &[Texel], sphere: &[f32]) -> [Vec<[f32; 4]>; 2] {
    let mut one = vec![[0.0; 4]; N * N];
    let mut two = vec![[0.0; 4]; N * N];
    for i in 0..N * N {
        let mut inverse = table[i].m.inverse();
        inverse /= inverse.y_axis.y;
        one[i] = [
            inverse.x_axis.x,
            inverse.x_axis.z,
            inverse.z_axis.x,
            inverse.z_axis.z,
        ];
        two[i] = [table[i].magnitude, table[i].fresnel, 0.0, sphere[i]];
    }
    [one, two]
}

fn main() -> ExitCode {
    let Some(out) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: ltc-bake <out.bin>");
        return ExitCode::from(2);
    };
    let started = std::time::Instant::now();
    let table = fit_table();
    let sphere = sphere_table();
    let layers = pack(&table, &sphere);
    let mut bytes = Vec::with_capacity(2 * N * N * 8);
    // Gate 3's precision report: each channel's largest absolute change in half precision,
    // and its largest relative change over the texels at least 1 % of the channel's peak
    // (smaller values, such as the Fresnel average at normal incidence, round to tiny
    // absolute errors whose ratio means nothing).
    let mut absolute = [[0.0f32; 4]; 2];
    let mut relative = [[0.0f32; 4]; 2];
    let mut non_finite = 0usize;
    for (layer, texels) in layers.iter().enumerate() {
        let peak: [f32; 4] = core::array::from_fn(|channel| {
            texels
                .iter()
                .map(|texel| texel[channel].abs())
                .fold(0.0, f32::max)
        });
        for texel in texels {
            for (channel, &value) in texel.iter().enumerate() {
                let half = moba_assets::f16_from_f32(value);
                let back = moba_assets::f32_from_f16(half);
                if !value.is_finite() || !back.is_finite() {
                    non_finite += 1;
                } else {
                    let error = (back - value).abs();
                    absolute[layer][channel] = absolute[layer][channel].max(error);
                    if value.abs() >= 0.01 * peak[channel] {
                        relative[layer][channel] =
                            relative[layer][channel].max(error / value.abs());
                    }
                }
                bytes.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    if let Err(error) = std::fs::write(&out, &bytes) {
        eprintln!("ltc-bake: {}: {error}", out.display());
        return ExitCode::FAILURE;
    }
    println!(
        "ltc-bake: {} ({} bytes, 2 x {N} x {N} RGBA16F) in {:.1} s",
        out.display(),
        bytes.len(),
        started.elapsed().as_secs_f32()
    );
    println!(
        "ltc-bake: half precision, largest absolute change: ltc_1 {:?}, ltc_2 {:?}",
        absolute[0], absolute[1]
    );
    println!(
        "ltc-bake: half precision, largest relative change (values >= 1 % of the channel's \
         peak): ltc_1 {:?}, ltc_2 {:?}; non-finite {non_finite}",
        relative[0], relative[1]
    );
    ExitCode::SUCCESS
}
