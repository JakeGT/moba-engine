#![allow(missing_docs)]
//! R4b animation CPU cost (perf/0018): VAT minions and bone-LOD gathers.
//!
//! - `vat/200_minions`: the R4 stress case. 200 [`VatPlayer`]s on the shipped sample minion
//!   asset (`content/vat/minion.toml`): half run (idle ↔ run blend, rate by speed), half stand
//!   and attack every 1.2 s (attack crossfaded over locomotion). Each frame computes every
//!   minion's [`VatPose`] at `High` quality and its model-to-world transform (rotation ×
//!   translation × the fit), the whole per-minion CPU path of the client.
//! - `vat/200_minions_low`: the same at `Low` quality (nearest frame, no crossfade).
//! - `skin_lod/gather_10_heroes`: copying a reduced skin's joints out of 10 heroes' full
//!   matrices (LOD1 of a 10-joint rig), the extra CPU work of the bone-count LOD.

use criterion::{Criterion, criterion_group, criterion_main};
use moba_assets::{
    JointMatrix, JointTransform, MeshData, MeshSkin, Skeleton, SkinLod, VatAnimation,
    VatDescriptor, VatPlayer, VatPose, VatQuality,
};
use moba_core::math::{Mat4, Quat, Vec3};
use std::hint::black_box;

const MINIONS: usize = 200;

fn animation() -> VatAnimation {
    let descriptor =
        VatDescriptor::parse(include_str!("../../../content/vat/minion.toml")).expect("descriptor");
    VatAnimation::bind(&descriptor)
}

/// One frame of the stress scene: attack triggers, poses and transforms for every minion.
fn frame(
    animation: &VatAnimation,
    players: &mut [VatPlayer],
    out: &mut [(Mat4, VatPose)],
    now: f32,
    quality: VatQuality,
) {
    let fit = Mat4::from_scale(Vec3::splat(0.58));
    for (index, (player, slot)) in players.iter_mut().zip(out.iter_mut()).enumerate() {
        let running = index % 2 == 0;
        let phase = index as f32 * 0.37;
        let speed = if running { 4.0 } else { 0.0 };
        if !running {
            let local = now + phase;
            let hit = ((local - 0.4) / 1.2).floor() * 1.2 + 1.2 - phase;
            player.attack(animation, hit, 1.2);
        }
        let pose = player.pose(animation, now, speed, quality);
        let facing = phase + now;
        let ground = Vec3::new(index as f32 * 3.0, 0.0, (index / 15) as f32 * 3.0);
        let transform = Mat4::from_rotation_translation(
            Quat::from_rotation_y(core::f32::consts::FRAC_PI_2 - facing),
            ground,
        ) * fit;
        *slot = (transform, pose);
    }
}

fn bench_vat(c: &mut Criterion) {
    let animation = animation();
    let mut group = c.benchmark_group("vat");
    for (name, quality) in [
        ("200_minions", VatQuality::FULL),
        ("200_minions_low", VatQuality::NEAREST),
    ] {
        let mut players: Vec<VatPlayer> = (0..MINIONS)
            .map(|index| VatPlayer::new(&animation, index as f32 * 0.173))
            .collect();
        let mut out = vec![(Mat4::IDENTITY, VatPose::default()); MINIONS];
        let mut tick = 0u32;
        group.bench_function(name, |b| {
            b.iter(|| {
                tick = tick.wrapping_add(1);
                frame(
                    &animation,
                    &mut players,
                    &mut out,
                    black_box(tick as f32 / 60.0),
                    quality,
                );
                black_box(&out);
            });
        });
    }
    group.finish();
}

/// A 10-joint skin shaped like the sample (hips, chest, head, legs, arms, a 3-joint cape) with
/// one vertex per joint pair.
fn skinned() -> MeshData {
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
    let skeleton = Skeleton::new(
        parents,
        vec![JointTransform::IDENTITY; 10],
        vec![Mat4::IDENTITY; 10],
        vec![Mat4::IDENTITY; 10],
        None,
    )
    .expect("skeleton");
    let joints: Vec<[u8; 4]> = (0..10u8)
        .map(|joint| [joint, (joint + 1) % 10, 0, 0])
        .collect();
    let weights = vec![[200, 55, 0, 0]; 10];
    let skin = MeshSkin::new(joints, weights, skeleton).expect("skin");
    MeshData::new(
        vec![[0.0; 3]; 10],
        vec![[0.0, 1.0, 0.0]; 10],
        (0..9).collect(),
    )
    .and_then(|mesh| mesh.with_skin(skin))
    .expect("mesh")
}

fn bench_gather(c: &mut Criterion) {
    let lod = SkinLod::build(&skinned(), 8, 2, &[7, 8, 9]).expect("lod");
    let full: Vec<JointMatrix> = (0..10).map(|joint| [[joint as f32; 4]; 3]).collect();
    let mut out = vec![[[0.0; 4]; 3]; 10 * lod.joint_count()];
    c.bench_function("skin_lod/gather_10_heroes", |b| {
        b.iter(|| {
            for hero in out.chunks_mut(lod.joint_count()) {
                black_box(lod.gather(black_box(&full), hero));
            }
        });
    });
}

criterion_group!(benches, bench_vat, bench_gather);
criterion_main!(benches);
