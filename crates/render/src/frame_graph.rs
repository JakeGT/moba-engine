//! The engine's frame, declared as a render graph, and barrier recording.
//!
//! ```text
//! portraits     (R7b; frames that update them) hero models → portrait atlas (persistent)
//! minimap       (R7b; frames that bake it) top-down map → minimap image (persistent)
//! fow-raster    (R3a; fog of war on) vision sources → fixed-size R8 transient (MAX blend)
//! fow-resolve   (R3a) raw (sampled, blurred) + the other frame's image (history) → this
//!               frame's fog-of-war image (persistent, ping-pong by frame in flight)
//! vfx_simulate  compute: spawn, age, integrate and compact particles; indirect args (R5)
//! shadow        (R2a; when shadows are on) the main view's units + skinned, depth only →
//!               shadow map (fixed size)
//! prepass       (R6 AO, R2b contact shadows or screen outlines) main view depth → depth
//!               (msaa-depth + resolve with MSAA), plus R2b's R8_UINT outline IDs
//!               (msaa-outline-ids resolved likewise) with screen outlines
//! contact_shadows (R2b) depth (sampled) → R8 contact visibility (render resolution)
//! ao            depth → ao-raw (half render resolution): GTAO
//! ao_blur       ao-raw → ao: 4×4 depth-aware blur
//! scene         terrain + units + skinned + sky (+ R2b's hull outlines) → hdr (colour,
//!               cleared; msaa-hdr resolved into it with MSAA) + depth (sampled-capable;
//!               loaded after the prepass), sampling the shadow map, this frame's fog-of-war
//!               image, ao and the contact visibility
//! vfx_draw      particles → hdr (+ OIT accum/reveal), single-sampled after any MSAA
//!               resolve; depth read-only + sampled (soft particles)
//! vfx_resolve   OIT accum/reveal (sampled) blended over hdr (Medium and up)
//! bloom_d*      hdr → bloom0 … bloom4 (R6, bloom or auto exposure): 13-tap down-samples
//! exposure      bloom4 → exposure (1×1, persistent; auto exposure)
//! bloom_u*      bloom[i+1] tent-filtered and added into bloom[i] (bloom)
//! composite     hdr + bloom0 + exposure → AgX + grading + screen effects → ldr | swapchain
//! outline       (R2b; screen outlines) outline IDs (sampled) → silhouettes blended over the
//!               composite's output, before FXAA and the upscale
//! fxaa          ldr → ldr2 | swapchain (FXAA)
//! upscale       ldr | ldr2 → swapchain (render scale < 1 or dynamic resolution)
//! ui            HUD batch blended over the swapchain (not tonemapped); samples both
//!               offscreen images and (the minimap mask) the fog-of-war image
//! capture       swapchain → readback buffer (only in the capture variant)
//! ```
//!
//! The offscreen images are imports owned by the renderer, not transients: their contents
//! survive the frame, they start and end every frame in `SHADER_READ_ONLY_OPTIMAL`, and their
//! passes appear only in the variants for frames that re-render them. Every combination
//! (capture × which offscreen passes run) is compiled once at renderer creation, and all of
//! them share the same transient slots. Per frame the renderer picks a variant, fills a
//! fixed-size [`Bindings`] table with this frame's handles and records the precompiled
//! barriers, so nothing here allocates on the frame path. The post chain (ADR-0015) is the
//! same in every variant of one renderer ([`FrameSpec`]).
//!
//! The offscreen passes never sample the shadow map: their views bind a lighting set whose
//! shadow binding is a persistent 1×1 default image, and their camera uniform turns shadows
//! off (ADR-0011), so the shadow transient is only read by the scene, after the shadow pass.
//! They also bind a white AO image.

use core::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use crate::device::DeviceContext;
use crate::graph::{
    Access, Barrier, CompiledGraph, GraphBuilder, GraphError, ImageDesc, ImageSize, ImportState,
    MAX_BARRIERS, MAX_RESOURCES, ResourceId,
};
use crate::ui::UI_IMAGE_SLOTS;

/// Pass name of each offscreen image's pass, by [`UiImage`](crate::UiImage) slot (also its
/// GPU-timing name).
pub(crate) const OFFSCREEN_PASS_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits", "minimap"];
const OFFSCREEN_DEPTH_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits-depth", "minimap-depth"];
const OFFSCREEN_BONE_NAMES: [&str; UI_IMAGE_SLOTS] = ["portraits-bones", "minimap-bones"];
/// Number of offscreen-pass combinations (one bit per slot).
pub(crate) const OFFSCREEN_VARIANTS: usize = 1 << UI_IMAGE_SLOTS;

/// Bloom mip levels: ½ … 1/32 of the render resolution (ADR-0015 §3).
pub(crate) const BLOOM_LEVELS: usize = 5;
/// Down-sample pass names (`bloom_d{i}` writes mip `i`), also their GPU-timing names.
pub(crate) const BLOOM_DOWN_NAMES: [&str; BLOOM_LEVELS] =
    ["bloom_d0", "bloom_d1", "bloom_d2", "bloom_d3", "bloom_d4"];
/// Up-sample pass names (`bloom_u{i}` adds mip `i + 1` into mip `i`).
pub(crate) const BLOOM_UP_NAMES: [&str; BLOOM_LEVELS - 1] =
    ["bloom_u0", "bloom_u1", "bloom_u2", "bloom_u3"];
const BLOOM_MIP_NAMES: [&str; BLOOM_LEVELS] = ["bloom0", "bloom1", "bloom2", "bloom3", "bloom4"];
/// The tonemapped, graded image at render resolution (display-encoded, luma in alpha).
pub(crate) const LDR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// GTAO output: occlusion and the pixel's view distance (for the depth-aware blur).
pub(crate) const AO_RAW_FORMAT: vk::Format = vk::Format::R16G16_SFLOAT;
/// The blurred occlusion the scene samples.
pub(crate) const AO_FORMAT: vk::Format = vk::Format::R8_UNORM;
/// R2b's outline ID target: a mandatory colour-attachment and sampled format (ADR-0016).
pub(crate) const OUTLINE_ID_FORMAT: vk::Format = vk::Format::R8_UINT;
/// R2b's contact-shadow target.
pub(crate) const CONTACT_FORMAT: vk::Format = vk::Format::R8_UNORM;

/// What a compiled pass records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassKind {
    FogRaster,
    FogResolve,
    Shadow,
    VfxSimulate,
    Prepass,
    /// Screen-space contact shadows from the prepass depth (R2b).
    ContactShadows,
    Ao,
    AoBlur,
    Scene,
    VfxDraw,
    VfxResolve,
    /// Writes bloom mip `i`.
    BloomDown(u8),
    /// Exposure metering and adaptation.
    Exposure,
    /// Adds bloom mip `i + 1` into mip `i`.
    BloomUp(u8),
    Composite,
    /// Screen-space outlines over the tonemapped image (R2b).
    Outline,
    Fxaa,
    Upscale,
    Ui,
    Capture,
    /// Renders the offscreen image of this [`UiImage`](crate::UiImage) slot.
    Offscreen(u8),
}

/// Resource ids of one offscreen image: colour (sampled by the UI), depth and its bones.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OffscreenResources {
    pub(crate) color: ResourceId,
    pub(crate) depth: ResourceId,
    pub(crate) bones: ResourceId,
}

/// Whether and how the frame draws GPU particles (ADR-0013).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VfxMode {
    /// No VFX passes (disabled, or no compute on the graphics queue).
    Off,
    /// Particles blend straight into the HDR target (alpha unsorted; Low).
    Unsorted,
    /// Alpha particles go through weighted blended OIT and a resolve pass.
    Oit,
}

/// Resource ids of the VFX passes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VfxResources {
    pub(crate) particles: ResourceId,
    pub(crate) alive: ResourceId,
    pub(crate) args: ResourceId,
    pub(crate) spawns: ResourceId,
    pub(crate) stats: ResourceId,
    pub(crate) accum: Option<ResourceId>,
    pub(crate) reveal: Option<ResourceId>,
}

/// The multisampled scene attachments (MSAA 4×), resolved into `hdr` and `depth`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MsaaResources {
    pub(crate) color: ResourceId,
    pub(crate) depth: ResourceId,
}

