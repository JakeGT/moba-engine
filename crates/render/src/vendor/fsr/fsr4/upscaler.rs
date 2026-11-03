//! FSR 4 behind [`TemporalUpscaler`] (ADR-0021 §4b, N1c), through the FidelityFX API.
//!
//! - **Creation** (with the renderer): one upscaling context (`ffxCreateContext` with the
//!   upscale description and the Vulkan back-end description chained) at the
//!   dynamic-resolution ceiling and the output size, with linear HDR input, render-resolution
//!   motion without jitter, non-inverted finite depth, our own exposure texture and dynamic
//!   resolution. The provider the loader picks is queried (`GET_PROVIDER_VERSION`): a 3.x
//!   provider (a GPU without FSR 4) is rejected, since the open build's `fsr3` port covers
//!   FSR 3.1.
//! - **Each frame** `ffxDispatch` with the upscale dispatch description records the
//!   upscaler: every input at the top-left render sub-rectangle (`renderSize`), the jitter in
//!   render pixels (FSR's convention, ours, amendment 1), motion scale (1, 1), the reactive
//!   and transparency-and-composition masks, the camera's planes and field of view (metres),
//!   the reset flag and FSR's own RCAS at the chosen sharpness.
//! - **Resource states**: the inputs are passed as `COMPUTE_READ` (`SHADER_READ_ONLY_OPTIMAL`)
//!   and the output as `UNORDERED_ACCESS` (`GENERAL`), as the graph leaves them; the
//!   FidelityFX Vulkan back end records its own barriers and returns each resource to the
//!   state it was given.
//! - **Resize** destroys and re-creates the context (the device is idle).
//!
//! Compile-only on this project's machines (Windows binaries, AMD hardware; ADR-0021
//! amendment 5): nothing here has run.

#![allow(
    clippy::unnecessary_cast,
    reason = "FidelityFX's enum constants are c_uint for GCC and Clang targets but c_int for MSVC"
)]

use std::ffi::{CStr, c_void};
use std::ptr;

use ash::vk::{self, Handle as _};

use super::sys;
use crate::upscale::{
    Extent, TemporalUpscaler, UpscaleImage, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind,
};
use crate::vendor::{Unavailable, UpscalerRequest};

/// `FFX_API_MAKE_EFFECT_SUB_ID(FFX_API_EFFECT_ID_UPSCALE, sub)` and
/// `FFX_API_MAKE_BACKEND_SUB_ID(FFX_API_BACKEND_ID_VK, 0x03)`: function-like macros bindgen
/// cannot evaluate (`ffx_upscale.h`, `ffx_api_vk.h`).
const EFFECT_UPSCALE: u64 = 0x0001_0000;
pub(crate) const CREATE_CONTEXT_UPSCALE: u64 = EFFECT_UPSCALE;
pub(crate) const DISPATCH_UPSCALE: u64 = EFFECT_UPSCALE | 0x01;
pub(crate) const QUERY_RENDER_RESOLUTION: u64 = EFFECT_UPSCALE | 0x03;
pub(crate) const QUERY_GPU_MEMORY_USAGE: u64 = EFFECT_UPSCALE | 0x08;
pub(crate) const CREATE_BACKEND_VK: u64 = 0x0100_0000 | 0x03;
/// `FFX_API_QUERY_DESC_TYPE_GET_PROVIDER_VERSION`.
const QUERY_PROVIDER_VERSION: u64 = 6;

/// `FFX_API_RETURN_OK`.
const OK: sys::ffxReturnCode_t = 0;

fn header(kind: u64) -> sys::ffxApiHeader {
    sys::ffxApiHeader {
        type_: kind,
        pNext: ptr::null_mut(),
    }
}

