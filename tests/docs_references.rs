//! Docs-reference drift guard: every repo path a top-level doc names
//! must exist, and every `path.rs::symbol` reference must resolve.
//!
//! The docs audit of 2026-08-19 found dozens of dead references
//! (`src/ingest.rs` after it became a directory, deleted test names,
//! renamed lowering artifacts). Prose claims can't be tested, but
//! path references can — this is the same move as
//! `every_runtime_method_body_is_fully_typed`: turn a discipline into
//! a gate. Scope is the curated docs (README, DEVELOPMENT, AGENTS,
//! WHY, BETS, docs/README, docs/data/, docs/pipeline/); working plans
//! and docs/archive/ are point-in-time records and exempt.
//!
//! Extraction rules (deliberately conservative — a missed reference
//! costs nothing, a false positive costs trust):
//! - Backtick spans and markdown link targets are candidates.
//! - Only single-token spans whose first path segment is an
//!   unambiguous repo root are checked. Rails-app-relative paths
//!   (`app/models/…`, `config/routes.rb`) and emitted-tree paths
//!   share names with nothing in the repo root, so they fall out.
//! - `src/` is both a repo root and the emitted TS/Rust tree root, so
//!   under `src/` only `.rs` paths (and directories) are checked.
//! - `path.rs::symbol` (or `path.rs:symbol`) additionally requires
//!   the symbol text to appear in the file; `path.rs:123` line
//!   suffixes are stripped and not verified (lines rot too fast).

use std::fs;
use std::path::Path;

const DOCS: &[&str] = &[
    "README.md",
    "DEVELOPMENT.md",
    "AGENTS.md",
    "WHY.md",
    "BETS.md",
    "docs/README.md",
];

/// First segments that unambiguously name repo directories.
const REPO_ROOTS: &[&str] = &[
    "src", "docs", "tests", "runtime", "scripts", "tools", "e2e",
    "wasm", "editors", "site", "bench", ".github", "kotlin-reference",
    "swift-reference",
];

/// Prefixes that are generated or otherwise expected to be absent
/// from a fresh checkout.
const EXEMPT_PREFIXES: &[&str] = &[
    "fixtures/real-blog",
    "build/",
    "downloads/",
    "_site",
];

/// Exact paths docs legitimately mention that name files in the
/// *emitted* project tree, which shares the `src/` root with the
/// repo. None of these exist in the repo's own `src/`.
const EXEMPT_EMITTED: &[&str] = &["src/db.rs", "src/runtime.rs", "src/main.rs"];

fn doc_files() -> Vec<String> {
    let mut files: Vec<String> = DOCS.iter().map(|s| s.to_string()).collect();
    for dir in ["docs/data", "docs/pipeline"] {
        for entry in fs::read_dir(dir).expect(dir) {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "md") {
                files.push(p.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files
}

/// Pull candidate references out of one doc: backtick spans plus
/// markdown link targets (resolved relative to the doc's directory).
fn candidates(doc: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Backtick spans, single-line only.
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else { break };
        let span = &rest[..end];
        rest = &rest[end + 1..];
        if !span.contains('\n') {
            out.push(span.to_string());
        }
    }
    // Markdown link targets: [text](target)
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = &rest[..end];
        rest = &rest[end..];
        if target.starts_with("http") || target.starts_with('#') {
            continue;
        }
        let target = target.split('#').next().unwrap();
        // Resolve relative to the doc's directory.
        let base = Path::new(doc).parent().unwrap();
        let joined = base.join(target);
        // Normalize `..` textually (docs never nest deeper than ../).
        let mut parts: Vec<&str> = Vec::new();
        for c in joined.to_str().unwrap().split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        out.push(parts.join("/"));
    }
    out
}

/// Is this candidate a checkable repo path? Returns the path plus an
/// optional required symbol.
fn checkable(cand: &str) -> Option<(String, Option<String>)> {
    if cand.is_empty()
        || cand.contains(char::is_whitespace)
        || cand.contains(['<', '>', '*', '{', '}', '|', '$', '(', ')'])
    {
        return None;
    }
    // `path::symbol` / `path.rs:symbol` / `path.rs:123`.
    let (path, symbol) = if let Some((p, s)) = cand.split_once("::") {
        (p, Some(s))
    } else if let Some((p, s)) = cand.rsplit_once(':') {
        if s.chars().all(|c| c.is_ascii_digit()) {
            (p, None) // line number — check the file only
        } else if p.ends_with(".rs") {
            (p, Some(s))
        } else {
            return None; // `key: value` prose, not a path
        }
    } else {
        (cand, None)
    };
    let first = path.split('/').next().unwrap();
    // bin/ collides with Rails app bin/; only bin/rh is ours.
    if path == "bin/rh" {
        return Some((path.into(), None));
    }
    if path.starts_with("fixtures/") {
        // real-blog is generated; the checked-in fixtures are real.
        if EXEMPT_PREFIXES.iter().any(|e| path.starts_with(e)) {
            return None;
        }
        return Some((path.into(), symbol.map(Into::into)));
    }
    if !REPO_ROOTS.contains(&first) || !path.contains('/') {
        return None;
    }
    if EXEMPT_PREFIXES.iter().any(|e| path.starts_with(e))
        || EXEMPT_EMITTED.contains(&path)
    {
        return None;
    }
    // Under src/, tests/, tools/: emitted trees reuse these names, so
    // only .rs / .md / .toml files and directories are ours to check.
    if matches!(first, "src" | "tools")
        && !path.ends_with('/')
        && Path::new(path).extension().is_some()
        && !path.ends_with(".rs")
        && !path.ends_with(".md")
        && !path.ends_with(".toml")
    {
        return None;
    }
    Some((path.into(), symbol.map(Into::into)))
}

#[test]
fn doc_path_references_resolve() {
    let mut failures = Vec::new();
    for doc in doc_files() {
        let text = fs::read_to_string(&doc).unwrap_or_else(|e| panic!("{doc}: {e}"));
        for cand in candidates(&doc, &text) {
            let Some((path, symbol)) = checkable(&cand) else { continue };
            let fs_path = path.trim_end_matches('/');
            if !Path::new(fs_path).exists() {
                failures.push(format!("{doc}: `{cand}` — {path} does not exist"));
                continue;
            }
            if let Some(sym) = symbol {
                let sym = sym.trim_end_matches("()");
                match fs::read_to_string(fs_path) {
                    Ok(body) if body.contains(sym) => {}
                    Ok(_) => failures.push(format!(
                        "{doc}: `{cand}` — `{sym}` not found in {path}"
                    )),
                    Err(_) => {} // directory with ::symbol — skip
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale doc references ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
