//! Offline baked ambient-occlusion baker for a map's static occluders (R2b, ADR-0016).
//!
//! ```text
//! ao-bake bake [--pack <pack.toml>] [--root <asset root>] [--visuals <name>] [--texel M]
//!              [--height M] [<out-stem>]
//! ```
//!
//! Rasterises the map's static occluders into a top-down height field — the pack's walls as
//! the boxes the client draws (`wall_height` high, 2.5 m thick; a `wall_mesh` is fitted into
//! the same box, so the box stands in for it) and the visuals descriptor's props (their
//! meshes' triangles, or the built-in block) — then computes height-field horizon AO for a
//! ground-level receiver at each texel (16 directions, 12 m). Structures are not baked: they
//! are destroyed in play. Writes:
//!
//! - `<out-stem>.ktx2`: the square `R8_UNORM` occlusion map (1 = open), row 0 at world
//!   `y = 0`;
//! - `<out-stem>.toml`: the `moba_assets::BakedAoDesc` sidecar (extent, texels, falloff height
//!   and the `moba_assets::ao_fingerprint` of the inputs) after header comments.
//!
//! Defaults: the engine's default content when `--pack` is absent, `--root content`, the
//! pack's visuals name, 0.5 m texels, a 3 m falloff and
//! `<root>/maps/ao/<visuals>-ao` as the output stem. The output is deterministic for a
//! given platform.
#![allow(clippy::print_stdout)]

mod bake;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use moba_assets::{
    BakedAoDesc, MAX_BAKED_AO_TEXELS, MapProp, MapVisuals, MeshData, TextureData, TextureFormat,
    ao_fingerprint,
};

const USAGE: &str = "usage:
  ao-bake bake [--pack <pack.toml>] [--root <asset root>] [--visuals <name>] [--texel M]
               [--height M] [<out-stem>]";

