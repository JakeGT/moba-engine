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
    // ADR-0021 amendment 5: once TAAU runs in the slot's place (auto passes XeSS over off
    // Intel), the public view and the summary name what runs; the slot's choice stays.
    let mut selection = selection;
    selection.set_upscaler_in_use("builtin");
    assert_eq!(selection.backends().upscaler, "builtin");
    assert_eq!(selection.backend(Slot::Upscaler), "xess");
    let summary = selection
        .summary()
        .expect("a summary with back ends compiled in");
    assert!(
        summary.starts_with("[vendor] selected: upscaler=builtin denoiser=nrd"),
        "{summary}"
    );
}

fn fixture(name: &str) -> DeviceCapabilities {
    let text = match name {
        "rtx-5070-ti" => include_str!("../caps/fixtures/rtx-5070-ti.json"),
        "gtx-1060" => include_str!("../caps/fixtures/gtx-1060.json"),
        "nvk-rtx-2060" => include_str!("../caps/fixtures/nvk-rtx-2060.json"),
        "radeon-610m" => include_str!("../caps/fixtures/radeon-610m.json"),
        "lavapipe" => include_str!("../caps/fixtures/lavapipe.json"),
        _ => panic!("no fixture {name}"),
    };
    serde_json::from_str(text).expect("fixture")
}

/// The device extensions `caps` is created with besides swapchain: the tiered set of its own
/// admission (what `DeviceContext` enables in the open build).
fn enabled_on(caps: &DeviceCapabilities) -> (RenderPath, Vec<String>) {
    // The fixtures are judged as on Linux or Windows: on a macOS host every device is Compat.
    let mut overrides = crate::caps::Overrides::new(crate::caps::PathChoice::Auto);
    overrides.host_macos = false;
    let path = crate::caps::admit(caps, overrides).path;
    let names = crate::device::TieredExtensions::of(caps, path)
        .names
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    (path, names)
}

#[test]
fn latency_comes_from_enabled_extensions_reflex_first() {
    let enabled = |names: &[&str]| {
        names
            .iter()
            .map(|name| String::from(*name))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        latency::detect(&enabled(&["VK_NV_low_latency2", "VK_AMD_anti_lag"])),
        LatencyMode::Reflex
    );
    assert_eq!(
        latency::detect(&enabled(&["VK_AMD_anti_lag"])),
        LatencyMode::AntiLag
    );
    assert_eq!(
        latency::detect(&enabled(&["VK_KHR_present_id"])),
        LatencyMode::None
    );
    assert_eq!(latency::detect(&[]), LatencyMode::None);
}

#[test]
fn latency_follows_what_the_device_enables_not_what_it_advertises() {
    // The RTX 5070 Ti is tiered with presentId: VK_NV_low_latency2 is enabled.
    let caps = fixture("rtx-5070-ti");
    let (path, enabled) = enabled_on(&caps);
    assert_eq!(path, RenderPath::Tiered);
    assert_eq!(latency::detect(&enabled), LatencyMode::Reflex);
    // The GTX 1060 advertises VK_NV_low_latency2 but runs Compat, which enables neither
    // latency extension.
    let caps = fixture("gtx-1060");
    assert!(caps.has_extension("VK_NV_low_latency2"));
    let (path, enabled) = enabled_on(&caps);
    assert_eq!(path, RenderPath::Compat);
    assert_eq!(latency::detect(&enabled), LatencyMode::None);
}

#[test]
fn nvidia_probes_need_nvidias_driver_not_just_its_vendor_id() {
    assert!(nvidia_driver(&fixture("rtx-5070-ti"), "DLSS").is_ok());
    // NVK reports NVIDIA's PCI vendor ID on Mesa's driver: NGX must not run there.
    let nvk = fixture("nvk-rtx-2060");
    assert_eq!(nvk.vendor_id, pci::NVIDIA);
    let reason = nvidia_driver(&nvk, "DLSS").expect_err("NVK is not NVIDIA's driver");
    assert!(
        reason.starts_with("DLSS needs an NVIDIA RTX GPU on NVIDIA's driver, and the device is "),
        "{reason}"
    );
    assert!(reason.ends_with("(vendor 0x10de, driver NVK)"), "{reason}");
    assert!(nvidia_driver(&fixture("radeon-610m"), "DLSS").is_err());
    assert!(nvidia_driver(&fixture("lavapipe"), "DLSS").is_err());
}

#[test]
fn fsr4_needs_amds_own_driver() {
    // RADV is Mesa's driver for AMD GPUs; the signed FSR 4 binaries need AMD's.
    let radv = fixture("radeon-610m");
    assert_eq!(radv.vendor_id, pci::AMD);
    let reason = amd_driver(&radv).expect_err("RADV is not AMD's driver");
    assert!(
        reason.starts_with("needs an AMD GPU on AMD's driver"),
        "{reason}"
    );
    let mut proprietary = radv;
    proprietary.driver_id = driver::AMD_PROPRIETARY;
    assert!(amd_driver(&proprietary).is_ok());
    assert!(amd_driver(&fixture("rtx-5070-ti")).is_err());
}

