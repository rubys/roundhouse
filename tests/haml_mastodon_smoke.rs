//! Mastodon HAML coverage smoke — compile + ingest every real `.haml`
//! under a Mastodon checkout and report how many flow through cleanly.
//!
//! `#[ignore]`d and env-gated (like the toolchain tests) so CI skips it;
//! it depends on an out-of-tree checkout. The point is the robustness
//! property — the panic-resistant parser must never crash on real input —
//! plus a running coverage number as the HAML subset (#59) fills in.
//!
//! Run:
//!
//!     MASTODON_DIR=/Users/rubys/git/mastodon \
//!       cargo test --test haml_mastodon_smoke -- --ignored --nocapture

use std::path::{Path, PathBuf};

use roundhouse::haml::compile_haml;
use roundhouse::ingest::view::{ViewEngine, ingest_template};

fn haml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            haml_files(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("haml") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs MASTODON_DIR; run with --ignored"]
fn mastodon_haml_compiles_and_ingests() {
    let dir = std::env::var("MASTODON_DIR").expect("set MASTODON_DIR to a Mastodon checkout");
    let views = Path::new(&dir).join("app/views");
    let mut files = Vec::new();
    haml_files(&views, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no .haml found under {}", views.display());

    let mut ingested = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).expect("read haml");
        // Robustness: compile must not panic on any real template.
        let _ = compile_haml(&src);
        let rel = f.strip_prefix(&views).unwrap_or(f);
        match ingest_template(&src, rel, &f.display().to_string(), ViewEngine::Haml.compile_fn()) {
            Ok(_) => ingested += 1,
            Err(e) => failures.push((rel.display().to_string(), e.to_string())),
        }
    }

    let total = files.len();
    eprintln!(
        "\nMastodon HAML: {ingested}/{total} ingested cleanly ({:.1}%)",
        100.0 * ingested as f64 / total as f64,
    );
    eprintln!("first failures:");
    for (path, err) in failures.iter().take(25) {
        eprintln!("  {path}: {err}");
    }
}
