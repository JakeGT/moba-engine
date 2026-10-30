//! The post-processing chain (R6, ADR-0015): GTAO and its blur, the bloom down/up chain,
//! auto exposure, the composite (bloom, exposure, `AgX`, grading LUT, screen effects), FXAA
//! and the upscale. Every pass is one fullscreen triangle with one descriptor set of four
//! combined image samplers and a push-constant block.
//!
//! Descriptor sets name graph transients, so they are written at creation and after a
//! resize's device-idle wait ([`PostPass::bind`]), plus on [`PostPass::set_grading_lut`]
//! (load time). Per frame the passes only push constants: nothing allocates.
//!
//! Dynamic resolution renders a sub-rectangle of every render-resolution image. Each pass
//! gets its destination viewport and, per source, the valid uv region and texel size
//! ([`Region`]), and the shaders clamp every tap inside it (`include/post.glsl`).

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use moba_core::math::{Mat4, Vec3, Vec4};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::frame_graph::{AO_FORMAT, AO_RAW_FORMAT, BLOOM_LEVELS, FrameSpec, LDR_FORMAT};
use crate::pipeline::{self, FullscreenBlend};
use crate::post::{PostConfig, PostLook, ScreenEffects, UpscaleFilter};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuImage, GraphicsPipeline, PipelineLayout, Sampler,
};
use crate::shaders::ShaderSource;
use crate::targets::shifted;
use crate::texture;

/// `AgX` look saturation (the "punchy" preset's 1.4 with identity slope/offset/power): keeps
/// team colours as saturated as the pre-tonemap image. The grading LUT goes on top (R6).
pub(crate) const TONEMAP_SATURATION: f32 = 1.4;
/// Push-constant range of every post pipeline (the guaranteed minimum).
const PUSH_SIZE: u32 = 128;
/// Combined image samplers per post descriptor set.
const SET_BINDINGS: usize = 4;
/// Mid-grey key auto exposure targets.
const EXPOSURE_KEY: f32 = 0.18;
/// GTAO probes per side of each slice.
const AO_STEPS: f32 = 4.0;
/// Occluders fade out from this fraction of the AO radius to the radius.
const AO_FALLOFF_START: f32 = 0.6;
/// The AO blur's depth weight (`exp(-sharpness * relative distance difference)`).
const AO_BLUR_SHARPNESS: f32 = 40.0;
/// Bloom up-sample tent radius in source texels.
const BLOOM_RADIUS: f32 = 1.0;

/// Which descriptor set a pass uses (index into [`PostPass::sets`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetIndex {
    Composite,
    BloomDown(usize),
    BloomUp(usize),
    Exposure,
    Fxaa,
    Upscale,
    Ao,
    AoBlur,
}

impl SetIndex {
    const COUNT: usize = 6 + BLOOM_LEVELS + (BLOOM_LEVELS - 1);

    const fn index(self) -> usize {
        match self {
            Self::Composite => 0,
            Self::Exposure => 1,
            Self::Fxaa => 2,
            Self::Upscale => 3,
            Self::Ao => 4,
            Self::AoBlur => 5,
            Self::BloomDown(level) => 6 + level,
            Self::BloomUp(level) => 6 + BLOOM_LEVELS + level,
        }
    }
}

/// The transient views the post sets name, from the frame plan (`None` when absent).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PostViews {
    pub(crate) hdr: Option<vk::ImageView>,
    pub(crate) depth: Option<vk::ImageView>,
    pub(crate) ao_raw: Option<vk::ImageView>,
    pub(crate) bloom: [Option<vk::ImageView>; BLOOM_LEVELS],
    pub(crate) ldr: Option<vk::ImageView>,
    pub(crate) ldr2: Option<vk::ImageView>,
}

/// One image's valid region this frame: its rendered sub-rectangle and its allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) actual: vk::Extent2D,
    pub(crate) alloc: vk::Extent2D,
}

impl Region {
    /// Mip `shift` of a render-resolution region (bloom, AO).
    pub(crate) fn shifted(self, shift: u8) -> Self {
        Self {
            actual: shifted(self.actual, shift),
            alloc: shifted(self.alloc, shift),
        }
    }

    /// The valid uv region (`actual / alloc`).
    fn scale(self) -> [f32; 2] {
        [
            self.actual.width as f32 / self.alloc.width.max(1) as f32,
            self.actual.height as f32 / self.alloc.height.max(1) as f32,
        ]
    }

    /// `1 / alloc`.
    fn texel(self) -> [f32; 2] {
        [
            1.0 / self.alloc.width.max(1) as f32,
            1.0 / self.alloc.height.max(1) as f32,
        ]
    }

    /// `1 / actual`, the destination mapping of a pass writing this region.
    fn inverse(self) -> [f32; 2] {
        [
            1.0 / self.actual.width.max(1) as f32,
            1.0 / self.actual.height.max(1) as f32,
        ]
    }

