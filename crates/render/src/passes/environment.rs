//! GPU side of the R3b foliage and water (ADR-0017): their set-2 layouts, per-frame
//! environment blocks and descriptor sets, the uploaded instances and mesh, and the draws.
//!
//! Both pipeline layouts are `[camera, instances, set 2, lighting]` with the scene's push
//! range, so the scene's sets 0 and 1 stay bound across them; like the VAT draws they bind
//! sets 2 and 3 with their own layout, after every scene-layout draw. No layout adds a
//! sampler to the scene's: the foliage's set 2 has none and the water's two (the refraction
//! copy and the scene depth) are in a layout of their own (ADR-0017 §6).

use ash::vk;
use bytemuck::bytes_of;
use moba_core::math::Mat4;

use crate::device::DeviceContext;
use crate::environment::{
    EnvironmentBlock, EnvironmentConfig, FOLIAGE_BLADE_VERTICES, FoliageChunk, FoliageData,
    FoliageLook, MAX_FOLIAGE_DISPLACERS, MAX_WATER_BODIES, MAX_WATER_VERTICES, WaterData,
    WaterMode, chunk_foliage, frustum_planes, ground_focus, visible_runs,
};
use crate::error::{RenderError, RenderResult};
use crate::mesh::{self, GpuMesh};
use crate::resources::{
    DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, PipelineLayout, Sampler,
};
use crate::scene::Scene;

/// Screen-space refraction offset (in UV units at full strength).
const REFRACTION_STRENGTH: f32 = 0.025;

/// Set 2 of the foliage layout: binding 0 the environment block (vertex + fragment),
/// binding 1 the instances (vertex storage).
pub(crate) fn foliage_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 2] {
    [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX),
    ]
}

/// Set 2 of the water layout: binding 0 the environment block, 1 the refraction copy and 2
/// the scene depth (fragment; read only by the full water's permutation).
pub(crate) fn water_set_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 3] {
    let sampled = |binding| {
        vk::DescriptorSetLayoutBinding::default()
            .binding(binding)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
    };
    [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
        sampled(1),
        sampled(2),
    ]
}

/// One frame in flight's environment block and sets.
struct EnvironmentFrameSets {
    block: GpuBuffer,
    foliage: vk::DescriptorSet,
    /// Water with the default images (simple water, offscreen views).
    water_simple: vk::DescriptorSet,
    /// Water with the refraction copy and the scene depth (full water).
    water_full: vk::DescriptorSet,
}

/// The map's uploaded foliage.
struct FoliageGpu {
    instances: GpuBuffer,
    chunks: Vec<FoliageChunk>,
}

/// Foliage and water resources of the scene pass.
pub(crate) struct EnvironmentResources {
    config: EnvironmentConfig,
    /// Whether the full water passes run (Full, and not on a portability-subset device).
    full_water: bool,
    frames: Vec<EnvironmentFrameSets>,
    _pool: DescriptorPool,
    _foliage_layout: DescriptorSetLayout,
    _water_layout: DescriptorSetLayout,
    foliage_pipeline_layout: PipelineLayout,
    water_pipeline_layout: PipelineLayout,
    /// Bound at set 2 binding 1 without foliage (16 bytes).
    default_instances: GpuBuffer,
    /// 1×1 image the simple sets (and the full sets before the first resize) name.
    default_image: GpuImage,
    linear_clamp: Sampler,
    point_clamp: Sampler,
    foliage: Option<FoliageGpu>,
    look: FoliageLook,
    water: Option<GpuMesh>,
    water_styles: ([[f32; 4]; MAX_WATER_BODIES], [[f32; 4]; MAX_WATER_BODIES]),
    /// This frame's visible foliage runs `(first instance, count)` (capacity: every chunk).
    draws: Vec<(u32, u32)>,
    block: EnvironmentBlock,
}