/// The FidelityFX quality mode for one of ours (the same ratios).
pub(crate) fn quality_mode(mode: UpscaleMode) -> u32 {
    let mode = match mode {
        UpscaleMode::Native => sys::FFX_UPSCALE_QUALITY_MODE_NATIVEAA,
        UpscaleMode::Quality => sys::FFX_UPSCALE_QUALITY_MODE_QUALITY,
        UpscaleMode::Balanced => sys::FFX_UPSCALE_QUALITY_MODE_BALANCED,
        UpscaleMode::Performance => sys::FFX_UPSCALE_QUALITY_MODE_PERFORMANCE,
        UpscaleMode::UltraPerformance => sys::FFX_UPSCALE_QUALITY_MODE_ULTRA_PERFORMANCE,
    };
    mode as u32
}

/// Linear HDR colour, our exposure texture (no auto-exposure), finite non-inverted depth,
/// render-resolution motion without jitter, a render size that varies (dynamic resolution).
pub(crate) fn create_flags() -> u32 {
    (sys::FFX_UPSCALE_ENABLE_HIGH_DYNAMIC_RANGE | sys::FFX_UPSCALE_ENABLE_DYNAMIC_RESOLUTION) as u32
}

/// The FidelityFX surface format of a Vulkan format: a port of the SDK's
/// `ffxApiGetSurfaceFormatVK` (`ffx_api_vk.h`, C++-only, so not in the bindings). Formats it
/// does not map are `UNKNOWN`.
pub(crate) fn surface_format(format: vk::Format) -> u32 {
    use vk::Format as F;
    let format = match format {
        F::R32G32B32A32_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R32G32B32A32_FLOAT,
        F::R32G32B32_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R32G32B32_FLOAT,
        F::R32G32B32A32_UINT => sys::FFX_API_SURFACE_FORMAT_R32G32B32A32_UINT,
        F::R16G16B16A16_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R16G16B16A16_FLOAT,
        F::R32G32_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R32G32_FLOAT,
        F::R32_UINT | F::D24_UNORM_S8_UINT | F::X8_D24_UNORM_PACK32 => {
            sys::FFX_API_SURFACE_FORMAT_R32_UINT
        }
        F::R8G8B8A8_UNORM => sys::FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM,
        F::R8G8B8A8_SNORM => sys::FFX_API_SURFACE_FORMAT_R8G8B8A8_SNORM,
        F::R8G8B8A8_SRGB => sys::FFX_API_SURFACE_FORMAT_R8G8B8A8_SRGB,
        F::B8G8R8A8_UNORM => sys::FFX_API_SURFACE_FORMAT_B8G8R8A8_UNORM,
        F::B8G8R8A8_SRGB => sys::FFX_API_SURFACE_FORMAT_B8G8R8A8_SRGB,
        F::B10G11R11_UFLOAT_PACK32 => sys::FFX_API_SURFACE_FORMAT_R11G11B10_FLOAT,
        F::A2B10G10R10_UNORM_PACK32 => sys::FFX_API_SURFACE_FORMAT_R10G10B10A2_UNORM,
        F::R16G16_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R16G16_FLOAT,
        F::R16G16_UINT => sys::FFX_API_SURFACE_FORMAT_R16G16_UINT,
        F::R16G16_SINT => sys::FFX_API_SURFACE_FORMAT_R16G16_SINT,
        F::R16_SFLOAT => sys::FFX_API_SURFACE_FORMAT_R16_FLOAT,
        F::R16_UINT => sys::FFX_API_SURFACE_FORMAT_R16_UINT,
        F::R16_UNORM | F::D16_UNORM | F::D16_UNORM_S8_UINT => sys::FFX_API_SURFACE_FORMAT_R16_UNORM,
        F::R16_SNORM => sys::FFX_API_SURFACE_FORMAT_R16_SNORM,
        F::R8_UNORM => sys::FFX_API_SURFACE_FORMAT_R8_UNORM,
        F::R8_SNORM => sys::FFX_API_SURFACE_FORMAT_R8_SNORM,
        F::R8_UINT | F::S8_UINT => sys::FFX_API_SURFACE_FORMAT_R8_UINT,
        F::R8G8_UNORM => sys::FFX_API_SURFACE_FORMAT_R8G8_UNORM,
        F::R8G8_UINT => sys::FFX_API_SURFACE_FORMAT_R8G8_UINT,
        F::R32_SFLOAT | F::D32_SFLOAT | F::D32_SFLOAT_S8_UINT => {
            sys::FFX_API_SURFACE_FORMAT_R32_FLOAT
        }
        F::E5B9G9R9_UFLOAT_PACK32 => sys::FFX_API_SURFACE_FORMAT_R9G9B9E5_SHAREDEXP,
        _ => sys::FFX_API_SURFACE_FORMAT_UNKNOWN,
    };
    format as u32
}

