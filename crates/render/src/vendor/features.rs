//! Device features a vendor back end asks for before `vkCreateDevice` (ADR-0020 §1e, ADR-0021
//! amendment 5): XeSS's `xessVKGetRequiredDeviceFeatures` returns a `VkPhysicalDeviceFeatures2`
//! chain. The renderer already chains `VkPhysicalDeviceVulkan12Features` and
//! `VkPhysicalDeviceVulkan13Features`, and a standalone structure promoted to one of those may
//! not sit beside it (`VUID-VkDeviceCreateInfo-pNext-02830`), so the chain is **folded** into
//! bit sets of the core structures (the base features, Vulkan 1.1, 1.2 and 1.3) here. The
//! device enables each requested bit it supports; the back end's probe rejects it, with one
//! line, when one was not.

#![cfg_attr(
    not(feature = "xess"),
    allow(dead_code, reason = "only XeSS asks for device features")
)]

use std::ffi::c_void;
use std::mem::offset_of;

use ash::vk;

/// Where a `VkBool32` member sits in one of the four core structures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Core {
    /// `VkPhysicalDeviceFeatures`.
    Base,
    Vulkan11,
    Vulkan12,
    Vulkan13,
    /// `VkPhysicalDeviceMutableDescriptorTypeFeaturesEXT` (not promoted; XeSS needs it), chained
    /// on its own when asked for and supported.
    MutableDescriptorType,
}

/// `mutableDescriptorType`.
pub(crate) const MUTABLE_DESCRIPTOR_TYPE: (Core, usize) = (Core::MutableDescriptorType, 0);

/// The first `VkBool32` member's offset in an extensible feature structure (after `sType`
/// and `pNext`).
const HEADER: usize = offset_of!(
    vk::PhysicalDeviceVulkan12Features<'static>,
    sampler_mirror_clamp_to_edge
);

/// How many `VkBool32` members each core structure has.
const BASE_COUNT: usize =
    offset_of!(vk::PhysicalDeviceFeatures, inherited_queries) / size_of::<vk::Bool32>() + 1;
const V11_COUNT: usize = (offset_of!(
    vk::PhysicalDeviceVulkan11Features<'static>,
    shader_draw_parameters
) - HEADER)
    / size_of::<vk::Bool32>()
    + 1;
const V12_COUNT: usize = (offset_of!(
    vk::PhysicalDeviceVulkan12Features<'static>,
    subgroup_broadcast_dynamic_id
) - HEADER)
    / size_of::<vk::Bool32>()
    + 1;
const V13_COUNT: usize = (offset_of!(vk::PhysicalDeviceVulkan13Features<'static>, maintenance4)
    - HEADER)
    / size_of::<vk::Bool32>()
    + 1;

/// Feature bits requested (or supported), by core structure and member index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FeatureBits {
    bits: Vec<(Core, usize)>,
}

/// The member index of a field at byte `offset` in an extensible core structure.
const fn index(offset: usize) -> usize {
    (offset - HEADER) / size_of::<vk::Bool32>()
}

macro_rules! v11 {
    ($field:ident) => {
        (
            Core::Vulkan11,
            index(offset_of!(
                vk::PhysicalDeviceVulkan11Features<'static>,
                $field
            )),
        )
    };
}
macro_rules! v12 {
    ($field:ident) => {
        (
            Core::Vulkan12,
            index(offset_of!(
                vk::PhysicalDeviceVulkan12Features<'static>,
                $field
            )),
        )
    };
}
macro_rules! v13 {
    ($field:ident) => {
        (
            Core::Vulkan13,
            index(offset_of!(
                vk::PhysicalDeviceVulkan13Features<'static>,
                $field
            )),
        )
    };
}

