//! DLSS super resolution behind [`TemporalUpscaler`] (ADR-0021 §4b, N1c), NGX direct.
//!
//! - **Creation** (with the renderer, never mid-match): a parameter map from
//!   `NVSDK_NGX_VULKAN_GetCapabilityParameters` (the one that carries DLSS's optimal-settings
//!   and statistics callbacks), the optimal-settings query for the output and every mode, then
//!   `NVSDK_NGX_VULKAN_CreateFeature1` recorded into a one-time command buffer and waited for.
//!   The feature is created at the mode's optimal render size, "exactly as given" (DLSS guide
//!   §3.2.2), with linear HDR input, render-resolution motion vectors without jitter and a
//!   non-inverted depth: flags `IsHDR | MVLowRes`.
//! - **The SDK's `static inline` helpers** (`NGX_VULKAN_CREATE_DLSS_EXT1`,
//!   `NGX_VULKAN_EVALUATE_DLSS_EXT`, `NGX_DLSS_GET_OPTIMAL_SETTINGS`, `NGX_DLSS_GET_STATS`)
//!   have no symbol to bind. They only set named parameters and call the exported functions,
//!   so this module sets the same parameters through the exported accessors.
//! - **Each frame** `NVSDK_NGX_VULKAN_EvaluateFeature_C` records DLSS's dispatches: colour,
//!   depth, motion (scale 1, 1), the 1×1 linear exposure and the output, the jitter in render
//!   pixels, the render sub-rectangle at the top-left of every input, the reset flag and the
//!   frame time. No reactive or transparency mask: `BiasCurrentColor` must not be used with
//!   the current models (guide §3.15). DLSS's own sharpening is deprecated (sharpness 0; the
//!   renderer's `sharpen` pass runs instead when a sharpness is chosen).
//! - **Resource states** (guide §3.4): the inputs are sampled images in
//!   `SHADER_READ_ONLY_OPTIMAL` and the output a storage image in `GENERAL`, as the render graph
//!   leaves them before the `temporal` pass; DLSS returns them to those states.
//! - **Command-buffer state**: NGX changes the command buffer's bound state (guide §5.2.5).
//!   Every pass the graph records afterwards binds its own pipeline, descriptor sets and push
//!   constants, so nothing of ours is assumed to survive the call.
//! - **Dynamic resolution** (§4c): the render size may vary between the minimum and maximum
//!   DLSS reports for the mode (inclusive), which [`TemporalUpscaler::render_size_range`]
//!   returns; the renderer clamps every frame's size to it. Where DLSS reports one size (no
//!   dynamic resolution for that mode), the range is that size.
//! - **Frames in flight:** DLSS keeps its own per-evaluation state; a feature is released only
//!   when no command buffer that evaluated it is pending (the device is idle on resize and
//!   drop).
//! - **Reflex** (`VK_NV_low_latency2`, N1d) needs nothing from DLSS super resolution: NGX's SR
//!   evaluation takes no latency markers (only frame generation, through Streamline in N9,
//!   does). The renderer's markers bracket the whole frame's recording and submission, DLSS
//!   included.
//!
//! - **A record that is never submitted** still advanced the SDK's own internal state (its
//!   history and frame counter live inside the SDK, which recorded its work into our command
//!   buffer). `reset_history` asks the next evaluation for a reset, which covers the history;
//!   anything else the SDK tracks is outside our control, so a caller that drops a recorded
//!   command buffer must treat it as the `record` contract's fatal case and drop the back end
//!   (the renderer submits every recorded frame, and a failed submission is fatal).

use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::Arc;

use ash::vk::{self, Handle as _};

use super::{SDK_VERSION, Session, handle, result_name, succeeded, sys};
use crate::upscale::{
    Extent, TemporalUpscaler, UpscaleImage, UpscaleInputs, UpscaleMode, UpscaleOutput, UpscalerKind,
};
use crate::vendor::barriers::{inputs_after, internal_barrier, output_after, output_before};
use crate::vendor::{Unavailable, UpscalerRequest};

