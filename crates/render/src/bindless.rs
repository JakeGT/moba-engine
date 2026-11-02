//! The tiered path's bindless descriptor table (ADR-0020 §3).
//!
//! One global set, bound at index 0 of every tiered pipeline layout and never rebound within a
//! frame. Its layout is `UPDATE_AFTER_BIND_POOL` and every binding is `PARTIALLY_BOUND |
//! UPDATE_AFTER_BIND | UPDATE_UNUSED_WHILE_PENDING`:
//!
//! | Binding | Type | Capacity |
//! |---|---|---|
//! | 0 | `SAMPLER[]` | 32: a fixed palette, written once at creation |
//! | 1 | `STORAGE_IMAGE[]` | 1,024 |
//! | 2 | `SAMPLED_IMAGE[]` | 16,384, `VARIABLE_DESCRIPTOR_COUNT` (the last binding, as the spec requires) |
//!
//! - **Handles** are typed `u32` indices handed out from free lists ([`SlotAllocator`]).
//! - **Fallbacks.** Slot 0 of each array is valid: a 1×1 magenta texture
//!   ([`TextureHandle::MISSING`]; slots 1 and 2 are white and a flat normal), a 1×1 storage
//!   image (cleared to zero), and the linear/wrap sampler. Creation writes the fallback into
//!   **every** slot, so a zero or never-used handle never reads an unwritten descriptor, and a
//!   released slot gets the fallback back when it retires: a stale handle reads the fallback
//!   rather than a destroyed image until the slot is reused, and another texture after that
//!   (there is no generation counter yet; N1 may add one).
//! - **Writes happen at load time only:** creation, [`BindlessTable::register_texture`] /
//!   [`BindlessTable::register_storage_image`], and [`BindlessTable::retire`] when something was
//!   released. Nothing is written per frame ([`BindlessTable::writes`] counts every descriptor
//!   written, for the evidence).
//! - **Recycling waits for the frames in flight:** a slot released at frame `n` returns to
//!   its free list only when the caller reports, through [`BindlessTable::retire`], that every
//!   frame up to and including `n` has retired (their fences signalled). The contract: `n` is
//!   the **last** submitted frame
//!   that can index the handle, and no frame recorded after the release indexes it, so the
//!   rewrite at retirement and the slot's reuse never touch a descriptor a pending command
//!   buffer uses (`UPDATE_UNUSED_WHILE_PENDING` allows only that). The caller keeps the image
//!   alive until the retirement.
//! - **Descriptor heap later** (§3d): this is the only code that creates or writes the set.
//!
//! N0 creates the table on the tiered path and the `bindless_smoke`/`rt_smoke` tests exercise
//! it; no production pass uses it yet. Compat never creates it (ADR-0009 plain sets).

use core::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use ash::vk;
use ash::vk::TaggedStructure as _;
use gpu_allocator::MemoryLocation;

use crate::caps::{BINDLESS_SAMPLED_IMAGES, BINDLESS_SAMPLERS, BINDLESS_STORAGE_IMAGES};
use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{
    CommandPool, DescriptorPool, DescriptorSetLayout, Fence, GpuBuffer, GpuImage, Sampler,
};

/// Binding 0: the sampler palette.
pub(crate) const BINDING_SAMPLERS: u32 = 0;
/// Binding 1: storage images.
pub(crate) const BINDING_STORAGE_IMAGES: u32 = 1;
/// Binding 2: sampled images (textures).
pub(crate) const BINDING_TEXTURES: u32 = 2;

/// Sampler slots (ADR-0020 §3a).
pub(crate) const SAMPLER_SLOTS: u32 = BINDLESS_SAMPLERS as u32;
/// Storage-image slots.
pub(crate) const STORAGE_IMAGE_SLOTS: u32 = BINDLESS_STORAGE_IMAGES as u32;
/// Texture slots (the variable-count binding's allocated count).
pub(crate) const TEXTURE_SLOTS: u32 = BINDLESS_SAMPLED_IMAGES as u32;

/// The image format of every fallback image.
const FALLBACK_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// Magenta (missing), white and a flat tangent-space normal: texture slots 0, 1 and 2.
const FALLBACK_TEXELS: [[u8; 4]; 3] = [
    [255, 0, 255, 255],
    [255, 255, 255, 255],
    [128, 128, 255, 255],
];

