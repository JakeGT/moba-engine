//! Materials: shader permutation + parameter block + texture set (ADR-0009, ADR-0010).
//!
//! Textures are registered once ([`Materials::register_texture`]), then materials name them
//! by [`TextureId`] ([`Materials::register_materials`]). Each textured material gets its own
//! descriptor set (set 2: parameter block plus four combined image samplers). The sets of one
//! registration call come from one descriptor pool sized exactly for that call's content,
//! and their parameter blocks from one host-written uniform buffer: everything is allocated
//! and written at load time, so drawing a material only binds its set.
//!
//! Unbound slots use 1×1 defaults (white base colour, flat normal, metallic 0 / roughness 1,
//! full team mask). BC textures upload as is when the device enables
//! `textureCompressionBC` and samples the format with linear filtering; otherwise (or when
//! [`RenderConfig::compressed_textures`](crate::RenderConfig) is off) they are decoded to
//! their 8-bit equivalent on the CPU at load.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::MemoryLocation;
use moba_assets::{MaterialShader, TextureData, TextureFormat, TextureSlot};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, Sampler};
use crate::texture;

/// Most textures one renderer holds (including none of the built-in defaults).
pub const MAX_TEXTURES: usize = 256;
/// Most materials one renderer holds.
pub const MAX_MATERIALS: usize = 64;

/// A texture uploaded with [`Renderer::register_texture`](crate::Renderer::register_texture).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(u16);

impl TextureId {
    /// The id with registration index `index`. Ids are checked wherever they are used, so
    /// an id the renderer never handed out is rejected ([`RenderError::InvalidMaterial`]),
    /// not dereferenced.
    #[must_use]
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// Registration index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A material created with
/// [`Renderer::register_materials`](crate::Renderer::register_materials).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(u16);

impl MaterialId {
    /// The id with registration index `index`. Ids are checked wherever they are used, so
    /// an id the renderer never handed out is rejected ([`RenderError::InvalidMaterial`]),
    /// not dereferenced.
    #[must_use]
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// Registration index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A material to register: the permutation, the parameter block and the texture set.
///
/// Build it with [`MaterialData::new`] and the `with_*` methods, or from a validated
/// descriptor with [`MaterialData::from_desc`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MaterialData {
    /// Shader permutation.
    pub shader: MaterialShader,
    /// Texture per [`TextureSlot`]; private so a new slot is not a breaking change (use
    /// [`MaterialData::with_texture`] and [`MaterialData::texture`]).
    textures: [Option<TextureId>; TextureSlot::COUNT],
    /// Linear RGBA multiplier of the base colour.
    pub base_color_factor: [f32; 4],
    /// Metallic factor, times the metallic-roughness texture's R.
    pub metallic: f32,
    /// Roughness factor, times the metallic-roughness texture's G.
    pub roughness: f32,
    /// Normal map strength.
    pub normal_scale: f32,
    /// How strongly the mask tints the base colour by the unit colour.
    pub team_tint: f32,
    /// Linear RGB emissive factor, added after lighting.
    pub emissive: [f32; 3],
    /// Wrap lighting `0..=1`; `None` uses the scene's ([`SceneLighting::wrap`]).
    ///
    /// [`SceneLighting::wrap`]: crate::SceneLighting::wrap
    pub wrap: Option<f32>,
    /// Multiplier of the scene's rim strength, `0..=4`.
    pub rim: f32,
    /// Multiplier of the scene's ramp strength, `0..=1`.
    pub ramp: f32,
    /// Specular scale, `0..=2` (times the scene's).
    pub specular: f32,
    /// Painted-specular tint, linear RGB `0..=1` (times the scene's).
    pub specular_tint: [f32; 3],
}

impl MaterialData {
    /// A material with no textures and neutral parameters.
    #[must_use]
    pub const fn new(shader: MaterialShader) -> Self {
        Self {
            shader,
            textures: [None; TextureSlot::COUNT],
            base_color_factor: [1.0; 4],
            metallic: 0.0,
            roughness: 1.0,
            normal_scale: 1.0,
            team_tint: 1.0,
            emissive: [0.0; 3],
            wrap: None,
            rim: 1.0,
            ramp: 1.0,
            specular: 1.0,
            specular_tint: [1.0; 3],
        }
    }

