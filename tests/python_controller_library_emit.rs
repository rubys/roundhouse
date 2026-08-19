//! Forcing function for the Python universal-IR overlay (the
//! CtrlWalker retirement): lower the fixtures to `LibraryClass` shape
//! and drive every family — controllers, models, views — through
//! `emit::python`'s library walker, plus the assembled overlay file
//! set through py_compile.
//!
//! Python is the last emitter deriving controllers per-artifact via
//! `lower::CtrlWalker` (`src/emit/python/controller.rs`). Every class
//! that emits and py_compiles here is ready for the switchover; every
//! failure line is the worklist. First run (2026-08-19) measured
//! all-green across all three families on both fixtures, so the pins
//! below are exact — the remaining port work is dispatch wiring, not
//! walker coverage. See src/emit/python/overlay.rs and
//! docs/python-overlay-plan.md.
//!
//! Mirrors `tests/python_framework_units.rs`'s inventory style.

use std::path::Path;
use std::process::Command;

use roundhouse::dialect::LibraryClass;
use roundhouse::emit::python::overlay::{emit_overlay_files, lower_overlay};
use roundhouse::ingest::ingest_app;
use roundhouse::session::analyze_and_lower;

fn py3() -> bool {
    Command::new("python3").arg("--version").output().is_ok()
}

fn py_compile(path: &Path) -> Result<(), String> {
    let out = Command::new("python3")
        .arg("-m")
        .arg("py_compile")
        .arg(path)
        .output()
        .unwrap();
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        Err(format!("py_compile: {last}"))
    }
}

/// Per-class emit inventory for one LC family.
fn family_inventory(
    lcs: &[LibraryClass],
    scratch: &Path,
    py: bool,
) -> Vec<(String, Result<(), String>)> {
    std::fs::create_dir_all(scratch).unwrap();
    let mut out = Vec::new();
    for lc in lcs {
        let name = lc.name.0.as_str().to_string();
        let result = match roundhouse::emit::python::emit_library_class(lc) {
            Err(e) => Err(format!("emit: {e}")),
            Ok(src) => {
                if py {
                    let path = scratch.join(format!("{}.py", name.replace("::", "__")));
                    std::fs::write(&path, &src).unwrap();
                    py_compile(&path)
                } else {
                    Ok(())
                }
            }
        };
        out.push((name, result));
    }
    out
}

fn assert_all_green(label: &str, inv: &[(String, Result<(), String>)]) {
    let ok = inv.iter().filter(|(_, r)| r.is_ok()).count();
    println!("  {label}: {ok}/{}", inv.len());
    for (name, r) in inv {
        if let Err(e) = r {
            println!("    FAIL  {name} — {e}");
        }
    }
    assert_eq!(
        ok,
        inv.len(),
        "{label}: a class stopped emitting through the library path"
    );
}

/// Full gate over one fixture: every LC family all-green, and every
/// assembled overlay file (imports, merged views, dispatch glue
/// included) syntactically valid Python.
fn overlay_gate(fixture: &Path, scratch: &str) {
    let mut app = ingest_app(fixture).expect("ingest fixture");
    analyze_and_lower(&mut app);
    let py = py3();

    let dir = std::path::PathBuf::from(scratch);
    let _ = std::fs::remove_dir_all(&dir);

    println!("\n  {} overlay:", fixture.display());
    let lcs = lower_overlay(&app);
    assert!(!lcs.controllers.is_empty(), "expected controller LCs");
    assert_all_green(
        "controllers",
        &family_inventory(&lcs.controllers, &dir.join("ctrl"), py),
    );
    assert_all_green(
        "models",
        &family_inventory(&lcs.models, &dir.join("model"), py),
    );
    assert_all_green(
        "views",
        &family_inventory(&lcs.views, &dir.join("view"), py),
    );

    let files = emit_overlay_files(&app).expect("assemble overlay files");
    assert!(files.iter().any(|f| f.path.ends_with("dispatch.py")));
    let mut failures = Vec::new();
    for f in &files {
        let flat = f.path.to_string_lossy().replace('/', "__");
        let path = dir.join(&flat);
        std::fs::write(&path, &f.content).unwrap();
        if py && flat.ends_with(".py") && !f.content.is_empty() {
            if let Err(e) = py_compile(&path) {
                failures.push(format!("{}: {e}", f.path.display()));
            }
        }
    }
    println!("  overlay files: {} emitted", files.len());
    assert!(
        failures.is_empty(),
        "overlay files must py_compile:\n  {}",
        failures.join("\n  ")
    );
}

/// tiny-blog: the always-present fixture.
#[test]
fn tiny_blog_overlay_emits_and_compiles() {
    overlay_gate(Path::new("fixtures/tiny-blog"), "tmp/rh-py-overlay-tiny");
}

/// real-blog: the Phase-1 fixture (generated; present in CI's unit
/// job and via `bin/rh fixture` locally — same convention as
/// tests/real_blog.rs).
#[test]
fn real_blog_overlay_emits_and_compiles() {
    overlay_gate(Path::new("fixtures/real-blog"), "tmp/rh-py-overlay-real");
}
