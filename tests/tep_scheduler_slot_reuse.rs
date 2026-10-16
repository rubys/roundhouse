//! Tep's fiber scheduler reuses dead slots, so its arrays stay bounded under
//! connection churn. The Ruby driver runs the real Tep::Scheduler under CRuby
//! with the poll primitives stubbed out.

use std::path::Path;
use std::process::Command;

#[test]
fn spawn_fiber_reuses_dead_slots() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/tep_scheduler_slot_reuse.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.lines().any(|line| line == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(
        out.status.success(),
        "driver exited {:?}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        out.status.code()
    );
    let failed: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("FAIL"))
        .collect();
    assert!(
        failed.is_empty(),
        "{}\n=== stdout ===\n{stdout}",
        failed.join("\n")
    );
    assert!(
        stdout.lines().any(|line| line == "5/5 checks pass"),
        "fewer checks ran than expected\n=== stdout ===\n{stdout}"
    );
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL to its path"]
fn dead_slots_are_reused_on_spinel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir =
        std::env::temp_dir().join(format!("roundhouse-scheduler-slots-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("slots");
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(compiler)
        .arg(root.join("tests/tep_scheduler_slot_reuse_native.rb"))
        .arg("-o")
        .arg(&binary)
        .current_dir(root)
        .output()
        .expect("spawn Spinel");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(&binary).output().expect("run native probe");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "slots after churn: 2\nslots after idle tick: 1\nalive: 1\n"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
