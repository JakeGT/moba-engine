//! Device capabilities and the render-path gate (ADR-0020 §1e, §2, §3b).
//!
//! One probe per physical device reads its extensions, one `vkGetPhysicalDeviceFeatures2`
//! chain and one `vkGetPhysicalDeviceProperties2` chain into [`DeviceCapabilities`]: plain data
//! with no Vulkan pointers, printed by `vulkan-probe --json` and recorded as test fixtures.
//! [`admit`] is a pure function of that data: it picks [`RenderPath::Tiered`] or
//! [`RenderPath::Compat`] and names everything a Compat device lacks.

use std::collections::BTreeMap;
use std::ffi::CStr;

use ash::vk;
use ash::vk::TaggedStructure as _;
use serde::{Deserialize, Serialize};

use crate::quality::QualityTier;

/// Which renderer a device runs (ADR-0020 §2a).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RenderPath {
    /// The next-gen renderer N1–N11 build. In N0 it still runs today's frame.
    Tiered,
    /// Today's forward renderer: Vulkan 1.3, plain descriptor sets, GLSL; frozen after C1.
    Compat,
}

impl RenderPath {
    /// The lower-case name logged as `path=`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tiered => "tiered",
            Self::Compat => "compat",
        }
    }
}

/// The player's or the environment's choice of path (`MOBA_RENDER_PATH`, ADR-0020 §2b).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PathChoice {
    /// Rank devices, then use the chosen device's own admission. CPU devices run Compat.
    #[default]
    Auto,
    /// Only devices admitted to Tiered are candidates; none is an error. CPU devices qualify
    /// only with [`Overrides::allow_cpu`].
    Tiered,
    /// Same ranking as `Auto` (see `rank`); the chosen device runs Compat.
    Compat,
}

impl PathChoice {
    /// Parses `auto`, `tiered` or `compat` (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "tiered" => Some(Self::Tiered),
            "compat" => Some(Self::Compat),
            _ => None,
        }
    }
}

/// What the gate sees besides the device: the choice and the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Overrides {
    /// `MOBA_RENDER_PATH` or the settings entry.
    pub choice: PathChoice,
    /// The process runs on macOS (`cfg!(target_os = "macos")`), which is always Compat.
    pub host_macos: bool,
    /// [`PathChoice::Tiered`] may admit CPU devices such as lavapipe. Only the
    /// `MOBA_RENDER_PATH=tiered` override and `vulkan-probe` (for every `--path`) set it, for
    /// CI's RT goldens; the settings entry never does, so a player's `tiered` never silently
    /// runs on a software rasteriser.
    pub allow_cpu: bool,
}

impl Overrides {
    /// The given choice on this host.
    #[must_use]
    pub const fn new(choice: PathChoice) -> Self {
        Self {
            choice,
            host_macos: cfg!(target_os = "macos"),
            allow_cpu: false,
        }
    }

    /// Returns these overrides with [`Self::allow_cpu`] set.
    #[must_use]
    pub const fn with_cpu_devices(mut self, allow: bool) -> Self {
        self.allow_cpu = allow;
        self
    }
}

/// `VkPhysicalDeviceType`, as plain data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DeviceKind {
    /// A discrete GPU.
    Discrete,
    /// An integrated GPU.
    Integrated,
    /// A virtual GPU.
    Virtual,
    /// A CPU implementation (lavapipe, `SwiftShader`).
    Cpu,
    /// Anything else.
    Other,
}

impl DeviceKind {
    /// Maps the Vulkan enum.
    #[must_use]
    pub fn from_vk(kind: vk::PhysicalDeviceType) -> Self {
        match kind {
            vk::PhysicalDeviceType::DISCRETE_GPU => Self::Discrete,
            vk::PhysicalDeviceType::INTEGRATED_GPU => Self::Integrated,
            vk::PhysicalDeviceType::VIRTUAL_GPU => Self::Virtual,
            vk::PhysicalDeviceType::CPU => Self::Cpu,
            _ => Self::Other,
        }
    }

    /// The device-type rank of ADR-0020 §2b: discrete > integrated > virtual > CPU.
    #[must_use]
    pub const fn rank(self) -> u32 {
        match self {
            Self::Discrete => 1000,
            Self::Integrated => 500,
            Self::Virtual => 200,
            Self::Cpu => 100,
            Self::Other => 10,
        }
    }
}

