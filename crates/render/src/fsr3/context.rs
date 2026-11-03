//! The FSR 3.1.6 context: internal resources, pipelines, descriptor sets and the recorded
//! dispatch sequence (a port of `fsr3upscalerDispatch` and the parts of the SDK's Vulkan back
//! end it drives).

use ash::vk;
use ash::vk::TaggedStructure as _;
use gpu_allocator::MemoryLocation;
use moba_shaders::fsr3::{self as shaders, Kind, Pass};

use super::constants::{RcasConstants, SpdConstants, UpscalerConstants};
use super::{Fsr3Mode, jitter_phases};
use crate::caps::RenderPath;
use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    Sampler, ShaderModule,
};
use crate::upscale::{
    Extent, TemporalUpscaler, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind,
};

/// Frame-in-flight slots: each pass keeps a descriptor set per slot and history parity, and
/// the constants a block per slot, chosen by `UpscaleInputs::frame_slot` (ADR-0021 amendment
/// 2, item 17). The caller records a slot only after that slot's last submission finished,
/// so a set or constant block is never rewritten while a pending frame reads it, however many
/// recorded frames were dropped unsubmitted. The renderer asserts `SET_RING >=
/// FRAMES_IN_FLIGHT`.
pub(crate) const SET_RING: usize = 2;

/// `FSR3UPSCALER_MAX_QUEUED_FRAMES`: the resource frame index runs modulo this (even) count;
/// its parity picks the history images.
const MAX_QUEUED_FRAMES: u32 = 16;
/// Work-group tile of the 8×8 passes and of RCAS.
const TILE: u32 = 8;
const RCAS_TILE: u32 = 16;
/// The SPD mip levels the pyramids write (`rw_spd_mip0..5`).
const SPD_STORAGE_MIPS: u32 = 6;

/// The passes by their index in [`shaders::PASSES`] (a test holds the names to it).
const PREPARE_INPUTS: usize = 0;
const LUMA_PYRAMID: usize = 1;
const SHADING_CHANGE_PYRAMID: usize = 2;
const SHADING_CHANGE: usize = 3;
const PREPARE_REACTIVITY: usize = 4;
const LUMA_INSTABILITY: usize = 5;
const ACCUMULATE: usize = 6;
const ACCUMULATE_SHARPEN: usize = 7;
const RCAS: usize = 8;
const PASS_COUNT: usize = 9;
const _: () = assert!(shaders::PASSES.len() == PASS_COUNT);

/// At most this many descriptors name a caller's image, over every pass (checked when the
/// context is created): the per-frame writes live in fixed arrays of this size.
const MAX_EXTERNAL_WRITES: usize = 32;

/// Storage-image formats and the format features FSR needs on each (Vulkan 1.3 format
/// features 2), from the passes' SPIR-V:
///
/// - Every float storage image is declared without a format (`Unknown`) and written, so each
///   format needs `STORAGE_WRITE_WITHOUT_FORMAT`. Two are also read back through a storage
///   view without a format, so they need `STORAGE_READ_WITHOUT_FORMAT`: `RGBA32F`
///   (`rw_frame_info`) and `R16G16_SFLOAT` (`rw_spd_mip5`). `rw_new_locks` is declared `R8`
///   and `rw_dilated_reactive_masks` `Rgba8`, so their reads need no format feature.
/// - `R32_UINT` is declared `R32ui` and takes atomics.
/// - Images sampled through `s_LinearClamp` need `SAMPLED_IMAGE_FILTER_LINEAR`: every format
///   here but `R32_SFLOAT` (the dilated depth is only loaded), `RGBA32F` (the 1×1 frame info,
///   loaded) and `R32_UINT`.
///
/// The caller's `RGBA16F` output is in the list (written without a format).
const FORMAT_NEEDS: &[(vk::Format, vk::FormatFeatureFlags2)] = &[
    (vk::Format::R8_UNORM, FILTERED),
    (vk::Format::R16_SFLOAT, FILTERED),
    (
        vk::Format::R16G16_SFLOAT,
        vk::FormatFeatureFlags2::from_raw(FILTERED.as_raw() | READ.as_raw()),
    ),
    (vk::Format::R16G16B16A16_SFLOAT, FILTERED),
    (vk::Format::R32_SFLOAT, WRITE),
    (vk::Format::R8G8B8A8_UNORM, FILTERED),
    (
        vk::Format::R32G32B32A32_SFLOAT,
        vk::FormatFeatureFlags2::from_raw(WRITE.as_raw() | READ.as_raw()),
    ),
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
/// A storage image written without a format, sampled, and cleared.
const WRITE: vk::FormatFeatureFlags2 = vk::FormatFeatureFlags2::from_raw(
    vk::FormatFeatureFlags2::STORAGE_IMAGE.as_raw()
        | vk::FormatFeatureFlags2::STORAGE_WRITE_WITHOUT_FORMAT.as_raw()
        | vk::FormatFeatureFlags2::SAMPLED_IMAGE.as_raw()
        | vk::FormatFeatureFlags2::TRANSFER_DST.as_raw(),
);
/// As [`WRITE`], and sampled with linear filtering.
const FILTERED: vk::FormatFeatureFlags2 = vk::FormatFeatureFlags2::from_raw(
    WRITE.as_raw() | vk::FormatFeatureFlags2::SAMPLED_IMAGE_FILTER_LINEAR.as_raw(),
);
/// Read through a storage view without a format.
const READ: vk::FormatFeatureFlags2 = vk::FormatFeatureFlags2::STORAGE_READ_WITHOUT_FORMAT;

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
    // SAFETY: `physical` was enumerated from `instance`, which outlives this call, and the
    // output chain (`properties` → `subgroup`) is two initialised structures borrowed mutably
    // for the call only.
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
        // SAFETY: `physical` was enumerated from `instance`, which outlives this call; the
        // output chain (`properties` → `features`, `VkFormatProperties3` is core in Vulkan
        // 1.3) is initialised and borrowed mutably for the call only.
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

/// Where one internal descriptor comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Uniform(Block),
    Image(Internal),
    LinearClamp,
}

