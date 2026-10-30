//! Offline colour-grading LUT baker (R6 post-processing).
//!
//! ```text
//! grade-bake bake [--preset NAME] [--size N] [grade flags] <out.cube>
//! grade-bake info <file.cube>
//! grade-bake apply <file.cube> <in.png> <out.png>
//! ```
//!
//! `bake` evaluates a deterministic grade (`grade.rs`: lift/gamma/gain, contrast, saturation
//! and white balance, all on display-encoded RGB) at every point of a `size`³ lattice and
//! writes a `.cube` 3D LUT that `moba_assets::GradingLut` loads, after header comments
//! recording the tool, preset and every parameter. Presets are `identity` (the default),
//! `ember`, `frost` and `meadow`; the flags `--saturation S` (0..=2), `--contrast C`
//! (0.5..=2), `--temperature T` and `--tint T` (-1..=1), `--lift r,g,b` (-0.5..=0.5),
//! `--gamma r,g,b` (0.2..=5) and `--gain r,g,b` (0..=2) override the preset; `--size` is
//! 2..=64 (default 33). The output is deterministic for a given platform.
//!
//! `bake` refuses a grade that fails `GradingLut::check_team_readability` (ADR-0011 rule 1:
//! team colours stay distinguishable at full strength, nominal exposure ± 1 EV), the check
//! the client repeats when it loads a map's grade.
//!
//! `info` prints the size, the per-channel output range, the mean absolute deviation from
//! the identity and the readability result. `apply` grades an 8-bit PNG (its values taken as display-encoded, like the
//! tonemapper's output) for side-by-side previews; alpha is kept.
#![allow(clippy::print_stdout)]

mod grade;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grade::Grade;
use moba_assets::{GradingLut, MAX_GRADING_LUT_SIZE, MIN_GRADING_LUT_SIZE};

const USAGE: &str = "usage:
  grade-bake bake [--preset identity|ember|frost|meadow] [--size N] [--saturation S]
                  [--contrast C] [--temperature T] [--tint T] [--lift r,g,b]
                  [--gamma r,g,b] [--gain r,g,b] <out.cube>
  grade-bake info <file.cube>
  grade-bake apply <file.cube> <in.png> <out.png>";

/// Exposures (EV) the readability check covers at bake time: nominal ± 1 stop, the margin the
/// client checks around a map's fixed EV.
const READABILITY_EXPOSURES: [f32; 3] = [-1.0, 0.0, 1.0];

/// Recorded in the `.cube` header.
const WRITER: &str = concat!("moba-tools grade-bake ", env!("CARGO_PKG_VERSION"));
/// Default lattice size (a common `.cube` size; the shipped samples use 17).
const DEFAULT_SIZE: u32 = 33;
/// Largest PNG accepted by `apply`, in bytes.
const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;
/// Largest PNG edge accepted by `apply`, in pixels.
const MAX_PNG_DIMENSION: u32 = 8192;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bake") => bake_command(&args[1..]),
        Some("info") => info(&args[1..]),
        Some("apply") => apply_command(&args[1..]),
        _ => Err(String::from(USAGE)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("grade-bake: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Parsed `bake` arguments.
#[derive(Debug)]
struct BakeArgs {
    preset: String,
    grade: Grade,
    size: u32,
    output: PathBuf,
}

fn parse_scalar(flag: &str, value: &str, (min, max): (f32, f32)) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (min..=max).contains(value))
        .ok_or_else(|| format!("{flag} takes a number in {min}..={max}"))
}

fn parse_v3(flag: &str, value: &str, range: (f32, f32)) -> Result<[f32; 3], String> {
    let parts: Vec<&str> = value.split(',').collect();
    let [a, b, c] = parts[..] else {
        return Err(format!(
            "{flag} takes three comma-separated numbers in {}..={}",
            range.0, range.1
        ));
    };
    Ok([
        parse_scalar(flag, a, range)?,
        parse_scalar(flag, b, range)?,
        parse_scalar(flag, c, range)?,
    ])
}