/// One physical device's capabilities, as plain data (ADR-0020 §1e).
///
/// Features and limits are keyed by their Vulkan names (`rayQuery`,
/// `maxPerStageDescriptorUpdateAfterBindSampledImages`); a feature whose structure was not
/// queried (its extension is not advertised) is absent, which reads as `false`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    /// How this record was made: `"recorded"` (a `vulkan-probe --json` dump) or
    /// `"synthetic"` (written by hand for a device we do not own; never closes a target).
    pub source: String,
    /// `deviceName`.
    pub name: String,
    /// PCI vendor ID.
    pub vendor_id: u32,
    /// PCI device ID.
    pub device_id: u32,
    /// Device type.
    pub kind: DeviceKind,
    /// `apiVersion` as `major.minor.patch`.
    pub api_version: String,
    /// `VkDriverId` (0 when unknown).
    pub driver_id: u32,
    /// `driverName`.
    pub driver_name: String,
    /// `driverInfo`.
    pub driver_info: String,
    /// Advertised device extensions, sorted.
    pub extensions: Vec<String>,
    /// Feature bits by Vulkan name.
    pub features: BTreeMap<String, bool>,
    /// Limits by Vulkan name.
    pub limits: BTreeMap<String, u64>,
}

impl DeviceCapabilities {
    /// Whether the device advertises `extension`.
    #[must_use]
    pub fn has_extension(&self, extension: &str) -> bool {
        self.extensions
            .binary_search_by(|name| name.as_str().cmp(extension))
            .is_ok()
    }

    /// Whether `feature` is reported and true.
    #[must_use]
    pub fn feature(&self, feature: &str) -> bool {
        self.features.get(feature).copied().unwrap_or(false)
    }

    /// The limit `name`, or 0 when not reported.
    #[must_use]
    pub fn limit(&self, name: &str) -> u64 {
        self.limits.get(name).copied().unwrap_or(0)
    }

    /// `(major, minor)` of [`Self::api_version`].
    #[must_use]
    pub fn api_major_minor(&self) -> (u32, u32) {
        let mut parts = self
            .api_version
            .split('.')
            .map(|part| part.parse().unwrap_or(0));
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    }
}

/// How an extension in [`EXTENSIONS`] is treated on the tiered path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Requirement {
    /// A device without it is Compat.
    Required,
    /// Enabled when present; each user has a fallback.
    Optional,
    /// Needed only from this tier up; without it the device tops out one tier lower.
    ForTier(QualityTier),
}

/// One extension of the tiered set and the feature bits it brings (ADR-0020 §1e).
#[derive(Clone, Copy, Debug)]
pub struct ExtensionSpec {
    /// The extension name.
    pub name: &'static str,
    /// How the gate treats it.
    pub requirement: Requirement,
    /// Feature bits (Vulkan names) that must be true for the extension to count as present.
    pub features: &'static [&'static str],
}

/// The tiered path's extensions (ADR-0020 §1e). The vendor back ends' requests are added at
/// device creation (`vendor::device_extensions`), not here.
pub const EXTENSIONS: &[ExtensionSpec] = &[
    spec("VK_KHR_swapchain", Requirement::Required, &[]),
    spec(
        "VK_KHR_acceleration_structure",
        Requirement::Required,
        &["accelerationStructure"],
    ),
    spec(
        "VK_KHR_deferred_host_operations",
        Requirement::Required,
        &[],
    ),
    spec("VK_KHR_ray_query", Requirement::Required, &["rayQuery"]),
    spec(
        "VK_KHR_ray_tracing_pipeline",
        Requirement::ForTier(QualityTier::Overdrive),
        &["rayTracingPipeline"],
    ),
    spec(
        "VK_KHR_ray_tracing_maintenance1",
        Requirement::ForTier(QualityTier::Overdrive),
        &["rayTracingMaintenance1"],
    ),
    spec(
        "VK_KHR_ray_tracing_position_fetch",
        Requirement::ForTier(QualityTier::Overdrive),
        &["rayTracingPositionFetch"],
    ),
    // SER reorders ray-tracing-pipeline invocations, so it also needs that pipeline.
    spec(
        "VK_EXT_ray_tracing_invocation_reorder",
        Requirement::Optional,
        &["rayTracingInvocationReorder", "rayTracingPipeline"],
    ),
    spec(
        "VK_EXT_mesh_shader",
        Requirement::Optional,
        &["meshShader", "taskShader"],
    ),
    spec(
        "VK_KHR_fragment_shading_rate",
        Requirement::Optional,
        &["pipelineFragmentShadingRate"],
    ),
    // Depends on `VK_KHR_present_id`, so it needs that feature too.
    spec("VK_NV_low_latency2", Requirement::Optional, &["presentId"]),
    spec("VK_KHR_present_id", Requirement::Optional, &["presentId"]),
    spec("VK_AMD_anti_lag", Requirement::Optional, &["antiLag"]),
];