/// The three constant blocks of one frame slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Upscaler,
    Spd,
    Rcas,
}

/// A caller's image: one of [`UpscaleInputs`]' or the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum External {
    Color,
    Depth,
    Motion,
    Reactive,
    Transparency,
    Exposure,
    Output,
}

impl External {
    /// The caller's image behind the vendor binding `name`, if it names one.
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "r_input_color_jittered" => Self::Color,
            "r_input_depth" => Self::Depth,
            "r_input_motion_vectors" => Self::Motion,
            "r_reactive_mask" => Self::Reactive,
            "r_transparency_and_composition_mask" => Self::Transparency,
            "r_input_exposure" => Self::Exposure,
            "rw_upscaled_output" => Self::Output,
            _ => return None,
        })
    }

    fn view(self, inputs: &UpscaleInputs, out: &UpscaleOutput) -> vk::ImageView {
        match self {
            Self::Color => inputs.color.view,
            Self::Depth => inputs.depth.view,
            Self::Motion => inputs.motion.view,
            Self::Reactive => inputs.reactive.view,
            Self::Transparency => inputs.transparency.view,
            Self::Exposure => inputs.exposure.view,
            Self::Output => out.image.view,
        }
    }

    /// The inputs are sampled in `SHADER_READ_ONLY_OPTIMAL`; the output is a storage image in
    /// `GENERAL` (`record`'s preconditions).
    fn layout(self) -> vk::ImageLayout {
        match self {
            Self::Output => vk::ImageLayout::GENERAL,
            _ => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }
    }
}

/// One descriptor that names a caller's image: rewritten in this frame's sets on every
/// `record`, so a view the caller recreated (even one whose handle value is reused) is never
/// read through a stale descriptor.
#[derive(Clone, Copy, Debug)]
struct ExternalBinding {
    pass: usize,
    binding: u32,
    ty: vk::DescriptorType,
    external: External,
}

/// One pass's pipeline and its descriptor sets, by frame slot and history parity.
struct PassPipeline {
    // Destroyed in field order: the pipeline before its layouts.
    pipeline: GraphicsPipeline,
    layout: PipelineLayout,
    _set_layout: DescriptorSetLayout,
    sets: [[vk::DescriptorSet; 2]; SET_RING],
}

/// One internal image with its format and the value the first frame clears it to.
struct Entry<'a> {
    image: &'a GpuImage,
    format: vk::Format,
    initial: vk::ClearColorValue,
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
        // SAFETY: the view was created on this device and is destroyed once, before its image
        // (`Images` declares the views first). The GPU no longer uses it: `Fsr3`'s owner idles
        // the context's work before dropping it or calling `resize` (both documented).
        unsafe { self.device.destroy_image_view(self.view, None) };
    }
}

