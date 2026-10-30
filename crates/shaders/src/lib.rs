//! GLSL shader sources, compiled to SPIR-V at build time.
//!
//! The build script uses `glslc` (or `glslangValidator`) and validates every module with
//! `spirv-val` when it is installed. The renderer consumes the byte slices below and never
//! ships source code to the driver.
//!
//! Sources share declarations through `#include` files in `shaders/include/` (camera block,
//! instance buffer, lighting, material set, and the `AgX` tonemap that the composite and the
//! UI's offscreen images share). A few sources are also built as **permutations** with
//! preprocessor defines, each its own module: the `*_TEXTURED` modules are the
//! `MATERIAL_TEXTURED` builds that sample a material's texture set (ADR-0010), and the
//! `*_shadow` modules are the `SHADOW_PASS` depth-only casters of the shadow map (ADR-0011),
//! the `*_prepass` modules the `DEPTH_PREPASS` vertex shaders of the depth prepass that also
//! write R2b's outline IDs and the `*_outline` modules the `OUTLINE_HULL` inverted-hull
//! outlines (ADR-0016). `vat.vert` plays vertex-animation textures for minions (R4b,
//! ADR-0012). The R6 post-processing chain (ADR-0015) adds fullscreen passes sharing
//! `include/post.glsl`.

#![forbid(unsafe_code)]

/// Instanced unit vertex shader.
pub const UNIT_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/unit.vert.spv"));
/// Instanced unit fragment shader.
pub const UNIT_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/unit.frag.spv"));
/// Instanced unit vertex shader, `MATERIAL_TEXTURED` permutation (also passes UVs).
pub const UNIT_TEXTURED_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_textured.vert.spv"));
/// Unit fragment shader, `MATERIAL_TEXTURED` permutation: samples the material set (set 2).
pub const UNIT_TEXTURED_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_textured.frag.spv"));
/// Skinned unit vertex shader, `MATERIAL_TEXTURED` permutation (also passes UVs; pairs with
/// [`UNIT_TEXTURED_FRAG`]).
pub const SKINNED_TEXTURED_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_textured.vert.spv"));
/// Skinned unit vertex shader (bone buffer + push-constant bone range; pairs with
/// [`UNIT_FRAG`]).
pub const SKINNED_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/skinned.vert.spv"));
/// Depth-only shadow caster for static meshes (`unit.vert` + `SHADOW_PASS`): position only,
/// through the light's view-projection (R2a, ADR-0011).
pub const UNIT_SHADOW_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_shadow.vert.spv"));
/// Depth-only shadow caster for skinned meshes (`skinned.vert` + `SHADOW_PASS`).
pub const SKINNED_SHADOW_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_shadow.vert.spv"));
/// Depth pre-pass for static meshes (`unit.vert` + `DEPTH_PREPASS`): position and the
/// instance's outline slot (R2b, ADR-0016).
pub const UNIT_PREPASS_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_prepass.vert.spv"));
/// Depth pre-pass for skinned meshes (`skinned.vert` + `DEPTH_PREPASS`).
pub const SKINNED_PREPASS_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_prepass.vert.spv"));
/// Inverted-hull outline for static meshes (`unit.vert` + `OUTLINE_HULL`).
pub const UNIT_OUTLINE_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_outline.vert.spv"));
/// Inverted-hull outline for skinned meshes (`skinned.vert` + `OUTLINE_HULL`).
pub const SKINNED_OUTLINE_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_outline.vert.spv"));
/// Depth pre-pass fragment shader: writes the outline slot into the `R8_UINT` ID target.
pub const PREPASS_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prepass.frag.spv"));
/// Inverted-hull outline fragment shader: the slot's flat palette colour.
pub const OUTLINE_HULL_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/outline_hull.frag.spv"));
/// Screen-space outline fragment shader: dilates the ID target over the tonemapped image.
pub const OUTLINE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outline.frag.spv"));
/// Screen-space contact-shadow fragment shader: a short depth-buffer ray march to the sun.
pub const CONTACT_SHADOWS_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/contact_shadows.frag.spv"));
/// Vertex-animation-texture vertex shader (R4b): plays baked clips from the VAT texture
/// (set 2) by the instance's frame samples; pairs with [`UNIT_FRAG`].
pub const VAT_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vat.vert.spv"));
/// Depth-only shadow caster for VAT meshes (`vat.vert` + `SHADOW_PASS`).
pub const VAT_SHADOW_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vat_shadow.vert.spv"));
/// Depth pre-pass for VAT meshes (`vat.vert` + `DEPTH_PREPASS`, R2b).
pub const VAT_PREPASS_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vat_prepass.vert.spv"));
/// Inverted-hull outline for VAT meshes (`vat.vert` + `OUTLINE_HULL`, R2b).
pub const VAT_OUTLINE_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vat_outline.vert.spv"));
/// Procedural terrain vertex shader.
pub const TERRAIN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.vert.spv"));
/// Procedural terrain fragment shader.
pub const TERRAIN_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.frag.spv"));
/// Fullscreen-triangle vertex shader (no vertex buffer) for fullscreen passes.
pub const FULLSCREEN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fullscreen.vert.spv"));
/// Composite fragment shader: HDR scene colour, bloom, exposure, the `AgX` tonemap, the
/// grading LUT and the gameplay screen effects (R6).
pub const COMPOSITE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/composite.frag.spv"));
/// Bloom down-sample (R6): the 13-tap filter, Karis average on the first level.
pub const BLOOM_DOWN_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bloom_down.frag.spv"));
/// Bloom up-sample (R6): a 3x3 tent added into the larger mip.
pub const BLOOM_UP_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bloom_up.frag.spv"));
/// Auto exposure (R6): log-average metering of the last bloom mip, clamped.
pub const EXPOSURE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/exposure.frag.spv"));
/// FXAA (R6) on the tonemapped image at render resolution.
pub const FXAA_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fxaa.frag.spv"));
/// Render-scale upscale (R6): bilinear or Catmull-Rom bicubic.
pub const UPSCALE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/upscale.frag.spv"));
/// Ground-truth ambient occlusion (R6) at half resolution from the prepass depth.
pub const GTAO_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gtao.frag.spv"));
/// Depth-aware 4x4 AO blur (R6).
pub const AO_BLUR_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ao_blur.frag.spv"));
/// Sky backdrop vertex shader (R3a): a far-plane triangle with unprojected view rays.
pub const SKY_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.vert.spv"));
/// Sky backdrop fragment shader (R3a): environment cube or gradient, horizon haze.
pub const SKY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.frag.spv"));
/// Fog-of-war raster vertex shader (R3a): one quad per vision source.
pub const FOW_RASTER_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fow_raster.vert.spv"));
/// Fog-of-war raster fragment shader (R3a): disc coverage with line-of-sight walls.
pub const FOW_RASTER_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fow_raster.frag.spv"));
/// Fog-of-war resolve fragment shader (R3a): blur and temporal fade.
pub const FOW_RESOLVE_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/fow_resolve.frag.spv"));
/// Screen-space UI vertex shader.
pub const UI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.vert.spv"));
/// Screen-space UI fragment shader.
pub const UI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.frag.spv"));

