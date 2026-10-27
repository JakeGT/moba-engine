#![allow(clippy::print_stdout)]
//! Generate the sample "sentinel" hero model shipped as `content/models/sentinel.glb`.
//!
//! The model is built from code so its licence is the repository's own (no third-party art)
//! and so the bytes are reproducible: only `+ - * /` and `sqrt` are used (no
//! transcendental functions), which are correctly rounded IEEE operations on every platform.
//! A unit test checks the committed file still matches this generator.
//!
//! Usage: `cargo run -p moba-tools --bin sample-mesh -- [out.glb]`
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

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Flat-shaded polygon soup with per-face normals and `u16` indices.
#[derive(Default)]
struct Model {
    positions: Vec<V3>,
    normals: Vec<V3>,
    indices: Vec<u16>,
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
    for x in [-0.28, 0.28] {
        model.lathe(x, 0.0, 1.0, 1.0, &[(0.25, 0.2), (1.15, 0.25)]);
        model.cuboid([x - 0.2, 0.0, -0.22], [x + 0.2, 0.25, 0.32]);
    }
    // Armoured skirt and torso (flattened front to back).
    model.lathe(0.0, 0.0, 1.0, 0.7, &[(0.95, 0.62), (1.45, 0.52)]);
    model.lathe(
        0.0,
        0.0,
        1.0,
        0.62,
        &[(1.45, 0.5), (2.15, 0.62), (2.55, 0.7), (2.72, 0.42)],
    );
    // Pauldrons and arms.
    for x in [-0.78, 0.78] {
        model.lathe(
            x,
            0.0,
            1.0,
            1.0,
            &[(2.3, 0.3), (2.55, 0.36), (2.75, 0.26), (2.84, 0.0)],
        );
        model.lathe(x * 1.05, 0.05, 1.0, 1.0, &[(1.35, 0.15), (2.3, 0.19)]);
    }
    // Helmeted head with a crest.
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
    model.cuboid([0.85, 0.15, 0.17], [0.93, 3.35, 0.25]);
    model.lathe(
        0.89,
        0.21,
        1.0,
        0.45,
        &[(3.35, 0.13), (3.48, 0.15), (3.78, 0.0)],
    );
    // Kite shield on the left forearm, facing forward.
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

/// Serialises a model as a single-mesh binary glTF 2.0 file.
fn encode_glb(model: &Model) -> Vec<u8> {
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
            r#""nodes":[{{"name":"sentinel","mesh":0}}],"#,
            r#""meshes":[{{"name":"sentinel","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":2}}]}}],"#,
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

fn main() {
    let output = std::env::args_os().nth(1).map_or_else(
        || PathBuf::from("content/models/sentinel.glb"),
        PathBuf::from,
    );
    let model = sentinel();
    let bytes = encode_glb(&model);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("create model directory");
    }
    std::fs::write(&output, &bytes).expect("write model");
    println!(
        "wrote {} ({} vertices, {} triangles, {} bytes)",
        output.display(),
        model.positions.len(),
        model.indices.len() / 3,
        bytes.len()
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn committed_model_matches_the_generator_and_loads() {
        let generated = super::encode_glb(&super::sentinel());
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
    }
}