    /// Scene AO lookup values (`camera.post_params.yz`, `camera.post_clamp.xy`) for an AO
    /// image of this region: the AO uv per render pixel and the largest valid uv.
    pub(crate) fn ao_lookup(self) -> ([f32; 2], [f32; 2]) {
        let texel = self.texel();
        let scale = self.scale();
        (
            [0.5 * texel[0], 0.5 * texel[1]],
            [scale[0] - 0.5 * texel[0], scale[1] - 0.5 * texel[1]],
        )
    }
}

/// This frame's inputs to the post passes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PostFrame {
    /// The render-resolution region (scene, composite, FXAA).
    pub(crate) render: Region,
    /// The swapchain extent (upscale target).
    pub(crate) swapchain: vk::Extent2D,
    /// Exposure adaptation this frame (a reset clears the image and takes the metered value).
    pub(crate) adaptation: crate::post::AdaptationStep,
    pub(crate) effects: ScreenEffects,
    /// The main camera (GTAO reconstructs positions with it).
    pub(crate) view_proj: Mat4,
    pub(crate) eye: Vec3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RegionPush {
    dst_inv: [f32; 2],
    scale: [f32; 2],
    texel: [f32; 2],
}

impl RegionPush {
    fn new(dst: Region, src: Region) -> Self {
        Self {
            dst_inv: dst.inverse(),
            scale: src.scale(),
            texel: src.texel(),
        }
    }
}

