//! Validate a content pack and print its catalogue summary.
#![allow(clippy::print_stdout)]

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
    let mut path: Option<String> = None;
    let mut json = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--json" => json = true,
            "--help" => {
                eprintln!("usage: content-check [--json] <pack.toml>");
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
    let content =
        moba_game::load_content_file(std::path::Path::new(&path)).unwrap_or_else(|error| {
            eprintln!("invalid content {path}: {error}");
            std::process::exit(1);
        });
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
        println!(
            "{{\"file\":{},\"pack\":{},\"version\":{},\"author\":{},\"items\":{},\"heroes\":{},\"abilities\":{},\"lanes\":{},\"structures\":{},\"hash\":\"{:#018x}\",\"valid\":true,\"warnings\":{}}}",
            json_string(&path),
            json_string(&content.manifest.name),
            json_string(&content.manifest.version),
            json_string(&content.manifest.author),
            content.items.len(),
            content.heroes.len(),
            content.abilities.len(),
            content.map.lanes.len(),
            content.map.structures.len(),
            content.hash(),
            warnings.len()
        );
        return;
    }
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
