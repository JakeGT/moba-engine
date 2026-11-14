//! The temporal upscaler interface (ADR-0021 §4): one trait over the in-house TAAU, FSR 3.1,
//! DLSS, FSR 4 and XeSS, chosen at renderer creation. Vendor code stays behind
//! `crate::vendor` (ADR-0020 §6a); call sites never `cfg`.
//!
//! **Inputs** ([`UpscaleInputs`]) are all at render resolution, in the render sub-rectangle
//! at the top-left of their allocation: the jittered linear HDR colour, depth (`D32_SFLOAT`,
//! not inverted, finite far plane), `motion` (`R16G16_SFLOAT`, render pixels from the current
//! sample to its previous position, x right and y down, without jitter: every back end takes
//! it with a scale of (1, 1)), `reactive` and `tc_mask` (`R8_UNORM`), a 1×1 `R32_SFLOAT`
//! linear exposure multiplier, and the frame's scalars. The graph moves each input to
//! `SHADER_READ_ONLY_OPTIMAL` and the output ([`UpscaleOutput`], `RGBA16F` at output
//! resolution) to `GENERAL` before the `temporal` pass; a back end records only its own work
//! (its dispatches and the barriers of the resources it owns, such as its history).
//!
//! **Contexts** are created with the renderer at the maximum render size of the
//! dynamic-resolution range. A swapchain resize only re-sizes their images
//! ([`TemporalUpscaler::resize`]); pipelines are built at creation and on a settings change,
//! never mid-match (`locked_pipelines=0`).

pub(crate) mod exposure;
pub(crate) mod frame;
pub(crate) mod sharpen;
pub(crate) mod taau;

use ash::vk;

pub(crate) use crate::post::{UpscaleMode, UpscalerKind};
pub(crate) use crate::temporal::Extent;

/// One image the upscaler reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UpscaleImage {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    pub(crate) format: vk::Format,
    /// The image's allocation (the valid region is [`UpscaleInputs::render_size`] for the
    /// inputs, the whole image for the output).
    pub(crate) extent: Extent,
}

/// This frame's inputs (ADR-0021 §4a), all at render resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UpscaleInputs {
    /// The jittered linear HDR colour.
    pub(crate) color: UpscaleImage,
    /// Depth, `D32_SFLOAT`, 0 near, 1 far.
    pub(crate) depth: UpscaleImage,
    /// Motion vectors, `R16G16_SFLOAT`, render pixels to the previous position.
    pub(crate) motion: UpscaleImage,
    /// FSR's reactive mask (`R8_UNORM`): particles and distortion.
    pub(crate) reactive: UpscaleImage,
    /// FSR's transparency-and-composition mask (`R8_UNORM`): water and animated-UV
    /// materials.
    pub(crate) transparency: UpscaleImage,
    /// 1×1 `R32_SFLOAT` linear exposure multiplier (`exposure_linear`, ADR-0021 §4a).
    pub(crate) exposure: UpscaleImage,
    /// The jitter in render pixels, x right and y down.
    pub(crate) jitter: [f32; 2],
    /// The render size this frame (the valid sub-rectangle of every input).
    pub(crate) render_size: Extent,
    /// Frame time in milliseconds.
    pub(crate) dt_ms: f32,
    /// The camera's near and far planes and vertical field of view (radians).
    pub(crate) near: f32,
    pub(crate) far: f32,
    pub(crate) fov_y: f32,
    /// Start from the current frame: no history.
    pub(crate) reset: bool,
    /// The shared sharpening amount, `0..=1`.
    pub(crate) sharpness: f32,
    /// The frame-in-flight slot being recorded (`< FRAMES_IN_FLIGHT`). The caller waited on
    /// this slot's previous submission, so per-slot state (descriptor sets, constants) is
    /// free to rewrite, also after recorded frames were dropped unsubmitted.
    pub(crate) frame_slot: usize,
}