/// The standalone structures promoted to Vulkan 1.1, 1.2 and 1.3 that a back end may ask for,
/// with where each of their members lives in the core structure. Members are listed in the
/// standalone structure's order.
fn promoted(s_type: vk::StructureType) -> Option<Vec<(Core, usize)>> {
    use vk::StructureType as S;
    Some(match s_type {
        S::PHYSICAL_DEVICE_16BIT_STORAGE_FEATURES => vec![
            v11!(storage_buffer16_bit_access),
            v11!(uniform_and_storage_buffer16_bit_access),
            v11!(storage_push_constant16),
            v11!(storage_input_output16),
        ],
        S::PHYSICAL_DEVICE_MULTIVIEW_FEATURES => vec![
            v11!(multiview),
            v11!(multiview_geometry_shader),
            v11!(multiview_tessellation_shader),
        ],
        S::PHYSICAL_DEVICE_VARIABLE_POINTERS_FEATURES => vec![
            v11!(variable_pointers_storage_buffer),
            v11!(variable_pointers),
        ],
        S::PHYSICAL_DEVICE_SHADER_DRAW_PARAMETERS_FEATURES => vec![v11!(shader_draw_parameters)],
        S::PHYSICAL_DEVICE_8BIT_STORAGE_FEATURES => vec![
            v12!(storage_buffer8_bit_access),
            v12!(uniform_and_storage_buffer8_bit_access),
            v12!(storage_push_constant8),
        ],
        S::PHYSICAL_DEVICE_SHADER_ATOMIC_INT64_FEATURES => vec![
            v12!(shader_buffer_int64_atomics),
            v12!(shader_shared_int64_atomics),
        ],
        S::PHYSICAL_DEVICE_SHADER_FLOAT16_INT8_FEATURES => {
            vec![v12!(shader_float16), v12!(shader_int8)]
        }
        S::PHYSICAL_DEVICE_SCALAR_BLOCK_LAYOUT_FEATURES => vec![v12!(scalar_block_layout)],
        S::PHYSICAL_DEVICE_SHADER_SUBGROUP_EXTENDED_TYPES_FEATURES => {
            vec![v12!(shader_subgroup_extended_types)]
        }
        S::PHYSICAL_DEVICE_TIMELINE_SEMAPHORE_FEATURES => vec![v12!(timeline_semaphore)],
        S::PHYSICAL_DEVICE_BUFFER_DEVICE_ADDRESS_FEATURES => vec![
            v12!(buffer_device_address),
            v12!(buffer_device_address_capture_replay),
            v12!(buffer_device_address_multi_device),
        ],
        S::PHYSICAL_DEVICE_HOST_QUERY_RESET_FEATURES => vec![v12!(host_query_reset)],
        S::PHYSICAL_DEVICE_VULKAN_MEMORY_MODEL_FEATURES => vec![
            v12!(vulkan_memory_model),
            v12!(vulkan_memory_model_device_scope),
            v12!(vulkan_memory_model_availability_visibility_chains),
        ],
        S::PHYSICAL_DEVICE_SHADER_INTEGER_DOT_PRODUCT_FEATURES => {
            vec![v13!(shader_integer_dot_product)]
        }
        S::PHYSICAL_DEVICE_SUBGROUP_SIZE_CONTROL_FEATURES => {
            vec![v13!(subgroup_size_control), v13!(compute_full_subgroups)]
        }
        S::PHYSICAL_DEVICE_SYNCHRONIZATION_2_FEATURES => vec![v13!(synchronization2)],
        S::PHYSICAL_DEVICE_DYNAMIC_RENDERING_FEATURES => vec![v13!(dynamic_rendering)],
        S::PHYSICAL_DEVICE_MAINTENANCE_4_FEATURES => vec![v13!(maintenance4)],
        S::PHYSICAL_DEVICE_SHADER_DEMOTE_TO_HELPER_INVOCATION_FEATURES => {
            vec![v13!(shader_demote_to_helper_invocation)]
        }
        S::PHYSICAL_DEVICE_ZERO_INITIALIZE_WORKGROUP_MEMORY_FEATURES => {
            vec![v13!(shader_zero_initialize_workgroup_memory)]
        }
        S::PHYSICAL_DEVICE_MUTABLE_DESCRIPTOR_TYPE_FEATURES_EXT => vec![MUTABLE_DESCRIPTOR_TYPE],
        _ => return None,
    })
}

