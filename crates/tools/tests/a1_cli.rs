//! Exit codes and `--json` of the A1 tools: `content-check --source` and `review-sheet`.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(binary: &str, args: &[&str]) -> Run {
    let output = Command::new(binary).args(args).output().expect("run");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moba-a1-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Writes an 8-bit (or 16-bit) RGB PNG.
fn write_rgb(
    path: &Path,
    (width, height): (u32, u32),
    sixteen: bool,
    pixel: impl Fn(u32, u32) -> [u8; 3],
) {
    let file = std::fs::File::create(path).expect("create");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(if sixteen {
        png::BitDepth::Sixteen
    } else {
        png::BitDepth::Eight
    });
    let mut writer = encoder.write_header().expect("header");
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            for value in pixel(x, y) {
                if sixteen {
                    bytes.extend_from_slice(&u16::from(value).to_be_bytes());
                } else {
                    bytes.push(value);
                }
            }
        }
    }
    writer.write_image_data(&bytes).expect("data");
}

/// A 16×16 ash terrain layer at `albedo` grey; returns the manifest path.
fn layer(dir: &Path, albedo: u8) -> PathBuf {
    write_rgb(&dir.join("base.png"), (16, 16), false, |x, y| {
        [albedo + ((x + y) % 5) as u8; 3]
    });
    write_rgb(&dir.join("mr.png"), (16, 16), false, |x, y| {
        [0, 220 + ((x * 7 + y * 3) % 35) as u8, 0]
    });
    write_rgb(&dir.join("normal.png"), (16, 16), false, |_, _| {
        [128, 128, 255]
    });
    let path = dir.join("ash.source.toml");
    std::fs::write(
        &path,
        "asset = \"ash\"\nclass = \"terrain\"\ntile_m = 4.0\nshipped_size = 1024\n\
         material_class = \"CHAR\"\n[maps]\nbase = \"base.png\"\nmetallic_roughness = \"mr.png\"\n\
         normal = \"normal.png\"\n",
    )
    .expect("manifest");
    path
}

