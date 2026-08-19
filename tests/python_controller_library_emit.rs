//! Forcing function for the Python universal-IR overlay (the
//! CtrlWalker retirement): lower the fixtures to `LibraryClass` shape
//! and drive every family — controllers, models, views — through
//! `emit::python`'s library walker, plus the assembled overlay file
//! set through py_compile.
//!
//! Historically the forcing function for the CtrlWalker retirement:
//! the first run (2026-08-19) measured all-green across all three
//! families on both fixtures, the overlay became the live dispatch
//! path the same day, and the per-artifact controller emit + the
//! `CtrlWalker` trait were deleted. The gate remains as the overlay's
//! standing walker-coverage pin. See src/emit/python/overlay.rs and
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

    // Phase E remainder: the fixture + test-module LC families,
    // assembled the way the Ruby (spinel) emit does. Measured here
    // ahead of the per-artifact test-emit retirement.
    let fixture_lcs = roundhouse::lower::lower_fixtures_to_library_classes(&app);
    let (_, model_registry) = roundhouse::lower::lower_models_with_registry(
        &app.models,
        &app.schema,
        Vec::new(),
    );
    let fixture_extras: Vec<_> = fixture_lcs
        .iter()
        .map(|lc| {
            (lc.name.clone(), roundhouse::lower::class_info_from_library_class(lc))
        })
        .chain(model_registry)
        .collect();
    let test_lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        fixture_extras,
        &roundhouse::lower::routes::helper_id_segments(&app),
    );
    assert_all_green(
        "fixtures",
        &family_inventory(&fixture_lcs, &dir.join("fixture"), py),
    );
    let tests_inv = family_inventory(&test_lcs, &dir.join("test"), py);
    let ok = tests_inv.iter().filter(|(_, r)| r.is_ok()).count();
    println!("  test modules: {ok}/{}", tests_inv.len());
    for (name, r) in &tests_inv {
        if let Err(e) = r {
            println!("    FAIL  {name} — {e}");
        }
    }
    if let Some(first) = test_lcs.first() {
        if let Ok(src) = roundhouse::emit::python::emit_library_class(first) {
            println!("  --- sample test class head ({}):", first.name.0.as_str());
            for l in src.lines().take(20) {
                println!("  | {l}");
            }
        }
    }

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

/// Phase C driver: assemble the real emitted tree plus the overlay in
/// a scratch dir, boot an in-memory DB, and dispatch
/// `GET <resource>#index` through `app/v2/dispatch.py` — an actual
/// request through the overlay, not just a compile. Runtime gaps
/// (unresolved imports, degraded helper bodies) surface here as
/// Python tracebacks.
fn overlay_request_driver(fixture: &Path, scratch: &str, resource: &str) {
    if !py3() {
        return;
    }
    let mut app = ingest_app(fixture).expect("ingest fixture");
    analyze_and_lower(&mut app);
    let dir = std::path::PathBuf::from(scratch);
    let _ = std::fs::remove_dir_all(&dir);
    let mut files = roundhouse::emit::python::emit(&app);
    files.extend(emit_overlay_files(&app).expect("assemble overlay files"));
    for f in &files {
        let path = dir.join(&f.path);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(&path, &f.content).unwrap();
    }
    let driver = format!(
        "from app.db import setup_test_db\n\
         from app.schema_sql import CREATE_TABLES\n\
         setup_test_db(CREATE_TABLES)\n\
         from app.v2.dispatch import dispatch\n\
         c = dispatch(\"{resource}\", \"index\", request_path=\"/{resource}\")\n\
         assert c.status == 200, f\"status={{c.status}}\"\n\
         assert c.body, \"empty body\"\n\
         print(f\"OK {resource}#index status={{c.status}} bytes={{len(c.body)}}\")\n"
    );
    std::fs::write(dir.join("overlay_driver.py"), driver).unwrap();
    let out = Command::new("python3")
        .arg("overlay_driver.py")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "overlay driver failed for {fixture:?}:\n--- stdout:\n{}\n--- stderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("  {}", String::from_utf8_lossy(&out.stdout).trim());
}

/// tiny-blog: the always-present fixture.
#[test]
fn tiny_blog_overlay_emits_and_compiles() {
    overlay_gate(Path::new("fixtures/tiny-blog"), "tmp/rh-py-overlay-tiny");
}

#[test]
fn tiny_blog_overlay_serves_index() {
    overlay_request_driver(
        Path::new("fixtures/tiny-blog"),
        "tmp/rh-py-overlay-drive-tiny",
        "posts",
    );
}

#[test]
fn real_blog_overlay_serves_index() {
    overlay_request_driver(
        Path::new("fixtures/real-blog"),
        "tmp/rh-py-overlay-drive-real",
        "articles",
    );
}

/// real-blog: the Phase-1 fixture (generated; present in CI's unit
/// job and via `bin/rh fixture` locally — same convention as
/// tests/real_blog.rs).
#[test]
fn real_blog_overlay_emits_and_compiles() {
    overlay_gate(Path::new("fixtures/real-blog"), "tmp/rh-py-overlay-real");
}