/// An NGX parameter map from `GetCapabilityParameters`, destroyed on drop (before the session
/// it belongs to shuts down).
struct Params {
    raw: *mut sys::NVSDK_NGX_Parameter,
}

/// A NUL-terminated parameter name from the generated bindings (`b"…\0"` literals). Only the
/// terminator is checked (O(1); `record` names about 40 parameters a frame): C reads up to
/// the first NUL, so an interior one would only shorten the name.
fn name(bytes: &'static [u8]) -> *const c_char {
    assert_eq!(
        bytes.last(),
        Some(&0),
        "NGX parameter names are NUL-terminated string literals"
    );
    bytes.as_ptr().cast()
}

impl Params {
    fn new() -> Result<Self, Unavailable> {
        let mut raw: *mut sys::NVSDK_NGX_Parameter = ptr::null_mut();
        // SAFETY: NGX is initialised (the session is alive); the out pointer is valid.
        let result = unsafe { sys::NVSDK_NGX_VULKAN_GetCapabilityParameters(&raw mut raw) };
        if !succeeded(result) || raw.is_null() {
            return Err(format!(
                "NGX capability parameters failed ({})",
                result_name(result)
            ));
        }
        Ok(Self { raw })
    }

    fn set_ui(&self, key: &'static [u8], value: u32) {
        // SAFETY: the map is live; the name is NUL-terminated and NGX copies the value.
        unsafe { sys::NVSDK_NGX_Parameter_SetUI(self.raw, name(key), value) };
    }

    fn set_i(&self, key: &'static [u8], value: i32) {
        // SAFETY: as for `set_ui`.
        unsafe { sys::NVSDK_NGX_Parameter_SetI(self.raw, name(key), value) };
    }

    fn set_f(&self, key: &'static [u8], value: f32) {
        // SAFETY: as for `set_ui`.
        unsafe { sys::NVSDK_NGX_Parameter_SetF(self.raw, name(key), value) };
    }

    /// Stores a pointer; NGX reads through it only during the next create or evaluate call,
    /// so the pointee must live until that call returns. The map keeps the pointer value
    /// afterwards, so every caller nulls it again before the pointee goes out of scope
    /// (`record` clears the resource pointers right after the evaluation); a stale pointer
    /// is never left in the map for a later call to read.
    fn set_ptr(&self, key: &'static [u8], value: *mut c_void) {
        // SAFETY: as for `set_ui`; NGX stores the pointer value, not the pointee.
        unsafe { sys::NVSDK_NGX_Parameter_SetVoidPointer(self.raw, name(key), value) };
    }

    fn get_ui(&self, key: &'static [u8]) -> Option<u32> {
        let mut value = 0u32;
        // SAFETY: the map is live; the name is NUL-terminated; `value` outlives the call.
        let result = unsafe { sys::NVSDK_NGX_Parameter_GetUI(self.raw, name(key), &raw mut value) };
        succeeded(result).then_some(value)
    }

    fn get_ull(&self, key: &'static [u8]) -> Option<u64> {
        let mut value: std::ffi::c_ulonglong = 0;
        // SAFETY: as for `get_ui`.
        let result =
            unsafe { sys::NVSDK_NGX_Parameter_GetULL(self.raw, name(key), &raw mut value) };
        succeeded(result).then_some(value)
    }

    fn get_ptr(&self, key: &'static [u8]) -> Option<*mut c_void> {
        let mut value: *mut c_void = ptr::null_mut();
        // SAFETY: as for `get_ui`.
        let result =
            unsafe { sys::NVSDK_NGX_Parameter_GetVoidPointer(self.raw, name(key), &raw mut value) };
        (succeeded(result) && !value.is_null()).then_some(value)
    }

