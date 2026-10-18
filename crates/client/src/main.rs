//! Binary entry point; the application lives in `app` and the HUD in `hud`.

mod app;
mod hud;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
