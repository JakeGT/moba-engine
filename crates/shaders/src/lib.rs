//! GLSL and Slang shader sources, compiled to SPIR-V at build time.
//!
//! The build script compiles GLSL with `glslc` (or `glslangValidator`) and validates every
//! module with `spirv-val` when it is installed. New tiered passes are written in **Slang**
//! (ADR-0020 §4): `shaders/slang/*.slang`, compiled by the `slangc` pinned in
//! `slang-version.txt` (fetch it once with `scripts/fetch-slang.sh`, or `.ps1` on Windows) and
//! always validated by `spirv-val`. The renderer consumes the byte slices below and never
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
//!
//! With the `fsr3` feature, [`fsr3`] also holds the AMD FSR 3.1.6 upscaler passes, built from
//! the vendored MIT HLSL in `third_party/fidelityfx/` (ADR-0021 §4b).

#![forbid(unsafe_code)]

#[cfg(feature = "fsr3")]
pub mod fsr3;

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

/// Tiered-path smoke test (ADR-0020 gate 3), graphics half: samples bindless textures through
/// divergent handles (`nonuniformEXT`). Created only on a device admitted to the tiered path,
/// which enables the descriptor-indexing and buffer-device-address features it declares.
pub const BINDLESS_SMOKE_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/bindless_smoke.frag.spv"));
/// Tiered-path smoke test, compute half: the same sampling written through a bindless storage
/// image without a format. Tiered devices only.
pub const BINDLESS_SMOKE_COMP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/bindless_smoke.comp.spv"));
/// Tiered-path ray-query smoke test (ADR-0020 gate 3): closest hits against a TLAS written as
/// integers. Tiered devices only (`VK_KHR_ray_query` is in the tiered required set).
pub const RT_SMOKE_COMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rt_smoke.comp.spv"));

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

/// Slang toolchain smoke module (`slang/toolchain_smoke.slang`, ADR-0020 §4c): a compute
/// entry point `main` (16 threads) that copies the frame's vision circles (set 0, binding 0)
/// into a storage buffer (set 0, binding 1) and writes `mul(view_proj, camera_pos)` after
/// them. The renderer does not create it; `moba_render::slang_smoke` (the `vulkan-probe
/// --slang-smoke` CI step) dispatches it once and checks the output.
pub const TOOLCHAIN_SMOKE_SLANG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/toolchain_smoke.slang.spv"));

