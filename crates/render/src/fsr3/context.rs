//! The FSR 3.1.6 context: internal resources, pipelines, descriptor sets and the recorded
//! dispatch sequence (a port of `fsr3upscalerDispatch` and the parts of the SDK's Vulkan back
//! end it drives).

use ash::vk;
use ash::vk::TaggedStructure as _;
use gpu_allocator::MemoryLocation;
use moba_shaders::fsr3::{self as shaders, Kind, Pass};

use super::constants::{RcasConstants, SpdConstants, UpscalerConstants};
use super::{Fsr3Inputs, Fsr3Mode, Fsr3Output, KIND, jitter_phases};
use crate::caps::RenderPath;
use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    Sampler, ShaderModule,
};

/// `FSR3UPSCALER_MAX_QUEUED_FRAMES`: the resource frame index runs modulo this (even) count;
/// its parity picks the history images.
const MAX_QUEUED_FRAMES: u32 = 16;
/// Work-group tile of the 8×8 passes and of RCAS.
const TILE: u32 = 8;
const RCAS_TILE: u32 = 16;
/// The SPD mip levels the shading-change pyramid writes (`rw_spd_mip0..5`).
const SPD_STORAGE_MIPS: u32 = 6;

/// Storage-image formats and the format features FSR needs on each (Vulkan 1.3 format
/// features 2): the shaders declare float storage images without a format, so every one
/// needs `STORAGE_WRITE_WITHOUT_FORMAT`, and the three read back through a storage view
/// (`rw_new_locks`, `rw_frame_info`, `rw_spd_mip5`) also `STORAGE_READ_WITHOUT_FORMAT`.
/// `R32_UINT` is declared `R32ui` and takes atomics. The caller's `RGBA16F` output is in the
/// list.
const FORMAT_NEEDS: &[(vk::Format, vk::FormatFeatureFlags2)] = &[
    (vk::Format::R8_UNORM, READ_WRITE),
    (vk::Format::R16_SFLOAT, WRITE),
    (vk::Format::R16G16_SFLOAT, READ_WRITE),
    (vk::Format::R16G16B16A16_SFLOAT, WRITE),
    (vk::Format::R32_SFLOAT, WRITE),
    (vk::Format::R8G8B8A8_UNORM, WRITE),
    (vk::Format::R32G32B32A32_SFLOAT, READ_WRITE),
    (
        vk::Format::R32_UINT,
        vk::FormatFeatureFlags2::from_raw(
            vk::FormatFeatureFlags2::STORAGE_IMAGE.as_raw()
                | vk::FormatFeatureFlags2::STORAGE_IMAGE_ATOMIC.as_raw()
                | vk::FormatFeatureFlags2::SAMPLED_IMAGE.as_raw()
                | vk::FormatFeatureFlags2::TRANSFER_DST.as_raw(),
        ),
    ),
];
const WRITE: vk::FormatFeatureFlags2 = vk::FormatFeatureFlags2::from_raw(
    vk::FormatFeatureFlags2::STORAGE_IMAGE.as_raw()
        | vk::FormatFeatureFlags2::STORAGE_WRITE_WITHOUT_FORMAT.as_raw()
        | vk::FormatFeatureFlags2::SAMPLED_IMAGE.as_raw()
        | vk::FormatFeatureFlags2::SAMPLED_IMAGE_FILTER_LINEAR.as_raw()
        | vk::FormatFeatureFlags2::TRANSFER_DST.as_raw(),
);
const READ_WRITE: vk::FormatFeatureFlags2 = vk::FormatFeatureFlags2::from_raw(
    WRITE.as_raw() | vk::FormatFeatureFlags2::STORAGE_READ_WITHOUT_FORMAT.as_raw(),
);

/// Why `device` cannot run FSR 3.1, or `None` when it can: the tiered path (which enables
/// storage writes without a format), quad subgroup operations in compute (the two SPD
/// passes), and [`FORMAT_NEEDS`].
pub(crate) fn unavailable(device: &DeviceContext) -> Option<String> {
    let mut missing = Vec::new();
    if device.admission.path != RenderPath::Tiered {
        missing.push(String::from("the tiered render path"));
    }
    let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push(&mut subgroup);
    // SAFETY: `physical` belongs to this instance and the chain is valid for the call.
    unsafe {
        device
            .instance
            .get_physical_device_properties2(device.physical, &mut properties);
    }
    if !subgroup
        .supported_stages
        .contains(vk::ShaderStageFlags::COMPUTE)
        || !subgroup
            .supported_operations
            .contains(vk::SubgroupFeatureFlags::QUAD)
    {
        missing.push(String::from("quad subgroup operations in compute shaders"));
    }
    for &(format, needs) in FORMAT_NEEDS {
        let mut features = vk::FormatProperties3::default();
        let mut properties = vk::FormatProperties2::default().push(&mut features);
        // SAFETY: as above.
        unsafe {
            device.instance.get_physical_device_format_properties2(
                device.physical,
                format,
                &mut properties,
            );
        }
        let lacking = needs & !features.optimal_tiling_features;
        if !lacking.is_empty() {
            missing.push(format!("{format:?} optimal-tiling features {lacking:?}"));
        }
    }
    (!missing.is_empty()).then(|| missing.join(", "))
}

