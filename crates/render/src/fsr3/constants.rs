//! The FSR 3.1.6 constant buffers and the per-frame state behind them: a port of the constant
//! set-up in the SDK's `fsr3upscalerDispatch` (`ffx_fsr3upscaler.cpp`), `ffxSpdSetup`
//! (`ffx_spd.h`) and `FsrRcasCon` (`ffx_fsr1.h`).

use bytemuck::{Pod, Zeroable};

use crate::upscale::UpscaleInputs;

/// `cbFSR3Upscaler` (HLSL packing; the field offsets are checked against the `slangc`
/// reflection in the tests).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct UpscalerConstants {
    pub(super) render_size: [i32; 2],
    pub(super) previous_frame_render_size: [i32; 2],
    pub(super) upscale_size: [i32; 2],
    pub(super) previous_frame_upscale_size: [i32; 2],
    pub(super) max_render_size: [i32; 2],
    pub(super) max_upscale_size: [i32; 2],
    pub(super) device_to_view_depth: [f32; 4],
    pub(super) jitter: [f32; 2],
    pub(super) previous_frame_jitter: [f32; 2],
    pub(super) motion_vector_scale: [f32; 2],
    pub(super) downscale_factor: [f32; 2],
    pub(super) motion_vector_jitter_cancellation: [f32; 2],
    pub(super) tan_half_fov: f32,
    pub(super) jitter_sequence_length: f32,
    pub(super) delta_time: f32,
    pub(super) delta_pre_exposure: f32,
    pub(super) view_space_to_meters_factor: f32,
    pub(super) frame_index: f32,
    pub(super) velocity_factor: f32,
    pub(super) reactiveness_scale: f32,
    pub(super) shading_change_scale: f32,
    pub(super) accumulation_added_per_frame: f32,
    pub(super) min_disocclusion_accumulation: f32,
}

/// `cbSPD` of the two pyramid passes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(super) struct SpdConstants {
    pub(super) mips: u32,
    pub(super) num_work_groups: u32,
    pub(super) work_group_offset: [u32; 2],
    pub(super) render_size: [u32; 2],
}

/// `cbRCAS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(super) struct RcasConstants {
    pub(super) config: [u32; 4],
}

impl UpscalerConstants {
    /// The context's initial constants: zero, with the SDK's tuning defaults
    /// (`ffxFsr3UpscalerContextCreate`), none of which the port changes.
    pub(super) fn new(max_render: [u32; 2], max_upscale: [u32; 2]) -> Self {
        Self {
            max_render_size: max_render.map(to_i32),
            max_upscale_size: max_upscale.map(to_i32),
            velocity_factor: 1.0,
            reactiveness_scale: 1.0,
            shading_change_scale: 1.0,
            accumulation_added_per_frame: 1.0 / 3.0,
            min_disocclusion_accumulation: -1.0 / 3.0,
            ..Self::default()
        }
    }

    /// Advances to the next frame as `fsr3upscalerDispatch` does. `reset` is the caller's
    /// reset or the context's first frame; `upscale` is the output size; `phases` the jitter
    /// phase count of this frame's sizes (ADR-0021 §1, see [`super::jitter_phases`]).
    pub(super) fn advance(
        &mut self,
        inputs: &UpscaleInputs,
        upscale: [u32; 2],
        reset: bool,
        phases: u32,
    ) {
        let render = [inputs.render_size.width, inputs.render_size.height];
        self.previous_frame_jitter = self.jitter;
        self.jitter = inputs.jitter;
        self.previous_frame_render_size = self.render_size;
        self.render_size = render.map(to_i32);

        // The horizontal field of view from the vertical one.
        let aspect = render[0] as f32 / render[1] as f32;
        let horizontal = ((inputs.fov_y / 2.0).tan() * aspect).atan() * 2.0;
        self.tan_half_fov = (horizontal * 0.5).tan();
        self.view_space_to_meters_factor = 1.0;
        self.device_to_view_depth = device_to_view_depth(inputs, aspect);

        self.previous_frame_upscale_size = self.upscale_size;
        self.upscale_size = upscale.map(to_i32);
        self.downscale_factor = [
            render[0] as f32 / upscale[0] as f32,
            render[1] as f32 / upscale[1] as f32,
        ];
        // The exposure is a texture; there is no separate pre-exposure.
        self.delta_pre_exposure = 1.0;
        // Render-resolution motion vectors in render pixels: scale (1, 1) over the render
        // size, and no jitter cancellation (they carry no jitter).
        self.motion_vector_scale = [1.0 / render[0] as f32, 1.0 / render[1] as f32];
        self.motion_vector_jitter_cancellation = [0.0; 2];

        // The sequence length steps by one per frame towards the current phase count.
        let phases = phases as f32;
        if reset || self.jitter_sequence_length == 0.0 {
            self.jitter_sequence_length = phases;
        } else if phases > self.jitter_sequence_length {
            self.jitter_sequence_length += 1.0;
        } else if phases < self.jitter_sequence_length {
            self.jitter_sequence_length -= 1.0;
        }

        self.delta_time = (inputs.dt_ms / 1000.0).clamp(0.0, 1.0);
        self.frame_index = if reset { 0.0 } else { self.frame_index + 1.0 };
    }
}

