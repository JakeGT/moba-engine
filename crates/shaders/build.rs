//! Build-time GLSL and Slang → SPIR-V compilation.
//!
//! **GLSL** (every existing pass, all of Compat) compiles with the system `glslc` (Vulkan SDK)
//! and falls back to `glslangValidator`. Both are standard Vulkan toolchain components.
//! `spirv-val` is run on every GLSL module when it is available, so malformed shaders fail the
//! build rather than the driver. Override the compiler with `GLSLC=/path/to/glslc`.
//!
//! **Slang** (new tiered passes, ADR-0020 §4) compiles every `shaders/slang/*.slang` file with
//! the `slangc` pinned in `slang-version.txt`, to `<file>.slang.spv` next to the GLSL modules.
//! Each file is one module holding all of its `[shader(...)]` entry points under their source
//! names. Shared Slang modules live in `shaders/slang/lib/` (`import`ed, never compiled on
//! their own). The rules:
//!
//! - **Compiler lookup:** `SLANGC`, then `tools/slang-<version>/bin` in the Cargo target
//!   directory (where `scripts/fetch-slang.sh` / `.ps1` unpack it), then `PATH`.
//! - **Exact version:** `slangc -version` must report the pin. `MOBA_SLANG_ANY_VERSION=1`
//!   accepts any version with a warning (for distribution packagers).
//! - **Flags:** `-target spirv -profile spirv_1_6` (SPIR-V 1.6, the Vulkan 1.3 core version),
//!   column-major matrices, `-O2`, warnings as errors. Resources declare explicit
//!   `[[vk::binding(binding, set)]]`; a unit test checks every module's reflected bindings
//!   against a table, which catches drift (`slangc` has no flag that rejects implicit ones).
//! - **Validation:** `spirv-val --target-env vulkan1.3` is **mandatory** for Slang modules: a
//!   missing validator is a build error.
//! - **Reflection:** `slangc -reflection-json` is written next to each module as
//!   `<file>.slang.json`; the layout tests compare it with the Rust `#[repr(C)]` mirrors.
//! - **No network access:** this script never downloads anything.
//!
//! **FSR 3.1.6** (feature `fsr3`, ADR-0020 §6a and Q9, ADR-0021 §4b): the AMD FidelityFX
//! upscaler's MIT HLSL passes, vendored unmodified under `third_party/fidelityfx/`, compile
//! with the same pinned `slangc` to `fsr3_<pass>.spv` (see [`FSR3_PASSES`] and
//! `docs/dev/fsr3.md`). They differ from our own modules in four flags only: `-lang slang`
//! (Slang's HLSL front end applies the legacy for-loop scope, which two of the passes do not
//! compile under), the subgroup quad capability (the SPD passes), DXC-style register shifts
//! for explicit bindings, and two warnings that the vendor code trips disabled.
//!
//! - **Includes:** shaders `#include "name.glsl"` (with
//!   `#extension GL_GOOGLE_include_directive : require`) from `shaders/include/`, which holds
//!   the shared camera block, instance buffer, lighting and material declarations. Include
//!   files are never compiled on their own.
//! - **Permutations:** [`PERMUTATIONS`] lists a small, enumerated set of extra builds of a
//!   source with preprocessor defines, each to its own SPIR-V module
//!   (`unit.frag` + `MATERIAL_TEXTURED` → `unit_textured.frag.spv`). The renderer builds a
//!   pipeline for every permutation at load time, so nothing compiles mid-match. A Slang
//!   permutation names its source as `slang/<file>.slang` and its output `<name>.slang`.

