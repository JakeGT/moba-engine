//! `fsr3_smoke` (ADR-0021 §4b and §4c, N1b): the FSR 3.1.6 port on synthetic inputs.
//!
//! The scene is an analytic colour field over the output plane, so every frame's
//! render-resolution input and the ideal output are known exactly. Each render pixel `i`
//! samples the field at `i + 0.5 − jitter` (Halton (2, 3), ADR-0021 §1), scaled to output
//! pixels; depth is a constant plane (two depths in the occluder runs), the masks are zero and
//! the exposure is 1. Outside the
//! frame's render rectangle every input holds a sentinel (colour 1000, motion ±40 px, depth 0,
//! masks 1), so a pass that read outside the rectangle would show: besides the interior
//! bounds, every checkpoint bounds the largest error over the whole output, edges included.
//!
//! The run, on one context created for Quality at 192×108 (render 128×72):
//!
//! 1. **flat**: a constant colour converges to itself (every output texel within 1 %);
//! 2. **static**: after a reset, a smooth field converges to the ideal output;
//! 3. **moving**: the field moves one render pixel a frame with matching motion vectors and
//!    the output tracks it;
//! 4. **sharpened**: as 2 with sharpness 0.25 (accumulate-and-sharpen, then RCAS): within its
//!    bound, and with more local contrast than 2's output;
//! 5. **sizes** (ADR-0021 §4c): without a reset, the render size steps through Balanced,
//!    Performance, Ultra Performance, an odd 77×43 and back to Quality, all inside the one
//!    context; the first frame after each change and the converged one are bounded. Then
//!    9×5, 2×1 and 1×1 (finite and inside the image only) and back to Quality again;
//! 6. **fine**: a field with detail near the render resolution's limit, still and then moving
//!    one render pixel a frame: one frame cannot resolve it, so the converged error must be
//!    at most 0.9× the first frame's (0.95× moving, which holds only when the reprojection is
//!    right);
//! 7. **occluder** (ADR-0021 amendment 4): a box at a nearer depth crosses the still fine
//!    field 3 output pixels a frame, with motion vectors on the box only: a depth edge, and a
//!    disoccluded trail behind it. The trail the box left over 40 frames before the last must
//!    converge to at most 0.9× the fine field's single-frame error, which needs each frame's
//!    reconstructed previous depth cleared to the far plane (without the clear the whole trail
//!    stays disoccluded: 0.97×);
//! 8. **occluder across a size step**: the same over a coarse checkerboard of the fine field,
//!    stepping from Performance to Quality halfway without a reset. The background below the
//!    box's rows must keep its error within 5 % of the frame before for the two frames after
//!    the step, which needs the previous frame's render size in the constants (a stale one
//!    reads the previous luma and accumulation at the wrong scale: 11 % lower).
//!
//! The concurrency a renderer has is reproduced: two frames are in flight (each frame is
//! submitted without waiting, and its slot's staging and readback buffers are reused two
//! frames later), and the output alternates between two images every two frames (so each
//! parity's descriptors see both).
//!
//! The whole run happens twice on fresh contexts, and every output frame of the second must
//! be bit-identical to the first. Before the first run's first frame every internal image is
//! filled with garbage (NaN, all ones) through [`Fsr3::poison`]; the second run starts from
//! fresh allocations, so the comparison also shows that nothing in an internal image before
//! the first frame reaches any output. Any validation or synchronization-validation message
//! fails it (as every smoke). Last, the context's `vram_bytes` is reported at real sizes.

use std::collections::VecDeque;

use ash::vk;
use gpu_allocator::MemoryLocation;

use super::bindless::host_read_barrier;
use super::{Headless, SmokeReport};
use crate::bindless::{PendingSubmit, submit};
use crate::error::{RenderError, RenderResult};
use crate::fsr3::{Fsr3, Fsr3Mode, jitter_phases};
use crate::passes::vfx::f16_bits;
use crate::resources::{GpuBuffer, GpuImage};
use crate::upscale::{TemporalUpscaler, UpscaleImage, UpscaleInputs, UpscaleMode, UpscaleOutput};

/// The output size.
const OUTPUT: vk::Extent2D = vk::Extent2D {
    width: 192,
    height: 108,
};
/// Output texels this close to an edge are left out of the interior metrics: FSR clamps its
/// samples there and the moving fields enter from off screen. The full-image bound covers
/// them.
const MARGIN: u32 = 6;
/// The flat colour.
const FLAT: [f32; 3] = [0.6, 0.3, 0.15];
/// Depth everywhere inside the render rectangle (0 near, 1 far).
const DEPTH: f32 = 0.5;

/// Error bounds against the ideal output, as (mean, max) absolute error per channel over the
/// interior. About twice what lavapipe measures (`docs/perf/0030` has the numbers): loose
/// enough for any conformant device's rounding, tight enough that a wrong resource, parity,
/// constant or jitter sign fails.
const FLAT_BOUND: (f32, f32) = (0.002, 0.004);
const STATIC_BOUND: (f32, f32) = (0.012, 0.06);
const MOVING_BOUND: (f32, f32) = (0.012, 0.07);
const SHARPENED_BOUND: (f32, f32) = (0.015, 0.07);
const SIZES_BOUND: (f32, f32) = (0.025, 0.1);
/// The first frame after a render-size change, before the history settles at the new size.
const SIZE_STEP_BOUND: (f32, f32) = (0.025, 0.12);
/// The fine field's first frame after a reset: one jittered sample, upscaled. Only the mean is
/// bounded (clean runs: 0.039); a jitter flipped on either axis alone misplaces the detail
/// (0.056 for y, 0.071 for x).
const FINE_FIRST_BOUND: (f32, f32) = (0.05, 1.0);
/// The fine fields converged: only their ratio to the first frame is checked.
const UNBOUNDED: (f32, f32) = (1.0, 1.0);
/// The largest error anywhere in the output, edges included (clean runs: at most 0.27, at the
/// fine field's edges). A sentinel that leaked from outside the render rectangle is 1000, and
/// a render size in the constants stuck at the maximum gives errors up to 762 here.
const FULL_BOUND: f32 = 0.5;
/// The same at 9×5, 2×1 and 1×1, where the output is a few blurred samples of the field.
const TINY_FULL_BOUND: f32 = 1.5;
/// Converged, the still fine field's error must be at most this fraction of its first
/// frame's (clean runs: 0.79): the accumulation recovers detail one frame lacks.
const CONVERGENCE: f32 = 0.9;
/// The same for the moving fine field, which keeps detail only if the reprojection is right
/// (clean runs: 0.84 to 0.88; motion vectors misscaled: 1.003, ignored: 1.7). Do not loosen
/// it: RADV's clean 0.88 is about 7 % under it, and above about 1.0 it stops catching
/// misscaled vectors.
const MOVING_CONVERGENCE: f32 = 0.95;
/// RCAS at sharpness 0.25 must raise the converged static field's local contrast by at least
/// this factor over the unsharpened output (clean runs: 1.24; RCAS skipped: 1.0).
const SHARPENING: f32 = 1.02;

