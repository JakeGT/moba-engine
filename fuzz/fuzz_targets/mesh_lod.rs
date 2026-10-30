#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

// Mesh LOD files (`models/<model>.lod`, R8) come from a client-local, player-controlled asset
// directory next to the model. The parser must return an error, never panic, and never
// allocate past its caps. A file it accepts must index only the mesh's vertices, hold whole
// triangles, have finite non-negative errors that never decrease, and round trip through
// the writer. The file is bound to its mesh by hash, so every input is tried against all
// four sample models (seeded with their committed `.lod` files in `corpus/mesh_lod`).
const MODELS: [&[u8]; 4] = [
    include_bytes!("../../content/models/crystal.glb"),
    include_bytes!("../../content/models/rampart.glb"),
    include_bytes!("../../content/models/sentinel.glb"),
    include_bytes!("../../content/models/sentinel-skinned.glb"),
];

fn meshes() -> &'static [moba_assets::MeshData] {
    static MESHES: OnceLock<Vec<moba_assets::MeshData>> = OnceLock::new();
    MESHES.get_or_init(|| {
        MODELS
            .iter()
            .map(|bytes| moba_assets::load_glb(bytes).expect("sample model loads"))
            .collect()
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > moba_assets::MAX_LOD_FILE_BYTES {
        return;
    }
    for mesh in meshes() {
        let Ok(lods) = moba_assets::MeshLods::parse(data, mesh) else {
            continue;
        };
        assert!(lods.levels().len() <= moba_assets::MAX_MESH_LODS);
        let mut previous_error = 0.0f32;
        for level in lods.levels() {
            let indices = level.indices();
            assert!(!indices.is_empty() && indices.len() % 3 == 0);
            assert!(indices.iter().all(|&index| (index as usize) < mesh.vertex_count()));
            assert!(level.error().is_finite() && level.error() >= previous_error);
            previous_error = level.error();
        }
        let round = moba_assets::MeshLods::parse(&lods.to_bytes(mesh), mesh)
            .expect("writer output parses");
        assert_eq!(round, lods);
        // The levels attach to the mesh the renderer draws from.
        let with = mesh.clone().with_lods(lods);
        assert!(with.lods().len() <= moba_assets::MAX_MESH_LODS);
    }
});