/// Where one descriptor comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Uniform(Block),
    Image(vk::ImageView),
    LinearClamp,
}

/// The three constant blocks of one parity slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Upscaler,
    Spd,
    Rcas,
}

/// The caller's views a descriptor set was last written with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bound {
    inputs: [vk::ImageView; 6],
    output: vk::ImageView,
}

/// One pass's pipeline and its descriptor set for each parity.
struct PassPipeline {
    pass: &'static Pass,
    // Destroyed in field order: the pipeline before its layouts.
    pipeline: GraphicsPipeline,
    layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    sets: [vk::DescriptorSet; 2],
}

/// The internal images (`getInternalResourceDescriptions` and
/// `getSharedResourceDescriptions`), at the context's maximum sizes.
struct Images {
    /// Views of `spd_mips` levels 0 to 5 (clamped to its last level); dropped before it.
    spd_mip_views: Vec<MipView>,
    /// `R8_UNORM` render; the parity's SRV is `[odd]`, its UAV `[!odd]`.
    accumulation: [GpuImage; 2],
    /// `R16_SFLOAT` render; the current luma is `[odd]`, the previous `[!odd]`.
    luma: [GpuImage; 2],
    /// `R16_SFLOAT` render: the farthest depth, then the luma instability.
    intermediate: GpuImage,
    /// `R8_UNORM`, half render.
    shading_change: GpuImage,
    /// `R8_UNORM` upscale.
    new_locks: GpuImage,
    /// `RGBA16F` upscale; the parity's SRV is `[odd]`, its UAV `[!odd]`.
    upscaled: [GpuImage; 2],
    /// `R16G16_SFLOAT`, half render, every mip level.
    spd_mips: GpuImage,
    /// `R16_SFLOAT`, half render.
    farthest_depth_mip1: GpuImage,
    /// `RGBA16F` render; SRV `[odd]`, UAV `[!odd]`.
    luma_history: [GpuImage; 2],
    /// `R32_UINT` 1×1.
    spd_atomic: GpuImage,
    /// `RGBA8_UNORM` render.
    dilated_reactive_masks: GpuImage,
    /// `RGBA32F` 1×1.
    frame_info: GpuImage,
    /// `R32_SFLOAT` render.
    dilated_depth: GpuImage,
    /// `R16G16_SFLOAT` render.
    dilated_motion: GpuImage,
    /// `R32_UINT` render.
    reconstructed_previous_depth: GpuImage,
}

/// A view of one mip level, destroyed with its owner.
struct MipView {
    device: ash::Device,
    view: vk::ImageView,
}

impl Drop for MipView {
    fn drop(&mut self) {
        // SAFETY: the view belongs to this device and is destroyed once, before its image
        // (`Images` declares the views first), and `Fsr3`'s owner keeps the GPU from using it
        // past the drop like every other context resource.
        unsafe { self.device.destroy_image_view(self.view, None) };
    }
}

