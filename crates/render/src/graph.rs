//! A small, explicit render graph (ADR-0008).
//!
//! Passes are declared in execution order with the images and buffers they touch and *how*
//! ([`Access`]). [`GraphBuilder::compile`] turns that list into a [`CompiledGraph`]:
//!
//! - passes whose outputs nothing consumes are culled;
//! - transient images get a physical slot, and transients with identical descriptions whose
//!   lifetimes do not overlap share one slot (pooling/aliasing);
//! - every hazard (read-after-write, write-after-read, write-after-write, layout change) gets
//!   a `synchronization2` barrier with exact stage/access masks and layout transitions,
//!   grouped per pass, plus the final transitions of imported resources (for example the
//!   swapchain image to `PRESENT_SRC_KHR`).
//!
//! This module is pure logic: it uses `ash::vk` flag types but never touches a device, so
//! the barrier plan is unit-tested headless. Compilation allocates and runs only at
//! renderer creation; recording a compiled graph walks slices and allocates nothing.

use ash::vk;

/// Most passes one graph may hold (the GPU profiler sizes its query pools from this). R6's
/// post chain (ADR-0015), R2b's contact shadows and outlines (ADR-0016), R3a's two
/// fog-of-war passes, R3b's two full-water passes (ADR-0017) and the heat haze's two passes
/// (ADR-0019) bring a frame with everything on (both offscreen passes and the capture
/// included) to 33.
pub(crate) const MAX_PASSES: usize = 40;
/// Most resources (imported + transient) one graph may declare (at most 64: a pass's
/// resource set is a `u64` bit mask).
pub(crate) const MAX_RESOURCES: usize = 48;
/// Most barriers emitted before one pass or at the end of the graph.
pub(crate) const MAX_BARRIERS: usize = 16;
const _: () = assert!(MAX_RESOURCES <= u64::BITS as usize);

/// A resource declared on a [`GraphBuilder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ResourceId(u16);

impl ResourceId {
    #[inline]
    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

/// How a pass (or the end of the frame) uses a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// Colour attachment, previous contents discarded or cleared (`loadOp` `CLEAR` or `DONT_CARE`).
    ColorWrite,
    /// Colour attachment that loads and blends over the previous contents.
    ColorReadWrite,
    /// Depth attachment, cleared, tested and written.
    DepthWrite,
    /// Depth attachment that loads the previous contents, tests and writes (the scene after
    /// a depth prepass, ADR-0015).
    DepthReadWrite,
    /// [`Self::DepthWrite`] on a multisampled attachment that is also the **source** of a
    /// depth resolve: the resolve reads it at `COLOR_ATTACHMENT_OUTPUT` with
    /// `COLOR_ATTACHMENT_READ`, which a later writer must wait for.
    DepthWriteResolveSource,
    /// [`Self::DepthReadWrite`] on a multisampled attachment that is also the source of a
    /// depth resolve (the Ultra scene after the MSAA prepass when a later pass reads the
    /// single-sample depth, R3b): loads, tests, writes, then resolves.
    DepthReadWriteResolveSource,
    /// Single-sample depth written by a multisample depth resolve at the end of a rendering
    /// scope. Resolves run at `COLOR_ATTACHMENT_OUTPUT` with `COLOR_ATTACHMENT_WRITE`; the
    /// fragment-test stages and depth write are included so the access is conservative.
    DepthResolveWrite,
    /// Sampled from a fragment shader.
    SampledFragment,
    /// Copy source (image or buffer).
    TransferRead,
    /// Copy destination (image or buffer).
    TransferWrite,
    /// Storage buffer read by a vertex shader.
    VertexStorageRead,
    /// Storage buffer read by a compute shader.
    ComputeStorageRead,
    /// Storage buffer read and written by a compute shader (contents kept).
    ComputeStorageReadWrite,
    /// Indirect draw arguments (`vkCmdDraw*Indirect`).
    IndirectRead,
    /// Depth attachment that is tested but not written, sampled by the fragment shader in
    /// the same pass (soft particles): `DEPTH_STENCIL_READ_ONLY_OPTIMAL`.
    DepthReadSampled,
    /// Host read after the submission completes (buffers only; final access).
    HostRead,
    /// Presentation (swapchain images only; final access). Ordered before the submit's
    /// semaphore signal, which the present operation waits on.
    Present,
}

/// Synchronisation facts for one [`Access`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AccessInfo {
    pub(crate) stage: vk::PipelineStageFlags2,
    pub(crate) access: vk::AccessFlags2,
    /// The write subset of `access` (what a later barrier must make available).
    pub(crate) write_access: vk::AccessFlags2,
    /// Image layout the access needs; ignored for buffers.
    pub(crate) layout: vk::ImageLayout,
    /// Whether the access needs the previous contents (keeps their producer alive).
    pub(crate) reads_previous: bool,
    /// Image usage the access needs; empty for buffers.
    pub(crate) usage: vk::ImageUsageFlags,
}