const STATIC: &str = "static";
const SHARPENED: &str = "sharpened";
const FINE_FIRST: &str = "fine: first frame after the reset";
const FINE: &str = "fine: converged";
const FINE_MOVING_FIRST: &str = "fine moving: first frame after the reset";
const FINE_MOVING: &str = "fine moving: converged";
const OCCLUDER_RUN: &str = "occluder";
const OCCLUDER_STEP: &str = "occluder across a size step";
/// Frames of each occluder run (half of them after the size step).
const OCCLUDER_FRAMES: u32 = 60;
/// The trail checkpoints: label, and the newest and oldest frames since the box left.
const OCCLUDER_TRAILS: [(&str, u32, u32); 3] = [
    ("2-8 frames old", 2, 8),
    ("8-24 frames old", 8, 24),
    (OLD_TRAIL, 40, 60),
];
const OLD_TRAIL: &str = "over 40 frames old";
/// The occluder's oldest trail must converge to at most this fraction of the fine field's
/// single-frame error (clean runs: 0.785 on lavapipe). Each frame's reconstructed previous
/// depth must start at the far plane: without that clear, every place the box ever covered
/// stays disoccluded and the trail stays at the single-frame error (0.97).
const TRAIL_CONVERGENCE: f32 = 0.9;
/// Across the size step, the converged background's error stays within this fraction of the
/// frame before (clean runs: 1.0 % and 1.7 % lower on the two frames after the step, on
/// lavapipe). With the previous render size stale (the current one), FSR reads the previous
/// frame's luma and accumulation at the wrong scale and the error jumps by 11 % (lavapipe).
const STEP_CONTINUITY: f32 = 0.05;

/// The analytic field at output position `(u, v)` (output pixels), linear HDR.
fn field(u: f32, v: f32) -> [f32; 3] {
    use core::f32::consts::TAU;
    [
        0.5 + 0.25 * (TAU * u / 32.0).sin(),
        0.5 + 0.25 * (TAU * v / 24.0).cos(),
        0.4 + 0.2 * (TAU * (u + v) / 40.0).sin(),
    ]
}

/// A field with detail near the render resolution's limit (periods of 5 and 6 output pixels,
/// 3.3 and 4 render pixels at Quality): one frame cannot resolve it, accumulated jittered
/// frames can.
fn fine(u: f32, v: f32) -> [f32; 3] {
    use core::f32::consts::TAU;
    [
        0.5 + 0.25 * (TAU * u / 5.0).sin(),
        0.5 + 0.25 * (TAU * v / 6.0).cos(),
        0.4,
    ]
}

/// The occluder's colour: far from every value of the fine field behind it.
const OCCLUDER: [f32; 3] = [0.1, 0.2, 0.9];
/// The occluder's rectangle at the start of its run, output pixels: left edge, width, top
/// and bottom.
const OCCLUDER_LEFT: f32 = 8.0;
const OCCLUDER_WIDTH: f32 = 30.0;
const OCCLUDER_TOP: f32 = 30.0;
const OCCLUDER_BOTTOM: f32 = 78.0;
/// Its speed, output pixels a frame (2 render pixels at Quality).
const OCCLUDER_SPEED: f32 = 3.0;
/// Depths of the occluder and of the background behind it (0 near).
const OCCLUDER_DEPTH: f32 = 0.3;
/// The checkerboard's cell, output pixels.
const CHECKER: f32 = 24.0;
const BACKGROUND_DEPTH: f32 = 0.6;

/// What the frame shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Content {
    Flat,
    /// The smooth field, shifted right by this many output pixels.
    Field(f32),
    /// The fine field, shifted right by this many output pixels.
    Fine(f32),
    /// The still fine field at [`BACKGROUND_DEPTH`] behind an [`OCCLUDER`] box at
    /// [`OCCLUDER_DEPTH`] whose left edge is this many output pixels from the left. Only the
    /// box moves: a depth edge, and a disoccluded trail behind it. With `true`, the field is
    /// modulated by a coarse checkerboard (strong luma structure at the scale of FSR's
    /// shading-change pyramid).
    Occluded(f32, bool),
}

impl Content {
    fn at(self, u: f32, v: f32) -> [f32; 3] {
        match self {
            Self::Flat => FLAT,
            Self::Field(shift) => field(u - shift, v),
            Self::Fine(shift) => fine(u - shift, v),
            Self::Occluded(..) if self.in_front(u, v) => OCCLUDER,
            Self::Occluded(_, false) => fine(u, v),
            Self::Occluded(_, true) => {
                let dark = ((u / CHECKER).floor() + (v / CHECKER).floor()) as i32 % 2 == 0;
                fine(u, v).map(|c| c * if dark { 0.3 } else { 1.3 })
            }
        }
    }

    /// Whether `(u, v)` shows the occluder (the content that moves; all of it otherwise).
    fn in_front(self, u: f32, v: f32) -> bool {
        match self {
            Self::Occluded(left, _) => {
                (left..left + OCCLUDER_WIDTH).contains(&u)
                    && (OCCLUDER_TOP..OCCLUDER_BOTTOM).contains(&v)
            }
            _ => true,
        }
    }

    fn depth(self, u: f32, v: f32) -> f32 {
        match self {
            Self::Occluded(..) if self.in_front(u, v) => OCCLUDER_DEPTH,
            Self::Occluded(..) => BACKGROUND_DEPTH,
            _ => DEPTH,
        }
    }

    /// The same content shifted right by `shift` output pixels.
    fn shifted(self, shift: f32) -> Self {
        match self {
            Self::Flat => Self::Flat,
            Self::Field(_) => Self::Field(shift),
            Self::Fine(_) => Self::Fine(shift),
            Self::Occluded(_, checker) => Self::Occluded(OCCLUDER_LEFT + shift, checker),
        }
    }
}

/// Halton radical inverse of `index` in `base`.
fn halton(mut index: u32, base: u32) -> f32 {
    let (mut result, mut fraction) = (0.0f32, 1.0f32);
    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }
    result
}

