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
use std::time::{Duration, Instant};

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

/// The share of the frame interval a frame must have lasted, from its start to the end of
/// its latency sleep, for the sleep to count as having paced it. A shorter frame means the
/// driver did not hold it to `minimumIntervalUs` / `maxFPS`, so the platform limiter waits out
/// the rest (the backstop).
const BACKSTOP: f64 = 0.9;

/// Whether a frame counts as paced by the latency sleep: the back end slept (`slept`) and,
/// when there is a limit, the frame lasted at least [`BACKSTOP`] of its interval
/// (`since_start`, from the frame's start to the end of the sleep; `None` for the first frame).
/// Otherwise the platform limiter paces it, so the cap never rests on the driver alone.
pub(crate) fn held(slept: bool, since_start: Option<Duration>, max_frame_rate: u32) -> bool {
    match (slept, since_start) {
        (false, _) => false,
        (true, Some(since)) if max_frame_rate > 0 => {
            since.as_secs_f64() * f64::from(max_frame_rate) >= BACKSTOP
        }
        (true, _) => true,
    }
}

/// The renderer's side of the frame loop around a [`LatencyBackend`]: which frames sleep,
/// the [`held`] backstop, and one `SIMULATION_START` per present ID.
///
/// The backstop's clock runs from one sleep's end to the next: a paced frame starts right
/// after its sleep, so that span is the frame's whole period, event dispatch included. After a
/// frame the platform limiter paced (handed back, or not slept), the platform's wait lies
/// between the sleep's end and the next frame, so the clock restarts at that frame's
/// `SIMULATION_START` instead: counting the wait would let the next frame through at once, and
/// a driver that ignores the limit would then alternate long and short frames past the cap.
#[derive(Debug, Default)]
pub(crate) struct Pacer {
    /// When the frame being built started, for the backstop (`None`: not yet known).
    frame_start: Option<Instant>,
    /// The latest frame was not paced by the sleep: the next `SIMULATION_START` restarts the
    /// clock.
    platform_paced: bool,
    /// The present ID whose `SimulationStart` was already set (0: none yet).
    started: u64,
}

impl Pacer {
    /// The latency sleep after `frame - 1`'s present ([`crate::Renderer::latency_sleep`]).
    /// Skipped, returning `false`, when the latest frame was not submitted (a minimised window
    /// would otherwise spin through sleeps). Returns whether the sleep paced the frame.
    pub(crate) fn sleep(
        &mut self,
        backend: &mut dyn LatencyBackend,
        swapchain: vk::SwapchainKHR,
        frame: u64,
        submitted: bool,
        max_frame_rate: u32,
    ) -> bool {
        let slept = submitted && backend.sleep(swapchain, frame);
        self.slept_at(slept, max_frame_rate, Instant::now())
    }

    /// [`Self::sleep`] once the back end returned, at `now`.
    fn slept_at(&mut self, slept: bool, max_frame_rate: u32, now: Instant) -> bool {
        let since = self
            .frame_start
            .map(|start| now.saturating_duration_since(start));
        let paced = held(slept, since, max_frame_rate);
        self.frame_start = Some(now);
        self.platform_paced = !paced;
        paced
    }

    /// Whether `marker` for `frame` goes to the back end. A frame skipped before submission
    /// keeps its present ID, so its next attempt would set `SIMULATION_START` again (possibly
    /// on a new swapchain): only the first one is sent.
    pub(crate) fn marker(&mut self, frame: u64, marker: LatencyMarker) -> bool {
        self.marker_at(frame, marker, Instant::now())
    }

    /// [`Self::marker`] at `now`.
    fn marker_at(&mut self, frame: u64, marker: LatencyMarker, now: Instant) -> bool {
        if marker == LatencyMarker::SimulationStart {
            if self.started == frame {
                return false;
            }
            self.started = frame;
            if self.platform_paced {
                self.frame_start = Some(now);
                self.platform_paced = false;
            }
        }
        true
    }
}

