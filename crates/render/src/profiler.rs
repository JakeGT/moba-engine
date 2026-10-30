//! Per-pass GPU timing with timestamp queries.
//!
//! One query pool per frame in flight, created with the renderer (never per frame). Each
//! pass writes a timestamp after its barriers and another after its commands, both at
//! `ALL_COMMANDS`, so a pass's time excludes waits on *earlier commands*. Passes before the
//! first one that touches the swapchain are submitted in a batch that does not wait on the
//! acquire semaphore, so the scene pass never absorbs that wait; the first swapchain pass
//! (the composite) can, when the presentation engine still holds the image. Results are read
//! back without waiting, after that frame slot's fence has been waited on, so they lag the
//! current frame by `FRAMES_IN_FLIGHT` frames. Each result carries the sequence number of
//! the frame it measured ([`GpuTimings::frame`]), so a caller polling every frame can skip
//! samples it has already seen. Devices whose graphics queue reports `timestampValidBits == 0`
//! (or a zero `timestampPeriod`) get no pools and [`GpuTimings`] is never produced.

use ash::vk;

use crate::error::RenderResult;
use crate::graph::MAX_PASSES;

const QUERIES_PER_FRAME: u32 = 2 * MAX_PASSES as u32;

/// GPU time of one render-graph pass.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PassTiming {
    /// Pass name (`"scene"`, `"composite"`, `"ui"`, `"capture"`).
    pub name: &'static str,
    /// Milliseconds between the pass's first and last command on the GPU.
    pub ms: f32,
}

/// GPU timings of the most recent frame whose queries completed.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct GpuTimings {
    passes: [PassTiming; MAX_PASSES],
    count: u8,
    /// Sum of the pass times: GPU work, without inter-pass barriers and waits.
    pub total_ms: f32,
    /// First pass start to last pass end, including barriers and any waits between passes.
    pub span_ms: f32,
    /// Sequence number of the measured frame: `0` for the first frame the renderer
    /// submitted, then one more per submitted frame. Unchanged between two calls means the
    /// timings are the same sample (no newer frame completed in between).
    pub frame: u64,
}

impl Default for GpuTimings {
    fn default() -> Self {
        Self {
            passes: [PassTiming::default(); MAX_PASSES],
            count: 0,
            total_ms: 0.0,
            span_ms: 0.0,
            frame: 0,
        }
    }
}

impl GpuTimings {
    /// Per-pass timings in execution order.
    #[must_use]
    pub fn passes(&self) -> &[PassTiming] {
        &self.passes[..usize::from(self.count)]
    }

    /// Milliseconds of the pass called `name`, if it ran in that frame.
    #[must_use]
    pub fn pass_ms(&self, name: &str) -> Option<f32> {
        self.passes()
            .iter()
            .find(|pass| pass.name == name)
            .map(|pass| pass.ms)
    }

    /// Total milliseconds of every pass whose name starts with `prefix` (for example
    /// `"bloom"`: the R6 down- and up-sample passes, or `"ao"`: GTAO and its blur); `0.0`
    /// when none ran.
    #[must_use]
    pub fn group_ms(&self, prefix: &str) -> f32 {
        self.passes()
            .iter()
            .filter(|pass| pass.name.starts_with(prefix))
            .map(|pass| pass.ms)
            .sum()
    }
}

#[derive(Clone, Copy)]
struct FrameQueries {
    names: [&'static str; MAX_PASSES],
    count: usize,
    open: bool,
    submitted: bool,
    /// Sequence number given at submission.
    sequence: u64,
}

/// Timestamp query pools and the latest decoded timings.
pub(crate) struct GpuProfiler {
    device: ash::Device,
    /// One pool per frame in flight; empty when the queue cannot write timestamps.
    pools: Vec<vk::QueryPool>,
    frames: Vec<FrameQueries>,
    period_ns: f64,
    valid_mask: u64,
    raw: [u64; QUERIES_PER_FRAME as usize],
    latest: Option<GpuTimings>,
    /// Sequence number the next submitted frame gets.
    next_sequence: u64,
}

impl GpuProfiler {
    /// Creates `frames` query pools, or none when `valid_bits` or `period_ns` is zero.
    pub(crate) fn new(
        device: ash::Device,
        frames: usize,
        valid_bits: u32,
        period_ns: f32,
    ) -> RenderResult<Self> {
        let mut profiler = Self {
            device,
            pools: Vec::with_capacity(frames),
            frames: vec![
                FrameQueries {
                    names: [""; MAX_PASSES],
                    count: 0,
                    open: false,
                    submitted: false,
                    sequence: 0,
                };
                frames
            ],
            period_ns: f64::from(period_ns),
            valid_mask: match valid_bits {
                0 => 0,
                64.. => u64::MAX,
                bits => (1u64 << bits) - 1,
            },
            raw: [0; QUERIES_PER_FRAME as usize],
            latest: None,
            next_sequence: 0,
        };
        if valid_bits == 0 || !(period_ns > 0.0 && period_ns.is_finite()) {
            return Ok(profiler);
        }
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(QUERIES_PER_FRAME);
        for _ in 0..frames {
            // SAFETY: the device is live and the create info is fully initialised. A pool
            // created before a later failure is destroyed by `Drop`.
            let pool = unsafe { profiler.device.create_query_pool(&info, None) }?;
            profiler.pools.push(pool);
        }
        Ok(profiler)
    }

