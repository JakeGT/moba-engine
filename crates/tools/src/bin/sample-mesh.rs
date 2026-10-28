#![allow(clippy::print_stdout)]
//! Generate the sample "sentinel" hero model shipped as `content/models/sentinel.glb`, and the
//! sample map meshes `content/models/crystal.glb` (a prop) and `content/models/rampart.glb`
//! (a wall section) used by the map visuals descriptors in `content/maps/` (#287).
//!
//! The model is built from code so its licence is the repository's own (no third-party art)
//! and so the bytes are reproducible: only `+ - * /` and `sqrt` are used (no
//! transcendental functions), which are correctly rounded IEEE operations on every platform.
//! A unit test checks the committed file still matches this generator.
//!
//! Usage: `cargo run -p moba-tools --bin sample-mesh -- [sentinel.glb [skinned.glb [crystal.glb
//! [rampart.glb]]]]`
//!
//! Conventions (glTF): metres, +Y up, the model faces +Z, feet at `y = 0`.

use std::fmt::Write as _;
use std::path::PathBuf;

/// `cos`/`sin` of `k * 45°`, spelled out so no platform libm is involved.
const H: f32 = 0.707_106_77;
const OCTAGON: [(f32, f32); 8] = [
    (1.0, 0.0),
    (H, H),
    (0.0, 1.0),
    (-H, H),
    (-1.0, 0.0),
    (-H, -H),
    (0.0, -1.0),
    (H, -H),
];

type V3 = [f32; 3];

/// Which joint(s) drive a vertex of the skinned sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Part {
    /// Rigidly bound to one joint.
    Joint(u8),
    /// The torso: blended from the hips to the chest by height.
    #[default]
    Torso,
}

// Joints of the skinned sample, in skin order. Bind pose: no rotation, so each inverse
// bind matrix is a pure translation by minus the joint's model-space position.
const HIPS: u8 = 0;
const CHEST: u8 = 1;
const HEAD: u8 = 2;
const LEG_NEG: u8 = 3;
const LEG_POS: u8 = 4;
const ARM_NEG: u8 = 5;
const ARM_POS: u8 = 6;
/// `(name, parent, model-space bind position)` per joint.
const JOINTS: [(&str, Option<u8>, V3); 7] = [
    ("hips", None, [0.0, 1.05, 0.0]),
    ("chest", Some(HIPS), [0.0, 1.6, 0.0]),
    ("head", Some(CHEST), [0.0, 2.72, 0.0]),
    ("leg_l", Some(HIPS), [-0.28, 1.1, 0.0]),
    ("leg_r", Some(HIPS), [0.28, 1.1, 0.0]),
    ("arm_l", Some(CHEST), [-0.8, 2.4, 0.0]),
    ("arm_r", Some(CHEST), [0.8, 2.4, 0.0]),
];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Flat-shaded polygon soup with per-face normals and `u16` indices.
#[derive(Default)]
struct Model {
    positions: Vec<V3>,
    normals: Vec<V3>,
    indices: Vec<u16>,
    /// Body part of each vertex (see [`Part`]); only the skinned encoding uses it.
    parts: Vec<Part>,
    /// Part assigned to vertices added from now on.
    part: Part,
}

impl Model {
    /// Adds a planar convex polygon; `outward` orients its normal.
    fn face(&mut self, corners: &[V3], outward: V3) {
        // Newell's method: robust for any planar polygon.
        let mut normal = [0.0f32; 3];
        for (index, current) in corners.iter().enumerate() {
            let next = corners[(index + 1) % corners.len()];
            normal[0] += (current[1] - next[1]) * (current[2] + next[2]);
            normal[1] += (current[2] - next[2]) * (current[0] + next[0]);
            normal[2] += (current[0] - next[0]) * (current[1] + next[1]);
        }
        let length = dot(normal, normal).sqrt();
        if length <= 1e-9 {
            return;
        }
        let mut normal = [normal[0] / length, normal[1] / length, normal[2] / length];
        let mut ordered: Vec<V3> = corners.to_vec();
        if dot(normal, outward) < 0.0 {
            normal = [-normal[0], -normal[1], -normal[2]];
            ordered.reverse();
        }
        let base = u16::try_from(self.positions.len()).expect("sample model fits u16");
        for corner in &ordered {
            self.positions.push(*corner);
            self.normals.push(normal);
            self.parts.push(self.part);
        }
        for index in 1..u16::try_from(ordered.len() - 1).expect("polygon size") {
            self.indices
                .extend_from_slice(&[base, base + index, base + index + 1]);
        }
    }

