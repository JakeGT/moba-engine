//! Offline texture baker: PNG → KTX2 with a mip chain (ADR-0010).
//!
//! ```text
//! texture-bake bake <in.png> <out.ktx2> --kind color|normal|data|mask [--format F] [--no-mips]
//! texture-bake generate sentinel <dir>     # the self-generated sample set, as PNG sources
//! texture-bake generate vfx <dir>          # the procedural particle atlas (ADR-0013)
//! texture-bake info <file.ktx2>            # format, size, levels, bytes
//! ```
//!
//! `--kind` fixes the colour space and the default format: `color` (sRGB, `bc7-srgb`),
//! `normal` (tangent XY in RG, `bc5`), `data` (linear RG such as metallic/roughness, `bc5`)
//! and `mask` (linear R, `bc4`). `--format` may pick the kind's 8-bit fallback instead
//! (`rgba8-srgb`, `rg8`, `r8`). Mips are filtered per kind (linear-light colour,
//! renormalised normals); see `mips.rs`. A baked BC file reports its level-0 PSNR against
//! the source, decoded with the same decoder the runtime fallback uses.
#![allow(clippy::print_stdout)]

mod bc;
mod mips;
mod patterns;

use std::path::Path;
use std::process::ExitCode;

use mips::Kind;
use moba_assets::{TextureData, TextureFormat};

const USAGE: &str = "usage:
  texture-bake bake <in.png> <out.ktx2> --kind color|normal|data|mask [--format F] [--no-mips]
  texture-bake generate sentinel|vfx <dir>
  texture-bake info <file.ktx2>";

/// Largest PNG accepted, in bytes (sources are authoring files, still bounded).
const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;
/// Written as the KTX2 `KTXwriter` value.
const WRITER: &str = concat!("moba-tools texture-bake ", env!("CARGO_PKG_VERSION"));

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bake") => bake(&args[1..]),
        Some("generate") => generate(&args[1..]),
        Some("info") => info(&args[1..]),
        _ => Err(String::from(USAGE)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("texture-bake: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Decodes a PNG to tightly packed RGBA8.
fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.len() > MAX_PNG_BYTES {
        return Err(format!(
            "{}: larger than {MAX_PNG_BYTES} bytes",
            path.display()
        ));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > moba_assets::MAX_TEXTURE_DIMENSION || height > moba_assets::MAX_TEXTURE_DIMENSION {
        return Err(format!(
            "{}: {width}x{height} exceeds {} texels",
            path.display(),
            moba_assets::MAX_TEXTURE_DIMENSION
        ));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{}: image too large", path.display()))?;
    let mut buffer = vec![0; size];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    buffer.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return Err(format!("{}: unexpanded palette", path.display())),
    };
    Ok((width, height, rgba))
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file =
        std::fs::File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Encodes one level of `kind` texels to `format`.
fn encode(texels: &[u8], width: u32, height: u32, format: TextureFormat) -> Vec<u8> {
    if format.is_block_compressed() {
        bc::encode_level(texels, width, height, format.channels() as usize)
    } else {
        texels.to_vec()
    }
}

/// Bakes RGBA8 `rgba` into a texture of `kind`/`format` with the full mip chain (or one level).
pub(crate) fn bake_texture(
    rgba: &[u8],
    width: u32,
    height: u32,
    kind: Kind,
    format: TextureFormat,
    mips: bool,
) -> Result<TextureData, String> {
    let mut texels = mips::extract(rgba, kind);
    let levels = if mips {
        moba_assets::full_mip_chain(width, height)
    } else {
        1
    };
    let mut encoded = Vec::with_capacity(levels as usize);
    let (mut w, mut h) = (width, height);
    for level in 0..levels {
        encoded.push(encode(&texels, w, h, format));
        if level + 1 < levels {
            texels = mips::downsample(&texels, w, h, kind);
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
    }
    TextureData::new(format, width, height, &encoded).map_err(|error| error.to_string())
}

/// PSNR (dB) of a baked texture's level 0 against the kind's source texels.
fn level0_psnr(texture: &TextureData, source: &[u8]) -> Option<f64> {
    let decoded = texture.decode_to_uncompressed().ok()?;
    let (_, _, bytes) = decoded.level(0)?;
    let mse = bytes
        .iter()
        .zip(source)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
        .sum::<f64>()
        / bytes.len().max(1) as f64;
    Some(if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    })
}

fn bake(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut kind = None;
    let mut format = None;
    let mut mips = true;
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--kind" => {
                let value = args.next().ok_or("--kind needs a value")?;
                kind = Some(Kind::from_name(value).ok_or_else(|| format!("unknown kind {value}"))?);
            }
            "--format" => {
                let value = args.next().ok_or("--format needs a value")?;
                format = Some(
                    TextureFormat::from_name(value)
                        .ok_or_else(|| format!("unknown format {value}"))?,
                );
            }
            "--no-mips" => mips = false,
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => positional.push(other),
        }
    }
    let [input, output] = positional[..] else {
        return Err(String::from(USAGE));
    };
    let kind = kind.ok_or("--kind is required (color, normal, data or mask)")?;
    let format = format.unwrap_or(kind.default_format());
    if !kind.allows(format) {
        return Err(format!(
            "format {format} does not hold {kind:?} data (use {} or {})",
            kind.default_format(),
            kind.default_format().uncompressed()
        ));
    }
    let (width, height, rgba) = read_png(Path::new(input))?;
    let texture = bake_texture(&rgba, width, height, kind, format, mips)?;
    let bytes = moba_assets::write_ktx2(&texture, WRITER);
    // Round-trip through the runtime loader before writing, so a shipped file always loads.
    let reloaded =
        moba_assets::load_ktx2(&bytes).map_err(|error| format!("self-check: {error}"))?;
    if reloaded != texture {
        return Err(String::from("self-check: reloaded texture differs"));
    }
    std::fs::write(output, &bytes).map_err(|error| format!("{output}: {error}"))?;
    let quality = if format.is_block_compressed() {
        level0_psnr(&texture, &mips::extract(&rgba, kind))
            .map_or_else(String::new, |psnr| format!(" psnr={psnr:.2}dB"))
    } else {
        String::new()
    };
    println!(
        "{output}: {format} {width}x{height} levels={} bytes={}{quality}",
        texture.level_count(),
        bytes.len()
    );
    Ok(())
}