/// Every binding's flags; the texture array adds `VARIABLE_DESCRIPTOR_COUNT`.
const BINDING_FLAGS: vk::DescriptorBindingFlags = vk::DescriptorBindingFlags::from_raw(
    vk::DescriptorBindingFlags::PARTIALLY_BOUND.as_raw()
        | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND.as_raw()
        | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING.as_raw(),
);

/// The bindless set's bindings, visible to every stage. Also counted by the descriptor budget
/// test against the admission minimums.
pub(crate) fn set_layout_bindings() -> [vk::DescriptorSetLayoutBinding<'static>; 3] {
    [
        vk::DescriptorSetLayoutBinding::default()
            .binding(BINDING_SAMPLERS)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .descriptor_count(SAMPLER_SLOTS)
            .stage_flags(vk::ShaderStageFlags::ALL),
        vk::DescriptorSetLayoutBinding::default()
            .binding(BINDING_STORAGE_IMAGES)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(STORAGE_IMAGE_SLOTS)
            .stage_flags(vk::ShaderStageFlags::ALL),
        vk::DescriptorSetLayoutBinding::default()
            .binding(BINDING_TEXTURES)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(TEXTURE_SLOTS)
            .stage_flags(vk::ShaderStageFlags::ALL),
    ]
}

fn binding_flags() -> [vk::DescriptorBindingFlags; 3] {
    [
        BINDING_FLAGS,
        BINDING_FLAGS,
        BINDING_FLAGS | vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT,
    ]
}

/// A texture's index in binding 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TextureHandle(u32);

impl TextureHandle {
    /// The magenta fallback; also what a zero handle reads.
    pub(crate) const MISSING: Self = Self(0);
    /// A white 1×1 texture.
    pub(crate) const WHITE: Self = Self(1);
    /// A flat tangent-space normal (0.5, 0.5, 1).
    pub(crate) const FLAT_NORMAL: Self = Self(2);
    /// Slots below this are the fixed fallbacks; handles start here.
    const RESERVED: u32 = 3;

    /// The index shaders use.
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// A storage image's index in binding 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct StorageImageHandle(u32);

impl StorageImageHandle {
    /// The 1×1 fallback storage image.
    #[cfg_attr(not(test), allow(dead_code))] // Named for shaders and tests; N1 passes use it.
    pub(crate) const FALLBACK: Self = Self(0);
    const RESERVED: u32 = 1;

    /// The index shaders use.
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// A sampler's index in binding 0: a [`SamplerKey`]'s palette slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SamplerHandle(u32);

impl SamplerHandle {
    /// The index shaders use.
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// Address mode of a palette sampler (all three axes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SamplerAddress {
    /// `REPEAT`.
    Wrap = 0,
    /// `CLAMP_TO_EDGE`.
    Clamp = 1,
}

/// Filtering of a palette sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SamplerFilter {
    /// Linear min/mag/mip.
    Linear = 0,
    /// Nearest min/mag/mip.
    Point = 1,
    /// Linear with the device's anisotropy (up to 16×; plain linear without the feature).
    Anisotropic = 2,
}

/// One entry of the fixed sampler palette (ADR-0020 §3a): wrap/clamp × point/linear/aniso ×
/// compare. Slot `compare * 6 + filter * 2 + address`, so slot 0 is linear/wrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SamplerKey {
    pub(crate) address: SamplerAddress,
    pub(crate) filter: SamplerFilter,
    /// A comparison sampler (`LESS_OR_EQUAL`, for shadow maps).
    pub(crate) compare: bool,
}

/// Palette entries in use; slots past this hold the slot-0 sampler.
pub(crate) const PALETTE_LEN: u32 = 12;
/// The non-comparison entries are slots `0..NON_COMPARE_SAMPLERS`.
pub(crate) const NON_COMPARE_SAMPLERS: u32 = 6;

impl SamplerKey {
    /// The palette slot.
    pub(crate) const fn handle(self) -> SamplerHandle {
        SamplerHandle(
            (self.compare as u32) * NON_COMPARE_SAMPLERS
                + (self.filter as u32) * 2
                + self.address as u32,
        )
    }

