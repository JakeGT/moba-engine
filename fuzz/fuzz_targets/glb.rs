#![no_main]

use libfuzzer_sys::fuzz_target;

// Model files come from a client-local, player-controlled asset directory: the loader must
// return an error, never panic, and stay within its size/vertex/index/joint caps. A skinned
// result must also satisfy the skinning invariants the renderer relies on and pose without
// panicking (the per-frame path).
fuzz_target!(|data: &[u8]| {
    if let Ok(mesh) = moba_assets::load_glb(data) {
        assert!(mesh.vertex_count() <= moba_assets::MeshData::MAX_VERTICES);
        assert!(mesh.indices().len() <= moba_assets::MeshData::MAX_INDICES);
        assert!(
            mesh.indices()
                .iter()
                .all(|&index| (index as usize) < mesh.vertex_count())
        );
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
            let mut pose = [[[0.0f32; 4]; 3]; moba_assets::MAX_JOINTS];
            for (time, blend) in [(0.0, 0.0), (0.37, 1.0), (-5.0, 0.5), (1e9, 1.0)] {
                assert!(skin.skeleton().sample(time, blend, &mut pose));
            }
        }
    }
});