/// GPU particle simulation compute shader (spawn, integrate, compact, indirect args).
pub const VFX_SIMULATE_COMP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_simulate.comp.spv"));
/// Particle vertex shader: sprites, flipbooks, segments and ground quads.
pub const VFX_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vfx.vert.spv"));
/// Mesh-particle vertex shader.
pub const VFX_MESH_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vfx_mesh.vert.spv"));
/// Particle fragment shader: one premultiplied HDR output (unsorted alpha, Low).
pub const VFX_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vfx.frag.spv"));
/// Particle fragment shader, `VFX_OIT` permutation: HDR + weighted blended OIT targets.
pub const VFX_OIT_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vfx_oit.frag.spv"));
/// Weighted blended OIT resolve onto the HDR target.
pub const VFX_RESOLVE_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_resolve.frag.spv"));
/// Heat-haze vertex shader (ADR-0019): upright sprites from the distortion list.
pub const VFX_DISTORT_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_distort.vert.spv"));
/// Heat-haze fragment shader (ADR-0019): refracts the HDR copy by a capped, depth-tested,
/// unit-masked offset.
pub const VFX_DISTORT_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_distort.frag.spv"));

/// Foliage vertex shader (R3b): procedural, wind-animated blades from the instance buffer.
pub const FOLIAGE_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/foliage.vert.spv"));
/// Foliage fragment shader (R3b).
pub const FOLIAGE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/foliage.frag.spv"));
/// Water vertex shader (R3b).
pub const WATER_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/water.vert.spv"));
/// Water fragment shader (R3b): flow-mapped normals, depth fade, foam; premultiplied alpha.
pub const WATER_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/water.frag.spv"));
/// The terrain without the decal code (`terrain.frag` + `MOBA_NO_DECALS`; decals off).
pub const TERRAIN_NODECALS_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/terrain_nodecals.frag.spv"));
/// Water with screen-space refraction and contact foam (`water.frag` + `WATER_REFRACTION`).
pub const WATER_REFRACTION_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/water_refraction.frag.spv"));

