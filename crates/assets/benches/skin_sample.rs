#![allow(missing_docs)]
//! Per-frame skeleton posing cost, the CPU half of GPU skinning.
//!
//! - `skin_sample/*`: the WP-4.1 single-clip path (`Skeleton::sample`). Each client frame poses
//!   up to 16 skinned heroes (the default bone budget is 16 × 64 joints). The rigs are joint
//!   chains with a looping rotation track per joint and a root translation track, at 7 joints
//!   and at the cap (64).
//! - `animator/*` (ADR-0012): the animation runtime on a rig shaped like the shipped sample
//!   (10 joints, a 3-joint spring cape): locomotion blend space, an upper-body action, an
//!   additive hit, springs and a socket. `200_units` is the R4 stress case (every unit
//!   evaluated every frame); `200_units_lod4` evaluates a quarter of them per frame, as the
//!   client's distance LOD does for far units.

use criterion::{Criterion, criterion_group, criterion_main};
use moba_assets::{
    AnimationClip, AnimationEvent, AnimationGraph, AnimationScratch, AnimationSet, Animator,
    Interpolation, JointMatrix, JointTransform, MAX_JOINTS, Skeleton, Track, TrackPath,
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

/// The shipped sample's shape: hips, chest, head, two legs, two arms and a three-joint cape,
/// with one rotation clip per state.
fn rig() -> Skeleton {
    let parents = vec![
        None,
        Some(0),
        Some(1),
        Some(0),
        Some(0),
        Some(1),
        Some(1),
        Some(1),
        Some(7),
        Some(8),
    ];
    let rest = [
        [0.0, 1.05, 0.0],
        [0.0, 0.55, 0.0],
        [0.0, 1.12, 0.0],
        [-0.28, 0.05, 0.0],
        [0.28, 0.05, 0.0],
        [-0.8, 0.8, 0.0],
        [0.8, 0.8, 0.0],
        [0.0, 0.95, -0.47],
        [0.0, -0.45, -0.03],
        [0.0, -0.45, -0.03],
    ]
    .map(|[x, y, z]| JointTransform {
        translation: Vec3::new(x, y, z),
        ..JointTransform::IDENTITY
    })
    .to_vec();
    let names = [
        "hips", "chest", "head", "leg_l", "leg_r", "arm_l", "arm_r", "cape_0", "cape_1", "cape_2",
    ]
    .map(|name| Some(String::from(name)))
    .to_vec();
    let clip = |name: &str, joints: &[u8], length: f32| {
        let tracks = joints
            .iter()
            .map(|&joint| {
                Track::new(
                    joint,
                    TrackPath::Rotation,
                    Interpolation::Linear,
                    vec![0.0, length * 0.5, length],
                    [0.0, 0.6, 0.0]
                        .map(|angle| Quat::from_rotation_x(angle).to_array())
                        .to_vec(),
                )
                .expect("track")
            })
            .collect();
        AnimationClip::new(tracks)
            .and_then(|clip| clip.with_name(name))
            .expect("clip")
    };
    let clips = vec![
        clip("idle", &[1, 2], 1.6),
        clip("run", &[1, 3, 4, 5, 6], 0.8),
        clip("attack", &[1, 6], 0.7),
        clip("cast1", &[5, 6], 0.9),
        clip("death", &[0], 1.2),
        clip("hit", &[1, 2], 0.3),
    ];
    Skeleton::new(
        parents,
        rest,
        vec![Mat4::IDENTITY; 10],
        vec![Mat4::IDENTITY; 10],
        None,
    )
    .and_then(|skeleton| skeleton.with_clips(clips))
    .and_then(|skeleton| skeleton.with_joint_names(names))
    .expect("rig")
}

const SET: &str = r#"
upper_body = "chest"

[[locomotion]]
clip = "idle"
speed = 0.0

[[locomotion]]
clip = "run"
speed = 4.0

[[states]]
name = "attack"
clip = "attack"
layer = "upper"
release = 0.35

[[states]]
name = "cast1"
clip = "cast1"
layer = "upper"
release = 0.3

[[states]]
name = "death"
clip = "death"
mode = "hold"

[[states]]
name = "hit"
clip = "hit"
layer = "additive"

[triggers]
attack = "attack"
casts = ["cast1"]
death = "death"
hit = "hit"

[[abilities]]
name = "Fireball"
state = "cast1"

[[springs]]
joints = ["cape_0", "cape_1", "cape_2"]

[[sockets]]
name = "weapon"
joint = "arm_r"
offset = [0.0, -0.8, 0.2]
"#;

/// Evaluates `units` animators for one 144 Hz frame; every `stride`-th unit (offset by the
/// frame) is evaluated, the rest reuse their last matrices, like the client's LOD.
fn animate(
    graph: &AnimationGraph,
    skeleton: &Skeleton,
    animators: &mut [Animator],
    scratch: &mut AnimationScratch,
    frame: u32,
    stride: usize,
) -> f32 {
    let now = frame as f32 / 144.0;
    let mut checksum = 0.0;
    for (unit, animator) in animators.iter_mut().enumerate() {
        if !(unit + frame as usize).is_multiple_of(stride) {
            checksum += animator.matrices().first().map_or(0.0, |m| m[0][0]);
            continue;
        }
        // Retrigger the swing and the flinch on a cadence so actions are always blending.
        if (frame as usize + unit).is_multiple_of(120) {
            animator.trigger(graph, AnimationEvent::Attack { interval: 0.8 }, now + 0.3);
            animator.trigger(graph, AnimationEvent::Hit, now);
        }
        let speed = if unit % 2 == 0 { 6.0 } else { 0.0 };
        let world = Mat4::from_translation(Vec3::new(now * speed, 0.0, unit as f32));
        checksum += animator.evaluate(graph, skeleton, now, speed, world, scratch)[0][0][0];
    }
    checksum
}

fn bench_animator(c: &mut Criterion) {
    let skeleton = rig();
    let graph =
        AnimationGraph::bind(&AnimationSet::parse(SET).expect("set"), &skeleton).expect("bind");
    let mut scratch = AnimationScratch::default();
    for (label, units, stride) in [
        ("1_unit", 1, 1),
        ("200_units", 200, 1),
        ("200_units_lod4", 200, 4),
    ] {
        let mut animators: Vec<Animator> = (0..units)
            .map(|unit| Animator::new(unit as f32 * 0.137))
            .collect();
        let mut frame = 0u32;
        c.bench_function(&format!("animator/{label}"), |b| {
            b.iter(|| {
                frame = frame.wrapping_add(1);
                black_box(animate(
                    &graph,
                    &skeleton,
                    &mut animators,
                    &mut scratch,
                    frame,
                    stride,
                ));
            });
        });
    }
}

criterion_group!(benches, bench_sample, bench_animator);
criterion_main!(benches);