use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// One extra build of a shader source with preprocessor defines.
struct Permutation {
    /// Source file in `shaders/` (`slang/<file>.slang` for a Slang module).
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
    // Depth pre-pass (High and up) and inverted-hull outlines (Low/Medium), R2b/ADR-0016.
    Permutation {
        source: "unit.vert",
        output: "unit_prepass.vert",
        defines: &["DEPTH_PREPASS"],
    },
    Permutation {
        source: "skinned.vert",
        output: "skinned_prepass.vert",
        defines: &["DEPTH_PREPASS"],
    },
    Permutation {
        source: "unit.vert",
        output: "unit_outline.vert",
        defines: &["OUTLINE_HULL"],
    },
    Permutation {
        source: "skinned.vert",
        output: "skinned_outline.vert",
        defines: &["OUTLINE_HULL"],
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
    // VAT depth pre-pass and inverted-hull outline (R2b, ADR-0016).
    Permutation {
        source: "vat.vert",
        output: "vat_prepass.vert",
        defines: &["DEPTH_PREPASS"],
    },
    Permutation {
        source: "vat.vert",
        output: "vat_outline.vert",
        defines: &["OUTLINE_HULL"],
    },
    // The terrain with the decal code compiled out (`MOBA_DECALS=off`, ADR-0017 §1): saves
    // its fixed cost, and is the baseline `scripts/terrain-decal-check.sh` times against.
    Permutation {
        source: "terrain.frag",
        output: "terrain_nodecals.frag",
        defines: &["MOBA_NO_DECALS"],
    },
    // Water with screen-space refraction (High and up, ADR-0017).
    Permutation {
        source: "water.frag",
        output: "water_refraction.frag",
        defines: &["WATER_REFRACTION"],
    },
];

/// The pinned Slang version and per-host archive checksums (ADR-0020 §4c).
const SLANG_PIN: &str = "slang-version.txt";

/// The vendored FidelityFX tree (`fsr3` feature), relative to this crate.
const FIDELITYFX: &str = "../../third_party/fidelityfx/Kits/FidelityFX";

/// One FSR 3.1.6 upscaler pass: the vendored entry file, the module it becomes
/// (`<output>.spv`) and the defines beyond [`FSR3_DEFINES`].
struct VendorPass {
    source: &'static str,
    output: &'static str,
    defines: &'static [&'static str],
}

/// The passes of one FSR 3.1.6 upscale, in dispatch order (`fsr3upscalerDispatch` in the
/// SDK's `ffx_fsr3upscaler.cpp`), plus the sharpening variant of the accumulate pass.
/// `moba_shaders::fsr3::PASSES` lists the same modules with their binding tables.
const FSR3_PASSES: &[VendorPass] = &[
    VendorPass {
        source: "ffx_fsr3upscaler_prepare_inputs_pass.hlsl",
        output: "fsr3_prepare_inputs",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_luma_pyramid_pass.hlsl",
        output: "fsr3_luma_pyramid",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_shading_change_pyramid_pass.hlsl",
        output: "fsr3_shading_change_pyramid",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_shading_change_pass.hlsl",
        output: "fsr3_shading_change",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_prepare_reactivity_pass.hlsl",
        output: "fsr3_prepare_reactivity",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_luma_instability_pass.hlsl",
        output: "fsr3_luma_instability",
        defines: &[],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_accumulate_pass.hlsl",
        output: "fsr3_accumulate",
        defines: &["FFX_FSR3UPSCALER_OPTION_APPLY_SHARPENING=0"],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_accumulate_pass.hlsl",
        output: "fsr3_accumulate_sharpen",
        defines: &["FFX_FSR3UPSCALER_OPTION_APPLY_SHARPENING=1"],
    },
    VendorPass {
        source: "ffx_fsr3upscaler_rcas_pass.hlsl",
        output: "fsr3_rcas",
        defines: &[],
    },
];

