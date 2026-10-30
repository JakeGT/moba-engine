//! RAII wrappers for Vulkan objects and GPU allocations.
//!
//! Every wrapper destroys exactly its own handle. GPU buffers/images own their allocator
//! handle, so freeing memory cannot be forgotten on an error path.

use core::ptr::NonNull;

use ash::vk;
use bytemuck::Pod;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{RenderError, RenderResult};

pub(crate) type AllocatorHandle = Arc<Mutex<Allocator>>;

/// A GPU buffer plus its memory allocation.
pub(crate) struct GpuBuffer {
    device: ash::Device,
    allocator: AllocatorHandle,
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    size: u64,
    mapped: Option<NonNull<u8>>,
}

impl GpuBuffer {
    pub(crate) fn new(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
    ) -> RenderResult<Self> {
        assert!(size > 0, "Vulkan buffers must be non-empty");
        let create_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live and `create_info` is fully initialised.
        let buffer = unsafe { device.create_buffer(&create_info, None) }?;
        // SAFETY: `buffer` was created by this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };

        let allocation = {
            let mut allocator = allocator.lock().unwrap_or_else(PoisonError::into_inner);
            match allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            }) {
                Ok(allocation) => allocation,
                Err(error) => {
                    // SAFETY: the buffer is unbound and destroyed exactly once.
                    unsafe { device.destroy_buffer(buffer, None) };
                    return Err(error.into());
                }
            }
        };
        let mapped = allocation.mapped_ptr().map(NonNull::cast::<u8>);
        if mapped.is_some()
            && !allocation
                .memory_properties()
                .contains(vk::MemoryPropertyFlags::HOST_COHERENT)
        {
            // The engine writes uniforms/instances directly from the CPU. Rather than
            // carrying flush bookkeeping, require coherent memory for mapped buffers.
            // gpu-allocator selects coherent CpuToGpu memory on all desktop drivers.
            let _ = allocator_free(&allocator, allocation);
            // SAFETY: `buffer` has not been bound or used and is destroyed exactly once.
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(RenderError::NoSuitableDevice(String::from(
                "host-visible buffer memory is not coherent",
            )));
        }

        // SAFETY: the allocation is live and owns a valid `VkDeviceMemory`.
        let memory = unsafe { allocation.memory() };
        let memory_offset = allocation.offset();
        // SAFETY: the allocation satisfies this buffer's memory requirements.
        if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, memory_offset) } {
            drop_allocator_free(&allocator, allocation);
            // SAFETY: the buffer is unbound and destroyed once.
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(error.into());
        }

        Ok(Self {
            device,
            allocator,
            buffer,
            allocation: Some(allocation),
            size,
            mapped,
        })
    }

    #[inline]
    pub(crate) fn handle(&self) -> vk::Buffer {
        self.buffer
    }

    #[inline]
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Copies bytes into a mapped buffer.
    ///
    /// # Panics
    /// Panics if the buffer is not host visible or the range is out of bounds.
    pub(crate) fn write_bytes(&mut self, offset: usize, bytes: &[u8]) {
        let mapped = self.mapped.expect("buffer was not created host-visible");
        assert!(
            offset + bytes.len() <= self.size as usize,
            "write out of bounds"
        );
        // SAFETY: the range is bounds-checked and the caller has exclusive access.
        let destination = unsafe { mapped.as_ptr().add(offset) };
        // SAFETY: `mapped` points at the allocation and `destination` is inside it.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        }
    }

    /// Returns the mapped memory as a typed slice.
    pub(crate) fn mapped_slice_mut<T: Pod>(&mut self, count: usize) -> Option<&mut [T]> {
        let mapped = self.mapped?;
        let bytes = core::mem::size_of::<T>()
            .checked_mul(count)
            .filter(|bytes| *bytes <= self.size as usize)?;
        if bytes == 0 || !(mapped.as_ptr() as usize).is_multiple_of(core::mem::align_of::<T>()) {
            return None;
        }
        // SAFETY: the requested byte range is inside the mapping, the mapped pointer is
        // aligned for `T`, and `&mut self` guarantees exclusive access.
        Some(unsafe { core::slice::from_raw_parts_mut(mapped.as_ptr().cast::<T>(), count) })
    }
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        // SAFETY: the buffer belongs to this device and is destroyed once.
        unsafe { self.device.destroy_buffer(self.buffer, None) };
        if let Some(allocation) = self.allocation.take() {
            drop_allocator_free(&self.allocator, allocation);
        }
    }
}

