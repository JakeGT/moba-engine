//! Application loop, frame pacing and the fixed-timestep contract.
//!
//! The platform drives two clocks:
//! - **simulation**: whole ticks at `WindowConfig::tick_rate` via [`moba_core::FixedStep`],
//!   so gameplay never sees a wall clock;
//! - **presentation**: one update per redraw, interpolated with `frame.alpha()` between the
//!   previous and current tick.
//!
//! The app owns its renderer and simulation; the platform only supplies input, window
//! events and the tick count for this frame.

use core::fmt;
use std::time::{Duration, Instant};

use moba_core::{FixedStep, Tick};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;

use crate::input::{InputState, map_key, map_mouse_button};
use crate::window::{PlatformError, Window, WindowConfig};

/// Per-frame context handed to [`App::on_frame`].
pub struct Frame<'a> {
    window: &'a Window,
    input: &'a InputState,
    ticks: u32,
    tick: Tick,
    alpha: f32,
    delta_secs: f32,
    elapsed_secs: f64,
}

impl<'a> Frame<'a> {
    /// The window this frame is presented to.
    #[inline]
    #[must_use]
    pub fn window(&self) -> &'a Window {
        self.window
    }

    /// Input snapshot for this frame.
    #[inline]
    #[must_use]
    pub fn input(&self) -> &'a InputState {
        self.input
    }

    /// Number of simulation ticks due this frame.
    ///
    /// The app must run exactly this many authoritative ticks before rendering.
    #[inline]
    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// Tick counter *after* this frame's ticks have run.
    #[inline]
    #[must_use]
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Fraction of the next tick already elapsed, in `[0, 1)`.
    ///
    /// Use this to interpolate entity transforms between the previous and current tick.
    #[inline]
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Wall-clock duration of the last presented frame, in seconds.
    #[inline]
    #[must_use]
    pub fn delta_secs(&self) -> f32 {
        self.delta_secs
    }

    /// Wall-clock seconds since the app started.
    #[inline]
    #[must_use]
    pub fn elapsed_secs(&self) -> f64 {
        self.elapsed_secs
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("ticks", &self.ticks)
            .field("tick", &self.tick)
            .field("alpha", &self.alpha)
            .field("delta_secs", &self.delta_secs)
            .field("elapsed_secs", &self.elapsed_secs)
            .finish_non_exhaustive()
    }
}

/// Whether the app wants to keep running after this frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Keep the loop running.
    Continue,
    /// Close the window and shut the app down.
    Exit,
}

/// Who paced the frame that just ended ([`App::pace`]).
///
/// The platform's own limiter waits until `1 / WindowConfig::max_frame_rate` after a frame
/// started before it starts the next one. An app that paces itself (a latency-reduction
/// sleep such as Reflex or Anti-Lag, ADR-0021 §6b) has already waited the right amount when
/// `pace` returns, so a second wait would only add latency: one limiter, not two.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pacing {
    /// The platform limiter paces the next frame, as without the hook.
    #[default]
    Platform,
    /// The app paced this frame itself: the next frame is due at once.
    External,
}

/// An application driven by the platform loop.
///
/// `on_start` is called every time a window becomes available (including after a resume),
/// so resources tied to the surface must be (re)created there.
pub trait App {
    /// Called when a window is created or restored.
    fn on_start(&mut self, window: &Window) {
        let _ = window;
    }

