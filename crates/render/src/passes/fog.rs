//! Fog-of-war passes (R3a, ADR-0014): `fow-raster` rasterises this frame's vision sources into
//! a fixed-size `R8` transient (one instanced quad per source, line-of-sight walls tested per
//! texel, `MAX` blending), and `fow-resolve` blurs it and eases the previous frame's result
//! towards it, into one of two persistent images.
//!
//! The two images ping-pong by frame in flight: frame slot `f` writes image `f` and reads
//! image `1 - f` as history, so every descriptor set is written once. The scene (set 3) and
//! the UI (the minimap mask) of slot `f` sample image `f`. Per-frame source and wall buffers
//! are host-visible and sized at creation: preparing a frame never allocates.

use core::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use bytemuck::{Pod, Zeroable, bytes_of};
use gpu_allocator::MemoryLocation;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::pipeline::{self, VertexlessDepth};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, GraphicsPipeline, PipelineLayout,
    Sampler,
};
use crate::shaders::ShaderSource;
use crate::world::{FogOfWar, MAX_VISION_WALLS};

/// The fog-of-war images' format.
pub(crate) const FOG_FORMAT: vk::Format = vk::Format::R8_UNORM;
/// Seconds for a revealed area to reach ~63 % visibility, and for a lost one to fade (after
/// its instant drop to at most half visible: `LOST_VISIBILITY` in `fow_resolve.frag`).
const REVEAL_SECONDS: f32 = 0.08;
const FADE_SECONDS: f32 = 0.3;
/// Persistent images: one per frame in flight.
pub(crate) const FOG_IMAGES: usize = 2;
static SOURCES_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Vision sources (and walls) left out of the most recent frame's fog-of-war texture because
/// they exceeded [`RenderConfig::max_vision_sources`](crate::RenderConfig::max_vision_sources)
/// (or [`MAX_VISION_WALLS`]).
#[must_use]
pub fn vision_sources_dropped() -> u64 {
    SOURCES_DROPPED.load(Ordering::Relaxed)
}

/// Push constants of `fow_raster.vert`/`.frag` (32 bytes, both stages).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RasterPush {
    /// xy world corner, z 1 / size, w texel size (world units): the disc's inset, and 1.5×
    /// its soft edge.
    rect: [f32; 4],
    /// x walls, y sources.
    counts: [u32; 4],
}

/// Push constants of `fow_resolve.frag` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ResolvePush {
    texel: [f32; 2],
    reveal: f32,
    fade: f32,
}

struct FogFrame {
    sources: GpuBuffer,
    walls: GpuBuffer,
    raster_set: vk::DescriptorSet,
    /// Reads the raw transient and the other image (history).
    resolve_set: vk::DescriptorSet,
    raster: RasterPush,
    resolve: ResolvePush,
}

/// Pipelines, buffers and the two persistent images of the fog-of-war passes.
pub(crate) struct FogPass {
    frames: Vec<FogFrame>,
    images: [GpuImage; FOG_IMAGES],
    _pool: DescriptorPool,
    _raster_layout: DescriptorSetLayout,
    _resolve_layout: DescriptorSetLayout,
    sampler: Sampler,
    raster_pipeline_layout: PipelineLayout,
    resolve_pipeline_layout: PipelineLayout,
    raster: GraphicsPipeline,
    resolve: GraphicsPipeline,
    resolution: u32,
    max_sources: usize,
}

const RASTER_PUSH_SIZE: u32 = core::mem::size_of::<RasterPush>() as u32;
const RESOLVE_PUSH_SIZE: u32 = core::mem::size_of::<ResolvePush>() as u32;
const RASTER_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::VERTEX.as_raw() | vk::ShaderStageFlags::FRAGMENT.as_raw(),
);