const fn spec(
    name: &'static str,
    requirement: Requirement,
    features: &'static [&'static str],
) -> ExtensionSpec {
    ExtensionSpec {
        name,
        requirement,
        features,
    }
}

/// Core feature bits the tiered path requires (ADR-0020 §1e and §3b).
pub const REQUIRED_FEATURES: &[&str] = &[
    // Core 1.3.
    "dynamicRendering",
    "synchronization2",
    "maintenance4",
    // Core 1.2.
    "bufferDeviceAddress",
    "descriptorIndexing",
    "runtimeDescriptorArray",
    "scalarBlockLayout",
    "timelineSemaphore",
    "drawIndirectCount",
    "hostQueryReset",
    // Bindless (§3b).
    "shaderSampledImageArrayNonUniformIndexing",
    "shaderStorageImageArrayNonUniformIndexing",
    "descriptorBindingSampledImageUpdateAfterBind",
    "descriptorBindingStorageImageUpdateAfterBind",
    "descriptorBindingPartiallyBound",
    "descriptorBindingVariableDescriptorCount",
    "descriptorBindingUpdateUnusedWhilePending",
    // Core 1.0, for bindless storage images (§3a).
    "shaderStorageImageWriteWithoutFormat",
    // Core 1.0: per-attachment blend state for the prepass's motion and IDs and the
    // particles' reactive mask (ADR-0021 §2e, Q7).
    "independentBlend",
];

/// The bindless table's capacities (ADR-0020 §3a): samplers, storage images, textures.
pub const BINDLESS_SAMPLERS: u64 = 32;
/// Storage images in the bindless table.
pub const BINDLESS_STORAGE_IMAGES: u64 = 1_024;
/// Sampled images (textures) in the bindless table.
pub const BINDLESS_SAMPLED_IMAGES: u64 = 16_384;
const BINDLESS_TOTAL: u64 = BINDLESS_SAMPLERS + BINDLESS_STORAGE_IMAGES + BINDLESS_SAMPLED_IMAGES;

/// Limits the tiered path relies on and their minimums (ADR-0020 §3b). A failure means
/// Compat, never a smaller table.
pub const REQUIRED_LIMITS: &[(&str, u64)] = &[
    (
        "maxPerStageDescriptorUpdateAfterBindSampledImages",
        BINDLESS_SAMPLED_IMAGES,
    ),
    (
        "maxDescriptorSetUpdateAfterBindSampledImages",
        BINDLESS_SAMPLED_IMAGES,
    ),
    (
        "maxPerStageDescriptorUpdateAfterBindSamplers",
        BINDLESS_SAMPLERS,
    ),
    (
        "maxPerStageDescriptorUpdateAfterBindStorageImages",
        BINDLESS_STORAGE_IMAGES,
    ),
    ("maxUpdateAfterBindDescriptorsInAllPools", BINDLESS_TOTAL),
    ("maxPerStageUpdateAfterBindResources", BINDLESS_TOTAL),
    // ADR-0021 Amendment 2 item 15: OIT with the temporal prepass outputs writes five colour
    // attachments at once (Vulkan only guarantees four); the fragment stage must be able to
    // output to all five, and they count against its combined output resources.
    ("maxColorAttachments", 5),
    ("maxFragmentOutputAttachments", 5),
    ("maxFragmentCombinedOutputResources", 5),
];

/// Vulkan 1.4-promoted features recorded as optional capabilities (ADR-0020 amendment 1).
/// A tiered pass that adopts one makes it a required item in the same PR.
pub const VULKAN14_OPTIONAL: &[&str] = &[
    "pushDescriptor",
    "maintenance5",
    "maintenance6",
    "dynamicRenderingLocalRead",
    "indexTypeUint8",
    "hostImageCopy",
];

const DRIVER_MOLTENVK: u32 = vk::DriverId::MOLTENVK.as_raw().cast_unsigned();
const DRIVER_KOSMICKRISP: u32 = vk::DriverId::MESA_KOSMICKRISP.as_raw().cast_unsigned();
const PORTABILITY_SUBSET: &str = "VK_KHR_portability_subset";

/// Why the gate chose a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdmissionReason {
    /// Every required item is present.
    Admitted,
    /// Required items are missing (listed in [`Admission::missing`]).
    Missing,
    /// macOS, `MoltenVK`, `KosmicKrisp` or the portability subset: Compat only (§2d).
    MacOs,
    /// A CPU device is Tiered only with `MOBA_RENDER_PATH=tiered`.
    CpuDevice,
    /// `MOBA_RENDER_PATH=compat` (or the settings entry).
    CompatRequested,
}