fn generate(args: &[String]) -> Result<(), String> {
    let [set, dir] = args else {
        return Err(String::from(USAGE));
    };
    if set == "vfx" {
        return generate_vfx_atlas(Path::new(dir));
    }
    if set != "sentinel" {
        return Err(format!("unknown set {set} (known: sentinel, vfx)"));
    }
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for map in ["base", "normal", "mr", "mask"] {
        let rgba = patterns::sentinel(map).expect("known map");
        let path = dir.join(format!("sentinel-{map}.png"));
        write_png(&path, patterns::SIZE, patterns::SIZE, &rgba)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// The procedural particle atlas (ADR-0013) as an RGBA PNG source: R coverage, G hot core.
fn generate_vfx_atlas(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let atlas = moba_assets::procedural_atlas();
    let (width, height, texels) = atlas.level(0).ok_or("the atlas has no level 0")?;
    let rgba: Vec<u8> = texels
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|&[coverage, core]| [coverage, core, 0, 255])
        .collect();
    let path = dir.join("vfx-atlas.png");
    write_png(&path, width, height, &rgba)?;
    println!("{}", path.display());
    Ok(())
}

fn info(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err(String::from(USAGE));
    };
    let texture =
        moba_assets::load_ktx2_file(Path::new(path)).map_err(|error| format!("{path}: {error}"))?;
    // The CPU fallback's cost for this file (devices without textureCompressionBC).
    let decode = if texture.format().is_block_compressed() {
        let started = std::time::Instant::now();
        texture
            .decode_to_uncompressed()
            .map_err(|error| format!("{path}: {error}"))?;
        format!(
            " cpu_decode_ms={:.3}",
            started.elapsed().as_secs_f64() * 1_000.0
        )
    } else {
        String::new()
    };
    println!(
        "{path}: {} {}x{} levels={} texel_bytes={}{decode}",
        texture.format(),
        texture.width(),
        texture.height(),
        texture.level_count(),
        texture.data().len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_set_bakes_to_loadable_ktx2_at_good_quality() {
        for (map, kind) in [
            ("base", Kind::Color),
            ("normal", Kind::Normal),
            ("mr", Kind::Data),
            ("mask", Kind::Mask),
        ] {
            let rgba = patterns::sentinel(map).expect("map");
            let size = patterns::SIZE;
            let texture =
                bake_texture(&rgba, size, size, kind, kind.default_format(), true).expect("bakes");
            assert_eq!(texture.level_count(), 9);
            let bytes = moba_assets::write_ktx2(&texture, WRITER);
            assert_eq!(moba_assets::load_ktx2(&bytes).as_ref(), Ok(&texture));
            let psnr = level0_psnr(&texture, &mips::extract(&rgba, kind)).expect("decodes");
            assert!(psnr > 36.0, "{map}: {psnr:.2} dB");
            let uncompressed = bake_texture(
                &rgba,
                size,
                size,
                kind,
                kind.default_format().uncompressed(),
                false,
            )
            .expect("bakes");
            assert_eq!(uncompressed.level_count(), 1);
            assert_eq!(uncompressed.data(), mips::extract(&rgba, kind));
        }
        assert!(
            !Kind::Color.allows(TextureFormat::Bc7Unorm),
            "colour is sRGB"
        );
        assert!(!Kind::Normal.allows(TextureFormat::Bc4Unorm));
    }

    #[test]
    fn committed_sample_textures_match_the_generator() {
        // Level 0 uses only correctly rounded IEEE operations (no libm), so it must match the
        // generator bit for bit; the mips go through `powf` (sRGB) and only have to load.
        for (map, kind, committed) in [
            (
                "base",
                Kind::Color,
                &include_bytes!("../../../../../content/textures/sentinel-base.ktx2")[..],
            ),
            (
                "normal",
                Kind::Normal,
                &include_bytes!("../../../../../content/textures/sentinel-normal.ktx2")[..],
            ),
            (
                "mr",
                Kind::Data,
                &include_bytes!("../../../../../content/textures/sentinel-mr.ktx2")[..],
            ),
            (
                "mask",
                Kind::Mask,
                &include_bytes!("../../../../../content/textures/sentinel-mask.ktx2")[..],
            ),
        ] {
            let committed = moba_assets::load_ktx2(committed).expect("committed texture loads");
            let rgba = patterns::sentinel(map).expect("map");
            let size = patterns::SIZE;
            let level0 =
                bake_texture(&rgba, size, size, kind, kind.default_format(), false).expect("bakes");
            assert_eq!(committed.format(), kind.default_format(), "{map}");
            assert_eq!(committed.level_count(), 9, "{map}");
            assert!(
                committed.level(0) == level0.level(0),
                "content/textures/sentinel-{map}.ktx2 is stale; run scripts/bake-sample-textures.sh"
            );
        }
    }

    /// The shipped particle atlas is the BC5 bake of the procedural one. The generator uses
    /// libm (`exp`, `atan2`, ...), so the comparison allows its last-bit differences.
    #[test]
    fn committed_vfx_atlas_matches_the_generator() {
        let committed =
            moba_assets::load_ktx2(include_bytes!("../../../../../content/vfx/atlas.ktx2"))
                .expect("committed atlas loads");
        let atlas = moba_assets::procedural_atlas();
        assert_eq!(committed.format(), TextureFormat::Bc5Unorm);
        assert_eq!(
            (
                committed.width(),
                committed.height(),
                committed.level_count()
            ),
            (atlas.width(), atlas.height(), atlas.level_count())
        );
        let (_, _, source) = atlas.level(0).expect("level 0");
        let psnr = level0_psnr(&committed, source).expect("decodes");
        assert!(
            psnr > 40.0,
            "content/vfx/atlas.ktx2 is stale ({psnr:.1} dB); run scripts/bake-sample-textures.sh"
        );
    }
}
