//! moba-render build script. The open build does nothing here; the vendor SDK features
//! (ADR-0020 §6b) locate the fetched SDKs, generate their FFI declarations with `bindgen` into
//! `OUT_DIR` and link what each SDK needs. It never downloads.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(any(
        feature = "dlss",
        feature = "streamline",
        feature = "nrd",
        feature = "rtxdi",
        feature = "sharc",
        feature = "fsr4",
        feature = "xess"
    ))]
    vendor::main();
}

#[cfg(any(
    feature = "dlss",
    feature = "streamline",
    feature = "nrd",
    feature = "rtxdi",
    feature = "sharc",
    feature = "fsr4",
    feature = "xess"
))]
#[path = "build/vendor.rs"]
mod vendor;
