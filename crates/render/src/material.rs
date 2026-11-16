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
//! full team mask, white emissive and model textures).
//!
//! **Tiered (N2c, ADR-0023 §6d):** every texture is also registered in the bindless table,
//! and every material gets a [`MaterialRecord`] in one host-written table the textured units'
//! G-buffer stage reads by device address (`unit_material_gbuffer.slang`). The per-material
//! sets stay: Compat and the forward Tiered frame (`unit.frag`) bind them. Their parameter
//! block is the material's Compat fallback ([`moba_assets::MaterialDesc::compat_fallback`],
//! ADR-0023 §10), with the emissive-mask and unlit flags and the dielectric F0. BC textures upload as is when the device enables
//! `textureCompressionBC` and samples the format with linear filtering; otherwise (or when
//! [`RenderConfig::compressed_textures`](crate::RenderConfig) is off) they are decoded to
//! their 8-bit equivalent on the CPU at load.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::MemoryLocation;
use moba_assets::{
    CompatMaterial, MaterialShader, ModelParams, TextureData, TextureFormat, TextureSlot,
};

use crate::bindless::{BindlessTable, TextureHandle};
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
    /// The shading model and its parameters (ADR-0023 §6c). The deferred Tiered frame shades
    /// it; Compat and the forward Tiered frame draw [`Self::compat`].
    pub model: ModelParams,
    /// Dielectric F0, `0..=0.16` (Tiered; ADR-0023 §6b): 0.04 is IOR 1.5.
    pub f0: f32,
    /// Dust layer amount, `0..=1` (Tiered; ADR-0023 §6a).
    pub dust: f32,
    /// What Compat and the forward Tiered frame draw (ADR-0023 §10); `None` draws the
    /// material's own factors (a `STANDARD` material's fallback is itself).
    pub compat: Option<CompatMaterial>,
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
            model: ModelParams::Standard,
            f0: 0.04,
            dust: 0.0,
            compat: None,
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
            model: desc.model,
            f0: desc.dielectric_f0(),
            dust: desc.dust,
            compat: Some(desc.compat_fallback()),
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
    /// x wrap (negative: the scene's), y rim, z ramp, w flags ([`MATERIAL_FLAG_EMISSIVE_MASK`]
    /// + 2 × [`MATERIAL_FLAG_UNLIT`], as a float).
    style: [f32; 4],
    /// rgb painted-specular tint, w the dielectric F0 (the forward Tiered frame's).
    specular_tint: [f32; 4],
}

/// The material's emissive is masked by `metallic_roughness.b` (ADR-0023 §6d): set when the
/// bound map has a third channel ([`moba_assets::has_emissive_mask`]).
pub(crate) const MATERIAL_FLAG_EMISSIVE_MASK: u32 = 1;
/// `UNLIT` (the UBO only): the forward Tiered frame draws the emissive alone.
pub(crate) const MATERIAL_FLAG_UNLIT: u32 = 2;

/// A finite `value` clamped to `0..=high` (0 for NaN or infinity).
fn clamp(value: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, high)
    } else {
        0.0
    }
}