/// Every FSR pass's permutation: HLSL-style GPU code, 32-bit math (no `FFX_HALF`), linear HDR
/// colour, render-resolution motion vectors without jitter, non-inverted depth (ADR-0021 §2a),
/// and the reference Lanczos kernels (the SDK's choice on every device but 32/64-wide AMD
/// ones, where it takes a lookup table instead). `FFX_HLSL_SM` and
/// `FFX_IMPLICIT_SHADER_REGISTER_BINDING_HLSL` are what the SDK's own build defines.
const FSR3_DEFINES: &[&str] = &[
    "FFX_GPU=1",
    "FFX_HLSL=1",
    "FFX_HLSL_SM=62",
    "FFX_IMPLICIT_SHADER_REGISTER_BINDING_HLSL=0",
    "FFX_HALF=0",
    "FFX_FSR3UPSCALER_OPTION_HDR_COLOR_INPUT=1",
    "FFX_FSR3UPSCALER_OPTION_LOW_RESOLUTION_MOTION_VECTORS=1",
    "FFX_FSR3UPSCALER_OPTION_JITTERED_MOTION_VECTORS=0",
    "FFX_FSR3UPSCALER_OPTION_INVERTED_DEPTH=0",
    "FFX_FSR3UPSCALER_OPTION_REPROJECT_USE_LANCZOS_TYPE=0",
];

/// Register shifts that turn the vendor HLSL's `register(bN)`, `tN`, `uN` and `sN` into
/// explicit Vulkan bindings of set 0: constant buffers from 0, sampled images from 16,
/// storage images from 32 and samplers from 48 (no pass declares more than 16 of a kind).
const FSR3_SHIFTS: &[(&str, &str)] = &[
    ("-fvk-b-shift", "0"),
    ("-fvk-t-shift", "16"),
    ("-fvk-u-shift", "32"),
    ("-fvk-s-shift", "48"),
];

/// One compile: a source, its output module and its defines.
struct Job {
    source: PathBuf,
    output: PathBuf,
    defines: &'static [&'static str],
}

#[allow(clippy::too_many_lines)] // Three kinds of job, the tools, then the run, in order.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A directory watch covers every file below it, including `shaders/include/` and
    // `shaders/slang/lib/`.
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed={SLANG_PIN}");
    // Switching compilers rebuilds the shaders.
    for variable in ["GLSLC", "SLANGC", "MOBA_SLANG_ANY_VERSION"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let fsr3 = env::var_os("CARGO_FEATURE_FSR3").is_some();

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let shader_dir = manifest_dir.join("shaders");
    let include_dir = shader_dir.join("include");
    let slang_dir = shader_dir.join("slang");
    let slang_lib_dir = slang_dir.join("lib");
    let fidelityfx = manifest_dir.join(FIDELITYFX);
    let fsr3_jobs = if fsr3 {
        fsr3_jobs(&fidelityfx, &out_dir)
    } else {
        Vec::new()
    };

    let shaders = sources(&shader_dir, &["vert", "frag", "comp"]);
    assert!(!shaders.is_empty(), "no GLSL shaders found in shaders/");
    let slang_sources = sources(&slang_dir, &["slang"]);

    let mut glsl_jobs: Vec<Job> = shaders
        .iter()
        .map(|shader| Job {
            source: shader.clone(),
            output: output_path(shader, &out_dir),
            defines: &[],
        })
        .collect();
    let mut slang_jobs: Vec<Job> = slang_sources
        .iter()
        .map(|source| Job {
            source: source.clone(),
            output: output_path(source, &out_dir),
            defines: &[],
        })
        .collect();
    for permutation in PERMUTATIONS {
        let source = shader_dir.join(permutation.source);
        let slang = Path::new(permutation.source)
            .extension()
            .is_some_and(|extension| extension == "slang");
        let known = if slang { &slang_sources } else { &shaders };
        assert!(
            known.contains(&source),
            "permutation {} names a missing source {}",
            permutation.output,
            permutation.source
        );
        let job = Job {
            source,
            output: out_dir.join(format!("{}.spv", permutation.output)),
            defines: permutation.defines,
        };
        if slang {
            slang_jobs.push(job);
        } else {
            glsl_jobs.push(job);
        }
    }

    // Find every tool before compiling anything, so a missing one is the first error.
    let compiler = find_compiler();
    let validator = find_program("spirv-val");
    let slang = (!slang_jobs.is_empty() || !fsr3_jobs.is_empty()).then(|| {
        let version = pinned_slang_version(&manifest_dir.join(SLANG_PIN));
        let slangc = find_slangc(&out_dir, &version);
        // Re-fetching or replacing the compiler in place rebuilds the modules.
        println!("cargo:rerun-if-changed={}", slangc.display());
        check_slang_version(&slangc, &version);
        let validator = validator.clone().unwrap_or_else(|| {
            panic!(
                "spirv-val not found on PATH. It is mandatory for the Slang modules \
                 (ADR-0020 §4c). Install SPIRV-Tools: `spirv-tools` (apt or brew) or the \
                 Vulkan SDK"
            )
        });
        (slangc, validator)
    });

    // Each compile and validation is its own process, so run them in parallel. A failing
    // job panics in its thread and `thread::scope` re-raises the panic.
    let mut work: Vec<Box<dyn Fn() + Sync + '_>> = Vec::new();
    for job in &glsl_jobs {
        let (compiler, include_dir, validator) = (&compiler, &include_dir, &validator);
        work.push(Box::new(move || {
            compile(compiler, job, include_dir);
            if let Some(validator) = validator {
                validate(validator, &job.output);
            }
        }));
    }
    if let Some((slangc, validator)) = &slang {
        for job in &slang_jobs {
            let lib_dir = &slang_lib_dir;
            work.push(Box::new(move || {
                compile_slang(slangc, job, lib_dir);
                validate(validator, &job.output);
            }));
        }
        for job in &fsr3_jobs {
            let include = fidelityfx.join("upscalers/fsr3/include/gpu");
            work.push(Box::new(move || {
                compile_fsr3(slangc, job, &include);
                validate(validator, &job.output);
            }));
        }
    }
    run_parallel(&work);
    write_reflection_table(
        &slang_jobs,
        &out_dir,
        "slang_reflection.rs",
        "SLANG_REFLECTION",
    );
    if fsr3 {
        write_reflection_table(
            &fsr3_jobs,
            &out_dir,
            "fsr3_reflection.rs",
            "FSR3_REFLECTION",
        );
    }
}