/// Bytes of one frame's inputs at the context's maximum render size, sentinels outside the
/// render rectangle.
struct Upload {
    color: Vec<u8>,
    depth: Vec<u8>,
    motion: Vec<u8>,
    masks: Vec<u8>,
}

fn upload(
    max: vk::Extent2D,
    render: vk::Extent2D,
    content: Content,
    jitter: [f32; 2],
    motion: [f32; 2],
) -> Upload {
    let texels = (max.width * max.height) as usize;
    let mut out = Upload {
        color: Vec::with_capacity(texels * 8),
        depth: Vec::with_capacity(texels * 4),
        motion: Vec::with_capacity(texels * 4),
        masks: Vec::with_capacity(texels),
    };
    let scale = [
        OUTPUT.width as f32 / render.width as f32,
        OUTPUT.height as f32 / render.height as f32,
    ];
    for y in 0..max.height {
        for x in 0..max.width {
            let inside = x < render.width && y < render.height;
            let (rgb, depth, mv, mask) = if inside {
                let u = (x as f32 + 0.5 - jitter[0]) * scale[0];
                let v = (y as f32 + 0.5 - jitter[1]) * scale[1];
                // What does not move (the occluder's background) has no motion.
                let mv = if content.in_front(u, v) {
                    motion
                } else {
                    [0.0; 2]
                };
                (content.at(u, v), content.depth(u, v), mv, 0u8)
            } else {
                ([1000.0; 3], 0.0, [40.0, -40.0], 255u8)
            };
            for channel in [rgb[0], rgb[1], rgb[2], 1.0] {
                out.color.extend(f16_bits(channel).to_le_bytes());
            }
            out.depth.extend(depth.to_le_bytes());
            for channel in mv {
                out.motion.extend(f16_bits(channel).to_le_bytes());
            }
            out.masks.push(mask);
        }
    }
    out
}

/// IEEE half to `f32`.
fn f16_value(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * (-24f32).exp2(),
        31 if mantissa == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + mantissa / 1024.0) * ((exponent - 15) as f32).exp2(),
    }
}

/// What one checkpoint measured on the output.
#[derive(Clone, Copy, Debug)]
struct Measured {
    /// Mean and largest absolute error per channel over the interior.
    mean: f32,
    max: f32,
    /// The largest error over the whole image, edges included.
    full_max: f32,
    /// Every texel finite.
    finite: bool,
    /// Local contrast: the mean absolute 4-neighbour Laplacian of the channel sum over the
    /// interior.
    contrast: f32,
    /// FNV-1a of the output frame.
    hash: u64,
}

/// Measures `rgba` (the output's halves) against `content`, the interior metrics over `region`
/// (or every texel at least [`MARGIN`] from an edge).
fn measure(rgba: &[u8], content: Content, region: Option<[u32; 4]>) -> Measured {
    let texel = |x: u32, y: u32| -> [f32; 3] {
        let at = ((y * OUTPUT.width + x) * 8) as usize;
        core::array::from_fn(|c| {
            f16_value(u16::from_le_bytes([rgba[at + c * 2], rgba[at + c * 2 + 1]]))
        })
    };
    let (mut sum, mut max, mut full_max, mut count, mut finite) =
        (0.0f64, 0.0f32, 0.0f32, 0u32, true);
    let (mut contrast, mut contrast_count) = (0.0f64, 0u32);
    for y in 0..OUTPUT.height {
        for x in 0..OUTPUT.width {
            let value = texel(x, y);
            finite &= value.iter().all(|v| v.is_finite());
            let ideal = content.at(x as f32 + 0.5, y as f32 + 0.5);
            let [x0, y0, x1, y1] = region.unwrap_or([
                MARGIN,
                MARGIN,
                OUTPUT.width - MARGIN,
                OUTPUT.height - MARGIN,
            ]);
            let interior = x >= x0.max(1)
                && y >= y0.max(1)
                && x < x1.min(OUTPUT.width - 1)
                && y < y1.min(OUTPUT.height - 1);
            for c in 0..3 {
                // NaN fails `finite`; keep it out of the maxima so they stay readable.
                let e = (value[c] - ideal[c]).abs();
                if e.is_finite() {
                    full_max = full_max.max(e);
                }
                if interior {
                    sum += f64::from(e);
                    if e.is_finite() {
                        max = max.max(e);
                    }
                    count += 1;
                }
            }
            if interior {
                let luma = |[r, g, b]: [f32; 3]| r + g + b;
                let laplacian = 4.0 * luma(value)
                    - luma(texel(x - 1, y))
                    - luma(texel(x + 1, y))
                    - luma(texel(x, y - 1))
                    - luma(texel(x, y + 1));
                contrast += f64::from(laplacian.abs());
                contrast_count += 1;
            }
        }
    }
    Measured {
        mean: (sum / f64::from(count.max(1))) as f32,
        max,
        full_max,
        finite,
        contrast: (contrast / f64::from(contrast_count.max(1))) as f32,
        hash: fnv(0xcbf2_9ce4_8422_2325, rgba),
    }
}

/// FNV-1a over `bytes`, continuing from `hash`.
fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One frame of the run.
#[derive(Clone, Copy, Debug)]
struct Frame {
    render: vk::Extent2D,
    content: Content,
    /// Motion in render pixels (current to previous).
    motion: [f32; 2],
    /// Reset through the inputs' flag.
    reset: bool,
    /// Reset through `Fsr3::reset_history` before recording.
    reset_history: bool,
    sharpness: f32,
}

/// A check made on one frame's output.
#[derive(Clone, Debug)]
struct Checkpoint {
    name: String,
    content: Content,
    /// The output rectangle the interior metrics cover (`[x0, y0, x1, y1)`), or every texel at
    /// least [`MARGIN`] from an edge.
    region: Option<[u32; 4]>,
    /// (mean, max) over the interior.
    bound: (f32, f32),
    /// The largest error anywhere.
    full: f32,
}

/// One phase of the run: `frames` frames of `content` at `render`, then a checkpoint.
#[derive(Clone, Copy, Debug)]
struct Phase {
    name: &'static str,
    frames: u32,
    render: vk::Extent2D,
    content: Content,
    /// Horizontal motion in render pixels (current to previous): the content moves the other
    /// way.
    motion: f32,
    /// Reset on the first frame through the flag.
    reset: bool,
    /// Reset on the first frame through `Fsr3::reset_history`.
    reset_history: bool,
    sharpness: f32,
    bound: (f32, f32),
    full: f32,
    /// A bound for the phase's first frame too (a render-size change).
    first: Option<(f32, f32)>,
}

