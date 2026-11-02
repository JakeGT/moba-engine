//! `fsr3_smoke` (ADR-0021 §4b and §4c, N1b): the FSR 3.1.6 port on synthetic inputs.
//!
//! The scene is a smooth analytic colour field over the output plane, so every frame's
//! render-resolution input and the ideal output are known exactly. Each render pixel `i`
//! samples the field at `i + 0.5 − jitter` (Halton (2, 3), ADR-0021 §1), scaled to output
//! pixels; depth is a constant plane, the masks are zero and the exposure is 1. Outside the
//! frame's render rectangle every input holds a sentinel (colour 1000, motion ±40 px, depth 0,
//! masks 1), so a pass that read outside the rectangle would show.
//!
//! The run, on one context created for Quality at 192×108 (render 128×72):
//!
//! 1. **flat**: a constant colour converges to itself (every output texel within 1 %);
//! 2. **static**: after a reset, the field converges to the ideal output;
//! 3. **moving**: the field moves one render pixel a frame with matching motion vectors and
//!    the output tracks it;
//! 4. **sharpened**: as 2 with sharpness 0.25 (accumulate-and-sharpen, then RCAS);
//! 5. **sizes** (ADR-0021 §4c): without a reset, the render size steps through Balanced,
//!    Performance, Ultra Performance, an odd 77×43 and back to Quality, all inside the one
//!    context; each size must converge and stay finite.
//!
//! The whole run happens twice on fresh contexts, and every output frame of the second must
//! be bit-identical to the first. Any validation or synchronization-validation message fails
//! it (as every smoke).

use ash::vk;
use gpu_allocator::MemoryLocation;

use super::bindless::host_read_barrier;
use super::{Headless, SmokeReport};
use crate::bindless::submit_one_shot;
use crate::error::{RenderError, RenderResult};
use crate::fsr3::{Fsr3, Fsr3Image, Fsr3Inputs, Fsr3Mode, Fsr3Output, jitter_phases};
use crate::passes::vfx::f16_bits;
use crate::resources::{GpuBuffer, GpuImage};

/// The output size.
const OUTPUT: vk::Extent2D = vk::Extent2D {
    width: 192,
    height: 108,
};
/// Output texels this close to an edge are left out of the error metrics: FSR clamps its
/// samples there and the moving field enters from off screen.
const MARGIN: u32 = 6;
/// The flat colour.
const FLAT: [f32; 3] = [0.6, 0.3, 0.15];
/// Depth everywhere inside the render rectangle (0 near, 1 far).
const DEPTH: f32 = 0.5;

/// Error bounds against the ideal output, as (mean, max) absolute error per channel.
/// About twice what lavapipe measures (`docs/perf` has the numbers): loose enough for any
/// conformant device's rounding, tight enough that a wrong resource, parity, constant or
/// jitter sign fails.
const FLAT_BOUND: (f32, f32) = (0.002, 0.004);
const STATIC_BOUND: (f32, f32) = (0.012, 0.06);
const MOVING_BOUND: (f32, f32) = (0.012, 0.07);
const SHARPENED_BOUND: (f32, f32) = (0.015, 0.07);
const SIZES_BOUND: (f32, f32) = (0.025, 0.1);
/// The checkpoints of the fine phase: its first frame after a reset (one jittered sample,
/// upscaled), and converged.
const FINE_FIRST: &str = "fine: first frame after the reset";
const FINE: &str = "fine: converged";
/// Converged, the fine field's error must be at most this fraction of its first frame's
/// (lavapipe: 0.78): the accumulation recovers detail one frame lacks.
const CONVERGENCE: f32 = 0.9;

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

/// What the frame shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Content {
    Flat,
    /// The field, shifted right by this many output pixels.
    Field(f32),
    /// The fine field, still.
    Fine,
}

