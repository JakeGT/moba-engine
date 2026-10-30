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
//! depth_prepass (R2b; High+, or screen outlines) terrain + units + skinned + VAT → depth
//!               (+ the R8_UINT outline ID target with screen outlines)
//! contact_shadows (R2b; with contact shadows) depth (sampled) → R8 contact visibility
//! scene         terrain + units + skinned + sky → hdr (colour, cleared) + depth (cleared, or
//!               loaded after the pre-pass; sampled-capable), sampling the shadow map, this
//!               frame's fog-of-war image and the contact target
//! vfx_draw      particles → hdr (+ OIT accum/reveal), depth read-only + sampled (soft)
//! vfx_resolve   OIT accum/reveal (sampled) blended over hdr (Medium and up)
//! composite     hdr (sampled) → exposure + AgX tonemap → swapchain
//! outline       (R2b; screen outlines) ID target (sampled) → outlines blended over the
//!               swapchain
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
//! barriers, so nothing here allocates on the frame path.
//!
//! The offscreen passes never sample the shadow map: their views bind a lighting set whose
//! shadow binding is a persistent 1×1 default image, and their camera uniform turns shadows
//! off (ADR-0011), so the shadow transient is only read by the scene, after the shadow pass.

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

/// What a compiled pass records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassKind {
    FogRaster,
    FogResolve,
    Shadow,
    VfxSimulate,
    /// The main view's depth (and outline IDs) before shading (R2b).
    DepthPrepass,
    /// Screen-space contact shadows from the pre-pass depth (R2b).
    ContactShadows,
    Scene,
    /// Screen-space outlines over the tonemapped image (R2b).
    Outline,
    VfxDraw,
    VfxResolve,
    Composite,
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
    /// The `R8_UINT` outline ID target; `None` without screen outlines.
    pub(crate) outline_ids: Option<ResourceId>,
    /// The `R8_UNORM` contact-shadow target; `None` without contact shadows.
    pub(crate) contact: Option<ResourceId>,
    /// Whether the depth pre-pass runs (the scene then loads its depth).
    pub(crate) prepass: bool,
    /// The fog-of-war resources; `None` unless the fog of war uses the texture.
    pub(crate) fog: Option<FogResources>,
}

/// The depth pre-pass features of a renderer (the same for every variant, ADR-0016).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DepthFeatures {
    /// Screen-space outlines: the pre-pass writes the ID target and the outline pass reads it.
    pub(crate) outline_ids: bool,
    /// Contact shadows: a pass between the pre-pass and the scene.
    pub(crate) contact: bool,
}

impl DepthFeatures {
    /// No pre-pass.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        outline_ids: false,
        contact: false,
    };

    /// Whether the depth pre-pass runs.
    pub(crate) const fn prepass(self) -> bool {
        self.outline_ids || self.contact
    }
}

/// The outline ID target's format: a mandatory colour-attachment and sampled format.
pub(crate) const OUTLINE_ID_FORMAT: vk::Format = vk::Format::R8_UINT;
/// The contact-shadow target's format.
pub(crate) const CONTACT_FORMAT: vk::Format = vk::Format::R8_UNORM;

/// The shadow map a frame renders, when shadows are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShadowSpec {
    pub(crate) format: vk::Format,
    pub(crate) size: u32,
}

/// A compiled frame variant and its resource ids.
pub(crate) struct FramePlan {
    pub(crate) graph: CompiledGraph<PassKind>,
    pub(crate) resources: FrameResources,
    /// Index of the first pass that touches the swapchain image. Passes before it go in a
    /// first batch that does not wait on the acquire semaphore; the rest wait on it.
    pub(crate) acquire_split: usize,
}

