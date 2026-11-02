//! `bindless_smoke` (ADR-0020 gate 3): see `moba_render::bindless_smoke` and `tiered_smoke.rs`.
//!
//! ```text
//! MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required bindless-smoke
//! ```
#![allow(clippy::print_stdout)] // The report is the tool's output.

#[path = "../tiered_smoke.rs"]
mod tiered_smoke;

fn main() {
    tiered_smoke::main(moba_render::bindless_smoke);
}