    /// The shader and parameters of `desc`, with no textures yet: the caller uploads each
    /// texture `desc` names and binds it with [`Self::with_texture`].
    #[must_use]
    pub fn from_desc(desc: &moba_assets::MaterialDesc) -> Self {
        Self {
            shader: desc.shader,
            textures: [None; TextureSlot::COUNT],
            base_color_factor: desc.base_color_factor,
            metallic: desc.metallic,
            roughness: desc.roughness,
            normal_scale: desc.normal_scale,
            team_tint: desc.team_tint,
            emissive: desc.emissive,
            wrap: desc.wrap,
            rim: desc.rim,
            ramp: desc.ramp,
            specular: desc.specular,
            specular_tint: desc.specular_tint,
        }
    }

    /// Returns this material with `texture` bound to `slot`.
    #[must_use]
    pub const fn with_texture(mut self, slot: TextureSlot, texture: TextureId) -> Self {
        self.textures[slot.index()] = Some(texture);
        self
    }

    /// The texture bound to `slot`, if any.
    #[must_use]
    pub const fn texture(&self, slot: TextureSlot) -> Option<TextureId> {
        self.textures[slot.index()]
    }

    /// Returns this material with [`MaterialData::base_color_factor`] set.
    #[must_use]
    pub const fn with_base_color_factor(mut self, factor: [f32; 4]) -> Self {
        self.base_color_factor = factor;
        self
    }

    /// Returns this material with [`MaterialData::team_tint`] set.
    #[must_use]
    pub const fn with_team_tint(mut self, team_tint: f32) -> Self {
        self.team_tint = team_tint;
        self
    }

    /// Returns this material with the metallic and roughness factors set.
    #[must_use]
    pub const fn with_metallic_roughness(mut self, metallic: f32, roughness: f32) -> Self {
        self.metallic = metallic;
        self.roughness = roughness;
        self
    }

    /// Returns this material with [`MaterialData::emissive`] set.
    #[must_use]
    pub const fn with_emissive(mut self, emissive: [f32; 3]) -> Self {
        self.emissive = emissive;
        self
    }

    /// Returns this material with the stylisation set (ADR-0011): wrap (`None` = the
    /// scene's), rim and ramp multipliers.
    #[must_use]
    pub const fn with_style(mut self, wrap: Option<f32>, rim: f32, ramp: f32) -> Self {
        self.wrap = wrap;
        self.rim = rim;
        self.ramp = ramp;
        self
    }

    /// Returns this material with the painted-specular tint and scale set.
    #[must_use]
    pub const fn with_specular(mut self, tint: [f32; 3], scale: f32) -> Self {
        self.specular_tint = tint;
        self.specular = scale;
        self
    }
}

/// `MaterialBlock` in `shaders/include/material.glsl` (std140, 80 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MaterialParamsGpu {
    base_color_factor: [f32; 4],
    params: [f32; 4],
    /// rgb emissive, w specular scale.
    emissive: [f32; 4],
    /// x wrap (negative: the scene's), y rim, z ramp, w reserved.
    style: [f32; 4],
    /// rgb painted-specular tint, w reserved.
    specular_tint: [f32; 4],
}

