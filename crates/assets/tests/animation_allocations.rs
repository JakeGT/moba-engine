//! Allocation guard for the animation runtime (ADR-0012): once the animators and scratch
//! exist, triggering events and evaluating frames must not allocate.

use moba_assets::{
    AnimationClip, AnimationEvent, AnimationGraph, AnimationScratch, AnimationSet, Animator,
    Interpolation, JointTransform, Skeleton, Track, TrackPath,
};
use moba_core::math::{Mat4, Quat, Vec3};

#[global_allocator]
static ALLOCATOR: moba_test_support::CountingAllocator = moba_test_support::CountingAllocator;

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

#[test]
fn steady_state_animation_does_not_allocate() {
    let skeleton = rig();
    let graph =
        AnimationGraph::bind(&AnimationSet::parse(SET).expect("set"), &skeleton).expect("bind");
    let weapon = graph.socket("weapon").expect("socket");
    let mut scratch = AnimationScratch::default();
    let mut animators: Vec<Animator> = (0..16u8)
        .map(|unit| Animator::new(f32::from(unit) * 0.137))
        .collect();
    let mut frame = |animators: &mut Vec<Animator>, tick: u32| {
        let now = tick as f32 / 60.0;
        let mut checksum = 0.0f32;
        for (unit, animator) in animators.iter_mut().enumerate() {
            let unit_tick = tick + unit as u32;
            match unit_tick % 90 {
                0 => {
                    animator.trigger(&graph, AnimationEvent::Attack { interval: 0.8 }, now + 0.2);
                }
                30 => {
                    animator.trigger(
                        &graph,
                        AnimationEvent::Cast {
                            slot: 0,
                            ability: Some("fireball"),
                        },
                        now,
                    );
                }
                45 => {
                    animator.trigger(&graph, AnimationEvent::Hit, now);
                }
                _ => {}
            }
            if unit_tick % 600 == 300 {
                animator.trigger(&graph, AnimationEvent::Death, now);
            } else if unit_tick % 600 == 420 {
                animator.trigger(&graph, AnimationEvent::Respawn, now);
            }
            let speed = if unit % 2 == 0 { 6.0 } else { 0.0 };
            let world = Mat4::from_translation(Vec3::new(now * speed, 0.0, unit as f32));
            let matrices = animator.evaluate(&graph, &skeleton, now, speed, world, &mut scratch);
            checksum += matrices[9][1][3];
            checksum += animator
                .socket_transform(weapon)
                .map_or(0.0, |socket| socket.w_axis.y);
            checksum += animator.report(&graph, now, speed).action_weight;
        }
        checksum
    };
    // Warm-up: the first evaluations initialise the spring particles.
    for tick in 0..60 {
        frame(&mut animators, tick);
    }
    moba_test_support::reset();
    let mut checksum = 0.0;
    for tick in 60..1_260 {
        checksum += frame(&mut animators, tick);
    }
    let (allocations, bytes) = moba_test_support::counters();
    assert!(checksum.is_finite());
    assert_eq!(
        (allocations, bytes),
        (0, 0),
        "1,200 frames of 16 animated units allocated"
    );
}
