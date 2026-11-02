//! Selection rules (ADR-0020 §6c): per slot the first available candidate wins; each earlier
//! unavailable one logs exactly one line naming the fallback; nothing is logged when nothing
//! is compiled in. Uses fake probes, so it runs in every build.

#![allow(clippy::unnecessary_wraps)] // Fake probes share the probe signature.

use super::*;

/// The fake probes' context (a stand-in for the device).
struct FakeDevice;

struct Fake(&'static str);

impl Backend for Fake {
    fn name(&self) -> &'static str {
        self.0
    }

    fn detail(&self) -> String {
        format!("fake {}", self.0)
    }
}

type FakeProbe = fn(&FakeDevice) -> Result<Box<dyn Backend>, Unavailable>;

fn ok_xess(_: &FakeDevice) -> Result<Box<dyn Backend>, Unavailable> {
    Ok(Box::new(Fake("xess")))
}

fn ok_nrd(_: &FakeDevice) -> Result<Box<dyn Backend>, Unavailable> {
    Ok(Box::new(Fake("nrd")))
}

fn missing_binary(_: &FakeDevice) -> Result<Box<dyn Backend>, Unavailable> {
    Err(String::from("libfake.so not found"))
}

fn never(_: &FakeDevice) -> Result<Box<dyn Backend>, Unavailable> {
    panic!("a candidate after the slot's winner must not be probed")
}

fn candidate(name: &'static str, slot: Slot, probe: FakeProbe) -> Candidate<FakeProbe> {
    Candidate { name, slot, probe }
}

#[test]
fn open_build_selects_builtins_and_logs_nothing() {
    let selection = resolve::<FakeDevice, FakeProbe>(&[], &FakeDevice, LatencyMode::Reflex);
    assert_eq!(selection.lines(), [] as [String; 0]);
    let backends = selection.backends();
    assert_eq!(backends.upscaler, "builtin");
    assert_eq!(backends.denoiser, "builtin");
    assert_eq!(backends.direct_lighting, "builtin");
    assert_eq!(backends.radiance_cache, "none");
    assert_eq!(backends.frame_generation, "off");
    assert_eq!(backends.latency, LatencyMode::Reflex);
    assert_eq!(backends.compiled, [] as [&str; 0]);
}

#[test]
fn every_missing_binary_logs_one_line_and_falls_back_to_the_builtin() {
    let candidates = [
        candidate("dlss", Slot::Upscaler, missing_binary),
        candidate("fsr4", Slot::Upscaler, missing_binary),
        candidate("nrd", Slot::Denoiser, missing_binary),
    ];
    let selection = resolve(&candidates, &FakeDevice, LatencyMode::None);
    let lines = selection.lines();
    // One line per back end, then the summary.
    assert_eq!(lines.len(), 4, "{lines:#?}");
    assert_eq!(
        lines[0],
        "[vendor] dlss unavailable: libfake.so not found; upscaler falls back to builtin"
    );
    assert_eq!(
        lines[1],
        "[vendor] fsr4 unavailable: libfake.so not found; upscaler falls back to builtin"
    );
    assert_eq!(
        lines[2],
        "[vendor] nrd unavailable: libfake.so not found; denoiser falls back to builtin"
    );
    assert_eq!(
        lines[3],
        "[vendor] selected: upscaler=builtin denoiser=builtin direct_lighting=builtin \
         radiance_cache=none frame_generation=off latency=none compiled=dlss,fsr4,nrd"
    );
}

#[test]
fn a_lower_priority_candidate_takes_over_and_later_ones_are_not_probed() {
    let candidates = [
        candidate("dlss", Slot::Upscaler, missing_binary),
        candidate("xess", Slot::Upscaler, ok_xess),
        candidate("fsr4", Slot::Upscaler, never),
        candidate("nrd", Slot::Denoiser, ok_nrd),
    ];
    let selection = resolve(&candidates, &FakeDevice, LatencyMode::AntiLag);
    let lines = selection.lines();
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(
        lines[0],
        "[vendor] dlss unavailable: libfake.so not found; upscaler falls back to xess"
    );
    assert!(
        lines[1].starts_with(
            "[vendor] selected: upscaler=xess denoiser=nrd direct_lighting=builtin \
             radiance_cache=none frame_generation=off latency=anti-lag \
             compiled=dlss,xess,fsr4,nrd"
        ),
        "{}",
        lines[1]
    );
    assert!(
        lines[1].ends_with("xess=[fake xess] nrd=[fake nrd]"),
        "{}",
        lines[1]
    );
    assert_eq!(selection.backend(Slot::Upscaler), "xess");
    assert_eq!(selection.backend(Slot::Denoiser), "nrd");
}

#[test]
fn latency_comes_from_extensions_reflex_first() {
    let mut caps: DeviceCapabilities =
        serde_json::from_str(include_str!("../caps/fixtures/rtx-5070-ti.json")).expect("fixture");
    assert_eq!(latency::detect(&caps), LatencyMode::Reflex);
    caps.extensions.retain(|name| name != "VK_NV_low_latency2");
    assert_eq!(latency::detect(&caps), LatencyMode::None);
    caps.extensions.push(String::from("VK_AMD_anti_lag"));
    caps.extensions.sort();
    assert_eq!(latency::detect(&caps), LatencyMode::AntiLag);
}

#[test]
fn compiled_candidates_follow_the_feature_set() {
    let names: Vec<&str> = candidates()
        .iter()
        .map(|candidate| candidate.name)
        .collect();
    let mut expected = Vec::new();
    for (enabled, name) in [
        (cfg!(feature = "dlss"), "dlss"),
        (cfg!(feature = "fsr4"), "fsr4"),
        (cfg!(feature = "xess"), "xess"),
        (cfg!(feature = "nrd"), "nrd"),
        (cfg!(feature = "rtxdi"), "rtxdi"),
        (cfg!(feature = "sharc"), "sharc"),
        (cfg!(feature = "streamline"), "streamline"),
    ] {
        if enabled {
            expected.push(name);
        }
    }
    assert_eq!(names, expected);
}