/// The `count` `VkBool32` members `offset` bytes into the structure at `start`, as values.
///
/// # Safety
/// `start` points at a live structure with `count` `VkBool32` members from `offset` (they are
/// 4-byte aligned: every Vulkan structure is at least pointer-aligned).
#[allow(
    clippy::cast_ptr_alignment,
    reason = "the members are VkBool32s, 4-byte aligned"
)]
unsafe fn read_bools(start: *const u8, offset: usize, count: usize) -> Vec<bool> {
    let first = start.wrapping_add(offset).cast::<vk::Bool32>();
    // SAFETY: the caller guarantees `count` aligned, initialised members from `first`.
    let members = unsafe { std::slice::from_raw_parts(first, count) };
    members.iter().map(|&value| value != vk::FALSE).collect()
}

/// The `count` `VkBool32` members `offset` bytes into `value`, writable.
///
/// # Safety
/// `T` has `count` `VkBool32` members from `offset`.
#[allow(
    clippy::cast_ptr_alignment,
    reason = "the members are VkBool32s, 4-byte aligned"
)]
unsafe fn members_mut<T>(value: &mut T, offset: usize, count: usize) -> &mut [vk::Bool32] {
    let first = std::ptr::from_mut(value)
        .cast::<u8>()
        .wrapping_add(offset)
        .cast::<vk::Bool32>();
    // SAFETY: the caller guarantees `count` members inside `value`, which is borrowed
    // exclusively for the result's lifetime.
    unsafe { std::slice::from_raw_parts_mut(first, count) }
}

impl FeatureBits {
    /// Whether nothing is requested.
    pub(crate) fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    fn insert(&mut self, bit: (Core, usize)) {
        if !self.bits.contains(&bit) {
            self.bits.push(bit);
            self.bits.sort_unstable();
        }
    }

