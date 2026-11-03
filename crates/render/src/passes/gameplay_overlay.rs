//! The post-upscale gameplay overlay (ADR-0021 §5): one fullscreen pass at output resolution
//! after the post chain, over the displayed LDR image into the swapchain. It draws the glow
//! decals (telegraphs, zones, range indicators) on the overlay prepass's `GROUND` pixels and
//! dilates the overlay IDs into the outlines, so neither ever enters a temporal history and
//! both have the same coverage whichever upscaler runs (`overlay.frag`).
//!
//! Set 0: binding 0 the [`OverlayBlock`] (per frame in flight), 1 the LDR image, 2 the
//! overlay depth, 3 the overlay IDs, 4 this frame's decal buffer (the lighting set's, read
//! with the same 32×32 ground bins and `decals.glsl` shape code as the scene shaders).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;
use moba_core::math::Mat4;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::pipeline;
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::scene::{OUTLINE_SLOTS, OutlinePalette};
use crate::shaders::ShaderSource;

/// `OverlayBlock` in `overlay.frag` (std140, 224 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct OverlayBlock {
    /// The unjittered output-resolution camera's inverse (clip → world).
    inv_view_proj: [[f32; 4]; 4],
    /// xy output size, zw its reciprocal.
    viewport: [f32; 4],
    /// x 1 = sRGB target, y the widest outline (output pixels), z debug view (1 = the
    /// overlay IDs), w 1 = decals on.
    params: [f32; 4],
    /// Outline slots 1..7: linear display colour and width in output pixels.
    palette: [[f32; 4]; OUTLINE_SLOTS],
}

impl OverlayBlock {
    /// The block for one frame. Widths are output pixels with no render-scale factor
    /// (ADR-0021 §5b).
    pub(crate) fn new(
        view_proj: Mat4,
        output: vk::Extent2D,
        srgb: bool,
        outlines: &OutlinePalette,
        debug_ids: bool,
        decals: bool,
    ) -> Self {
        let inverse = view_proj.inverse();
        let inverse = if inverse.is_finite() {
            inverse
        } else {
            Mat4::IDENTITY
        };
        let width = output.width.max(1) as f32;
        let height = output.height.max(1) as f32;
        let palette = outlines.sanitized();
        let widest = palette
            .iter()
            .skip(1)
            .map(|slot| slot[3])
            .fold(0.0_f32, f32::max);
        Self {
            inv_view_proj: inverse.to_cols_array_2d(),
            viewport: [width, height, 1.0 / width, 1.0 / height],
            params: [
                f32::from(u8::from(srgb)),
                widest,
                f32::from(u8::from(debug_ids)),
                f32::from(u8::from(decals)),
            ],
            palette,
        }
    }
}

const BLOCK_SIZE: u64 = core::mem::size_of::<OverlayBlock>() as u64;

/// The images the overlay reads (transients; re-bound on a resize).
#[derive(Clone, Copy, Debug)]
pub(crate) struct OverlayImages {
    pub(crate) ldr: vk::ImageView,
    pub(crate) depth: vk::ImageView,
    pub(crate) ids: vk::ImageView,
}

struct OverlayFrame {
    block: GpuBuffer,
    set: vk::DescriptorSet,
}

/// The overlay's pipeline, sets and per-frame blocks.
pub(crate) struct GameplayOverlay {
    frames: Vec<OverlayFrame>,
    _descriptor_pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    /// Nearest: every read is a `texelFetch`.
    sampler: Sampler,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
    swapchain_format: vk::Format,
}