impl Images {
    fn new(
        device: &DeviceContext,
        render: vk::Extent2D,
        upscale: vk::Extent2D,
    ) -> RenderResult<Self> {
        let half = vk::Extent2D {
            width: (render.width / 2).max(1),
            height: (render.height / 2).max(1),
        };
        let one = vk::Extent2D {
            width: 1,
            height: 1,
        };
        let usage = vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_DST;
        let image = |name: &str, extent, format| {
            GpuImage::new(
                device.device.clone(),
                device.allocator(),
                name,
                extent,
                format,
                usage,
                vk::ImageAspectFlags::COLOR,
            )
        };
        let spd_mip_levels = half.width.max(half.height).ilog2() + 1;
        let spd_mips = GpuImage::with_mips(
            device.device.clone(),
            device.allocator(),
            "fsr3-spd-mips",
            half,
            vk::Format::R16G16_SFLOAT,
            usage,
            vk::ImageAspectFlags::COLOR,
            spd_mip_levels,
        )?;
        let mut spd_mip_views = Vec::with_capacity(SPD_STORAGE_MIPS as usize);
        for level in 0..SPD_STORAGE_MIPS {
            let info = vk::ImageViewCreateInfo::default()
                .image(spd_mips.image())
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R16G16_SFLOAT)
                .subresource_range(color_range(level.min(spd_mip_levels - 1), 1));
            // SAFETY: the image is live and the create info is fully initialised.
            let view = unsafe { device.device.create_image_view(&info, None) }?;
            spd_mip_views.push(MipView {
                device: device.device.clone(),
                view,
            });
        }
        Ok(Self {
            spd_mip_views,
            accumulation: [
                image("fsr3-accumulation-1", render, vk::Format::R8_UNORM)?,
                image("fsr3-accumulation-2", render, vk::Format::R8_UNORM)?,
            ],
            luma: [
                image("fsr3-luma-1", render, vk::Format::R16_SFLOAT)?,
                image("fsr3-luma-2", render, vk::Format::R16_SFLOAT)?,
            ],
            intermediate: image("fsr3-intermediate", render, vk::Format::R16_SFLOAT)?,
            shading_change: image("fsr3-shading-change", half, vk::Format::R8_UNORM)?,
            new_locks: image("fsr3-new-locks", upscale, vk::Format::R8_UNORM)?,
            upscaled: [
                image("fsr3-upscaled-1", upscale, vk::Format::R16G16B16A16_SFLOAT)?,
                image("fsr3-upscaled-2", upscale, vk::Format::R16G16B16A16_SFLOAT)?,
            ],
            spd_mips,
            farthest_depth_mip1: image("fsr3-farthest-depth-mip1", half, vk::Format::R16_SFLOAT)?,
            luma_history: [
                image(
                    "fsr3-luma-history-1",
                    render,
                    vk::Format::R16G16B16A16_SFLOAT,
                )?,
                image(
                    "fsr3-luma-history-2",
                    render,
                    vk::Format::R16G16B16A16_SFLOAT,
                )?,
            ],
            spd_atomic: image("fsr3-spd-atomic", one, vk::Format::R32_UINT)?,
            dilated_reactive_masks: image(
                "fsr3-dilated-reactive-masks",
                render,
                vk::Format::R8G8B8A8_UNORM,
            )?,
            frame_info: image("fsr3-frame-info", one, vk::Format::R32G32B32A32_SFLOAT)?,
            dilated_depth: image("fsr3-dilated-depth", render, vk::Format::R32_SFLOAT)?,
            dilated_motion: image("fsr3-dilated-motion", render, vk::Format::R16G16_SFLOAT)?,
            reconstructed_previous_depth: image(
                "fsr3-reconstructed-previous-depth",
                render,
                vk::Format::R32_UINT,
            )?,
        })
    }

    /// Every image, for the first frame's layout transitions and the memory total.
    fn all(&self) -> Vec<&GpuImage> {
        let mut all: Vec<&GpuImage> = vec![
            &self.intermediate,
            &self.shading_change,
            &self.new_locks,
            &self.spd_mips,
            &self.farthest_depth_mip1,
            &self.spd_atomic,
            &self.dilated_reactive_masks,
            &self.frame_info,
            &self.dilated_depth,
            &self.dilated_motion,
            &self.reconstructed_previous_depth,
        ];
        for pair in [
            &self.accumulation,
            &self.luma,
            &self.upscaled,
            &self.luma_history,
        ] {
            all.extend(pair.iter());
        }
        all
    }

    /// The view behind `slot`.
    fn view(&self, slot: Internal) -> vk::ImageView {
        let image = match slot {
            Internal::Accumulation(index) => &self.accumulation[index],
            Internal::Luma(index) => &self.luma[index],
            Internal::Intermediate => &self.intermediate,
            Internal::ShadingChange => &self.shading_change,
            Internal::NewLocks => &self.new_locks,
            Internal::Upscaled(index) => &self.upscaled[index],
            Internal::SpdMips => &self.spd_mips,
            Internal::SpdMip(level) => return self.spd_mip_views[level].view,
            Internal::FarthestDepthMip1 => &self.farthest_depth_mip1,
            Internal::LumaHistory(index) => &self.luma_history[index],
            Internal::SpdAtomic => &self.spd_atomic,
            Internal::DilatedReactiveMasks => &self.dilated_reactive_masks,
            Internal::FrameInfo => &self.frame_info,
            Internal::DilatedDepth => &self.dilated_depth,
            Internal::DilatedMotion => &self.dilated_motion,
            Internal::ReconstructedPreviousDepth => &self.reconstructed_previous_depth,
        };
        image.view()
    }
}

/// One internal image (or SPD mip view); a pair's index is `[0]` or `[1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Internal {
    Accumulation(usize),
    Luma(usize),
    Intermediate,
    ShadingChange,
    NewLocks,
    Upscaled(usize),
    SpdMips,
    SpdMip(usize),
    FarthestDepthMip1,
    LumaHistory(usize),
    SpdAtomic,
    DilatedReactiveMasks,
    FrameInfo,
    DilatedDepth,
    DilatedMotion,
    ReconstructedPreviousDepth,
}