impl AdmissionReason {
    /// A short lower-case name for logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Missing => "missing",
            Self::MacOs => "macos-compat-only",
            Self::CpuDevice => "cpu-device",
            Self::CompatRequested => "compat-requested",
        }
    }
}

/// The gate's decision for one device.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Admission {
    /// The path the device runs.
    pub path: RenderPath,
    /// The highest tiered tier the device may select; `None` on Compat.
    pub max_tier: Option<QualityTier>,
    /// Required items the device lacks (empty when it qualifies for Tiered).
    pub missing: Vec<&'static str>,
    /// Why [`Self::path`] was chosen.
    pub reason: AdmissionReason,
    /// The device qualifies for Tiered on its own merits (whatever was chosen).
    pub qualifies: bool,
}

impl Admission {
    /// `path=… max_tier=… missing=…` for the frame log and `vulkan-probe`.
    #[must_use]
    pub fn log_fields(&self) -> String {
        let missing = if self.missing.is_empty() {
            String::from("none")
        } else {
            self.missing.join(",")
        };
        format!(
            "path={} max_tier={} missing={missing} admission={}",
            self.path.name(),
            self.max_tier.map_or("none", QualityTier::name),
            self.reason.name()
        )
    }
}

/// Everything the tiered path requires that `caps` lacks, in table order.
#[must_use]
pub fn missing_items(caps: &DeviceCapabilities) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if caps.api_major_minor() < (1, 3) {
        missing.push("vulkan-1.3");
    }
    for extension in EXTENSIONS {
        if extension.requirement == Requirement::Required && !extension_present(caps, extension) {
            missing.push(extension.name);
        }
    }
    for feature in REQUIRED_FEATURES {
        if !caps.feature(feature) {
            missing.push(feature);
        }
    }
    for (limit, minimum) in REQUIRED_LIMITS {
        if caps.limit(limit) < *minimum {
            missing.push(limit);
        }
    }
    missing
}

/// The extension is advertised and every feature bit it brings is true.
#[must_use]
pub(crate) fn extension_present(caps: &DeviceCapabilities, extension: &ExtensionSpec) -> bool {
    caps.has_extension(extension.name)
        && extension
            .features
            .iter()
            .all(|feature| caps.feature(feature))
}

/// The device belongs to Apple's stack: Compat only, whatever it exposes (§2d).
fn apple_stack(caps: &DeviceCapabilities, overrides: Overrides) -> bool {
    overrides.host_macos
        || caps.driver_id == DRIVER_MOLTENVK
        || caps.driver_id == DRIVER_KOSMICKRISP
        || caps.has_extension(PORTABILITY_SUBSET)
}

/// The highest tier a qualifying device may select: Overdrive needs every
/// [`Requirement::ForTier`] extension, otherwise Ultra.
#[must_use]
pub fn max_tier(caps: &DeviceCapabilities) -> QualityTier {
    let overdrive = EXTENSIONS
        .iter()
        .filter(|extension| extension.requirement == Requirement::ForTier(QualityTier::Overdrive))
        .all(|extension| extension_present(caps, extension));
    if overdrive {
        QualityTier::Overdrive
    } else {
        QualityTier::Ultra
    }
}

/// Decides one device's path (ADR-0020 §2b). Pure: unit-tested against the fixtures.
#[must_use]
pub fn admit(caps: &DeviceCapabilities, overrides: Overrides) -> Admission {
    let missing = missing_items(caps);
    let apple = apple_stack(caps, overrides);
    let qualifies = missing.is_empty() && !apple;
    let compat = |reason| Admission {
        path: RenderPath::Compat,
        max_tier: None,
        missing: missing.clone(),
        reason,
        qualifies,
    };
    if apple {
        return compat(AdmissionReason::MacOs);
    }
    if !missing.is_empty() {
        return compat(AdmissionReason::Missing);
    }
    match overrides.choice {
        PathChoice::Compat => compat(AdmissionReason::CompatRequested),
        PathChoice::Auto if caps.kind == DeviceKind::Cpu => compat(AdmissionReason::CpuDevice),
        PathChoice::Tiered if caps.kind == DeviceKind::Cpu && !overrides.allow_cpu => {
            compat(AdmissionReason::CpuDevice)
        }
        PathChoice::Auto | PathChoice::Tiered => Admission {
            path: RenderPath::Tiered,
            max_tier: Some(max_tier(caps)),
            missing,
            reason: AdmissionReason::Admitted,
            qualifies,
        },
    }
}

/// PCI vendor IDs of the default-tier table.
const NVIDIA: u32 = 0x10DE;
const AMD: u32 = 0x1002;

