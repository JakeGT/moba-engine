//! Block encoders for the formats the engine ships (ADR-0010): BC7 (mode 6 only), BC5 and BC4.
//!
//! - **BC7 mode 6** is one subset with 7-bit RGBA endpoints plus a shared p-bit each and
//!   4-bit indices: the "quick" class of BC7 encoders (`DirectXTex`'s `BC7_QUICK` uses the same
//!   mode). Endpoints start from the block's principal axis, get two least-squares
//!   refinements against the chosen indices, and every p-bit pair is tried. Other modes
//!   (partitions, separate alpha) would raise quality on high-contrast blocks; see the ADR.
//! - **BC4** tries the eight-value palette with endpoints inset from the block's range and
//!   the six-value palette (with exact 0 and 255), keeping the lower error. **BC5** is two BC4
//!   blocks (R, then G).
//!
//! Every encoder is deterministic: the same input always produces the same bytes.

/// BC7 4-bit interpolation weights (out of 64).
const WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn interpolate(e0: u32, e1: u32, weight: u32) -> u32 {
    ((64 - weight) * e0 + weight * e1 + 32) >> 6
}

/// Squared RGBA distance.
fn distance(a: [u32; 4], b: [u32; 4]) -> u32 {
    a.iter()
        .zip(&b)
        .map(|(&x, &y)| {
            let d = x.abs_diff(y);
            d * d
        })
        .sum()
}

/// Quantised mode-6 endpoints: 7 bits per channel and a p-bit each.
#[derive(Clone, Copy)]
struct Endpoints {
    q: [[u32; 4]; 2],
    p: [u32; 2],
}

impl Endpoints {
    fn expanded(&self, endpoint: usize) -> [u32; 4] {
        self.q[endpoint].map(|value| (value << 1) | self.p[endpoint])
    }

    fn palette(&self) -> [[u32; 4]; 16] {
        let (e0, e1) = (self.expanded(0), self.expanded(1));
        core::array::from_fn(|index| {
            core::array::from_fn(|channel| interpolate(e0[channel], e1[channel], WEIGHTS4[index]))
        })
    }

    /// Best palette index per pixel and the total error.
    fn assign(&self, pixels: &[[u32; 4]; 16]) -> ([u8; 16], u32) {
        let palette = self.palette();
        let mut indices = [0u8; 16];
        let mut total = 0;
        for (pixel, index) in pixels.iter().zip(&mut indices) {
            let (best, error) = palette
                .iter()
                .enumerate()
                .map(|(candidate, colour)| (candidate, distance(*pixel, *colour)))
                .min_by_key(|&(_, error)| error)
                .expect("16 palette entries");
            *index = best as u8;
            total += error;
        }
        (indices, total)
    }
}

/// Quantises float endpoints with the given p-bits.
fn quantize(e0: [f32; 4], e1: [f32; 4], p: [u32; 2]) -> Endpoints {
    let channel =
        |value: f32, p: u32| (((value - p as f32) / 2.0).round()).clamp(0.0, 127.0) as u32;
    Endpoints {
        q: [
            core::array::from_fn(|c| channel(e0[c], p[0])),
            core::array::from_fn(|c| channel(e1[c], p[1])),
        ],
        p,
    }
}

/// Principal-axis endpoints of `pixels` (float, 0..=255).
fn principal_endpoints(pixels: &[[u32; 4]; 16]) -> ([f32; 4], [f32; 4]) {
    let mut mean = [0f32; 4];
    for pixel in pixels {
        for c in 0..4 {
            mean[c] += pixel[c] as f32 / 16.0;
        }
    }
    let mut covariance = [[0f32; 4]; 4];
    for pixel in pixels {
        let d: [f32; 4] = core::array::from_fn(|c| pixel[c] as f32 - mean[c]);
        for row in 0..4 {
            for column in 0..4 {
                covariance[row][column] += d[row] * d[column];
            }
        }
    }
    // Power iteration from the bounding-box diagonal (deterministic).
    let mut axis: [f32; 4] = core::array::from_fn(|c| {
        let (low, high) = pixels.iter().fold((u32::MAX, 0), |(low, high), pixel| {
            (low.min(pixel[c]), high.max(pixel[c]))
        });
        high.saturating_sub(low) as f32 + 1e-3
    });
    for _ in 0..8 {
        let next: [f32; 4] = core::array::from_fn(|row| {
            (0..4)
                .map(|column| covariance[row][column] * axis[column])
                .sum()
        });
        let length = next.iter().map(|value| value * value).sum::<f32>().sqrt();
        if length < 1e-6 {
            break;
        }
        axis = next.map(|value| value / length);
    }
    let (mut low, mut high) = (f32::MAX, f32::MIN);
    for pixel in pixels {
        let t: f32 = (0..4).map(|c| (pixel[c] as f32 - mean[c]) * axis[c]).sum();
        low = low.min(t);
        high = high.max(t);
    }
    let at = |t: f32| core::array::from_fn(|c| (mean[c] + axis[c] * t).clamp(0.0, 255.0));
    (at(low), at(high))
}