/// The upscaler's output: `hdr_out`, `RGBA16F` storage at output resolution, in `GENERAL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UpscaleOutput {
    pub(crate) image: UpscaleImage,
}

/// One temporal upscaler back end (ADR-0021 §4a). Exactly this shape: FSR 3.1 (N1b) and the
/// vendor back ends (N1c) implement it too, so a change is an amendment to the ADR.
pub(crate) trait TemporalUpscaler {
    /// Which back end this is.
    fn kind(&self) -> UpscalerKind; // Taau, Fsr3, Dlss, Fsr4, Xess
    /// The smallest and largest render size this back end accepts for `output` in `mode`.
    fn render_size_range(&self, output: Extent, mode: UpscaleMode) -> (Extent, Extent);
    /// Jitter phases for this frame's sizes.
    fn jitter_phases(&self, render: Extent, output: Extent) -> u32;
    /// Whether the back end sharpens itself (otherwise the renderer's `sharpen` pass runs).
    fn has_sharpening(&self) -> bool;
    /// Records the back end's work for this frame into `cmd`.
    ///
    /// # Safety
    ///
    /// The same contract as FSR 3.1's `Fsr3::record` (ADR-0021 amendment 2, item 17):
    /// - `cmd` is a command buffer of the back end's device in the recording state.
    /// - **Every command buffer recorded here is submitted exactly once, in `record` order, on
    ///   one queue.** A back end advances its own state when it records (history parity,
    ///   first-use layout transitions) and orders each frame only against earlier commands
    ///   on the same queue (a history reset's re-initialisation waits for every earlier
    ///   compute write and read of its images, amendment 2 item 28). A caller that
    ///   drops a recorded command buffer unsubmitted (a failure between recording and
    ///   submission) calls [`Self::reset_history`] before the next `record`, which makes that
    ///   record a first frame again, or treats the failure as fatal and drops the back end.
    /// - **At most `FRAMES_IN_FLIGHT` (2) records are pending, and `inputs.frame_slot`'s last
    ///   submission has finished.** Per-frame state (descriptor sets, constants) is keyed by
    ///   the slot, not by a ring of records, so frames dropped unsubmitted cannot make a
    ///   rewrite land on a set a pending frame reads.
    /// - Every image and view in `inputs` and `out` stays alive until this command buffer has
    ///   finished executing, in the layouts the module documentation states.
    unsafe fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        inputs: &UpscaleInputs,
        out: &UpscaleOutput,
    );
    /// Forgets every history: the next frame starts from its current sample, as a first
    /// frame (its own images are re-initialised, whatever was or was not submitted before).
    fn reset_history(&mut self);
    /// Re-sizes the back end's own images for a new `output` and dynamic-resolution ceiling
    /// `max_render` (a swapchain resize) without building pipelines (`locked_pipelines=0`,
    /// ADR-0021 amendment 2, item 18). Forgets every history.
    ///
    /// # Safety
    ///
    /// The device is idle: no command buffer recorded by [`Self::record`] is pending. The
    /// back end drops its old images and rewrites its descriptor sets here, which a pending
    /// frame may still use (ADR-0021 amendment 4).
    unsafe fn resize(
        &mut self,
        device: &crate::device::DeviceContext,
        output: Extent,
        max_render: Extent,
    ) -> crate::error::RenderResult<()>;
    /// Whether [`Self::resize`]'s `max_render` sizes anything: false for a back end that
    /// accepts any render size up to the output whatever it was created with (DLSS, XeSS), so
    /// the renderer does not re-create it only to raise its render allocation (ADR-0021
    /// amendment 5).
    fn sized_by_max_render(&self) -> bool {
        true
    }
    /// Device memory the back end owns (its histories and internal targets).
    fn vram_bytes(&self) -> u64;
    /// The part of [`Self::vram_bytes`] a vendor SDK allocated itself, outside the renderer's
    /// allocator and so outside its memory accounting (ADR-0021 amendment 5); the renderer
    /// adds it to the frame log's VRAM figures. 0 for our own back ends.
    fn sdk_bytes(&self) -> u64 {
        0
    }
    /// Debug builds' shader hot reload (`MOBA_SHADER_DIR`): rebuilds the back end's pipelines
    /// from `shaders`, replacing them only on success. The device is idle. A back end without
    /// shaders of ours (the vendor SDKs) keeps the default, which does nothing.
    fn reload_shaders(
        &mut self,
        _device: &crate::device::DeviceContext,
        _shaders: &crate::shaders::ShaderSource,
    ) -> crate::error::RenderResult<()> {
        Ok(())
    }
}