/// The reference GPUs' default tiers (ADR-0020 §2b, roadmap §3), keyed by PCI vendor and
/// device ID from the PCI ID database (hwdata `pci.ids` 2026.10.01). Desktop parts only;
/// an AMD ID covers its whole family (the RX 6600 ID is also the 6600 XT). Cards with less
/// memory than a tier's VRAM budget assumes (§5e) stay a tier lower: the 12 GB RTX 4070 Ti
/// and 4070 SUPER are High, not Ultra.
const DEFAULT_TIERS: &[(u32, u32, QualityTier)] = &[
    // RTX 2060 family (TU106/TU104): Low.
    (NVIDIA, 0x1E89, QualityTier::Low),
    (NVIDIA, 0x1F03, QualityTier::Low),
    (NVIDIA, 0x1F06, QualityTier::Low),
    (NVIDIA, 0x1F08, QualityTier::Low),
    (NVIDIA, 0x1F42, QualityTier::Low),
    (NVIDIA, 0x1F47, QualityTier::Low),
    // RTX 3060 (GA106/GA104): Medium.
    (NVIDIA, 0x2487, QualityTier::Medium),
    (NVIDIA, 0x24C7, QualityTier::Medium),
    (NVIDIA, 0x2501, QualityTier::Medium),
    (NVIDIA, 0x2503, QualityTier::Medium),
    (NVIDIA, 0x2504, QualityTier::Medium),
    (NVIDIA, 0x2509, QualityTier::Medium),
    (NVIDIA, 0x2544, QualityTier::Medium),
    // RTX 4060 Ti, 4070, 4070 SUPER, 4070 Ti (AD106/AD104/AD103): High.
    (NVIDIA, 0x2803, QualityTier::High),
    (NVIDIA, 0x2805, QualityTier::High),
    (NVIDIA, 0x2788, QualityTier::High),
    (NVIDIA, 0x2786, QualityTier::High),
    (NVIDIA, 0x2709, QualityTier::High),
    (NVIDIA, 0x2783, QualityTier::High),
    (NVIDIA, 0x2782, QualityTier::High),
    // RTX 4070 Ti SUPER and 5070 Ti (16 GB): Ultra.
    (NVIDIA, 0x2705, QualityTier::Ultra),
    (NVIDIA, 0x2689, QualityTier::Ultra),
    (NVIDIA, 0x2C05, QualityTier::Ultra),
    // RTX 4080, 4080 SUPER, 4090, 5080, 5090 and their D variants: Overdrive.
    (NVIDIA, 0x2704, QualityTier::Overdrive),
    (NVIDIA, 0x2702, QualityTier::Overdrive),
    (NVIDIA, 0x2703, QualityTier::Overdrive),
    (NVIDIA, 0x2684, QualityTier::Overdrive),
    (NVIDIA, 0x2685, QualityTier::Overdrive),
    (NVIDIA, 0x2C02, QualityTier::Overdrive),
    (NVIDIA, 0x2B85, QualityTier::Overdrive),
    (NVIDIA, 0x2B87, QualityTier::Overdrive),
    (NVIDIA, 0x2B8C, QualityTier::Overdrive),
    // RX 6600 / 6600 XT (Navi 23): Low. RX 6700 / 6700 XT / 6750 XT (Navi 22): Medium.
    (AMD, 0x73FF, QualityTier::Low),
    (AMD, 0x73DF, QualityTier::Medium),
    // RX 7700 XT / 7800 XT (Navi 32): High. RX 9070 / 9070 XT / 9070 GRE (Navi 48): Ultra.
    (AMD, 0x747E, QualityTier::High),
    (AMD, 0x7550, QualityTier::Ultra),
];

/// The tier a device starts on before the player picks one (ADR-0020 §2b): a reference GPU
/// gets its table tier (capped at its `max_tier`), any other device admitted to Tiered gets
/// Low, and a Compat device gets Compat Medium.
#[must_use]
pub fn default_tier(caps: &DeviceCapabilities, admission: &Admission) -> QualityTier {
    let Some(max_tier) = admission
        .max_tier
        .filter(|_| admission.path == RenderPath::Tiered)
    else {
        return QualityTier::Medium;
    };
    DEFAULT_TIERS
        .iter()
        .find(|(vendor, device, _)| *vendor == caps.vendor_id && *device == caps.device_id)
        .map_or(QualityTier::Low, |&(_, _, tier)| tier.min(max_tier))
}

