#![allow(missing_docs)]
//! Per-frame skeleton posing cost (`Skeleton::sample`), the CPU half of GPU skinning.
//!
//! Each client frame poses up to 16 skinned heroes (the default bone budget is 16 × 64
//! joints). The rigs here are joint chains with a looping rotation track per joint and a
//! root translation track, at the shipped sample's size (7 joints) and the cap (64).

use criterion::{Criterion, criterion_group, criterion_main};
use moba_assets::{
    AnimationClip, Interpolation, JointMatrix, JointTransform, MAX_JOINTS, Skeleton, Track,
    TrackPath,
};
use moba_core::math::{Mat4, Quat, Vec3};
use std::hint::black_box;

/// A chain of `joints` joints, each 0.2 m above its parent, with a 0.8 s swing clip.
fn chain(joints: usize) -> Skeleton {
    let parents = (0..joints)
        .map(|joint| {
            joint
                .checked_sub(1)
                .map(|parent| u8::try_from(parent).expect("u8"))
        })
        .collect();
    let rest = (0..joints)
        .map(|joint| JointTransform {
            translation: if joint == 0 {
                Vec3::ZERO
            } else {
                Vec3::Y * 0.2
            },
            ..JointTransform::IDENTITY
        })
        .collect();
    let inverse_bind = (0..joints)
        .map(|joint| Mat4::from_translation(-Vec3::Y * 0.2 * joint as f32))
        .collect();
    let times = vec![0.0, 0.2, 0.4, 0.6, 0.8];
    let swing =
        |angle: f32| [angle, 0.0, -angle, 0.0, angle].map(|a| Quat::from_rotation_x(a).to_array());
    let mut tracks: Vec<Track> = (0..joints)
        .map(|joint| {
            Track::new(
                u8::try_from(joint).expect("u8"),
                TrackPath::Rotation,
                Interpolation::Linear,
                times.clone(),
                swing(0.3).to_vec(),
            )
            .expect("track")
        })
        .collect();
    tracks.push(
        Track::new(
            0,
            TrackPath::Translation,
            Interpolation::Linear,
            times.clone(),
            vec![
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 1.1, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 1.1, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
            ],
        )
        .expect("track"),
    );
    let clip = AnimationClip::new(tracks).expect("clip");
    Skeleton::new(
        parents,
        rest,
        vec![Mat4::IDENTITY; joints],
        inverse_bind,
        Some(clip),
    )
    .expect("skeleton")
}

fn bench_sample(c: &mut Criterion) {
    let mut out: [JointMatrix; MAX_JOINTS] = [[[0.0; 4]; 3]; MAX_JOINTS];
    for joints in [7, MAX_JOINTS] {
        let skeleton = chain(joints);
        for (label, blend) in [("rest", 0.0), ("walk", 1.0)] {
            c.bench_function(&format!("skin_sample/{joints}_joints/{label}"), |b| {
                let mut time = 0.0f32;
                b.iter(|| {
                    time += 1.0 / 144.0;
                    black_box(skeleton.sample(black_box(time), blend, &mut out));
                });
            });
        }
    }
}

criterion_group!(benches, bench_sample);
criterion_main!(benches);
