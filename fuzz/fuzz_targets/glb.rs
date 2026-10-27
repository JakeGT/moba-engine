#![no_main]

use libfuzzer_sys::fuzz_target;

// Model files come from a client-local, player-controlled asset directory: the loader must
// return an error, never panic, and stay within its size/vertex/index caps.
fuzz_target!(|data: &[u8]| {
    if let Ok(mesh) = moba_assets::load_glb(data) {
        assert!(mesh.vertex_count() <= moba_assets::MeshData::MAX_VERTICES);
        assert!(mesh.indices().len() <= moba_assets::MeshData::MAX_INDICES);
        assert!(
            mesh.indices()
                .iter()
                .all(|&index| (index as usize) < mesh.vertex_count())
        );
    }
});
