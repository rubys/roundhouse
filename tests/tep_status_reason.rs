//! Tep's status line uses the registered reason for conflicts and validation errors.
use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn conflict_and_validation_status_lines_match_rails() {
    let output = Command::new("ruby")
        .arg(root().join("tests/tep_status_reason.rb"))
        .output()
        .expect("Ruby is on PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "9 status lines pass\n"
    );
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL to its path"]
fn conflict_and_validation_reasons_run_on_spinel() {
    let dir =
        std::env::temp_dir().join(format!("roundhouse-status-reasons-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("reasons");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(compiler)
        .arg(root().join("tests/tep_status_reason_native.rb"))
        .arg("-o")
        .arg(&binary)
        .current_dir(root())
        .output()
        .expect("spawn Spinel");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(&binary).output().expect("run native reasons");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout),
        "200 OK\n201 Created\n409 Conflict\n422 Unprocessable Content\n404 Not Found\n500 Internal Server Error\n");
    std::fs::remove_dir_all(dir).unwrap();
}
