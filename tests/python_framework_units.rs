//! Strangler inventory signal: drive `runtime_loader::python_units` over
//! the framework Ruby and emit each as `app/*.py`. Keeps the new
//! transpile path exercised while it's still dormant in `emit::python`.
//!
//! The test writes the emitted files to a temp dir and (if `python3` is
//! on PATH) reports which transpile to syntactically valid Python via
//! `py_compile` — the per-file readiness signal for graduating an entry
//! from hand-written `runtime/python/*.py` to transpiled.

use roundhouse::runtime_loader::python_units;
use std::process::Command;

#[test]
fn python_framework_units_emit() {
    let units = python_units(|_path, classes| classes).expect("python_units");
    assert!(!units.is_empty(), "expected framework units");

    let dir = std::path::PathBuf::from("tmp/rh-py-framework-units");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let py = Command::new("python3").arg("--version").output().is_ok();
    let mut valid = 0;
    let mut total = 0;
    println!("\n  file                              bytes  py_compile");
    println!("  -------------------------------------------------------");
    for u in &units {
        total += 1;
        assert!(!u.content.trim().is_empty(), "{:?} emitted empty", u.out_path);
        let flat = u.out_path.to_string_lossy().replace('/', "__");
        let path = dir.join(&flat);
        std::fs::write(&path, &u.content).unwrap();
        let status = if py {
            let out = Command::new("python3")
                .arg("-m")
                .arg("py_compile")
                .arg(&path)
                .output()
                .unwrap();
            if out.status.success() {
                valid += 1;
                "OK".to_string()
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
                format!("FAIL — {}", last.trim())
            }
        } else {
            "skipped (no python3)".to_string()
        };
        println!("  {:32} {:6} {}", u.out_path.to_string_lossy(), u.content.len(), status);
    }
    println!("\n  STRANGLER READINESS: {valid}/{total} framework files transpile to valid Python\n");
}
