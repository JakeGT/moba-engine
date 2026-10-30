//! Graphics pipeline construction for the scene, composite and UI passes.
//!
//! Every pipeline goes through [`DeviceContext::create_graphics_pipeline`], i.e. the
//! persistent pipeline cache, and is built when the renderer is created.

use ash::vk;

use crate::device::DeviceContext;
use crate::error::RenderResult;
use crate::mesh::{SkinnedVertex, Vertex};

/// Vertex buffer layout consumed by a scene pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VertexLayout {
    /// [`Vertex`] (stride 48), position and normal only: the vertex-colour unit and terrain
    /// shaders, which do not read the UV or tangent.
    Static,
    /// [`Vertex`] with the UV at location 4 and the tangent at 5: the `MATERIAL_TEXTURED`
    /// unit permutation.
    StaticTextured,
    /// [`Vertex`], position only: the static shadow caster (`unit_shadow.vert`).
    StaticShadow,
    /// [`Vertex`], position, normal and the UV at location 4 (the terrain's baked relief
    /// occlusion; `terrain.vert`, R3a).
    Terrain,
    /// [`SkinnedVertex`] (stride 56): position, normal, `R8G8B8A8_UINT` joints and
    /// `R8G8B8A8_UNORM` weights (`skinned.vert`).
    Skinned,
    /// [`SkinnedVertex`] with the UV at location 4 and the tangent at 5
    /// (`skinned_textured.vert`).
    SkinnedTextured,
    /// [`SkinnedVertex`], position, joints and weights: the skinned shadow caster
    /// (`skinned_shadow.vert`).
    SkinnedShadow,
}

impl VertexLayout {
    fn stride(self) -> u32 {
        let size = match self {
            Self::Static | Self::StaticTextured | Self::StaticShadow | Self::Terrain => {
                core::mem::size_of::<Vertex>()
            }
            Self::Skinned | Self::SkinnedTextured | Self::SkinnedShadow => {
                core::mem::size_of::<SkinnedVertex>()
            }
        };
        u32::try_from(size).expect("vertex stride fits u32")
    }

    /// Attribute descriptions: locations 0/1 (position, normal), 2/3 (joints, weights) when
    /// skinned, 4/5 (UV, tangent) only for the textured permutations and only 0 (+ 2/3) for
    /// the shadow casters, so every declared attribute is consumed by its shader. The stride
    /// is the same for every layout of one vertex type.
    fn attributes(self) -> &'static [vk::VertexInputAttributeDescription] {
        const fn attribute(
            location: u32,
            format: vk::Format,
            offset: u32,
        ) -> vk::VertexInputAttributeDescription {
            vk::VertexInputAttributeDescription {
                location,
                binding: 0,
                format,
                offset,
            }
        }
        const STATIC: [vk::VertexInputAttributeDescription; 4] = [
            attribute(0, vk::Format::R32G32B32_SFLOAT, 0),
            attribute(1, vk::Format::R32G32B32_SFLOAT, 12),
            attribute(4, vk::Format::R32G32_SFLOAT, 24),
            attribute(5, vk::Format::R32G32B32A32_SFLOAT, 32),
        ];
        const SKINNED: [vk::VertexInputAttributeDescription; 6] = [
            attribute(0, vk::Format::R32G32B32_SFLOAT, 0),
            attribute(1, vk::Format::R32G32B32_SFLOAT, 12),
            attribute(2, vk::Format::R8G8B8A8_UINT, 24),
            attribute(3, vk::Format::R8G8B8A8_UNORM, 28),
            attribute(4, vk::Format::R32G32_SFLOAT, 32),
            attribute(5, vk::Format::R32G32B32A32_SFLOAT, 40),
        ];
        const TERRAIN: [vk::VertexInputAttributeDescription; 3] = [
            attribute(0, vk::Format::R32G32B32_SFLOAT, 0),
            attribute(1, vk::Format::R32G32B32_SFLOAT, 12),
            attribute(4, vk::Format::R32G32_SFLOAT, 24),
        ];
        const SKINNED_SHADOW: [vk::VertexInputAttributeDescription; 3] = [
            attribute(0, vk::Format::R32G32B32_SFLOAT, 0),
            attribute(2, vk::Format::R8G8B8A8_UINT, 24),
            attribute(3, vk::Format::R8G8B8A8_UNORM, 28),
        ];
        match self {
            Self::Static => &STATIC[..2],
            Self::StaticTextured => &STATIC,
            Self::StaticShadow => &STATIC[..1],
            Self::Terrain => &TERRAIN,
            Self::Skinned => &SKINNED[..4],
            Self::SkinnedTextured => &SKINNED,
            Self::SkinnedShadow => &SKINNED_SHADOW,
        }
    }
}