impl AccessInfo {
    #[inline]
    pub(crate) fn writes(&self) -> bool {
        !self.write_access.is_empty()
    }
}

impl Access {
    /// Stage, access, layout and usage of this access.
    #[must_use]
    #[allow(clippy::too_many_lines)] // One exhaustive table.
    pub(crate) fn info(self) -> AccessInfo {
        use vk::AccessFlags2 as A;
        use vk::ImageLayout as L;
        use vk::ImageUsageFlags as U;
        use vk::PipelineStageFlags2 as S;
        let (stage, access, write_access, layout, reads_previous, usage) = match self {
            Self::ColorWrite => (
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
                A::COLOR_ATTACHMENT_WRITE,
                L::COLOR_ATTACHMENT_OPTIMAL,
                false,
                U::COLOR_ATTACHMENT,
            ),
            Self::ColorReadWrite => (
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE,
                A::COLOR_ATTACHMENT_WRITE,
                L::COLOR_ATTACHMENT_OPTIMAL,
                true,
                U::COLOR_ATTACHMENT,
            ),
            Self::DepthWrite => (
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
                A::DEPTH_STENCIL_ATTACHMENT_READ | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                false,
                U::DEPTH_STENCIL_ATTACHMENT,
            ),
            Self::DepthReadWrite => (
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
                A::DEPTH_STENCIL_ATTACHMENT_READ | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                true,
                U::DEPTH_STENCIL_ATTACHMENT,
            ),
            Self::DepthReadWriteResolveSource => (
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS | S::COLOR_ATTACHMENT_OUTPUT,
                A::DEPTH_STENCIL_ATTACHMENT_READ
                    | A::DEPTH_STENCIL_ATTACHMENT_WRITE
                    | A::COLOR_ATTACHMENT_READ,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                true,
                U::DEPTH_STENCIL_ATTACHMENT,
            ),
            Self::DepthWriteResolveSource => (
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS | S::COLOR_ATTACHMENT_OUTPUT,
                A::DEPTH_STENCIL_ATTACHMENT_READ
                    | A::DEPTH_STENCIL_ATTACHMENT_WRITE
                    | A::COLOR_ATTACHMENT_READ,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                false,
                U::DEPTH_STENCIL_ATTACHMENT,
            ),
            Self::DepthResolveWrite => (
                S::COLOR_ATTACHMENT_OUTPUT | S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
                A::COLOR_ATTACHMENT_WRITE | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                A::COLOR_ATTACHMENT_WRITE | A::DEPTH_STENCIL_ATTACHMENT_WRITE,
                L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                false,
                U::DEPTH_STENCIL_ATTACHMENT,
            ),
            Self::SampledFragment => (
                S::FRAGMENT_SHADER,
                A::SHADER_SAMPLED_READ,
                A::NONE,
                L::SHADER_READ_ONLY_OPTIMAL,
                true,
                U::SAMPLED,
            ),
            Self::TransferRead => (
                S::TRANSFER,
                A::TRANSFER_READ,
                A::NONE,
                L::TRANSFER_SRC_OPTIMAL,
                true,
                U::TRANSFER_SRC,
            ),
            Self::TransferWrite => (
                S::TRANSFER,
                A::TRANSFER_WRITE,
                A::TRANSFER_WRITE,
                L::TRANSFER_DST_OPTIMAL,
                false,
                U::TRANSFER_DST,
            ),
            Self::VertexStorageRead => (
                S::VERTEX_SHADER,
                A::SHADER_STORAGE_READ,
                A::NONE,
                L::UNDEFINED,
                true,
                U::empty(),
            ),
            Self::ComputeStorageRead => (
                S::COMPUTE_SHADER,
                A::SHADER_STORAGE_READ,
                A::NONE,
                L::UNDEFINED,
                true,
                U::empty(),
            ),
            Self::ComputeStorageReadWrite => (
                S::COMPUTE_SHADER,
                A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE,
                A::SHADER_STORAGE_WRITE,
                L::UNDEFINED,
                true,
                U::empty(),
            ),
            Self::IndirectRead => (
                S::DRAW_INDIRECT,
                A::INDIRECT_COMMAND_READ,
                A::NONE,
                L::UNDEFINED,
                true,
                U::empty(),
            ),
            Self::DepthReadSampled => (
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS | S::FRAGMENT_SHADER,
                A::DEPTH_STENCIL_ATTACHMENT_READ | A::SHADER_SAMPLED_READ,
                A::NONE,
                L::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                true,
                U::DEPTH_STENCIL_ATTACHMENT | U::SAMPLED,
            ),
            Self::HostRead => (
                S::HOST,
                A::HOST_READ,
                A::NONE,
                L::UNDEFINED,
                true,
                U::empty(),
            ),
            // ALL_COMMANDS, not NONE: the transition must happen-before the submission's
            // semaphore signal (legacy `vkQueueSubmit` signals at ALL_COMMANDS), which the
            // present waits on. With NONE, sync validation reports PRESENT_AFTER_WRITE.
            Self::Present => (
                S::ALL_COMMANDS,
                A::NONE,
                A::NONE,
                L::PRESENT_SRC_KHR,
                true,
                U::empty(),
            ),
        };
        AccessInfo {
            stage,
            access,
            write_access,
            layout,
            reads_previous,
            usage,
        }
    }

