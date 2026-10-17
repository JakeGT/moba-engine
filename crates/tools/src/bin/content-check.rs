//! Validate a content pack and print its catalogue summary.
#![allow(clippy::print_stdout)]

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: content-check <pack.toml>");
        std::process::exit(2);
    };
    let content =
        moba_game::load_content_file(std::path::Path::new(&path)).unwrap_or_else(|error| {
            eprintln!("invalid content {path}: {error}");
            std::process::exit(1);
        });
    println!("file       {path}");
    println!(
        "pack       {} {}",
        content.manifest.name, content.manifest.version
    );
    println!("author     {}", content.manifest.author);
    println!("items      {}", content.items.len());
    println!("heroes     {}", content.heroes.len());
    println!("abilities  {}", content.abilities.len());
    println!("lanes      {}", content.map.lanes.len());
    println!("structures {}", content.map.structures.len());
    println!("hash       {:#018x}", content.hash());
    println!("valid      true");
}