fn extent(width: u32, height: u32) -> vk::Extent2D {
    vk::Extent2D { width, height }
}

/// The phases, in order (see the module documentation).
#[allow(clippy::too_many_lines)] // One table.
fn phases() -> Vec<Phase> {
    let quality = Fsr3Mode::Quality.render_size(OUTPUT);
    let converge = 2 * jitter_phases(quality, OUTPUT);
    let field = Phase {
        name: STATIC,
        frames: converge,
        render: quality,
        content: Content::Field(0.0),
        motion: 0.0,
        reset: true,
        reset_history: false,
        sharpness: 0.0,
        bound: STATIC_BOUND,
        full: FULL_BOUND,
        first: None,
    };
    let mut phases = vec![
        Phase {
            name: "flat",
            frames: 12,
            content: Content::Flat,
            reset: false,
            bound: FLAT_BOUND,
            ..field
        },
        field,
        Phase {
            name: "moving",
            motion: -1.0,
            reset: false,
            bound: MOVING_BOUND,
            ..field
        },
        Phase {
            name: SHARPENED,
            reset: false,
            reset_history: true,
            sharpness: 0.25,
            bound: SHARPENED_BOUND,
            ..field
        },
    ];
    for (name, render) in [
        ("sizes: balanced", Fsr3Mode::Balanced.render_size(OUTPUT)),
        (
            "sizes: performance",
            Fsr3Mode::Performance.render_size(OUTPUT),
        ),
        (
            "sizes: ultra performance",
            Fsr3Mode::UltraPerformance.render_size(OUTPUT),
        ),
        ("sizes: odd 77x43", extent(77, 43)),
        ("sizes: back to quality", quality),
    ] {
        phases.push(Phase {
            name,
            frames: jitter_phases(render, OUTPUT).min(40),
            render,
            reset: false,
            bound: SIZES_BOUND,
            first: Some(SIZE_STEP_BOUND),
            ..field
        });
    }
    // The smallest sizes: no reference to converge to, only finite output inside the image's
    // range and no read outside the rectangle; then the context must recover.
    for (name, render) in [
        ("sizes: tiny 9x5", extent(9, 5)),
        ("sizes: tiny 2x1", extent(2, 1)),
        ("sizes: tiny 1x1", extent(1, 1)),
    ] {
        phases.push(Phase {
            name,
            frames: 8,
            render,
            reset: false,
            bound: UNBOUNDED,
            full: TINY_FULL_BOUND,
            ..field
        });
    }
    phases.push(Phase {
        name: "sizes: quality after 1x1",
        frames: converge,
        reset: false,
        bound: SIZES_BOUND,
        ..field
    });
    for (first_name, name, motion) in [
        (FINE_FIRST, FINE, 0.0),
        (FINE_MOVING_FIRST, FINE_MOVING, -1.0),
    ] {
        let fine = Phase {
            name: first_name,
            frames: 1,
            content: Content::Fine(0.0),
            motion,
            bound: FINE_FIRST_BOUND,
            ..field
        };
        phases.push(fine);
        phases.push(Phase {
            name,
            frames: converge - 1,
            reset: false,
            bound: UNBOUNDED,
            ..fine
        });
    }
    phases
}

/// The run as frames, with the checkpoints made on each.
fn script() -> Vec<(Frame, Vec<Checkpoint>)> {
    let mut frames = Vec::new();
    for phase in phases() {
        for index in 0..phase.frames {
            // One render pixel is `OUTPUT / render` output pixels.
            let shift =
                -phase.motion * OUTPUT.width as f32 / phase.render.width as f32 * index as f32;
            let content = phase.content.shifted(shift);
            let frame = Frame {
                render: phase.render,
                content,
                motion: [phase.motion, 0.0],
                reset: phase.reset && index == 0,
                reset_history: phase.reset_history && index == 0,
                sharpness: phase.sharpness,
            };
            let mut checks = Vec::new();
            if let (0, Some(bound)) = (index, phase.first) {
                checks.push(Checkpoint {
                    name: format!("{}: first frame", phase.name),
                    content,
                    region: None,
                    bound,
                    full: phase.full,
                });
            }
            if index + 1 == phase.frames {
                checks.push(Checkpoint {
                    name: String::from(phase.name),
                    content,
                    region: None,
                    bound: phase.bound,
                    full: phase.full,
                });
            }
            frames.push((frame, checks));
        }
    }
    frames.extend(occluder_frames());
    frames
}

/// The checkpoint of the background below the occluder `offset` frames after the size step.
fn step_name(offset: i64) -> String {
    format!("{OCCLUDER_STEP}: below the box, frame {offset} after the step")
}

/// The trail the occluder left between `newest` and `oldest` frames before frame `frame` of
/// its run (`[x0, y0, x1, y1)`, clear of the box's edges by 3 output pixels), if any.
fn trail(frame: u32, newest: u32, oldest: u32) -> Option<[u32; 4]> {
    let at = |age: u32| OCCLUDER_LEFT + OCCLUDER_SPEED * frame.saturating_sub(age) as f32;
    let x0 = at(oldest).max(OCCLUDER_LEFT + 3.0).ceil() as u32;
    let x1 = at(newest).floor() as u32;
    (frame >= newest + 2 && x1 > x0).then_some([
        x0,
        (OCCLUDER_TOP + 3.0) as u32,
        x1,
        (OCCLUDER_BOTTOM - 3.0) as u32,
    ])
}