impl GameplayOverlay {
    /// Creates the pass for `frames` frames in flight; `decals[f]` is frame `f`'s decal
    /// buffer (bound once: the buffers live as long as the renderer).
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        swapchain_format: vk::Format,
        decals: &[vk::Buffer],
    ) -> RenderResult<Self> {
        let fragment = vk::ShaderStageFlags::FRAGMENT;
        let binding = |binding, kind| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(kind)
                .descriptor_count(1)
                .stage_flags(fragment)
        };
        let sampled = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let bindings = [
            binding(0, vk::DescriptorType::UNIFORM_BUFFER),
            binding(1, sampled),
            binding(2, sampled),
            binding(3, sampled),
            binding(4, vk::DescriptorType::STORAGE_BUFFER),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let frames = u32::try_from(decals.len()).expect("frames in flight fit u32");
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(frames),
            vk::DescriptorPoolSize::default()
                .ty(sampled)
                .descriptor_count(3 * frames),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(frames),
        ];
        let descriptor_pool = DescriptorPool::new(device.device.clone(), &pool_sizes, frames)?;
        let layouts = vec![set_layout.handle(); decals.len()];
        let sets = descriptor_pool.allocate(&device.device, &layouts)?;
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let mut overlay_frames = Vec::with_capacity(decals.len());
        for (&set, &decal_buffer) in sets.iter().zip(decals) {
            let block = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "overlay-block",
                BLOCK_SIZE,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                MemoryLocation::CpuToGpu,
            )?;
            let block_info = [vk::DescriptorBufferInfo::default()
                .buffer(block.handle())
                .offset(0)
                .range(BLOCK_SIZE)];
            let decal_info = [vk::DescriptorBufferInfo::default()
                .buffer(decal_buffer)
                .offset(0)
                .range(vk::WHOLE_SIZE)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&block_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(4)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&decal_info),
            ];
            // SAFETY: the set was just allocated with this layout; both buffers are live and
            // outlive the set (the decal buffers belong to the renderer's lighting resources).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
            overlay_frames.push(OverlayFrame { block, set });
        }
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &[],
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle(), swapchain_format)?;
        Ok(Self {
            frames: overlay_frames,
            _descriptor_pool: descriptor_pool,
            _set_layout: set_layout,
            sampler,
            pipeline_layout,
            pipeline,
            swapchain_format,
        })
    }

    /// Builds a replacement pipeline (dev shader reload) without installing it.
    pub(crate) fn build_pipeline(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<GraphicsPipeline> {
        build_pipeline(
            device,
            shaders,
            self.pipeline_layout.handle(),
            self.swapchain_format,
        )
    }

    /// Installs a pipeline from [`Self::build_pipeline`]. The device must be idle.
    pub(crate) fn install_pipeline(&mut self, pipeline: GraphicsPipeline) {
        self.pipeline = pipeline;
    }

    /// Points every frame's set at `images`. Only while no submitted frame uses the sets
    /// (creation, resize).
    pub(crate) fn bind(&self, device: &DeviceContext, images: OverlayImages) {
        for frame in &self.frames {
            let info = |view| {
                [vk::DescriptorImageInfo::default()
                    .sampler(self.sampler.handle())
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
            };
            let (ldr, depth, ids) = (info(images.ldr), info(images.depth), info(images.ids));
            let writes = [
                image_write(frame.set, 1, &ldr),
                image_write(frame.set, 2, &depth),
                image_write(frame.set, 3, &ids),
            ];
            // SAFETY: the sets, sampler and views are live and no pending command buffer uses
            // the sets (see the method contract).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    /// Writes frame `frame`'s block. The frame's fence must have been waited on.
    pub(crate) fn prepare(&mut self, frame: usize, block: &OverlayBlock) {
        self.frames[frame].block.write_bytes(0, bytes_of(block));
    }

    /// Records the overlay into `target` (the whole `extent`; every pixel is written).
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: vk::ImageView,
        extent: vk::Extent2D,
    ) {
        let color = vk::RenderingAttachmentInfo::default()
            .image_view(target)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color));
        device.cmd_begin_rendering(command_buffer, &render_info);
        super::set_viewport_scissor(device, command_buffer, super::full_viewport(extent), extent);
        // SAFETY: the pipeline and set are live and match the layout; the graph moved the
        // three images to SHADER_READ_ONLY_OPTIMAL before this pass.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline.handle(),
            );
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[self.frames[frame].set],
                &[],
            );
            device.device.cmd_draw(command_buffer, 3, 1, 0, 0);
        }
        device.cmd_end_rendering(command_buffer);
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
    swapchain_format: vk::Format,
) -> RenderResult<GraphicsPipeline> {
    let vertex = shaders.module(device, "fullscreen.vert")?;
    let fragment = shaders.module(device, "overlay.frag")?;
    let handle = pipeline::create_fullscreen(
        device,
        layout,
        vertex.handle(),
        fragment.handle(),
        swapchain_format,
        pipeline::FullscreenBlend::None,
    )?;
    Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
}

/// A combined-image-sampler write of `info` to `binding` of `set`.
fn image_write<'a>(
    set: vk::DescriptorSet,
    binding: u32,
    info: &'a [vk::DescriptorImageInfo; 1],
) -> vk::WriteDescriptorSet<'a> {
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::Vec3;

    #[test]
    fn the_block_matches_the_shader_and_keeps_widths_in_output_pixels() {
        assert_eq!(core::mem::size_of::<OverlayBlock>(), 224);
        let palette = OutlinePalette::NONE
            .with_slot(1, Vec3::new(0.1, 0.2, 1.0), 1.5)
            .with_slot(3, Vec3::new(2.0, 0.5, 0.0), 3.0);
        let extent = vk::Extent2D {
            width: 2560,
            height: 1440,
        };
        let block = OverlayBlock::new(Mat4::IDENTITY, extent, true, &palette, false, true);
        assert_eq!(block.params, [1.0, 3.0, 0.0, 1.0]);
        assert_eq!(block.palette[1], [0.1, 0.2, 1.0, 1.5]);
        assert_eq!(block.palette[3], [1.0, 0.5, 0.0, 3.0], "colours clamped");
        assert_eq!(block.viewport[0], 2560.0);
        let singular = OverlayBlock::new(Mat4::ZERO, extent, false, &palette, true, false);
        assert_eq!(singular.inv_view_proj, Mat4::IDENTITY.to_cols_array_2d());
        assert_eq!(singular.params[2], 1.0);
    }
}
