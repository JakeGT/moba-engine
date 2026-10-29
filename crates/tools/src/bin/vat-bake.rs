#![allow(clippy::print_stdout, clippy::print_stderr)]
//! Bake a skinned model's clips into a vertex-animation texture (R4b, ADR-0012 amendment).
//!
//! ```text
//! vat-bake <model.glb> <out> [--model NAME] [--clips idle:loop,run:loop,attack,death]
//!          [--fps 20] [--run-speed 4] [--attack-release 0.35] [--crossfade 0.15]
//! ```
//!
//! Writes `<out>.ktx2` (RGBA16F skinned positions and normals, one level) and `<out>.toml`
//! (the descriptor, `moba_assets::VatDescriptor`). `--model` is the model name the client
//! draws (`models/<NAME>.glb`; default: the input's file stem). A clip with `:loop` loops;
//! `idle`, `run`, `walk` and `recall` loop by default. The sample minion is
//! `vat-bake content/models/sentinel-skinned.glb content/vat/minion`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use moba_assets::{
    DEFAULT_VAT_FPS, VatBakeClip, VatPlaybackParams, bake_vat, load_glb_file, write_ktx2,
};

const DEFAULT_CLIPS: &str = "idle,run,attack,death";

struct Options {
    input: PathBuf,
    output: PathBuf,
    model: Option<String>,
    clips: String,
    fps: f32,
    playback: VatPlaybackParams,
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: vat-bake <model.glb> <out> [--model NAME] [--clips idle:loop,run:loop,attack,death] \
         [--fps 20] [--run-speed 4] [--attack-release 0.35] [--crossfade 0.15]"
    );
    ExitCode::from(2)
}

fn parse() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut model = None;
    let mut clips = String::from(DEFAULT_CLIPS);
    let mut fps = DEFAULT_VAT_FPS;
    let mut playback = VatPlaybackParams::default();
    let number = |value: Option<String>, flag: &str| -> Result<f32, String> {
        value
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("{flag} needs a number"))
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = Some(args.next().ok_or("--model needs a name")?),
            "--clips" => clips = args.next().ok_or("--clips needs a list")?,
            "--fps" => fps = number(args.next(), "--fps")?,
            "--run-speed" => playback.run_speed = number(args.next(), "--run-speed")?,
            "--attack-release" => {
                playback.attack_release = number(args.next(), "--attack-release")?;
            }
            "--crossfade" => playback.crossfade = number(args.next(), "--crossfade")?,
            "--help" | "-h" => return Err(String::new()),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let [input, output]: [PathBuf; 2] = positional
        .try_into()
        .map_err(|_| String::from("expected <model.glb> <out>"))?;
    Ok(Options {
        input,
        output,
        model,
        clips,
        fps,
        playback,
    })
}

fn with_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

fn main() -> ExitCode {
    let options = match parse() {
        Ok(options) => options,
        Err(error) => {
            if !error.is_empty() {
                eprintln!("vat-bake: {error}");
            }
            return usage();
        }
    };
    let mesh = match load_glb_file(&options.input) {
        Ok(mesh) => mesh,
        Err(error) => {
            eprintln!("vat-bake: {}: {error}", options.input.display());
            return ExitCode::FAILURE;
        }
    };
    let model = options.model.clone().unwrap_or_else(|| {
        options
            .input
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
    });
    let clips: Vec<VatBakeClip<'_>> = options
        .clips
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (name, looping) = match entry.split_once(':') {
                Some((name, mode)) => (name, mode.eq_ignore_ascii_case("loop")),
                None => (entry, matches!(entry, "idle" | "run" | "walk" | "recall")),
            };
            VatBakeClip::new(name, looping)
        })
        .collect();
    let (texture, descriptor) = match bake_vat(&mesh, &model, &clips, options.fps, options.playback)
    {
        Ok(baked) => baked,
        Err(error) => {
            eprintln!("vat-bake: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Never write a descriptor the client would reject (every minion would be a cube).
    if let Err(error) = moba_assets::VatDescriptor::parse(&descriptor.to_toml()) {
        eprintln!("vat-bake: the baked descriptor does not validate: {error}");
        return ExitCode::FAILURE;
    }
    if let Some(parent) = options.output.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("vat-bake: {}: {error}", parent.display());
        return ExitCode::FAILURE;
    }
    let ktx2 = with_extension(&options.output, "ktx2");
    let toml = with_extension(&options.output, "toml");
    let bytes = write_ktx2(&texture, "moba-tools vat-bake");
    if let Err(error) =
        std::fs::write(&ktx2, &bytes).and_then(|()| std::fs::write(&toml, descriptor.to_toml()))
    {
        eprintln!("vat-bake: {error}");
        return ExitCode::FAILURE;
    }
    println!(
        "wrote {} ({}x{} rgba16f, {} bytes) and {}: {} vertices, {} frames at {} fps, clips {}",
        ktx2.display(),
        descriptor.width,
        descriptor.height,
        bytes.len(),
        toml.display(),
        descriptor.vertex_count,
        descriptor.frame_count,
        descriptor.fps,
        descriptor
            .clips
            .iter()
            .map(|clip| format!("{}[{}+{}]", clip.name, clip.first, clip.frames))
            .collect::<Vec<_>>()
            .join(" ")
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    /// The committed sample minion VAT matches a fresh bake of the committed model.
    #[test]
    fn committed_minion_vat_matches_a_fresh_bake() {
        let mesh = moba_assets::load_glb(include_bytes!(
            "../../../../content/models/sentinel-skinned.glb"
        ))
        .expect("model");
        let clips = ["idle", "run", "attack", "death"]
            .map(|name| moba_assets::VatBakeClip::new(name, matches!(name, "idle" | "run")));
        let (texture, descriptor) = moba_assets::bake_vat(
            &mesh,
            "sentinel-skinned",
            &clips,
            moba_assets::DEFAULT_VAT_FPS,
            moba_assets::VatPlaybackParams::default(),
        )
        .expect("bake");
        let committed =
            moba_assets::VatDescriptor::parse(include_str!("../../../../content/vat/minion.toml"))
                .expect("descriptor");
        assert_eq!(committed, descriptor, "content/vat/minion.toml is stale");
        let ktx2 = moba_assets::load_ktx2(include_bytes!("../../../../content/vat/minion.ktx2"))
            .expect("texture");
        committed.check_texture(&ktx2).expect("matches");
        committed.check_mesh(&mesh).expect("matches");
        // Within half-float rounding: the skinning maths may round differently per platform
        // (SIMD widths, fused multiply-add), so compare values, not bytes.
        let decode = |bytes: &[u8]| -> Vec<f32> {
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| moba_assets::f32_from_f16(u16::from_le_bytes(*pair)))
                .collect()
        };
        let (fresh, stored) = (decode(texture.data()), decode(ktx2.data()));
        assert_eq!(fresh.len(), stored.len());
        let worst = fresh
            .iter()
            .zip(&stored)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 4e-3,
            "content/vat/minion.ktx2 is stale (max diff {worst}); re-run vat-bake"
        );
    }
}