/// The render-size range for `mode` at `output` with a dynamic-resolution floor of `floor`
/// (a render scale): the mode's size and the floor's, both at least 1×1 and with the output's
/// aspect ratio (the controller scales both axes together, ADR-0021 §4c). TAAU's rule; vendor
/// back ends clamp it to what their SDK reports.
///
/// **Rounding is deliberate** (ADR-0021 amendment 2, item 19): sizes round to nearest, the
/// same rule as the dynamic-resolution controller (`dynres::render_extent`), so TAAU's
/// ceiling is exactly a size the controller can produce. Quality at 2560 is 1707 here; FSR
/// 3.1's SDK truncates (1706). A back end with an SDK rule reports its own range, and the
/// controller clamps to it.
pub(crate) fn scaled_range(output: Extent, mode: UpscaleMode, floor: f32) -> (Extent, Extent) {
    let size = |scale: f32| crate::dynres::render_extent(output, output, scale);
    let ceiling = mode.scale();
    (size(floor.min(ceiling)), size(ceiling))
}

/// Extra usage of the temporal output and the `sharpen` intermediate a vendor SDK writes
/// into: `TRANSFER_DST` (NGX clears its output with `vkCmdClearColorImage`, ADR-0021
/// amendment 5) in a build with a vendor upscaler, none in the open build.
pub(crate) const VENDOR_OUTPUT_USAGE: ash::vk::ImageUsageFlags =
    if cfg!(any(feature = "dlss", feature = "fsr4", feature = "xess")) {
        ash::vk::ImageUsageFlags::TRANSFER_DST
    } else {
        ash::vk::ImageUsageFlags::empty()
    };

/// `auto`'s back end in the open build, after the vendor back ends (ADR-0021 §4b and Q1).
/// FSR 3.1 takes the slot only once its port passes the readability gates and costs within
/// 0.2 ms of TAAU on the RTX 5070 Ti; the Q1 measurement is in perf/0033 (amendment 4).
pub(crate) const AUTO_OPEN_BUILD: UpscalerKind = UpscalerKind::Taau;

/// What `MOBA_UPSCALER` asked for among the built-in back ends: `auto` resolves to
/// [`AUTO_OPEN_BUILD`] (TAAU when that is FSR 3.1 and `fsr3` is not compiled in), `taau` to
/// TAAU, and `fsr3` to FSR 3.1 when the `fsr3` feature is compiled in, else TAAU with one log
/// line. The vendor back ends (DLSS, FSR 4, XeSS) are [`create`]'s, through the vendor
/// selection; they resolve to TAAU here, their fallback. Whether the chosen back end can run
/// on this device is [`create`]'s question.
#[must_use]
pub(crate) fn resolve(requested: Option<UpscalerKind>) -> UpscalerKind {
    let fsr3 = cfg!(feature = "fsr3");
    match requested {
        None if AUTO_OPEN_BUILD == UpscalerKind::Fsr3 && !fsr3 => UpscalerKind::Taau,
        None => AUTO_OPEN_BUILD,
        Some(UpscalerKind::Fsr3) if fsr3 => UpscalerKind::Fsr3,
        Some(UpscalerKind::Fsr3) => {
            eprintln!(
                "[render] upscaler fsr3 is not available in this build (the `fsr3` feature is \
                 not compiled in); using taau"
            );
            UpscalerKind::Taau
        }
        Some(_) => UpscalerKind::Taau,
    }
}