    /// Calls the DLSS callback stored under `key` (optimal settings, statistics) on this map.
    fn call(&self, key: &'static [u8], what: &str) -> Result<(), Unavailable> {
        let callback = self.get_ptr(key).ok_or_else(|| {
            format!(
                "the DLSS feature binary has no {what} callback (it is older than DLSS \
                 {SDK_VERSION} expects; NVSDK_NGX_Result_FAIL_OutOfDate)"
            )
        })?;
        // SAFETY: NGX stores a `PFN_NVSDK_NGX_DLSS_*Callback` (one parameter-map argument,
        // returning a result) under these keys; a data pointer and a function pointer have the
        // same size and representation on every target `dlss` builds for (checked below).
        let callback = unsafe {
            std::mem::transmute::<*mut c_void, sys::PFN_NVSDK_NGX_DLSS_GetOptimalSettingsCallback>(
                callback,
            )
        };
        let callback = callback.ok_or_else(|| format!("the DLSS {what} callback is null"))?;
        // SAFETY: the callback belongs to the loaded feature binary and takes this live map.
        let result = unsafe { callback(self.raw) };
        if succeeded(result) {
            Ok(())
        } else {
            Err(format!("DLSS {what} failed ({})", result_name(result)))
        }
    }
}

impl Drop for Params {
    fn drop(&mut self) {
        // SAFETY: the map came from GetCapabilityParameters and is destroyed once, with the
        // matching function, while NGX is still initialised (the session outlives it).
        unsafe { sys::NVSDK_NGX_VULKAN_DestroyParameters(self.raw) };
    }
}

/// A created DLSS feature, released on drop (the device is idle then: the renderer waits for
/// it before resizing or dropping its temporal state).
struct Feature {
    raw: *mut sys::NVSDK_NGX_Handle,
}

impl Drop for Feature {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFeature1 and is released once; no command buffer
        // that evaluated it is pending (the device is idle on resize and drop).
        unsafe { sys::NVSDK_NGX_VULKAN_ReleaseFeature(self.raw) };
    }
}

/// NGX's quality value for one of our modes (ADR-0021 §4b: the nearest NGX mode by ratio).
fn perf_quality(mode: UpscaleMode) -> sys::NVSDK_NGX_PerfQuality_Value {
    match mode {
        UpscaleMode::Native => sys::NVSDK_NGX_PerfQuality_Value_DLAA,
        UpscaleMode::Quality => sys::NVSDK_NGX_PerfQuality_Value_MaxQuality,
        UpscaleMode::Balanced => sys::NVSDK_NGX_PerfQuality_Value_Balanced,
        UpscaleMode::Performance => sys::NVSDK_NGX_PerfQuality_Value_MaxPerf,
        UpscaleMode::UltraPerformance => sys::NVSDK_NGX_PerfQuality_Value_UltraPerformance,
    }
}

/// A C enum constant as the `int` NGX's `SetI` takes. Some NGX enums have values above
/// `INT_MAX`, so bindgen makes them `unsigned int` for GCC and Clang but `int` for MSVC; the
/// bits are passed through either way.
fn as_int<T: Into<i64>>(value: T) -> i32 {
    let value: i64 = value.into();
    i32::from_ne_bytes((value as u32).to_ne_bytes())
}

/// What DLSS's optimal-settings query returned for one mode at one output size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModeSizes {
    /// The size to create the feature with, exactly as given.
    pub(crate) optimal: Extent,
    /// The smallest and largest render size an evaluation accepts (inclusive).
    pub(crate) min: Extent,
    pub(crate) max: Extent,
}

impl ModeSizes {
    /// The range clamped to `output` and at least 1×1, with the optimal size inside it.
    pub(crate) fn sanitized(self, output: Extent) -> Self {
        let clamp = |size: Extent| Extent {
            width: size.width.clamp(1, output.width.max(1)),
            height: size.height.clamp(1, output.height.max(1)),
        };
        let optimal = clamp(self.optimal);
        let max = clamp(self.max);
        let min = clamp(self.min);
        let max = Extent {
            width: max.width.max(optimal.width),
            height: max.height.max(optimal.height),
        };
        let min = Extent {
            width: min.width.min(optimal.width),
            height: min.height.min(optimal.height),
        };
        Self { optimal, min, max }
    }
}