    /// Whether timestamps are recorded on this device.
    pub(crate) fn enabled(&self) -> bool {
        !self.pools.is_empty()
    }

    /// Reads the completed queries of `frame`. Call after waiting on that frame's fence.
    pub(crate) fn collect(&mut self, frame: usize) {
        let Some(&pool) = self.pools.get(frame) else {
            return;
        };
        let queries = self.frames[frame];
        self.frames[frame].submitted = false;
        if !queries.submitted || queries.count == 0 {
            return;
        }
        let raw = &mut self.raw[..2 * queries.count];
        // SAFETY: the pool belongs to this device; the frame's fence was waited on, so the
        // queries written by that submission are complete. No WAIT flag: an unavailable
        // result returns NOT_READY instead of blocking, and is skipped.
        let read = unsafe {
            self.device
                .get_query_pool_results(pool, 0, raw, vk::QueryResultFlags::TYPE_64)
        };
        if read.is_err() {
            return;
        }
        let ms = |ticks: u64| (ticks as f64 * self.period_ns / 1.0e6) as f32;
        let mut timings = GpuTimings::default();
        for (index, pair) in raw.as_chunks::<2>().0.iter().enumerate() {
            let pass_ms = ms(pair[1].wrapping_sub(pair[0]) & self.valid_mask);
            timings.passes[index] = PassTiming {
                name: queries.names[index],
                ms: pass_ms,
            };
            timings.total_ms += pass_ms;
        }
        timings.count = u8::try_from(queries.count).unwrap_or(u8::MAX);
        timings.span_ms = ms(raw[2 * queries.count - 1].wrapping_sub(raw[0]) & self.valid_mask);
        timings.frame = queries.sequence;
        self.latest = Some(timings);
    }

    /// Resets `frame`'s queries; record before any pass.
    pub(crate) fn begin_frame(&mut self, command_buffer: vk::CommandBuffer, frame: usize) {
        let Some(&pool) = self.pools.get(frame) else {
            return;
        };
        let queries = &mut self.frames[frame];
        queries.count = 0;
        queries.open = false;
        queries.submitted = false;
        // SAFETY: the command buffer is recording outside a render pass instance and the
        // query range is inside the pool.
        unsafe {
            self.device
                .cmd_reset_query_pool(command_buffer, pool, 0, QUERIES_PER_FRAME);
        }
    }

    /// Starts timing a pass (after its barriers).
    pub(crate) fn begin_pass(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        name: &'static str,
    ) {
        let Some(&pool) = self.pools.get(frame) else {
            return;
        };
        let queries = &mut self.frames[frame];
        if queries.open || queries.count >= MAX_PASSES {
            return;
        }
        queries.names[queries.count] = name;
        queries.open = true;
        let query = u32::try_from(2 * queries.count).expect("bounded by MAX_PASSES");
        // SAFETY: the query was reset in this command buffer and is written once;
        // ALL_COMMANDS is a single stage flag, valid for timestamps on a queue with
        // non-zero timestampValidBits.
        unsafe {
            self.device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                pool,
                query,
            );
        }
    }

    /// Stops timing the open pass.
    pub(crate) fn end_pass(&mut self, command_buffer: vk::CommandBuffer, frame: usize) {
        let Some(&pool) = self.pools.get(frame) else {
            return;
        };
        let queries = &mut self.frames[frame];
        if !queries.open {
            return;
        }
        let query = u32::try_from(2 * queries.count + 1).expect("bounded by MAX_PASSES");
        queries.open = false;
        queries.count += 1;
        // SAFETY: as in `begin_pass`; this is the pass's second, distinct query.
        unsafe {
            self.device.cmd_write_timestamp2(
                command_buffer,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                pool,
                query,
            );
        }
    }

    /// Marks `frame`'s queries as submitted, so the next [`Self::collect`] reads them, and
    /// gives them the next sequence number.
    pub(crate) fn submitted(&mut self, frame: usize) {
        let enabled = self.enabled();
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        if let Some(queries) = self.frames.get_mut(frame)
            && enabled
        {
            queries.submitted = !queries.open;
            queries.sequence = sequence;
        }
    }

    /// The most recent completed timings.
    pub(crate) fn latest(&self) -> Option<GpuTimings> {
        self.latest
    }
}

impl Drop for GpuProfiler {
    fn drop(&mut self) {
        for pool in self.pools.drain(..) {
            // SAFETY: the renderer waits for the device to idle before dropping its
            // resources, so no submission still uses the pool; each pool is destroyed once.
            unsafe { self.device.destroy_query_pool(pool, None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuTimings, PassTiming};

    #[test]
    fn timings_are_looked_up_by_pass_name() {
        let mut timings = GpuTimings::default();
        timings.passes[0] = PassTiming {
            name: "scene",
            ms: 1.5,
        };
        timings.passes[1] = PassTiming {
            name: "ui",
            ms: 0.25,
        };
        timings.count = 2;
        assert_eq!(timings.pass_ms("ui"), Some(0.25));
        assert_eq!(timings.pass_ms("composite"), None);
        assert_eq!(timings.passes().len(), 2);
        timings.passes[2] = PassTiming {
            name: "bloom_d0",
            ms: 0.5,
        };
        timings.passes[3] = PassTiming {
            name: "bloom_u0",
            ms: 0.125,
        };
        timings.count = 4;
        assert_eq!(timings.group_ms("bloom"), 0.625);
        assert_eq!(timings.group_ms("fxaa"), 0.0);
    }
}
