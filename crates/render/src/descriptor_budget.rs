//! Per-stage descriptor budgets of a pipeline layout (R3a review).
//!
//! Vulkan bounds, per shader stage, the samplers and sampled images every set of one pipeline
//! layout makes accessible (`maxPerStageDescriptorSamplers`/`SampledImages`, 16 on the
//! minimum conformant device and on `MoltenVK`). A `COMBINED_IMAGE_SAMPLER` counts against
//! both. The scene pipelines share one layout (camera, instances, material, lighting/world),
//! so every binding any of them adds counts for all of them (and for the VAT pipelines' layout,
//! which shares sets 0, 1 and 3): [`check`] sums them from the
//! same binding arrays the set layouts are created from and fails renderer creation with
//! [`RenderError::DescriptorLimit`] instead of a driver error or undefined behaviour.
//!
//! R3b (ADR-0017) also counts storage buffers per stage
//! (`maxPerStageDescriptorStorageBuffers`, 4 on the minimum): the decal buffer is a set-3
//! storage buffer, and the foliage layout adds its instance buffer to the vertex stage.

use ash::vk;

use crate::error::{RenderError, RenderResult};

/// Samplers and sampled images one shader stage can access through a pipeline layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageTotals {
    pub(crate) samplers: u32,
    pub(crate) sampled_images: u32,
    pub(crate) storage_buffers: u32,
}

