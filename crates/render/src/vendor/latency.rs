//! Latency reduction through device extensions (ADR-0020 §6a, ADR-0021 §6):
//! `VK_NV_low_latency2` (the Vulkan face of Reflex) and `VK_AMD_anti_lag`. No SDK code is
//! used, so there is no feature: every build detects them, on the tiered path only (Compat
//! enables neither, ADR-0020 amendment).
//!
//! A [`LatencyBackend`] has three implementations, chosen once per device from the enabled
//! extensions (Reflex first):
//! - **Reflex** creates swapchains with `VkSwapchainLatencyCreateInfoNV`, sets the sleep mode
//!   on each one, sleeps in `vkLatencySleepNV` plus a timeline-semaphore wait, sets the frame
//!   markers by present ID and reads `vkGetLatencyTimingsNV` for the frame log;
//! - **Anti-Lag** calls `vkAntiLagUpdateAMD` at stage `INPUT` in the sleep and at stage
//!   `PRESENT` before the present, with the same frame index;
//! - **Null** does nothing, and the platform's limiter paces the frames.
//!
//! The sleep runs after a frame's present and before the next frame's input
//! ([`crate::Renderer::latency_sleep`], driven by `moba_platform::App::pace`). While a back
//! end sleeps it replaces the platform limiter: `max_frame_rate` becomes Reflex's
//! `minimumIntervalUs` or Anti-Lag's `maxFPS` (maintainer answer to ADR-0021 Q5).

use core::fmt::Write as _;
use std::time::Duration;

use ash::vk;

use crate::caps::RenderPath;
use crate::resources::Semaphore;

/// The latency-reduction extension enabled on the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatencyMode {
    /// `VK_NV_low_latency2`.
    Reflex,
    /// `VK_AMD_anti_lag`.
    AntiLag,
    /// Neither extension is enabled on the device.
    None,
}

impl LatencyMode {
    /// The name used in logs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Reflex => "reflex",
            Self::AntiLag => "anti-lag",
            Self::None => "none",
        }
    }
}

/// The player's latency setting (`MOBA_LATENCY`, ADR-0021 §6b). Reflex and Anti-Lag are on
/// by default and boost is off (maintainer answer to ADR-0021 Q5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatencySetting {
    /// No latency sleep: the platform limiter paces the frames. Reflex still sets its markers
    /// and reports its timings, so `off` is the baseline that `on` is measured against.
    Off,
    /// The back end sleeps before each frame and replaces the platform limiter.
    #[default]
    On,
    /// [`Self::On`] plus Reflex's `lowLatencyBoost` (Anti-Lag has no boost: same as `On`).
    Boost,
}

impl LatencySetting {
    /// Parses `off`, `on` or `boost` (case-insensitive).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            "boost" => Some(Self::Boost),
            _ => None,
        }
    }

    /// The name used in logs and settings.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Boost => "boost",
        }
    }
}

/// A frame-loop point the application reports ([`crate::Renderer::latency_marker`]). The
/// renderer sets the render-submit and present markers itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatencyMarker {
    /// The frame's CPU work starts (top of the app's frame).
    SimulationStart,
    /// The frame's CPU work is done: the scene is built, rendering starts.
    SimulationEnd,
    /// Input for the frame is sampled.
    InputSample,
    /// A click whose result the frame shows (Reflex's latency flash).
    TriggerFlash,
}

impl LatencyMarker {
    pub(crate) fn nv(self) -> vk::LatencyMarkerNV {
        match self {
            Self::SimulationStart => vk::LatencyMarkerNV::SIMULATION_START,
            Self::SimulationEnd => vk::LatencyMarkerNV::SIMULATION_END,
            Self::InputSample => vk::LatencyMarkerNV::INPUT_SAMPLE,
            Self::TriggerFlash => vk::LatencyMarkerNV::TRIGGER_FLASH,
        }
    }
}

/// One frame's timings from `vkGetLatencyTimingsNV`, in microseconds
/// ([`crate::Renderer::latency_report`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LatencyFrame {
    /// The frame's present ID.
    pub present_id: u64,
    /// Simulation start to simulation end (the app's CPU work before rendering).
    pub simulation_us: u64,
    /// Render-submit start to end (recording and `vkQueueSubmit`).
    pub render_submit_us: u64,
    /// Present start to end (`vkQueuePresentKHR`).
    pub present_us: u64,
    /// GPU render start to end.
    pub gpu_us: u64,
    /// Simulation start to the end of the frame's GPU work: the PC latency up to the display
    /// queue (the extension reports no scan-out time).
    pub total_us: u64,
}

