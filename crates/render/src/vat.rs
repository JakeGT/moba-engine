//! Vertex-animation-texture meshes (R4b, ADR-0012 amendment): their textures, the VAT
//! descriptor set layout (set 2 of the VAT pipelines) and one descriptor set per mesh.
//!
//! Everything is created at load time ([`VatMeshes::register`]); the frame only binds a set
//! and pushes the mesh's layout per VAT draw.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use moba_assets::{TextureData, VatDescriptor};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{DescriptorPool, DescriptorSetLayout, GpuImage, Sampler};

/// Most VAT meshes one renderer holds (melee and ranged minions and a few spares).
pub const MAX_VAT_MESHES: usize = 8;

/// Push-constant block of `vat.vert` (16 bytes, the same range as the skinned push).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct VatPush {
    pub(crate) vertex_count: u32,
    pub(crate) frame_count: u32,
    pub(crate) width: u32,
    pub(crate) reserved: u32,
}

/// One registered VAT mesh's texture and descriptor set.
struct VatEntry {
    _image: GpuImage,
    set: vk::DescriptorSet,
    push: VatPush,
}

/// The VAT textures, their sampler, set layout and descriptor sets.
pub(crate) struct VatMeshes {
    entries: Vec<VatEntry>,
    pool: DescriptorPool,
    layout: DescriptorSetLayout,
    sampler: Sampler,
}

/// Bindings of the VAT pipelines' set 2: the mesh's VAT texture, vertex stage. Also counted
/// by the VAT layout's descriptor budget (`descriptor_budget`).
pub(crate) fn set_layout_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 1] {
    [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::VERTEX)]
}

impl VatMeshes {
    /// The set layout (one vertex-stage combined image sampler), a nearest clamp sampler and a
    /// pool for [`MAX_VAT_MESHES`] sets.
    pub(crate) fn new(device: &DeviceContext) -> RenderResult<Self> {
        let binding = set_layout_bindings();
        let layout = DescriptorSetLayout::new(device.device.clone(), &binding)?;
        let count = u32::try_from(MAX_VAT_MESHES).unwrap_or(u32::MAX);
        let pool = DescriptorPool::new(
            device.device.clone(),
            &[vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(count)],
            count,
        )?;
        // `texelFetch` ignores filtering; nearest and clamp keep the sampler trivially valid.
        let sampler = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .max_lod(0.0),
        )?;
        Ok(Self {
            entries: Vec::with_capacity(MAX_VAT_MESHES),
            pool,
            layout,
            sampler,
        })
    }

    /// The VAT set layout (set 2 of the VAT pipeline layout).
    pub(crate) fn set_layout(&self) -> vk::DescriptorSetLayout {
        self.layout.handle()
    }

    /// Checks `texture` and `mesh` against `descriptor`, uploads the texture and writes its
    /// descriptor set; returns the entry index. Load time only.
    ///
    /// # Errors
    /// [`RenderError::InvalidVat`] for a mismatch or with every slot used, otherwise upload
    /// errors.
    pub(crate) fn register(
        &mut self,
        device: &DeviceContext,
        mesh: &moba_assets::MeshData,
        texture: &TextureData,
        descriptor: &VatDescriptor,
    ) -> RenderResult<usize> {
        validate(self.entries.len(), mesh, texture, descriptor)?;
        let image = crate::texture::upload(device, "vat", texture)?;
        let sets = self
            .pool
            .allocate(&device.device, &[self.layout.handle()])?;
        let set = sets[0];
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(self.sampler.handle())
            .image_view(image.view())
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .image_info(&image_info)];
        // SAFETY: the set was just allocated with the VAT layout and is not in use by any
        // command buffer; the view and sampler are live and owned by `self`.
        unsafe { device.device.update_descriptor_sets(&write, &[]) };
        self.entries.push(VatEntry {
            _image: image,
            set,
            push: VatPush {
                vertex_count: descriptor.vertex_count,
                frame_count: descriptor.frame_count,
                width: descriptor.width,
                reserved: 0,
            },
        });
        Ok(self.entries.len() - 1)
    }

    /// Entry `index`'s descriptor set and push constants.
    pub(crate) fn entry(&self, index: usize) -> Option<(vk::DescriptorSet, VatPush)> {
        self.entries.get(index).map(|entry| (entry.set, entry.push))
    }

    /// Registered VAT meshes.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Every check [`VatMeshes::register`] makes before touching the GPU, with `used` VAT slots