impl FogPass {
    /// Builds both pipelines, `frames` sets of source/wall buffers (room for `max_sources`
    /// sources and [`MAX_VISION_WALLS`] walls each) and the two `resolution²` images, cleared
    /// to 0 (fogged) and left in `SHADER_READ_ONLY_OPTIMAL`.
    #[allow(clippy::too_many_lines)] // Layouts, pipelines, buffers and images, in order.
    pub(crate) fn new(
        device: &DeviceContext,
        shaders: &ShaderSource,
        frames: usize,
        resolution: u32,
        max_sources: usize,
    ) -> RenderResult<Self> {
        let storage = |binding, stages| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let raster_layout = DescriptorSetLayout::new(
            device.device.clone(),
            &[
                storage(0, vk::ShaderStageFlags::VERTEX),
                storage(1, vk::ShaderStageFlags::FRAGMENT),
            ],
        )?;
        let sampled = |binding| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        };
        let resolve_layout =
            DescriptorSetLayout::new(device.device.clone(), &[sampled(0), sampled(1)])?;
        let frame_count = u32::try_from(frames).expect("frame count fits u32");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(2 * frame_count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(2 * frame_count),
            ],
            2 * frame_count,
        )?;
        let raster_sets = pool.allocate(&device.device, &vec![raster_layout.handle(); frames])?;
        let resolve_sets = pool.allocate(&device.device, &vec![resolve_layout.handle(); frames])?;
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .max_lod(0.0),
        )?;
        let raster_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[raster_layout.handle()],
            &[vk::PushConstantRange::default()
                .stage_flags(RASTER_STAGES)
                .offset(0)
                .size(RASTER_PUSH_SIZE)],
        )?;
        let resolve_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[resolve_layout.handle()],
            &[vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(RESOLVE_PUSH_SIZE)],
        )?;
        let (raster, resolve) = build_pipelines(
            device,
            shaders,
            raster_pipeline_layout.handle(),
            resolve_pipeline_layout.handle(),
        )?;
        let extent = vk::Extent2D {
            width: resolution,
            height: resolution,
        };
        let image = |name| {
            let image = GpuImage::new(
                device.device.clone(),
                device.allocator(),
                name,
                extent,
                FOG_FORMAT,
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageAspectFlags::COLOR,
            )?;
            if let Err((error, in_use)) = crate::targets::initialise_sampled(device, image.image())
            {
                if in_use {
                    // The clear may still run on the GPU: leak rather than free its memory.
                    core::mem::forget(image);
                }
                return Err(error);
            }
            Ok::<_, crate::RenderError>(image)
        };
        let images = [image("fog-of-war-0")?, image("fog-of-war-1")?];
        let max_sources = source_capacity(max_sources, device.max_storage_buffer_range);
        let mut frame_data = Vec::with_capacity(frames);
        for frame in 0..frames {
            let buffer = |name, count: usize| {
                GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    name,
                    (count * core::mem::size_of::<[f32; 4]>()) as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    MemoryLocation::CpuToGpu,
                )
            };
            let sources = buffer("fog-sources", max_sources)?;
            let walls = buffer("fog-walls", MAX_VISION_WALLS)?;
            frame_data.push(FogFrame {
                sources,
                walls,
                raster_set: raster_sets[frame],
                resolve_set: resolve_sets[frame],
                raster: RasterPush::zeroed(),
                resolve: ResolvePush::zeroed(),
            });
        }
        let pass = Self {
            frames: frame_data,
            images,
            _pool: pool,
            _raster_layout: raster_layout,
            _resolve_layout: resolve_layout,
            sampler,
            raster_pipeline_layout,
            resolve_pipeline_layout,
            raster,
            resolve,
            resolution,
            max_sources,
        };
        pass.write_raster_sets(device);
        Ok(pass)
    }

    /// The persistent image frame slot `frame` writes (and its scene and UI sample).
    pub(crate) fn image(&self, frame: usize) -> &GpuImage {
        &self.images[frame % FOG_IMAGES]
    }

    /// Both images' views, by frame slot.
    pub(crate) fn views(&self) -> [vk::ImageView; FOG_IMAGES] {
        [self.images[0].view(), self.images[1].view()]
    }

    /// Points each frame's resolve set at the raw transient's view and the other frame's
    /// image (history). Only while no submitted frame uses the sets (creation, after a
    /// resize's idle wait).
    pub(crate) fn bind_raw(&self, device: &DeviceContext, raw: vk::ImageView) {
        for (frame, data) in self.frames.iter().enumerate() {
            let history = self.images[(frame + 1) % FOG_IMAGES].view();
            let infos = [raw, history].map(|view| {
                [vk::DescriptorImageInfo::default()
                    .sampler(self.sampler.handle())
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
            });
            let writes: [vk::WriteDescriptorSet<'_>; 2] = core::array::from_fn(|binding| {
                vk::WriteDescriptorSet::default()
                    .dst_set(data.resolve_set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&infos[binding])
            });
            // SAFETY: the set, sampler and views are live and no pending command buffer
            // uses the set (see the method contract).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    fn write_raster_sets(&self, device: &DeviceContext) {
        for data in &self.frames {
            let infos = [&data.sources, &data.walls].map(|buffer| {
                [vk::DescriptorBufferInfo::default()
                    .buffer(buffer.handle())
                    .offset(0)
                    .range(vk::WHOLE_SIZE)]
            });
            let writes: [vk::WriteDescriptorSet<'_>; 2] = core::array::from_fn(|binding| {
                vk::WriteDescriptorSet::default()
                    .dst_set(data.raster_set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&infos[binding])
            });
            // SAFETY: the set and buffers are live and the set is unused (creation).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    /// Writes frame slot `frame`'s sources and walls from `fog` (none: an empty texture that
    /// fades everything to fog) and its push constants. The frame's fence must have been
    /// waited on. Never allocates.
    pub(crate) fn prepare(&mut self, frame: usize, fog: Option<&FogOfWar<'_>>) {
        let resolution = self.resolution;
        let max_sources = self.max_sources;
        let data = &mut self.frames[frame];
        let Some(fog) = fog else {
            data.raster.counts = [0; 4];
            data.resolve = resolve_push(resolution, 1.0 / 60.0);
            SOURCES_DROPPED.store(0, Ordering::Relaxed);
            return;
        };
        let size = if fog.size.is_finite() && fog.size > 1.0 {
            fog.size
        } else {
            1.0
        };
        let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
        let mut sources = 0usize;
        if let Some(mapped) = data.sources.mapped_slice_mut::<[f32; 4]>(max_sources) {
            for source in fog.sources.iter().take(max_sources) {
                let radius = finite(source.radius).clamp(0.0, size);
                if radius <= 0.0 {
                    continue;
                }
                mapped[sources] = [finite(source.x), finite(source.z), radius, 0.0];
                sources += 1;
            }
        }
        let mut walls = 0usize;
        if let Some(mapped) = data.walls.mapped_slice_mut::<[f32; 4]>(MAX_VISION_WALLS) {
            for wall in fog.walls.iter().take(MAX_VISION_WALLS) {
                mapped[walls] = [
                    finite(wall.a[0]),
                    finite(wall.a[1]),
                    finite(wall.b[0]),
                    finite(wall.b[1]),
                ];
                walls += 1;
            }
        }
        let dropped = fog.sources.len().saturating_sub(max_sources)
            + fog.walls.len().saturating_sub(MAX_VISION_WALLS);
        SOURCES_DROPPED.store(
            u64::try_from(dropped).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let texel = size / resolution as f32;
        data.raster = RasterPush {
            rect: [
                finite(fog.origin[0]),
                finite(fog.origin[1]),
                1.0 / size,
                texel,
            ],
            counts: [
                u32::try_from(walls).unwrap_or(0),
                u32::try_from(sources).unwrap_or(0),
                0,
                0,
            ],
        };
        data.resolve = resolve_push(resolution, fog.dt);
    }

    /// Records `fow-raster` into `raw` (the transient, cleared to 0).
    pub(crate) fn record_raster(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        raw: vk::ImageView,
    ) {
        let data = &self.frames[frame];
        let extent = vk::Extent2D {
            width: self.resolution,
            height: self.resolution,
        };
        begin(
            device,
            command_buffer,
            raw,
            extent,
            vk::AttachmentLoadOp::CLEAR,
        );
        let sources = data.raster.counts[1];
        if sources > 0 {
            // SAFETY: the pipeline is live and was built for the R8 attachment.
            unsafe {
                device.device.cmd_bind_pipeline(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.raster.handle(),
                );
            }
            // SAFETY: the set matches set 0 of the raster layout and names this frame's
            // live buffers.
            unsafe {
                device.device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.raster_pipeline_layout.handle(),
                    0,
                    &[data.raster_set],
                    &[],
                );
            }
            // SAFETY: the layout declares exactly this 32-byte range for both stages.
            unsafe {
                device.device.cmd_push_constants(
                    command_buffer,
                    self.raster_pipeline_layout.handle(),
                    RASTER_STAGES,
                    0,
                    bytes_of(&data.raster),
                );
            }
            // SAFETY: six vertices per source quad, no vertex buffers; `sources` instances
            // are written to the buffer this frame (at most its capacity, `max_sources`),
            // and the fragment shader reads at most `counts.x <= MAX_VISION_WALLS` walls.
            unsafe { device.device.cmd_draw(command_buffer, 6, sources, 0, 0) };
        }
        device.cmd_end_rendering(command_buffer);
    }

    /// Records `fow-resolve` into frame slot `frame`'s image.
    pub(crate) fn record_resolve(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
    ) {
        let data = &self.frames[frame];
        let extent = vk::Extent2D {
            width: self.resolution,
            height: self.resolution,
        };
        begin(
            device,
            command_buffer,
            self.image(frame).view(),
            extent,
            vk::AttachmentLoadOp::DONT_CARE,
        );
        // SAFETY: the pipeline is live and was built for the R8 attachment.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.resolve.handle(),
            );
        }
        // SAFETY: the set matches set 0 of the resolve layout; the graph transitioned the
        // raw transient and the history image to SHADER_READ_ONLY_OPTIMAL.
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.resolve_pipeline_layout.handle(),
                0,
                &[data.resolve_set],
                &[],
            );
        }
        // SAFETY: the layout declares exactly this 16-byte fragment range.
        unsafe {
            device.device.cmd_push_constants(
                command_buffer,
                self.resolve_pipeline_layout.handle(),
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes_of(&data.resolve),
            );
        }
        // SAFETY: the fullscreen triangle reads no vertex buffers.
        unsafe { device.device.cmd_draw(command_buffer, 3, 1, 0, 0) };
        device.cmd_end_rendering(command_buffer);
    }
}

