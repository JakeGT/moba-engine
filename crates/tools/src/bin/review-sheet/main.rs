//! Review-render contact sheets (roadmap §9.4; A1, ADR-0022 amendment 2).
//!
//! ```text
//! review-sheet <render_meta.json> [--out DIR] [--json]
//! ```
//!
//! Reads the raw frames `scripts/blender/review_render.py` writes, refuses them unless they were
//! made with the engine's default camera (yaw -0.7, pitch 0.95, 46 m) and the bible's Golden
//! Crossing light, runs the numeric checks (CON-1/CON-4, VAL-2 to VAL-4, PBR-EMIT-1 to
//! PBR-EMIT-3 as rendered, SIL-1 to SIL-8) and writes five sheets (`sheet_46m.png`,
//! `sheet_8m.png`, `sheet_turntable.png`, `sheet_lineup.png`, `sheet_silhouettes.png`) and
//! `metrics.json` into `--out` (default `<meta dir>/sheets`). Every sheet prints the camera and
//! the sun. Exits 0 when every check passes, 1 when one fails, 2 on an unusable input.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod checks;
#[path = "../../font5x7.rs"]
mod font5x7;
#[allow(dead_code)] // The texture-statistics half is for `content-check --source`.
#[path = "../../imaging.rs"]
mod imaging;
mod meta;
mod sheet;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `render_meta.json` is small; refuse anything absurd before parsing it.
const MAX_META_BYTES: u64 = 4 * 1024 * 1024;

fn usage() -> ExitCode {
    eprintln!("usage: review-sheet <render_meta.json> [--out DIR] [--json]");
    ExitCode::from(2)
}