/// Unit depth pre-pass with motion vectors (`unit.vert` + `DEPTH_PREPASS` + `MOTION_VECTORS`, ADR-0021 §2).
pub const UNIT_MOTION_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_motion.vert.spv"));
/// Skinned depth pre-pass with motion vectors (previous bone palette, ADR-0021 §2).
pub const SKINNED_MOTION_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_motion.vert.spv"));
/// VAT depth pre-pass with motion vectors (previous VAT pose, ADR-0021 §2).
pub const VAT_MOTION_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vat_motion.vert.spv"));
/// Terrain depth pre-pass with camera motion (ADR-0021 §2).
pub const TERRAIN_MOTION_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/terrain_motion.vert.spv"));
/// Foliage depth pre-pass with motion from the previous wind time and displacers (ADR-0021 §2).
pub const FOLIAGE_MOTION_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/foliage_motion.vert.spv"));
/// Foliage colour pass with an `invariant` position (matches `foliage_motion.vert`'s depth).
pub const FOLIAGE_TIERED_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/foliage_tiered.vert.spv"));
/// Pre-pass fragment shader writing motion vectors (ADR-0021 §2a).
pub const PREPASS_MOTION_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/prepass_motion.frag.spv"));
/// Pre-pass fragment shader writing motion vectors and the unit mask (`PREPASS_IDS`).
pub const PREPASS_MOTION_IDS_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/prepass_motion_ids.frag.spv"));
/// Sky motion vectors (camera rotation only, ADR-0021 §2).
pub const SKY_MOTION_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky_motion.frag.spv"));
/// Water's transparency-and-composition mask (ADR-0021 §2b).
pub const TC_MASK_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tc_mask.frag.spv"));
/// The post-upscale gameplay overlay (ADR-0021 §5): outlines, rims and telegraphs at output resolution.
pub const OVERLAY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay.frag.spv"));
/// The overlay pre-pass's ground and water IDs (ADR-0021 §5).
pub const OVERLAY_GROUND_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/overlay_ground.frag.spv"));
/// The terrain without glow decals (they draw in the overlay, ADR-0021 §5).
pub const TERRAIN_TIERED_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/terrain_tiered.frag.spv"));
/// Foliage without glow decals (ADR-0021 §5).
pub const FOLIAGE_TIERED_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/foliage_tiered.frag.spv"));
/// Water without glow decals (ADR-0021 §5).
pub const WATER_TIERED_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/water_tiered.frag.spv"));
/// Refracting water without glow decals (ADR-0021 §5).
pub const WATER_REFRACTION_TIERED_FRAG: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/water_refraction_tiered.frag.spv"
));
/// Particles with motion vectors; telegraphs skipped (ADR-0021 §2, §5).
pub const VFX_TEMPORAL_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_temporal.vert.spv"));
/// Telegraph particles only, in the overlay (ADR-0021 §5).
pub const VFX_OVERLAY_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_overlay.vert.spv"));
/// Particles writing the reactive mask (ADR-0021 §2b).
pub const VFX_TEMPORAL_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_temporal.frag.spv"));
/// OIT particles writing the reactive mask (ADR-0021 §2b).
pub const VFX_OIT_TEMPORAL_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_oit_temporal.frag.spv"));
/// The particles' motion-only draw: coverage from 0.1 replaces the motion (ADR-0021 §2b,
/// amendment 4). One module for both blend paths: the draw writes no colour, so `VFX_OIT`
/// would compile to the same SPIR-V.
pub const VFX_MOTION_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vfx_motion.frag.spv"));
/// Telegraph particles tone-mapped in the overlay (ADR-0021 §5).
pub const VFX_OVERLAY_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_overlay.frag.spv"));
/// Mesh particles with motion vectors (ADR-0021 §2).
pub const VFX_MESH_TEMPORAL_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_mesh_temporal.vert.spv"));
/// Distortion writing the reactive mask (ADR-0021 §2b).
pub const VFX_DISTORT_TEMPORAL_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vfx_distort_temporal.frag.spv"));
/// The G-buffer prepass's static meshes (ADR-0023 §1e): varyings, outline slot, motion.
pub const UNIT_GBUFFER_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_gbuffer.vert.spv"));
/// The G-buffer prepass's textured static meshes (ADR-0023 §1e).
pub const UNIT_TEXTURED_GBUFFER_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_textured_gbuffer.vert.spv"));
/// The G-buffer prepass's skinned meshes (ADR-0023 §1e).
pub const SKINNED_GBUFFER_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skinned_gbuffer.vert.spv"));
/// The G-buffer prepass's textured skinned meshes (ADR-0023 §1e).
pub const SKINNED_TEXTURED_GBUFFER_VERT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/skinned_textured_gbuffer.vert.spv"
));
/// The G-buffer prepass's VAT meshes (ADR-0023 §1e).
pub const VAT_GBUFFER_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vat_gbuffer.vert.spv"));
/// Units into the G-buffer and motion (ADR-0023 §1e).
pub const UNIT_GBUFFER_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_gbuffer.frag.spv"));
/// Units into the G-buffer, motion and the outline IDs (ADR-0023 §1e).
pub const UNIT_GBUFFER_IDS_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_gbuffer_ids.frag.spv"));
/// Textured units into the G-buffer and motion (ADR-0023 §1e).
pub const UNIT_TEXTURED_GBUFFER_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/unit_textured_gbuffer.frag.spv"));
/// Textured units into the G-buffer, motion and the outline IDs (ADR-0023 §1e).
pub const UNIT_TEXTURED_GBUFFER_IDS_FRAG: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/unit_textured_gbuffer_ids.frag.spv"
));
/// The terrain into the G-buffer and motion (ADR-0023 §1e).
pub const TERRAIN_GBUFFER_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/terrain_gbuffer.frag.spv"));
/// Foliage into the G-buffer (`FOLIAGE` model) and motion (ADR-0023 §1e).
pub const FOLIAGE_GBUFFER_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/foliage_gbuffer.frag.spv"));
/// The G-buffer debug views (ADR-0023 §1f): one decoded channel over the upscaler's output.
pub const GBUFFER_DEBUG_SLANG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gbuffer_debug.slang.spv"));
/// The sky backdrop at the reversed-Z far plane (Tiered, ADR-0023 §1c).
pub const SKY_REVERSED_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/sky_reversed.vert.spv"));
/// GTAO reconstructing from reversed-Z depth (Tiered, ADR-0023 §1c).
pub const GTAO_REVERSED_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gtao_reversed.frag.spv"));
/// Contact shadows marching reversed-Z depth (Tiered, ADR-0023 §1c).
pub const CONTACT_SHADOWS_REVERSED_FRAG: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/contact_shadows_reversed.frag.spv"
));
/// The linear exposure multiplier the upscaler reads (ADR-0021 §4a), a 1×1 compute pass.
pub const EXPOSURE_LINEAR_SLANG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/exposure_linear.slang.spv"));
/// The in-house temporal AA and upscaler (ADR-0021 §3) with fused RCAS.
pub const TEMPORAL_TAAU_SLANG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/temporal_taau.slang.spv"));
/// `gbuffer_smoke`'s raster half (ADR-0023 gate 1): `encode_gbuffer` (include/gbuffer.glsl)
/// into the three G-buffer targets, one fixed surface per pixel.
pub const GBUFFER_SMOKE_FRAG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gbuffer_smoke.frag.spv"));
/// `gbuffer_smoke`'s Slang half: `encode_gbuffer` and `decode_gbuffer` (lib/gbuffer.slang).
pub const GBUFFER_SMOKE_SLANG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gbuffer_smoke.slang.spv"));