    /// Surface of revolution around a vertical axis through `(cx, cz)`.
    ///
    /// `profile` is `(y, radius)` from bottom to top; the ring is an octagon scaled by
    /// `(sx, sz)`. Non-zero end radii get flat caps.
    fn lathe(&mut self, cx: f32, cz: f32, sx: f32, sz: f32, profile: &[(f32, f32)]) {
        let ring = |y: f32, radius: f32, k: usize| {
            let (c, s) = OCTAGON[k % 8];
            [cx + c * radius * sx, y, cz + s * radius * sz]
        };
        for pair in profile.windows(2) {
            let ((y0, r0), (y1, r1)) = (pair[0], pair[1]);
            for k in 0..8 {
                let mid = (
                    OCTAGON[k].0 + OCTAGON[(k + 1) % 8].0,
                    OCTAGON[k].1 + OCTAGON[(k + 1) % 8].1,
                );
                let outward = [mid.0 * sx, 0.0, mid.1 * sz];
                let mut corners = Vec::with_capacity(4);
                corners.push(ring(y0, r0, k));
                if r0 > 0.0 {
                    corners.push(ring(y0, r0, k + 1));
                }
                corners.push(ring(y1, r1, k + 1));
                if r1 > 0.0 {
                    corners.push(ring(y1, r1, k));
                }
                // Every side face of a lathe leans away from the axis, so the horizontal
                // direction alone orients it (cones and domes included).
                self.face(&corners, outward);
            }
        }
        let (bottom_y, bottom_r) = profile[0];
        if bottom_r > 0.0 {
            let cap: Vec<V3> = (0..8).map(|k| ring(bottom_y, bottom_r, k)).collect();
            self.face(&cap, [0.0, -1.0, 0.0]);
        }
        let (top_y, top_r) = profile[profile.len() - 1];
        if top_r > 0.0 {
            let cap: Vec<V3> = (0..8).map(|k| ring(top_y, top_r, k)).collect();
            self.face(&cap, [0.0, 1.0, 0.0]);
        }
    }