    /// Every palette entry, in slot order.
    pub(crate) fn palette() -> [Self; PALETTE_LEN as usize] {
        let mut keys = [Self {
            address: SamplerAddress::Wrap,
            filter: SamplerFilter::Linear,
            compare: false,
        }; PALETTE_LEN as usize];
        for compare in [false, true] {
            for filter in [
                SamplerFilter::Linear,
                SamplerFilter::Point,
                SamplerFilter::Anisotropic,
            ] {
                for address in [SamplerAddress::Wrap, SamplerAddress::Clamp] {
                    let key = Self {
                        address,
                        filter,
                        compare,
                    };
                    keys[key.handle().index() as usize] = key;
                }
            }
        }
        keys
    }

    fn create_info(self, max_anisotropy: Option<f32>) -> vk::SamplerCreateInfo<'static> {
        let address = match self.address {
            SamplerAddress::Wrap => vk::SamplerAddressMode::REPEAT,
            SamplerAddress::Clamp => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        };
        let (filter, mipmap) = match self.filter {
            SamplerFilter::Point => (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST),
            SamplerFilter::Linear | SamplerFilter::Anisotropic => {
                (vk::Filter::LINEAR, vk::SamplerMipmapMode::LINEAR)
            }
        };
        let anisotropy = max_anisotropy
            .filter(|_| self.filter == SamplerFilter::Anisotropic)
            .map(|max| max.min(16.0));
        vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .mipmap_mode(mipmap)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .anisotropy_enable(anisotropy.is_some())
            .max_anisotropy(anisotropy.unwrap_or(1.0))
            .compare_enable(self.compare)
            .compare_op(if self.compare {
                vk::CompareOp::LESS_OR_EQUAL
            } else {
                vk::CompareOp::ALWAYS
            })
            .min_lod(0.0)
            .max_lod(vk::LOD_CLAMP_NONE)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
    }
}

/// A free list of slots `reserved..capacity` with deferred recycling. Plain data, so its rules
/// are unit-tested without a GPU.
///
/// Every buffer is sized for all the slots at creation, so `allocate`, `release` and `retire`
/// never allocate, and each costs O(log n) per slot: a future pass may call them every frame.
#[derive(Clone, Debug)]
pub(crate) struct SlotAllocator {
    /// Free slots; the lowest pops first, so allocation is deterministic.
    free: BinaryHeap<Reverse<u32>>,
    /// Released slots and the frame they were released in, oldest first.
    pending: VecDeque<(u64, u32)>,
    /// Whether each slot is handed out (reserved slots are never live).
    live: Vec<bool>,
    live_count: usize,
    reserved: u32,
}

impl SlotAllocator {
    pub(crate) fn new(capacity: u32, reserved: u32) -> Self {
        assert!(reserved <= capacity);
        let slots = (capacity - reserved) as usize;
        // A slot is free, pending or live, never two at once, so neither buffer outgrows this.
        let mut free = BinaryHeap::with_capacity(slots);
        free.extend((reserved..capacity).map(Reverse));
        Self {
            free,
            pending: VecDeque::with_capacity(slots),
            live: vec![false; capacity as usize],
            live_count: 0,
            reserved,
        }
    }

    /// The lowest free slot, or `None` when every slot is live or waiting to retire.
    pub(crate) fn allocate(&mut self) -> Option<u32> {
        let Reverse(slot) = self.free.pop()?;
        self.live[slot as usize] = true;
        self.live_count += 1;
        Some(slot)
    }

    /// Releases a live slot during `frame`; it is reused only after [`Self::retire`] reports
    /// that frame retired. Returns `false` (and changes nothing) for a reserved, free or
    /// already released slot.
    pub(crate) fn release(&mut self, slot: u32, frame: u64) -> bool {
        if slot < self.reserved || !self.live.get(slot as usize).copied().unwrap_or(false) {
            return false;
        }
        self.live[slot as usize] = false;
        self.live_count -= 1;
        debug_assert!(
            self.pending.back().is_none_or(|&(last, _)| last <= frame),
            "frames are released in order"
        );
        self.pending.push_back((frame, slot));
        true
    }

    /// Every frame up to and including `completed` has retired: their released slots return
    /// to the free list and are appended to `recycled` (whose descriptors the caller resets).
    /// Does nothing when no release is pending, and never allocates beyond `recycled`.
    pub(crate) fn retire(&mut self, completed: u64, recycled: &mut Vec<u32>) {
        while let Some(&(frame, slot)) = self.pending.front() {
            if frame > completed {
                break;
            }
            self.pending.pop_front();
            recycled.push(slot);
            self.free.push(Reverse(slot));
        }
    }

