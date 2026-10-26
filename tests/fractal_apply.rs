//! Sparse overlay emission and in-place application through the public API and CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::fractal;

struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("roundhouse-fractal-apply-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn dir(&self, path: &str) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn put(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn apply_replaces_only_overlay_files_and_keeps_the_overlay() {
    let tree = Tree::new("files");
    let app = tree.dir("app");
    let overlay = tree.dir("overlay");
    put(&app, "app/models/widget.rb", b"old method\n");
    put(&app, "public/keep.bin", &[0, 255, 128, 10]);
    put(&app, "config/unrelated.yml", b"keep: true\n");
    put(&overlay, "app/models/widget.rb", b"new method\n");
    put(&overlay, "config/initializers/fractal.rb", b"new initializer\n");
    put(&overlay, ".settings", b"setting\n");

    let output = Command::new(env!("CARGO_BIN_EXE_fractal"))
        .arg("apply").arg(&overlay).arg(&app).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    for (path, expected) in [
        ("app/models/widget.rb", b"new method\n".as_slice()),
        ("config/initializers/fractal.rb", b"new initializer\n".as_slice()),
        (".settings", b"setting\n".as_slice()),
    ] {
        assert_eq!(fs::read(app.join(path)).unwrap(), expected);
        assert_eq!(fs::read(overlay.join(path)).unwrap(), expected);
    }
    assert_eq!(fs::read(app.join("public/keep.bin")).unwrap(), [0, 255, 128, 10]);
    assert_eq!(fs::read_to_string(app.join("config/unrelated.yml")).unwrap(), "keep: true\n");
    assert!(!overlay.join("public").exists(), "apply must not copy the app into the overlay");
}

#[test]
fn apply_defaults_to_the_overlay_inside_the_current_app() {
    let tree = Tree::new("defaults");
    let app = tree.dir("app");
    put(&app, "app/models/widget.rb", b"original\n");
    put(&app, "out/fractal/app/models/widget.rb", b"changed\n");
    let output = Command::new(env!("CARGO_BIN_EXE_fractal"))
        .arg("apply").current_dir(&app).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(app.join("app/models/widget.rb")).unwrap(), "changed\n");
    assert_eq!(fs::read_to_string(app.join("out/fractal/app/models/widget.rb")).unwrap(), "changed\n");
}

#[test]
fn empty_overlay_keeps_all_base_files() {
    let tree = Tree::new("empty");
    let app = tree.dir("app");
    let overlay = tree.dir("overlay");
    put(&app, "config/initializers/fractal.rb", b"previous build\n");
    assert_eq!(fractal::apply(&overlay, &app).unwrap(), 0);
    assert_eq!(fs::read_to_string(app.join("config/initializers/fractal.rb")).unwrap(), "previous build\n");
}

#[test]
fn invalid_roots_and_overlaps_do_not_change_the_app() {
    let tree = Tree::new("roots");
    let app = tree.dir("app");
    let overlay = tree.dir("overlay");
    let missing = tree.0.join("missing");
    let nested = tree.dir("overlay/nested");
    put(&app, "keep", b"original");
    for (source, dest) in [(&missing, &app), (&overlay, &missing), (&app, &app), (&overlay, &nested)] {
        assert!(fractal::apply(source, dest).is_err(), "{} -> {}", source.display(), dest.display());
    }
    assert!(!missing.exists());
    assert_eq!(fs::read(app.join("keep")).unwrap(), b"original");
}

#[test]
fn destination_type_conflicts_are_checked_before_any_copy() {
    let tree = Tree::new("types");
    let app = tree.dir("app");
    let overlay = tree.dir("overlay");
    put(&app, "first", b"original");
    put(&overlay, "first", b"changed");
    put(&app, "app/models", b"not a directory");
    put(&overlay, "app/models/widget.rb", b"changed");
    assert!(fractal::apply(&overlay, &app).is_err());
    assert_eq!(fs::read(app.join("first")).unwrap(), b"original");
    assert_eq!(fs::read(app.join("app/models")).unwrap(), b"not a directory");
}

