#![allow(missing_docs)]
//! A0 `MikkTSpace` tangent generation cost (ADR-0022 and its amendment 1). The glb loader runs
//! it once per mesh without stored tangents (shipped meshes carry baked ones, so this is the
//! fallback for other files), so it is load-time work, not per-frame. It is roughly 100× the
//! pre-A0 area-weighted generator (about 0.15 ms and 0.8 ms for the grids below).
//!
//! - `mesh/generate_tangents/10k`: a 100 × 100-vertex UV grid (≈10 k vertices, 19.6 k
//!   triangles), about a hero-sized mesh: about 13 ms on the reference machine.
//! - `mesh/generate_tangents/65k`: a 256 × 256 grid (65 536 vertices, the
//!   [`MeshData::MAX_VERTICES`] cap; 130 k triangles): about 90 ms.
//! - `mesh/generate_tangents/sliver-fans-at-cap`: 964 sliver fans of 64 triangles with
//!   incoherent UVs, just under `TANGENT_MAX_SHARING_COST`: the slowest single-cap case,
//!   about 0.23 s.
//! - `mesh/generate_tangents/big-fans-at-cap`: two such fans of 510, just under
//!   `TANGENT_MAX_SUBGROUP_COST`: about 25 ms.
//! - `mesh/generate_tangents/degenerates-at-cap`: a 255 × 255 grid plus 230 degenerate
//!   triangles, just under `TANGENT_MAX_DEGENERATE_COST`: about 0.12 s.
//! - `mesh/generate_tangents/two-caps-combined`: 940 sliver fans of 64 plus 495 degenerate
//!   triangles, just under the sharing and degenerate costs at once: the slowest accepted
//!   mesh found, about 0.25 s.
//!
//! The grid is a gentle height field with per-vertex normals, so every triangle has a
//! non-degenerate UV mapping and `MikkTSpace` runs its full path (no fallbacks).

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use moba_assets::MeshData;
use std::hint::black_box;

/// A `side × side` vertex grid over the unit square with a sine height field.
fn grid(side: u32) -> MeshData {
    let height = |x: f32, z: f32| 0.05 * (x * 7.0).sin() * (z * 5.0).cos();
    let step = 1.0 / (side - 1) as f32;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for row in 0..side {
        for column in 0..side {
            let (x, z) = (column as f32 * step, row as f32 * step);
            positions.push([x, height(x, z), z]);
            let dx = (height(x + 1e-3, z) - height(x - 1e-3, z)) / 2e-3;
            let dz = (height(x, z + 1e-3) - height(x, z - 1e-3)) / 2e-3;
            let length = (dx * dx + 1.0 + dz * dz).sqrt();
            normals.push([-dx / length, 1.0 / length, -dz / length]);
            uvs.push([x, z]);
        }
    }
    let mut indices = Vec::new();
    for row in 0..side - 1 {
        for column in 0..side - 1 {
            let at = row * side + column;
            // Counter-clockwise seen from +Y.
            indices.extend_from_slice(&[at, at + side, at + 1, at + 1, at + side, at + side + 1]);
        }
    }
    MeshData::new(positions, normals, indices)
        .and_then(|mesh| mesh.with_uvs(uvs))
        .expect("valid grid")
}

/// `count` sliver fans (0.01 rad) of `triangles` triangles with a pseudo-random UV angle per
/// rim vertex: nearly antiparallel tangents, `MikkTSpace`'s most expensive case per corner
/// pair (the `mesh` unit tests' `FanUvs::Sliver`).
fn sliver_fans(count: u32, triangles: u32) -> MeshData {
    let (mut positions, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for fan in 0..count {
        let offset = fan as f32 * 3.0;
        let base = positions.len() as u32;
        for i in 0..=triangles {
            let angle = i as f32 / triangles as f32 * 0.01;
            positions.push([offset + angle.cos(), angle.sin(), 0.0]);
            let hash = i.wrapping_mul(2_654_435_761) ^ fan.wrapping_mul(40_503);
            let uv_angle = hash as f32 / u32::MAX as f32 * core::f32::consts::TAU;
            uvs.push([0.5 + 0.5 * uv_angle.cos(), 0.5 + 0.5 * uv_angle.sin()]);
        }
        positions.push([offset, 0.0, 0.1]);
        uvs.push([0.5, 0.5]);
        let centre = positions.len() as u32 - 1;
        for i in 0..triangles {
            indices.extend([centre, base + i, base + i + 1]);
        }
    }
    let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
    MeshData::new(positions, normals, indices)
        .and_then(|mesh| mesh.with_uvs(uvs))
        .expect("valid fans")
}

/// A `side × side` flat grid plus `degenerate` triangles `(a, a, b)` on their own vertices.
fn grid_with_degenerates(side: u32, degenerate: u32) -> MeshData {
    let (mut positions, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for row in 0..side {
        for column in 0..side {
            positions.push([column as f32, row as f32, 0.0]);
            uvs.push([column as f32 / side as f32, row as f32 / side as f32]);
        }
    }
    for row in 0..side - 1 {
        for column in 0..side - 1 {
            let at = row * side + column;
            indices.extend([at, at + 1, at + side, at + 1, at + side + 1, at + side]);
        }
    }
    let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
    let grid = MeshData::new(positions, normals, indices)
        .and_then(|mesh| mesh.with_uvs(uvs))
        .expect("valid grid");
    with_degenerates(&grid, degenerate)
}

/// `mesh` plus `degenerate` triangles `(a, a, b)` on their own vertices, away from it.
fn with_degenerates(mesh: &MeshData, degenerate: u32) -> MeshData {
    let mut positions = mesh.positions().to_vec();
    let mut uvs = mesh.uvs().expect("uvs").to_vec();
    let mut indices = mesh.indices().to_vec();
    for d in 0..degenerate {
        let at = positions.len() as u32;
        positions.extend([[-1.0 - d as f32, -5.0, 0.0], [-1.0 - d as f32, -6.0, 0.0]]);
        uvs.extend([[0.0, 0.0], [0.0, 1.0]]);
        indices.extend([at, at, at + 1]);
    }
    let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
    MeshData::new(positions, normals, indices)
        .and_then(|mesh| mesh.with_uvs(uvs))
        .expect("valid mesh")
}

fn bench_tangents(c: &mut Criterion) {
    let mut group = c.benchmark_group("mesh/generate_tangents");
    // Load-time work of up to ~0.25 s per call: fewer samples keep the run short.
    group.sample_size(10);
    let cases = [
        ("10k", grid(100)),
        ("65k", grid(256)),
        ("sliver-fans-at-cap", sliver_fans(964, 64)),
        ("big-fans-at-cap", sliver_fans(2, 510)),
        ("degenerates-at-cap", grid_with_degenerates(255, 230)),
        (
            "two-caps-combined",
            with_degenerates(&sliver_fans(940, 64), 495),
        ),
    ];
    for (name, mesh) in cases {
        assert!(mesh.generate_tangents().is_some(), "the mesh has UVs");
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| black_box(black_box(&mesh).generate_tangents()));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_tangents);
criterion_main!(benches);
