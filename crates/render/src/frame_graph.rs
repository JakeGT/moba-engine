//! The engine's frame, declared as a render graph, and barrier recording.
//!
//! ```text
//! portraits  (R7b; frames that update them) hero models → portrait atlas (persistent)
//! minimap    (R7b; frames that bake it) top-down map → minimap image (persistent)
//! shadow     (R2a; when shadows are on) the main view's units + skinned, depth only →
//!            shadow map (fixed size)
//! scene      terrain + units + skinned → hdr (colour, cleared) + depth (sampled-capable),
//!            sampling the shadow map
//! composite  hdr (sampled) → exposure + AgX tonemap → swapchain
//! ui         HUD batch blended over the swapchain (not tonemapped); samples both
//!            offscreen images
//! capture    swapchain → readback buffer (only in the capture variant)
//! ```
//!
//! The offscreen images are imports owned by the renderer, not transients: their contents
//! survive the frame, they start and end every frame in `SHADER_READ_ONLY_OPTIMAL`, and their
//! passes appear only in the variants for frames that re-render them. Every combination
//! (capture × which offscreen passes run) is compiled once at renderer creation, and all of
//! them share the same transient slots. Per frame the renderer picks a variant, fills a
//! fixed-size [`Bindings`] table with this frame's handles and records the precompiled
//! barriers, so nothing here allocates on the frame path.
//!
//! The offscreen passes never sample the shadow map: their views bind a lighting set whose
//! shadow binding is a persistent 1×1 default image, and their camera uniform turns shadows
//! off (ADR-0011), so the shadow transient is only read by the scene, after the shadow pass.

use core::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use crate::device::DeviceContext;
use crate::graph::{
    Access, Barrier, CompiledGraph, GraphBuilder, GraphError, ImageDesc, ImageSize, ImportState,
    MAX_BARRIERS, MAX_RESOURCES, ResourceId,
};
use crate::ui::UI_IMAGE_SLOTS;

/// Pass name of each offscreen image's pass, by [`UiImage`](crate::UiImage) slot (also its
/// GPU-timing name).
pub(crate) const OFFSCREEN_PASS_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits", "minimap"];
const OFFSCREEN_DEPTH_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits-depth", "minimap-depth"];
const OFFSCREEN_BONE_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits-bones", "minimap-bones"];
/// Number of offscreen-pass combinations (one bit per slot).
pub(crate) const OFFSCREEN_VARIANTS: usize = 1 << UI_IMAGE_SLOTS;

/// What a compiled pass records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassKind {
    Shadow,
    Scene,
    Composite,
    Ui,
    Capture,
    /// Renders the offscreen image of this [`UiImage`](crate::UiImage) slot.
    Offscreen(u8),
}

/// Resource ids of one offscreen image: colour (sampled by the UI), depth and its bones.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OffscreenResources {
    pub(crate) color: ResourceId,
    pub(crate) depth: ResourceId,
    pub(crate) bones: ResourceId,
}

/// Resource ids of the engine frame graph.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameResources {
    pub(crate) swapchain: ResourceId,
    pub(crate) hdr: ResourceId,
    pub(crate) depth: ResourceId,
    pub(crate) bones: ResourceId,
    pub(crate) capture: Option<ResourceId>,
    pub(crate) offscreen: [OffscreenResources; UI_IMAGE_SLOTS],
    /// The directional shadow map; `None` when shadows are off.
    pub(crate) shadow: Option<ResourceId>,
}

/// The shadow map a frame renders, when shadows are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShadowSpec {
    pub(crate) format: vk::Format,
    pub(crate) size: u32,
}

/// A compiled frame variant and its resource ids.
pub(crate) struct FramePlan {
    pub(crate) graph: CompiledGraph<PassKind>,
    pub(crate) resources: FrameResources,
    /// Index of the first pass that touches the swapchain image. Passes before it go in a
    /// first batch that does not wait on the acquire semaphore; the rest wait on it.
    pub(crate) acquire_split: usize,
}

