//! Game client: rendering, prediction, interpolation and UI.
//!
//! At this milestone the client only proves the platform contract: window creation, input,
//! fixed timestep and a clean shutdown. The renderer attaches in M3 and the simulation in
//! M4.
//!
//! Set `MOBA_SMOKE_FRAMES=<n>` to run exactly `n` frames and exit; CI and the smoke test
//! use this to prove the event loop works without a human closing the window.

#![forbid(unsafe_code)]

use moba_platform::{App, Frame, FrameOutcome, Key, PlatformError, Window, WindowConfig, run};

struct ClientApp {
    frames: u64,
    ticks: u64,
    quit_after_frames: u64,
    window: Option<Window>,
}

impl App for ClientApp {
    fn on_start(&mut self, window: &Window) {
        self.window = Some(window.clone());
        window.set_title("MOBA");
    }

    fn on_frame(&mut self, frame: &mut Frame<'_>) -> FrameOutcome {
        self.frames += 1;
        self.ticks += u64::from(frame.ticks());

        if frame.input().was_key_pressed(Key::Escape) {
            return FrameOutcome::Exit;
        }
        if self.frames >= self.quit_after_frames {
            return FrameOutcome::Exit;
        }
        FrameOutcome::Continue
    }
}

fn main() -> Result<(), PlatformError> {
    let quit_after_frames = std::env::var("MOBA_SMOKE_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX);
    let config = WindowConfig::new("MOBA", 1280, 720);
    run(
        config,
        ClientApp {
            frames: 0,
            ticks: 0,
            quit_after_frames,
            window: None,
        },
    )
}