fn is_depth(format: vk::Format) -> bool {
    matches!(format, vk::Format::D32_SFLOAT | vk::Format::D16_UNORM)
}

/// One of our images as a FidelityFX resource: an input (`COMPUTE_READ`) or the output
/// (`UNORDERED_ACCESS`).
fn resource(image: &UpscaleImage, output: bool) -> sys::FfxApiResource {
    let mut usage = sys::FFX_API_RESOURCE_USAGE_READ_ONLY as u32;
    if is_depth(image.format) {
        usage |= sys::FFX_API_RESOURCE_USAGE_DEPTHTARGET as u32;
    }
    if output {
        usage |= sys::FFX_API_RESOURCE_USAGE_UAV as u32;
    }
    let description = sys::FfxApiResourceDescription {
        type_: sys::FFX_API_RESOURCE_TYPE_TEXTURE2D as u32,
        format: surface_format(image.format),
        __bindgen_anon_1: sys::FfxApiResourceDescription__bindgen_ty_1 {
            width: image.extent.width,
        },
        __bindgen_anon_2: sys::FfxApiResourceDescription__bindgen_ty_2 {
            height: image.extent.height,
        },
        __bindgen_anon_3: sys::FfxApiResourceDescription__bindgen_ty_3 { depth: 1 },
        mipCount: 1,
        flags: 0,
        usage,
    };
    sys::FfxApiResource {
        resource: ptr::with_exposed_provenance_mut(image.image.as_raw() as usize),
        description,
        state: if output {
            sys::FFX_API_RESOURCE_STATE_UNORDERED_ACCESS as u32
        } else {
            sys::FFX_API_RESOURCE_STATE_COMPUTE_READ as u32
        },
    }
}

/// The FSR 4 back end behind the trait.
pub(crate) struct Fsr4Upscaler {
    api: &'static sys::FfxApi,
    context: sys::ffxContext,
    device: vk::Device,
    physical: vk::PhysicalDevice,
    get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    output: Extent,
    max_render: Extent,
    /// The provider's render size for every mode at `output` (its ceiling).
    sizes: [Extent; 5],
    reset: bool,
    vram: u64,
    failures: u32,
}

impl Fsr4Upscaler {
    /// Creates FSR 4 for `request`.
    pub(crate) fn new(
        api: &'static sys::FfxApi,
        request: &UpscalerRequest<'_>,
    ) -> Result<Self, Unavailable> {
        let device = request.device;
        let mut upscaler = Self {
            api,
            context: ptr::null_mut(),
            device: device.device.handle(),
            physical: device.physical,
            get_device_proc_addr: device.instance.fp_v1_0().get_device_proc_addr,
            output: request.output,
            max_render: request.max_render,
            sizes: [request.output; 5],
            reset: true,
            vram: 0,
            failures: 0,
        };
        upscaler.create(request.output, request.max_render)?;
        Ok(upscaler)
    }

    fn destroy(&mut self) {
        if !self.context.is_null() {
            // SAFETY: the context came from ffxCreateContext and is destroyed once; no command
            // buffer that dispatched it is pending (the device is idle on resize and drop).
            let _ = unsafe {
                self.api
                    .ffxDestroyContext(&raw mut self.context, ptr::null())
            };
            self.context = ptr::null_mut();
        }
    }

