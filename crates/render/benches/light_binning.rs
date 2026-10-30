#![allow(missing_docs)]
//! CPU cost of binning a frame's dynamic lights into the clustered grid (R2b, perf/0020).
//!
//! - `light_binning/256_spread`: the Ultra worst case by count, 256 lights (the global cap)
//!   of 4 m radius scattered over the player camera's ground footprint.
//! - `light_binning/256_large`: 256 lights at the largest effect-light radius (24 m), so each
//!   covers many columns and most columns hit the 16-per-column cap: the worst case by work.
//! - `light_binning/16_low`: Low's caps (16 lights, 4 per column) on the shipped-content scale.
//!
//! The grid's count words are written once per frame from CPU scratch, so the binning never
//! reads the (write-combined) mapped grid; these runs use heap buffers of the same shape.

use criterion::{Criterion, criterion_group, criterion_main};
use moba_core::math::{Mat4, Vec3, look_at_rh, perspective_rh};
use moba_render::PointLight;
use moba_render::bench_support::LightBinning;
use std::hint::black_box;

fn player_camera() -> Mat4 {
    let eye = Vec3::new(0.0, 40.0, 23.0);
    perspective_rh(1.05, 16.0 / 9.0, 0.1, 1000.0) * look_at_rh(eye, Vec3::ZERO, Vec3::Y)
}

/// `count` lights of `radius` on a deterministic scatter over ±30 m × ±20 m.
fn lights(count: usize, radius: f32) -> Vec<PointLight> {
    (0..count)
        .map(|index| {
            let i = index as f32;
            let x = (i * 7.31).sin() * 30.0;
            let z = (i * 3.17).cos() * 20.0;
            PointLight::new(Vec3::new(x, 1.0, z), radius, Vec3::new(4.0, 2.0, 1.0))
        })
        .collect()
}

fn binning(c: &mut Criterion) {
    let camera = player_camera();
    let mut binner = LightBinning::new();
    let mut group = c.benchmark_group("light_binning");
    for (name, lights, max, per_cell) in [
        ("256_spread", lights(256, 4.0), 256, 16),
        ("256_large", lights(256, 24.0), 256, 16),
        ("16_low", lights(16, 4.0), 16, 4),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| black_box(binner.bin(black_box(&lights), camera, max, per_cell)));
        });
    }
    group.finish();
}

criterion_group!(benches, binning);
criterion_main!(benches);