/// Which vendor back end `auto` takes, given the one the vendor selection chose for the
/// upscaler slot (`chosen`: `dlss`, `fsr4`, `xess`, or `builtin`) on a GPU of PCI vendor
/// `vendor_id` (ADR-0021 §4b, amendment 5): DLSS wherever its probe passed (an NVIDIA RTX GPU
/// on NVIDIA's driver); XeSS on Intel GPUs only (elsewhere DLSS or the open build is the
/// better default); FSR 4 not until it is verified on AMD hardware (gate 5). `Err` is the one
/// line explaining why a chosen back end is passed over.
pub(crate) fn auto_vendor(chosen: &str, vendor_id: u32) -> Result<Option<UpscalerKind>, String> {
    match chosen {
        "dlss" => Ok(Some(UpscalerKind::Dlss)),
        "xess" if vendor_id == crate::vendor::pci::INTEL => Ok(Some(UpscalerKind::Xess)),
        "xess" => Err(String::from(
            "xess is auto's choice on Intel GPUs only (ADR-0021 amendment 5); \
             MOBA_UPSCALER=xess selects it",
        )),
        "fsr4" => Err(String::from(
            "fsr4 is not auto's choice until it is verified on AMD hardware (ADR-0021 gate 5, \
             amendment 5); MOBA_UPSCALER=fsr4 selects it",
        )),
        _ => Ok(None),
    }
}

/// What [`create`] needs: the device and shaders, the vendor back ends selected for the
/// device, what `MOBA_UPSCALER` asked for, the sizes, the mode and the post-processing
/// settings.
pub(crate) struct Setup<'a> {
    pub(crate) device: &'a crate::device::DeviceContext,
    pub(crate) shaders: &'a crate::shaders::ShaderSource,
    pub(crate) vendor: &'a mut crate::vendor::Selection,
    pub(crate) probe: &'a crate::vendor::ProbeContext<'a>,
    pub(crate) requested: Option<UpscalerKind>,
    pub(crate) output: Extent,
    pub(crate) max_render: Extent,
    pub(crate) mode: UpscaleMode,
    pub(crate) post: &'a crate::post::PostConfig,
    pub(crate) debug: taau::TaauDebugView,
}

/// The created back end and the sharpness it runs with.
pub(crate) struct Created {
    pub(crate) upscaler: Box<dyn TemporalUpscaler>,
    pub(crate) sharpness: f32,
    /// The renderer's `sharpen` pass runs after the back end (it does not sharpen itself).
    pub(crate) sharpen_pass: bool,
}

