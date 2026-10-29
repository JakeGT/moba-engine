//! Texture kinds, channel extraction and mip-chain filtering (ADR-0010 colour-space rules).
//!
//! - **Colour** is sRGB-encoded: mips are box-filtered in *linear* light (decode, average,
//!   re-encode), so dark/bright detail does not shift as it shrinks. Alpha is linear.
//! - **Normal** maps keep tangent-space X/Y in R/G: mips average the reconstructed unit
//!   vectors and renormalise them.
//! - **Data** (metallic/roughness, two channels) and **mask** (one channel) are linear and
//!   averaged as stored.

use moba_assets::TextureFormat;

/// What a texture holds; it fixes the colour space, channel count and allowed formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Color,
    Normal,
    Data,
    Mask,
}

impl Kind {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "color" => Some(Self::Color),
            "normal" => Some(Self::Normal),
            "data" => Some(Self::Data),
            "mask" => Some(Self::Mask),
            _ => None,
        }
    }

    pub(crate) const fn channels(self) -> usize {
        match self {
            Self::Color => 4,
            Self::Normal | Self::Data => 2,
            Self::Mask => 1,
        }
    }

    /// The format baked by default (block-compressed).
    pub(crate) const fn default_format(self) -> TextureFormat {
        match self {
            Self::Color => TextureFormat::Bc7Srgb,
            Self::Normal | Self::Data => TextureFormat::Bc5Unorm,
            Self::Mask => TextureFormat::Bc4Unorm,
        }
    }

    /// Formats this kind may be baked to (compressed or the 8-bit fallback).
    pub(crate) fn allows(self, format: TextureFormat) -> bool {
        format == self.default_format() || format == self.default_format().uncompressed()
    }
}

/// sRGB-encoded byte → linear light (exact piecewise curve).
pub(crate) fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light → sRGB-encoded byte.
pub(crate) fn linear_to_srgb(value: f32) -> u8 {
    let c = value.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// Keeps the channels `kind` stores from tightly packed RGBA8.
pub(crate) fn extract(rgba: &[u8], kind: Kind) -> Vec<u8> {
    let channels = kind.channels();
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| pixel[..channels].iter().copied())
        .collect()
}

/// The next mip level (half size, at least 1×1) of `texels`.
pub(crate) fn downsample(texels: &[u8], width: u32, height: u32, kind: Kind) -> Vec<u8> {
    let channels = kind.channels();
    let (source_width, source_height) = (width as usize, height as usize);
    let (target_width, target_height) = ((source_width / 2).max(1), (source_height / 2).max(1));
    let mut out = Vec::with_capacity(target_width * target_height * channels);
    let at = |column: usize, row: usize| {
        let start = (row * source_width + column) * channels;
        &texels[start..start + channels]
    };
    for row in 0..target_height {
        for column in 0..target_width {
            let left = (2 * column).min(source_width - 1);
            let right = (2 * column + 1).min(source_width - 1);
            let top = (2 * row).min(source_height - 1);
            let bottom = (2 * row + 1).min(source_height - 1);
            let taps = [
                at(left, top),
                at(right, top),
                at(left, bottom),
                at(right, bottom),
            ];
            match kind {
                Kind::Color => {
                    for channel in 0..3 {
                        let sum: f32 = taps.iter().map(|tap| srgb_to_linear(tap[channel])).sum();
                        out.push(linear_to_srgb(sum / 4.0));
                    }
                    let alpha: u32 = taps.iter().map(|tap| u32::from(tap[3])).sum();
                    out.push(((alpha + 2) / 4) as u8);
                }
                Kind::Normal => {
                    let mut sum = [0f32; 3];
                    for tap in taps {
                        let x = f32::from(tap[0]) / 255.0 * 2.0 - 1.0;
                        let y = f32::from(tap[1]) / 255.0 * 2.0 - 1.0;
                        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
                        sum[0] += x;
                        sum[1] += y;
                        sum[2] += z;
                    }
                    let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
                    let (nx, ny) = if length > 1e-6 {
                        (sum[0] / length, sum[1] / length)
                    } else {
                        (0.0, 0.0)
                    };
                    out.push(((nx * 0.5 + 0.5) * 255.0).round() as u8);
                    out.push(((ny * 0.5 + 0.5) * 255.0).round() as u8);
                }
                Kind::Data | Kind::Mask => {
                    for channel in 0..channels {
                        let sum: u32 = taps.iter().map(|tap| u32::from(tap[channel])).sum();
                        out.push(((sum + 2) / 4) as u8);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips_every_byte() {
        for value in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(value)), value);
        }
    }

    #[test]
    fn colour_mips_average_in_linear_light() {
        // Black and white average to linear 0.5, which is sRGB 188, not 128.
        let texels = [
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let mip = downsample(&texels, 2, 2, Kind::Color);
        assert_eq!(mip, [188, 188, 188, 255]);
        let data = downsample(&[0, 255, 0, 255], 2, 2, Kind::Mask);
        assert_eq!(data, [128]);
    }

    #[test]
    fn normal_mips_stay_unit_length() {
        // Two opposite tilts (x = +-0.6, z = 0.8) average back to straight up (128, 128).
        let texels = [204, 128, 51, 128, 204, 128, 51, 128];
        let mip = downsample(&texels, 2, 2, Kind::Normal);
        assert_eq!(mip, [128, 128]);
    }

    #[test]
    fn odd_sizes_shrink_to_one_texel() {
        let texels = vec![10u8; 5 * 3];
        let mip = downsample(&texels, 5, 3, Kind::Mask);
        assert_eq!(mip.len(), 2);
        assert_eq!(downsample(&[7], 1, 1, Kind::Mask), [7]);
    }
}