    /// (Re)creates the context (the device is idle).
    fn create(&mut self, output: Extent, max_render: Extent) -> Result<(), Unavailable> {
        self.destroy();
        let output = Extent {
            width: output.width.max(1),
            height: output.height.max(1),
        };
        let max_render = Extent {
            width: max_render.width.clamp(1, output.width),
            height: max_render.height.clamp(1, output.height),
        };
        let mut backend = sys::ffxCreateBackendVKDesc {
            header: header(CREATE_BACKEND_VK),
            deviceContext: sys::VkDeviceContext {
                vkDevice: ptr::with_exposed_provenance_mut(self.device.as_raw() as usize),
                vkPhysicalDevice: ptr::with_exposed_provenance_mut(self.physical.as_raw() as usize),
                // SAFETY: ash's and the header's `vkGetDeviceProcAddr` have the same C
                // signature (handles are pointer-sized, checked below).
                vkDeviceProcAddr: Some(unsafe {
                    std::mem::transmute::<
                        vk::PFN_vkGetDeviceProcAddr,
                        unsafe extern "C" fn(
                            sys::VkDevice,
                            *const std::ffi::c_char,
                        ) -> sys::PFN_vkVoidFunction,
                    >(self.get_device_proc_addr)
                }),
            },
        };
        let mut desc = sys::ffxCreateContextDescUpscale {
            header: sys::ffxApiHeader {
                type_: CREATE_CONTEXT_UPSCALE,
                pNext: (&raw mut backend.header).cast(),
            },
            flags: create_flags(),
            maxRenderSize: sys::FfxApiDimensions2D {
                width: max_render.width,
                height: max_render.height,
            },
            maxUpscaleSize: sys::FfxApiDimensions2D {
                width: output.width,
                height: output.height,
            },
            fpMessage: None,
        };
        let mut context: sys::ffxContext = ptr::null_mut();
        // SAFETY: both descriptions are valid for the call and chained through `pNext`; the
        // device and physical device are live; the out pointer is valid.
        let result = unsafe {
            self.api
                .ffxCreateContext(&raw mut context, &raw mut desc.header, ptr::null())
        };
        if result != OK || context.is_null() {
            return Err(format!(
                "ffxCreateContext for the upscaler failed (ffxReturnCode_t {result}; FSR 4 \
                 needs an RDNA GPU the FidelityFX API supports)"
            ));
        }
        self.context = context;
        let provider = self.provider_version();
        if provider.starts_with("3.") || provider.is_empty() {
            self.destroy();
            return Err(format!(
                "the FidelityFX API offers upscaler {} on this GPU, not FSR 4 (the open \
                 build's fsr3 port covers FSR 3.1)",
                if provider.is_empty() {
                    "of an unknown version"
                } else {
                    &provider
                }
            ));
        }
        eprintln!("[render] fsr4: FidelityFX upscaler provider {provider}");
        let mut sizes = [max_render; 5];
        for (slot, &mode) in sizes.iter_mut().zip(UpscaleMode::ALL) {
            *slot = self.render_resolution(output, mode).unwrap_or(max_render);
        }
        self.sizes = sizes;
        self.output = output;
        self.max_render = max_render;
        self.reset = true;
        self.vram = self.memory_usage();
        Ok(())
    }

    fn query(&mut self, header: *mut sys::ffxApiHeader) -> sys::ffxReturnCode_t {
        // SAFETY: the context is live and `header` heads a query description valid for the
        // call.
        unsafe { self.api.ffxQuery(&raw mut self.context, header) }
    }