/// `setupDeviceDepthToViewSpaceDepthParams` for an inverted (reversed-Z, ADR-0023 §1c) depth
/// with a finite far plane (ADR-0021 §2a): the SDK swaps the planes for an inverted depth.
fn device_to_view_depth(inputs: &UpscaleInputs, aspect: f32) -> [f32; 4] {
    // The SDK's `fMin` and `fMax` after its inverted-depth swap: the far and the near plane.
    let min = inputs.near.max(inputs.far);
    let max = inputs.near.min(inputs.far);
    let q = max / (min - max);
    let cot_half_fov_y = (0.5 * inputs.fov_y).cos() / (0.5 * inputs.fov_y).sin();
    let a = cot_half_fov_y / aspect;
    let b = cot_half_fov_y;
    [-q, q * min, 1.0 / a, 1.0 / b]
}

impl SpdConstants {
    /// `ffxSpdSetup` over the whole render rectangle, and the dispatch size of both pyramid
    /// passes.
    pub(super) fn new(render: [u32; 2]) -> (Self, [u32; 2]) {
        // The rectangle starts at (0, 0), so the work-group offset is 0.
        let groups = [(render[0] - 1) / 64 + 1, (render[1] - 1) / 64 + 1];
        let largest = render[0].max(render[1]);
        let mips = (largest as f32).log2().floor().min(12.0) as u32;
        (
            Self {
                mips,
                num_work_groups: groups[0] * groups[1],
                work_group_offset: [0, 0],
                render_size: render,
            },
            groups,
        )
    }
}

impl RcasConstants {
    /// `FsrRcasCon` for the SDK's mapping of a `0..=1` sharpness to stops
    /// (`2 − 2 × sharpness`; 1 is the sharpest).
    pub(super) fn new(sharpness: f32) -> Self {
        let stops = -2.0 * sharpness.clamp(0.0, 1.0) + 2.0;
        let linear = (-stops).exp2();
        Self {
            config: [
                linear.to_bits(),
                crate::passes::vfx::pack_half2(linear, linear),
                0,
                0,
            ],
        }
    }
}

fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upscale::{UpscaleImage, UpscaleInputs};
    use ash::vk;
    use moba_shaders::fsr3 as shaders;

    /// The Rust mirrors have the reflected size and every field's offset.
    #[test]
    #[allow(clippy::too_many_lines)] // One table row per field.
    fn mirrors_match_the_reflection() {
        let fields = |pass: &str, name: &str| -> Vec<(String, u64)> {
            let json: serde_json::Value =
                serde_json::from_str(shaders::reflection(pass).unwrap()).unwrap();
            json["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|parameter| parameter["name"] == name)
                .unwrap()["type"]["elementType"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| {
                    (
                        field["name"].as_str().unwrap().to_owned(),
                        field["binding"]["offset"].as_u64().unwrap(),
                    )
                })
                .collect()
        };
        assert_eq!(
            size_of::<UpscalerConstants>(),
            shaders::CB_FSR3_UPSCALER_SIZE
        );
        assert_eq!(size_of::<SpdConstants>(), shaders::CB_SPD_SIZE);
        assert_eq!(size_of::<RcasConstants>(), shaders::CB_RCAS_SIZE);
        let upscaler = [
            (
                "iRenderSize",
                core::mem::offset_of!(UpscalerConstants, render_size),
            ),
            (
                "iPreviousFrameRenderSize",
                core::mem::offset_of!(UpscalerConstants, previous_frame_render_size),
            ),
            (
                "iUpscaleSize",
                core::mem::offset_of!(UpscalerConstants, upscale_size),
            ),
            (
                "iPreviousFrameUpscaleSize",
                core::mem::offset_of!(UpscalerConstants, previous_frame_upscale_size),
            ),
            (
                "iMaxRenderSize",
                core::mem::offset_of!(UpscalerConstants, max_render_size),
            ),
            (
                "iMaxUpscaleSize",
                core::mem::offset_of!(UpscalerConstants, max_upscale_size),
            ),
            (
                "fDeviceToViewDepth",
                core::mem::offset_of!(UpscalerConstants, device_to_view_depth),
            ),
            ("fJitter", core::mem::offset_of!(UpscalerConstants, jitter)),
            (
                "fPreviousFrameJitter",
                core::mem::offset_of!(UpscalerConstants, previous_frame_jitter),
            ),
            (
                "fMotionVectorScale",
                core::mem::offset_of!(UpscalerConstants, motion_vector_scale),
            ),
            (
                "fDownscaleFactor",
                core::mem::offset_of!(UpscalerConstants, downscale_factor),
            ),
            (
                "fMotionVectorJitterCancellation",
                core::mem::offset_of!(UpscalerConstants, motion_vector_jitter_cancellation),
            ),
            (
                "fTanHalfFOV",
                core::mem::offset_of!(UpscalerConstants, tan_half_fov),
            ),
            (
                "fJitterSequenceLength",
                core::mem::offset_of!(UpscalerConstants, jitter_sequence_length),
            ),
            (
                "fDeltaTime",
                core::mem::offset_of!(UpscalerConstants, delta_time),
            ),
            (
                "fDeltaPreExposure",
                core::mem::offset_of!(UpscalerConstants, delta_pre_exposure),
            ),
            (
                "fViewSpaceToMetersFactor",
                core::mem::offset_of!(UpscalerConstants, view_space_to_meters_factor),
            ),
            (
                "fFrameIndex",
                core::mem::offset_of!(UpscalerConstants, frame_index),
            ),
            (
                "fVelocityFactor",
                core::mem::offset_of!(UpscalerConstants, velocity_factor),
            ),
            (
                "fReactivenessScale",
                core::mem::offset_of!(UpscalerConstants, reactiveness_scale),
            ),
            (
                "fShadingChangeScale",
                core::mem::offset_of!(UpscalerConstants, shading_change_scale),
            ),
            (
                "fAccumulationAddedPerFrame",
                core::mem::offset_of!(UpscalerConstants, accumulation_added_per_frame),
            ),
            (
                "fMinDisocclusionAccumulation",
                core::mem::offset_of!(UpscalerConstants, min_disocclusion_accumulation),
            ),
        ];
        let spd = [
            ("mips", core::mem::offset_of!(SpdConstants, mips)),
            (
                "numWorkGroups",
                core::mem::offset_of!(SpdConstants, num_work_groups),
            ),
            (
                "workGroupOffset",
                core::mem::offset_of!(SpdConstants, work_group_offset),
            ),
            (
                "renderSize",
                core::mem::offset_of!(SpdConstants, render_size),
            ),
        ];
        let rcas = [("rcasConfig", core::mem::offset_of!(RcasConstants, config))];
        for (pass, block, mirror) in [
            ("fsr3_accumulate", "cbFSR3Upscaler", &upscaler[..]),
            ("fsr3_luma_pyramid", "cbSPD", &spd[..]),
            ("fsr3_rcas", "cbRCAS", &rcas[..]),
        ] {
            let mirror: Vec<(String, u64)> = mirror
                .iter()
                .map(|(name, offset)| ((*name).to_owned(), *offset as u64))
                .collect();
            assert_eq!(fields(pass, block), mirror, "{block}");
        }
    }

    fn inputs(render: (u32, u32)) -> UpscaleInputs {
        let image = UpscaleImage {
            image: vk::Image::null(),
            view: vk::ImageView::null(),
            format: vk::Format::UNDEFINED,
            extent: vk::Extent2D::default(),
        };
        UpscaleInputs {
            color: image,
            depth: image,
            motion: image,
            reactive: image,
            transparency: image,
            exposure: image,
            jitter: [0.25, -0.125],
            render_size: vk::Extent2D {
                width: render.0,
                height: render.1,
            },
            dt_ms: 16.0,
            near: 0.1,
            far: 100.0,
            fov_y: core::f32::consts::FRAC_PI_3,
            reset: false,
            sharpness: 0.25,
            frame_slot: 0,
        }
    }

    #[test]
    fn constants_follow_the_sdk() {
        let mut constants = UpscalerConstants::new([1280, 720], [1920, 1080]);
        constants.advance(&inputs((1280, 720)), [1920, 1080], true, 18);
        assert_eq!(constants.render_size, [1280, 720]);
        assert_eq!(constants.previous_frame_render_size, [0, 0]);
        assert_eq!(constants.upscale_size, [1920, 1080]);
        assert_eq!(constants.jitter_sequence_length, 18.0);
        assert_eq!(constants.frame_index, 0.0);
        assert_eq!(constants.delta_time, 0.016);
        assert_eq!(constants.motion_vector_scale, [1.0 / 1280.0, 1.0 / 720.0]);
        // Inverted depth (ADR-0023 §1c): the SDK swaps the planes.
        let q = 0.1f32 / (100.0 - 0.1);
        assert_eq!(constants.device_to_view_depth[0], -q);
        assert_eq!(constants.device_to_view_depth[1], q * 100.0);
        // View depth `[1] / (d - [0])`: the near plane at depth 1, the far plane at 0.
        let view =
            |d: f32| constants.device_to_view_depth[1] / (d - constants.device_to_view_depth[0]);
        assert!((view(1.0) - 0.1).abs() < 1e-5);
        assert!((view(0.0) - 100.0).abs() < 1e-2);
        // tan(30°) × aspect and its inverse.
        let tan = (core::f32::consts::FRAC_PI_6).tan();
        assert!((constants.device_to_view_depth[3] - tan).abs() < 1e-6);
        assert!((constants.device_to_view_depth[2] - tan * 16.0 / 9.0).abs() < 1e-6);
        assert!((constants.tan_half_fov - tan * 16.0 / 9.0).abs() < 1e-5);

        // A smaller render size: the sequence length steps by one, the frame counts on.
        constants.advance(&inputs((960, 540)), [1920, 1080], false, 32);
        assert_eq!(constants.previous_frame_render_size, [1280, 720]);
        assert_eq!(constants.previous_frame_jitter, [0.25, -0.125]);
        assert_eq!(constants.jitter_sequence_length, 19.0);
        assert_eq!(constants.frame_index, 1.0);
        assert_eq!(constants.downscale_factor, [0.5, 0.5]);
        constants.advance(&inputs((960, 540)), [1920, 1080], true, 32);
        assert_eq!(constants.jitter_sequence_length, 32.0);
        assert_eq!(constants.frame_index, 0.0);
    }

    #[test]
    fn spd_and_rcas_set_up_as_the_sdk() {
        let (spd, groups) = SpdConstants::new([1280, 720]);
        assert_eq!(groups, [20, 12]);
        assert_eq!(spd.num_work_groups, 240);
        assert_eq!(spd.mips, 10);
        assert_eq!(SpdConstants::new([64, 36]).0.mips, 6);
        assert_eq!(SpdConstants::new([65, 1]).1, [2, 1]);
        // Sharpness 1 is 0 stops: exp2(0) = 1.
        let rcas = RcasConstants::new(1.0);
        assert_eq!(rcas.config, [1.0f32.to_bits(), 0x3c00_3c00, 0, 0]);
        // 0.25 is 1.5 stops.
        let rcas = RcasConstants::new(0.25);
        assert_eq!(rcas.config[0], (-1.5f32).exp2().to_bits());
    }
}
