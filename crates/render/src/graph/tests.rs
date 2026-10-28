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
    extra_usage: vk::ImageUsageFlags::empty(),
};

const DEPTH: ImageDesc = ImageDesc {
    format: vk::Format::D32_SFLOAT,
    aspect: vk::ImageAspectFlags::DEPTH,
    size: ImageSize::Swapchain,
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
