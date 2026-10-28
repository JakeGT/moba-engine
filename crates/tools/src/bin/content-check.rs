//! Validate a content pack and print its catalogue summary.
#![allow(clippy::print_stdout)]

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            // Loader errors (TOML snippets, I/O messages) can contain any control character.
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[allow(clippy::too_many_lines)] // CLI validation/report is linear.
fn main() {
    let mut path: Option<String> = None;
    let mut json = false;
    let mut require_attestation = false;
    let mut publisher: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--require-attestation" => require_attestation = true,
            "--publisher" => {
                let Some(value) = args.next() else {
                    eprintln!("--publisher needs a 64-hex key");
                    std::process::exit(2);
                };
                publisher = Some(value);
            }
            "--help" => {
                eprintln!(
                    "usage: content-check [--json] [--require-attestation] [--publisher HEX] <pack.toml>"
                );
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
        eprintln!("usage: content-check [--json] <pack.toml>");
        std::process::exit(2);
    };
    let pack = std::path::Path::new(&path);
    let content = moba_game::load_content_file(pack).unwrap_or_else(|error| {
        if json {
            println!(
                "{{\"file\":{},\"valid\":false,\"errors\":[{}]}}",
                json_string(&path),
                json_string(&error.to_string())
            );
        }
        eprintln!("invalid content {path}: {error}");
        std::process::exit(1);
    });
    // `[assets]` paths are relative to the pack file's directory; check every file exists
    // there (and, for unit models, parses). Unknown names, bad paths and fallback cycles were
    // already rejected by the loader above.
    let pack_dir = pack
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let errors: Vec<String> = content
        .assets
        .check_files(pack_dir)
        .iter()
        .map(ToString::to_string)
        .collect();
    for error in &errors {
        eprintln!("error: {error}");
    }
    let valid = errors.is_empty();
    let attested = moba_net::crypto::verify_pack(&content);
    let publisher_hex = content
        .attestation
        .as_ref()
        .map(|attestation| hex(&attestation.publisher));
    if (require_attestation || publisher.is_some()) && !attested {
        eprintln!("content is not validly attested");
        std::process::exit(1);
    }
    if let Some(expected) = publisher.as_deref()
        && publisher_hex.as_deref() != Some(expected)
    {
        eprintln!("publisher key does not match trusted key");
        std::process::exit(1);
    }
    let self_centred_point_effect = |ability: &moba_game::AbilitySpec| {
        if !matches!(
            ability.targeting,
            moba_game::Targeting::Point { range } if range.to_i32() == 0
        ) {
            return false;
        }
        ability.effects.iter().any(|effect| {
            let moba_game::Effect::Area { effects, .. } = effect else {
                return false;
            };
            effects.iter().any(|effect| {
                let moba_game::AreaEffect::ApplyModifier { modifier, .. } = effect else {
                    return false;
                };
                content
                    .modifiers
                    .get(usize::from(*modifier))
                    .is_some_and(|definition| {
                        matches!(
                            definition.kind,
                            moba_game::ModifierKind::Slow
                                | moba_game::ModifierKind::Root
                                | moba_game::ModifierKind::Silence
                                | moba_game::ModifierKind::Burn
                                | moba_game::ModifierKind::Stun
                        )
                    })
            })
        })
    };
    let warnings: Vec<String> = content
        .abilities
        .iter()
        .filter(|ability| self_centred_point_effect(ability))
        .map(|ability| {
            format!(
                "{} is point-targeted with cast_range=0 (self-centred)",
                ability.name
            )
        })
        .collect();
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if json {
        let error_list: Vec<String> = errors.iter().map(|error| json_string(error)).collect();
        println!(
            "{{\"file\":{},\"pack\":{},\"version\":{},\"author\":{},\"items\":{},\"heroes\":{},\"abilities\":{},\"lanes\":{},\"structures\":{},\"assets\":{},\"hash\":\"{:#018x}\",\"attested\":{},\"publisher\":{},\"valid\":{},\"warnings\":{},\"errors\":[{}]}}",
            json_string(&path),
            json_string(&content.manifest.name),
            json_string(&content.manifest.version),
            json_string(&content.manifest.author),
            content.items.len(),
            content.heroes.len(),
            content.abilities.len(),
            content.map.lanes.len(),
            content.map.structures.len(),
            content.assets.len(),
            content.hash(),
            attested,
            json_string(publisher_hex.as_deref().unwrap_or("")),
            valid,
            warnings.len(),
            error_list.join(",")
        );
    } else {
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
        println!("assets     {}", content.assets.len());
        println!("hash       {:#018x}", content.hash());
        println!("valid      {valid}");
    }
    if !valid {
        std::process::exit(1);
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
