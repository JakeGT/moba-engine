#![no_main]

//! VFX effect descriptors and bindings (ADR-0013) are untrusted client-local TOML: parsing
//! never panics, and whatever loads stays within the documented caps and ranges.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    if let Ok(effect) = moba_assets::VfxEffect::parse(text) {
        assert!(!effect.emitters.is_empty() || !effect.decals.is_empty());
        assert!(effect.emitters.len() <= moba_assets::MAX_VFX_EMITTERS);
        assert!(effect.decals.len() <= moba_assets::MAX_VFX_DECALS);
        for decal in &effect.decals {
            assert!(decal.radius.is_finite() && decal.radius > 0.0);
            assert!(decal.color.iter().all(|c| c.is_finite()));
            assert!(!(decal.stamp && decal.telegraph));
            assert!(!decal.stamp || decal.lifetime.is_some());
            assert!(
                decal
                    .lifetime
                    .is_none_or(|seconds| seconds <= moba_assets::MAX_DECAL_SECONDS)
            );
        }
        assert!((0.0..=moba_assets::MAX_EFFECT_SECONDS).contains(&effect.duration));
        for emitter in &effect.emitters {
            assert!(emitter.burst <= moba_assets::MAX_VFX_BURST);
            assert!(emitter.rate <= moba_assets::MAX_VFX_RATE);
            assert!(emitter.lifetime[0] <= emitter.lifetime[1]);
            assert!(emitter.lifetime[1] <= moba_assets::MAX_PARTICLE_SECONDS);
            assert!(emitter.color.iter().chain(&emitter.color_end).all(|c| c.is_finite()));
            assert!(u32::from(emitter.sprite.cell) + u32::from(emitter.sprite.frames) <= 64);
            if let Some(mesh) = emitter.mesh.as_deref() {
                assert!(moba_assets::valid_mesh_name(mesh));
            }
        }
        let lifespan = effect.lifespan(f32::from(data.first().copied().unwrap_or(0)));
        assert!(lifespan.is_finite() && lifespan >= 0.0);
    }
    if let Ok(bindings) = moba_assets::VfxBindings::parse(text) {
        assert!(bindings.abilities.len() <= moba_assets::MAX_VFX_ABILITY_BINDINGS);
        for name in bindings.effect_names() {
            assert!(moba_assets::valid_vfx_name(name));
        }
    }
});