/// The internal resource behind the vendor binding `name` in a frame of parity `odd`
/// (`fsr3upscalerDispatch`'s per-frame resource table).
fn internal(name: &str, odd: bool) -> Option<Internal> {
    let (this, other) = (usize::from(odd), usize::from(!odd));
    Some(match name {
        "r_accumulation" => Internal::Accumulation(this),
        "rw_accumulation" => Internal::Accumulation(other),
        "r_current_luma" | "rw_current_luma" => Internal::Luma(this),
        "r_previous_luma" => Internal::Luma(other),
        // The farthest depth and the luma instability alias one image.
        "r_farthest_depth" | "rw_farthest_depth" | "r_luma_instability" | "rw_luma_instability" => {
            Internal::Intermediate
        }
        "r_shading_change" | "rw_shading_change" => Internal::ShadingChange,
        "rw_new_locks" => Internal::NewLocks,
        "r_internal_upscaled_color" => Internal::Upscaled(this),
        "rw_internal_upscaled_color" | "r_rcas_input" => Internal::Upscaled(other),
        "r_spd_mips" => Internal::SpdMips,
        "r_farthest_depth_mip1" | "rw_farthest_depth_mip1" => Internal::FarthestDepthMip1,
        "r_luma_history" => Internal::LumaHistory(this),
        "rw_luma_history" => Internal::LumaHistory(other),
        "rw_spd_global_atomic" => Internal::SpdAtomic,
        "r_dilated_reactive_masks" | "rw_dilated_reactive_masks" => Internal::DilatedReactiveMasks,
        "r_frame_info" | "rw_frame_info" => Internal::FrameInfo,
        "r_dilated_depth" | "rw_dilated_depth" => Internal::DilatedDepth,
        "r_dilated_motion_vectors" | "rw_dilated_motion_vectors" => Internal::DilatedMotion,
        "r_reconstructed_previous_nearest_depth" | "rw_reconstructed_previous_nearest_depth" => {
            Internal::ReconstructedPreviousDepth
        }
        _ => {
            let level = name.strip_prefix("rw_spd_mip")?.parse::<usize>().ok()?;
            if level >= SPD_STORAGE_MIPS as usize {
                return None;
            }
            Internal::SpdMip(level)
        }
    })
}

/// The FSR 3.1.6 upscaler for one maximum render size and output size (ADR-0021 §4a shape).
pub(crate) struct Fsr3 {
    device: ash::Device,
    max_render: vk::Extent2D,
    max_output: vk::Extent2D,
    /// Host-visible: two parity slots of the three constant blocks.
    uniforms: GpuBuffer,
    /// Bytes from one constant block to the next (aligned for uniform offsets).
    block_stride: u64,
    constants: UpscalerConstants,
    /// `resourceFrameIndex`.
    frame_index: u32,
    first_frame: bool,
    reset_requested: bool,
    bound: [Option<Bound>; 2],
    // Destroyed in field order: the pipelines and their sets' pool before the images and the
    // sampler the sets point to.
    passes: Vec<PassPipeline>,
    _pool: DescriptorPool,
    linear_clamp: Sampler,
    images: Images,
}