fn extent(width: u32, height: u32) -> Extent {
    Extent { width, height }
}

/// DLSS's optimal settings for `mode` at `output` (`NGX_DLSS_GET_OPTIMAL_SETTINGS`), or why
/// the mode is unsupported.
fn optimal_settings(
    params: &Params,
    output: Extent,
    mode: UpscaleMode,
) -> Result<ModeSizes, Unavailable> {
    params.set_ui(sys::NVSDK_NGX_Parameter_Width, output.width);
    params.set_ui(sys::NVSDK_NGX_Parameter_Height, output.height);
    params.set_i(
        sys::NVSDK_NGX_Parameter_PerfQualityValue,
        as_int(perf_quality(mode)),
    );
    // Older feature binaries still read it.
    params.set_i(sys::NVSDK_NGX_Parameter_RTXValue, 0);
    params.call(
        sys::NVSDK_NGX_Parameter_DLSSOptimalSettingsCallback,
        "optimal-settings",
    )?;
    let width = params
        .get_ui(sys::NVSDK_NGX_Parameter_OutWidth)
        .unwrap_or(0);
    let height = params
        .get_ui(sys::NVSDK_NGX_Parameter_OutHeight)
        .unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(format!(
            "DLSS does not support {} mode at {}x{}",
            mode.name(),
            output.width,
            output.height
        ));
    }
    let optimal = extent(width, height);
    // Older binaries leave the dynamic range unset: it is then the optimal size.
    let get = |key, fallback| params.get_ui(key).filter(|&v| v > 0).unwrap_or(fallback);
    let max = extent(
        get(
            sys::NVSDK_NGX_Parameter_DLSS_Get_Dynamic_Max_Render_Width,
            width,
        ),
        get(
            sys::NVSDK_NGX_Parameter_DLSS_Get_Dynamic_Max_Render_Height,
            height,
        ),
    );
    let min = extent(
        get(
            sys::NVSDK_NGX_Parameter_DLSS_Get_Dynamic_Min_Render_Width,
            width,
        ),
        get(
            sys::NVSDK_NGX_Parameter_DLSS_Get_Dynamic_Min_Render_Height,
            height,
        ),
    );
    Ok(ModeSizes { optimal, min, max }.sanitized(output))
}

/// The DLSS back end behind the trait. Fields drop in order: the feature, then the parameter
/// map, then (with the last holder) the NGX session.
pub(crate) struct DlssUpscaler {
    feature: Option<Feature>,
    device: ash::Device,
    params: Params,
    /// Keeps NGX initialised while the feature and the map exist.
    _session: Arc<Session>,
    mode: UpscaleMode,
    output: Extent,
    /// The optimal-settings answer for every mode at `output` (`None`: unsupported).
    sizes: [Option<ModeSizes>; 5],
    /// Start the next evaluation without history.
    reset: bool,
    /// DLSS's own memory (`NGX_DLSS_GET_STATS`), 0 when the binary does not report it.
    vram: u64,
    /// Evaluation failures logged so far (the first few are, then they are counted silently).
    failures: u32,
}

impl DlssUpscaler {
    /// Creates DLSS for `request` on the session's device.
    pub(crate) fn new(
        session: Arc<Session>,
        request: &UpscalerRequest<'_>,
    ) -> Result<Self, Unavailable> {
        let params = Params::new()?;
        let mut upscaler = Self {
            feature: None,
            device: request.device.device.clone(),
            params,
            _session: session,
            mode: request.mode,
            output: request.output,
            sizes: [None; 5],
            reset: true,
            vram: 0,
            failures: 0,
        };
        upscaler.create(request.device, request.output)?;
        Ok(upscaler)
    }