/// A named SPIR-V module, for tooling and validation.
#[derive(Clone, Copy, Debug)]
pub struct ShaderModule {
    /// Human-readable name.
    pub name: &'static str,
    /// SPIR-V bytes.
    pub spirv: &'static [u8],
}

/// Every shader module in the crate.
pub const MODULES: &[ShaderModule] = &[
    ShaderModule {
        name: "foliage.vert",
        spirv: FOLIAGE_VERT,
    },
    ShaderModule {
        name: "foliage.frag",
        spirv: FOLIAGE_FRAG,
    },
    ShaderModule {
        name: "water.vert",
        spirv: WATER_VERT,
    },
    ShaderModule {
        name: "water.frag",
        spirv: WATER_FRAG,
    },
    ShaderModule {
        name: "water_refraction.frag",
        spirv: WATER_REFRACTION_FRAG,
    },
    ShaderModule {
        name: "terrain_nodecals.frag",
        spirv: TERRAIN_NODECALS_FRAG,
    },
    ShaderModule {
        name: "sky.vert",
        spirv: SKY_VERT,
    },
    ShaderModule {
        name: "sky.frag",
        spirv: SKY_FRAG,
    },
    ShaderModule {
        name: "fow_raster.vert",
        spirv: FOW_RASTER_VERT,
    },
    ShaderModule {
        name: "fow_raster.frag",
        spirv: FOW_RASTER_FRAG,
    },
    ShaderModule {
        name: "fow_resolve.frag",
        spirv: FOW_RESOLVE_FRAG,
    },
    ShaderModule {
        name: "unit.vert",
        spirv: UNIT_VERT,
    },
    ShaderModule {
        name: "unit.frag",
        spirv: UNIT_FRAG,
    },
    ShaderModule {
        name: "unit_textured.vert",
        spirv: UNIT_TEXTURED_VERT,
    },
    ShaderModule {
        name: "unit_textured.frag",
        spirv: UNIT_TEXTURED_FRAG,
    },
    ShaderModule {
        name: "skinned.vert",
        spirv: SKINNED_VERT,
    },
    ShaderModule {
        name: "skinned_textured.vert",
        spirv: SKINNED_TEXTURED_VERT,
    },
    ShaderModule {
        name: "unit_shadow.vert",
        spirv: UNIT_SHADOW_VERT,
    },
    ShaderModule {
        name: "skinned_shadow.vert",
        spirv: SKINNED_SHADOW_VERT,
    },
    ShaderModule {
        name: "unit_prepass.vert",
        spirv: UNIT_PREPASS_VERT,
    },
    ShaderModule {
        name: "skinned_prepass.vert",
        spirv: SKINNED_PREPASS_VERT,
    },
    ShaderModule {
        name: "unit_outline.vert",
        spirv: UNIT_OUTLINE_VERT,
    },
    ShaderModule {
        name: "skinned_outline.vert",
        spirv: SKINNED_OUTLINE_VERT,
    },
    ShaderModule {
        name: "prepass.frag",
        spirv: PREPASS_FRAG,
    },
    ShaderModule {
        name: "outline_hull.frag",
        spirv: OUTLINE_HULL_FRAG,
    },
    ShaderModule {
        name: "outline.frag",
        spirv: OUTLINE_FRAG,
    },
    ShaderModule {
        name: "contact_shadows.frag",
        spirv: CONTACT_SHADOWS_FRAG,
    },
    ShaderModule {
        name: "vat.vert",
        spirv: VAT_VERT,
    },
    ShaderModule {
        name: "vat_prepass.vert",
        spirv: VAT_PREPASS_VERT,
    },
    ShaderModule {
        name: "vat_outline.vert",
        spirv: VAT_OUTLINE_VERT,
    },
    ShaderModule {
        name: "vat_shadow.vert",
        spirv: VAT_SHADOW_VERT,
    },
    ShaderModule {
        name: "terrain.vert",
        spirv: TERRAIN_VERT,
    },
    ShaderModule {
        name: "terrain.frag",
        spirv: TERRAIN_FRAG,
    },
    ShaderModule {
        name: "fullscreen.vert",
        spirv: FULLSCREEN_VERT,
    },
    ShaderModule {
        name: "composite.frag",
        spirv: COMPOSITE_FRAG,
    },
    ShaderModule {
        name: "bloom_down.frag",
        spirv: BLOOM_DOWN_FRAG,
    },
    ShaderModule {
        name: "bloom_up.frag",
        spirv: BLOOM_UP_FRAG,
    },
    ShaderModule {
        name: "exposure.frag",
        spirv: EXPOSURE_FRAG,
    },
    ShaderModule {
        name: "fxaa.frag",
        spirv: FXAA_FRAG,
    },
    ShaderModule {
        name: "upscale.frag",
        spirv: UPSCALE_FRAG,
    },
    ShaderModule {
        name: "gtao.frag",
        spirv: GTAO_FRAG,
    },
    ShaderModule {
        name: "ao_blur.frag",
        spirv: AO_BLUR_FRAG,
    },
    ShaderModule {
        name: "ui.vert",
        spirv: UI_VERT,
    },
    ShaderModule {
        name: "ui.frag",
        spirv: UI_FRAG,
    },
    ShaderModule {
        name: "vfx_simulate.comp",
        spirv: VFX_SIMULATE_COMP,
    },
    ShaderModule {
        name: "vfx.vert",
        spirv: VFX_VERT,
    },
    ShaderModule {
        name: "vfx_mesh.vert",
        spirv: VFX_MESH_VERT,
    },
    ShaderModule {
        name: "vfx.frag",
        spirv: VFX_FRAG,
    },
    ShaderModule {
        name: "vfx_oit.frag",
        spirv: VFX_OIT_FRAG,
    },
    ShaderModule {
        name: "vfx_resolve.frag",
        spirv: VFX_RESOLVE_FRAG,
    },
    ShaderModule {
        name: "vfx_distort.vert",
        spirv: VFX_DISTORT_VERT,
    },
    ShaderModule {
        name: "vfx_distort.frag",
        spirv: VFX_DISTORT_FRAG,
    },
];