impl Fsr3 {
    /// A context for render sizes up to `max_render` and outputs up to `max_output`.
    ///
    /// # Errors
    /// The device cannot run FSR 3.1 ([`unavailable`]: the caller falls back to TAAU), or a
    /// Vulkan or allocation error.
    pub(crate) fn new(
        device: &DeviceContext,
        max_render: vk::Extent2D,
        max_output: vk::Extent2D,
    ) -> RenderResult<Self> {
        if let Some(missing) = unavailable(device) {
            return Err(RenderError::NoSuitableDevice(format!(
                "FSR 3.1 needs {missing} on {}",
                device.name
            )));
        }
        assert!(
            max_render.width > 0
                && max_render.height > 0
                && max_render.width <= max_output.width
                && max_render.height <= max_output.height,
            "FSR 3.1 upscales: 0 < render <= output"
        );
        let images = Images::new(device, max_render, max_output)?;
        let linear_clamp = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE),
        )?;
        let alignment = device.min_uniform_offset_alignment.max(16);
        let block_stride = (shaders::CB_FSR3_UPSCALER_SIZE as u64).next_multiple_of(alignment);
        let uniforms = GpuBuffer::new(
            device.device.clone(),
            device.allocator(),
            "fsr3-constants",
            block_stride * 3 * 2,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            MemoryLocation::CpuToGpu,
        )?;

        let mut pool_sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
        for pass in shaders::PASSES {
            for binding in pass.bindings.iter().filter(|binding| binding.used) {
                let ty = descriptor_type(binding.kind);
                match pool_sizes.iter_mut().find(|size| size.ty == ty) {
                    Some(size) => size.descriptor_count += 2,
                    None => pool_sizes.push(vk::DescriptorPoolSize {
                        ty,
                        descriptor_count: 2,
                    }),
                }
            }
        }
        let pass_count = u32::try_from(shaders::PASSES.len()).unwrap_or(u32::MAX);
        let pool = DescriptorPool::new(device.device.clone(), &pool_sizes, pass_count * 2)?;
        let mut passes = Vec::with_capacity(shaders::PASSES.len());
        for pass in shaders::PASSES {
            passes.push(PassPipeline::new(device, pass, &pool)?);
        }

        Ok(Self {
            device: device.device.clone(),
            max_render,
            max_output,
            uniforms,
            block_stride,
            constants: UpscalerConstants::new(
                [max_render.width, max_render.height],
                [max_output.width, max_output.height],
            ),
            frame_index: 0,
            first_frame: true,
            reset_requested: false,
            bound: [None; 2],
            passes,
            _pool: pool,
            linear_clamp,
            images,
        })
    }

    /// `"fsr3"`.
    #[allow(clippy::unused_self)] // The shape of ADR-0021 §4a's trait method.
    pub(crate) fn kind(&self) -> &'static str {
        KIND
    }

    /// The render sizes this context accepts for `output` in `mode`: from Ultra Performance's
    /// size (the smallest ratio FSR 3.1 documents, and the dynamic-resolution floor's lowest
    /// mode, ADR-0021 §4c) up to the mode's size, both clamped to the context's maximum. Any
    /// size in between works (§4c, amendment 1).
    pub(crate) fn render_size_range(
        &self,
        output: vk::Extent2D,
        mode: Fsr3Mode,
    ) -> (vk::Extent2D, vk::Extent2D) {
        let clamp = |size: vk::Extent2D| vk::Extent2D {
            width: size.width.min(self.max_render.width),
            height: size.height.min(self.max_render.height),
        };
        (
            clamp(Fsr3Mode::UltraPerformance.render_size(output)),
            clamp(mode.render_size(output)),
        )
    }

    /// The jitter phase count for these sizes (ADR-0021 §1).
    #[allow(clippy::unused_self)] // The shape of ADR-0021 §4a's trait method.
    pub(crate) fn jitter_phases(&self, render: vk::Extent2D, output: vk::Extent2D) -> u32 {
        jitter_phases(render, output)
    }

    /// FSR sharpens with RCAS itself, so the renderer's `sharpen` pass does not run.
    #[allow(clippy::unused_self)] // The shape of ADR-0021 §4a's trait method.
    pub(crate) fn has_sharpening(&self) -> bool {
        true
    }

    /// Whether `record` takes this render size and output size.
    pub(crate) fn accepts(&self, render: vk::Extent2D, output: vk::Extent2D) -> bool {
        render.width > 0
            && render.height > 0
            && render.width <= self.max_render.width
            && render.height <= self.max_render.height
            && output.width <= self.max_output.width
            && output.height <= self.max_output.height
            && render.width <= output.width
            && render.height <= output.height
    }

    /// Forgets every history: the next frame starts from its own sample.
    pub(crate) fn reset_history(&mut self) {
        self.reset_requested = true;
    }

    /// Device memory the context owns: its images and constant buffer.
    pub(crate) fn vram_bytes(&self) -> u64 {
        self.images
            .all()
            .iter()
            .map(|image| image.allocated_bytes())
            .sum::<u64>()
            + self.uniforms.allocated_bytes()
    }

    /// Records one upscale into `cmd`: the clears, then prepare inputs, the luma and
    /// shading-change pyramids, shading change, prepare reactivity, luma instability, and
    /// accumulate (or accumulate with sharpening and RCAS), each after a compute barrier.
    ///
    /// Preconditions (the render graph's, ADR-0021 §4a): the inputs are in
    /// `SHADER_READ_ONLY_OPTIMAL` and the output in `GENERAL`, with their earlier writes
    /// visible to compute; at most two frames are in flight, so the descriptor sets and
    /// constant slot of this frame's parity are not in use by the GPU; the caller makes the
    /// output's compute writes visible to whatever reads it next.
    ///
    /// # Panics
    /// When [`Self::accepts`] rejects the sizes.
    #[allow(clippy::too_many_lines)] // The SDK's dispatch, in order.
    pub(crate) fn record(&mut self, cmd: vk::CommandBuffer, inputs: &Fsr3Inputs, out: &Fsr3Output) {
        let render = inputs.render_size;
        let output = out.image.extent;
        assert!(
            self.accepts(render, output),
            "FSR 3.1: render {render:?} / output {output:?} outside the context's \
             {:?} / {:?}",
            self.max_render,
            self.max_output
        );
        let odd = self.frame_index & 1 == 1;
        let parity = usize::from(odd);
        let first = self.first_frame;
        let reset = inputs.reset || self.reset_requested || first;
        self.first_frame = false;
        self.reset_requested = false;

        // Constants.
        self.constants.advance(
            inputs,
            [output.width, output.height],
            reset,
            jitter_phases(render, output),
        );
        let (spd, spd_groups) = SpdConstants::new([render.width, render.height]);
        let rcas = RcasConstants::new(inputs.sharpness);
        let slot = self.slot_offset(parity, Block::Upscaler);
        self.uniforms
            .write_bytes(slot as usize, bytemuck::bytes_of(&self.constants));
        let slot = self.slot_offset(parity, Block::Spd);
        self.uniforms
            .write_bytes(slot as usize, bytemuck::bytes_of(&spd));
        let slot = self.slot_offset(parity, Block::Rcas);
        self.uniforms
            .write_bytes(slot as usize, bytemuck::bytes_of(&rcas));

        let bound = Bound {
            inputs: [
                inputs.color.view,
                inputs.depth.view,
                inputs.motion.view,
                inputs.reactive.view,
                inputs.transparency.view,
                inputs.exposure.view,
            ],
            output: out.image.view,
        };
        if self.bound[parity] != Some(bound) {
            self.write_sets(parity, odd, inputs, out);
            self.bound[parity] = Some(bound);
        }

        // Clears. The first frame moves every internal image to GENERAL; later frames wait for
        // the previous frame's compute work on them.
        if first {
            let barriers: Vec<vk::ImageMemoryBarrier2> = self
                .images
                .all()
                .iter()
                .map(|image| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::NONE)
                        .src_access_mask(vk::AccessFlags2::NONE)
                        .dst_stage_mask(
                            vk::PipelineStageFlags2::CLEAR
                                | vk::PipelineStageFlags2::COMPUTE_SHADER,
                        )
                        .dst_access_mask(
                            vk::AccessFlags2::TRANSFER_WRITE
                                | vk::AccessFlags2::SHADER_SAMPLED_READ
                                | vk::AccessFlags2::SHADER_STORAGE_READ
                                | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                        )
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .image(image.image())
                        .subresource_range(color_range(0, vk::REMAINING_MIP_LEVELS))
                })
                .collect();
            self.barrier(cmd, &[], &barriers);
        } else {
            self.memory_barrier(
                cmd,
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::CLEAR | vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::TRANSFER_WRITE
                        | vk::AccessFlags2::SHADER_SAMPLED_READ
                        | vk::AccessFlags2::SHADER_STORAGE_READ
                        | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
            );
        }
        let zero = vk::ClearColorValue { float32: [0.0; 4] };
        if first {
            for image in self.images.accumulation.iter().chain(&self.images.luma) {
                self.clear(cmd, image, zero);
            }
        }
        if reset {
            // The first frame cleared both accumulation images just above (clearing one twice
            // would be an unordered write-after-write).
            if !first {
                self.clear(cmd, &self.images.accumulation[parity], zero);
            }
            self.clear(
                cmd,
                &self.images.frame_info,
                vk::ClearColorValue {
                    float32: [-1.0, 1.0, 0.0, 0.0],
                },
            );
        }
        // Every frame: the reconstructed depth starts at the far plane (the SDK's Vulkan back
        // end clears this R32_UINT image with the float 1.0's bits), the SPD counter at 0, and
        // the SPD mips at 0.
        self.clear(
            cmd,
            &self.images.reconstructed_previous_depth,
            vk::ClearColorValue {
                uint32: [1.0f32.to_bits(); 4],
            },
        );
        self.clear(
            cmd,
            &self.images.spd_atomic,
            vk::ClearColorValue { uint32: [0; 4] },
        );
        self.clear(cmd, &self.images.spd_mips, zero);
        self.memory_barrier(
            cmd,
            (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            (
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_SAMPLED_READ
                    | vk::AccessFlags2::SHADER_STORAGE_READ
                    | vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
        );

        // The passes.
        let groups = |size: u32, tile: u32| size.div_ceil(tile);
        let src = [groups(render.width, TILE), groups(render.height, TILE)];
        let dst = [groups(output.width, TILE), groups(output.height, TILE)];
        let shading = [
            groups(((render.width as f32) * 0.5) as u32, TILE),
            groups(((render.height as f32) * 0.5) as u32, TILE),
        ];
        let sharpen = inputs.sharpness > 0.0;
        let mut sequence: Vec<(&'static str, [u32; 2])> = vec![
            (shaders::PREPARE_INPUTS.name, src),
            (shaders::LUMA_PYRAMID.name, spd_groups),
            (shaders::SHADING_CHANGE_PYRAMID.name, spd_groups),
            (shaders::SHADING_CHANGE.name, shading),
            (shaders::PREPARE_REACTIVITY.name, src),
            (shaders::LUMA_INSTABILITY.name, src),
        ];
        if sharpen {
            sequence.push((shaders::ACCUMULATE_SHARPEN.name, dst));
            sequence.push((
                shaders::RCAS.name,
                [
                    groups(output.width, RCAS_TILE),
                    groups(output.height, RCAS_TILE),
                ],
            ));
        } else {
            sequence.push((shaders::ACCUMULATE.name, dst));
        }
        for (index, (name, [x, y])) in sequence.into_iter().enumerate() {
            if index > 0 {
                self.memory_barrier(
                    cmd,
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_SAMPLED_READ
                            | vk::AccessFlags2::SHADER_STORAGE_READ
                            | vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                );
            }
            let pass = self
                .passes
                .iter()
                .find(|pass| pass.pass.name == name)
                .expect("every pass has a pipeline");
            // SAFETY: the command buffer is recording and the pipeline is live.
            unsafe {
                self.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pass.pipeline.handle(),
                );
            }
            // SAFETY: the layout and the set are live; the set was written for this parity
            // and is not in use by the GPU (the preconditions), and every image it names is
            // in the layout its descriptor expects (internal ones in GENERAL since the first
            // frame).
            unsafe {
                self.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pass.layout.handle(),
                    0,
                    &[pass.sets[parity]],
                    &[],
                );
            }
            // SAFETY: a compute pipeline and its set are bound; the counts are at least 1.
            unsafe { self.device.cmd_dispatch(cmd, x.max(1), y.max(1), 1) };
        }

        self.frame_index = (self.frame_index + 1) % MAX_QUEUED_FRAMES;
    }

    fn slot_offset(&self, parity: usize, block: Block) -> u64 {
        let index = parity as u64 * 3
            + match block {
                Block::Upscaler => 0,
                Block::Spd => 1,
                Block::Rcas => 2,
            };
        index * self.block_stride
    }

    /// Writes every descriptor of parity `parity`'s sets.
    fn write_sets(&self, parity: usize, odd: bool, inputs: &Fsr3Inputs, out: &Fsr3Output) {
        let mut image_infos: Vec<[vk::DescriptorImageInfo; 1]> = Vec::new();
        let mut buffer_infos: Vec<[vk::DescriptorBufferInfo; 1]> = Vec::new();
        // (set, binding, type, image info index or buffer info index).
        let mut targets: Vec<(vk::DescriptorSet, u32, vk::DescriptorType, bool, usize)> =
            Vec::new();
        for pass in &self.passes {
            for binding in pass.pass.bindings.iter().filter(|binding| binding.used) {
                let ty = descriptor_type(binding.kind);
                let source = self.source(binding.name, odd, inputs, out);
                let (is_image, index) = match source {
                    Source::Uniform(block) => {
                        let size = match block {
                            Block::Upscaler => shaders::CB_FSR3_UPSCALER_SIZE,
                            Block::Spd => shaders::CB_SPD_SIZE,
                            Block::Rcas => shaders::CB_RCAS_SIZE,
                        };
                        buffer_infos.push([vk::DescriptorBufferInfo {
                            buffer: self.uniforms.handle(),
                            offset: self.slot_offset(parity, block),
                            range: size as u64,
                        }]);
                        (false, buffer_infos.len() - 1)
                    }
                    Source::Image(view) => {
                        let layout = if binding.kind == Kind::Sampled && is_input(binding.name) {
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                        } else {
                            vk::ImageLayout::GENERAL
                        };
                        image_infos.push([vk::DescriptorImageInfo {
                            sampler: vk::Sampler::null(),
                            image_view: view,
                            image_layout: layout,
                        }]);
                        (true, image_infos.len() - 1)
                    }
                    Source::LinearClamp => {
                        image_infos.push([vk::DescriptorImageInfo {
                            sampler: self.linear_clamp.handle(),
                            image_view: vk::ImageView::null(),
                            image_layout: vk::ImageLayout::UNDEFINED,
                        }]);
                        (true, image_infos.len() - 1)
                    }
                };
                targets.push((pass.sets[parity], binding.binding, ty, is_image, index));
            }
        }
        let writes: Vec<vk::WriteDescriptorSet> = targets
            .iter()
            .map(|&(set, binding, ty, is_image, index)| {
                let write = vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(binding)
                    .descriptor_type(ty);
                if is_image {
                    write.image_info(&image_infos[index])
                } else {
                    write.buffer_info(&buffer_infos[index])
                }
            })
            .collect();
        // SAFETY: every set, buffer, view and sampler is live; the sets of this parity are not
        // in use by the GPU (the record precondition), and each write matches its binding's
        // type in the set layout.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }

    /// The resource behind the vendor binding `name` this frame.
    fn source(&self, name: &str, odd: bool, inputs: &Fsr3Inputs, out: &Fsr3Output) -> Source {
        match name {
            "cbFSR3Upscaler" => Source::Uniform(Block::Upscaler),
            "cbSPD" => Source::Uniform(Block::Spd),
            "cbRCAS" => Source::Uniform(Block::Rcas),
            "s_LinearClamp" => Source::LinearClamp,
            "r_input_color_jittered" => Source::Image(inputs.color.view),
            "r_input_depth" => Source::Image(inputs.depth.view),
            "r_input_motion_vectors" => Source::Image(inputs.motion.view),
            "r_reactive_mask" => Source::Image(inputs.reactive.view),
            "r_transparency_and_composition_mask" => Source::Image(inputs.transparency.view),
            "r_input_exposure" => Source::Image(inputs.exposure.view),
            "rw_upscaled_output" => Source::Image(out.image.view),
            _ => Source::Image(
                self.images.view(
                    internal(name, odd)
                        .unwrap_or_else(|| panic!("FSR 3.1 binding `{name}` has no resource")),
                ),
            ),
        }
    }

    fn clear(&self, cmd: vk::CommandBuffer, image: &GpuImage, value: vk::ClearColorValue) {
        let range = color_range(0, vk::REMAINING_MIP_LEVELS);
        // SAFETY: the command buffer is recording; the image is live, created with
        // TRANSFER_DST, and in GENERAL (moved there by this frame's or the first frame's
        // barrier).
        unsafe {
            self.device.cmd_clear_color_image(
                cmd,
                image.image(),
                vk::ImageLayout::GENERAL,
                &value,
                core::slice::from_ref(&range),
            );
        }
    }

    fn memory_barrier(
        &self,
        cmd: vk::CommandBuffer,
        src: (vk::PipelineStageFlags2, vk::AccessFlags2),
        dst: (vk::PipelineStageFlags2, vk::AccessFlags2),
    ) {
        let barrier = vk::MemoryBarrier2::default()
            .src_stage_mask(src.0)
            .src_access_mask(src.1)
            .dst_stage_mask(dst.0)
            .dst_access_mask(dst.1);
        self.barrier(cmd, core::slice::from_ref(&barrier), &[]);
    }

    fn barrier(
        &self,
        cmd: vk::CommandBuffer,
        memory: &[vk::MemoryBarrier2],
        images: &[vk::ImageMemoryBarrier2],
    ) {
        let info = vk::DependencyInfo::default()
            .memory_barriers(memory)
            .image_memory_barriers(images);
        // SAFETY: the command buffer is recording, the barriers' images are live, and
        // `synchronization2` is enabled on every device.
        unsafe { self.device.cmd_pipeline_barrier2(cmd, &info) };
    }
}