/// Vision sources the per-frame buffer holds: the requested count, clamped to
/// `1..=MAX_VISION_SOURCES` and to what one storage-buffer descriptor may address (16 bytes
/// per source), so the buffer size can never overflow or exceed the device's range.
fn source_capacity(requested: usize, max_storage_buffer_range: u32) -> usize {
    let by_range = (max_storage_buffer_range / 16).max(1) as usize;
    requested
        .clamp(1, crate::world::MAX_VISION_SOURCES)
        .min(by_range)
}

/// The resolve's per-frame blend factors: exponential easing with the reveal and fade time
/// constants, frame-rate independent (`dt` clamped to `0..=FOG_OF_WAR_SETTLE_DT`, where both
/// factors reach 1: the texture settles at once).
fn resolve_push(resolution: u32, dt: f32) -> ResolvePush {
    let dt = if dt.is_finite() {
        dt.clamp(0.0, crate::world::FOG_OF_WAR_SETTLE_DT)
    } else {
        0.0
    };
    ResolvePush {
        texel: [1.0 / resolution as f32; 2],
        reveal: 1.0 - (-dt / REVEAL_SECONDS).exp(),
        fade: 1.0 - (-dt / FADE_SECONDS).exp(),
    }
}

/// Begins rendering into the R8 `view` with a full, unflipped viewport (texel row 0 is the
/// world's lowest z).
fn begin(
    device: &DeviceContext,
    command_buffer: vk::CommandBuffer,
    view: vk::ImageView,
    extent: vk::Extent2D,
    load_op: vk::AttachmentLoadOp,
) {
    let attachment = vk::RenderingAttachmentInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .load_op(load_op)
        .store_op(vk::AttachmentStoreOp::STORE)
        .clear_value(vk::ClearValue {
            color: vk::ClearColorValue { float32: [0.0; 4] },
        });
    let render_info = vk::RenderingInfo::default()
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        })
        .layer_count(1)
        .color_attachments(core::slice::from_ref(&attachment));
    device.cmd_begin_rendering(command_buffer, &render_info);
    super::set_viewport_scissor(device, command_buffer, super::full_viewport(extent), extent);
}