/// The reconstructed depth's far plane: the SDK's Vulkan back end clears this `R32_UINT`
/// image with the bits of the float 0.0 for an inverted (reversed-Z, ADR-0023 §1c) depth,
/// whose reconstruction stores with `InterlockedMax` (1.0 for a non-inverted one).
const FAR_DEPTH_BITS: vk::ClearColorValue = vk::ClearColorValue { uint32: [0; 4] };
const ZERO: vk::ClearColorValue = vk::ClearColorValue { float32: [0.0; 4] };
const ZERO_UINT: vk::ClearColorValue = vk::ClearColorValue { uint32: [0; 4] };
/// The frame info after a reset: no previous exposure (−1), pre-exposure 1.
const FRAME_INFO_RESET: vk::ClearColorValue = vk::ClearColorValue {
    float32: [-1.0, 1.0, 0.0, 0.0],
};

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
            // A maximum render size below 64 gives the half-size pyramid fewer than six
            // levels; the views of the missing ones alias the last level so every pass's
            // `rw_spd_mip0..5` stays bound. They are never written: SPD writes level `i` only
            // for `i < floor(log2(max(render)))` (`SpdConstants`, `ffxSpdSetup`), which is at
            // most `spd_mip_levels` for any render size up to the maximum, and its
            // cross-work-group exchange through level 5 runs only above six mips. (The SDK
            // creates a view per existing level and assumes six.)
            let info = vk::ImageViewCreateInfo::default()
                .image(spd_mips.image())
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R16G16_SFLOAT)
                .subresource_range(color_range(level.min(spd_mip_levels - 1), 1));
            // SAFETY: `spd_mips` is live on this device with `level.min(levels - 1)` in range,
            // and the create info is fully initialised with no extension chain.
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

    /// Every image with its format and first-frame value (the first frame's transitions and
    /// clears, the poison hook, the memory total). Zero everywhere but the reconstructed depth
    /// (the far plane) and the frame info (reset). The SDK leaves most of these uninitialised;
    /// clearing all of them once costs nothing per frame and means no pass can read garbage on
    /// the first frame (`new_locks`, for one, is only written where a lock forms, and read
    /// everywhere).
    fn all(&self) -> Vec<Entry<'_>> {
        let entry = |image, format, initial| Entry {
            image,
            format,
            initial,
        };
        let mut all = vec![
            entry(&self.intermediate, vk::Format::R16_SFLOAT, ZERO),
            entry(&self.shading_change, vk::Format::R8_UNORM, ZERO),
            entry(&self.new_locks, vk::Format::R8_UNORM, ZERO),
            entry(&self.spd_mips, vk::Format::R16G16_SFLOAT, ZERO),
            entry(&self.farthest_depth_mip1, vk::Format::R16_SFLOAT, ZERO),
            entry(&self.spd_atomic, vk::Format::R32_UINT, ZERO_UINT),
            entry(
                &self.dilated_reactive_masks,
                vk::Format::R8G8B8A8_UNORM,
                ZERO,
            ),
            entry(
                &self.frame_info,
                vk::Format::R32G32B32A32_SFLOAT,
                FRAME_INFO_RESET,
            ),
            entry(&self.dilated_depth, vk::Format::R32_SFLOAT, ZERO),
            entry(&self.dilated_motion, vk::Format::R16G16_SFLOAT, ZERO),
            entry(
                &self.reconstructed_previous_depth,
                vk::Format::R32_UINT,
                FAR_DEPTH_BITS,
            ),
        ];
        for (pair, format) in [
            (&self.accumulation, vk::Format::R8_UNORM),
            (&self.luma, vk::Format::R16_SFLOAT),
            (&self.upscaled, vk::Format::R16G16B16A16_SFLOAT),
            (&self.luma_history, vk::Format::R16G16B16A16_SFLOAT),
        ] {
            all.extend(pair.iter().map(|image| entry(image, format, ZERO)));
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

/// The resource behind an internal (not a caller's) vendor binding in a frame of parity `odd`.
fn source(name: &str, odd: bool) -> Source {
    match name {
        "cbFSR3Upscaler" => Source::Uniform(Block::Upscaler),
        "cbSPD" => Source::Uniform(Block::Spd),
        "cbRCAS" => Source::Uniform(Block::Rcas),
        "s_LinearClamp" => Source::LinearClamp,
        _ => Source::Image(
            internal(name, odd)
                .unwrap_or_else(|| panic!("FSR 3.1 binding `{name}` has no resource")),
        ),
    }
}

/// The FSR 3.1.6 upscaler for one maximum render size and output size (ADR-0021 §4a shape).
///
/// Dropping it destroys its Vulkan objects at once, as every renderer pass does: its owner
/// must first make sure the GPU has finished every command buffer it recorded (the renderer
/// idles the device before tearing passes down). Switching upscalers at run time (stage 2)
/// goes through the same idle, or a deferred deletion if one is added then.
pub(crate) struct Fsr3 {
    device: ash::Device,
    max_render: vk::Extent2D,
    max_output: vk::Extent2D,
    /// Host-visible: the three constant blocks per frame slot ([`SET_RING`] of them).
    uniforms: GpuBuffer,
    /// Bytes from one constant block to the next (aligned for uniform offsets).
    block_stride: u64,
    constants: UpscalerConstants,
    /// `resourceFrameIndex`.
    frame_index: u32,
    first_frame: bool,
    /// The internal images are already in `GENERAL` before the first frame (the poison hook).
    poisoned: bool,
    reset_requested: bool,
    /// The images' and constant buffer's allocations, summed when they are created.
    vram_bytes: u64,
    /// The descriptors that name a caller's image, the same in both parities.
    externals: Vec<ExternalBinding>,
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
    ///
    /// # Panics
    /// Unless `0 < max_render <= max_output` on both axes.
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
        check_sizes(max_render, max_output);
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
            block_stride * 3 * SET_RING as u64,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            MemoryLocation::CpuToGpu,
        )?;

        let mut pool_sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
        let mut externals = Vec::new();
        for (index, pass) in shaders::PASSES.iter().enumerate() {
            for binding in pass.bindings.iter().filter(|binding| binding.used) {
                let ty = descriptor_type(binding.kind);
                // One per set: two history parities in each frame slot.
                let sets = 2 * SET_RING as u32;
                match pool_sizes.iter_mut().find(|size| size.ty == ty) {
                    Some(size) => size.descriptor_count += sets,
                    None => pool_sizes.push(vk::DescriptorPoolSize {
                        ty,
                        descriptor_count: sets,
                    }),
                }
                if let Some(external) = External::of(binding.name) {
                    externals.push(ExternalBinding {
                        pass: index,
                        binding: binding.binding,
                        ty,
                        external,
                    });
                }
            }
        }
        assert!(
            externals.len() <= MAX_EXTERNAL_WRITES,
            "FSR 3.1: {} descriptors name a caller's image, over {MAX_EXTERNAL_WRITES}",
            externals.len()
        );
        let pool = DescriptorPool::new(
            device.device.clone(),
            &pool_sizes,
            (PASS_COUNT * 2 * SET_RING) as u32,
        )?;
        let mut passes = Vec::with_capacity(PASS_COUNT);
        for pass in shaders::PASSES {
            passes.push(PassPipeline::new(device, pass, &pool)?);
        }

        let mut fsr = Self {
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
            poisoned: false,
            reset_requested: false,
            vram_bytes: 0,
            externals,
            passes,
            _pool: pool,
            linear_clamp,
            images,
        };
        fsr.write_internal_sets();
        fsr.vram_bytes = fsr.measure_vram();
        Ok(fsr)
    }

    /// Re-creates the internal images for new maximum sizes, keeping the pipelines, the
    /// descriptor pool and sets, the sampler and the constant buffer, and starts over as a new
    /// context (the next frame is a first frame). [`TemporalUpscaler::resize`].
    ///
    /// # Errors
    /// A Vulkan or allocation error; the context is then unchanged. (The new images are
    /// created before the old ones are freed, so both sets are allocated for a moment.)
    ///
    /// # Panics
    /// Unless `0 < max_render <= max_output` on both axes.
    fn reallocate(
        &mut self,
        device: &DeviceContext,
        max_render: vk::Extent2D,
        max_output: vk::Extent2D,
    ) -> RenderResult<()> {
        check_sizes(max_render, max_output);
        self.images = Images::new(device, max_render, max_output)?;
        self.max_render = max_render;
        self.max_output = max_output;
        self.constants = UpscalerConstants::new(
            [max_render.width, max_render.height],
            [max_output.width, max_output.height],
        );
        self.frame_index = 0;
        self.first_frame = true;
        self.poisoned = false;
        self.reset_requested = false;
        self.write_internal_sets();
        self.vram_bytes = self.measure_vram();
        Ok(())
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

    fn measure_vram(&self) -> u64 {
        self.images
            .all()
            .iter()
            .map(|entry| entry.image.allocated_bytes())
            .sum::<u64>()
            + self.uniforms.allocated_bytes()
    }

    /// Test hook for `fsr3_smoke`: records commands that fill every internal image with
    /// garbage (NaN in the float formats, all bits set in the UNORM and UINT ones) before the
    /// context's first frame, so a pass that read an internal image the first frame had not
    /// initialised would show in the output. Leaves the images in `GENERAL`; the first
    /// `record` orders its clears after these.
    ///
    /// # Panics
    /// After the first `record`.
    ///
    /// # Safety
    /// `cmd` is a command buffer of this context's device in the recording state, and the
    /// first `record`'s commands follow these in submission order on the same queue.
    pub(crate) unsafe fn poison(&mut self, cmd: vk::CommandBuffer) {
        assert!(
            self.first_frame,
            "FSR 3.1: poison before the first frame only"
        );
        let entries = self.images.all();
        // As a fresh first frame's transition (also valid after a mid-stream history reset).
        let barriers: Vec<vk::ImageMemoryBarrier2> = entries
            .iter()
            .map(|entry| first_frame_barrier(entry.image.image(), false))
            .collect();
        self.barrier(cmd, &[], &barriers);
        for entry in &entries {
            let garbage = match entry.format {
                vk::Format::R32_UINT => vk::ClearColorValue {
                    uint32: [u32::MAX; 4],
                },
                vk::Format::R8_UNORM | vk::Format::R8G8B8A8_UNORM => {
                    vk::ClearColorValue { float32: [1.0; 4] }
                }
                _ => vk::ClearColorValue {
                    float32: [f32::NAN; 4],
                },
            };
            self.clear(cmd, entry.image, garbage);
        }
        self.poisoned = true;
    }

    /// Records one upscale into `cmd`: the clears, then prepare inputs, the luma and
    /// shading-change pyramids, shading change, prepare reactivity, luma instability, and
    /// accumulate (or accumulate with sharpening and RCAS), each after a compute barrier.
    ///
    /// Preconditions of the render graph (ADR-0021 §4a), on which the result but not memory
    /// safety depends: the inputs are in `SHADER_READ_ONLY_OPTIMAL` and the output in
    /// `GENERAL`, with their earlier writes visible to compute; the caller makes the output's
    /// compute writes visible to whatever reads it next.
    ///
    /// # Panics
    /// When [`Self::accepts`] rejects the sizes.
    ///
    /// # Safety
    /// - `cmd` is a command buffer of this context's device in the recording state.
    /// - **Every command buffer recorded here is submitted exactly once, in `record` order,
    ///   on one queue** (with the poison hook's commands, if any, before the first). The
    ///   context's state assumes it: after the first record the internal images are taken to
    ///   be in `GENERAL` and initialised, and each frame's barriers order it only against the
    ///   previous record's commands earlier on the same queue. Dropping a recorded command
    ///   buffer unsubmitted (or reordering two) leaves later records reading images that were
    ///   never transitioned or cleared; to start over, call
    ///   [`TemporalUpscaler::reset_history`] or `resize` (either makes the next record a
    ///   first frame again, whose re-initialisation waits for every earlier compute and clear
    ///   on the queue) or drop the context.
    /// - **At most [`SET_RING`] records pending, and `inputs.frame_slot`'s last submission
    ///   has finished.** This call rewrites that slot's descriptor sets and constant blocks,
    ///   which that submission used: rewriting them while it is pending is undefined
    ///   behaviour.
    /// - Every image and view in `inputs` and `out` stays alive until this command buffer
    ///   has finished executing.
    #[allow(clippy::too_many_lines)] // The SDK's dispatch, in order.
    unsafe fn record_frame(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        let render = inputs.render_size;
        let output = out.image.extent;
        assert!(
            self.accepts(render, output),
            "FSR 3.1: render {render:?} / output {output:?} outside the context's \
             {:?} / {:?}",
            self.max_render,
            self.max_output
        );
        debug_assert!(
            [
                inputs.color,
                inputs.depth,
                inputs.motion,
                inputs.reactive,
                inputs.transparency,
            ]
            .iter()
            .all(|image| image.extent.width >= render.width
                && image.extent.height >= render.height),
            "FSR 3.1: an input is smaller than the render size {render:?}"
        );
        debug_assert!(
            inputs.exposure.extent.width >= 1 && inputs.exposure.extent.height >= 1,
            "FSR 3.1: the exposure is 1x1"
        );
        debug_assert_eq!(
            out.image.format,
            vk::Format::R16G16B16A16_SFLOAT,
            "FSR 3.1 writes an RGBA16F output"
        );
        let odd = self.frame_index & 1 == 1;
        let parity = usize::from(odd);
        let slot = inputs.frame_slot % SET_RING;
        let first = self.first_frame;
        let reset = inputs.reset || self.reset_requested || first;
        self.first_frame = false;
        self.reset_requested = false;

        // Constants, into this frame slot's blocks (not in use by the GPU: the safety
        // contract).
        self.constants.advance(
            inputs,
            [output.width, output.height],
            reset,
            jitter_phases(render, output),
        );
        let (spd, spd_groups) = SpdConstants::new([render.width, render.height]);
        let rcas = RcasConstants::new(inputs.sharpness);
        let offset = self.slot_offset(slot, Block::Upscaler);
        self.uniforms
            .write_bytes(offset as usize, bytemuck::bytes_of(&self.constants));
        let offset = self.slot_offset(slot, Block::Spd);
        self.uniforms
            .write_bytes(offset as usize, bytemuck::bytes_of(&spd));
        let offset = self.slot_offset(slot, Block::Rcas);
        self.uniforms
            .write_bytes(offset as usize, bytemuck::bytes_of(&rcas));

        // The caller's views, every frame.
        self.write_external_sets(slot, parity, inputs, out);

        // Clears. The first frame moves every internal image to GENERAL (from the poison
        // hook's clears when it ran) and initialises all of them; later frames wait for the
        // previous frame's compute work on them.
        if first {
            let poisoned = self.poisoned;
            self.poisoned = false;
            let entries = self.images.all();
            let barriers: Vec<vk::ImageMemoryBarrier2> = entries
                .iter()
                .map(|entry| first_frame_barrier(entry.image.image(), poisoned))
                .collect();
            self.barrier(cmd, &[], &barriers);
            for entry in &entries {
                self.clear(cmd, entry.image, entry.initial);
            }
        } else {
            // The previous frame's compute writes, to this frame's clears and compute reads
            // and writes (its reads of what this frame clears need only the execution order).
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
            if reset {
                // `fsr3upscalerDispatch`'s reset: this frame's accumulation (its SRV) and the
                // frame info.
                self.clear(cmd, &self.images.accumulation[parity], ZERO);
                self.clear(cmd, &self.images.frame_info, FRAME_INFO_RESET);
            }
            // Every frame, as the SDK does: the reconstructed depth at the far plane, the SPD
            // counter and the SPD mips at 0. Whole images, as the SDK's clear jobs: a clear
            // takes whole subresources, and a compute clear of the frame's rectangle would
            // cost a dispatch and a barrier for a few hundred kilobytes at 1440p Quality.
            self.clear(
                cmd,
                &self.images.reconstructed_previous_depth,
                FAR_DEPTH_BITS,
            );
            self.clear(cmd, &self.images.spd_atomic, ZERO_UINT);
            self.clear(cmd, &self.images.spd_mips, ZERO);
        }
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

        // The passes, in `fsr3upscalerDispatch`'s order. The two SPD pyramids hand data
        // between work groups inside one dispatch (the last group to finish, counted with an
        // atomic on `rw_spd_global_atomic`, reads the others' `globallycoherent` level 5): the
        // upstream SPD pattern, which needs no barrier here.
        let groups = |size: u32, tile: u32| size.div_ceil(tile);
        let src = [groups(render.width, TILE), groups(render.height, TILE)];
        let dst = [groups(output.width, TILE), groups(output.height, TILE)];
        let shading = [
            groups(((render.width as f32) * 0.5) as u32, TILE),
            groups(((render.height as f32) * 0.5) as u32, TILE),
        ];
        let rcas_groups = [
            groups(output.width, RCAS_TILE),
            groups(output.height, RCAS_TILE),
        ];
        let mut sequence: [(usize, [u32; 2]); 8] = [
            (PREPARE_INPUTS, src),
            (LUMA_PYRAMID, spd_groups),
            (SHADING_CHANGE_PYRAMID, spd_groups),
            (SHADING_CHANGE, shading),
            (PREPARE_REACTIVITY, src),
            (LUMA_INSTABILITY, src),
            (ACCUMULATE, dst),
            (RCAS, rcas_groups),
        ];
        // RCAS runs when the sharpness is above 0 (the SDK's `enableSharpening` flag).
        let count = if inputs.sharpness > 0.0 {
            sequence[6].0 = ACCUMULATE_SHARPEN;
            8
        } else {
            7
        };
        for (index, &(pass, [x, y])) in sequence[..count].iter().enumerate() {
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
            let pass = &self.passes[pass];
            // SAFETY: `cmd` is recording (the caller's contract) and the pipeline is a live
            // compute pipeline of this device.
            unsafe {
                self.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pass.pipeline.handle(),
                );
            }
            // SAFETY: `cmd` is recording; the layout and the set are live and compatible.
            // Every descriptor of the set `sets[slot][parity]` is written: the internal ones at
            // creation (their images are in GENERAL from this frame's first-frame barrier on)
            // and the caller's just above, in the layouts this method's preconditions name; the
            // caller guarantees that frame slot `slot`'s last submission has finished, so the
            // GPU is not using any of its sets (the safety contract).
            unsafe {
                self.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    pass.layout.handle(),
                    0,
                    &[pass.sets[slot][parity]],
                    &[],
                );
            }
            // SAFETY: `cmd` is recording with a compute pipeline and its set bound; the group
            // counts are at least 1.
            unsafe { self.device.cmd_dispatch(cmd, x.max(1), y.max(1), 1) };
        }

        self.frame_index = (self.frame_index + 1) % MAX_QUEUED_FRAMES;
    }

    /// The byte offset of `block` in frame slot `slot`'s constants.
    fn slot_offset(&self, slot: usize, block: Block) -> u64 {
        let index = slot as u64 * 3
            + match block {
                Block::Upscaler => 0,
                Block::Spd => 1,
                Block::Rcas => 2,
            };
        index * self.block_stride
    }

    /// Writes every internal descriptor (constant blocks, the sampler, internal images) of
    /// every slot's and parity's sets. At creation and after `resize` only: these never change
    /// between frames.
    fn write_internal_sets(&self) {
        let mut image_infos: Vec<[vk::DescriptorImageInfo; 1]> = Vec::new();
        let mut buffer_infos: Vec<[vk::DescriptorBufferInfo; 1]> = Vec::new();
        // (set, binding, type, image info index or buffer info index).
        let mut targets: Vec<(vk::DescriptorSet, u32, vk::DescriptorType, bool, usize)> =
            Vec::new();
        for (slot, parity) in (0..SET_RING).flat_map(|slot| [(slot, 0), (slot, 1)]) {
            let odd = parity == 1;
            for (pass, pipeline) in shaders::PASSES.iter().zip(&self.passes) {
                for binding in pass.bindings.iter().filter(|binding| binding.used) {
                    if External::of(binding.name).is_some() {
                        continue;
                    }
                    let ty = descriptor_type(binding.kind);
                    let (is_image, index) = match source(binding.name, odd) {
                        Source::Uniform(block) => {
                            let size = match block {
                                Block::Upscaler => shaders::CB_FSR3_UPSCALER_SIZE,
                                Block::Spd => shaders::CB_SPD_SIZE,
                                Block::Rcas => shaders::CB_RCAS_SIZE,
                            };
                            buffer_infos.push([vk::DescriptorBufferInfo {
                                buffer: self.uniforms.handle(),
                                offset: self.slot_offset(slot, block),
                                range: size as u64,
                            }]);
                            (false, buffer_infos.len() - 1)
                        }
                        Source::Image(slot) => {
                            image_infos.push([vk::DescriptorImageInfo {
                                sampler: vk::Sampler::null(),
                                image_view: self.images.view(slot),
                                image_layout: vk::ImageLayout::GENERAL,
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
                    targets.push((
                        pipeline.sets[slot][parity],
                        binding.binding,
                        ty,
                        is_image,
                        index,
                    ));
                }
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
        // SAFETY: every set, buffer, view and sampler is live on this device and each write
        // matches its binding's type in the set layout. No command buffer that uses these sets
        // is pending: they are new (`new`), or the GPU has finished with the context
        // (`resize`'s contract).
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
    }

    /// Writes the descriptors that name the caller's images in frame slot `slot`'s sets of
    /// parity `parity`, from fixed arrays (no allocation per frame).
    fn write_external_sets(
        &self,
        slot: usize,
        parity: usize,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        let count = self.externals.len();
        let mut infos = [vk::DescriptorImageInfo::default(); MAX_EXTERNAL_WRITES];
        for (info, external) in infos.iter_mut().zip(&self.externals) {
            *info = vk::DescriptorImageInfo {
                sampler: vk::Sampler::null(),
                image_view: external.external.view(inputs, out),
                image_layout: external.external.layout(),
            };
        }
        let mut writes = [vk::WriteDescriptorSet::default(); MAX_EXTERNAL_WRITES];
        for ((write, external), info) in writes.iter_mut().zip(&self.externals).zip(&infos) {
            *write = vk::WriteDescriptorSet::default()
                .dst_set(self.passes[external.pass].sets[slot][parity])
                .dst_binding(external.binding)
                .descriptor_type(external.ty)
                .image_info(core::slice::from_ref(info));
        }
        // SAFETY: every set and the caller's views are live (the views until this frame's
        // command buffer completes, by `record`'s contract) and each write matches its
        // binding's type. The GPU is not using this slot's sets: `record`'s in-flight
        // contract (the slot's last submission has finished).
        unsafe { self.device.update_descriptor_sets(&writes[..count], &[]) };
    }

    /// Records a clear of every level of `image`, which is in `GENERAL`.
    fn clear(&self, cmd: vk::CommandBuffer, image: &GpuImage, value: vk::ClearColorValue) {
        let range = color_range(0, vk::REMAINING_MIP_LEVELS);
        // SAFETY: callers record into a recording `cmd` (their own contracts); the image is
        // live, was created with TRANSFER_DST, and is in GENERAL here: every caller runs after
        // the barrier that moved it there (the poison hook's or the first frame's), and it
        // stays there.
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
        // SAFETY: callers record into a recording `cmd` (their own contracts); the barriers'
        // images are this context's live images, and `synchronization2` is enabled on every
        // device the renderer creates.
        unsafe { self.device.cmd_pipeline_barrier2(cmd, &info) };
    }
}

impl TemporalUpscaler for Fsr3 {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Fsr3
    }

    /// From Ultra Performance's size (the largest ratio FSR 3.1 documents, 3×, and the
    /// dynamic-resolution floor's lowest mode, ADR-0021 §4c) up to the mode's size, as the SDK
    /// computes them (truncated: 1706 at 2560 Quality, where TAAU's round-to-nearest gives
    /// 1707, amendment 2 item 19), both clamped to the context's maximum. Any size from 1×1 to
    /// the maximum works (§4c, amendment 1).
    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        let clamp = |size: vk::Extent2D| vk::Extent2D {
            width: size.width.min(self.max_render.width),
            height: size.height.min(self.max_render.height),
        };
        (
            clamp(Fsr3Mode::UltraPerformance.render_size(output)),
            clamp(Fsr3Mode::from(mode).render_size(output)),
        )
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        jitter_phases(render, output)
    }

    /// FSR sharpens with RCAS itself, so the renderer's `sharpen` pass does not run.
    fn has_sharpening(&self) -> bool {
        true
    }

    /// See [`Fsr3::record_frame`].
    ///
    /// # Panics
    /// When [`Fsr3::accepts`] rejects the sizes.
    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        // SAFETY: the trait's contract gives `record_frame`'s: each recorded buffer submitted
        // once, in order, on one queue, and at most FRAMES_IN_FLIGHT pending with
        // `inputs.frame_slot`'s last submission finished. `record_frame` needs at most
        // SET_RING pending, which the renderer's `SET_RING >= FRAMES_IN_FLIGHT` assert
        // (renderer.rs) guarantees.
        unsafe { self.record_frame(cmd, inputs, out) };
    }

    /// The next frame is a first frame: every internal image is re-initialised (after every
    /// earlier compute and clear on the queue, [`first_frame_barrier`]), so a record that was
    /// never submitted leaves nothing assumed.
    fn reset_history(&mut self) {
        self.first_frame = true;
        self.reset_requested = true;
    }

    unsafe fn resize(
        &mut self,
        device: &DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> RenderResult<()> {
        let output = vk::Extent2D {
            width: output.width.max(1),
            height: output.height.max(1),
        };
        let max_render = vk::Extent2D {
            width: max_render.width.clamp(1, output.width),
            height: max_render.height.clamp(1, output.height),
        };
        self.reallocate(device, max_render, output)
    }

    /// The context's images and constant buffer, as allocated.
    fn vram_bytes(&self) -> u64 {
        self.vram_bytes
    }
}

/// A barrier moving every level of `image` from `old_layout` (with `src`) to `GENERAL` for
/// `dst`.
fn image_barrier(
    image: vk::Image,
    src: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dst: (vk::PipelineStageFlags2, vk::AccessFlags2),
    old_layout: vk::ImageLayout,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src.0)
        .src_access_mask(src.1)
        .dst_stage_mask(dst.0)
        .dst_access_mask(dst.1)
        .old_layout(old_layout)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range(0, vk::REMAINING_MIP_LEVELS))
}

/// The first frame's transition of one internal image to `GENERAL` for its clear. Fresh (at
/// creation, after `resize` or [`TemporalUpscaler::reset_history`]), the image goes from
/// `UNDEFINED`; after the poison hook's clears, from `GENERAL`, keeping its garbage.
///
/// A history reset happens mid-stream: the previous frame's passes may still be reading and
/// writing the image, and its per-frame clears writing it. The transition (a write) and the
/// clears after it therefore wait for every earlier compute and clear on the queue and make
/// their writes available (WAR and WAW), never `NONE` (ADR-0021 amendment 2, item 28;
/// `first_frame_barrier_waits_for_earlier_work`). At creation there is no earlier work and
/// the dependency is free.
fn first_frame_barrier(image: vk::Image, poisoned: bool) -> vk::ImageMemoryBarrier2<'static> {
    image_barrier(
        image,
        (
            vk::PipelineStageFlags2::COMPUTE_SHADER | vk::PipelineStageFlags2::CLEAR,
            vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::TRANSFER_WRITE,
        ),
        (
            vk::PipelineStageFlags2::CLEAR,
            vk::AccessFlags2::TRANSFER_WRITE,
        ),
        if poisoned {
            vk::ImageLayout::GENERAL
        } else {
            vk::ImageLayout::UNDEFINED
        },
    )
}

/// `new`'s and `resize`'s size check.
fn check_sizes(max_render: vk::Extent2D, max_output: vk::Extent2D) {
    assert!(
        max_render.width > 0
            && max_render.height > 0
            && max_render.width <= max_output.width
            && max_render.height <= max_output.height,
        "FSR 3.1 upscales: 0 < render <= output"
    );
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
        // `GraphicsPipeline` is the renderer's owner of any `VkPipeline`, compute ones
        // included (fog, VFX), despite its name.
        let pipeline = GraphicsPipeline::from_raw(
            device.device.clone(),
            device.create_compute_pipeline(&info)?,
        );
        let sets = pool.allocate(&device.device, &[set_layout.handle(); 2 * SET_RING])?;
        Ok(Self {
            pipeline,
            layout,
            _set_layout: set_layout,
            sets: core::array::from_fn(|slot| [sets[2 * slot], sets[2 * slot + 1]]),
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

    /// Every used binding of every pass resolves to a resource in both parities, so the set
    /// writes never panic, and the parity swaps exactly the history pairs.
    #[test]
    fn every_used_binding_has_a_source() {
        for pass in shaders::PASSES {
            for binding in pass.bindings.iter().filter(|binding| binding.used) {
                let name = binding.name;
                let known = matches!(name, "cbFSR3Upscaler" | "cbSPD" | "cbRCAS")
                    || name == "s_LinearClamp"
                    || External::of(name).is_some()
                    || (internal(name, false).is_some() && internal(name, true).is_some());
                assert!(known, "{}: `{name}` has no source", pass.name);
                if binding.kind == Kind::Sampler {
                    assert_eq!(name, "s_LinearClamp", "only the linear sampler is used");
                }
                // The output is the one storage image the caller owns; its inputs are sampled.
                if let Some(external) = External::of(name) {
                    let storage = binding.kind == Kind::Storage;
                    assert_eq!(storage, external == External::Output, "{name}");
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

    /// The pass indices `record` uses name the passes they mean, and the caller's images fit
    /// the fixed per-frame write arrays.
    #[test]
    fn pass_indices_and_external_count() {
        let named = [
            (PREPARE_INPUTS, shaders::PREPARE_INPUTS.name),
            (LUMA_PYRAMID, shaders::LUMA_PYRAMID.name),
            (SHADING_CHANGE_PYRAMID, shaders::SHADING_CHANGE_PYRAMID.name),
            (SHADING_CHANGE, shaders::SHADING_CHANGE.name),
            (PREPARE_REACTIVITY, shaders::PREPARE_REACTIVITY.name),
            (LUMA_INSTABILITY, shaders::LUMA_INSTABILITY.name),
            (ACCUMULATE, shaders::ACCUMULATE.name),
            (ACCUMULATE_SHARPEN, shaders::ACCUMULATE_SHARPEN.name),
            (RCAS, shaders::RCAS.name),
        ];
        for (index, name) in named {
            assert_eq!(shaders::PASSES[index].name, name);
        }
        let externals = shaders::PASSES
            .iter()
            .flat_map(|pass| pass.bindings.iter())
            .filter(|binding| binding.used && External::of(binding.name).is_some())
            .count();
        assert!(
            externals > 0 && externals <= MAX_EXTERNAL_WRITES,
            "{externals}"
        );
    }

    /// A history reset re-initialises images the previous frame's passes and clears may still
    /// use, so the first frame's transition waits for them (ADR-0021 amendment 2, item 28).
    /// Synchronization validation does not catch a `NONE` source on lavapipe, the RTX 5070 Ti
    /// or RADV, so this pins it.
    #[test]
    fn first_frame_barrier_waits_for_earlier_work() {
        for poisoned in [false, true] {
            let b = first_frame_barrier(vk::Image::null(), poisoned);
            assert!(
                b.src_stage_mask.contains(
                    vk::PipelineStageFlags2::COMPUTE_SHADER | vk::PipelineStageFlags2::CLEAR
                ),
                "{:?}",
                b.src_stage_mask
            );
            assert!(
                b.src_access_mask.contains(
                    vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::TRANSFER_WRITE
                ),
                "{:?}",
                b.src_access_mask
            );
            assert_eq!(b.dst_stage_mask, vk::PipelineStageFlags2::CLEAR);
            assert_eq!(b.new_layout, vk::ImageLayout::GENERAL);
            let old = if poisoned {
                vk::ImageLayout::GENERAL
            } else {
                vk::ImageLayout::UNDEFINED
            };
            assert_eq!(b.old_layout, old);
        }
    }

    /// The far plane's bits are the float 0.0's (reversed-Z).
    #[test]
    fn far_depth_is_zero() {
        // SAFETY: reading the `uint32` member of a value written through it.
        assert_eq!(unsafe { FAR_DEPTH_BITS.uint32 }[0], 0.0f32.to_bits());
    }
}