/// taken: a free slot, the texture and mesh matching the descriptor, and (re-checked, because
/// the descriptor and the texture come from different untrusted files) the texture holding
/// every frame at the descriptor's width, which keeps the shader's fetches inside it.
///
/// # Errors
/// [`RenderError::InvalidVat`] naming the first failed check.
pub(crate) fn validate(
    used: usize,
    mesh: &moba_assets::MeshData,
    texture: &TextureData,
    descriptor: &VatDescriptor,
) -> RenderResult<()> {
    if used >= MAX_VAT_MESHES {
        return Err(RenderError::InvalidVat("every VAT slot is in use"));
    }
    descriptor
        .check_texture(texture)
        .map_err(|_| RenderError::InvalidVat("the texture does not match its descriptor"))?;
    descriptor
        .check_mesh(mesh)
        .map_err(|_| RenderError::InvalidVat("the mesh does not match its descriptor"))?;
    let texels = u64::from(descriptor.vertex_count) * u64::from(descriptor.frame_count) * 2;
    if texels > u64::from(texture.width()) * u64::from(texture.height())
        || descriptor.width != texture.width()
    {
        return Err(RenderError::InvalidVat(
            "the texture is too small for its frames",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_VAT_MESHES, validate};
    use crate::error::RenderError;
    use moba_assets::{MeshData, TextureData, TextureFormat, VatDescriptor};

    /// The shipped sample: `sentinel-skinned` and its baked minion VAT.
    fn sample() -> (MeshData, TextureData, VatDescriptor) {
        let mesh = moba_assets::load_glb(include_bytes!(
            "../../../content/models/sentinel-skinned.glb"
        ))
        .expect("model");
        let texture = moba_assets::load_ktx2(include_bytes!("../../../content/vat/minion.ktx2"))
            .expect("texture");
        let descriptor = VatDescriptor::parse(include_str!("../../../content/vat/minion.toml"))
            .expect("descriptor");
        (mesh, texture, descriptor)
    }

    fn reason(result: crate::error::RenderResult<()>) -> &'static str {
        match result {
            Err(RenderError::InvalidVat(reason)) => reason,
            other => panic!("expected InvalidVat, got {other:?}"),
        }
    }

    #[test]
    fn the_sample_validates_until_the_slots_run_out() {
        let (mesh, texture, descriptor) = sample();
        assert!(validate(0, &mesh, &texture, &descriptor).is_ok());
        assert!(validate(MAX_VAT_MESHES - 1, &mesh, &texture, &descriptor).is_ok());
        assert_eq!(
            reason(validate(MAX_VAT_MESHES, &mesh, &texture, &descriptor)),
            "every VAT slot is in use"
        );
    }

    #[test]
    fn textures_that_do_not_hold_every_frame_are_rejected() {
        let (mesh, texture, descriptor) = sample();
        // One row short of the descriptor's extent.
        let bytes = TextureFormat::Rgba16Float
            .level_bytes(texture.width(), texture.height() - 1)
            .expect("size");
        let short = TextureData::new(
            TextureFormat::Rgba16Float,
            texture.width(),
            texture.height() - 1,
            &[vec![0; bytes]],
        )
        .expect("texture");
        assert_eq!(
            reason(validate(0, &mesh, &short, &descriptor)),
            "the texture does not match its descriptor"
        );
        // A descriptor (from its own untrusted file) claiming one frame more than the texture
        // holds at the same extent: the shader would fetch past the last texel.
        let mut longer = descriptor.clone();
        longer.frame_count += 1;
        assert_eq!(
            reason(validate(0, &mesh, &texture, &longer)),
            "the texture is too small for its frames"
        );
    }

    #[test]
    fn a_mesh_that_was_not_baked_is_rejected() {
        let (_, texture, descriptor) = sample();
        let other = moba_assets::load_glb(include_bytes!("../../../content/models/sentinel.glb"))
            .expect("model");
        assert_eq!(
            reason(validate(0, &other, &texture, &descriptor)),
            "the mesh does not match its descriptor"
        );
    }
}