impl EnvironmentResources {
    /// Layouts, sets and defaults. `camera_layout`/`instance_layout`/`lighting_layout` and
    /// `push` are the scene layout's sets 0, 1, 3 and push range.
    #[allow(clippy::too_many_lines)] // Layouts, samplers, defaults and sets, in order.
    pub(crate) fn new(
        device: &DeviceContext,
        frames: usize,
        camera_layout: vk::DescriptorSetLayout,
        instance_layout: vk::DescriptorSetLayout,
        lighting_layout: vk::DescriptorSetLayout,
        push: &[vk::PushConstantRange],
        config: EnvironmentConfig,
    ) -> RenderResult<Self> {
        let config = config.sanitized();
        let full_water = config.water == WaterMode::Full && !device.portability_subset;
        let foliage_layout =
            DescriptorSetLayout::new(device.device.clone(), &foliage_set_bindings())?;
        let water_layout = DescriptorSetLayout::new(device.device.clone(), &water_set_bindings())?;
        let foliage_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[
                camera_layout,
                instance_layout,
                foliage_layout.handle(),
                lighting_layout,
            ],
            push,
        )?;
        let water_pipeline_layout = PipelineLayout::with_push_constants(
            device.device.clone(),
            &[
                camera_layout,
                instance_layout,
                water_layout.handle(),
                lighting_layout,
            ],
            push,
        )?;
        let frame_count = u32::try_from(frames).expect("frames in flight fit u32");
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(3 * frame_count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(frame_count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(4 * frame_count),
            ],
            3 * frame_count,
        )?;
        let layouts: Vec<vk::DescriptorSetLayout> = (0..frames)
            .flat_map(|_| {
                [
                    foliage_layout.handle(),
                    water_layout.handle(),
                    water_layout.handle(),
                ]
            })
            .collect();
        let sets = pool.allocate(&device.device, &layouts)?;
        let clamp = vk::SamplerAddressMode::CLAMP_TO_EDGE;
        let sampler = |filter| {
            Sampler::new(
                device.device.clone(),
                &vk::SamplerCreateInfo::default()
                    .mag_filter(filter)
                    .min_filter(filter)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(clamp)
                    .address_mode_v(clamp)
                    .address_mode_w(clamp)
                    .max_lod(0.0),
            )
        };
        let linear_clamp = sampler(vk::Filter::LINEAR)?;
        let point_clamp = sampler(vk::Filter::NEAREST)?;
        let default_image =
            crate::texture::upload_rgba(device, "environment-default", 1, 1, &[0, 0, 0, 255])?;
        let mut default_instances = GpuBuffer::new(
            device.device.clone(),
            device.allocator(),
            "foliage-default",
            16,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            gpu_allocator::MemoryLocation::CpuToGpu,
        )?;
        default_instances.write_bytes(0, &[0; 16]);
        let mut frame_sets = Vec::with_capacity(frames);
        for frame in 0..frames {
            let mut block = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "environment-block",
                core::mem::size_of::<EnvironmentBlock>() as u64,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                gpu_allocator::MemoryLocation::CpuToGpu,
            )?;
            block.write_bytes(0, bytes_of(&EnvironmentBlock::zeroed()));
            frame_sets.push(EnvironmentFrameSets {
                block,
                foliage: sets[3 * frame],
                water_simple: sets[3 * frame + 1],
                water_full: sets[3 * frame + 2],
            });
        }
        let resources = Self {
            config,
            full_water,
            frames: frame_sets,
            _pool: pool,
            _foliage_layout: foliage_layout,
            _water_layout: water_layout,
            foliage_pipeline_layout,
            water_pipeline_layout,
            default_instances,
            default_image,
            linear_clamp,
            point_clamp,
            foliage: None,
            look: FoliageLook::default(),
            water: None,
            water_styles: ([[0.0; 4]; MAX_WATER_BODIES], [[0.0; 4]; MAX_WATER_BODIES]),
            draws: Vec::new(),
            block: EnvironmentBlock::zeroed(),
        };
        resources.write_foliage_sets(device);
        let view = resources.default_image.view();
        resources.write_water_sets(device, false, view, view);
        resources.write_water_sets(device, true, view, view);
        Ok(resources)
    }

    pub(crate) fn config(&self) -> EnvironmentConfig {
        self.config
    }

    /// Whether the frame has the full water passes.
    pub(crate) fn full_water(&self) -> bool {
        self.full_water
    }

    /// The layouts and mode the scene's R3b pipelines are built for.
    pub(crate) fn layouts(&self) -> crate::passes::scene::EnvironmentLayouts {
        crate::passes::scene::EnvironmentLayouts {
            foliage: self.foliage_pipeline_layout(),
            water: self.water_pipeline_layout(),
            full_water: self.full_water(),
            decals: self.config.decals,
        }
    }

    pub(crate) fn foliage_pipeline_layout(&self) -> vk::PipelineLayout {
        self.foliage_pipeline_layout.handle()
    }

    pub(crate) fn water_pipeline_layout(&self) -> vk::PipelineLayout {
        self.water_pipeline_layout.handle()
    }

    pub(crate) fn has_water(&self) -> bool {
        self.water.is_some() && self.config.water != WaterMode::Off
    }

    fn write_foliage_sets(&self, device: &DeviceContext) {
        let instances = self
            .foliage
            .as_ref()
            .map_or(&self.default_instances, |foliage| &foliage.instances);
        for frame in &self.frames {
            let block = [vk::DescriptorBufferInfo::default()
                .buffer(frame.block.handle())
                .range(vk::WHOLE_SIZE)];
            let storage = [vk::DescriptorBufferInfo::default()
                .buffer(instances.handle())
                .range(vk::WHOLE_SIZE)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(frame.foliage)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&block),
                vk::WriteDescriptorSet::default()
                    .dst_set(frame.foliage)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&storage),
            ];
            // SAFETY: the sets, buffers are live, and the callers guarantee no pending
            // command buffer uses the sets (creation, or after a device-idle wait).
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    fn write_water_sets(
        &self,
        device: &DeviceContext,
        full: bool,
        refraction: vk::ImageView,
        depth: vk::ImageView,
    ) {
        let depth_layout = if full && depth != self.default_image.view() {
            vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        } else {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        };
        for frame in &self.frames {
            let set = if full {
                frame.water_full
            } else {
                frame.water_simple
            };
            let block = [vk::DescriptorBufferInfo::default()
                .buffer(frame.block.handle())
                .range(vk::WHOLE_SIZE)];
            let refraction_info = [vk::DescriptorImageInfo::default()
                .sampler(self.linear_clamp.handle())
                .image_view(refraction)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let depth_info = [vk::DescriptorImageInfo::default()
                .sampler(self.point_clamp.handle())
                .image_view(depth)
                .image_layout(depth_layout)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&block),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&refraction_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&depth_info),
            ];
            // SAFETY: as in `write_foliage_sets`.
            unsafe { device.device.update_descriptor_sets(&writes, &[]) };
        }
    }

    /// Points the full water sets at the refraction copy and the scene depth (resize). The
    /// device must be idle or the sets unused by pending work.
    pub(crate) fn bind_targets(
        &self,
        device: &DeviceContext,
        refraction: vk::ImageView,
        depth: vk::ImageView,
    ) {
        self.write_water_sets(device, true, refraction, depth);
    }

    /// Uploads the map's foliage (replacing any). The device must be idle.
    ///
    /// # Errors
    /// Allocation and Vulkan errors from the upload; then the previous foliage is gone.
    pub(crate) fn set_foliage(
        &mut self,
        device: &DeviceContext,
        data: &FoliageData<'_>,
    ) -> RenderResult<()> {
        self.foliage = None;
        self.draws = Vec::new();
        let (packed, chunks) = chunk_foliage(data.instances, self.config.max_foliage as usize);
        if !packed.is_empty() {
            let instances =
                mesh::upload_storage(device, "foliage-instances", bytemuck::cast_slice(&packed))?;
            self.draws = Vec::with_capacity(chunks.len());
            self.foliage = Some(FoliageGpu { instances, chunks });
        }
        self.look = FoliageLook::from_data(data);
        self.write_foliage_sets(device);
        Ok(())
    }

    /// Removes the foliage. The device must be idle.
    pub(crate) fn clear_foliage(&mut self, device: &DeviceContext) {
        self.foliage = None;
        self.draws = Vec::new();
        self.write_foliage_sets(device);
    }

    /// Instances uploaded.
    pub(crate) fn foliage_instances(&self) -> usize {
        self.foliage.as_ref().map_or(0, |foliage| {
            foliage.chunks.iter().map(|c| c.count as usize).sum()
        })
    }

    /// Uploads the map's water (replacing any). The device must be idle.
    ///
    /// # Errors
    /// [`RenderError::InvalidWater`] for an inconsistent mesh; allocation and Vulkan errors.
    pub(crate) fn set_water(
        &mut self,
        device: &DeviceContext,
        data: &WaterData<'_>,
    ) -> RenderResult<()> {
        validate_water(data)?;
        let mesh = mesh::upload_raw(device, bytemuck::cast_slice(data.vertices), data.indices)?;
        let (mut deep, mut shallow) = ([[0.0; 4]; MAX_WATER_BODIES], [[0.0; 4]; MAX_WATER_BODIES]);
        let clean = |value: f32, low: f32, high: f32| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                low
            }
        };
        for index in 0..MAX_WATER_BODIES {
            let style = data.styles.get(index).copied().unwrap_or_default();
            let [r, g, b] = style.deep.map(|v| clean(v, 0.0, 1.0));
            deep[index] = [r, g, b, clean(style.clarity, 0.02, 8.0)];
            let [r, g, b] = style.shallow.map(|v| clean(v, 0.0, 1.0));
            shallow[index] = [r, g, b, clean(style.foam, 0.0, 1.0)];
        }
        self.water = Some(mesh);
        self.water_styles = (deep, shallow);
        Ok(())
    }

    /// Removes the water. The device must be idle.
    pub(crate) fn clear_water(&mut self) {
        self.water = None;
    }

    /// Fills frame `frame`'s environment block from `scene` and culls the foliage against
    /// its camera. `extent` is the main target's size.
    pub(crate) fn prepare(&mut self, frame: usize, scene: &Scene<'_>, extent: vk::Extent2D) {
        let config = self.config;
        let environment = scene.environment;
        let look = self.look;
        let block = &mut self.block;
        let inverse = scene.camera.view_proj.inverse();
        block.inverse_view_proj = if inverse.is_finite() {
            inverse.to_cols_array_2d()
        } else {
            Mat4::IDENTITY.to_cols_array_2d()
        };
        let time = if environment.time.is_finite() {
            // Wrapped so f32 keeps sub-millisecond resolution in long sessions.
            environment.time.rem_euclid(3_600.0)
        } else {
            0.0
        };
        block.time = [
            time,
            look.wind_speed,
            config.foliage_blades as f32,
            config.foliage_distance,
        ];
        let mut count = 0usize;
        for displacer in environment
            .displacers
            .iter()
            .take((config.max_displacers as usize).min(MAX_FOLIAGE_DISPLACERS))
        {
            let values = [
                displacer.position[0],
                displacer.position[1],
                displacer.radius,
                displacer.strength,
            ];
            if values.iter().all(|value| value.is_finite()) && displacer.radius > 0.0 {
                block.displacers[count] = [
                    values[0],
                    values[1],
                    values[2].min(8.0),
                    values[3].clamp(0.0, 1.0),
                ];
                count += 1;
            }
        }
        block.wind = [
            look.wind[0],
            look.wind[1],
            count as f32,
            config.water_octaves as f32,
        ];
        let rgb = |color: [f32; 3]| [color[0], color[1], color[2], 1.0];
        block.base_color = rgb(look.base_color);
        block.tip_color = rgb(look.tip_color);
        block.flower_color = rgb(look.flower_color);
        block.screen = [
            1.0 / extent.width.max(1) as f32,
            1.0 / extent.height.max(1) as f32,
            REFRACTION_STRENGTH,
            if self.full_water { 1.0 } else { 0.0 },
        ];
        let focus = ground_focus(scene.camera.view_proj, scene.camera.position);
        block.focus = [focus.x, focus.y, focus.z, 0.0];
        block.water_deep = self.water_styles.0;
        block.water_shallow = self.water_styles.1;
        self.frames[frame]
            .block
            .write_bytes(0, bytes_of(&self.block));

        self.draws.clear();
        if config.foliage
            && let Some(foliage) = &self.foliage
        {
            let planes = frustum_planes(scene.camera.view_proj);
            visible_runs(
                &foliage.chunks,
                &planes,
                scene.camera.position,
                focus,
                config.foliage_distance,
                &mut self.draws,
            );
        }
    }

    /// Records this frame's foliage draws (inside the main view's scene pass, after every
    /// scene-layout draw): sets 2 and 3 with the foliage layout.
    pub(crate) fn record_foliage(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        pipeline: vk::Pipeline,
        lighting: vk::DescriptorSet,
    ) {
        if self.draws.is_empty() {
            return;
        }
        // SAFETY: the pipeline was built with the foliage layout.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline,
            );
        }
        // SAFETY: both sets match the foliage layout's sets 2 and 3 and name live resources;
        // sets 0/1 bound with the scene layout stay valid (identical layouts and push range).
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.foliage_pipeline_layout.handle(),
                2,
                &[self.frames[frame].foliage, lighting],
                &[],
            );
        }
        let vertices = self.config.foliage_blades * FOLIAGE_BLADE_VERTICES;
        for &(first, count) in &self.draws {
            // SAFETY: no vertex input; the instance range lies inside the uploaded buffer
            // (runs of chunks built from it).
            unsafe {
                device
                    .device
                    .cmd_draw(command_buffer, vertices, count, 0, first);
            }
        }
    }

    /// Records the water (simple: in a scene pass, `full`: the water pass) with `pipeline`.
    pub(crate) fn record_water(
        &self,
        device: &DeviceContext,
        command_buffer: vk::CommandBuffer,
        frame: usize,
        pipeline: vk::Pipeline,
        lighting: vk::DescriptorSet,
        full: bool,
    ) {
        let Some(mesh) = self
            .water
            .as_ref()
            .filter(|_| self.config.water != WaterMode::Off)
        else {
            return;
        };
        let sets = &self.frames[frame];
        let set = if full {
            sets.water_full
        } else {
            sets.water_simple
        };
        // SAFETY: the pipeline was built with the water layout.
        unsafe {
            device.device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline,
            );
        }
        // SAFETY: as in `record_foliage`, with the water layout; the full set's images were
        // transitioned by the graph (refraction sampled, depth read-only).
        unsafe {
            device.device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                self.water_pipeline_layout.handle(),
                2,
                &[set, lighting],
                &[],
            );
        }
        // SAFETY: the mesh buffers are live and hold `WaterVertexData` and in-range `u32`
        // indices (`validate_water`).
        unsafe {
            device.device.cmd_bind_vertex_buffers(
                command_buffer,
                0,
                &[mesh.vertices.handle()],
                &[0],
            );
        }
        // SAFETY: as above.
        unsafe {
            device.device.cmd_bind_index_buffer(
                command_buffer,
                mesh.indices.handle(),
                0,
                vk::IndexType::UINT32,
            );
        }
        // SAFETY: the bound index buffer holds `index_count` in-range indices.
        unsafe {
            device
                .device
                .cmd_draw_indexed(command_buffer, mesh.index_count, 1, 0, 0, 0);
        }
    }
}