impl PassPipeline {
    fn new(
        device: &DeviceContext,
        pass: &'static Pass,
        pool: &DescriptorPool,
    ) -> RenderResult<Self> {
        let bindings: Vec<vk::DescriptorSetLayoutBinding> = pass
            .bindings
            .iter()
            .filter(|binding| binding.used)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding.binding)
                    .descriptor_type(descriptor_type(binding.kind))
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[set_layout.handle()],
            &[],
        )?;
        let module = ShaderModule::new(device.device.clone(), pass.spirv)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module.handle())
            .name(c"CS");
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout.handle());
        let pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            device.create_compute_pipeline(&info)?,
        );
        let sets = pool.allocate(&device.device, &[set_layout.handle(); 2])?;
        Ok(Self {
            pass,
            pipeline,
            layout,
            _set_layout: set_layout,
            sets: [sets[0], sets[1]],
        })
    }
}

fn descriptor_type(kind: Kind) -> vk::DescriptorType {
    match kind {
        Kind::Constants => vk::DescriptorType::UNIFORM_BUFFER,
        Kind::Sampled => vk::DescriptorType::SAMPLED_IMAGE,
        Kind::Storage => vk::DescriptorType::STORAGE_IMAGE,
        Kind::Sampler => vk::DescriptorType::SAMPLER,
    }
}

