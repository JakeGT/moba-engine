//! Admission against recorded and synthetic device fixtures (ADR-0020 §2b, gate 2).
//!
//! Recorded fixtures are `vulkan-probe --json` dumps from real devices; synthetic ones (marked
//! `"source": "synthetic"`) stand for devices we do not own and never close a target.

use super::*;

fn fixture(name: &str) -> DeviceCapabilities {
    let text = match name {
        "rtx-5070-ti" => include_str!("fixtures/rtx-5070-ti.json"),
        "radeon-610m" => include_str!("fixtures/radeon-610m.json"),
        "lavapipe" => include_str!("fixtures/lavapipe.json"),
        "moltenvk-m1" => include_str!("fixtures/moltenvk-m1.json"),
        "kosmickrisp-m1" => include_str!("fixtures/kosmickrisp-m1.json"),
        "nvk-rtx-2060" => include_str!("fixtures/nvk-rtx-2060.json"),
        "gtx-1060" => include_str!("fixtures/gtx-1060.json"),
        "uhd-620" => include_str!("fixtures/uhd-620.json"),
        "rt-no-mesh" => include_str!("fixtures/rt-no-mesh.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

const RECORDED: [&str; 3] = ["rtx-5070-ti", "radeon-610m", "lavapipe"];
const SYNTHETIC: [&str; 6] = [
    "moltenvk-m1",
    "kosmickrisp-m1",
    "nvk-rtx-2060",
    "gtx-1060",
    "uhd-620",
    "rt-no-mesh",
];

fn on(choice: PathChoice) -> Overrides {
    Overrides {
        choice,
        host_macos: false,
        allow_cpu: false,
    }
}

#[test]
fn fixtures_are_labelled_sorted_and_plain() {
    for name in RECORDED {
        let caps = fixture(name);
        assert_eq!(caps.source, "recorded", "{name}");
        assert!(
            caps.extensions.windows(2).all(|w| w[0] < w[1]),
            "{name}: sorted, no dups"
        );
        assert!(caps.has_extension("VK_KHR_swapchain"), "{name}");
    }
    for name in SYNTHETIC {
        let caps = fixture(name);
        assert_eq!(caps.source, "synthetic", "{name}: labelled synthetic");
        // `has_extension` binary-searches, so a hand-edited fixture must stay sorted.
        assert!(
            caps.extensions.windows(2).all(|w| w[0] < w[1]),
            "{name}: sorted, no dups"
        );
    }
}

/// A fixture's name and its admission: path, `max_tier`, `missing` and reason.
type Expected = (
    &'static str,
    RenderPath,
    Option<QualityTier>,
    &'static [&'static str],
    AdmissionReason,
);

/// Every fixture's exact admission under `auto` (ADR-0020 gate 2).
#[test]
fn every_fixture_admits_exactly_as_expected() {
    const NO_RT: &[&str] = &[
        "VK_KHR_acceleration_structure",
        "VK_KHR_deferred_host_operations",
        "VK_KHR_ray_query",
    ];
    let table: [Expected; 9] = [
        (
            "rtx-5070-ti",
            RenderPath::Tiered,
            Some(QualityTier::Overdrive),
            &[],
            AdmissionReason::Admitted,
        ),
        (
            "radeon-610m",
            RenderPath::Tiered,
            Some(QualityTier::Overdrive),
            &[],
            AdmissionReason::Admitted,
        ),
        (
            "rt-no-mesh",
            RenderPath::Tiered,
            Some(QualityTier::Overdrive),
            &[],
            AdmissionReason::Admitted,
        ),
        (
            "lavapipe",
            RenderPath::Compat,
            None,
            &[],
            AdmissionReason::CpuDevice,
        ),
        (
            "nvk-rtx-2060",
            RenderPath::Compat,
            None,
            NO_RT,
            AdmissionReason::Missing,
        ),
        (
            "gtx-1060",
            RenderPath::Compat,
            None,
            NO_RT,
            AdmissionReason::Missing,
        ),
        (
            "uhd-620",
            RenderPath::Compat,
            None,
            NO_RT,
            AdmissionReason::Missing,
        ),
        (
            "moltenvk-m1",
            RenderPath::Compat,
            None,
            NO_RT,
            AdmissionReason::MacOs,
        ),
        (
            "kosmickrisp-m1",
            RenderPath::Compat,
            None,
            NO_RT,
            AdmissionReason::MacOs,
        ),
    ];
    for (name, path, tier, missing, reason) in table {
        let admission = admit(&fixture(name), on(PathChoice::Auto));
        assert_eq!(
            (
                admission.path,
                admission.max_tier,
                admission.missing.as_slice(),
                admission.reason
            ),
            (path, tier, missing, reason),
            "{name}"
        );
    }
    // CI's RT goldens need the bundled lavapipe to have the whole required set.
    let lavapipe = admit(
        &fixture("lavapipe"),
        on(PathChoice::Tiered).with_cpu_devices(true),
    );
    assert_eq!(lavapipe.path, RenderPath::Tiered);
    assert_eq!(lavapipe.max_tier, Some(QualityTier::Overdrive));
}

#[test]
fn an_extension_with_its_feature_off_is_missing() {
    let mut caps = fixture("rtx-5070-ti");
    caps.features
        .insert(String::from("accelerationStructure"), false);
    let admission = admit(&caps, on(PathChoice::Auto));
    assert_eq!(admission.path, RenderPath::Compat);
    assert_eq!(admission.missing, ["VK_KHR_acceleration_structure"]);
}

#[test]
fn low_latency_needs_present_id() {
    let low_latency = EXTENSIONS
        .iter()
        .find(|spec| spec.name == "VK_NV_low_latency2")
        .expect("spec");
    let mut caps = fixture("rtx-5070-ti");
    assert!(extension_present(&caps, low_latency));
    caps.features.insert(String::from("presentId"), false);
    assert!(!extension_present(&caps, low_latency));
}

#[test]
fn the_rtx_5070_ti_is_tiered_up_to_overdrive() {
    let admission = admit(&fixture("rtx-5070-ti"), on(PathChoice::Auto));
    assert_eq!(admission.path, RenderPath::Tiered);
    assert_eq!(admission.max_tier, Some(QualityTier::Overdrive));
    assert!(admission.missing.is_empty(), "{:?}", admission.missing);
    assert_eq!(admission.reason, AdmissionReason::Admitted);
    assert_eq!(
        admission.log_fields(),
        "path=tiered max_tier=overdrive missing=none admission=admitted"
    );
}

#[test]
fn the_radeon_610m_is_tiered() {
    // RADV on the iGPU: an RT correctness smoke test, not a performance reference.
    let admission = admit(&fixture("radeon-610m"), on(PathChoice::Auto));
    assert_eq!(
        admission.path,
        RenderPath::Tiered,
        "{:?}",
        admission.missing
    );
    assert!(admission.max_tier >= Some(QualityTier::Ultra));
}

#[test]
fn lavapipe_is_compat_unless_tiered_is_asked_for() {
    let caps = fixture("lavapipe");
    assert_eq!(caps.kind, DeviceKind::Cpu);
    let auto = admit(&caps, on(PathChoice::Auto));
    assert_eq!(auto.path, RenderPath::Compat);
    assert_eq!(auto.max_tier, None);
    // The settings entry's `tiered` never admits a CPU device (no silent software render).
    let settings = admit(&caps, on(PathChoice::Tiered));
    assert_eq!(settings.path, RenderPath::Compat);
    assert_eq!(settings.reason, AdmissionReason::CpuDevice);
    // `MOBA_RENDER_PATH=tiered` does.
    let tiered = admit(&caps, on(PathChoice::Tiered).with_cpu_devices(true));
    if auto.missing.is_empty() {
        // The bundled lavapipe has the whole required set: CI's RT goldens run on it.
        assert_eq!(auto.reason, AdmissionReason::CpuDevice);
        assert!(auto.qualifies);
        assert_eq!(tiered.path, RenderPath::Tiered);
    } else {
        assert_eq!(auto.reason, AdmissionReason::Missing);
        assert_eq!(tiered.path, RenderPath::Compat);
        assert_eq!(tiered.missing, auto.missing);
    }
}

#[test]
fn apple_stacks_are_always_compat() {
    for name in ["moltenvk-m1", "kosmickrisp-m1"] {
        let caps = fixture(name);
        for choice in [PathChoice::Auto, PathChoice::Tiered] {
            let admission = admit(&caps, on(choice));
            assert_eq!(admission.path, RenderPath::Compat, "{name}");
            assert_eq!(admission.reason, AdmissionReason::MacOs, "{name}");
            assert!(!admission.qualifies, "{name}");
        }
    }
    // KosmicKrisp is conformant and has no portability subset: its driver ID decides.
    let kosmickrisp = fixture("kosmickrisp-m1");
    assert!(!kosmickrisp.has_extension("VK_KHR_portability_subset"));
    assert_eq!(kosmickrisp.driver_id, 28);
    // On a macOS host even a fully capable device is Compat.
    let host_macos = Overrides {
        choice: PathChoice::Tiered,
        host_macos: true,
        allow_cpu: true,
    };
    let rtx = admit(&fixture("rtx-5070-ti"), host_macos);
    assert_eq!(rtx.path, RenderPath::Compat);
    assert_eq!(rtx.reason, AdmissionReason::MacOs);
}

#[test]
fn devices_without_hardware_rt_are_compat_and_say_why() {
    for name in ["nvk-rtx-2060", "gtx-1060", "uhd-620"] {
        let admission = admit(&fixture(name), on(PathChoice::Auto));
        assert_eq!(admission.path, RenderPath::Compat, "{name}");
        assert_eq!(admission.reason, AdmissionReason::Missing, "{name}");
        for needed in ["VK_KHR_acceleration_structure", "VK_KHR_ray_query"] {
            assert!(admission.missing.contains(&needed), "{name}: {needed}");
        }
    }
    // NVK has mesh shaders and fragment shading rate but no RT: still Compat.
    let nvk = fixture("nvk-rtx-2060");
    assert!(nvk.has_extension("VK_EXT_mesh_shader"));
}

#[test]
fn mesh_shaders_are_optional() {
    let caps = fixture("rt-no-mesh");
    assert!(!caps.has_extension("VK_EXT_mesh_shader"));
    let admission = admit(&caps, on(PathChoice::Auto));
    assert_eq!(
        admission.path,
        RenderPath::Tiered,
        "{:?}",
        admission.missing
    );
}

#[test]
fn overdrive_needs_the_ray_tracing_pipeline() {
    let mut caps = fixture("rtx-5070-ti");
    caps.extensions
        .retain(|name| name != "VK_KHR_ray_tracing_pipeline");
    let admission = admit(&caps, on(PathChoice::Auto));
    assert_eq!(admission.path, RenderPath::Tiered);
    assert_eq!(admission.max_tier, Some(QualityTier::Ultra));
    // An advertised extension with its feature bit off does not count either.
    let mut caps = fixture("rtx-5070-ti");
    caps.features
        .insert(String::from("rayTracingPipeline"), false);
    assert_eq!(max_tier(&caps), QualityTier::Ultra);
}

#[test]
fn a_short_bindless_limit_is_compat_never_a_smaller_table() {
    let mut caps = fixture("rtx-5070-ti");
    caps.limits.insert(
        String::from("maxPerStageDescriptorUpdateAfterBindSampledImages"),
        BINDLESS_SAMPLED_IMAGES - 1,
    );
    let admission = admit(&caps, on(PathChoice::Auto));
    assert_eq!(admission.path, RenderPath::Compat);
    assert_eq!(
        admission.missing,
        ["maxPerStageDescriptorUpdateAfterBindSampledImages"]
    );
    let mut caps = fixture("rtx-5070-ti");
    caps.features.remove("descriptorBindingPartiallyBound");
    assert_eq!(missing_items(&caps), ["descriptorBindingPartiallyBound"]);
    let mut caps = fixture("rtx-5070-ti");
    caps.api_version = String::from("1.2.198");
    assert_eq!(missing_items(&caps), ["vulkan-1.3"]);
}

/// ADR-0021 Q7: the tiered path's per-attachment blend states need `independentBlend`, and
/// every fixture (each recorded device reports it) has it.
#[test]
fn independent_blend_is_required_on_tiered() {
    for name in RECORDED.iter().chain(&SYNTHETIC) {
        assert!(
            fixture(name).feature("independentBlend"),
            "{name} lacks independentBlend"
        );
    }
    let mut caps = fixture("rtx-5070-ti");
    caps.features
        .insert(String::from("independentBlend"), false);
    assert_eq!(missing_items(&caps), ["independentBlend"]);
    assert_eq!(admit(&caps, on(PathChoice::Auto)).path, RenderPath::Compat);
}

#[test]
fn compat_can_be_chosen_on_any_device() {
    let admission = admit(&fixture("rtx-5070-ti"), on(PathChoice::Compat));
    assert_eq!(admission.path, RenderPath::Compat);
    assert_eq!(admission.reason, AdmissionReason::CompatRequested);
    assert!(admission.qualifies);
    assert_eq!(admission.max_tier, None);
}

#[test]
fn ranking_is_device_type_first_then_path() {
    let score = |name: &str, shared: bool| {
        let caps = fixture(name);
        rank(caps.kind, &admit(&caps, on(PathChoice::Auto)), shared)
    };
    // A non-RT discrete GPU beats an RT-capable iGPU (and runs Compat).
    assert!(score("gtx-1060", false) > score("radeon-610m", true));
    // A GPU always beats lavapipe.
    assert!(score("uhd-620", false) > score("lavapipe", true));
    // Within one type, Tiered beats Compat even when only Compat shares its queue family.
    let tiered = admit(&fixture("rt-no-mesh"), on(PathChoice::Auto));
    let compat = admit(&fixture("gtx-1060"), on(PathChoice::Auto));
    assert!(rank(DeviceKind::Discrete, &tiered, false) > rank(DeviceKind::Discrete, &compat, true));
    // `compat` ranks exactly like `auto`, so a Compat/Tiered A/B stays on one GPU.
    for name in RECORDED.into_iter().chain(SYNTHETIC) {
        let caps = fixture(name);
        for shared in [false, true] {
            assert_eq!(
                rank(caps.kind, &admit(&caps, on(PathChoice::Compat)), shared),
                rank(caps.kind, &admit(&caps, on(PathChoice::Auto)), shared),
                "{name}"
            );
        }
    }
}

#[test]
fn every_extension_spec_is_well_formed() {
    let mut names: Vec<&str> = EXTENSIONS.iter().map(|spec| spec.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), EXTENSIONS.len(), "no duplicate specs");
    assert!(names.iter().all(|name| name.starts_with("VK_")));
    // The required extensions and features of a recorded tiered device are all reported.
    let rtx = fixture("rtx-5070-ti");
    for feature in REQUIRED_FEATURES {
        assert!(
            rtx.features.contains_key(*feature),
            "probe records {feature}"
        );
    }
    for (limit, _) in REQUIRED_LIMITS {
        assert!(rtx.limits.contains_key(*limit), "probe records {limit}");
    }
    for feature in VULKAN14_OPTIONAL {
        assert!(
            rtx.features.contains_key(*feature),
            "probe records {feature}"
        );
    }
}

#[test]
fn path_choices_parse() {
    assert_eq!(PathChoice::parse("auto"), Some(PathChoice::Auto));
    assert_eq!(PathChoice::parse(""), Some(PathChoice::Auto));
    assert_eq!(PathChoice::parse(" Tiered "), Some(PathChoice::Tiered));
    assert_eq!(PathChoice::parse("COMPAT"), Some(PathChoice::Compat));
    assert_eq!(PathChoice::parse("fast"), None);
    assert_eq!(
        QualityTier::ALL
            .iter()
            .map(|tier| tier.name())
            .collect::<Vec<_>>(),
        ["low", "medium", "high", "ultra", "overdrive"]
    );
}

#[test]
fn default_tiers_come_from_the_reference_table() {
    let tier = |name: &str, choice| {
        let caps = fixture(name);
        default_tier(&caps, &admit(&caps, on(choice)))
    };
    // The RTX 5070 Ti is an Ultra reference GPU; the Radeon 610M is not in the table.
    assert_eq!(tier("rtx-5070-ti", PathChoice::Auto), QualityTier::Ultra);
    assert_eq!(tier("radeon-610m", PathChoice::Auto), QualityTier::Low);
    // Compat devices start on Compat Medium, whatever their ID.
    assert_eq!(tier("gtx-1060", PathChoice::Auto), QualityTier::Medium);
    assert_eq!(tier("lavapipe", PathChoice::Auto), QualityTier::Medium);
    assert_eq!(tier("rtx-5070-ti", PathChoice::Compat), QualityTier::Medium);
    // Settings' `tiered` leaves lavapipe on Compat; `MOBA_RENDER_PATH=tiered` admits it.
    assert_eq!(tier("lavapipe", PathChoice::Tiered), QualityTier::Medium);
    let lavapipe = fixture("lavapipe");
    let allowed = on(PathChoice::Tiered).with_cpu_devices(true);
    assert_eq!(
        default_tier(&lavapipe, &admit(&lavapipe, allowed)),
        QualityTier::Low
    );
    // A table tier is capped by what the device admits: an Overdrive card without the RT
    // pipeline starts on Ultra.
    let mut caps = fixture("rtx-5070-ti");
    caps.device_id = 0x2C02; // RTX 5080
    assert_eq!(
        default_tier(&caps, &admit(&caps, on(PathChoice::Auto))),
        QualityTier::Overdrive
    );
    caps.extensions
        .retain(|name| name != "VK_KHR_ray_tracing_pipeline");
    assert_eq!(
        default_tier(&caps, &admit(&caps, on(PathChoice::Auto))),
        QualityTier::Ultra
    );
    // Every entry is unique and names a known vendor.
    let mut keys: Vec<_> = DEFAULT_TIERS.iter().map(|&(v, d, _)| (v, d)).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), DEFAULT_TIERS.len());
    assert!(
        DEFAULT_TIERS
            .iter()
            .all(|&(v, _, _)| v == NVIDIA || v == AMD)
    );
}