/// The occluder runs (see the module documentation): the box crosses the still fine field
/// from a reset, once at Quality and once stepping from Performance to Quality halfway.
fn occluder_frames() -> Vec<(Frame, Vec<Checkpoint>)> {
    let quality = Fsr3Mode::Quality.render_size(OUTPUT);
    let performance = Fsr3Mode::Performance.render_size(OUTPUT);
    let mut frames = Vec::new();
    for (name, checker, legs) in [
        (
            OCCLUDER_RUN,
            false,
            [
                (quality, OCCLUDER_FRAMES / 2),
                (quality, OCCLUDER_FRAMES / 2),
            ],
        ),
        (
            OCCLUDER_STEP,
            true,
            [
                (performance, OCCLUDER_FRAMES / 2),
                (quality, OCCLUDER_FRAMES / 2),
            ],
        ),
    ] {
        let mut index = 0;
        for (render, count) in legs {
            for _ in 0..count {
                let content =
                    Content::Occluded(OCCLUDER_LEFT + OCCLUDER_SPEED * index as f32, checker);
                // Render pixels from the box's sample to where it was: it moves right.
                let motion = -OCCLUDER_SPEED * render.width as f32 / OUTPUT.width as f32;
                let frame = Frame {
                    render,
                    content,
                    motion: [motion, 0.0],
                    reset: index == 0,
                    reset_history: false,
                    sharpness: 0.0,
                };
                let mut checks = Vec::new();
                // The background below the box's rows, which it never crosses: around the
                // size step (or the same frames without one).
                let below = [
                    MARGIN,
                    OCCLUDER_BOTTOM as u32 + 3,
                    OUTPUT.width - MARGIN,
                    OUTPUT.height - MARGIN,
                ];
                let half = OCCLUDER_FRAMES / 2;
                if checker && (half - 1..half + 2).contains(&index) {
                    checks.push(Checkpoint {
                        name: step_name(i64::from(index) - i64::from(half)),
                        content,
                        region: Some(below),
                        bound: UNBOUNDED,
                        full: TINY_FULL_BOUND,
                    });
                }
                if index + 1 == OCCLUDER_FRAMES {
                    for (label, newest, oldest) in OCCLUDER_TRAILS {
                        checks.push(Checkpoint {
                            name: format!("{name}: trail {label}"),
                            content,
                            region: trail(index, newest, oldest),
                            bound: UNBOUNDED,
                            full: TINY_FULL_BOUND,
                        });
                    }
                }
                frames.push((frame, checks));
                index += 1;
            }
        }
    }
    frames
}

/// A frame slot: its staging and readback buffers, reused two frames later.
struct Slot {
    staging: GpuBuffer,
    readback: GpuBuffer,
}

/// The caller-owned images: inputs at the maximum render size, two outputs, and a slot per
/// frame in flight.
struct Targets {
    color: GpuImage,
    depth: GpuImage,
    motion: GpuImage,
    reactive: GpuImage,
    transparency: GpuImage,
    exposure: GpuImage,
    outputs: [GpuImage; 2],
    slots: [Slot; 2],
}

impl Targets {
    fn new(gpu: &Headless, max: vk::Extent2D) -> RenderResult<Self> {
        let device = &gpu.device;
        let input = |name: &str, extent, format, aspect| {
            GpuImage::new(
                device.device.clone(),
                device.allocator(),
                name,
                extent,
                format,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                aspect,
            )
        };
        let output = |name: &str| {
            GpuImage::new(
                device.device.clone(),
                device.allocator(),
                name,
                OUTPUT,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
                vk::ImageAspectFlags::COLOR,
            )
        };
        let texels = u64::from(max.width * max.height);
        let slot = |name: &str| -> RenderResult<Slot> {
            Ok(Slot {
                // Colour 8, depth 4, motion 4, two masks 1 each, and the exposure.
                staging: GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    &format!("fsr3-smoke-staging-{name}"),
                    texels * 18 + 4,
                    vk::BufferUsageFlags::TRANSFER_SRC,
                    MemoryLocation::CpuToGpu,
                )?,
                readback: GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    &format!("fsr3-smoke-readback-{name}"),
                    u64::from(OUTPUT.width * OUTPUT.height) * 8,
                    vk::BufferUsageFlags::TRANSFER_DST,
                    MemoryLocation::GpuToCpu,
                )?,
            })
        };
        let color = vk::ImageAspectFlags::COLOR;
        Ok(Self {
            color: input(
                "fsr3-smoke-color",
                max,
                vk::Format::R16G16B16A16_SFLOAT,
                color,
            )?,
            depth: input(
                "fsr3-smoke-depth",
                max,
                vk::Format::D32_SFLOAT,
                vk::ImageAspectFlags::DEPTH,
            )?,
            motion: input("fsr3-smoke-motion", max, vk::Format::R16G16_SFLOAT, color)?,
            reactive: input("fsr3-smoke-reactive", max, vk::Format::R8_UNORM, color)?,
            transparency: input("fsr3-smoke-tc", max, vk::Format::R8_UNORM, color)?,
            exposure: input(
                "fsr3-smoke-exposure",
                extent(1, 1),
                vk::Format::R32_SFLOAT,
                color,
            )?,
            outputs: [
                output("fsr3-smoke-output-a")?,
                output("fsr3-smoke-output-b")?,
            ],
            slots: [slot("a")?, slot("b")?],
        })
    }

    fn image(image: &GpuImage, format: vk::Format, extent: vk::Extent2D) -> UpscaleImage {
        UpscaleImage {
            image: image.image(),
            view: image.view(),
            format,
            extent,
        }
    }
}

/// What one run produced.
struct Run {
    /// FNV-1a of every output frame.
    hashes: Vec<u64>,
    results: Vec<(Checkpoint, Measured)>,
    vram_bytes: u64,
    /// The context's answers to the ADR-0021 §4a queries.
    facts: Vec<String>,
    /// Whether it accepts one render pixel over its maximum, and an odd 77×43.
    accepts: (bool, bool),
}

/// A submitted frame: its slot and checks.
struct InFlight<'a> {
    submit: PendingSubmit<'a>,
    slot: usize,
    checks: Vec<Checkpoint>,
}