/// `bloom_down.frag` / `bloom_up.frag` (`extra[0]`: Karis flag bits or the tent radius).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BloomPush {
    region: RegionPush,
    extra: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ExposurePush {
    scale: [f32; 2],
    texel: [f32; 2],
    min_ev: f32,
    max_ev: f32,
    key: f32,
    reserved: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CompositePush {
    dst_inv: [f32; 2],
    bloom_scale: [f32; 2],
    bloom_texel: [f32; 2],
    exposure: f32,
    saturation: f32,
    bloom_strength: f32,
    grading: f32,
    lut_scale: f32,
    lut_offset: f32,
    desaturation: f32,
    vignette: f32,
    aspect: f32,
    flags: u32,
    vignette_color: [f32; 4],
}

const COMPOSITE_SRGB_TARGET: u32 = 1;
const COMPOSITE_AUTO_EXPOSURE: u32 = 2;
const COMPOSITE_LUMA_ALPHA: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FxaaPush {
    region: RegionPush,
    subpixel: f32,
    edge_threshold: f32,
    edge_min: f32,
    steps: u32,
    srgb_target: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct UpscalePush {
    region: RegionPush,
    mode: u32,
    srgb_target: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GtaoPush {
    ray_origin: [f32; 4],
    ray_du: [f32; 4],
    ray_dv: [f32; 4],
    eye_radius: [f32; 4],
    planes: [f32; 4],
    render: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BlurPush {
    last: [i32; 2],
    sharpness: f32,
    reserved: f32,
}

/// Every post pipeline this renderer's frame needs (`None` for passes it does not have).
pub(crate) struct PostPipelines {
    composite: GraphicsPipeline,
    bloom_down: Option<GraphicsPipeline>,
    bloom_up: Option<GraphicsPipeline>,
    exposure: Option<GraphicsPipeline>,
    fxaa: Option<GraphicsPipeline>,
    upscale: Option<GraphicsPipeline>,
    gtao: Option<GraphicsPipeline>,
    ao_blur: Option<GraphicsPipeline>,
}

/// The post passes' pipelines, sets, samplers and small images.
pub(crate) struct PostPass {
    sets: [vk::DescriptorSet; SetIndex::COUNT],
    _pool: DescriptorPool,
    _set_layout: DescriptorSetLayout,
    pipeline_layout: PipelineLayout,
    pipelines: PostPipelines,
    linear: Sampler,
    nearest: Sampler,
    /// 1×1 white: bound where a pass has no input in that slot.
    white: GpuImage,
    /// 2³ identity LUT: bound without a map LUT.
    identity_lut: GpuImage,
    lut: Option<(GpuImage, u32)>,
    /// The adapted exposure (1×1, persistent), with auto exposure.
    exposure_image: Option<GpuImage>,
    /// The views the sets were last written with.
    views: PostViews,
    spec: FrameSpec,
    config: PostConfig,
    swapchain_format: vk::Format,
    base_exposure: f32,
    look: PostLook,
}

impl PostPass {
    /// Builds the pipelines `spec`'s frame needs, the samplers, default images and (with auto
    /// exposure) the persistent exposure image, cleared and in `SHADER_READ_ONLY_OPTIMAL`.
    #[allow(clippy::too_many_lines)] // Layout, sets, samplers and images, in order.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        spec: FrameSpec,
        config: PostConfig,
        swapchain_format: vk::Format,
        exposure: f32,
    ) -> RenderResult<Self> {
        let bindings: [vk::DescriptorSetLayoutBinding<'_>; SET_BINDINGS] =
            core::array::from_fn(|index| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(index as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            });
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let count = u32::try_from(SetIndex::COUNT).expect("small");
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(count * SET_BINDINGS as u32)];
        let pool = DescriptorPool::new(device.device.clone(), &pool_sizes, count)?;
        let allocated = pool.allocate(&device.device, &[set_layout.handle(); SetIndex::COUNT])?;
        let sets: [vk::DescriptorSet; SetIndex::COUNT] = core::array::from_fn(|i| allocated[i]);
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(PUSH_SIZE)];
        let pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &push_range,
        )?;
        let pipelines = build_pipelines(
            device,
            shaders,
            pipeline_layout.handle(),
            &spec,
            swapchain_format,
        )?;
        let clamp = vk::SamplerAddressMode::CLAMP_TO_EDGE;
        let sampler = |filter| {
            Sampler::new(
                device.device.clone(),
                &vk::SamplerCreateInfo::default()
                    .mag_filter(filter)
                    .min_filter(filter)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(clamp)
                    .address_mode_v(clamp)
                    .address_mode_w(clamp)
                    .max_lod(0.0),
            )
        };
        let linear = sampler(vk::Filter::LINEAR)?;
        let nearest = sampler(vk::Filter::NEAREST)?;
        let white = texture::upload_rgba(device, "post-white", 1, 1, &[255; 4])?;
        let identity_lut = upload_lut(device, "grading-identity", 2, &identity_texels(2))?;
        let exposure_image = if spec.auto_exposure {
            let image = GpuImage::new(
                device.device.clone(),
                device.allocator(),
                "exposure",
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                spec.exposure_format,
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageAspectFlags::COLOR,
            )?;
            if let Err((error, in_use)) = crate::targets::initialise_sampled(device, image.image())
            {
                if in_use {
                    core::mem::forget(image);
                }
                return Err(error);
            }
            Some(image)
        } else {
            None
        };
        let base_exposure = if exposure.is_finite() && exposure > 0.0 {
            exposure
        } else {
            1.0
        };
        let pass = Self {
            sets,
            _pool: pool,
            _set_layout: set_layout,
            pipeline_layout,
            pipelines,
            linear,
            nearest,
            white,
            identity_lut,
            lut: None,
            exposure_image,
            views: PostViews::default(),
            spec,
            config,
            swapchain_format,
            base_exposure,
            look: PostLook::DEFAULT,
        };
        pass.write_sets(device);
        Ok(pass)
    }

    /// Builds replacement pipelines (dev shader reload) without installing them.
    pub(crate) fn build_pipelines(
        &self,
        device: &DeviceContext,
        shaders: &ShaderSource,
    ) -> RenderResult<PostPipelines> {
        build_pipelines(
            device,
            shaders,
            self.pipeline_layout.handle(),
            &self.spec,
            self.swapchain_format,
        )
    }

    /// Installs pipelines from [`Self::build_pipelines`]. The device must be idle.
    pub(crate) fn install_pipelines(&mut self, pipelines: PostPipelines) {
        self.pipelines = pipelines;
    }

    /// `RenderConfig::exposure`, sanitised (the UI's offscreen images use it too).
    pub(crate) fn exposure(&self) -> f32 {
        self.base_exposure
    }

    /// The persistent exposure image, with auto exposure.
    pub(crate) fn exposure_image(&self) -> Option<vk::Image> {
        self.exposure_image.as_ref().map(GpuImage::image)
    }

    /// The persistent exposure image's view, with auto exposure.
    pub(crate) fn exposure_view(&self) -> Option<vk::ImageView> {
        self.exposure_image.as_ref().map(GpuImage::view)
    }

    /// The map's art direction (sanitised).
    pub(crate) fn look(&self) -> PostLook {
        self.look
    }

    /// Sets the map's art direction. Takes effect on the next recorded frame.
    pub(crate) fn set_look(&mut self, look: PostLook) {
        self.look = look.sanitized();
    }

    /// Whether this frame meters and adapts exposure: the frame has the pass, the config
    /// allows it and the map asks for it.
    pub(crate) fn auto_exposure_active(&self) -> bool {
        self.spec.auto_exposure && self.config.auto_exposure && self.look.auto_exposure
    }

    /// Points every set at `views` (and the LUT, the exposure image and white elsewhere).
    /// Only while no submitted frame uses the sets (creation, after a resize's idle wait).
    pub(crate) fn bind(&mut self, device: &DeviceContext, views: PostViews) {
        self.views = views;
        self.write_sets(device);
    }

    /// Uploads a grading LUT (`size`³ texels, red fastest) or restores the identity with
    /// `None`. The device must be idle.
    pub(crate) fn set_grading_lut(
        &mut self,
        device: &DeviceContext,
        lut: Option<(u32, &[[f32; 3]])>,
    ) -> RenderResult<()> {
        self.lut = match lut {
            Some((size, texels)) => {
                let expected = (size as usize).pow(3);
                if size < 2 || texels.len() != expected {
                    return Err(RenderError::InvalidTexture);
                }
                Some((upload_lut(device, "grading-lut", size, texels)?, size))
            }
            None => None,
        };
        self.write_sets(device);
        Ok(())
    }

    fn write_sets(&self, device: &DeviceContext) {
        let white = self.white.view();
        let views = &self.views;
        let or_white = |view: Option<vk::ImageView>| view.unwrap_or(white);
        let lut = self
            .lut
            .as_ref()
            .map_or(self.identity_lut.view(), |(image, _)| image.view());
        let exposure = self.exposure_image.as_ref().map_or(white, GpuImage::view);
        let (linear, nearest) = (self.linear.handle(), self.nearest.handle());
        let write = |set: SetIndex, entries: [(vk::Sampler, vk::ImageView); SET_BINDINGS]| {
            let infos: [[vk::DescriptorImageInfo; 1]; SET_BINDINGS] =
                core::array::from_fn(|index| {
                    [vk::DescriptorImageInfo::default()
                        .sampler(entries[index].0)
                        .image_view(entries[index].1)
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
                });
            let writes: [vk::WriteDescriptorSet<'_>; SET_BINDINGS] =
                core::array::from_fn(|index| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.sets[set.index()])
                        .dst_binding(index as u32)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&infos[index])
                });
            // SAFETY: the sets, samplers and views are live, and the callers guarantee no
            // pending command buffer uses the sets (creation, or after a device-idle wait).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        };
        let fill = |first: (vk::Sampler, vk::ImageView)| {
            [first, (linear, white), (linear, white), (linear, white)]
        };
        write(
            SetIndex::Composite,
            [
                (nearest, or_white(views.hdr)),
                (linear, or_white(views.bloom[0])),
                (nearest, exposure),
                (linear, lut),
            ],
        );
        for level in 0..BLOOM_LEVELS {
            let source = if level == 0 {
                views.hdr
            } else {
                views.bloom[level - 1]
            };
            write(SetIndex::BloomDown(level), fill((linear, or_white(source))));
        }
        for level in 0..BLOOM_LEVELS - 1 {
            write(
                SetIndex::BloomUp(level),
                fill((linear, or_white(views.bloom[level + 1]))),
            );
        }
        write(
            SetIndex::Exposure,
            fill((linear, or_white(views.bloom[BLOOM_LEVELS - 1]))),
        );
        write(SetIndex::Fxaa, fill((linear, or_white(views.ldr))));
        write(
            SetIndex::Upscale,
            fill((linear, or_white(views.ldr2.or(views.ldr)))),
        );
        write(SetIndex::Ao, fill((nearest, or_white(views.depth))));
        write(SetIndex::AoBlur, fill((nearest, or_white(views.ao_raw))));
    }

    #[allow(clippy::too_many_arguments)] // One fullscreen draw, flattened.
    fn draw(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        extent: vk::Extent2D,
        load: vk::AttachmentLoadOp,
        pipeline: &GraphicsPipeline,
        set: SetIndex,
        push: &[u8],
    ) {
        super::begin_color_only(
            device,
            command_buffer,
            target,
            extent,
            load,
            super::full_viewport(extent),
        );
        self.bind_and_draw(device, command_buffer, pipeline, set, push);
        device.cmd_end_rendering(command_buffer);
    }

    /// Binds `pipeline` and `set`, pushes `push` and draws the fullscreen triangle inside
    /// the current rendering scope.
    fn bind_and_draw(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        pipeline: &GraphicsPipeline,
        set: SetIndex,
        push: &[u8],
    ) {
        // SAFETY: the pipeline is live and was built for this attachment format.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.handle(),
            );
        }
        // SAFETY: the set matches set 0 of the layout and names live views the graph
        // transitioned to SHADER_READ_ONLY_OPTIMAL (or persistent images kept there).
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout.handle(),
                0,
                &[self.sets[set.index()]],
                &[],
            );
        }
        debug_assert!(push.len() <= PUSH_SIZE as usize);
        // SAFETY: the layout declares a 128-byte fragment-stage range at offset 0 and every
        // block is at most that size.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                push,
            );
        }
        // SAFETY: the fullscreen vertex shader reads no vertex buffers; three vertices form
        // the covering triangle.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
    }

    /// GTAO into `target` (the raw AO image) from the prepass depth.
    pub(crate) fn record_ao(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.gtao else {
            return;
        };
        let ao = frame.render.shifted(1);
        let push = gtao_push(frame, self.config.ao_slices, self.config.ao_radius);
        self.draw(
            device,
            command_buffer,
            target,
            ao.actual,
            vk::AttachmentLoadOp::DONT_CARE,
            pipeline,
            SetIndex::Ao,
            bytes_of(&push),
        );
    }

    /// The depth-aware AO blur into `target`.
    pub(crate) fn record_ao_blur(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.ao_blur else {
            return;
        };
        let ao = frame.render.shifted(1);
        let last = |side: u32| i32::try_from(side.saturating_sub(1)).unwrap_or(i32::MAX);
        let push = BlurPush {
            last: [last(ao.actual.width), last(ao.actual.height)],
            sharpness: AO_BLUR_SHARPNESS,
            reserved: 0.0,
        };
        self.draw(
            device,
            command_buffer,
            target,
            ao.actual,
            vk::AttachmentLoadOp::DONT_CARE,
            pipeline,
            SetIndex::AoBlur,
            bytes_of(&push),
        );
    }

    /// Bloom down-sample into mip `level` (`target`).
    pub(crate) fn record_bloom_down(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        level: usize,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.bloom_down else {
            return;
        };
        let dst = frame.render.shifted(mip_shift(level));
        let src = if level == 0 {
            frame.render
        } else {
            frame.render.shifted(mip_shift(level - 1))
        };
        let push = BloomPush {
            region: RegionPush::new(dst, src),
            extra: [u32::from(level == 0), 0],
        };
        self.draw(
            device,
            command_buffer,
            target,
            dst.actual,
            vk::AttachmentLoadOp::DONT_CARE,
            pipeline,
            SetIndex::BloomDown(level),
            bytes_of(&push),
        );
    }

    /// Bloom up-sample: mip `level + 1` added into mip `level` (`target`).
    pub(crate) fn record_bloom_up(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        level: usize,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.bloom_up else {
            return;
        };
        let dst = frame.render.shifted(mip_shift(level));
        let src = frame.render.shifted(mip_shift(level + 1));
        let push = BloomPush {
            region: RegionPush::new(dst, src),
            extra: [BLOOM_RADIUS.to_bits(), 0],
        };
        self.draw(
            device,
            command_buffer,
            target,
            dst.actual,
            vk::AttachmentLoadOp::LOAD,
            pipeline,
            SetIndex::BloomUp(level),
            bytes_of(&push),
        );
    }

    /// Meters the last bloom mip and blends the clamped target EV into the exposure image
    /// with `frame.adaptation`'s factor; a reset clears the image first, so a non-finite
    /// value can never stay latched. A map without auto exposure keeps the pass but writes
    /// 0 EV at once, so turning it on later starts from neutral.
    pub(crate) fn record_exposure(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.exposure else {
            return;
        };
        let source = frame.render.shifted(mip_shift(BLOOM_LEVELS - 1));
        let (min_ev, max_ev, factor) = if self.auto_exposure_active() {
            (
                self.look.exposure_min,
                self.look.exposure_max,
                frame.adaptation.factor.clamp(0.0, 1.0),
            )
        } else {
            (0.0, 0.0, 1.0)
        };
        let load = if frame.adaptation.reset {
            vk::AttachmentLoadOp::CLEAR
        } else {
            vk::AttachmentLoadOp::LOAD
        };
        let push = ExposurePush {
            scale: source.scale(),
            texel: source.texel(),
            min_ev,
            max_ev,
            key: EXPOSURE_KEY,
            reserved: 0.0,
        };
        let extent = vk::Extent2D {
            width: 1,
            height: 1,
        };
        super::begin_color_only(
            device,
            command_buffer,
            target,
            extent,
            load,
            super::full_viewport(extent),
        );
        // SAFETY: the exposure pipeline declares BLEND_CONSTANTS dynamic.
        unsafe {
            device
                .device
                .cmd_set_blend_constants(command_buffer, &[factor; 4]);
        }
        self.bind_and_draw(
            device,
            command_buffer,
            pipeline,
            SetIndex::Exposure,
            bytes_of(&push),
        );
        device.cmd_end_rendering(command_buffer);
    }

    /// The composite into `target`: the LDR image (`swapchain` false) or the swapchain.
    pub(crate) fn record_composite(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        swapchain: bool,
        frame: &PostFrame,
    ) {
        let push = self.composite_push(swapchain, frame);
        self.draw(
            device,
            command_buffer,
            target,
            frame.render.actual,
            vk::AttachmentLoadOp::DONT_CARE,
            &self.pipelines.composite,
            SetIndex::Composite,
            bytes_of(&push),
        );
    }

    fn composite_push(&self, swapchain: bool, frame: &PostFrame) -> CompositePush {
        let look = self.look;
        let bloom = frame.render.shifted(mip_shift(0));
        let (lut_size, grading) = match &self.lut {
            Some((_, size)) if self.config.grading => (*size, look.grading_strength),
            _ => (2, 0.0),
        };
        let effects = if self.config.screen_effects {
            frame.effects.sanitized()
        } else {
            ScreenEffects::NONE
        };
        let mut flags = 0;
        if swapchain && super::composite::is_srgb(self.swapchain_format) {
            flags |= COMPOSITE_SRGB_TARGET;
        }
        if self.auto_exposure_active() {
            flags |= COMPOSITE_AUTO_EXPOSURE;
        }
        if !swapchain {
            flags |= COMPOSITE_LUMA_ALPHA;
        }
        let actual = frame.render.actual;
        let color = effects.vignette_color;
        CompositePush {
            dst_inv: frame.render.inverse(),
            bloom_scale: bloom.scale(),
            bloom_texel: bloom.texel(),
            exposure: self.base_exposure * look.exposure_ev.exp2(),
            saturation: TONEMAP_SATURATION,
            bloom_strength: if self.spec.bloom { look.bloom } else { 0.0 },
            grading,
            lut_scale: (lut_size - 1) as f32 / lut_size as f32,
            lut_offset: 0.5 / lut_size as f32,
            desaturation: effects.desaturation,
            vignette: effects.vignette,
            aspect: actual.width as f32 / actual.height.max(1) as f32,
            flags,
            vignette_color: [color[0], color[1], color[2], 1.0],
        }
    }

    /// FXAA into `target`: the second LDR image (`swapchain` false) or the swapchain.
    pub(crate) fn record_fxaa(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        swapchain: bool,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.fxaa else {
            return;
        };
        let (edge_threshold, edge_min, steps) = if self.config.fxaa_fast {
            (0.25, 0.0833, 5)
        } else {
            (0.125, 0.0625, 10)
        };
        let push = FxaaPush {
            region: RegionPush::new(frame.render, frame.render),
            subpixel: 0.75,
            edge_threshold,
            edge_min,
            steps,
            srgb_target: u32::from(swapchain && super::composite::is_srgb(self.swapchain_format)),
            reserved: 0,
        };
        self.draw(
            device,
            command_buffer,
            target,
            frame.render.actual,
            vk::AttachmentLoadOp::DONT_CARE,
            pipeline,
            SetIndex::Fxaa,
            bytes_of(&push),
        );
    }

    /// The upscale from the render-resolution LDR image into the swapchain.
    pub(crate) fn record_upscale(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        target: vk::ImageView,
        frame: &PostFrame,
    ) {
        let Some(pipeline) = &self.pipelines.upscale else {
            return;
        };
        let dst = Region {
            actual: frame.swapchain,
            alloc: frame.swapchain,
        };
        let push = UpscalePush {
            region: RegionPush::new(dst, frame.render),
            mode: u32::from(self.config.upscale == UpscaleFilter::Bicubic),
            srgb_target: u32::from(super::composite::is_srgb(self.swapchain_format)),
        };
        self.draw(
            device,
            command_buffer,
            target,
            frame.swapchain,
            vk::AttachmentLoadOp::DONT_CARE,
            pipeline,
            SetIndex::Upscale,
            bytes_of(&push),
        );
    }
}