    /// Slots handed out now.
    pub(crate) fn live(&self) -> usize {
        self.live_count
    }

    /// Released slots waiting for their frame to retire.
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// The bindless set, its fallbacks and its free lists (ADR-0020 §3a). Tiered devices only.
pub(crate) struct BindlessTable {
    device: ash::Device,
    set: vk::DescriptorSet,
    textures: SlotAllocator,
    storage_images: SlotAllocator,
    /// Descriptors written since creation (creation included).
    writes: u64,
    /// Comparison samplers may be written (`DeviceContext::comparison_samplers`).
    comparison_samplers: bool,
    recycled: Vec<u32>,
    // Destroyed in this order: the pool (and with it the set), the layout, the samplers, then
    // the fallback images the descriptors pointed at.
    _pool: DescriptorPool,
    layout: DescriptorSetLayout,
    samplers: Vec<Sampler>,
    fallback_textures: Vec<GpuImage>,
    fallback_storage: GpuImage,
}

impl BindlessTable {
    /// Creates the set, the sampler palette and the fallbacks, and writes the fallback into
    /// every slot. Load time only: it waits for the graphics queue once.
    pub(crate) fn new(device: &DeviceContext) -> RenderResult<Self> {
        // The guard for everything below: only a tiered device enables descriptor indexing
        // and device-address allocation.
        if device.admission.path != crate::caps::RenderPath::Tiered {
            return Err(RenderError::NoSuitableDevice(format!(
                "{}: the bindless table needs the tiered path",
                device.name
            )));
        }
        let bindings = set_layout_bindings();
        crate::descriptor_budget::check_tiered("bindless", &[&bindings], device.update_after_bind)?;
        // SAFETY: the device is tiered (checked above), so the descriptor-indexing features
        // the binding flags need are enabled.
        let layout = unsafe {
            DescriptorSetLayout::with_binding_flags(
                device.device.clone(),
                &bindings,
                vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL,
                &binding_flags(),
            )
        }?;
        let pool = DescriptorPool::with_flags(
            device.device.clone(),
            &[
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: SAMPLER_SLOTS,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: STORAGE_IMAGE_SLOTS,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: TEXTURE_SLOTS,
                },
            ],
            1,
            vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND,
        )?;
        let counts = [TEXTURE_SLOTS];
        let mut variable = vk::DescriptorSetVariableDescriptorCountAllocateInfo::default()
            .descriptor_counts(&counts);
        let layouts = [layout.handle()];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool.handle())
            .set_layouts(&layouts)
            .push(&mut variable);
        // SAFETY: the pool and layout are live and belong to this device; the variable count
        // equals the binding's declared maximum, and the pool holds exactly one such set.
        let set = unsafe { device.device.allocate_descriptor_sets(&allocate) }?
            .into_iter()
            .next()
            .ok_or(RenderError::Shader("no bindless descriptor set"))?;

        let samplers = SamplerKey::palette()
            .iter()
            .map(|key| {
                Sampler::new(
                    device.device.clone(),
                    &key.create_info(device.max_anisotropy),
                )
            })
            .collect::<RenderResult<Vec<_>>>()?;
        let fallback_textures = upload_solid(device, "bindless-fallback", &FALLBACK_TEXELS)?;
        let fallback_storage = storage_fallback(device)?;