impl LatencyFrame {
    /// The frame's durations, or `None` while the driver has not filled them in yet.
    fn from_report(report: &vk::LatencyTimingsFrameReportNV<'_>) -> Option<Self> {
        if report.present_id == 0
            || report.sim_start_time_us == 0
            || report.gpu_render_end_time_us == 0
        {
            return None;
        }
        let span = |start: u64, end: u64| end.saturating_sub(start);
        Some(Self {
            present_id: report.present_id,
            simulation_us: span(report.sim_start_time_us, report.sim_end_time_us),
            render_submit_us: span(
                report.render_submit_start_time_us,
                report.render_submit_end_time_us,
            ),
            present_us: span(report.present_start_time_us, report.present_end_time_us),
            gpu_us: span(
                report.gpu_render_start_time_us,
                report.gpu_render_end_time_us,
            ),
            total_us: span(report.sim_start_time_us, report.gpu_render_end_time_us),
        })
    }
}

/// The extension the device was created with, Reflex first. This reads the enabled set, not
/// what the device advertises: the tiered path enables `VK_NV_low_latency2` only with
/// `presentId`, never both latency extensions (`crate::device::TieredExtensions`), and the
/// compat path enables neither (ADR-0020 §1e).
pub(crate) fn detect(enabled: &[String]) -> LatencyMode {
    let has = |name: &str| enabled.iter().any(|enabled| enabled == name);
    if has("VK_NV_low_latency2") {
        LatencyMode::Reflex
    } else if has("VK_AMD_anti_lag") {
        LatencyMode::AntiLag
    } else {
        LatencyMode::None
    }
}

/// Reflex's `minimumIntervalUs` (and Anti-Lag's `maxFPS` in hertz) for the platform's
/// `max_frame_rate`: `10⁶ / max_frame_rate`, 0 (no limit) for an uncapped loop (ADR-0021 §6b).
pub(crate) fn minimum_interval_us(max_frame_rate: u32) -> u32 {
    1_000_000_u32.checked_div(max_frame_rate).unwrap_or(0)
}

/// How long a Reflex sleep may take before the back end gives up sleeping. A healthy sleep
/// lasts at most about one frame; this only guards against a driver that never signals.
const SLEEP_TIMEOUT: Duration = Duration::from_millis(250);

/// Frame reports `vkGetLatencyTimingsNV` keeps (the extension's limit).
const TIMING_FRAMES: usize = 64;

/// The frame-loop side of a latency extension (ADR-0021 §6a). `frame` is the present ID of
/// the frame being built: one more than the last presented one.
pub(crate) trait LatencyBackend: Send {
    /// The extension behind this back end.
    fn mode(&self) -> LatencyMode;
    /// Whether swapchains are created with `VkSwapchainLatencyCreateInfoNV` and frames carry
    /// present IDs (`VkPresentIdKHR`, `VkLatencySubmissionPresentIdNV`).
    fn reflex(&self) -> bool {
        false
    }
    /// Called after each swapchain is created. Never fails the renderer: a back end that
    /// cannot set itself up stops sleeping, with one log line.
    fn swapchain_created(&mut self, swapchain: vk::SwapchainKHR) {
        let _ = swapchain;
    }
    /// The latency sleep before `frame`'s input. Returns whether it paced the frame (the
    /// platform limiter then stands down).
    fn sleep(&mut self, swapchain: vk::SwapchainKHR, frame: u64) -> bool {
        let _ = (swapchain, frame);
        false
    }
    /// A frame-loop point of `frame`.
    fn marker(&self, swapchain: vk::SwapchainKHR, frame: u64, marker: vk::LatencyMarkerNV) {
        let _ = (swapchain, frame, marker);
    }
    /// Frames whose timings completed since the last call, oldest first.
    fn report(&mut self, swapchain: vk::SwapchainKHR) -> &[LatencyFrame] {
        let _ = swapchain;
        &[]
    }
}