impl MaterialParamsGpu {
    /// The parameter block: the material's Compat fallback (or its own factors), its
    /// stylisation, `mask` (the emissive mask flag) and its F0.
    fn from(material: &MaterialData, mask: bool) -> Self {
        // (base colour factor, metallic, roughness, emissive, specular scale, rim)
        let fallback = material.compat.map_or(
            (
                material.base_color_factor,
                material.metallic,
                material.roughness,
                material.emissive,
                material.specular,
                material.rim,
            ),
            |compat: CompatMaterial| {
                (
                    compat.base_color_factor,
                    compat.metallic,
                    compat.roughness,
                    compat.emissive,
                    compat.specular,
                    compat.rim,
                )
            },
        );
        let (base_color_factor, metallic, roughness, emissive, specular, rim) = fallback;
        let unlit = matches!(material.model, ModelParams::Unlit);
        let flags =
            u32::from(mask) * MATERIAL_FLAG_EMISSIVE_MASK + u32::from(unlit) * MATERIAL_FLAG_UNLIT;
        Self {
            base_color_factor: base_color_factor.map(|value| clamp(value, 1.0)),
            params: [
                clamp(metallic, 1.0),
                clamp(roughness, 1.0),
                clamp(material.normal_scale, 2.0),
                clamp(material.team_tint, 1.0),
            ],
            emissive: [
                clamp(emissive[0], 16.0),
                clamp(emissive[1], 16.0),
                clamp(emissive[2], 16.0),
                clamp(specular, 2.0),
            ],
            style: [
                material.wrap.map_or(-1.0, |wrap| clamp(wrap, 1.0)),
                clamp(rim, 4.0),
                clamp(material.ramp, 1.0),
                flags as f32,
            ],
            specular_tint: [
                clamp(material.specular_tint[0], 1.0),
                clamp(material.specular_tint[1], 1.0),
                clamp(material.specular_tint[2], 1.0),
                clamp(material.f0, moba_assets::MAX_DIELECTRIC_F0),
            ],
        }
    }
}

/// One material in the Tiered material table (ADR-0023 §6d; 64 bytes, `MaterialRecord` in
/// `shaders/slang/lib/material.slang`, checked against its reflection). Texture words hold
/// two bindless handles each (low, high); the model word the model data as the three
/// G-buffer channels (§6c, UNORM8), the model ID in bits 24–27 and flags in 28–31.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct MaterialRecord {
    pub(crate) base_color_factor: [f32; 4],
    pub(crate) emissive: [f32; 3],
    pub(crate) normal_scale: f32,
    pub(crate) metallic: f32,
    pub(crate) roughness: f32,
    pub(crate) team_tint: f32,
    pub(crate) dust: f32,
    /// Base colour | normal << 16.
    pub(crate) base_normal: u32,
    /// Metallic-roughness | team mask << 16.
    pub(crate) mr_mask: u32,
    /// Emissive | model texture << 16.
    pub(crate) emissive_model: u32,
    /// Data r | g << 8 | b << 16 | model << 24 | flags << 28.
    pub(crate) model: u32,
}

/// `x` (clamped to `0..=1`) as a UNORM8 byte.
fn unorm8(x: f32) -> u32 {
    (clamp(x, 1.0) * 255.0 + 0.5) as u32
}

impl MaterialRecord {
    /// The record of `material` with `textures` (bindless handles in [`TextureSlot`] order)
    /// and `mask` (the emissive mask flag).
    pub(crate) fn new(
        material: &MaterialData,
        textures: [u32; TextureSlot::COUNT],
        mask: bool,
    ) -> Self {
        let ao = 255;
        let f0_byte = unorm8((clamp(material.f0, moba_assets::MAX_DIELECTRIC_F0) / 0.16).sqrt());
        let thickness = |nm: f32| unorm8((nm - 100.0) / 300.0);
        let [r, g, b] = match material.model {
            ModelParams::Clearcoat { weight, roughness } => [unorm8(weight), unorm8(roughness), ao],
            ModelParams::Sheen {
                weight,
                roughness,
                tint,
            } => [unorm8(weight), unorm8(roughness), unorm8(tint)],
            ModelParams::Anisotropic { strength, rotation } => {
                let angle = if rotation.is_finite() {
                    rotation.rem_euclid(core::f32::consts::PI)
                } else {
                    0.0
                };
                [unorm8(strength), unorm8(angle / core::f32::consts::PI), ao]
            }
            ModelParams::ThinFilm {
                thickness_min,
                thickness_max,
                ior,
            } => [
                thickness(thickness_min),
                unorm8(ior - 1.0),
                thickness(thickness_max),
            ],
            ModelParams::Foliage { translucency } => [unorm8(translucency), ao, 0],
            ModelParams::Unlit => [0, 0, 0],
            // `Standard`, `Transmissive` and any later variant: F0 and the material AO.
            _ => [f0_byte, ao, 0],
        };
        let model = u32::from(material.model.model().gbuffer_id());
        let pack = |low: u32, high: u32| (low & 0xFFFF) | (high << 16);
        let slot = |slot: TextureSlot| textures[slot.index()];
        Self {
            base_color_factor: material.base_color_factor.map(|value| clamp(value, 1.0)),
            emissive: material.emissive.map(|value| clamp(value, 16.0)),
            normal_scale: clamp(material.normal_scale, 2.0),
            metallic: clamp(material.metallic, 1.0),
            roughness: clamp(material.roughness, 1.0),
            team_tint: clamp(material.team_tint, 1.0),
            dust: clamp(material.dust, 1.0),
            base_normal: pack(slot(TextureSlot::BaseColor), slot(TextureSlot::Normal)),
            mr_mask: pack(
                slot(TextureSlot::MetallicRoughness),
                slot(TextureSlot::Mask),
            ),
            emissive_model: pack(slot(TextureSlot::Emissive), slot(TextureSlot::Model)),
            model: r
                | (g << 8)
                | (b << 16)
                | ((model & 0xF) << 24)
                | (u32::from(mask) * MATERIAL_FLAG_EMISSIVE_MASK) << 28,
        }
    }
}

