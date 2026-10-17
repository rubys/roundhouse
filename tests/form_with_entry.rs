//! A template `form_with` used to lower to `ActionView::Base.new.form_with`.
//! Strict emitters keep only the last segment, so the view called `Base`
//! — missing in the Rust view prelude, and `ActiveRecord::Base` on
//! TypeScript, which has no `form_with`. The site now calls
//! `ViewHelpers.enter_form_with`. The ruby family enters
//! `ActionView::Base#form_with` from that helper's reopen.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::fs;
use std::path::Path;

use roundhouse::project::BuildTarget;

fn emitted_tree(target: BuildTarget) -> emit_and_run::Emitted {
    let (dir, errors) = emit_and_run::real_blog().emit(target);
    assert!(errors.is_empty(), "errors:\n{}", errors.join("\n"));
    dir
}

fn files_under(root: &Path, suffix: &str) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(suffix) {
                let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
                out.push((path, text));
            }
        }
    }
    out
}

#[test]
fn rust_form_site_calls_view_helpers_not_base() {
    let dir = emitted_tree(BuildTarget::Rust);
    let views = files_under(&dir.join("src/views"), ".rs");
    assert!(!views.is_empty(), "no rust views under {}", dir.display());
    let mut entries = 0;
    for (path, text) in &views {
        assert!(
            !text.contains("Base::new()"),
            "{} names Base:\n{text}",
            path.display()
        );
        entries += text.matches("ViewHelpers::enter_form_with(").count();
    }
    assert!(entries >= 2, "expected both real-blog forms to enter, found {entries}");
    let helpers = fs::read_to_string(dir.join("src/view_helpers.rs")).expect("view_helpers.rs");
    assert!(
        helpers.contains("fn enter_form_with"),
        "rust runtime dropped enter_form_with:\n{helpers}"
    );
    let controller = fs::read_to_string(dir.join("src/action_controller_base.rs")).expect("controller");
    assert!(
        controller.contains("vec![ControllerConfig::new()]"),
        "ControllerConfig slot was not constructed:\n{controller}"
    );
    assert!(
        !controller.contains("TODO rust2 const value"),
        "ControllerConfig slot fell through:\n{controller}"
    );
    // The config object is cloned out of its slot. The root itself
    // has to live in a string slot so the write replaces the element
    // the next read clones.
    assert!(
        controller.contains("RELATIVE_URL_ROOT_SLOT.lock().unwrap()[(0_i64) as usize] ="),
        "relative_url_root= does not write the shared slot:\n{controller}"
    );
    assert!(
        controller.contains("static RELATIVE_URL_ROOT_SLOT: std::sync::LazyLock<std::sync::Mutex<Vec<String>>>"),
        "url root slot is not an owned String:\n{controller}"
    );
}

/// `cargo check` is the compare failure (`cannot find type Base`).
/// Needs crates.io; the string test above is the hermetic gate.
#[test]
#[ignore = "requires crates.io; cargo check of the emitted app"]
fn rust_form_site_cargo_checks() {
    let dir = emitted_tree(BuildTarget::Rust);
    let output = std::process::Command::new("cargo")
        .args(["check", "--message-format=short"])
        .current_dir(&*dir)
        .output()
        .expect("cargo check");
    assert!(
        output.status.success(),
        "cargo check failed\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn typescript_form_site_calls_view_helpers_not_base() {
    let dir = emitted_tree(BuildTarget::Typescript);
    let views = files_under(&dir.join("app/views"), ".ts");
    assert!(!views.is_empty(), "no typescript views under {}", dir.display());
    let mut entries = 0;
    for (path, text) in &views {
        assert!(
            !text.contains("new Base(") && !text.contains("Base.form_with"),
            "{} names Base:\n{text}",
            path.display()
        );
        entries += text.matches("ViewHelpers.enter_form_with(").count();
    }
    assert!(entries >= 2, "expected both real-blog forms to enter, found {entries}");
    let helpers = fs::read_to_string(dir.join("src/view_helpers.ts")).expect("view_helpers.ts");
    assert!(
        helpers.contains("enter_form_with"),
        "typescript runtime dropped enter_form_with:\n{helpers}"
    );
    let controller = fs::read_to_string(dir.join("src/action_controller_base.ts")).expect("controller");
    let class_at = controller
        .find("class ControllerConfig")
        .expect("ControllerConfig class");
    let slot_at = controller
        .find("CONTROLLER_CONFIG_SLOT")
        .expect("config slot");
    assert!(
        class_at < slot_at,
        "ControllerConfig is constructed before it is declared:\n{controller}"
    );
}

/// `tsc` is the TypeScript compare / browser-smoke failure
/// (`Property 'form_with' does not exist on type 'Base'`).
#[test]
#[ignore = "requires npm; tsc of the emitted app"]
fn typescript_form_site_typechecks() {
    let dir = emitted_tree(BuildTarget::Typescript);
    let install = std::process::Command::new("npm")
        .args(["install", "--silent", "--no-audit", "--no-fund"])
        .current_dir(&*dir)
        .output()
        .expect("npm install");
    assert!(
        install.status.success(),
        "npm install failed\n{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let tsc = std::process::Command::new("npx")
        .args(["tsc", "--noEmit", "--pretty", "false"])
        .current_dir(&*dir)
        .output()
        .expect("tsc");
    assert!(
        tsc.status.success(),
        "tsc failed\n{}\n{}",
        String::from_utf8_lossy(&tsc.stdout),
        String::from_utf8_lossy(&tsc.stderr)
    );
}
