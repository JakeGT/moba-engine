//! AMD FidelityFX **FSR 3.1.6** upscaler passes (feature `fsr3`; ADR-0020 §6a and Q9,
//! ADR-0021 §4b).
//!
//! The SDK's MIT HLSL is vendored unmodified under `third_party/fidelityfx/` and compiled by
//! the pinned `slangc` with the permutation the build script records (`FSR3_DEFINES`: 32-bit
//! math, HDR colour, render-resolution unjittered motion vectors, non-inverted depth). Every
//! binding sits in descriptor set 0: constant buffers from 0, sampled images from 16, storage
//! images from 32 and samplers from 48 (the HLSL `register` numbers shifted, see
//! `docs/dev/fsr3.md`).
//!
//! [`PASSES`] lists the modules in dispatch order with their workgroup sizes and every
//! binding the vendor code declares. A binding with `used: false` is declared but not
//! referenced by that permutation; `slangc` keeps it in the reflection but drops it from the
//! SPIR-V, so the renderer leaves it out of the set layout. The tests below hold this table
//! to the reflection and to the SPIR-V itself. `moba_render::fsr3` consumes it.

/// What a binding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A uniform buffer (an HLSL `cbuffer`).
    Constants,
    /// A sampled image (an HLSL `Texture2D`, `t` register).
    Sampled,
    /// A storage image without a format (an HLSL `RWTexture2D`, `u` register).
    Storage,
    /// A sampler (`s` register).
    Sampler,
}

/// One declared binding of a pass, in descriptor set 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The Vulkan binding number.
    pub binding: u32,
    /// The vendor name (`r_input_color_jittered`, `rw_upscaled_output`, `cbFSR3Upscaler`).
    pub name: &'static str,
    /// The descriptor kind.
    pub kind: Kind,
    /// Whether the SPIR-V references it. Unused bindings are not in the set layout.
    pub used: bool,
}

/// One compiled FSR pass.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    /// The module name (`fsr3_prepare_inputs`, ...), also its reflection key.
    pub name: &'static str,
    /// SPIR-V 1.6, entry point `CS`.
    pub spirv: &'static [u8],
    /// `numthreads` of the entry point.
    pub workgroup: [u32; 3],
    /// Every binding the pass declares, used or not.
    pub bindings: &'static [Binding],
}

/// The FSR constant buffer (`cbFSR3Upscaler`), 148 bytes, at binding 0 of every pass.
pub const CB_FSR3_UPSCALER: u32 = 0;
/// The SPD constant buffer (`cbSPD`) of the two pyramid passes.
pub const CB_SPD: u32 = 1;
/// The RCAS constant buffer (`cbRCAS`).
pub const CB_RCAS: u32 = 1;
/// The point-clamp sampler binding.
pub const POINT_CLAMP: u32 = 48;
/// The linear-clamp sampler binding.
pub const LINEAR_CLAMP: u32 = 49;
/// Size of `cbFSR3Upscaler` in bytes.
pub const CB_FSR3_UPSCALER_SIZE: usize = 148;
/// Size of `cbSPD` in bytes.
pub const CB_SPD_SIZE: usize = 24;
/// Size of `cbRCAS` in bytes.
pub const CB_RCAS_SIZE: usize = 16;

macro_rules! binding {
    ($binding:expr, $name:literal, $kind:ident) => {
        Binding {
            binding: $binding,
            name: $name,
            kind: Kind::$kind,
            used: true,
        }
    };
    ($binding:expr, $name:literal, $kind:ident, unused) => {
        Binding {
            binding: $binding,
            name: $name,
            kind: Kind::$kind,
            used: false,
        }
    };
}

macro_rules! spirv {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".spv"))
    };
}

