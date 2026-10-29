//! Validated textures and the bounded KTX2 container (ADR-0010).
//!
//! [`TextureData`] is the hand-off type between the texture loader and the renderer: one 2D
//! image, or one cube map (six square faces, R2a image-based lighting), with a full or
//! partial mip chain in one of the [`TextureFormat`]s the engine samples. Its invariants are
//! checked at construction, so the renderer's upload copies never read past a level:
//!
//! - width and height in `1..=`[`MAX_TEXTURE_DIMENSION`] (cube faces: square and at most
//!   [`MAX_CUBE_DIMENSION`]);
//! - `1..=` the full chain length of mip levels, level `i` being `max(1, size >> i)`;
//! - every level holds exactly the bytes its format and size need (whole 4×4 blocks for the
//!   block-compressed formats), times six for a cube map, whose faces are stored one after
//!   another in each level in the KTX2/Vulkan order `+X, -X, +Y, -Y, +Z, -Z`.
//!
//! [`load_ktx2`] parses a KTX 2.0 container from untrusted bytes. Every count, dimension,
//! offset and length is checked against the caps and the file size *before* anything is
//! allocated, level ranges must not overlap (so the texel data never exceeds the file), the data format descriptor's transfer function must agree with the format's
//! sRGB-ness, and supercompression is rejected (Zstd/Basis would need a decoder dependency;
//! see the ADR). [`write_ktx2`] produces the files the `texture-bake` tool ships.

use std::io::Read;
use std::path::Path;

/// Largest accepted `.ktx2` file (64 MiB).
pub const MAX_KTX2_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted width or height, in texels.
pub const MAX_TEXTURE_DIMENSION: u32 = 4096;
/// Most mip levels a texture can have (the full chain of a
/// [`MAX_TEXTURE_DIMENSION`]-sized image).
pub const MAX_MIP_LEVELS: u32 = MAX_TEXTURE_DIMENSION.ilog2() + 1;
/// Largest accepted cube-map face edge, in texels (environment maps are small: the
/// shipped ones are 64²).
pub const MAX_CUBE_DIMENSION: u32 = 1024;
/// Faces of a cube map.
const CUBE_FACES: u32 = 6;

/// KTX 2.0 file identifier.
const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
/// Identifier, 9 header words and the index (4 words, 2 double words).
const HEADER_BYTES: usize = 12 + 9 * 4 + 4 * 4 + 2 * 8;
/// One level-index entry: byte offset, byte length, uncompressed byte length.
const LEVEL_INDEX_BYTES: usize = 3 * 8;

/// Khronos data format descriptor constants (KDFS 1.3, basic descriptor block).
mod dfd {
    pub(super) const MODEL_RGBSDA: u8 = 1;
    pub(super) const MODEL_BC4: u8 = 131;
    pub(super) const MODEL_BC5: u8 = 132;
    pub(super) const MODEL_BC7: u8 = 134;
    pub(super) const PRIMARIES_BT709: u8 = 1;
    pub(super) const TRANSFER_LINEAR: u8 = 1;
    pub(super) const TRANSFER_SRGB: u8 = 2;
    pub(super) const CHANNEL_ALPHA: u8 = 15;
    pub(super) const QUALIFIER_LINEAR: u8 = 0x10;
    pub(super) const QUALIFIER_SIGNED: u8 = 0x40;
    pub(super) const QUALIFIER_FLOAT: u8 = 0x80;
    /// `-1.0f32` and `1.0f32`: a float sample's lower and upper bounds.
    pub(super) const FLOAT_LOWER: u32 = 0xBF80_0000;
    pub(super) const FLOAT_UPPER: u32 = 0x3F80_0000;
    /// Block header (2 words) plus model/dimension/plane words.
    pub(super) const BLOCK_HEADER_BYTES: usize = 24;
    pub(super) const SAMPLE_BYTES: usize = 16;
}

/// Texel formats the engine samples.
///
/// Colour data is sRGB-encoded and everything else (normals, metallic/roughness, masks) is
/// linear: the format, not a flag, says which, so a sampler can never apply the wrong curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextureFormat {
    /// 8-bit RGBA, sRGB colour with linear alpha (`VK_FORMAT_R8G8B8A8_SRGB`).
    Rgba8Srgb,
    /// 8-bit RGBA, linear (`VK_FORMAT_R8G8B8A8_UNORM`).
    Rgba8Unorm,
    /// 8-bit RG, linear (`VK_FORMAT_R8G8_UNORM`): normals (XY), metallic/roughness.
    Rg8Unorm,
    /// 8-bit R, linear (`VK_FORMAT_R8_UNORM`): masks.
    R8Unorm,
    /// BC7 RGBA, sRGB colour (`VK_FORMAT_BC7_SRGB_BLOCK`), 16 bytes per 4×4 block.
    Bc7Srgb,
    /// BC7 RGBA, linear (`VK_FORMAT_BC7_UNORM_BLOCK`).
    Bc7Unorm,
    /// BC5 two-channel, linear (`VK_FORMAT_BC5_UNORM_BLOCK`), 16 bytes per block.
    Bc5Unorm,
    /// BC4 one-channel, linear (`VK_FORMAT_BC4_UNORM_BLOCK`), 8 bytes per block.
    Bc4Unorm,
    /// 16-bit float RGBA, linear (`VK_FORMAT_R16G16B16A16_SFLOAT`), 8 bytes per texel: HDR
    /// data such as prefiltered environment cube maps (R2a). No material slot takes it.
    Rgba16Float,
}