    /// Folds a `VkPhysicalDeviceFeatures2` chain (or a chain of extensible feature
    /// structures) into core bits. Unknown structures are an error naming their `sType`.
    ///
    /// # Safety
    /// `head` is null or points at a valid Vulkan structure chain whose structures stay
    /// alive for the call.
    pub(crate) unsafe fn fold(head: *const c_void) -> Result<Self, String> {
        let mut bits = Self::default();
        let mut next = head.cast::<vk::BaseInStructure<'_>>();
        let mut guard = 0;
        while !next.is_null() {
            guard += 1;
            if guard > 64 {
                return Err(String::from("the feature chain does not end"));
            }
            // SAFETY: the caller guarantees a valid chain; every structure starts with the
            // `sType`/`pNext` header.
            let base = unsafe { next.read() };
            let start = next.cast::<u8>();
            let read = |count| {
                // SAFETY: a structure of the `sType` read has `count` members after the
                // header (the core structures' layouts, from ash's declarations).
                unsafe { read_bools(start, HEADER, count) }
            };
            match base.s_type {
                vk::StructureType::PHYSICAL_DEVICE_FEATURES_2 => {
                    let offset = offset_of!(vk::PhysicalDeviceFeatures2<'static>, features);
                    // SAFETY: as above; `features` is a `VkPhysicalDeviceFeatures` of
                    // `BASE_COUNT` members.
                    let values = unsafe { read_bools(start, offset, BASE_COUNT) };
                    bits.extend(Core::Base, &values);
                }
                vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_1_FEATURES => {
                    bits.extend(Core::Vulkan11, &read(V11_COUNT));
                }
                vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES => {
                    bits.extend(Core::Vulkan12, &read(V12_COUNT));
                }
                vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_3_FEATURES => {
                    bits.extend(Core::Vulkan13, &read(V13_COUNT));
                }
                other => {
                    let members = promoted(other).ok_or_else(|| {
                        format!(
                            "a feature structure the renderer cannot merge ({other:?}, \
                             VUID-VkDeviceCreateInfo-pNext-02830)"
                        )
                    })?;
                    for (value, bit) in read(members.len()).into_iter().zip(members) {
                        if value {
                            bits.insert(bit);
                        }
                    }
                }
            }
            next = base.p_next;
        }
        Ok(bits)
    }

    fn extend(&mut self, core: Core, values: &[bool]) {
        for (index, &value) in values.iter().enumerate() {
            if value {
                self.insert((core, index));
            }
        }
    }

    /// The bits of `self` that `supported` has too.
    pub(crate) fn common(&self, supported: &Self) -> Self {
        Self {
            bits: self
                .bits
                .iter()
                .copied()
                .filter(|bit| supported.bits.contains(bit))
                .collect(),
        }
    }

    /// The bits of `self` that `supported` lacks.
    pub(crate) fn missing(&self, supported: &Self) -> Self {
        Self {
            bits: self
                .bits
                .iter()
                .copied()
                .filter(|bit| !supported.bits.contains(bit))
                .collect(),
        }
    }

    /// The bits set in the four core structures, as the device reported or will enable them.
    pub(crate) fn of(
        base: &vk::PhysicalDeviceFeatures,
        v11: &vk::PhysicalDeviceVulkan11Features<'_>,
        v12: &vk::PhysicalDeviceVulkan12Features<'_>,
        v13: &vk::PhysicalDeviceVulkan13Features<'_>,
    ) -> Self {
        let mut bits = Self::default();
        // SAFETY: each is a live core structure with the stated member count.
        let base = unsafe { read_bools(std::ptr::from_ref(base).cast(), 0, BASE_COUNT) };
        // SAFETY: as above.
        let v11 = unsafe { read_bools(std::ptr::from_ref(v11).cast(), HEADER, V11_COUNT) };
        // SAFETY: as above.
        let v12 = unsafe { read_bools(std::ptr::from_ref(v12).cast(), HEADER, V12_COUNT) };
        // SAFETY: as above.
        let v13 = unsafe { read_bools(std::ptr::from_ref(v13).cast(), HEADER, V13_COUNT) };
        bits.extend(Core::Base, &base);
        bits.extend(Core::Vulkan11, &v11);
        bits.extend(Core::Vulkan12, &v12);
        bits.extend(Core::Vulkan13, &v13);
        bits
    }

    /// Sets every bit of `self` in the four core structures (the device's own requests stay
    /// set).
    pub(crate) fn enable(
        &self,
        base: &mut vk::PhysicalDeviceFeatures,
        v11: &mut vk::PhysicalDeviceVulkan11Features<'_>,
        v12: &mut vk::PhysicalDeviceVulkan12Features<'_>,
        v13: &mut vk::PhysicalDeviceVulkan13Features<'_>,
    ) {
        for &(core, index) in &self.bits {
            let members = match core {
                // SAFETY: each is a core structure with the stated member count.
                Core::Base => unsafe { members_mut(base, 0, BASE_COUNT) },
                // SAFETY: as above.
                Core::Vulkan11 => unsafe { members_mut(v11, HEADER, V11_COUNT) },
                // SAFETY: as above.
                Core::Vulkan12 => unsafe { members_mut(v12, HEADER, V12_COUNT) },
                // SAFETY: as above.
                Core::Vulkan13 => unsafe { members_mut(v13, HEADER, V13_COUNT) },
                // Chained by the device on its own (`contains`).
                Core::MutableDescriptorType => continue,
            };
            if let Some(member) = members.get_mut(index) {
                *member = vk::TRUE;
            }
        }
    }

    /// Whether `bit` is set.
    pub(crate) fn contains(&self, bit: (Core, usize)) -> bool {
        self.bits.contains(&bit)
    }

    /// `self` with `bit` set.
    #[must_use]
    pub(crate) fn with(mut self, bit: (Core, usize)) -> Self {
        self.insert(bit);
        self
    }

    /// Whether any Vulkan 1.1 bit is set (the renderer chains that structure only then).
    pub(crate) fn has_vulkan11(&self) -> bool {
        self.bits.iter().any(|(core, _)| *core == Core::Vulkan11)
    }

    /// How many bits are set.
    pub(crate) fn len(&self) -> usize {
        self.bits.len()
    }
}

#[cfg(test)]
mod tests {
    use ash::vk::TaggedStructure as _;

