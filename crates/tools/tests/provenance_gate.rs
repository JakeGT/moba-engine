//! `content-check --provenance` (A0, ADR-0022): exit 0 when the gate passes and the notices
//! are current, 1 for a gate failure or a stale notice (`--check`), 2 for a usage error;
//! without `--check` it writes the notices, after which `--check` passes. Shipped `.glb`
//! files with UVs need baked tangents, and notice paths must be plain files (amendment 1).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch content root and notices file under the system temporary directory.
struct Root {
    dir: PathBuf,
}

impl Root {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("moba-provenance-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content/models")).expect("content dir");
        std::fs::write(dir.join("content/pack.toml"), "").expect("pack");
        std::fs::write(dir.join("content/models/a.glb"), triangle_glb(true)).expect("asset");
        std::fs::write(
            dir.join("content/models/provenance.toml"),
            "[[asset]]\nfiles = [\"a.glb\"]\ntitle = \"A\"\nsource = \"original\"\n\
             author = \"MOBA Engine contributors\"\nlicence = \"MIT OR Apache-2.0\"\n",
        )
        .expect("manifest");
        std::fs::write(dir.join("NOTICES.md"), "# Notices\n").expect("notices");
        Self { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// Runs the gate over this root with `extra` arguments.
    fn gate(&self, extra: &[&str]) -> Output {
        let content = self.path("content/");
        let notices = self.path("NOTICES.md");
        let mut args = vec![
            "--provenance",
            "--content",
            content.to_str().expect("utf-8"),
            "--third-party",
            notices.to_str().expect("utf-8"),
        ];
        args.extend_from_slice(extra);
        run(&args)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A one-triangle `.glb` with `POSITION`, `NORMAL`, `TEXCOORD_0` and, with `tangents`,
/// `TANGENT`.
fn triangle_glb(tangents: bool) -> Vec<u8> {
    let floats =
        |values: &[f32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    let mut bin = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    bin.extend(floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    bin.extend(floats(&[0.0, 1.0, 1.0, 1.0, 0.0, 0.0]));
    bin.extend(floats(&[
        1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0,
    ]));
    let tangent = if tangents { r#","TANGENT":3"# } else { "" };
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2{tangent}}}}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}},{{"buffer":0,"byteOffset":72,"byteLength":24}},{{"buffer":0,"byteOffset":96,"byteLength":48}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"}},{{"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"}}]}}"#,
        bin.len()
    );
    let mut json = json.into_bytes();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend(b"glTF");
    glb.extend(2_u32.to_le_bytes());
    glb.extend(u32::try_from(total).expect("small").to_le_bytes());
    glb.extend(u32::try_from(json.len()).expect("small").to_le_bytes());
    glb.extend(b"JSON");
    glb.extend(&json);
    glb.extend(u32::try_from(bin.len()).expect("small").to_le_bytes());
    glb.extend(b"BIN\0");
    glb.extend(&bin);
    glb
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_content-check"))
        .args(args)
        .output()
        .expect("content-check runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("notice")
}

#[test]
fn check_fails_on_stale_notices_until_they_are_written() {
    let root = Root::new("write");
    let stale = root.gate(&["--check"]);
    assert_eq!(stale.status.code(), Some(1), "{}", stderr(&stale));
    assert!(stderr(&stale).contains("is stale"), "{}", stderr(&stale));
    assert!(
        !root.path("content/CREDITS.txt").exists(),
        "--check writes nothing"
    );

    let write = root.gate(&[]);
    assert_eq!(write.status.code(), Some(0), "{}", stderr(&write));
    let notices = read(&root.path("NOTICES.md"));
    assert!(
        notices.starts_with("# Notices\n\n<!-- BEGIN content provenance"),
        "{notices}"
    );
    // The trailing slash of `--content` is not part of the label.
    assert!(notices.contains("content/models/a.glb"), "{notices}");
    assert!(!notices.contains("content//"), "{notices}");
    assert!(read(&root.path("content/CREDITS.txt")).contains("A - MOBA Engine contributors"));
    assert!(root.path("content/AI-DISCLOSURE.md").exists());

    let current = root.gate(&["--check"]);
    assert_eq!(current.status.code(), Some(0), "{}", stderr(&current));

    // A hand edit of a generated file is stale again.
    std::fs::write(root.path("content/CREDITS.txt"), "edited\n").expect("edit");
    assert_eq!(root.gate(&["--check"]).status.code(), Some(1));
}

#[test]
fn a_gate_failure_exits_1_and_writes_nothing() {
    let root = Root::new("fail");
    std::fs::write(root.path("content/models/b.glb"), "").expect("uncovered");
    for extra in [&[][..], &["--check"][..]] {
        let output = root.gate(extra);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{extra:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("models/b.glb has no provenance"),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(read(&root.path("NOTICES.md")), "# Notices\n");
    assert!(!root.path("content/CREDITS.txt").exists());
}

#[test]
fn broken_markers_exit_1_without_touching_the_file() {
    let root = Root::new("markers");
    let broken = "# Notices\n<!-- END content provenance -->\n";
    std::fs::write(root.path("NOTICES.md"), broken).expect("notices");
    let output = root.gate(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("expected one BEGIN then one END"),
        "{}",
        stderr(&output)
    );
    assert_eq!(read(&root.path("NOTICES.md")), broken);
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &["--check"][..],
        &["--provenance", "--json"][..],
        &["--provenance", "content/example-pack.toml"][..],
        &["--provenance", "--content"][..],
        &["--provenance", "--bogus", "x"][..],
    ] {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

/// A shipped `.glb` with UVs must carry its own tangents; one that does not load fails too.
#[test]
fn shipped_glbs_with_uvs_need_baked_tangents() {
    let root = Root::new("tangents");
    assert_eq!(
        root.gate(&[]).status.code(),
        Some(0),
        "the fixture has tangents"
    );
    std::fs::write(root.path("content/models/a.glb"), triangle_glb(false)).expect("glb");
    let output = root.gate(&["--check"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("models/a.glb: has TEXCOORD_0 but no TANGENT"),
        "{}",
        stderr(&output)
    );
    std::fs::write(root.path("content/models/a.glb"), "not a glb").expect("glb");
    let output = root.gate(&["--check"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("models/a.glb: does not load"),
        "{}",
        stderr(&output)
    );
}

/// A notices path that is a symlink (here to `/dev/zero`) or a FIFO is an error, is not
/// read, and is never written through.
#[cfg(unix)]
#[test]
fn notice_paths_must_be_plain_files() {
    let root = Root::new("special");
    std::fs::remove_file(root.path("NOTICES.md")).expect("rm");
    std::os::unix::fs::symlink("/dev/zero", root.path("NOTICES.md")).expect("link");
    let start = std::time::Instant::now();
    let output = root.gate(&[]);
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("only a plain file is read"),
        "{}",
        stderr(&output)
    );
    // A credits file linked elsewhere: refused by the walk and never written through.
    std::fs::remove_file(root.path("NOTICES.md")).expect("rm");
    std::fs::write(root.path("NOTICES.md"), "# Notices\n").expect("notices");
    std::fs::write(root.path("elsewhere.txt"), "untouched\n").expect("target");
    std::os::unix::fs::symlink(root.path("elsewhere.txt"), root.path("content/CREDITS.txt"))
        .expect("link");
    let output = root.gate(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(read(&root.path("elsewhere.txt")), "untouched\n");
    // A FIFO is not opened (which would block).
    std::fs::remove_file(root.path("content/CREDITS.txt")).expect("rm");
    std::fs::remove_file(root.path("NOTICES.md")).expect("rm");
    let fifo = Command::new("mkfifo")
        .arg(root.path("NOTICES.md"))
        .status()
        .is_ok_and(|status| status.success());
    if fifo {
        let output = root.gate(&["--check"]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    }
}

/// `./content`, `content/` and `content` write the same notices.
#[test]
fn the_content_path_is_normalised_in_the_notices() {
    let root = Root::new("label");
    let gate = |content: &str, extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_content-check"))
            .current_dir(&root.dir)
            .args([
                "--provenance",
                "--content",
                content,
                "--third-party",
                "NOTICES.md",
            ])
            .args(extra)
            .output()
            .expect("content-check runs")
    };
    let write = gate("./content/", &[]);
    assert_eq!(write.status.code(), Some(0), "{}", stderr(&write));
    let notices = read(&root.path("NOTICES.md"));
    assert!(notices.contains("`content/models/a.glb`"), "{notices}");
    assert!(!notices.contains("./content"), "{notices}");
    for content in ["content", "content/", "./content"] {
        let check = gate(content, &["--check"]);
        assert_eq!(
            check.status.code(),
            Some(0),
            "{content}: {}",
            stderr(&check)
        );
    }
}