impl TextureFormat {
    /// Every format, for tools and tests.
    pub const ALL: &'static [Self] = &[
        Self::Rgba8Srgb,
        Self::Rgba8Unorm,
        Self::Rg8Unorm,
        Self::R8Unorm,
        Self::Bc7Srgb,
        Self::Bc7Unorm,
        Self::Bc5Unorm,
        Self::Bc4Unorm,
        Self::Rgba16Float,
    ];

    /// The `VkFormat` value (the KTX2 header's `vkFormat`).
    #[must_use]
    pub const fn vk_format(self) -> u32 {
        match self {
            Self::R8Unorm => 9,
            Self::Rg8Unorm => 16,
            Self::Rgba8Unorm => 37,
            Self::Rgba8Srgb => 43,
            Self::Rgba16Float => 97,
            Self::Bc4Unorm => 139,
            Self::Bc5Unorm => 141,
            Self::Bc7Unorm => 145,
            Self::Bc7Srgb => 146,
        }
    }

    /// The format with this `VkFormat` value, if the engine supports it.
    #[must_use]
    pub fn from_vk_format(value: u32) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.vk_format() == value)
    }

    /// Short lower-case name (`"bc7-srgb"`, `"rg8"`, ...), as the bake tool accepts it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rgba8Srgb => "rgba8-srgb",
            Self::Rgba8Unorm => "rgba8",
            Self::Rg8Unorm => "rg8",
            Self::R8Unorm => "r8",
            Self::Bc7Srgb => "bc7-srgb",
            Self::Bc7Unorm => "bc7",
            Self::Bc5Unorm => "bc5",
            Self::Bc4Unorm => "bc4",
            Self::Rgba16Float => "rgba16f",
        }
    }

    /// The format called `name` (see [`Self::name`]), ignoring ASCII case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether colour channels are sRGB-encoded (the sampler decodes them to linear).
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Rgba8Srgb | Self::Bc7Srgb)
    }

    /// Whether the format is a 4×4 block-compressed (BC) format.
    #[must_use]
    pub const fn is_block_compressed(self) -> bool {
        matches!(
            self,
            Self::Bc7Srgb | Self::Bc7Unorm | Self::Bc5Unorm | Self::Bc4Unorm
        )
    }

    /// Number of channels the shader sees (4, 2 or 1).
    #[must_use]
    pub const fn channels(self) -> u32 {
        match self {
            Self::Rgba8Srgb
            | Self::Rgba8Unorm
            | Self::Bc7Srgb
            | Self::Bc7Unorm
            | Self::Rgba16Float => 4,
            Self::Rg8Unorm | Self::Bc5Unorm => 2,
            Self::R8Unorm | Self::Bc4Unorm => 1,
        }
    }

    /// Bytes per texel (uncompressed) or per 4×4 block (compressed).
    #[must_use]
    pub const fn block_bytes(self) -> u32 {
        match self {
            Self::Rgba8Srgb | Self::Rgba8Unorm => 4,
            Self::Rg8Unorm => 2,
            Self::R8Unorm => 1,
            Self::Rgba16Float | Self::Bc4Unorm => 8,
            Self::Bc7Srgb | Self::Bc7Unorm | Self::Bc5Unorm => 16,
        }
    }

    /// Texel block edge: 4 for BC formats, 1 otherwise.
    #[must_use]
    pub const fn block_dimension(self) -> u32 {
        if self.is_block_compressed() { 4 } else { 1 }
    }

    /// The uncompressed format with the same channels and encoding, which a CPU decode of
    /// this format produces (itself for uncompressed formats).
    #[must_use]
    pub const fn uncompressed(self) -> Self {
        match self {
            Self::Bc7Srgb => Self::Rgba8Srgb,
            Self::Bc7Unorm => Self::Rgba8Unorm,
            Self::Bc5Unorm => Self::Rg8Unorm,
            Self::Bc4Unorm => Self::R8Unorm,
            other => other,
        }
    }

    /// Bytes of one level of `width` × `height` texels, or `None` on overflow.
    #[must_use]
    pub fn level_bytes(self, width: u32, height: u32) -> Option<usize> {
        let block = self.block_dimension();
        let columns = u64::from(width.div_ceil(block));
        let rows = u64::from(height.div_ceil(block));
        let bytes = columns
            .checked_mul(rows)?
            .checked_mul(u64::from(self.block_bytes()))?;
        usize::try_from(bytes).ok()
    }
}

impl core::fmt::Display for TextureFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a texture or KTX2 file was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextureError {
    /// The file does not exist or cannot be read; the text is the I/O error.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file is larger than [`MAX_KTX2_BYTES`].
    TooLarge,
    /// Not a KTX 2.0 file, or the header or index is cut short.
    NotKtx2,
    /// The `vkFormat` is not one of the [`TextureFormat`]s (the value is given).
    UnsupportedFormat(u32),
    /// A KTX2 feature the engine does not load: the text names it (arrays, 3D,
    /// supercompression, a face count other than 1 or 6, ...).
    Unsupported(&'static str),
    /// Width or height is zero or above [`MAX_TEXTURE_DIMENSION`], or a cube map's faces are
    /// not square or larger than [`MAX_CUBE_DIMENSION`].
    BadDimensions,
    /// More mip levels than the full chain, or none.
    BadLevelCount,
    /// A level's byte range is outside the file or its length does not match its size.
    BadLevel,
    /// The data format descriptor is missing, cut short, or disagrees with `vkFormat`
    /// (for example a linear transfer function on an sRGB format).
    BadDescriptor,
    /// A BC block could not be decoded (only with the `bc-decode` feature).
    Decode,
}

impl core::fmt::Display for TextureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "missing or unreadable ({error})"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::TooLarge => write!(f, "larger than {MAX_KTX2_BYTES} bytes"),
            Self::NotKtx2 => f.write_str("not a KTX 2.0 file"),
            Self::UnsupportedFormat(value) => write!(f, "unsupported vkFormat {value}"),
            Self::Unsupported(what) => write!(f, "unsupported KTX2 feature: {what}"),
            Self::BadDimensions => {
                write!(f, "texture size outside 1..={MAX_TEXTURE_DIMENSION} texels")
            }
            Self::BadLevelCount => f.write_str("bad mip level count"),
            Self::BadLevel => f.write_str("mip level data out of range or wrong size"),
            Self::BadDescriptor => f.write_str("bad data format descriptor"),
            Self::Decode => f.write_str("block decode failed"),
        }
    }
}

impl std::error::Error for TextureError {}

/// One mip level's place in [`TextureData::data`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Level {
    offset: usize,
    len: usize,
}

/// A validated 2D texture or cube map with its mip chain (see the module docs for the
/// invariants).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureData {
    format: TextureFormat,
    width: u32,
    height: u32,
    /// 1, or 6 for a cube map.
    faces: u32,
    levels: Vec<Level>,
    data: Vec<u8>,
}

/// Number of levels in the full mip chain of a `width` × `height` image.
#[must_use]
pub fn full_mip_chain(width: u32, height: u32) -> u32 {
    width.max(height).max(1).ilog2() + 1
}

/// Size of mip level `level` of a `width` × `height` image.
#[must_use]
pub fn mip_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    (
        width.checked_shr(level).unwrap_or(0).max(1),
        height.checked_shr(level).unwrap_or(0).max(1),
    )
}

fn check_shape(width: u32, height: u32, faces: u32, levels: usize) -> Result<(), TextureError> {
    if width == 0 || height == 0 || width > MAX_TEXTURE_DIMENSION || height > MAX_TEXTURE_DIMENSION
    {
        return Err(TextureError::BadDimensions);
    }
    if faces == CUBE_FACES && (width != height || width > MAX_CUBE_DIMENSION) {
        return Err(TextureError::BadDimensions);
    }
    let chain = full_mip_chain(width, height) as usize;
    if levels == 0 || levels > chain {
        return Err(TextureError::BadLevelCount);
    }
    Ok(())
}