/// Waits for `frame` and reads its output back into `run`.
fn finish(frame: InFlight<'_>, targets: &Targets, run: &mut Run) -> RenderResult<()> {
    frame.submit.wait()?;
    let mut bytes = vec![0u8; (OUTPUT.width * OUTPUT.height * 8) as usize];
    if !targets.slots[frame.slot].readback.read_bytes(0, &mut bytes) {
        return Err(RenderError::Shader(
            "fsr3 smoke readback is not host visible",
        ));
    }
    run.hashes.push(fnv(0xcbf2_9ce4_8422_2325, &bytes));
    for checkpoint in frame.checks {
        let measured = measure(&bytes, checkpoint.content, checkpoint.region);
        run.results.push((checkpoint, measured));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Uploads, barriers, the upscale and the readback, in order.
fn run_once(gpu: &Headless, poison: bool) -> RenderResult<Run> {
    let device = &gpu.device;
    let max = Fsr3Mode::Quality.render_size(OUTPUT);
    let mut fsr = Fsr3::new(device, max, OUTPUT)?;
    let mut targets = Targets::new(gpu, max)?;
    let texels = (max.width * max.height) as usize;
    let mut run = Run {
        hashes: Vec::new(),
        results: Vec::new(),
        vram_bytes: fsr.vram_bytes(),
        facts: Vec::new(),
        accepts: (true, false),
    };
    run.facts.push(format!(
        "back end {} (sharpens itself: {}), context max render {}x{}, output {}x{}",
        fsr.kind().name(),
        fsr.has_sharpening(),
        max.width,
        max.height,
        OUTPUT.width,
        OUTPUT.height
    ));
    for &mode in UpscaleMode::ALL {
        let (low, high) = fsr.render_size_range(OUTPUT, mode);
        run.facts.push(format!(
            "{mode:?}: render sizes {}x{} to {}x{}, {} jitter phases at the top",
            low.width,
            low.height,
            high.width,
            high.height,
            fsr.jitter_phases(high, OUTPUT)
        ));
    }
    run.accepts = (
        fsr.accepts(extent(max.width + 1, max.height), OUTPUT),
        fsr.accepts(extent(77, 43), OUTPUT),
    );
    let mut jitter_index = 0u32;
    let mut output_used = [false; 2];
    let mut in_flight: VecDeque<InFlight<'_>> = VecDeque::with_capacity(2);
    for (number, (frame, checks)) in script().into_iter().enumerate() {
        let slot = number % 2;
        // Two frames in flight: the frame two back used this slot (and this parity of the
        // context), so wait for it before reusing either.
        if in_flight.len() == 2 {
            let oldest = in_flight.pop_front().expect("two frames in flight");
            finish(oldest, &targets, &mut run)?;
        }
        if frame.reset_history {
            fsr.reset_history();
        }
        let phases = fsr.jitter_phases(frame.render, OUTPUT);
        jitter_index += 1;
        let index = (jitter_index - 1) % phases + 1;
        let jitter = [halton(index, 2) - 0.5, halton(index, 3) - 0.5];
        let bytes = upload(max, frame.render, frame.content, jitter, frame.motion);
        let staging = &mut targets.slots[slot].staging;
        let mut offset = 0;
        let mut offsets = [0u64; 6];
        for (index, data) in [
            &bytes.color,
            &bytes.depth,
            &bytes.motion,
            &bytes.masks,
            &vec![0u8; texels],
            &1.0f32.to_le_bytes().to_vec(),
        ]
        .into_iter()
        .enumerate()
        {
            staging.write_bytes(offset, data);
            offsets[index] = offset as u64;
            offset += data.len();
        }
        // The output alternates every two frames, so each parity sees both images.
        let target = (number >> 1) & 1;
        let output_first = !core::mem::replace(&mut output_used[target], true);
        // The reactive mask is `masks` (1 outside the rectangle); the transparency and
        // composition mask is all zero.
        let inputs = UpscaleInputs {
            color: Targets::image(&targets.color, vk::Format::R16G16B16A16_SFLOAT, max),
            depth: Targets::image(&targets.depth, vk::Format::D32_SFLOAT, max),
            motion: Targets::image(&targets.motion, vk::Format::R16G16_SFLOAT, max),
            reactive: Targets::image(&targets.reactive, vk::Format::R8_UNORM, max),
            transparency: Targets::image(&targets.transparency, vk::Format::R8_UNORM, max),
            exposure: Targets::image(&targets.exposure, vk::Format::R32_SFLOAT, extent(1, 1)),
            jitter,
            render_size: frame.render,
            dt_ms: 16.0,
            near: 0.1,
            far: 100.0,
            fov_y: core::f32::consts::FRAC_PI_3,
            reset: frame.reset,
            sharpness: frame.sharpness,
            frame_slot: slot,
        };
        let output = UpscaleOutput {
            image: Targets::image(
                &targets.outputs[target],
                vk::Format::R16G16B16A16_SFLOAT,
                OUTPUT,
            ),
        };
        let first = number == 0;
        let images = [
            (&targets.color, vk::ImageAspectFlags::COLOR, max, offsets[0]),
            (&targets.depth, vk::ImageAspectFlags::DEPTH, max, offsets[1]),
            (
                &targets.motion,
                vk::ImageAspectFlags::COLOR,
                max,
                offsets[2],
            ),
            (
                &targets.reactive,
                vk::ImageAspectFlags::COLOR,
                max,
                offsets[3],
            ),
            (
                &targets.transparency,
                vk::ImageAspectFlags::COLOR,
                max,
                offsets[4],
            ),
            (
                &targets.exposure,
                vk::ImageAspectFlags::COLOR,
                extent(1, 1),
                offsets[5],
            ),
        ];
        let staging = staging.handle();
        let readback = targets.slots[slot].readback.handle();
        let output_image = targets.outputs[target].image();
        let fsr = &mut fsr;
        let pending = submit(device, |cmd| {
            if first && poison {
                // SAFETY: `cmd` is recording on this device, and this frame's `record` below
                // follows in the same command buffer.
                unsafe { fsr.poison(cmd) };
            }
            // Inputs to transfer (after the previous frame's reads), the output from its last
            // copy to compute.
            let to_transfer: Vec<vk::ImageMemoryBarrier2> = images
                .iter()
                .map(|(image, aspect, _, _)| {
                    crate::bindless::image_barrier(
                        image.image(),
                        *aspect,
                        if first {
                            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
                        } else {
                            (
                                vk::PipelineStageFlags2::COMPUTE_SHADER,
                                vk::AccessFlags2::NONE,
                            )
                        },
                        (
                            vk::PipelineStageFlags2::COPY,
                            vk::AccessFlags2::TRANSFER_WRITE,
                        ),
                        if first {
                            vk::ImageLayout::UNDEFINED
                        } else {
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                        },
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    )
                })
                .chain(core::iter::once(crate::bindless::color_barrier(
                    output_image,
                    if output_first {
                        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
                    } else {
                        (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::NONE)
                    },
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                    if output_first {
                        vk::ImageLayout::UNDEFINED
                    } else {
                        vk::ImageLayout::GENERAL
                    },
                    vk::ImageLayout::GENERAL,
                )))
                .collect();
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
            );
            for (image, aspect, extent, offset) in &images {
                let region = vk::BufferImageCopy::default()
                    .buffer_offset(*offset)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(*aspect)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    });
                // SAFETY: the image was created with TRANSFER_DST and is in
                // TRANSFER_DST_OPTIMAL; this slot's staging buffer holds the whole image at
                // `offset` and the host finished writing it before this submission.
                unsafe {
                    device.device.cmd_copy_buffer_to_image(
                        cmd,
                        staging,
                        image.image(),
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        core::slice::from_ref(&region),
                    );
                }
            }
            let to_read: Vec<vk::ImageMemoryBarrier2> = images
                .iter()
                .map(|(image, aspect, _, _)| {
                    crate::bindless::image_barrier(
                        image.image(),
                        *aspect,
                        (
                            vk::PipelineStageFlags2::COPY,
                            vk::AccessFlags2::TRANSFER_WRITE,
                        ),
                        (
                            vk::PipelineStageFlags2::COMPUTE_SHADER,
                            vk::AccessFlags2::SHADER_SAMPLED_READ,
                        ),
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    )
                })
                .collect();
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_read),
            );

            // SAFETY: `cmd` is recording on this device and is submitted once, just below.
            // The record two back (the frame that used this slot) has finished: it was waited
            // for at the top of this iteration. The inputs and outputs live in `targets`,
            // which outlives every submission (each is waited for before `run_once` returns).
            unsafe { fsr.record(cmd, &inputs, &output) };

            let to_copy = crate::bindless::color_barrier(
                output_image,
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_STORAGE_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::GENERAL,
            );
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(core::slice::from_ref(&to_copy)),
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: OUTPUT.width,
                    height: OUTPUT.height,
                    depth: 1,
                });
            // SAFETY: the output was created with TRANSFER_SRC and is in GENERAL; this slot's
            // readback buffer holds the whole image and the host reads it only after this
            // submission's fence.
            unsafe {
                device.device.cmd_copy_image_to_buffer(
                    cmd,
                    output_image,
                    vk::ImageLayout::GENERAL,
                    readback,
                    core::slice::from_ref(&region),
                );
            }
            host_read_barrier(device, cmd);
        })?;
        in_flight.push_back(InFlight {
            submit: pending,
            slot,
            checks,
        });
    }
    while let Some(frame) = in_flight.pop_front() {
        finish(frame, &targets, &mut run)?;
    }
    Ok(run)
}

