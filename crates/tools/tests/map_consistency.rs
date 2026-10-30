//! The shipped sample maps' visuals descriptors agree with their packs' simulation maps
//! (ADR-0014 §2): no consistency errors and no obstacle warnings. Independent of the terrain
//! texture files, which `content-check` checks separately.

#[path = "../src/map_site.rs"]
mod map_site;

use std::path::{Path, PathBuf};
use std::process::Command;

fn content_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// Loads `pack`, checks that its map's visuals name is `visuals`, and runs the consistency
/// check on `maps/<visuals>.toml`.
fn check(pack: &str, visuals: &str) -> moba_assets::MapConsistency {
    let content = moba_game::load_content_file(&content_dir().join(pack)).expect("pack loads");
    assert_eq!(content.map.visuals_name(), visuals, "{pack}");
    let descriptor =
        moba_assets::MapVisuals::load_file(&content_dir().join(format!("maps/{visuals}.toml")))
            .expect("descriptor loads");
    assert!(
        descriptor.ground.is_some(),
        "{visuals} has a [terrain] table"
    );
    moba_assets::check_map_consistency(&descriptor, &map_site::terrain_site(&content.map))
}

#[test]
fn sample_maps_are_consistent_with_their_simulation_maps() {
    for (pack, visuals) in [
        // The default map has no `visuals` name: it resolves to the grass theme's name.
        ("example-pack.toml", "map-grass"),
        ("ember-map.toml", "ember"),
        ("frost-map.toml", "frost"),
    ] {
        let report = check(pack, visuals);
        assert!(report.is_ok(), "{pack}: {:?}", report.errors);
        assert!(report.warnings.is_empty(), "{pack}: {:?}", report.warnings);
    }
}

/// Review fix: with 48 m cliffs, the Low (128-cell) surface once interpolated the rise onto
/// the board (0.627 m at [0, 48.8]) between grid samples that all passed. The rise now starts
/// one Low cell past the edge, and the check samples every tier's interpolated surface.
#[test]
fn tall_ember_cliffs_stay_off_the_board_on_every_tier_grid() {
    let content =
        moba_game::load_content_file(&content_dir().join("ember-map.toml")).expect("pack");
    let site = map_site::terrain_site(&content.map);
    let text = std::fs::read_to_string(content_dir().join("maps/ember.toml")).expect("read");
    let text = text.replace("edge_height = 26.0", "edge_height = 48.0");
    let descriptor = moba_assets::MapVisuals::parse(&text).expect("descriptor");
    let terrain = descriptor.ground.as_ref().expect("terrain");
    assert_eq!(terrain.edge_height, 48.0);
    let report = moba_assets::check_map_consistency(&descriptor, &site);
    assert!(report.is_ok(), "{:?}", report.errors);
    for cells in moba_assets::TERRAIN_GRID_SIZES {
        let field = moba_assets::generate_heightfield(terrain, &site, cells);
        let (h, at) = moba_assets::board_edge_relief(&field, site.extent).expect("edge cells");
        assert!(
            h.abs() <= moba_assets::MAX_PLAY_RELIEF,
            "{cells} cells: {h} m at {at:?}"
        );
        // The board corner the review measured.
        assert!(field.sample(0.0, 48.8).abs() <= moba_assets::MAX_PLAY_RELIEF);
    }
}

#[test]
fn a_prop_on_a_lane_is_a_warning_and_a_low_wall_an_error() {
    let content =
        moba_game::load_content_file(&content_dir().join("ember-map.toml")).expect("pack");
    let site = map_site::terrain_site(&content.map);
    let text = std::fs::read_to_string(content_dir().join("maps/ember.toml")).expect("read");
    // Back onto the mid lane, and walls too low to read as line-of-sight blockers.
    let text = text
        .replace("position = [121, 138]", "position = [128, 128]")
        .replace("wall_height = 6.0", "wall_height = 1.5");
    let descriptor = moba_assets::MapVisuals::parse(&text).expect("descriptor");
    let report = moba_assets::check_map_consistency(&descriptor, &site);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].starts_with("prop 2 at [128, 128]"));
    assert_eq!(
        report.errors.len(),
        content.map.walls.len(),
        "{:?}",
        report.errors
    );
}

