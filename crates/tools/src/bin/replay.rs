//! Verify a deterministic replay file.
//!
//! Usage: `cargo run -p moba-tools --bin replay -- match.mbr`
#![allow(clippy::print_stdout)]

use std::path::PathBuf;

use moba_sim::Replay;

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: replay <file.mbr>");
        std::process::exit(2);
    };
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            std::process::exit(2);
        }
    };
    let replay = match Replay::decode(&data) {
        Ok(replay) => replay,
        Err(error) => {
            eprintln!("invalid replay: {error}");
            std::process::exit(1);
        }
    };
    match replay.play() {
        Ok(result) => {
            println!("file       {}", path.display());
            println!("seed       {}", replay.config.seed);
            println!("frames     {}", replay.frames.len());
            println!("ticks      {}", result.ticks);
            println!("outcome    {:?}", result.outcome);
            println!("state hash {:#018x}", result.state_hash);
            println!("verified   true");
        }
        Err(error) => {
            eprintln!("replay failed: {error}");
            std::process::exit(1);
        }
    }
}