/// The context's `vram_bytes` at real sizes, one context resized through them (nothing is
/// recorded, so no GPU work is pending at any resize).
fn vram_at_real_sizes(gpu: &Headless) -> RenderResult<Vec<String>> {
    let sizes = [
        ("1920x1080 Quality", extent(1920, 1080), Fsr3Mode::Quality),
        ("2560x1440 Quality", extent(2560, 1440), Fsr3Mode::Quality),
        ("2560x1440 NativeAa", extent(2560, 1440), Fsr3Mode::NativeAa),
        (
            "3840x2160 Performance",
            extent(3840, 2160),
            Fsr3Mode::Performance,
        ),
    ];
    let mut notes = Vec::new();
    let mut fsr: Option<Fsr3> = None;
    for (name, output, mode) in sizes {
        let render = mode.render_size(output);
        match fsr.as_mut() {
            None => fsr = Some(Fsr3::new(&gpu.device, render, output)?),
            // Nothing was recorded, so no GPU work is pending (the device-idle precondition).
            Some(fsr) => fsr.resize(&gpu.device, output, render)?,
        }
        let bytes = fsr.as_ref().map_or(0, TemporalUpscaler::vram_bytes);
        notes.push(format!(
            "vram at {name} (render {}x{}): {:.1} MiB",
            render.width,
            render.height,
            bytes as f64 / f64::from(1u32 << 20)
        ));
    }
    Ok(notes)
}