/// `content-check` runs the same rules on the pack's map descriptor and reports them in
/// text and JSON.
#[test]
fn content_check_reports_map_consistency() {
    let dir = std::env::temp_dir().join(format!("moba-map-consistency-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).expect("dir");
    let pack = std::fs::read_to_string(content_dir().join("ember-map.toml")).expect("pack");
    std::fs::write(dir.join("pack.toml"), pack).expect("write");
    let descriptor = "wall = [0.5, 0.2, 0.1]\nwall_height = 1.5\nterrain = [0.2, 0.1, 0.05]\n\
                      [[props]]\nposition = [128, 128]\ncolor = [1, 1, 1]\nheight = 16\n";
    std::fs::write(dir.join("maps/ember.toml"), descriptor).expect("write");
    let run = |json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_content-check"));
        if json {
            command.arg("--json");
        }
        command.arg(dir.join("pack.toml")).output().expect("run")
    };
    let text = run(false);
    let stderr = String::from_utf8_lossy(&text.stderr);
    assert_eq!(text.status.code(), Some(1), "{stderr}");
    assert!(
        stderr
            .contains("error: map visuals 'ember': wall 0 ([112, 60] to [112, 100]): wall_height"),
        "{stderr}"
    );
    assert!(
        stderr.contains("warning: map visuals 'ember': prop 0 at [128, 128]"),
        "{stderr}"
    );
    let json = run(true);
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert!(stdout.contains("\"map_errors\":4"), "{stdout}");
    assert!(stdout.contains("\"map_warnings\":1"), "{stdout}");
    assert!(stdout.contains("\"valid\":false"), "{stdout}");
    assert!(stdout.contains("map visuals 'ember': wall 3"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Review fix: the client loads `maps/<visuals>.toml` by name whether or not the pack lists
/// it under `[[assets.maps]]`, so `content-check` checks that file too (and reports one that
/// does not load).
#[test]
fn content_check_checks_an_unlisted_descriptor_the_client_loads_by_name() {
    let dir = std::env::temp_dir().join(format!("moba-map-unlisted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("maps")).expect("dir");
    // The default pack lists no `[[assets.maps]]`; its map's visuals name is the theme's.
    let pack = std::fs::read_to_string(content_dir().join("example-pack.toml")).expect("pack");
    assert!(!pack.contains("[[assets.maps]]"), "the map is not listed");
    std::fs::write(dir.join("pack.toml"), &pack).expect("write");
    let content = moba_game::load_content_file(&dir.join("pack.toml")).expect("pack loads");
    let name = content.map.visuals_name().to_owned();
    let lane = &map_site::terrain_site(&content.map).lanes[0];
    let on_lane = [
        f32::midpoint(lane[0][0], lane[1][0]),
        f32::midpoint(lane[0][1], lane[1][1]),
    ];
    let file = dir.join(format!("maps/{name}.toml"));
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_content-check"))
            .arg(dir.join("pack.toml"))
            .output()
            .expect("run")
    };
    // A tall prop on a lane: the consistency rules apply to the unlisted file.
    std::fs::write(
        &file,
        format!(
            "wall = [0.5, 0.2, 0.1]\nterrain = [0.2, 0.1, 0.05]\n[[props]]\nposition = [{}, {}]\ncolor = [1, 1, 1]\n\
             height = 16\n",
            on_lane[0], on_lane[1]
        ),
    )
    .expect("write");
    let output = run();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("warning: map visuals '{name}': prop 0 at")),
        "{stderr}"
    );
    // A descriptor that does not load is an error too.
    std::fs::write(&file, "terrain = [9, 0, 0]\n").expect("write");
    let output = run();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "map visuals '{name}' (maps/{name}.toml, loaded by the client by name)"
        )),
        "{stderr}"
    );
    // Without the file, nothing to check.
    std::fs::remove_file(&file).expect("remove");
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
