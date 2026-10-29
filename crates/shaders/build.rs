//! Build-time GLSL → SPIR-V compilation.
//!
//! We compile with the system `glslc` (Vulkan SDK) and fall back to `glslangValidator`.
//! Both are standard Vulkan toolchain components. `spirv-val` is run on every module when
//! it is available, so malformed shaders fail the build rather than the driver.
//!
//! Override the compiler with `GLSLC=/path/to/glslc`.
//!
//! - **Includes:** shaders `#include "name.glsl"` (with
//!   `#extension GL_GOOGLE_include_directive : require`) from `shaders/include/`, which holds
//!   the shared camera block, instance buffer, lighting and material declarations. Include
//!   files are never compiled on their own.
//! - **Permutations:** [`PERMUTATIONS`] lists a small, enumerated set of extra builds of a
//!   source with preprocessor defines, each to its own SPIR-V module
//!   (`unit.frag` + `MATERIAL_TEXTURED` → `unit_textured.frag.spv`). The renderer builds a
//!   pipeline for every permutation at load time, so nothing compiles mid-match.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

/// One extra build of a shader source with preprocessor defines.
struct Permutation {
    /// Source file in `shaders/`.
    source: &'static str,
    /// Output name; the module is written to `<output>.spv`.
    output: &'static str,
    /// `-D` defines (each `NAME` or `NAME=VALUE`).
    defines: &'static [&'static str],
}

/// Extra permutations beyond each source's default build. Keep this list short: every entry
/// is another module, and the renderer builds a pipeline per permutation at load time.
/// `moba_shaders::MODULES` must list every output (a unit test checks the names).
const PERMUTATIONS: &[Permutation] = &[
    Permutation {
        source: "unit.vert",
        output: "unit_textured.vert",
        defines: &["MATERIAL_TEXTURED"],
    },
    Permutation {
        source: "skinned.vert",
        output: "skinned_textured.vert",
        defines: &["MATERIAL_TEXTURED"],
    },
    Permutation {
        source: "unit.frag",
        output: "unit_textured.frag",
        defines: &["MATERIAL_TEXTURED"],
    },
    // Depth-only shadow casters (R2a, ADR-0011): no fragment shader.
    Permutation {
        source: "unit.vert",
        output: "unit_shadow.vert",
        defines: &["SHADOW_PASS"],
    },
    Permutation {
        source: "skinned.vert",
        output: "skinned_shadow.vert",
        defines: &["SHADOW_PASS"],
    },
    // Particles with weighted blended OIT outputs (Medium and up, ADR-0013).
    Permutation {
        source: "vfx.frag",
        output: "vfx_oit.frag",
        defines: &["VFX_OIT"],
    },
    // Vertex-animation-texture shadow caster (R4b, ADR-0012).
    Permutation {
        source: "vat.vert",
        output: "vat_shadow.vert",
        defines: &["SHADOW_PASS"],
    },
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A directory watch covers every file below it, including `shaders/include/`.
    println!("cargo:rerun-if-changed=shaders");

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let shader_dir = manifest_dir.join("shaders");
    let include_dir = shader_dir.join("include");

    let compiler = find_compiler();
    let mut shaders: Vec<PathBuf> = fs::read_dir(&shader_dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", shader_dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("vert" | "frag" | "comp")
            )
        })
        .collect();
    shaders.sort();

    assert!(!shaders.is_empty(), "no GLSL shaders found in shaders/");
    let validator = find_program("spirv-val");

    let mut jobs: Vec<(PathBuf, PathBuf, &[&str])> = shaders
        .iter()
        .map(|shader| (shader.clone(), output_path(shader, &out_dir), &[][..]))
        .collect();
    for permutation in PERMUTATIONS {
        let source = shader_dir.join(permutation.source);
        assert!(
            shaders.contains(&source),
            "permutation {} names a missing source {}",
            permutation.output,
            permutation.source
        );
        let output = out_dir.join(format!("{}.spv", permutation.output));
        jobs.push((source, output, permutation.defines));
    }
    for (source, output, defines) in &jobs {
        compile(&compiler, source, output, &include_dir, defines);
        if let Some(validator) = &validator {
            validate(validator, output);
        }
    }
}

enum Compiler {
    Glslc(PathBuf),
    Glslang(PathBuf),
}

impl Compiler {
    fn path(&self) -> &Path {
        match self {
            Self::Glslc(path) | Self::Glslang(path) => path,
        }
    }
}

fn find_compiler() -> Compiler {
    if let Some(override_path) = env::var_os("GLSLC") {
        let path = PathBuf::from(override_path);
        if path.exists() {
            return Compiler::Glslc(path);
        }
        panic!("GLSLC points at a missing file: {}", path.display());
    }
    if let Some(path) = find_program("glslc") {
        return Compiler::Glslc(path);
    }
    if let Some(path) = find_program("glslangValidator") {
        return Compiler::Glslang(path);
    }
    panic!(
        "no GLSL compiler found. Install the Vulkan SDK or `glslang-tools` \
         (provides glslc/glslangValidator), or set GLSLC=/path/to/glslc"
    );
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .flat_map(|dir| {
            let candidate = dir.join(name);
            let windows = dir.join(format!("{name}.exe"));
            [candidate, windows]
        })
        .find(|candidate| candidate.is_file())
}

fn output_path(shader: &Path, out_dir: &Path) -> PathBuf {
    let file_name = shader.file_name().expect("shader file name");
    let mut name = OsString::from(file_name);
    name.push(".spv");
    out_dir.join(name)
}

fn compile(
    compiler: &Compiler,
    shader: &Path,
    output: &Path,
    include_dir: &Path,
    defines: &[&str],
) {
    let mut command = Command::new(compiler.path());
    match compiler {
        Compiler::Glslc(_) => {
            command.arg("--target-env=vulkan1.3").arg("-O");
        }
        Compiler::Glslang(_) => {
            command.arg("-V").arg("--target-env").arg("vulkan1.3");
        }
    }
    // Both compilers take `-I<dir>` and `-D<name>` the same way.
    let mut include = OsString::from("-I");
    include.push(include_dir);
    command.arg(include);
    for define in defines {
        command.arg(format!("-D{define}"));
    }
    command.arg("-o").arg(output).arg(shader);
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", compiler.path().display()));
    assert!(
        result.status.success(),
        "shader compilation failed for {} (defines {defines:?}):\n{}{}",
        shader.display(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rerun-if-changed={}", shader.display());
}

fn validate(validator: &Path, module: &Path) {
    let status: ExitStatus = Command::new(validator)
        .args(["--target-env", "vulkan1.3"])
        .arg(module)
        .status()
        .unwrap_or_else(|error| panic!("failed to run spirv-val: {error}"));
    assert!(status.success(), "spirv-val rejected {}", module.display());
}
