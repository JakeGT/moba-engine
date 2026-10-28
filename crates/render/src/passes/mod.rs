//! Render-graph pass implementations. Each pass owns its pipelines, descriptor sets and
//! per-frame buffers; the graph (`crate::frame_graph`) owns ordering and barriers.

pub(crate) mod capture;
pub(crate) mod composite;
pub(crate) mod overlay;
pub(crate) mod scene;

use ash::vk;

use crate::device::DeviceContext;

/// Sets the dynamic viewport and a full-extent scissor.
pub(crate) fn set_viewport_scissor(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    viewport: vk::Viewport,
    extent: vk::Extent2D,
) {
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent,
    };
    // SAFETY: the command buffer is recording inside a rendering scope and the viewport is
    // valid for this extent.
    unsafe {
        device
            .device
            .cmd_set_viewport(command_buffer, 0, &[viewport]);
    }
    // SAFETY: same as above; the scissor covers exactly the render area.
    unsafe {
        device.device.cmd_set_scissor(command_buffer, 0, &[scissor]);
    }
}

/// A viewport covering `extent` with the conventional top-left origin.
pub(crate) fn full_viewport(extent: vk::Extent2D) -> vk::Viewport {
    vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.width as f32,
        height: extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

/// A viewport with negative height: NDC +Y is the top of the image. The scene (world +Y up)
/// and the UI shaders (`ndc.y = 1 - y / height * 2`) are written for it.
pub(crate) fn flipped_viewport(extent: vk::Extent2D) -> vk::Viewport {
    vk::Viewport {
        x: 0.0,
        y: extent.height as f32,
        width: extent.width as f32,
        height: -(extent.height as f32),
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

/// Begins dynamic rendering into one colour attachment with no depth.
pub(crate) fn begin_color_only(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    view: vk::ImageView,
    extent: vk::Extent2D,
    load_op: vk::AttachmentLoadOp,
    viewport: vk::Viewport,
) {
    let attachment = vk::RenderingAttachmentInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(load_op)
        .store_op(vk::AttachmentStoreOp::STORE);
    let render_info = vk::RenderingInfo::default()
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        })
        .layer_count(1)
        .color_attachments(core::slice::from_ref(&attachment));
    device.cmd_begin_rendering(command_buffer, &render_info);
    set_viewport_scissor(device, command_buffer, viewport, extent);
}