    /// (Re)creates the feature for `output` (the device is idle).
    fn create(
        &mut self,
        device: &crate::device::DeviceContext,
        output: Extent,
    ) -> Result<(), Unavailable> {
        // Released first: NGX caches its memory unless asked to free it, and a resize must not
        // hold two features' memory.
        self.feature = None;
        let output = extent(output.width.max(1), output.height.max(1));
        let answers: [Result<ModeSizes, Unavailable>; 5] =
            std::array::from_fn(|i| optimal_settings(&self.params, output, UpscaleMode::ALL[i]));
        let chosen = answers[mode_index(self.mode)].clone()?;
        let sizes = answers.map(Result::ok);
        let params = &self.params;
        params.set_ui(sys::NVSDK_NGX_Parameter_CreationNodeMask, 1);
        params.set_ui(sys::NVSDK_NGX_Parameter_VisibilityNodeMask, 1);
        params.set_ui(sys::NVSDK_NGX_Parameter_Width, chosen.optimal.width);
        params.set_ui(sys::NVSDK_NGX_Parameter_Height, chosen.optimal.height);
        params.set_ui(sys::NVSDK_NGX_Parameter_OutWidth, output.width);
        params.set_ui(sys::NVSDK_NGX_Parameter_OutHeight, output.height);
        params.set_i(
            sys::NVSDK_NGX_Parameter_PerfQualityValue,
            as_int(perf_quality(self.mode)),
        );
        params.set_i(
            sys::NVSDK_NGX_Parameter_DLSS_Feature_Create_Flags,
            create_flags(),
        );
        params.set_i(sys::NVSDK_NGX_Parameter_DLSS_Enable_Output_Subrects, 0);
        params.set_i(sys::NVSDK_NGX_Parameter_FreeMemOnReleaseFeature, 1);
        let mut raw: *mut sys::NVSDK_NGX_Handle = ptr::null_mut();
        let mut result = sys::NVSDK_NGX_Result_Fail;
        let device_handle = device.device.handle().as_raw();
        let submitted = crate::bindless::submit_one_shot(device, |cmd| {
            // SAFETY: NGX is initialised on this device (the session is alive); `cmd` is a
            // command buffer of this device in the recording state, submitted once and waited
            // for before the feature is used; the parameter map is live and holds only values.
            result = unsafe {
                sys::NVSDK_NGX_VULKAN_CreateFeature1(
                    handle(device_handle),
                    handle(cmd.as_raw()),
                    sys::NVSDK_NGX_Feature_SuperSampling,
                    params.raw,
                    &raw mut raw,
                )
            };
        });
        // Owned at once, so a feature NGX made is released whatever happens next.
        let feature = (!raw.is_null()).then_some(Feature { raw });
        submitted.map_err(|error| format!("submitting DLSS feature creation failed ({error})"))?;
        let Some(feature) = feature.filter(|_| succeeded(result)) else {
            return Err(format!(
                "creating the DLSS feature ({} mode, {}x{} to {}x{}) failed ({})",
                self.mode.name(),
                chosen.optimal.width,
                chosen.optimal.height,
                output.width,
                output.height,
                result_name(result)
            ));
        };
        self.feature = Some(feature);
        self.sizes = sizes;
        self.output = output;
        self.reset = true;
        self.vram = self.stats();
        Ok(())
    }

    /// DLSS's allocated memory (`NGX_DLSS_GET_STATS`), 0 if not reported.
    fn stats(&self) -> u64 {
        if self
            .params
            .call(sys::NVSDK_NGX_Parameter_DLSSGetStatsCallback, "statistics")
            .is_err()
        {
            return 0;
        }
        self.params
            .get_ull(sys::NVSDK_NGX_Parameter_SizeInBytes)
            .unwrap_or(0)
    }

    /// The sizes DLSS reported for `mode` at the current output.
    pub(crate) fn sizes(&self, mode: UpscaleMode) -> Option<ModeSizes> {
        self.sizes[mode_index(mode)]
    }
}