/// Prepare inputs: dilates depth and motion vectors, reconstructs the previous frame's
/// nearest depth, writes the farthest depth and the current luma. Render resolution.
pub const PREPARE_INPUTS: Pass = Pass {
    name: "fsr3_prepare_inputs",
    spirv: spirv!("fsr3_prepare_inputs"),
    workgroup: [8, 8, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(16, "r_input_motion_vectors", Sampled),
        binding!(17, "r_input_depth", Sampled),
        binding!(18, "r_input_color_jittered", Sampled),
        binding!(32, "rw_dilated_motion_vectors", Storage),
        binding!(33, "rw_dilated_depth", Storage),
        binding!(34, "rw_reconstructed_previous_nearest_depth", Storage),
        binding!(35, "rw_farthest_depth", Storage),
        binding!(36, "rw_current_luma", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler, unused),
    ],
};

/// Luma pyramid (single-pass downsampler): the frame's average luma into `frame_info` and
/// the farthest-depth mip 1.
pub const LUMA_PYRAMID: Pass = Pass {
    name: "fsr3_luma_pyramid",
    spirv: spirv!("fsr3_luma_pyramid"),
    workgroup: [256, 1, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(1, "cbSPD", Constants),
        binding!(16, "r_current_luma", Sampled),
        binding!(17, "r_farthest_depth", Sampled),
        binding!(32, "rw_spd_global_atomic", Storage),
        binding!(33, "rw_frame_info", Storage),
        binding!(34, "rw_spd_mip0", Storage, unused),
        binding!(35, "rw_spd_mip1", Storage, unused),
        binding!(36, "rw_spd_mip2", Storage, unused),
        binding!(37, "rw_spd_mip3", Storage, unused),
        binding!(38, "rw_spd_mip4", Storage, unused),
        binding!(39, "rw_spd_mip5", Storage),
        binding!(40, "rw_farthest_depth_mip1", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler, unused),
    ],
};

/// Shading-change pyramid (single-pass downsampler): luma change against the reprojected
/// previous frame, mips 0 to 5.
pub const SHADING_CHANGE_PYRAMID: Pass = Pass {
    name: "fsr3_shading_change_pyramid",
    spirv: spirv!("fsr3_shading_change_pyramid"),
    workgroup: [256, 1, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(1, "cbSPD", Constants),
        binding!(16, "r_current_luma", Sampled),
        binding!(17, "r_previous_luma", Sampled),
        binding!(18, "r_dilated_motion_vectors", Sampled),
        binding!(19, "r_input_exposure", Sampled),
        binding!(32, "rw_spd_global_atomic", Storage),
        binding!(33, "rw_spd_mip0", Storage),
        binding!(34, "rw_spd_mip1", Storage),
        binding!(35, "rw_spd_mip2", Storage),
        binding!(36, "rw_spd_mip3", Storage),
        binding!(37, "rw_spd_mip4", Storage),
        binding!(38, "rw_spd_mip5", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler, unused),
    ],
};

/// Shading change: resolves the pyramid into the half-resolution shading-change image.
pub const SHADING_CHANGE: Pass = Pass {
    name: "fsr3_shading_change",
    spirv: spirv!("fsr3_shading_change"),
    workgroup: [8, 8, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(16, "r_spd_mips", Sampled),
        binding!(32, "rw_shading_change", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler),
    ],
};

/// Prepare reactivity: dilated reactive masks, new locks and the accumulation weights.
pub const PREPARE_REACTIVITY: Pass = Pass {
    name: "fsr3_prepare_reactivity",
    spirv: spirv!("fsr3_prepare_reactivity"),
    workgroup: [8, 8, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(16, "r_reconstructed_previous_nearest_depth", Sampled),
        binding!(17, "r_dilated_motion_vectors", Sampled),
        binding!(18, "r_dilated_depth", Sampled),
        binding!(19, "r_reactive_mask", Sampled),
        binding!(20, "r_transparency_and_composition_mask", Sampled),
        binding!(21, "r_accumulation", Sampled),
        binding!(22, "r_shading_change", Sampled),
        binding!(23, "r_current_luma", Sampled),
        binding!(24, "r_input_exposure", Sampled),
        binding!(32, "rw_dilated_reactive_masks", Storage),
        binding!(33, "rw_new_locks", Storage),
        binding!(34, "rw_accumulation", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler),
    ],
};

