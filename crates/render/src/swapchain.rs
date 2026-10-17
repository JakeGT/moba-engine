//! Swapchain creation, surface format selection and per-image depth buffers.

use ash::{Device, Instance, khr, vk};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::{GpuImage, Semaphore, create_image_view};

/// Surface and swapchain state, recreated on resize.
pub(crate) struct Swapchain {
    device: Device,
    loader: khr::swapchain::Device,
    handle: vk::SwapchainKHR,
    pub(crate) format: vk::Format,
    pub(crate) extent: vk::Extent2D,
    pub(crate) images: Vec<vk::Image>,
    pub(crate) views: Vec<vk::ImageView>,
    pub(crate) depth_images: Vec<GpuImage>,
    pub(crate) depth_format: vk::Format,
    pub(crate) depth_aspect: vk::ImageAspectFlags,
    pub(crate) color_layouts: Vec<vk::ImageLayout>,
    pub(crate) depth_layouts: Vec<vk::ImageLayout>,
    pub(crate) render_finished: Vec<Semaphore>,
}

impl Swapchain {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // Swapchain setup is one ordered creation sequence.
    pub(crate) fn new(
        instance_context: &crate::instance::InstanceContext,
        device_context: &DeviceContext,
        surface: vk::SurfaceKHR,
        desired_extent: vk::Extent2D,
        vsync: bool,
        old: Option<vk::SwapchainKHR>,
    ) -> RenderResult<Self> {
        // SAFETY: surface/physical device belong to this instance.
        let capabilities = unsafe {
            device_context
                .surface
                .get_physical_device_surface_capabilities(device_context.physical, surface)
        }?;
        // SAFETY: same ownership as above.
        let formats = unsafe {
            device_context
                .surface
                .get_physical_device_surface_formats(device_context.physical, surface)
        }?;
        if formats.is_empty() {
            return Err(RenderError::NoSuitableDevice(String::from(
                "surface reports no formats",
            )));
        }
        // SAFETY: same ownership as above.
        let present_modes = unsafe {
            device_context
                .surface
                .get_physical_device_surface_present_modes(device_context.physical, surface)
        }?;

        let (format, color_space) = choose_format(&formats);
        let present_mode = choose_present_mode(&present_modes, vsync);
        let extent = choose_extent(capabilities, desired_extent);
        if extent.width == 0 || extent.height == 0 {
            return Err(RenderError::SwapchainOutOfDate);
        }
        let image_count = if capabilities.max_image_count == 0 {
            capabilities.min_image_count + 1
        } else {
            (capabilities.min_image_count + 1).min(capabilities.max_image_count)
        };
        let mut usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_SRC)
        {
            usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }
        let composite_alpha = choose_composite_alpha(capabilities.supported_composite_alpha);