/// Creates the back end `setup.requested` asks for (ADR-0021 §4b). A vendor back end comes
/// from the vendor selection ([`crate::vendor::Selection::create_upscaler`]); one that is
/// not compiled in, not supported here or fails to create falls back to TAAU with one log
/// line, and `auto` takes DLSS or XeSS where [`auto_vendor`] says so, else
/// [`AUTO_OPEN_BUILD`] ([`resolve`]). FSR 3.1 that cannot run on this device (its format
/// features or quad subgroups, amendment 1) falls back to TAAU with one line too. The device is
/// never rejected, and only the running back end holds memory (amendment 1). A back end
/// without its own sharpening
/// is wrapped in the renderer's `sharpen` pass ([`sharpen::Sharpened`]) when its sharpness
/// ([`crate::post::PostConfig::sharpness_for`]) is above 0.
///
/// # Errors
/// TAAU cannot be created (a Vulkan or allocation error).
pub(crate) fn create(setup: &mut Setup<'_>) -> crate::error::RenderResult<Created> {
    let vendor_kind = match setup.requested {
        None => match auto_vendor(
            setup.vendor.backend(crate::vendor::Slot::Upscaler),
            setup.probe.capabilities.vendor_id,
        ) {
            Ok(kind) => kind,
            Err(why) => {
                eprintln!("[render] {why}; auto uses {}", AUTO_OPEN_BUILD.name());
                None
            }
        },
        Some(kind @ (UpscalerKind::Dlss | UpscalerKind::Fsr4 | UpscalerKind::Xess)) => Some(kind),
        Some(UpscalerKind::Taau | UpscalerKind::Fsr3) => None,
    };
    let vendor = match vendor_kind {
        Some(kind) => {
            let request = crate::vendor::UpscalerRequest {
                device: setup.device,
                output: setup.output,
                max_render: setup.max_render,
                mode: setup.mode,
            };
            match setup
                .vendor
                .create_upscaler(kind.name(), setup.probe, &request)
            {
                Ok(upscaler) => Some(upscaler),
                Err(why) => {
                    eprintln!(
                        "[render] upscaler {} cannot run here ({why}); using taau",
                        kind.name()
                    );
                    None
                }
            }
        }
        None => None,
    };
    // A built-in request, or `auto` without a vendor back end: FSR 3.1 when it resolves to it
    // and can run here (amendment 4), else TAAU. A vendor request that fell back above gets
    // TAAU.
    let upscaler = if let Some(upscaler) = vendor {
        upscaler
    } else {
        let built_in = if vendor_kind.is_none() {
            resolve(setup.requested)
        } else {
            UpscalerKind::Taau
        };
        let fsr3 = if built_in == UpscalerKind::Fsr3 {
            new_fsr3(setup.device, setup.output, setup.max_render)
                .inspect_err(|error| {
                    eprintln!("[render] upscaler fsr3 cannot run here ({error}); using taau");
                })
                .ok()
        } else {
            None
        };
        if let Some(fsr3) = fsr3 {
            fsr3
        } else {
            Box::new(taau::Taau::new(
                setup.device,
                setup.shaders,
                setup.output,
                setup.max_render,
                setup.debug,
            )?)
        }
    };
    if setup.debug != taau::TaauDebugView::None && upscaler.kind() != UpscalerKind::Taau {
        eprintln!(
            "[render] MOBA_DEBUG_VIEW {:?} is drawn by taau only; {} shows the frame",
            setup.debug,
            upscaler.kind().name()
        );
    }
    Ok(finish(
        setup.device,
        setup.shaders,
        upscaler,
        setup.output,
        setup.post,
    ))
}

/// TAAU in place of a vendor back end that stopped working (a resize it cannot follow,
/// ADR-0021 amendment 5), with its sharpness and `sharpen` pass.
///
/// # Errors
/// TAAU cannot be created (a Vulkan or allocation error).
pub(crate) fn taau_fallback(
    device: &crate::device::DeviceContext,
    shaders: &crate::shaders::ShaderSource,
    output: Extent,
    max_render: Extent,
    debug: taau::TaauDebugView,
    post: &crate::post::PostConfig,
) -> crate::error::RenderResult<Created> {
    let upscaler = Box::new(taau::Taau::new(device, shaders, output, max_render, debug)?);
    Ok(finish(device, shaders, upscaler, output, post))
}

/// Picks the back end's sharpness and wraps it in the `sharpen` pass when it needs one. A
/// `sharpen` pass that cannot be created leaves the back end unsharpened, with one line.
fn finish(
    device: &crate::device::DeviceContext,
    shaders: &crate::shaders::ShaderSource,
    upscaler: Box<dyn TemporalUpscaler>,
    output: Extent,
    post: &crate::post::PostConfig,
) -> Created {
    let mut sharpness = post.sharpness_for(upscaler.kind());
    let mut sharpen_pass = needs_sharpen_pass(upscaler.has_sharpening(), sharpness);
    let upscaler = if sharpen_pass {
        match sharpen::Sharpened::new(device, shaders, upscaler, output) {
            Ok(sharpened) => Box::new(sharpened) as Box<dyn TemporalUpscaler>,
            Err((upscaler, error)) => {
                eprintln!(
                    "[render] the sharpen pass cannot be created ({error}); {} runs unsharpened",
                    upscaler.kind().name()
                );
                sharpness = 0.0;
                sharpen_pass = false;
                upscaler
            }
        }
    } else {
        upscaler
    };
    Created {
        upscaler,
        sharpness,
        sharpen_pass,
    }
}