        let mut table = Self {
            device: device.device.clone(),
            set,
            textures: SlotAllocator::new(TEXTURE_SLOTS, TextureHandle::RESERVED),
            storage_images: SlotAllocator::new(STORAGE_IMAGE_SLOTS, StorageImageHandle::RESERVED),
            writes: 0,
            comparison_samplers: device.comparison_samplers,
            // Never more than every slot of the larger binding retiring at once.
            recycled: Vec::with_capacity(TEXTURE_SLOTS as usize),
            _pool: pool,
            layout,
            samplers,
            fallback_textures,
            fallback_storage,
        };
        table.write_initial();
        Ok(table)
    }

    /// Writes the palette into binding 0 (slot 0's sampler past [`PALETTE_LEN`]) and the
    /// fallbacks into every slot of bindings 1 and 2.
    fn write_initial(&mut self) {
        let samplers: Vec<vk::DescriptorImageInfo> = (0..SAMPLER_SLOTS)
            .map(|slot| {
                // Past the palette, and for the comparison entries on a device without
                // mutable comparison samplers (a portability driver; tiered never admits one
                // today), the slot holds the slot-0 sampler.
                let usable = slot < NON_COMPARE_SAMPLERS || self.comparison_samplers;
                let sampler = self
                    .samplers
                    .get(slot as usize)
                    .filter(|_| usable)
                    .unwrap_or(&self.samplers[0])
                    .handle();
                vk::DescriptorImageInfo::default().sampler(sampler)
            })
            .collect();
        let storage = vec![
            vk::DescriptorImageInfo::default()
                .image_view(self.fallback_storage.view())
                .image_layout(vk::ImageLayout::GENERAL);
            STORAGE_IMAGE_SLOTS as usize
        ];
        let mut textures = vec![
            vk::DescriptorImageInfo::default()
                .image_view(self.fallback_textures[0].view())
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            TEXTURE_SLOTS as usize
        ];
        for (slot, image) in textures.iter_mut().zip(&self.fallback_textures) {
            *slot = slot.image_view(image.view());
        }
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(BINDING_SAMPLERS)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&samplers),
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(BINDING_STORAGE_IMAGES)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&storage),
            vk::WriteDescriptorSet::default()
                .dst_set(self.set)
                .dst_binding(BINDING_TEXTURES)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&textures),
        ];
        // SAFETY: the set is live and not yet used by any command buffer; every sampler and
        // view is live, and each array matches its binding's type and count.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        self.writes += u64::from(SAMPLER_SLOTS + STORAGE_IMAGE_SLOTS + TEXTURE_SLOTS);
    }

    /// The set layout, set 0 of every tiered pipeline layout.
    pub(crate) fn layout(&self) -> vk::DescriptorSetLayout {
        self.layout.handle()
    }

    /// The set, bound once per command buffer at index 0.
    pub(crate) fn set(&self) -> vk::DescriptorSet {
        self.set
    }

    /// Descriptors written since creation, creation included.
    pub(crate) fn writes(&self) -> u64 {
        self.writes
    }

    /// Live texture handles (the fallbacks not counted).
    pub(crate) fn live_textures(&self) -> usize {
        self.textures.live()
    }

    /// Released handles waiting for their frame to retire (textures, storage images).
    pub(crate) fn pending(&self) -> (usize, usize) {
        (self.textures.pending(), self.storage_images.pending())
    }

    /// Hands out a texture handle for `view` (in `SHADER_READ_ONLY_OPTIMAL` whenever a shader
    /// reads it). Load time only. The view must outlive the handle's release and retirement.
    pub(crate) fn register_texture(&mut self, view: vk::ImageView) -> RenderResult<TextureHandle> {
        let slot = self
            .textures
            .allocate()
            .ok_or(RenderError::BindlessFull("textures"))?;
        self.write_texture(slot, view);
        Ok(TextureHandle(slot))
    }

    /// Hands out a storage-image handle for `view` (in `GENERAL` whenever a shader accesses
    /// it). Load time only. The view must outlive the handle's release and retirement.
    pub(crate) fn register_storage_image(
        &mut self,
        view: vk::ImageView,
    ) -> RenderResult<StorageImageHandle> {
        let slot = self
            .storage_images
            .allocate()
            .ok_or(RenderError::BindlessFull("storage images"))?;
        self.write_storage(slot, view);
        Ok(StorageImageHandle(slot))
    }

    /// Releases `handle` during `frame`: the last submitted frame that may index it; no frame
    /// recorded later may. Returns `false` for a fallback or a handle that is not live.
    pub(crate) fn release_texture(&mut self, handle: TextureHandle, frame: u64) -> bool {
        self.textures.release(handle.0, frame)
    }

    /// Releases `handle` during `frame`, under the same contract as
    /// [`Self::release_texture`]. Returns `false` for the fallback or a handle that is not live.
    pub(crate) fn release_storage_image(&mut self, handle: StorageImageHandle, frame: u64) -> bool {
        self.storage_images.release(handle.0, frame)
    }

    /// Frames up to `completed` have retired: their released slots point at the fallback
    /// again and become free. Writes nothing when nothing was released.
    pub(crate) fn retire(&mut self, completed: u64) {
        let mut recycled = core::mem::take(&mut self.recycled);
        self.textures.retire(completed, &mut recycled);
        let magenta = self.fallback_textures[0].view();
        for &slot in &recycled {
            self.write_texture(slot, magenta);
        }
        recycled.clear();
        self.storage_images.retire(completed, &mut recycled);
        let storage = self.fallback_storage.view();
        for &slot in &recycled {
            self.write_storage(slot, storage);
        }
        recycled.clear();
        self.recycled = recycled;
    }

    fn write_texture(&mut self, slot: u32, view: vk::ImageView) {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(BINDING_TEXTURES)
            .dst_array_element(slot)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&info);
        // SAFETY: the slot is inside the binding and no pending command buffer uses it: it is
        // newly allocated, or its release retired and, by the release contract, no frame
        // recorded since indexes it. `UPDATE_UNUSED_WHILE_PENDING` and `UPDATE_AFTER_BIND`
        // allow that write even while the set is bound; the view is live.
        unsafe {
            self.device
                .update_descriptor_sets(core::slice::from_ref(&write), &[]);
        }
        self.writes += 1;
    }

    fn write_storage(&mut self, slot: u32, view: vk::ImageView) {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(BINDING_STORAGE_IMAGES)
            .dst_array_element(slot)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&info);
        // SAFETY: as in `write_texture`.
        unsafe {
            self.device
                .update_descriptor_sets(core::slice::from_ref(&write), &[]);
        }
        self.writes += 1;
    }
}