/// Immutable inputs for one scene pipeline.
pub(crate) struct PipelineSpec {
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) vertex_layout: VertexLayout,
    pub(crate) vertex: vk::ShaderModule,
    pub(crate) fragment: vk::ShaderModule,
    pub(crate) color_format: vk::Format,
    pub(crate) depth_format: vk::Format,
    pub(crate) cull_mode: vk::CullModeFlags,
}

pub(crate) fn create(device: &DeviceContext, spec: &PipelineSpec) -> RenderResult<vk::Pipeline> {
    let entry_point = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(spec.vertex)
            .name(entry_point),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(spec.fragment)
            .name(entry_point),
    ];

    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(spec.vertex_layout.stride())
        .input_rate(vk::VertexInputRate::VERTEX)];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(spec.vertex_layout.attributes());

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);

    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);

    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(spec.cull_mode)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .depth_bias_enable(false)
        .line_width(1.0);

    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);

    let color_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(false)
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(&color_attachments);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let color_formats = [spec.color_format];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(spec.depth_format);

    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(spec.layout)
        .push_next(&mut rendering);

    // Every referenced struct outlives the call; the layout matches the shaders.
    device.create_graphics_pipeline(&create_info)
}

/// Creates a depth-only shadow-caster pipeline: no fragment shader and no colour attachment,
/// depth test/write `LESS_OR_EQUAL`, no culling (sample meshes are not all closed), a dynamic
/// slope-scaled depth bias and, with `depth_clamp` (the device feature), depth clamping, so a
/// caster nearer the light than the map's near plane is flattened onto it instead of clipped
/// ("pancaking").
pub(crate) fn create_depth_only(
    device: &DeviceContext,
    layout: vk::PipelineLayout,
    vertex_layout: VertexLayout,
    vertex: vk::ShaderModule,
    depth_format: vk::Format,
    depth_clamp: bool,
) -> RenderResult<vk::Pipeline> {
    let stages = [vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::VERTEX)
        .module(vertex)
        .name(c"main")];
    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(vertex_layout.stride())
        .input_rate(vk::VertexInputRate::VERTEX)];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(vertex_layout.attributes());
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .depth_clamp_enable(depth_clamp)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .depth_bias_enable(true)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default().logic_op_enable(false);
    let dynamic_states = [
        vk::DynamicState::VIEWPORT,
        vk::DynamicState::SCISSOR,
        vk::DynamicState::DEPTH_BIAS,
    ];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let mut rendering =
        vk::PipelineRenderingCreateInfo::default().depth_attachment_format(depth_format);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);
    // Every referenced struct outlives the call; the layout matches the shader.
    device.create_graphics_pipeline(&create_info)
}

/// Creates the screen-space UI pipeline: no depth attachment, alpha blending, the atlas set.
#[allow(clippy::too_many_lines)] // One linear pipeline description (seven vertex attributes).
pub(crate) fn create_ui(
    device: &DeviceContext,
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    color_format: vk::Format,
) -> RenderResult<vk::Pipeline> {
    let entry_point = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(entry_point),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(entry_point),
    ];
    let bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(u32::try_from(core::mem::size_of::<crate::ui::UiVertex>()).expect("stride fits"))
        .input_rate(vk::VertexInputRate::VERTEX)];
    let attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(8),
        vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R32_UINT)
            .offset(16),
        vk::VertexInputAttributeDescription::default()
            .location(3)
            .binding(0)
            .format(vk::Format::R32G32B32A32_SFLOAT)
            .offset(20),
        // R7a: packed outline colour and the per-mode parameters.
        vk::VertexInputAttributeDescription::default()
            .location(4)
            .binding(0)
            .format(vk::Format::R32_UINT)
            .offset(36),
        vk::VertexInputAttributeDescription::default()
            .location(5)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(40),
        // R7b: the offscreen image slot of `MODE_IMAGE`.
        vk::VertexInputAttributeDescription::default()
            .location(6)
            .binding(0)
            .format(vk::Format::R32_UINT)
            .offset(48),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::ALWAYS);
    let attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(core::slice::from_ref(&attachment));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    // The UI pass renders without a depth attachment, so the format must be UNDEFINED.
    let color_formats = [color_format];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(vk::Format::UNDEFINED);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);
    // Every referenced struct outlives the call; the layout matches the shaders.
    device.create_graphics_pipeline(&create_info)
}

