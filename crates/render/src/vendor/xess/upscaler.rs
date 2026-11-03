//! XeSS super resolution behind [`TemporalUpscaler`] (ADR-0021 §4b, N1c).
//!
//! - **Creation** (with the renderer): `xessVKCreateContext` on the renderer's instance,
//!   physical device and device, the jitter and velocity scales set to (1, 1) (render pixels,
//!   our convention), the optimal-input query for every mode, then `xessVKInit` for the
//!   output size and the mode's quality setting with our exposure texture and the responsive
//!   (reactive) mask. XeSS allocates its own memory and builds its pipelines in `xessVKInit`.
//! - **Each frame** `xessVKExecute` records XeSS's dispatches: colour, motion, depth, the 1×1
//!   exposure multiplier, the reactive mask and the output, the jitter in render pixels, the
//!   render size, every input at the top-left of its allocation, and the reset flag. The
//!   inputs are in `SHADER_READ_ONLY_OPTIMAL` and the output in `GENERAL`, as the graph leaves
//!   them; [`crate::vendor::barriers`] order XeSS's work against the graph and its previous
//!   frame.
//! - **Quality settings** by ratio: Native → AA (1.0×), Quality → Ultra Quality (1.5×),
//!   Balanced → Quality (1.7×), Performance → Balanced (2.0×), Ultra Performance → Ultra
//!   Performance (3.0×).
//! - **Mip bias** is XeSS's own rule (`crate::temporal::MipBiasRule::Xess`).
//! - **Resize** destroys and re-creates the context (the device is idle).
//!
//! Compile-only on this project's machines (XeSS 3.0.2 ships Windows binaries only; ADR-0021
//! amendment 5): nothing here has run, and the jitter sign follows FSR's and DLSS's (verified
//! on DLSS only).

#![allow(
    clippy::unnecessary_cast,
    reason = "XeSS's enum constants are c_uint for GCC and Clang targets but c_int for MSVC"
)]

use std::ptr;

use ash::vk::{self, Handle as _};

use super::sys;
use crate::upscale::{
    Extent, TemporalUpscaler, UpscaleImage, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind,
};
use crate::vendor::barriers::{internal_barrier, output_after, output_before};
use crate::vendor::{Unavailable, UpscalerRequest};

/// XeSS's quality setting for one of our modes (nearest by ratio).
pub(crate) fn quality(mode: UpscaleMode) -> sys::xess_quality_settings_t {
    match mode {
        UpscaleMode::Native => sys::XESS_QUALITY_SETTING_AA,
        UpscaleMode::Quality => sys::XESS_QUALITY_SETTING_ULTRA_QUALITY,
        UpscaleMode::Balanced => sys::XESS_QUALITY_SETTING_QUALITY,
        UpscaleMode::Performance => sys::XESS_QUALITY_SETTING_BALANCED,
        UpscaleMode::UltraPerformance => sys::XESS_QUALITY_SETTING_ULTRA_PERFORMANCE,
    }
}

/// Our exposure texture and reactive mask; linear HDR colour, render-resolution motion in
/// pixels without jitter, non-inverted depth (no other flag).
pub(crate) fn init_flags() -> u32 {
    (sys::XESS_INIT_FLAG_EXPOSURE_SCALE_TEXTURE | sys::XESS_INIT_FLAG_RESPONSIVE_PIXEL_MASK) as u32
}

fn xy(extent: Extent) -> sys::xess_2d_t {
    sys::xess_2d_t {
        x: extent.width,
        y: extent.height,
    }
}

fn handle<T>(raw: u64) -> *mut T {
    ptr::with_exposed_provenance_mut(raw as usize)
}

fn aspect(format: vk::Format) -> vk::ImageAspectFlags {
    match format {
        vk::Format::D32_SFLOAT | vk::Format::D16_UNORM => vk::ImageAspectFlags::DEPTH,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

fn view_info(image: &UpscaleImage) -> sys::xess_vk_image_view_info {
    sys::xess_vk_image_view_info {
        imageView: handle(image.view.as_raw()),
        image: handle(image.image.as_raw()),
        subresourceRange: sys::VkImageSubresourceRange {
            aspectMask: aspect(image.format).as_raw(),
            baseMipLevel: 0,
            levelCount: 1,
            baseArrayLayer: 0,
            layerCount: 1,
        },
        format: image.format.as_raw(),
        width: image.extent.width,
        height: image.extent.height,
    }
}

fn check(result: sys::xess_result_t, what: &str) -> Result<(), Unavailable> {
    if result == sys::XESS_RESULT_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what} failed (xess_result_t {result})"))
    }
}

