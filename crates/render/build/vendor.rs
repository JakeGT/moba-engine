//! The vendor SDK half of moba-render's build script (ADR-0020 §6b).
//!
//! For each enabled feature it finds the SDK that `scripts/fetch-sdks.sh` unpacked into
//! `MOBA_SDK_DIR` (default `<workspace>/target/vendor-sdks`), checks the fetch's stamp against
//! the pin in `vendor-sdks.toml`, generates the FFI declarations with `bindgen` into `OUT_DIR`
//! and emits the link lines. A missing or stale SDK fails the build with the fetch command;
//! nothing is ever downloaded here.

use std::env;
use std::fmt::Display;
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
    out: PathBuf,
    target_os: String,
    #[cfg_attr(not(any(feature = "dlss", feature = "nrd")), allow(dead_code))]
    target_arch: String,
    #[cfg_attr(not(any(feature = "dlss", feature = "nrd")), allow(dead_code))]
    target_vendor: String,
}

struct Sdk {
    dir: PathBuf,
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
        let sdk_dir = env::var_os("MOBA_SDK_DIR")
            .map_or_else(|| root.join("target").join("vendor-sdks"), PathBuf::from);
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
        Sdk { dir }
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
            // The dynamic-CRT (/MD) build, matching Rust's MSVC targets.
            let dir = lib.join("Windows_x86_64").join("x64");
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static=nvsdk_ngx_d");
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

/// NRD: built from source with CMake as a static library, without network access (ADR-0020
/// §6b lists the flags), plus bindings for its library descriptor.
#[cfg(feature = "nrd")]
fn nrd(context: &Context) {
    let sdk = context.require("nrd", "nrd");
    let shadermake = context.require("nrd-shadermake", "nrd");
    let mathlib = context.require("nrd-mathlib", "nrd");
    let include = sdk.dir.join("Include");

    let builder = bindings("nrd", "#include \"NRD.h\"\n", &include)
        .clang_args(["-x", "c++", "-std=c++17"])
        .allowlist_function("(nrd::)?GetLibraryDesc")
        .allowlist_type("(nrd::)?LibraryDesc")
        .allowlist_var("NRD_VERSION_(MAJOR|MINOR|BUILD)");
    context.write(builder, "nrd.rs");

    // CMake 3.30+ (NRD's minimum) and the Vulkan SDK's DXC (SHADERMAKE_FIND_DXC_VK) are build
    // requirements of this feature only. Say so plainly instead of failing inside CMake.
    let cmake = env::var("CMAKE").unwrap_or_else(|_| String::from("cmake"));
    match Command::new(&cmake).arg("--version").output() {
        Ok(output) if output.status.success() => {}
        _ => fail(
            "moba-render feature `nrd` builds NRD with CMake 3.30 or newer, which was not found \
             (set CMAKE to its path)",
        ),
    }
    let dxc = if context.target_os == "windows" {
        env::var_os("VULKAN_SDK").map(|sdk| PathBuf::from(sdk).join("Bin").join("dxc.exe"))
    } else {
        Some(PathBuf::from("dxc"))
    };
    let dxc_ok = dxc
        .as_ref()
        .is_some_and(|dxc| Command::new(dxc).arg("--version").output().is_ok());
    if !dxc_ok {
        fail(
            "moba-render feature `nrd` compiles NRD's shaders with the Vulkan SDK's dxc \
             (SHADERMAKE_FIND_DXC_VK), which was not found: on Windows install the Vulkan SDK \
             (VULKAN_SDK), elsewhere put dxc on PATH",
        );
    }

    let lib_dir = context.out.join("nrd-lib");
    let shaders = context.out.join("nrd-shaders");
    cmake::Config::new(&sdk.dir)
        .profile("Release")
        .build_target("NRD")
        // No downloads at configure time: the fetch script unpacked both dependencies.
        .define("FETCHCONTENT_FULLY_DISCONNECTED", "ON")
        .define("FETCHCONTENT_SOURCE_DIR_SHADERMAKE", &shadermake.dir)
        .define("FETCHCONTENT_SOURCE_DIR_MATHLIB", &mathlib.dir)
        .define("NRD_NRI", "OFF")
        // ShaderMake's own compiler downloads stay off; its Vulkan-SDK DXC lookup stays on.
        .define("SHADERMAKE_FIND_DXC", "OFF")
        .define("SHADERMAKE_FIND_FXC", "OFF")
        .define("SHADERMAKE_FIND_DXC_VK", "ON")
        // SPIR-V embeds only; the DXIL and DXBC embeds would need DXIL and FXC.
        .define("NRD_EMBEDS_SPIRV_SHADERS", "ON")
        .define("NRD_EMBEDS_DXIL_SHADERS", "OFF")
        .define("NRD_EMBEDS_DXBC_SHADERS", "OFF")
        // Linked into moba-render, so nothing extra ships next to the executable.
        .define("NRD_STATIC_LIBRARY", "ON")
        // Keep outputs in OUT_DIR instead of NRD's source tree (_Bin, _Shaders).
        .define("CMAKE_RUNTIME_OUTPUT_DIRECTORY", &lib_dir)
        .define("CMAKE_LIBRARY_OUTPUT_DIRECTORY", &lib_dir)
        .define("NRD_SHADERS_PATH", &shaders)
        .build();
    // Multi-config generators (Visual Studio) add a per-configuration directory.
    for dir in [lib_dir.clone(), lib_dir.join("Release")] {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    println!("cargo:rustc-link-lib=static=NRD");
    // NRD is C++: libstdc++ on Linux and the other GNU-style targets, libc++ on Apple's
    // (MSVC links its C++ runtime itself).
    if context.target_vendor == "apple" {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if context.target_os != "windows" {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
    let _ = &context.target_arch;
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