#[allow(clippy::too_many_lines)] // One check after another.
pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let first = run_once(gpu, true)?;
    let second = run_once(gpu, false)?;
    report.notes.extend(first.facts.iter().cloned());
    report.check(
        || String::from("refuses a render size above the maximum, accepts 77x43"),
        first.accepts,
        (false, true),
    );
    let result = |name: &str| {
        first
            .results
            .iter()
            .find(|(checkpoint, _)| checkpoint.name == name)
            .map(|(_, measured)| *measured)
    };
    for (first_name, name, convergence) in [
        (FINE_FIRST, FINE, CONVERGENCE),
        (FINE_MOVING_FIRST, FINE_MOVING, MOVING_CONVERGENCE),
    ] {
        let single = result(first_name).map_or(f32::NAN, |measured| measured.mean);
        let converged = result(name).map_or(f32::NAN, |measured| measured.mean);
        report.notes.push(format!(
            "accumulation: {name} {converged:.5} vs {single:.5} on the first frame ({:.2}x)",
            converged / single
        ));
        report.check(
            || format!("{name}: {converged} at most {convergence} x the first frame's {single}"),
            converged <= convergence * single,
            true,
        );
    }
    // The occluder (ADR-0021 amendment 4): its trail converges once the box has left it.
    let single = result(FINE_FIRST).map_or(f32::NAN, |measured| measured.mean);
    let trail_name = format!("{OCCLUDER_RUN}: trail {OLD_TRAIL}");
    let old_trail = result(&trail_name).map_or(f32::NAN, |measured| measured.mean);
    report.notes.push(format!(
        "occluder: {trail_name} {old_trail:.5} vs {single:.5} for one frame ({:.3}x)",
        old_trail / single
    ));
    report.check(
        || format!("{trail_name}: {old_trail} at most {TRAIL_CONVERGENCE} x {single}"),
        old_trail <= TRAIL_CONVERGENCE * single,
        true,
    );
    // ... and the background's error carries across a size step.
    let before = result(&step_name(-1)).map_or(f32::NAN, |measured| measured.mean);
    for offset in [0, 1] {
        let name = step_name(offset);
        let after = result(&name).map_or(f32::NAN, |measured| measured.mean);
        let change = after / before - 1.0;
        report.notes.push(format!(
            "size step: {name} {after:.5} vs {before:.5} the frame before ({:+.2} %)",
            change * 100.0
        ));
        report.check(
            || format!("{name}: {after} within {STEP_CONTINUITY} of {before}"),
            change.abs() <= STEP_CONTINUITY,
            true,
        );
    }
    let (plain, sharpened) = (result(STATIC), result(SHARPENED));
    let ratio = match (plain, sharpened) {
        (Some(plain), Some(sharpened)) => sharpened.contrast / plain.contrast,
        _ => f32::NAN,
    };
    report.notes.push(format!(
        "sharpening: local contrast {ratio:.4}x the unsharpened output's"
    ));
    report.check(
        || format!("sharpened contrast {ratio} x the unsharpened, at least {SHARPENING}"),
        ratio >= SHARPENING,
        true,
    );
    report.check(
        || String::from("the sharpened output differs from the unsharpened one"),
        plain.map(|measured| measured.hash) != sharpened.map(|measured| measured.hash),
        true,
    );

    for (checkpoint, measured) in &first.results {
        let name = &checkpoint.name;
        report.notes.push(format!(
            "{name}: mean error {:.5}, max {:.5} (bounds {:.3}, {:.3}), whole image {:.5} \
             (bound {:.2}), finite {}",
            measured.mean,
            measured.max,
            checkpoint.bound.0,
            checkpoint.bound.1,
            measured.full_max,
            checkpoint.full,
            measured.finite
        ));
        report.check(
            || format!("{name}: every output texel finite"),
            measured.finite,
            true,
        );
        report.check(
            || {
                format!(
                    "{name}: mean error {} below {}",
                    measured.mean, checkpoint.bound.0
                )
            },
            measured.mean < checkpoint.bound.0,
            true,
        );
        report.check(
            || {
                format!(
                    "{name}: max error {} below {}",
                    measured.max, checkpoint.bound.1
                )
            },
            measured.max < checkpoint.bound.1,
            true,
        );
        report.check(
            || {
                format!(
                    "{name}: whole-image max error {} below {}",
                    measured.full_max, checkpoint.full
                )
            },
            measured.full_max < checkpoint.full,
            true,
        );
    }
    let differing = first
        .hashes
        .iter()
        .zip(&second.hashes)
        .filter(|(a, b)| a != b)
        .count();
    report.check(
        || String::from("output frames differing between the poisoned and the fresh run"),
        (differing, second.hashes.len()),
        (0, first.hashes.len()),
    );
    let digest = first
        .hashes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, frame| {
            fnv(hash, &frame.to_le_bytes())
        });
    report.notes.push(format!(
        "{} frames, two in flight, output digest {digest:016x} (identical across two runs, \
         the first poisoned: {})",
        first.hashes.len(),
        differing == 0
    ));
    report
        .notes
        .push(format!("context vram_bytes {}", first.vram_bytes));
    report.notes.extend(vram_at_real_sizes(gpu)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_trip() {
        for value in [0.0f32, 1.0, -2.0, 0.5, 0.6, 1000.0, 6.0e-5] {
            let back = f16_value(f16_bits(value));
            assert!(
                (back - value).abs() <= value.abs() / 1024.0,
                "{value} -> {back}"
            );
        }
    }

    #[test]
    fn halton_starts_as_documented() {
        assert_eq!(halton(1, 2), 0.5);
        assert_eq!(halton(2, 2), 0.25);
        assert_eq!(halton(1, 3), 1.0 / 3.0);
    }

    #[test]
    fn the_script_stays_inside_one_context() {
        let max = Fsr3Mode::Quality.render_size(OUTPUT);
        let frames = script();
        assert!(
            frames
                .iter()
                .all(|(frame, _)| frame.render.width <= max.width
                    && frame.render.height <= max.height)
        );
        let checks: Vec<&str> = frames
            .iter()
            .flat_map(|(_, checks)| checks.iter().map(|check| check.name.as_str()))
            .collect();
        // 4 still/moving/sharpened phases, 5 size steps with their first frames, 3 tiny sizes,
        // the recovery, 2 fine phases with their first frames, and the two occluder runs'
        // trails (3 each) and the 3 frames around the second's size step.
        assert_eq!(
            checks.len(),
            4 + 5 * 2 + 3 + 1 + 4 + 2 * 3 + 3,
            "{checks:?}"
        );
        for name in [step_name(-1), step_name(0), step_name(1)] {
            assert!(checks.contains(&name.as_str()), "{name}");
        }
        assert!(checks.contains(&format!("{OCCLUDER_RUN}: trail {OLD_TRAIL}").as_str()));
        // Only the static, the two fine and the two occluder phases (by flag) and the
        // sharpened one (through `reset_history`) reset; the size sweeps never do.
        assert_eq!(frames.iter().filter(|(frame, _)| frame.reset).count(), 5);
        assert_eq!(
            frames
                .iter()
                .filter(|(frame, _)| frame.reset_history)
                .count(),
            1
        );
    }

    /// The moving fine field moves one render pixel a frame (1.5 output pixels at Quality),
    /// against its motion vectors.
    /// The occluder's trail regions lie where the box was and is no longer, clear of it.
    #[test]
    fn the_trail_is_behind_the_box() {
        let last = OCCLUDER_FRAMES - 1;
        let [x0, y0, x1, y1] = trail(last, 40, 60).expect("an old trail");
        let box_left = OCCLUDER_LEFT + OCCLUDER_SPEED * last as f32;
        assert!(
            x0 as f32 >= OCCLUDER_LEFT && (x1 as f32) < box_left,
            "{x0}..{x1}"
        );
        assert!(y0 as f32 > OCCLUDER_TOP && (y1 as f32) < OCCLUDER_BOTTOM);
        let content = Content::Occluded(box_left, false);
        for x in x0..x1 {
            assert!(!content.in_front(x as f32 + 0.5, (y0 + y1) as f32 * 0.5));
            assert_eq!(
                content.depth(x as f32 + 0.5, y0 as f32 + 0.5),
                BACKGROUND_DEPTH
            );
        }
        assert!(content.in_front(box_left + 1.0, 50.0));
        assert_eq!(content.depth(box_left + 1.0, 50.0), OCCLUDER_DEPTH);
        // Every pixel of the trail was covered by the box at least 40 frames ago.
        let covered = |x: f32| {
            (0..last - 40).any(|frame| {
                let left = OCCLUDER_LEFT + OCCLUDER_SPEED * frame as f32;
                (left..left + OCCLUDER_WIDTH).contains(&x)
            })
        };
        assert!((x0..x1).all(|x| covered(x as f32 + 0.5)));
        assert_eq!(trail(5, 40, 60), None);
    }

    #[test]
    fn the_fine_field_moves_with_its_motion() {
        let frames = script();
        let moving: Vec<&Frame> = frames
            .iter()
            .map(|(frame, _)| frame)
            .filter(|frame| matches!(frame.content, Content::Fine(_)) && frame.motion[0] != 0.0)
            .collect();
        assert!(moving.len() > 2);
        let (Content::Fine(a), Content::Fine(b)) = (moving[1].content, moving[2].content) else {
            unreachable!()
        };
        assert!((b - a - 1.5).abs() < 1e-4, "{a} -> {b}");
        assert_eq!(moving[1].motion, [-1.0, 0.0]);
    }
}
