#![allow(clippy::print_stdout, clippy::print_stderr)]
//! Bake index-only mesh levels of detail with meshoptimizer (R8, ADR-0018 §3).
//!
//! ```text
//! mesh-bake lod <model.glb> [--out FILE.lod] [--ratios 0.5,0.25,0.125] [--keep-seams]
//!               [--protect-uv-seams] [--lock-border]
//! mesh-bake info <model.glb>
//! ```
//!
//! Built only with the `mesh-bake` feature (meshoptimizer is C++; the released tools do not
//! compile it): `cargo run -p moba-tools --features mesh-bake --bin mesh-bake`.
//!
//! `lod` simplifies the model's LOD0 triangles to each target ratio of the LOD0 triangle count
//! with `meshopt_simplifyWithAttributes`. A level is kept only if it removes at least 20 % of
//! the previous level's triangles. The levels are written as `models/<model>.lod` next to the
//! model (`moba_assets::MeshLods`, bound to the model by `mesh_hash`), and each level's
//! triangles and error are printed. A level is a new index list over the LOD0 vertices, so one
//! file serves the static mesh, the skinned mesh, its bone-LOD reduced skins and a VAT minion
//! baked from it.
//!
//! How seams are treated:
//! - Normals (and UVs, when the model has them) are weighted attributes, so collapses that
//!   bend shading or stretch the texture cost error.
//! - meshoptimizer keeps every attribute seam by default. On a flat-shaded low-poly model,
//!   where every edge is a hard edge, almost nothing would collapse, so the bake is
//!   *permissive*: it may collapse across normal seams.
//! - On a **textured** model (one with UVs) the UV weight makes a collapse that slides a
//!   vertex across a UV island expensive, and the measured error below keeps such a level
//!   out of sight. meshoptimizer moves all copies of a seam vertex together, so seams never
//!   crack.
//! - `--protect-uv-seams` also forbids collapses across UV seams: every vertex that shares
//!   its position with a vertex of a different UV is protected (`SimplifyVertex::Protect`).
//!   Use it for atlas-mapped production models. The samples are palette-mapped per face, so
//!   83-100 % of their vertices are on a UV seam and protecting them leaves no levels.
//! - `--keep-seams` turns permissive off (every seam kept); `--lock-border` keeps open
//!   borders in place.
//!
//! A level's error is the largest of:
//! - meshoptimizer's own error;
//! - the measured distance from every LOD0 vertex to the level's surface;
//! - for a **skinned** model, that same distance in sampled poses of every clip, skinned on
//!   the CPU. Collapsing across differently weighted vertices only shows when posed, so the
//!   runtime's projected-error selection uses this posed error.
//!
//! The error is absolute, in model units. `info` prints the model's triangles, joints and any
//! existing levels. `scripts/bake-mesh-lods.sh` re-bakes the sample models; the result is
//! deterministic (CI re-bakes and diffs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use meshopt::{SimplifyOptions, SimplifyVertex};
use moba_assets::{MAX_MESH_LODS, MeshData, MeshLods, load_glb_file, load_lods_for, lod_path};
use moba_core::math::Vec3;

/// Default target ratios of the LOD0 triangle count, LOD1 first.
const DEFAULT_RATIOS: [f32; MAX_MESH_LODS] = [0.5, 0.25, 0.125];
/// A level must keep at most this share of the previous level's triangles.
const MIN_REDUCTION: f32 = 0.8;
/// Attribute weights relative to the (normalised) position error: normals, then UVs.
const NORMAL_WEIGHT: f32 = 0.5;
const UV_WEIGHT: f32 = 1.0;
/// Poses a skinned model's level error is measured in (bind pose plus clip frames).
const ERROR_POSES: usize = 32;
/// Clip sampling rate for the posed error.
const ERROR_POSE_FPS: f32 = 10.0;

fn usage() -> ExitCode {
    eprintln!(
        "usage: mesh-bake lod <model.glb> [--out FILE.lod] [--ratios 0.5,0.25,0.125] \
         [--keep-seams] [--protect-uv-seams] [--lock-border]\n       \
         mesh-bake info <model.glb>"
    );
    ExitCode::from(2)
}