    /// Called once per frame, after input events have been collected.
    ///
    /// Return [`FrameOutcome::Exit`] to close the app cleanly (menu quit, `Escape` during
    /// development, a scripted test run).
    fn on_frame(&mut self, frame: &mut Frame<'_>) -> FrameOutcome;

    /// Called once per frame after [`App::on_frame`] returned (so after the frame's present),
    /// and before the platform gathers the next frame's input.
    ///
    /// A latency-reduction sleep belongs here: input that arrives during it is queued by the
    /// OS and delivered before the next `on_frame`, which therefore samples the freshest
    /// input (ADR-0021 §6b). Return [`Pacing::External`] when this call paced the frame, so
    /// the platform's frame limiter stands down for it; the default does nothing and returns
    /// [`Pacing::Platform`]. Not called for the frame that exits.
    fn pace(&mut self) -> Pacing {
        Pacing::Platform
    }

    /// Called when the window's physical size changes.
    fn on_resize(&mut self, size: [u32; 2]) {
        let _ = size;
    }

    /// Called when the DPI scale factor changes.
    fn on_scale_factor(&mut self, scale_factor: f32) {
        let _ = scale_factor;
    }

    /// Called when the window gains or loses focus.
    fn on_focus(&mut self, focused: bool) {
        let _ = focused;
    }

    /// Called when the platform suspends the app (renderer surface must be released).
    fn on_suspend(&mut self) {}
}

/// Creates the event loop and runs `app` until the window is closed.
///
/// Must be called on the main thread. Returns an error if the event loop or window cannot
/// be created.
pub fn run<A: App + 'static>(config: WindowConfig, app: A) -> Result<(), PlatformError> {
    assert!(config.tick_rate > 0, "tick_rate must be non-zero");
    let event_loop = EventLoop::new().map_err(PlatformError::EventLoop)?;
    let mut runner = Runner::new(config, app);
    let result = event_loop.run_app(&mut runner);
    if let Some(error) = runner.error.take() {
        return Err(error);
    }
    result.map_err(PlatformError::EventLoop)
}

struct Runner<A: App> {
    config: WindowConfig,
    app: A,
    window: Option<Window>,
    input: InputState,
    clock: FixedStep,
    frame_interval: Option<Duration>,
    last_frame: Instant,
    next_frame: Instant,
    elapsed_secs: f64,
    error: Option<PlatformError>,
}