    const fn valid_for_image(self) -> bool {
        !matches!(
            self,
            Self::VertexStorageRead
                | Self::ComputeStorageRead
                | Self::ComputeStorageReadWrite
                | Self::IndirectRead
                | Self::HostRead
        )
    }

    const fn valid_for_buffer(self) -> bool {
        matches!(
            self,
            Self::TransferRead
                | Self::TransferWrite
                | Self::VertexStorageRead
                | Self::ComputeStorageRead
                | Self::ComputeStorageReadWrite
                | Self::IndirectRead
                | Self::HostRead
        )
    }
}

/// Size of a transient image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageSize {
    /// The swapchain extent. The R6 frame has no swapchain-sized transient (the scene and
    /// post images are render-sized and the swapchain itself is imported); kept for passes
    /// that work at native resolution.
    #[allow(dead_code)]
    Swapchain,
    /// The render allocation (the swapchain extent times the maximum render scale,
    /// ADR-0015) divided by `2^shift` (at least 1×1): the scene targets (`shift` 0), bloom
    /// mips and half-resolution AO.
    Render { shift: u8 },
    /// A fixed extent (for example the shadow map), independent of the window.
    Fixed { width: u32, height: u32 },
}

/// Description of a transient image; transients alias only when these are equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageDesc {
    pub(crate) format: vk::Format,
    pub(crate) aspect: vk::ImageAspectFlags,
    pub(crate) size: ImageSize,
    /// Samples per pixel (`TYPE_1` except the MSAA scene attachments).
    pub(crate) samples: vk::SampleCountFlags,
    /// Usage beyond what the graph's accesses imply (for example `SAMPLED` on a depth
    /// buffer that later passes will read).
    pub(crate) extra_usage: vk::ImageUsageFlags,
}

/// Synchronisation state of an imported resource when the graph starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImportState {
    /// Stages that last touched the resource (the first barrier's source stages).
    pub(crate) stage: vk::PipelineStageFlags2,
    /// Writes that must be made available (the first barrier's source access).
    pub(crate) access: vk::AccessFlags2,
    /// Current layout; `UNDEFINED` discards the contents. Ignored for buffers.
    pub(crate) layout: vk::ImageLayout,
}

impl ImportState {
    /// Contents are discarded; the first use waits for `stage` (for a swapchain image this
    /// is the stage the acquire semaphore is waited on).
    pub(crate) const fn discard(stage: vk::PipelineStageFlags2) -> Self {
        Self {
            stage,
            access: vk::AccessFlags2::NONE,
            layout: vk::ImageLayout::UNDEFINED,
        }
    }

    /// Written by the host before submission.
    pub(crate) const fn host_written() -> Self {
        Self {
            stage: vk::PipelineStageFlags2::HOST,
            access: vk::AccessFlags2::HOST_WRITE,
            layout: vk::ImageLayout::UNDEFINED,
        }
    }

    /// No outstanding GPU work (the host waited for the device).
    pub(crate) const fn idle() -> Self {
        Self::discard(vk::PipelineStageFlags2::NONE)
    }

    /// Contents kept across frames in `layout`, last touched at `stage` in an earlier frame
    /// with every write already made visible there (R7b's offscreen targets: written by
    /// their pass, sampled by the UI, and back in `SHADER_READ_ONLY_OPTIMAL` at the end of
    /// every frame). A write in this frame waits for `stage` (write-after-read); a read in
    /// the same layout only gets an execution dependency.
    pub(crate) const fn persistent(
        stage: vk::PipelineStageFlags2,
        layout: vk::ImageLayout,
    ) -> Self {
        Self {
            stage,
            access: vk::AccessFlags2::NONE,
            layout,
        }
    }

