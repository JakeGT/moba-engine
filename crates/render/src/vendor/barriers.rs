//! Barriers around a vendor SDK's recorded work (ADR-0021 amendment 5): NGX and XeSS record
//! their dispatches into our command buffer without ordering them against the graph's own
//! barriers or against their previous frame's work, so the back end records these around the
//! SDK call. Found with synchronization validation on the RTX 5070 Ti (driver 615.78.08):
//! NGX clears its output (`vkCmdClearColorImage`) inside the evaluation and fills its own
//! buffers each Performance-mode evaluation with no barrier between frames. XeSS is given the
//! same barriers (its runtime is not observable here).

#![cfg_attr(
    not(any(all(feature = "dlss", not(target_vendor = "apple")), feature = "xess")),
    allow(dead_code, reason = "only the DLSS and XeSS back ends record SDK work")
)]

use ash::vk;

/// What the SDK's evaluation may do to the output: compute writes and reads, a clear
/// (`vkCmdClearColorImage`, seen with driver 615.78.08 on the RTX 5070 Ti; NGX documents
/// neither, and records no barrier for it), and any other transfer (a copy or blit, which
/// XeSS's unobserved runtime may record): every transfer stage, not only `CLEAR`.
const SDK_STAGES: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::COMPUTE_SHADER.as_raw()
        | vk::PipelineStageFlags2::ALL_TRANSFER.as_raw(),
);
const SDK_ACCESS: vk::AccessFlags2 = vk::AccessFlags2::from_raw(
    vk::AccessFlags2::SHADER_STORAGE_WRITE.as_raw()
        | vk::AccessFlags2::SHADER_STORAGE_READ.as_raw()
        | vk::AccessFlags2::TRANSFER_WRITE.as_raw()
        | vk::AccessFlags2::TRANSFER_READ.as_raw(),
);

/// What the SDK's evaluation does to its own resources: compute, clears and fills
/// (`vkCmdFillBuffer` on its internal buffers each evaluation in Performance mode, seen with
/// driver 615.78.08), and copies.
const SDK_INTERNAL_STAGES: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::COMPUTE_SHADER.as_raw()
        | vk::PipelineStageFlags2::ALL_TRANSFER.as_raw(),
);

/// Before the evaluation, for NGX's internal resources: NGX records no barrier between one
/// evaluation's writes to its buffers and the next one's (synchronization validation reports
/// a write-after-write between two frames' fills), so the previous evaluation's work, in
/// whichever submission, is ordered before this one's with a global barrier. The stages are
/// ones the `temporal` pass waits for anyway (the frame's earlier compute passes).
pub(crate) fn internal_barrier() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(SDK_INTERNAL_STAGES)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(SDK_INTERNAL_STAGES)
        .dst_access_mask(vk::AccessFlags2::MEMORY_WRITE | vk::AccessFlags2::MEMORY_READ)
}

/// After the evaluation, for the inputs and the SDK's own resources: the graph orders the
/// next frame's writes to the inputs after the `temporal` pass's compute stage only, so any
/// transfer the SDK recorded on them (a copy of an input) is chained into the compute stage
/// here (a write-after-read otherwise). Its writes are made available as well.
pub(crate) fn inputs_after() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(SDK_INTERNAL_STAGES)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
        )
}

fn output_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// Before the evaluation: the graph's barrier made the output ready for compute storage
/// writes (`GENERAL`); this one chains from it to NGX's clear as well (a WAW against the
/// graph's layout transition and the previous use).
pub(crate) fn output_before(image: vk::Image) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .dst_stage_mask(SDK_STAGES)
        .dst_access_mask(SDK_ACCESS)
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(output_range())
}

/// After the evaluation: NGX's writes, the clear included, are made available and chained
/// to compute storage writes, the graph's next barrier's source (the output is then read).
pub(crate) fn output_after(image: vk::Image) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(SDK_STAGES)
        .src_access_mask(SDK_ACCESS)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(
            vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
        )
        .old_layout(vk::ImageLayout::GENERAL)
        .new_layout(vk::ImageLayout::GENERAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(output_range())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NGX's clear of the output is ordered after the graph's barrier (which ends in compute
    /// storage writes) and before the graph's next one (which starts from them).
    #[test]
    fn output_barriers_chain_ngx_clear_into_the_graph() {
        let image = vk::Image::null();
        let before = output_before(image);
        assert_eq!(
            before.src_stage_mask,
            vk::PipelineStageFlags2::COMPUTE_SHADER
        );
        // Every transfer stage, not only clears (a copy or blit by an SDK).
        assert!(
            before
                .dst_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_TRANSFER)
        );
        assert!(
            before
                .dst_access_mask
                .contains(vk::AccessFlags2::TRANSFER_WRITE)
        );
        let after = output_after(image);
        assert!(
            after
                .src_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_TRANSFER)
        );
        assert!(
            after
                .src_access_mask
                .contains(vk::AccessFlags2::TRANSFER_READ)
        );
        assert!(
            after
                .src_access_mask
                .contains(vk::AccessFlags2::TRANSFER_WRITE)
        );
        assert!(
            after
                .src_access_mask
                .contains(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        );
        assert_eq!(
            after.dst_stage_mask,
            vk::PipelineStageFlags2::COMPUTE_SHADER
        );
        for barrier in [before, after] {
            assert_eq!(barrier.old_layout, vk::ImageLayout::GENERAL);
            assert_eq!(barrier.new_layout, vk::ImageLayout::GENERAL);
        }
    }

    /// One evaluation's fills and compute writes to NGX's own buffers are ordered before the
    /// next evaluation's (a WAW across frames that NGX leaves to the caller).
    #[test]
    fn evaluations_are_ordered_against_each_other() {
        let barrier = internal_barrier();
        for stages in [barrier.src_stage_mask, barrier.dst_stage_mask] {
            assert!(stages.contains(vk::PipelineStageFlags2::ALL_TRANSFER));
            assert!(stages.contains(vk::PipelineStageFlags2::COMPUTE_SHADER));
        }
        assert!(
            barrier
                .src_access_mask
                .contains(vk::AccessFlags2::MEMORY_WRITE)
        );
        assert!(
            barrier
                .dst_access_mask
                .contains(vk::AccessFlags2::MEMORY_WRITE)
        );
    }

    /// A transfer the SDK records on an input (or its own resources) is chained into the
    /// compute stage the graph's next barrier on the inputs starts from.
    #[test]
    fn sdk_transfers_chain_into_the_graph() {
        let barrier = inputs_after();
        assert!(
            barrier
                .src_stage_mask
                .contains(vk::PipelineStageFlags2::ALL_TRANSFER)
        );
        assert!(
            barrier
                .src_stage_mask
                .contains(vk::PipelineStageFlags2::COMPUTE_SHADER)
        );
        assert_eq!(
            barrier.dst_stage_mask,
            vk::PipelineStageFlags2::COMPUTE_SHADER
        );
    }
}