fn build_pipelines(
    device: &DeviceContext,
    shaders: &ShaderSource,
    raster_layout: vk::PipelineLayout,
    resolve_layout: vk::PipelineLayout,
) -> RenderResult<(GraphicsPipeline, GraphicsPipeline)> {
    let raster_vertex = shaders.module(device, "fow_raster.vert")?;
    let raster_fragment = shaders.module(device, "fow_raster.frag")?;
    let fullscreen = shaders.module(device, "fullscreen.vert")?;
    let resolve_fragment = shaders.module(device, "fow_resolve.frag")?;
    let raster = pipeline::create_vertexless(
        device,
        raster_layout,
        raster_vertex.handle(),
        raster_fragment.handle(),
        FOG_FORMAT,
        true,
        VertexlessDepth::None,
    )?;
    let raster = GraphicsPipeline::from_raw(device.device.clone(), raster);
    let resolve = pipeline::create_vertexless(
        device,
        resolve_layout,
        fullscreen.handle(),
        resolve_fragment.handle(),
        FOG_FORMAT,
        false,
        VertexlessDepth::None,
    )?;
    Ok((
        raster,
        GraphicsPipeline::from_raw(device.device.clone(), resolve),
    ))
}

#[cfg(test)]
mod tests {
    use super::{RasterPush, ResolvePush, resolve_push};