#[test]
fn requirements_are_checked_against_the_enabled_sets() {
    let names = |list: &[&str]| {
        list.iter()
            .map(|name| String::from(*name))
            .collect::<Vec<_>>()
    };
    let requested = names(&["VK_NVX_binary_import", "VK_NVX_image_view_handle"]);
    assert_eq!(
        require_enabled(
            "DLSS",
            "device",
            &requested,
            &names(&["VK_KHR_swapchain", "VK_NVX_binary_import"]),
            RenderPath::Tiered
        ),
        Err(String::from(
            "DLSS needs device extension VK_NVX_image_view_handle, which the device does not enable"
        ))
    );
    assert_eq!(
        require_enabled("DLSS", "device", &requested, &requested, RenderPath::Tiered),
        Ok(())
    );
    // The tiered path's core bufferDeviceAddress stands in for the EXT extension, which is
    // never enabled beside it.
    let bda = names(&["VK_EXT_buffer_device_address"]);
    assert_eq!(
        require_enabled("DLSS", "device", &bda, &[], RenderPath::Tiered),
        Ok(())
    );
    assert!(require_enabled("DLSS", "device", &bda, &[], RenderPath::Compat).is_err());
    assert_eq!(
        require_enabled(
            "XeSS",
            "instance",
            &names(&["VK_A", "VK_B"]),
            &[],
            RenderPath::Tiered
        ),
        Err(String::from(
            "XeSS needs instance extensions VK_A, VK_B, which the instance does not enable"
        ))
    );
}

