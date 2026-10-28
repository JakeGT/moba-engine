//! The engine's frame, declared as a render graph, and barrier recording.
//!
//! ```text
//! scene      terrain + units + skinned → hdr (colour, cleared) + depth (sampled-capable)
//! composite  hdr (sampled) → exposure + AgX tonemap → swapchain
//! ui         HUD batch blended over the swapchain (not tonemapped)
//! capture    swapchain → readback buffer (only in the capture variant)
//! ```
//!
//! Both variants are compiled once at renderer creation. Per frame the renderer fills a
//! fixed-size [`Bindings`] table with this frame's handles and records the precompiled
//! barriers, so nothing here allocates on the frame path.

use ash::vk;

use crate::device::DeviceContext;
use crate::graph::{
    Access, Barrier, CompiledGraph, GraphBuilder, GraphError, ImageDesc, ImageSize, ImportState,
    MAX_BARRIERS, MAX_RESOURCES, ResourceId,
};

/// What a compiled pass records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassKind {
    Scene,
    Composite,
    Ui,
    Capture,
}

/// Resource ids of the engine frame graph.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameResources {
    pub(crate) swapchain: ResourceId,
    pub(crate) hdr: ResourceId,
    pub(crate) depth: ResourceId,
    pub(crate) bones: ResourceId,
    pub(crate) capture: Option<ResourceId>,
}

/// A compiled frame variant and its resource ids.
pub(crate) struct FramePlan {
    pub(crate) graph: CompiledGraph<PassKind>,
    pub(crate) resources: FrameResources,
}

/// Builds and compiles the frame graph.
///
/// # Errors
/// Only if the declaration below is inconsistent (covered by unit tests).
pub(crate) fn build(
    hdr_format: vk::Format,
    depth_format: vk::Format,
    depth_aspect: vk::ImageAspectFlags,
    capture: bool,
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
    let capture_buffer = capture.then(|| {
        // Every capture waits for the device to idle before reading the buffer back, so no
        // earlier submission still touches it when the next capture frame starts.
        graph.import_buffer("capture", ImportState::idle(), Some(Access::HostRead))
    });

    let _ = graph
        .pass("scene", PassKind::Scene)
        .access(hdr, Access::ColorWrite)
        .access(depth, Access::DepthWrite)
        .access(bones, Access::VertexStorageRead);
    let _ = graph
        .pass("composite", PassKind::Composite)
        .access(hdr, Access::SampledFragment)
        .access(swapchain, Access::ColorWrite);
    let _ = graph
        .pass("ui", PassKind::Ui)
        .access(swapchain, Access::ColorReadWrite);
    if let Some(buffer) = capture_buffer {
        let _ = graph
            .pass("capture", PassKind::Capture)
            .access(swapchain, Access::TransferRead)
            .access(buffer, Access::TransferWrite);
    }
    Ok(FramePlan {
        graph: graph.compile()?,
        resources: FrameResources {
            swapchain,
            hdr,
            depth,
            bones,
            capture: capture_buffer,
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
            binding => debug_assert!(
                false,
                "resource {:?} bound as {binding:?} for barrier {barrier:?}",
                barrier.resource
            ),
        }
    }
    let dependency = vk::DependencyInfo::default()
        .image_memory_barriers(&images[..image_count])
        .buffer_memory_barriers(&buffers[..buffer_count]);
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

#[cfg(test)]
mod tests {
    use super::{PassKind, build};
    use ash::vk;
    use vk::ImageLayout as L;

    fn plan(capture: bool) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            capture,
        )
        .expect("the engine frame graph compiles")
    }

    #[test]
    fn engine_frame_runs_scene_composite_ui_then_presents() {
        let frame = plan(false);
        let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
        assert_eq!(kinds, [PassKind::Scene, PassKind::Composite, PassKind::Ui]);
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
        assert_eq!(capture.graph.slots(), normal.graph.slots());
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
}