impl MaterialParamsGpu {
    fn from(material: &MaterialData) -> Self {
        let clamp = |value: f32, high: f32| {
            if value.is_finite() {
                value.clamp(0.0, high)
            } else {
                0.0
            }
        };
        Self {
            base_color_factor: material.base_color_factor.map(|value| clamp(value, 1.0)),
            params: [
                clamp(material.metallic, 1.0),
                clamp(material.roughness, 1.0),
                clamp(material.normal_scale, 2.0),
                clamp(material.team_tint, 1.0),
            ],
            emissive: [
                clamp(material.emissive[0], 16.0),
                clamp(material.emissive[1], 16.0),
                clamp(material.emissive[2], 16.0),
                clamp(material.specular, 2.0),
            ],
            style: [
                material.wrap.map_or(-1.0, |wrap| clamp(wrap, 1.0)),
                clamp(material.rim, 4.0),
                clamp(material.ramp, 1.0),
                0.0,
            ],
            specular_tint: [
                clamp(material.specular_tint[0], 1.0),
                clamp(material.specular_tint[1], 1.0),
                clamp(material.specular_tint[2], 1.0),
                0.0,
            ],
        }
    }
}

struct RegisteredTexture {
    image: GpuImage,
    /// Format as uploaded (the 8-bit equivalent when a BC texture was decoded).
    format: TextureFormat,
    /// A cube map (never bindable to a material slot or as a ramp).
    cube: bool,
}

struct GpuMaterial {
    /// Null for vertex-colour materials, which draw without a material set.
    set: vk::DescriptorSet,
}

/// Counts for logs and `content-check`-style reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MaterialStats {
    /// Registered textures.
    pub textures: usize,
    /// Of those, BC textures decoded on the CPU because the device cannot sample them.
    pub decoded_textures: usize,
    /// Registered materials.
    pub materials: usize,
    /// Descriptor sets allocated (one per textured material).
    pub descriptor_sets: usize,
    /// Descriptor pools created (one per registration call with textured materials).
    pub descriptor_pools: usize,
    /// Bytes of texel data uploaded, every level.
    pub texture_bytes: usize,
}

/// The material set layout, shared sampler, defaults and every registered texture/material.
pub(crate) struct Materials {
    entries: Vec<GpuMaterial>,
    pools: Vec<DescriptorPool>,
    param_buffers: Vec<GpuBuffer>,
    textures: Vec<RegisteredTexture>,
    /// White base colour, flat normal, metallic 0 / roughness 1, full mask (slot order).
    defaults: [GpuImage; TextureSlot::COUNT],
    sampler: Sampler,
    set_layout: DescriptorSetLayout,
    /// Upload BC formats as is (device feature + config).
    compressed: bool,
    stats: MaterialStats,
}

/// Bindings of the material set (set 2): the material block and one combined image sampler
/// per [`TextureSlot`], all fragment-only. Also counted by the scene layout's descriptor
/// budget (`descriptor_budget`).
pub(crate) fn set_layout_bindings()
-> [vk::DescriptorSetLayoutBinding<'static>; 1 + TextureSlot::COUNT] {
    let mut bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        1 + TextureSlot::COUNT];
    for (index, binding) in bindings.iter_mut().enumerate().skip(1) {
        *binding = vk::DescriptorSetLayoutBinding::default()
            .binding(index as u32)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
    }
    bindings
}