/// Runs every item of `work` on up to Cargo's `NUM_JOBS` threads (`-j`).
fn run_parallel(work: &[Box<dyn Fn() + Sync + '_>]) {
    let next = AtomicUsize::new(0);
    let threads = env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<usize>().ok())
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, usize::from))
        .clamp(1, work.len().max(1));
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(item) = work.get(next.fetch_add(1, Ordering::Relaxed)) {
                    item();
                }
            });
        }
    });
}

/// The FSR 3.1.6 passes ([`FSR3_PASSES`]) under the vendored tree `fidelityfx`, which a
/// change anywhere in rebuilds.
fn fsr3_jobs(fidelityfx: &Path, out_dir: &Path) -> Vec<Job> {
    println!("cargo:rerun-if-changed={}", fidelityfx.display());
    FSR3_PASSES
        .iter()
        .map(|pass| Job {
            source: fidelityfx
                .join("upscalers/fsr3/internal/shaders")
                .join(pass.source),
            output: out_dir.join(format!("{}.spv", pass.output)),
            defines: pass.defines,
        })
        .collect()
}

/// `file` (`slang_reflection.rs`, or `fsr3_reflection.rs` for the FSR passes): `(module name,
/// reflection JSON)` for every module of `jobs`, as the constant `name`, for
/// `moba_shaders::reflection` and the crate's binding tests.
fn write_reflection_table(jobs: &[Job], out_dir: &Path, file: &str, name: &str) {
    let mut table = format!(
        "/// `(module name, slangc -reflection-json)` for every module (generated).\n\
         const {name}: &[(&str, &str)] = &[\n",
    );
    for job in jobs {
        let module = job.output.file_stem().and_then(|stem| stem.to_str());
        let json = job.output.with_extension("json");
        let json = json.file_name().and_then(|name| name.to_str());
        let (Some(module), Some(json)) = (module, json) else {
            panic!("non-UTF-8 Slang module name {}", job.output.display());
        };
        writeln!(
            table,
            "    ({module:?}, include_str!(concat!(env!(\"OUT_DIR\"), \"/{json}\"))),"
        )
        .expect("writing to a String cannot fail");
    }
    table.push_str("];\n");
    let path = out_dir.join(file);
    fs::write(&path, table)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// The files directly in `dir` with one of `extensions`, sorted. A missing directory has none.
fn sources(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sources: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extensions.contains(&extension))
        })
        .collect();
    sources.sort();
    sources
}