/// Least-squares endpoints for fixed indices; `None` if the indices are all equal.
fn refine(pixels: &[[u32; 4]; 16], indices: &[u8; 16]) -> Option<([f32; 4], [f32; 4])> {
    let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
    let mut ax = [0f32; 4];
    let mut bx = [0f32; 4];
    for (pixel, &index) in pixels.iter().zip(indices) {
        let w = WEIGHTS4[usize::from(index)] as f32 / 64.0;
        let (a, b) = (1.0 - w, w);
        aa += a * a;
        ab += a * b;
        bb += b * b;
        for c in 0..4 {
            ax[c] += a * pixel[c] as f32;
            bx[c] += b * pixel[c] as f32;
        }
    }
    let determinant = aa * bb - ab * ab;
    if determinant.abs() < 1e-6 {
        return None;
    }
    let solve = |c: usize| {
        (
            ((bb * ax[c] - ab * bx[c]) / determinant).clamp(0.0, 255.0),
            ((aa * bx[c] - ab * ax[c]) / determinant).clamp(0.0, 255.0),
        )
    };
    let solved: [(f32, f32); 4] = core::array::from_fn(solve);
    Some((solved.map(|(e0, _)| e0), solved.map(|(_, e1)| e1)))
}

/// Best quantisation of float endpoints over the four p-bit pairs.
fn best_quantized(
    pixels: &[[u32; 4]; 16],
    e0: [f32; 4],
    e1: [f32; 4],
) -> (Endpoints, [u8; 16], u32) {
    let mut best: Option<(Endpoints, [u8; 16], u32)> = None;
    for p in [[0, 0], [0, 1], [1, 0], [1, 1]] {
        let endpoints = quantize(e0, e1, p);
        let (indices, error) = endpoints.assign(pixels);
        if best.as_ref().is_none_or(|(_, _, best)| error < *best) {
            best = Some((endpoints, indices, error));
        }
    }
    best.expect("four candidates")
}

/// Little-endian bit writer for one 128-bit block.
struct Bits {
    value: u128,
    position: u32,
}

impl Bits {
    fn push(&mut self, value: u32, bits: u32) {
        self.value |= u128::from(value & ((1 << bits) - 1)) << self.position;
        self.position += bits;
    }
}

/// Encodes 16 RGBA8 pixels (row-major) as one BC7 mode-6 block.
pub(crate) fn encode_bc7_block(rgba: &[[u8; 4]; 16]) -> [u8; 16] {
    let pixels: [[u32; 4]; 16] = rgba.map(|pixel| pixel.map(u32::from));
    let (e0, e1) = principal_endpoints(&pixels);
    let (mut endpoints, mut indices, mut error) = best_quantized(&pixels, e0, e1);
    for _ in 0..2 {
        let Some((r0, r1)) = refine(&pixels, &indices) else {
            break;
        };
        let candidate = best_quantized(&pixels, r0, r1);
        if candidate.2 >= error {
            break;
        }
        (endpoints, indices, error) = candidate;
    }
    // The anchor (pixel 0) index is stored with 3 bits: its MSB must be 0.
    if indices[0] >= 8 {
        endpoints.q.swap(0, 1);
        endpoints.p.swap(0, 1);
        for index in &mut indices {
            *index = 15 - *index;
        }
    }
    let mut bits = Bits {
        value: 0,
        position: 0,
    };
    bits.push(1 << 6, 7); // mode 6
    for channel in 0..4 {
        bits.push(endpoints.q[0][channel], 7);
        bits.push(endpoints.q[1][channel], 7);
    }
    bits.push(endpoints.p[0], 1);
    bits.push(endpoints.p[1], 1);
    bits.push(u32::from(indices[0]), 3);
    for &index in &indices[1..] {
        bits.push(u32::from(index), 4);
    }
    debug_assert_eq!(bits.position, 128);
    bits.value.to_le_bytes()
}

