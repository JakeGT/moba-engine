//! The vendor SDK half of moba-render's build script (ADR-0020 §6b).
//!
//! For each enabled feature it finds the SDK that `scripts/fetch-sdks.sh` unpacked into
//! `MOBA_SDK_DIR` (default `<workspace>/target/vendor-sdks`), checks the fetch's stamp against
//! the pin in `vendor-sdks.toml`, generates the FFI declarations with `bindgen` into `OUT_DIR`
//! and emits the link lines. A missing or stale SDK fails the build with the fetch command;
//! nothing is ever downloaded here.

use std::env;
use std::fmt::Display;
#[cfg(feature = "nrd")]
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "nrd")]
use std::process::Command;

/// The stamp the fetch scripts write: the archive's SHA-256.
const STAMP: &str = ".moba-sdk-sha256";

pub(crate) fn main() {
    let context = Context::new();
    #[cfg(feature = "dlss")]
    dlss(&context);
    #[cfg(feature = "streamline")]
    streamline(&context);
    #[cfg(feature = "nrd")]
    nrd(&context);
    #[cfg(feature = "rtxdi")]
    rtxdi(&context);
    #[cfg(feature = "sharc")]
    sharc(&context);
    #[cfg(feature = "fsr4")]
    fsr4(&context);
    #[cfg(feature = "xess")]
    xess(&context);
}

/// Fails the build script with `message` (cargo prints stderr of a failed script).
fn fail(message: impl Display) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

struct Context {
    sdk_dir: PathBuf,
    manifest: toml::Table,
    /// Read by every feature that writes bindings (all but `sharc`).
    #[cfg_attr(
        not(any(
            feature = "dlss",
            feature = "streamline",
            feature = "nrd",
            feature = "rtxdi",
            feature = "fsr4",
            feature = "xess"
        )),
        allow(dead_code, reason = "only `sharc` writes no bindings")
    )]
    out: PathBuf,
    #[cfg_attr(
        not(any(feature = "dlss", feature = "nrd")),
        allow(dead_code, reason = "only `dlss` and `nrd` link per target")
    )]
    target_os: String,
    #[cfg_attr(
        not(feature = "dlss"),
        allow(dead_code, reason = "only `dlss` picks a library per architecture")
    )]
    target_arch: String,
    #[cfg_attr(
        not(any(feature = "dlss", feature = "nrd")),
        allow(dead_code, reason = "only `dlss` and `nrd` link per target")
    )]
    target_vendor: String,
}

struct Sdk {
    dir: PathBuf,
    /// The pinned archive's SHA-256 (the stamp that was checked).
    #[cfg_attr(
        not(feature = "nrd"),
        allow(dead_code, reason = "only `nrd` keys a build on the pins")
    )]
    sha256: String,
}