/// The render allocation and scale bounds a back end needs (ADR-0021 §4c, amendment 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    /// The allocation, raised on each side to the back end's minimum for the mode (DLAA's is
    /// close to native whatever the render scale), never above the output.
    pub(crate) alloc: Extent,
    /// The render scales the back end accepts, `lo <= hi`, both in `0..=1`: the
    /// dynamic-resolution controller's floor and ceiling are clamped into them.
    pub(crate) scale: (f32, f32),
}

/// What a back end with render-size `range` at `output` needs of the allocation `alloc`.
#[must_use]
pub(crate) fn fit(output: Extent, alloc: Extent, range: (Extent, Extent)) -> Fit {
    let (low, high) = range;
    let side = |alloc: u32, low: u32, output: u32| alloc.max(low.min(output)).max(1);
    let ratio = |size: u32, output: u32| size as f32 / output.max(1) as f32;
    let lo = ratio(low.width, output.width)
        .max(ratio(low.height, output.height))
        .min(1.0);
    // The larger side's ratio, so rounding never puts the ceiling a pixel under the range's
    // maximum (`clamp_render` trims the other side to it).
    let hi = ratio(high.width, output.width)
        .max(ratio(high.height, output.height))
        .clamp(lo, 1.0);
    Fit {
        alloc: Extent {
            width: side(alloc.width, low.width, output.width),
            height: side(alloc.height, low.height, output.height),
        },
        scale: (lo, hi),
    }
}

/// Whether the renderer's `sharpen` pass runs after a back end (ADR-0021 §3): only when the
/// back end does not sharpen itself and a sharpness above 0 is chosen.
#[must_use]
pub(crate) fn needs_sharpen_pass(has_sharpening: bool, sharpness: f32) -> bool {
    !has_sharpening && sharpness > 0.0
}

/// Clamps this frame's render size to the back end's range for the mode (ADR-0021 §4c,
/// amendment 2 item 19) and to the render allocation `alloc`. The allocation wins when the
/// two disagree, since the images cannot be larger than they are; [`fit`] raises the
/// allocation to the back end's minimum, so with temporal AA they never disagree.
pub(crate) fn clamp_render(render: Extent, range: (Extent, Extent), alloc: Extent) -> Extent {
    let (low, high) = range;
    let side = |size: u32, low: u32, high: u32, cap: u32| {
        size.clamp(low.min(high), high).clamp(1, cap.max(1))
    };
    Extent {
        width: side(render.width, low.width, high.width, alloc.width),
        height: side(render.height, low.height, high.height, alloc.height),
    }
}

/// FSR 3.1's context for `output` and render sizes up to `max_render` (at least 1×1, at most
/// the output).
#[cfg(feature = "fsr3")]
fn new_fsr3(
    device: &crate::device::DeviceContext,
    output: Extent,
    max_render: Extent,
) -> crate::error::RenderResult<Box<dyn TemporalUpscaler>> {
    let output = Extent {
        width: output.width.max(1),
        height: output.height.max(1),
    };
    let max_render = Extent {
        width: max_render.width.clamp(1, output.width),
        height: max_render.height.clamp(1, output.height),
    };
    Ok(Box::new(crate::fsr3::Fsr3::new(
        device, max_render, output,
    )?))
}