/// R2b's outline ID images (ADR-0016).
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutlineIdResources {
    /// The single-sample IDs the outline pass samples (the prepass's MSAA IDs resolve into
    /// it, sample 0).
    pub(crate) ids: ResourceId,
    /// The multisampled attachment; `None` without MSAA.
    pub(crate) msaa: Option<ResourceId>,
}

/// The half-resolution AO images.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AoResources {
    pub(crate) raw: ResourceId,
    pub(crate) blurred: ResourceId,
}

/// Resource ids of the fog-of-war passes (R3a).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FogResources {
    /// This frame's raw raster (a fixed-size transient).
    pub(crate) raw: ResourceId,
    /// The persistent image this frame writes (frame slot `f`: image `f`).
    pub(crate) current: ResourceId,
    /// The persistent image the previous frame wrote (image `1 - f`).
    pub(crate) history: ResourceId,
}

/// Resource ids of the engine frame graph.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameResources {
    pub(crate) swapchain: ResourceId,
    pub(crate) hdr: ResourceId,
    pub(crate) depth: ResourceId,
    pub(crate) bones: ResourceId,
    pub(crate) capture: Option<ResourceId>,
    pub(crate) offscreen: [OffscreenResources; UI_IMAGE_SLOTS],
    /// The directional shadow map; `None` when shadows are off.
    pub(crate) shadow: Option<ResourceId>,
    pub(crate) vfx: Option<VfxResources>,
    /// MSAA attachments; `None` without MSAA.
    pub(crate) msaa: Option<MsaaResources>,
    /// AO images; `None` without AO (and so without the prepass).
    pub(crate) ao: Option<AoResources>,
    /// Bloom mips (½ … 1/32); `None` without bloom or auto exposure.
    pub(crate) bloom: Option<[ResourceId; BLOOM_LEVELS]>,
    /// The persistent 1×1 exposure image; `None` without auto exposure.
    pub(crate) exposure: Option<ResourceId>,
    /// The composite's output when a later post pass reads it.
    pub(crate) ldr: Option<ResourceId>,
    /// FXAA's output when the upscale reads it.
    pub(crate) ldr2: Option<ResourceId>,
    /// The fog-of-war resources; `None` unless the fog of war uses the texture.
    pub(crate) fog: Option<FogResources>,
    /// R2b's outline IDs; `None` without screen-space outlines.
    pub(crate) outline_ids: Option<OutlineIdResources>,
    /// R2b's contact-shadow visibility; `None` without contact shadows.
    pub(crate) contact: Option<ResourceId>,
}

/// The shadow map a frame renders, when shadows are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShadowSpec {
    pub(crate) format: vk::Format,
    pub(crate) size: u32,
}

/// Formats and the post-processing topology every variant of one renderer shares
/// (ADR-0015).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent feature switches.
pub(crate) struct FrameSpec {
    pub(crate) hdr_format: vk::Format,
    pub(crate) depth_format: vk::Format,
    pub(crate) depth_aspect: vk::ImageAspectFlags,
    /// The shadow pass and map, when shadows are on.
    pub(crate) shadow: Option<ShadowSpec>,
    /// The fog-of-war texture's edge in texels (R3a, ADR-0014), adding the `fow-raster` and
    /// `fow-resolve` passes; `None` unless the fog of war uses the texture.
    pub(crate) fog: Option<u32>,
    /// The GPU particle passes (ADR-0013).
    pub(crate) vfx: VfxMode,
    /// MSAA 4× scene attachments.
    pub(crate) msaa: bool,
    /// The main view's depth prepass on its own; [`Self::ambient_occlusion`],
    /// [`Self::outline_ids`] and [`Self::contact_shadows`] imply it, see
    /// [`Self::has_prepass`].
    pub(crate) prepass: bool,
    /// R2b's screen-space outlines: the prepass writes the ID target, the outline pass reads
    /// it after the composite (ADR-0016).
    pub(crate) outline_ids: bool,
    /// R2b's contact shadows: a pass between the prepass and the scene.
    pub(crate) contact_shadows: bool,
    /// The AO passes (and with them the depth prepass).
    pub(crate) ambient_occlusion: bool,
    /// The bloom up-chain (and the composite's bloom input).
    pub(crate) bloom: bool,
    /// The exposure pass.
    pub(crate) auto_exposure: bool,
    /// Bloom mip format (`B10G11R11_UFLOAT_PACK32` where supported).
    pub(crate) bloom_format: vk::Format,
    /// The 1×1 adapted-exposure format (`R32_SFLOAT` where blendable, else `R16_SFLOAT`).
    pub(crate) exposure_format: vk::Format,
    /// The FXAA pass.
    pub(crate) fxaa: bool,
    /// The upscale pass (render scale below 1 or dynamic resolution).
    pub(crate) upscale: bool,
}

impl FrameSpec {
    /// The R2a frame with no post passes (tests and the simplest configuration).
    #[cfg(test)]
    pub(crate) const fn plain(
        hdr_format: vk::Format,
        depth_format: vk::Format,
        depth_aspect: vk::ImageAspectFlags,
    ) -> Self {
        Self {
            hdr_format,
            depth_format,
            depth_aspect,
            shadow: None,
            fog: None,
            vfx: VfxMode::Off,
            msaa: false,
            prepass: false,
            outline_ids: false,
            contact_shadows: false,
            ambient_occlusion: false,
            bloom: false,
            auto_exposure: false,
            bloom_format: vk::Format::R16G16B16A16_SFLOAT,
            exposure_format: vk::Format::R32_SFLOAT,
            fxaa: false,
            upscale: false,
        }
    }

    /// Whether the depth prepass runs: asked for, or needed by AO, contact shadows or the
    /// outline IDs.
    pub(crate) const fn has_prepass(&self) -> bool {
        self.prepass || self.ambient_occlusion || self.outline_ids || self.contact_shadows
    }

    /// Whether the bloom down-chain runs (bloom, or metering for auto exposure).
    pub(crate) const fn bloom_chain(&self) -> bool {
        self.bloom || self.auto_exposure
    }
}

/// A compiled frame variant and its resource ids.
pub(crate) struct FramePlan {
    pub(crate) graph: CompiledGraph<PassKind>,
    pub(crate) resources: FrameResources,
    /// Index of the first pass that touches the swapchain image. Passes before it go in a
    /// first batch that does not wait on the acquire semaphore; the rest wait on it.
    pub(crate) acquire_split: usize,
}

const fn image(format: vk::Format, size: ImageSize) -> ImageDesc {
    ImageDesc {
        format,
        aspect: vk::ImageAspectFlags::COLOR,
        size,
        samples: vk::SampleCountFlags::TYPE_1,
        extra_usage: vk::ImageUsageFlags::empty(),
    }
}

