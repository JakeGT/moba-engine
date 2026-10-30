#![allow(clippy::print_stdout, clippy::print_stderr)]
//! Bake index-only mesh levels of detail with meshoptimizer (R8, ADR-0018 §3).
//!
//! ```text
//! mesh-bake lod <model.glb> [--out FILE.lod] [--ratios 0.5,0.25,0.125] [--permissive]
//!               [--lock-border]
//! mesh-bake info <model.glb>
//! ```
//!
//! `lod` simplifies the model's LOD0 triangles with `meshopt_simplify` (an absolute error
//! metric in model units) to each target ratio of the LOD0 triangle count and
//! keeps a level only if it removes at least 20 % of the previous level's triangles; it writes
//! the levels as `models/<model>.lod` next to the model (`moba_assets::MeshLods`, bound to the
//! model by `mesh_hash`) and prints each level's triangles and error. A level is a new index
//! list over the LOD0 vertices, so one file serves the static mesh, the skinned mesh, its
//! bone-LOD reduced skins and a VAT minion baked from it. meshoptimizer keeps hard edges and
//! UV seams (vertices that share a position) by default, so a flat-shaded low-poly model,
//! where every edge is such a seam, barely simplifies; `--permissive` lets it collapse across
//! them (a level is only drawn where its error is below the tier's pixel tolerance, so the
//! seams' shading changes are sub-pixel). `--lock-border` keeps open borders in place. `info` prints the model's triangles,
//! joints and any existing levels. `scripts/bake-mesh-lods.sh` re-bakes the sample models.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use moba_assets::{MAX_MESH_LODS, MeshData, MeshLods, load_glb_file, load_lods_for, lod_path};

/// Default target ratios of the LOD0 triangle count, LOD1 first.
const DEFAULT_RATIOS: [f32; MAX_MESH_LODS] = [0.5, 0.25, 0.125];
/// A level must keep at most this share of the previous level's triangles.
const MIN_REDUCTION: f32 = 0.8;

fn usage() -> ExitCode {
    eprintln!(
        "usage: mesh-bake lod <model.glb> [--out FILE.lod] [--ratios 0.5,0.25,0.125] \
         [--permissive] [--lock-border]\n       \
         mesh-bake info <model.glb>"
    );
    ExitCode::from(2)
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
    let mut options = meshopt::SimplifyOptions::ErrorAbsolute;
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
            "--permissive" => options |= meshopt::SimplifyOptions::Permissive,
            "--lock-border" => options |= meshopt::SimplifyOptions::LockBorder,
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
            bake(&mesh, &ratios, options, &output)
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

/// Simplifies `mesh` to each ratio and returns the kept levels `(error, indices)`.
fn simplify_levels(
    mesh: &MeshData,
    ratios: &[f32],
    options: meshopt::SimplifyOptions,
) -> Vec<(f32, Vec<u32>)> {
    let positions = mesh.positions();
    let lod0 = mesh.indices();
    let mut levels: Vec<(f32, Vec<u32>)> = Vec::new();
    let mut previous_triangles = mesh.triangle_count();
    let mut previous_error = 0.0f32;
    for &ratio in ratios {
        let target = ((lod0.len() as f32 * ratio) as usize / 3 * 3).max(3);
        let mut error = 0.0f32;
        // Absolute error in model units; no error limit (the runtime selects by projected
        // error, so a coarse level is only ever drawn where its error is below a pixel).
        let indices = meshopt::simplify_decoder(
            lod0,
            positions,
            target,
            f32::MAX,
            options | meshopt::SimplifyOptions::ErrorAbsolute,
            Some(&mut error),
        );
        let triangles = indices.len() / 3;
        if triangles == 0 || triangles as f32 > previous_triangles as f32 * MIN_REDUCTION {
            continue;
        }
        let error = error.max(previous_error);
        previous_triangles = triangles;
        previous_error = error;
        levels.push((error, indices));
    }
    levels
}

fn bake(
    mesh: &MeshData,
    ratios: &[f32],
    options: meshopt::SimplifyOptions,
    output: &Path,
) -> ExitCode {
    let levels = simplify_levels(mesh, ratios, options);
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

    /// A finely tessellated flat-ish quad grid (open: its border is locked).
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
        let levels = simplify_levels(&mesh, &DEFAULT_RATIOS, meshopt::SimplifyOptions::LockBorder);
        assert!(!levels.is_empty(), "a 2048-triangle grid simplifies");
        let mut previous = mesh.triangle_count();
        for (error, indices) in &levels {
            assert!(
                indices.len() / 3 <= previous * 4 / 5,
                "each level removes 20 %"
            );
            assert!(error.is_finite() && *error >= 0.0);
            previous = indices.len() / 3;
        }
        let lods = MeshLods::new(&mesh, levels).expect("valid levels");
        let parsed = MeshLods::parse(&lods.to_bytes(&mesh), &mesh).expect("round trip");
        assert_eq!(parsed, lods);
    }

    /// The committed `content/models/*.lod` files must match their models (a model changed
    /// without a re-bake would silently draw LOD0 only): every sample model has levels, and
    /// they load against its current mesh hash.
    #[test]
    fn committed_lod_files_match_their_models() {
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/models");
        for name in ["crystal", "rampart", "sentinel", "sentinel-skinned"] {
            let glb = models.join(format!("{name}.glb"));
            let mesh = load_glb_file(&glb).expect("sample model loads");
            let lods = load_lods_for(&glb, &mesh)
                .unwrap_or_else(|error| {
                    panic!("{name}.lod: {error}; run scripts/bake-mesh-lods.sh")
                })
                .unwrap_or_else(|| panic!("{name}.lod is missing; run scripts/bake-mesh-lods.sh"));
            assert!(!lods.is_empty(), "{name}");
            assert!(lods.levels()[0].triangle_count() < mesh.triangle_count());
        }
    }

    #[test]
    fn ratios_parse_only_when_decreasing_and_in_range() {
        assert_eq!(parse_ratios("0.5, 0.25"), Some(vec![0.5, 0.25]));
        for bad in ["", "0", "1", "0.5,0.6", "0.5,0.4,0.3,0.2", "x"] {
            assert_eq!(parse_ratios(bad), None, "{bad}");
        }
    }
}