impl TextureData {
    /// Builds a texture from its levels, largest first, after checking every invariant.
    ///
    /// # Errors
    /// [`TextureError::BadDimensions`], [`TextureError::BadLevelCount`], or
    /// [`TextureError::BadLevel`] when a level's length is not exactly what its size needs.
    pub fn new(
        format: TextureFormat,
        width: u32,
        height: u32,
        levels: &[Vec<u8>],
    ) -> Result<Self, TextureError> {
        Self::build(format, width, height, 1, levels)
    }

    /// Builds a cube map with `size` × `size` faces from its levels, largest first. Each
    /// level holds the six faces one after another in the order `+X, -X, +Y, -Y, +Z, -Z`
    /// (KTX2 and Vulkan), each `format.level_bytes(w, h)` bytes.
    ///
    /// # Errors
    /// [`TextureError::BadDimensions`] (also for `size` above [`MAX_CUBE_DIMENSION`]),
    /// [`TextureError::BadLevelCount`], or [`TextureError::BadLevel`] when a level's length is
    /// not exactly six faces.
    pub fn new_cube(
        format: TextureFormat,
        size: u32,
        levels: &[Vec<u8>],
    ) -> Result<Self, TextureError> {
        Self::build(format, size, size, CUBE_FACES, levels)
    }

    fn build(
        format: TextureFormat,
        width: u32,
        height: u32,
        faces: u32,
        levels: &[Vec<u8>],
    ) -> Result<Self, TextureError> {
        check_shape(width, height, faces, levels.len())?;
        let mut total = 0usize;
        for (index, level) in levels.iter().enumerate() {
            let (w, h) = mip_size(width, height, index as u32);
            if level_bytes(format, w, h, faces) != Some(level.len()) {
                return Err(TextureError::BadLevel);
            }
            total = total
                .checked_add(level.len())
                .ok_or(TextureError::BadLevel)?;
        }
        let mut data = Vec::with_capacity(total);
        let mut table = Vec::with_capacity(levels.len());
        for level in levels {
            table.push(Level {
                offset: data.len(),
                len: level.len(),
            });
            data.extend_from_slice(level);
        }
        Ok(Self {
            format,
            width,
            height,
            faces,
            levels: table,
            data,
        })
    }

    /// Texel format.
    #[must_use]
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Width of level 0 in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of level 0 in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of mip levels (at least 1).
    #[must_use]
    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Number of faces: 1, or 6 for a cube map.
    #[must_use]
    pub fn face_count(&self) -> u32 {
        self.faces
    }

    /// Whether this is a cube map (six square faces).
    #[must_use]
    pub fn is_cube(&self) -> bool {
        self.faces == CUBE_FACES
    }

    /// Level `index`'s size and bytes (largest level first). For a cube map the bytes are
    /// the whole level, every face in order (see [`Self::face`] for one face).
    #[must_use]
    pub fn level(&self, index: u32) -> Option<(u32, u32, &[u8])> {
        let level = self.levels.get(index as usize)?;
        let (width, height) = mip_size(self.width, self.height, index);
        Some((
            width,
            height,
            &self.data[level.offset..level.offset + level.len],
        ))
    }

    /// The bytes of face `face` (`0..face_count()`, cube order `+X, -X, +Y, -Y, +Z, -Z`) of
    /// level `level`.
    #[must_use]
    pub fn face(&self, level: u32, face: u32) -> Option<&[u8]> {
        if face >= self.faces {
            return None;
        }
        let (_, _, bytes) = self.level(level)?;
        let face_len = bytes.len() / self.faces as usize;
        let start = face_len * face as usize;
        bytes.get(start..start + face_len)
    }

    /// Every level's bytes, contiguous and largest first.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Byte offset of level `index` in [`Self::data`].
    #[must_use]
    pub fn level_offset(&self, index: u32) -> Option<usize> {
        self.levels.get(index as usize).map(|level| level.offset)
    }

    /// Decodes a block-compressed texture to its [`TextureFormat::uncompressed`] format on
    /// the CPU (every level). An uncompressed texture is returned as a copy. This is the
    /// fallback for devices without `textureCompressionBC` (ADR-0010).
    ///
    /// # Errors
    /// [`TextureError::Decode`] if a level cannot be decoded (not reachable for a validated
    /// texture, but reported rather than panicking).
    #[cfg(feature = "bc-decode")]
    pub fn decode_to_uncompressed(&self) -> Result<Self, TextureError> {
        if !self.format.is_block_compressed() {
            return Ok(self.clone());
        }
        let target = self.format.uncompressed();
        let mut levels = Vec::with_capacity(self.levels.len());
        for index in 0..self.level_count() {
            let (width, height, _) = self.level(index).ok_or(TextureError::Decode)?;
            let mut level = Vec::new();
            for face in 0..self.faces {
                let bytes = self.face(index, face).ok_or(TextureError::Decode)?;
                level.extend_from_slice(&decode_level(self.format, width, height, bytes)?);
            }
            levels.push(level);
        }
        Self::build(target, self.width, self.height, self.faces, &levels)
    }
}

/// Decodes one BC level into tightly packed texels of the uncompressed format.
#[cfg(feature = "bc-decode")]
fn decode_level(
    format: TextureFormat,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> Result<Vec<u8>, TextureError> {
    let block_bytes = format.block_bytes() as usize;
    let channels = format.channels() as usize;
    let out_len = format
        .uncompressed()
        .level_bytes(width, height)
        .ok_or(TextureError::Decode)?;
    let mut out = vec![0u8; out_len];
    let (width, height) = (width as usize, height as usize);
    let columns = width.div_ceil(4);
    let rows = height.div_ceil(4);
    if bytes.len() != columns * rows * block_bytes {
        return Err(TextureError::Decode);
    }
    let mut texels = [0u8; 4 * 4 * 4];
    for (index, block) in bytes.chunks_exact(block_bytes).enumerate() {
        let pitch = 4 * channels;
        match format {
            TextureFormat::Bc7Srgb | TextureFormat::Bc7Unorm => {
                bcdec_rs::bc7(block, &mut texels, pitch);
            }
            TextureFormat::Bc5Unorm => bcdec_rs::bc5(block, &mut texels, pitch, false),
            TextureFormat::Bc4Unorm => bcdec_rs::bc4(block, &mut texels, pitch, false),
            _ => return Err(TextureError::Decode),
        }
        let (bx, by) = ((index % columns) * 4, (index / columns) * 4);
        for y in 0..4.min(height - by) {
            let visible = 4.min(width - bx) * channels;
            let destination = ((by + y) * width + bx) * channels;
            out[destination..destination + visible]
                .copy_from_slice(&texels[y * pitch..y * pitch + visible]);
        }
    }
    Ok(out)
}

/// Bytes of one level with `faces` faces of `width` × `height` texels.
fn level_bytes(format: TextureFormat, width: u32, height: u32, faces: u32) -> Option<usize> {
    format
        .level_bytes(width, height)?
        .checked_mul(faces as usize)
}

/// Converts an `f32` to IEEE 754 binary16 bits, rounding to nearest even. Values beyond the
/// half range become infinity, tiny values become (signed) subnormals or zero, and NaN stays
/// a (quiet) NaN.
#[must_use]
pub fn f16_from_f32(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff).cast_signed();
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        // Infinity, or NaN with the quiet bit set and the payload's top bits kept.
        let nan = if mantissa == 0 {
            0
        } else {
            0x0200 | (mantissa >> 13) as u16
        };
        return sign | 0x7c00 | nan;
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        // Normal: a carry out of the mantissa bumps the exponent (to infinity at the top).
        // `unbiased + 15` is in 1..=30 here.
        let mut half = ((unbiased + 15).cast_unsigned() << 10) | (mantissa >> 13);
        let rest = mantissa & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && half & 1 == 1) {
            half += 1;
        }
        return sign | half as u16;
    }
    if unbiased < -25 {
        return sign;
    }
    // Subnormal half (value = m * 2^-24); the implicit leading one is made explicit.
    let full = mantissa | 0x0080_0000;
    let shift = (-1 - unbiased).cast_unsigned(); // 14..=24
    let mut half = full >> shift;
    let rest = full & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if rest > halfway || (rest == halfway && half & 1 == 1) {
        half += 1;
    }
    sign | half as u16
}

