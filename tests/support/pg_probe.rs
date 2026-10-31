//! A PostgreSQL probe: the same `PROBE.lines` answered by native Rails
//! and by the emitted program, each on a fresh copy of a database that
//! holds the app's `db/structure.sql`, on the server ROUNDHOUSE_PG_TEST_URL
//! names (tests/support/pg_probe_driver.rb does the work).
//!
//! Without ROUNDHOUSE_PG_TEST_URL the probe says so and answers `None`, so
//! the test passes without running, unless ROUNDHOUSE_PG_TEST_REQUIRED is
//! set (tests/support/pg_test_url.rs). Native Rails is the release
//! `tests/support/rails_oracle.rb` activates; both sides need the pg gem.
#![allow(dead_code)]

#[path = "pg_test_url.rs"]
pub mod pg_test_url;

use std::path::{Path, PathBuf};
use std::process::Command;

/// `(native, emitted)` lines of `probe.lines`, each on a fresh database
/// named after `database`. `files` are the probe app's, written for the
/// native side; `loads` are the files native Rails requires, in order.
pub fn native_and_emitted(
    name: &str,
    files: &[(&str, &str)],
    emitted: &Path,
    database: &str,
    probe: &str,
    loads: &[&str],
) -> Option<(Vec<String>, Vec<String>)> {
    let server = pg_test_url::pg_test_db(name)?;
    // Per process: parallel runs sharing a target directory must not
    // rewrite each other's app. Kept when a side fails, for inspection.
    let native_root: PathBuf =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&native_root);
    for (path, text) in files {
        let full = native_root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let result = Command::new("ruby")
        .arg("tests/support/pg_probe_driver.rb")
        .arg(&native_root)
        .arg(emitted)
        .arg(database)
        .arg(probe)
        .args(loads)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("ROUNDHOUSE_PG_TEST_URL", &server.url)
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(result.status.success(), "stdout={stdout}\nstderr={}", String::from_utf8_lossy(&result.stderr));
    let answers: serde_json::Value = serde_json::from_str(stdout.trim()).expect("driver prints JSON");
    let lines = |side: &str| -> Vec<String> {
        let run = &answers[side];
        assert_eq!(run["ok"], true, "{side} run failed:\n{}", run["stderr"].as_str().unwrap_or(""));
        run["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect()
    };
    let answered = (lines("native"), lines("emitted"));
    let _ = std::fs::remove_dir_all(&native_root);
    Some(answered)
}
