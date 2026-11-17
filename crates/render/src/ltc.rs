//! LTC area lights (N2d, ADR-0023 §4): the fitted GGX table and the Rust twin of
//! `include/ltc.glsl`, which the gate-3 tests hold against a Monte Carlo reference.
//!
//! The table is `assets/ltc-ggx.bin`, baked once by `moba-tools ltc-bake` (a port of the LTC
//! reference code's `fitLTC.cpp`, THIRD_PARTY.md) and uploaded as one two-layer `RGBA16F`
//! 64 × 64 array: layer 0 the inverse matrix's four terms, layer 1 the lobe's magnitude,
//! average Fresnel and the horizon-clipped sphere table. Its sampling, and the evaluation of
//! each shape, follow the reference's WebGL demos (`ltc_quad.fs`, `ltc_line.fs`,
//! `ltc_disk.fs`; Heitz, Dupuy, Hill and Neubelt 2016; Heitz and Hill 2017), adapted:
//!
//! - **`Rect`**: the clipless polygon (four edge vectors, then the sphere table for the
//!   horizon), one-sided;
//! - **`Sphere`**: the disk facing the shaded point with the sphere's own cone of directions
//!   (a disk across the cone is exactly the sphere's silhouette), then the ellipse's vector
//!   form factor and the sphere table, as `ltc_disk.fs` does;
//! - **`Capsule`**: the line integral of a thin cylinder (`ltc_line.fs`) plus two half-sphere
//!   ends, each a point evaluation of the LTC distribution over half the sphere's disk; the
//!   composition is ours (ADR-0023 §4a).
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
