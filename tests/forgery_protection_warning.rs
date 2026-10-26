//! Rails' default forgery check heads every controller rooted at
//! ActionController::Base. Only the Ruby family's runtime issues and
//! verifies the token, so every other target names the gap as a warning.

use std::process::Command;

fn forgery_warnings(target: &str, fixture: &str) -> Vec<String> {
    let out_dir = std::env::temp_dir().join(format!("roundhouse-forgery-{fixture}-{target}-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target, "--allow-unsupported", "-o"])
        .arg(&out_dir)
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(fixture))
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&out_dir).ok();
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.contains("forgery_protection")).map(String::from).collect()
}

#[test]
fn a_target_without_the_token_runtime_warns_once() {
    for target in ["rust", "typescript", "go", "python"] {
        let lines = forgery_warnings(target, "real-blog");
        assert_eq!(lines.len(), 1, "{target}: {lines:#?}");
        assert!(lines[0].starts_with("roundhouse: warning["), "{target}: {lines:#?}");
    }
}

#[test]
fn the_ruby_family_verifies_the_token_and_does_not_warn() {
    for target in ["ruby", "spinel"] {
        assert!(forgery_warnings(target, "real-blog").is_empty(), "{target}");
    }
}

/// tiny-blog's controller has no ApplicationController above it, so the
/// lowering heads no chain with the check either.
#[test]
fn a_chain_not_rooted_at_base_does_not_warn() {
    assert!(forgery_warnings("rust", "tiny-blog").is_empty());
}
