//! `content-check` asset validation (#285): every invalid-reference case is a clear error in
//! text and `--json`, with exit code 1 and never a panic.

use std::path::{Path, PathBuf};
use std::process::Command;

const SENTINEL: &[u8] = include_bytes!("../../../content/models/sentinel.glb");

/// A pack whose first hero names `model` and whose `[assets]` has `units`.
fn pack(units: &str, model: &str, map: &str) -> String {
    format!(
        r#"name = "Asset Test"
{units}
{map}
[[heroes]]
name = "Knight"
max_health = 600
max_mana = 300
attack_damage = 50
attack_range = 2.0
attack_interval_ticks = 30
move_speed = 7.0
vision_range = 14.0
abilities = [0, 1, 2]
model = "{model}"
"#
    )
}

fn unit(name: &str, path: &str) -> String {
    format!("[[assets.units]]\nname = \"{name}\"\npath = \"{path}\"\n")
}

struct Pack {
    dir: PathBuf,
}

impl Pack {
    fn new(tag: &str, text: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("moba-content-check-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models")).expect("pack dir");
        std::fs::write(dir.join("models/sentinel.glb"), SENTINEL).expect("model");
        std::fs::write(dir.join("models/broken.glb"), &SENTINEL[..64]).expect("model");
        std::fs::write(dir.join("models/sentinel.obj"), b"o x\n").expect("model");
        std::fs::write(dir.join("pack.toml"), text).expect("pack");
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("pack.toml")
    }
}

impl Drop for Pack {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn content_check(json: bool, pack: &Path) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_content-check"));
    if json {
        command.arg("--json");
    }
    let output = command.arg(pack).output().expect("run content-check");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Runs both modes and asserts a clean failure mentioning `needle` in each.
fn assert_rejected(tag: &str, text: &str, needle: &str) {
    let pack = Pack::new(tag, text);
    let run = content_check(false, &pack.path());
    assert_eq!(
        run.code,
        Some(1),
        "{tag}: text exit code; stderr: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains(needle),
        "{tag}: text stderr lacks {needle:?}: {}",
        run.stderr
    );
    assert!(!run.stderr.contains("panicked"), "{tag}: {}", run.stderr);
    let run = content_check(true, &pack.path());
    assert_eq!(
        run.code,
        Some(1),
        "{tag}: json exit code; stderr: {}",
        run.stderr
    );
    let line = run.stdout.trim();
    assert!(
        line.starts_with('{') && line.ends_with('}') && line.lines().count() == 1,
        "{tag}: one JSON object: {line}"
    );
    assert!(line.contains("\"valid\":false"), "{tag}: {line}");
    assert!(line.contains("\"errors\":[\""), "{tag}: {line}");
    assert!(
        line.contains(needle),
        "{tag}: JSON lacks {needle:?}: {line}"
    );
}

#[test]
fn shipped_sentinel_pack_validates_with_its_asset() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/sentinel-pack.toml");
    let run = content_check(true, &pack);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.contains("\"assets\":1"), "{}", run.stdout);
    assert!(run.stdout.contains("\"valid\":true"), "{}", run.stdout);
    assert!(run.stdout.contains("\"errors\":[]"), "{}", run.stdout);
    let run = content_check(false, &pack);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.contains("assets     1"), "{}", run.stdout);
    assert!(run.stdout.contains("valid      true"), "{}", run.stdout);
}

#[test]
fn packs_without_assets_report_zero() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/example-pack.toml");
    let run = content_check(true, &pack);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.contains("\"assets\":0"), "{}", run.stdout);
}

#[test]
fn a_valid_asset_pack_passes() {
    let text = pack(&unit("knight", "models/sentinel.glb"), "knight", "");
    let pack = Pack::new("valid", &text);
    let run = content_check(true, &pack.path());
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.contains("\"assets\":1"), "{}", run.stdout);
}

#[test]
fn unknown_names_are_rejected() {
    assert_rejected(
        "unknown-model",
        &pack(&unit("knight", "models/sentinel.glb"), "ghost", ""),
        "has no [[assets.units]] entry in this pack",
    );
    assert_rejected(
        "no-assets-table",
        &pack("", "knight", ""),
        "has no [[assets.units]] entry",
    );
    let map = "[map]\nvisuals = \"lava\"\nblue_spawn = [32, 128]\nred_spawn = [224, 128]\n\
               [[map.lanes]]\nname = \"Mid\"\npath = [[32, 128], [224, 128]]\n";
    assert_rejected(
        "unknown-visuals",
        &pack(&unit("knight", "models/sentinel.glb"), "knight", map),
        "has no [[assets.maps]] entry in this pack",
    );
    assert_rejected(
        "path-like-name",
        &pack(&unit("knight", "models/sentinel.glb"), "../knight", ""),
        "heroes.model must be 1..=16 of [A-Za-z0-9_-]",
    );
}