/// Builds and compiles the frame graph for `spec`. Bit `s` of `offscreen` adds the pass that
/// renders offscreen image `s` this frame; `capture` adds the readback. Every variant of one
/// renderer passes the same `spec` (shadows, the fog of war's texture edge, the post chain),
/// so they share transients.
///
/// # Errors
/// Only if the declaration below is inconsistent (covered by unit tests).
#[allow(clippy::too_many_lines)] // One declaration of the whole frame.
pub(crate) fn build(
    spec: &FrameSpec,
    capture: bool,
    offscreen: usize,
) -> Result<FramePlan, GraphError> {
    let render = ImageSize::Render { shift: 0 };
    let half = ImageSize::Render { shift: 1 };
    let mut graph = GraphBuilder::new();
    // The acquire semaphore is waited on at COLOR_ATTACHMENT_OUTPUT; the last post pass
    // writes every pixel, so the previous contents are discarded.
    let swapchain = graph.import_image(
        "swapchain",
        ImportState::discard(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT),
        Some(Access::Present),
    );
    // The frame fence covers write-after-read against the previous use of this frame's
    // bone buffer; the host writes it before submission.
    let bones = graph.import_buffer("bones", ImportState::host_written(), None);
    let hdr = graph.transient_image("hdr", image(spec.hdr_format, render));
    // Sampled-capable so later passes (soft particles, decals, SSAO) can read it.
    let depth = graph.transient_image(
        "depth",
        ImageDesc {
            format: spec.depth_format,
            aspect: spec.depth_aspect,
            size: render,
            samples: vk::SampleCountFlags::TYPE_1,
            extra_usage: vk::ImageUsageFlags::SAMPLED,
        },
    );
    let msaa = spec.msaa.then(|| MsaaResources {
        // Resolved at the end of the scene pass and never stored (DONT_CARE): transient.
        color: graph.transient_image(
            "msaa-hdr",
            ImageDesc {
                samples: vk::SampleCountFlags::TYPE_4,
                extra_usage: vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
                ..image(spec.hdr_format, render)
            },
        ),
        depth: graph.transient_image(
            "msaa-depth",
            ImageDesc {
                format: spec.depth_format,
                aspect: spec.depth_aspect,
                size: render,
                samples: vk::SampleCountFlags::TYPE_4,
                extra_usage: vk::ImageUsageFlags::empty(),
            },
        ),
    });
    // R2b (ADR-0016): render-resolution targets of their own formats, so they never alias
    // the HDR or LDR images.
    let outline_ids = spec.outline_ids.then(|| OutlineIdResources {
        ids: graph.transient_image("outline-ids", image(OUTLINE_ID_FORMAT, render)),
        msaa: spec.msaa.then(|| {
            graph.transient_image(
                "msaa-outline-ids",
                ImageDesc {
                    samples: vk::SampleCountFlags::TYPE_4,
                    ..image(OUTLINE_ID_FORMAT, render)
                },
            )
        }),
    });
    let contact = spec
        .contact_shadows
        .then(|| graph.transient_image("contact-shadows", image(CONTACT_FORMAT, render)));
    let ao = spec.ambient_occlusion.then(|| AoResources {
        raw: graph.transient_image("ao-raw", image(AO_RAW_FORMAT, half)),
        blurred: graph.transient_image("ao", image(AO_FORMAT, half)),
    });
    // Depth only and fixed-size, so it never aliases the window-sized targets. Written by
    // the shadow pass, sampled (depth compare) by the scene's fragment shaders.
    let shadow_map = spec.shadow.map(|shadow| {
        graph.transient_image(
            "shadow-map",
            ImageDesc {
                format: shadow.format,
                aspect: vk::ImageAspectFlags::DEPTH,
                size: ImageSize::Fixed {
                    width: shadow.size,
                    height: shadow.size,
                },
                samples: vk::SampleCountFlags::TYPE_1,
                extra_usage: vk::ImageUsageFlags::empty(),
            },
        )
    });
    // R3a fog of war: the raw raster is fixed-size (never aliases swapchain-sized targets);
    // the two persistent images start and end every frame sampled, like R7b's.
    let fog_resources = spec.fog.map(|resolution| FogResources {
        raw: graph.transient_image(
            "fow-raw",
            image(
                crate::passes::fog::FOG_FORMAT,
                ImageSize::Fixed {
                    width: resolution,
                    height: resolution,
                },
            ),
        ),
        current: graph.import_image(
            "fow-current",
            ImportState::persistent(
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ),
            Some(Access::SampledFragment),
        ),
        history: graph.import_image(
            "fow-history",
            ImportState::persistent(
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ),
            Some(Access::SampledFragment),
        ),
    });
    let bloom: Option<[ResourceId; BLOOM_LEVELS]> = spec.bloom_chain().then(|| {
        core::array::from_fn(|level| {
            let shift = u8::try_from(level + 1).unwrap_or(u8::MAX);
            graph.transient_image(
                BLOOM_MIP_NAMES[level],
                image(spec.bloom_format, ImageSize::Render { shift }),
            )
        })
    });
    // Kept across frames (the adapted value) and back in SHADER_READ_ONLY_OPTIMAL at the end
    // of every frame, like R7b's offscreen images.
    let exposure = spec.auto_exposure.then(|| {
        graph.import_image(
            "exposure",
            ImportState::persistent(
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ),
            Some(Access::SampledFragment),
        )
    });
    let vfx = (spec.vfx != VfxMode::Off).then(|| {
        // The particle state, lists and arguments persist across frames on this queue: a
        // frame's first barrier orders against every earlier frame's compute, vertex,
        // indirect and copy work on them. The spawn upload is host-written per frame slot
        // (its fence covers the previous use), and the stats copy is read by the host after
        // the frame fence.
        let persistent = ImportState::persistent_buffer(
            vk::PipelineStageFlags2::COMPUTE_SHADER
                | vk::PipelineStageFlags2::VERTEX_SHADER
                | vk::PipelineStageFlags2::DRAW_INDIRECT
                | vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::SHADER_STORAGE_WRITE,
        );
        let oit = spec.vfx == VfxMode::Oit;
        let (accum_format, reveal_format) = crate::passes::vfx::oit_formats();
        // Render-sized like the HDR target they resolve into (R6 resolution scaling).
        VfxResources {
            particles: graph.import_buffer("vfx-particles", persistent, None),
            alive: graph.import_buffer("vfx-alive", persistent, None),
            args: graph.import_buffer("vfx-args", persistent, None),
            spawns: graph.import_buffer("vfx-spawns", ImportState::host_written(), None),
            stats: graph.import_buffer("vfx-stats", ImportState::idle(), Some(Access::HostRead)),
            accum: oit.then(|| graph.transient_image("vfx-accum", image(accum_format, render))),
            reveal: oit.then(|| graph.transient_image("vfx-reveal", image(reveal_format, render))),
        }
    });
    let ldr = (spec.fxaa || spec.upscale)
        .then(|| graph.transient_image("ldr", image(LDR_FORMAT, render)));
    let ldr2 = (spec.fxaa && spec.upscale)
        .then(|| graph.transient_image("ldr2", image(LDR_FORMAT, render)));
    let capture_buffer = capture.then(|| {
        // Every capture waits for the device to idle before reading the buffer back, so no
        // earlier submission still touches it when the next capture frame starts.
        graph.import_buffer("capture", ImportState::idle(), Some(Access::HostRead))
    });
    // R7b offscreen images. Colour: kept across frames, sampled by the previous frame's UI,
    // and back in SHADER_READ_ONLY_OPTIMAL at the end of every frame (the UI read below
    // transitions it after a write). Depth: discarded per use, but last written by an
    // earlier frame's pass. Bones: written by the host, like the scene's.
    let offscreen_resources: [OffscreenResources; UI_IMAGE_SLOTS] =
        core::array::from_fn(|slot| OffscreenResources {
            color: graph.import_image(
                OFFSCREEN_PASS_NAMES[slot],
                ImportState::persistent(
                    vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
                Some(Access::SampledFragment),
            ),
            depth: graph.import_image(
                OFFSCREEN_DEPTH_NAMES[slot],
                ImportState::rewritten(
                    vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
                None,
            ),
            bones: graph.import_buffer(
                OFFSCREEN_BONE_NAMES[slot],
                ImportState::host_written(),
                None,
            ),
        });

    // Offscreen passes first: they do not touch the swapchain, so they go in the batch that
    // is submitted ahead of the acquire wait, with the scene.
    for (slot, resources) in offscreen_resources.iter().enumerate() {
        if offscreen & (1 << slot) == 0 {
            continue;
        }
        let _ = graph
            .pass(
                OFFSCREEN_PASS_NAMES[slot],
                PassKind::Offscreen(u8::try_from(slot).unwrap_or(u8::MAX)),
            )
            .access(resources.color, Access::ColorWrite)
            .access(resources.depth, Access::DepthWrite)
            .access(resources.bones, Access::VertexStorageRead);
    }
    if let Some(fog) = fog_resources {
        let _ = graph
            .pass("fow-raster", PassKind::FogRaster)
            .access(fog.raw, Access::ColorWrite);
        let _ = graph
            .pass("fow-resolve", PassKind::FogResolve)
            .access(fog.raw, Access::SampledFragment)
            .access(fog.history, Access::SampledFragment)
            .access(fog.current, Access::ColorWrite);
    }
    if let Some(vfx) = vfx {
        let _ = graph
            .pass("vfx_simulate", PassKind::VfxSimulate)
            .access(vfx.spawns, Access::ComputeStorageRead)
            .access(vfx.particles, Access::ComputeStorageReadWrite)
            .access(vfx.alive, Access::ComputeStorageReadWrite)
            .access(vfx.args, Access::ComputeStorageReadWrite)
            .access(vfx.stats, Access::TransferWrite);
    }
    if let Some(shadow_map) = shadow_map {
        let _ = graph
            .pass("shadow", PassKind::Shadow)
            .access(shadow_map, Access::DepthWrite)
            .access(bones, Access::VertexStorageRead);
    }
    let prepass = spec.has_prepass();
    if prepass {
        // The prepass writes the depth the AO reads and the scene tests against; with MSAA it
        // renders the multisampled depth and resolves sample 0 into `depth`. (An extra
        // colour output, such as R2b's outline IDs, is one more access on this pass and a
        // colour attachment in `ScenePass::record_prepass`.)
        let pass = graph
            .pass("prepass", PassKind::Prepass)
            .access(bones, Access::VertexStorageRead);
        let pass = match msaa {
            Some(msaa) => pass
                .access(msaa.depth, Access::DepthWriteResolveSource)
                .access(depth, Access::DepthResolveWrite),
            None => pass.access(depth, Access::DepthWrite),
        };
        // R2b's outline IDs: written with the depth (and resolved, sample 0, with MSAA).
        let _ = match outline_ids {
            Some(OutlineIdResources {
                ids,
                msaa: Some(msaa_ids),
            }) => pass
                .access(msaa_ids, Access::ColorWrite)
                .access(ids, Access::ColorWrite),
            Some(OutlineIdResources { ids, msaa: None }) => pass.access(ids, Access::ColorWrite),
            None => pass,
        };
    }
    if let Some(contact) = contact {
        let _ = graph
            .pass("contact_shadows", PassKind::ContactShadows)
            .access(depth, Access::SampledFragment)
            .access(contact, Access::ColorWrite);
    }
    if let Some(ao) = ao {
        let _ = graph
            .pass("ao", PassKind::Ao)
            .access(depth, Access::SampledFragment)
            .access(ao.raw, Access::ColorWrite);
        let _ = graph
            .pass("ao_blur", PassKind::AoBlur)
            .access(ao.raw, Access::SampledFragment)
            .access(ao.blurred, Access::ColorWrite);
    }
    let mut scene = graph
        .pass("scene", PassKind::Scene)
        .access(hdr, Access::ColorWrite)
        .access(bones, Access::VertexStorageRead);
    let scene_depth = if prepass {
        Access::DepthReadWrite
    } else {
        Access::DepthWrite
    };
    scene = match msaa {
        // Colour resolves into `hdr`; without the prepass the depth resolves too (with it,
        // `depth` already holds the prepass's resolved depth, which the scene reproduces).
        Some(msaa) => {
            let scene = scene.access(msaa.color, Access::ColorWrite);
            if prepass {
                scene.access(msaa.depth, scene_depth)
            } else {
                scene
                    .access(msaa.depth, Access::DepthWriteResolveSource)
                    .access(depth, Access::DepthResolveWrite)
            }
        }
        None => scene.access(depth, scene_depth),
    };
    if let Some(shadow_map) = shadow_map {
        scene = scene.access(shadow_map, Access::SampledFragment);
    }
    if let Some(fog) = fog_resources {
        scene = scene.access(fog.current, Access::SampledFragment);
    }
    if let Some(ao) = ao {
        scene = scene.access(ao.blurred, Access::SampledFragment);
    }
    if let Some(contact) = contact {
        scene = scene.access(contact, Access::SampledFragment);
    }
    let _ = scene;
    // Particles draw single-sampled into the resolved HDR target and test against the
    // single-sampled depth (resolved, or the prepass's), before every post pass.
    if let Some(vfx) = vfx {
        let mut pass = graph
            .pass("vfx_draw", PassKind::VfxDraw)
            .access(hdr, Access::ColorReadWrite)
            .access(depth, Access::DepthReadSampled)
            .access(vfx.particles, Access::VertexStorageRead)
            .access(vfx.alive, Access::VertexStorageRead)
            .access(vfx.args, Access::IndirectRead);
        if let (Some(accum), Some(reveal)) = (vfx.accum, vfx.reveal) {
            pass = pass
                .access(accum, Access::ColorWrite)
                .access(reveal, Access::ColorWrite);
        }
        let _ = pass;
        if let (Some(accum), Some(reveal)) = (vfx.accum, vfx.reveal) {
            let _ = graph
                .pass("vfx_resolve", PassKind::VfxResolve)
                .access(accum, Access::SampledFragment)
                .access(reveal, Access::SampledFragment)
                .access(hdr, Access::ColorReadWrite);
        }
    }
    if let Some(mips) = bloom {
        for (level, &mip) in mips.iter().enumerate() {
            let source = if level == 0 { hdr } else { mips[level - 1] };
            let _ = graph
                .pass(
                    BLOOM_DOWN_NAMES[level],
                    PassKind::BloomDown(u8::try_from(level).unwrap_or(u8::MAX)),
                )
                .access(source, Access::SampledFragment)
                .access(mip, Access::ColorWrite);
        }
        if let Some(exposure) = exposure {
            let _ = graph
                .pass("exposure", PassKind::Exposure)
                .access(mips[BLOOM_LEVELS - 1], Access::SampledFragment)
                .access(exposure, Access::ColorReadWrite);
        }
        if spec.bloom {
            for level in (0..BLOOM_LEVELS - 1).rev() {
                let _ = graph
                    .pass(
                        BLOOM_UP_NAMES[level],
                        PassKind::BloomUp(u8::try_from(level).unwrap_or(u8::MAX)),
                    )
                    .access(mips[level + 1], Access::SampledFragment)
                    .access(mips[level], Access::ColorReadWrite);
            }
        }
    }
    let mut composite = graph
        .pass("composite", PassKind::Composite)
        .access(hdr, Access::SampledFragment)
        .access(ldr.unwrap_or(swapchain), Access::ColorWrite);
    if let (Some(mips), true) = (bloom, spec.bloom) {
        composite = composite.access(mips[0], Access::SampledFragment);
    }
    if let Some(exposure) = exposure {
        composite = composite.access(exposure, Access::SampledFragment);
    }
    let _ = composite;
    // R2b: outlines on the tonemapped image at render resolution, so FXAA and the upscale
    // treat them like any other edge.
    if let Some(outline_ids) = outline_ids {
        let _ = graph
            .pass("outline", PassKind::Outline)
            .access(outline_ids.ids, Access::SampledFragment)
            .access(ldr.unwrap_or(swapchain), Access::ColorReadWrite);
    }
    if let (true, Some(ldr)) = (spec.fxaa, ldr) {
        let _ = graph
            .pass("fxaa", PassKind::Fxaa)
            .access(ldr, Access::SampledFragment)
            .access(ldr2.unwrap_or(swapchain), Access::ColorWrite);
    }
    if spec.upscale
        && let Some(source) = ldr2.or(ldr)
    {
        let _ = graph
            .pass("upscale", PassKind::Upscale)
            .access(source, Access::SampledFragment)
            .access(swapchain, Access::ColorWrite);
    }
    // The UI samples every offscreen image in every variant (its descriptors always name
    // them), so each read is ordered after this frame's write, if any.
    let mut ui = graph
        .pass("ui", PassKind::Ui)
        .access(swapchain, Access::ColorReadWrite);
    for resources in &offscreen_resources {
        ui = ui.access(resources.color, Access::SampledFragment);
    }
    if let Some(fog) = fog_resources {
        ui = ui.access(fog.current, Access::SampledFragment);
    }
    let _ = ui;
    if let Some(buffer) = capture_buffer {
        let _ = graph
            .pass("capture", PassKind::Capture)
            .access(swapchain, Access::TransferRead)
            .access(buffer, Access::TransferWrite);
    }
    let graph = graph.compile()?;
    let acquire_split = graph.first_use(swapchain);
    Ok(FramePlan {
        graph,
        acquire_split,
        resources: FrameResources {
            swapchain,
            hdr,
            depth,
            bones,
            capture: capture_buffer,
            offscreen: offscreen_resources,
            shadow: shadow_map,
            vfx,
            msaa,
            ao,
            bloom,
            exposure,
            ldr,
            ldr2,
            fog: fog_resources,
            outline_ids,
            contact,
        },
    })
}

/// The handle a resource resolves to in one frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum Binding {
    #[default]
    Unbound,
    Image {
        image: vk::Image,
        aspect: vk::ImageAspectFlags,
    },
    Buffer(vk::Buffer),
}

/// This frame's handles, indexed by [`ResourceId`].
pub(crate) struct Bindings(pub(crate) [Binding; MAX_RESOURCES]);

impl Bindings {
    pub(crate) fn new() -> Self {
        Self([Binding::Unbound; MAX_RESOURCES])
    }

    pub(crate) fn set(&mut self, resource: ResourceId, binding: Binding) {
        self.0[resource.index()] = binding;
    }
}

static BINDING_MISMATCH_LOGGED: AtomicBool = AtomicBool::new(false);

/// Records `barriers` as one `vkCmdPipelineBarrier2`, without allocating.
pub(crate) fn record_barriers(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    barriers: &[Barrier],
    bindings: &Bindings,
) {
    if barriers.is_empty() {
        return;
    }
    let mut images = [vk::ImageMemoryBarrier2::default(); MAX_BARRIERS];
    let mut buffers = [vk::BufferMemoryBarrier2::default(); MAX_BARRIERS];
    let (mut image_count, mut buffer_count) = (0, 0);
    // `compile` bounds barriers per pass by MAX_BARRIERS.
    for barrier in barriers.iter().take(MAX_BARRIERS) {
        match bindings.0[barrier.resource.index()] {
            Binding::Image { image, aspect } if barrier.image => {
                images[image_count] = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(barrier.src_stage)
                    .src_access_mask(barrier.src_access)
                    .dst_stage_mask(barrier.dst_stage)
                    .dst_access_mask(barrier.dst_access)
                    .old_layout(barrier.old_layout)
                    .new_layout(barrier.new_layout)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(aspect)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    );
                image_count += 1;
            }
            Binding::Buffer(buffer) if !barrier.image => {
                buffers[buffer_count] = vk::BufferMemoryBarrier2::default()
                    .src_stage_mask(barrier.src_stage)
                    .src_access_mask(barrier.src_access)
                    .dst_stage_mask(barrier.dst_stage)
                    .dst_access_mask(barrier.dst_access)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(buffer)
                    .offset(0)
                    .size(vk::WHOLE_SIZE);
                buffer_count += 1;
            }
            binding => {
                // A renderer bug: the barrier cannot be recorded. Fail loudly in debug builds
                // and log once in release (validation then reports the missing sync).
                debug_assert!(
                    false,
                    "resource {:?} bound as {binding:?} for barrier {barrier:?}",
                    barrier.resource
                );
                if !BINDING_MISMATCH_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[render] graph barrier dropped: resource {:?} bound as {binding:?}",
                        barrier.resource
                    );
                }
            }
        }
    }
    let dependency = vk::DependencyInfo::default()
        .image_memory_barriers(&images[..image_count])
        .buffer_memory_barriers(&buffers[..buffer_count]);
    device.cmd_pipeline_barrier2(command_buffer, &dependency);
}

