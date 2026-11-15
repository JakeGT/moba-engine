//! PNG input and output, colour conversions and statistics shared by the A1 asset tools
//! (`content-check --source` and `review-sheet`; ADR-0022 amendment 2).
//!
//! Colour follows the art bible's definitions (`docs/art/material-vocabulary.md`, "How to read
//! this document"): **luma** is Rec. 709 relative luminance of linear RGB re-encoded to 8-bit
//! sRGB, and **L / C / h** are `OKLab` lightness, `OKLCH` chroma and hue in degrees.

use std::path::Path;

/// Largest PNG file read, in bytes. Source bakes are authoring files, still bounded.
pub(crate) const MAX_PNG_BYTES: u64 = 256 * 1024 * 1024;
/// Largest PNG side accepted, in pixels (4K renders and 4096² bakes fit).
pub(crate) const MAX_PNG_SIDE: u32 = 8192;

/// A decoded image, kept at the depth and channel count the file stored (a 4096² RGB8 bake is
/// 48 MiB, not the 128 MiB of an RGBA16 copy). Reads present it as RGBA: grey is replicated to
/// R, G and B, and a missing alpha reads 1.
#[derive(Clone, Debug)]
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Channels the file stored (1 grey, 2 grey + alpha, 3 RGB, 4 RGBA).
    pub(crate) channels: u8,
    /// Whether the file stored 16 bits per channel (else 8).
    pub(crate) sixteen_bit: bool,
    /// Samples, row-major from the top, `channels` per texel; 16-bit samples big-endian.
    samples: Vec<u8>,
}

impl Image {
    /// Channel `channel` (0 R, 1 G, 2 B, 3 A) of texel `(x, y)` in 0..=1.
    pub(crate) fn get(&self, x: u32, y: u32, channel: usize) -> f32 {
        self.at((y * self.width + x) as usize, channel)
    }

    /// Channel `channel` (0 R, 1 G, 2 B, 3 A) of the texel at linear index `index`, in 0..=1.
    /// Grey + alpha files keep their alpha in channel 3.
    pub(crate) fn at(&self, index: usize, channel: usize) -> f32 {
        let stored = usize::from(self.channels);
        let source = match (stored, channel) {
            (1 | 2, 0..=2) => 0,
            (1 | 3, _) if channel >= 3 => return 1.0,
            (2, _) => 1,
            (_, channel) => channel,
        };
        let sample = index * stored + source;
        if self.sixteen_bit {
            f32::from(u16::from_be_bytes([
                self.samples[2 * sample],
                self.samples[2 * sample + 1],
            ])) / 65535.0
        } else {
            f32::from(self.samples[sample]) / 255.0
        }
    }

    /// The RGB of the texel at `index`, in 0..=1.
    pub(crate) fn rgb(&self, index: usize) -> [f32; 3] {
        [self.at(index, 0), self.at(index, 1), self.at(index, 2)]
    }

    /// Number of texels.
    pub(crate) fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// Reads a PNG (8 or 16 bits, grey, grey + alpha, RGB, RGBA or palette) into an [`Image`].
pub(crate) fn read_png(path: &Path) -> Result<Image, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|error| format!("{shown}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{shown}: not a regular file"));
    }
    if metadata.len() > MAX_PNG_BYTES {
        return Err(format!("{shown}: larger than {MAX_PNG_BYTES} bytes"));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("{shown}: {error}"))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("{shown}: {error}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > MAX_PNG_SIDE || height > MAX_PNG_SIDE {
        return Err(format!(
            "{shown}: {width}x{height} is outside 1..={MAX_PNG_SIDE} per side"
        ));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{shown}: image too large"))?;
    let mut samples = vec![0; size];
    let frame = reader
        .next_frame(&mut samples)
        .map_err(|error| format!("{shown}: {error}"))?;
    let (colour, depth) = (frame.color_type, frame.bit_depth);
    let channels: usize = match colour {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(format!("{shown}: unexpanded palette")),
    };
    let sixteen = depth == png::BitDepth::Sixteen;
    if !sixteen && depth != png::BitDepth::Eight {
        return Err(format!("{shown}: {depth:?}-bit samples after expansion"));
    }
    let needed = width as usize * height as usize * channels * if sixteen { 2 } else { 1 };
    if frame.buffer_size() < needed
        || frame.line_size != width as usize * channels * if sixteen { 2 } else { 1 }
    {
        return Err(format!("{shown}: truncated image data"));
    }
    samples.truncate(needed);
    samples.shrink_to_fit();
    Ok(Image {
        width,
        height,
        channels: channels as u8,
        sixteen_bit: sixteen,
        samples,
    })
}

