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

#[cfg(test)]
mod tests;