fn read_meta(path: &Path) -> Result<meta::Meta, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let size = metadata.len();
    if size > MAX_META_BYTES {
        return Err(format!(
            "{}: {size} B is over {MAX_META_BYTES} B",
            path.display()
        ));
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

#[allow(clippy::too_many_lines)] // CLI, checks, sheets and the report, in order.
fn main() -> ExitCode {
    let mut meta_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut json = false;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--out" => match args.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => return usage(),
            },
            "-h" | "--help" => return usage(),
            value if meta_path.is_none() && !value.starts_with("--") => {
                meta_path = Some(PathBuf::from(value));
            }
            _ => return usage(),
        }
    }
    let Some(meta_path) = meta_path else {
        return usage();
    };
    let dir = meta_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let out = out.unwrap_or_else(|| dir.join("sheets"));
    let meta = match read_meta(&meta_path) {
        Ok(meta) => meta,
        Err(error) => {
            eprintln!("review-sheet: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(errors) = meta::check_constants(&meta) {
        for error in errors {
            eprintln!("review-sheet: {error}");
        }
        eprintln!(
            "review-sheet: refusing a render made with other numbers than the engine's and the bible's"
        );
        return ExitCode::from(2);
    }
    let mut results = match checks::run(&meta, &dir) {
        Ok(results) => results,
        Err(error) => {
            eprintln!("review-sheet: {error}");
            return ExitCode::from(2);
        }
    };
    for model in meta.models.iter().rev() {
        if !model.pose.clip.is_empty() {
            results.insert(
                0,
                checks::Check {
                    rule: format!("POSE ({})", model.name),
                    pass: None,
                    value: if model.pose.applied {
                        format!("{} frame {}", model.pose.clip, model.pose.frame)
                    } else {
                        format!("{} not found: bind pose", model.pose.clip)
                    },
                    limit: "the pose the frames show".into(),
                },
            );
        }
    }
    let sheets = match sheet::write_sheets(&meta, &dir, &out, &results) {
        Ok(sheets) => sheets,
        Err(error) => {
            eprintln!("review-sheet: {error}");
            return ExitCode::from(2);
        }
    };
    let failed = results.iter().any(|check| check.pass == Some(false));
    let (look, sun, gap) = meta::camera_sun_gap();
    let metrics = serde_json::json!({
        "name": meta.name,
        "passed": !failed,
        "camera": {"yaw": meta.camera.yaw, "pitch": meta.camera.pitch, "distance_m": meta.camera.distance_m,
                   "look_azimuth_deg": look},
        "sun": {"dir": meta.sun.dir, "intensity": meta.sun.intensity, "azimuth_deg": sun,
                "elevation_deg": meta.sun.elevation_deg},
        "camera_sun_gap_deg": gap,
        "exposure_ev": meta.exposure_ev,
        "l_ref": meta.l_ref,
        "sheets": sheets,
        "checks": results.iter().map(|check| serde_json::json!({
            "rule": check.rule, "pass": check.pass, "value": check.value, "limit": check.limit,
        })).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&metrics).unwrap_or_default();
    if let Err(error) = std::fs::write(out.join("metrics.json"), format!("{text}\n")) {
        eprintln!(
            "review-sheet: {}: {error}",
            out.join("metrics.json").display()
        );
        return ExitCode::from(2);
    }
    if json {
        println!("{text}");
    } else {
        for line in meta::camera_and_sun_lines(&meta) {
            println!("{line}");
        }
        for check in &results {
            let status = match check.pass {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "info",
            };
            println!(
                "{status}  {:<34} {:<44} {}",
                check.rule, check.value, check.limit
            );
        }
        println!(
            "{}: {} ({})",
            meta.name,
            if failed { "FAIL" } else { "pass" },
            out.display()
        );
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::meta;

    fn sample_meta() -> meta::Meta {
        let frame = |w: u32, h: u32, d: f64| {
            serde_json::json!({"file": "f.png", "width": w, "height": h, "distance_m": d,
                               "models": [], "post_bbox": [0, 0, 1, 1]})
        };
        serde_json::from_value(serde_json::json!({
            "schema": 1, "name": "t", "renderer": "Cycles", "device": "CPU", "blender": "5.2.3",
            "samples": 1,
            "camera": {"yaw": -0.7, "pitch": 0.95, "distance_m": 46.0, "zoom_distance_m": 8.0,
                       "fov_y_deg": 60.0},
            "sun": {"dir": [-0.885, 0.532, 0.465], "colour": [1.0, 0.80, 0.62], "intensity": 1.5,
                    "elevation_deg": meta::sun_elevation_deg()},
            "sky": {"ibl": 0.35, "zenith": [0.16, 0.22, 0.36], "horizon": [0.95, 0.62, 0.38],
                    "ground": [0.18, 0.11, 0.07], "sun_color": [2.6, 1.55, 0.85], "sun_glow": 6.0},
            "ground_hex": "#6B6560", "lane_hex": "#906B58", "l_ref": meta::l_ref(), "post_m": 1.8,
            "view_transform": "AgX", "exposure_ev": 0.1, "ground_l_target": 0.5,
            "ground_l_calibrated": 0.4996, "id_colours": {}, "post_colour": [1, 1, 1],
            "models": [],
            "frames": {"46m_1440p": frame(2560, 1440, 46.0), "46m_2160p": frame(3840, 2160, 46.0),
                       "zoom": [frame(2560, 1440, 8.0)], "lineup_1440p": frame(2560, 1440, 12.0),
                       "turntable": [], "silhouettes": []},
        }))
        .expect("sample meta parses")
    }

    #[test]
    fn the_engine_camera_and_golden_crossing_are_accepted() {
        assert!(meta::check_constants(&sample_meta()).is_ok());
    }

    #[test]
    fn another_camera_or_sun_is_refused() {
        let mut moved = sample_meta();
        moved.camera.yaw = -0.5;
        assert!(meta::check_constants(&moved).is_err());
        let mut lit = sample_meta();
        lit.sun.dir = [-0.6, 0.35, 0.45]; // Ember Dusk, not Golden Crossing.
        let errors = meta::check_constants(&lit).unwrap_err();
        assert!(errors.iter().any(|error| error.contains("Golden Crossing")));
        let mut far = sample_meta();
        far.frames.zoom[0].distance_m = 10.0;
        assert!(meta::check_constants(&far).is_err());
        // A sky without the glow lobe, or with another one, is not Golden Crossing.
        let mut dull = sample_meta();
        dull.sky.sun_glow = None;
        assert!(meta::check_constants(&dull).is_err());
        let mut tight = sample_meta();
        tight.sky.sun_glow = Some(8.0);
        assert!(meta::check_constants(&tight).is_err());
        let mut ember = sample_meta();
        ember.sky.sun_color = Some([2.4, 0.9, 0.3]);
        assert!(meta::check_constants(&ember).is_err());
    }

    #[test]
    fn absurd_bboxes_are_refused() {
        let mut inside_out = sample_meta();
        inside_out.frames.gameplay_1440p.post_bbox = [10, 10, 5, 20];
        assert!(meta::check_constants(&inside_out).is_err());
        let mut far = sample_meta();
        far.frames.zoom[0].post_bbox = [0, 0, 1, i64::MAX];
        assert!(meta::check_constants(&far).is_err());
        // Off the frame by less than a frame is a tall model at 8 m, not an error.
        let mut tall = sample_meta();
        tall.frames.zoom[0].post_bbox = [100, -300, 400, 1500];
        assert!(meta::check_constants(&tall).is_ok());
    }

    #[test]
    fn the_camera_looks_twelve_degrees_off_the_sun() {
        let (look, sun, gap) = meta::camera_sun_gap();
        assert!((look - 229.9).abs() < 0.1, "look azimuth {look}");
        assert!((sun - 242.3).abs() < 0.1, "sun azimuth {sun}");
        assert!((gap - 12.4).abs() < 0.1, "gap {gap}");
        assert!((meta::sun_elevation_deg() - 28.0).abs() < 0.2);
    }

    #[test]
    fn the_constants_are_the_engine_default_camera() {
        // The same orbit as `Camera::orbit(target, 46, -0.7, 0.95, aspect)` (the client's default).
        let camera = moba_render::Camera::orbit(glam::Vec3::ZERO, 46.0, -0.7, 0.95, 16.0 / 9.0);
        let expected = [
            meta::CAMERA_YAW.cos() * meta::CAMERA_PITCH.cos(),
            meta::CAMERA_PITCH.sin(),
            meta::CAMERA_YAW.sin() * meta::CAMERA_PITCH.cos(),
        ]
        .map(|value| value * meta::CAMERA_DISTANCE);
        let got = camera.position.to_array();
        for (got, want) in got.iter().zip(expected) {
            assert!((f64::from(*got) - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn l_ref_is_the_lane_grit_in_golden_crossing() {
        assert!((meta::l_ref() - 0.1208).abs() < 2e-4, "{}", meta::l_ref());
    }
}