    /// Contents discarded on first use, but last written at `stage` with `access` in an
    /// earlier frame (a persistent depth buffer): the first barrier orders that write
    /// before this frame's (write-after-write).
    pub(crate) const fn rewritten(
        stage: vk::PipelineStageFlags2,
        access: vk::AccessFlags2,
    ) -> Self {
        Self {
            stage,
            access,
            layout: vk::ImageLayout::UNDEFINED,
        }
    }

    /// A buffer whose contents persist across frames on this queue: the first barrier of a
    /// frame orders against the previous frames' `stage` work and makes their `access`
    /// writes available (the particle state, written by compute and read by the vertex
    /// stage every frame, ADR-0013).
    pub(crate) const fn persistent_buffer(
        stage: vk::PipelineStageFlags2,
        access: vk::AccessFlags2,
    ) -> Self {
        Self {
            stage,
            access,
            layout: vk::ImageLayout::UNDEFINED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Transient(ImageDesc),
    ImportedImage,
    ImportedBuffer,
}

#[derive(Clone, Copy, Debug)]
struct ResourceDecl {
    name: &'static str,
    kind: Kind,
    initial: ImportState,
    final_access: Option<Access>,
}

struct PassDecl<T> {
    name: &'static str,
    tag: T,
    accesses: Vec<(ResourceId, Access)>,
    side_effect: bool,
}

/// Why a graph failed to compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GraphError {
    /// A pass names a resource that was not declared on this builder.
    UnknownResource { pass: &'static str },
    /// The access does not apply to the resource kind (for example `Present` on a buffer).
    AccessMismatch {
        pass: &'static str,
        resource: &'static str,
    },
    /// A pass names the same resource twice.
    DuplicateAccess {
        pass: &'static str,
        resource: &'static str,
    },
    /// A live pass (or a final access) reads a transient, or an imported image whose start
    /// layout is `UNDEFINED` (discarded contents, such as the swapchain), that no earlier live
    /// pass wrote.
    ReadBeforeWrite {
        pass: &'static str,
        resource: &'static str,
    },
    /// More than [`MAX_PASSES`] passes, [`MAX_RESOURCES`] resources or [`MAX_BARRIERS`]
    /// barriers at one point.
    TooLarge(&'static str),
}

impl core::fmt::Display for GraphError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownResource { pass } => write!(f, "pass {pass} uses an unknown resource"),
            Self::AccessMismatch { pass, resource } => {
                write!(
                    f,
                    "pass {pass} uses {resource} with an access of the wrong kind"
                )
            }
            Self::DuplicateAccess { pass, resource } => {
                write!(f, "pass {pass} declares {resource} twice")
            }
            Self::ReadBeforeWrite { pass, resource } => {
                write!(f, "pass {pass} reads {resource} before any pass writes it")
            }
            Self::TooLarge(what) => write!(f, "render graph has too many {what}"),
        }
    }
}

/// Declares resources and passes in execution order.
pub(crate) struct GraphBuilder<T> {
    resources: Vec<ResourceDecl>,
    passes: Vec<PassDecl<T>>,
}

/// Adds accesses to one pass; see [`GraphBuilder::pass`].
pub(crate) struct PassBuilder<'a, T> {
    pass: &'a mut PassDecl<T>,
}

impl<T> PassBuilder<'_, T> {
    /// Declares that the pass uses `resource` as `access`.
    pub(crate) fn access(self, resource: ResourceId, access: Access) -> Self {
        self.pass.accesses.push((resource, access));
        self
    }

    /// Keeps the pass even if nothing consumes its outputs (debug or readback passes).
    #[allow(dead_code)] // Part of the graph contract; the engine frame has none yet.
    pub(crate) fn side_effect(self) -> Self {
        self.pass.side_effect = true;
        self
    }
}

impl<T: Copy> GraphBuilder<T> {
    pub(crate) fn new() -> Self {
        Self {
            resources: Vec::with_capacity(MAX_RESOURCES),
            passes: Vec::with_capacity(MAX_PASSES),
        }
    }

    fn declare(
        &mut self,
        name: &'static str,
        kind: Kind,
        initial: ImportState,
        final_access: Option<Access>,
    ) -> ResourceId {
        let id = ResourceId(u16::try_from(self.resources.len()).unwrap_or(u16::MAX));
        self.resources.push(ResourceDecl {
            name,
            kind,
            initial,
            final_access,
        });
        id
    }

    /// A graph-owned image that lives for one frame; its contents never survive the frame.
    pub(crate) fn transient_image(&mut self, name: &'static str, desc: ImageDesc) -> ResourceId {
        self.declare(name, Kind::Transient(desc), ImportState::idle(), None)
    }