    fn provider_version(&mut self) -> String {
        let mut query = sys::ffxQueryGetProviderVersion {
            header: header(QUERY_PROVIDER_VERSION),
            versionId: 0,
            versionName: ptr::null(),
        };
        if self.query(&raw mut query.header) != OK || query.versionName.is_null() {
            return String::new();
        }
        // SAFETY: the provider returned a NUL-terminated name it owns (the library stays
        // loaded).
        unsafe { CStr::from_ptr(query.versionName) }
            .to_string_lossy()
            .into_owned()
    }

    fn render_resolution(&mut self, output: Extent, mode: UpscaleMode) -> Option<Extent> {
        let (mut width, mut height) = (0u32, 0u32);
        let mut query = sys::ffxQueryDescUpscaleGetRenderResolutionFromQualityMode {
            header: header(QUERY_RENDER_RESOLUTION),
            displayWidth: output.width,
            displayHeight: output.height,
            qualityMode: quality_mode(mode),
            pOutRenderWidth: &raw mut width,
            pOutRenderHeight: &raw mut height,
        };
        (self.query(&raw mut query.header) == OK && width > 0 && height > 0)
            .then_some(Extent { width, height })
    }

    fn memory_usage(&mut self) -> u64 {
        let mut usage = sys::FfxApiEffectMemoryUsage {
            totalUsageInBytes: 0,
            aliasableUsageInBytes: 0,
        };
        let mut query = sys::ffxQueryDescUpscaleGetGPUMemoryUsage {
            header: header(QUERY_GPU_MEMORY_USAGE),
            gpuMemoryUsageUpscaler: &raw mut usage,
        };
        if self.query(&raw mut query.header) == OK {
            usage.totalUsageInBytes
        } else {
            0
        }
    }
}

impl Drop for Fsr4Upscaler {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl TemporalUpscaler for Fsr4Upscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Fsr4
    }

    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        let (floor, ceiling) =
            crate::upscale::scaled_range(output, mode, crate::post::MIN_TEMPORAL_RENDER_SCALE);
        if output != self.output {
            return (floor, ceiling);
        }
        let index = UpscaleMode::ALL
            .iter()
            .position(|&m| m == mode)
            .unwrap_or(0);
        let ceiling = Extent {
            width: self.sizes[index].width.min(self.max_render.width),
            height: self.sizes[index].height.min(self.max_render.height),
        };
        let floor = Extent {
            width: floor.width.min(ceiling.width),
            height: floor.height.min(ceiling.height),
        };
        (floor, ceiling)
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        // ADR-0021 §1: ceil(8 r²), FSR's own rule.
        crate::temporal::jitter_phases(render, output)
    }

    fn has_sharpening(&self) -> bool {
        true
    }

    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        if self.context.is_null() {
            return;
        }
        let desc = sys::ffxDispatchDescUpscale {
            header: header(DISPATCH_UPSCALE),
            commandList: ptr::with_exposed_provenance_mut::<c_void>(cmd.as_raw() as usize),
            color: resource(&inputs.color, false),
            depth: resource(&inputs.depth, false),
            motionVectors: resource(&inputs.motion, false),
            exposure: resource(&inputs.exposure, false),
            reactive: resource(&inputs.reactive, false),
            transparencyAndComposition: resource(&inputs.transparency, false),
            output: resource(&out.image, true),
            jitterOffset: sys::FfxApiFloatCoords2D {
                x: inputs.jitter[0],
                y: inputs.jitter[1],
            },
            motionVectorScale: sys::FfxApiFloatCoords2D { x: 1.0, y: 1.0 },
            renderSize: sys::FfxApiDimensions2D {
                width: inputs.render_size.width,
                height: inputs.render_size.height,
            },
            upscaleSize: sys::FfxApiDimensions2D {
                width: out.image.extent.width,
                height: out.image.extent.height,
            },
            enableSharpening: inputs.sharpness > 0.0,
            sharpness: inputs.sharpness.clamp(0.0, 1.0),
            frameTimeDelta: inputs.dt_ms,
            preExposure: 1.0,
            reset: self.reset || inputs.reset,
            cameraNear: inputs.near,
            cameraFar: inputs.far,
            cameraFovAngleVertical: inputs.fov_y,
            viewSpaceToMetersFactor: 1.0,
            flags: 0,
        };
        // SAFETY: `cmd` is recording on the context's device; the description is valid for
        // the call; every image stays alive and in the stated state until the command buffer
        // finishes (the `record` contract).
        let result = unsafe {
            self.api
                .ffxDispatch(&raw mut self.context, &raw const desc.header)
        };
        if result == OK {
            self.reset = false;
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures <= 3 {
                eprintln!(
                    "[render] FSR 4 dispatch failed (ffxReturnCode_t {result}; render {}x{}, \
                     output {}x{}){}",
                    inputs.render_size.width,
                    inputs.render_size.height,
                    out.image.extent.width,
                    out.image.extent.height,
                    if self.failures == 3 {
                        "; further failures are not logged"
                    } else {
                        ""
                    }
                );
            }
            self.reset = true;
        }
    }

    fn reset_history(&mut self) {
        self.reset = true;
    }

    fn resize(
        &mut self,
        _device: &crate::device::DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> crate::error::RenderResult<()> {
        self.create(output, max_render)
            .map_err(crate::error::RenderError::NoSuitableDevice)
    }

    fn vram_bytes(&self) -> u64 {
        self.vram
    }
}