/// Builds and compiles the frame graph. Bit `s` of `offscreen` adds the pass that renders
/// offscreen image `s` this frame; `shadow` adds the shadow pass and map, `fog` (the
/// texture's edge) the fog-of-war passes and `depth_features` the depth pre-pass, contact
/// shadows and screen outlines (every variant of one renderer passes the same `shadow`,
/// `fog`, `vfx_mode` and `depth_features`, so they share transients).
///
/// # Errors
/// Only if the declaration below is inconsistent (covered by unit tests).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One declaration of the frame.
pub(crate) fn build(
    hdr_format: vk::Format,
    depth_format: vk::Format,
    depth_aspect: vk::ImageAspectFlags,
    shadow: Option<ShadowSpec>,
    fog: Option<u32>,
    capture: bool,
    offscreen: usize,
    vfx_mode: VfxMode,
    depth_features: DepthFeatures,
) -> Result<FramePlan, GraphError> {
    let mut graph = GraphBuilder::new();
    // The acquire semaphore is waited on at COLOR_ATTACHMENT_OUTPUT; the composite writes
    // every pixel, so the previous contents are discarded.
    let swapchain = graph.import_image(
        "swapchain",
        ImportState::discard(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT),
        Some(Access::Present),
    );
    // The frame fence covers write-after-read against the previous use of this frame's
    // bone buffer; the host writes it before submission.
    let bones = graph.import_buffer("bones", ImportState::host_written(), None);
    let hdr = graph.transient_image(
        "hdr",
        ImageDesc {
            format: hdr_format,
            aspect: vk::ImageAspectFlags::COLOR,
            size: ImageSize::Swapchain,
            extra_usage: vk::ImageUsageFlags::empty(),
        },
    );
    // Sampled-capable so later passes (soft particles, decals, SSAO) can read it.
    let depth = graph.transient_image(
        "depth",
        ImageDesc {
            format: depth_format,
            aspect: depth_aspect,
            size: ImageSize::Swapchain,
            extra_usage: vk::ImageUsageFlags::SAMPLED,
        },
    );
    // Depth only and fixed-size, so it never aliases the swapchain-sized targets. Written by
    // the shadow pass, sampled (depth compare) by the scene's fragment shaders.
    let shadow_map = shadow.map(|spec| {
        graph.transient_image(
            "shadow-map",
            ImageDesc {
                format: spec.format,
                aspect: vk::ImageAspectFlags::DEPTH,
                size: ImageSize::Fixed {
                    width: spec.size,
                    height: spec.size,
                },
                extra_usage: vk::ImageUsageFlags::empty(),
            },
        )
    });
    // R3a fog of war: the raw raster is fixed-size (never aliases swapchain-sized targets);
    // the two persistent images start and end every frame sampled, like R7b's.
    let fog_resources = fog.map(|resolution| FogResources {
        raw: graph.transient_image(
            "fow-raw",
            ImageDesc {
                format: crate::passes::fog::FOG_FORMAT,
                aspect: vk::ImageAspectFlags::COLOR,
                size: ImageSize::Fixed {
                    width: resolution,
                    height: resolution,
                },
                extra_usage: vk::ImageUsageFlags::empty(),
            },
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

    let screen_target = |format| ImageDesc {
        format,
        aspect: vk::ImageAspectFlags::COLOR,
        size: ImageSize::Swapchain,
        extra_usage: vk::ImageUsageFlags::empty(),
    };
    let outline_ids = depth_features
        .outline_ids
        .then(|| graph.transient_image("outline-ids", screen_target(OUTLINE_ID_FORMAT)));
    let contact = depth_features
        .contact
        .then(|| graph.transient_image("contact-shadows", screen_target(CONTACT_FORMAT)));

    let vfx = (vfx_mode != VfxMode::Off).then(|| {
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
        let oit = vfx_mode == VfxMode::Oit;
        let (accum_format, reveal_format) = crate::passes::vfx::oit_formats();
        let target = |format| ImageDesc {
            format,
            aspect: vk::ImageAspectFlags::COLOR,
            size: ImageSize::Swapchain,
            extra_usage: vk::ImageUsageFlags::empty(),
        };
        VfxResources {
            particles: graph.import_buffer("vfx-particles", persistent, None),
            alive: graph.import_buffer("vfx-alive", persistent, None),
            args: graph.import_buffer("vfx-args", persistent, None),
            spawns: graph.import_buffer("vfx-spawns", ImportState::host_written(), None),
            stats: graph.import_buffer("vfx-stats", ImportState::idle(), Some(Access::HostRead)),
            accum: oit.then(|| graph.transient_image("vfx-accum", target(accum_format))),
            reveal: oit.then(|| graph.transient_image("vfx-reveal", target(reveal_format))),
        }
    });

    // Declared after every resource both variants share, so their ids match.
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
    let prepass = depth_features.prepass();
    if prepass {
        let mut pass = graph
            .pass("depth_prepass", PassKind::DepthPrepass)
            .access(depth, Access::DepthWrite)
            .access(bones, Access::VertexStorageRead);
        if let Some(ids) = outline_ids {
            pass = pass.access(ids, Access::ColorWrite);
        }
        let _ = pass;
    }
    if let Some(contact) = contact {
        let _ = graph
            .pass("contact_shadows", PassKind::ContactShadows)
            .access(depth, Access::SampledFragment)
            .access(contact, Access::ColorWrite);
    }
    let mut scene = graph
        .pass("scene", PassKind::Scene)
        .access(hdr, Access::ColorWrite)
        .access(
            depth,
            if prepass {
                Access::DepthReadWrite
            } else {
                Access::DepthWrite
            },
        )
        .access(bones, Access::VertexStorageRead);
    if let Some(shadow_map) = shadow_map {
        scene = scene.access(shadow_map, Access::SampledFragment);
    }
    if let Some(fog) = fog_resources {
        scene = scene.access(fog.current, Access::SampledFragment);
    }
    if let Some(contact) = contact {
        scene = scene.access(contact, Access::SampledFragment);
    }
    let _ = scene;
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
    let _ = graph
        .pass("composite", PassKind::Composite)
        .access(hdr, Access::SampledFragment)
        .access(swapchain, Access::ColorWrite);
    if let Some(ids) = outline_ids {
        let _ = graph
            .pass("outline", PassKind::Outline)
            .access(ids, Access::SampledFragment)
            .access(swapchain, Access::ColorReadWrite);
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
            outline_ids,
            contact,
            prepass,
            fog: fog_resources,
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
    use super::{DepthFeatures, OFFSCREEN_VARIANTS, PassKind, ShadowSpec, VfxMode, build};
    use crate::graph::ResourceId;
    use ash::vk;
    use vk::ImageLayout as L;

    fn variant(capture: bool, offscreen: usize) -> super::FramePlan {
        full(capture, offscreen, VfxMode::Off)
    }

    fn plan_with(capture: bool, vfx: VfxMode) -> super::FramePlan {
        full(capture, 0, vfx)
    }

    fn full(capture: bool, offscreen: usize, vfx: VfxMode) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            None,
            None,
            capture,
            offscreen,
            vfx,
            DepthFeatures::NONE,
        )
        .expect("the engine frame graph compiles")
    }

    fn shadowed(capture: bool, offscreen: usize) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            Some(ShadowSpec {
                format: vk::Format::D32_SFLOAT,
                size: 2048,
            }),
            None,
            capture,
            offscreen,
            VfxMode::Off,
            DepthFeatures::NONE,
        )
        .expect("the shadowed frame graph compiles")
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
        // Its own slot: a fixed size never aliases the swapchain-sized depth.
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
        let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
        assert_eq!(kinds, [PassKind::Scene, PassKind::Composite, PassKind::Ui]);
        // The scene is submitted ahead of the acquire wait; the composite waits for it.
        assert_eq!(frame.acquire_split, 1);
        assert_eq!(plan(true).acquire_split, 1);
        assert_eq!(frame.graph.culled(), [] as [&str; 0]);
        // HDR colour and depth overlap in the scene pass, so they get separate slots.
        assert_eq!(frame.graph.slots().len(), 2);
        let depth = frame.graph.slots()[frame.graph.slot_of(frame.resources.depth).unwrap()];
        assert!(depth.usage.contains(vk::ImageUsageFlags::SAMPLED));
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
    fn vfx_passes_wrap_the_scene_and_share_slots_across_variants() {
        for mode in [VfxMode::Unsorted, VfxMode::Oit] {
            let frame = plan_with(false, mode);
            let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
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
            let capture = plan_with(true, mode);
            assert!(capture.graph.shares_transients_with(&frame.graph));
            // The stats copy reaches the host after the frame: present + stats readback.
            assert_eq!(frame.graph.final_barriers().len(), 2);
            assert!(frame.graph.final_barriers().iter().any(|barrier| {
                barrier.resource == vfx.stats && barrier.dst_access == vk::AccessFlags2::HOST_READ
            }));
        }
    }

    #[test]
    fn every_shadowed_vfx_and_fog_variant_fits_the_graph_limits() {
        let shadow = Some(ShadowSpec {
            format: vk::Format::D32_SFLOAT,
            size: 2048,
        });
        let plan = |capture, offscreen| {
            build(
                vk::Format::R16G16B16A16_SFLOAT,
                vk::Format::D32_SFLOAT,
                vk::ImageAspectFlags::DEPTH,
                shadow,
                Some(256),
                capture,
                offscreen,
                VfxMode::Oit,
                DepthFeatures::NONE,
            )
            .expect("the shadowed VFX frame graph compiles")
        };
        let plain = plan(false, 0);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = plan(capture, offscreen);
                assert!(frame.graph.shares_transients_with(&plain.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
                let count = offscreen.count_ones() as usize;
                let mut expected = vec![
                    PassKind::FogRaster,
                    PassKind::FogResolve,
                    PassKind::VfxSimulate,
                    PassKind::Shadow,
                    PassKind::Scene,
                    PassKind::VfxDraw,
                    PassKind::VfxResolve,
                    PassKind::Composite,
                    PassKind::Ui,
                ];
                if capture {
                    expected.push(PassKind::Capture);
                }
                assert_eq!(&kinds[count..], expected.as_slice());
                assert!(kinds.len() <= crate::graph::MAX_PASSES);
                // Every pass before the composite goes ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 7);
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
    }

    fn depth_plan(capture: bool, offscreen: usize, features: DepthFeatures) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            Some(ShadowSpec {
                format: vk::Format::D32_SFLOAT,
                size: 2048,
            }),
            None,
            capture,
            offscreen,
            VfxMode::Oit,
            features,
        )
        .expect("the pre-pass frame graph compiles")
    }

    #[test]
    fn the_prepass_feeds_contact_shadows_the_scene_and_the_outline_pass() {
        let all = DepthFeatures {
            outline_ids: true,
            contact: true,
        };
        let plain = depth_plan(false, 0, all);
        for capture in [false, true] {
            for offscreen in 0..OFFSCREEN_VARIANTS {
                let frame = depth_plan(capture, offscreen, all);
                assert!(frame.graph.shares_transients_with(&plain.graph));
                assert_eq!(frame.graph.culled(), [] as [&str; 0]);
                let count = offscreen.count_ones() as usize;
                let kinds: Vec<_> = frame.graph.passes().iter().map(|pass| pass.tag).collect();
                let mut expected = vec![
                    PassKind::VfxSimulate,
                    PassKind::Shadow,
                    PassKind::DepthPrepass,
                    PassKind::ContactShadows,
                    PassKind::Scene,
                    PassKind::VfxDraw,
                    PassKind::VfxResolve,
                    PassKind::Composite,
                    PassKind::Outline,
                    PassKind::Ui,
                ];
                if capture {
                    expected.push(PassKind::Capture);
                }
                assert_eq!(&kinds[count..], expected.as_slice());
                assert!(kinds.len() <= crate::graph::MAX_PASSES);
                // Everything up to the resolve is ahead of the acquire wait.
                assert_eq!(frame.acquire_split, count + 7);
            }
        }
        let resources = plain.resources;
        let (depth, contact, ids) = (
            resources.depth,
            resources.contact.expect("contact target"),
            resources.outline_ids.expect("ID target"),
        );
        let pass = |tag| {
            plain
                .graph
                .passes()
                .iter()
                .find(|pass| pass.tag == tag)
                .expect("pass")
        };
        let barrier = |tag, resource| {
            *pass(tag)
                .barriers
                .iter()
                .find(|barrier| barrier.resource == resource)
                .expect("barrier")
        };
        // Pre-pass depth write -> sampled by the contact pass.
        let sampled = barrier(PassKind::ContactShadows, depth);
        assert_eq!(
            (sampled.old_layout, sampled.new_layout),
            (
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                L::SHADER_READ_ONLY_OPTIMAL
            )
        );
        assert_eq!(
            sampled.src_access,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE
        );
        // -> loaded, tested and written by the scene: contents kept (not UNDEFINED).
        let loaded = barrier(PassKind::Scene, depth);
        assert_eq!(
            (loaded.old_layout, loaded.new_layout),
            (
                L::SHADER_READ_ONLY_OPTIMAL,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
            )
        );
        assert!(
            loaded
                .dst_access
                .contains(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ)
        );
        // The contact target: written, then sampled by the scene's fragment shaders.
        let read = barrier(PassKind::Scene, contact);
        assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(read.src_access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        // The ID target lives until the outline pass, after the composite.
        let ids_read = barrier(PassKind::Outline, ids);
        assert_eq!(ids_read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
        assert!(pass(PassKind::Outline).uses(plain.resources.swapchain));
        // Their own slots (R8_UINT, R8_UNORM): no aliasing with HDR or the OIT targets.
        for resource in [ids, contact] {
            let slot = plain.graph.slots()[plain.graph.slot_of(resource).expect("slot")];
            assert!(
                slot.usage
                    .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
            );
        }
    }

    #[test]
    fn a_prepass_for_one_feature_leaves_out_the_other() {
        let contact_only = depth_plan(
            false,
            0,
            DepthFeatures {
                outline_ids: false,
                contact: true,
            },
        );
        assert!(contact_only.resources.outline_ids.is_none());
        assert!(contact_only.resources.prepass);
        assert!(
            !contact_only
                .graph
                .passes()
                .iter()
                .any(|pass| pass.tag == PassKind::Outline)
        );
        let outlines_only = depth_plan(
            false,
            0,
            DepthFeatures {
                outline_ids: true,
                contact: false,
            },
        );
        assert!(outlines_only.resources.contact.is_none());
        let kinds: Vec<_> = outlines_only
            .graph
            .passes()
            .iter()
            .map(|pass| pass.tag)
            .collect();
        assert!(kinds.contains(&PassKind::DepthPrepass));
        assert!(!kinds.contains(&PassKind::ContactShadows));
        // Pre-pass depth write -> scene load: a write-after-write in the same layout.
        let scene = outlines_only
            .graph
            .passes()
            .iter()
            .find(|pass| pass.tag == PassKind::Scene)
            .expect("scene");
        let depth = scene
            .barriers
            .iter()
            .find(|barrier| barrier.resource == outlines_only.resources.depth)
            .expect("depth barrier");
        assert_eq!(depth.old_layout, L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let none = depth_plan(false, 0, DepthFeatures::NONE);
        assert!(!none.resources.prepass);
    }

    fn world(capture: bool, offscreen: usize) -> super::FramePlan {
        build(
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::D32_SFLOAT,
            vk::ImageAspectFlags::DEPTH,
            Some(ShadowSpec {
                format: vk::Format::D32_SFLOAT,
                size: 2048,
            }),
            Some(256),
            capture,
            offscreen,
            VfxMode::Off,
            DepthFeatures::NONE,
        )
        .expect("the frame graph with fog of war compiles")
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