fn allocator_free(allocator: &AllocatorHandle, allocation: Allocation) -> Option<()> {
    allocator
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .free(allocation)
        .ok()
}

fn drop_allocator_free(allocator: &AllocatorHandle, allocation: Allocation) {
    let _ = allocator_free(allocator, allocation);
}

/// A GPU image plus an image view.
pub(crate) struct GpuImage {
    device: ash::Device,
    allocator: AllocatorHandle,
    image: vk::Image,
    view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl GpuImage {
    pub(crate) fn new(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        extent: vk::Extent2D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> RenderResult<Self> {
        Self::with_mips(device, allocator, name, extent, format, usage, aspect, 1)
    }

    /// An image with `mip_levels` levels (at least 1); the view covers every level.
    #[allow(clippy::too_many_arguments)] // The create info, flattened.
    pub(crate) fn with_mips(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        extent: vk::Extent2D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
        mip_levels: u32,
    ) -> RenderResult<Self> {
        Self::create(
            device,
            allocator,
            name,
            extent,
            format,
            usage,
            aspect,
            mip_levels,
            Layers::Single,
        )
    }

    /// A 2D array of `layers` colour layers with `mip_levels` levels; the view is a
    /// `2D_ARRAY` view of every layer and level (R3a terrain layers and splat maps).
    #[allow(clippy::too_many_arguments)] // The create info, flattened.
    pub(crate) fn array_with_mips(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        extent: vk::Extent2D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        layers: u32,
        mip_levels: u32,
    ) -> RenderResult<Self> {
        Self::create(
            device,
            allocator,
            name,
            extent,
            format,
            usage,
            vk::ImageAspectFlags::COLOR,
            mip_levels,
            Layers::Array(layers.max(1)),
        )
    }

    /// A cube map (six square layers, `CUBE_COMPATIBLE`) with `mip_levels` levels; the view
    /// is a `CUBE` view of every level.
    #[allow(clippy::too_many_arguments)] // The create info, flattened.
    pub(crate) fn cube_with_mips(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        size: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        mip_levels: u32,
    ) -> RenderResult<Self> {
        Self::create(
            device,
            allocator,
            name,
            vk::Extent2D {
                width: size,
                height: size,
            },
            format,
            usage,
            vk::ImageAspectFlags::COLOR,
            mip_levels,
            Layers::Cube,
        )
    }

    #[allow(clippy::too_many_arguments)] // The create info, flattened.
    fn create(
        device: ash::Device,
        allocator: AllocatorHandle,
        name: &str,
        extent: vk::Extent2D,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
        mip_levels: u32,
        kind: Layers,
    ) -> RenderResult<Self> {
        assert!(extent.width > 0 && extent.height > 0);
        assert!(mip_levels > 0);
        let (layers, flags) = match kind {
            Layers::Single => (1, vk::ImageCreateFlags::empty()),
            Layers::Cube => (6, vk::ImageCreateFlags::CUBE_COMPATIBLE),
            Layers::Array(layers) => (layers, vk::ImageCreateFlags::empty()),
        };
        let create_info = vk::ImageCreateInfo::default()
            .flags(flags)
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })
            .mip_levels(mip_levels)
            .array_layers(layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: the device is live and the create info is fully initialised.
        let image = unsafe { device.create_image(&create_info, None) }?;
        // SAFETY: `image` was created by this device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };

        let allocation = {
            let mut allocator = allocator.lock().unwrap_or_else(PoisonError::into_inner);
            match allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            }) {
                Ok(allocation) => allocation,
                Err(error) => {
                    // SAFETY: the image has no views and is destroyed exactly once.
                    unsafe { device.destroy_image(image, None) };
                    return Err(error.into());
                }
            }
        };
        // SAFETY: the allocation is live and owns a valid `VkDeviceMemory`.
        let memory = unsafe { allocation.memory() };
        let memory_offset = allocation.offset();
        // SAFETY: the allocation satisfies the image's memory requirements.
        let bind_result = unsafe { device.bind_image_memory(image, memory, memory_offset) };
        if let Err(error) = bind_result {
            drop_allocator_free(&allocator, allocation);
            // SAFETY: the image is not bound and is destroyed once.
            unsafe { device.destroy_image(image, None) };
            return Err(error.into());
        }