/// The Vulkan handles above cross the FFI as pointers of the same size.
const _: () = {
    assert!(size_of::<sys::VkDevice>() == size_of::<vk::Device>());
    assert!(size_of::<sys::VkPhysicalDevice>() == size_of::<vk::PhysicalDevice>());
    assert!(size_of::<*mut c_void>() == size_of::<vk::CommandBuffer>());
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The description type IDs `ffx_upscale.h` and `ffx_api_vk.h` build with macros.
    #[test]
    fn description_types_match_the_headers() {
        assert_eq!(CREATE_CONTEXT_UPSCALE, 0x0001_0000);
        assert_eq!(DISPATCH_UPSCALE, 0x0001_0001);
        assert_eq!(QUERY_RENDER_RESOLUTION, 0x0001_0003);
        assert_eq!(QUERY_GPU_MEMORY_USAGE, 0x0001_0008);
        assert_eq!(CREATE_BACKEND_VK, 0x0100_0003);
    }

    #[test]
    fn modes_and_formats_map_to_the_fidelityfx_values() {
        let modes: Vec<u32> = UpscaleMode::ALL.iter().map(|&m| quality_mode(m)).collect();
        assert_eq!(modes, [0, 1, 2, 3, 4]);
        assert_eq!(create_flags(), (1 << 0) | (1 << 6));
        assert_eq!(
            surface_format(vk::Format::D32_SFLOAT),
            surface_format(vk::Format::R32_SFLOAT)
        );
        assert_ne!(
            surface_format(vk::Format::R16G16B16A16_SFLOAT),
            sys::FFX_API_SURFACE_FORMAT_UNKNOWN as u32
        );
    }

    /// Every format the temporal pass can hand FSR 4: each tier's HDR colour (Low's is the
    /// packed `B10G11R11`), its fallback, the motion, depth, reactive and exposure targets.
    #[test]
    fn every_temporal_input_format_is_known() {
        let unknown = sys::FFX_API_SURFACE_FORMAT_UNKNOWN as u32;
        let tiers = crate::quality::QualityTier::ALL
            .iter()
            .map(|tier| tier.hdr_format());
        let others = [
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::R16G16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::Format::R8_UNORM,
            vk::Format::R32_SFLOAT,
            vk::Format::R16_SFLOAT,
        ];
        for format in tiers.chain(others) {
            assert_ne!(surface_format(format), unknown, "{format:?}");
        }
        assert_eq!(
            surface_format(vk::Format::B10G11R11_UFLOAT_PACK32),
            sys::FFX_API_SURFACE_FORMAT_R11G11B10_FLOAT as u32
        );
    }
}
