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
/// triggers on HEAD and its loose or packed branch ref. Git resolves
/// these paths because linked worktrees share refs but have their own HEAD.
fn git_head() -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let head = git_path(root, "HEAD")?;
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(contents) = std::fs::read_to_string(&head) {
        if let Some(rf) = contents.trim().strip_prefix("ref: ") {
            let loose = git_path(root, rf)?;
            if loose.exists() {
                println!("cargo:rerun-if-changed={}", loose.display());
            } else {
                // A nonexistent watch makes Cargo rebuild on every run.
                // Watch the nearest existing parent to detect creation of
                // a loose ref (including a new nested branch directory).
                if let Some(parent) = loose.ancestors().skip(1).find(|p| p.exists()) {
                    println!("cargo:rerun-if-changed={}", parent.display());
                }
                if let Some(packed) = git_path(root, "packed-refs").filter(|p| p.exists()) {
                    println!("cargo:rerun-if-changed={}", packed.display());
                }
            }
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

fn git_path(root: &Path, name: &str) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let path = String::from_utf8_lossy(&out.stdout);
    let path = path.trim();
    (!path.is_empty()).then(|| root.join(path))
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
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(dest.join("runtime_files.rs"), runtime_table(entries.iter()))
        .expect("write runtime_files.rs");
    // The browser analyzer also needs framework and Spinel shim
    // declarations. Exclude emit scaffolds and tests from its table.
    let ruby = entries.iter().filter(|(path, _)| {
        (path.starts_with("runtime/ruby/") || path.starts_with("runtime/spinel/"))
            && path.ends_with(".rb")
            && !path
                .split('/')
                .any(|part| part == "test" || part == "scaffold")
    });
    std::fs::write(dest.join("runtime_const_sources.rs"), runtime_table(ruby))
        .expect("write runtime_const_sources.rs");
}

fn runtime_table<'a>(entries: impl Iterator<Item = &'a (String, PathBuf)>) -> String {
    let mut out = String::from("&[\n");
    for (rel, abs) in entries {
        out.push_str(&format!(
            "    ({rel:?}, include_str!({:?})),\n",
            abs.display().to_string()
        ));
    }
    out.push_str("]\n");
    out
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
