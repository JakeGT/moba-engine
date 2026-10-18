//! Headless Vulkan instance/device probe for CI portability coverage.
#![allow(clippy::print_stdout)]

fn main() {
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
}