impl Materials {
    /// The set layout, sampler and default textures. `anisotropy` is the tier's maximum
    /// sampler anisotropy (`None` or 1 disables it).
    pub(crate) fn new(
        device: &DeviceContext,
        compressed: bool,
        anisotropy: Option<f32>,
    ) -> RenderResult<Self> {
        let bindings = set_layout_bindings();
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let anisotropy = anisotropy
            .zip(device.max_anisotropy)
            .map(|(wanted, limit)| wanted.min(limit))
            .filter(|&value| value > 1.0);
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::REPEAT)
                .address_mode_v(vk::SamplerAddressMode::REPEAT)
                .address_mode_w(vk::SamplerAddressMode::REPEAT)
                .anisotropy_enable(anisotropy.is_some())
                .max_anisotropy(anisotropy.unwrap_or(1.0))
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE),
        )?;
        let default = |name, format, texel: &[u8]| {
            let data = TextureData::new(format, 1, 1, &[texel.to_vec()])
                .map_err(|_| RenderError::InvalidTexture)?;
            texture::upload(device, name, &data)
        };
        let defaults = [
            default("default-base-color", TextureFormat::Rgba8Srgb, &[255; 4])?,
            default("default-normal", TextureFormat::Rg8Unorm, &[128, 128])?,
            default(
                "default-metallic-roughness",
                TextureFormat::Rg8Unorm,
                &[0, 255],
            )?,
            default("default-mask", TextureFormat::R8Unorm, &[255])?,
        ];
        Ok(Self {
            entries: Vec::with_capacity(MAX_MATERIALS),
            pools: Vec::new(),
            param_buffers: Vec::new(),
            textures: Vec::with_capacity(MAX_TEXTURES),
            defaults,
            sampler,
            set_layout,
            compressed: compressed && device.texture_compression_bc,
            stats: MaterialStats::default(),
        })
    }

    pub(crate) fn set_layout(&self) -> vk::DescriptorSetLayout {
        self.set_layout.handle()
    }

    pub(crate) fn stats(&self) -> MaterialStats {
        self.stats
    }

    /// Whether BC textures upload without a CPU decode.
    pub(crate) fn compressed(&self) -> bool {
        self.compressed
    }

    /// Uploads `data` (decoding BC on the CPU when needed). Load time only.
    /// Uploads `data` within `texture_budget` bytes of textures in all (R8, ADR-0018 §4): a
    /// texture that does not fit drops its largest mip levels until it does (counted and
    /// logged), and one that does not fit at its smallest level is refused with
    /// [`RenderError::OverBudget`].
    pub(crate) fn register_texture(
        &mut self,
        device: &DeviceContext,
        data: &TextureData,
        texture_budget: u64,
    ) -> RenderResult<TextureId> {
        if self.textures.len() >= MAX_TEXTURES {
            return Err(RenderError::TooManyTextures);
        }
        let id = TextureId(
            u16::try_from(self.textures.len()).map_err(|_| RenderError::TooManyTextures)?,
        );
        let native = data.format().is_block_compressed()
            && self.compressed
            && texture::sampleable(device, texture::vk_format(data.format()));
        let decoded;
        let upload = if data.format().is_block_compressed() && !native {
            decoded = data
                .decode_to_uncompressed()
                .map_err(|_| RenderError::InvalidTexture)?;
            &decoded
        } else {
            data
        };
        if !texture::sampleable(device, texture::vk_format(upload.format())) {
            return Err(RenderError::InvalidTexture);
        }
        let free = texture_budget.saturating_sub(crate::memory::texture_bytes());
        let reduced;
        let upload = match crate::memory::mips_to_drop(upload, free) {
            Some(0) => upload,
            Some(drop) => {
                reduced = crate::memory::without_top_mips(upload, drop)
                    .ok_or(RenderError::InvalidTexture)?;
                crate::memory::note_degraded();
                eprintln!(
                    "[vram] texture {}x{} over the tier's texture budget ({} MiB free): \
                     uploaded without its {drop} largest mip level(s), {}x{}",
                    upload.width(),
                    upload.height(),
                    free >> 20,
                    reduced.width(),
                    reduced.height(),
                );
                &reduced
            }
            None => {
                crate::memory::note_refused();
                let needed = upload
                    .level(upload.level_count().saturating_sub(1))
                    .map_or(upload.data().len(), |(_, _, bytes)| bytes.len());
                return Err(RenderError::OverBudget {
                    needed: needed as u64,
                    free,
                });
            }
        };
        let image = texture::upload(device, "material-texture", upload)?;
        self.stats.textures += 1;
        self.stats.decoded_textures += usize::from(upload.format() != data.format());
        self.stats.texture_bytes += upload.data().len();
        self.textures.push(RegisteredTexture {
            image,
            format: upload.format(),
            cube: upload.is_cube(),
        });
        Ok(id)
    }

    /// The view of a registered 2D colour texture usable as the lighting ramp: sRGB or
    /// linear RGBA (8-bit or BC7), not a cube.
    pub(crate) fn ramp_view(&self, texture: TextureId) -> RenderResult<vk::ImageView> {
        let registered = self
            .textures
            .get(texture.index())
            .ok_or(RenderError::InvalidMaterial("unknown texture id"))?;
        let colour = matches!(
            registered.format,
            TextureFormat::Rgba8Srgb | TextureFormat::Rgba8Unorm
        ) || matches!(
            registered.format,
            TextureFormat::Bc7Srgb | TextureFormat::Bc7Unorm
        );
        if registered.cube || !colour {
            return Err(RenderError::InvalidMaterial(
                "a lighting ramp must be a 2D RGBA colour texture",
            ));
        }
        Ok(registered.image.view())
    }

    /// Checks one material against the registered textures.
    fn validate(&self, material: &MaterialData) -> RenderResult<()> {
        for (slot, texture) in TextureSlot::ALL.iter().zip(&material.textures) {
            let Some(texture) = texture else { continue };
            let registered = self
                .textures
                .get(texture.index())
                .ok_or(RenderError::InvalidMaterial("unknown texture id"))?;
            if !slot.accepts(registered.format) {
                return Err(RenderError::InvalidMaterial(
                    "texture format not allowed for its slot (sRGB colour, linear data)",
                ));
            }
            if registered.cube {
                return Err(RenderError::InvalidMaterial(
                    "a cube map cannot be bound to a material slot",
                ));
            }
        }
        match material.shader {
            MaterialShader::Textured
                if material.textures[TextureSlot::BaseColor.index()].is_none() =>
            {
                Err(RenderError::InvalidMaterial(
                    "a textured material needs a base colour",
                ))
            }
            MaterialShader::VertexColor | MaterialShader::Textured => Ok(()),
            _ => Err(RenderError::InvalidMaterial("unsupported shader")),
        }
    }

    /// Creates `materials`, with one descriptor pool and one parameter buffer sized for the
    /// textured ones among them. Load time only. All-or-nothing: on error nothing is added.
    pub(crate) fn register_materials(
        &mut self,
        device: &DeviceContext,
        materials: &[MaterialData],
    ) -> RenderResult<Vec<MaterialId>> {
        if self.entries.len() + materials.len() > MAX_MATERIALS {
            return Err(RenderError::TooManyMaterials);
        }
        for material in materials {
            self.validate(material)?;
        }
        let textured: Vec<&MaterialData> = materials
            .iter()
            .filter(|material| material.shader == MaterialShader::Textured)
            .collect();
        let mut sets = Vec::new();
        if !textured.is_empty() {
            let count = textured.len() as u32;
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(count),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .descriptor_count(count * TextureSlot::COUNT as u32),
            ];
            let pool = DescriptorPool::new(device.device.clone(), &pool_sizes, count)?;
            let layouts = vec![self.set_layout.handle(); textured.len()];
            sets = pool.allocate(&device.device, &layouts)?;
            let stride = (core::mem::size_of::<MaterialParamsGpu>() as u64)
                .next_multiple_of(device.min_uniform_offset_alignment);
            let mut params = GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "material-params",
                stride * u64::from(count),
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                MemoryLocation::CpuToGpu,
            )?;
            for (index, material) in textured.iter().enumerate() {
                let block = MaterialParamsGpu::from(material);
                params.write_bytes(index * stride as usize, bytemuck::bytes_of(&block));
            }
            for (index, (material, &set)) in textured.iter().zip(&sets).enumerate() {
                self.write_set(device, set, material, &params, index as u64 * stride);
            }
            self.pools.push(pool);
            self.param_buffers.push(params);
            self.stats.descriptor_pools += 1;
            self.stats.descriptor_sets += sets.len();
        }
        let mut next_set = sets.into_iter();
        let mut ids = Vec::with_capacity(materials.len());
        for material in materials {
            let set = if material.shader == MaterialShader::Textured {
                next_set.next().unwrap_or_default()
            } else {
                vk::DescriptorSet::null()
            };
            ids.push(MaterialId(self.entries.len() as u16));
            self.entries.push(GpuMaterial { set });
        }
        self.stats.materials = self.entries.len();
        Ok(ids)
    }

    fn write_set(
        &self,
        device: &DeviceContext,
        set: vk::DescriptorSet,
        material: &MaterialData,
        params: &GpuBuffer,
        offset: u64,
    ) {
        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(params.handle())
            .offset(offset)
            .range(core::mem::size_of::<MaterialParamsGpu>() as u64)];
        let image_infos: [[vk::DescriptorImageInfo; 1]; TextureSlot::COUNT] =
            core::array::from_fn(|slot| {
                let view = material.textures[slot]
                    .and_then(|texture| self.textures.get(texture.index()))
                    .map_or(self.defaults[slot].view(), |texture| texture.image.view());
                [vk::DescriptorImageInfo::default()
                    .sampler(self.sampler.handle())
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
            });
        let mut writes = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&buffer_info); 1 + TextureSlot::COUNT];
        for (slot, info) in image_infos.iter().enumerate() {
            writes[slot + 1] = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(slot as u32 + 1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(info);
        }
        // SAFETY: the set was just allocated from a live pool with this layout, the buffer,
        // views and sampler are live, and no command buffer uses the set yet.
        unsafe { device.device.update_descriptor_sets(&writes, &[]) };
    }

    /// The material's descriptor set; null for a vertex-colour material or an unknown id.
    pub(crate) fn set(&self, material: MaterialId) -> vk::DescriptorSet {
        self.entries
            .get(material.index())
            .map_or(vk::DescriptorSet::null(), |material| material.set)
    }

    /// Whether `material` exists.
    pub(crate) fn contains(&self, material: MaterialId) -> bool {
        material.index() < self.entries.len()
    }

    /// The view of a registered texture (the VFX atlas binds one), if `texture` exists.
    pub(crate) fn texture_view(&self, texture: TextureId) -> Option<vk::ImageView> {
        self.textures
            .get(texture.index())
            .map(|registered| registered.image.view())
    }
}