#[test]
fn fallback_cycles_and_missing_fallbacks_are_rejected() {
    let cycle = "[[assets.units]]\nname = \"knight\"\npath = \"models/sentinel.glb\"\n\
                 fallback = \"rook\"\n\n[[assets.units]]\nname = \"rook\"\n\
                 path = \"models/sentinel.glb\"\nfallback = \"knight\"\n";
    assert_rejected(
        "cycle",
        &pack(cycle, "knight", ""),
        "fallback names a missing entry, or the chain is cyclic",
    );
    let missing = "[[assets.units]]\nname = \"knight\"\npath = \"models/sentinel.glb\"\n\
                   fallback = \"rook\"\n";
    assert_rejected(
        "missing-fallback",
        &pack(missing, "knight", ""),
        "fallback names a missing entry",
    );
}

#[test]
fn path_escapes_are_rejected() {
    for (tag, path) in [
        ("dotdot", "../models/sentinel.glb"),
        ("absolute", "/etc/passwd.glb"),
        ("backslash", "models\\\\sentinel.glb"),
        ("drive", "C:/models/sentinel.glb"),
        ("dot", "./models/sentinel.glb"),
    ] {
        assert_rejected(
            tag,
            &pack(&unit("knight", path), "knight", ""),
            "asset paths must be relative ASCII",
        );
    }
}

#[test]
fn missing_wrong_type_and_malformed_files_are_rejected() {
    assert_rejected(
        "missing",
        &pack(&unit("knight", "models/knight.glb"), "knight", ""),
        "assets.units 'knight' (models/knight.glb): missing or unreadable",
    );
    assert_rejected(
        "not-glb",
        &pack(&unit("knight", "models/sentinel.obj"), "knight", ""),
        "assets.units 'knight' (models/sentinel.obj): not a .glb file",
    );
    assert_rejected(
        "directory",
        &pack(&unit("knight", "models"), "knight", ""),
        "not a .glb file",
    );
    assert_rejected(
        "malformed",
        &pack(&unit("knight", "models/broken.glb"), "knight", ""),
        "assets.units 'knight' (models/broken.glb): malformed mesh",
    );
    // Unreferenced entries are still checked: the manifest is the pack's file list.
    let units = format!(
        "{}{}",
        unit("knight", "models/sentinel.glb"),
        unit("spare", "models/spare.glb")
    );
    assert_rejected(
        "unreferenced-missing",
        &pack(&units, "knight", ""),
        "assets.units 'spare' (models/spare.glb): missing",
    );
}

#[test]
fn syntax_errors_are_json_too() {
    assert_rejected("toml", "name = \n\t[[", "invalid TOML");
}

/// ADR-0019: the pack's client-local effects are validated too: a heat-haze emitter with an
/// out-of-range strength (or any field the parser rejects) fails the pack, in text and JSON.
#[test]
fn invalid_effects_are_rejected() {
    let text = pack(&unit("knight", "models/sentinel.glb"), "knight", "");
    let pack = Pack::new("vfx", &text);
    std::fs::create_dir_all(pack.dir.join("vfx")).expect("vfx dir");
    let haze = "duration = 1\n[[emitters]]\nburst = 1\nlifetime = [1, 1]\nsize = [1, 1]\n\
                blend = \"distortion\"\n";
    std::fs::write(
        pack.dir.join("vfx/heat-vent.toml"),
        format!("{haze}distortion = 0.5\n"),
    )
    .expect("write");
    let run = content_check(true, &pack.path());
    assert_eq!(run.code, Some(0), "a valid haze passes: {}", run.stderr);
    std::fs::write(
        pack.dir.join("vfx/heat-vent.toml"),
        format!("{haze}distortion = 3.0\n"),
    )
    .expect("write");
    for json in [false, true] {
        let run = content_check(json, &pack.path());
        assert_eq!(run.code, Some(1), "{}", run.stderr);
        let output = if json { &run.stdout } else { &run.stderr };
        assert!(
            output.contains("vfx/heat-vent.toml: ") && output.contains("distortion must be"),
            "{output}"
        );
    }
}
