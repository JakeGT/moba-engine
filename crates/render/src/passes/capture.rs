//! Capture pass: copies the finished swapchain image into the readback buffer.

use ash::vk;

use crate::device::DeviceContext;

/// Copies `image` (in `TRANSFER_SRC_OPTIMAL`, tightly packed 4-byte texels) into `buffer`.
/// The caller checked that `buffer` holds `extent.width * extent.height * 4` bytes.
pub(crate) fn record(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    extent: vk::Extent2D,
    buffer: vk::Buffer,
) {
    let region = vk::BufferImageCopy::default()
        .buffer_offset(0)
        .buffer_row_length(0)
        .buffer_image_height(0)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1),
        )
        .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
        .image_extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        });
    // SAFETY: the graph transitioned the image to TRANSFER_SRC_OPTIMAL and the buffer to a
    // transfer destination; the buffer is large enough (caller contract) and was created
    // with TRANSFER_DST usage.
    unsafe {
        device.device.cmd_copy_image_to_buffer(
            command_buffer,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            buffer,
            core::slice::from_ref(&region),
        );
    }
}
