//! `content-check --provenance` (A0, ADR-0022): exit 0 when the gate passes and the notices
//! are current, 1 for a gate failure or a stale notice (`--check`), 2 for a usage error;
//! without `--check` it writes the notices, after which `--check` passes.

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
        std::fs::write(dir.join("content/models/a.glb"), "").expect("asset");
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