/// The graph's shift of bloom mip `level` (mip 0 is half resolution).
fn mip_shift(level: usize) -> u8 {
    u8::try_from(level + 1).unwrap_or(u8::MAX)
}

/// GTAO's push block: the view rays at unit view depth (corner and per-axis change, affine in
/// the viewport uv), the eye and radius, the near and far view depths and the focal length in
/// render pixels, the render sub-rectangle and the slice/step counts.
fn gtao_push(frame: &PostFrame, slices: u32, radius: f32) -> GtaoPush {
    let inverse = frame.view_proj.inverse();
    let unproject = |x: f32, y: f32, z: f32| {
        let world = inverse * Vec4::new(x, y, z, 1.0);
        world.truncate() / world.w
    };
    let near_point = unproject(0.0, 0.0, 0.0);
    let far_point = unproject(0.0, 0.0, 1.0);
    let forward = (far_point - near_point).normalize_or_zero();
    let near = (near_point - frame.eye).dot(forward).max(1e-4);
    let far = (far_point - frame.eye).dot(forward).max(near * 2.0);
    // The ray at unit view depth through viewport uv (NDC y = 1 - 2 v: the scene's viewport
    // is flipped), from points at a mid depth where the unprojection is well conditioned.
    let ray = |u: f32, v: f32| {
        let point = unproject(u * 2.0 - 1.0, 1.0 - v * 2.0, 0.5);
        let offset = point - frame.eye;
        offset / offset.dot(forward).max(1e-6)
    };
    let origin = ray(0.0, 0.0);
    let du = ray(1.0, 0.0) - origin;
    let dv = ray(0.0, 1.0) - origin;
    // Half the view's height at unit depth.
    let tan_half = 0.5 * dv.length();
    let actual = frame.render.actual;
    let focal = 0.5 * actual.height as f32 / tan_half.max(1e-4);
    GtaoPush {
        ray_origin: [origin.x, origin.y, origin.z, 0.0],
        ray_du: [du.x, du.y, du.z, 0.0],
        ray_dv: [dv.x, dv.y, dv.z, 0.0],
        eye_radius: [frame.eye.x, frame.eye.y, frame.eye.z, radius],
        planes: [near, far, focal, 0.0],
        render: [
            1.0 / actual.width.max(1) as f32,
            1.0 / actual.height.max(1) as f32,
            actual.width.saturating_sub(1) as f32,
            actual.height.saturating_sub(1) as f32,
        ],
        params: [slices as f32, AO_STEPS, AO_FALLOFF_START, 0.0],
    }
}