/// The `version=` line of `slang-version.txt`.
fn pinned_slang_version(pin: &Path) -> String {
    let text = fs::read_to_string(pin)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", pin.display()));
    text.lines()
        .find_map(|line| line.trim().strip_prefix("version="))
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| panic!("{} has no `version=` line", pin.display()))
}

/// How far above `OUT_DIR` the target directory can be:
/// `<target>/[<triple>/]<profile>/build/<package>-<hash>/out`.
const MAX_TARGET_DEPTH: usize = 5;

/// `SLANGC`, then `tools/slang-<version>/bin/slangc` in the Cargo target directory, then
/// `PATH`. Never downloads.
///
/// The target directory is found by walking up from `OUT_DIR`, so `CARGO_TARGET_DIR` and
/// `build.target-dir` both work. The walk looks at most [`MAX_TARGET_DEPTH`] levels up and
/// stops at the outermost ancestor in that range with a `CACHEDIR.TAG` (Cargo and the fetch
/// scripts write one). The outermost, because `cargo build --target <triple>` also tags
/// `<target>/<triple>/`.
fn find_slangc(out_dir: &Path, version: &str) -> PathBuf {
    if let Some(override_path) = env::var_os("SLANGC") {
        let path = PathBuf::from(&override_path);
        if path.is_file() {
            return path;
        }
        // A bare name (`SLANGC=slangc`) is looked up on `PATH`.
        if path.components().count() == 1
            && let Some(found) = override_path.to_str().and_then(find_program)
        {
            println!("cargo:rerun-if-env-changed=PATH");
            return found;
        }
        panic!(
            "SLANGC points at a missing file (or a name not on PATH): {}",
            path.display()
        );
    }
    let fetched = Path::new("tools")
        .join(format!("slang-{version}"))
        .join("bin")
        .join(format!("slangc{}", env::consts::EXE_SUFFIX));
    let ancestors: Vec<&Path> = out_dir.ancestors().take(MAX_TARGET_DEPTH + 1).collect();
    let target = ancestors
        .iter()
        .rposition(|dir| dir.join("CACHEDIR.TAG").is_file())
        .unwrap_or(ancestors.len() - 1);
    for dir in &ancestors[..=target] {
        let candidate = dir.join(&fetched);
        if candidate.is_file() {
            return candidate;
        }
    }
    if let Some(path) = find_program("slangc") {
        // Pointing PATH at another slangc rebuilds the modules.
        println!("cargo:rerun-if-env-changed=PATH");
        return path;
    }
    panic!(
        "slangc {version} not found (ADR-0020 §4c). Run `scripts/fetch-slang.sh` once from the \
         repository root (Windows: `scripts/fetch-slang.ps1`): it downloads the pinned \
         compiler into target/{}. Or set SLANGC=/path/to/slangc (version {version})",
        fetched.display()
    );
}

/// `slangc -version` must report exactly the pinned `version`, unless
/// `MOBA_SLANG_ANY_VERSION=1`.
fn check_slang_version(slangc: &Path, version: &str) {
    let result = Command::new(slangc)
        .arg("-version")
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", slangc.display()));
    let mut text = String::from_utf8_lossy(&result.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&result.stderr));
    // A compiler that cannot even report its version is broken, whatever the override says.
    assert!(
        result.status.success(),
        "failed to run `{} -version` ({}):\n{text}",
        slangc.display(),
        result.status
    );
    let reported = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .trim_start_matches('v')
        .to_owned();
    if reported == version {
        return;
    }
    let message = format!(
        "{} reports Slang version `{reported}`, but crates/shaders/{SLANG_PIN} pins {version}",
        slangc.display()
    );
    if env::var_os("MOBA_SLANG_ANY_VERSION").is_some_and(|value| value == "1") {
        println!("cargo:warning={message}; accepted because MOBA_SLANG_ANY_VERSION=1");
        return;
    }
    panic!(
        "{message}. Run `scripts/fetch-slang.sh` (Windows: `scripts/fetch-slang.ps1`) to fetch \
         the pinned compiler, point SLANGC at it, or set MOBA_SLANG_ANY_VERSION=1 to accept \
         another version (packagers only)"
    );
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