/// Builds and compiles the frame graph. Bit `s` of `offscreen` adds the pass that renders
/// offscreen image `s` this frame; `shadow` adds the shadow pass and map (every variant of
/// one renderer passes the same `shadow`, so they share transients).
///
/// # Errors
/// Only if the declaration below is inconsistent (covered by unit tests).
#[allow(clippy::too_many_lines)] // One declaration of the whole frame.
pub(crate) fn build(
    hdr_format: vk::Format,
    depth_format: vk::Format,
    depth_aspect: vk::ImageAspectFlags,
    shadow: Option<ShadowSpec>,
    capture: bool,
    offscreen: usize,
) -> Result<FramePlan, GraphError> {
    let mut graph = GraphBuilder::new();
    // The acquire semaphore is waited on at COLOR_ATTACHMENT_OUTPUT; the composite writes
    // every pixel, so the previous contents are discarded.
    let swapchain = graph.import_image(
        "swapchain",
        ImportState::discard(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT),
        Some(Access::Present),
    );
    // The frame fence covers write-after-read against the previous use of this frame's
    // bone buffer; the host writes it before submission.
    let bones = graph.import_buffer("bones", ImportState::host_written(), None);
    let hdr = graph.transient_image(
        "hdr",
        ImageDesc {
            format: hdr_format,
            aspect: vk::ImageAspectFlags::COLOR,
            size: ImageSize::Swapchain,
            extra_usage: vk::ImageUsageFlags::empty(),
        },
    );
    // Sampled-capable so later passes (soft particles, decals, SSAO) can read it.
    let depth = graph.transient_image(
        "depth",
        ImageDesc {
            format: depth_format,
            aspect: depth_aspect,
            size: ImageSize::Swapchain,
            extra_usage: vk::ImageUsageFlags::SAMPLED,
        },
    );
    // Depth only and fixed-size, so it never aliases the swapchain-sized targets. Written by
    // the shadow pass, sampled (depth compare) by the scene's fragment shaders.
    let shadow_map = shadow.map(|spec| {
        graph.transient_image(
            "shadow-map",
            ImageDesc {
                format: spec.format,
                aspect: vk::ImageAspectFlags::DEPTH,
                size: ImageSize::Fixed {
                    width: spec.size,
                    height: spec.size,
                },
                extra_usage: vk::ImageUsageFlags::empty(),
            },
        )
    });
    let capture_buffer = capture.then(|| {
        // Every capture waits for the device to idle before reading the buffer back, so no
        // earlier submission still touches it when the next capture frame starts.
        graph.import_buffer("capture", ImportState::idle(), Some(Access::HostRead))
    });
    // R7b offscreen images. Colour: kept across frames, sampled by the previous frame's UI,
    // and back in SHADER_READ_ONLY_OPTIMAL at the end of every frame (the UI read below
    // transitions it after a write). Depth: discarded per use, but last written by an
    // earlier frame's pass. Bones: written by the host, like the scene's.
    let offscreen_resources: [OffscreenResources; UI_IMAGE_SLOTS] =
        core::array::from_fn(|slot| OffscreenResources {
            color: graph.import_image(
                OFFSCREEN_PASS_NAMES[slot],
                ImportState::persistent(
                    vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
                Some(Access::SampledFragment),
            ),
            depth: graph.import_image(
                OFFSCREEN_DEPTH_NAMES[slot],
                ImportState::rewritten(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
                None,
            ),
            bones: graph.import_buffer(
                OFFSCREEN_BONE_NAMES[slot],
                ImportState::host_written(),
                None,
            ),
        });

    // Offscreen passes first: they do not touch the swapchain, so they go in the batch that
    // is submitted ahead of the acquire wait, with the scene.
    for (slot, resources) in offscreen_resources.iter().enumerate() {
        if offscreen & (1 << slot) == 0 {
            continue;
        }
        let _ = graph
            .pass(
                OFFSCREEN_PASS_NAMES[slot],
                PassKind::Offscreen(u8::try_from(slot).unwrap_or(u8::MAX)),
            )
            .access(resources.color, Access::ColorWrite)
            .access(resources.depth, Access::DepthWrite)
            .access(resources.bones, Access::VertexStorageRead);
    }
    if let Some(shadow_map) = shadow_map {
        let _ = graph
            .pass("shadow", PassKind::Shadow)
            .access(shadow_map, Access::DepthWrite)
            .access(bones, Access::VertexStorageRead);
    }
    let scene = graph
        .pass("scene", PassKind::Scene)
        .access(hdr, Access::ColorWrite)
        .access(depth, Access::DepthWrite)
        .access(bones, Access::VertexStorageRead);
    if let Some(shadow_map) = shadow_map {
        let _ = scene.access(shadow_map, Access::SampledFragment);
    }
    let _ = graph
        .pass("composite", PassKind::Composite)
        .access(hdr, Access::SampledFragment)
        .access(swapchain, Access::ColorWrite);
    // The UI samples every offscreen image in every variant (its descriptors always name
    // them), so each read is ordered after this frame's write, if any.
    let mut ui = graph
        .pass("ui", PassKind::Ui)
        .access(swapchain, Access::ColorReadWrite);
    for resources in &offscreen_resources {
        ui = ui.access(resources.color, Access::SampledFragment);
    }
    let _ = ui;
    if let Some(buffer) = capture_buffer {
        let _ = graph
            .pass("capture", PassKind::Capture)
            .access(swapchain, Access::TransferRead)
            .access(buffer, Access::TransferWrite);
    }
    let graph = graph.compile()?;
    let acquire_split = graph.first_use(swapchain);
    Ok(FramePlan {
        graph,
        acquire_split,
        resources: FrameResources {
            swapchain,
            hdr,
            depth,
            bones,
            capture: capture_buffer,
            offscreen: offscreen_resources,
            shadow: shadow_map,
        },
    })
}

/// The handle a resource resolves to in one frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum Binding {
    #[default]
    Unbound,
    Image {
        image: vk::Image,
        aspect: vk::ImageAspectFlags,
    },
    Buffer(vk::Buffer),
}

/// This frame's handles, indexed by [`ResourceId`].
pub(crate) struct Bindings(pub(crate) [Binding; MAX_RESOURCES]);

impl Bindings {
    pub(crate) fn new() -> Self {
        Self([Binding::Unbound; MAX_RESOURCES])
    }

    pub(crate) fn set(&mut self, resource: ResourceId, binding: Binding) {
        self.0[resource.index()] = binding;
    }
}

static BINDING_MISMATCH_LOGGED: AtomicBool = AtomicBool::new(false);

/// Records `barriers` as one `vkCmdPipelineBarrier2`, without allocating.
pub(crate) fn record_barriers(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    barriers: &[Barrier],
    bindings: &Bindings,
) {
    if barriers.is_empty() {
        return;
    }
    let mut images = [vk::ImageMemoryBarrier2::default(); MAX_BARRIERS];
    let mut buffers = [vk::BufferMemoryBarrier2::default(); MAX_BARRIERS];
    let (mut image_count, mut buffer_count) = (0, 0);
    // `compile` bounds barriers per pass by MAX_BARRIERS.
    for barrier in barriers.iter().take(MAX_BARRIERS) {
        match bindings.0[barrier.resource.index()] {
            Binding::Image { image, aspect } if barrier.image => {
                images[image_count] = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(barrier.src_stage)
                    .src_access_mask(barrier.src_access)
                    .dst_stage_mask(barrier.dst_stage)
                    .dst_access_mask(barrier.dst_access)
                    .old_layout(barrier.old_layout)
                    .new_layout(barrier.new_layout)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(aspect)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    );
                image_count += 1;
            }
            Binding::Buffer(buffer) if !barrier.image => {
                buffers[buffer_count] = vk::BufferMemoryBarrier2::default()
                    .src_stage_mask(barrier.src_stage)
                    .src_access_mask(barrier.src_access)
                    .dst_stage_mask(barrier.dst_stage)
                    .dst_access_mask(barrier.dst_access)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(buffer)
                    .offset(0)
                    .size(vk::WHOLE_SIZE);
                buffer_count += 1;
            }
            binding => {
                // A renderer bug: the barrier cannot be recorded. Fail loudly in debug builds
                // and log once in release (validation then reports the missing sync).
                debug_assert!(
                    false,
                    "resource {:?} bound as {binding:?} for barrier {barrier:?}",
                    barrier.resource
                );
                if !BINDING_MISMATCH_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[render] graph barrier dropped: resource {:?} bound as {binding:?}",
                        barrier.resource
                    );
                }
            }
        }
    }
    let dependency = vk::DependencyInfo::default()
        .image_memory_barriers(&images[..image_count])
        .buffer_memory_barriers(&buffers[..buffer_count]);
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