    /// An image owned outside the graph (for example the acquired swapchain image).
    /// `final_access`, if any, is applied after the last pass.
    pub(crate) fn import_image(
        &mut self,
        name: &'static str,
        initial: ImportState,
        final_access: Option<Access>,
    ) -> ResourceId {
        self.declare(name, Kind::ImportedImage, initial, final_access)
    }

    /// A buffer owned outside the graph.
    pub(crate) fn import_buffer(
        &mut self,
        name: &'static str,
        initial: ImportState,
        final_access: Option<Access>,
    ) -> ResourceId {
        self.declare(name, Kind::ImportedBuffer, initial, final_access)
    }

    /// Appends a pass; passes execute in declaration order.
    pub(crate) fn pass(&mut self, name: &'static str, tag: T) -> PassBuilder<'_, T> {
        self.passes.push(PassDecl {
            name,
            tag,
            accesses: Vec::with_capacity(4),
            side_effect: false,
        });
        let pass = self.passes.last_mut().expect("pass was just pushed");
        PassBuilder { pass }
    }

    /// Culls, assigns transient slots and derives the barrier plan.
    ///
    /// # Errors
    /// See [`GraphError`].
    pub(crate) fn compile(self) -> Result<CompiledGraph<T>, GraphError> {
        compile(self)
    }
}

/// One `VkImageMemoryBarrier2`/`VkBufferMemoryBarrier2`, without handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Barrier {
    pub(crate) resource: ResourceId,
    pub(crate) src_stage: vk::PipelineStageFlags2,
    pub(crate) src_access: vk::AccessFlags2,
    pub(crate) dst_stage: vk::PipelineStageFlags2,
    pub(crate) dst_access: vk::AccessFlags2,
    /// Image layouts; both `UNDEFINED` for buffers.
    pub(crate) old_layout: vk::ImageLayout,
    pub(crate) new_layout: vk::ImageLayout,
    pub(crate) image: bool,
}

/// A live pass and the barriers recorded before it.
#[derive(Debug)]
pub(crate) struct CompiledPass<T> {
    pub(crate) name: &'static str,
    pub(crate) tag: T,
    pub(crate) barriers: Vec<Barrier>,
    /// Bit `r` is set when the pass accesses resource `r` (`MAX_RESOURCES` <= 64).
    uses: u64,
}

impl<T> CompiledPass<T> {
    /// Whether the pass accesses `resource`.
    pub(crate) fn uses(&self, resource: ResourceId) -> bool {
        resource.index() < MAX_RESOURCES && self.uses & (1 << resource.index()) != 0
    }
}

/// A physical transient image shared by one or more transient resources.
///
/// Equality covers the cross-frame start state too: two compiled variants that the renderer
/// alternates between (the normal and the capture frame) may share physical images only if
/// every slot also starts from the same `(stage, access)`, because a slot's first barrier
/// orders against whatever the *previous* frame's variant did with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PhysicalImage {
    pub(crate) desc: ImageDesc,
    /// Union of every aliased resource's usage plus `extra_usage`.
    pub(crate) usage: vk::ImageUsageFlags,
    /// Every stage that touches the slot during a frame: the source stages of the slot's
    /// first barrier in the next frame.
    pub(crate) start_stage: vk::PipelineStageFlags2,
    /// Every write to the slot during a frame: the source access of that first barrier.
    pub(crate) start_access: vk::AccessFlags2,
}

/// The executable plan produced by [`GraphBuilder::compile`].
#[derive(Debug)]
pub(crate) struct CompiledGraph<T> {
    passes: Vec<CompiledPass<T>>,
    final_barriers: Vec<Barrier>,
    /// Per resource: its physical slot if it is a live transient.
    slot_of: Vec<Option<u16>>,
    slots: Vec<PhysicalImage>,
    culled: Vec<&'static str>,
}

impl<T> CompiledGraph<T> {
    /// Live passes in execution order.
    pub(crate) fn passes(&self) -> &[CompiledPass<T>] {
        &self.passes
    }

    /// Index of the first live pass that accesses `resource`, or the pass count if none
    /// does. The renderer submits the passes before it without waiting on the acquire
    /// semaphore when `resource` is the swapchain image.
    pub(crate) fn first_use(&self, resource: ResourceId) -> usize {
        self.passes
            .iter()
            .position(|pass| pass.uses(resource))
            .unwrap_or(self.passes.len())
    }

    /// Barriers recorded after the last pass (final transitions of imported resources).
    pub(crate) fn final_barriers(&self) -> &[Barrier] {
        &self.final_barriers
    }

    /// Physical transient images to allocate, with each slot's cross-frame start state.
    pub(crate) fn slots(&self) -> &[PhysicalImage] {
        &self.slots
    }