impl Context {
    fn new() -> Self {
        println!("cargo:rerun-if-env-changed=MOBA_SDK_DIR");
        let crate_dir = PathBuf::from(env_var("CARGO_MANIFEST_DIR"));
        let root = crate_dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| fail("moba-render is expected at <workspace>/crates/render"))
            .to_path_buf();
        let manifest_path = root.join("vendor-sdks.toml");
        println!("cargo:rerun-if-changed={}", manifest_path.display());
        let text = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| fail(format!("{}: {error}", manifest_path.display())));
        let manifest: toml::Table = text
            .parse()
            .unwrap_or_else(|error| fail(format!("{}: {error}", manifest_path.display())));
        // A relative MOBA_SDK_DIR is resolved against the workspace root, as the fetch
        // scripts do (a build script runs in the crate's directory, not the caller's).
        let sdk_dir = env::var_os("MOBA_SDK_DIR")
            .filter(|dir| !dir.is_empty())
            .map_or_else(
                || root.join("target").join("vendor-sdks"),
                |dir| root.join(dir),
            );
        Self {
            sdk_dir,
            manifest,
            out: PathBuf::from(env_var("OUT_DIR")),
            target_os: env_var("CARGO_CFG_TARGET_OS"),
            target_arch: env_var("CARGO_CFG_TARGET_ARCH"),
            target_vendor: env_var("CARGO_CFG_TARGET_VENDOR"),
        }
    }

    /// The fetched, stamp-checked SDK of manifest entry `entry`, which `feature` needs.
    fn require(&self, entry: &str, feature: &str) -> Sdk {
        let sdks = self
            .manifest
            .get("sdk")
            .and_then(toml::Value::as_array)
            .unwrap_or_else(|| fail("vendor-sdks.toml has no [[sdk]] entries"));
        let sdk = sdks
            .iter()
            .filter_map(toml::Value::as_table)
            .find(|sdk| sdk.get("name").and_then(toml::Value::as_str) == Some(entry))
            .unwrap_or_else(|| fail(format!("vendor-sdks.toml has no entry `{entry}`")));
        let field = |key: &str| {
            sdk.get(key)
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| fail(format!("vendor-sdks.toml: `{entry}` has no `{key}`")))
        };
        let fetch = format!(
            "`scripts/fetch-sdks.sh --only {feature}` (Windows: `scripts\\fetch-sdks.ps1 -Only {feature}`)"
        );
        if field("status") != "verified" {
            fail(format!(
                "moba-render feature `{feature}` needs `{entry}`, which vendor-sdks.toml marks \
                 `{}`: it cannot be fetched automatically (see docs/dev/vendor-sdks.md)",
                field("status")
            ));
        }
        let dir = self.sdk_dir.join(field("dir"));
        let stamp = dir.join(STAMP);
        println!("cargo:rerun-if-changed={}", stamp.display());
        match fs::read_to_string(&stamp) {
            Ok(text) if text.trim() == field("sha256") => {}
            Ok(_) => fail(format!(
                "moba-render feature `{feature}`: {} is not the pinned `{entry}` {} (its stamp \
                 differs from vendor-sdks.toml). Re-fetch it with {fetch}.",
                dir.display(),
                field("version")
            )),
            Err(_) => fail(format!(
                "moba-render feature `{feature}` needs `{entry}` {} in {}, which is missing. \
                 Fetch it with {fetch}, or set MOBA_SDK_DIR to a directory that has it. Build \
                 scripts never download (ADR-0020 §6b).",
                field("version"),
                dir.display()
            )),
        }
        println!(
            "cargo:rustc-env=MOBA_SDK_{}_VERSION={}",
            entry.to_ascii_uppercase().replace('-', "_"),
            field("version")
        );
        Sdk {
            dir,
            sha256: field("sha256").to_owned(),
        }
    }

    /// Writes `builder`'s bindings to `OUT_DIR/<file>`.
    #[cfg(any(
        feature = "dlss",
        feature = "streamline",
        feature = "nrd",
        feature = "rtxdi",
        feature = "fsr4",
        feature = "xess"
    ))]
    fn write(&self, builder: bindgen::Builder, file: &str) {
        let bindings = builder.generate().unwrap_or_else(|error| {
            fail(format!(
                "bindgen could not generate {file}: {error} (the vendor features need libclang; \
                 set LIBCLANG_PATH if it is not found)"
            ))
        });
        bindings
            .write_to_file(self.out.join(file))
            .unwrap_or_else(|error| fail(format!("writing {file}: {error}")));
    }
}

fn env_var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| fail(format!("{name} is not set")))
}