fn compile(compiler: &Compiler, job: &Job, include_dir: &Path) {
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
    for define in job.defines {
        command.arg(format!("-D{define}"));
    }
    command.arg("-o").arg(&job.output).arg(&job.source);
    run(&mut command, compiler.path(), job);
}

/// One Slang module: SPIR-V 1.6 for Vulkan 1.3, column-major, `-O2`, every entry point
/// under its source name, warnings as errors, and the reflection JSON next to it.
fn compile_slang(slangc: &Path, job: &Job, lib_dir: &Path) {
    let mut command = Command::new(slangc);
    command
        .arg(&job.source)
        .args(["-target", "spirv", "-profile", "spirv_1_6"])
        .args([
            "-matrix-layout-column-major",
            "-O2",
            "-fvk-use-entrypoint-name",
        ])
        .args(["-warnings-as-errors", "all"])
        .arg("-I")
        .arg(lib_dir);
    for define in job.defines {
        command.arg(format!("-D{define}"));
    }
    command
        .arg("-o")
        .arg(&job.output)
        .arg("-reflection-json")
        .arg(job.output.with_extension("json"));
    run(&mut command, slangc, job);
}

/// One vendored FSR 3.1.6 pass (`fsr3` feature): the vendor HLSL compiled with the flags of
/// [`compile_slang`] plus the Slang language front end, the quad-subgroup capability, explicit
/// register shifts ([`FSR3_SHIFTS`]), [`FSR3_DEFINES`], and two warnings off for the vendor
/// code: 15205 (an undefined macro in `#if`, which the vendor headers rely on for their
/// console paths) and 41000 (unreachable code after a commented-out return in `ffx_spd.h`).
/// The entry point is the SDK's `CS`.
fn compile_fsr3(slangc: &Path, job: &Job, include: &Path) {
    let mut command = Command::new(slangc);
    command
        .args(["-lang", "slang"])
        .arg(&job.source)
        .args(["-entry", "CS", "-stage", "compute"])
        .args(["-target", "spirv", "-profile", "spirv_1_6"])
        .args(["-capability", "spvGroupNonUniformQuad"])
        .args([
            "-matrix-layout-column-major",
            "-O2",
            "-fvk-use-entrypoint-name",
        ])
        .args(["-warnings-as-errors", "all"])
        .args(["-warnings-disable", "15205,41000"])
        .arg("-I")
        .arg(include);
    for (flag, shift) in FSR3_SHIFTS {
        command.args([flag, shift, "0"]);
    }
    for define in FSR3_DEFINES.iter().chain(job.defines) {
        command.arg(format!("-D{define}"));
    }
    command
        .arg("-o")
        .arg(&job.output)
        .arg("-reflection-json")
        .arg(job.output.with_extension("json"));
    run(&mut command, slangc, job);
}

fn run(command: &mut Command, program: &Path, job: &Job) {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", program.display()));
    assert!(
        result.status.success(),
        "shader compilation failed for {} (defines {:?}):\n{}{}",
        job.source.display(),
        job.defines,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rerun-if-changed={}", job.source.display());
}

fn validate(validator: &Path, module: &Path) {
    let status: ExitStatus = Command::new(validator)
        .args(["--target-env", "vulkan1.3"])
        .arg(module)
        .status()
        .unwrap_or_else(|error| panic!("failed to run spirv-val: {error}"));
    assert!(status.success(), "spirv-val rejected {}", module.display());
}