impl<A: App> Runner<A> {
    fn new(config: WindowConfig, app: A) -> Self {
        let clock = FixedStep::new(config.tick_rate);
        let frame_interval = if config.max_frame_rate == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(
                1.0 / f64::from(config.max_frame_rate),
            ))
        };
        let now = Instant::now();
        Self {
            config,
            app,
            window: None,
            input: InputState::new(),
            clock,
            frame_interval,
            last_frame: now,
            next_frame: now,
            elapsed_secs: 0.0,
            error: None,
        }
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        self.elapsed_secs += delta.as_secs_f64();
        let delta_ns = u64::try_from(delta.as_nanos()).unwrap_or(u64::MAX);
        let ticks = self.clock.push(delta_ns);
        let tick = self.clock.tick();
        let alpha = self.clock.alpha();
        let mut frame = Frame {
            window: &window,
            input: &self.input,
            ticks,
            tick,
            alpha,
            delta_secs: delta.as_secs_f32(),
            elapsed_secs: self.elapsed_secs,
        };
        let outcome = self.app.on_frame(&mut frame);
        self.input.clear_edges();
        if outcome == FrameOutcome::Exit {
            event_loop.exit();
            return;
        }
        // ADR-0021 §6b: the app's latency sleep runs after this frame's present and before
        // the next frame's input is gathered.
        self.next_frame = paced_next_frame(&mut self.app, now, self.frame_interval);
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match Window::create(event_loop, &self.config) {
            Ok(window) => {
                self.app.on_start(&window);
                self.window = Some(window);
                let now = Instant::now();
                self.last_frame = now;
                self.next_frame = now;
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window = None;
        self.input = InputState::new();
        self.app.on_suspend();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_some_and(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.update(event_loop),
            WindowEvent::Resized(size) => {
                self.app.on_resize([size.width, size.height]);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.app.on_scale_factor(scale_factor as f32);
            }
            WindowEvent::Focused(focused) => {
                self.input.set_focused(focused);
                self.app.on_focus(focused);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = map_key(code)
                {
                    self.input
                        .handle_key(key, event.state == ElementState::Pressed, event.repeat);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = map_mouse_button(button) {
                    self.input
                        .handle_mouse_button(button, state == ElementState::Pressed);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.handle_cursor(moba_core::math::Vec2::new(
                    position.x as f32,
                    position.y as f32,
                ));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let vertical = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 120.0,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                self.input.handle_wheel(vertical);
            }
            WindowEvent::Occluded(true) => self.input.set_focused(false),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.as_ref() else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        let now = Instant::now();
        if now >= self.next_frame {
            window.request_redraw();
            match due_wait(now, self.frame_interval) {
                Some(until) => event_loop.set_control_flow(ControlFlow::WaitUntil(until)),
                None => event_loop.set_control_flow(ControlFlow::Poll),
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        }
    }
}

/// When the frame after one that started at `started` is due: one interval later under the
/// platform limiter, at once when the app paced the frame itself or there is no limit.
fn next_frame(started: Instant, interval: Option<Duration>, pacing: Pacing) -> Instant {
    match (pacing, interval) {
        (Pacing::Platform, Some(interval)) => started + interval,
        _ => started,
    }
}

/// Asks the app to pace the frame that started at `now` ([`App::pace`]) and returns when the
/// next one is due.
fn paced_next_frame<A: App>(app: &mut A, now: Instant, interval: Option<Duration>) -> Instant {
    next_frame(now, interval, app.pace())
}

/// How long the loop may wait after requesting a redraw at `now`: until one interval later,
/// or not at all (`None`, poll) when there is no limit. This holds when the app paces itself
/// too: the requested redraw ends the wait at once, and a window that draws nothing
/// (minimised, occluded) wakes once per interval instead of spinning a core.
fn due_wait(now: Instant, interval: Option<Duration>) -> Option<Instant> {
    interval.map(|interval| now + interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_defaults_to_platform() {
        struct Plain;
        impl App for Plain {
            fn on_frame(&mut self, _frame: &mut Frame<'_>) -> FrameOutcome {
                FrameOutcome::Continue
            }
        }
        assert_eq!(Plain.pace(), Pacing::Platform);
        assert_eq!(Pacing::default(), Pacing::Platform);
    }

    #[test]
    fn platform_pacing_keeps_the_limiter() {
        let start = Instant::now();
        let interval = Duration::from_micros(4_167);
        assert_eq!(
            next_frame(start, Some(interval), Pacing::Platform),
            start + interval
        );
        assert_eq!(due_wait(start, Some(interval)), Some(start + interval));
        // Uncapped: due at once, polling.
        assert_eq!(next_frame(start, None, Pacing::Platform), start);
        assert_eq!(due_wait(start, None), None);
    }

    #[test]
    fn external_pacing_replaces_the_limiter() {
        let start = Instant::now();
        let interval = Duration::from_micros(4_167);
        assert_eq!(next_frame(start, Some(interval), Pacing::External), start);
        // The loop still sleeps between redraw requests, so a window that draws nothing
        // does not spin.
        assert_eq!(due_wait(start, Some(interval)), Some(start + interval));
    }

    #[test]
    fn the_runner_schedules_by_what_pace_returns() {
        struct Paced {
            pacing: Pacing,
            calls: u32,
        }
        impl App for Paced {
            fn on_frame(&mut self, _frame: &mut Frame<'_>) -> FrameOutcome {
                FrameOutcome::Continue
            }
            fn pace(&mut self) -> Pacing {
                self.calls += 1;
                self.pacing
            }
        }
        let start = Instant::now();
        let interval = Some(Duration::from_micros(4_167));
        let mut app = Paced {
            pacing: Pacing::External,
            calls: 0,
        };
        assert_eq!(paced_next_frame(&mut app, start, interval), start);
        app.pacing = Pacing::Platform;
        assert_eq!(
            paced_next_frame(&mut app, start, interval),
            start + Duration::from_micros(4_167)
        );
        assert_eq!(app.calls, 2);
    }
}