/// The module called `name` (for example `"unit_textured.frag"`), if it exists.
#[must_use]
pub fn module(name: &str) -> Option<&'static [u8]> {
    MODULES
        .iter()
        .find(|module| module.name == name)
        .map(|module| module.spirv)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPIRV_MAGIC: u32 = 0x0723_0203;

    #[test]
    fn modules_are_well_formed_spirv() {
        for module in MODULES {
            assert!(!module.spirv.is_empty(), "{} is empty", module.name);
            assert_eq!(
                module.spirv.len() % 4,
                0,
                "{} is not word aligned",
                module.name
            );
            let magic = u32::from_le_bytes(module.spirv[..4].try_into().unwrap());
            assert_eq!(magic, SPIRV_MAGIC, "{} has a bad magic number", module.name);
        }
    }

    #[test]
    fn permutations_are_distinct_modules() {
        assert_eq!(module("unit.frag"), Some(UNIT_FRAG));
        assert_eq!(module("unit_textured.frag"), Some(UNIT_TEXTURED_FRAG));
        assert_eq!(module("missing.frag"), None);
        assert_ne!(UNIT_FRAG, UNIT_TEXTURED_FRAG);
        assert_ne!(UNIT_VERT, UNIT_TEXTURED_VERT);
        assert_ne!(SKINNED_VERT, SKINNED_TEXTURED_VERT);
        assert_ne!(UNIT_VERT, UNIT_SHADOW_VERT);
        assert_ne!(SKINNED_VERT, SKINNED_SHADOW_VERT);
        assert_ne!(VFX_FRAG, VFX_OIT_FRAG);
        assert_ne!(UNIT_VERT, UNIT_PREPASS_VERT);
        assert_ne!(UNIT_VERT, UNIT_OUTLINE_VERT);
        assert_ne!(SKINNED_VERT, SKINNED_PREPASS_VERT);
        assert_ne!(SKINNED_VERT, SKINNED_OUTLINE_VERT);
        assert_ne!(VAT_VERT, VAT_SHADOW_VERT);
        assert_ne!(VAT_VERT, VAT_PREPASS_VERT);
        assert_ne!(VAT_VERT, VAT_OUTLINE_VERT);
        let mut names: Vec<_> = MODULES.iter().map(|module| module.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), MODULES.len(), "module names are unique");
    }

    /// The depth prepass (R6, ADR-0015) draws with the scene's own vertex shaders and the
    /// scene tests `LESS_OR_EQUAL` against it, so every one must declare an invariant
    /// position (the textured permutations are built from these sources).
    #[test]
    fn prepass_vertex_shaders_declare_an_invariant_position() {
        for (name, source) in [
            ("terrain.vert", include_str!("../shaders/terrain.vert")),
            ("unit.vert", include_str!("../shaders/unit.vert")),
            ("skinned.vert", include_str!("../shaders/skinned.vert")),
            ("vat.vert", include_str!("../shaders/vat.vert")),
        ] {
            assert!(
                source
                    .lines()
                    .any(|line| line.trim() == "invariant gl_Position;"),
                "{name} must declare `invariant gl_Position;`"
            );
        }
    }
}