/// Uploads one 1×1 `R8G8B8A8_UNORM` texture per entry of `texels` with a single submission,
/// left in `SHADER_READ_ONLY_OPTIMAL` and visible to every shader stage (bindless textures
/// can be read anywhere). Load time only.
pub(crate) fn upload_solid(
    device: &DeviceContext,
    name: &str,
    texels: &[[u8; 4]],
) -> RenderResult<Vec<GpuImage>> {
    let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
    let mut staging = GpuBuffer::new(
        device.device.clone(),
        device.allocator(),
        "bindless-staging",
        bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
    )?;
    staging.write_bytes(0, &bytes);
    let images = texels
        .iter()
        .map(|_| {
            // Counted like every other texture upload (R8).
            crate::memory::count_upload();
            GpuImage::new(
                device.device.clone(),
                device.allocator(),
                name,
                vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                FALLBACK_FORMAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageAspectFlags::COLOR,
            )
        })
        .collect::<RenderResult<Vec<_>>>()?;
    submit_one_shot(device, |command| {
        let to_transfer: Vec<_> = images
            .iter()
            .map(|image| {
                color_barrier(
                    image.image(),
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                )
            })
            .collect();
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_transfer),
        );
        for (index, image) in images.iter().enumerate() {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(index as u64 * 4)
                .image_subresource(color_layers())
                .image_extent(vk::Extent3D {
                    width: 1,
                    height: 1,
                    depth: 1,
                });
            // SAFETY: the staging buffer holds four bytes per image at this offset, and the
            // image was created for transfer and is in `TRANSFER_DST_OPTIMAL`.
            unsafe {
                device.device.cmd_copy_buffer_to_image(
                    command,
                    staging.handle(),
                    image.image(),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    core::slice::from_ref(&region),
                );
            }
        }
        let to_shader: Vec<_> = images
            .iter()
            .map(|image| {
                color_barrier(
                    image.image(),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags2::ALL_COMMANDS,
                        vk::AccessFlags2::SHADER_SAMPLED_READ,
                    ),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                )
            })
            .collect();
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(&to_shader),
        );
    })?;
    drop(staging);
    Ok(images)
}