    /// `LOST_VISIBILITY` in `fow_resolve.frag`.
    const LOST_VISIBILITY: f32 = 0.5;

    /// Review fix: the source buffer's capacity is bounded, so `count * 16` never overflows
    /// and never exceeds the storage-buffer range.
    #[test]
    fn source_capacity_is_bounded() {
        use super::source_capacity;
        assert_eq!(source_capacity(0, u32::MAX), 1);
        assert_eq!(source_capacity(1_024, u32::MAX), 1_024);
        assert_eq!(
            source_capacity(usize::MAX, u32::MAX),
            crate::world::MAX_VISION_SOURCES
        );
        // The Vulkan minimum range (128 MiB) holds the cap; a tiny one bounds it further.
        assert_eq!(
            source_capacity(usize::MAX, 1 << 27),
            crate::world::MAX_VISION_SOURCES
        );
        assert_eq!(source_capacity(usize::MAX, 1_600), 100);
        assert_eq!(source_capacity(usize::MAX, 0), 1);
    }

    /// CPU reference of `fow_resolve.frag`'s ease for one texel (R8 values as `0..=1`).
    fn resolve_reference(previous: f32, blurred: f32, push: &ResolvePush) -> f32 {
        let rate = if blurred > previous {
            push.reveal
        } else {
            push.fade
        };
        let gap = blurred - previous;
        let step = 1.01 / 255.0;
        let delta = if gap.abs() > step {
            gap.signum() * (gap.abs() * rate).max(step)
        } else {
            gap
        };
        let mut eased = previous + delta;
        if gap < 0.0 {
            eased = eased.min(blurred.max(LOST_VISIBILITY));
        }
        eased
    }

    /// CPU reference of the resolve's blur: four bilinear taps at half-texel offsets with
    /// clamp-to-edge, i.e. a 3x3 tent (1 2 1 / 2 4 2 / 1 2 1, over 16).
    fn blur_reference(raw: &[u8], resolution: usize) -> Vec<f32> {
        let last = resolution - 1;
        let at = |i: usize, j: usize| f32::from(raw[j * resolution + i]) / 255.0;
        let mut out = vec![0.0; resolution * resolution];
        for j in 0..resolution {
            let rows = [
                (j.saturating_sub(1), 1.0),
                (j, 2.0),
                ((j + 1).min(last), 1.0),
            ];
            for i in 0..resolution {
                let columns = [
                    (i.saturating_sub(1), 1.0),
                    (i, 2.0),
                    ((i + 1).min(last), 1.0),
                ];
                let mut sum = 0.0;
                for (row, weight_row) in rows {
                    for (column, weight_column) in columns {
                        sum += weight_row * weight_column * at(column, row);
                    }
                }
                out[j * resolution + i] = sum / 16.0;
            }
        }
        out
    }

