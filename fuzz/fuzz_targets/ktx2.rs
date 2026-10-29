#![no_main]

use libfuzzer_sys::fuzz_target;

// Textures come from a client-local, player-controlled asset directory (ADR-0010): the KTX2
// loader must return an error, never panic, and stay within its dimension/level/size caps. A
// texture it accepts (a 2D image or a cube map) must satisfy the invariants the renderer's
// upload copies rely on, round trip through the writer, and decode on the CPU fallback path
// without panicking. The same bytes are also tried as a material descriptor and as an
// environment descriptor (other untrusted client-local files).
fuzz_target!(|data: &[u8]| {
    if let Ok(texture) = moba_assets::load_ktx2(data) {
        let (width, height) = (texture.width(), texture.height());
        assert!((1..=moba_assets::MAX_TEXTURE_DIMENSION).contains(&width));
        assert!((1..=moba_assets::MAX_TEXTURE_DIMENSION).contains(&height));
        let faces = texture.face_count();
        assert!(faces == 1 || faces == 6);
        if texture.is_cube() {
            assert_eq!(width, height);
            assert!(width <= moba_assets::MAX_CUBE_DIMENSION);
        }
        let levels = texture.level_count();
        assert!(levels >= 1 && levels <= moba_assets::full_mip_chain(width, height));
        let mut total = 0;
        for level in 0..levels {
            let (w, h, bytes) = texture.level(level).expect("level");
            assert_eq!((w, h), moba_assets::mip_size(width, height, level));
            let face_bytes = texture.format().level_bytes(w, h).expect("size");
            assert_eq!(bytes.len(), face_bytes * faces as usize);
            for face in 0..faces {
                assert_eq!(texture.face(level, face).map(<[u8]>::len), Some(face_bytes));
            }
            assert_eq!(texture.level_offset(level), Some(total));
            total += bytes.len();
        }
        assert_eq!(total, texture.data().len());
        assert!(total as u64 <= moba_assets::MAX_KTX2_BYTES);
        let written = moba_assets::write_ktx2(&texture, "fuzz");
        assert_eq!(moba_assets::load_ktx2(&written).as_ref(), Ok(&texture));
        // Cap the CPU decode so a maximal texture does not slow the fuzzer to a crawl.
        if total <= 1 << 20 {
            let decoded = texture.decode_to_uncompressed().expect("valid BC data decodes");
            assert_eq!(decoded.level_count(), levels);
            assert_eq!(decoded.face_count(), faces);
            assert!(!decoded.format().is_block_compressed());
        }
    }
    if let Ok(text) = core::str::from_utf8(data) {
        if let Ok(material) = moba_assets::MaterialDesc::parse(text) {
            for (_, name) in material.bound_textures() {
                assert!(moba_assets::valid_mesh_name(name));
            }
        }
        if let Ok(environment) = moba_assets::EnvironmentDesc::parse(text) {
            assert!(environment.intensity.is_finite());
            assert!((1..=moba_assets::MAX_SPECULAR_MIPS).contains(&environment.specular_mips));
            let value = environment.irradiance_over_pi([0.0, 1.0, 0.0]);
            assert!(value.iter().all(|channel| channel.is_finite() && *channel >= 0.0));
        }
    }
});
