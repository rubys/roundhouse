//! Stamps the build with the commit it came from, so `roundhouse
//! --version` (and the MCP `serverInfo`) name the exact tree behind a
//! number someone pastes into an issue — the same `ROUNDHOUSE_COMMIT`
//! the wasm build already carries for the /ide/ summary line.
//!
//! Precedence: an explicit `ROUNDHOUSE_COMMIT` in the environment (CI
//! sets `github.sha`), else `git rev-parse --short HEAD` on the source
//! tree, else nothing — a tarball build simply has no commit.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ROUNDHOUSE_COMMIT");
    let commit = std::env::var("ROUNDHOUSE_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(git_head);
    if let Some(commit) = commit {
        println!("cargo:rustc-env=ROUNDHOUSE_COMMIT={commit}");
    }
}

/// Short HEAD of the checkout containing this manifest, with rerun
/// triggers on the files whose change would move it (`.git/HEAD` and,
/// when HEAD is symbolic, the branch ref it points at), so a stale stamp
/// can't survive a commit.
fn git_head() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git_dir = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    let git_dir = root.join(git_dir);
    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(contents) = std::fs::read_to_string(&head) {
        if let Some(rf) = contents.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed={}", git_dir.join(rf).display());
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}