#[cfg(test)]
mod tests {
    use super::{
        BLOOM_LEVELS, FrameSpec, OFFSCREEN_VARIANTS, PassKind, ShadowSpec, VfxMode, build,
    };
    use crate::graph::{ImageSize, ResourceId};
    use ash::vk;
    use vk::ImageLayout as L;

    fn plain() -> FrameSpec {
        FrameSpec::plain(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
        )
    }

    fn variant(capture: bool, offscreen: usize) -> super::FramePlan {
        build(&plain(), capture, offscreen).expect("the engine frame graph compiles")
    }

    fn shadowed_spec() -> FrameSpec {
        FrameSpec {
            shadow: Some(ShadowSpec {
                format: vk::Format::D32_SFLOAT,
                size: 2048,
            }),
            ..plain()
        }
    }

    fn shadowed(capture: bool, offscreen: usize) -> super::FramePlan {
        build(&shadowed_spec(), capture, offscreen).expect("the shadowed frame graph compiles")
    }

    /// Every post topology the renderer can build (2^9 switch combinations, the standalone
    /// prepass and R2b's outline IDs with contact shadows included) with each particle mode:
    /// off, unsorted (Low) and OIT. The shadowed half also runs R3a's fog-of-war passes, as
    /// every tier with shadows does.
    fn post_specs() -> Vec<FrameSpec> {
        (0..1536u32)
            .map(|bits| FrameSpec {
                vfx: match bits >> 9 {
                    0 => VfxMode::Off,
                    1 => VfxMode::Unsorted,
                    _ => VfxMode::Oit,
                },
                prepass: bits & 128 != 0,
                outline_ids: bits & 256 != 0,
                contact_shadows: bits & 256 != 0,
                msaa: bits & 1 != 0,
                ambient_occlusion: bits & 2 != 0,
                bloom: bits & 4 != 0,
                auto_exposure: bits & 8 != 0,
                fxaa: bits & 16 != 0,
                upscale: bits & 32 != 0,
                bloom_format: vk::Format::B10G11R11_UFLOAT_PACK32,
                ..if bits & 64 != 0 {
                    FrameSpec {
                        fog: Some(256),
                        ..shadowed_spec()
                    }
                } else {
                    plain()
                }
            })
            .collect()
    }

