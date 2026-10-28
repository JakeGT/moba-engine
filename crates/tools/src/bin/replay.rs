//! Verify a deterministic replay file.
//!
//! Usage: `cargo run -p moba-tools --bin replay -- match.mbr`
#![allow(clippy::print_stdout)]

use std::path::PathBuf;

use moba_game::GameContent;
use moba_sim::{MatchOutcome, Replay};

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

fn main() {
    let mut path: Option<PathBuf> = None;
    let mut json = false;
    let mut content: Option<GameContent> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--items" => {
                let Some(value) = args.next() else {
                    eprintln!("--items needs a path");
                    std::process::exit(2);
                };
                let loaded = moba_game::load_content_file(std::path::Path::new(&value))
                    .unwrap_or_else(|error| {
                        eprintln!("invalid content {value}: {error}");
                        std::process::exit(2);
                    });
                content = Some(loaded);
            }
            "--help" => {
                eprintln!("usage: replay [--items FILE] [--json] <file.mbr>");
                return;
            }
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: replay [--items FILE] [--json] <file.mbr>");
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
    let result = match &content {
        Some(content) => replay.play_with_content(content),
        None => replay.play(),
    };
    match result {
        Ok(result) => {
            if json {
                let outcome = match result.outcome {
                    Some(MatchOutcome::Winner(moba_game::Team::Blue)) => "\"blue\"".to_string(),
                    Some(MatchOutcome::Winner(moba_game::Team::Red)) => "\"red\"".to_string(),
                    Some(MatchOutcome::Draw) => "\"draw\"".to_string(),
                    Some(_) => "\"unknown\"".to_string(),
                    None => String::from("null"),
                };
                let checkpoints: Vec<String> = replay
                    .checkpoints
                    .iter()
                    .map(|(tick, hash)| format!("{{\"tick\":{tick},\"hash\":\"{hash:#018x}\"}}"))
                    .collect();
                println!(
                    "{{\"file\":{},\"content_hash\":\"{:#018x}\",\"pack\":{},\"version\":{},\"seed\":{},\"frames\":{},\"checkpoints\":[{}],\"ticks\":{},\"outcome\":{outcome},\"state_hash\":\"{:#018x}\",\"verified\":true}}",
                    json_string(&path.to_string_lossy()),
                    replay.content_hash,
                    json_string(&replay.content_name),
                    json_string(&replay.content_version),
                    replay.config.seed,
                    replay.frames.len(),
                    checkpoints.join(","),
                    result.ticks,
                    result.state_hash
                );
                return;
            }
            println!("file       {}", path.display());
            println!("content    {:#018x}", replay.content_hash);
            println!(
                "pack       {} {}",
                replay.content_name, replay.content_version
            );
            println!("seed       {}", replay.config.seed);
            println!("frames     {}", replay.frames.len());
            println!("checkpoints {}", replay.checkpoints.len());
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