/// The 1×1 storage fallback, in `GENERAL`.
fn storage_fallback(device: &DeviceContext) -> RenderResult<GpuImage> {
    let image = GpuImage::new(
        device.device.clone(),
        device.allocator(),
        "bindless-fallback-storage",
        vk::Extent2D {
            width: 1,
            height: 1,
        },
        FALLBACK_FORMAT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageAspectFlags::COLOR,
    )?;
    submit_one_shot(device, |command| {
        let to_clear = color_barrier(
            image.image(),
            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
            (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::GENERAL,
        );
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_clear)),
        );
        // Cleared, so a read through a zero or stale storage handle sees defined data: zeros
        // until a shader writes through such a handle (the fallback is a shared sink).
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        // SAFETY: the image has `TRANSFER_DST` usage and is in `GENERAL`, which clears allow;
        // the range is its only level and layer.
        unsafe {
            device.device.cmd_clear_color_image(
                command,
                image.image(),
                vk::ImageLayout::GENERAL,
                &vk::ClearColorValue { float32: [0.0; 4] },
                core::slice::from_ref(&range),
            );
        }
        let to_shader = color_barrier(
            image.image(),
            (
                vk::PipelineStageFlags2::CLEAR,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            (
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::SHADER_STORAGE_WRITE,
            ),
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::GENERAL,
        );
        device.cmd_pipeline_barrier2(
            command,
            &vk::DependencyInfo::default().image_memory_barriers(core::slice::from_ref(&to_shader)),
        );
    })?;
    Ok(image)
}

/// A whole-image colour barrier from `(stage, access)` to `(stage, access)`.
pub(crate) fn color_barrier(
    image: vk::Image,
    src: (vk::PipelineStageFlags2, vk::AccessFlags2),
    dst: (vk::PipelineStageFlags2, vk::AccessFlags2),
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(src.0)
        .src_access_mask(src.1)
        .dst_stage_mask(dst.0)
        .dst_access_mask(dst.1)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
}

