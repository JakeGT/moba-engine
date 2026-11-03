//! The renderer's temporal state (ADR-0021 §1, §2c, §4): the back end, the jitter counter,
//! the previous presented view and the reset triggers. Presentation state only; nothing here
//! reaches the simulation, the wire or a replay (§8).

use moba_core::math::{Mat4, Vec3};

use super::{Extent, TemporalUpscaler, UpscaleMode};
use crate::scene::TemporalCamera;
use crate::temporal::{Jitter, MipBiasRule, PreviousView, jittered, mip_bias, view_forward};

/// What the frame being recorded uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TemporalFrame {
    pub(crate) camera: TemporalCamera,
    /// Jitter phases at this frame's sizes.
    pub(crate) phases: u32,
    /// Every history starts again this frame (no motion, no colour history).
    pub(crate) reset: bool,
    /// The camera's near and far planes and vertical field of view (radians).
    pub(crate) lens: [f32; 3],
    /// The frame time in milliseconds (the renderer fills it in once known).
    pub(crate) dt_ms: f32,
    /// The camera's forward and eye, kept for the next frame's cut test.
    eye: Vec3,
    forward: Vec3,
}

/// Temporal AA's state across frames.
pub(crate) struct TemporalState {
    pub(crate) upscaler: Box<dyn TemporalUpscaler>,
    pub(crate) mode: UpscaleMode,
    pub(crate) sharpness: f32,
    jitter: Jitter,
    previous: Option<PreviousView>,
    /// A reset asked for outside the scene (resize, settings, capture).
    reset_pending: bool,
    /// The latest frame's, for the frame log.
    last_phases: u32,
    /// `maxSamplerLodBias`: the mip bias is clamped to it, so the terrain's explicit-gradient
    /// scale and the materials' `texture(.., bias)` (which the device clamps) agree.
    max_lod_bias: f32,
}

impl TemporalState {
    pub(crate) fn new(
        upscaler: Box<dyn TemporalUpscaler>,
        mode: UpscaleMode,
        sharpness: f32,
    ) -> Self {
        Self {
            upscaler,
            mode,
            sharpness,
            jitter: Jitter::default(),
            previous: None,
            reset_pending: true,
            last_phases: 0,
            // The minimum every device guarantees.
            max_lod_bias: 2.0,
        }
    }

    /// Clamps the material mip bias to the device's `maxSamplerLodBias`.
    #[must_use]
    pub(crate) fn with_max_lod_bias(mut self, max_lod_bias: f32) -> Self {
        if max_lod_bias.is_finite() && max_lod_bias > 0.0 {
            self.max_lod_bias = max_lod_bias;
        }
        self
    }

    /// Forgets every history and restarts the jitter pattern (captures, ADR-0021 §1).
    pub(crate) fn restart(&mut self) {
        self.jitter.reset();
        self.reset_pending = true;
        self.upscaler.reset_history();
    }

    /// Forgets every history at the next frame (resize, a replaced back end).
    pub(crate) fn invalidate(&mut self) {
        self.reset_pending = true;
        self.upscaler.reset_history();
    }

    /// Jitter phases of the latest frame (0 before the first).
    pub(crate) fn jitter_phases(&self) -> u32 {
        self.last_phases
    }

    /// The jitter of the latest submitted frame (the motion probe's twin).
    pub(crate) fn previous_jitter(&self) -> Option<[f32; 2]> {
        self.previous.map(|previous| previous.jitter)
    }

