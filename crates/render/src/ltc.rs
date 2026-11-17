//! LTC area lights (N2d, ADR-0023 §4): the fitted GGX table, Tiered's LUT atlas and (in
//! tests) the Rust twin of `include/ltc.glsl`, which the gate-3 tests hold against a Monte
//! Carlo reference.
//!
//! The table is `assets/ltc-ggx.bin`, baked once by `moba-tools ltc-bake` (a port of the LTC
//! reference code's `fitLTC.cpp`, `THIRD_PARTY.md`): two `RGBA16F` 64 × 64 layers, layer 0 the
//! inverse matrix's four terms, layer 1 the lobe's magnitude, average Fresnel and the
//! horizon-clipped sphere table. Its sampling and the evaluations follow the reference's
//! WebGL demos `ltc_quad.fs` and `ltc_disk.fs` (Heitz, Dupuy, Hill and Neubelt 2016; Heitz
//! and Hill 2017), adapted (ADR-0023 §4a and amendment 4):
//!
//! - **`Rect`**: itself, one-sided (four edge vectors, then the sphere table for the horizon);
//! - **`Sphere`**: the diffuse its own vector form factor; the lobe over the disk spanning its
//!   cone (exactly its silhouette), the ellipse's form factor through a cubic solve;
//! - **`Capsule`**: the quad facing the point with the capsule's projected area at every angle
//!   (ours): the radius either side across the view, π R / 4 beyond each end along the axis.
//!   The demos' line (a second integral plus end caps) misses more on glossy lobes.
//!
//! A light spanning less than 1% of its distance is evaluated grown to that and its integrals
//! scaled back by the growth squared (`twin::grow`): below it `f32` loses the shape, while
//! its integral is already its area times the lobe's value.
//!
//! Every evaluation returns the integral of the (transformed) clamped cosine over the shape,
//! in 0..=1: with the identity it is the form factor (the diffuse), with the table's matrix
//! the specular lobe before its magnitude and Fresnel (`ltc_2.xy`).
//!
//! Adapted from the LTC reference code (`THIRD_PARTY.md`), whose notice is:
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

/// The table's side (`ltc-bake`'s `N`).
pub(crate) const LTC_SIZE: usize = 64;

/// The committed table: two layers of `LTC_SIZE`² `RGBA16F` texels.
pub(crate) const LTC_TABLE: &[u8] = include_bytes!("../assets/ltc-ggx.bin");

/// Bytes in one layer.
pub(crate) const LTC_LAYER_BYTES: usize = LTC_SIZE * LTC_SIZE * 8;

/// The atlas's size in texels (`ltc.glsl`'s `MOBA_LUT_ATLAS`), for the split-sum LUT's
/// `BRDF_LUT_SIZE` (32).
pub(crate) const LUT_ATLAS_EXTENT: [u32; 2] = [32 + 2 * LTC_SIZE as u32, LTC_SIZE as u32];

/// Tiered's binding-3 atlas (`ltc.glsl`'s `ltc_fetch`, `lighting.glsl`'s `brdf_lut_sample`):
/// `RGBA16F`, `lut_size + 128` × [`LTC_SIZE`] texels, the split-sum LUT (`rg`, `lut_size`²)
/// at the origin and the LTC table's two layers at x = `lut_size` and `lut_size + 64`. One
/// combined image sampler instead of a second one: the scene layout already uses the 16
/// samplers a stage may have on the minimum device and on `MoltenVK` (ADR-0023 amendment 4).
pub(crate) fn lut_atlas(split_sum: &[u8], lut_size: usize) -> Vec<u8> {
    let width = lut_size + 2 * LTC_SIZE;
    assert_eq!(
        split_sum.len(),
        lut_size * lut_size * 4,
        "R16G16 split-sum LUT"
    );
    let mut atlas = vec![0u8; width * LTC_SIZE * 8];
    for y in 0..LTC_SIZE {
        for x in 0..width {
            let at = (y * width + x) * 8;
            if x < lut_size {
                if y < lut_size {
                    let from = (y * lut_size + x) * 4;
                    atlas[at..at + 4].copy_from_slice(&split_sum[from..from + 4]);
                }
            } else {
                let layer = (x - lut_size) / LTC_SIZE;
                let from = layer * LTC_LAYER_BYTES + (y * LTC_SIZE + (x - lut_size) % LTC_SIZE) * 8;
                atlas[at..at + 8].copy_from_slice(&LTC_TABLE[from..from + 8]);
            }
        }
    }
    atlas
}

// The Rust twin of `ltc.glsl`'s evaluation, which the gate-3 tests hold against Monte Carlo
// (the GPU runs the shader).
#[cfg(test)]
mod twin;

// The Rust twin of one shaped light's term in `lighting.glsl`'s `dynamic_lights`.
#[cfg(test)]
mod shade;

#[cfg(test)]
mod tests;