/// An 8-bit RGB canvas for contact sheets.
#[derive(Clone, Debug)]
pub(crate) struct Canvas {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<[u8; 3]>,
}

impl Canvas {
    /// A canvas filled with `colour`.
    pub(crate) fn new(width: u32, height: u32, colour: [u8; 3]) -> Self {
        Self {
            width,
            height,
            pixels: vec![colour; width as usize * height as usize],
        }
    }

    /// Sets pixel `(x, y)`; outside the canvas is ignored.
    pub(crate) fn put(&mut self, x: i64, y: i64, colour: [u8; 3]) {
        if x >= 0 && y >= 0 && x < i64::from(self.width) && y < i64::from(self.height) {
            self.pixels[y as usize * self.width as usize + x as usize] = colour;
        }
    }

    /// Pixel `(x, y)`.
    pub(crate) fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Copies `other` with its top-left corner at `(x, y)`, clipped to this canvas.
    pub(crate) fn paste(&mut self, other: &Canvas, x: i64, y: i64) {
        for row in 0..other.height {
            for column in 0..other.width {
                self.put(
                    x + i64::from(column),
                    y + i64::from(row),
                    other.pixel(column, row),
                );
            }
        }
    }

    /// A rectangle outline `width` pixels thick.
    pub(crate) fn outline(&mut self, rect: [i64; 4], thickness: i64, colour: [u8; 3]) {
        let [x0, y0, x1, y1] = rect;
        for t in 0..thickness {
            for x in x0..=x1 {
                self.put(x, y0 + t, colour);
                self.put(x, y1 - t, colour);
            }
            for y in y0..=y1 {
                self.put(x0 + t, y, colour);
                self.put(x1 - t, y, colour);
            }
        }
    }

    /// The RGB (8-bit, display-encoded) of an [`Image`], dropping alpha.
    pub(crate) fn from_image(image: &Image) -> Self {
        let pixels = (0..image.len())
            .map(|index| {
                let [r, g, b] = image.rgb(index);
                [to_byte(r), to_byte(g), to_byte(b)]
            })
            .collect();
        Self {
            width: image.width,
            height: image.height,
            pixels,
        }
    }

    /// The sub-rectangle `[x, y, width, height]`, clipped (outside reads black).
    pub(crate) fn crop(&self, rect: [i64; 4]) -> Self {
        let [x, y, width, height] = rect;
        let mut out = Self::new(width.max(1) as u32, height.max(1) as u32, [0, 0, 0]);
        for row in 0..out.height {
            for column in 0..out.width {
                let (sx, sy) = (x + i64::from(column), y + i64::from(row));
                if sx >= 0 && sy >= 0 && sx < i64::from(self.width) && sy < i64::from(self.height) {
                    out.put(
                        i64::from(column),
                        i64::from(row),
                        self.pixel(sx as u32, sy as u32),
                    );
                }
            }
        }
        out
    }