/// A bindgen builder with the settings every SDK shares: Rust 2024 output (`unsafe extern`),
/// no layout tests, no copied comments, enums as plain constants.
#[cfg(any(
    feature = "dlss",
    feature = "streamline",
    feature = "nrd",
    feature = "rtxdi",
    feature = "fsr4",
    feature = "xess"
))]
fn bindings(name: &str, wrapper: &str, include: &Path) -> bindgen::Builder {
    let target = bindgen::RustTarget::stable(88, 0)
        .unwrap_or_else(|error| fail(format!("bindgen target: {error}")));
    bindgen::Builder::default()
        .header_contents(&format!("moba_{name}.h"), wrapper)
        .clang_arg(format!("-I{}", include.display()))
        .rust_target(target)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .formatter(bindgen::Formatter::None)
        .layout_tests(false)
        .generate_comments(false)
        .prepend_enum_name(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
}

/// The handful of Vulkan declarations NGX's and XeSS's headers need (`nvsdk_ngx_vk.h`
/// includes no Vulkan header; `xess_vk.h` includes `<vulkan/vulkan.h>`, which `xess` points
/// at a copy of this). Written from the Vulkan specification's ABI for 64-bit targets, where
/// every handle is pointer-sized; no system Vulkan headers are needed.
#[cfg(any(feature = "dlss", feature = "xess"))]
const VULKAN_DECLARATIONS: &str = "\
#ifndef MOBA_VULKAN_DECLARATIONS
#define MOBA_VULKAN_DECLARATIONS
#include <stdint.h>
typedef uint32_t VkFlags;
typedef VkFlags VkImageAspectFlags;
typedef VkFlags VkImageUsageFlags;
typedef VkFlags VkBufferUsageFlags;
typedef struct VkInstance_T *VkInstance;
typedef struct VkPhysicalDevice_T *VkPhysicalDevice;
typedef struct VkDevice_T *VkDevice;
typedef struct VkCommandBuffer_T *VkCommandBuffer;
typedef struct VkImage_T *VkImage;
typedef struct VkImageView_T *VkImageView;
typedef struct VkBuffer_T *VkBuffer;
typedef struct VkDeviceMemory_T *VkDeviceMemory;
typedef struct VkPipelineCache_T *VkPipelineCache;
typedef int32_t VkFormat;
typedef struct VkImageSubresourceRange {
    VkImageAspectFlags aspectMask;
    uint32_t baseMipLevel;
    uint32_t levelCount;
    uint32_t baseArrayLayer;
    uint32_t layerCount;
} VkImageSubresourceRange;
typedef struct VkExtensionProperties {
    char extensionName[256];
    uint32_t specVersion;
} VkExtensionProperties;
typedef void (*PFN_vkVoidFunction)(void);
typedef PFN_vkVoidFunction (*PFN_vkGetInstanceProcAddr)(VkInstance, const char *);
typedef PFN_vkVoidFunction (*PFN_vkGetDeviceProcAddr)(VkDevice, const char *);
#endif
";

/// DLSS through NGX: bindings for the extension-requirement queries, the Vulkan init,
/// capability query and shutdown, and the static NGX library (ADR-0020 §6b). NGX has no Apple
/// library, so on Apple targets the feature needs nothing here and compiles to a probe that
/// always reports DLSS unavailable.
#[cfg(feature = "dlss")]
fn dlss(context: &Context) {
    if context.target_vendor == "apple" {
        return;
    }
    let sdk = context.require("dlss", "dlss");
    let wrapper = format!("{VULKAN_DECLARATIONS}#include \"nvsdk_ngx_vk.h\"\n");
    let builder = bindings("ngx", &wrapper, &sdk.dir.join("include"))
        .clang_args(["-x", "c", "-std=c11"])
        .allowlist_function(
            "NVSDK_NGX_VULKAN_(Init_with_ProjectID|Shutdown1|GetCapabilityParameters|DestroyParameters|GetFeatureInstanceExtensionRequirements|GetFeatureDeviceExtensionRequirements)",
        )
        .allowlist_function("NVSDK_NGX_Parameter_GetI")
        .allowlist_var("NVSDK_NGX_Parameter_SuperSampling_(Available|NeedsUpdatedDriver|FeatureInitResult|MinDriverVersionMajor|MinDriverVersionMinor)")
        // Enum constants come with their (allowlisted) enum types.
        .allowlist_type("NVSDK_NGX_(Result|EngineType|Version|Feature|Application_Identifier_Type)");
    context.write(builder, "ngx.rs");

    let lib = sdk.dir.join("lib");
    match (context.target_os.as_str(), context.target_arch.as_str()) {
        ("linux", arch @ ("x86_64" | "aarch64")) => {
            let dir = lib.join(format!("Linux_{arch}"));
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=nvsdk_ngx");
            // The NGX static library is C++ and opens the driver's NGX core with dlopen.
            println!("cargo:rustc-link-lib=dylib=stdc++");
            println!("cargo:rustc-link-lib=dylib=dl");
        }
        ("windows", "x86_64") => {
            // The build matching Rust's C runtime: static (/MT, `_s`) with `crt-static`, else
            // dynamic (/MD, `_d`), as NRD's flags are chosen.
            let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
                .is_ok_and(|features| features.split(',').any(|f| f == "crt-static"));
            let dir = lib.join("Windows_x86_64").join("x64");
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!(
                "cargo:rustc-link-lib=static={}",
                if crt_static {
                    "nvsdk_ngx_s"
                } else {
                    "nvsdk_ngx_d"
                }
            );
        }
        (os, arch) => fail(format!(
            "moba-render feature `dlss`: DLSS SDK has no NGX library for {os}-{arch}"
        )),
    }
}

/// Streamline: its version macros only. The interposer is opened at run time (Windows).
#[cfg(feature = "streamline")]
fn streamline(context: &Context) {
    let sdk = context.require("streamline", "streamline");
    let builder = bindings(
        "streamline",
        "#include \"sl_version.h\"\n",
        &sdk.dir.join("include"),
    )
    .clang_args(["-x", "c++", "-std=c++17"])
    .allowlist_var("SL_VERSION_(MAJOR|MINOR|PATCH)");
    context.write(builder, "streamline.rs");
}

/// NRD: built from source with `cmake` as a static library, without network access (ADR-0020
/// §6b lists the flags), plus bindings for its library descriptor.
#[cfg(feature = "nrd")]
fn nrd(context: &Context) {
    let sdk = context.require("nrd", "nrd");
    let shadermake = context.require("nrd-shadermake", "nrd");
    let mathlib = context.require("nrd-mathlib", "nrd");
    let include = sdk.dir.join("Include");

    let builder = bindings("nrd", "#include \"NRD.h\"\n", &include)
        .clang_args(["-x", "c++", "-std=c++17"])
        // NRD's enum classes each have a `MAX_NUM`: keep their names apart.
        .prepend_enum_name(true)
        .allowlist_function("(nrd::)?GetLibraryDesc")
        .allowlist_type("(nrd::)?LibraryDesc")
        .allowlist_var("NRD_VERSION_(MAJOR|MINOR|BUILD)");
    context.write(builder, "nrd.rs");

    // NRD is built once per pin, target, profile and toolchain, not once per build script
    // run: OUT_DIR differs for every feature set and for clippy, and each NRD build compiles
    // its SPIR-V permutations with dxc. The build lives beside cargo's own `build` directory
    // (`<build-dir>/<profile>/vendor-build/nrd`; the build dir is the target dir unless
    // `CARGO_BUILD_BUILD_DIR` moves it), and a stamp written last records what it
    // was built from (the pins, the CMake and dxc versions, the recipe and the toolchain
    // environment), so an interrupted or outdated build is redone from scratch. Cargo holds
    // the build directory's lock while build scripts run, so two never share it at once.
    let build = shared_build_dir(context, "nrd");
    let lib_dir = build.join("lib");
    // On MSVC, the C runtime NRD links must match Rust's: `/MT` with `crt-static`, else `/MD`.
    let msvc_runtime = env::var("CARGO_CFG_TARGET_ENV")
        .is_ok_and(|env| env == "msvc")
        .then(|| {
            let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
                .is_ok_and(|features| features.split(',').any(|f| f == "crt-static"));
            if crt_static { "/MT" } else { "/MD" }
        });
    let defines = nrd_defines(
        &shadermake,
        &mathlib,
        &lib_dir,
        &build.join("shaders"),
        msvc_runtime,
    );
    let (cmake_version, dxc_version) = nrd_tools(context);
    let mut key = format!(
        "nrd {} shadermake {} mathlib {}\ncmake {cmake_version}\ndxc {dxc_version}\n\
         recipe: {NRD_RECIPE}",
        sdk.sha256, shadermake.sha256, mathlib.sha256
    );
    for (name, value) in &defines {
        let _ = write!(key, "\n-D{name}={value}");
    }
    // What cargo sets for this build, then what the `cmake` and `cc` crates read to pick the
    // generator and the C++ compiler and its flags (plain, per target and `TARGET_` forms).
    let target = env_var("TARGET");
    let mut vars: Vec<String> = ["TARGET", "PROFILE", "OPT_LEVEL", "DEBUG"]
        .map(String::from)
        .to_vec();
    let tracked = [
        "CMAKE",
        "VULKAN_SDK",
        "CMAKE_GENERATOR",
        "CMAKE_TOOLCHAIN_FILE",
        "CMAKE_PREFIX_PATH",
        "CC",
        "CXX",
        "CFLAGS",
        "CXXFLAGS",
        "AR",
    ];
    for var in tracked {
        vars.push(var.to_owned());
        if !var.starts_with("CMAKE") && var != "VULKAN_SDK" {
            vars.push(format!("{var}_{target}"));
            vars.push(format!("{var}_{}", target.replace('-', "_")));
            vars.push(format!("TARGET_{var}"));
        }
    }
    for var in &vars {
        if !matches!(var.as_str(), "TARGET" | "PROFILE" | "OPT_LEVEL" | "DEBUG") {
            println!("cargo:rerun-if-env-changed={var}");
        }
        let _ = write!(key, "\n{var}={}", env::var(var).unwrap_or_default());
    }
    let done = build.join(".moba-nrd-built");
    // A missing stamp (the directory deleted, or wiped by another feature set's rebuild)
    // reruns this script, so no OUT_DIR is left linking against libraries that are gone.
    println!("cargo:rerun-if-changed={}", done.display());
    if fs::read_to_string(&done).ok().as_deref() != Some(key.as_str()) {
        build_nrd(&build, &sdk, &defines);
        fs::write(&done, &key)
            .unwrap_or_else(|error| fail(format!("writing {}: {error}", done.display())));
        // Dated no later than this run's start (the build script's own modification time),
        // as cargo dates the script's output to its start: a newer stamp would make the
        // `rerun-if-changed` above rerun the script once more. Failing to set it only costs
        // that one rerun.
        if let Ok(built) = env::current_exe()
            .and_then(fs::metadata)
            .and_then(|metadata| metadata.modified())
        {
            let _ = fs::File::options()
                .write(true)
                .open(&done)
                .and_then(|file| file.set_modified(built));
        }
    }
    // Multi-config generators (Visual Studio) add a per-configuration directory.
    for dir in [lib_dir.clone(), lib_dir.join("Release")] {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    println!("cargo:rustc-link-lib=static=NRD");
    // NRD links ShaderMake's blob reader PRIVATE, which a static NRD does not absorb: its
    // shader lookup (`ShaderMake::FindPermutationInBlob`) comes from this archive.
    println!("cargo:rustc-link-lib=static=ShaderMakeBlob");
    // NRD is C++: libstdc++ on Linux and the other GNU-style targets, libc++ on Apple's
    // (MSVC links its C++ runtime itself).
    if context.target_vendor == "apple" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if context.target_os != "windows" {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

/// `<build-dir>/<profile>/vendor-build/<name>`, found from `OUT_DIR`
/// (`<build-dir>/<profile>/build/<package>-<hash>/out`); `OUT_DIR` itself when the layout is
/// not that one.
#[cfg(feature = "nrd")]
fn shared_build_dir(context: &Context, name: &str) -> PathBuf {
    let mut up = context.out.ancestors();
    let profile = match (up.nth(2), up.next()) {
        (Some(build), Some(profile)) if build.file_name().is_some_and(|n| n == "build") => {
            profile.to_path_buf()
        }
        _ => return context.out.join(name),
    };
    profile.join("vendor-build").join(name)
}

/// The `CMake` and `dxc` versions NRD is built with (their `--version` output), failing with
/// a plain message when either is missing: `CMake` 3.30+ (NRD's minimum) and the Vulkan SDK's
/// `dxc` (`SHADERMAKE_FIND_DXC_VK`) are build requirements of this feature only.
#[cfg(feature = "nrd")]
fn nrd_tools(context: &Context) -> (String, String) {
    let version = |program: &Path| {
        Command::new(program)
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            })
    };
    let cmake = env::var_os("CMAKE").map_or_else(|| PathBuf::from("cmake"), PathBuf::from);
    let cmake = version(&cmake).unwrap_or_else(|| {
        fail(
            "moba-render feature `nrd` builds NRD with CMake 3.30 or newer, which was not found \
             (set CMAKE to its path)",
        )
    });
    let dxc = if context.target_os == "windows" {
        env::var_os("VULKAN_SDK").map(|sdk| PathBuf::from(sdk).join("Bin").join("dxc.exe"))
    } else {
        Some(PathBuf::from("dxc"))
    };
    // It must run, not merely exist: on Linux the SDK's dxc needs its libdxcompiler.so on
    // the library path.
    let dxc = dxc.as_deref().and_then(version).unwrap_or_else(|| {
        fail(
            "moba-render feature `nrd` compiles NRD's shaders with the Vulkan SDK's dxc \
             (SHADERMAKE_FIND_DXC_VK), which was not found: on Windows install the Vulkan SDK \
             (VULKAN_SDK), elsewhere put dxc on PATH",
        )
    });
    (cmake, dxc)
}

/// What `build_nrd` does besides the definitions, recorded in the stamp so a change to it
/// rebuilds NRD.
#[cfg(feature = "nrd")]
const NRD_RECIPE: &str = "profile=Release build_target=NRD source_skip=_deps keep=lib";

/// The `CMake` definitions NRD is configured with (ADR-0020 §6b, amendment 6).
#[cfg(feature = "nrd")]
fn nrd_defines(
    shadermake: &Sdk,
    mathlib: &Sdk,
    lib_dir: &Path,
    shaders: &Path,
    msvc_runtime: Option<&str>,
) -> Vec<(&'static str, String)> {
    let path = |path: &Path| path.display().to_string();
    let on = |name: &'static str, on: bool| (name, String::from(if on { "ON" } else { "OFF" }));
    let mut defines = Vec::new();
    if let Some(runtime) = msvc_runtime {
        // The `cmake` crate sets the MSVC flag variables to `cc`'s flags, which replaces
        // CMake's defaults: without `/EHsc`, ShaderMake's and NRD's `/W4 /WX` turn C4530 into
        // an error, and without `/O2 /DNDEBUG` NRD would be an unoptimised build with
        // asserts. It leaves a variable alone once it is defined, so these restore CMake's
        // MSVC defaults, with Rust's C runtime (`runtime`).
        let flags = "/DWIN32 /D_WINDOWS";
        let release = format!("{runtime} /O2 /Ob2 /DNDEBUG");
        defines.extend([
            ("CMAKE_C_FLAGS", String::from(flags)),
            ("CMAKE_CXX_FLAGS", format!("{flags} /EHsc /GR")),
            ("CMAKE_C_FLAGS_RELEASE", release.clone()),
            ("CMAKE_CXX_FLAGS_RELEASE", release),
        ]);
    }
    defines.extend([
        // No downloads at configure time: the fetch script unpacked both dependencies.
        on("FETCHCONTENT_FULLY_DISCONNECTED", true),
        ("FETCHCONTENT_SOURCE_DIR_SHADERMAKE", path(&shadermake.dir)),
        ("FETCHCONTENT_SOURCE_DIR_MATHLIB", path(&mathlib.dir)),
        on("NRD_NRI", false),
        // ShaderMake's own compiler downloads stay off; its Vulkan-SDK DXC lookup stays on.
        on("SHADERMAKE_FIND_DXC", false),
        on("SHADERMAKE_FIND_FXC", false),
        on("SHADERMAKE_FIND_DXC_VK", true),
        // SPIR-V embeds only; the DXIL and DXBC embeds would need DXIL and FXC.
        on("NRD_EMBEDS_SPIRV_SHADERS", true),
        on("NRD_EMBEDS_DXIL_SHADERS", false),
        on("NRD_EMBEDS_DXBC_SHADERS", false),
        // Linked into moba-render, so nothing extra ships next to the executable.
        on("NRD_STATIC_LIBRARY", true),
        // Outputs stay out of NRD's source tree (_Bin, _Shaders). NRD's own archive goes to
        // the runtime directory (its ARCHIVE_OUTPUT_DIRECTORY); the archive directory
        // collects the static libraries it depends on (ShaderMakeBlob).
        ("CMAKE_RUNTIME_OUTPUT_DIRECTORY", path(lib_dir)),
        ("CMAKE_LIBRARY_OUTPUT_DIRECTORY", path(lib_dir)),
        ("CMAKE_ARCHIVE_OUTPUT_DIRECTORY", path(lib_dir)),
        ("NRD_SHADERS_PATH", path(shaders)),
    ]);
    defines
}

/// Builds NRD from scratch in `build` (removing what is there) with `defines`, then removes
/// everything but the libraries: the source copy, `CMake`'s tree and the generated shaders are
/// not read again (the shaders are embedded in the library).
#[cfg(feature = "nrd")]
fn build_nrd(build: &Path, sdk: &Sdk, defines: &[(&'static str, String)]) {
    if build.exists() {
        fs::remove_dir_all(build)
            .unwrap_or_else(|error| fail(format!("removing {}: {error}", build.display())));
    }
    // NRD's configure step writes `Shaders/NRDConfig.hlsli` into its source tree, so CMake
    // runs on a copy and the fetched SDK stays as verified. ShaderMake and MathLib are only
    // read, from where the fetch put them.
    let source = build.join("src");
    copy_tree(&sdk.dir, &source, &["_deps"])
        .unwrap_or_else(|error| fail(format!("copying NRD into {}: {error}", source.display())));
    let mut config = cmake::Config::new(&source);
    config
        .out_dir(build.join("cmake"))
        .profile("Release")
        .build_target("NRD");
    for (name, value) in defines {
        config.define(name, value);
    }
    // Panics (failing the build script, before the stamp is written) if CMake fails.
    config.build();
    for scratch in ["src", "cmake", "shaders"] {
        let dir = build.join(scratch);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .unwrap_or_else(|error| fail(format!("removing {}: {error}", dir.display())));
        }
    }
}

/// Copies the tree at `from` into `to`, leaving out the top-level entries named in `skip`.
/// Symbolic links are skipped: today's NRD archive has none,
/// and a future SDK that relies on one fails at the `CMake` step, not here.
#[cfg(feature = "nrd")]
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|skip| name == *skip) {
            continue;
        }
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, &[])?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
        // Symbolic links are not followed or copied: the fetched archives have none that
        // NRD's build needs.
    }
    Ok(())
}

