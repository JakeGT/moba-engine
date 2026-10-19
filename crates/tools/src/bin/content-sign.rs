//! Sign a content pack with an Ed25519 publisher key.
//!
//! Usage: `MOBA_SIGNING_SEED=<64 hex> cargo run -p moba-tools --bin content-sign -- <pack.toml> [--json]`
//! Prints the `publisher`/`signature` TOML fields; add them to the pack manifest.

#![allow(clippy::print_stdout)]

fn main() {
    let mut path: Option<String> = None;
    let mut json = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--json" => json = true,
            "--help" => {
                eprintln!("usage: content-sign <pack.toml> [--json]  (MOBA_SIGNING_SEED required)");
                return;
            }
            other if path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: content-sign <pack.toml> [--json]");
        std::process::exit(2);
    };
    let Some(seed) = std::env::var("MOBA_SIGNING_SEED").ok().and_then(|value| {
        let bytes = value.as_bytes();
        if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut seed = [0u8; 32];
        for (index, byte) in seed.iter_mut().enumerate() {
            let text = std::str::from_utf8(&bytes[index * 2..index * 2 + 2]).ok()?;
            *byte = u8::from_str_radix(text, 16).ok()?;
        }
        Some(seed)
    }) else {
        eprintln!("MOBA_SIGNING_SEED must be 64 hex characters");
        std::process::exit(2);
    };
    let mut content =
        moba_game::load_content_file(std::path::Path::new(&path)).unwrap_or_else(|error| {
            eprintln!("invalid content {path}: {error}");
            std::process::exit(1);
        });
    content.attestation = Some(moba_net::crypto::sign_pack(&content, &seed));
    let attestation = content.attestation.as_ref().expect("just set");
    let publisher = hex(&attestation.publisher);
    let signature = hex(&attestation.signature);
    if json {
        println!(
            "{{\"publisher\":\"{publisher}\",\"signature\":\"{signature}\",\"hash\":\"{:#018x}\"}}",
            content.hash()
        );
    } else {
        println!("publisher = \"{publisher}\"");
        println!("signature = \"{signature}\"");
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