    /// Axis-aligned box.
    fn cuboid(&mut self, min: V3, max: V3) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        self.face(
            &[[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            [1.0, 0.0, 0.0],
        );
        self.face(
            &[[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            [-1.0, 0.0, 0.0],
        );
        self.face(
            &[[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
            [0.0, 1.0, 0.0],
        );
        self.face(
            &[[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            [0.0, -1.0, 0.0],
        );
        self.face(
            &[[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 0.0, 1.0],
        );
        self.face(
            &[[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
            [0.0, 0.0, -1.0],
        );
    }

    /// Extrudes a convex outline in the XY plane between `z0` and `z1`.
    fn prism(&mut self, outline: &[(f32, f32)], z0: f32, z1: f32) {
        let back: Vec<V3> = outline.iter().map(|&(x, y)| [x, y, z0]).collect();
        let front: Vec<V3> = outline.iter().map(|&(x, y)| [x, y, z1]).collect();
        self.face(&front, [0.0, 0.0, 1.0]);
        self.face(&back, [0.0, 0.0, -1.0]);
        let (mut cx, mut cy) = (0.0, 0.0);
        for &(x, y) in outline {
            cx += x;
            cy += y;
        }
        let count = outline.len() as f32;
        let (cx, cy) = (cx / count, cy / count);
        for index in 0..outline.len() {
            let next = (index + 1) % outline.len();
            let (ax, ay) = outline[index];
            let (bx, by) = outline[next];
            let outward = [ax.midpoint(bx) - cx, ay.midpoint(by) - cy, 0.0];
            self.face(
                &[[ax, ay, z0], [bx, by, z0], [bx, by, z1], [ax, ay, z1]],
                outward,
            );
        }
    }
}

/// Builds the sentinel: an armoured spear-and-shield figure about 3.75 m tall.
fn sentinel() -> Model {
    let mut model = Model::default();
    // Legs and boots.
    for (x, leg) in [(-0.28, Part::Joint(LEG_NEG)), (0.28, Part::Joint(LEG_POS))] {
        model.part = leg;
        model.lathe(x, 0.0, 1.0, 1.0, &[(0.25, 0.2), (1.15, 0.25)]);
        model.cuboid([x - 0.2, 0.0, -0.22], [x + 0.2, 0.25, 0.32]);
    }
    // Armoured skirt and torso (flattened front to back).
    model.part = Part::Joint(HIPS);
    model.lathe(0.0, 0.0, 1.0, 0.7, &[(0.95, 0.62), (1.45, 0.52)]);
    model.part = Part::Torso;
    model.lathe(
        0.0,
        0.0,
        1.0,
        0.62,
        &[(1.45, 0.5), (2.15, 0.62), (2.55, 0.7), (2.72, 0.42)],
    );
    // Pauldrons and arms.
    for (x, arm) in [(-0.78, Part::Joint(ARM_NEG)), (0.78, Part::Joint(ARM_POS))] {
        model.part = Part::Joint(CHEST);
        model.lathe(
            x,
            0.0,
            1.0,
            1.0,
            &[(2.3, 0.3), (2.55, 0.36), (2.75, 0.26), (2.84, 0.0)],
        );
        model.part = arm;
        model.lathe(x * 1.05, 0.05, 1.0, 1.0, &[(1.35, 0.15), (2.3, 0.19)]);
    }
    // Helmeted head with a crest.
    model.part = Part::Joint(HEAD);
    model.lathe(
        0.0,
        0.0,
        1.0,
        1.0,
        &[
            (2.72, 0.0),
            (2.8, 0.22),
            (2.97, 0.31),
            (3.18, 0.3),
            (3.32, 0.2),
            (3.4, 0.0),
        ],
    );
    model.cuboid([-0.05, 3.2, -0.34], [0.05, 3.62, 0.2]);
    // Visor slit plate on the face (+Z).
    model.cuboid([-0.2, 3.0, 0.26], [0.2, 3.08, 0.32]);
    // Spear in the right hand: shaft and blade.
    model.part = Part::Joint(ARM_POS);
    model.cuboid([0.85, 0.15, 0.17], [0.93, 3.35, 0.25]);
    model.lathe(
        0.89,
        0.21,
        1.0,
        0.45,
        &[(3.35, 0.13), (3.48, 0.15), (3.78, 0.0)],
    );
    // Kite shield on the left forearm, facing forward.
    model.part = Part::Joint(ARM_NEG);
    model.prism(
        &[
            (-1.15, 2.25),
            (-0.35, 2.25),
            (-0.3, 1.75),
            (-0.75, 1.05),
            (-1.2, 1.75),
        ],
        0.42,
        0.52,
    );
    model
}

/// A crystal cluster prop about 3 m tall: one tall octagonal spire and two shorter ones.
fn crystal() -> Model {
    let mut model = Model::default();
    model.lathe(
        0.0,
        0.0,
        1.0,
        1.0,
        &[(0.0, 0.3), (0.35, 0.5), (2.2, 0.38), (3.0, 0.0)],
    );
    model.lathe(
        0.62,
        0.25,
        1.0,
        0.8,
        &[(0.0, 0.18), (0.2, 0.28), (1.25, 0.22), (1.8, 0.0)],
    );
    model.lathe(
        -0.5,
        -0.35,
        0.8,
        1.0,
        &[(0.0, 0.14), (0.15, 0.22), (0.85, 0.17), (1.3, 0.0)],
    );
    model
}

/// A crenellated wall section authored along +X (4 m long, 3 m tall, 1.2 m thick) on a wider
/// footing. Map visuals stretch it along each wall segment.
fn rampart() -> Model {
    let mut model = Model::default();
    model.cuboid([-2.0, 0.0, -0.75], [2.0, 0.4, 0.75]);
    model.cuboid([-2.0, 0.4, -0.6], [2.0, 2.4, 0.6]);
    for x in [-1.75, -0.75, 0.25, 1.25] {
        model.cuboid([x, 2.4, -0.6], [x + 0.5, 3.0, 0.6]);
    }
    model
}

fn bounds(points: &[V3]) -> (V3, V3) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in points {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    (min, max)
}

/// Serialises a model as a single-mesh binary glTF 2.0 file whose node and mesh are `name`.
fn encode_glb(model: &Model, name: &str) -> Vec<u8> {
    let mut bin = Vec::new();
    for position in &model.positions {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let normals_offset = bin.len();
    for normal in &model.normals {
        for value in normal {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let indices_offset = bin.len();
    for index in &model.indices {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    let indices_length = bin.len() - indices_offset;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let (min, max) = bounds(&model.positions);
    let count = model.positions.len();
    let mut json = String::new();
    write!(
        json,
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"moba-tools sample-mesh","#,
            r#""copyright":"MOBA Engine contributors, MIT OR Apache-2.0"}},"#,
            r#""scene":0,"scenes":[{{"nodes":[0]}}],"#,
            r#""nodes":[{{"name":"{}","mesh":0}}],"#,
            r#""meshes":[{{"name":"{}","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":2}}]}}],"#,
            r#""buffers":[{{"byteLength":{}}}],"#,
            r#""bufferViews":["#,
            r#"{{"buffer":0,"byteOffset":0,"byteLength":{},"target":34962}},"#,
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{},"target":34962}},"#,
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{},"target":34963}}],"#,
            r#""accessors":["#,
            r#"{{"bufferView":0,"componentType":5126,"count":{},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}},"#,
            r#"{{"bufferView":1,"componentType":5126,"count":{},"type":"VEC3"}},"#,
            r#"{{"bufferView":2,"componentType":5123,"count":{},"type":"SCALAR"}}]}}"#
        ),
        name,
        name,
        bin.len(),
        normals_offset,
        normals_offset,
        indices_offset - normals_offset,
        indices_offset,
        indices_length,
        count,
        min[0],
        min[1],
        min[2],
        max[0],
        max[1],
        max[2],
        count,
        model.indices.len(),
    )
    .expect("write to string");
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(total).expect("glb size").to_le_bytes());
    out.extend_from_slice(&u32::try_from(json.len()).expect("json size").to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&u32::try_from(bin.len()).expect("bin size").to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// Per-vertex `(joints, weights)` of the skinned sample (`UNORM8` weights summing to 255).
fn influence(part: Part, position: V3) -> ([u8; 4], [u8; 4]) {
    match part {
        Part::Joint(joint) => ([joint, 0, 0, 0], [255, 0, 0, 0]),
        Part::Torso => {
            // 0 at the waist (y = 1.45) to 255 at the shoulders (y = 2.55). `as` truncates, which
            // is exact, after correctly rounded arithmetic.
            let chest = ((position[1] - 1.45) / 1.1 * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            ([CHEST, HIPS, 0, 0], [chest, 255 - chest, 0, 0])
        }
    }
}

/// Unit quaternion (`xyzw`) about axis `axis` (0 = X, 1 = Y) by `2·atan(s)`, using only
/// `+ * / sqrt` so the bytes are reproducible everywhere.
fn quat(axis: usize, s: f32) -> [f32; 4] {
    let norm = (1.0 + s * s).sqrt();
    let mut q = [0.0, 0.0, 0.0, 1.0 / norm];
    q[axis] = s / norm;
    q
}

/// Key times of the walk cycle (seconds): contact, passing, contact, passing, loop.
const WALK_TIMES: [f32; 5] = [0.0, 0.2, 0.4, 0.6, 0.8];

/// One animated joint property of the walk clip.
struct WalkTrack {
    joint: u8,
    rotation: bool,
    /// Rotation keys are `xyzw`, translation keys `xyz_`.
    keys: [[f32; 4]; 5],
}

/// The walk clip: legs and arms counter-swing, the chest twists, the hips bob.
fn walk_tracks() -> [WalkTrack; 6] {
    let swing = |joint: u8, axis: usize, s: f32| WalkTrack {
        joint,
        rotation: true,
        keys: [
            quat(axis, s),
            quat(axis, 0.0),
            quat(axis, -s),
            quat(axis, 0.0),
            quat(axis, s),
        ],
    };
    let (low, high) = ([0.0, 1.05, 0.0, 0.0], [0.0, 1.13, 0.0, 0.0]);
    [
        swing(LEG_NEG, 0, 0.3),
        swing(LEG_POS, 0, -0.3),
        swing(ARM_NEG, 0, -0.2),
        swing(ARM_POS, 0, 0.2),
        swing(CHEST, 1, 0.06),
        WalkTrack {
            joint: HIPS,
            rotation: false,
            keys: [low, high, low, high, low],
        },
    ]
}

/// Binary chunk plus the JSON for its buffer views and accessors.
#[derive(Default)]
struct Chunks {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Chunks {
    /// Appends a 4-byte-aligned buffer view and one accessor over it; returns the accessor.
    fn accessor(&mut self, bytes: &[u8], target: &str, layout: &str) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let view = self.views.len();
        self.views.push(format!(
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{}{target}}}"#,
            self.bin.len(),
            bytes.len()
        ));
        self.bin.extend_from_slice(bytes);
        self.accessors
            .push(format!(r#"{{"bufferView":{view},{layout}}}"#));
        self.accessors.len() - 1
    }
}

fn f32_bytes<'a>(values: impl IntoIterator<Item = &'a f32>) -> Vec<u8> {
    values
        .into_iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Serialises the model as a skinned, animated binary glTF 2.0 file: seven joints, the
/// same geometry in bind pose and a looping 0.8 s walk clip.
#[allow(clippy::too_many_lines)] // One linear GLB layout.
fn encode_skinned_glb(model: &Model) -> Vec<u8> {
    const VERTEX: &str = r#","target":34962"#;
    let count = model.positions.len();
    let (min, max) = bounds(&model.positions);
    let mut chunks = Chunks::default();
    let position = chunks.accessor(
        &f32_bytes(model.positions.iter().flatten()),
        VERTEX,
        &format!(
            r#""componentType":5126,"count":{count},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]"#,
            min[0], min[1], min[2], max[0], max[1], max[2]
        ),
    );
    let normal = chunks.accessor(
        &f32_bytes(model.normals.iter().flatten()),
        VERTEX,
        &format!(r#""componentType":5126,"count":{count},"type":"VEC3""#),
    );
    let influences: Vec<([u8; 4], [u8; 4])> = model
        .parts
        .iter()
        .zip(&model.positions)
        .map(|(&part, &position)| influence(part, position))
        .collect();
    let joint_bytes: Vec<u8> = influences.iter().flat_map(|(joints, _)| *joints).collect();
    let weight_bytes: Vec<u8> = influences
        .iter()
        .flat_map(|(_, weights)| *weights)
        .collect();
    let joints = chunks.accessor(
        &joint_bytes,
        VERTEX,
        &format!(r#""componentType":5121,"count":{count},"type":"VEC4""#),
    );
    let weights = chunks.accessor(
        &weight_bytes,
        VERTEX,
        &format!(r#""componentType":5121,"normalized":true,"count":{count},"type":"VEC4""#),
    );
    let inverse_bind: Vec<f32> = JOINTS
        .iter()
        .flat_map(|(_, _, [x, y, z])| {
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -x, -y, -z, 1.0,
            ]
        })
        .collect();
    let inverse_bind = chunks.accessor(
        &f32_bytes(&inverse_bind),
        "",
        &format!(
            r#""componentType":5126,"count":{},"type":"MAT4""#,
            JOINTS.len()
        ),
    );
    let times = chunks.accessor(
        &f32_bytes(&WALK_TIMES),
        "",
        &format!(
            r#""componentType":5126,"count":{},"type":"SCALAR","min":[{}],"max":[{}]"#,
            WALK_TIMES.len(),
            WALK_TIMES[0],
            WALK_TIMES[WALK_TIMES.len() - 1]
        ),
    );
    let mut samplers = Vec::new();
    let mut channels = Vec::new();
    for track in walk_tracks() {
        let (kind, lanes, path) = if track.rotation {
            ("VEC4", 4, "rotation")
        } else {
            ("VEC3", 3, "translation")
        };
        let values = f32_bytes(track.keys.iter().flat_map(|key| &key[..lanes]));
        let output = chunks.accessor(
            &values,
            "",
            &format!(
                r#""componentType":5126,"count":{},"type":"{kind}""#,
                track.keys.len()
            ),
        );
        channels.push(format!(
            r#"{{"sampler":{},"target":{{"node":{},"path":"{path}"}}}}"#,
            samplers.len(),
            track.joint + 1
        ));
        samplers.push(format!(
            r#"{{"input":{times},"output":{output},"interpolation":"LINEAR"}}"#
        ));
    }
    let index_bytes: Vec<u8> = model
        .indices
        .iter()
        .flat_map(|index| index.to_le_bytes())
        .collect();
    let indices = chunks.accessor(
        &index_bytes,
        r#","target":34963"#,
        &format!(
            r#""componentType":5123,"count":{},"type":"SCALAR""#,
            model.indices.len()
        ),
    );
    while !chunks.bin.len().is_multiple_of(4) {
        chunks.bin.push(0);
    }
    // Node 0 is the skinned mesh; joint `j` is node `j + 1`.
    let mut nodes = vec![String::from(r#"{"name":"sentinel","mesh":0,"skin":0}"#)];
    for (index, (name, parent, [x, y, z])) in JOINTS.iter().enumerate() {
        let origin = parent.map_or([0.0; 3], |parent| JOINTS[usize::from(parent)].2);
        let children: Vec<String> = JOINTS
            .iter()
            .enumerate()
            .filter(|(_, (_, child_parent, _))| *child_parent == u8::try_from(index).ok())
            .map(|(child, _)| (child + 1).to_string())
            .collect();
        let children = if children.is_empty() {
            String::new()
        } else {
            format!(r#","children":[{}]"#, children.join(","))
        };
        nodes.push(format!(
            r#"{{"name":"{name}","translation":[{},{},{}]{children}}}"#,
            x - origin[0],
            y - origin[1],
            z - origin[2]
        ));
    }
    let joint_nodes: Vec<String> = (1..=JOINTS.len()).map(|node| node.to_string()).collect();
    let json = format!(
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"moba-tools sample-mesh","#,
            r#""copyright":"MOBA Engine contributors, MIT OR Apache-2.0"}},"#,
            r#""scene":0,"scenes":[{{"nodes":[0,1]}}],"#,
            r#""nodes":[{}],"#,
            r#""meshes":[{{"name":"sentinel","primitives":[{{"attributes":"#,
            r#"{{"POSITION":{},"NORMAL":{},"JOINTS_0":{},"WEIGHTS_0":{}}},"indices":{}}}]}}],"#,
            r#""skins":[{{"name":"sentinel","skeleton":1,"joints":[{}],"inverseBindMatrices":{}}}],"#,
            r#""animations":[{{"name":"walk","samplers":[{}],"channels":[{}]}}],"#,
            r#""buffers":[{{"byteLength":{}}}],"bufferViews":[{}],"accessors":[{}]}}"#
        ),
        nodes.join(","),
        position,
        normal,
        joints,
        weights,
        indices,
        joint_nodes.join(","),
        inverse_bind,
        samplers.join(","),
        channels.join(","),
        chunks.bin.len(),
        chunks.views.join(","),
        chunks.accessors.join(","),
    );
    container(json.into_bytes(), &chunks.bin)
}

/// Wraps a JSON and a BIN chunk in a GLB container (JSON padded with spaces).
fn container(mut json: Vec<u8>, bin: &[u8]) -> Vec<u8> {
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(total).expect("glb size").to_le_bytes());
    out.extend_from_slice(&u32::try_from(json.len()).expect("json size").to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&u32::try_from(bin.len()).expect("bin size").to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

fn write(output: &std::path::Path, bytes: &[u8], model: &Model) {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("create model directory");
    }
    std::fs::write(output, bytes).expect("write model");
    println!(
        "wrote {} ({} vertices, {} triangles, {} bytes)",
        output.display(),
        model.positions.len(),
        model.indices.len() / 3,
        bytes.len()
    );
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().map_or_else(
        || PathBuf::from("content/models/sentinel.glb"),
        PathBuf::from,
    );
    let skinned_output = args.next().map_or_else(
        || PathBuf::from("content/models/sentinel-skinned.glb"),
        PathBuf::from,
    );
    let crystal_output = args.next().map_or_else(
        || PathBuf::from("content/models/crystal.glb"),
        PathBuf::from,
    );
    let rampart_output = args.next().map_or_else(
        || PathBuf::from("content/models/rampart.glb"),
        PathBuf::from,
    );
    let model = sentinel();
    write(&output, &encode_glb(&model, "sentinel"), &model);
    write(&skinned_output, &encode_skinned_glb(&model), &model);
    let crystal = crystal();
    write(&crystal_output, &encode_glb(&crystal, "crystal"), &crystal);
    let rampart = rampart();
    write(&rampart_output, &encode_glb(&rampart, "rampart"), &rampart);
}

#[cfg(test)]
mod tests {
    #[test]
    fn committed_model_matches_the_generator_and_loads() {
        let generated = super::encode_glb(&super::sentinel(), "sentinel");
        let committed = include_bytes!("../../../../content/models/sentinel.glb");
        assert!(
            generated == committed,
            "content/models/sentinel.glb is stale; run `cargo run -p moba-tools --bin sample-mesh`"
        );
        let mesh = moba_assets::load_glb(&generated).expect("generated model loads");
        let (min, max) = mesh.bounds();
        assert!(min[1].abs() < 1e-6, "feet on the ground: {min:?}");
        assert!(max[1] > 3.7 && max[1] < 3.8, "height: {max:?}");
        assert!(max[2] > 0.5, "shield faces +Z: {max:?}");
        assert!(mesh.skin().is_none());
    }

    #[test]
    fn committed_map_meshes_match_the_generator_and_load() {
        for (name, model, committed, height) in [
            (
                "crystal",
                super::crystal(),
                &include_bytes!("../../../../content/models/crystal.glb")[..],
                3.0f32,
            ),
            (
                "rampart",
                super::rampart(),
                &include_bytes!("../../../../content/models/rampart.glb")[..],
                3.0,
            ),
        ] {
            let generated = super::encode_glb(&model, name);
            assert!(
                generated == committed,
                "content/models/{name}.glb is stale; run `cargo run -p moba-tools --bin sample-mesh`"
            );
            let mesh = moba_assets::load_glb(&generated).expect("generated model loads");
            let (min, max) = mesh.bounds();
            assert!(min[1].abs() < 1e-6, "{name} stands on the ground: {min:?}");
            assert!((max[1] - height).abs() < 1e-6, "{name} height: {max:?}");
        }
    }

    #[test]
    fn committed_skinned_model_matches_the_generator_and_animates() {
        let generated = super::encode_skinned_glb(&super::sentinel());
        let committed = include_bytes!("../../../../content/models/sentinel-skinned.glb");
        assert!(
            generated == committed,
            "content/models/sentinel-skinned.glb is stale; run `cargo run -p moba-tools --bin sample-mesh`"
        );
        let mesh = moba_assets::load_glb(&generated).expect("skinned model loads");
        let static_mesh = moba_assets::load_glb(&super::encode_glb(&super::sentinel(), "sentinel"))
            .expect("static model loads");
        // Same bind-pose geometry as the static sample.
        assert_eq!(mesh.positions(), static_mesh.positions());
        assert_eq!(mesh.indices(), static_mesh.indices());
        let skin = mesh.skin().expect("skin");
        let skeleton = skin.skeleton();
        assert_eq!(skeleton.joint_count(), super::JOINTS.len());
        let clip = skeleton.clip().expect("walk clip");
        assert!((clip.duration() - 0.8).abs() < 1e-6);
        assert_eq!(clip.tracks().len(), 6);
        // Some torso vertices blend two joints.
        assert!(
            skin.weights()
                .iter()
                .any(|lanes| lanes[0] != 0 && lanes[1] != 0)
        );
        // The rest pose is the bind pose (identity skinning matrices).
        let mut pose = vec![[[0.0f32; 4]; 3]; skeleton.joint_count()];
        assert!(skeleton.sample(0.0, 0.0, &mut pose));
        for joint in &pose {
            for (row, expected) in
                joint
                    .iter()
                    .zip([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
            {
                assert!(
                    row[..3]
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() < 1e-6)
                );
                assert!(row[3].abs() < 1e-6);
            }
        }
        // The contact pose swings the foot of a leg forwards or backwards.
        assert!(skeleton.sample(0.0, 1.0, &mut pose));
        let foot = [-0.28f32, 0.0, 0.0, 1.0];
        let leg = pose[usize::from(super::LEG_NEG)];
        let z: f32 = leg[2].iter().zip(foot).map(|(a, b)| a * b).sum();
        assert!(z.abs() > 0.4, "contact pose swings the foot: z = {z}");
    }
}
