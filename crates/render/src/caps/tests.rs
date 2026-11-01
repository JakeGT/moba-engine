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
        assert_eq!(
            fixture(name).source,
            "synthetic",
            "{name}: labelled synthetic"
        );
    }
}

#[test]
fn the_rtx_5070_ti_is_tiered_up_to_overdrive() {
    let admission = admit(&fixture("rtx-5070-ti"), on(PathChoice::Auto));
    assert_eq!(admission.path, RenderPath::Tiered);
    assert_eq!(admission.max_tier, Some(Tier::Overdrive));
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
    assert!(admission.max_tier >= Some(Tier::Ultra));
}

#[test]
fn lavapipe_is_compat_unless_tiered_is_asked_for() {
    let caps = fixture("lavapipe");
    assert_eq!(caps.kind, DeviceKind::Cpu);
    let auto = admit(&caps, on(PathChoice::Auto));
    assert_eq!(auto.path, RenderPath::Compat);
    assert_eq!(auto.max_tier, None);
    let tiered = admit(&caps, on(PathChoice::Tiered));
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
    assert_eq!(admission.max_tier, Some(Tier::Ultra));
    // An advertised extension with its feature bit off does not count either.
    let mut caps = fixture("rtx-5070-ti");
    caps.features
        .insert(String::from("rayTracingPipeline"), false);
    assert_eq!(max_tier(&caps), Tier::Ultra);
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
        Tier::ALL.map(Tier::name),
        ["low", "medium", "high", "ultra", "overdrive"]
    );
}