/// Never reached: [`resolve`] picks FSR 3.1 only with the `fsr3` feature.
#[cfg(not(feature = "fsr3"))]
fn new_fsr3(
    _device: &crate::device::DeviceContext,
    _output: Extent,
    _max_render: Extent,
) -> crate::error::RenderResult<Box<dyn TemporalUpscaler>> {
    Err(crate::error::RenderError::Upscaler(String::from(
        "the `fsr3` feature is not compiled in",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TAAU rounds to nearest (amendment 2, item 19); FSR 3.1's SDK truncates.
    #[test]
    fn taau_mode_sizes_round_to_nearest() {
        let output = Extent {
            width: 2560,
            height: 1440,
        };
        let (_, quality) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!((quality.width, quality.height), (1707, 960));
        let (floor, _) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!((floor.width, floor.height), (1280, 720));
    }

    #[test]
    fn modes_have_one_naming_scheme_by_ratio() {
        let names: Vec<_> = UpscaleMode::ALL.iter().map(|mode| mode.name()).collect();
        assert_eq!(
            names,
            [
                "native",
                "quality",
                "balanced",
                "performance",
                "ultra-performance"
            ]
        );
        for &mode in UpscaleMode::ALL {
            assert_eq!(
                UpscaleMode::from_name(&mode.name().to_uppercase()),
                Some(mode)
            );
        }
        let ratios: Vec<_> = UpscaleMode::ALL.iter().map(|mode| mode.ratio()).collect();
        assert_eq!(ratios, [1.0, 1.5, 1.7, 2.0, 3.0]);
        assert_eq!(UpscalerKind::from_name(" DLSS "), Some(UpscalerKind::Dlss));
        assert_eq!(UpscalerKind::from_name("auto"), None);
    }

    #[test]
    fn tier_defaults_follow_the_adr() {
        use crate::QualityTier::{High, Low, Medium, Overdrive, Ultra};
        for tier in [Low, Medium, High] {
            assert_eq!(UpscaleMode::for_tier(tier, 1440), UpscaleMode::Quality);
        }
        assert_eq!(UpscaleMode::for_tier(Ultra, 1440), UpscaleMode::Quality);
        assert_eq!(UpscaleMode::for_tier(Ultra, 2160), UpscaleMode::Performance);
        assert_eq!(
            UpscaleMode::for_tier(Overdrive, 1440),
            UpscaleMode::Performance
        );
    }

    #[test]
    fn ranges_keep_the_aspect_and_the_floor() {
        let output = Extent {
            width: 2560,
            height: 1440,
        };
        let (low, high) = scaled_range(output, UpscaleMode::Quality, 0.5);
        assert_eq!(
            (high.width, high.height, low.width, low.height),
            (1707, 960, 1280, 720)
        );
        let (low, high) = scaled_range(output, UpscaleMode::Performance, 0.67);
        assert_eq!(
            (low, high),
            (high, high),
            "the floor never exceeds the ceiling"
        );
    }

    #[test]
    fn built_in_choices_resolve_to_what_this_build_has() {
        let fsr3 = if cfg!(feature = "fsr3") {
            UpscalerKind::Fsr3
        } else {
            UpscalerKind::Taau
        };
        let auto = if AUTO_OPEN_BUILD == UpscalerKind::Fsr3 {
            fsr3
        } else {
            UpscalerKind::Taau
        };
        assert_eq!(resolve(None), auto);
        assert_eq!(resolve(Some(UpscalerKind::Taau)), UpscalerKind::Taau);
        assert_eq!(resolve(Some(UpscalerKind::Fsr3)), fsr3);
        // The vendor back ends are `create`'s; here they resolve to their fallback.
        for kind in [UpscalerKind::Dlss, UpscalerKind::Fsr4, UpscalerKind::Xess] {
            assert_eq!(resolve(Some(kind)), UpscalerKind::Taau);
        }
    }

    /// ADR-0021 §4b, amendment 5: `auto` takes DLSS wherever its probe passed, XeSS on Intel
    /// only, and never FSR 4 before gate 5 on AMD hardware.
    #[test]
    fn auto_takes_dlss_and_xess_on_intel_but_not_fsr4_yet() {
        use crate::vendor::pci::{AMD, INTEL, NVIDIA};
        assert_eq!(auto_vendor("dlss", NVIDIA), Ok(Some(UpscalerKind::Dlss)));
        assert_eq!(auto_vendor("xess", INTEL), Ok(Some(UpscalerKind::Xess)));
        assert!(auto_vendor("xess", NVIDIA).is_err());
        assert!(auto_vendor("xess", AMD).is_err());
        let fsr4 = auto_vendor("fsr4", AMD).unwrap_err();
        assert!(fsr4.contains("MOBA_UPSCALER=fsr4"), "{fsr4}");
        assert_eq!(auto_vendor("builtin", NVIDIA), Ok(None));
    }

    /// ADR-0021 §3: the `sharpen` pass runs only for a back end without its own sharpening
    /// and with a sharpness above 0 (DLSS and XeSS default to 0, so none by default).
    #[test]
    fn the_sharpen_pass_runs_only_when_needed() {
        assert!(!needs_sharpen_pass(true, 0.25));
        assert!(!needs_sharpen_pass(false, 0.0));
        assert!(needs_sharpen_pass(false, 0.25));
        assert_eq!(UpscalerKind::Dlss.default_sharpness(), 0.0);
        assert_eq!(UpscalerKind::Xess.default_sharpness(), 0.0);
        assert_eq!(
            UpscalerKind::Fsr4.default_sharpness(),
            crate::post::DEFAULT_SHARPNESS
        );
    }

    /// ADR-0021 §4c, amendment 2 item 19: the render size is clamped to the back end's
    /// range, and never beyond the allocation.
    #[test]
    fn render_sizes_clamp_to_the_back_end_range_and_the_allocation() {
        let e = |width, height| Extent { width, height };
        let range = (e(1280, 720), e(1708, 960));
        let alloc = e(1707, 960);
        assert_eq!(clamp_render(e(1000, 500), range, alloc), e(1280, 720));
        assert_eq!(clamp_render(e(1500, 844), range, alloc), e(1500, 844));
        // The range's maximum is above the allocation: the allocation wins.
        assert_eq!(clamp_render(e(1708, 960), range, alloc), e(1707, 960));
        // A fixed range (no dynamic resolution for the mode).
        let fixed = (e(1708, 960), e(1708, 960));
        assert_eq!(
            clamp_render(e(1280, 720), fixed, e(2560, 1440)),
            e(1708, 960)
        );
    }

    /// ADR-0021 §4c, amendment 5: DLAA's range (1267x713..1280x720 at 720p, measured)
    /// raises a 0.6 allocation to its minimum, and the controller's bounds follow the range.
    #[test]
    fn the_allocation_covers_the_back_end_minimum() {
        let e = |width, height| Extent { width, height };
        let output = e(1280, 720);
        let dlaa = fit(output, e(768, 432), (e(1267, 713), e(1280, 720)));
        assert_eq!(dlaa.alloc, e(1267, 713));
        // The larger of the two sides' ratios (713/720 > 1267/1280), so both fit.
        assert!((dlaa.scale.0 - 713.0 / 720.0).abs() < 1e-6);
        assert!((dlaa.scale.1 - 1.0).abs() < 1e-6);
        // A range inside the allocation changes nothing; the bounds are the range's.
        let quality = fit(output, e(853, 480), (e(640, 360), e(1280, 720)));
        assert_eq!(quality.alloc, e(853, 480));
        assert!((quality.scale.0 - 0.5).abs() < 1e-6 && (quality.scale.1 - 1.0).abs() < 1e-6);
        // A degenerate range never inverts the bounds or leaves the output.
        let odd = fit(e(8, 8), e(4, 4), (e(16, 4), e(2, 2)));
        assert_eq!(odd.alloc, e(8, 4));
        assert!(odd.scale.0 <= odd.scale.1 && odd.scale.1 <= 1.0);
    }
}
