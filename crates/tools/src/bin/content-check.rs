//! Validate a content pack and print its catalogue summary.
#![allow(clippy::print_stdout)]

#[path = "../map_site.rs"]
mod map_site;

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
    let mut errors: Vec<String> = content
        .assets
        .check_files(pack_dir)
        .iter()
        .map(ToString::to_string)
        .collect();
    // The map's own visuals descriptor must agree with the simulation's map (ADR-0014 §2):
    // relief, wall coverage, walls and edge props on the right side of the board edge, and
    // tall props off the lanes (a warning); with water and foliage (ADR-0017 §4), water on
    // the board and wadeable, and warnings for water at structures, spawns or walls,
    // overlapping bodies and brush that would read as cover.
    let (map_errors, map_warnings) = map_consistency(&content, pack_dir);
    let map_error_count = map_errors.len();
    errors.extend(map_errors);
    for error in &errors {
        eprintln!("error: {error}");
    }
    let valid = errors.is_empty();
    // Materials next to the unit models (`materials/<model>.toml`, ADR-0010): what a client
    // would load. One descriptor set per textured material; the pool is sized from this.
    let materials = content.assets.material_report(pack_dir);
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
    let mut warnings: Vec<String> = content
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
    let map_warning_count = map_warnings.len();
    warnings.extend(map_warnings);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if json {
        let error_list: Vec<String> = errors.iter().map(|error| json_string(error)).collect();
        println!(
            "{{\"file\":{},\"pack\":{},\"version\":{},\"author\":{},\"items\":{},\"heroes\":{},\"abilities\":{},\"lanes\":{},\"structures\":{},\"assets\":{},\"materials\":{},\"textured_materials\":{},\"textures\":{},\"texture_bytes\":{},\"hash\":\"{:#018x}\",\"attested\":{},\"publisher\":{},\"valid\":{},\"warnings\":{},\"map_errors\":{},\"map_warnings\":{},\"errors\":[{}]}}",
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
            materials.materials,
            materials.textured,
            materials.textures,
            materials.texture_bytes,
            content.hash(),
            attested,
            json_string(publisher_hex.as_deref().unwrap_or("")),
            valid,
            warnings.len(),
            map_error_count,
            map_warning_count,
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
        println!(
            "materials  {} ({} textured, {} descriptor sets)",
            materials.materials, materials.textured, materials.textured
        );
        println!(
            "textures   {} ({} bindings, {} KiB)",
            materials.textures,
            materials.bound_textures,
            materials.texture_bytes.div_ceil(1024)
        );
        println!("hash       {:#018x}", content.hash());
        println!("valid      {valid}");
    }
    if !valid {
        std::process::exit(1);
    }
}

/// Runs [`moba_assets::check_map_consistency`] on the descriptors the pack's map uses, as
/// `(errors, warnings)` lines: every `[[assets.maps]]` entry with the map's visuals name, and
/// `maps/<visuals>.toml` under the pack directory whenever it exists, listed or not, because
/// the client loads that file by name. Listed descriptors that do not load (or resolve
/// outside the pack) are skipped, since the file check above already reports them; an
/// unlisted `maps/<visuals>.toml` that does not load is an error here.
fn map_consistency(
    content: &moba_game::GameContent,
    pack_dir: &std::path::Path,
) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    let name = content.map.visuals_name();
    let root = pack_dir.canonicalize().ok();
    let site = map_site::terrain_site(&content.map);
    let inside = |path: &std::path::Path| root.as_ref().is_none_or(|root| path.starts_with(root));
    // (path, listed): the listed entries first, then the client's by-name lookup.
    let mut paths: Vec<(std::path::PathBuf, bool)> = content
        .assets
        .maps
        .iter()
        .filter(|entry| entry.name == name)
        .filter_map(|entry| pack_dir.join(&entry.path).canonicalize().ok())
        .filter(|path| inside(path))
        .map(|path| (path, true))
        .collect();
    // The client's lookup: `<root>/maps/<name>.toml` for a 1-32 `[A-Za-z0-9_-]` name.
    let valid = (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if valid
        && let Ok(path) = pack_dir
            .join("maps")
            .join(format!("{name}.toml"))
            .canonicalize()
        && inside(&path)
        && !paths.iter().any(|(listed, _)| *listed == path)
    {
        paths.push((path, false));
    }
    for (path, listed) in paths {
        let visuals = match moba_assets::MapVisuals::load_file(&path) {
            Ok(visuals) => visuals,
            Err(_) if listed => continue,
            Err(error) => {
                errors.push(format!(
                    "map visuals '{name}' (maps/{name}.toml, loaded by the client by name): \
                     {error}"
                ));
                continue;
            }
        };
        // The edge mesh's real footprint, as the client places it (`models/<edge_mesh>.glb`
        // under the pack directory; the assumed footprint when it does not load).
        let footprint = visuals.ground.as_ref().and_then(|ground| {
            let mesh = ground.edge_mesh.as_deref()?;
            let data =
                moba_assets::load_glb_file(&pack_dir.join("models").join(format!("{mesh}.glb")))
                    .ok()?;
            let (min, max) = data.bounds();
            moba_assets::edge_footprint_radius(min, max, ground.edge_mesh_height)
        });
        let report =
            moba_assets::check_map_consistency_with_edge_footprint(&visuals, &site, footprint);
        let label = |line: String| format!("map visuals '{name}': {line}");
        errors.extend(report.errors.into_iter().map(label));
        warnings.extend(report.warnings.into_iter().map(label));
    }
    (errors, warnings)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