#[cfg(test)]
mod tests {
    use super::{OFFSCREEN_VARIANTS, PassKind, ShadowSpec, build};
    use ash::vk;
    use vk::ImageLayout as L;

    fn variant(capture: bool, offscreen: usize) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            None,
            capture,
            offscreen,
        )
        .expect("the engine frame graph compiles")
    }

    fn shadowed(capture: bool, offscreen: usize) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            Some(ShadowSpec {
                format: vk::Format::D32_SFLOAT,
                size: 2048,
            }),
            capture,
            offscreen,
        )
        .expect("the shadowed frame graph compiles")
    }

    #[test]
    fn shadow_pass_writes_depth_then_the_scene_samples_it() {
        let plain = shadowed(false, 0);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = shadowed(capture, offscreen);
                // Every variant (capture x offscreen) runs on the same transients.
                assert!(frame.graph.shares_transients_with(&plain.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let count = offscreen.count_ones() as usize;
                let passes = frame.graph.passes();
                assert_eq!(passes[count].tag, PassKind::Shadow);
                assert_eq!(passes[count + 1].tag, PassKind::Scene);
                // Offscreen, shadow and scene all go ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 2);
                let shadow = frame.resources.shadow.expect("shadow map");
                // Only the shadow and scene passes touch the map (never an offscreen view).
                for (index, pass) in passes.iter().enumerate() {
                    assert_eq!(pass.uses(shadow), index == count || index == count + 1);
                }
                assert!(passes[count].barriers.iter().any(|barrier| {
                    barrier.resource == shadow
                        && barrier.new_layout == L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
                }));
                // Scene: depth write -> fragment sampled read with the layout change; the
                // bone buffer needs no second barrier.
                let scene = &passes[count + 1].barriers;
                let read = scene
                    .iter()
                    .find(|barrier| barrier.resource == shadow)
                    .expect("shadow barrier before the scene");
                assert_eq!(
                    (read.old_layout, read.new_layout),
                    (
                        L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                        L::SHADER_READ_ONLY_OPTIMAL
                    )
                );
                assert_eq!(
                    read.src_access,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                );
                assert_eq!(read.dst_stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
                assert_eq!(read.dst_access, vk::AccessFlags2::SHADER_SAMPLED_READ);
                assert!(
                    !scene
                        .iter()
                        .any(|barrier| barrier.resource == frame.resources.bones)
                );
            }
        }
        // Its own slot: a fixed size never aliases the swapchain-sized depth.
        assert_eq!(plain.graph.slots().len(), 3);
        let slot = plain.graph.slots()[plain
            .graph
            .slot_of(plain.resources.shadow.unwrap())
            .unwrap()];
        assert!(slot.usage.contains(
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
        ));
    }

    fn plan(capture: bool) -> super::FramePlan {
        variant(capture, 0)
    }

    #[test]
    fn engine_frame_runs_scene_composite_ui_then_presents() {
        let frame = plan(false);
        let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
        assert_eq!(kinds, [PassKind::Scene, PassKind::Composite, PassKind::Ui]);
        // The scene is submitted ahead of the acquire wait; the composite waits for it.
        assert_eq!(frame.acquire_split, 1);
        assert_eq!(plan(true).acquire_split, 1);
        assert_eq!(frame.graph.culled(), [] as [&str; 0]);
        // HDR colour and depth overlap in the scene pass, so they get separate slots.
        assert_eq!(frame.graph.slots().len(), 2);
        let depth = frame.graph.slots()[frame.graph.slot_of(frame.resources.depth).unwrap()];
        assert!(depth.usage.contains(vk::ImageUsageFlags::SAMPLED));
        let scene = &frame.graph.passes()[0].barriers;
        assert_eq!(
            scene.len(),
            3,
            "hdr, depth and the host-written bone buffer"
        );
        let composite = &frame.graph.passes()[1].barriers;
        assert!(composite.iter().any(|barrier| {
            barrier.resource == frame.resources.hdr
                && barrier.new_layout == L::SHADER_READ_ONLY_OPTIMAL
        }));
        let present = frame.graph.final_barriers();
        assert_eq!(present.len(), 1);
        assert_eq!(present[0].resource, frame.resources.swapchain);
        assert_eq!(
            (present[0].old_layout, present[0].new_layout),
            (L::COLOR_ATTACHMENT_OPTIMAL, L::PRESENT_SRC_KHR)
        );
    }

    #[test]
    fn capture_variant_copies_after_ui_and_shares_transient_slots() {
        let normal = plan(false);
        let capture = plan(true);
        assert!(capture.graph.shares_transients_with(&normal.graph));
        let last = capture.graph.passes().last().expect("capture pass");
        assert_eq!(last.tag, PassKind::Capture);
        assert_eq!(last.barriers[0].new_layout, L::TRANSFER_SRC_OPTIMAL);
        let finals = capture.graph.final_barriers();
        assert_eq!(finals.len(), 2, "present + host read of the capture buffer");
        assert!(finals.iter().any(|barrier| {
            barrier.old_layout == L::TRANSFER_SRC_OPTIMAL
                && barrier.new_layout == L::PRESENT_SRC_KHR
                && barrier.src_access == vk::AccessFlags2::NONE
        }));
        assert!(finals.iter().any(|barrier| {
            !barrier.image
                && barrier.src_access == vk::AccessFlags2::TRANSFER_WRITE
                && barrier.dst_access == vk::AccessFlags2::HOST_READ
        }));
    }

    #[test]
    fn offscreen_passes_render_first_and_the_ui_samples_their_images() {
        let normal = plan(false);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = variant(capture, offscreen);
                // Every combination runs on the same transients as the plain frame.
                assert!(frame.graph.shares_transients_with(&normal.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let passes = frame.graph.passes();
                let count = offscreen.count_ones() as usize;
                assert!(
                    passes[..count]
                        .iter()
                        .all(|pass| matches!(pass.tag, PassKind::Offscreen(_)))
                );
                assert_eq!(passes[count].tag, PassKind::Scene);
                // Offscreen passes join the scene's batch, ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 1);
                let ui = passes
                    .iter()
                    .find(|pass| pass.tag == PassKind::Ui)
                    .expect("ui pass");
                for (slot, resources) in frame.resources.offscreen.iter().enumerate() {
                    assert!(ui.uses(resources.color));
                    let read = ui
                        .barriers
                        .iter()
                        .find(|barrier| barrier.resource == resources.color)
                        .expect("ui read barrier");
                    assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
                    assert_eq!(read.dst_access, vk::AccessFlags2::SHADER_SAMPLED_READ);
                    if offscreen & (1 << slot) != 0 {
                        assert_eq!(read.old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
                        assert_eq!(read.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
                        let tag = PassKind::Offscreen(u8::try_from(slot).expect("slot"));
                        let pass = passes
                            .iter()
                            .find(|pass| pass.tag == tag)
                            .expect("offscreen pass");
                        assert_eq!(pass.barriers.len(), 3, "colour, depth and bones");
                        assert!(pass.barriers.iter().any(|barrier| {
                            barrier.resource == resources.color
                                && barrier.old_layout == L::SHADER_READ_ONLY_OPTIMAL
                                && barrier.new_layout == L::COLOR_ATTACHMENT_OPTIMAL
                        }));
                    } else {
                        assert_eq!(read.old_layout, L::SHADER_READ_ONLY_OPTIMAL);
                        assert_eq!(read.src_access, vk::AccessFlags2::NONE);
                    }
                }
                // Each image is back in SHADER_READ_ONLY_OPTIMAL after the UI: only the
                // swapchain (and the capture buffer) need final barriers.
                assert_eq!(frame.graph.final_barriers().len(), 1 + usize::from(capture));
            }
        }
    }
}