    /// Whether `other` can run on the same physical transients, in any interleaving with
    /// this graph: the same slots, descriptions and usages, and the same per-slot start
    /// `(stage, access)` (see [`PhysicalImage`]).
    pub(crate) fn shares_transients_with<U>(&self, other: &CompiledGraph<U>) -> bool {
        self.slots == other.slots
    }

    /// Physical slot of a transient resource, or `None` for imports and culled transients.
    pub(crate) fn slot_of(&self, resource: ResourceId) -> Option<usize> {
        self.slot_of
            .get(resource.index())
            .copied()
            .flatten()
            .map(usize::from)
    }

    /// Names of the culled passes.
    pub(crate) fn culled(&self) -> &[&'static str] {
        &self.culled
    }

    /// Number of declared resources (the size of a binding table).
    pub(crate) fn resource_count(&self) -> usize {
        self.slot_of.len()
    }
}

/// Distinct read (stage, access) pairs tracked per write; see `Track::visible`.
const MAX_VISIBLE: usize = 4;

/// Hazard-tracking state of one physical resource (an import or a transient slot).
#[derive(Clone, Copy, Debug)]
struct Track {
    layout: vk::ImageLayout,
    /// Last write (stage, write access) not yet followed by a full barrier to a writer.
    write: Option<(vk::PipelineStageFlags2, vk::AccessFlags2)>,
    /// (stage, access) pairs the last write has been made visible to, one per barrier. A
    /// union of stages and a union of accesses would wrongly cover a pair that no barrier
    /// named. When full, later reads always get a barrier (conservative).
    visible: [(vk::PipelineStageFlags2, vk::AccessFlags2); MAX_VISIBLE],
    visible_len: usize,
    /// Stages that read since the last write or layout transition.
    readers: vk::PipelineStageFlags2,
    image: bool,
}

impl Track {
    fn new(initial: ImportState, image: bool) -> Self {
        Self {
            layout: initial.layout,
            write: Some((initial.stage, initial.access)),
            visible: [(vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE); MAX_VISIBLE],
            visible_len: 0,
            readers: vk::PipelineStageFlags2::NONE,
            image,
        }
    }

    /// Whether one earlier barrier already made the last write visible to this read.
    fn is_visible(&self, stage: vk::PipelineStageFlags2, access: vk::AccessFlags2) -> bool {
        self.visible[..self.visible_len]
            .iter()
            .any(|&(seen_stage, seen_access)| {
                seen_stage.contains(stage) && seen_access.contains(access)
            })
    }

    fn set_visible(&mut self, pairs: &[(vk::PipelineStageFlags2, vk::AccessFlags2)]) {
        self.visible_len = 0;
        for &(stage, access) in pairs {
            self.mark_visible(stage, access);
        }
    }

    fn mark_visible(&mut self, stage: vk::PipelineStageFlags2, access: vk::AccessFlags2) {
        if self.visible_len < MAX_VISIBLE {
            self.visible[self.visible_len] = (stage, access);
            self.visible_len += 1;
        }
    }

    /// Applies `access` to this track and returns the barrier it needs, if any.
    fn apply(&mut self, resource: ResourceId, access: Access) -> Option<Barrier> {
        let info = access.info();
        let layout_change = self.image && self.layout != info.layout;
        if !info.writes() && !layout_change {
            // Read in the current layout: only a pending write needs a barrier, once per
            // destination stage/access.
            self.readers |= info.stage;
            let (stage, access_mask) = self.write?;
            if self.is_visible(info.stage, info.access) {
                return None;
            }
            self.mark_visible(info.stage, info.access);
            return Some(Barrier {
                resource,
                src_stage: stage,
                src_access: access_mask,
                dst_stage: info.stage,
                dst_access: info.access,
                old_layout: self.layout_or_undefined(),
                new_layout: self.layout_or_undefined(),
                image: self.image,
            });
        }
        // A write or a layout transition. Readers since the last write only need an
        // execution dependency (write-after-read); otherwise the pending write must be made
        // available (write-after-write / read-after-write with a transition).
        let (src_stage, src_access) = if self.readers.is_empty() {
            self.write
                .unwrap_or((vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE))
        } else {
            (self.readers, vk::AccessFlags2::NONE)
        };
        let old_layout = self.layout_or_undefined();
        let new_layout = if self.image {
            info.layout
        } else {
            vk::ImageLayout::UNDEFINED
        };
        self.layout = new_layout;
        if info.writes() {
            self.write = Some((info.stage, info.write_access));
            self.set_visible(&[]);
            self.readers = vk::PipelineStageFlags2::NONE;
        } else {
            // The transition is visible to this reader; later readers in other stages chain
            // from it.
            self.write = Some((info.stage, vk::AccessFlags2::NONE));
            self.set_visible(&[(info.stage, info.access)]);
            self.readers = info.stage;
        }
        Some(Barrier {
            resource,
            src_stage,
            src_access,
            dst_stage: info.stage,
            dst_access: info.access,
            old_layout,
            new_layout,
            image: self.image,
        })
    }

