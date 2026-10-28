#![no_main]

use libfuzzer_sys::fuzz_target;

/// A pack that already uses `[assets]` and visual names (#285). Each input is also appended to
/// it, so mutations reach the manifest/reference checks (more `[[assets.*]]` entries, fallback
/// chains, paths, a `[[heroes]]` entry with a `model`) instead of failing at the first table.
const ASSET_SEED: &str = r#"
[[assets.units]]
name = "knight"
path = "models/knight.glb"
fallback = "sentinel"

[[assets.units]]
name = "sentinel"
path = "models/sentinel.glb"

[[assets.maps]]
name = "lava"
path = "maps/lava.toml"
"#;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = core::str::from_utf8(data) {
        let _ = moba_game::load_content_toml(text);
        // Map visuals descriptors (#287) are untrusted client-local TOML too.
        if let Ok(visuals) = moba_assets::MapVisuals::parse(text) {
            assert!(visuals.props.len() <= moba_assets::MAX_MAP_PROPS);
            assert!(visuals.mesh_names().count() <= moba_assets::MAX_MAP_MESHES);
        }
        // So are animation sets (ADR-0012): parsing stays within its caps.
        if let Ok(set) = moba_assets::AnimationSet::parse(text) {
            assert!(set.state_count() <= moba_assets::MAX_ANIMATION_STATES);
            assert!(set.spring_joint_count() <= moba_assets::MAX_SPRING_JOINTS);
            assert!(set.socket_count() <= moba_assets::MAX_SOCKETS);
        }
        if let Ok(content) = moba_game::load_content_toml(&format!("{ASSET_SEED}\n{text}")) {
            // Anything that loads must have resolvable, wire-safe visual names.
            for hero in &content.heroes {
                if let Some(model) = hero.model.as_deref() {
                    assert!(moba_game::valid_visual_name(model));
                    assert!(content.assets.unit(model).is_some());
                }
            }
            if let Some(visuals) = content.map.visuals.as_deref() {
                assert!(moba_game::valid_visual_name(visuals));
            }
        }
    }
});
