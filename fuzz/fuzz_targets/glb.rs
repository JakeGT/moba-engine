#![no_main]

use libfuzzer_sys::fuzz_target;

// Model files come from a client-local, player-controlled asset directory: the loader must
// return an error, never panic, and stay within its size/vertex/index/joint/clip caps. A
// skinned result must also satisfy the skinning invariants the renderer relies on and pose
// without panicking (the per-frame path): the legacy single-clip pose, every clip of a
// multi-clip file, and the animation runtime driven by the shipped animation set when the
// file's names let it bind (ADR-0012).
const SET: &str = include_str!("../../content/animations/sentinel-skinned.toml");

fuzz_target!(|data: &[u8]| {
    if let Ok(mesh) = moba_assets::load_glb(data) {
        assert!(mesh.vertex_count() <= moba_assets::MeshData::MAX_VERTICES);
        assert!(mesh.indices().len() <= moba_assets::MeshData::MAX_INDICES);
        assert!(
            mesh.indices()
                .iter()
                .all(|&index| (index as usize) < mesh.vertex_count())
        );
        // File tangents: one per vertex, finite, handedness exactly ±1 (R2a).
        if let Some(tangents) = mesh.tangents() {
            assert_eq!(tangents.len(), mesh.vertex_count());
            assert!(
                tangents
                    .iter()
                    .all(|tangent| tangent.iter().all(|value| value.is_finite())
                        && (tangent[3] == 1.0 || tangent[3] == -1.0))
            );
        }
        if let Some(tangents) = mesh.generate_tangents() {
            assert_eq!(tangents.len(), mesh.vertex_count());
        }
        if let Some(skin) = mesh.skin() {
            let joints = skin.skeleton().joint_count();
            assert!((1..=moba_assets::MAX_JOINTS).contains(&joints));
            assert_eq!(skin.joints().len(), mesh.vertex_count());
            assert!(
                skin.joints()
                    .iter()
                    .flatten()
                    .all(|&joint| usize::from(joint) < joints)
            );
            let skeleton = skin.skeleton();
            let mut pose = [[[0.0f32; 4]; 3]; moba_assets::MAX_JOINTS];
            for (time, blend) in [(0.0, 0.0), (0.37, 1.0), (-5.0, 0.5), (1e9, 1.0)] {
                assert!(skeleton.sample(time, blend, &mut pose));
            }
            assert!(skeleton.clips().len() <= moba_assets::MAX_CLIPS);
            let keys: usize = skeleton.clips().iter().map(|clip| clip.key_count()).sum();
            assert!(keys <= moba_assets::MAX_SKELETON_KEYS);
            let mut local = moba_assets::Pose::default();
            for clip in 0..skeleton.clips().len() {
                for wrap in [moba_assets::ClipWrap::Loop, moba_assets::ClipWrap::Clamp] {
                    assert!(skeleton.sample_clip(clip, 0.41, wrap, &mut local));
                }
            }
            let set = moba_assets::AnimationSet::parse(SET).expect("shipped set");
            if let Ok(graph) = moba_assets::AnimationGraph::bind(&set, skeleton) {
                let mut animator = moba_assets::Animator::new(0.0);
                let mut scratch = moba_assets::AnimationScratch::default();
                animator.trigger(
                    &graph,
                    moba_assets::AnimationEvent::Attack { interval: 0.5 },
                    0.2,
                );
                for frame in 0..8u8 {
                    let now = f32::from(frame) * 0.1;
                    let matrices = animator.evaluate(
                        &graph,
                        skeleton,
                        now,
                        f32::from(frame),
                        moba_core::math::Mat4::IDENTITY,
                        &mut scratch,
                    );
                    assert_eq!(matrices.len(), skeleton.joint_count());
                }
            }
        }
    }
});