    /// This frame's camera state for `view_proj` (the client's, unjittered) at `render` of
    /// `output`. `scene_reset` is [`crate::Scene::history_reset`]; a camera cut (§2c) resets
    /// too.
    pub(crate) fn begin_frame(
        &mut self,
        view_proj: Mat4,
        eye: Vec3,
        render: Extent,
        output: Extent,
        scene_reset: bool,
    ) -> TemporalFrame {
        let phases = self.upscaler.jitter_phases(render, output);
        let jitter = self.jitter.offset(phases);
        let forward = view_forward(view_proj);
        let cut = self
            .previous
            .is_none_or(|previous| previous.is_cut(eye, forward));
        let reset = scene_reset || self.reset_pending || cut;
        let (previous, previous_jitter) = match self.previous {
            Some(previous) if !reset => (previous.view_proj, previous.jitter),
            _ => (view_proj, jitter),
        };
        self.last_phases = phases;
        TemporalFrame {
            camera: TemporalCamera {
                jittered: jittered(view_proj, jitter, render),
                unjittered: view_proj,
                previous,
                jitter,
                previous_jitter,
                mip_bias: mip_bias(
                    MipBiasRule::for_kind(self.upscaler.kind()),
                    render.width,
                    output.width,
                )
                .clamp(-self.max_lod_bias, self.max_lod_bias),
                render: [render.width, render.height],
                live: true,
            },
            phases,
            reset,
            lens: [0.0; 3],
            dt_ms: 0.0,
            eye,
            forward,
        }
    }

