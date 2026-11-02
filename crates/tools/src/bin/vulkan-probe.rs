//! Headless Vulkan instance/device probe for CI portability coverage and the render-path gate
//! (ADR-0020 §2b).
//!
//! ```text
//! vulkan-probe [--json] [--path auto|tiered|compat]
//! ```
//!
//! Prints the instance line, then one line per physical device with its admission
//! (`path=`, `max_tier=`, `missing=`), then the highest-ranked device. The renderer also drops
//! devices below Vulkan 1.3 or without a graphics queue, swapchain or dynamic rendering, and
//! adds +10 for a shared graphics and present family (it has a surface), so with such a device
//! present its choice can differ. `--json`
//! prints every device's capabilities as a JSON array instead: the format of the recorded
//! fixtures in `crates/render/src/caps/fixtures/`. `--path` (default: `MOBA_RENDER_PATH`, else
//! `auto`) is the choice the admission is evaluated under; like `MOBA_RENDER_PATH=tiered`,
//! `tiered` here admits CPU devices such as lavapipe. Exits non-zero when no device is
//! enumerated, or when `--path tiered` admits none.
#![allow(clippy::print_stdout)] // The report is the tool's output.

use moba_render::{PathChoice, RenderPath};

/// `(json, choice)` from the command line; exits 2 on a bad argument.
fn parse_args() -> (bool, PathChoice) {
    let mut json = false;
    let mut choice = std::env::var("MOBA_RENDER_PATH")
        .ok()
        .and_then(|value| {
            let parsed = PathChoice::parse(&value);
            if parsed.is_none() {
                eprintln!("MOBA_RENDER_PATH={value:?} is not auto/tiered/compat; using auto");
            }
            parsed
        })
        .unwrap_or_default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--path" => {
                let Some(parsed) = args.next().as_deref().and_then(PathChoice::parse) else {
                    eprintln!("--path takes auto, tiered or compat");
                    std::process::exit(2);
                };
                choice = parsed;
            }
            other => {
                eprintln!(
                    "unknown argument {other:?}; usage: vulkan-probe [--json] [--path auto|tiered|compat]"
                );
                std::process::exit(2);
            }
        }
    }
    (json, choice)
}

fn main() {
    let (json, choice) = parse_args();
    let probe = match moba_render::probe_vulkan() {
        Ok(probe) => probe,
        Err(error) => {
            eprintln!("vulkan probe failed: {error}");
            std::process::exit(1);
        }
    };
    let devices = match moba_render::probe_devices(choice, true) {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("vulkan device probe failed: {error}");
            std::process::exit(1);
        }
    };
    if json {
        let capabilities: Vec<_> = devices.iter().map(|device| &device.capabilities).collect();
        match serde_json::to_string_pretty(&capabilities) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("could not encode the capabilities: {error}");
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "vulkan probe: api={}.{}.{} devices={} portability={}",
            probe.api_version >> 22,
            (probe.api_version >> 12) & 0x3ff,
            probe.api_version & 0xfff,
            probe.devices,
            probe.portability
        );
        for (index, device) in devices.iter().enumerate() {
            let caps = &device.capabilities;
            println!(
                "device {index}: \"{}\" kind={:?} api={} driver=\"{}\" vendor=0x{:04x} device=0x{:04x} {}",
                caps.name,
                caps.kind,
                caps.api_version,
                caps.driver_name,
                caps.vendor_id,
                caps.device_id,
                device.admission.log_fields()
            );
        }
    }
    if probe.devices == 0 {
        eprintln!("no Vulkan physical devices enumerated");
        std::process::exit(1);
    }
    // The renderer's choice: the highest score, the earlier device on a tie; with
    // `--path tiered` only tiered admissions are candidates.
    let chosen = devices
        .iter()
        .enumerate()
        .filter(|(_, device)| {
            choice != PathChoice::Tiered || device.admission.path == RenderPath::Tiered
        })
        .fold(None::<(usize, u32)>, |best, (index, device)| match best {
            Some((_, score)) if score >= device.score => best,
            _ => Some((index, device.score)),
        });
    let Some((index, _)) = chosen else {
        eprintln!("no device is admitted to the tiered path");
        std::process::exit(1);
    };
    let device = &devices[index];
    let line = format!(
        "chosen: device {index} \"{}\" {}",
        device.capabilities.name,
        device.admission.log_fields()
    );
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}
