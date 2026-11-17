//! `gbuffer_decals` (N2c, ADR-0023 §7a): the `Paint` decals as a raster pass between the
//! G-buffer pass's two groups, on the deferred Tiered frame.
//!
//! The receivers (`prepass`: the terrain) write depth and the G-buffer; this pass draws one box
//! per decal of the frame's decal buffer (`decal_gbuffer.vert`, instanced, highest index first)
//! and blends the decal's colour into `gb0` through its sRGB view (`decal_gbuffer.frag`), so the
//! blend is linear, as the forward terrain's `apply_paint_decals`; the rest (`gbuffer_rest`:
//! units, walls, props, minions, foliage, sky) is drawn after it and overwrites it. The depth
//! is attached read-only (`DEPTH_STENCIL_READ_ONLY_OPTIMAL`, §7a) and sampled in the same
//! layout: the boxes draw their back faces only (front faces culled, so a camera inside a box
//! still draws it), depth-tested so a back face in front of the ground (a box wholly above or
//! beside it, or the sky) is rejected before shading, and the fragment rebuilds the ground
//! from the sampled depth. Glow decals' boxes collapse in the vertex stage.
//!
//! Set 0 is the scene's camera set; set 1 is this pass's: binding 0 the depth (sampled image),
//! binding 1 the frame's decal buffer (the lighting resources', as the overlay binds it).

use ash::vk;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::pipeline::{self, ColorTarget, TargetBlend, TargetsSpec};
use crate::resources::{DescriptorPool, DescriptorSetLayout, GraphicsPipeline, PipelineLayout};
use crate::shaders::ShaderSource;

/// The pass's pipeline, its set per frame in flight and the decal count to draw.
pub(crate) struct DecalGBufferPass {
    device: ash::Device,
    sets: Vec<vk::DescriptorSet>,
    _pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    pipeline_layout: PipelineLayout,
    pipeline: GraphicsPipeline,
}

/// What [`DecalGBufferPass::record`] draws into.
pub(crate) struct DecalGBufferTarget {
    /// `gb0`'s sRGB view, loaded and stored.
    pub(crate) gb0_srgb: vk::ImageView,
    /// The receivers' depth, attached read-only (also bound sampled, [`DecalGBufferPass::bind_depth`]).
    pub(crate) depth: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    /// The main view's camera set for this frame.
    pub(crate) camera: vk::DescriptorSet,
    /// Decals in this frame's buffer (paint and glow; the glow ones collapse in the vertex
    /// stage).
    pub(crate) decals: u32,
}

impl DecalGBufferPass {
    /// Creates the pass; `decals[f]` is frame slot `f`'s decal buffer (bound once: the
    /// buffers live as long as the renderer); `depth_format` is the main depth's.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        camera_layout: vk::DescriptorSetLayout,
        decals: &[vk::Buffer],
        depth_format: vk::Format,
    ) -> RenderResult<Self> {
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let frames = u32::try_from(decals.len()).expect("frames in flight fit u32");
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(frames),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(frames),
        ];
        let pool = DescriptorPool::new(device.device.clone(), &pool_sizes, frames)?;
        let layouts = vec![set_layout.handle(); decals.len()];
        let sets = pool.allocate(&device.device, &layouts)?;
        for (&set, &buffer) in sets.iter().zip(decals) {
            let info = [vk::DescriptorBufferInfo::default()
                .buffer(buffer)
                .offset(0)
                .range(vk::WHOLE_SIZE)];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&info);
            // SAFETY: the set was just allocated with this layout and the buffer outlives it
            // (the decal buffers belong to the renderer's lighting resources).
            unsafe {
                device
                    .device
                    .update_descriptor_sets(core::slice::from_ref(&write), &[]);
            }
        }
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[camera_layout, set_layout.handle()],
            &[],
        )?;
        let pipeline = build_pipeline(device, shaders, pipeline_layout.handle(), depth_format)?;
        Ok(Self {
            device: device.device.clone(),
            sets,
            _pool: pool,
            _set_layout: set_layout,
            pipeline_layout,
            pipeline,
        })
    }

    /// Points every frame's set at the depth `depth`. Only while no submitted frame uses the
    /// sets (creation, resize).
    pub(crate) fn bind_depth(&self, depth: vk::ImageView) {
        for &set in &self.sets {
            let info = [vk::DescriptorImageInfo::default()
                .image_view(depth)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&info);
            // SAFETY: the set and view are live and no pending command buffer uses the set
            // (see the method contract).
            unsafe {
                self.device
                    .update_descriptor_sets(core::slice::from_ref(&write), &[]);
            }
        }
    }

    /// Records the decals into `gb0` (loaded, blended, stored). Nothing is drawn without
    /// decals, but the pass still begins and ends its rendering, so the graph's layouts hold.
    pub(crate) fn record(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        target: &DecalGBufferTarget,
    ) {
        let color = vk::RenderingAttachmentInfo::default()
            .image_view(target.gb0_srgb)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE);
        let depth = vk::RenderingAttachmentInfo::default()
            .image_view(target.depth)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::NONE);
        let render_info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: target.extent,
            })
            .layer_count(1)
            .color_attachments(core::slice::from_ref(&color))
            .depth_attachment(&depth);
        device.cmd_begin_rendering(command_buffer, &render_info);
        if target.decals > 0 {
            super::set_viewport_scissor(
                device,
                command_buffer,
                super::flipped_viewport(target.extent),
                target.extent,
            );
            let sets = [target.camera, self.sets[frame % self.sets.len()]];
            // SAFETY: the pipeline is live and the command buffer is recording.
            unsafe {
                device.device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline.handle(),
                );
            }
            // SAFETY: both sets were allocated with the layout's set layouts and are live.
            unsafe {
                device.device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout.handle(),
                    0,
                    &sets,
                    &[],
                );
            }
            // SAFETY: the box is built from `gl_VertexIndex` and the decal records,
            // `gl_InstanceIndex` below the buffer's 128 records (the shader clamps it to the
            // frame's count).
            unsafe {
                device
                    .device
                    .cmd_draw(command_buffer, 36, target.decals.min(128), 0, 0);
            }
        }
        device.cmd_end_rendering(command_buffer);
    }
}

fn build_pipeline(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
    depth_format: vk::Format,
) -> RenderResult<GraphicsPipeline> {
    let vertex = shaders.module(device, "decal_gbuffer.vert")?;
    let fragment = shaders.module(device, "decal_gbuffer.frag")?;
    let colors = [ColorTarget {
        format: vk::Format::R8G8B8A8_SRGB,
        write_mask: vk::ColorComponentFlags::R
            | vk::ColorComponentFlags::G
            | vk::ColorComponentFlags::B,
        blend: TargetBlend::Alpha,
    }];
    pipeline::create_targets(
        device,
        &TargetsSpec {
            layout,
            vertex_layout: None,
            vertex: vertex.handle(),
            fragment: Some(fragment.handle()),
            fragment_specialization: None,
            colors: &colors,
            depth_format,
            depth_write: false,
            samples: vk::SampleCountFlags::TYPE_1,
            // Reversed-Z (the deferred frame is Tiered's): a back face at or behind the
            // ground passes; one in front of it, whose box cannot hold the ground, fails.
            depth_compare: vk::CompareOp::LESS_OR_EQUAL,
            cull_mode: vk::CullModeFlags::FRONT,
        },
    )
    .map(|handle| GraphicsPipeline::from_raw(device.device.clone(), handle))
}
