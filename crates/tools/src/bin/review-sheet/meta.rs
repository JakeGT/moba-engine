//! `render_meta.json` (written by `scripts/blender/review_render.py`) and the constants it must
//! carry: the engine's default camera and the bible's Golden Crossing light.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// `Camera::orbit(target, 46.0, -0.7, 0.95, aspect)` (`crates/client/src/app/scene_build.rs`).
pub(crate) const CAMERA_YAW: f64 = -0.7;
/// The default camera's pitch (radians).
pub(crate) const CAMERA_PITCH: f64 = 0.95;
/// The default follow distance (`network_view.rs`).
pub(crate) const CAMERA_DISTANCE: f64 = 46.0;
/// The zoom camera's distance (silhouettes-and-scale.md, material-vocabulary.md).
pub(crate) const ZOOM_DISTANCE: f64 = 8.0;
/// `Camera::orbit`'s vertical field of view.
pub(crate) const CAMERA_FOV_Y_DEG: f64 = 60.0;
/// Golden Crossing `sun_dir` (lighting-mood.md, option A): towards the sun.
pub(crate) const SUN_DIR: [f64; 3] = [-0.885, 0.532, 0.465];
/// Golden Crossing sun colour.
pub(crate) const SUN_COLOR: [f64; 3] = [1.0, 0.80, 0.62];
/// Golden Crossing sun intensity.
pub(crate) const SUN_INTENSITY: f64 = 1.5;
/// Golden Crossing image-based light.
pub(crate) const IBL: f64 = 0.35;
/// Golden Crossing analytic sky: zenith, horizon, ground.
pub(crate) const SKY: [[f64; 3]; 3] = [[0.16, 0.22, 0.36], [0.95, 0.62, 0.38], [0.18, 0.11, 0.07]];
/// Golden Crossing's environment-bake sun glow: the lobe's linear colour (`sun_color` of the
/// env bake, not the `[lighting]` sun) and exponent (lighting-mood.md option A; env-bake's
/// `sky.rs` adds `sun_color × max(dot(d, sun), 0)^sun_glow × smoothstep(-0.12, 0.04, y)`).
pub(crate) const SKY_SUN_COLOR: [f64; 3] = [2.6, 1.55, 0.85];
/// The glow lobe's exponent.
pub(crate) const SKY_SUN_GLOW: f64 = 6.0;
/// Largest frame side a bbox may name, in pixels (the 1080p4x silhouettes are 7680 wide).
const MAX_FRAME_SIDE: i64 = 8192;
/// The lane grit that defines `L_ref` (palette-and-value §6, material-vocabulary §1.1).
pub(crate) const LANE_HEX: &str = "#906B58";
/// The review ground: ash (palette-and-value §6).
pub(crate) const ASH_HEX: &str = "#6B6560";
/// The measured gameplay ground mean `OKLab` L (lighting-mood.md, first measurements).
pub(crate) const GROUND_L: f64 = 0.50;
/// The reference post's height.
pub(crate) const POST_M: f64 = 1.8;