/// Mip 0, layer 0 of a colour image.
pub(crate) fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Records `record` into a one-time command buffer, submits it on the graphics queue and
/// waits for its fence. Load time (and the smoke tests) only. If the wait fails the device is
/// waited idle before returning, so the caller may free what the commands used.
pub(crate) fn submit_one_shot(
    device: &DeviceContext,
    record: impl FnOnce(vk::CommandBuffer),
) -> RenderResult<()> {
    let pool = CommandPool::new(device.device.clone(), device.graphics_family)?;
    let command = pool.allocate_one()?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer was just allocated and is not recording.
    unsafe { device.device.begin_command_buffer(command, &begin) }?;
    record(command);
    // SAFETY: the command buffer is recording and is ended once.
    unsafe { device.device.end_command_buffer(command) }?;
    let fence = Fence::new(device.device.clone(), false)?;
    let commands = [command];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    // SAFETY: the command buffer is executable on the graphics queue, the fence is unsignalled
    // and unused, and submissions happen only on the renderer's thread.
    unsafe {
        device
            .device
            .queue_submit(device.graphics_queue, &[submit], fence.handle())
    }?;
    let fences = [fence.handle()];
    // SAFETY: the fence belongs to this device and was submitted just above.
    let waited = unsafe { device.device.wait_for_fences(&fences, true, u64::MAX) };
    if let Err(error) = waited {
        // SAFETY: the device is live; waiting idle has no other preconditions. Its result is
        // the device's fate either way: idle, or lost (where destroying objects is allowed).
        let _ = unsafe { device.device.device_wait_idle() };
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NON_COMPARE_SAMPLERS, PALETTE_LEN, SamplerAddress, SamplerFilter, SamplerKey,
        SlotAllocator, StorageImageHandle, TEXTURE_SLOTS, TextureHandle, binding_flags,
        set_layout_bindings,
    };
    use ash::vk;

    #[test]
    fn the_layout_matches_adr_0020_section_3a() {
        let bindings = set_layout_bindings();
        let summary: Vec<_> = bindings
            .iter()
            .map(|binding| {
                (
                    binding.binding,
                    binding.descriptor_type,
                    binding.descriptor_count,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (0, vk::DescriptorType::SAMPLER, 32),
                (1, vk::DescriptorType::STORAGE_IMAGE, 1_024),
                (2, vk::DescriptorType::SAMPLED_IMAGE, 16_384),
            ]
        );
        let flags = binding_flags();
        let common = vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
            | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING;
        assert!(flags.iter().all(|flag| flag.contains(common)));
        // VUID-VkDescriptorSetLayoutBindingFlagsCreateInfo-pBindingFlags-03004: only the
        // highest-numbered binding may have a variable count.
        let variable = vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT;
        assert!(!flags[0].contains(variable) && !flags[1].contains(variable));
        assert!(flags[2].contains(variable));
        assert_eq!(bindings[2].descriptor_count, TEXTURE_SLOTS);
    }

    #[test]
    fn the_sampler_palette_fills_twelve_distinct_slots_with_linear_wrap_first() {
        let palette = SamplerKey::palette();
        assert_eq!(palette.len(), PALETTE_LEN as usize);
        for (slot, key) in palette.iter().enumerate() {
            assert_eq!(key.handle().index() as usize, slot);
        }
        assert_eq!(
            palette[0],
            SamplerKey {
                address: SamplerAddress::Wrap,
                filter: SamplerFilter::Linear,
                compare: false,
            }
        );
        assert!(
            palette[..NON_COMPARE_SAMPLERS as usize]
                .iter()
                .all(|key| !key.compare)
        );
        assert!(
            palette[NON_COMPARE_SAMPLERS as usize..]
                .iter()
                .all(|key| key.compare)
        );
        let unique: std::collections::BTreeSet<_> = palette
            .iter()
            .map(|key| (key.address as u8, key.filter as u8, key.compare))
            .collect();
        assert_eq!(unique.len(), palette.len());
    }

    #[test]
    fn fallback_handles_are_the_reserved_slots() {
        assert_eq!(TextureHandle::MISSING.index(), 0);
        assert_eq!(TextureHandle::WHITE.index(), 1);
        assert_eq!(TextureHandle::FLAT_NORMAL.index(), 2);
        assert_eq!(StorageImageHandle::FALLBACK.index(), 0);
        let mut textures = SlotAllocator::new(TEXTURE_SLOTS, TextureHandle::RESERVED);
        assert_eq!(
            textures.allocate(),
            Some(3),
            "handles start after the fallbacks"
        );
        assert!(!textures.release(0, 0), "a fallback is never released");
        assert!(!textures.release(2, 0));
    }

    #[test]
    fn released_slots_recycle_only_after_their_frame_retires() {
        let mut slots = SlotAllocator::new(8, 1);
        let first: Vec<_> = (0..7).map(|_| slots.allocate().expect("free")).collect();
        assert_eq!(first, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(slots.allocate(), None, "full");
        assert!(slots.release(4, 10));
        assert!(!slots.release(4, 10), "double release is refused");
        assert!(slots.release(2, 11));
        assert_eq!(slots.pending(), 2);
        // Frame 10 still in flight: nothing is reused.
        let mut recycled = Vec::new();
        slots.retire(9, &mut recycled);
        assert_eq!(recycled, Vec::<u32>::new());
        assert_eq!(slots.allocate(), None);
        // Frame 10 retired: slot 4 only.
        slots.retire(10, &mut recycled);
        assert_eq!(recycled, [4]);
        assert_eq!(slots.allocate(), Some(4));
        recycled.clear();
        slots.retire(11, &mut recycled);
        assert_eq!(recycled, [2]);
        assert_eq!(slots.pending(), 0);
        assert_eq!(slots.allocate(), Some(2));
        assert_eq!(slots.live(), 7);
        assert!(!slots.release(99, 12), "out of range");
    }

    #[test]
    fn retiring_with_nothing_pending_does_not_allocate() {
        let mut slots = SlotAllocator::new(4, 1);
        let mut recycled = Vec::new();
        slots.retire(u64::MAX, &mut recycled);
        assert_eq!(recycled.capacity(), 0);
    }

    #[test]
    fn churn_never_grows_the_free_lists() {
        let mut slots = SlotAllocator::new(64, 3);
        let capacities = (slots.free.capacity(), slots.pending.capacity());
        let mut recycled = Vec::with_capacity(64);
        for frame in 0..10 {
            while slots.allocate().is_some() {}
            for slot in 3..64 {
                assert!(slots.release(slot, frame));
            }
            assert_eq!(slots.live(), 0);
            slots.retire(frame, &mut recycled);
            assert_eq!(recycled.len(), 61);
            recycled.clear();
        }
        assert_eq!(
            (slots.free.capacity(), slots.pending.capacity()),
            capacities
        );
        assert_eq!(recycled.capacity(), 64);
    }

    #[test]
    fn freed_slots_are_reused_lowest_first() {
        let mut slots = SlotAllocator::new(6, 0);
        for _ in 0..6 {
            slots.allocate();
        }
        for slot in [5, 1, 3] {
            assert!(slots.release(slot, 1));
        }
        slots.retire(1, &mut Vec::new());
        assert_eq!(
            [slots.allocate(), slots.allocate(), slots.allocate()],
            [Some(1), Some(3), Some(5)]
        );
    }
}