/// Creates a fullscreen-triangle pipeline: no vertex input, no depth, no blending.
pub(crate) fn create_fullscreen(
    device: &DeviceContext,
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    color_format: vk::Format,
) -> RenderResult<vk::Pipeline> {
    let entry_point = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(entry_point),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(entry_point),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::ALWAYS);
    let attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(false)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(core::slice::from_ref(&attachment));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let color_formats = [color_format];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(vk::Format::UNDEFINED);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);
    // Every referenced struct outlives the call; the layout matches the shaders.
    device.create_graphics_pipeline(&create_info)
}

/// How a [`create_vertexless`] pipeline treats depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VertexlessDepth {
    /// No depth attachment.
    None,
    /// Test `LESS_OR_EQUAL` against `format` without writing (the R3a sky on the far plane).
    TestOnly(vk::Format),
}

/// Creates a pipeline with no vertex input (the vertex shader builds its own geometry from
/// `gl_VertexIndex`/`gl_InstanceIndex`) into one colour attachment, optionally blended with
/// `max_blend` (`MAX` of source and destination: the R3a fog-of-war raster) and with the given
/// depth treatment. Used by the fog-of-war passes and the sky (ADR-0014).
pub(crate) fn create_vertexless(
    device: &DeviceContext,
    layout: vk::PipelineLayout,
    vertex: vk::ShaderModule,
    fragment: vk::ShaderModule,
    color_format: vk::Format,
    max_blend: bool,
    depth: VertexlessDepth,
) -> RenderResult<vk::Pipeline> {
    let entry_point = c"main";
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(entry_point),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(entry_point),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let (depth_format, depth_stencil) = match depth {
        VertexlessDepth::None => (
            vk::Format::UNDEFINED,
            vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(false)
                .depth_write_enable(false)
                .depth_compare_op(vk::CompareOp::ALWAYS),
        ),
        VertexlessDepth::TestOnly(format) => (
            format,
            vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(false)
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
                .depth_bounds_test_enable(false)
                .stencil_test_enable(false),
        ),
    };
    let attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(max_blend)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE)
        .color_blend_op(vk::BlendOp::MAX)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
        .alpha_blend_op(vk::BlendOp::MAX)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(core::slice::from_ref(&attachment));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let color_formats = [color_format];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(depth_format);
    let create_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push_next(&mut rendering);
    // Every referenced struct outlives the call; the layout matches the shaders.
    device.create_graphics_pipeline(&create_info)
}

#[cfg(test)]
mod tests {
    use super::VertexLayout;
    use crate::mesh::SkinnedVertex;

    #[test]
    fn skinned_attributes_match_the_vertex_struct() {
        let attributes = VertexLayout::SkinnedTextured.attributes();
        assert_eq!(VertexLayout::Skinned.stride(), 56);
        assert_eq!(VertexLayout::SkinnedTextured.stride(), 56);
        assert_eq!(VertexLayout::SkinnedShadow.stride(), 56);
        assert_eq!(VertexLayout::Static.stride(), 48);
        assert_eq!(VertexLayout::StaticShadow.attributes().len(), 1);
        assert_eq!(
            attributes[5].offset as usize,
            core::mem::offset_of!(SkinnedVertex, tangent)
        );
        let shadow = VertexLayout::SkinnedShadow.attributes();
        assert_eq!(
            shadow.iter().map(|a| a.location).collect::<Vec<_>>(),
            [0, 2, 3]
        );
        assert_eq!(VertexLayout::Skinned.attributes().len(), 4);
        assert_eq!(
            attributes[2].offset as usize,
            core::mem::offset_of!(SkinnedVertex, joints)
        );
        assert_eq!(
            attributes[3].offset as usize,
            core::mem::offset_of!(SkinnedVertex, weights)
        );
        assert_eq!(
            attributes[4].offset as usize,
            core::mem::offset_of!(SkinnedVertex, uv)
        );
        assert_eq!(VertexLayout::Static.attributes().len(), 2);
        assert_eq!(
            VertexLayout::StaticTextured.attributes()[2].offset as usize,
            core::mem::offset_of!(crate::mesh::Vertex, uv)
        );
        assert_eq!(
            VertexLayout::StaticTextured.attributes()[3].offset as usize,
            core::mem::offset_of!(crate::mesh::Vertex, tangent)
        );
    }
}