/// The identity LUT of `size`³ texels, red fastest.
fn identity_texels(size: u32) -> Vec<[f32; 3]> {
    let n = size as usize;
    let step = 1.0 / (size - 1).max(1) as f32;
    (0..n * n * n)
        .map(|index| {
            [
                (index % n) as f32 * step,
                ((index / n) % n) as f32 * step,
                (index / (n * n)) as f32 * step,
            ]
        })
        .collect()
}

/// Uploads a `size`³ LUT as an `R16G16B16A16_SFLOAT` 3D texture (load time).
fn upload_lut(
    device: &DeviceContext,
    name: &str,
    size: u32,
    texels: &[[f32; 3]],
) -> RenderResult<GpuImage> {
    let mut bytes = Vec::with_capacity(texels.len() * 8);
    for texel in texels {
        for value in [texel[0], texel[1], texel[2], 1.0] {
            bytes
                .extend_from_slice(&moba_assets::f16_from_f32(value.clamp(0.0, 1.0)).to_le_bytes());
        }
    }
    texture::upload_raw(
        device,
        name,
        &texture::RawImage {
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent2D {
                width: size,
                height: size,
            },
            cube: false,
            depth: size,
            levels: &[(0, bytes.len())],
            bytes: &bytes,
        },
    )
}

