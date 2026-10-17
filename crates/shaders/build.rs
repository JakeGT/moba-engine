//! Build-time GLSL → SPIR-V compilation.
//!
//! We compile with the system `glslc` (Vulkan SDK) and fall back to `glslangValidator`.
//! Both are standard Vulkan toolchain components. `spirv-val` is run on every module when
//! it is available, so malformed shaders fail the build rather than the driver.
//!
//! Override the compiler with `GLSLC=/path/to/glslc`.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shaders");

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let shader_dir = manifest_dir.join("shaders");

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

    for shader in &shaders {
        compile(&compiler, shader, &out_dir);
        let output = output_path(shader, &out_dir);
        if let Some(validator) = &validator {
            validate(validator, &output);
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

fn compile(compiler: &Compiler, shader: &Path, out_dir: &Path) {
    let output = output_path(shader, out_dir);
    let mut command = Command::new(compiler.path());
    match compiler {
        Compiler::Glslc(_) => {
            command.arg("--target-env=vulkan1.2").arg("-O");
        }
        Compiler::Glslang(_) => {
            command.arg("-V").arg("--target-env").arg("vulkan1.2");
        }
    }
    command.arg("-o").arg(&output).arg(shader);
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", compiler.path().display()));
    assert!(
        result.status.success(),
        "shader compilation failed for {}:\n{}",
        shader.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rerun-if-changed={}", shader.display());
}

fn validate(validator: &Path, module: &Path) {
    let status: ExitStatus = Command::new(validator)
        .arg(module)
        .status()
        .unwrap_or_else(|error| panic!("failed to run spirv-val: {error}"));
    assert!(status.success(), "spirv-val rejected {}", module.display());
}
