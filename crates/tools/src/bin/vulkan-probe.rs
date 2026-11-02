//! Headless Vulkan instance/device probe for CI portability coverage.
//!
//! `vulkan-probe --slang-smoke` also runs the Slang toolchain smoke module once on the first
//! Vulkan 1.3 compute device and checks its output (ADR-0020 §4c). Validation follows
//! `MOBA_VULKAN_VALIDATION=required|off` (default: when the layer is installed); any
//! validation error or output mismatch fails the run.
#![allow(clippy::print_stdout)]

use moba_render::ValidationMode;

fn main() {
    let slang_smoke = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--slang-smoke") => true,
        Some(other) => {
            eprintln!("usage: vulkan-probe [--slang-smoke] (unknown argument {other:?})");
            std::process::exit(2);
        }
    };
    match moba_render::probe_vulkan() {
        Ok(probe) => {
            println!(
                "vulkan probe: api={}.{}.{} devices={} portability={}",
                probe.api_version >> 22,
                (probe.api_version >> 12) & 0x3ff,
                probe.api_version & 0xfff,
                probe.devices,
                probe.portability
            );
            if probe.devices == 0 {
                eprintln!("no Vulkan physical devices enumerated");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("vulkan probe failed: {error}");
            std::process::exit(1);
        }
    }
    if slang_smoke {
        run_slang_smoke();
    }
}

fn run_slang_smoke() {
    let validation = match std::env::var("MOBA_VULKAN_VALIDATION").as_deref() {
        Ok("required") => ValidationMode::Required,
        Ok("off") => ValidationMode::Off,
        _ => ValidationMode::IfAvailable,
    };
    match moba_render::slang_smoke(validation) {
        Ok(smoke) => {
            println!(
                "slang smoke: device={:?} validation={} validation_errors={} output={}",
                smoke.device,
                smoke.validation,
                smoke.validation_errors,
                smoke.mismatch.as_deref().unwrap_or("ok")
            );
            if !smoke.passed() {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("slang smoke failed: {error}");
            std::process::exit(1);
        }
    }
}