/// `IsHDR | MVLowRes`: linear HDR colour, motion vectors at render resolution without jitter
/// (ADR-0021 §2a), non-inverted depth, an exposure texture rather than auto-exposure (presets J
/// and K read it; L, and possibly M, auto-expose regardless, guide §3.9).
fn create_flags() -> i32 {
    as_int(sys::NVSDK_NGX_DLSS_Feature_Flags_IsHDR | sys::NVSDK_NGX_DLSS_Feature_Flags_MVLowRes)
}

fn mode_index(mode: UpscaleMode) -> usize {
    UpscaleMode::ALL
        .iter()
        .position(|&m| m == mode)
        .expect("every mode is in ALL")
}

/// The aspect of an input image's view: depth for the depth formats, else colour.
fn aspect(format: vk::Format) -> vk::ImageAspectFlags {
    match format {
        vk::Format::D32_SFLOAT | vk::Format::D16_UNORM | vk::Format::X8_D24_UNORM_PACK32 => {
            vk::ImageAspectFlags::DEPTH
        }
        _ => vk::ImageAspectFlags::COLOR,
    }
}

/// NGX's description of one of our images (the whole allocation; the render sub-rectangle is
/// a separate parameter).
fn resource(image: &UpscaleImage, read_write: bool) -> sys::NVSDK_NGX_Resource_VK {
    sys::NVSDK_NGX_Resource_VK {
        Resource: sys::NVSDK_NGX_Resource_VK__bindgen_ty_1 {
            ImageViewInfo: sys::NVSDK_NGX_ImageViewInfo_VK {
                ImageView: handle(image.view.as_raw()),
                Image: handle(image.image.as_raw()),
                SubresourceRange: sys::VkImageSubresourceRange {
                    aspectMask: aspect(image.format).as_raw(),
                    baseMipLevel: 0,
                    levelCount: 1,
                    baseArrayLayer: 0,
                    layerCount: 1,
                },
                Format: image.format.as_raw(),
                Width: image.extent.width,
                Height: image.extent.height,
            },
        },
        Type: sys::NVSDK_NGX_RESOURCE_VK_TYPE_VK_IMAGEVIEW,
        ReadWrite: read_write,
    }
}

