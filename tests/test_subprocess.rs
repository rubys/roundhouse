//! A child test must execute its body in both standalone and combined suites.
#[path = "support/test_subprocess.rs"]
mod test_subprocess;

use std::process::Command;

#[test]
fn exact_child_selection_runs_the_test_body() {
    const PROBE: &str = "ROUNDHOUSE_CHILD_SELECTION_PROBE";
    if std::env::var_os(PROBE).is_some() {
        panic!("child selection probe executed");
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_subprocess::name!("exact_child_selection_runs_the_test_body"), "--nocapture"])
        .env(PROBE, "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101), "{}", String::from_utf8_lossy(&output.stdout));
    assert!(String::from_utf8_lossy(&output.stderr).contains("child selection probe executed"));
}