/// Converts IEEE 754 binary16 bits to `f32` (exact).
#[must_use]
pub fn f32_from_f16(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x03ff);
    match exponent {
        0 => {
            // Zero or subnormal: m * 2^-24, exact in f32.
            let magnitude = mantissa as f32 * (1.0 / 16_777_216.0);
            if sign == 0 { magnitude } else { -magnitude }
        }
        31 => f32::from_bits(sign | 0x7f80_0000 | (mantissa << 13)),
        _ => f32::from_bits(sign | ((exponent + 112) << 23) | (mantissa << 13)),
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let chunk = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(chunk.try_into().ok()?))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let chunk = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes(chunk.try_into().ok()?))
}

/// A byte range `offset..offset + len` that must lie inside a file of `file_len` bytes.
fn range_in(file_len: usize, offset: u64, len: u64) -> Option<core::ops::Range<usize>> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    (end <= file_len).then_some(start..end)
}

/// Reads and validates a `.ktx2` file of at most [`MAX_KTX2_BYTES`].
///
/// # Errors
/// [`TextureError::Io`], [`TextureError::NotAFile`], [`TextureError::TooLarge`], or any
/// [`load_ktx2`] error.
pub fn load_ktx2_file(path: &Path) -> Result<TextureData, TextureError> {
    let metadata = std::fs::metadata(path).map_err(|error| TextureError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(TextureError::NotAFile);
    }
    if metadata.len() > MAX_KTX2_BYTES {
        return Err(TextureError::TooLarge);
    }
    let file = std::fs::File::open(path).map_err(|error| TextureError::Io(error.to_string()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // `take` bounds the read even if the file grows after the metadata check.
    file.take(MAX_KTX2_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| TextureError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_KTX2_BYTES {
        return Err(TextureError::TooLarge);
    }
    load_ktx2(&bytes)
}

/// Parses a KTX 2.0 container from untrusted bytes.
///
/// Accepts one 2D image, or one cube map (`faceCount` 6, square faces of at most
/// [`MAX_CUBE_DIMENSION`]), without array layers or depth, in a [`TextureFormat`], with
/// `1..=` the full chain of mip levels, no supercompression, and a basic data format
/// descriptor whose transfer function matches the format (sRGB for `*_SRGB`, linear
/// otherwise). A `levelCount` of 0 ("generate mips at load") is read as one level. Never
/// panics; allocates only the texel data, after every check has passed.
///
/// # Errors
/// The first [`TextureError`] found.
pub fn load_ktx2(bytes: &[u8]) -> Result<TextureData, TextureError> {
    if bytes.len() as u64 > MAX_KTX2_BYTES {
        return Err(TextureError::TooLarge);
    }
    if bytes.len() < HEADER_BYTES || bytes[..12] != KTX2_IDENTIFIER {
        return Err(TextureError::NotKtx2);
    }
    let word = |index: usize| u32_at(bytes, 12 + 4 * index).ok_or(TextureError::NotKtx2);
    let vk_format = word(0)?;
    let type_size = word(1)?;
    let width = word(2)?;
    let height = word(3)?;
    let depth = word(4)?;
    let layers = word(5)?;
    let faces = word(6)?;
    let level_count = word(7)?;
    let supercompression = word(8)?;
    let dfd_offset = word(9)?;
    let dfd_length = word(10)?;
    let kvd_offset = word(11)?;
    let kvd_length = word(12)?;
    let sgd_offset = u64_at(bytes, 64).ok_or(TextureError::NotKtx2)?;
    let sgd_length = u64_at(bytes, 72).ok_or(TextureError::NotKtx2)?;

    let format = TextureFormat::from_vk_format(vk_format)
        .ok_or(TextureError::UnsupportedFormat(vk_format))?;
    if type_size != 1 {
        return Err(TextureError::Unsupported("typeSize other than 1"));
    }
    if depth != 0 {
        return Err(TextureError::Unsupported("3D textures"));
    }
    if layers > 1 {
        return Err(TextureError::Unsupported("array textures"));
    }
    if faces != 1 && faces != CUBE_FACES {
        return Err(TextureError::Unsupported("cube maps need 6 faces"));
    }
    if supercompression != 0 || sgd_length != 0 {
        return Err(TextureError::Unsupported("supercompression"));
    }
    let levels = level_count.max(1);
    if levels > MAX_MIP_LEVELS {
        return Err(TextureError::BadLevelCount);
    }
    check_shape(width, height, faces, levels as usize)?;
    // Unused when its length is zero, but a stated offset must still be in the file.
    if range_in(bytes.len(), sgd_offset, sgd_length).is_none() {
        return Err(TextureError::BadLevel);
    }
    if range_in(bytes.len(), u64::from(kvd_offset), u64::from(kvd_length)).is_none() {
        return Err(TextureError::BadDescriptor);
    }
    let dfd = range_in(bytes.len(), u64::from(dfd_offset), u64::from(dfd_length))
        .ok_or(TextureError::BadDescriptor)?;
    check_descriptor(&bytes[dfd], format)?;

    // Every level's range and exact size, before allocating anything.
    let index_end = HEADER_BYTES + LEVEL_INDEX_BYTES * levels as usize;
    if bytes.len() < index_end {
        return Err(TextureError::NotKtx2);
    }
    let mut ranges: [core::ops::Range<usize>; MAX_MIP_LEVELS as usize] =
        core::array::from_fn(|_| 0..0);
    let mut total = 0usize;
    for level in 0..levels {
        let entry = HEADER_BYTES + LEVEL_INDEX_BYTES * level as usize;
        let offset = u64_at(bytes, entry).ok_or(TextureError::NotKtx2)?;
        let length = u64_at(bytes, entry + 8).ok_or(TextureError::NotKtx2)?;
        let uncompressed = u64_at(bytes, entry + 16).ok_or(TextureError::NotKtx2)?;
        let (w, h) = mip_size(width, height, level);
        let expected = level_bytes(format, w, h, faces).ok_or(TextureError::BadLevel)?;
        if length != expected as u64 || uncompressed != length {
            return Err(TextureError::BadLevel);
        }
        let range = range_in(bytes.len(), offset, length).ok_or(TextureError::BadLevel)?;
        total = total
            .checked_add(range.len())
            .ok_or(TextureError::BadLevel)?;
        // Levels may not share bytes (the specification stores them disjoint, smallest
        // first), so the texel data can never exceed the file: no aliased level ranges that
        // inflate a small file into a large allocation.
        if ranges[..level as usize]
            .iter()
            .any(|other| range.start < other.end && other.start < range.end)
        {
            return Err(TextureError::BadLevel);
        }
        ranges[level as usize] = range;
    }
    debug_assert!(total <= bytes.len());
    let mut data = Vec::with_capacity(total);
    let mut table = Vec::with_capacity(levels as usize);
    for range in &ranges[..levels as usize] {
        table.push(Level {
            offset: data.len(),
            len: range.len(),
        });
        data.extend_from_slice(&bytes[range.clone()]);
    }
    Ok(TextureData {
        format,
        width,
        height,
        faces,
        levels: table,
        data,
    })
}

/// Checks the basic data format descriptor block against `format`.
fn check_descriptor(dfd: &[u8], format: TextureFormat) -> Result<(), TextureError> {
    let total = u32_at(dfd, 0).ok_or(TextureError::BadDescriptor)? as usize;
    if total != dfd.len() || total < 4 + dfd::BLOCK_HEADER_BYTES {
        return Err(TextureError::BadDescriptor);
    }
    let block = &dfd[4..];
    let vendor_and_type = u32_at(block, 0).ok_or(TextureError::BadDescriptor)?;
    let version_and_size = u32_at(block, 4).ok_or(TextureError::BadDescriptor)?;
    let block_size = (version_and_size >> 16) as usize;
    // Khronos vendor (0), basic descriptor type (0); the block fits in the descriptor.
    if vendor_and_type != 0
        || block_size < dfd::BLOCK_HEADER_BYTES
        || block_size > block.len()
        || !(block_size - dfd::BLOCK_HEADER_BYTES).is_multiple_of(dfd::SAMPLE_BYTES)
    {
        return Err(TextureError::BadDescriptor);
    }
    let model = block[8];
    let transfer = block[10];
    let expected_model = match format {
        TextureFormat::Bc7Srgb | TextureFormat::Bc7Unorm => dfd::MODEL_BC7,
        TextureFormat::Bc5Unorm => dfd::MODEL_BC5,
        TextureFormat::Bc4Unorm => dfd::MODEL_BC4,
        _ => dfd::MODEL_RGBSDA,
    };
    let expected_transfer = if format.is_srgb() {
        dfd::TRANSFER_SRGB
    } else {
        dfd::TRANSFER_LINEAR
    };
    if model != expected_model || transfer != expected_transfer {
        return Err(TextureError::BadDescriptor);
    }
    Ok(())
}

/// The basic data format descriptor (with its leading total size) for `format`.
fn descriptor(format: TextureFormat) -> Vec<u8> {
    // (channel id | qualifiers, bit offset, bit length, lower, upper)
    const HALF: u8 = dfd::QUALIFIER_FLOAT | dfd::QUALIFIER_SIGNED;
    const LOWER: u32 = dfd::FLOAT_LOWER;
    const UPPER: u32 = dfd::FLOAT_UPPER;
    let samples: &[(u8, u16, u8, u32, u32)] = match format {
        TextureFormat::Rgba8Srgb => &[
            (0, 0, 8, 0, 255),
            (1, 8, 8, 0, 255),
            (2, 16, 8, 0, 255),
            (dfd::CHANNEL_ALPHA | dfd::QUALIFIER_LINEAR, 24, 8, 0, 255),
        ],
        TextureFormat::Rgba8Unorm => &[
            (0, 0, 8, 0, 255),
            (1, 8, 8, 0, 255),
            (2, 16, 8, 0, 255),
            (dfd::CHANNEL_ALPHA, 24, 8, 0, 255),
        ],
        TextureFormat::Rg8Unorm => &[(0, 0, 8, 0, 255), (1, 8, 8, 0, 255)],
        TextureFormat::R8Unorm => &[(0, 0, 8, 0, 255)],
        TextureFormat::Bc7Srgb | TextureFormat::Bc7Unorm => &[(0, 0, 128, 0, u32::MAX)],
        TextureFormat::Bc5Unorm => &[(0, 0, 64, 0, u32::MAX), (1, 64, 64, 0, u32::MAX)],
        TextureFormat::Bc4Unorm => &[(0, 0, 64, 0, u32::MAX)],
        TextureFormat::Rgba16Float => &[
            (HALF, 0, 16, LOWER, UPPER),
            (1 | HALF, 16, 16, LOWER, UPPER),
            (2 | HALF, 32, 16, LOWER, UPPER),
            (dfd::CHANNEL_ALPHA | HALF, 48, 16, LOWER, UPPER),
        ],
    };
    let model = match format {
        TextureFormat::Bc7Srgb | TextureFormat::Bc7Unorm => dfd::MODEL_BC7,
        TextureFormat::Bc5Unorm => dfd::MODEL_BC5,
        TextureFormat::Bc4Unorm => dfd::MODEL_BC4,
        _ => dfd::MODEL_RGBSDA,
    };
    let transfer = if format.is_srgb() {
        dfd::TRANSFER_SRGB
    } else {
        dfd::TRANSFER_LINEAR
    };
    let block_size = dfd::BLOCK_HEADER_BYTES + dfd::SAMPLE_BYTES * samples.len();
    let mut out = Vec::with_capacity(4 + block_size);
    out.extend_from_slice(&((4 + block_size) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // vendor 0 (Khronos), type 0 (basic)
    out.extend_from_slice(&(2u32 | ((block_size as u32) << 16)).to_le_bytes()); // version 2
    out.extend_from_slice(&[model, dfd::PRIMARIES_BT709, transfer, 0]); // straight alpha
    let edge = (format.block_dimension() - 1) as u8;
    out.extend_from_slice(&[edge, edge, 0, 0]);
    out.extend_from_slice(&[format.block_bytes() as u8, 0, 0, 0, 0, 0, 0, 0]);
    for &(channel, bit_offset, bit_length, lower, upper) in samples {
        out.extend_from_slice(&bit_offset.to_le_bytes());
        out.push(bit_length - 1);
        out.push(channel);
        out.extend_from_slice(&[0; 4]); // sample position
        out.extend_from_slice(&lower.to_le_bytes());
        out.extend_from_slice(&upper.to_le_bytes());
    }
    out
}

/// Serialises `texture` (a 2D image or a cube map) as a KTX 2.0 file: no supercompression, a basic data format
/// descriptor, a `KTXwriter` key, and mip data stored smallest level first, each level
/// aligned to `lcm(block bytes, 4)` as the specification requires.
#[must_use]
pub fn write_ktx2(texture: &TextureData, writer: &str) -> Vec<u8> {
    let levels = texture.levels.len();
    let dfd = descriptor(texture.format);
    let mut kvd = Vec::new();
    let key = b"KTXwriter\0";
    let value: Vec<u8> = writer
        .bytes()
        .filter(|byte| *byte != 0)
        .take(255)
        .chain([0])
        .collect();
    kvd.extend_from_slice(&((key.len() + value.len()) as u32).to_le_bytes());
    kvd.extend_from_slice(key);
    kvd.extend_from_slice(&value);
    while kvd.len() % 4 != 0 {
        kvd.push(0);
    }
    let dfd_offset = HEADER_BYTES + LEVEL_INDEX_BYTES * levels;
    let kvd_offset = dfd_offset + dfd.len();
    let alignment = match texture.format.block_bytes() {
        8 => 8,
        16 => 16,
        _ => 4,
    };
    let mut out = Vec::with_capacity(kvd_offset + kvd.len() + texture.data.len() + 16 * levels);
    out.extend_from_slice(&KTX2_IDENTIFIER);
    for value in [
        texture.format.vk_format(),
        1, // typeSize
        texture.width,
        texture.height,
        0, // pixelDepth
        0, // layerCount
        texture.faces,
        levels as u32,
        0, // supercompressionScheme
        dfd_offset as u32,
        dfd.len() as u32,
        kvd_offset as u32,
        kvd.len() as u32,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u64.to_le_bytes()); // sgdByteOffset
    out.extend_from_slice(&0u64.to_le_bytes()); // sgdByteLength
    let index_at = out.len();
    out.resize(index_at + LEVEL_INDEX_BYTES * levels, 0);
    out.extend_from_slice(&dfd);
    out.extend_from_slice(&kvd);
    for level in (0..levels).rev() {
        while out.len() % alignment != 0 {
            out.push(0);
        }
        let Level { offset, len } = texture.levels[level];
        let entry = index_at + LEVEL_INDEX_BYTES * level;
        let position = out.len() as u64;
        out[entry..entry + 8].copy_from_slice(&position.to_le_bytes());
        out[entry + 8..entry + 16].copy_from_slice(&(len as u64).to_le_bytes());
        out[entry + 16..entry + 24].copy_from_slice(&(len as u64).to_le_bytes());
        out.extend_from_slice(&texture.data[offset..offset + len]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(format: TextureFormat, size: u32) -> TextureData {
        let levels: Vec<Vec<u8>> = (0..full_mip_chain(size, size))
            .map(|level| {
                let (w, h) = mip_size(size, size, level);
                let len = format.level_bytes(w, h).expect("size");
                (0..len)
                    .map(|index| (index * 7 + level as usize) as u8)
                    .collect()
            })
            .collect();
        TextureData::new(format, size, size, &levels).expect("valid texture")
    }

    #[test]
    fn formats_round_trip_and_know_their_sizes() {
        for &format in TextureFormat::ALL {
            assert_eq!(
                TextureFormat::from_vk_format(format.vk_format()),
                Some(format)
            );
            assert_eq!(TextureFormat::from_name(format.name()), Some(format));
            assert_eq!(format.uncompressed().channels(), format.channels());
            assert_eq!(format.uncompressed().is_srgb(), format.is_srgb());
        }
        assert_eq!(TextureFormat::from_vk_format(0), None);
        assert_eq!(TextureFormat::Bc7Srgb.level_bytes(1, 1), Some(16));
        assert_eq!(TextureFormat::Bc7Srgb.level_bytes(5, 4), Some(32));
        assert_eq!(TextureFormat::Bc4Unorm.level_bytes(8, 8), Some(32));
        assert_eq!(TextureFormat::Rg8Unorm.level_bytes(3, 2), Some(12));
        assert_eq!(full_mip_chain(256, 64), 9);
        assert_eq!(full_mip_chain(1, 1), 1);
        assert_eq!(mip_size(256, 64, 8), (1, 1));
        assert_eq!(MAX_MIP_LEVELS, 13);
    }

    #[test]
    fn written_files_load_back_identically() {
        for &format in TextureFormat::ALL {
            let texture = checker(format, 64);
            let bytes = write_ktx2(&texture, "moba test");
            assert_eq!(load_ktx2(&bytes).as_ref(), Ok(&texture), "{format}");
        }
        // Non-square, non-multiple-of-four sizes with a partial chain.
        let levels = vec![vec![1u8; 6 * 3 * 4], vec![2u8; 3 * 4]];
        let texture = TextureData::new(TextureFormat::Rgba8Srgb, 6, 3, &levels).expect("valid");
        assert_eq!(load_ktx2(&write_ktx2(&texture, "")), Ok(texture));
    }

    #[test]
    fn construction_checks_every_level() {
        let format = TextureFormat::Bc5Unorm;
        assert_eq!(
            TextureData::new(format, 0, 4, &[vec![0; 16]]),
            Err(TextureError::BadDimensions)
        );
        assert_eq!(
            TextureData::new(format, MAX_TEXTURE_DIMENSION + 1, 4, &[vec![0; 16]]),
            Err(TextureError::BadDimensions)
        );
        assert_eq!(
            TextureData::new(format, 4, 4, &[]),
            Err(TextureError::BadLevelCount)
        );
        assert_eq!(
            TextureData::new(
                format,
                4,
                4,
                &[vec![0; 16], vec![0; 16], vec![0; 16], vec![0; 16]]
            ),
            Err(TextureError::BadLevelCount)
        );
        assert_eq!(
            TextureData::new(format, 4, 4, &[vec![0; 15]]),
            Err(TextureError::BadLevel)
        );
        let texture = TextureData::new(format, 8, 4, &[vec![1; 32], vec![2; 16], vec![3; 16]])
            .expect("full chain");
        assert_eq!(texture.level_count(), 3);
        assert_eq!(texture.level(1), Some((4, 2, &[2u8; 16][..])));
        assert_eq!(texture.level(3), None);
        assert_eq!(texture.level_offset(2), Some(48));
    }

    /// Writes a little-endian u32 into `bytes` at `offset`.
    fn put(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn hostile_headers_are_rejected_before_allocation() {
        let good = write_ktx2(&checker(TextureFormat::Bc7Srgb, 16), "x");
        assert!(load_ktx2(&good).is_ok());
        let mutate = |offset: usize, value: u32| {
            let mut bytes = good.clone();
            put(&mut bytes, offset, value);
            load_ktx2(&bytes)
        };
        assert_eq!(load_ktx2(&good[..40]), Err(TextureError::NotKtx2));
        let mut wrong = good.clone();
        wrong[1] = b'X';
        assert_eq!(load_ktx2(&wrong), Err(TextureError::NotKtx2));
        assert_eq!(mutate(12, 44), Err(TextureError::UnsupportedFormat(44)));
        assert!(matches!(mutate(16, 4), Err(TextureError::Unsupported(_))));
        assert_eq!(mutate(20, 0), Err(TextureError::BadDimensions));
        assert_eq!(mutate(24, 1 << 20), Err(TextureError::BadDimensions));
        assert!(matches!(mutate(28, 2), Err(TextureError::Unsupported(_))));
        assert!(matches!(mutate(32, 6), Err(TextureError::Unsupported(_))));
        assert!(matches!(mutate(36, 2), Err(TextureError::Unsupported(_))));
        // Six faces with one face's bytes per level is a length mismatch.
        assert_eq!(mutate(36, 6), Err(TextureError::BadLevel));
        assert_eq!(mutate(40, 6), Err(TextureError::BadLevelCount));
        assert_eq!(mutate(40, u32::MAX), Err(TextureError::BadLevelCount));
        assert!(matches!(mutate(44, 2), Err(TextureError::Unsupported(_))));
        // Descriptor out of the file, or claiming a linear transfer for an sRGB format.
        assert_eq!(mutate(48, u32::MAX), Err(TextureError::BadDescriptor));
        assert_eq!(mutate(52, 1 << 30), Err(TextureError::BadDescriptor));
        let dfd_offset = u32_at(&good, 48).unwrap() as usize;
        let mut linear = good.clone();
        linear[dfd_offset + 4 + 10] = dfd::TRANSFER_LINEAR;
        assert_eq!(load_ktx2(&linear), Err(TextureError::BadDescriptor));
        // A level past the end, a level with the wrong length, a huge offset.
        let level0 = HEADER_BYTES;
        assert_eq!(mutate(level0, u32::MAX), Err(TextureError::BadLevel));
        assert_eq!(mutate(level0 + 8, 17), Err(TextureError::BadLevel));
        let mut huge = good.clone();
        huge[level0..level0 + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(load_ktx2(&huge), Err(TextureError::BadLevel));
        // Truncation anywhere, or any single flipped bit, never panics; whatever loads still
        // satisfies the level invariants.
        for cut in 0..good.len() {
            let _ = load_ktx2(&good[..cut]);
        }
        for byte in 0..good.len() {
            for bit in 0..8 {
                let mut flipped = good.clone();
                flipped[byte] ^= 1 << bit;
                if let Ok(texture) = load_ktx2(&flipped) {
                    let total: usize = (0..texture.level_count())
                        .map(|level| texture.level(level).expect("level").2.len())
                        .sum();
                    assert_eq!(total, texture.data().len());
                }
            }
        }
        // Level 1 aliasing level 0's bytes (or overlapping them) is rejected.
        let level0_offset = u64_at(&good, level0).unwrap();
        let mut aliased = good.clone();
        aliased[level0 + LEVEL_INDEX_BYTES..level0 + LEVEL_INDEX_BYTES + 8]
            .copy_from_slice(&level0_offset.to_le_bytes());
        assert_eq!(load_ktx2(&aliased), Err(TextureError::BadLevel));
        let mut overlapping = good.clone();
        overlapping[level0 + LEVEL_INDEX_BYTES..level0 + LEVEL_INDEX_BYTES + 8]
            .copy_from_slice(&(level0_offset + 16).to_le_bytes());
        assert_eq!(load_ktx2(&overlapping), Err(TextureError::BadLevel));
        // levelCount 0 ("generate at load") reads only level 0.
        let one = mutate(40, 0).expect("level 0 only");
        assert_eq!(one.level_count(), 1);
    }

    fn cube(format: TextureFormat, size: u32) -> TextureData {
        let levels: Vec<Vec<u8>> = (0..full_mip_chain(size, size))
            .map(|level| {
                let (w, h) = mip_size(size, size, level);
                let len = format.level_bytes(w, h).expect("size") * 6;
                (0..len)
                    .map(|index| (index * 13 + level as usize * 3) as u8)
                    .collect()
            })
            .collect();
        TextureData::new_cube(format, size, &levels).expect("valid cube")
    }

    #[test]
    fn half_floats_round_trip_and_round_to_nearest_even() {
        for value in [
            0.0f32,
            -0.0,
            1.0,
            -2.5,
            0.5,
            65_504.0,
            6.103_515_6e-5,
            5.960_464_5e-8,
        ] {
            assert_eq!(f32_from_f16(f16_from_f32(value)), value, "{value}");
        }
        assert_eq!(f16_from_f32(1.0), 0x3c00);
        assert_eq!(f16_from_f32(-2.0), 0xc000);
        assert_eq!(f16_from_f32(65_504.0), 0x7bff);
        assert_eq!(f16_from_f32(65_520.0), 0x7c00, "rounds up to infinity");
        assert_eq!(f16_from_f32(1.0e9), 0x7c00);
        assert_eq!(f16_from_f32(f32::NEG_INFINITY), 0xfc00);
        assert!(f32_from_f16(f16_from_f32(f32::NAN)).is_nan());
        assert_eq!(f16_from_f32(5.960_464_5e-8), 0x0001, "smallest subnormal");
        assert_eq!(
            f16_from_f32(2.980_232_2e-8),
            0x0000,
            "half of it ties to even zero"
        );
        assert_eq!(f16_from_f32(1.0e-10), 0x0000);
        assert_eq!(f16_from_f32(-1.0e-10), 0x8000);
        // 1 + 2^-11 is halfway between 1 and the next half: ties to even (1.0).
        assert_eq!(f16_from_f32(1.0 + 1.0 / 2048.0), 0x3c00);
        assert_eq!(f16_from_f32(1.0 + 3.0 / 2048.0), 0x3c02);
        assert_eq!(f32_from_f16(0x7c00), f32::INFINITY);
        assert_eq!(f32_from_f16(0x03ff), 1023.0 / 16_777_216.0);
        // Every finite half survives f32 and back.
        for bits in 0..=u16::MAX {
            if bits & 0x7c00 != 0x7c00 {
                assert_eq!(f16_from_f32(f32_from_f16(bits)), bits, "{bits:#06x}");
            }
        }
    }

    #[test]
    fn cube_maps_round_trip_and_expose_faces() {
        for format in [
            TextureFormat::Rgba16Float,
            TextureFormat::Rgba8Srgb,
            TextureFormat::Bc7Srgb,
        ] {
            let texture = cube(format, 16);
            assert!(texture.is_cube());
            assert_eq!(texture.face_count(), 6);
            let bytes = write_ktx2(&texture, "cube");
            assert_eq!(u32_at(&bytes, 36), Some(6), "faceCount");
            assert_eq!(load_ktx2(&bytes).as_ref(), Ok(&texture), "{format}");
            let face_len = format.level_bytes(16, 16).expect("size");
            let (_, _, level0) = texture.level(0).expect("level 0");
            assert_eq!(level0.len(), 6 * face_len);
            assert_eq!(
                texture.face(0, 2),
                Some(&level0[2 * face_len..3 * face_len])
            );
            assert_eq!(texture.face(0, 6), None);
        }
        let flat = checker(TextureFormat::Rgba16Float, 8);
        assert!(!flat.is_cube());
        assert_eq!(flat.face(0, 0), flat.level(0).map(|(_, _, bytes)| bytes));
        assert_eq!(flat.face(0, 1), None);
        // Constructors reject non-cube shapes and wrong face counts.
        assert_eq!(
            TextureData::new_cube(TextureFormat::Rgba8Unorm, 4, &[vec![0; 4 * 4 * 4]]),
            Err(TextureError::BadLevel)
        );
        assert_eq!(
            TextureData::new_cube(
                TextureFormat::Rgba8Unorm,
                MAX_CUBE_DIMENSION * 2,
                &[Vec::new()]
            ),
            Err(TextureError::BadDimensions)
        );
    }

    #[test]
    fn hostile_cube_headers_are_rejected() {
        let good = write_ktx2(&cube(TextureFormat::Rgba16Float, 8), "x");
        assert!(load_ktx2(&good).is_ok());
        let mutate = |offset: usize, value: u32| {
            let mut bytes = good.clone();
            put(&mut bytes, offset, value);
            load_ktx2(&bytes)
        };
        // Non-square faces, an oversized cube, layered cubes, other face counts.
        assert_eq!(mutate(24, 4), Err(TextureError::BadDimensions));
        assert_eq!(mutate(20, 2048), Err(TextureError::BadDimensions));
        assert!(matches!(mutate(32, 6), Err(TextureError::Unsupported(_))));
        assert!(matches!(mutate(36, 3), Err(TextureError::Unsupported(_))));
        // One face's worth of data per level.
        assert_eq!(mutate(36, 1), Err(TextureError::BadLevel));
        let mut oversized = good.clone();
        put(&mut oversized, 20, MAX_CUBE_DIMENSION + 4);
        put(&mut oversized, 24, MAX_CUBE_DIMENSION + 4);
        assert_eq!(load_ktx2(&oversized), Err(TextureError::BadDimensions));
        for cut in 0..good.len() {
            let _ = load_ktx2(&good[..cut]);
        }
        for byte in 0..good.len() {
            for bit in 0..8 {
                let mut flipped = good.clone();
                flipped[byte] ^= 1 << bit;
                if let Ok(texture) = load_ktx2(&flipped) {
                    let mut total = 0;
                    for level in 0..texture.level_count() {
                        let (w, h, bytes) = texture.level(level).expect("level");
                        let face = texture.format().level_bytes(w, h).expect("size");
                        assert_eq!(bytes.len(), face * texture.face_count() as usize);
                        total += bytes.len();
                    }
                    assert_eq!(total, texture.data().len());
                    assert!(texture.face_count() == 1 || texture.width() == texture.height());
                }
            }
        }
    }

    #[cfg(feature = "bc-decode")]
    #[test]
    fn cube_maps_decode_face_by_face() {
        let texture = cube(TextureFormat::Bc4Unorm, 8);
        let decoded = texture.decode_to_uncompressed().expect("decodes");
        assert!(decoded.is_cube());
        assert_eq!(decoded.format(), TextureFormat::R8Unorm);
        assert_eq!(decoded.face(0, 5).map(<[u8]>::len), Some(64));
        assert_eq!(decoded.level_count(), texture.level_count());
    }

    #[test]
    fn written_levels_are_aligned_and_smallest_first() {
        let texture = checker(TextureFormat::Bc4Unorm, 32);
        let bytes = write_ktx2(&texture, "x");
        let offsets: Vec<u64> = (0..texture.level_count() as usize)
            .map(|level| u64_at(&bytes, HEADER_BYTES + LEVEL_INDEX_BYTES * level).unwrap())
            .collect();
        assert!(offsets.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(offsets.iter().all(|offset| offset % 8 == 0));
    }

    #[cfg(feature = "bc-decode")]
    #[test]
    fn bc_textures_decode_to_their_uncompressed_format() {
        // A BC4 block with endpoints 255/0 and every index 0 decodes to 255 everywhere; a
        // 6x6 level has partial edge blocks.
        let block = [255u8, 0, 0, 0, 0, 0, 0, 0];
        let level0: Vec<u8> = block.repeat(4);
        let levels = vec![level0, block.to_vec(), block.to_vec()];
        let texture = TextureData::new(TextureFormat::Bc4Unorm, 6, 6, &levels).expect("valid");
        let decoded = texture.decode_to_uncompressed().expect("decodes");
        assert_eq!(decoded.format(), TextureFormat::R8Unorm);
        assert_eq!(
            decoded.level(0).map(|(w, h, d)| (w, h, d.len())),
            Some((6, 6, 36))
        );
        assert!(decoded.data().iter().all(|&value| value == 255));
        assert_eq!(decoded.level_count(), 3);
        // BC7 mode 6 block, both endpoints white, opaque: decodes to (255, 255, 255, 255).
        let mut bc7 = [0u8; 16];
        bc7[0] = 0b0100_0000; // mode 6
        // Endpoints are 7-bit + p-bit; set every endpoint bit and both p-bits.
        for bit in 7..7 + 56 + 2 {
            bc7[bit / 8] |= 1 << (bit % 8);
        }
        let texture =
            TextureData::new(TextureFormat::Bc7Srgb, 4, 4, &[bc7.to_vec()]).expect("valid");
        let decoded = texture.decode_to_uncompressed().expect("decodes");
        assert_eq!(decoded.format(), TextureFormat::Rgba8Srgb);
        assert!(
            decoded.data().iter().all(|&value| value == 255),
            "{:?}",
            decoded.data()
        );
    }
}