#[test]
fn content_check_source_exits_by_outcome_in_text_and_json() {
    let bin = env!("CARGO_BIN_EXE_content-check");
    let dir = scratch("source");
    let manifest = layer(&dir, 0x66);
    let path = manifest.to_str().expect("utf-8 path");

    let pass = run(bin, &["--source", path]);
    assert_eq!(pass.code, Some(0), "{}{}", pass.stdout, pass.stderr);
    assert!(
        pass.stdout.trim_end().ends_with("ash: pass"),
        "{}",
        pass.stdout
    );
    let pass = run(bin, &["--json", "--source", path]);
    assert_eq!(pass.code, Some(0));
    let report: serde_json::Value = serde_json::from_str(pass.stdout.trim()).expect("one JSON");
    assert_eq!(report["pass"], true);
    assert!(
        report["rules"]
            .as_array()
            .is_some_and(|rules| !rules.is_empty())
    );

    // Crushed albedo: a measured failure is exit 1, with the failing rules in the JSON.
    layer(&dir, 20);
    let fail = run(bin, &["--json", "--source", path]);
    assert_eq!(fail.code, Some(1));
    let report: serde_json::Value = serde_json::from_str(fail.stdout.trim()).expect("one JSON");
    assert_eq!(report["pass"], false);
    assert!(fail.stdout.contains("\"PBR-ALB-1\""));

    // An unreadable manifest is exit 1 too, with the error in the JSON.
    let missing = dir.join("absent.source.toml");
    let error = run(
        bin,
        &["--json", "--source", missing.to_str().expect("utf-8")],
    );
    assert_eq!(error.code, Some(1));
    let report: serde_json::Value = serde_json::from_str(error.stdout.trim()).expect("one JSON");
    assert_eq!(report["pass"], false);
    assert!(report["errors"][0].is_string());
    assert!(!error.stderr.contains("panicked"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `render_meta.json` with Golden Crossing's numbers and no models.
fn meta_json(sun_glow: f64) -> String {
    let frame = |file: &str, w: u32, h: u32, d: f64| {
        serde_json::json!({"file": file, "width": w, "height": h, "distance_m": d,
                           "models": [], "post_bbox": [1270, 600, 1290, 720],
                           "ids": "ids.png", "emit": "emit.png"})
    };
    // L_ref: the lane grit #906B58 x 1.5 x sin(28 deg) (material-vocabulary §1.1).
    let lane = [0x90, 0x6B, 0x58].map(|v: u8| {
        let c = f64::from(v) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let sun: [f64; 3] = [-0.885, 0.532, 0.465];
    let elevation = (sun[1] / (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2]).sqrt()).asin();
    let l_ref = (0.2126 * lane[0] + 0.7152 * lane[1] + 0.0722 * lane[2]) * 1.5 * elevation.sin();
    serde_json::json!({
        "schema": 1, "name": "cli", "renderer": "Cycles", "device": "CPU", "blender": "5.2.3",
        "samples": 1,
        "camera": {"yaw": -0.7, "pitch": 0.95, "distance_m": 46.0, "zoom_distance_m": 8.0,
                   "fov_y_deg": 60.0},
        "sun": {"dir": sun, "colour": [1.0, 0.80, 0.62], "intensity": 1.5,
                "elevation_deg": elevation.to_degrees()},
        "sky": {"ibl": 0.35, "zenith": [0.16, 0.22, 0.36], "horizon": [0.95, 0.62, 0.38],
                "ground": [0.18, 0.11, 0.07], "sun_color": [2.6, 1.55, 0.85], "sun_glow": sun_glow},
        "ground_hex": "#6B6560", "lane_hex": "#906B58", "l_ref": l_ref, "post_m": 1.8,
        "view_transform": "AgX", "exposure_ev": 0.1, "ground_l_target": 0.5,
        "ground_l_calibrated": 0.5, "id_colours": {}, "post_colour": [1, 1, 1],
        "models": [],
        "frames": {"46m_1440p": frame("f1440.png", 2560, 1440, 46.0),
                   "46m_2160p": frame("f2160.png", 3840, 2160, 46.0),
                   "zoom": [], "lineup_1440p": frame("f1440.png", 2560, 1440, 12.0),
                   "turntable": [], "silhouettes": []},
    })
    .to_string()
}

#[test]
fn review_sheet_exits_two_on_bad_input_and_one_on_a_failed_check() {
    let bin = env!("CARGO_BIN_EXE_review-sheet");
    let dir = scratch("review");
    assert_eq!(run(bin, &[]).code, Some(2));
    let missing = run(bin, &[dir.join("absent.json").to_str().expect("utf-8")]);
    assert_eq!(missing.code, Some(2));
    let folder = run(bin, &[dir.to_str().expect("utf-8")]);
    assert_eq!(folder.code, Some(2));
    assert!(
        folder.stderr.contains("not a regular file"),
        "{}",
        folder.stderr
    );

    // Other numbers than Golden Crossing's (here a tighter sky glow) are refused: exit 2.
    let meta = dir.join("render_meta.json");
    let meta_path = meta.to_str().expect("utf-8");
    std::fs::write(&meta, meta_json(8.0)).expect("meta");
    let refused = run(bin, &[meta_path]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.stderr.contains("sun glow"), "{}", refused.stderr);

    // The right numbers and frames with no model in them: VAL-3 has no ground ring to read,
    // which is a failure (exit 1), reported in the JSON.
    std::fs::write(&meta, meta_json(6.0)).expect("meta");
    write_rgb(&dir.join("f1440.png"), (2560, 1440), false, |_, _| [118; 3]);
    write_rgb(&dir.join("f2160.png"), (3840, 2160), false, |_, _| [118; 3]);
    write_rgb(&dir.join("ids.png"), (2560, 1440), false, |x, y| {
        if (1270..1290).contains(&x) && (600..720).contains(&y) {
            [255; 3]
        } else {
            [0; 3]
        }
    });
    write_rgb(&dir.join("emit.png"), (2560, 1440), true, |_, _| [0; 3]);
    let out = dir.join("sheets");
    let failed = run(
        bin,
        &[meta_path, "--json", "--out", out.to_str().expect("utf-8")],
    );
    assert_eq!(failed.code, Some(1), "{}", failed.stderr);
    let metrics: serde_json::Value = serde_json::from_str(failed.stdout.trim()).expect("JSON");
    assert_eq!(metrics["passed"], false);
    let checks = metrics["checks"].as_array().expect("checks");
    let rule = |id: &str| checks.iter().find(|check| check["rule"] == id).expect(id);
    assert_eq!(rule("VAL-2")["pass"], true);
    assert_eq!(rule("VAL-3")["pass"], false);
    assert!(out.join("metrics.json").is_file() && out.join("sheet_46m.png").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}