fn build_pipelines(
    device: &DeviceContext,
    shaders: &ShaderSource,
    layout: vk::PipelineLayout,
    spec: &FrameSpec,
    swapchain_format: vk::Format,
) -> RenderResult<PostPipelines> {
    let vertex = shaders.module(device, "fullscreen.vert")?;
    let build = |fragment: &str, format, blend| -> RenderResult<GraphicsPipeline> {
        let fragment = shaders.module(device, fragment)?;
        let handle = pipeline::create_fullscreen(
            device,
            layout,
            vertex.handle(),
            fragment.handle(),
            format,
            blend,
        )?;
        Ok(GraphicsPipeline::from_raw(device.device.clone(), handle))
    };
    let optional = |on: bool, fragment: &str, format, blend| {
        on.then(|| build(fragment, format, blend)).transpose()
    };
    let composite_format = if spec.fxaa || spec.upscale {
        LDR_FORMAT
    } else {
        swapchain_format
    };
    let fxaa_format = if spec.upscale {
        LDR_FORMAT
    } else {
        swapchain_format
    };
    Ok(PostPipelines {
        composite: build("composite.frag", composite_format, FullscreenBlend::None)?,
        bloom_down: optional(
            spec.bloom_chain(),
            "bloom_down.frag",
            spec.bloom_format,
            FullscreenBlend::None,
        )?,
        bloom_up: optional(
            spec.bloom,
            "bloom_up.frag",
            spec.bloom_format,
            FullscreenBlend::Additive,
        )?,
        exposure: optional(
            spec.auto_exposure,
            "exposure.frag",
            spec.exposure_format,
            FullscreenBlend::Constant,
        )?,
        fxaa: optional(spec.fxaa, "fxaa.frag", fxaa_format, FullscreenBlend::None)?,
        upscale: optional(
            spec.upscale,
            "upscale.frag",
            swapchain_format,
            FullscreenBlend::None,
        )?,
        gtao: optional(
            spec.ambient_occlusion,
            "gtao.frag",
            AO_RAW_FORMAT,
            FullscreenBlend::None,
        )?,
        ao_blur: optional(
            spec.ambient_occlusion,
            "ao_blur.frag",
            AO_FORMAT,
            FullscreenBlend::None,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;
    use moba_core::math::{look_at_rh, perspective_rh};

    #[test]
    fn push_blocks_match_the_shaders_and_fit_the_range() {
        assert_eq!(size_of::<BloomPush>(), 32);
        assert_eq!(size_of::<ExposurePush>(), 32);
        assert_eq!(size_of::<CompositePush>(), 80);
        assert_eq!(core::mem::offset_of!(CompositePush, vignette_color), 64);
        assert_eq!(size_of::<FxaaPush>(), 48);
        assert_eq!(size_of::<UpscalePush>(), 32);
        assert_eq!(size_of::<GtaoPush>(), 112);
        assert_eq!(size_of::<BlurPush>(), 16);
        assert!(size_of::<GtaoPush>() <= PUSH_SIZE as usize);
        let indices: Vec<usize> = (0..BLOOM_LEVELS)
            .map(|level| SetIndex::BloomDown(level).index())
            .chain((0..BLOOM_LEVELS - 1).map(|level| SetIndex::BloomUp(level).index()))
            .chain(
                [
                    SetIndex::Composite,
                    SetIndex::Exposure,
                    SetIndex::Fxaa,
                    SetIndex::Upscale,
                    SetIndex::Ao,
                    SetIndex::AoBlur,
                ]
                .map(SetIndex::index),
            )
            .collect();
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..SetIndex::COUNT).collect::<Vec<_>>());
    }

    fn extent(width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    #[test]
    fn regions_map_sub_rectangles_and_mips() {
        let full = Region {
            actual: extent(1920, 1080),
            alloc: extent(1920, 1080),
        };
        assert_eq!(full.scale(), [1.0, 1.0]);
        let scaled = Region {
            actual: extent(1344, 756),
            alloc: extent(1920, 1080),
        };
        assert_eq!(scaled.scale(), [0.7, 0.7]);
        let half = scaled.shifted(1);
        assert_eq!(half.actual, extent(672, 378));
        assert_eq!(half.alloc, extent(960, 540));
        assert_eq!(scaled.shifted(5).alloc, extent(60, 33));
        // The AO lookup maps render pixel p to AO uv p / (2 * alloc), clamped inside.
        let (per_pixel, clamp) = half.ao_lookup();
        assert!((per_pixel[0] - 0.5 / 960.0).abs() < 1e-9);
        assert!((clamp[0] - (672.0 - 0.5) / 960.0).abs() < 1e-6);
        assert_eq!(mip_shift(0), 1);
    }

    #[test]
    fn identity_lut_is_the_lattice() {
        let texels = identity_texels(3);
        assert_eq!(texels.len(), 27);
        assert_eq!(texels[0], [0.0, 0.0, 0.0]);
        assert_eq!(texels[1], [0.5, 0.0, 0.0]);
        assert_eq!(texels[3], [0.0, 0.5, 0.0]);
        assert_eq!(texels[26], [1.0, 1.0, 1.0]);
    }

    #[test]
    fn gtao_push_recovers_the_planes_and_focal_length() {
        let eye = Vec3::new(0.0, 40.0, 30.0);
        let fov = 0.9_f32;
        let view_proj =
            perspective_rh(fov, 16.0 / 9.0, 0.5, 600.0) * look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let frame = PostFrame {
            render: Region {
                actual: extent(1600, 900),
                alloc: extent(1920, 1080),
            },
            swapchain: extent(1920, 1080),
            adaptation: crate::post::AdaptationStep {
                factor: 1.0,
                reset: true,
            },
            effects: ScreenEffects::NONE,
            view_proj,
            eye,
        };
        let push = gtao_push(&frame, 2, 1.5);
        assert!(
            (push.planes[0] - 0.5).abs() < 1e-3,
            "near {}",
            push.planes[0]
        );
        assert!(
            (push.planes[1] - 600.0).abs() / 600.0 < 1e-2,
            "far {}",
            push.planes[1]
        );
        let expected_focal = 0.5 * 900.0 / (fov * 0.5).tan();
        assert!(
            (push.planes[2] - expected_focal).abs() / expected_focal < 1e-2,
            "{} vs {expected_focal}",
            push.planes[2]
        );
        assert_eq!(push.render[2], 1599.0);
        // The centre ray at unit depth points at the target.
        let centre = Vec3::from_slice(&push.ray_origin[..3])
            + 0.5 * Vec3::from_slice(&push.ray_du[..3])
            + 0.5 * Vec3::from_slice(&push.ray_dv[..3]);
        assert!(
            (centre.normalize() - (-eye).normalize()).length() < 1e-3,
            "{centre}"
        );
        // A point reconstructed from depth lands where the camera projects it.
        let world = Vec3::new(3.0, 0.0, -4.0);
        let clip = view_proj * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let (u, v) = (ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let (near, far) = (push.planes[0], push.planes[1]);
        let depth = near * far / (far - ndc.z * (far - near));
        let rebuilt = eye
            + (Vec3::from_slice(&push.ray_origin[..3])
                + u * Vec3::from_slice(&push.ray_du[..3])
                + v * Vec3::from_slice(&push.ray_dv[..3]))
                * depth;
        assert!((rebuilt - world).length() < 1e-2, "{rebuilt} vs {world}");
        assert_eq!(push.params[0], 2.0);
    }
}