impl TemporalUpscaler for DlssUpscaler {
    fn kind(&self) -> UpscalerKind {
        UpscalerKind::Dlss
    }

    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent) {
        match self.sizes(mode) {
            Some(sizes) if output == self.output => (sizes.min, sizes.max),
            // Another output than the feature's (never asked by the renderer, which resizes
            // first): TAAU's rule, as a stand-in.
            _ => crate::upscale::scaled_range(output, mode, crate::post::MIN_TEMPORAL_RENDER_SCALE),
        }
    }

    fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
        // ADR-0021 §1: ceil(8 r²), which meets DLSS's minimums (24 at 1.724×).
        crate::temporal::jitter_phases(render, output)
    }

    fn has_sharpening(&self) -> bool {
        false
    }

    #[allow(clippy::too_many_lines, reason = "one NGX parameter per line")]
    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    ) {
        let Some(feature) = self.feature.as_ref() else {
            return;
        };
        let mut color = resource(&inputs.color, false);
        let mut depth = resource(&inputs.depth, false);
        let mut motion = resource(&inputs.motion, false);
        let mut exposure = resource(&inputs.exposure, false);
        let mut output = resource(&out.image, true);
        let p = &self.params;
        p.set_ptr(sys::NVSDK_NGX_Parameter_Color, (&raw mut color).cast());
        p.set_ptr(sys::NVSDK_NGX_Parameter_Output, (&raw mut output).cast());
        p.set_ptr(sys::NVSDK_NGX_Parameter_Depth, (&raw mut depth).cast());
        p.set_ptr(
            sys::NVSDK_NGX_Parameter_MotionVectors,
            (&raw mut motion).cast(),
        );
        p.set_ptr(
            sys::NVSDK_NGX_Parameter_ExposureTexture,
            (&raw mut exposure).cast(),
        );
        // The jitter in render pixels, x right and y down: the motion vectors' convention
        // (guide §3.7.3), and FSR's (ADR-0021 amendment 1).
        p.set_f(sys::NVSDK_NGX_Parameter_Jitter_Offset_X, inputs.jitter[0]);
        p.set_f(sys::NVSDK_NGX_Parameter_Jitter_Offset_Y, inputs.jitter[1]);
        p.set_f(sys::NVSDK_NGX_Parameter_Sharpness, 0.0);
        p.set_i(
            sys::NVSDK_NGX_Parameter_Reset,
            i32::from(self.reset || inputs.reset),
        );
        p.set_f(sys::NVSDK_NGX_Parameter_MV_Scale_X, 1.0);
        p.set_f(sys::NVSDK_NGX_Parameter_MV_Scale_Y, 1.0);
        for key in [
            &sys::NVSDK_NGX_Parameter_DLSS_Input_Color_Subrect_Base_X[..],
            sys::NVSDK_NGX_Parameter_DLSS_Input_Color_Subrect_Base_Y,
            sys::NVSDK_NGX_Parameter_DLSS_Input_Depth_Subrect_Base_X,
            sys::NVSDK_NGX_Parameter_DLSS_Input_Depth_Subrect_Base_Y,
            sys::NVSDK_NGX_Parameter_DLSS_Input_MV_SubrectBase_X,
            sys::NVSDK_NGX_Parameter_DLSS_Input_MV_SubrectBase_Y,
            sys::NVSDK_NGX_Parameter_DLSS_Output_Subrect_Base_X,
            sys::NVSDK_NGX_Parameter_DLSS_Output_Subrect_Base_Y,
        ] {
            p.set_ui(key, 0);
        }
        p.set_ui(
            sys::NVSDK_NGX_Parameter_DLSS_Render_Subrect_Dimensions_Width,
            inputs.render_size.width,
        );
        p.set_ui(
            sys::NVSDK_NGX_Parameter_DLSS_Render_Subrect_Dimensions_Height,
            inputs.render_size.height,
        );
        p.set_f(sys::NVSDK_NGX_Parameter_DLSS_Pre_Exposure, 1.0);
        p.set_f(sys::NVSDK_NGX_Parameter_DLSS_Exposure_Scale, 1.0);
        p.set_f(sys::NVSDK_NGX_Parameter_FrameTimeDeltaInMsec, inputs.dt_ms);
        let image = out.image.image;
        let before = [output_before(image)];
        let internal = [internal_barrier()];
        // SAFETY: `cmd` is recording; the image barrier names our output image, which the
        // graph left in GENERAL (the `record` contract).
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&internal)
                    .image_memory_barriers(&before),
            );
        }
        // SAFETY: `cmd` is recording on the device NGX was initialised with; the feature and
        // the parameter map are live; the resource descriptions the map points at live on this
        // stack until the call returns (NGX reads them during the call only); the images and
        // views stay alive and in the stated layouts until the command buffer finishes (the
        // `record` contract).
        let result = unsafe {
            sys::NVSDK_NGX_VULKAN_EvaluateFeature_C(handle(cmd.as_raw()), feature.raw, p.raw, None)
        };
        let after = [output_after(image)];
        let sdk_after = [inputs_after()];
        // SAFETY: as for the barrier before the evaluation.
        unsafe {
            self.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&sdk_after)
                    .image_memory_barriers(&after),
            );
        }
        // The descriptions are about to go out of scope: clear the pointers to them.
        for key in [
            &sys::NVSDK_NGX_Parameter_Color[..],
            sys::NVSDK_NGX_Parameter_Output,
            sys::NVSDK_NGX_Parameter_Depth,
            sys::NVSDK_NGX_Parameter_MotionVectors,
            sys::NVSDK_NGX_Parameter_ExposureTexture,
        ] {
            p.set_ptr(key, ptr::null_mut());
        }
        if succeeded(result) {
            self.reset = false;
        } else {
            // The output is not written this frame. A bug, not a device condition: log the
            // first few and keep going (the next frame starts from no history).
            self.failures = self.failures.saturating_add(1);
            if self.failures <= 3 {
                eprintln!(
                    "[render] DLSS evaluation failed ({}; render {}x{}, output {}x{}){}",
                    result_name(result),
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

    unsafe fn resize(
        &mut self,
        device: &crate::device::DeviceContext,
        output: Extent,
        _max_render: Extent,
    ) -> crate::error::RenderResult<()> {
        // DLSS must be re-created for a new output size (guide §3.2.2).
        self.create(device, output)
            .map_err(crate::error::RenderError::Upscaler)
    }

    fn sized_by_max_render(&self) -> bool {
        // Created for the output only; any render size in its range is accepted.
        false
    }

    fn vram_bytes(&self) -> u64 {
        self.vram
    }

    fn sdk_bytes(&self) -> u64 {
        // All of it: the SDK allocates its own memory.
        self.vram_bytes()
    }
}

/// The callback transmute above relies on data and function pointers having one size.
const _: () = {
    assert!(
        size_of::<*mut c_void>() == size_of::<sys::PFN_NVSDK_NGX_DLSS_GetOptimalSettingsCallback>()
    );
    assert!(size_of::<sys::VkImageView>() == size_of::<vk::ImageView>());
    assert!(size_of::<sys::VkImage>() == size_of::<vk::Image>());
    assert!(size_of::<sys::VkCommandBuffer>() == size_of::<vk::CommandBuffer>());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_to_the_nearest_ngx_mode() {
        assert_eq!(
            perf_quality(UpscaleMode::Native),
            sys::NVSDK_NGX_PerfQuality_Value_DLAA
        );
        assert_eq!(
            perf_quality(UpscaleMode::Quality),
            sys::NVSDK_NGX_PerfQuality_Value_MaxQuality
        );
        assert_eq!(
            perf_quality(UpscaleMode::Balanced),
            sys::NVSDK_NGX_PerfQuality_Value_Balanced
        );
        assert_eq!(
            perf_quality(UpscaleMode::Performance),
            sys::NVSDK_NGX_PerfQuality_Value_MaxPerf
        );
        assert_eq!(
            perf_quality(UpscaleMode::UltraPerformance),
            sys::NVSDK_NGX_PerfQuality_Value_UltraPerformance
        );
    }

    #[test]
    fn create_flags_are_hdr_and_low_res_motion_only() {
        assert_eq!(create_flags(), 0b11);
    }

    /// The reported range always holds the optimal size and stays inside the output.
    #[test]
    fn mode_sizes_are_sanitized() {
        let output = extent(2560, 1440);
        let sizes = ModeSizes {
            optimal: extent(1708, 960),
            min: extent(1280, 720),
            max: extent(4000, 2000),
        }
        .sanitized(output);
        assert_eq!(sizes.max, output);
        assert_eq!(sizes.min, extent(1280, 720));
        // An older binary reports only the optimal size: the range is that size.
        let fixed = ModeSizes {
            optimal: extent(1708, 960),
            min: extent(1708, 960),
            max: extent(1708, 960),
        }
        .sanitized(output);
        assert_eq!((fixed.min, fixed.max), (fixed.optimal, fixed.optimal));
        // A minimum above the optimal size is pulled down to it.
        let odd = ModeSizes {
            optimal: extent(1000, 500),
            min: extent(1200, 600),
            max: extent(900, 400),
        }
        .sanitized(output);
        assert_eq!((odd.min, odd.max), (odd.optimal, odd.optimal));
    }

    #[test]
    fn depth_inputs_use_the_depth_aspect() {
        assert_eq!(aspect(vk::Format::D32_SFLOAT), vk::ImageAspectFlags::DEPTH);
        assert_eq!(aspect(vk::Format::D16_UNORM), vk::ImageAspectFlags::DEPTH);
        assert_eq!(
            aspect(vk::Format::R16G16_SFLOAT),
            vk::ImageAspectFlags::COLOR
        );
    }
}