/// The device-selection score (ADR-0020 §2b): device type first, then a GPU that qualifies
/// for Tiered over one that does not, then +10 for a shared graphics and present family.
///
/// It reads `qualifies`, not the chosen path, so `compat` ranks exactly like `auto` and a
/// Compat-against-Tiered A/B stays on the same GPU.
#[must_use]
pub(crate) fn rank(kind: DeviceKind, admission: &Admission, shared_family: bool) -> u32 {
    let tiered = if admission.qualifies && kind != DeviceKind::Cpu {
        20
    } else {
        0
    };
    kind.rank() + tiered + if shared_family { 10 } else { 0 }
}

/// Probes `physical` into plain data: extensions, one features chain, one properties chain.
/// Structures whose extension is not advertised stay out of the chains.
///
/// The instance asks for API 1.3, so no 1.4 core structure is ever chained: the 1.4-promoted
/// features are read through their KHR/EXT extension structures (ADR-0020 amendment 1).
///
/// # Errors
/// Fails only if extension enumeration fails.
pub(crate) fn probe(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<DeviceCapabilities, vk::Result> {
    let extensions = probe_extensions(instance, physical)?;
    // SAFETY: `physical` was enumerated from this live instance.
    let base = unsafe { instance.get_physical_device_properties(physical) };
    // A core structure may be chained only up to min(instance API 1.3, device API).
    let api_12 = base.api_version >= vk::API_VERSION_1_2;
    let features = probe_features(instance, physical, &extensions, base.api_version);
    let mut properties_12 = vk::PhysicalDeviceVulkan12Properties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default();
    if api_12 {
        properties = properties.push(&mut properties_12);
    }
    // SAFETY: as above; the chain holds at most one core 1.2 structure, chained only when the
    // device reports 1.2 or later (the instance asks for 1.3), live for the call.
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    let limits = record_limits(&properties.properties.limits, &properties_12);
    let text = |name: Result<&CStr, core::ffi::FromBytesUntilNulError>| {
        name.map_or_else(
            |_| String::new(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let api = base.api_version;
    Ok(DeviceCapabilities {
        source: String::from("recorded"),
        name: text(base.device_name_as_c_str()),
        vendor_id: base.vendor_id,
        device_id: base.device_id,
        kind: DeviceKind::from_vk(base.device_type),
        api_version: format!(
            "{}.{}.{}",
            vk::api_version_major(api),
            vk::api_version_minor(api),
            vk::api_version_patch(api)
        ),
        driver_id: properties_12.driver_id.as_raw().cast_unsigned(),
        driver_name: text(properties_12.driver_name_as_c_str()),
        driver_info: text(properties_12.driver_info_as_c_str()),
        extensions,
        features,
        limits,
    })
}

/// The device's extension names, sorted and deduplicated.
fn probe_extensions(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<Vec<String>, vk::Result> {
    // SAFETY: `physical` was enumerated from this live instance.
    let available = unsafe { instance.enumerate_device_extension_properties(physical) }?;
    let mut extensions: Vec<String> = available
        .iter()
        .filter_map(|extension| {
            let name = extension.extension_name_as_c_str().ok()?;
            Some(name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .collect();
    extensions.sort_unstable();
    extensions.dedup();
    Ok(extensions)
}

/// One `vkGetPhysicalDeviceFeatures2` query. Every feature of an advertised extension is
/// recorded; a feature whose extension is missing is left out, which `feature()` reads as off.
#[allow(clippy::too_many_lines)] // One chain, then one flat list of recorded bits.
fn probe_features(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    extensions: &[String],
    api_version: u32,
) -> BTreeMap<String, bool> {
    let has = |name: &str| {
        extensions
            .binary_search_by(|e| e.as_str().cmp(name))
            .is_ok()
    };
    let index_u8 = has("VK_KHR_index_type_uint8") || has("VK_EXT_index_type_uint8");
    let mut features_12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut features_13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut acceleration = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut ray_query = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
    let mut ray_pipeline = vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default();
    let mut ray_maintenance = vk::PhysicalDeviceRayTracingMaintenance1FeaturesKHR::default();
    let mut position_fetch = vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR::default();
    let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default();
    let mut shading_rate = vk::PhysicalDeviceFragmentShadingRateFeaturesKHR::default();
    let mut maintenance5 = vk::PhysicalDeviceMaintenance5FeaturesKHR::default();
    let mut maintenance6 = vk::PhysicalDeviceMaintenance6FeaturesKHR::default();
    let mut local_read = vk::PhysicalDeviceDynamicRenderingLocalReadFeaturesKHR::default();
    let mut index_type_u8 = vk::PhysicalDeviceIndexTypeUint8FeaturesKHR::default();
    let mut host_image_copy = vk::PhysicalDeviceHostImageCopyFeaturesEXT::default();
    let mut reorder = vk::PhysicalDeviceRayTracingInvocationReorderFeaturesEXT::default();
    let mut present_id = vk::PhysicalDevicePresentIdFeaturesKHR::default();
    let mut anti_lag = vk::PhysicalDeviceAntiLagFeaturesAMD::default();
    let mut query = vk::PhysicalDeviceFeatures2::default();
    // Core structures only up to the device's version (the instance asks for 1.3). A device
    // below 1.2 or 1.3 records those bits as off, which admission reads as missing.
    if api_version >= vk::API_VERSION_1_2 {
        query = query.push(&mut features_12);
    }
    if api_version >= vk::API_VERSION_1_3 {
        query = query.push(&mut features_13);
    }
    if has("VK_KHR_acceleration_structure") {
        query = query.push(&mut acceleration);
    }
    if has("VK_KHR_ray_query") {
        query = query.push(&mut ray_query);
    }
    if has("VK_KHR_ray_tracing_pipeline") {
        query = query.push(&mut ray_pipeline);
    }
    if has("VK_KHR_ray_tracing_maintenance1") {
        query = query.push(&mut ray_maintenance);
    }
    if has("VK_KHR_ray_tracing_position_fetch") {
        query = query.push(&mut position_fetch);
    }
    if has("VK_EXT_mesh_shader") {
        query = query.push(&mut mesh);
    }
    if has("VK_KHR_fragment_shading_rate") {
        query = query.push(&mut shading_rate);
    }
    if has("VK_KHR_maintenance5") {
        query = query.push(&mut maintenance5);
    }
    if has("VK_KHR_maintenance6") {
        query = query.push(&mut maintenance6);
    }
    if has("VK_KHR_dynamic_rendering_local_read") {
        query = query.push(&mut local_read);
    }
    if index_u8 {
        // The KHR and EXT structures share one sType (the EXT was promoted unchanged).
        query = query.push(&mut index_type_u8);
    }
    if has("VK_EXT_host_image_copy") {
        query = query.push(&mut host_image_copy);
    }
    if has("VK_EXT_ray_tracing_invocation_reorder") {
        query = query.push(&mut reorder);
    }
    if has("VK_KHR_present_id") {
        query = query.push(&mut present_id);
    }
    if has("VK_AMD_anti_lag") {
        query = query.push(&mut anti_lag);
    }
    // SAFETY: the device belongs to this instance; the chain holds core 1.2/1.3 structures
    // only where the device reports that version (the instance asks for 1.3), and
    // structures of advertised extensions, each in the chain once and live for the call.
    unsafe { instance.get_physical_device_features2(physical, &mut query) };
    let base = query.features;
    let f12 = &features_12;
    let f13 = &features_13;
    let mut recorded: Vec<(&str, vk::Bool32)> = vec![
        (
            "shaderStorageImageWriteWithoutFormat",
            base.shader_storage_image_write_without_format,
        ),
        ("independentBlend", base.independent_blend),
        ("dynamicRendering", f13.dynamic_rendering),
        ("synchronization2", f13.synchronization2),
        ("maintenance4", f13.maintenance4),
        ("bufferDeviceAddress", f12.buffer_device_address),
        ("descriptorIndexing", f12.descriptor_indexing),
        ("runtimeDescriptorArray", f12.runtime_descriptor_array),
        ("scalarBlockLayout", f12.scalar_block_layout),
        ("timelineSemaphore", f12.timeline_semaphore),
        ("drawIndirectCount", f12.draw_indirect_count),
        ("hostQueryReset", f12.host_query_reset),
        (
            "shaderSampledImageArrayNonUniformIndexing",
            f12.shader_sampled_image_array_non_uniform_indexing,
        ),
        (
            "shaderStorageImageArrayNonUniformIndexing",
            f12.shader_storage_image_array_non_uniform_indexing,
        ),
        (
            "descriptorBindingSampledImageUpdateAfterBind",
            f12.descriptor_binding_sampled_image_update_after_bind,
        ),
        (
            "descriptorBindingStorageImageUpdateAfterBind",
            f12.descriptor_binding_storage_image_update_after_bind,
        ),
        (
            "descriptorBindingPartiallyBound",
            f12.descriptor_binding_partially_bound,
        ),
        (
            "descriptorBindingVariableDescriptorCount",
            f12.descriptor_binding_variable_descriptor_count,
        ),
        (
            "descriptorBindingUpdateUnusedWhilePending",
            f12.descriptor_binding_update_unused_while_pending,
        ),
    ];
    let mut optional = |present: bool, bits: &[(&'static str, vk::Bool32)]| {
        if present {
            recorded.extend_from_slice(bits);
        }
    };
    optional(
        has("VK_KHR_acceleration_structure"),
        &[("accelerationStructure", acceleration.acceleration_structure)],
    );
    optional(
        has("VK_KHR_ray_query"),
        &[("rayQuery", ray_query.ray_query)],
    );
    optional(
        has("VK_KHR_ray_tracing_pipeline"),
        &[("rayTracingPipeline", ray_pipeline.ray_tracing_pipeline)],
    );
    optional(
        has("VK_EXT_ray_tracing_invocation_reorder"),
        &[(
            "rayTracingInvocationReorder",
            reorder.ray_tracing_invocation_reorder,
        )],
    );
    optional(
        has("VK_KHR_present_id"),
        &[("presentId", present_id.present_id)],
    );
    optional(has("VK_AMD_anti_lag"), &[("antiLag", anti_lag.anti_lag)]);
    optional(
        has("VK_KHR_ray_tracing_maintenance1"),
        &[(
            "rayTracingMaintenance1",
            ray_maintenance.ray_tracing_maintenance1,
        )],
    );
    optional(
        has("VK_KHR_ray_tracing_position_fetch"),
        &[(
            "rayTracingPositionFetch",
            position_fetch.ray_tracing_position_fetch,
        )],
    );
    optional(
        has("VK_EXT_mesh_shader"),
        &[
            ("meshShader", mesh.mesh_shader),
            ("taskShader", mesh.task_shader),
        ],
    );
    optional(
        has("VK_KHR_fragment_shading_rate"),
        &[
            (
                "pipelineFragmentShadingRate",
                shading_rate.pipeline_fragment_shading_rate,
            ),
            (
                "primitiveFragmentShadingRate",
                shading_rate.primitive_fragment_shading_rate,
            ),
            (
                "attachmentFragmentShadingRate",
                shading_rate.attachment_fragment_shading_rate,
            ),
        ],
    );
    // VK_KHR_push_descriptor has no feature structure: advertising it is support.
    optional(
        has("VK_KHR_push_descriptor"),
        &[("pushDescriptor", vk::TRUE)],
    );
    optional(
        has("VK_KHR_maintenance5"),
        &[("maintenance5", maintenance5.maintenance5)],
    );
    optional(
        has("VK_KHR_maintenance6"),
        &[("maintenance6", maintenance6.maintenance6)],
    );
    optional(
        has("VK_KHR_dynamic_rendering_local_read"),
        &[(
            "dynamicRenderingLocalRead",
            local_read.dynamic_rendering_local_read,
        )],
    );
    optional(
        index_u8,
        &[("indexTypeUint8", index_type_u8.index_type_uint8)],
    );
    optional(
        has("VK_EXT_host_image_copy"),
        &[("hostImageCopy", host_image_copy.host_image_copy)],
    );
    recorded
        .into_iter()
        .map(|(name, value)| (String::from(name), value == vk::TRUE))
        .collect()
}

/// The update-after-bind limits the bindless set needs and the base limits worth logging.
fn record_limits(
    base: &vk::PhysicalDeviceLimits,
    p: &vk::PhysicalDeviceVulkan12Properties<'_>,
) -> BTreeMap<String, u64> {
    [
        (
            "maxPerStageDescriptorUpdateAfterBindSampledImages",
            p.max_per_stage_descriptor_update_after_bind_sampled_images,
        ),
        (
            "maxDescriptorSetUpdateAfterBindSampledImages",
            p.max_descriptor_set_update_after_bind_sampled_images,
        ),
        (
            "maxPerStageDescriptorUpdateAfterBindSamplers",
            p.max_per_stage_descriptor_update_after_bind_samplers,
        ),
        (
            "maxPerStageDescriptorUpdateAfterBindStorageImages",
            p.max_per_stage_descriptor_update_after_bind_storage_images,
        ),
        (
            "maxUpdateAfterBindDescriptorsInAllPools",
            p.max_update_after_bind_descriptors_in_all_pools,
        ),
        (
            "maxPerStageUpdateAfterBindResources",
            p.max_per_stage_update_after_bind_resources,
        ),
        ("maxPushConstantsSize", base.max_push_constants_size),
        ("maxColorAttachments", base.max_color_attachments),
        (
            "maxFragmentOutputAttachments",
            base.max_fragment_output_attachments,
        ),
        (
            "maxFragmentCombinedOutputResources",
            base.max_fragment_combined_output_resources,
        ),
        ("maxBoundDescriptorSets", base.max_bound_descriptor_sets),
        (
            "maxPerStageDescriptorSamplers",
            base.max_per_stage_descriptor_samplers,
        ),
    ]
    .into_iter()
    .map(|(name, value)| (String::from(name), u64::from(value)))
    .collect()
}

#[cfg(test)]
mod tests;
