//! Headless tests of the barrier plan: no Vulkan device is created.

use ash::vk;
use vk::AccessFlags2 as A;
use vk::ImageLayout as L;
use vk::PipelineStageFlags2 as S;

use super::{Access, GraphBuilder, GraphError, ImageDesc, ImageSize, ImportState, Track};

const HDR: ImageDesc = ImageDesc {
    format: vk::Format::R16G16B16A16_SFLOAT,
    aspect: vk::ImageAspectFlags::COLOR,
    size: ImageSize::Swapchain,
    samples: vk::SampleCountFlags::TYPE_1,
    extra_usage: vk::ImageUsageFlags::empty(),
};

const DEPTH: ImageDesc = ImageDesc {
    format: vk::Format::D32_SFLOAT,
    aspect: vk::ImageAspectFlags::DEPTH,
    size: ImageSize::Swapchain,
    samples: vk::SampleCountFlags::TYPE_1,
    extra_usage: vk::ImageUsageFlags::SAMPLED,
};

fn swapchain<T: Copy>(graph: &mut GraphBuilder<T>) -> super::ResourceId {
    graph.import_image(
        "swapchain",
        ImportState::discard(S::COLOR_ATTACHMENT_OUTPUT),
        Some(Access::Present),
    )
}

#[test]
fn read_after_write_transitions_to_sampled_with_exact_masks() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let _ = graph.pass("scene", 0).access(hdr, Access::ColorWrite);
    let _ = graph
        .pass("composite", 1)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    let composite = &plan.passes()[1];
    let read = composite
        .barriers
        .iter()
        .find(|barrier| barrier.resource == hdr)
        .expect("hdr barrier");
    assert_eq!(read.src_stage, S::COLOR_ATTACHMENT_OUTPUT);
    assert_eq!(read.src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(read.dst_stage, S::FRAGMENT_SHADER);
    assert_eq!(read.dst_access, A::SHADER_SAMPLED_READ);
    assert_eq!(read.old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
    assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
    assert!(read.image);
    // The transient's first use discards its contents and orders against the previous
    // frame's use of the same slot (every stage and write that touches it).
    let first = &plan.passes()[0].barriers[0];
    assert_eq!(first.old_layout, L::UNDEFINED);
    assert_eq!(first.new_layout, L::COLOR_ATTACHMENT_OPTIMAL);
    assert_eq!(
        first.src_stage,
        S::COLOR_ATTACHMENT_OUTPUT | S::FRAGMENT_SHADER
    );
    assert_eq!(first.src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(
        plan.slots()[0].usage,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
    );
}

#[test]
fn write_after_read_is_an_execution_dependency_with_a_transition() {
    let mut graph = GraphBuilder::new();
    let image = graph.import_image(
        "history",
        ImportState {
            stage: S::COLOR_ATTACHMENT_OUTPUT,
            access: A::COLOR_ATTACHMENT_WRITE,
            layout: L::COLOR_ATTACHMENT_OPTIMAL,
        },
        Some(Access::Present),
    );
    let _ = graph
        .pass("read", 0)
        .access(image, Access::SampledFragment)
        .side_effect();
    let _ = graph.pass("write", 1).access(image, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    let read = plan.passes()[0].barriers[0];
    assert_eq!(read.src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
    let write = plan.passes()[1].barriers[0];
    assert_eq!(write.src_stage, S::FRAGMENT_SHADER);
    assert_eq!(write.src_access, A::NONE, "WAR needs no memory dependency");
    assert_eq!(write.dst_stage, S::COLOR_ATTACHMENT_OUTPUT);
    assert_eq!(write.dst_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(write.old_layout, L::SHADER_READ_ONLY_OPTIMAL);
    assert_eq!(write.new_layout, L::COLOR_ATTACHMENT_OPTIMAL);
}

#[test]
fn write_after_write_and_repeated_reads() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let bones = graph.import_buffer("bones", ImportState::host_written(), None);
    let _ = graph
        .pass("composite", 0)
        .access(target, Access::ColorWrite)
        .access(bones, Access::VertexStorageRead);
    let _ = graph
        .pass("ui", 1)
        .access(target, Access::ColorReadWrite)
        .access(bones, Access::VertexStorageRead);
    let plan = graph.compile().expect("valid graph");
    let host = plan.passes()[0]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == bones)
        .expect("host write → vertex read");
    assert_eq!((host.src_stage, host.src_access), (S::HOST, A::HOST_WRITE));
    assert_eq!(
        (host.dst_stage, host.dst_access),
        (S::VERTEX_SHADER, A::SHADER_STORAGE_READ)
    );
    assert!(!host.image);
    let ui = &plan.passes()[1].barriers;
    assert_eq!(
        ui.len(),
        1,
        "the second read of the bone buffer needs no barrier"
    );
    assert_eq!(ui[0].src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(
        ui[0].dst_access,
        A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE
    );
    assert_eq!(ui[0].old_layout, ui[0].new_layout);

    // Visibility is tracked per (stage, access) pair: reads at (X, a) and (Y, b) do not cover
    // a later read at (X, b), which no barrier named.
    let mut track = Track::new(ImportState::host_written(), false);
    track.mark_visible(S::FRAGMENT_SHADER, A::SHADER_SAMPLED_READ);
    track.mark_visible(S::VERTEX_SHADER, A::SHADER_STORAGE_READ);
    assert!(track.is_visible(S::VERTEX_SHADER, A::SHADER_STORAGE_READ));
    assert!(!track.is_visible(S::FRAGMENT_SHADER, A::SHADER_STORAGE_READ));
}

#[test]
fn transients_with_disjoint_lifetimes_share_a_slot() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let first = graph.transient_image("first", HDR);
    let second = graph.transient_image("second", HDR);
    let third = graph.transient_image("third", HDR);
    let depth = graph.transient_image("depth", DEPTH);
    let _ = graph
        .pass("a", 0)
        .access(first, Access::ColorWrite)
        .access(depth, Access::DepthWrite);
    let _ = graph
        .pass("b", 1)
        .access(first, Access::SampledFragment)
        .access(second, Access::ColorWrite);
    let _ = graph
        .pass("c", 2)
        .access(second, Access::SampledFragment)
        .access(third, Access::ColorWrite);
    let _ = graph
        .pass("d", 3)
        .access(third, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    assert_eq!(
        plan.slot_of(first),
        plan.slot_of(third),
        "first ends before third"
    );
    assert_ne!(
        plan.slot_of(first),
        plan.slot_of(second),
        "lifetimes overlap in b"
    );
    assert_ne!(
        plan.slot_of(depth),
        plan.slot_of(first),
        "descriptions differ"
    );
    assert_eq!(plan.slot_of(target), None, "imports have no slot");
    assert_eq!(plan.slots().len(), 3);
    // The alias starts by discarding the slot after its previous user's sampled read.
    let alias = plan.passes()[2]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == third)
        .expect("alias barrier");
    assert_eq!(alias.old_layout, L::UNDEFINED);
    assert_eq!(alias.src_stage, S::FRAGMENT_SHADER);
    assert_eq!(alias.src_access, A::NONE);
    assert_eq!(alias.new_layout, L::COLOR_ATTACHMENT_OPTIMAL);
    let depth_slot = plan.slot_of(depth).expect("depth slot");
    assert_eq!(
        plan.slots()[depth_slot].usage,
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
    );
}

#[test]
fn passes_with_unused_outputs_are_culled() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let debug = graph.transient_image("debug", HDR);
    let _ = graph.pass("scene", 0).access(hdr, Access::ColorWrite);
    let _ = graph
        .pass("debug-view", 1)
        .access(hdr, Access::SampledFragment)
        .access(debug, Access::ColorWrite);
    // Overwritten by the composite before anything reads it.
    let _ = graph.pass("clear", 2).access(target, Access::ColorWrite);
    let _ = graph
        .pass("composite", 3)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    let names: Vec<_> = plan.passes().iter().map(|pass| pass.name).collect();
    assert_eq!(names, ["scene", "composite"]);
    assert_eq!(plan.culled(), ["debug-view", "clear"]);
    assert_eq!(plan.slot_of(debug), None, "culled transients get no memory");
    assert_eq!(plan.slots().len(), 1);
}

#[test]
fn swapchain_is_transitioned_for_present_after_the_last_writer() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let _ = graph.pass("ui", 0).access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    let acquire = plan.passes()[0].barriers[0];
    assert_eq!(
        acquire.src_stage,
        S::COLOR_ATTACHMENT_OUTPUT,
        "chains with acquire"
    );
    assert_eq!(acquire.old_layout, L::UNDEFINED);
    assert_eq!(acquire.new_layout, L::COLOR_ATTACHMENT_OPTIMAL);
    let present = plan.final_barriers();
    assert_eq!(present.len(), 1);
    assert_eq!(present[0].resource, target);
    assert_eq!(present[0].src_stage, S::COLOR_ATTACHMENT_OUTPUT);
    assert_eq!(present[0].src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(
        present[0].dst_stage,
        S::ALL_COMMANDS,
        "before the semaphore signal"
    );
    assert_eq!(present[0].dst_access, A::NONE);
    assert_eq!(present[0].old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
    assert_eq!(present[0].new_layout, L::PRESENT_SRC_KHR);
}

#[test]
fn invalid_graphs_are_rejected() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let _ = graph
        .pass("composite", 0)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    assert_eq!(
        graph.compile().unwrap_err(),
        GraphError::ReadBeforeWrite {
            pass: "composite",
            resource: "hdr"
        }
    );

    let mut graph = GraphBuilder::new();
    let buffer = graph.import_buffer("bones", ImportState::host_written(), None);
    let _ = graph
        .pass("bad", 0)
        .access(buffer, Access::ColorWrite)
        .side_effect();
    assert!(matches!(
        graph.compile(),
        Err(GraphError::AccessMismatch { .. })
    ));

    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let _ = graph
        .pass("twice", 0)
        .access(target, Access::ColorWrite)
        .access(target, Access::TransferRead);
    assert!(matches!(
        graph.compile(),
        Err(GraphError::DuplicateAccess { .. })
    ));
}

#[test]
fn discarded_imports_must_be_written_before_they_are_read() {
    // The swapchain starts UNDEFINED: blending over it (or presenting it) before any write
    // reads garbage, so it is rejected like a transient.
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let _ = graph.pass("ui", 0).access(target, Access::ColorReadWrite);
    assert_eq!(
        graph.compile().unwrap_err(),
        GraphError::ReadBeforeWrite {
            pass: "ui",
            resource: "swapchain"
        }
    );

    let mut graph = GraphBuilder::<u8>::new();
    let _ = swapchain(&mut graph);
    assert_eq!(
        graph.compile().unwrap_err(),
        GraphError::ReadBeforeWrite {
            pass: "<final>",
            resource: "swapchain"
        }
    );

    // An import that keeps its contents (a defined layout) may be read first.
    let mut graph = GraphBuilder::new();
    let history = graph.import_image(
        "history",
        ImportState {
            stage: S::COLOR_ATTACHMENT_OUTPUT,
            access: A::COLOR_ATTACHMENT_WRITE,
            layout: L::COLOR_ATTACHMENT_OPTIMAL,
        },
        None,
    );
    let _ = graph
        .pass("blend", 0)
        .access(history, Access::ColorReadWrite)
        .side_effect();
    assert!(graph.compile().is_ok());
}

#[test]
fn variants_share_transients_only_with_the_same_start_state() {
    let build = |capture: bool| {
        let mut graph = GraphBuilder::new();
        let target = swapchain(&mut graph);
        let hdr = graph.transient_image("hdr", HDR);
        let _ = graph.pass("scene", 0).access(hdr, Access::ColorWrite);
        let _ = graph
            .pass("composite", 1)
            .access(hdr, Access::SampledFragment)
            .access(target, Access::ColorWrite);
        if capture {
            // Extra work on an import only: the transient slots must still agree.
            let _ = graph.pass("copy", 2).access(target, Access::TransferRead);
        }
        graph.compile().expect("valid graph")
    };
    let normal = build(false);
    let capture = build(true);
    assert!(normal.shares_transients_with(&capture));
    assert_eq!(
        normal.slots()[0].start_stage,
        S::COLOR_ATTACHMENT_OUTPUT | S::FRAGMENT_SHADER
    );
    assert_eq!(normal.slots()[0].start_access, A::COLOR_ATTACHMENT_WRITE);

    // Same description and usage, but the other variant touches the slot in a stage this one
    // does not: its first barrier would not order against that work, so they must not share.
    let mut other = build(false);
    other.slots[0].start_stage |= S::VERTEX_SHADER;
    assert_eq!(normal.slots()[0].usage, other.slots()[0].usage);
    assert!(!normal.shares_transients_with(&other));
    let mut other = build(false);
    other.slots[0].start_access = A::NONE;
    assert!(!normal.shares_transients_with(&other));
}

/// R7b: a persistent offscreen target (colour + depth imports) that a pass renders into and
/// the UI samples, with and without the writer in the frame.
#[test]
#[allow(clippy::too_many_lines)] // Both variants' full barrier plan, checked together.
fn persistent_offscreen_targets_are_written_then_sampled_by_the_ui() {
    let build = |with_writer: bool| {
        let mut graph = GraphBuilder::new();
        let target = swapchain(&mut graph);
        let portrait = graph.import_image(
            "portrait",
            ImportState::persistent(S::FRAGMENT_SHADER, L::SHADER_READ_ONLY_OPTIMAL),
            Some(Access::SampledFragment),
        );
        let depth = graph.import_image(
            "portrait-depth",
            ImportState::rewritten(
                S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS,
                A::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
            None,
        );
        if with_writer {
            let _ = graph
                .pass("portraits", 0)
                .access(portrait, Access::ColorWrite)
                .access(depth, Access::DepthWrite);
        }
        let _ = graph
            .pass("composite", 1)
            .access(target, Access::ColorWrite);
        let _ = graph
            .pass("ui", 2)
            .access(target, Access::ColorReadWrite)
            .access(portrait, Access::SampledFragment);
        (graph.compile().expect("valid graph"), portrait, depth)
    };

    let (plan, portrait, depth) = build(true);
    assert_eq!(
        plan.culled(),
        [] as [&str; 0],
        "the UI read keeps the writer"
    );
    assert!(plan.slots().is_empty(), "imports take no transient slots");
    let write = &plan.passes()[0].barriers;
    let colour = write
        .iter()
        .find(|barrier| barrier.resource == portrait)
        .expect("colour barrier");
    // Write-after-read against the previous frame's UI sampling, keeping the contents.
    assert_eq!(
        (colour.src_stage, colour.src_access),
        (S::FRAGMENT_SHADER, A::NONE)
    );
    assert_eq!(
        (colour.dst_stage, colour.dst_access),
        (S::COLOR_ATTACHMENT_OUTPUT, A::COLOR_ATTACHMENT_WRITE)
    );
    assert_eq!(
        (colour.old_layout, colour.new_layout),
        (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL)
    );
    let depth_barrier = write
        .iter()
        .find(|barrier| barrier.resource == depth)
        .expect("depth barrier");
    // Write-after-write against the previous use, discarding the contents.
    assert_eq!(depth_barrier.src_access, A::DEPTH_STENCIL_ATTACHMENT_WRITE);
    assert_eq!(depth_barrier.old_layout, L::UNDEFINED);
    assert_eq!(
        depth_barrier.new_layout,
        L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
    );
    // The UI waits for the colour writes and samples in SHADER_READ_ONLY_OPTIMAL.
    let ui = plan.passes()[2]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == portrait)
        .expect("ui barrier");
    assert_eq!(
        (ui.src_stage, ui.src_access),
        (S::COLOR_ATTACHMENT_OUTPUT, A::COLOR_ATTACHMENT_WRITE)
    );
    assert_eq!(
        (ui.dst_stage, ui.dst_access),
        (S::FRAGMENT_SHADER, A::SHADER_SAMPLED_READ)
    );
    assert_eq!(
        (ui.old_layout, ui.new_layout),
        (L::COLOR_ATTACHMENT_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL)
    );
    // The frame ends in the import's start layout: no final transition is needed.
    assert!(
        plan.final_barriers()
            .iter()
            .all(|barrier| barrier.resource != portrait)
    );

    // Without the writer the UI reads last frame's contents: same layout, only an
    // execution dependency, and still no final transition.
    let (plan, portrait, _) = build(false);
    assert_eq!(plan.passes().len(), 2);
    let ui = plan.passes()[1]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == portrait)
        .expect("ui barrier");
    assert_eq!(ui.src_access, A::NONE);
    assert_eq!(ui.old_layout, ui.new_layout);
    assert_eq!(ui.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
    assert!(
        plan.final_barriers()
            .iter()
            .all(|barrier| barrier.resource != portrait)
    );
}

#[test]
fn compute_writes_reach_vertex_and_indirect_reads_with_exact_masks() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let persistent = ImportState::persistent_buffer(
        S::COMPUTE_SHADER | S::VERTEX_SHADER | S::DRAW_INDIRECT | S::TRANSFER,
        A::SHADER_STORAGE_WRITE,
    );
    let particles = graph.import_buffer("particles", persistent, None);
    let args = graph.import_buffer("args", persistent, None);
    let spawns = graph.import_buffer("spawns", ImportState::host_written(), None);
    let _ = graph
        .pass("simulate", 0)
        .access(spawns, Access::ComputeStorageRead)
        .access(particles, Access::ComputeStorageReadWrite)
        .access(args, Access::ComputeStorageReadWrite);
    let _ = graph
        .pass("draw", 1)
        .access(hdr, Access::ColorWrite)
        .access(particles, Access::VertexStorageRead)
        .access(args, Access::IndirectRead);
    let _ = graph
        .pass("composite", 2)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    assert_eq!(plan.culled(), [] as [&str; 0]);
    let simulate = &plan.passes()[0].barriers;
    // The first compute access orders against every earlier frame's use of the buffer.
    let first = simulate
        .iter()
        .find(|barrier| barrier.resource == particles)
        .expect("particle barrier");
    assert_eq!(first.src_stage, persistent.stage);
    assert_eq!(first.src_access, A::SHADER_STORAGE_WRITE);
    assert_eq!(first.dst_stage, S::COMPUTE_SHADER);
    assert_eq!(
        first.dst_access,
        A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE
    );
    assert!(!first.image);
    let host = simulate
        .iter()
        .find(|barrier| barrier.resource == spawns)
        .expect("spawn barrier");
    assert_eq!((host.src_stage, host.src_access), (S::HOST, A::HOST_WRITE));
    assert_eq!(host.dst_access, A::SHADER_STORAGE_READ);
    let draw = &plan.passes()[1].barriers;
    let vertex = draw
        .iter()
        .find(|barrier| barrier.resource == particles)
        .expect("vertex read barrier");
    assert_eq!(
        (vertex.src_stage, vertex.src_access),
        (S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE)
    );
    assert_eq!(
        (vertex.dst_stage, vertex.dst_access),
        (S::VERTEX_SHADER, A::SHADER_STORAGE_READ)
    );
    let indirect = draw
        .iter()
        .find(|barrier| barrier.resource == args)
        .expect("indirect barrier");
    assert_eq!(
        (indirect.src_stage, indirect.src_access),
        (S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE)
    );
    assert_eq!(
        (indirect.dst_stage, indirect.dst_access),
        (S::DRAW_INDIRECT, A::INDIRECT_COMMAND_READ)
    );
}

#[test]
fn depth_is_read_only_and_sampled_after_the_scene_writes_it() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let depth = graph.transient_image("depth", DEPTH);
    let _ = graph
        .pass("scene", 0)
        .access(hdr, Access::ColorWrite)
        .access(depth, Access::DepthWrite);
    let _ = graph
        .pass("particles", 1)
        .access(hdr, Access::ColorReadWrite)
        .access(depth, Access::DepthReadSampled);
    let _ = graph
        .pass("composite", 2)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let plan = graph.compile().expect("valid graph");
    let particles = &plan.passes()[1].barriers;
    let read = particles
        .iter()
        .find(|barrier| barrier.resource == depth)
        .expect("depth barrier");
    assert_eq!(
        read.src_stage,
        S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS
    );
    assert_eq!(read.src_access, A::DEPTH_STENCIL_ATTACHMENT_WRITE);
    assert_eq!(
        read.dst_stage,
        S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS | S::FRAGMENT_SHADER
    );
    assert_eq!(
        read.dst_access,
        A::DEPTH_STENCIL_ATTACHMENT_READ | A::SHADER_SAMPLED_READ
    );
    assert_eq!(read.old_layout, L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    assert_eq!(read.new_layout, L::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
    // HDR: a colour write-after-write between the scene and the particle pass.
    let colour = particles
        .iter()
        .find(|barrier| barrier.resource == hdr)
        .expect("hdr barrier");
    assert_eq!(colour.src_access, A::COLOR_ATTACHMENT_WRITE);
    assert_eq!(
        colour.dst_access,
        A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE
    );
    // The depth slot's next-frame start state covers the sampled read too.
    let slot = plan.slots()[plan.slot_of(depth).expect("live")];
    assert!(slot.start_stage.contains(S::FRAGMENT_SHADER));
    assert!(slot.usage.contains(vk::ImageUsageFlags::SAMPLED));
}

#[test]
fn compute_and_indirect_accesses_are_buffer_only() {
    for access in [
        Access::ComputeStorageRead,
        Access::ComputeStorageReadWrite,
        Access::IndirectRead,
    ] {
        let mut graph = GraphBuilder::<u8>::new();
        let image = graph.import_image("image", ImportState::idle(), None);
        let _ = graph.pass("bad", 0).access(image, access).side_effect();
        assert!(matches!(
            graph.compile(),
            Err(GraphError::AccessMismatch { .. })
        ));
    }
    let mut graph = GraphBuilder::<u8>::new();
    let buffer = graph.import_buffer("buffer", ImportState::idle(), None);
    let _ = graph
        .pass("bad", 0)
        .access(buffer, Access::DepthReadSampled)
        .side_effect();
    assert!(matches!(
        graph.compile(),
        Err(GraphError::AccessMismatch { .. })
    ));
}

#[test]
fn prepass_depth_is_sampled_then_loaded_and_written_by_the_scene() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let hdr = graph.transient_image("hdr", HDR);
    let depth = graph.transient_image("depth", DEPTH);
    let ao = graph.transient_image(
        "ao",
        ImageDesc {
            format: vk::Format::R8_UNORM,
            size: ImageSize::Render { shift: 1 },
            ..HDR
        },
    );
    let _ = graph.pass("prepass", 0).access(depth, Access::DepthWrite);
    let _ = graph
        .pass("ao", 1)
        .access(depth, Access::SampledFragment)
        .access(ao, Access::ColorWrite);
    let _ = graph
        .pass("scene", 2)
        .access(hdr, Access::ColorWrite)
        .access(depth, Access::DepthReadWrite)
        .access(ao, Access::SampledFragment);
    let _ = graph
        .pass("composite", 3)
        .access(hdr, Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let compiled = graph.compile().expect("compiles");
    assert_eq!(compiled.culled(), [] as [&str; 0]);
    let passes = compiled.passes();
    // AO reads the prepass depth: a depth write -> fragment sampled read with a transition.
    let read = passes[1]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == depth)
        .expect("depth barrier before ao");
    assert_eq!(
        (read.old_layout, read.new_layout),
        (
            L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            L::SHADER_READ_ONLY_OPTIMAL
        )
    );
    assert_eq!(read.src_access, A::DEPTH_STENCIL_ATTACHMENT_WRITE);
    // The scene loads it back as an attachment after the read (write-after-read, so no
    // access to make available, but the layout changes back and keeps the contents).
    let load = passes[2]
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
    assert_eq!(load.src_stage, S::FRAGMENT_SHADER);
    assert!(load.dst_access.contains(A::DEPTH_STENCIL_ATTACHMENT_READ));
    // The half-resolution AO image has its own slot.
    assert_eq!(compiled.slots().len(), 3);
}

#[test]
fn a_depth_resolve_is_made_visible_to_a_later_sampled_read() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let msaa = ImageDesc {
        samples: vk::SampleCountFlags::TYPE_4,
        extra_usage: vk::ImageUsageFlags::empty(),
        ..DEPTH
    };
    let msaa_depth = graph.transient_image("msaa-depth", msaa);
    let depth = graph.transient_image("depth", DEPTH);
    let _ = graph
        .pass("scene", 0)
        .access(msaa_depth, Access::DepthWriteResolveSource)
        .access(depth, Access::DepthResolveWrite)
        .access(target, Access::ColorWrite);
    let _ = graph
        .pass("post", 1)
        .access(depth, Access::SampledFragment)
        .access(target, Access::ColorReadWrite);
    // A later writer of the multisampled source waits for the resolve's read of it.
    let _ = graph
        .pass("again", 2)
        .access(msaa_depth, Access::DepthReadWrite)
        .access(target, Access::ColorReadWrite);
    let compiled = graph.compile().expect("compiles");
    // Different sample counts never alias.
    assert_eq!(compiled.slots().len(), 2);
    let read = compiled.passes()[1]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == depth)
        .expect("resolve -> sampled barrier");
    assert!(read.src_stage.contains(S::COLOR_ATTACHMENT_OUTPUT));
    assert!(read.src_access.contains(A::COLOR_ATTACHMENT_WRITE));
    assert_eq!(read.new_layout, L::SHADER_READ_ONLY_OPTIMAL);
    let rewrite = compiled.passes()[2]
        .barriers
        .iter()
        .find(|barrier| barrier.resource == msaa_depth)
        .expect("source -> rewrite barrier");
    assert!(
        rewrite.src_stage.contains(S::COLOR_ATTACHMENT_OUTPUT),
        "{rewrite:?}"
    );
}