/// How long a Reflex sleep may take before the back end gives up sleeping. A healthy sleep
/// lasts at most about one frame; this only guards against a driver that never signals.
const SLEEP_TIMEOUT: Duration = Duration::from_millis(250);

/// How long a swapchain's destruction waits for a timed-out sleep's late signal before the
/// semaphore is leaked instead (the driver may still signal it).
const SETTLE_TIMEOUT: Duration = Duration::from_millis(100);

/// How many times Reflex resumes sleeping on a new swapchain after it stopped.
const MAX_RESUMES: u32 = 3;

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
    /// Called before the current swapchain is destroyed (recreation, or the renderer's drop
    /// when `last`), after the device is idle: anything the driver still owes on it is waited
    /// for, bounded.
    fn settle(&mut self, last: bool) {
        let _ = last;
    }
}

/// Whether Reflex sleeps, and what a stopped sleep still owes (ADR-0021 §6b).
#[derive(Debug, Default, PartialEq, Eq)]
struct SleepState {
    /// A sleep failed or timed out: sleeping stopped (logged once per stop).
    stopped: bool,
    /// The semaphore value a timed-out sleep still owes; the semaphore and the swapchain wait
    /// for it ([`LatencyBackend::settle`]) before either is destroyed.
    pending: Option<u64>,
    /// Times sleeping resumed on a new swapchain, at most [`MAX_RESUMES`].
    resumes: u32,
}

impl SleepState {
    /// `vkLatencySleepNV` failed: nothing is owed.
    fn failed(&mut self) {
        self.stopped = true;
    }

    /// The sleep's semaphore did not reach `value` in time: the driver still owes it.
    fn timed_out(&mut self, value: u64) {
        self.stopped = true;
        self.pending = Some(value);
    }

    /// The owed value arrived, or its semaphore was replaced.
    fn settled(&mut self) {
        self.pending = None;
    }

    /// A new swapchain has its sleep mode: a stopped sleep resumes if nothing is owed and it
    /// has resumes left. Returns whether it resumed.
    fn resume(&mut self) -> bool {
        if !self.stopped || self.pending.is_some() || self.resumes >= MAX_RESUMES {
            return false;
        }
        self.stopped = false;
        self.resumes += 1;
        true
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
    /// Signalled by the driver when a sleep ends (timeline). `None` once a semaphore whose
    /// signal never came was leaked and no new one could be made: sleeping stays stopped.
    semaphore: Option<Semaphore>,
    /// The value the latest sleep asked for.
    value: u64,
    /// Whether it sleeps, and what a stopped sleep still owes.
    state: SleepState,
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
        match unsafe { self.loader.set_latency_sleep_mode(swapchain, Some(&mode)) } {
            Err(error) => {
                if !self.state.stopped {
                    self.state.failed();
                    Self::log_stop(&format!("vkSetLatencySleepModeNV failed ({error})"));
                }
            }
            Ok(()) => {
                if self.setting != LatencySetting::Off
                    && self.semaphore.is_some()
                    && self.state.resume()
                {
                    eprintln!(
                        "[latency] reflex: sleeping resumes on the new swapchain ({} of {MAX_RESUMES})",
                        self.state.resumes
                    );
                }
            }
        }
    }

    fn sleep(&mut self, swapchain: vk::SwapchainKHR, _frame: u64) -> bool {
        if self.setting == LatencySetting::Off || self.state.stopped {
            return false;
        }
        let Some(semaphore) = self.semaphore.as_ref().map(Semaphore::handle) else {
            return false;
        };
        self.value += 1;
        let info = vk::LatencySleepInfoNV::default()
            .signal_semaphore(semaphore)
            .value(self.value);
        // SAFETY: the swapchain belongs to this device and has a sleep mode; the semaphore is
        // a live timeline semaphore of this device, and `value` grows with every call.
        if let Err(error) = unsafe { self.loader.latency_sleep(swapchain, &info) } {
            self.state.failed();
            Self::log_stop(&format!("vkLatencySleepNV failed ({error})"));
            return false;
        }
        match self.wait(self.value, SLEEP_TIMEOUT) {
            Ok(()) => true,
            Err(error) => {
                self.state.timed_out(self.value);
                Self::log_stop(&format!(
                    "the sleep's semaphore did not signal within {} ms ({error})",
                    SLEEP_TIMEOUT.as_millis()
                ));
                false
            }
        }
    }

