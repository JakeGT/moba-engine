//! Offline environment baker for image-based lighting (R2a).
//!
//! ```text
//! env-bake bake --preset meadow|ember|frost [--size 64] [--samples 256] [sky flags] <out-stem>
//! env-bake info <file.ktx2>
//! ```
//!
//! `bake` evaluates an analytic sky (`sky.rs`: zenith/horizon/ground gradient plus a broad
//! sun glow, no sun disk because the shading's sun provides the direct light) and writes:
//!
//! - `<out-stem>.ktx2`: the prefiltered specular cube map, `RGBA16F`, `size`² faces and the
//!   full mip chain; level `m` of `M` is GGX-prefiltered (split-sum, `N = V = R`) for
//!   roughness `m / (M - 1)`, level 0 is the sky itself;
//! - `<out-stem>.toml`: the `moba_assets::EnvironmentDesc` with the cosine-convolved SH-L2
//!   diffuse ambient (divided by π), `intensity = 1` and `specular_mips = M`, after header
//!   comments recording the tool, preset and every sky parameter.
//!
//! Sky flags override the preset: `--zenith r,g,b`, `--horizon r,g,b`, `--ground r,g,b`,
//! `--sun-dir x,y,z`, `--sun-color r,g,b` (linear RGB, may exceed 1) and `--sun-glow <exp>`.
//! The output is deterministic for a given platform. `info` prints the format, faces, levels
//! and each face's average colour per level.
#![allow(clippy::print_stdout)]

mod bake;
mod sky;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use moba_assets::{EnvironmentDesc, f32_from_f16};
use sky::Sky;

const USAGE: &str = "usage:
  env-bake bake --preset meadow|ember|frost [--size N] [--samples N] [--zenith r,g,b]
                [--horizon r,g,b] [--ground r,g,b] [--sun-dir x,y,z] [--sun-color r,g,b]
                [--sun-glow EXP] <out-stem>
  env-bake info <file.ktx2>";

/// Written as the KTX2 `KTXwriter` value.
const WRITER: &str = concat!("moba-tools env-bake ", env!("CARGO_PKG_VERSION"));
/// Default face edge (a 64² cube has 7 levels, ~262 KiB as RGBA16F).
const DEFAULT_SIZE: u32 = 64;
/// Default GGX samples per texel.
const DEFAULT_SAMPLES: u32 = 256;
/// Edge of the cube the SH projection integrates over (independent of the output size).
const SH_SIZE: u32 = 64;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bake") => bake_command(&args[1..]),
        Some("info") => info(&args[1..]),
        _ => Err(String::from(USAGE)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("env-bake: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_v3(flag: &str, value: &str) -> Result<sky::V3, String> {
    let parts: Vec<f32> = value
        .split(',')
        .map(|part| part.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{flag} takes three comma-separated numbers"))?;
    match parts[..] {
        [a, b, c]
            if parts
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 64.0) =>
        {
            Ok([a, b, c])
        }
        _ => Err(format!("{flag} takes three finite numbers (|v| <= 64)")),
    }
}

/// Parsed `bake` arguments.
struct BakeArgs {
    preset: String,
    sky: Sky,
    size: u32,
    samples: u32,
    output: PathBuf,
}

fn parse_bake(args: &[String]) -> Result<BakeArgs, String> {
    let mut preset = None;
    let mut overrides: Vec<(String, String)> = Vec::new();
    let mut size = DEFAULT_SIZE;
    let mut samples = DEFAULT_SAMPLES;
    let mut output = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--preset" => preset = Some(value()?),
            "--size" => {
                size = value()?
                    .parse()
                    .ok()
                    .filter(|size: &u32| size.is_power_of_two() && (4..=512).contains(size))
                    .ok_or("--size must be a power of two in 4..=512")?;
            }
            "--samples" => {
                samples = value()?
                    .parse()
                    .ok()
                    .filter(|samples| (16..=4096).contains(samples))
                    .ok_or("--samples must be 16..=4096")?;
            }
            "--zenith" | "--horizon" | "--ground" | "--sun-dir" | "--sun-color" | "--sun-glow" => {
                overrides.push((arg.clone(), value()?));
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            path if output.is_none() => output = Some(PathBuf::from(path)),
            _ => return Err(String::from(USAGE)),
        }
    }
    let preset = preset.unwrap_or_else(|| String::from("meadow"));
    let mut sky = Sky::preset(&preset)
        .ok_or_else(|| format!("unknown preset {preset} (one of {:?})", sky::PRESETS))?;
    for (flag, value) in overrides {
        match flag.as_str() {
            "--zenith" => sky.zenith = parse_v3(&flag, &value)?,
            "--horizon" => sky.horizon = parse_v3(&flag, &value)?,
            "--ground" => sky.ground = parse_v3(&flag, &value)?,
            "--sun-dir" => sky.sun_dir = parse_v3(&flag, &value)?,
            "--sun-color" => sky.sun_color = parse_v3(&flag, &value)?,
            _ => {
                sky.sun_glow = value
                    .parse()
                    .ok()
                    .filter(|glow: &f32| glow.is_finite() && (1.0..=4096.0).contains(glow))
                    .ok_or("--sun-glow must be 1..=4096")?;
            }
        }
    }
    Ok(BakeArgs {
        preset,
        sky,
        size,
        samples,
        output: output.ok_or_else(|| String::from(USAGE))?,
    })
}

