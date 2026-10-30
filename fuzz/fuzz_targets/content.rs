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
            // A `[terrain]` table (ADR-0014) keeps 1..=8 layers and its relief bound.
            if let Some(ground) = &visuals.ground {
                assert!((1..=moba_assets::MAX_TERRAIN_LAYERS).contains(&ground.layers.len()));
                assert!(ground.relief <= moba_assets::MAX_PLAY_RELIEF);
                // The generator and the consistency check accept anything that parses, and
                // the generator always satisfies its own relief rules.
                let site = moba_assets::TerrainSite::new(256.0)
                    .with_lane(vec![[32.0, 128.0], [128.0, 128.0], [224.0, 128.0]])
                    .with_wall([112.0, 60.0], [112.0, 100.0]);
                let terrain = moba_assets::generate_terrain(ground, &site, 16, 16);
                assert_eq!(terrain.heightfield.heights.len(), 17 * 17);
                // Water beds (ADR-0017) carve within the same relief rules, and the water
                // mesh and foliage stay bounded and finite.
                assert!(visuals.water.len() <= moba_assets::MAX_MAP_WATER);
                let wet =
                    moba_assets::generate_terrain_with_water(ground, &site, &visuals.water, 16, 16);
                let mesh = moba_assets::generate_water(&visuals.water, &wet.heightfield);
                assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices.len()));
                assert!(mesh.vertices.iter().all(|v| v.depth.is_finite()));
                if let Some(foliage) = &visuals.foliage {
                    assert!(foliage.patches.len() <= moba_assets::MAX_FOLIAGE_PATCHES);
                    let clumps = moba_assets::generate_foliage(
                        foliage,
                        ground,
                        &site,
                        &wet.heightfield,
                        &visuals.water,
                        1.0,
                        1024,
                    );
                    assert!(clumps.len() <= 1024);
                    assert!(clumps.iter().all(|c| c.position.iter().all(|v| v.is_finite())));
                }
                let report = moba_assets::check_map_consistency(&visuals, &site);
                assert!(
                    report.errors.iter().all(|error| !error.starts_with("terrain ")),
                    "{:?}",
                    report.errors
                );
            }
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