struct RegisteredTexture {
    image: GpuImage,
    /// Format as uploaded (the 8-bit equivalent when a BC texture was decoded).
    format: TextureFormat,
    /// A cube map (never bindable to a material slot or as a ramp).
    cube: bool,
    /// Its bindless handle (Tiered only).
    bindless: Option<TextureHandle>,
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
    /// White base colour, flat normal, metallic 0 / roughness 1, full mask, white emissive
    /// and white model data (slot order).
    defaults: [GpuImage; TextureSlot::COUNT],
    /// The defaults' bindless handles (Tiered; zero, the magenta fallback, on Compat).
    default_handles: [u32; TextureSlot::COUNT],
    /// The Tiered material table: [`MAX_MATERIALS`] records, host-written at load.
    table: Option<GpuBuffer>,
    /// Its device address (0 without a table).
    table_address: u64,
    sampler: Sampler,
    set_layout: DescriptorSetLayout,
    /// Upload BC formats as is (device feature + config).
    compressed: bool,
    stats: MaterialStats,
}

/// Bindings of the material set (set 2): the material block and one combined image sampler
/// per set slot ([`TextureSlot::SET_SLOTS`]; the emissive and model textures are Tiered-only,
/// bindless), all fragment-only. Also counted by the scene layout's descriptor budget
/// (`descriptor_budget`).
pub(crate) fn set_layout_bindings()
-> [vk::DescriptorSetLayoutBinding<'static>; 1 + TextureSlot::SET_COUNT] {
    let mut bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        1 + TextureSlot::SET_COUNT];
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
    /// sampler anisotropy (`None` or 1 disables it). With `bindless` (Tiered) the defaults
    /// are registered in it, the sampler goes into its material slot and the material table
    /// is created.
    pub(crate) fn new(
        device: &DeviceContext,
        compressed: bool,
        anisotropy: Option<f32>,
        mut bindless: Option<&mut BindlessTable>,
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
            default("default-emissive", TextureFormat::Rgba8Srgb, &[255; 4])?,
            default("default-model", TextureFormat::Rgba8Unorm, &[255; 4])?,
        ];
        let mut default_handles = [0; TextureSlot::COUNT];
        let mut table = None;
        if let Some(bindless) = bindless.as_deref_mut() {
            for (handle, image) in default_handles.iter_mut().zip(&defaults) {
                *handle = bindless.register_texture(image.view())?.index();
            }
            bindless.set_material_sampler(sampler.handle());
            table = Some(GpuBuffer::new(
                device.device.clone(),
                device.allocator(),
                "material-table",
                (MAX_MATERIALS * core::mem::size_of::<MaterialRecord>()) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                MemoryLocation::CpuToGpu,
            )?);
        }
        let table_address = table.as_ref().map_or(0, |table| {
            // SAFETY: a bindless table exists only on a Tiered device, which enables
            // `bufferDeviceAddress` and allocates every block with `DEVICE_ADDRESS`
            // (`device.rs`); the buffer has the usage flag.
            unsafe { table.device_address() }
        });
        Ok(Self {
            entries: Vec::with_capacity(MAX_MATERIALS),
            pools: Vec::new(),
            param_buffers: Vec::new(),
            textures: Vec::with_capacity(MAX_TEXTURES),
            defaults,
            default_handles,
            table,
            table_address,
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
        bindless: Option<&mut BindlessTable>,
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
        // Tiered: also a bindless handle (a cube map is never a material texture).
        let handle = match bindless {
            Some(bindless) if !upload.is_cube() => Some(bindless.register_texture(image.view())?),
            _ => None,
        };
        self.stats.textures += 1;
        self.stats.decoded_textures += usize::from(upload.format() != data.format());
        self.stats.texture_bytes += upload.data().len();
        self.textures.push(RegisteredTexture {
            image,
            format: upload.format(),
            cube: upload.is_cube(),
            bindless: handle,
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
                    .descriptor_count(count * TextureSlot::SET_COUNT as u32),
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
                let block = MaterialParamsGpu::from(material, self.emissive_mask(material));
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
        // Tiered: every material's record, at its id (no pending frame reads a new index).
        if self.table.is_some() {
            let records: Vec<MaterialRecord> = materials
                .iter()
                .map(|material| {
                    let handles: [u32; TextureSlot::COUNT] = core::array::from_fn(|slot| {
                        material.textures[slot]
                            .and_then(|texture| self.textures.get(texture.index()))
                            .and_then(|texture| texture.bindless)
                            .map_or(self.default_handles[slot], TextureHandle::index)
                    });
                    MaterialRecord::new(material, handles, self.emissive_mask(material))
                })
                .collect();
            if let Some(table) = &mut self.table {
                table.write_bytes(
                    self.entries.len() * core::mem::size_of::<MaterialRecord>(),
                    bytemuck::cast_slice(&records),
                );
            }
        }
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
        let image_infos: [[vk::DescriptorImageInfo; 1]; TextureSlot::SET_COUNT] =
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
            .buffer_info(&buffer_info); 1 + TextureSlot::SET_COUNT];
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

    /// Whether `material`'s metallic-roughness map carries the emissive mask (a third
    /// channel, ADR-0023 §6d).
    fn emissive_mask(&self, material: &MaterialData) -> bool {
        material.textures[TextureSlot::MetallicRoughness.index()]
            .and_then(|texture| self.textures.get(texture.index()))
            .is_some_and(|texture| moba_assets::has_emissive_mask(texture.format))
    }

    /// The material table's device address (Tiered), for the G-buffer stage's push constant.
    pub(crate) fn table_address(&self) -> Option<u64> {
        self.table.is_some().then_some(self.table_address)
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
        let block = MaterialParamsGpu::from(&material, false);
        assert_eq!(block.base_color_factor, [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(block.params, [0.0, 1.0, 2.0, 0.25]);
        // The scene's wrap unless the material sets one; stylisation clamped.
        assert_eq!(block.style, [-1.0, 1.0, 1.0, 0.0]);
        let styled = MaterialParamsGpu::from(
            &material
                .with_style(Some(3.0), 9.0, 0.5)
                .with_specular([0.5, 2.0, f32::INFINITY], 5.0),
            true,
        );
        // The emissive-mask flag in style.w, the F0 (0.04 by default) in specular_tint.w.
        assert_eq!(styled.style, [1.0, 4.0, 0.5, 1.0]);
        assert_eq!(styled.specular_tint, [0.5, 1.0, 0.0, 0.04]);
        assert_eq!(styled.emissive[3], 2.0);
        assert_eq!(material.texture(TextureSlot::Mask), Some(TextureId(3)));
        assert_eq!(material.texture(TextureSlot::Normal), None);
    }
}