/// RTXDI: bindings for the host-side parameter structures its shaders share with C++.
#[cfg(feature = "rtxdi")]
fn rtxdi(context: &Context) {
    let sdk = context.require("rtxdi", "rtxdi");
    let builder = bindings(
        "rtxdi",
        "#include <stdint.h>\n#include \"Rtxdi/DI/ReSTIRDIParameters.h\"\n",
        &sdk.dir.join("Include"),
    )
    .clang_args(["-x", "c++", "-std=c++17"])
    // Its scoped enums reuse variant names (`Off`, `Basic`); keep the enum prefix.
    .prepend_enum_name(true)
    .allowlist_type("RTXDI_.*")
    .allowlist_var("RTXDI_RESERVOIR_BLOCK_SIZE");
    context.write(builder, "rtxdi.rs");
}

/// SHaRC: shader headers only (HLSL/GLSL; no host API to bind). The build checks they are
/// there for the phase that compiles them.
#[cfg(feature = "sharc")]
fn sharc(context: &Context) {
    let sdk = context.require("sharc", "sharc");
    for header in ["SharcCommon.h", "HashGridCommon.h"] {
        let path = sdk.dir.join("include").join(header);
        if !path.is_file() {
            fail(format!(
                "moba-render feature `sharc`: {} is missing from the fetched SDK",
                path.display()
            ));
        }
    }
}