    /// The frame was submitted: it becomes the previous view and the jitter advances.
    pub(crate) fn end_frame(&mut self, frame: &TemporalFrame) {
        self.previous = Some(PreviousView {
            view_proj: frame.camera.unjittered,
            eye: frame.eye,
            forward: frame.forward,
            jitter: frame.camera.jitter,
        });
        self.jitter.advance();
        self.reset_pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in back end: TAAU's phase rule, no GPU work.
    struct Stub;

    impl TemporalUpscaler for Stub {
        fn kind(&self) -> super::super::UpscalerKind {
            super::super::UpscalerKind::Taau
        }
        fn render_size_range(&self, output: Extent, _mode: UpscaleMode) -> (Extent, Extent) {
            (output, output)
        }
        fn jitter_phases(&self, render: Extent, output: Extent) -> u32 {
            crate::temporal::jitter_phases(render, output)
        }
        fn has_sharpening(&self) -> bool {
            true
        }
        unsafe fn record(
            &mut self,
            _cmd: ash::vk::CommandBuffer,
            _inputs: &super::super::UpscaleInputs,
            _out: &super::super::UpscaleOutput,
        ) {
        }
        fn reset_history(&mut self) {}
        unsafe fn resize(
            &mut self,
            _device: &crate::device::DeviceContext,
            _output: Extent,
            _max_render: Extent,
        ) -> crate::error::RenderResult<()> {
            Ok(())
        }
        fn vram_bytes(&self) -> u64 {
            0
        }
    }

    fn extent(width: u32, height: u32) -> Extent {
        Extent { width, height }
    }

    /// ADR-0021 §1: the mip bias is clamped to `maxSamplerLodBias`, as the device clamps a
    /// `texture(.., bias)`, so the terrain's gradient scale agrees with the materials.
    #[test]
    fn the_mip_bias_is_clamped_to_the_device_limit() {
        let mut temporal = TemporalState::new(Box::new(Stub), UpscaleMode::UltraPerformance, 0.25)
            .with_max_lod_bias(1.5);
        let view = moba_core::math::perspective_rh(1.0, 16.0 / 9.0, 0.1, 100.0)
            * moba_core::math::look_at_rh(Vec3::new(0.0, 10.0, 10.0), Vec3::ZERO, Vec3::Y);
        let frame = temporal.begin_frame(
            view,
            Vec3::new(0.0, 10.0, 10.0),
            extent(640, 360),
            extent(1920, 1080),
            false,
        );
        // log2(1/3) - 1 = -2.58, clamped to -1.5.
        assert_eq!(frame.camera.mip_bias, -1.5);
    }

    #[test]
    fn the_first_frame_and_cuts_reset_and_still_frames_do_not() {
        let mut temporal = TemporalState::new(Box::new(Stub), UpscaleMode::Quality, 0.25);
        let view = moba_core::math::perspective_rh(1.0, 16.0 / 9.0, 0.1, 100.0)
            * moba_core::math::look_at_rh(Vec3::new(0.0, 10.0, 10.0), Vec3::ZERO, Vec3::Y);
        let eye = Vec3::new(0.0, 10.0, 10.0);
        let (render, output) = (extent(1280, 720), extent(1920, 1080));
        let first = temporal.begin_frame(view, eye, render, output, false);
        assert!(first.reset);
        assert_eq!(first.camera.previous, view);
        assert_eq!(first.phases, 18);
        temporal.end_frame(&first);
        let second = temporal.begin_frame(view, eye, render, output, false);
        assert!(!second.reset);
        assert_eq!(second.camera.previous_jitter, first.camera.jitter);
        assert_ne!(second.camera.jitter, first.camera.jitter);
        temporal.end_frame(&second);
        // A 20 m jump is a cut (§2c), as is the scene's own reset.
        let cut = temporal.begin_frame(view, eye + Vec3::X * 20.0, render, output, false);
        assert!(cut.reset);
        let asked = temporal.begin_frame(view, eye, render, output, true);
        assert!(asked.reset);
        // The jittered matrix moves the image by the jitter, never the client's matrix.
        assert_eq!(second.camera.unjittered, view);
        assert_ne!(second.camera.jittered, view);
    }

    /// The jitter handed to the back ends (`UpscaleInputs::jitter` is `camera.jitter`) is the
    /// one the frame's jittered matrix applies, unchanged: points move by `(+jx, +jy)` render
    /// pixels, x right and y down. That is FSR 3.1's convention (its projection offset is
    /// `(2 jx / w, -2 jy / h)` in y-up NDC; ADR-0021 amendments 1 and 4), so no back end
    /// flips a sign.
    #[test]
    fn the_jitter_handed_on_is_the_one_applied() {
        let mut temporal = TemporalState::new(Box::new(Stub), UpscaleMode::Quality, 0.25);
        let eye = Vec3::new(3.0, 8.0, 6.0);
        let view = moba_core::math::perspective_rh(1.0, 16.0 / 9.0, 0.1, 100.0)
            * moba_core::math::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let (render, output) = (extent(1280, 720), extent(1920, 1080));
        for _ in 0..4 {
            let frame = temporal.begin_frame(view, eye, render, output, false);
            let [jx, jy] = frame.camera.jitter;
            assert!(jx != 0.0 || jy != 0.0);
            for point in [Vec3::ZERO, Vec3::new(1.0, 0.5, -2.0)] {
                let plain =
                    crate::temporal::clip_to_pixels(view * point.extend(1.0), render).unwrap();
                let moved = crate::temporal::clip_to_pixels(
                    frame.camera.jittered * point.extend(1.0),
                    render,
                )
                .unwrap();
                assert!(
                    (moved[0] - plain[0] - jx).abs() < 1e-3,
                    "{plain:?} {moved:?}"
                );
                assert!(
                    (moved[1] - plain[1] - jy).abs() < 1e-3,
                    "{plain:?} {moved:?}"
                );
            }
            temporal.end_frame(&frame);
        }
    }

    #[test]
    fn restart_returns_to_the_first_phase() {
        let mut temporal = TemporalState::new(Box::new(Stub), UpscaleMode::Quality, 0.25);
        let view = Mat4::IDENTITY;
        let (render, output) = (extent(64, 64), extent(64, 64));
        let first = temporal.begin_frame(view, Vec3::ZERO, render, output, false);
        temporal.end_frame(&first);
        let second = temporal.begin_frame(view, Vec3::ZERO, render, output, false);
        temporal.end_frame(&second);
        temporal.restart();
        let again = temporal.begin_frame(view, Vec3::ZERO, render, output, false);
        assert_eq!(again.camera.jitter, first.camera.jitter);
        assert!(again.reset);
    }
}
