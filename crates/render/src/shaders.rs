//! Where pipelines get their SPIR-V: the modules embedded by `moba-shaders`, or, in dev
//! builds, overrides from a directory ([`RenderConfig::shader_dir`](crate::RenderConfig)).
//!
//! An override is `<dir>/<module name>.spv` (for example `unit_textured.frag.spv`, the
//! names in `moba_shaders::MODULES`). It is used only if it is a regular file of at most
//! [`MAX_SHADER_BYTES`], word-aligned, with the SPIR-V magic; anything else falls back to the
//! embedded module with one log line. [`Renderer::reload_shaders`](crate::Renderer) rebuilds
//! the scene and composite pipelines from the current files, so a shader edit shows up
//! without restarting (hot reload).
//!
//! **Debug builds only** (`debug_assertions`). The checks above cannot prove that a module
//! matches the pipeline interface it is built into, and a mismatched module is invalid usage
//! of `vkCreateGraphicsPipelines` (undefined behaviour without the validation layer), so
//! release builds ignore the directory.

use std::io::Read;
use std::path::PathBuf;

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::resources::ShaderModule;

/// Largest override module accepted.
pub(crate) const MAX_SHADER_BYTES: u64 = 4 * 1024 * 1024;
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// The shader modules pipelines are built from.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShaderSource {
    dir: Option<PathBuf>,
}

impl ShaderSource {
    pub(crate) fn new(dir: Option<PathBuf>) -> Self {
        if !cfg!(debug_assertions)
            && let Some(dir) = &dir
        {
            eprintln!(
                "[render] shader overrides in {} ignored: a debug-build feature",
                dir.display()
            );
            return Self { dir: None };
        }
        Self { dir }
    }

    /// Whether an override directory is configured.
    pub(crate) fn has_overrides(&self) -> bool {
        self.dir.is_some()
    }

    /// The override bytes for `name`, if a valid one exists.
    fn override_bytes(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.dir.as_ref()?.join(format!("{name}.spv"));
        let metadata = std::fs::metadata(&path).ok()?;
        let problem = if !metadata.is_file() {
            Some("not a file")
        } else if metadata.len() > MAX_SHADER_BYTES {
            Some("too large")
        } else {
            None
        };
        if let Some(problem) = problem {
            eprintln!(
                "[render] shader override {}: {problem}; using the built-in module",
                path.display()
            );
            return None;
        }
        // `take` bounds the read even if the file grew after the metadata check.
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .ok()?
            .take(MAX_SHADER_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_SHADER_BYTES {
            return None;
        }
        let magic = bytes
            .get(..4)
            .and_then(|word| word.try_into().ok())
            .map(u32::from_le_bytes);
        if bytes.len() % 4 != 0 || magic != Some(SPIRV_MAGIC) {
            eprintln!(
                "[render] shader override {}: not SPIR-V; using the built-in module",
                path.display()
            );
            return None;
        }
        Some(bytes)
    }

    /// Creates the module `name` (an override if present, else the embedded one).
    pub(crate) fn module(&self, device: &DeviceContext, name: &str) -> RenderResult<ShaderModule> {
        if let Some(bytes) = self.override_bytes(name) {
            match ShaderModule::new(device.device.clone(), &bytes) {
                Ok(module) => return Ok(module),
                Err(error) => eprintln!(
                    "[render] shader override {name}: rejected ({error}); using the built-in module"
                ),
            }
        }
        let embedded =
            moba_shaders::module(name).ok_or(RenderError::Shader("unknown shader module"))?;
        ShaderModule::new(device.device.clone(), embedded)
    }
}