fn parse_bake(args: &[String]) -> Result<BakeArgs, String> {
    let mut preset = None;
    let mut overrides: Vec<(String, String)> = Vec::new();
    let mut size = DEFAULT_SIZE;
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
                    .filter(|size| (MIN_GRADING_LUT_SIZE..=MAX_GRADING_LUT_SIZE).contains(size))
                    .ok_or("--size must be 2..=64")?;
            }
            "--saturation" | "--contrast" | "--temperature" | "--tint" | "--lift" | "--gamma"
            | "--gain" => overrides.push((arg.clone(), value()?)),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            path if output.is_none() => output = Some(PathBuf::from(path)),
            _ => return Err(String::from(USAGE)),
        }
    }
    let preset = preset.unwrap_or_else(|| String::from("identity"));
    let mut grade = Grade::preset(&preset)
        .ok_or_else(|| format!("unknown preset {preset} (one of {:?})", grade::PRESETS))?;
    for (flag, value) in overrides {
        match flag.as_str() {
            "--saturation" => grade.saturation = parse_scalar(&flag, &value, (0.0, 2.0))?,
            "--contrast" => grade.contrast = parse_scalar(&flag, &value, (0.5, 2.0))?,
            "--temperature" => grade.temperature = parse_scalar(&flag, &value, (-1.0, 1.0))?,
            "--tint" => grade.tint = parse_scalar(&flag, &value, (-1.0, 1.0))?,
            "--lift" => grade.lift = parse_v3(&flag, &value, (-0.5, 0.5))?,
            "--gamma" => grade.gamma = parse_v3(&flag, &value, (0.2, 5.0))?,
            _ => grade.gain = parse_v3(&flag, &value, (0.0, 2.0))?,
        }
    }
    Ok(BakeArgs {
        preset,
        grade,
        size,
        output: output.ok_or_else(|| String::from(USAGE))?,
    })
}

/// The LUT of `grade` on a `size`³ lattice.
fn bake_lut(grade: &Grade, size: u32) -> Result<GradingLut, String> {
    let n = size as usize;
    let scale = 1.0 / (size.max(2) - 1) as f32;
    let mut texels = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                texels.push(grade.apply([r as f32 * scale, g as f32 * scale, b as f32 * scale]));
            }
        }
    }
    GradingLut::from_texels(size, texels).map_err(|error| format!("baked LUT invalid: {error}"))
}

/// The `.cube` text for `args` (no I/O).
fn bake(args: &BakeArgs) -> Result<String, String> {
    let lut = bake_lut(&args.grade, args.size)?;
    let mut header = vec![
        format!("Colour-grading LUT baked by {WRITER} (R6 post-processing)."),
        String::from("Display-encoded RGB in, display-encoded RGB out. Client-local and cosmetic:"),
        String::from("never hashed or sent. See docs/content-authoring.md#post-processing."),
        format!("preset = {:?}, size = {}", args.preset, args.size),
    ];
    header.extend(args.grade.describe());
    let mut text = String::new();
    for line in &header {
        text.push_str("# ");
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(&lut.to_cube_string(&args.preset));
    // The written file must load back (and fit the loader's size cap), and pass the
    // readability check the client applies when it loads a map's grade.
    let parsed =
        GradingLut::parse(&text).map_err(|error| format!("baked LUT does not parse: {error}"))?;
    parsed
        .check_team_readability(1.0, &READABILITY_EXPOSURES)
        .map_err(|error| format!("refusing to write {}: {error}", args.output.display()))?;
    Ok(text)
}

fn bake_command(args: &[String]) -> Result<(), String> {
    let args = parse_bake(args)?;
    let text = bake(&args)?;
    std::fs::write(&args.output, &text)
        .map_err(|error| format!("{}: {error}", args.output.display()))?;
    println!(
        "{}: {}³ LUT ({}), {} bytes",
        args.output.display(),
        args.size,
        args.preset,
        text.len()
    );
    Ok(())
}

/// Per-channel output range and deviation from the identity.
#[derive(Debug, PartialEq)]
struct Stats {
    min: [f32; 3],
    max: [f32; 3],
    mean_deviation: f32,
    max_deviation: f32,
}

fn stats(lut: &GradingLut) -> Stats {
    let identity = GradingLut::identity(lut.size());
    let mut stats = Stats {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
        mean_deviation: 0.0,
        max_deviation: 0.0,
    };
    let mut total = 0.0f64;
    for (texel, reference) in lut.texels().iter().zip(identity.texels()) {
        for channel in 0..3 {
            stats.min[channel] = stats.min[channel].min(texel[channel]);
            stats.max[channel] = stats.max[channel].max(texel[channel]);
            let deviation = (texel[channel] - reference[channel]).abs();
            total += f64::from(deviation);
            stats.max_deviation = stats.max_deviation.max(deviation);
        }
    }
    stats.mean_deviation = (total / (lut.texels().len() * 3) as f64) as f32;
    stats
}

fn info(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err(String::from(USAGE));
    };
    let path = Path::new(path);
    let lut =
        GradingLut::load_file(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let stats = stats(&lut);
    println!(
        "{}: {}³ LUT, {} texels",
        path.display(),
        lut.size(),
        lut.texels().len()
    );
    for (channel, name) in ["r", "g", "b"].into_iter().enumerate() {
        println!(
            "  {name}: {:.4}..={:.4}",
            stats.min[channel], stats.max[channel]
        );
    }
    println!(
        "  mean |out - in| = {:.5} (max {:.5})",
        stats.mean_deviation, stats.max_deviation
    );
    match lut.check_team_readability(1.0, &READABILITY_EXPOSURES) {
        Ok(()) => println!("  team colours: readable at full strength, EV -1..=1"),
        Err(error) => println!("  {error} (at full strength, EV -1..=1)"),
    }
    Ok(())
}

/// Grades tightly packed RGBA8 `rgba` in place (alpha untouched).
fn apply_rgba(lut: &GradingLut, rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let graded = lut.sample([
            f32::from(pixel[0]) / 255.0,
            f32::from(pixel[1]) / 255.0,
            f32::from(pixel[2]) / 255.0,
        ]);
        for (out, value) in pixel.iter_mut().zip(graded) {
            *out = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

/// Decodes a PNG to tightly packed RGBA8.
fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let at = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let metadata = std::fs::metadata(path).map_err(|error| at(&error))?;
    if metadata.len() > MAX_PNG_BYTES {
        return Err(at(&format!("larger than {MAX_PNG_BYTES} bytes")));
    }
    let file = std::fs::File::open(path).map_err(|error| at(&error))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| at(&error))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > MAX_PNG_DIMENSION || height > MAX_PNG_DIMENSION {
        return Err(at(&format!(
            "{width}x{height} exceeds {MAX_PNG_DIMENSION} pixels"
        )));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| at(&"image too large"))?;
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(|error| at(&error))?;
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
        png::ColorType::Indexed => return Err(at(&"unexpanded palette")),
    };
    Ok((width, height, rgba))
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let at = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let file = std::fs::File::create(path).map_err(|error| at(&error))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| at(&error))?;
    writer.write_image_data(rgba).map_err(|error| at(&error))
}