    fn settle(&mut self, last: bool) {
        let Some(value) = self.state.pending else {
            return;
        };
        if self.wait(value, SETTLE_TIMEOUT).is_ok() {
            self.state.settled();
            return;
        }
        // The driver may still signal the old semaphore: leak it rather than destroy it under
        // a pending signal. On the renderer's drop the device is destroyed next with the
        // leaked semaphore still alive (`VUID-vkDestroyDevice-device-05137`): accepted, only
        // after a sleep timed out and its signal then missed 100 ms more, as the lesser
        // hazard than destroying a semaphore the driver may still signal.
        core::mem::forget(self.semaphore.take());
        self.value = 0;
        self.state.settled();
        if last {
            eprintln!(
                "[latency] reflex: a timed-out sleep never signalled; its semaphore is leaked"
            );
            return;
        }
        // A fresh semaphore for the next swapchain (values restart at 0).
        // SAFETY: Reflex runs only on the tiered path, whose device enables
        // `timelineSemaphore` (`crate::device`).
        self.semaphore = unsafe { Semaphore::timeline(self.device.clone()) }.ok();
        eprintln!(
            "[latency] reflex: a timed-out sleep never signalled; its semaphore is leaked{}",
            if self.semaphore.is_some() {
                ""
            } else {
                " and no new one could be made, so sleeping stays stopped"
            }
        );
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
    /// The one line for a stop.
    fn log_stop(reason: &str) {
        eprintln!(
            "[latency] reflex: {reason}; sleeping stops until the next swapchain, the platform \
             limiter paces frames"
        );
    }

    /// Waits up to `timeout` for the sleep semaphore to reach `value`.
    fn wait(&self, value: u64, timeout: Duration) -> Result<(), vk::Result> {
        let Some(semaphore) = self.semaphore.as_ref() else {
            return Err(vk::Result::ERROR_UNKNOWN);
        };
        let semaphores = [semaphore.handle()];
        let values = [value];
        let wait = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        let timeout = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: the semaphore is a live timeline semaphore of this device (the tiered path
        // enables `timelineSemaphore`), and the arrays outlive the call.
        unsafe { self.device.wait_semaphores(&wait, timeout) }
    }
}

/// Appends the complete frames of `timings` newer than `reported` to `frames`, oldest first,
/// and advances `reported`. An incomplete frame is skipped: the newest ones are still in
/// flight (nothing newer is complete, so `reported` stays below them and a later call reports
/// them), and an older one never completes (a frame retried on a new swapchain, whose
/// `SIMULATION_START` went to the old one), so waiting on it would stall the report.
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
            continue;
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
    /// The frame whose `INPUT` stage was sent: `PRESENT` goes only with a matching one.
    input_frame: Option<u64>,
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
        self.input_frame = Some(frame);
        anti_lag_sleeps(self.setting)
    }

    fn marker(&self, _swapchain: vk::SwapchainKHR, frame: u64, marker: vk::LatencyMarkerNV) {
        if anti_lag_presents(self.setting, self.input_frame, frame, marker) {
            self.update(vk::AntiLagStageAMD::PRESENT, frame);
        }
    }
}

/// Whether Anti-Lag's `INPUT` call paces the frame: it waits only when the mode is on.
fn anti_lag_sleeps(setting: LatencySetting) -> bool {
    setting != LatencySetting::Off
}