/// The caller's sampled inputs, which are in `SHADER_READ_ONLY_OPTIMAL`.
fn is_input(name: &str) -> bool {
    matches!(
        name,
        "r_input_color_jittered"
            | "r_input_depth"
            | "r_input_motion_vectors"
            | "r_reactive_mask"
            | "r_transparency_and_composition_mask"
            | "r_input_exposure"
    )
}

fn color_range(base_mip: u32, levels: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(base_mip)
        .level_count(levels)
        .base_array_layer(0)
        .layer_count(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every used binding of every pass resolves to a resource in both parities, so
    /// `write_sets` never panics, and the parity swaps exactly the history pairs.
    #[test]
    fn every_used_binding_has_a_source() {
        for pass in shaders::PASSES {
            for binding in pass.bindings.iter().filter(|binding| binding.used) {
                let name = binding.name;
                let external = matches!(name, "cbFSR3Upscaler" | "cbSPD" | "cbRCAS")
                    || name == "s_LinearClamp"
                    || name == "rw_upscaled_output"
                    || is_input(name);
                assert!(
                    external || (internal(name, false).is_some() && internal(name, true).is_some()),
                    "{}: `{name}` has no source",
                    pass.name
                );
                if binding.kind == Kind::Sampler {
                    assert_eq!(name, "s_LinearClamp", "only the linear sampler is used");
                }
            }
        }
        // Even frames read pair [0] and write [1]; odd frames the reverse.
        assert_eq!(
            internal("r_accumulation", false),
            Some(Internal::Accumulation(0))
        );
        assert_eq!(
            internal("rw_accumulation", false),
            Some(Internal::Accumulation(1))
        );
        assert_eq!(
            internal("r_internal_upscaled_color", true),
            Some(Internal::Upscaled(1))
        );
        assert_eq!(internal("r_rcas_input", true), Some(Internal::Upscaled(0)));
        assert_eq!(internal("r_previous_luma", true), Some(Internal::Luma(0)));
        assert_eq!(internal("rw_current_luma", true), Some(Internal::Luma(1)));
        assert_eq!(internal("rw_spd_mip5", false), Some(Internal::SpdMip(5)));
        assert_eq!(internal("rw_spd_mip6", false), None);
        assert_eq!(internal("r_lanczos_lut", false), None);
    }
}