fn apply_command(args: &[String]) -> Result<(), String> {
    let [lut, input, output] = args else {
        return Err(String::from(USAGE));
    };
    let lut_path = Path::new(lut);
    let lut = GradingLut::load_file(lut_path)
        .map_err(|error| format!("{}: {error}", lut_path.display()))?;
    let (width, height, mut rgba) = read_png(Path::new(input))?;
    apply_rgba(&lut, &mut rgba);
    write_png(Path::new(output), width, height, &rgba)?;
    println!(
        "{output}: {width}x{height} graded with a {}³ LUT",
        lut.size()
    );
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
            "--saturation",
            "1.2",
            "--lift",
            "0.01,0.02,0.03",
            "--size",
            "17",
            "out.cube",
        ]))
        .expect("valid");
        assert_eq!(parsed.size, 17);
        assert_eq!(parsed.grade.saturation, 1.2);
        assert_eq!(parsed.grade.lift, [0.01, 0.02, 0.03]);
        let ember = Grade::preset("ember").expect("preset");
        assert_eq!(parsed.grade.temperature, ember.temperature);
        assert_eq!(parsed.output, PathBuf::from("out.cube"));
        let default = parse_bake(&args(&["x.cube"])).expect("defaults");
        assert_eq!(
            (default.grade, default.size),
            (Grade::IDENTITY, DEFAULT_SIZE)
        );
        assert_eq!(default.preset, "identity");
        for bad in [
            &["--preset", "lava", "x"][..],
            &["--size", "1", "x"],
            &["--size", "65", "x"],
            &["--size", "big", "x"],
            &["--saturation", "3", "x"],
            &["--contrast", "0.1", "x"],
            &["--temperature", "nan", "x"],
            &["--tint", "-2", "x"],
            &["--lift", "0,0", "x"],
            &["--lift", "0,0,0,0", "x"],
            &["--gamma", "0,1,1", "x"],
            &["--gain", "1,1,inf", "x"],
            &["--bogus", "x"],
            &["--preset"],
            &["--preset", "frost"],
            &["a", "b"],
        ] {
            assert!(parse_bake(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bake_refuses_a_grade_that_breaks_team_readability() {
        // Fully desaturated: the teams turn grey.
        let grey = parse_bake(&args(&["--saturation", "0", "--size", "9", "grey.cube"]))
            .expect("valid flags");
        let error = bake(&grey).unwrap_err();
        assert!(error.contains("team colours unreadable"), "{error}");
        for preset in ["identity", "ember", "frost", "meadow"] {
            let parsed =
                parse_bake(&args(&["--preset", preset, "--size", "17", "x.cube"])).expect("valid");
            bake(&parsed).unwrap_or_else(|error| panic!("{preset}: {error}"));
        }
    }

    #[test]
    fn baked_files_round_trip_and_identity_is_exact() {
        let parsed = parse_bake(&args(&["--size", "5", "id.cube"])).expect("ok");
        let text = bake(&parsed).expect("bakes");
        assert!(text.starts_with("# Colour-grading LUT baked by moba-tools grade-bake"));
        let lut = GradingLut::parse(&text).expect("parses");
        assert_eq!(lut.size(), 5);
        let identity = stats(&lut);
        assert!(identity.max_deviation < 1e-6, "{identity:?}");
        assert_eq!((identity.min, identity.max), ([0.0; 3], [1.0; 3]));
        let ember = parse_bake(&args(&["--preset", "ember", "--size", "17", "e.cube"]))
            .and_then(|args| bake(&args))
            .expect("ember");
        let ember = stats(&GradingLut::parse(&ember).expect("parses"));
        assert!(
            ember.mean_deviation > 0.005 && ember.max_deviation < 0.12,
            "{ember:?}"
        );
        // Shadows are lifted: black is no longer black.
        assert!(ember.min[0] > 0.0);
    }

    #[test]
    fn shipped_samples_match_their_presets_and_maps() {
        let samples = [
            (
                "ember",
                include_str!("../../../../../content/grading/ember.cube"),
                include_str!("../../../../../content/maps/ember.toml"),
            ),
            (
                "frost",
                include_str!("../../../../../content/grading/frost.cube"),
                include_str!("../../../../../content/maps/frost.toml"),
            ),
            (
                "meadow",
                include_str!("../../../../../content/grading/meadow.cube"),
                include_str!("../../../../../content/maps/map-grass.toml"),
            ),
        ];
        for (preset, cube, map) in samples {
            let shipped = GradingLut::parse(cube).expect("shipped LUT parses");
            let baked = bake_lut(&Grade::preset(preset).expect("preset"), 17).expect("bakes");
            assert_eq!(shipped.size(), 17);
            // Six decimals, plus slack for another platform's powf.
            for (a, b) in shipped.texels().iter().zip(baked.texels()) {
                assert!((0..3).all(|c| (a[c] - b[c]).abs() < 1e-5), "{preset}");
            }
            let visuals = moba_assets::MapVisuals::parse(map).expect("map descriptor");
            assert_eq!(visuals.post.grading.as_deref(), Some(preset));
            assert_eq!(visuals.post.exposure, Some(0.0));
            assert_eq!(
                visuals.post.auto_exposure,
                (preset == "ember").then_some(true)
            );
        }
    }

    #[test]
    fn apply_grades_pixels_and_keeps_alpha() {
        let mut rgba: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, v]).collect();
        let original = rgba.clone();
        apply_rgba(&GradingLut::identity(17), &mut rgba);
        assert_eq!(rgba, original);
        let invert = GradingLut::from_texels(
            2,
            GradingLut::identity(2)
                .texels()
                .iter()
                .map(|t| t.map(|v| 1.0 - v))
                .collect(),
        )
        .expect("valid");
        let mut pixel = [10, 200, 0, 77];
        apply_rgba(&invert, &mut pixel);
        assert_eq!(pixel, [245, 55, 255, 77]);

        let dir = std::env::temp_dir().join(format!("moba-grade-bake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        write_png(&dir.join("in.png"), 16, 16, &original[..16 * 16 * 4]).expect("write");
        let (width, height, read) = read_png(&dir.join("in.png")).expect("read");
        assert_eq!((width, height), (16, 16));
        assert_eq!(read, &original[..16 * 16 * 4]);
        assert!(read_png(&dir.join("missing.png")).is_err());
        std::fs::write(dir.join("bad.png"), b"not a png").expect("write");
        assert!(read_png(&dir.join("bad.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