    /// Area-average resample to `width` × `height` (for shrinking; also used to enlarge).
    pub(crate) fn resample(&self, width: u32, height: u32) -> Self {
        let mut out = Self::new(width.max(1), height.max(1), [0, 0, 0]);
        let sx = f64::from(self.width) / f64::from(out.width);
        let sy = f64::from(self.height) / f64::from(out.height);
        for row in 0..out.height {
            let y0 = f64::from(row) * sy;
            let y1 = (y0 + sy).min(f64::from(self.height));
            for column in 0..out.width {
                let x0 = f64::from(column) * sx;
                let x1 = (x0 + sx).min(f64::from(self.width));
                let mut sum = [0f64; 3];
                let mut weight = 0f64;
                let mut y = y0.floor();
                while y < y1 {
                    let wy = (y + 1.0).min(y1) - y.max(y0);
                    let mut x = x0.floor();
                    while x < x1 {
                        let wx = (x + 1.0).min(x1) - x.max(x0);
                        let w = wx * wy;
                        let p = self.pixel(x as u32, y as u32);
                        for c in 0..3 {
                            sum[c] += f64::from(p[c]) * w;
                        }
                        weight += w;
                        x += 1.0;
                    }
                    y += 1.0;
                }
                let weight = weight.max(1e-12);
                out.pixels[(row * out.width + column) as usize] =
                    sum.map(|s| (s / weight).round().clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    /// Nearest-neighbour enlargement by an integer `factor` (true pixels stay visible).
    pub(crate) fn enlarge(&self, factor: u32) -> Self {
        let factor = factor.max(1);
        let mut out = Self::new(self.width * factor, self.height * factor, [0, 0, 0]);
        for row in 0..out.height {
            for column in 0..out.width {
                out.pixels[(row * out.width + column) as usize] =
                    self.pixel(column / factor, row / factor);
            }
        }
        out
    }

    /// A value-only (grey) copy: each pixel's `OKLab` L, re-encoded as display grey.
    pub(crate) fn value_only(&self) -> Self {
        let pixels = self
            .pixels
            .iter()
            .map(|&[r, g, b]| {
                let l = oklab_l_display([r, g, b].map(|v| f32::from(v) / 255.0));
                // OKLab L is roughly perceptual: show it as the sRGB grey of the same L.
                let grey = to_byte(linear_to_srgb(l * l * l));
                [grey; 3]
            })
            .collect();
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Writes an 8-bit RGB PNG.
    pub(crate) fn write_png(&self, path: &Path) -> Result<(), String> {
        let shown = path.display();
        let file = std::fs::File::create(path).map_err(|error| format!("{shown}: {error}"))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("{shown}: {error}"))?;
        let bytes: Vec<u8> = self.pixels.iter().flatten().copied().collect();
        writer
            .write_image_data(&bytes)
            .map_err(|error| format!("{shown}: {error}"))
    }
}

/// 0..=1 to a byte.
pub(crate) fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// sRGB-encoded 0..=1 to linear light.
pub(crate) fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to sRGB-encoded 0..=1.
pub(crate) fn linear_to_srgb(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Rec. 709 relative luminance of linear RGB.
pub(crate) fn luminance(linear: [f32; 3]) -> f32 {
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

/// The bible's **luma**: relative luminance of display-encoded `rgb`, re-encoded to 8-bit sRGB
/// (0..=255).
pub(crate) fn luma(rgb: [f32; 3]) -> f32 {
    linear_to_srgb(luminance(rgb.map(srgb_to_linear))) * 255.0
}

/// `OKLab` `[L, a, b]` of linear RGB.
#[allow(clippy::many_single_char_names)] // The published matrices' names.
pub(crate) fn oklab(linear: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = linear;
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// `OKLCH` `[L, C, h°]` of display-encoded `rgb`.
pub(crate) fn oklch_display(rgb: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = oklab(rgb.map(srgb_to_linear));
    let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
    [l, a.hypot(b), hue]
}

/// `OKLab` L of display-encoded `rgb`.
pub(crate) fn oklab_l_display(rgb: [f32; 3]) -> f32 {
    oklab(rgb.map(srgb_to_linear))[0]
}

/// Parses `#RRGGBB` to display-encoded 0..=1.
pub(crate) fn parse_hex(hex: &str) -> Option<[f32; 3]> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?].map(|v| f32::from(v) / 255.0))
}

/// The `q`th percentile (0..=100) of `values`, interpolated linearly between ranks (numpy's
/// default), or `None` when empty. Sorts `values`.
pub(crate) fn percentile(values: &mut [f32], q: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f32::total_cmp);
    Some(percentile_sorted(values, q))
}

/// [`percentile`] over already sorted, non-empty `values`.
pub(crate) fn percentile_sorted(values: &[f32], q: f32) -> f32 {
    let rank = (q.clamp(0.0, 100.0) / 100.0) * (values.len() - 1) as f32;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    let t = rank - low as f32;
    values[low] + (values[high] - values[low]) * t
}

/// Mean of `values` (0 when empty).
pub(crate) fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    (values.iter().map(|&v| f64::from(v)).sum::<f64>() / values.len() as f64) as f32
}

/// Population standard deviation of `values` (0 when empty).
pub(crate) fn std_dev(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let m = f64::from(mean(values));
    let variance = values
        .iter()
        .map(|&v| (f64::from(v) - m).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt() as f32
}

/// Rounds for reports: `digits` decimals.
pub(crate) fn round(value: f32, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (f64::from(value) * scale).round() / scale
}