/// The descriptor and cube map for `args` (no I/O).
fn bake(args: &BakeArgs) -> Result<(String, Vec<u8>), String> {
    let sun = args.sky.sun();
    let radiance = |d| args.sky.radiance(d, sun);
    let sh = bake::project_sh(&radiance, SH_SIZE);
    let levels = bake::prefilter_levels(&radiance, args.size, args.samples);
    let mips = u32::try_from(levels.len()).map_err(|_| "too many levels")?;
    let texture = bake::to_texture(&levels, args.size)?;
    let desc = EnvironmentDesc::new(sh).with_specular_mips(mips);
    let mut header = vec![
        format!("Environment baked by {WRITER} (R2a image-based lighting)."),
        String::from("Client-local and cosmetic: never hashed or sent. Regenerate with"),
        String::from("scripts/bake-sample-environments.sh; see docs/content-authoring.md."),
        format!(
            "preset = {:?}, size = {}, samples = {}",
            args.preset, args.size, args.samples
        ),
    ];
    header.extend(args.sky.describe());
    header.push(String::from(
        "sh: cosine-convolved SH-L2 / pi, linear RGB, order Y00 Y1-1 Y10 Y11 Y2-2 Y2-1 Y20 Y21 Y22",
    ));
    let text = desc.to_toml(&header);
    // The written descriptor must load back (and fit the loader's size cap).
    EnvironmentDesc::parse(&text).map_err(|error| format!("baked descriptor invalid: {error}"))?;
    Ok((text, moba_assets::write_ktx2(&texture, WRITER)))
}

fn bake_command(args: &[String]) -> Result<(), String> {
    let args = parse_bake(args)?;
    let (text, bytes) = bake(&args)?;
    let toml = args.output.with_extension("toml");
    let ktx2 = args.output.with_extension("ktx2");
    std::fs::write(&toml, &text).map_err(|error| format!("{}: {error}", toml.display()))?;
    std::fs::write(&ktx2, &bytes).map_err(|error| format!("{}: {error}", ktx2.display()))?;
    println!(
        "{}: rgba16f cube {}², {} bytes; {}",
        ktx2.display(),
        args.size,
        bytes.len(),
        toml.display()
    );
    Ok(())
}

fn info(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err(String::from(USAGE));
    };
    let path = Path::new(path);
    let texture = moba_assets::load_ktx2_file(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    println!(
        "{}: {} {}x{}, {} face(s), {} level(s), {} bytes",
        path.display(),
        texture.format(),
        texture.width(),
        texture.height(),
        texture.face_count(),
        texture.level_count(),
        texture.data().len()
    );
    if texture.format() != moba_assets::TextureFormat::Rgba16Float {
        return Ok(());
    }
    for level in 0..texture.level_count() {
        let mut line = format!("  level {level}:");
        for face in 0..texture.face_count() {
            let bytes = texture.face(level, face).unwrap_or_default();
            let mut sum = [0.0f32; 3];
            let texels = bytes.len() / 8;
            for texel in bytes.as_chunks::<8>().0 {
                for (channel, total) in sum.iter_mut().enumerate() {
                    let half = u16::from_le_bytes([texel[channel * 2], texel[channel * 2 + 1]]);
                    *total += f32_from_f16(half);
                }
            }
            let mean = sum.map(|value| value / texels.max(1) as f32);
            let name = if texture.is_cube() {
                bake::FACE_NAMES[face as usize]
            } else {
                "2D"
            };
            let _ = write!(
                line,
                " {name} [{:.3}, {:.3}, {:.3}]",
                mean[0], mean[1], mean[2]
            );
        }
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn flags_override_the_preset_and_bad_input_is_rejected() {
        let parsed = parse_bake(&args(&[
            "--preset",
            "ember",
            "--zenith",
            "0.1,0.2,0.3",
            "--sun-glow",
            "12",
            "out/x",
        ]))
        .expect("valid");
        assert_eq!(parsed.sky.zenith, [0.1, 0.2, 0.3]);
        assert_eq!(parsed.sky.sun_glow, 12.0);
        assert_eq!(parsed.sky.ground, Sky::preset("ember").unwrap().ground);
        for bad in [
            &["--preset", "lava", "x"][..],
            &["--size", "48", "x"],
            &["--zenith", "1,2", "x"],
            &["--zenith", "nan,0,0", "x"],
            &["--sun-glow", "0", "x"],
            &["--bogus", "x"],
            &["--preset", "meadow"],
        ] {
            assert!(parse_bake(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn baked_files_round_trip_through_the_loaders() {
        let parsed = parse_bake(&args(&["--size", "8", "--samples", "32", "env"])).expect("ok");
        let (text, bytes) = bake(&parsed).expect("bakes");
        let dir = std::env::temp_dir().join(format!("moba-env-bake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("env.toml"), &text).expect("write");
        let desc = EnvironmentDesc::load_file(&dir.join("env.toml")).expect("descriptor");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(desc.specular_mips, 4);
        let texture = moba_assets::load_ktx2(&bytes).expect("cube map");
        assert!(texture.is_cube() && texture.level_count() == 4 && texture.width() == 8);
        // The meadow sky is brighter overhead than underfoot, and blue-ish overhead.
        let up = desc.irradiance_over_pi([0.0, 1.0, 0.0]);
        let down = desc.irradiance_over_pi([0.0, -1.0, 0.0]);
        assert!(up[2] > down[2] && up.iter().sum::<f32>() > down.iter().sum::<f32>());
        assert!(text.starts_with("# Environment baked by moba-tools env-bake"));
    }
}
