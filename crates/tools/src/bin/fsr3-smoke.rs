//! `fsr3_smoke` (ADR-0021 §4b and §4c, N1b): the FSR 3.1.6 port on synthetic inputs; see
//! `moba_render::fsr3_smoke` and `tiered_smoke.rs`.
//!
//! ```text
//! MOBA_RENDER_PATH=tiered MOBA_VULKAN_VALIDATION=required fsr3-smoke
//! ```
#![allow(clippy::print_stdout)] // The report is the tool's output.

#[path = "../tiered_smoke.rs"]
mod tiered_smoke;

fn main() {
    tiered_smoke::main(moba_render::fsr3_smoke);
}