/// No latency extension: the platform limiter paces every frame.
struct Null;

impl LatencyBackend for Null {
    fn mode(&self) -> LatencyMode {
        LatencyMode::None
    }
}

/// `VK_NV_low_latency2`.
struct Reflex {
    device: ash::Device,
    loader: ash::nv::low_latency2::Device,
    setting: LatencySetting,
    minimum_interval_us: u32,
    /// Signalled by the driver when a sleep ends (timeline).
    semaphore: Semaphore,
    /// The value the latest sleep asked for.
    value: u64,
    /// A sleep failed or timed out: the back end stopped sleeping (logged once).
    stopped: bool,
    /// `vkGetLatencyTimingsNV`'s output, allocated once.
    timings: Box<[vk::LatencyTimingsFrameReportNV<'static>]>,
    /// The latest report's complete new frames, capacity [`TIMING_FRAMES`].
    frames: Vec<LatencyFrame>,
    /// The newest present ID already reported.
    reported: u64,
}

impl LatencyBackend for Reflex {
    fn mode(&self) -> LatencyMode {
        LatencyMode::Reflex
    }

    fn reflex(&self) -> bool {
        true
    }

    fn swapchain_created(&mut self, swapchain: vk::SwapchainKHR) {
        let mode = vk::LatencySleepModeInfoNV::default()
            .low_latency_mode(self.setting != LatencySetting::Off)
            .low_latency_boost(self.setting == LatencySetting::Boost)
            .minimum_interval_us(self.minimum_interval_us);
        // SAFETY: the swapchain belongs to this device and was created with
        // `VkSwapchainLatencyCreateInfoNV` (`Self::reflex`); the info is fully initialised.
        if let Err(error) = unsafe { self.loader.set_latency_sleep_mode(swapchain, Some(&mode)) }
            && !self.stopped
        {
            self.stop(&format!("vkSetLatencySleepModeNV failed ({error})"));
        }
    }