#[derive(Debug, Deserialize)]
pub(crate) struct Meta {
    pub(crate) schema: u32,
    pub(crate) name: String,
    pub(crate) renderer: String,
    pub(crate) device: String,
    pub(crate) blender: String,
    pub(crate) samples: u32,
    pub(crate) camera: CameraMeta,
    pub(crate) sun: SunMeta,
    pub(crate) sky: SkyMeta,
    pub(crate) ground_hex: String,
    pub(crate) lane_hex: String,
    pub(crate) l_ref: f64,
    pub(crate) post_m: f64,
    pub(crate) view_transform: String,
    pub(crate) exposure_ev: f64,
    pub(crate) ground_l_target: f64,
    pub(crate) ground_l_calibrated: f64,
    pub(crate) id_colours: BTreeMap<String, [u8; 3]>,
    pub(crate) post_colour: [u8; 3],
    pub(crate) models: Vec<ModelMeta>,
    pub(crate) frames: Frames,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CameraMeta {
    pub(crate) yaw: f64,
    pub(crate) pitch: f64,
    pub(crate) distance_m: f64,
    pub(crate) zoom_distance_m: f64,
    pub(crate) fov_y_deg: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SunMeta {
    pub(crate) dir: [f64; 3],
    pub(crate) colour: [f64; 3],
    pub(crate) intensity: f64,
    pub(crate) elevation_deg: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SkyMeta {
    pub(crate) ibl: f64,
    pub(crate) zenith: [f64; 3],
    pub(crate) horizon: [f64; 3],
    pub(crate) ground: [f64; 3],
    /// The glow lobe's colour (absent in renders made before the lobe: refused).
    #[serde(default)]
    pub(crate) sun_color: Option<[f64; 3]>,
    /// The glow lobe's exponent.
    #[serde(default)]
    pub(crate) sun_glow: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelMeta {
    pub(crate) name: String,
    pub(crate) glb: PathBuf,
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) size_class: String,
    pub(crate) faction: String,
    pub(crate) pose: PoseMeta,
    #[serde(default)]
    pub(crate) tell: Vec<String>,
    #[serde(default)]
    pub(crate) weapon: Vec<String>,
    #[serde(default)]
    pub(crate) weapon_kind: String,
    #[serde(default)]
    pub(crate) emissive: EmissiveMeta,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PoseMeta {
    pub(crate) clip: String,
    pub(crate) frame: i64,
    pub(crate) applied: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct EmissiveMeta {
    #[serde(rename = "use")]
    pub(crate) usage: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelBox {
    pub(crate) name: String,
    pub(crate) bbox: [i64; 4],
}

#[derive(Debug, Deserialize)]
pub(crate) struct Frame {
    pub(crate) file: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) distance_m: f64,
    pub(crate) models: Vec<ModelBox>,
    pub(crate) post_bbox: [i64; 4],
    #[serde(default)]
    pub(crate) ids: Option<String>,
    #[serde(default)]
    pub(crate) emit: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Turntable {
    pub(crate) model: String,
    pub(crate) files: Vec<String>,
    pub(crate) yaws_deg: Vec<f64>,
    pub(crate) distance_m: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Silhouettes {
    pub(crate) model: String,
    pub(crate) res: String,
    pub(crate) files: Vec<String>,
    pub(crate) yaws_deg: Vec<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Frames {
    #[serde(rename = "46m_1440p")]
    pub(crate) gameplay_1440p: Frame,
    #[serde(rename = "46m_2160p")]
    pub(crate) gameplay_2160p: Frame,
    pub(crate) zoom: Vec<Frame>,
    pub(crate) lineup_1440p: Frame,
    pub(crate) turntable: Vec<Turntable>,
    pub(crate) silhouettes: Vec<Silhouettes>,
}

impl Meta {
    /// Every single frame with a bbox list, and its key.
    pub(crate) fn frame_list(&self) -> Vec<(&Frame, &'static str)> {
        let mut frames = vec![
            (&self.frames.gameplay_1440p, "46m_1440p"),
            (&self.frames.gameplay_2160p, "46m_2160p"),
            (&self.frames.lineup_1440p, "lineup_1440p"),
        ];
        frames.extend(self.frames.zoom.iter().map(|frame| (frame, "zoom")));
        frames
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| close(*x, y))
}

/// Golden Crossing's sun elevation (degrees).
pub(crate) fn sun_elevation_deg() -> f64 {
    let [x, y, z] = SUN_DIR;
    (y / (x * x + y * y + z * z).sqrt()).asin().to_degrees()
}

/// Material-vocabulary §1.1: `luminance(lane grit) × sun intensity × sin(sun elevation)`.
pub(crate) fn l_ref() -> f64 {
    let lane = crate::imaging::parse_hex(LANE_HEX).unwrap_or([0.0; 3]);
    let linear = lane.map(crate::imaging::srgb_to_linear);
    f64::from(crate::imaging::luminance(linear))
        * SUN_INTENSITY
        * sun_elevation_deg().to_radians().sin()
}

/// Compass azimuth (degrees, 0 = -z, clockwise towards +x) of an engine-space direction.
pub(crate) fn azimuth_deg(direction: [f64; 3]) -> f64 {
    direction[0]
        .atan2(-direction[2])
        .to_degrees()
        .rem_euclid(360.0)
}

/// The default camera's horizontal look direction and the sun's, as azimuths, and the angle
/// between them: the camera looks almost straight into the sun, so units show their shade side.
pub(crate) fn camera_sun_gap() -> (f64, f64, f64) {
    let offset = [
        CAMERA_YAW.cos() * CAMERA_PITCH.cos(),
        CAMERA_PITCH.sin(),
        CAMERA_YAW.sin() * CAMERA_PITCH.cos(),
    ];
    let look = azimuth_deg([-offset[0], 0.0, -offset[2]]);
    let sun = azimuth_deg(SUN_DIR);
    let gap = (sun - look).abs().min(360.0 - (sun - look).abs());
    (look, sun, gap)
}

/// Refuses a render made with any camera, light or reference other than the engine's and the
/// bible's (a review signed off under other numbers would be meaningless).
pub(crate) fn check_constants(meta: &Meta) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut need = |ok: bool, what: &str| {
        if !ok {
            errors.push(format!("render_meta.json: {what}"));
        }
    };
    need(meta.schema == 1, "schema must be 1");
    let camera = &meta.camera;
    need(
        close(camera.yaw, CAMERA_YAW)
            && close(camera.pitch, CAMERA_PITCH)
            && close(camera.distance_m, CAMERA_DISTANCE)
            && close(camera.zoom_distance_m, ZOOM_DISTANCE)
            && close(camera.fov_y_deg, CAMERA_FOV_Y_DEG),
        "camera is not the engine default (yaw -0.7, pitch 0.95, 46 m, zoom 8 m, 60 deg)",
    );
    need(
        close3(meta.sun.dir, SUN_DIR)
            && close3(meta.sun.colour, SUN_COLOR)
            && close(meta.sun.intensity, SUN_INTENSITY)
            && (meta.sun.elevation_deg - sun_elevation_deg()).abs() < 1e-6,
        "sun is not Golden Crossing (lighting-mood.md option A)",
    );
    need(
        close(meta.sky.ibl, IBL)
            && close3(meta.sky.zenith, SKY[0])
            && close3(meta.sky.horizon, SKY[1])
            && close3(meta.sky.ground, SKY[2]),
        "sky is not Golden Crossing",
    );
    need(
        meta.sky
            .sun_color
            .is_some_and(|colour| close3(colour, SKY_SUN_COLOR))
            && meta
                .sky
                .sun_glow
                .is_some_and(|glow| close(glow, SKY_SUN_GLOW)),
        "sky has no Golden Crossing sun glow (sun_color 2.6, 1.55, 0.85; sun_glow 6)",
    );
    need(
        meta.lane_hex.eq_ignore_ascii_case(LANE_HEX) && (meta.l_ref - l_ref()).abs() < 1e-5,
        "L_ref is not the lane grit in Golden Crossing",
    );
    need(
        meta.ground_hex.eq_ignore_ascii_case(ASH_HEX),
        "ground is not ash #6B6560",
    );
    need(close(meta.post_m, POST_M), "reference post is not 1.8 m");
    need(
        close(meta.ground_l_target, GROUND_L)
            && (meta.ground_l_calibrated - GROUND_L).abs() <= 0.01,
        "exposure is not calibrated to ground L 0.50",
    );
    need(meta.view_transform == "AgX", "view transform is not AgX");
    need(
        close(meta.frames.gameplay_1440p.distance_m, CAMERA_DISTANCE)
            && close(meta.frames.gameplay_2160p.distance_m, CAMERA_DISTANCE)
            && meta
                .frames
                .zoom
                .iter()
                .all(|frame| close(frame.distance_m, ZOOM_DISTANCE)),
        "46 m or 8 m frames were taken at another distance",
    );
    need(
        (
            meta.frames.gameplay_1440p.width,
            meta.frames.gameplay_1440p.height,
        ) == (2560, 1440)
            && (
                meta.frames.gameplay_2160p.width,
                meta.frames.gameplay_2160p.height,
            ) == (3840, 2160),
        "46 m frames are not 2560x1440 and 3840x2160",
    );
    for (frame, what) in meta.frame_list() {
        // A projected bbox may run off the frame (a tall model at 8 m), but not by more than a
        // frame, and never inside out.
        let (w, h) = (i64::from(frame.width), i64::from(frame.height));
        let sane = |bbox: [i64; 4]| {
            let [x0, y0, x1, y1] = bbox;
            x0 < x1 && y0 < y1 && -w <= x0 && x1 <= 2 * w && -h <= y0 && y1 <= 2 * h
        };
        need(
            (1..=MAX_FRAME_SIDE).contains(&w)
                && (1..=MAX_FRAME_SIDE).contains(&h)
                && frame.models.iter().all(|model| sane(model.bbox))
                && sane(frame.post_bbox),
            &format!(
                "{what} ({}): a bbox or the frame size is out of range",
                frame.file
            ),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The camera, sun, finding and render lines every sheet prints.
pub(crate) fn camera_and_sun_lines(meta: &Meta) -> [String; 4] {
    let (look, sun, gap) = camera_sun_gap();
    [
        format!(
            "CAMERA  engine default Camera::orbit yaw {:.2} pitch {:.2} distance {:.0} m (zoom {:.0} m), fov {:.0} deg",
            meta.camera.yaw,
            meta.camera.pitch,
            meta.camera.distance_m,
            meta.camera.zoom_distance_m,
            meta.camera.fov_y_deg
        ),
        format!(
            "SUN  Golden Crossing dir ({:.3}, {:.3}, {:.3}) colour ({:.2}, {:.2}, {:.2}) x{:.1}, elevation {:.1} deg, IBL {:.2}; L_ref {:.4}",
            meta.sun.dir[0],
            meta.sun.dir[1],
            meta.sun.dir[2],
            meta.sun.colour[0],
            meta.sun.colour[1],
            meta.sun.colour[2],
            meta.sun.intensity,
            meta.sun.elevation_deg,
            meta.sky.ibl,
            meta.l_ref
        ),
        format!(
            "FINDING  camera looks to azimuth {look:.1} deg, sun at {sun:.1} deg: {gap:.1} deg apart, so assets face the camera from their shade side (reported, not fixed)"
        ),
        format!(
            "RENDER  exposure {:+.2} EV (bare ash ground L {:.3}), {} {} on {}, {} samples, Blender {}; no grading LUT, fog or rims",
            meta.exposure_ev,
            meta.ground_l_calibrated,
            meta.renderer,
            meta.view_transform,
            meta.device,
            meta.samples,
            meta.blender
        ),
    ]
}
