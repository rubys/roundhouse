//! Stamps the build with the commit it came from, so `roundhouse
//! --version` (and the MCP `serverInfo`) name the exact tree behind a
//! number someone pastes into an issue — the same `ROUNDHOUSE_COMMIT`
//! the wasm build already carries for the /ide/ summary line.
//!
//! Precedence: an explicit `ROUNDHOUSE_COMMIT` in the environment (CI
//! sets `github.sha`), else `git rev-parse --short HEAD` on the source
//! tree, else nothing — a tarball build simply has no commit.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    embed_runtime_files();
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

/// The `runtime/ruby/` and `runtime/spinel/` trees the ruby and spinel
/// targets compose their output from, as a generated table of
/// `include_str!`s (`$OUT_DIR/runtime_files.rs`, read by
/// `src/runtime_files.rs`). The emit used to read these from disk
/// relative to the working directory, which meant the shipped binary
/// could emit Go from anywhere but ruby/spinel only from inside the
/// repo. `include_str!` keeps the table cheap to compile, and cargo
/// re-runs this script when anything under either directory changes.
///
/// Same admission rule as the disk walkers this replaces: dotfiles
/// skipped, non-UTF-8 files skipped. Directory filtering (`SKIP_DIRS`)
/// stays with the walkers, which apply it per query.
fn embed_runtime_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for top in ["runtime/ruby", "runtime/spinel"] {
        let dir = root.join(top);
        println!("cargo:rerun-if-changed={}", dir.display());
        collect(&dir, top, &mut entries);
    }
    entries.sort();
    let mut out = String::from("&[\n");
    for (rel, abs) in &entries {
        out.push_str(&format!("    ({rel:?}, include_str!({:?})),\n", abs.display().to_string()));
    }
    out.push_str("]\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("runtime_files.rs");
    std::fs::write(&dest, out).expect("write runtime_files.rs");
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let nested = format!("{rel}/{name}");
        if path.is_dir() {
            collect(&path, &nested, out);
        } else if std::fs::read_to_string(&path).is_ok() {
            out.push((nested, path));
        }
    }
}