    fn sleep(&mut self, swapchain: vk::SwapchainKHR, _frame: u64) -> bool {
        if self.setting == LatencySetting::Off || self.stopped {
            return false;
        }
        self.value += 1;
        let info = vk::LatencySleepInfoNV::default()
            .signal_semaphore(self.semaphore.handle())
            .value(self.value);
        // SAFETY: the swapchain belongs to this device and has a sleep mode; the semaphore is
        // a live timeline semaphore of this device, and `value` grows with every call.
        if let Err(error) = unsafe { self.loader.latency_sleep(swapchain, &info) } {
            return self.stop(&format!("vkLatencySleepNV failed ({error})"));
        }
        let semaphores = [self.semaphore.handle()];
        let values = [self.value];
        let wait = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        let timeout = u64::try_from(SLEEP_TIMEOUT.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: the semaphore is a live timeline semaphore of this device (the tiered path
        // enables `timelineSemaphore`), and the arrays outlive the call.
        match unsafe { self.device.wait_semaphores(&wait, timeout) } {
            Ok(()) => true,
            Err(error) => self.stop(&format!(
                "the sleep's semaphore did not signal within {} ms ({error})",
                SLEEP_TIMEOUT.as_millis()
            )),
        }
    }

    fn marker(&self, swapchain: vk::SwapchainKHR, frame: u64, marker: vk::LatencyMarkerNV) {
        let info = vk::SetLatencyMarkerInfoNV::default()
            .present_id(frame)
            .marker(marker);
        // SAFETY: the swapchain belongs to this device and was created with
        // `VkSwapchainLatencyCreateInfoNV`; the info is fully initialised.
        unsafe { self.loader.set_latency_marker(swapchain, &info) };
    }

    fn report(&mut self, swapchain: vk::SwapchainKHR) -> &[LatencyFrame] {
        self.frames.clear();
        // The driver writes the structures back whole, so the next call would pass on what it
        // left in `sType`: every call starts from fresh ones
        // (`VUID-VkLatencyTimingsFrameReportNV-sType-sType`, seen on NVIDIA 615.78.08).
        self.timings
            .fill(vk::LatencyTimingsFrameReportNV::default());
        let mut info = vk::GetLatencyMarkerInfoNV::default().timings(&mut self.timings);
        // SAFETY: the swapchain belongs to this device; `timingCount` is the length of the
        // array `pTimings` points to, whose elements carry their `sType`.
        unsafe { self.loader.get_latency_timings(swapchain, &mut info) };
        let count = (info.timing_count as usize).min(self.timings.len());
        collect_frames(
            &mut self.timings[..count],
            &mut self.reported,
            &mut self.frames,
        );
        &self.frames
    }
}

impl Reflex {
    /// Gives up sleeping (one log line) and reports that this frame was not paced.
    fn stop(&mut self, reason: &str) -> bool {
        self.stopped = true;
        eprintln!("[latency] reflex: {reason}; sleeping stops, the platform limiter paces frames");
        false
    }
}

/// Appends the complete frames of `timings` newer than `reported` to `frames`, oldest first,
/// and advances `reported`. A frame the driver has not finished stops the scan, so it is
/// reported by a later call instead of being skipped.
fn collect_frames(
    timings: &mut [vk::LatencyTimingsFrameReportNV<'_>],
    reported: &mut u64,
    frames: &mut Vec<LatencyFrame>,
) {
    timings.sort_unstable_by_key(|report| report.present_id);
    let mut newest = *reported;
    for report in timings.iter() {
        if report.present_id <= newest {
            continue;
        }
        let Some(frame) = LatencyFrame::from_report(report) else {
            break;
        };
        if frames.len() == frames.capacity() {
            break;
        }
        frames.push(frame);
        newest = frame.present_id;
    }
    *reported = newest;
}

/// `VK_AMD_anti_lag`.
struct AntiLag {
    loader: ash::amd::anti_lag::Device,
    setting: LatencySetting,
    /// `maxFPS`: the platform's `max_frame_rate` (0, no limit, for an uncapped loop).
    max_fps: u32,
}

impl AntiLag {
    fn update(&self, stage: vk::AntiLagStageAMD, frame: u64) {
        let presentation = vk::AntiLagPresentationInfoAMD::default()
            .stage(stage)
            .frame_index(frame);
        let data = if self.setting == LatencySetting::Off {
            vk::AntiLagDataAMD::default().mode(vk::AntiLagModeAMD::OFF)
        } else {
            vk::AntiLagDataAMD::default()
                .mode(vk::AntiLagModeAMD::ON)
                .max_fps(self.max_fps)
                .presentation_info(&presentation)
        };
        // SAFETY: the device enables `VK_AMD_anti_lag` with its `antiLag` feature, and the
        // data (and the presentation info it may point to) is live for the call.
        unsafe { self.loader.anti_lag_update(&data) };
    }
}

impl LatencyBackend for AntiLag {
    fn mode(&self) -> LatencyMode {
        LatencyMode::AntiLag
    }

    fn sleep(&mut self, _swapchain: vk::SwapchainKHR, frame: u64) -> bool {
        // Off still says so once per frame, so the driver's own default cannot apply.
        self.update(vk::AntiLagStageAMD::INPUT, frame);
        self.setting != LatencySetting::Off
    }

    fn marker(&self, _swapchain: vk::SwapchainKHR, frame: u64, marker: vk::LatencyMarkerNV) {
        if marker == vk::LatencyMarkerNV::PRESENT_START && self.setting != LatencySetting::Off {
            self.update(vk::AntiLagStageAMD::PRESENT, frame);
        }
    }
}

/// What [`create`] needs: the created device and the configuration.
pub(crate) struct LatencyContext<'a> {
    pub(crate) instance: &'a ash::Instance,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) device: &'a ash::Device,
    pub(crate) path: RenderPath,
    /// The device extensions enabled at `vkCreateDevice`.
    pub(crate) enabled: &'a [String],
    pub(crate) setting: LatencySetting,
    /// The platform's `max_frame_rate` (0: uncapped).
    pub(crate) max_frame_rate: u32,
}

/// Creates the back end for a device and returns it with its one start-up log line. An
/// absent extension, or a failure to set one up, falls back to the null back end; nothing
/// else changes.
pub(crate) fn create(context: &LatencyContext<'_>) -> (Box<dyn LatencyBackend>, String) {
    let mode = detect(context.enabled);
    let interval = minimum_interval_us(context.max_frame_rate);
    let setting = context.setting;
    match mode {
        LatencyMode::Reflex if context.path == RenderPath::Tiered => {
            match Semaphore::timeline(context.device.clone()) {
                Ok(semaphore) => {
                    let backend = Reflex {
                        device: context.device.clone(),
                        loader: ash::nv::low_latency2::Device::load(
                            context.instance,
                            context.device,
                        ),
                        setting,
                        minimum_interval_us: interval,
                        semaphore,
                        value: 0,
                        stopped: false,
                        timings: vec![vk::LatencyTimingsFrameReportNV::default(); TIMING_FRAMES]
                            .into_boxed_slice(),
                        frames: Vec::with_capacity(TIMING_FRAMES),
                        reported: 0,
                    };
                    let revision = spec_version(context, c"VK_NV_low_latency2");
                    let line =
                        active_line(mode, revision, setting, interval, context.max_frame_rate);
                    (Box::new(backend), line)
                }
                Err(error) => (
                    Box::new(Null),
                    format!(
                        "[latency] unavailable: VK_NV_low_latency2's timeline semaphore could not \
                         be created ({error}); the platform limiter paces frames"
                    ),
                ),
            }
        }
        LatencyMode::AntiLag if context.path == RenderPath::Tiered => {
            let backend = AntiLag {
                loader: ash::amd::anti_lag::Device::load(context.instance, context.device),
                setting,
                max_fps: context.max_frame_rate,
            };
            let revision = spec_version(context, c"VK_AMD_anti_lag");
            let line = active_line(mode, revision, setting, interval, context.max_frame_rate);
            (Box::new(backend), line)
        }
        _ => (Box::new(Null), unavailable_line(context.path)),
    }
}

/// The start-up line for an active back end.
fn active_line(
    mode: LatencyMode,
    revision: Option<u32>,
    setting: LatencySetting,
    interval_us: u32,
    max_frame_rate: u32,
) -> String {
    let (extension, limit) = match mode {
        LatencyMode::Reflex => (
            "VK_NV_low_latency2",
            format!("minimumIntervalUs {interval_us}"),
        ),
        _ => ("VK_AMD_anti_lag", format!("maxFPS {max_frame_rate}")),
    };
    let mut line = format!("[latency] {}: {extension}", mode.name());
    if let Some(revision) = revision {
        let _ = write!(line, " rev {revision}");
    }
    let _ = write!(line, ", setting {}", setting.name());
    if setting == LatencySetting::Off {
        if mode == LatencyMode::Reflex {
            line.push_str("; markers and timings only, the platform limiter paces frames");
        } else {
            line.push_str("; the platform limiter paces frames");
        }
    } else {
        let _ = write!(
            line,
            " ({limit}); the latency sleep replaces the platform limiter"
        );
    }
    line
}

/// The start-up line when no back end runs.
fn unavailable_line(path: RenderPath) -> String {
    let reason = if path == RenderPath::Tiered {
        "the device enables neither VK_NV_low_latency2 (with presentId) nor VK_AMD_anti_lag"
    } else {
        "the compat path enables no latency extension"
    };
    format!("[latency] unavailable: {reason}; the platform limiter paces frames")
}

/// The device's `specVersion` of `name`, for the start-up line.
fn spec_version(context: &LatencyContext<'_>, name: &core::ffi::CStr) -> Option<u32> {
    // SAFETY: the physical device belongs to this instance.
    let available = unsafe {
        context
            .instance
            .enumerate_device_extension_properties(context.physical)
    }
    .ok()?;
    available
        .iter()
        .find(|extension| extension.extension_name_as_c_str() == Ok(name))
        .map(|extension| extension.spec_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(id: u64, start: u64, gpu_end: u64) -> vk::LatencyTimingsFrameReportNV<'static> {
        vk::LatencyTimingsFrameReportNV {
            present_id: id,
            sim_start_time_us: start,
            sim_end_time_us: start + 300,
            render_submit_start_time_us: start + 400,
            render_submit_end_time_us: start + 600,
            present_start_time_us: start + 700,
            present_end_time_us: start + 750,
            gpu_render_start_time_us: start + 650,
            gpu_render_end_time_us: gpu_end,
            ..Default::default()
        }
    }

    #[test]
    fn settings_parse_and_default_to_on() {
        assert_eq!(LatencySetting::default(), LatencySetting::On);
        assert_eq!(LatencySetting::parse("off"), Some(LatencySetting::Off));
        assert_eq!(LatencySetting::parse(" ON "), Some(LatencySetting::On));
        assert_eq!(LatencySetting::parse("boost"), Some(LatencySetting::Boost));
        assert_eq!(LatencySetting::parse("max"), None);
        for setting in [
            LatencySetting::Off,
            LatencySetting::On,
            LatencySetting::Boost,
        ] {
            assert_eq!(LatencySetting::parse(setting.name()), Some(setting));
        }
    }

    #[test]
    fn max_frame_rate_becomes_the_minimum_interval() {
        assert_eq!(minimum_interval_us(240), 4_166);
        assert_eq!(minimum_interval_us(60), 16_666);
        assert_eq!(minimum_interval_us(1), 1_000_000);
        // Uncapped: no limit.
        assert_eq!(minimum_interval_us(0), 0);
    }

    #[test]
    fn frame_durations_come_from_the_report() {
        let frame = LatencyFrame::from_report(&report(7, 1_000, 3_500)).expect("complete");
        assert_eq!(frame.present_id, 7);
        assert_eq!(frame.simulation_us, 300);
        assert_eq!(frame.render_submit_us, 200);
        assert_eq!(frame.present_us, 50);
        assert_eq!(frame.gpu_us, 3_500 - 1_650);
        assert_eq!(frame.total_us, 2_500);
        // Not finished on the GPU yet, or never started: nothing to report.
        assert_eq!(LatencyFrame::from_report(&report(8, 1_000, 0)), None);
        assert_eq!(LatencyFrame::from_report(&report(9, 0, 3_000)), None);
    }

    #[test]
    fn reports_are_new_complete_frames_in_order() {
        let mut frames = Vec::with_capacity(TIMING_FRAMES);
        let mut reported = 0;
        // Out of order, with frame 4 still on the GPU.
        let mut timings = [
            report(3, 3_000, 5_000),
            report(1, 1_000, 3_000),
            report(4, 4_000, 0),
            report(2, 2_000, 4_000),
            report(5, 5_000, 7_000),
        ];
        collect_frames(&mut timings, &mut reported, &mut frames);
        let ids: Vec<u64> = frames.iter().map(|frame| frame.present_id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(reported, 3);
        // Next call: the driver's ring still holds 1..=5, now complete; only 4 and 5 are new.
        frames.clear();
        let mut timings = [
            report(1, 1_000, 3_000),
            report(2, 2_000, 4_000),
            report(3, 3_000, 5_000),
            report(4, 4_000, 6_000),
            report(5, 5_000, 7_000),
        ];
        collect_frames(&mut timings, &mut reported, &mut frames);
        let ids: Vec<u64> = frames.iter().map(|frame| frame.present_id).collect();
        assert_eq!(ids, [4, 5]);
        assert_eq!(reported, 5);
    }

    #[test]
    fn start_up_lines_say_what_paces_the_frames() {
        assert_eq!(
            active_line(LatencyMode::Reflex, Some(3), LatencySetting::On, 4_166, 240),
            "[latency] reflex: VK_NV_low_latency2 rev 3, setting on (minimumIntervalUs 4166); \
             the latency sleep replaces the platform limiter"
        );
        assert_eq!(
            active_line(
                LatencyMode::Reflex,
                Some(3),
                LatencySetting::Off,
                4_166,
                240
            ),
            "[latency] reflex: VK_NV_low_latency2 rev 3, setting off; markers and timings \
             only, the platform limiter paces frames"
        );
        assert_eq!(
            active_line(
                LatencyMode::AntiLag,
                None,
                LatencySetting::Boost,
                4_166,
                240
            ),
            "[latency] anti-lag: VK_AMD_anti_lag, setting boost (maxFPS 240); the latency \
             sleep replaces the platform limiter"
        );
        assert_eq!(
            unavailable_line(RenderPath::Compat),
            "[latency] unavailable: the compat path enables no latency extension; the \
             platform limiter paces frames"
        );
        assert!(unavailable_line(RenderPath::Tiered).contains("neither VK_NV_low_latency2"));
    }
}