/// The XeSS back end behind the trait.
pub(crate) struct XessUpscaler {
    api: &'static sys::XessApi,
    context: sys::xess_context_handle_t,
    device: ash::Device,
    instance: vk::Instance,
    physical: vk::PhysicalDevice,
    mode: UpscaleMode,
    output: Extent,
    /// The (minimum, maximum) input size for every mode at `output`.
    ranges: [Option<(Extent, Extent)>; 5],
    reset: bool,
    failures: u32,
}

impl XessUpscaler {
    /// Creates XeSS for `request`.
    pub(crate) fn new(
        api: &'static sys::XessApi,
        request: &UpscalerRequest<'_>,
    ) -> Result<Self, Unavailable> {
        let device = request.device;
        let mut upscaler = Self {
            api,
            context: ptr::null_mut(),
            device: device.device.clone(),
            instance: device.instance.handle(),
            physical: device.physical,
            mode: request.mode,
            output: request.output,
            ranges: [None; 5],
            reset: true,
            failures: 0,
        };
        upscaler.create(request.output)?;
        Ok(upscaler)
    }

    fn destroy(&mut self) {
        if !self.context.is_null() {
            // SAFETY: the context came from xessVKCreateContext and is destroyed once; no
            // command buffer that executed it is pending (the device is idle on resize and
            // drop).
            let _ = unsafe { self.api.xessDestroyContext(self.context) };
            self.context = ptr::null_mut();
        }
    }

    /// (Re)creates and initialises the context for `output` (the device is idle).
    fn create(&mut self, output: Extent) -> Result<(), Unavailable> {
        self.destroy();
        let output = Extent {
            width: output.width.max(1),
            height: output.height.max(1),
        };
        let mut context: sys::xess_context_handle_t = ptr::null_mut();
        // SAFETY: the instance, physical device and device are live and related; the out
        // pointer is valid.
        let result = unsafe {
            self.api.xessVKCreateContext(
                handle(self.instance.as_raw()),
                handle(self.physical.as_raw()),
                handle(self.device.handle().as_raw()),
                &raw mut context,
            )
        };
        check(result, "xessVKCreateContext")?;
        self.context = context;
        // SAFETY: the context is live.
        let result = unsafe { self.api.xessSetJitterScale(self.context, 1.0, 1.0) };
        check(result, "xessSetJitterScale")?;
        // SAFETY: the context is live.
        let result = unsafe { self.api.xessSetVelocityScale(self.context, 1.0, 1.0) };
        check(result, "xessSetVelocityScale")?;
        let size = xy(output);
        for (slot, &mode) in self.ranges.iter_mut().zip(UpscaleMode::ALL) {
            let (mut optimal, mut min, mut max) = (xy(output), xy(output), xy(output));
            // SAFETY: the context is live; every pointer is valid for the call.
            let result = unsafe {
                self.api.xessGetOptimalInputResolution(
                    self.context,
                    &raw const size,
                    quality(mode),
                    &raw mut optimal,
                    &raw mut min,
                    &raw mut max,
                )
            };
            *slot = (result == sys::XESS_RESULT_SUCCESS && max.x > 0 && max.y > 0).then(|| {
                (
                    Extent {
                        width: min.x.clamp(1, max.x),
                        height: min.y.clamp(1, max.y),
                    },
                    Extent {
                        width: max.x.min(output.width),
                        height: max.y.min(output.height),
                    },
                )
            });
        }
        let init = sys::xess_vk_init_params_t {
            outputResolution: size,
            qualitySetting: quality(self.mode),
            initFlags: init_flags(),
            creationNodeMask: 1,
            visibleNodeMask: 1,
            tempBufferHeap: ptr::null_mut(),
            bufferHeapOffset: 0,
            tempTextureHeap: ptr::null_mut(),
            textureHeapOffset: 0,
            pipelineCache: ptr::null_mut(),
        };
        // SAFETY: the context is live and the parameters are valid for the call.
        let result = unsafe { self.api.xessVKInit(self.context, &raw const init) };
        if let Err(error) = check(result, "xessVKInit") {
            self.destroy();
            return Err(error);
        }
        self.output = output;
        self.reset = true;
        Ok(())
    }
}