/// Totals for `stage` over every binding of every set in `sets`.
pub(crate) fn stage_totals(
    sets: &[&[vk::DescriptorSetLayoutBinding<'_>]],
    stage: vk::ShaderStageFlags,
) -> StageTotals {
    let mut totals = StageTotals::default();
    for binding in sets.iter().flat_map(|set| set.iter()) {
        if !binding.stage_flags.intersects(stage) {
            continue;
        }
        let count = binding.descriptor_count;
        match binding.descriptor_type {
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER => {
                totals.samplers += count;
                totals.sampled_images += count;
            }
            vk::DescriptorType::SAMPLER => totals.samplers += count,
            vk::DescriptorType::SAMPLED_IMAGE | vk::DescriptorType::UNIFORM_TEXEL_BUFFER => {
                totals.sampled_images += count;
            }
            vk::DescriptorType::STORAGE_BUFFER | vk::DescriptorType::STORAGE_BUFFER_DYNAMIC => {
                totals.storage_buffers += count;
            }
            _ => {}
        }
    }
    totals
}

/// Fails when any graphics or compute stage of the layout made of `sets` needs more samplers,
/// sampled images or storage buffers than `limits` allow. `layout` names it in the error.
pub(crate) fn check(
    layout: &'static str,
    sets: &[&[vk::DescriptorSetLayoutBinding<'_>]],
    limits: StageTotals,
) -> RenderResult<()> {
    for (stage, name) in [
        (vk::ShaderStageFlags::VERTEX, "vertex"),
        (vk::ShaderStageFlags::FRAGMENT, "fragment"),
        (vk::ShaderStageFlags::COMPUTE, "compute"),
    ] {
        let totals = stage_totals(sets, stage);
        for (kind, needed, limit) in [
            ("samplers", totals.samplers, limits.samplers),
            (
                "sampled images",
                totals.sampled_images,
                limits.sampled_images,
            ),
            (
                "storage buffers",
                totals.storage_buffers,
                limits.storage_buffers,
            ),
        ] {
            if needed > limit {
                return Err(RenderError::DescriptorLimit {
                    layout,
                    stage: name,
                    kind,
                    needed,
                    limit,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{StageTotals, check, stage_totals};
    use crate::error::RenderError;
    use ash::vk;

    /// The Vulkan minimum of `maxPerStageDescriptorSamplers` and `...SampledImages`.
    const VULKAN_MINIMUM_PER_STAGE: u32 = 16;
    /// The Vulkan minimum of `maxPerStageDescriptorStorageBuffers`.
    const VULKAN_MINIMUM_STORAGE_BUFFERS: u32 = 4;

    fn binding(
        ty: vk::DescriptorType,
        count: u32,
        stages: vk::ShaderStageFlags,
    ) -> vk::DescriptorSetLayoutBinding<'static> {
        vk::DescriptorSetLayoutBinding::default()
            .descriptor_type(ty)
            .descriptor_count(count)
            .stage_flags(stages)
    }

    #[test]
    fn combined_samplers_count_as_both_and_stages_are_separate() {
        let fragment = vk::ShaderStageFlags::FRAGMENT;
        let set_a = [
            binding(vk::DescriptorType::UNIFORM_BUFFER, 1, fragment),
            binding(vk::DescriptorType::COMBINED_IMAGE_SAMPLER, 3, fragment),
        ];
        let set_b = [
            binding(vk::DescriptorType::SAMPLER, 1, fragment),
            binding(vk::DescriptorType::SAMPLED_IMAGE, 2, fragment),
            binding(
                vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                2,
                vk::ShaderStageFlags::VERTEX,
            ),
        ];
        let sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 2] = [&set_a, &set_b];
        assert_eq!(
            stage_totals(&sets, fragment),
            StageTotals {
                samplers: 4,
                sampled_images: 5,
                storage_buffers: 0,
            }
        );
        assert_eq!(
            stage_totals(&sets, vk::ShaderStageFlags::VERTEX).samplers,
            2
        );
        let limits = StageTotals {
            samplers: 4,
            sampled_images: 5,
            storage_buffers: 0,
        };
        assert!(check("test", &sets, limits).is_ok());
        let tight = StageTotals {
            samplers: 4,
            sampled_images: 4,
            storage_buffers: 0,
        };
        assert!(matches!(
            check("test", &sets, tight),
            Err(RenderError::DescriptorLimit {
                stage: "fragment",
                kind: "sampled images",
                needed: 5,
                limit: 4,
                ..
            })
        ));
    }

    /// The scene layout (camera, instances + bones, material, lighting/world) and the VAT
    /// layout fit the Vulkan minimum, so it runs on every conformant device; the headroom (2) is what later
    /// set-3 bindings (R2b) may still add. Counted from the arrays the layouts use.
    #[test]
    fn the_scene_layout_fits_the_vulkan_minimum() {
        let material = crate::material::set_layout_bindings();
        let lighting = crate::lighting::set_layout_bindings();
        let scene = crate::passes::scene::buffer_set_layout_bindings();
        let sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 4] =
            [&scene.0, &scene.1, &material, &lighting];
        let minimum = StageTotals {
            samplers: VULKAN_MINIMUM_PER_STAGE,
            sampled_images: VULKAN_MINIMUM_PER_STAGE,
            storage_buffers: VULKAN_MINIMUM_STORAGE_BUFFERS,
        };
        assert!(check("scene", &sets, minimum).is_ok());
        let fragment = stage_totals(&sets, vk::ShaderStageFlags::FRAGMENT);
        // Material 4 + set 3: lighting 5, fog of war 1, terrain 3, R6's AO 1 (binding 10),
        // R2b's baked AO and contact shadows 2 (bindings 13, 14; the light list and grid are
        // storage buffers): exactly the Vulkan minimum of 16. R3b adds no sampler: the decals
        // are a third fragment storage buffer (binding 20; the minimum is 4).
        assert_eq!(fragment.samplers, 16);
        assert_eq!(fragment.sampled_images, 16);
        assert_eq!(fragment.storage_buffers, 3);
        assert_eq!(
            stage_totals(&sets, vk::ShaderStageFlags::VERTEX).storage_buffers,
            2
        );
        // The VAT layout (R4b) swaps set 2 for one vertex-stage VAT texture.
        let vat = crate::vat::set_layout_bindings();
        let vat_sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 4] =
            [&scene.0, &scene.1, &vat, &lighting];
        assert!(check("VAT", &vat_sets, minimum).is_ok());
        assert_eq!(
            stage_totals(&vat_sets, vk::ShaderStageFlags::FRAGMENT).samplers,
            12
        );
        assert_eq!(
            stage_totals(&vat_sets, vk::ShaderStageFlags::VERTEX).samplers,
            1
        );
        // R3b: the foliage layout swaps set 2 for its block and instance buffer, the water
        // layout for its block and (full water) the refraction copy and the scene depth. Both
        // keep set 3 (lighting 5, fog of war 1, terrain 3, AO 1, R2b's baked AO and contact
        // shadows 2): 12 and 14 samplers of 16.
        let foliage = crate::passes::environment::foliage_set_bindings();
        let foliage_sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 4] =
            [&scene.0, &scene.1, &foliage, &lighting];
        assert!(check("foliage", &foliage_sets, minimum).is_ok());
        let foliage_vertex = stage_totals(&foliage_sets, vk::ShaderStageFlags::VERTEX);
        assert_eq!(foliage_vertex.storage_buffers, 3);
        assert_eq!(
            stage_totals(&foliage_sets, vk::ShaderStageFlags::FRAGMENT).samplers,
            12
        );
        let water = crate::passes::environment::water_set_bindings();
        let water_sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 4] =
            [&scene.0, &scene.1, &water, &lighting];
        assert!(check("water", &water_sets, minimum).is_ok());
        assert_eq!(
            stage_totals(&water_sets, vk::ShaderStageFlags::FRAGMENT).samplers,
            14
        );
        // ADR-0019: the heat haze has its own layout (the VFX frame set and the copy, depth
        // and unit-ID samplers), so the scene's 16 fragment samplers are unchanged.
        let vfx_frame = crate::passes::vfx::frame_set_bindings();
        let distortion = crate::passes::vfx::distortion_set_bindings();
        let distortion_sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 2] = [&vfx_frame, &distortion];
        assert!(check("distortion", &distortion_sets, minimum).is_ok());
        let distortion_fragment = stage_totals(&distortion_sets, vk::ShaderStageFlags::FRAGMENT);
        assert_eq!(distortion_fragment.samplers, 3);
        assert_eq!(distortion_fragment.storage_buffers, 0);
        assert_eq!(
            stage_totals(&distortion_sets, vk::ShaderStageFlags::VERTEX).storage_buffers,
            2
        );
    }
}