/// How a model is simplified.
#[derive(Clone, Copy, Debug)]
struct Settings {
    /// Collapse across normal seams (and, without UVs, every seam).
    permissive: bool,
    /// Keep open borders in place.
    lock_border: bool,
    /// Forbid collapses across UV seams (permissive only).
    protect_uv_seams: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            permissive: true,
            lock_border: false,
            protect_uv_seams: false,
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage();
    };
    let Some(input) = args.next().map(PathBuf::from) else {
        return usage();
    };
    let mut output: Option<PathBuf> = None;
    let mut ratios: Vec<f32> = DEFAULT_RATIOS.to_vec();
    let mut settings = Settings::default();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--out" => match args.next() {
                Some(path) => output = Some(PathBuf::from(path)),
                None => return usage(),
            },
            "--ratios" => {
                let Some(parsed) = args.next().as_deref().and_then(parse_ratios) else {
                    eprintln!("--ratios needs 1-3 comma-separated values in (0, 1), decreasing");
                    return ExitCode::from(2);
                };
                ratios = parsed;
            }
            "--keep-seams" => settings.permissive = false,
            "--lock-border" => settings.lock_border = true,
            "--protect-uv-seams" => settings.protect_uv_seams = true,
            _ => return usage(),
        }
    }
    let mesh = match load_glb_file(&input) {
        Ok(mesh) => mesh,
        Err(error) => {
            eprintln!("{}: {error}", input.display());
            return ExitCode::FAILURE;
        }
    };
    match command.as_str() {
        "info" => info(&input, &mesh),
        "lod" => {
            let output = output.unwrap_or_else(|| lod_path(&input));
            bake(&mesh, &ratios, settings, &output)
        }
        _ => usage(),
    }
}

/// `0.5,0.25` → `[0.5, 0.25]`: 1-3 values in (0, 1), strictly decreasing.
fn parse_ratios(text: &str) -> Option<Vec<f32>> {
    let values: Vec<f32> = text
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect::<Option<_>>()?;
    let ok = (1..=MAX_MESH_LODS).contains(&values.len())
        && values
            .iter()
            .all(|value| value.is_finite() && *value > 0.0 && *value < 1.0)
        && values.windows(2).all(|pair| pair[1] < pair[0]);
    ok.then_some(values)
}

