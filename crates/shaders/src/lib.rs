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
//! `*_shadow` modules are the `SHADOW_PASS` depth-only casters of the shadow map (ADR-0011).
//! `vat.vert` plays vertex-animation textures for minions (R4b, ADR-0012).

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
/// Vertex-animation-texture vertex shader (R4b): plays baked clips from the VAT texture
/// (set 2) by the instance's frame samples; pairs with [`UNIT_FRAG`].
pub const VAT_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vat.vert.spv"));
/// Depth-only shadow caster for VAT meshes (`vat.vert` + `SHADOW_PASS`).
pub const VAT_SHADOW_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vat_shadow.vert.spv"));
/// Procedural terrain vertex shader.
pub const TERRAIN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.vert.spv"));
/// Procedural terrain fragment shader.
pub const TERRAIN_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.frag.spv"));
/// Fullscreen-triangle vertex shader (no vertex buffer) for fullscreen passes.
pub const FULLSCREEN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fullscreen.vert.spv"));
/// Composite fragment shader: HDR scene colour, exposure and the `AgX` tonemap.
pub const COMPOSITE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/composite.frag.spv"));
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
        name: "vat.vert",
        spirv: VAT_VERT,
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
        assert_ne!(VAT_VERT, VAT_SHADOW_VERT);
        let mut names: Vec<_> = MODULES.iter().map(|module| module.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), MODULES.len(), "module names are unique");
    }
}
