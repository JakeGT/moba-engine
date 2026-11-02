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
//!
//! Tiered layouts (ADR-0020 §3b) put the update-after-bind bindless set at index 0. A pipeline
//! layout with any update-after-bind set is bounded by the update-after-bind limits, counted
//! over every set in it, *and* its ordinary sets still count against the ordinary limits.
//! [`check_tiered`] checks the first for the images, samplers and resources the bindless set
//! uses, in every stage a binding is visible to (`ALL` expands to every stage, mesh and ray
//! tracing included); the N0 tiered layouts' only other set is the smoke's acceleration structure, which
//! neither limit family's image counts include. Buffer, input-attachment and
//! acceleration-structure limits (and the colour attachments `maxPerStageUpdateAfterBindResources`
//! also counts in the fragment stage) are left to the first tiered pass that binds them (N1).
//! The 16-sampler ceiling above applies to Compat layouts only.

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

/// A tiered device's update-after-bind descriptor limits (`VkPhysicalDeviceVulkan12Properties`,
/// ADR-0020 §3b). Per stage, they bound every descriptor of a pipeline layout that contains an
/// update-after-bind set; per set, the sum over all of its sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpdateAfterBindLimits {
    /// `maxPerStageDescriptorUpdateAfterBindSamplers`.
    pub(crate) per_stage_samplers: u32,
    /// `maxPerStageDescriptorUpdateAfterBindSampledImages`.
    pub(crate) per_stage_sampled_images: u32,
    /// `maxPerStageDescriptorUpdateAfterBindStorageImages`.
    pub(crate) per_stage_storage_images: u32,
    /// `maxPerStageUpdateAfterBindResources`.
    pub(crate) per_stage_resources: u32,
    /// `maxDescriptorSetUpdateAfterBindSamplers`.
    pub(crate) set_samplers: u32,
    /// `maxDescriptorSetUpdateAfterBindSampledImages`.
    pub(crate) set_sampled_images: u32,
    /// `maxDescriptorSetUpdateAfterBindStorageImages`.
    pub(crate) set_storage_images: u32,
}

/// What a tiered layout needs from [`UpdateAfterBindLimits`] for one stage (or, with every
/// stage, for the whole layout).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TieredTotals {
    pub(crate) samplers: u32,
    pub(crate) sampled_images: u32,
    pub(crate) storage_images: u32,
    /// Descriptors that count as resources (`maxPerStageResources`): every image, texel
    /// buffer, buffer and input attachment; samplers and acceleration structures do not.
    pub(crate) resources: u32,
}