    /// Review fix: the disc is inset by one texel, so after the blur the visible area ends
    /// within half a texel of the vision radius (it reached 1.5 texels past it before).
    #[test]
    fn the_blurred_edge_stays_within_the_vision_radius() {
        use crate::world::{FogOfWar, VisionSource};
        let resolution = 64usize;
        let (size, radius) = (128.0f32, 20.0f32);
        let texel = size / resolution as f32;
        let sources = [VisionSource::new(64.0, 64.0, radius)];
        let fog = FogOfWar::new([0.0, 0.0], size).with_sources(&sources);
        let raw = crate::world::rasterize_reference(&fog, resolution as u32);
        let blurred = blur_reference(&raw, resolution);
        for j in 0..resolution {
            for i in 0..resolution {
                let x = (i as f32 + 0.5) * texel - 64.0;
                let z = (j as f32 + 0.5) * texel - 64.0;
                let distance = (x * x + z * z).sqrt();
                let value = blurred[j * resolution + i];
                if distance >= radius + 0.5 * texel {
                    assert_eq!(
                        value, 0.0,
                        "visible {value} at {distance} m (radius {radius})"
                    );
                }
                if distance <= radius - 4.0 * texel {
                    assert!(value > 0.99, "{value} at {distance} m");
                }
            }
        }
    }

    /// Review fix: losing vision drops the visibility at once to at most half, then it fades
    /// out within about a second at any frame rate; revealing stays fast.
    #[test]
    fn lost_vision_drops_at_once_then_fades() {
        for hz in [30.0f32, 60.0, 144.0, 240.0] {
            let push = resolve_push(256, 1.0 / hz);
            let mut value = resolve_reference(1.0, 0.0, &push);
            assert!(value <= LOST_VISIBILITY, "{hz} Hz: {value}");
            for _ in 1..hz as usize {
                value = resolve_reference(value, 0.0, &push);
            }
            assert!(value < 0.05, "{hz} Hz: {value} after a second");
            // Partly lost vision tracks the current vision when it is above half.
            assert_eq!(resolve_reference(1.0, 0.8, &push), 0.8);
            // Revealing: most of the way within 0.2 s.
            let mut value = 0.0;
            for _ in 0..(0.2 * hz) as usize {
                value = resolve_reference(value, 1.0, &push);
            }
            assert!(value > 0.85, "{hz} Hz reveal: {value}");
        }
    }

    #[test]
    fn push_blocks_match_the_shaders_and_fades_are_frame_rate_independent() {
        let resolve_source = include_str!("../../../shaders/shaders/fow_resolve.frag");
        assert!(resolve_source.contains(&format!("LOST_VISIBILITY = {LOST_VISIBILITY:?};")));
        assert_eq!(core::mem::size_of::<RasterPush>(), 32);
        assert_eq!(core::mem::size_of::<ResolvePush>(), 16);
        // Two 60 Hz frames fade as far as one 30 Hz frame.
        let fast = resolve_push(256, 1.0 / 60.0);
        let slow = resolve_push(256, 1.0 / 30.0);
        let two_fast = 1.0 - (1.0 - fast.fade) * (1.0 - fast.fade);
        assert!((two_fast - slow.fade).abs() < 1e-5);
        assert!(
            fast.reveal > fast.fade,
            "areas reveal faster than they fade"
        );
        assert_eq!(resolve_push(256, f32::NAN).fade, 0.0);
        let settled = resolve_push(256, crate::world::FOG_OF_WAR_SETTLE_DT);
        assert_eq!(
            (settled.reveal, settled.fade),
            (1.0, 1.0),
            "captures settle at once"
        );
        assert_eq!(resolve_push(256, 1e9).fade, 1.0);
    }
}