impl Content {
    fn at(self, u: f32, v: f32) -> [f32; 3] {
        match self {
            Self::Flat => FLAT,
            Self::Field(shift) => field(u - shift, v),
            Self::Fine => fine(u, v),
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
                (content.at(u, v), DEPTH, motion, 0u8)
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

/// Mean and largest absolute error of `rgba` (the output's halves) against `content`, over
/// the interior, and whether every texel is finite.
fn error(rgba: &[u8], content: Content) -> (f32, f32, bool) {
    let (mut sum, mut max, mut count, mut finite) = (0.0f64, 0.0f32, 0u32, true);
    for y in 0..OUTPUT.height {
        for x in 0..OUTPUT.width {
            let at = ((y * OUTPUT.width + x) * 8) as usize;
            let value: Vec<f32> = (0..3)
                .map(|c| f16_value(u16::from_le_bytes([rgba[at + c * 2], rgba[at + c * 2 + 1]])))
                .collect();
            finite &= value.iter().all(|v| v.is_finite());
            let interior = x >= MARGIN
                && y >= MARGIN
                && x < OUTPUT.width - MARGIN
                && y < OUTPUT.height - MARGIN;
            if !interior {
                continue;
            }
            let ideal = content.at(x as f32 + 0.5, y as f32 + 0.5);
            for c in 0..3 {
                let e = (value[c] - ideal[c]).abs();
                sum += f64::from(e);
                max = max.max(e);
                count += 1;
            }
        }
    }
    ((sum / f64::from(count.max(1))) as f32, max, finite)
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

/// The check made after a phase's last frame.
#[derive(Clone, Copy, Debug)]
struct Checkpoint {
    name: &'static str,
    content: Content,
    bound: (f32, f32),
}

/// One phase of the run: `frames` frames of `content` at `render`, then a checkpoint.
#[derive(Clone, Copy, Debug)]
struct Phase {
    name: &'static str,
    frames: u32,
    render: vk::Extent2D,
    content: Content,
    /// Horizontal motion in render pixels (current to previous): the field moves the other way.
    motion: f32,
    /// Reset on the first frame through the flag.
    reset: bool,
    /// Reset on the first frame through `Fsr3::reset_history`.
    reset_history: bool,
    sharpness: f32,
    bound: (f32, f32),
}

/// The phases, in order (see the module documentation).
fn phases() -> Vec<Phase> {
    let quality = Fsr3Mode::Quality.render_size(OUTPUT);
    let converge = 2 * jitter_phases(quality, OUTPUT);
    let field = Phase {
        name: "static",
        frames: converge,
        render: quality,
        content: Content::Field(0.0),
        motion: 0.0,
        reset: true,
        reset_history: false,
        sharpness: 0.0,
        bound: STATIC_BOUND,
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
            name: "sharpened",
            reset: false,
            reset_history: true,
            sharpness: 0.25,
            bound: SHARPENED_BOUND,
            ..field
        },
    ];
    let odd = vk::Extent2D {
        width: 77,
        height: 43,
    };
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
        ("sizes: odd 77x43", odd),
        ("sizes: back to quality", quality),
    ] {
        phases.push(Phase {
            name,
            frames: jitter_phases(render, OUTPUT).min(40),
            render,
            reset: false,
            bound: SIZES_BOUND,
            ..field
        });
    }
    // No bounds of their own: only the ratio of the two errors is checked.
    let fine = Phase {
        name: FINE_FIRST,
        frames: 1,
        content: Content::Fine,
        bound: (1.0, 1.0),
        ..field
    };
    phases.push(fine);
    phases.push(Phase {
        name: FINE,
        frames: converge - 1,
        reset: false,
        ..fine
    });
    phases
}

/// The run as frames, with a checkpoint after each phase's last.
fn script() -> Vec<(Frame, Option<Checkpoint>)> {
    let mut frames = Vec::new();
    for phase in phases() {
        for index in 0..phase.frames {
            let content = match phase.content {
                // One render pixel is `OUTPUT / render` output pixels.
                Content::Field(_) => Content::Field(
                    -phase.motion * OUTPUT.width as f32 / phase.render.width as f32 * index as f32,
                ),
                other => other,
            };
            let frame = Frame {
                render: phase.render,
                content,
                motion: [phase.motion, 0.0],
                reset: phase.reset && index == 0,
                reset_history: phase.reset_history && index == 0,
                sharpness: phase.sharpness,
            };
            let checkpoint = Checkpoint {
                name: phase.name,
                content,
                bound: phase.bound,
            };
            frames.push((frame, (index + 1 == phase.frames).then_some(checkpoint)));
        }
    }
    frames
}

/// The caller-owned images: inputs at the maximum render size, the output, and the
/// staging and readback buffers.
struct Targets {
    color: GpuImage,
    depth: GpuImage,
    motion: GpuImage,
    reactive: GpuImage,
    transparency: GpuImage,
    exposure: GpuImage,
    output: GpuImage,
    staging: GpuBuffer,
    readback: GpuBuffer,
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
        let color = vk::ImageAspectFlags::COLOR;
        let texels = u64::from(max.width * max.height);
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
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                vk::Format::R32_SFLOAT,
                color,
            )?,
            output: GpuImage::new(
                device.device.clone(),
                device.allocator(),
                "fsr3-smoke-output",
                OUTPUT,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
                color,
            )?,
            // Colour 8, depth 4, motion 4, two masks 1 each, and the exposure.
            staging: GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "fsr3-smoke-staging",
                texels * 18 + 4,
                vk::BufferUsageFlags::TRANSFER_SRC,
                MemoryLocation::CpuToGpu,
            )?,
            readback: GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "fsr3-smoke-readback",
                u64::from(OUTPUT.width * OUTPUT.height) * 8,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
            )?,
        })
    }

    fn image(image: &GpuImage, format: vk::Format, extent: vk::Extent2D) -> Fsr3Image {
        Fsr3Image {
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
    /// `(checkpoint, mean, max, finite)`.
    results: Vec<(Checkpoint, f32, f32, bool)>,
    vram_bytes: u64,
    /// The context's answers to the ADR-0021 §4a queries.
    facts: Vec<String>,
    /// Whether it accepts one render pixel over its maximum, and an odd 77×43.
    accepts: (bool, bool),
}

#[allow(clippy::too_many_lines)] // Uploads, barriers, the upscale and the readback, in order.
fn run_once(gpu: &Headless) -> RenderResult<Run> {
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
        fsr.kind(),
        fsr.has_sharpening(),
        max.width,
        max.height,
        OUTPUT.width,
        OUTPUT.height
    ));
    for &mode in Fsr3Mode::ALL {
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
    let too_big = vk::Extent2D {
        width: max.width + 1,
        height: max.height,
    };
    run.accepts = (
        fsr.accepts(too_big, OUTPUT),
        fsr.accepts(
            vk::Extent2D {
                width: 77,
                height: 43,
            },
            OUTPUT,
        ),
    );
    let mut jitter_index = 0u32;
    let mut output_bytes = vec![0u8; (OUTPUT.width * OUTPUT.height * 8) as usize];
    for (number, (frame, checkpoint)) in script().into_iter().enumerate() {
        if frame.reset_history {
            fsr.reset_history();
        }
        let phases = fsr.jitter_phases(frame.render, OUTPUT);
        jitter_index += 1;
        let index = (jitter_index - 1) % phases + 1;
        let jitter = [halton(index, 2) - 0.5, halton(index, 3) - 0.5];
        let bytes = upload(max, frame.render, frame.content, jitter, frame.motion);
        let mut offset = 0;
        let mut offsets = [0u64; 6];
        for (slot, data) in [
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
            targets.staging.write_bytes(offset, data);
            offsets[slot] = offset as u64;
            offset += data.len();
        }
        // The reactive mask is `masks` (1 outside the rectangle); the transparency and
        // composition mask is all zero.
        let inputs = Fsr3Inputs {
            color: Targets::image(&targets.color, vk::Format::R16G16B16A16_SFLOAT, max),
            depth: Targets::image(&targets.depth, vk::Format::D32_SFLOAT, max),
            motion: Targets::image(&targets.motion, vk::Format::R16G16_SFLOAT, max),
            reactive: Targets::image(&targets.reactive, vk::Format::R8_UNORM, max),
            transparency: Targets::image(&targets.transparency, vk::Format::R8_UNORM, max),
            exposure: Targets::image(
                &targets.exposure,
                vk::Format::R32_SFLOAT,
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
            ),
            jitter,
            render_size: frame.render,
            dt_ms: 16.0,
            near: 0.1,
            far: 100.0,
            fov_y: core::f32::consts::FRAC_PI_3,
            reset: frame.reset,
            sharpness: frame.sharpness,
        };
        let output = Fsr3Output {
            image: Targets::image(&targets.output, vk::Format::R16G16B16A16_SFLOAT, OUTPUT),
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
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                offsets[5],
            ),
        ];
        let staging = targets.staging.handle();
        let readback = targets.readback.handle();
        let output_image = targets.output.image();
        submit_one_shot(device, |cmd| {
            // Inputs to transfer, the output from its last copy to compute.
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
                    if first {
                        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
                    } else {
                        (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::NONE)
                    },
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                    if first {
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
                // TRANSFER_DST_OPTIMAL; the staging buffer holds the whole image at `offset`.
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

            fsr.record(cmd, &inputs, &output);

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
            // SAFETY: the output was created with TRANSFER_SRC and is in GENERAL; the
            // readback buffer holds the whole image.
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
        if !targets.readback.read_bytes(0, &mut output_bytes) {
            return Err(RenderError::Shader(
                "fsr3 smoke readback is not host visible",
            ));
        }
        run.hashes.push(fnv(0xcbf2_9ce4_8422_2325, &output_bytes));
        if let Some(checkpoint) = checkpoint {
            let (mean, max_error, finite) = error(&output_bytes, checkpoint.content);
            run.results.push((checkpoint, mean, max_error, finite));
        }
    }
    Ok(run)
}

pub(super) fn run(gpu: &Headless, report: &mut SmokeReport) -> RenderResult<()> {
    let first = run_once(gpu)?;
    let second = run_once(gpu)?;
    report.notes.extend(first.facts.iter().cloned());
    report.check(
        || String::from("refuses a render size above the maximum, accepts 77x43"),
        first.accepts,
        (false, true),
    );
    let mean = |name: &str| {
        first
            .results
            .iter()
            .find(|(checkpoint, ..)| checkpoint.name == name)
            .map_or(f32::NAN, |(_, mean, ..)| *mean)
    };
    let (single, converged) = (mean(FINE_FIRST), mean(FINE));
    report.notes.push(format!(
        "accumulation: fine-detail error {converged:.5} converged vs {single:.5} on the first \
         frame ({:.2}x)",
        converged / single
    ));
    report.check(
        || format!("fine error {converged} at most {CONVERGENCE} x the first frame's {single}"),
        converged <= CONVERGENCE * single,
        true,
    );

    for (checkpoint, mean, max, finite) in &first.results {
        report.notes.push(format!(
            "{}: mean error {mean:.5}, max {max:.5} (bounds {:.3}, {:.3}), finite {finite}",
            checkpoint.name, checkpoint.bound.0, checkpoint.bound.1
        ));
        report.check(
            || format!("{}: every output texel finite", checkpoint.name),
            *finite,
            true,
        );
        report.check(
            || {
                format!(
                    "{}: mean error {mean} below {}",
                    checkpoint.name, checkpoint.bound.0
                )
            },
            *mean < checkpoint.bound.0,
            true,
        );
        report.check(
            || {
                format!(
                    "{}: max error {max} below {}",
                    checkpoint.name, checkpoint.bound.1
                )
            },
            *max < checkpoint.bound.1,
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
        || String::from("output frames differing between the two runs"),
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
        "{} frames, output digest {digest:016x} (identical across two runs: {})",
        first.hashes.len(),
        differing == 0
    ));
    report
        .notes
        .push(format!("context vram_bytes {}", first.vram_bytes));
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
        assert_eq!(
            frames
                .iter()
                .filter(|(_, checkpoint)| checkpoint.is_some())
                .count(),
            11
        );
        // Only the static and fine phases (by flag) and the sharpened one (through
        // `reset_history`) reset; the size sweep never does.
        assert_eq!(frames.iter().filter(|(frame, _)| frame.reset).count(), 2);
        assert_eq!(
            frames
                .iter()
                .filter(|(frame, _)| frame.reset_history)
                .count(),
            1
        );
    }
}
