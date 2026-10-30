//! The dynamic-resolution controller (R6, ADR-0015 §6): pure logic, fed with the GPU
//! profiler's per-frame totals, so it is unit-tested without a device.

/// Frames a new scale is held before the controller changes it again.
const HOLD_FRAMES: u32 = 15;
/// Scale step granularity: 1/32 (quantised so the scale does not wander by tiny amounts).
const QUANTUM: f32 = 32.0;
/// Exponential-average weight of each new sample.
const SMOOTHING: f32 = 0.2;
/// Above this fraction of the target the scale steps down.
const HIGH_WATER: f32 = 0.95;
/// Below this fraction of the target the scale steps up.
const LOW_WATER: f32 = 0.75;
/// The step up (scale units).
const STEP_UP: f32 = 0.05;

/// Holds a GPU-time target by scaling the render resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DynamicResolution {
    scale: f32,
    min: f32,
    max: f32,
    target_ms: f32,
    average_ms: Option<f32>,
    held: u32,
    last_frame: Option<u64>,
}

impl DynamicResolution {
    /// A controller starting at `max`, within `min..=max` (both already in `0.5..=1`).
    pub(crate) fn new(min: f32, max: f32, target_ms: f32) -> Self {
        Self {
            scale: max,
            min: min.min(max),
            max,
            target_ms,
            average_ms: None,
            held: 0,
            last_frame: None,
        }
    }

    /// The current scale.
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Feeds the GPU time of profiled frame `frame` (repeats of the same frame are ignored)
    /// and returns the scale for the next frame. Pixel cost scales with the area, so a step
    /// down multiplies the scale by `sqrt(0.9 * target / measured)`; steps up are small and
    /// slow, so the scale settles instead of oscillating.
    pub(crate) fn observe(&mut self, frame: u64, gpu_ms: f32) -> f32 {
        if self.last_frame == Some(frame) || !gpu_ms.is_finite() || gpu_ms < 0.0 {
            return self.scale;
        }
        self.last_frame = Some(frame);
        let average = self
            .average_ms
            .map_or(gpu_ms, |average| average + SMOOTHING * (gpu_ms - average));
        self.average_ms = Some(average);
        self.held = self.held.saturating_add(1);
        if self.held < HOLD_FRAMES {
            return self.scale;
        }
        let wanted = if average > HIGH_WATER * self.target_ms {
            self.scale * (0.9 * self.target_ms / average).sqrt()
        } else if average < LOW_WATER * self.target_ms {
            self.scale + STEP_UP
        } else {
            self.scale
        };
        let next = quantise(wanted.clamp(self.min, self.max)).clamp(self.min, self.max);
        if (next - self.scale).abs() > f32::EPSILON {
            // Predict the new cost from the area ratio so the average does not trigger a
            // second step from stale samples.
            let ratio = next / self.scale;
            self.average_ms = Some(average * ratio * ratio);
            self.scale = next;
            self.held = 0;
        }
        self.scale
    }
}

fn quantise(scale: f32) -> f32 {
    (scale * QUANTUM).round() / QUANTUM
}

/// The render sub-rectangle for `scale` of `swapchain`, at most `alloc` and at least 1×1.
pub(crate) fn render_extent(
    swapchain: ash::vk::Extent2D,
    alloc: ash::vk::Extent2D,
    scale: f32,
) -> ash::vk::Extent2D {
    let side = |native: u32, cap: u32| {
        let scaled = (native as f32 * scale).round();
        // `scaled` is non-negative and at most `native`, so the cast is exact.
        (scaled as u32).clamp(1, cap.max(1))
    };
    ash::vk::Extent2D {
        width: side(swapchain.width, alloc.width),
        height: side(swapchain.height, alloc.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk;

    #[test]
    fn steps_down_under_load_and_recovers_slowly() {
        let mut control = DynamicResolution::new(0.6, 1.0, 10.0);
        let mut frame = 0;
        // 16 ms at full scale: well over the 10 ms target.
        for _ in 0..HOLD_FRAMES {
            frame += 1;
            control.observe(frame, 16.0);
        }
        let first = control.scale();
        assert!(first < 1.0, "{first}");
        assert!((first * QUANTUM - (first * QUANTUM).round()).abs() < 1e-4);
        // The same frame number again is ignored.
        assert_eq!(control.observe(frame, 100.0), first);
        // Cost proportional to area: settles where the target is met, above the floor.
        for _ in 0..400 {
            frame += 1;
            let cost = 16.0 * control.scale() * control.scale();
            control.observe(frame, cost);
        }
        let settled = control.scale();
        let cost = 16.0 * settled * settled;
        assert!((6.0..=10.0).contains(&cost), "{settled} -> {cost} ms");
        // Load gone: back up to the ceiling, one small step per hold period.
        for _ in 0..400 {
            frame += 1;
            control.observe(frame, 2.0);
        }
        assert_eq!(control.scale(), 1.0);
    }

    #[test]
    fn never_leaves_its_bounds_and_ignores_garbage() {
        let mut control = DynamicResolution::new(0.75, 0.9, 5.0);
        assert_eq!(control.scale(), 0.9);
        for frame in 0..200 {
            control.observe(frame, 500.0);
        }
        assert_eq!(control.scale(), 0.75);
        control.observe(1_000, f32::NAN);
        control.observe(1_001, -1.0);
        assert_eq!(control.scale(), 0.75);
    }

    #[test]
    fn render_extent_rounds_and_clamps() {
        let swapchain = vk::Extent2D {
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            render_extent(swapchain, swapchain, 0.5),
            vk::Extent2D {
                width: 960,
                height: 540
            }
        );
        let alloc = vk::Extent2D {
            width: 1440,
            height: 810,
        };
        assert_eq!(render_extent(swapchain, alloc, 1.0), alloc);
        assert_eq!(render_extent(swapchain, alloc, 0.0).width, 1);
    }
}