#[cfg(test)]
mod tests {
    use super::{MaterialData, MaterialParamsGpu, TextureId};
    use moba_assets::{MaterialShader, TextureSlot};

    #[test]
    fn parameter_block_is_std140_sized_and_clamped() {
        assert_eq!(core::mem::size_of::<MaterialParamsGpu>(), 80);
        let mut material = MaterialData::new(MaterialShader::Textured)
            .with_texture(TextureSlot::Mask, TextureId(3))
            .with_base_color_factor([2.0, 0.5, f32::NAN, 1.0])
            .with_team_tint(0.25);
        material.normal_scale = 5.0;
        let block = MaterialParamsGpu::from(&material);
        assert_eq!(block.base_color_factor, [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(block.params, [0.0, 1.0, 2.0, 0.25]);
        // The scene's wrap unless the material sets one; stylisation clamped.
        assert_eq!(block.style, [-1.0, 1.0, 1.0, 0.0]);
        let styled = MaterialParamsGpu::from(
            &material
                .with_style(Some(3.0), 9.0, 0.5)
                .with_specular([0.5, 2.0, f32::INFINITY], 5.0),
        );
        assert_eq!(styled.style, [1.0, 4.0, 0.5, 0.0]);
        assert_eq!(styled.specular_tint, [0.5, 1.0, 0.0, 0.0]);
        assert_eq!(styled.emissive[3], 2.0);
        assert_eq!(material.texture(TextureSlot::Mask), Some(TextureId(3)));
        assert_eq!(material.texture(TextureSlot::Normal), None);
    }
}