include!(concat!(env!("OUT_DIR"), "/slang_reflection.rs"));

/// The `slangc -reflection-json` output of the Slang module `name` (for example
/// `"toolchain_smoke.slang"`), if it exists: for layout tests that compare a Slang structure
/// with its Rust `#[repr(C)]` mirror (ADR-0020 §4c). Nothing at run time calls it, so the
/// linker drops the JSON from shipped binaries. (Not a cargo feature: a dev-dependency
/// feature would build this crate, and run its build script, twice.)
#[must_use]
pub fn reflection(name: &str) -> Option<&'static str> {
    SLANG_REFLECTION
        .iter()
        .find(|(module, _)| *module == name)
        .map(|(_, json)| *json)
}

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
    ShaderModule {
        name: "bindless_smoke.frag",
        spirv: BINDLESS_SMOKE_FRAG,
    },
    ShaderModule {
        name: "bindless_smoke.comp",
        spirv: BINDLESS_SMOKE_COMP,
    },
    ShaderModule {
        name: "rt_smoke.comp",
        spirv: RT_SMOKE_COMP,
    },
    ShaderModule {
        name: "toolchain_smoke.slang",
        spirv: TOOLCHAIN_SMOKE_SLANG,
    },
    ShaderModule {
        name: "unit_motion.vert",
        spirv: UNIT_MOTION_VERT,
    },
    ShaderModule {
        name: "skinned_motion.vert",
        spirv: SKINNED_MOTION_VERT,
    },
    ShaderModule {
        name: "vat_motion.vert",
        spirv: VAT_MOTION_VERT,
    },
    ShaderModule {
        name: "terrain_motion.vert",
        spirv: TERRAIN_MOTION_VERT,
    },
    ShaderModule {
        name: "foliage_motion.vert",
        spirv: FOLIAGE_MOTION_VERT,
    },
    ShaderModule {
        name: "foliage_tiered.vert",
        spirv: FOLIAGE_TIERED_VERT,
    },
    ShaderModule {
        name: "prepass_motion.frag",
        spirv: PREPASS_MOTION_FRAG,
    },
    ShaderModule {
        name: "prepass_motion_ids.frag",
        spirv: PREPASS_MOTION_IDS_FRAG,
    },
    ShaderModule {
        name: "sky_motion.frag",
        spirv: SKY_MOTION_FRAG,
    },
    ShaderModule {
        name: "tc_mask.frag",
        spirv: TC_MASK_FRAG,
    },
    ShaderModule {
        name: "overlay.frag",
        spirv: OVERLAY_FRAG,
    },
    ShaderModule {
        name: "overlay_ground.frag",
        spirv: OVERLAY_GROUND_FRAG,
    },
    ShaderModule {
        name: "terrain_tiered.frag",
        spirv: TERRAIN_TIERED_FRAG,
    },
    ShaderModule {
        name: "foliage_tiered.frag",
        spirv: FOLIAGE_TIERED_FRAG,
    },
    ShaderModule {
        name: "water_tiered.frag",
        spirv: WATER_TIERED_FRAG,
    },
    ShaderModule {
        name: "water_refraction_tiered.frag",
        spirv: WATER_REFRACTION_TIERED_FRAG,
    },
    ShaderModule {
        name: "vfx_temporal.vert",
        spirv: VFX_TEMPORAL_VERT,
    },
    ShaderModule {
        name: "vfx_overlay.vert",
        spirv: VFX_OVERLAY_VERT,
    },
    ShaderModule {
        name: "vfx_temporal.frag",
        spirv: VFX_TEMPORAL_FRAG,
    },
    ShaderModule {
        name: "vfx_oit_temporal.frag",
        spirv: VFX_OIT_TEMPORAL_FRAG,
    },
    ShaderModule {
        name: "vfx_motion.frag",
        spirv: VFX_MOTION_FRAG,
    },
    ShaderModule {
        name: "vfx_overlay.frag",
        spirv: VFX_OVERLAY_FRAG,
    },
    ShaderModule {
        name: "vfx_mesh_temporal.vert",
        spirv: VFX_MESH_TEMPORAL_VERT,
    },
    ShaderModule {
        name: "vfx_distort_temporal.frag",
        spirv: VFX_DISTORT_TEMPORAL_FRAG,
    },
    ShaderModule {
        name: "unit_gbuffer.vert",
        spirv: UNIT_GBUFFER_VERT,
    },
    ShaderModule {
        name: "unit_textured_gbuffer.vert",
        spirv: UNIT_TEXTURED_GBUFFER_VERT,
    },
    ShaderModule {
        name: "skinned_gbuffer.vert",
        spirv: SKINNED_GBUFFER_VERT,
    },
    ShaderModule {
        name: "skinned_textured_gbuffer.vert",
        spirv: SKINNED_TEXTURED_GBUFFER_VERT,
    },
    ShaderModule {
        name: "vat_gbuffer.vert",
        spirv: VAT_GBUFFER_VERT,
    },
    ShaderModule {
        name: "unit_gbuffer.frag",
        spirv: UNIT_GBUFFER_FRAG,
    },
    ShaderModule {
        name: "unit_gbuffer_ids.frag",
        spirv: UNIT_GBUFFER_IDS_FRAG,
    },
    ShaderModule {
        name: "unit_textured_gbuffer.frag",
        spirv: UNIT_TEXTURED_GBUFFER_FRAG,
    },
    ShaderModule {
        name: "unit_textured_gbuffer_ids.frag",
        spirv: UNIT_TEXTURED_GBUFFER_IDS_FRAG,
    },
    ShaderModule {
        name: "terrain_gbuffer.frag",
        spirv: TERRAIN_GBUFFER_FRAG,
    },
    ShaderModule {
        name: "foliage_gbuffer.frag",
        spirv: FOLIAGE_GBUFFER_FRAG,
    },
    ShaderModule {
        name: "gbuffer_debug.slang",
        spirv: GBUFFER_DEBUG_SLANG,
    },
    ShaderModule {
        name: "sky_reversed.vert",
        spirv: SKY_REVERSED_VERT,
    },
    ShaderModule {
        name: "gtao_reversed.frag",
        spirv: GTAO_REVERSED_FRAG,
    },
    ShaderModule {
        name: "contact_shadows_reversed.frag",
        spirv: CONTACT_SHADOWS_REVERSED_FRAG,
    },
    ShaderModule {
        name: "exposure_linear.slang",
        spirv: EXPOSURE_LINEAR_SLANG,
    },
    ShaderModule {
        name: "temporal_taau.slang",
        spirv: TEMPORAL_TAAU_SLANG,
    },
    ShaderModule {
        name: "gbuffer_smoke.frag",
        spirv: GBUFFER_SMOKE_FRAG,
    },
    ShaderModule {
        name: "gbuffer_smoke.slang",
        spirv: GBUFFER_SMOKE_SLANG,
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
    use std::borrow::Cow;

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

    /// Every Slang module is SPIR-V 1.6 (the Vulkan 1.3 profile, ADR-0020 §4c).
    #[test]
    fn slang_modules_target_spirv_1_6() {
        for module in MODULES.iter().filter(|module| {
            module
                .name
                .rsplit_once('.')
                .is_some_and(|(_, kind)| kind == "slang")
        }) {
            let version = u32::from_le_bytes(module.spirv[4..8].try_into().unwrap());
            assert_eq!(version, 0x0001_0600, "{} is not SPIR-V 1.6", module.name);
        }
        assert!(module("toolchain_smoke.slang").is_some());
    }

    /// Where a reflected Slang parameter lives.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Slot {
        /// One descriptor binding, with its reflected type: `constantBuffer`,
        /// `<baseShape>:<access>` for a resource (`structuredBuffer:readWrite`,
        /// `texture2D:read`, `texture2D:read:combined` for a combined image sampler, ...),
        /// `samplerState`, and `[N]<element>` for an array, so a type or array-size change
        /// that the Rust set layout would not match fails here too.
        Descriptor {
            set: u64,
            binding: u64,
            ty: Cow<'static, str>,
        },
        /// A `ParameterBlock` taking the whole set (its interior is laid out in field order:
        /// an implicit uniform buffer at binding 0 when the block has ordinary data, then
        /// one binding per resource field).
        Block { set: u64 },
        /// Push constants.
        PushConstants,
    }

    /// Every parameter of every Slang module, with its set and binding. ADR-0020 §4c wants
    /// explicit layouts; `slangc` has no flag that rejects implicit bindings, so the
    /// reflected layout must match this table exactly. A new module, resource or binding
    /// change is a deliberate edit here (and in the Rust descriptor layout that mirrors it).
    const SLANG_BINDINGS: &[(&str, &[(&str, Slot)])] = &[
        (
            "toolchain_smoke.slang",
            &[
                (
                    "camera",
                    Slot::Descriptor {
                        set: 0,
                        binding: 0,
                        ty: Cow::Borrowed("constantBuffer"),
                    },
                ),
                (
                    "circles",
                    Slot::Descriptor {
                        set: 0,
                        binding: 1,
                        ty: Cow::Borrowed("structuredBuffer:readWrite"),
                    },
                ),
            ],
        ),
        (
            "gbuffer_debug.slang",
            &[
                (
                    "gb0",
                    Slot::Descriptor {
                        set: 0,
                        binding: 0,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "gb1",
                    Slot::Descriptor {
                        set: 0,
                        binding: 1,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "gb2",
                    Slot::Descriptor {
                        set: 0,
                        binding: 2,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "exposure",
                    Slot::Descriptor {
                        set: 0,
                        binding: 3,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "hdr_out",
                    Slot::Descriptor {
                        set: 0,
                        binding: 4,
                        ty: Cow::Borrowed("texture2D:readWrite"),
                    },
                ),
                ("push", Slot::PushConstants),
            ],
        ),
        (
            "exposure_linear.slang",
            &[
                (
                    "adapted_ev",
                    Slot::Descriptor {
                        set: 0,
                        binding: 0,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "exposure_linear",
                    Slot::Descriptor {
                        set: 0,
                        binding: 1,
                        ty: Cow::Borrowed("texture2D:readWrite"),
                    },
                ),
                ("push", Slot::PushConstants),
            ],
        ),
        (
            "temporal_taau.slang",
            &[
                (
                    "color",
                    Slot::Descriptor {
                        set: 0,
                        binding: 0,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "depth",
                    Slot::Descriptor {
                        set: 0,
                        binding: 1,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "motion",
                    Slot::Descriptor {
                        set: 0,
                        binding: 2,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "reactive",
                    Slot::Descriptor {
                        set: 0,
                        binding: 3,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "tc_mask",
                    Slot::Descriptor {
                        set: 0,
                        binding: 4,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "exposure",
                    Slot::Descriptor {
                        set: 0,
                        binding: 5,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "history_in",
                    Slot::Descriptor {
                        set: 0,
                        binding: 6,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "history_out",
                    Slot::Descriptor {
                        set: 0,
                        binding: 7,
                        ty: Cow::Borrowed("texture2D:readWrite"),
                    },
                ),
                (
                    "output",
                    Slot::Descriptor {
                        set: 0,
                        binding: 8,
                        ty: Cow::Borrowed("texture2D:readWrite"),
                    },
                ),
                (
                    "prev_depth_in",
                    Slot::Descriptor {
                        set: 0,
                        binding: 9,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "prev_depth_out",
                    Slot::Descriptor {
                        set: 0,
                        binding: 10,
                        ty: Cow::Borrowed("texture2D:readWrite"),
                    },
                ),
                (
                    "linear_clamp",
                    Slot::Descriptor {
                        set: 0,
                        binding: 11,
                        ty: Cow::Borrowed("samplerState"),
                    },
                ),
                ("push", Slot::PushConstants),
            ],
        ),
        (
            "gbuffer_smoke.slang",
            &[
                (
                    "surfaces",
                    Slot::Descriptor {
                        set: 0,
                        binding: 0,
                        ty: Cow::Borrowed("structuredBuffer:read"),
                    },
                ),
                (
                    "encoded",
                    Slot::Descriptor {
                        set: 0,
                        binding: 1,
                        ty: Cow::Borrowed("structuredBuffer:readWrite"),
                    },
                ),
                (
                    "gb0",
                    Slot::Descriptor {
                        set: 0,
                        binding: 2,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "gb1",
                    Slot::Descriptor {
                        set: 0,
                        binding: 3,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "gb2",
                    Slot::Descriptor {
                        set: 0,
                        binding: 4,
                        ty: Cow::Borrowed("texture2D:read"),
                    },
                ),
                (
                    "decoded",
                    Slot::Descriptor {
                        set: 0,
                        binding: 5,
                        ty: Cow::Borrowed("structuredBuffer:readWrite"),
                    },
                ),
                ("push", Slot::PushConstants),
            ],
        ),
    ];

    /// The slot of one reflected parameter, or why it is not allowed.
    fn slot(parameter: &serde_json::Value) -> Result<Slot, String> {
        if parameter.get("bindings").is_some() {
            return Err(String::from(
                "it spans several binding kinds (a global struct holding resources?); \
                 use a ConstantBuffer or a ParameterBlock",
            ));
        }
        let binding = &parameter["binding"];
        let index = binding["index"].as_u64();
        match (binding["kind"].as_str(), index) {
            (Some("descriptorTableSlot"), Some(index)) => Ok(Slot::Descriptor {
                set: binding["space"].as_u64().unwrap_or(0),
                binding: index,
                ty: Cow::Owned(descriptor_type(&parameter["type"])),
            }),
            (Some("subElementRegisterSpace" | "registerSpace"), Some(set)) => {
                Ok(Slot::Block { set })
            }
            (Some("pushConstantBuffer"), _) => Ok(Slot::PushConstants),
            (Some("uniform"), _) => Err(String::from(
                "a loose `uniform` global lands in an implicit globals uniform buffer; \
                 declare it inside a ConstantBuffer with [[vk::binding(binding, set)]]",
            )),
            (Some("specializationConstant"), _) => Err(String::from(
                "specialization constants ([vk::constant_id]) are not in the table yet; add \
                 a Slot for them with the first module that needs one",
            )),
            _ => Err(format!("unexpected binding {binding}")),
        }
    }

    /// The reflected type of a descriptor binding, as [`Slot::Descriptor`] spells it.
    fn descriptor_type(ty: &serde_json::Value) -> String {
        match ty["kind"].as_str().unwrap_or("?") {
            // `Sampler2D` (a combined image sampler) reflects as `texture2D` plus
            // `"combined": true`; it is a different Vulkan descriptor type.
            "resource" => format!(
                "{}:{}{}",
                ty["baseShape"].as_str().unwrap_or("?"),
                ty["access"].as_str().unwrap_or("read"),
                if ty["combined"].as_bool() == Some(true) {
                    ":combined"
                } else {
                    ""
                },
            ),
            "array" => format!(
                "[{}]{}",
                ty["elementCount"].as_u64().unwrap_or(0),
                descriptor_type(&ty["elementType"]),
            ),
            kind => kind.to_owned(),
        }
    }

    /// Whether an entry-point parameter takes a uniform binding. Varying inputs and outputs
    /// (`POSITION`, `TEXCOORD0`, a fragment shader's `VOut` struct) reflect a
    /// `varyingInput`/`varyingOutput` binding and are fine; system values reflect none.
    fn entry_point_uniform(parameter: &serde_json::Value) -> bool {
        let varying = |binding: &serde_json::Value| {
            matches!(
                binding["kind"].as_str(),
                Some("varyingInput" | "varyingOutput")
            )
        };
        let single = parameter
            .get("binding")
            .is_some_and(|binding| !varying(binding));
        let several = parameter["bindings"]
            .as_array()
            .is_some_and(|bindings| bindings.iter().any(|binding| !varying(binding)));
        single || several
    }

    /// Varying entry-point parameters pass; uniform ones (any other binding kind) do not.
    #[test]
    fn entry_point_varyings_are_not_uniforms() {
        let parse = |json: &str| serde_json::from_str::<serde_json::Value>(json).unwrap();
        for varying in [
            r#"{"name":"thread","semanticName":"SV_DISPATCHTHREADID"}"#,
            r#"{"name":"p","binding":{"kind":"varyingInput","index":0}}"#,
            r#"{"name":"v","bindings":[{"kind":"varyingInput","index":0,"count":2},
                {"kind":"varyingOutput","index":0}]}"#,
        ] {
            assert!(!entry_point_uniform(&parse(varying)), "{varying}");
        }
        for uniform in [
            r#"{"name":"tint","binding":{"kind":"uniform","offset":0,"size":16}}"#,
            r#"{"name":"tex","binding":{"kind":"descriptorTableSlot","index":0}}"#,
            r#"{"name":"mixed","bindings":[{"kind":"varyingInput","index":0},
                {"kind":"descriptorTableSlot","index":1}]}"#,
        ] {
            assert!(entry_point_uniform(&parse(uniform)), "{uniform}");
        }
    }

    /// ADR-0020 §4c: the reflected descriptor layout of every Slang module (globals, `lib/`
    /// imports and entry-point parameters alike) is exactly [`SLANG_BINDINGS`].
    #[test]
    fn slang_descriptor_bindings_match_the_table() {
        let mut built: Vec<&str> = SLANG_REFLECTION.iter().map(|(name, _)| *name).collect();
        let mut listed: Vec<&str> = SLANG_BINDINGS.iter().map(|(name, _)| *name).collect();
        built.sort_unstable();
        listed.sort_unstable();
        assert_eq!(
            built, listed,
            "every Slang module needs a SLANG_BINDINGS entry"
        );

        for (module, json) in SLANG_REFLECTION {
            let reflection: serde_json::Value =
                serde_json::from_str(json).expect("slangc reflection JSON");
            let mut problems = Vec::new();
            let mut actual = Vec::new();
            for parameter in reflection["parameters"].as_array().into_iter().flatten() {
                let name = parameter["name"].as_str().unwrap_or("?").to_owned();
                match slot(parameter) {
                    Ok(slot) => actual.push((name, slot)),
                    Err(why) => problems.push(format!("`{name}`: {why}")),
                }
            }
            for entry in reflection["entryPoints"].as_array().into_iter().flatten() {
                for parameter in entry["parameters"].as_array().into_iter().flatten() {
                    if entry_point_uniform(parameter) {
                        problems.push(format!(
                            "entry point `{}` parameter `{}`: uniform entry-point parameters \
                             get implicit bindings; declare a global with \
                             [[vk::binding(binding, set)]]",
                            entry["name"].as_str().unwrap_or("?"),
                            parameter["name"].as_str().unwrap_or("?"),
                        ));
                    }
                }
            }
            assert!(
                problems.is_empty(),
                "{module}:\n  {}",
                problems.join("\n  ")
            );

            let expected = SLANG_BINDINGS
                .iter()
                .find(|(name, _)| name == module)
                .map(|(_, bindings)| *bindings)
                .unwrap_or_default();
            let mut expected: Vec<(String, Slot)> = expected
                .iter()
                .map(|(name, slot)| ((*name).to_owned(), slot.clone()))
                .collect();
            actual.sort();
            expected.sort();
            assert_eq!(
                actual, expected,
                "{module}: reflected bindings vs SLANG_BINDINGS"
            );
        }
    }

    /// Matrix convention (docs/dev/slang.md): Slang treats a `float4x4` as four HLSL rows
    /// and emits it as the transposed SPIR-V matrix, so `-matrix-layout-column-major` shows
    /// up as `RowMajor` on that type. The memory layout is then GLSL's `ColMajor` `mat4`
    /// (glam's `to_cols_array`), and `mul(M, v)` is the Rust `M * v`. This checks every
    /// matrix member of every Slang module carries `RowMajor` (and not `ColMajor`) with a
    /// row stride of 16 bytes, or 8 for two-column rows under std430, so the flag cannot be
    /// flipped silently; `moba_render::slang_smoke` checks the product on a
    /// GPU.
    #[test]
    fn slang_matrices_are_column_major() {
        const OP_MEMBER_DECORATE: u32 = 72;
        const ROW_MAJOR: u32 = 4;
        const COL_MAJOR: u32 = 5;
        const MATRIX_STRIDE: u32 = 7;
        for module in MODULES.iter().filter(|module| {
            module
                .name
                .rsplit_once('.')
                .is_some_and(|(_, kind)| kind == "slang")
        }) {
            let words: Vec<u32> = module
                .spirv
                .as_chunks::<4>()
                .0
                .iter()
                .map(|word| u32::from_le_bytes(*word))
                .collect();
            // (struct, member) -> (matrix stride, row major, column major).
            let mut matrices = std::collections::BTreeMap::new();
            let mut at = 5; // after the header
            while at < words.len() {
                let count = (words[at] >> 16) as usize;
                assert!(count > 0, "{}: malformed SPIR-V", module.name);
                if words[at] & 0xffff == OP_MEMBER_DECORATE && count >= 4 {
                    let key = (words[at + 1], words[at + 2]);
                    let entry = matrices.entry(key).or_insert((None, false, false));
                    match words[at + 3] {
                        MATRIX_STRIDE => entry.0 = Some(words[at + 4]),
                        ROW_MAJOR => entry.1 = true,
                        COL_MAJOR => entry.2 = true,
                        _ => {}
                    }
                }
                at += count;
            }
            let matrices: Vec<_> = matrices
                .into_iter()
                .filter(|(_, (stride, row, column))| stride.is_some() || *row || *column)
                .collect();
            assert!(
                module.name != "toolchain_smoke.slang" || !matrices.is_empty(),
                "the smoke module's frame constants hold matrices"
            );
            for ((structure, member), (stride, row, column)) in matrices {
                assert!(
                    row && !column && matches!(stride, Some(8 | 16)),
                    "{}: struct %{structure} member {member} is not a column-major \
                     (SPIR-V RowMajor) matrix with an 8- or 16-byte stride: stride {stride:?}, \
                     RowMajor {row}, ColMajor {column}",
                    module.name
                );
            }
        }
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