#[test]
fn graphs_up_to_the_limits_compile_and_past_them_fail() {
    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    let mut previous = None;
    for index in 0..super::MAX_PASSES - 1 {
        let image = graph.transient_image(
            "chain",
            ImageDesc {
                size: ImageSize::Render {
                    shift: u8::try_from(index % 6).expect("small"),
                },
                ..HDR
            },
        );
        let mut pass = graph.pass("link", index).access(image, Access::ColorWrite);
        if let Some(source) = previous {
            pass = pass.access(source, Access::SampledFragment);
        }
        let _ = pass;
        previous = Some(image);
    }
    let _ = graph
        .pass("last", super::MAX_PASSES)
        .access(previous.expect("chain"), Access::SampledFragment)
        .access(target, Access::ColorWrite);
    let compiled = graph.compile().expect("MAX_PASSES compile");
    assert_eq!(compiled.passes().len(), super::MAX_PASSES);
    // Resources past bit 15 are tracked (the use mask is 64-bit).
    assert!(compiled.passes()[super::MAX_PASSES - 1].uses(previous.expect("chain")));

    let mut graph = GraphBuilder::new();
    let target = swapchain(&mut graph);
    for index in 0..=super::MAX_PASSES {
        let _ = graph
            .pass("pass", index)
            .access(target, Access::ColorReadWrite);
    }
    assert_eq!(
        graph.compile().map(|_| ()),
        Err(GraphError::TooLarge("passes"))
    );
}
