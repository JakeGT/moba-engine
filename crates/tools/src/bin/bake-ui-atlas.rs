#![allow(clippy::print_stdout)]
//! Bake the deterministic UI glyph atlas to a raw RGBA file.
//!
//! Usage: `cargo run -p moba-tools --bin bake-ui-atlas -- <out.rgba>`

use std::path::PathBuf;

fn main() {
    let output = std::env::args_os().nth(1).map_or_else(
        || PathBuf::from("crates/render/assets/ui-atlas.rgba"),
        PathBuf::from,
    );
    let pixels = moba_render::build_atlas();
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("create atlas directory");
    }
    std::fs::write(&output, pixels).expect("write atlas");
    println!(
        "wrote {} ({}x{}, {} bytes)",
        output.display(),
        moba_render::ATLAS_WIDTH,
        moba_render::ATLAS_HEIGHT,
        moba_render::ATLAS_WIDTH * moba_render::ATLAS_HEIGHT * 4
    );
}