/// Luma instability: the luma history and this frame's instability.
pub const LUMA_INSTABILITY: Pass = Pass {
    name: "fsr3_luma_instability",
    spirv: spirv!("fsr3_luma_instability"),
    workgroup: [8, 8, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants),
        binding!(16, "r_input_exposure", Sampled),
        binding!(17, "r_dilated_reactive_masks", Sampled),
        binding!(18, "r_dilated_motion_vectors", Sampled),
        binding!(19, "r_frame_info", Sampled, unused),
        binding!(20, "r_luma_history", Sampled),
        binding!(21, "r_farthest_depth_mip1", Sampled, unused),
        binding!(22, "r_current_luma", Sampled),
        binding!(32, "rw_luma_history", Storage),
        binding!(33, "rw_luma_instability", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler),
    ],
};

/// The bindings of both accumulate permutations; `output_used` says whether the pass writes
/// the caller's output (only without sharpening, where RCAS does not follow).
macro_rules! accumulate_bindings {
    ($($output:tt)*) => {
        &[
            binding!(0, "cbFSR3Upscaler", Constants),
            binding!(16, "r_input_exposure", Sampled),
            binding!(17, "r_dilated_reactive_masks", Sampled),
            binding!(18, "r_dilated_motion_vectors", Sampled),
            binding!(19, "r_internal_upscaled_color", Sampled),
            binding!(20, "r_lanczos_lut", Sampled, unused),
            binding!(21, "r_farthest_depth_mip1", Sampled),
            binding!(22, "r_current_luma", Sampled, unused),
            binding!(23, "r_luma_instability", Sampled),
            binding!(24, "r_input_color_jittered", Sampled),
            binding!(32, "rw_internal_upscaled_color", Storage),
            binding!(33, "rw_upscaled_output", Storage $($output)*),
            binding!(34, "rw_new_locks", Storage),
            binding!(48, "s_PointClamp", Sampler, unused),
            binding!(49, "s_LinearClamp", Sampler),
        ]
    };
}

/// Accumulate without sharpening: reprojects and blends the history at output resolution
/// and writes the caller's output.
pub const ACCUMULATE: Pass = Pass {
    name: "fsr3_accumulate",
    spirv: spirv!("fsr3_accumulate"),
    workgroup: [8, 8, 1],
    bindings: accumulate_bindings!(),
};

/// Accumulate with sharpening (`FFX_FSR3UPSCALER_OPTION_APPLY_SHARPENING=1`): writes the
/// history only; [`RCAS`] then writes the output from it.
pub const ACCUMULATE_SHARPEN: Pass = Pass {
    name: "fsr3_accumulate_sharpen",
    spirv: spirv!("fsr3_accumulate_sharpen"),
    workgroup: [8, 8, 1],
    bindings: accumulate_bindings!(, unused),
};

/// Robust contrast-adaptive sharpening of the new history into the caller's output. Each
/// 64-thread group covers a 16x16 tile.
pub const RCAS: Pass = Pass {
    name: "fsr3_rcas",
    spirv: spirv!("fsr3_rcas"),
    workgroup: [64, 1, 1],
    bindings: &[
        binding!(0, "cbFSR3Upscaler", Constants, unused),
        binding!(1, "cbRCAS", Constants),
        binding!(16, "r_input_exposure", Sampled),
        binding!(17, "r_rcas_input", Sampled),
        binding!(32, "rw_upscaled_output", Storage),
        binding!(48, "s_PointClamp", Sampler, unused),
        binding!(49, "s_LinearClamp", Sampler, unused),
    ],
};