fn info(input: &Path, mesh: &MeshData) -> ExitCode {
    let joints = mesh.skin().map_or(0, |skin| skin.skeleton().joint_count());
    println!("model      {}", input.display());
    println!("vertices   {}", mesh.vertex_count());
    println!("triangles  {}", mesh.triangle_count());
    println!("joints     {joints}");
    println!(
        "uv seams   {} protected vertices",
        uv_seam_vertices(mesh).iter().filter(|&&seam| seam).count()
    );
    match load_lods_for(input, mesh) {
        Ok(Some(lods)) => {
            for (index, level) in lods.levels().iter().enumerate() {
                println!(
                    "lod{}       {} triangles, error {:.5}",
                    index + 1,
                    level.triangle_count(),
                    level.error()
                );
            }
        }
        Ok(None) => println!("lods       none ({} missing)", lod_path(input).display()),
        Err(error) => {
            eprintln!("{}: {error}", lod_path(input).display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Whether each vertex lies on a UV seam: another vertex has the same position but a
/// different UV. All `false` without UVs.
fn uv_seam_vertices(mesh: &MeshData) -> Vec<bool> {
    let mut seam = vec![false; mesh.vertex_count()];
    let Some(uvs) = mesh.uvs() else {
        return seam;
    };
    let mut by_position: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
    for (vertex, position) in mesh.positions().iter().enumerate() {
        by_position
            .entry(position.map(f32::to_bits))
            .or_default()
            .push(vertex);
    }
    for shared in by_position.values() {
        for &a in shared {
            if shared
                .iter()
                .any(|&b| uvs[b].map(f32::to_bits) != uvs[a].map(f32::to_bits))
            {
                seam[a] = true;
            }
        }
    }
    seam
}

/// Simplifies `mesh` to each ratio and returns the kept levels `(error, indices)`.
fn simplify_levels(mesh: &MeshData, ratios: &[f32], settings: Settings) -> Vec<(f32, Vec<u32>)> {
    let positions = mesh.positions();
    let lod0 = mesh.indices();
    // Per-vertex attributes: the normal, then the UV when there is one.
    let uvs = mesh.uvs();
    let mut weights = vec![NORMAL_WEIGHT; 3];
    if uvs.is_some() {
        weights.extend([UV_WEIGHT; 2]);
    }
    let stride = weights.len();
    let mut attributes = Vec::with_capacity(positions.len() * stride);
    for (vertex, normal) in mesh.normals().iter().enumerate() {
        attributes.extend_from_slice(normal);
        if let Some(uvs) = uvs {
            attributes.extend_from_slice(&uvs[vertex]);
        }
    }
    let mut options = SimplifyOptions::ErrorAbsolute;
    if settings.permissive {
        options |= SimplifyOptions::Permissive;
    }
    if settings.lock_border {
        options |= SimplifyOptions::LockBorder;
    }
    // Permissive collapses across seams; `--protect-uv-seams` keeps a textured model's UV
    // seams.
    let locks: Vec<SimplifyVertex> = uv_seam_vertices(mesh)
        .into_iter()
        .map(|seam| {
            if seam && settings.permissive && settings.protect_uv_seams {
                SimplifyVertex::Protect
            } else {
                SimplifyVertex::None
            }
        })
        .collect();
    let mut levels: Vec<(f32, Vec<u32>)> = Vec::new();
    let mut previous_triangles = mesh.triangle_count();
    let mut previous_error = 0.0f32;
    for &ratio in ratios {
        let target = ((lod0.len() as f32 * ratio) as usize / 3 * 3).max(3);
        let mut error = 0.0f32;
        // No error limit: the runtime selects by projected error, so a coarse level is only
        // ever drawn where its error is below the tier's pixel tolerance.
        let indices = meshopt::simplify_with_attributes_and_locks_decoder(
            lod0,
            positions,
            &attributes,
            &weights,
            stride * std::mem::size_of::<f32>(),
            &locks,
            target,
            f32::MAX,
            options,
            Some(&mut error),
        );
        let triangles = indices.len() / 3;
        if triangles == 0 || triangles as f32 > previous_triangles as f32 * MIN_REDUCTION {
            continue;
        }
        let measured = measured_error(mesh, &indices);
        let error = error.max(measured).max(previous_error);
        previous_triangles = triangles;
        previous_error = error;
        levels.push((error, indices));
    }
    levels
}

/// The largest distance from a LOD0 vertex to the surface of the level `indices`: in the
/// bind pose, and for a skinned mesh in up to [`ERROR_POSES`] sampled clip poses. Vertices
/// the level keeps are on its surface; only the removed ones are measured (brute force over
/// the level's triangles: offline, `O(removed × triangles × poses)`).
fn measured_error(mesh: &MeshData, indices: &[u32]) -> f32 {
    let mut kept = vec![false; mesh.vertex_count()];
    for &index in indices {
        kept[index as usize] = true;
    }
    let removed: Vec<usize> = (0..mesh.vertex_count()).filter(|&v| !kept[v]).collect();
    let triangles = indices.as_chunks::<3>().0;
    let mut worst = 0.0f32;
    let mut measure = |positions: &[[f32; 3]]| {
        for &vertex in &removed {
            let point = Vec3::from_array(positions[vertex]);
            let nearest = triangles
                .iter()
                .map(|triangle| {
                    let [a, b, c] =
                        triangle.map(|index| Vec3::from_array(positions[index as usize]));
                    point.distance(closest_point_on_triangle(point, a, b, c))
                })
                .fold(f32::INFINITY, f32::min);
            if nearest.is_finite() {
                worst = worst.max(nearest);
            }
        }
    };
    if mesh.skin().is_some() {
        moba_assets::for_each_sampled_pose(mesh, ERROR_POSE_FPS, ERROR_POSES - 1, &mut measure);
    } else {
        measure(mesh.positions());
    }
    worst
}

/// The point of triangle `abc` closest to `p` (Ericson, *Real-Time Collision Detection*
/// 5.1.5).
#[allow(clippy::many_single_char_names)] // The book's notation.
fn closest_point_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denominator = va + vb + vc;
    if denominator.abs() <= f32::EPSILON {
        // Degenerate (zero-area) triangle: the nearest of its corners.
        return [a, b, c]
            .into_iter()
            .min_by(|x, y| p.distance_squared(*x).total_cmp(&p.distance_squared(*y)))
            .unwrap_or(a);
    }
    let v = vb / denominator;
    let w = vc / denominator;
    a + ab * v + ac * w
}

fn bake(mesh: &MeshData, ratios: &[f32], settings: Settings, output: &Path) -> ExitCode {
    let levels = simplify_levels(mesh, ratios, settings);
    if levels.is_empty() {
        eprintln!(
            "no level removes 20 % of the triangles of this {}-triangle mesh; nothing written",
            mesh.triangle_count()
        );
        return ExitCode::FAILURE;
    }
    let lods = match MeshLods::new(mesh, levels) {
        Ok(lods) => lods,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = std::fs::write(output, lods.to_bytes(mesh)) {
        eprintln!("{}: {error}", output.display());
        return ExitCode::FAILURE;
    }
    println!("lod0       {} triangles", mesh.triangle_count());
    for (index, level) in lods.levels().iter().enumerate() {
        println!(
            "lod{}       {} triangles, error {:.5}",
            index + 1,
            level.triangle_count(),
            level.error()
        );
    }
    println!("wrote      {}", output.display());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_assets::{
        AnimationClip, Interpolation, JointTransform, MeshSkin, Skeleton, Track, TrackPath,
    };
    use moba_core::math::{Mat4, Quat};

    /// A finely tessellated flat-ish quad grid over x, z in 0..1 (open: `--lock-border`
    /// keeps its border).
    fn grid(side: u32) -> MeshData {
        let mut positions = Vec::new();
        for z in 0..=side {
            for x in 0..=side {
                let (fx, fz) = (x as f32 / side as f32, z as f32 / side as f32);
                positions.push([fx, (fx * 3.0).sin() * 0.05 + (fz * 2.0).cos() * 0.05, fz]);
            }
        }
        let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
        let mut indices = Vec::new();
        let row = side + 1;
        for z in 0..side {
            for x in 0..side {
                let a = z * row + x;
                indices.extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
            }
        }
        MeshData::new(positions, normals, indices).expect("grid")
    }

    #[test]
    fn levels_shrink_with_growing_error_and_validate_against_the_mesh() {
        let mesh = grid(32);
        let settings = Settings {
            lock_border: true,
            ..Settings::default()
        };
        let levels = simplify_levels(&mesh, &DEFAULT_RATIOS, settings);
        assert!(!levels.is_empty(), "a 2048-triangle grid simplifies");
        let mut previous = mesh.triangle_count();
        let mut previous_error = 0.0;
        for (error, indices) in &levels {
            assert!(
                indices.len() / 3 <= previous * 4 / 5,
                "each level removes 20 %"
            );
            assert!(error.is_finite() && *error >= previous_error);
            assert!(
                *error >= measured_error(&mesh, indices),
                "never below the measured error"
            );
            previous = indices.len() / 3;
            previous_error = *error;
        }
        let lods = MeshLods::new(&mesh, levels).expect("valid levels");
        let parsed = MeshLods::parse(&lods.to_bytes(&mesh), &mesh).expect("round trip");
        assert_eq!(parsed, lods);
        // Deterministic: the same input bakes the same bytes.
        let again = MeshLods::new(&mesh, simplify_levels(&mesh, &DEFAULT_RATIOS, settings))
            .expect("valid levels");
        assert_eq!(again.to_bytes(&mesh), lods.to_bytes(&mesh));
    }

    /// R8 review: a textured model must keep its UV seams. The grid's left and right halves
    /// are separate UV islands: the seam column is duplicated and the right copy's UVs are
    /// offset. A permissive collapse that moves one copy of a seam vertex but not the other
    /// opens a crack (and slides the texture); with the seam protected, both islands keep
    /// exactly the same seam positions in every level.
    #[test]
    fn textured_levels_keep_their_uv_seams_closed() {
        let side = 24u32;
        let seam = side / 2;
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut island = Vec::new();
        let mut index_of = HashMap::new();
        for z in 0..=side {
            for x in 0..=side {
                let (fx, fz) = (x as f32 / side as f32, z as f32 / side as f32);
                let position = [fx, (fx * 5.0).sin() * 0.02, fz];
                let copies: &[bool] = if x == seam {
                    &[false, true]
                } else {
                    &[x > seam]
                };
                for &right in copies {
                    index_of.insert((x, z, right), positions.len() as u32);
                    positions.push(position);
                    uvs.push([if right { fx + 0.001 } else { fx }, fz]);
                    island.push(right);
                }
            }
        }
        let mut indices = Vec::new();
        for z in 0..side {
            for x in 0..side {
                let right = x >= seam;
                let at = |dx: u32, dz: u32| index_of[&(x + dx, z + dz, right)];
                indices.extend_from_slice(&[at(0, 0), at(0, 1), at(1, 0)]);
                indices.extend_from_slice(&[at(1, 0), at(0, 1), at(1, 1)]);
            }
        }
        let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
        let mesh = MeshData::new(positions, normals, indices)
            .and_then(|mesh| mesh.with_uvs(uvs))
            .expect("textured grid");
        let seams = uv_seam_vertices(&mesh).iter().filter(|&&seam| seam).count();
        assert_eq!(
            seams,
            2 * (side as usize + 1),
            "both copies of the seam column"
        );
        let protect = Settings {
            protect_uv_seams: true,
            ..Settings::default()
        };
        let mut levels = simplify_levels(&mesh, &DEFAULT_RATIOS, Settings::default());
        assert!(!levels.is_empty(), "the islands still simplify");
        levels.extend(simplify_levels(&mesh, &DEFAULT_RATIOS, protect));
        let seam_x = mesh.positions()[index_of[&(seam, 0, false)] as usize][0];
        for (_, level) in &levels {
            // Seam positions each island's triangles still use.
            let mut used = [
                std::collections::BTreeSet::new(),
                std::collections::BTreeSet::new(),
            ];
            for &index in level {
                let position = mesh.positions()[index as usize];
                if position[0].to_bits() == seam_x.to_bits() {
                    used[usize::from(island[index as usize])].insert(position[2].to_bits());
                }
            }
            assert_eq!(used[0], used[1], "the UV seam opened a crack");
        }
    }

    /// The largest UV distortion of a level over a flat (y = 0) mesh: for each LOD0 vertex,
    /// the UV the level interpolates at the vertex's position against the vertex's own UV.
    fn uv_distortion(mesh: &MeshData, level: &[u32]) -> f32 {
        let uvs = mesh.uvs().expect("textured");
        let positions = mesh.positions();
        let mut worst = 0.0f32;
        for (vertex, position) in positions.iter().enumerate() {
            let p = Vec3::from_array(*position);
            let (triangle, _) = level
                .as_chunks::<3>()
                .0
                .iter()
                .map(|triangle| {
                    let [a, b, c] = triangle.map(|i| Vec3::from_array(positions[i as usize]));
                    (triangle, p.distance(closest_point_on_triangle(p, a, b, c)))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .expect("a level has triangles");
            let [a, b, c] = triangle.map(|i| Vec3::from_array(positions[i as usize]));
            // Barycentrics in the xz plane.
            let area =
                |u: Vec3, v: Vec3, w: Vec3| (v.x - u.x) * (w.z - u.z) - (w.x - u.x) * (v.z - u.z);
            let total = area(a, b, c);
            if total.abs() < 1e-9 {
                continue;
            }
            let (wa, wb) = (area(p, b, c) / total, area(a, p, c) / total);
            let wc = 1.0 - wa - wb;
            let [ua, ub, uc] = triangle.map(|i| uvs[i as usize]);
            let interpolated = [0, 1].map(|axis| wa * ua[axis] + wb * ub[axis] + wc * uc[axis]);
            let own = uvs[vertex];
            worst = worst.max(
                ((interpolated[0] - own[0]).powi(2) + (interpolated[1] - own[1]).powi(2)).sqrt(),
            );
        }
        worst
    }

    /// The UV attribute reaches the simplifier: on a flat grid (every collapse is free
    /// geometrically) whose UVs bend, the UV-weighted level keeps the texture mapping much
    /// closer than one simplified on geometry alone.
    #[test]
    fn uv_weights_keep_the_texture_mapping() {
        let flat = grid(16);
        let positions: Vec<[f32; 3]> = flat
            .positions()
            .iter()
            .map(|position| [position[0], 0.0, position[2]])
            .collect();
        let uvs: Vec<[f32; 2]> = positions
            .iter()
            .map(|position| [(position[0] * 9.0).sin() * 0.5 + 0.5, position[2]])
            .collect();
        let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
        let untextured = MeshData::new(positions, normals, flat.indices().to_vec()).expect("mesh");
        let textured = untextured.clone().with_uvs(uvs).expect("textured");
        let settings = Settings {
            lock_border: true,
            ..Settings::default()
        };
        let blind = simplify_levels(&untextured, &[0.5], settings);
        let mapped = simplify_levels(&textured, &[0.5], settings);
        assert_eq!(blind.len(), 1);
        assert_eq!(mapped.len(), 1);
        let blind = uv_distortion(&textured, &blind[0].1);
        let mapped = uv_distortion(&textured, &mapped[0].1);
        assert!(
            mapped < blind * 0.5,
            "UV-weighted {mapped} vs geometry-only {blind}"
        );
    }

    /// R8 review: a skinned model's level error is measured posed. The grid's right half is
    /// skinned to a joint that a clip folds about the middle: flat in the bind pose (so the
    /// simplifier collapses it with almost no error), folded when posed.
    #[test]
    fn skinned_level_error_covers_sampled_poses() {
        let flat = grid(16);
        let positions: Vec<[f32; 3]> = flat
            .positions()
            .iter()
            .map(|position| [position[0], 0.0, position[2]])
            .collect();
        let joints = positions
            .iter()
            .map(|position| {
                if position[0] > 0.5 {
                    [1, 0, 0, 0]
                } else {
                    [0, 0, 0, 0]
                }
            })
            .collect();
        let weights = vec![[255, 0, 0, 0]; positions.len()];
        let hinge = Vec3::new(0.5, 0.0, 0.0);
        let fold = Track::new(
            1,
            TrackPath::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            vec![
                Quat::IDENTITY.to_array(),
                Quat::from_rotation_z(1.5).to_array(),
            ],
        )
        .expect("track");
        let skeleton = Skeleton::new(
            vec![None, Some(0)],
            vec![
                JointTransform::IDENTITY,
                JointTransform {
                    translation: hinge,
                    ..JointTransform::IDENTITY
                },
            ],
            vec![Mat4::IDENTITY; 2],
            vec![Mat4::IDENTITY, Mat4::from_translation(-hinge)],
            None,
        )
        .expect("skeleton")
        .with_clips(vec![
            AnimationClip::new(vec![fold])
                .expect("clip")
                .with_name("fold")
                .expect("name"),
        ])
        .expect("clips");
        let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
        let static_mesh =
            MeshData::new(positions.clone(), normals.clone(), flat.indices().to_vec())
                .expect("mesh");
        let skinned = MeshData::new(positions, normals, flat.indices().to_vec())
            .expect("mesh")
            .with_skin(MeshSkin::new(joints, weights, skeleton).expect("skin"))
            .expect("skinned");
        let settings = Settings::default();
        let unposed = simplify_levels(&static_mesh, &DEFAULT_RATIOS, settings);
        let posed = simplify_levels(&skinned, &DEFAULT_RATIOS, settings);
        assert_eq!(unposed.len(), posed.len());
        let mut grew = false;
        for ((bind_error, bind_indices), (posed_error, posed_indices)) in unposed.iter().zip(&posed)
        {
            assert_eq!(bind_indices, posed_indices, "the same simplification");
            assert!(posed_error >= bind_error);
            assert!(*posed_error >= measured_error(&skinned, posed_indices));
            grew |= *posed_error > bind_error + 1e-3;
        }
        assert!(
            grew,
            "folding shows error the bind pose hides: {unposed:?} vs {posed:?}"
        );
    }

    #[test]
    fn closest_points_are_on_the_triangle() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Z);
        let cases = [
            (Vec3::new(0.2, 1.0, 0.2), Vec3::new(0.2, 0.0, 0.2)),
            (Vec3::new(-1.0, 0.0, -1.0), a),
            (Vec3::new(2.0, 0.0, -0.5), b),
            (Vec3::new(0.5, 0.0, -1.0), Vec3::new(0.5, 0.0, 0.0)),
            (Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.5, 0.0, 0.5)),
        ];
        for (point, expected) in cases {
            let closest = closest_point_on_triangle(point, a, b, c);
            assert!(
                closest.distance(expected) < 1e-5,
                "{point} -> {closest}, not {expected}"
            );
        }
        assert_eq!(closest_point_on_triangle(Vec3::Y, a, a, a), a);
    }

    #[test]
    fn ratios_parse_only_when_decreasing_and_in_range() {
        assert_eq!(parse_ratios("0.5, 0.25"), Some(vec![0.5, 0.25]));
        for bad in ["", "0", "1", "0.5,0.6", "0.5,0.4,0.3,0.2", "x"] {
            assert_eq!(parse_ratios(bad), None, "{bad}");
        }
    }
}
