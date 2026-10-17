//! GLSL shader sources, compiled to SPIR-V at build time.
//!
//! The build script uses `glslc` (or `glslangValidator`) and validates every module with
//! `spirv-val` when it is installed. The renderer consumes the byte slices below and never
//! ships source code to the driver.

#![forbid(unsafe_code)]

/// Instanced unit vertex shader.
pub const UNIT_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/unit.vert.spv"));
/// Instanced unit fragment shader.
pub const UNIT_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/unit.frag.spv"));
/// Procedural terrain vertex shader.
pub const TERRAIN_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.vert.spv"));
/// Procedural terrain fragment shader.
pub const TERRAIN_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terrain.frag.spv"));
/// Screen-space UI vertex shader.
pub const UI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.vert.spv"));
/// Screen-space UI fragment shader.
pub const UI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.frag.spv"));

/// A named SPIR-V module, for tooling and validation.
#[derive(Clone, Copy, Debug)]
pub struct ShaderModule {
    /// Human-readable name.
    pub name: &'static str,
    /// SPIR-V bytes.
    pub spirv: &'static [u8],
}

/// Every shader module in the crate.
pub const MODULES: &[ShaderModule] = &[
    ShaderModule {
        name: "unit.vert",
        spirv: UNIT_VERT,
    },
    ShaderModule {
        name: "unit.frag",
        spirv: UNIT_FRAG,
    },
    ShaderModule {
        name: "terrain.vert",
        spirv: TERRAIN_VERT,
    },
    ShaderModule {
        name: "terrain.frag",
        spirv: TERRAIN_FRAG,
    },
    ShaderModule {
        name: "ui.vert",
        spirv: UI_VERT,
    },
    ShaderModule {
        name: "ui.frag",
        spirv: UI_FRAG,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const SPIRV_MAGIC: u32 = 0x0723_0203;

    #[test]
    fn modules_are_well_formed_spirv() {
        for module in MODULES {
            assert!(!module.spirv.is_empty(), "{} is empty", module.name);
            assert_eq!(
                module.spirv.len() % 4,
                0,
                "{} is not word aligned",
                module.name
            );
            let magic = u32::from_le_bytes(module.spirv[..4].try_into().unwrap());
            assert_eq!(magic, SPIRV_MAGIC, "{} has a bad magic number", module.name);
        }
    }
}