        let view = match create_view(&device, image, format, aspect, mip_levels, kind) {
            Ok(view) => view,
            Err(error) => {
                drop_allocator_free(&allocator, allocation);
                // SAFETY: the image has no views and is destroyed once.
                unsafe { device.destroy_image(image, None) };
                return Err(error);
            }
        };

        Ok(Self {
            device,
            allocator,
            image,
            view,
            allocation: Some(allocation),
        })
    }

    #[inline]
    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    #[inline]
    pub(crate) fn view(&self) -> vk::ImageView {
        self.view
    }
}

impl Drop for GpuImage {
    fn drop(&mut self) {
        // SAFETY: the view and image belong to this device and are destroyed once.
        unsafe { self.device.destroy_image_view(self.view, None) };
        // SAFETY: same as above.
        unsafe { self.device.destroy_image(self.image, None) };
        if let Some(allocation) = self.allocation.take() {
            drop_allocator_free(&self.allocator, allocation);
        }
    }
}

pub(crate) fn create_image_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
) -> RenderResult<vk::ImageView> {
    create_image_view_levels(device, image, format, aspect, 1)
}

/// A 2D view of mip levels `0..levels`.
pub(crate) fn create_image_view_levels(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
    levels: u32,
) -> RenderResult<vk::ImageView> {
    create_view(device, image, format, aspect, levels, Layers::Single)
}

/// The layer structure of a [`GpuImage`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layers {
    /// One layer, a `2D` view.
    Single,
    /// Six layers, `CUBE_COMPATIBLE`, a `CUBE` view.
    Cube,
    /// `n` layers, a `2D_ARRAY` view.
    Array(u32),
}

/// A view of mip levels `0..levels` and every layer: `2D`, `CUBE` or `2D_ARRAY` by `kind`.
fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
    levels: u32,
    kind: Layers,
) -> RenderResult<vk::ImageView> {
    let (view_type, layers) = match kind {
        Layers::Single => (vk::ImageViewType::TYPE_2D, 1),
        Layers::Cube => (vk::ImageViewType::CUBE, 6),
        Layers::Array(layers) => (vk::ImageViewType::TYPE_2D_ARRAY, layers),
    };
    let create_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(view_type)
        .format(format)
        .components(vk::ComponentMapping::default())
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(levels)
                .base_array_layer(0)
                .layer_count(layers),
        );
    // SAFETY: the image is live and `create_info` is fully initialised.
    unsafe { device.create_image_view(&create_info, None) }.map_err(RenderError::from)
}

macro_rules! raii_handle {
    ($name:ident, $handle:ty, $destroy:ident) => {
        pub(crate) struct $name {
            device: ash::Device,
            handle: $handle,
        }

        impl $name {
            /// Uniform handle accessor; some wrappers only use their handle internally.
            #[allow(dead_code)]
            #[inline]
            pub(crate) fn handle(&self) -> $handle {
                self.handle
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: the handle belongs to this device and is destroyed once.
                unsafe { self.device.$destroy(self.handle, None) };
            }
        }
    };
}

raii_handle!(Semaphore, vk::Semaphore, destroy_semaphore);
raii_handle!(Fence, vk::Fence, destroy_fence);
raii_handle!(ShaderModule, vk::ShaderModule, destroy_shader_module);
raii_handle!(PipelineLayout, vk::PipelineLayout, destroy_pipeline_layout);
raii_handle!(GraphicsPipeline, vk::Pipeline, destroy_pipeline);
raii_handle!(
    DescriptorSetLayout,
    vk::DescriptorSetLayout,
    destroy_descriptor_set_layout
);
raii_handle!(DescriptorPool, vk::DescriptorPool, destroy_descriptor_pool);
raii_handle!(Sampler, vk::Sampler, destroy_sampler);

impl Sampler {
    pub(crate) fn new(device: ash::Device, info: &vk::SamplerCreateInfo<'_>) -> RenderResult<Self> {
        // SAFETY: the device is live and the create info is fully initialised.
        let handle = unsafe { device.create_sampler(info, None) }?;
        Ok(Self { device, handle })
    }
}

impl DescriptorSetLayout {
    pub(crate) fn new(
        device: ash::Device,
        bindings: &[vk::DescriptorSetLayoutBinding<'_>],
    ) -> RenderResult<Self> {
        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings);
        // SAFETY: the device is live and the bindings array is valid for the call.
        let handle = unsafe { device.create_descriptor_set_layout(&create_info, None) }?;
        Ok(Self { device, handle })
    }
}