/// Whether `marker` sends Anti-Lag's `PRESENT` stage for `frame`: at present start, with the
/// mode on, and only after that frame's `INPUT` (the first frame, and one after a skipped
/// sleep, have none).
fn anti_lag_presents(
    setting: LatencySetting,
    input_frame: Option<u64>,
    frame: u64,
    marker: vk::LatencyMarkerNV,
) -> bool {
    marker == vk::LatencyMarkerNV::PRESENT_START
        && anti_lag_sleeps(setting)
        && input_frame == Some(frame)
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
            // SAFETY: this arm runs on the tiered path only, whose device enables
            // `timelineSemaphore` (`crate::device`).
            match unsafe { Semaphore::timeline(context.device.clone()) } {
                Ok(semaphore) => {
                    let backend = Reflex {
                        device: context.device.clone(),
                        loader: ash::nv::low_latency2::Device::load(
                            context.instance,
                            context.device,
                        ),
                        setting,
                        minimum_interval_us: interval,
                        semaphore: Some(semaphore),
                        value: 0,
                        state: SleepState::default(),
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
                input_frame: None,
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
        // Out of order, with frames 4 and 5 still on the GPU.
        let mut timings = [
            report(3, 3_000, 5_000),
            report(1, 1_000, 3_000),
            report(4, 4_000, 0),
            report(2, 2_000, 4_000),
            report(5, 5_000, 0),
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
        // Frame 6 never started on this swapchain (retried on a new one) and never completes:
        // it does not stall the frames after it.
        frames.clear();
        let mut timings = [report(6, 0, 9_000), report(7, 7_000, 9_500)];
        collect_frames(&mut timings, &mut reported, &mut frames);
        let ids: Vec<u64> = frames.iter().map(|frame| frame.present_id).collect();
        assert_eq!(ids, [7]);
        assert_eq!(reported, 7);
    }

    /// A back end that records its sleeps and answers with `slept`.
    struct Fake {
        slept: bool,
        sleeps: Vec<u64>,
    }

    impl LatencyBackend for Fake {
        fn mode(&self) -> LatencyMode {
            LatencyMode::Reflex
        }

        fn sleep(&mut self, _swapchain: vk::SwapchainKHR, frame: u64) -> bool {
            self.sleeps.push(frame);
            self.slept
        }
    }

    #[test]
    fn the_backstop_hands_early_frames_to_the_platform_limiter() {
        let ms = Duration::from_millis;
        // 240 fps: an interval of 4.17 ms, so a frame needs 3.75 ms to count as paced.
        assert!(held(true, Some(ms(4)), 240));
        assert!(!held(true, Some(ms(3)), 240));
        // The first frame has no start to measure from; an uncapped loop has no interval.
        assert!(held(true, None, 240));
        assert!(held(true, Some(Duration::ZERO), 0));
        // A back end that did not sleep never paces.
        assert!(!held(false, Some(ms(10)), 240));
        assert!(!held(false, None, 0));
    }

    #[test]
    fn the_pacer_skips_unsubmitted_frames_and_backstops_early_ones() {
        let swapchain = vk::SwapchainKHR::null();
        let mut backend = Fake {
            slept: true,
            sleeps: Vec::new(),
        };
        let mut pacer = Pacer::default();
        // The latest frame was not submitted (a minimised window): no sleep at all.
        assert!(!pacer.sleep(&mut backend, swapchain, 1, false, 1));
        assert_eq!(backend.sleeps, Vec::<u64>::new());
        // At 1 fps the next frame "lasted" microseconds: the driver let it through early, so
        // the platform limiter takes it.
        assert!(!pacer.sleep(&mut backend, swapchain, 1, true, 1));
        assert_eq!(backend.sleeps, [1]);
        // Uncapped, a sleep that ran always paces.
        assert!(pacer.sleep(&mut backend, swapchain, 2, true, 0));
        // A back end that did not sleep (setting off, stopped) never paces.
        backend.slept = false;
        assert!(!pacer.sleep(&mut backend, swapchain, 3, true, 0));
        assert_eq!(backend.sleeps, [1, 2, 3]);
    }

    #[test]
    fn the_backstop_clock_runs_sleep_end_to_sleep_end() {
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        // 100 fps: a 10 ms interval, so a frame needs 9 ms. The first sleep has nothing to
        // measure from: paced.
        assert!(pacer.slept_at(true, 100, t0));
        // A paced frame's clock starts at the sleep's end, not at its later
        // `SIMULATION_START` (event dispatch between them is part of the frame).
        assert!(pacer.marker_at(1, LatencyMarker::SimulationStart, t0 + ms(2)));
        assert!(pacer.slept_at(true, 100, t0 + ms(10)));
        // Let through after 5 ms: handed to the platform limiter.
        assert!(pacer.marker_at(2, LatencyMarker::SimulationStart, t0 + ms(10)));
        assert!(!pacer.slept_at(true, 100, t0 + ms(15)));
        // The platform then waits until 20 ms; that wait is not this frame's, so the clock
        // restarts at its `SIMULATION_START` and a frame let through early is caught again
        // (instead of alternating long and short frames past the cap).
        assert!(pacer.marker_at(3, LatencyMarker::SimulationStart, t0 + ms(20)));
        assert!(!pacer.slept_at(true, 100, t0 + ms(21)));
        // A frame that lasts its interval from its start is paced again.
        assert!(pacer.marker_at(4, LatencyMarker::SimulationStart, t0 + ms(30)));
        assert!(pacer.slept_at(true, 100, t0 + ms(40)));
    }

    #[test]
    fn simulation_start_is_sent_once_per_frame() {
        let mut pacer = Pacer::default();
        assert!(pacer.marker(1, LatencyMarker::SimulationStart));
        assert!(pacer.marker(1, LatencyMarker::InputSample));
        // Frame 1 was skipped before submission and is retried with the same present ID.
        assert!(!pacer.marker(1, LatencyMarker::SimulationStart));
        assert!(pacer.marker(1, LatencyMarker::SimulationEnd));
        assert!(pacer.marker(2, LatencyMarker::SimulationStart));
    }

    #[test]
    fn a_stopped_sleep_resumes_on_a_new_swapchain_only_when_nothing_is_owed() {
        let mut state = SleepState::default();
        assert!(!state.resume(), "a running sleep has nothing to resume");
        // A failed `vkLatencySleepNV` owes nothing: the next swapchain resumes it.
        state.failed();
        assert!(state.stopped);
        assert_eq!(state.pending, None);
        assert!(state.resume());
        assert!(!state.stopped);
        // A timeout owes its value until it is settled.
        state.timed_out(41);
        assert_eq!(state.pending, Some(41));
        assert!(!state.resume());
        assert!(state.stopped);
        state.settled();
        assert!(state.resume());
        // At most `MAX_RESUMES` times.
        state.failed();
        assert!(state.resume());
        state.failed();
        assert_eq!(state.resumes, MAX_RESUMES);
        assert!(!state.resume());
        assert!(state.stopped);
    }

    #[test]
    fn anti_lag_paces_only_when_on_and_presents_after_its_input() {
        assert!(!anti_lag_sleeps(LatencySetting::Off));
        assert!(anti_lag_sleeps(LatencySetting::On));
        assert!(anti_lag_sleeps(LatencySetting::Boost));
        let present = vk::LatencyMarkerNV::PRESENT_START;
        assert!(anti_lag_presents(LatencySetting::On, Some(5), 5, present));
        // No `INPUT` for this frame yet (the first frame, or a skipped sleep).
        assert!(!anti_lag_presents(LatencySetting::On, None, 1, present));
        assert!(!anti_lag_presents(LatencySetting::On, Some(4), 5, present));
        // Off sends no `PRESENT`; other markers are not Anti-Lag stages.
        assert!(!anti_lag_presents(LatencySetting::Off, Some(5), 5, present));
        assert!(!anti_lag_presents(
            LatencySetting::On,
            Some(5),
            5,
            vk::LatencyMarkerNV::PRESENT_END
        ));
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