/// BC4 palette for endpoints `e0`, `e1` (eight-value mode when `e0 > e1`).
fn bc4_palette(e0: u32, e1: u32) -> [u32; 8] {
    if e0 > e1 {
        core::array::from_fn(|index| match index {
            0 => e0,
            1 => e1,
            i => ((8 - i as u32) * e0 + (i as u32 - 1) * e1 + 3) / 7,
        })
    } else {
        core::array::from_fn(|index| match index {
            0 => e0,
            1 => e1,
            6 => 0,
            7 => 255,
            i => ((6 - i as u32) * e0 + (i as u32 - 1) * e1 + 2) / 5,
        })
    }
}

fn bc4_assign(values: &[u8; 16], palette: &[u32; 8]) -> ([u8; 16], u32) {
    let mut indices = [0u8; 16];
    let mut total = 0;
    for (value, index) in values.iter().zip(&mut indices) {
        let (best, error) = palette
            .iter()
            .enumerate()
            .map(|(candidate, &entry)| {
                let d = entry.abs_diff(u32::from(*value));
                (candidate, d * d)
            })
            .min_by_key(|&(_, error)| error)
            .expect("8 palette entries");
        *index = best as u8;
        total += error;
    }
    (indices, total)
}

/// Encodes 16 single-channel values (row-major) as one BC4 block.
pub(crate) fn encode_bc4_block(values: &[u8; 16]) -> [u8; 8] {
    let low = u32::from(*values.iter().min().expect("16 values"));
    let high = u32::from(*values.iter().max().expect("16 values"));
    let mut best = (high, low, [0u8; 16], u32::MAX);
    if high == low {
        best = (high, low, [0; 16], 0);
    } else {
        for inset_high in 0..=2u32 {
            for inset_low in 0..=2u32 {
                let e0 = high.saturating_sub(inset_high);
                let e1 = (low + inset_low).min(255);
                if e0 <= e1 {
                    continue;
                }
                let (indices, error) = bc4_assign(values, &bc4_palette(e0, e1));
                if error < best.3 {
                    best = (e0, e1, indices, error);
                }
            }
        }
        // Six-value mode: exact 0/255 plus four steps between the non-extreme range.
        let inner: Vec<u32> = values
            .iter()
            .map(|&value| u32::from(value))
            .filter(|&value| value != 0 && value != 255)
            .collect();
        if let (Some(&inner_low), Some(&inner_high)) = (inner.iter().min(), inner.iter().max()) {
            let (indices, error) = bc4_assign(values, &bc4_palette(inner_low, inner_high));
            if error < best.3 {
                best = (inner_low, inner_high, indices, error);
            }
        }
    }
    let (e0, e1, indices, _) = best;
    let mut packed = 0u64;
    for (texel, &index) in indices.iter().enumerate() {
        packed |= u64::from(index) << (3 * texel);
    }
    let mut out = [0u8; 8];
    out[0] = e0 as u8;
    out[1] = e1 as u8;
    out[2..8].copy_from_slice(&packed.to_le_bytes()[..6]);
    out
}