/// Rejects a water mesh the GPU cannot draw safely.
fn validate_water(data: &WaterData<'_>) -> RenderResult<()> {
    let vertices = data.vertices.len();
    if vertices == 0
        || vertices > MAX_WATER_VERTICES
        || data.indices.is_empty()
        || !data.indices.len().is_multiple_of(3)
        || data.styles.len() > MAX_WATER_BODIES
    {
        return Err(RenderError::InvalidWater);
    }
    if data.indices.iter().any(|&index| index as usize >= vertices) {
        return Err(RenderError::InvalidWater);
    }
    let finite = data.vertices.iter().all(|vertex| {
        vertex.position.iter().all(|v| v.is_finite())
            && vertex.depth.is_finite()
            && vertex.flow.iter().all(|v| v.is_finite())
            && (vertex.body as usize) < MAX_WATER_BODIES
    });
    if !finite {
        return Err(RenderError::InvalidWater);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_water;
    use crate::environment::{WaterData, WaterStyle, WaterVertexData};

    #[test]
    fn water_meshes_are_validated() {
        let vertex = WaterVertexData::new([0.0, 0.1, 0.0], 0.3, [1.0, 0.0], 0);
        let vertices = [vertex; 3];
        let styles = [WaterStyle::default()];
        assert!(validate_water(&WaterData::new(&vertices, &[0, 1, 2], &styles)).is_ok());
        assert!(validate_water(&WaterData::new(&vertices, &[0, 1, 3], &styles)).is_err());
        assert!(validate_water(&WaterData::new(&vertices, &[0, 1], &styles)).is_err());
        let bad = [WaterVertexData::new([f32::NAN, 0.0, 0.0], 0.3, [0.0; 2], 0); 3];
        assert!(validate_water(&WaterData::new(&bad, &[0, 1, 2], &styles)).is_err());
        let body = [WaterVertexData::new([0.0; 3], 0.3, [0.0; 2], 9); 3];
        assert!(validate_water(&WaterData::new(&body, &[0, 1, 2], &styles)).is_err());
    }
}