    use super::*;

    #[test]
    fn counts_match_the_core_structures() {
        assert_eq!(BASE_COUNT, 55);
        assert_eq!(V11_COUNT, 12);
        assert_eq!(V12_COUNT, 47);
        assert_eq!(V13_COUNT, 15);
    }

    /// Standalone structures fold into the core ones (VUID-02830), and core structures read
    /// member by member.
    #[test]
    fn chains_fold_into_core_bits() {
        let mut float16 =
            vk::PhysicalDeviceShaderFloat16Int8Features::default().shader_float16(true);
        let mut dot = vk::PhysicalDeviceShaderIntegerDotProductFeatures::default()
            .shader_integer_dot_product(true);
        let mut v12 = vk::PhysicalDeviceVulkan12Features::default().shader_int8(true);
        let features = vk::PhysicalDeviceFeatures::default().shader_int16(true);
        let chain = vk::PhysicalDeviceFeatures2::default()
            .features(features)
            .push(&mut float16)
            .push(&mut dot)
            .push(&mut v12);
        // SAFETY: a valid chain on this stack.
        let bits = unsafe { FeatureBits::fold(std::ptr::from_ref(&chain).cast()) }.unwrap();
        assert_eq!(bits.len(), 4);
        assert!(!bits.has_vulkan11());

        let mut base = vk::PhysicalDeviceFeatures::default();
        let mut o11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut o12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
        let mut o13 = vk::PhysicalDeviceVulkan13Features::default();
        bits.enable(&mut base, &mut o11, &mut o12, &mut o13);
        assert_eq!(base.shader_int16, vk::TRUE);
        assert_eq!(o12.shader_float16, vk::TRUE);
        assert_eq!(o12.shader_int8, vk::TRUE);
        assert_eq!(
            o12.timeline_semaphore,
            vk::TRUE,
            "the renderer's own bits stay"
        );
        assert_eq!(o13.shader_integer_dot_product, vk::TRUE);
        assert_eq!(o13.dynamic_rendering, vk::FALSE);
        assert_eq!(
            FeatureBits::of(&base, &o11, &o12, &o13).missing(&bits),
            FeatureBits {
                bits: vec![v12!(timeline_semaphore)]
            }
        );
        assert!(
            bits.missing(&FeatureBits::of(&base, &o11, &o12, &o13))
                .is_empty()
        );
    }

    /// XeSS's `mutableDescriptorType` is an extension feature, kept as its own bit.
    #[test]
    fn mutable_descriptor_type_is_its_own_bit() {
        let mut mutable = vk::PhysicalDeviceMutableDescriptorTypeFeaturesEXT::default()
            .mutable_descriptor_type(true);
        let chain = vk::PhysicalDeviceFeatures2::default().push(&mut mutable);
        // SAFETY: a valid chain on this stack.
        let bits = unsafe { FeatureBits::fold(std::ptr::from_ref(&chain).cast()) }.unwrap();
        assert!(bits.contains(MUTABLE_DESCRIPTOR_TYPE));
        assert!(
            !bits
                .missing(&FeatureBits::default().with(MUTABLE_DESCRIPTOR_TYPE))
                .contains(MUTABLE_DESCRIPTOR_TYPE)
        );
    }

    #[test]
    fn unknown_structures_are_reported() {
        let mut mesh = vk::PhysicalDeviceMeshShaderFeaturesEXT::default().mesh_shader(true);
        let chain = vk::PhysicalDeviceFeatures2::default().push(&mut mesh);
        // SAFETY: a valid chain on this stack.
        let error = unsafe { FeatureBits::fold(std::ptr::from_ref(&chain).cast()) }.unwrap_err();
        assert!(error.contains("MESH_SHADER"), "{error}");
        // SAFETY: null is an empty chain.
        let empty = unsafe { FeatureBits::fold(std::ptr::null()) }.unwrap();
        assert!(empty.is_empty());
    }
}