        let families = [
            device_context.graphics_family,
            device_context.present_family,
        ];
        let concurrent = device_context.graphics_family != device_context.present_family;
        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format)
            .image_color_space(color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(usage)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(composite_alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old.unwrap_or(vk::SwapchainKHR::null()));
        if concurrent {
            create_info = create_info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&families);
        } else {
            create_info = create_info.image_sharing_mode(vk::SharingMode::EXCLUSIVE);
        }

        let loader =
            khr::swapchain::Device::new(&instance_context.instance, &device_context.device);
        // SAFETY: the surface is live, the device supports the swapchain extension, and all
        // create-info pointers are valid for the call.
        let handle = unsafe { loader.create_swapchain(&create_info, None) }?;
        // SAFETY: the swapchain was just created and belongs to this device.
        let images = unsafe { loader.get_swapchain_images(handle) }?;

        let mut views = Vec::with_capacity(images.len());
        let mut depth_images = Vec::with_capacity(images.len());
        let mut color_layouts = Vec::with_capacity(images.len());
        let mut depth_layouts = Vec::with_capacity(images.len());
        let mut render_finished = Vec::with_capacity(images.len());
        let (depth_format, depth_aspect) =
            choose_depth_format(&instance_context.instance, device_context);
        for (index, image) in images.iter().enumerate() {
            views.push(create_image_view(
                &device_context.device,
                *image,
                format,
                vk::ImageAspectFlags::COLOR,
            )?);
            depth_images.push(GpuImage::new(
                device_context.device.clone(),
                device_context.allocator(),
                &format!("depth-{index}"),
                extent,
                depth_format,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                depth_aspect,
            )?);
            color_layouts.push(vk::ImageLayout::UNDEFINED);
            depth_layouts.push(vk::ImageLayout::UNDEFINED);
            render_finished.push(Semaphore::new(device_context.device.clone())?);
        }

        Ok(Self {
            device: device_context.device.clone(),
            loader,
            handle,
            format,
            extent,
            images,
            views,
            depth_images,
            depth_format,
            depth_aspect,
            color_layouts,
            depth_layouts,
            render_finished,
        })
    }

    #[inline]
    pub(crate) fn handle(&self) -> vk::SwapchainKHR {
        self.handle
    }

    #[inline]
    pub(crate) fn image_count(&self) -> usize {
        self.images.len()
    }

    pub(crate) fn acquire(&self, semaphore: vk::Semaphore) -> RenderResult<(u32, bool)> {
        // SAFETY: the swapchain is live and the semaphore belongs to this device.
        let result = unsafe {
            self.loader
                .acquire_next_image(self.handle, u64::MAX, semaphore, vk::Fence::null())
        };
        match result {
            Ok((index, suboptimal)) => Ok((index, suboptimal)),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Err(RenderError::SwapchainOutOfDate),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn present(
        &self,
        queue: vk::Queue,
        image_index: u32,
        wait_semaphore: vk::Semaphore,
    ) -> RenderResult<bool> {
        let swapchains = [self.handle];
        let wait_semaphores = [wait_semaphore];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        // SAFETY: the queue belongs to this device, the swapchain is live, and the wait
        // semaphore is signalled by the frame's submit.
        let result = unsafe { self.loader.queue_present(queue, &present_info) };
        match result {
            Ok(false) => Ok(true),
            Ok(true) => Err(RenderError::SwapchainSuboptimal),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Err(RenderError::SwapchainOutOfDate),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // SAFETY: the device is idle before the swapchain is replaced or the renderer is
        // dropped. Views belong to the swapchain images and are destroyed first.
        unsafe {
            for view in self.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
        }
        // SAFETY: the swapchain belongs to this device and is destroyed once.
        unsafe { self.loader.destroy_swapchain(self.handle, None) };
    }
}

fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> (vk::Format, vk::ColorSpaceKHR) {
    let preferred = vk::SurfaceFormatKHR {
        format: vk::Format::B8G8R8A8_SRGB,
        color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
    };
    if formats.contains(&preferred) {
        return (preferred.format, preferred.color_space);
    }
    if formats.len() == 1 && formats[0].format == vk::Format::UNDEFINED {
        return (vk::Format::B8G8R8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR);
    }
    formats.first().map_or(
        (vk::Format::B8G8R8A8_SRGB, vk::ColorSpaceKHR::SRGB_NONLINEAR),
        |format| (format.format, format.color_space),
    )
}

fn choose_present_mode(modes: &[vk::PresentModeKHR], vsync: bool) -> vk::PresentModeKHR {
    if vsync {
        return vk::PresentModeKHR::FIFO;
    }
    for preferred in [
        vk::PresentModeKHR::MAILBOX,
        vk::PresentModeKHR::IMMEDIATE,
        vk::PresentModeKHR::FIFO,
    ] {
        if modes.contains(&preferred) {
            return preferred;
        }
    }
    vk::PresentModeKHR::FIFO
}

fn choose_extent(capabilities: vk::SurfaceCapabilitiesKHR, desired: vk::Extent2D) -> vk::Extent2D {
    if capabilities.current_extent.width != u32::MAX {
        return capabilities.current_extent;
    }
    vk::Extent2D {
        width: desired.width.clamp(
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        height: desired.height.clamp(
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    }
}

fn choose_composite_alpha(flags: vk::CompositeAlphaFlagsKHR) -> vk::CompositeAlphaFlagsKHR {
    for preferred in [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ] {
        if flags.contains(preferred) {
            return preferred;
        }
    }
    vk::CompositeAlphaFlagsKHR::OPAQUE
}

fn choose_depth_format(
    instance: &Instance,
    device: &DeviceContext,
) -> (vk::Format, vk::ImageAspectFlags) {
    let candidates = [
        (vk::Format::D32_SFLOAT, vk::ImageAspectFlags::DEPTH),
        (
            vk::Format::D24_UNORM_S8_UINT,
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
        ),
        (vk::Format::D16_UNORM, vk::ImageAspectFlags::DEPTH),
    ];
    for (format, aspect) in candidates {
        // SAFETY: the physical device belongs to this instance.
        let properties =
            unsafe { instance.get_physical_device_format_properties(device.physical, format) };
        if properties
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        {
            return (format, aspect);
        }
    }
    // D16_UNORM is mandatory for depth attachments on conformant implementations; this is
    // unreachable on real drivers but keeps the failure explicit.
    (vk::Format::D32_SFLOAT, vk::ImageAspectFlags::DEPTH)
}
