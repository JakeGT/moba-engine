//! Binary entry point; the application lives in `app` and the HUD in `hud`.
#![cfg_attr(not(test), forbid(unsafe_code))]

mod app;
mod audio;
mod hud;
mod input;
mod settings;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