#[test]
fn an_overlay_inside_the_app_cannot_overwrite_itself() {
    let tree = Tree::new("self");
    let app = tree.dir("app");
    let overlay = tree.dir("app/out/fractal");
    put(&overlay, "keep", b"original");
    put(&overlay, "out/fractal/keep", b"changed");
    assert!(fractal::apply(&overlay, &app).is_err());
    assert_eq!(fs::read(overlay.join("keep")).unwrap(), b"original");
}

#[cfg(unix)]
#[test]
fn source_and_destination_symlinks_are_rejected_before_any_copy() {
    use std::os::unix::fs::symlink;

    for kind in ["source-file", "source-directory", "destination-file", "destination-directory"] {
        let tree = Tree::new(kind);
        let app = tree.dir("app");
        let overlay = tree.dir("overlay");
        let outside = tree.dir("outside");
        put(&app, "first", b"original");
        put(&overlay, "first", b"changed");
        put(&outside, "keep", b"outside");
        match kind {
            "source-file" => symlink(outside.join("keep"), overlay.join("link")).unwrap(),
            "source-directory" => symlink(&outside, overlay.join("link")).unwrap(),
            "destination-file" => {
                put(&overlay, "link", b"changed");
                symlink(outside.join("keep"), app.join("link")).unwrap();
            }
            "destination-directory" => {
                put(&overlay, "link/keep", b"changed");
                symlink(&outside, app.join("link")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(fractal::apply(&overlay, &app).is_err(), "{kind}");
        assert_eq!(fs::read(app.join("first")).unwrap(), b"original", "{kind}");
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside", "{kind}");
    }
}

#[test]
fn emit_keeps_the_base_app_and_apply_rejects_invalid_commands() {
    let tree = Tree::new("cli");
    let app = tree.dir("app");
    let out = tree.0.join("out");
    let report = tree.0.join("report.json");
    let source = b"class Widget\n  def label\n    \"original\"\n  end\nend\n";
    put(&app, "app/models/widget.rb", source);
    put(&app, "public/keep", b"unchanged");
    let output = Command::new(env!("CARGO_BIN_EXE_fractal"))
        .args(["emit", "--file", "app/models/widget.rb", "--report"]).arg(&report)
        .arg(&app).arg("-o").arg(&out).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let decisions: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(decisions["compiled"], serde_json::json!(["app/models/widget.rb"]));
    assert_eq!(decisions["methods"][0]["key"], "Widget#label@app/models/widget.rb:2");
    assert!(!out.join("app").exists(), "no pass rewrites the file, so the overlay holds no app files");
    assert_eq!(fs::read(app.join("app/models/widget.rb")).unwrap(), source);
    assert_eq!(fs::read(app.join("public/keep")).unwrap(), b"unchanged");

    for args in [
        vec!["apply", "--file", "app/models/widget.rb"],
        vec!["apply", "one", "two", "three"],
        vec!["emit", "--output"],
        vec!["emit", "--unknown"],
        vec!["unknown"],
        vec![],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_fractal")).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    for (source, dest) in [(&tree.0.join("absent"), &app), (&out, &tree.0.join("absent"))] {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["fractal", "apply"]).arg(source).arg(dest).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
    }
}

#[test]
fn invalid_rewritten_paths_preserve_the_app_and_previous_overlay() {
    let tree = Tree::new("rewrite-paths");
    let app = tree.dir("app");
    let overlay = tree.dir("overlay");
    tree.dir("app/a");
    put(&tree.0, "outside", b"outside");
    put(&app, "keep.rb", b"original");
    put(&overlay, fractal::MANIFEST, b"previous manifest");
    put(&overlay, "keep.rb", b"previous build");
    let absolute = tree.0.join("outside").to_string_lossy().into_owned();
    for path in ["../outside", "a/../../outside", "./keep.rb", "", absolute.as_str()] {
        let rewritten = vec![(path.to_string(), "changed".to_string())];
        assert!(fractal::tree::write(&app, &overlay, &rewritten, "{}").is_err(), "{path}");
        assert_eq!(fs::read(tree.0.join("outside")).unwrap(), b"outside");
        assert_eq!(fs::read(app.join("keep.rb")).unwrap(), b"original");
        assert_eq!(fs::read(overlay.join("keep.rb")).unwrap(), b"previous build");
        assert_eq!(fs::read(overlay.join(fractal::MANIFEST)).unwrap(), b"previous manifest");
    }
}
