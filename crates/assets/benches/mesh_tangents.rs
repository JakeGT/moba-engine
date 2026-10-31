#![allow(missing_docs)]
//! A0 `MikkTSpace` tangent generation cost (ADR-0022). The glb loader runs it once per mesh
//! without stored tangents, so it is load-time work, not per-frame.
//!
//! - `mesh/generate_tangents/10k`: a 100 × 100-vertex UV grid (≈10 k vertices, 19.6 k
//!   triangles), about a hero-sized mesh.
//! - `mesh/generate_tangents/65k`: a 256 × 256 grid (65 536 vertices, the
//!   [`MeshData::MAX_VERTICES`] cap; 130 k triangles), the worst case one mesh can be.
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

fn bench_tangents(c: &mut Criterion) {
    let mut group = c.benchmark_group("mesh/generate_tangents");
    // Load-time work of up to ~0.1 s per call: fewer samples keep the run short.
    group.sample_size(20);
    for (name, side) in [("10k", 100), ("65k", 256)] {
        let mesh = grid(side);
        assert!(mesh.generate_tangents().is_some(), "the grid has UVs");
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| black_box(black_box(&mesh).generate_tangents()));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_tangents);
criterion_main!(benches);
