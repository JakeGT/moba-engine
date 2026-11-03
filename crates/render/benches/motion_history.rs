#![allow(missing_docs)]
//! CPU cost of the temporal path's per-unit motion work (ADR-0021 §2c, N1a perf/0031): for
//! every keyed unit, `MotionHistory::previous`, the previous-instance entry and the previous
//! joint palette copy, then `MotionHistory::record`.
//!
//! - `motion_history/1000x40`: 1,000 keyed units of 40 joints each, all moving (the review's
//!   stress shape; 40,000 joint matrices a frame each way).
//! - `motion_history/1000x0`: the same units unskinned (the lookup and the instance entry).
//! - `motion_history/150x40`: about a match's worth of skinned units (10 heroes, minions,
//!   jungle and structures).
//! - `motion_history/3072x0`: the history's key cap (3/4 of its 4,096 slots).
//!
//! The destinations are heap buffers shaped like the mapped previous-instance and bone
//! buffers, so this measures the work, not write-combined memory.

use criterion::{Criterion, criterion_group, criterion_main};
use moba_core::math::{Mat4, Vec3};
use moba_render::bench_support::{MotionUnit, MotionUpload};
use std::hint::black_box;

/// `count` units with `joints` joints, keyed like the client (`net_id << 16 | part`), at
/// `step` metres along x.
fn units(count: usize, joints: usize, step: f32) -> Vec<MotionUnit> {
    (0..count)
        .map(|index| {
            let i = index as f32;
            MotionUnit {
                key: (u32::try_from(index).unwrap_or(0) + 1) << 16 | 1,
                transform: Mat4::from_translation(Vec3::new(
                    (i * 7.31).sin() * 30.0 + step,
                    0.0,
                    (i * 3.17).cos() * 20.0,
                )),
                joints: vec![
                    [
                        [1.0, 0.0, 0.0, step],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0]
                    ];
                    joints
                ],
            }
        })
        .collect()
}

fn motion(c: &mut Criterion) {
    let mut group = c.benchmark_group("motion_history");
    for (name, count, joints) in [
        ("1000x40", 1000, 40),
        ("1000x0", 1000, 0),
        ("150x40", 150, 40),
        ("3072x0", 3072, 0),
    ] {
        // Two frames whose units moved between them, alternated, so every lookup hits.
        let frames = [units(count, joints, 0.0), units(count, joints, 0.1)];
        let mut upload = MotionUpload::new(count, count * joints);
        upload.frame(&frames[0]);
        let mut parity = 1;
        group.bench_function(name, |b| {
            b.iter(|| {
                let found = upload.frame(black_box(&frames[parity]));
                parity ^= 1;
                black_box(found)
            });
        });
        assert_eq!(
            upload.frame(&frames[parity]),
            count,
            "{name}: every unit hits"
        );
    }
    group.finish();
}

criterion_group!(benches, motion);
criterion_main!(benches);