/// Written as the KTX2 `KTXwriter` value.
const WRITER: &str = concat!("moba-tools ao-bake ", env!("CARGO_PKG_VERSION"));
/// Default metres per texel.
const DEFAULT_TEXEL: f32 = 0.5;
/// Wall thickness the client draws (`crates/client/src/app/map_visuals.rs`).
const WALL_THICKNESS: f32 = 2.5;
/// Footprint of the built-in prop block relative to its height (the client's block).
const BLOCK_WIDTH: f32 = 0.45;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("bake") => bake_command(&args[1..]),
        _ => Err(String::from(USAGE)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("ao-bake: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Parsed `bake` arguments.
struct BakeArgs {
    pack: Option<PathBuf>,
    root: PathBuf,
    visuals: Option<String>,
    texel: f32,
    height: f32,
    output: Option<PathBuf>,
}

fn parse_bake(args: &[String]) -> Result<BakeArgs, String> {
    let mut parsed = BakeArgs {
        pack: None,
        root: PathBuf::from("content"),
        visuals: None,
        texel: DEFAULT_TEXEL,
        height: moba_assets::DEFAULT_BAKED_AO_HEIGHT,
        output: None,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--pack" => parsed.pack = Some(PathBuf::from(value()?)),
            "--root" => parsed.root = PathBuf::from(value()?),
            "--visuals" => parsed.visuals = Some(value()?),
            "--texel" => {
                parsed.texel = value()?
                    .parse()
                    .ok()
                    .filter(|texel: &f32| texel.is_finite() && (0.1..=8.0).contains(texel))
                    .ok_or("--texel must be 0.1..=8 metres")?;
            }
            "--height" => {
                parsed.height = value()?
                    .parse()
                    .ok()
                    .filter(|height: &f32| height.is_finite() && (0.1..=64.0).contains(height))
                    .ok_or("--height must be 0.1..=64 metres")?;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}\n{USAGE}")),
            path if parsed.output.is_none() => parsed.output = Some(PathBuf::from(path)),
            _ => return Err(String::from(USAGE)),
        }
    }
    Ok(parsed)
}

/// The occluders of one map, in map coordinates.
struct Occluders {
    extent: f32,
    /// `[ax, ay, bx, by]`, world units, at most `moba_game::MAX_CONTENT_WALLS`.
    walls: Vec<[f32; 4]>,
    wall_height: f32,
    props: Vec<MapProp>,
}

fn rasterise(occluders: &Occluders, texel: f32, root: &Path) -> bake::HeightField {
    let mut field = bake::HeightField::new(occluders.extent, texel, MAX_BAKED_AO_TEXELS as usize);
    for [ax, ay, bx, by] in &occluders.walls {
        let (dx, dz) = (bx - ax, by - ay);
        let length = dx.hypot(dz);
        let axis = if length > 1e-4 {
            [dx / length, dz / length]
        } else {
            [1.0, 0.0]
        };
        field.add_box(
            [(ax + bx) * 0.5, (ay + by) * 0.5],
            axis,
            [length.max(0.5) * 0.5, WALL_THICKNESS * 0.5],
            occluders.wall_height,
        );
    }
    for prop in &occluders.props {
        let (sin, cos) = prop.yaw.to_radians().sin_cos();
        let mesh = prop.mesh.as_deref().and_then(|name| load_mesh(root, name));
        if let Some(mesh) = mesh {
            add_mesh(&mut field, &mesh, prop, [sin, cos]);
        } else {
            let half = prop.height * BLOCK_WIDTH * 0.5;
            field.add_box(prop.position, [cos, sin], [half, half], prop.height);
        }
    }
    field
}

/// A prop's mesh, or `None` (with a note) when it cannot be loaded: the client then draws
/// the built-in block, and so does the bake.
fn load_mesh(root: &Path, name: &str) -> Option<MeshData> {
    let path = root.join("models").join(format!("{name}.glb"));
    match moba_assets::load_glb_file(&path) {
        Ok(mesh) => Some(mesh),
        Err(error) => {
            eprintln!(
                "ao-bake: prop mesh {}: {error}; baking the block",
                path.display()
            );
            None
        }
    }
}

/// Rasterises a prop mesh placed the way the client places it: scaled uniformly to the
/// prop's height with its feet at the ground, centred, rotated by `yaw` and moved to the
/// prop's position.
fn add_mesh(field: &mut bake::HeightField, mesh: &MeshData, prop: &MapProp, [sin, cos]: [f32; 2]) {
    let (min, max) = mesh.bounds();
    let scale = prop.height / (max[1] - min[1]).max(1e-4);
    let base = [
        f32::midpoint(min[0], max[0]),
        min[1],
        f32::midpoint(min[2], max[2]),
    ];
    let place = |index: u32| {
        let p = mesh.positions()[index as usize];
        let local = [
            (p[0] - base[0]) * scale,
            (p[1] - base[1]) * scale,
            (p[2] - base[2]) * scale,
        ];
        [
            local[0] * cos - local[2] * sin + prop.position[0],
            local[1],
            local[0] * sin + local[2] * cos + prop.position[1],
        ]
    };
    for &[a, b, c] in mesh.indices().as_chunks::<3>().0 {
        field.add_triangle(place(a), place(b), place(c));
    }
}

/// The occluders of `args`' pack and visuals.
fn occluders(args: &BakeArgs) -> Result<(String, Occluders), String> {
    let content = match &args.pack {
        Some(path) => moba_game::load_content_file(path)
            .map_err(|error| format!("{}: {error}", path.display()))?,
        None => moba_game::GameContent::default(),
    };
    let map = &content.map;
    let name = args
        .visuals
        .clone()
        .unwrap_or_else(|| map.visuals_name().to_owned());
    if !moba_assets::valid_mesh_name(&name) {
        return Err(format!("invalid visuals name {name:?}"));
    }
    let path = args.root.join("maps").join(format!("{name}.toml"));
    let visuals =
        MapVisuals::load_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let walls = map
        .walls
        .iter()
        .take(moba_game::MAX_CONTENT_WALLS)
        .map(|wall| {
            [
                wall.a.x.to_f32(),
                wall.a.y.to_f32(),
                wall.b.x.to_f32(),
                wall.b.y.to_f32(),
            ]
        })
        .collect();
    Ok((
        name,
        Occluders {
            extent: map.extent.to_f32(),
            walls,
            wall_height: visuals.wall_height,
            props: visuals.props,
        },
    ))
}

fn bake_command(args: &[String]) -> Result<(), String> {
    let args = parse_bake(args)?;
    let (name, occluders) = occluders(&args)?;
    let field = rasterise(&occluders, args.texel, &args.root);
    let ao = bake::horizon_ao(&field);
    let texels = u32::try_from(field.texels).map_err(|_| "too many texels")?;
    let texture = TextureData::new(TextureFormat::R8Unorm, texels, texels, &[bake::to_r8(&ao)])
        .map_err(|error| format!("texture: {error}"))?;
    let fingerprint = ao_fingerprint(
        occluders.extent,
        &occluders.walls,
        occluders.wall_height,
        &occluders.props,
    );
    let desc = BakedAoDesc::new(occluders.extent, texels, fingerprint).with_height(args.height);
    let header = [
        format!("Baked AO for map visuals {name:?}, by {WRITER} (R2b, ADR-0016)."),
        String::from("Client-local and cosmetic: never hashed or sent. Regenerate with"),
        String::from("scripts/bake-map-ao.sh; see docs/content-authoring.md#baked-ao."),
        format!(
            "{} walls, {} props, {:.3} m texels, {} directions, {} m radius",
            occluders.walls.len(),
            occluders.props.len(),
            field.texel,
            bake::DIRECTIONS,
            bake::RADIUS
        ),
    ];
    let text = desc.to_toml(&header);
    // The written sidecar must load back (and fit the loader's size cap).
    BakedAoDesc::parse(&text).map_err(|error| format!("baked sidecar invalid: {error}"))?;
    let stem = args.output.clone().unwrap_or_else(|| {
        args.root
            .join(moba_assets::BAKED_AO_DIR)
            .join(format!("{name}-ao"))
    });
    if let Some(dir) = stem.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let toml = stem.with_extension("toml");
    let ktx2 = stem.with_extension("ktx2");
    let bytes = moba_assets::write_ktx2(&texture, WRITER);
    std::fs::write(&toml, &text).map_err(|error| format!("{}: {error}", toml.display()))?;
    std::fs::write(&ktx2, &bytes).map_err(|error| format!("{}: {error}", ktx2.display()))?;
    let mean = ao.iter().sum::<f32>() / ao.len().max(1) as f32;
    let darkest = ao.iter().copied().fold(1.0f32, f32::min);
    println!(
        "{}: r8 {texels}², {} bytes, mean {mean:.3}, min {darkest:.3}, fingerprint {fingerprint:#018x}; {}",
        ktx2.display(),
        bytes.len(),
        toml.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_and_rejects_bad_values() {
        let args: Vec<String> = ["--texel", "1", "--visuals", "ember", "out/x"]
            .map(String::from)
            .into();
        let parsed = parse_bake(&args).expect("parse");
        assert_eq!(parsed.texel, 1.0);
        assert_eq!(parsed.visuals.as_deref(), Some("ember"));
        assert_eq!(parsed.output, Some(PathBuf::from("out/x")));
        for bad in [
            &["--texel", "0"][..],
            &["--height", "nan"],
            &["--what"],
            &["a", "b"],
        ] {
            let bad: Vec<String> = bad.iter().map(|s| (*s).to_owned()).collect();
            assert!(parse_bake(&bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn walls_and_block_props_land_where_the_client_draws_them() {
        let visuals = MapVisuals::parse(
            "terrain = [0.1, 0.1, 0.1]\nwall = [0.2, 0.2, 0.2]\n\
             [[props]]\nposition = [30, 10.25]\ncolor = [1, 0, 0]\nheight = 4\nyaw = 45\n",
        )
        .expect("visuals");
        let occluders = Occluders {
            extent: 40.0,
            walls: vec![[10.0, 5.0, 10.0, 35.0]],
            wall_height: 6.0,
            props: visuals.props,
        };
        let field = rasterise(&occluders, 0.5, Path::new("missing-root"));
        let at = |x: f32, z: f32| {
            field.heights[(z / field.texel) as usize * field.texels + (x / field.texel) as usize]
        };
        // The wall runs along z at x = 10, 2.5 m thick.
        assert_eq!(at(10.0, 20.0), 6.0);
        assert_eq!(at(11.0, 20.0), 6.0);
        assert_eq!(at(12.0, 20.0), 0.0);
        assert_eq!(at(10.0, 36.0), 0.0);
        // The block prop: 1.8 m square, 4 m high; rotated 45° it reaches 1.27 m along x.
        assert_eq!(at(30.0, 10.3), 4.0);
        assert_eq!(at(31.3, 10.3), 4.0);
        assert_eq!(at(30.0, 12.0), 0.0);
    }
}