/// Encodes a whole level of tightly packed texels with `channels` per texel (4 for BC7,
/// 2 for BC5, 1 for BC4), clamping edge blocks of non-multiple-of-four sizes.
pub(crate) fn encode_level(texels: &[u8], width: u32, height: u32, channels: usize) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let columns = width.div_ceil(4);
    let rows = height.div_ceil(4);
    let block_bytes = match channels {
        4 | 2 => 16,
        _ => 8,
    };
    let mut out = Vec::with_capacity(columns * rows * block_bytes);
    let texel = |x: usize, y: usize, c: usize| {
        texels[(y.min(height - 1) * width + x.min(width - 1)) * channels + c]
    };
    for by in 0..rows {
        for bx in 0..columns {
            let at = |i: usize, c: usize| texel(bx * 4 + i % 4, by * 4 + i / 4, c);
            match channels {
                4 => {
                    let block: [[u8; 4]; 16] =
                        core::array::from_fn(|i| core::array::from_fn(|c| at(i, c)));
                    out.extend_from_slice(&encode_bc7_block(&block));
                }
                2 => {
                    for c in 0..2 {
                        let block: [u8; 16] = core::array::from_fn(|i| at(i, c));
                        out.extend_from_slice(&encode_bc4_block(&block));
                    }
                }
                _ => {
                    let block: [u8; 16] = core::array::from_fn(|i| at(i, 0));
                    out.extend_from_slice(&encode_bc4_block(&block));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mse = a
            .iter()
            .zip(b)
            .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
            .sum::<f64>()
            / a.len() as f64;
        if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0 * 255.0 / mse).log10()
        }
    }

    fn decode_bc7(block: &[u8; 16]) -> [u8; 64] {
        let mut out = [0u8; 64];
        bcdec_rs::bc7(block, &mut out, 16);
        out
    }

    #[test]
    fn bc7_constant_and_gradient_blocks_round_trip() {
        let constant = [[200u8, 120, 40, 255]; 16];
        let decoded = decode_bc7(&encode_bc7_block(&constant));
        for pixel in decoded.as_chunks::<4>().0 {
            for (got, want) in pixel.iter().zip(&constant[0]) {
                assert!(got.abs_diff(*want) <= 1, "{pixel:?}");
            }
        }
        let gradient: [[u8; 4]; 16] = core::array::from_fn(|i| {
            let t = (i * 16) as u8;
            [t, 255 - t, t / 2, 255]
        });
        let source: Vec<u8> = gradient.iter().flatten().copied().collect();
        let decoded = decode_bc7(&encode_bc7_block(&gradient));
        assert!(
            psnr(&source, &decoded) > 40.0,
            "{}",
            psnr(&source, &decoded)
        );
    }

    #[test]
    fn bc7_anchor_index_fits_three_bits() {
        // A block whose first pixel is the bright end forces the endpoint swap.
        let block: [[u8; 4]; 16] = core::array::from_fn(|i| {
            let v = 255 - (i * 17) as u8;
            [v, v, v, 255]
        });
        // The anchor's implicit MSB is 0: a wrong swap would decode to garbage.
        let encoded = encode_bc7_block(&block);
        let source: Vec<u8> = block.iter().flatten().copied().collect();
        assert!(psnr(&source, &decode_bc7(&encoded)) > 40.0);
    }

    #[test]
    fn bc4_is_close_and_exact_for_flat_blocks() {
        let flat = [77u8; 16];
        let mut out = [0u8; 16];
        bcdec_rs::bc4(&encode_bc4_block(&flat), &mut out, 4, false);
        assert_eq!(out, flat);
        let ramp: [u8; 16] = core::array::from_fn(|i| (i * 13 + 20) as u8);
        bcdec_rs::bc4(&encode_bc4_block(&ramp), &mut out, 4, false);
        // A 195-wide ramp over eight levels: at most half a palette step (~14) off.
        assert!(
            out.iter().zip(&ramp).all(|(a, b)| a.abs_diff(*b) <= 14),
            "{out:?}"
        );
        // Extremes favour the six-value mode with exact 0 and 255.
        let extremes: [u8; 16] = core::array::from_fn(|i| match i % 3 {
            0 => 0,
            1 => 255,
            _ => 128,
        });
        bcdec_rs::bc4(&encode_bc4_block(&extremes), &mut out, 4, false);
        assert!(
            out.iter().zip(&extremes).all(|(a, b)| a.abs_diff(*b) <= 8),
            "{out:?}"
        );
    }

    #[test]
    fn levels_cover_partial_blocks_deterministically() {
        let texels: Vec<u8> = (0..6 * 5 * 2).map(|i| (i * 5) as u8).collect();
        let a = encode_level(&texels, 6, 5, 2);
        assert_eq!(a.len(), 2 * 2 * 16);
        assert_eq!(a, encode_level(&texels, 6, 5, 2));
        assert_eq!(encode_level(&[9, 9, 9, 9], 1, 1, 4).len(), 16);
        assert_eq!(encode_level(&[9], 1, 1, 1).len(), 8);
    }
}