impl DescriptorPool {
    pub(crate) fn new(
        device: ash::Device,
        pool_sizes: &[vk::DescriptorPoolSize],
        max_sets: u32,
    ) -> RenderResult<Self> {
        let create_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(pool_sizes)
            .max_sets(max_sets);
        // SAFETY: the device is live and the pool-size array is valid for the call.
        let handle = unsafe { device.create_descriptor_pool(&create_info, None) }?;
        Ok(Self { device, handle })
    }

    pub(crate) fn allocate(
        &self,
        device: &ash::Device,
        layouts: &[vk::DescriptorSetLayout],
    ) -> RenderResult<Vec<vk::DescriptorSet>> {
        let create_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.handle)
            .set_layouts(layouts);
        // SAFETY: the pool is live and the layouts array is valid for the call.
        unsafe { device.allocate_descriptor_sets(&create_info) }.map_err(RenderError::from)
    }
}

impl PipelineLayout {
    pub(crate) fn with_push_constants(
        device: ash::Device,
        set_layouts: &[vk::DescriptorSetLayout],
        push_constants: &[vk::PushConstantRange],
    ) -> RenderResult<Self> {
        let create_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(set_layouts)
            .push_constant_ranges(push_constants);
        // SAFETY: the device is live and the layout array is valid for the call.
        let handle = unsafe { device.create_pipeline_layout(&create_info, None) }?;
        Ok(Self { device, handle })
    }
}

impl GraphicsPipeline {
    pub(crate) fn from_raw(device: ash::Device, handle: vk::Pipeline) -> Self {
        Self { device, handle }
    }
}

impl Semaphore {
    pub(crate) fn new(device: ash::Device) -> RenderResult<Self> {
        let create_info = vk::SemaphoreCreateInfo::default();
        // SAFETY: the device is live and the create info has no external pointers.
        let handle = unsafe { device.create_semaphore(&create_info, None) }?;
        Ok(Self { device, handle })
    }
}

impl Fence {
    pub(crate) fn new(device: ash::Device, signaled: bool) -> RenderResult<Self> {
        let flags = if signaled {
            vk::FenceCreateFlags::SIGNALED
        } else {
            vk::FenceCreateFlags::empty()
        };
        let create_info = vk::FenceCreateInfo::default().flags(flags);
        // SAFETY: the device is live and the create info has no external pointers.
        let handle = unsafe { device.create_fence(&create_info, None) }?;
        Ok(Self { device, handle })
    }
}

impl ShaderModule {
    pub(crate) fn new(device: ash::Device, spirv: &[u8]) -> RenderResult<Self> {
        assert_eq!(spirv.len() % 4, 0, "SPIR-V must be word aligned");
        let mut cursor = std::io::Cursor::new(spirv);
        let code = ash::util::read_spv(&mut cursor)?;
        let create_info = vk::ShaderModuleCreateInfo::default().code(&code);
        // SAFETY: the device is live and `code` is a valid SPIR-V word buffer.
        let handle = unsafe { device.create_shader_module(&create_info, None) }?;
        Ok(Self { device, handle })
    }
}

pub(crate) struct CommandPool {
    device: ash::Device,
    pool: vk::CommandPool,
}

impl CommandPool {
    pub(crate) fn new(device: ash::Device, family: u32) -> RenderResult<Self> {
        let create_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the device is live and the create info has no external pointers.
        let pool = unsafe { device.create_command_pool(&create_info, None) }?;
        Ok(Self { device, pool })
    }

    #[inline]
    pub(crate) fn handle(&self) -> vk::CommandPool {
        self.pool
    }

    pub(crate) fn allocate_one(&self) -> RenderResult<vk::CommandBuffer> {
        let create_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool belongs to this device and is live.
        let buffers = unsafe { self.device.allocate_command_buffers(&create_info) }?;
        buffers
            .into_iter()
            .next()
            .ok_or(RenderError::Shader("failed to allocate command buffer"))
    }
}

impl Drop for CommandPool {
    fn drop(&mut self) {
        // SAFETY: the pool belongs to this device, allocates no external children and is
        // destroyed once.
        unsafe { self.device.destroy_command_pool(self.pool, None) };
    }
}