/// Every pass, in dispatch order (one upscale runs the first six, then [`ACCUMULATE`], or
/// [`ACCUMULATE_SHARPEN`] and [`RCAS`]).
pub const PASSES: &[Pass] = &[
    PREPARE_INPUTS,
    LUMA_PYRAMID,
    SHADING_CHANGE_PYRAMID,
    SHADING_CHANGE,
    PREPARE_REACTIVITY,
    LUMA_INSTABILITY,
    ACCUMULATE,
    ACCUMULATE_SHARPEN,
    RCAS,
];

include!(concat!(env!("OUT_DIR"), "/fsr3_reflection.rs"));

/// The `slangc -reflection-json` output of the FSR pass `name`, for layout tests (the
/// `cbFSR3Upscaler` mirror in `moba_render`).
#[must_use]
pub fn reflection(name: &str) -> Option<&'static str> {
    FSR3_REFLECTION
        .iter()
        .find(|(module, _)| *module == name)
        .map(|(_, json)| *json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn words(spirv: &[u8]) -> Vec<u32> {
        spirv
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect()
    }

    /// Calls `visit(opcode, operands)` for every instruction after the header.
    fn instructions(spirv: &[u8], mut visit: impl FnMut(u32, &[u32])) {
        let words = words(spirv);
        assert_eq!(words[0], 0x0723_0203, "SPIR-V magic");
        let mut at = 5;
        while at < words.len() {
            let count = (words[at] >> 16) as usize;
            assert!(count > 0 && at + count <= words.len(), "malformed SPIR-V");
            visit(words[at] & 0xffff, &words[at + 1..at + count]);
            at += count;
        }
    }

    #[test]
    fn every_pass_is_built_and_listed_once() {
        let mut built: Vec<&str> = FSR3_REFLECTION.iter().map(|(name, _)| *name).collect();
        let mut listed: Vec<&str> = PASSES.iter().map(|pass| pass.name).collect();
        built.sort_unstable();
        listed.sort_unstable();
        assert_eq!(built, listed);
        for pass in PASSES {
            let version = u32::from_le_bytes(pass.spirv[4..8].try_into().unwrap());
            assert_eq!(version, 0x0001_0600, "{} is not SPIR-V 1.6", pass.name);
            let numbers: BTreeSet<u32> = pass.bindings.iter().map(|b| b.binding).collect();
            assert_eq!(
                numbers.len(),
                pass.bindings.len(),
                "{}: duplicate",
                pass.name
            );
        }
        assert_ne!(ACCUMULATE.spirv, ACCUMULATE_SHARPEN.spirv);
    }

    /// The reflected parameters of every pass are exactly its table: name, binding, set 0
    /// and descriptor kind.
    #[test]
    fn reflection_matches_the_table() {
        for pass in PASSES {
            let json: serde_json::Value =
                serde_json::from_str(reflection(pass.name).unwrap()).unwrap();
            let mut actual = BTreeMap::new();
            for parameter in json["parameters"].as_array().into_iter().flatten() {
                let name = parameter["name"].as_str().unwrap().to_owned();
                // `slangc` reflects HLSL register classes (after the shifts) for HLSL input.
                let binding = &parameter["binding"];
                assert_eq!(binding["space"].as_u64().unwrap_or(0), 0, "{name}: set");
                let ty = &parameter["type"];
                let texture = || {
                    assert_eq!(ty["baseShape"], "texture2D", "{}: `{name}`", pass.name);
                    assert_ne!(ty["combined"].as_bool(), Some(true), "{name}");
                };
                let kind = match (binding["kind"].as_str(), ty["kind"].as_str()) {
                    (Some("constantBuffer"), Some("constantBuffer")) => Kind::Constants,
                    (Some("samplerState"), Some("samplerState")) => Kind::Sampler,
                    (Some("shaderResource"), Some("resource")) => {
                        texture();
                        Kind::Sampled
                    }
                    (Some("unorderedAccess"), Some("resource")) => {
                        texture();
                        assert_eq!(ty["access"], "readWrite", "{}: `{name}`", pass.name);
                        Kind::Storage
                    }
                    other => panic!(
                        "{}: `{name}` has an unexpected binding {other:?}",
                        pass.name
                    ),
                };
                let index = u32::try_from(binding["index"].as_u64().unwrap()).unwrap();
                actual.insert(name, (index, kind));
            }
            let expected: BTreeMap<String, (u32, Kind)> = pass
                .bindings
                .iter()
                .map(|b| (b.name.to_owned(), (b.binding, b.kind)))
                .collect();
            assert_eq!(actual, expected, "{}: reflection vs table", pass.name);
            assert!(
                json["entryPoints"][0]["parameters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .all(|parameter| parameter.get("binding").is_none()),
                "{}: uniform entry-point parameter",
                pass.name
            );
        }
    }

    /// The bindings the SPIR-V decorates are exactly the table's used ones, all in set 0, and
    /// the entry point `CS` runs the table's workgroup size.
    #[test]
    fn spirv_uses_exactly_the_used_bindings() {
        const OP_ENTRY_POINT: u32 = 15;
        const OP_EXECUTION_MODE: u32 = 16;
        const OP_DECORATE: u32 = 71;
        const LOCAL_SIZE: u32 = 17;
        const BINDING: u32 = 33;
        const DESCRIPTOR_SET: u32 = 34;
        for pass in PASSES {
            let mut bindings = BTreeSet::new();
            let mut sets = BTreeSet::new();
            let mut workgroup = None;
            let mut entry = None;
            instructions(pass.spirv, |opcode, operands| match opcode {
                OP_DECORATE if operands[1] == BINDING => {
                    bindings.insert(operands[2]);
                }
                OP_DECORATE if operands[1] == DESCRIPTOR_SET => {
                    sets.insert(operands[2]);
                }
                OP_EXECUTION_MODE if operands[1] == LOCAL_SIZE => {
                    workgroup = Some([operands[2], operands[3], operands[4]]);
                }
                OP_ENTRY_POINT => {
                    let name: Vec<u8> = operands[2..]
                        .iter()
                        .flat_map(|word| word.to_le_bytes())
                        .take_while(|byte| *byte != 0)
                        .collect();
                    entry = Some(String::from_utf8(name).unwrap());
                }
                _ => {}
            });
            let used: BTreeSet<u32> = pass
                .bindings
                .iter()
                .filter(|b| b.used)
                .map(|b| b.binding)
                .collect();
            assert_eq!(bindings, used, "{}: SPIR-V bindings vs used", pass.name);
            assert_eq!(sets, BTreeSet::from([0]), "{}: descriptor sets", pass.name);
            assert_eq!(workgroup, Some(pass.workgroup), "{}: workgroup", pass.name);
            assert_eq!(entry.as_deref(), Some("CS"), "{}: entry point", pass.name);
        }
    }

    /// The constant buffers have the sizes the renderer's mirrors assume.
    #[test]
    fn constant_buffer_sizes() {
        let size = |pass: &Pass, name: &str| -> u64 {
            let json: serde_json::Value =
                serde_json::from_str(reflection(pass.name).unwrap()).unwrap();
            let parameter = json["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|parameter| parameter["name"] == name)
                .unwrap()
                .clone();
            parameter["type"]["elementType"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| {
                    field["binding"]["offset"].as_u64().unwrap()
                        + field["binding"]["size"].as_u64().unwrap()
                })
                .max()
                .unwrap()
        };
        assert_eq!(
            size(&ACCUMULATE, "cbFSR3Upscaler"),
            CB_FSR3_UPSCALER_SIZE as u64
        );
        assert_eq!(size(&LUMA_PYRAMID, "cbSPD"), CB_SPD_SIZE as u64);
        assert_eq!(size(&RCAS, "cbRCAS"), CB_RCAS_SIZE as u64);
    }
}