    fn layout_or_undefined(&self) -> vk::ImageLayout {
        if self.image {
            self.layout
        } else {
            vk::ImageLayout::UNDEFINED
        }
    }
}

#[allow(clippy::too_many_lines)] // One ordered sequence: validate, cull, alias, derive.
fn compile<T: Copy>(builder: GraphBuilder<T>) -> Result<CompiledGraph<T>, GraphError> {
    let GraphBuilder { resources, passes } = builder;
    if passes.len() > MAX_PASSES {
        return Err(GraphError::TooLarge("passes"));
    }
    if resources.len() > MAX_RESOURCES {
        return Err(GraphError::TooLarge("resources"));
    }

    // 1. Validate every access.
    for pass in &passes {
        for (index, &(resource, access)) in pass.accesses.iter().enumerate() {
            let decl = resources
                .get(resource.index())
                .ok_or(GraphError::UnknownResource { pass: pass.name })?;
            let valid = match decl.kind {
                Kind::Transient(_) | Kind::ImportedImage => access.valid_for_image(),
                Kind::ImportedBuffer => access.valid_for_buffer(),
            };
            if !valid {
                return Err(GraphError::AccessMismatch {
                    pass: pass.name,
                    resource: decl.name,
                });
            }
            if pass.accesses[..index]
                .iter()
                .any(|&(other, _)| other == resource)
            {
                return Err(GraphError::DuplicateAccess {
                    pass: pass.name,
                    resource: decl.name,
                });
            }
        }
    }
    for decl in &resources {
        if let Some(access) = decl.final_access {
            let valid = match decl.kind {
                Kind::Transient(_) => false,
                Kind::ImportedImage => access.valid_for_image(),
                Kind::ImportedBuffer => access.valid_for_buffer(),
            };
            if !valid {
                return Err(GraphError::AccessMismatch {
                    pass: "<final>",
                    resource: decl.name,
                });
            }
        }
    }

    // 2. Cull, walking backwards: a pass is live if it has side effects or writes contents
    // that a later live pass (or a final access) needs.
    let mut needed: Vec<bool> = resources
        .iter()
        .map(|decl| {
            decl.final_access
                .is_some_and(|access| access.info().reads_previous)
        })
        .collect();
    let mut live = vec![false; passes.len()];
    for (index, pass) in passes.iter().enumerate().rev() {
        let produces_needed = pass
            .accesses
            .iter()
            .any(|&(resource, access)| access.info().writes() && needed[resource.index()]);
        if !(pass.side_effect || produces_needed) {
            continue;
        }
        live[index] = true;
        for &(resource, access) in &pass.accesses {
            if access.info().writes() {
                needed[resource.index()] = false;
            }
        }
        for &(resource, access) in &pass.accesses {
            if access.info().reads_previous {
                needed[resource.index()] = true;
            }
        }
    }

    // 3. Transient lifetimes over live passes, and read-before-write checks. Transients and
    // imported images that start with discarded contents (`UNDEFINED`, such as the acquired
    // swapchain image) must be written before anything, including a final access, reads them.
    let discards = |decl: &ResourceDecl| match decl.kind {
        Kind::Transient(_) => true,
        Kind::ImportedImage => decl.initial.layout == vk::ImageLayout::UNDEFINED,
        Kind::ImportedBuffer => false,
    };
    let mut first_use: Vec<Option<usize>> = vec![None; resources.len()];
    let mut last_use: Vec<usize> = vec![0; resources.len()];
    let mut written = vec![false; resources.len()];
    let mut order = 0usize;
    for (index, pass) in passes.iter().enumerate() {
        if !live[index] {
            continue;
        }
        for &(resource, access) in &pass.accesses {
            let decl = &resources[resource.index()];
            let info = access.info();
            if discards(decl) && info.reads_previous && !written[resource.index()] {
                return Err(GraphError::ReadBeforeWrite {
                    pass: pass.name,
                    resource: decl.name,
                });
            }
            if info.writes() {
                written[resource.index()] = true;
            }
            if !matches!(decl.kind, Kind::Transient(_)) {
                continue;
            }
            first_use[resource.index()].get_or_insert(order);
            last_use[resource.index()] = order;
        }
        order += 1;
    }
    for (index, decl) in resources.iter().enumerate() {
        if let Some(access) = decl.final_access
            && discards(decl)
            && access.info().reads_previous
            && !written[index]
        {
            return Err(GraphError::ReadBeforeWrite {
                pass: "<final>",
                resource: decl.name,
            });
        }
    }

    // 4. Assign physical slots greedily in first-use order; a slot is reused when its
    // description matches and its last user finished before this resource's first use.
    let mut by_first: Vec<usize> = (0..resources.len())
        .filter(|&index| first_use[index].is_some())
        .collect();
    by_first.sort_by_key(|&index| (first_use[index], index));
    let mut slots: Vec<PhysicalImage> = Vec::new();
    let mut slot_last: Vec<usize> = Vec::new();
    let mut slot_of: Vec<Option<u16>> = vec![None; resources.len()];
    for index in by_first {
        let Kind::Transient(desc) = resources[index].kind else {
            continue;
        };
        let first = first_use[index].expect("filtered above");
        let reuse =
            (0..slots.len()).find(|&slot| slots[slot].desc == desc && slot_last[slot] < first);
        let slot = reuse.unwrap_or_else(|| {
            slots.push(PhysicalImage {
                desc,
                usage: desc.extra_usage,
                start_stage: vk::PipelineStageFlags2::NONE,
                start_access: vk::AccessFlags2::NONE,
            });
            slot_last.push(0);
            slots.len() - 1
        });
        slot_last[slot] = last_use[index];
        slot_of[index] = Some(u16::try_from(slot).map_err(|_| GraphError::TooLarge("slots"))?);
    }
    // Usage and the cross-frame start state of each slot: every stage that touches the slot
    // and every write to it, so the first barrier of the next frame orders against this
    // frame's work on the same queue.
    for (index, pass) in passes.iter().enumerate() {
        if !live[index] {
            continue;
        }
        for &(resource, access) in &pass.accesses {
            if let Some(slot) = slot_of[resource.index()] {
                let info = access.info();
                let slot = &mut slots[usize::from(slot)];
                slot.usage |= info.usage;
                slot.start_stage |= info.stage;
                slot.start_access |= info.write_access;
            }
        }
    }

    // 5. Derive barriers. Imports track themselves; transients track their slot.
    let mut tracks: Vec<Track> = resources
        .iter()
        .map(|decl| Track::new(decl.initial, !matches!(decl.kind, Kind::ImportedBuffer)))
        .collect();
    let slot_track_base = tracks.len();
    for slot in &slots {
        tracks.push(Track::new(
            ImportState {
                stage: slot.start_stage,
                access: slot.start_access,
                layout: vk::ImageLayout::UNDEFINED,
            },
            true,
        ));
    }
    let track_of = |resource: ResourceId| {
        slot_of[resource.index()]
            .map_or(resource.index(), |slot| slot_track_base + usize::from(slot))
    };
    let mut started = vec![false; resources.len()];
    let mut compiled = Vec::with_capacity(passes.len());
    let mut culled = Vec::new();
    for (index, pass) in passes.into_iter().enumerate() {
        if !live[index] {
            culled.push(pass.name);
            continue;
        }
        let mut barriers = Vec::with_capacity(pass.accesses.len());
        for &(resource, access) in &pass.accesses {
            let track = &mut tracks[track_of(resource)];
            if slot_of[resource.index()].is_some() && !started[resource.index()] {
                // A transient (or a new alias of a slot) starts with discarded contents.
                started[resource.index()] = true;
                track.layout = vk::ImageLayout::UNDEFINED;
            }
            if let Some(barrier) = track.apply(resource, access) {
                barriers.push(barrier);
            }
        }
        if barriers.len() > MAX_BARRIERS {
            return Err(GraphError::TooLarge("barriers"));
        }
        let uses = pass
            .accesses
            .iter()
            .fold(0u64, |mask, &(resource, _)| mask | (1 << resource.index()));
        compiled.push(CompiledPass {
            name: pass.name,
            tag: pass.tag,
            barriers,
            uses,
        });
    }
    let mut final_barriers = Vec::new();
    for (index, decl) in resources.iter().enumerate() {
        let Some(access) = decl.final_access else {
            continue;
        };
        let resource = ResourceId(u16::try_from(index).unwrap_or(u16::MAX));
        if let Some(barrier) = tracks[index].apply(resource, access) {
            final_barriers.push(barrier);
        }
    }
    if final_barriers.len() > MAX_BARRIERS {
        return Err(GraphError::TooLarge("barriers"));
    }

    Ok(CompiledGraph {
        passes: compiled,
        final_barriers,
        slot_of,
        slots,
        culled,
    })
}

#[cfg(test)]
mod tests;
