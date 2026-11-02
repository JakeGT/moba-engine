//! The shared driver of the tiered smoke binaries, `bindless-smoke` and `rt-smoke` (ADR-0020
//! gate 3): reads the environment, runs one test from `moba-render` and reports it.
//!
//! - `MOBA_RENDER_PATH` (default `auto`): the device choice. A CPU device (lavapipe) needs
//!   `tiered`; on a GPU admitted to the tiered path `auto` picks it.
//! - `MOBA_VULKAN_VALIDATION=required|off` (default: enabled when installed). `required`
//!   includes synchronization validation.
//!
//! Exits 0 when every value matched and validation ran and reported no error or warning, 1 on
//! a mismatch, a validation message, or a run without validation that was not asked for
//! (`MOBA_VULKAN_VALIDATION=off` is the only way to pass without the layer), and 2 when the
//! test could not run (no tiered device, no validation layer under `required`, a Vulkan
//! error).

use moba_render::{PathChoice, RenderResult, SmokeReport, ValidationMode};

/// Runs `test` and exits with its status.
pub(crate) fn main(test: fn(PathChoice, ValidationMode) -> RenderResult<SmokeReport>) -> ! {
    let choice = match std::env::var("MOBA_RENDER_PATH") {
        Ok(value) => PathChoice::parse(&value).unwrap_or_else(|| {
            eprintln!("MOBA_RENDER_PATH={value:?} is not auto, tiered or compat");
            std::process::exit(2);
        }),
        Err(_) => PathChoice::Auto,
    };
    let validation = match std::env::var("MOBA_VULKAN_VALIDATION").as_deref() {
        Ok("required") => ValidationMode::Required,
        Ok("off") => ValidationMode::Off,
        _ => ValidationMode::IfAvailable,
    };
    let report = match test(choice, validation) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("smoke test could not run: {error}");
            std::process::exit(2);
        }
    };
    println!(
        "{}: device \"{}\" {}",
        report.name, report.device, report.admission
    );
    for note in &report.notes {
        println!("{}: {note}", report.name);
    }
    for mismatch in &report.first_mismatches {
        println!("{}: MISMATCH {mismatch}", report.name);
    }
    let verdict = if report.passed() { "PASS" } else { "FAIL" };
    println!(
        "{}: {verdict} checked={} mismatches={} validation={} validation_errors={} \
         validation_warnings={}",
        report.name,
        report.checked,
        report.mismatches,
        if report.validation_enabled {
            "on"
        } else if report.validation_waived {
            "off"
        } else {
            "missing"
        },
        report.validation_errors,
        report.validation_warnings
    );
    std::process::exit(i32::from(!report.passed()));
}