    fn kinds(plan: &super::FramePlan) -> Vec<PassKind> {
        plan.graph.passes().iter().map(|pass| pass.tag).collect()
    }

    #[test]
    fn shadow_pass_writes_depth_then_the_scene_samples_it() {
        let plain = shadowed(false, 0);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = shadowed(capture, offscreen);
                // Every variant (capture x offscreen) runs on the same transients.
                assert!(frame.graph.shares_transients_with(&plain.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let count = offscreen.count_ones() as usize;
                let passes = frame.graph.passes();
                assert_eq!(passes[count].tag, PassKind::Shadow);
                assert_eq!(passes[count + 1].tag, PassKind::Scene);
                // Offscreen, shadow and scene all go ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 2);
                let shadow = frame.resources.shadow.expect("shadow map");
                // Only the shadow and scene passes touch the map (never an offscreen view).
                for (index, pass) in passes.iter().enumerate() {
                    assert_eq!(pass.uses(shadow), index == count || index == count + 1);
                }
                assert!(passes[count].barriers.iter().any(|barrier| {
                    barrier.resource == shadow
                        && barrier.new_layout == L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
                }));
                // Scene: depth write -> fragment sampled read with the layout change; the
                // bone buffer needs no second barrier.
                let scene = &passes[count + 1].barriers;
                let read = scene
                    .iter()
                    .find(|barrier| barrier.resource == shadow)
                    .expect("shadow barrier before the scene");
                assert_eq!(
                    (read.old_layout, read.new_layout),
                    (
                        L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                        L::SHADER_READ_ONLY_OPTIMAL
                    )
                );
                assert_eq!(
                    read.src_access,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                );
                assert_eq!(read.dst_stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
                assert_eq!(read.dst_access, vk::AccessFlags2::SHADER_SAMPLED_READ);
                assert!(
                    !scene
                        .iter()
                        .any(|barrier| barrier.resource == frame.resources.bones)
                );
            }
        }
        // Its own slot: a fixed size never aliases the window-sized depth.
        assert_eq!(plain.graph.slots().len(), 3);
        let slot = plain.graph.slots()[plain
            .graph
            .slot_of(plain.resources.shadow.unwrap())
            .unwrap()];
        assert!(slot.usage.contains(
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
        ));
    }

    fn plan(capture: bool) -> super::FramePlan {
        variant(capture, 0)
    }

    #[test]
    fn engine_frame_runs_scene_composite_ui_then_presents() {
        let frame = plan(false);
        assert_eq!(
            kinds(&frame),
            [PassKind::Scene, PassKind::Composite, PassKind::Ui]
        );
        // The scene is submitted ahead of the acquire wait; the composite waits for it.
        assert_eq!(frame.acquire_split, 1);
        assert_eq!(plan(true).acquire_split, 1);
        assert_eq!(frame.graph.culled(), [] as [&str; 0]);
        // HDR colour and depth overlap in the scene pass, so they get separate slots.
        assert_eq!(frame.graph.slots().len(), 2);
        let depth = frame.graph.slots()[frame.graph.slot_of(frame.resources.depth).unwrap()];
        assert!(depth.usage.contains(vk::ImageUsageFlags::SAMPLED));
        assert_eq!(depth.desc.size, ImageSize::Render { shift: 0 });
        let scene = &frame.graph.passes()[0].barriers;
        assert_eq!(
            scene.len(),
            3,
            "hdr, depth and the host-written bone buffer"
        );
        let composite = &frame.graph.passes()[1].barriers;
        assert!(composite.iter().any(|barrier| {
            barrier.resource == frame.resources.hdr
                && barrier.new_layout == L::SHADER_READ_ONLY_OPTIMAL
        }));
        let present = frame.graph.final_barriers();
        assert_eq!(present.len(), 1);
        assert_eq!(present[0].resource, frame.resources.swapchain);
        assert_eq!(
            (present[0].old_layout, present[0].new_layout),
            (L::COLOR_ATTACHMENT_OPTIMAL, L::PRESENT_SRC_KHR)
        );
    }

    #[test]
    fn capture_variant_copies_after_ui_and_shares_transient_slots() {
        let normal = plan(false);
        let capture = plan(true);
        assert!(capture.graph.shares_transients_with(&normal.graph));
        let last = capture.graph.passes().last().expect("capture pass");
        assert_eq!(last.tag, PassKind::Capture);
        assert_eq!(last.barriers[0].new_layout, L::TRANSFER_SRC_OPTIMAL);
        let finals = capture.graph.final_barriers();
        assert_eq!(finals.len(), 2, "present + host read of the capture buffer");
        assert!(finals.iter().any(|barrier| {
            barrier.old_layout == L::TRANSFER_SRC_OPTIMAL
                && barrier.new_layout == L::PRESENT_SRC_KHR
                && barrier.src_access == vk::AccessFlags2::NONE
        }));
        assert!(finals.iter().any(|barrier| {
            !barrier.image
                && barrier.src_access == vk::AccessFlags2::TRANSFER_WRITE
                && barrier.dst_access == vk::AccessFlags2::HOST_READ
        }));
    }

    #[test]
    fn offscreen_passes_render_first_and_the_ui_samples_their_images() {
        let normal = plan(false);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = variant(capture, offscreen);
                // Every combination runs on the same transients as the plain frame.
                assert!(frame.graph.shares_transients_with(&normal.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let passes = frame.graph.passes();
                let count = offscreen.count_ones() as usize;
                assert!(
                    passes[..count]
                        .iter()
                        .all(|pass| matches!(pass.tag, PassKind::Offscreen(_)))
                );
                assert_eq!(passes[count].tag, PassKind::Scene);
                // Offscreen passes join the scene's batch, ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 1);
                let ui = passes
                    .iter()
                    .find(|pass| pass.tag == PassKind::Ui)
                    .expect("ui pass");
                for (slot, resources) in frame.resources.offscreen.iter().enumerate() {
                    assert!(ui.uses(resources.color));
                    let read = ui
                        .barriers
                        .iter()
                        .find(|barrier| barrier.resource == resources.color)
                        .expect("ui read barrier");
                    assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
                    assert_eq!(read.dst_access, vk::AccessFlags2::SHADER_SAMPLED_READ);
                    if offscreen & (1 << slot) != 0 {
                        assert_eq!(read.old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
                        assert_eq!(read.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
                        let tag = PassKind::Offscreen(u8::try_from(slot).expect("slot"));
                        let pass = passes
                            .iter()
                            .find(|pass| pass.tag == tag)
                            .expect("offscreen pass");
                        assert_eq!(pass.barriers.len(), 3, "colour, depth and bones");
                        assert!(pass.barriers.iter().any(|barrier| {
                            barrier.resource == resources.color
                                && barrier.old_layout == L::SHADER_READ_ONLY_OPTIMAL
                                && barrier.new_layout == L::COLOR_ATTACHMENT_OPTIMAL
                        }));
                    } else {
                        assert_eq!(read.old_layout, L::SHADER_READ_ONLY_OPTIMAL);
                        assert_eq!(read.src_access, vk::AccessFlags2::NONE);
                    }
                }
                // Each image is back in SHADER_READ_ONLY_OPTIMAL after the UI: only the
                // swapchain (and the capture buffer) need final barriers.
                assert_eq!(frame.graph.final_barriers().len(), 1 + usize::from(capture));
            }
        }
    }

    #[test]
    fn every_post_topology_compiles_with_shared_transients_and_no_culling() {
        for spec in post_specs() {
            let base = build(&spec, false, 0).expect("compiles");
            for capture in [false, true] {
                for offscreen in 0..OFFSCREEN_VARIANTS {
                    let frame = build(&spec, capture, offscreen).expect("variant compiles");
                    assert!(
                        frame.graph.shares_transients_with(&base.graph),
                        "{spec:?} {capture} {offscreen}"
                    );
                    assert_eq!(frame.graph.culled(), [] as [&str; 0], "{spec:?}");
                }
            }
            let passes = kinds(&base);
            let has = |kind| passes.contains(&kind);
            assert_eq!(has(PassKind::Prepass), spec.has_prepass(), "{spec:?}");
            assert_eq!(has(PassKind::Ao), spec.ambient_occlusion);
            assert_eq!(has(PassKind::AoBlur), spec.ambient_occlusion);
            assert_eq!(has(PassKind::BloomDown(0)), spec.bloom_chain());
            assert_eq!(has(PassKind::BloomUp(0)), spec.bloom);
            assert_eq!(has(PassKind::Exposure), spec.auto_exposure);
            assert_eq!(has(PassKind::Fxaa), spec.fxaa);
            assert_eq!(has(PassKind::Upscale), spec.upscale);
            assert_eq!(has(PassKind::FogRaster), spec.fog.is_some());
            assert_eq!(has(PassKind::FogResolve), spec.fog.is_some());
            assert_eq!(has(PassKind::ContactShadows), spec.contact_shadows);
            assert_eq!(has(PassKind::Outline), spec.outline_ids);
            // Order: [fow-raster fow-resolve] [shadow] [prepass ao ao_blur] scene [bloom_d* exposure bloom_u*]
            // composite [fxaa] [upscale] ui.
            let at = |kind| passes.iter().position(|&pass| pass == kind);
            let scene = at(PassKind::Scene).expect("scene");
            let composite = at(PassKind::Composite).expect("composite");
            let ui = at(PassKind::Ui).expect("ui");
            assert!(at(PassKind::AoBlur).is_none_or(|blur| blur < scene));
            assert!(at(PassKind::FogResolve).is_none_or(|resolve| {
                at(PassKind::FogRaster) == Some(resolve - 1)
                    && at(PassKind::Prepass).is_none_or(|prepass| resolve < prepass)
                    && resolve < scene
            }));
            assert!(at(PassKind::BloomDown(0)).is_none_or(|down| down > scene));
            // Particles land in the HDR target after the scene and before any post pass
            // reads it (bloom, exposure metering, composite).
            match spec.vfx {
                VfxMode::Oit => {
                    let resolve = at(PassKind::VfxResolve).expect("vfx resolve");
                    assert_eq!(at(PassKind::VfxDraw), Some(scene + 1), "{spec:?}");
                    assert_eq!(resolve, scene + 2);
                    assert!(at(PassKind::BloomDown(0)).is_none_or(|down| down > resolve));
                    assert!(resolve < composite);
                }
                VfxMode::Unsorted => {
                    // Straight into the HDR target: no resolve pass, no OIT targets.
                    assert_eq!(at(PassKind::VfxDraw), Some(scene + 1), "{spec:?}");
                    assert!(at(PassKind::VfxResolve).is_none());
                    assert!(at(PassKind::BloomDown(0)).is_none_or(|down| down > scene + 1));
                }
                VfxMode::Off => assert!(at(PassKind::VfxDraw).is_none()),
            }
            assert!(at(PassKind::Exposure).is_none_or(|exposure| exposure < composite));
            assert!(at(PassKind::Fxaa).is_none_or(|fxaa| fxaa > composite));
            // R2b: contact shadows between the prepass and the scene; outlines right after
            // the composite, before FXAA and the upscale.
            assert!(at(PassKind::ContactShadows).is_none_or(|contact| {
                at(PassKind::Prepass).is_some_and(|prepass| prepass < contact) && contact < scene
            }));
            assert!(at(PassKind::Outline).is_none_or(|outline| outline == composite + 1));
            assert!(
                at(PassKind::Upscale)
                    .is_none_or(|upscale| upscale > composite && upscale + 1 == ui)
            );
            // Exactly one pass before the UI writes the swapchain, and the batch before the
            // acquire wait holds everything up to it.
            let writers: Vec<usize> = base
                .graph
                .passes()
                .iter()
                .enumerate()
                .filter(|(_, pass)| pass.uses(base.resources.swapchain))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(writers[0], base.acquire_split);
            // R2b's outline pass also draws on the swapchain when the composite writes it.
            let outline_on_swapchain = spec.outline_ids && !(spec.fxaa || spec.upscale);
            assert_eq!(
                writers.len(),
                2 + usize::from(outline_on_swapchain),
                "{spec:?}: one post writer (+ the outlines) + the ui"
            );
            assert_eq!(writers[writers.len() - 1], ui);
            // Imports with final accesses: the swapchain, the VFX stats readback and, with
            // auto exposure, the persistent exposure image, which is back in
            // SHADER_READ_ONLY_OPTIMAL after the composite's read (no final barrier needed).
            let finals = 1 + usize::from(spec.vfx != VfxMode::Off);
            assert_eq!(base.graph.final_barriers().len(), finals, "{spec:?}");
            assert!(base.graph.passes().len() <= crate::graph::MAX_PASSES);
            // Slots: hdr + depth, [shadow], [fog-of-war raw], [msaa x2], [ao x2], [bloom x5],
            // [ldr], [ldr2], [OIT accum + reveal], [outline IDs (+ msaa)], [contact].
            let expected = 2
                + 2 * usize::from(spec.vfx == VfxMode::Oit)
                + usize::from(spec.shadow.is_some())
                + usize::from(spec.fog.is_some())
                + 2 * usize::from(spec.msaa)
                + 2 * usize::from(spec.ambient_occlusion)
                + BLOOM_LEVELS * usize::from(spec.bloom_chain())
                + usize::from(spec.fxaa || spec.upscale)
                + usize::from(spec.fxaa && spec.upscale)
                + usize::from(spec.outline_ids) * (1 + usize::from(spec.msaa))
                + usize::from(spec.contact_shadows);
            assert_eq!(base.graph.slots().len(), expected, "{spec:?}");
        }
    }

    #[test]
    fn prepass_depth_feeds_ao_then_the_scene_loads_it() {
        for msaa in [false, true] {
            let spec = FrameSpec {
                ambient_occlusion: true,
                msaa,
                ..plain()
            };
            let frame = build(&spec, false, 0).expect("compiles");
            let passes = frame.graph.passes();
            assert_eq!(
                &kinds(&frame)[..4],
                [
                    PassKind::Prepass,
                    PassKind::Ao,
                    PassKind::AoBlur,
                    PassKind::Scene
                ]
            );
            let depth = frame.resources.depth;
            let read = passes[1]
                .barriers
                .iter()
                .find(|barrier| barrier.resource == depth)
                .expect("prepass depth -> ao");
            assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
            if msaa {
                // The resolve writes at COLOR_ATTACHMENT_OUTPUT.
                assert!(
                    read.src_stage
                        .contains(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                );
                let msaa = frame.resources.msaa.expect("msaa");
                // The scene loads the multisampled depth the prepass wrote and does not
                // touch the resolved one again.
                assert!(passes[3].uses(msaa.depth) && !passes[3].uses(depth));
                let load = passes[3]
                    .barriers
                    .iter()
                    .find(|barrier| barrier.resource == msaa.depth)
                    .expect("msaa depth barrier");
                assert_eq!(load.old_layout, L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
                assert!(
                    load.dst_access
                        .contains(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ)
                );
            } else {
                assert_eq!(
                    read.src_access,
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
                );
                let load = passes[3]
                    .barriers
                    .iter()
                    .find(|barrier| barrier.resource == depth)
                    .expect("depth barrier before the scene");
                assert_eq!(
                    (load.old_layout, load.new_layout),
                    (
                        L::SHADER_READ_ONLY_OPTIMAL,
                        L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
                    )
                );
            }
            let ao = frame.resources.ao.expect("ao");
            let sampled = passes[3]
                .barriers
                .iter()
                .find(|barrier| barrier.resource == ao.blurred)
                .expect("ao -> scene");
            assert_eq!(sampled.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
            let slot = frame.graph.slots()[frame.graph.slot_of(ao.blurred).expect("ao slot")];
            assert_eq!(slot.desc.size, ImageSize::Render { shift: 1 });
        }
    }

    #[test]
    fn r2b_prepass_feeds_contact_shadows_the_scene_and_the_outlines() {
        for (msaa, fxaa) in [(false, false), (true, false), (false, true), (true, true)] {
            let spec = FrameSpec {
                outline_ids: true,
                contact_shadows: true,
                msaa,
                fxaa,
                ..shadowed_spec()
            };
            let frame = build(&spec, false, 0).expect("compiles");
            let order = kinds(&frame);
            let at = |kind| order.iter().position(|&pass| pass == kind).expect("pass");
            assert_eq!(at(PassKind::ContactShadows), at(PassKind::Prepass) + 1);
            assert_eq!(at(PassKind::Scene), at(PassKind::ContactShadows) + 1);
            assert_eq!(at(PassKind::Outline), at(PassKind::Composite) + 1);
            let passes = frame.graph.passes();
            let resources = frame.resources;
            let barrier = |kind, resource| {
                *passes[at(kind)]
                    .barriers
                    .iter()
                    .find(|barrier| barrier.resource == resource)
                    .expect("barrier")
            };
            // The prepass's (resolved) depth is sampled by the contact pass.
            let sampled = barrier(PassKind::ContactShadows, resources.depth);
            assert_eq!(sampled.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
            // The contact target: written, then sampled by the scene's fragment shaders.
            let contact = resources.contact.expect("contact target");
            let read = barrier(PassKind::Scene, contact);
            assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
            assert_eq!(read.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
            // The IDs: written (and, with MSAA, resolved) by the prepass, read after the
            // composite; the outlines draw on the composite's output.
            let ids = resources.outline_ids.expect("ID target");
            assert_eq!(ids.msaa.is_some(), msaa);
            let prepass = &passes[at(PassKind::Prepass)];
            assert!(prepass.uses(ids.ids) && ids.msaa.is_none_or(|ids| prepass.uses(ids)));
            let ids_read = barrier(PassKind::Outline, ids.ids);
            assert_eq!(ids_read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
            let target = resources.ldr.unwrap_or(resources.swapchain);
            assert_eq!(resources.ldr.is_some(), fxaa);
            assert!(passes[at(PassKind::Outline)].uses(target));
            // Their own slots (R8_UINT, R8_UNORM) at render resolution.
            for resource in [ids.ids, contact] {
                let slot = frame.graph.slots()[frame.graph.slot_of(resource).expect("slot")];
                assert_eq!(slot.desc.size, ImageSize::Render { shift: 0 });
                assert!(slot.usage.contains(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
                ));
            }
        }
    }

    #[test]
    fn a_prepass_for_one_r2b_feature_leaves_out_the_other() {
        let contact_only = build(
            &FrameSpec {
                contact_shadows: true,
                ..plain()
            },
            false,
            0,
        )
        .expect("compiles");
        assert!(contact_only.resources.outline_ids.is_none());
        assert!(kinds(&contact_only).contains(&PassKind::Prepass));
        assert!(!kinds(&contact_only).contains(&PassKind::Outline));
        let outlines_only = build(
            &FrameSpec {
                outline_ids: true,
                ..plain()
            },
            false,
            0,
        )
        .expect("compiles");
        assert!(outlines_only.resources.contact.is_none());
        assert!(kinds(&outlines_only).contains(&PassKind::Prepass));
        assert!(!kinds(&outlines_only).contains(&PassKind::ContactShadows));
        assert!(!kinds(&outlines_only).contains(&PassKind::Ao));
    }

    #[test]
    fn bloom_chain_down_samples_meters_then_adds_back_up() {
        let spec = FrameSpec {
            bloom: true,
            auto_exposure: true,
            fxaa: true,
            upscale: true,
            bloom_format: vk::Format::B10G11R11_UFLOAT_PACK32,
            ..plain()
        };
        let frame = build(&spec, false, 0).expect("compiles");
        let mut expected = vec![PassKind::Scene];
        expected.extend((0..5).map(PassKind::BloomDown));
        expected.push(PassKind::Exposure);
        expected.extend((0..4).rev().map(PassKind::BloomUp));
        expected.extend([
            PassKind::Composite,
            PassKind::Fxaa,
            PassKind::Upscale,
            PassKind::Ui,
        ]);
        assert_eq!(kinds(&frame), expected);
        // Only the upscale waits for the acquired image.
        assert_eq!(frame.acquire_split, expected.len() - 2);
        let mips = frame.resources.bloom.expect("bloom");
        for (level, &mip) in mips.iter().enumerate() {
            let slot = frame.graph.slots()[frame.graph.slot_of(mip).expect("mip slot")];
            assert_eq!(
                slot.desc.size,
                ImageSize::Render {
                    shift: u8::try_from(level + 1).expect("small")
                }
            );
        }
        // bloom_u3 loads mip 3 after bloom_d4 sampled it: the layout returns to an
        // attachment and the contents are kept (old layout is not UNDEFINED).
        let up3 = frame
            .graph
            .passes()
            .iter()
            .find(|pass| pass.tag == PassKind::BloomUp(3))
            .expect("bloom_u3");
        let load = up3
            .barriers
            .iter()
            .find(|barrier| barrier.resource == mips[3])
            .expect("mip 3 load");
        assert_eq!(
            (load.old_layout, load.new_layout),
            (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL)
        );
        // The persistent exposure image: written (blended) by its pass, read by the
        // composite, and left in SHADER_READ_ONLY_OPTIMAL.
        let exposure = frame.resources.exposure.expect("exposure");
        let pass = frame
            .graph
            .passes()
            .iter()
            .find(|pass| pass.tag == PassKind::Exposure)
            .expect("exposure pass");
        let write = pass
            .barriers
            .iter()
            .find(|barrier| barrier.resource == exposure)
            .expect("exposure barrier");
        assert_eq!(
            (write.old_layout, write.new_layout),
            (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL)
        );
        assert!(
            frame
                .graph
                .final_barriers()
                .iter()
                .all(|barrier| barrier.resource != exposure)
        );
        // ldr and ldr2 overlap at the FXAA pass, so they never share a slot.
        assert_ne!(
            frame.graph.slot_of(frame.resources.ldr.expect("ldr")),
            frame.graph.slot_of(frame.resources.ldr2.expect("ldr2"))
        );
    }

    #[test]
    fn vfx_passes_wrap_the_scene_and_share_slots_across_variants() {
        for mode in [VfxMode::Unsorted, VfxMode::Oit] {
            let spec = FrameSpec {
                vfx: mode,
                ..plain()
            };
            let frame = build(&spec, false, 0).expect("compiles");
            let kinds = kinds(&frame);
            let mut expected = vec![PassKind::VfxSimulate, PassKind::Scene, PassKind::VfxDraw];
            if mode == VfxMode::Oit {
                expected.push(PassKind::VfxResolve);
            }
            expected.extend([PassKind::Composite, PassKind::Ui]);
            assert_eq!(kinds, expected, "{mode:?}");
            assert_eq!(frame.graph.culled(), [] as [&str; 0]);
            // Everything before the composite is submitted ahead of the acquire wait.
            assert_eq!(frame.acquire_split, expected.len() - 2);
            let vfx = frame.resources.vfx.expect("vfx resources");
            assert_eq!(vfx.accum.is_some(), mode == VfxMode::Oit);
            // HDR + depth, plus the two OIT targets.
            let slots = if mode == VfxMode::Oit { 4 } else { 2 };
            assert_eq!(frame.graph.slots().len(), slots);
            let draw = frame
                .graph
                .passes()
                .iter()
                .find(|pass| pass.tag == PassKind::VfxDraw)
                .expect("draw pass");
            assert!(draw.barriers.iter().any(|barrier| {
                barrier.resource == frame.resources.depth
                    && barrier.new_layout == L::DEPTH_STENCIL_READ_ONLY_OPTIMAL
            }));
            assert!(draw.barriers.iter().any(|barrier| {
                barrier.resource == vfx.args
                    && barrier.dst_access == vk::AccessFlags2::INDIRECT_COMMAND_READ
            }));
            let capture = build(&spec, true, 0).expect("compiles");
            assert!(capture.graph.shares_transients_with(&frame.graph));
            // The stats copy reaches the host after the frame: present + stats readback.
            assert_eq!(frame.graph.final_barriers().len(), 2);
            assert!(frame.graph.final_barriers().iter().any(|barrier| {
                barrier.resource == vfx.stats && barrier.dst_access == vk::AccessFlags2::HOST_READ
            }));
        }
    }

    #[test]
    fn particles_test_the_resolved_depth_with_msaa_and_the_prepass() {
        // With MSAA the particles read the single-sampled depth: resolved by the scene, or
        // by the prepass when AO is on (the scene then only loads the multisampled depth).
        for ambient_occlusion in [false, true] {
            let spec = FrameSpec {
                vfx: VfxMode::Oit,
                msaa: true,
                ambient_occlusion,
                ..shadowed_spec()
            };
            let frame = build(&spec, false, 0).expect("compiles");
            let draw = frame
                .graph
                .passes()
                .iter()
                .find(|pass| pass.tag == PassKind::VfxDraw)
                .expect("draw pass");
            let msaa = frame.resources.msaa.expect("msaa");
            assert!(!draw.uses(msaa.depth) && !draw.uses(msaa.color));
            let read = draw
                .barriers
                .iter()
                .find(|barrier| barrier.resource == frame.resources.depth)
                .expect("depth barrier");
            assert_eq!(read.new_layout, L::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
            // The VFX passes never touch the shadow map.
            let shadow = frame.resources.shadow.expect("shadow map");
            for pass in frame.graph.passes() {
                assert_eq!(
                    pass.uses(shadow),
                    matches!(pass.tag, PassKind::Shadow | PassKind::Scene)
                );
            }
        }
    }

    fn world(capture: bool, offscreen: usize) -> super::FramePlan {
        let spec = FrameSpec {
            fog: Some(256),
            ..shadowed_spec()
        };
        build(&spec, capture, offscreen).expect("the frame graph with fog of war compiles")
    }

    fn barrier(
        pass: &crate::graph::CompiledPass<PassKind>,
        resource: ResourceId,
    ) -> crate::graph::Barrier {
        *pass
            .barriers
            .iter()
            .find(|barrier| barrier.resource == resource)
            .expect("barrier")
    }

    /// R3a: the raster writes the raw transient, the resolve samples it and the history
    /// image and writes this frame's image, which the scene and the UI then sample. Both
    /// persistent images end the frame where they started, so no final barriers.
    #[test]
    fn fog_of_war_passes_raster_then_resolve_before_the_scene() {
        let plain = world(false, 0);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = world(capture, offscreen);
                assert!(frame.graph.shares_transients_with(&plain.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let count = offscreen.count_ones() as usize;
                let passes = frame.graph.passes();
                let kinds: Vec<_> = passes[count..count + 4]
                    .iter()
                    .map(|pass| pass.tag)
                    .collect();
                assert_eq!(
                    kinds,
                    [
                        PassKind::FogRaster,
                        PassKind::FogResolve,
                        PassKind::Shadow,
                        PassKind::Scene
                    ]
                );
                // Every pass before the composite stays ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 4);
                let fog = frame.resources.fog.expect("fog resources");
                let raster = &passes[count];
                let raw = barrier(raster, fog.raw);
                assert_eq!(raw.new_layout, L::COLOR_ATTACHMENT_OPTIMAL);
                let resolve = &passes[count + 1];
                let read_raw = barrier(resolve, fog.raw);
                assert_eq!(
                    (read_raw.old_layout, read_raw.new_layout),
                    (L::COLOR_ATTACHMENT_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL)
                );
                assert_eq!(
                    read_raw.src_access,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
                );
                // The history image is already sampled: an execution dependency at most.
                let history = barrier(resolve, fog.history);
                assert_eq!(history.old_layout, L::SHADER_READ_ONLY_OPTIMAL);
                assert_eq!(history.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
                // This frame's image: write-after-read against the previous frames' samples.
                let write = barrier(resolve, fog.current);
                assert_eq!(
                    (write.old_layout, write.new_layout),
                    (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL)
                );
                assert_eq!(write.src_stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
                let scene = &passes[count + 3];
                let read = barrier(scene, fog.current);
                assert_eq!(
                    (read.old_layout, read.new_layout),
                    (L::COLOR_ATTACHMENT_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL)
                );
                assert_eq!(read.dst_access, vk::AccessFlags2::SHADER_SAMPLED_READ);
                let ui = passes
                    .iter()
                    .find(|pass| pass.tag == PassKind::Ui)
                    .expect("ui");
                assert!(ui.uses(fog.current));
                assert!(
                    frame
                        .graph
                        .final_barriers()
                        .iter()
                        .all(|barrier| barrier.resource != fog.current
                            && barrier.resource != fog.history)
                );
            }
        }
        // Shadow map, raw fog raster, hdr and depth: four slots, the fixed-size ones apart.
        assert_eq!(plain.graph.slots().len(), 4);
        assert!(plain.graph.passes().len() <= crate::graph::MAX_PASSES);
    }
}