/// [`TieredTotals`] over every binding of `sets` visible to `stage`.
pub(crate) fn tiered_totals(
    sets: &[&[vk::DescriptorSetLayoutBinding<'_>]],
    stage: vk::ShaderStageFlags,
) -> TieredTotals {
    let mut totals = TieredTotals::default();
    for binding in sets.iter().flat_map(|set| set.iter()) {
        if !binding.stage_flags.intersects(stage) {
            continue;
        }
        let count = binding.descriptor_count;
        match binding.descriptor_type {
            vk::DescriptorType::SAMPLER => totals.samplers += count,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER => {
                totals.samplers += count;
                totals.sampled_images += count;
                totals.resources += count;
            }
            vk::DescriptorType::SAMPLED_IMAGE | vk::DescriptorType::UNIFORM_TEXEL_BUFFER => {
                totals.sampled_images += count;
                totals.resources += count;
            }
            vk::DescriptorType::STORAGE_IMAGE | vk::DescriptorType::STORAGE_TEXEL_BUFFER => {
                totals.storage_images += count;
                totals.resources += count;
            }
            vk::DescriptorType::UNIFORM_BUFFER
            | vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC
            | vk::DescriptorType::STORAGE_BUFFER
            | vk::DescriptorType::STORAGE_BUFFER_DYNAMIC
            | vk::DescriptorType::INPUT_ATTACHMENT => totals.resources += count,
            _ => {}
        }
    }
    totals
}

/// Every single shader stage a tiered device may run, named for errors, in bit order.
const TIERED_STAGES: [(vk::ShaderStageFlags, &str); 14] = [
    (vk::ShaderStageFlags::VERTEX, "vertex"),
    (
        vk::ShaderStageFlags::TESSELLATION_CONTROL,
        "tessellation control",
    ),
    (
        vk::ShaderStageFlags::TESSELLATION_EVALUATION,
        "tessellation evaluation",
    ),
    (vk::ShaderStageFlags::GEOMETRY, "geometry"),
    (vk::ShaderStageFlags::FRAGMENT, "fragment"),
    (vk::ShaderStageFlags::COMPUTE, "compute"),
    (vk::ShaderStageFlags::TASK_EXT, "task"),
    (vk::ShaderStageFlags::MESH_EXT, "mesh"),
    (vk::ShaderStageFlags::RAYGEN_KHR, "ray generation"),
    (vk::ShaderStageFlags::ANY_HIT_KHR, "any hit"),
    (vk::ShaderStageFlags::CLOSEST_HIT_KHR, "closest hit"),
    (vk::ShaderStageFlags::MISS_KHR, "miss"),
    (vk::ShaderStageFlags::INTERSECTION_KHR, "intersection"),
    (vk::ShaderStageFlags::CALLABLE_KHR, "callable"),
];

/// The single stages any binding of `sets` is visible to, each once, in bit order. `ALL`
/// (every stage the device supports) expands to every stage in [`TIERED_STAGES`]; a bit that
/// list does not name (a stage from an extension the engine does not enable) is checked as
/// `"other"`.
fn tiered_stages(
    sets: &[&[vk::DescriptorSetLayoutBinding<'_>]],
) -> impl Iterator<Item = (vk::ShaderStageFlags, &'static str)> {
    let union = sets
        .iter()
        .flat_map(|set| set.iter())
        .fold(vk::ShaderStageFlags::empty(), |union, binding| {
            union | binding.stage_flags
        });
    let named = TIERED_STAGES
        .iter()
        .fold(vk::ShaderStageFlags::empty(), |all, &(stage, _)| {
            all | stage
        });
    let union = if union.contains(vk::ShaderStageFlags::ALL) {
        named
    } else {
        union
    };
    (0..u32::BITS)
        .map(|bit| vk::ShaderStageFlags::from_raw(1 << bit))
        .filter(move |&stage| union.contains(stage))
        .map(|stage| {
            let name = TIERED_STAGES
                .iter()
                .find(|&&(named, _)| named == stage)
                .map_or("other", |&(_, name)| name);
            (stage, name)
        })
}

/// Fails when a tiered pipeline layout made of `sets` (the bindless set first) needs more than
/// the device's update-after-bind limits allow, per stage or across all sets. Every stage a
/// binding is visible to is checked ([`tiered_stages`]: `ALL` means every stage, mesh and ray
/// tracing included). `layout` names it in the error.
pub(crate) fn check_tiered(
    layout: &'static str,
    sets: &[&[vk::DescriptorSetLayoutBinding<'_>]],
    limits: UpdateAfterBindLimits,
) -> RenderResult<()> {
    let fail = |stage: &'static str, kind: &'static str, needed: u32, limit: u32| {
        if needed > limit {
            Err(RenderError::DescriptorLimit {
                layout,
                stage,
                kind,
                needed,
                limit,
            })
        } else {
            Ok(())
        }
    };
    for (stage, name) in tiered_stages(sets) {
        let totals = tiered_totals(sets, stage);
        fail(
            name,
            "update-after-bind samplers",
            totals.samplers,
            limits.per_stage_samplers,
        )?;
        fail(
            name,
            "update-after-bind sampled images",
            totals.sampled_images,
            limits.per_stage_sampled_images,
        )?;
        fail(
            name,
            "update-after-bind storage images",
            totals.storage_images,
            limits.per_stage_storage_images,
        )?;
        fail(
            name,
            "update-after-bind resources",
            totals.resources,
            limits.per_stage_resources,
        )?;
    }
    let all = tiered_totals(sets, vk::ShaderStageFlags::ALL);
    fail(
        "all",
        "update-after-bind samplers per layout",
        all.samplers,
        limits.set_samplers,
    )?;
    fail(
        "all",
        "update-after-bind sampled images per layout",
        all.sampled_images,
        limits.set_sampled_images,
    )?;
    fail(
        "all",
        "update-after-bind storage images per layout",
        all.storage_images,
        limits.set_storage_images,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        StageTotals, TieredTotals, UpdateAfterBindLimits, check, check_tiered, stage_totals,
        tiered_totals,
    };
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

    /// The ADR-0020 §3b minimums the admission gate checks, as limits: a device admitted to
    /// Tiered has at least these.
    fn admission_minimums() -> UpdateAfterBindLimits {
        let limit = |name: &str| {
            let value = crate::caps::REQUIRED_LIMITS
                .iter()
                .find(|(limit, _)| *limit == name)
                .map(|(_, value)| *value)
                .expect("limit is checked at admission");
            u32::try_from(value).expect("fits u32")
        };
        UpdateAfterBindLimits {
            per_stage_samplers: limit("maxPerStageDescriptorUpdateAfterBindSamplers"),
            per_stage_sampled_images: limit("maxPerStageDescriptorUpdateAfterBindSampledImages"),
            per_stage_storage_images: limit("maxPerStageDescriptorUpdateAfterBindStorageImages"),
            per_stage_resources: limit("maxPerStageUpdateAfterBindResources"),
            // Not checked at admission: the spec minimum when the features are supported is
            // 500,000 (ADR-0020 [S11]), far above the table.
            set_samplers: 500_000,
            set_sampled_images: limit("maxDescriptorSetUpdateAfterBindSampledImages"),
            set_storage_images: 500_000,
        }
    }

    /// The bindless set alone, and with the ray-query smoke's acceleration-structure set, fits
    /// every device the gate admits to Tiered (ADR-0020 §3b).
    #[test]
    fn the_bindless_layout_fits_the_admission_minimums() {
        let bindless = crate::bindless::set_layout_bindings();
        let sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 1] = [&bindless];
        assert!(check_tiered("bindless", &sets, admission_minimums()).is_ok());
        let compute = tiered_totals(&sets, vk::ShaderStageFlags::COMPUTE);
        assert_eq!(
            compute,
            TieredTotals {
                samplers: 32,
                sampled_images: 16_384,
                storage_images: 1_024,
                resources: 17_408,
            }
        );
        let rt = crate::smoke::rt::scene_set_bindings();
        let rt_sets: [&[vk::DescriptorSetLayoutBinding<'_>]; 2] = [&bindless, &rt];
        assert!(check_tiered("rt_smoke", &rt_sets, admission_minimums()).is_ok());
        // One texture too many for the gate's minimum fails with a named error.
        let mut tight = admission_minimums();
        tight.per_stage_sampled_images = 16_383;
        assert!(matches!(
            check_tiered("bindless", &sets, tight),
            Err(RenderError::DescriptorLimit {
                layout: "bindless",
                stage: "vertex",
                kind: "update-after-bind sampled images",
                needed: 16_384,
                limit: 16_383,
            })
        ));
        let mut set_tight = admission_minimums();
        set_tight.set_storage_images = 1_000;
        assert!(matches!(
            check_tiered("bindless", &sets, set_tight),
            Err(RenderError::DescriptorLimit {
                stage: "all",
                needed: 1_024,
                limit: 1_000,
                ..
            })
        ));
    }

    /// Every stage a binding is visible to is checked, not only vertex, fragment and compute:
    /// `ALL` covers mesh and ray-tracing stages, `ALL_GRAPHICS` covers tessellation and
    /// geometry but not compute, and a mesh-only binding is checked in the mesh stage.
    #[test]
    fn check_tiered_counts_every_stage_a_binding_reaches() {
        let all_graphics = super::tiered_stages(&[&[binding(
            vk::DescriptorType::SAMPLED_IMAGE,
            1,
            vk::ShaderStageFlags::ALL_GRAPHICS,
        )]])
        .map(|(_, name)| name)
        .collect::<Vec<_>>();
        assert_eq!(
            all_graphics,
            [
                "vertex",
                "tessellation control",
                "tessellation evaluation",
                "geometry",
                "fragment"
            ]
        );
        let all = super::tiered_stages(&[&crate::bindless::set_layout_bindings()]).count();
        assert_eq!(
            all,
            super::TIERED_STAGES.len(),
            "ALL expands to every named stage"
        );

        let mut limits = admission_minimums();
        limits.per_stage_storage_images = 4;
        let mesh_only = [binding(
            vk::DescriptorType::STORAGE_IMAGE,
            5,
            vk::ShaderStageFlags::MESH_EXT,
        )];
        assert!(matches!(
            check_tiered("mesh", &[&mesh_only], limits),
            Err(RenderError::DescriptorLimit {
                stage: "mesh",
                kind: "update-after-bind storage images",
                needed: 5,
                limit: 4,
                ..
            })
        ));
        // Split across a ray-generation and a compute binding, each stage needs 3 of 4.
        let split = [
            binding(
                vk::DescriptorType::STORAGE_IMAGE,
                3,
                vk::ShaderStageFlags::RAYGEN_KHR,
            ),
            binding(
                vk::DescriptorType::STORAGE_IMAGE,
                3,
                vk::ShaderStageFlags::COMPUTE,
            ),
        ];
        assert!(check_tiered("split", &[&split], limits).is_ok());
        // An `ALL` binding over the limit fails in the first stage, vertex.
        let everywhere = [binding(
            vk::DescriptorType::STORAGE_IMAGE,
            5,
            vk::ShaderStageFlags::ALL,
        )];
        assert!(matches!(
            check_tiered("all", &[&everywhere], limits),
            Err(RenderError::DescriptorLimit {
                stage: "vertex",
                ..
            })
        ));
        // A stage bit no list entry names is still checked.
        let unnamed = [binding(
            vk::DescriptorType::STORAGE_IMAGE,
            5,
            vk::ShaderStageFlags::from_raw(1 << 30),
        )];
        assert!(matches!(
            check_tiered("unnamed", &[&unnamed], limits),
            Err(RenderError::DescriptorLimit { stage: "other", .. })
        ));
    }
}