impl Drop for XessUpscaler {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl TemporalUpscaler for XessUpscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Xess
    }

    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        let index = UpscaleMode::ALL
            .iter()
            .position(|&m| m == mode)
            .unwrap_or(0);
        match self.ranges[index] {
            Some(range) if output == self.output => range,
            _ => crate::upscale::scaled_range(output, mode, crate::post::MIN_TEMPORAL_RENDER_SCALE),
        }
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        crate::temporal::jitter_phases(render, output)
    }

    fn has_sharpening(&self) -> bool {
        false
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
        let zero = sys::xess_2d_t { x: 0, y: 0 };
        let params = sys::xess_vk_execute_params_t {
            colorTexture: view_info(&inputs.color),
            velocityTexture: view_info(&inputs.motion),
            depthTexture: view_info(&inputs.depth),
            exposureScaleTexture: view_info(&inputs.exposure),
            responsivePixelMaskTexture: view_info(&inputs.reactive),
            outputTexture: view_info(&out.image),
            jitterOffsetX: inputs.jitter[0],
            jitterOffsetY: inputs.jitter[1],
            exposureScale: 1.0,
            resetHistory: u32::from(self.reset || inputs.reset),
            inputWidth: inputs.render_size.width,
            inputHeight: inputs.render_size.height,
            inputColorBase: zero,
            inputMotionVectorBase: zero,
            inputDepthBase: zero,
            inputResponsiveMaskBase: zero,
            reserved0: zero,
            outputColorBase: zero,
        };
        let image = out.image.image;
        let before = [output_before(image)];
        let internal = [internal_barrier()];
        // SAFETY: `cmd` is recording; the image barrier names our output, which the graph
        // left in GENERAL (the `record` contract).
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&internal)
                    .image_memory_barriers(&before),
            );
        }
        // SAFETY: `cmd` is recording on the context's device; the parameters are valid for
        // the call; every image and view stays alive and in the stated layout until the
        // command buffer finishes (the `record` contract).
        let result = unsafe {
            self.api
                .xessVKExecute(self.context, handle(cmd.as_raw()), &raw const params)
        };
        let after = [output_after(image)];
        // SAFETY: as for the barrier before the execution.
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&after),
            );
        }
        if result == sys::XESS_RESULT_SUCCESS {
            self.reset = false;
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures <= 3 {
                eprintln!(
                    "[render] XeSS execution failed (xess_result_t {result}; render {}x{}, \
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
        _max_render: Extent,
    ) -> crate::error::RenderResult<()> {
        self.create(output)
            .map_err(crate::error::RenderError::NoSuitableDevice)
    }

    fn vram_bytes(&self) -> u64 {
        // XeSS reports no memory query in its Vulkan API; its allocations are its own.
        0
    }
}

/// The handles above cross the FFI as pointers of the same size.
const _: () = {
    assert!(size_of::<sys::VkImageView>() == size_of::<vk::ImageView>());
    assert!(size_of::<sys::VkImage>() == size_of::<vk::Image>());
    assert!(size_of::<sys::VkCommandBuffer>() == size_of::<vk::CommandBuffer>());
    assert!(size_of::<sys::VkDevice>() == size_of::<vk::Device>());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_to_the_nearest_quality_setting() {
        let settings: Vec<_> = UpscaleMode::ALL.iter().map(|&m| quality(m)).collect();
        assert_eq!(
            settings,
            [
                sys::XESS_QUALITY_SETTING_AA,
                sys::XESS_QUALITY_SETTING_ULTRA_QUALITY,
                sys::XESS_QUALITY_SETTING_QUALITY,
                sys::XESS_QUALITY_SETTING_BALANCED,
                sys::XESS_QUALITY_SETTING_ULTRA_PERFORMANCE,
            ]
        );
        assert_eq!(init_flags(), (1 << 2) | (1 << 3));
    }
}