/// FSR 4 through the FidelityFX API loader: bindings for its entry points, loaded at run time
/// with libloading (no link line).
#[cfg(feature = "fsr4")]
fn fsr4(context: &Context) {
    let sdk = context.require("fidelityfx", "fsr4");
    let builder = bindings(
        "ffx",
        "#include <wchar.h>\n#include \"ffx_api.h\"\n",
        &sdk.dir
            .join("Kits")
            .join("FidelityFX")
            .join("api")
            .join("include"),
    )
    // `FFX_API_ENTRY` is `__declspec(dllexport)` on every platform.
    .clang_args([
        "-x",
        "c",
        "-std=c11",
        "-fdeclspec",
        "-Wno-ignored-attributes",
    ])
    .allowlist_function("ffx(CreateContext|DestroyContext|Query)")
    .dynamic_library_name("FfxApi")
    .dynamic_link_require_all(true);
    context.write(builder, "ffx.rs");
}

/// XeSS: bindings for `xessGetVersion` and the Vulkan extension-requirement queries, loaded
/// at run time with libloading (no link line). `xess_vk.h` includes `<vulkan/vulkan.h>`; a
/// minimal one is written into `OUT_DIR`, so no Vulkan SDK headers are needed.
#[cfg(feature = "xess")]
fn xess(context: &Context) {
    let sdk = context.require("xess", "xess");
    let shim = context.out.join("xess-vulkan");
    fs::create_dir_all(shim.join("vulkan"))
        .and_then(|()| fs::write(shim.join("vulkan").join("vulkan.h"), VULKAN_DECLARATIONS))
        .unwrap_or_else(|error| fail(format!("writing the Vulkan shim for xess: {error}")));
    let builder = bindings(
        "xess",
        "#include <stdbool.h>\n#include \"xess/xess.h\"\n#include \"xess/xess_vk.h\"\n",
        &sdk.dir.join("inc"),
    )
    .clang_args(["-x", "c", "-std=c11"])
    .clang_arg(format!("-I{}", shim.display()))
    .allowlist_function(
        "xess(GetVersion|VKGetRequiredInstanceExtensions|VKGetRequiredDeviceExtensions)",
    )
    .allowlist_type("_?xess_version_t")
    .allowlist_type("_?xess_result_t")
    .dynamic_library_name("XessApi")
    .dynamic_link_require_all(true);
    context.write(builder, "xess.rs");
}