#[test]
fn the_open_build_asks_for_no_extensions() {
    if candidates().is_empty() {
        assert_eq!(instance_extensions(), Vec::<CString>::new());
    }
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

/// C strings for the extension-planning tests.
fn c_names(list: &[&str]) -> Vec<CString> {
    list.iter()
        .map(|name| CString::new(*name).expect("no NUL"))
        .collect()
}

fn plain(names: &[CString]) -> Vec<String> {
    names
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn vendor_requests_drop_the_ext_device_address_and_what_the_device_lacks() {
    use crate::device::{Promoted12, vendor_extensions};
    // The RTX 5070 Ti advertises all three; lavapipe has neither NVX extension.
    let requested = c_names(&[
        "VK_EXT_buffer_device_address",
        "VK_NVX_binary_import",
        "VK_NVX_image_view_handle",
    ]);
    let rtx = vendor_extensions(
        requested.clone(),
        &fixture("rtx-5070-ti"),
        &[c"VK_KHR_swapchain"],
        Promoted12::default(),
    );
    // The tiered path's core bufferDeviceAddress stands in for the EXT extension, which must
    // not be enabled beside it (VUID-VkDeviceCreateInfo-pNext-04748).
    assert_eq!(
        plain(&rtx.names),
        ["VK_NVX_binary_import", "VK_NVX_image_view_handle"]
    );
    assert_eq!(rtx.features, Promoted12::default());
    let lavapipe = vendor_extensions(
        requested,
        &fixture("lavapipe"),
        &[c"VK_KHR_swapchain"],
        Promoted12::default(),
    );
    assert!(lavapipe.names.is_empty(), "{:?}", lavapipe.names);
}

#[test]
fn core_promoted_vendor_requests_set_their_vulkan_12_bits_or_are_dropped() {
    use crate::device::{Promoted12, vendor_extensions};
    let requested = c_names(&[
        "VK_EXT_sampler_filter_minmax",
        "VK_EXT_shader_viewport_index_layer",
        "VK_KHR_sampler_mirror_clamp_to_edge",
    ]);
    let all = Promoted12 {
        sampler_mirror_clamp_to_edge: true,
        sampler_filter_minmax: true,
        shader_output_viewport_index: true,
        shader_output_layer: true,
    };
    let supported = vendor_extensions(requested.clone(), &fixture("rtx-5070-ti"), &[], all);
    assert_eq!(plain(&supported.names), plain(&requested));
    assert_eq!(supported.features, all);
    // Unsupported bits: the name is dropped rather than enabled with its bit off
    // (VUID-VkDeviceCreateInfo-ppEnabledExtensionNames-02832, -02834, -02835).
    let partial = Promoted12 {
        sampler_filter_minmax: true,
        shader_output_viewport_index: true,
        ..Promoted12::default()
    };
    let some = vendor_extensions(requested, &fixture("rtx-5070-ti"), &[], partial);
    assert_eq!(plain(&some.names), ["VK_EXT_sampler_filter_minmax"]);
    assert_eq!(
        some.features,
        Promoted12 {
            sampler_filter_minmax: true,
            ..Promoted12::default()
        }
    );
}

#[test]
fn vendor_requests_whose_dependencies_are_not_enabled_are_dropped() {
    use crate::device::{Promoted12, vendor_extensions};
    let caps = fixture("rtx-5070-ti");
    // Ray query needs the acceleration structure extension, which needs deferred host
    // operations: with neither in the base set, both requests that depend on them go.
    let requested = c_names(&["VK_KHR_acceleration_structure", "VK_KHR_ray_query"]);
    let none = vendor_extensions(requested.clone(), &caps, &[], Promoted12::default());
    assert!(none.names.is_empty(), "{:?}", none.names);
    // With deferred host operations already enabled, both stay.
    let both = vendor_extensions(
        requested,
        &caps,
        &[c"VK_KHR_deferred_host_operations"],
        Promoted12::default(),
    );
    assert_eq!(
        plain(&both.names),
        ["VK_KHR_acceleration_structure", "VK_KHR_ray_query"]
    );
    // A dependency the device enables anyway (swapchain) counts.
    let present_wait = c_names(&["VK_KHR_present_id", "VK_KHR_present_wait"]);
    let with_swapchain = vendor_extensions(
        present_wait.clone(),
        &caps,
        &[c"VK_KHR_swapchain"],
        Promoted12::default(),
    );
    assert_eq!(plain(&with_swapchain.names), plain(&present_wait));
    let without = vendor_extensions(present_wait, &caps, &[], Promoted12::default());
    assert!(without.names.is_empty(), "{:?}", without.names);
}

/// Meaningful only in a build with a back end that requests device extensions (`dlss`, as the
/// vendor-sdks CI job builds it): in the open build `device_extensions` is always empty.
#[test]
fn nothing_is_requested_before_a_compat_device_is_created() {
    // SAFETY: the loader returns null for every name, so ash fills each entry point with a
    // stub that panics if called. Nothing is called: on Compat `device_extensions` returns
    // before any back end's query.
    let instance = unsafe { ash::Instance::load_with(|_| std::ptr::null(), vk::Instance::null()) };
    for name in ["rtx-5070-ti", "gtx-1060", "lavapipe"] {
        let caps = fixture(name);
        let query = DeviceQuery {
            instance: &instance,
            physical: vk::PhysicalDevice::null(),
            capabilities: &caps,
            path: RenderPath::Compat,
        };
        assert_eq!(device_extensions(&query), Vec::<CString>::new(), "{name}");
    }
}

#[test]
fn ray_traced_lighting_needs_the_tiered_path_and_ray_queries() {
    // Lavapipe has ray queries: available on the tiered path, not on Compat (its default).
    let lavapipe = fixture("lavapipe");
    let (path, _) = enabled_on(&lavapipe);
    assert_eq!(path, RenderPath::Compat);
    assert_eq!(
        ray_traced_lighting_on(path, &lavapipe),
        Err(String::from(
            "the device runs the compat path, which has no ray-traced lighting"
        ))
    );
    assert_eq!(
        ray_traced_lighting_on(RenderPath::Tiered, &lavapipe),
        Ok(())
    );
    // The GTX 1060 has no ray queries, whatever the path.
    let gtx = fixture("gtx-1060");
    assert!(ray_traced_lighting_on(RenderPath::Compat, &gtx).is_err());
    assert_eq!(
        ray_traced_lighting_on(RenderPath::Tiered, &gtx),
        Err(String::from("the device has no rayQuery"))
    );
}

/// `caps` as if it also advertised `VK_AMD_anti_lag` with its feature (Mesa 26.2's layer does
/// on every device; Mesa 26.3's RADV natively).
fn with_anti_lag(mut caps: DeviceCapabilities) -> DeviceCapabilities {
    caps.extensions.push(String::from("VK_AMD_anti_lag"));
    caps.extensions.sort();
    caps.features.insert(String::from("antiLag"), true);
    caps
}

#[test]
fn only_one_latency_extension_is_enabled_reflex_first() {
    // ADR-0021 §6a: NVIDIA's wins, and the other is not even enabled.
    let (path, enabled) = enabled_on(&with_anti_lag(fixture("rtx-5070-ti")));
    assert_eq!(path, RenderPath::Tiered);
    assert!(enabled.iter().any(|name| name == "VK_NV_low_latency2"));
    assert!(!enabled.iter().any(|name| name == "VK_AMD_anti_lag"));
    assert_eq!(latency::detect(&enabled), LatencyMode::Reflex);
    // RADV with native Anti-Lag (Mesa 26.3) runs it.
    let caps = with_anti_lag(fixture("radeon-610m"));
    let (path, enabled) = enabled_on(&caps);
    assert_eq!(path, RenderPath::Tiered);
    assert_eq!(latency::detect(&enabled), LatencyMode::AntiLag);
    // Today's RADV (Mesa 26.2.5) advertises neither natively: no back end.
    let (_, enabled) = enabled_on(&fixture("radeon-610m"));
    assert_eq!(latency::detect(&enabled), LatencyMode::None);
}
