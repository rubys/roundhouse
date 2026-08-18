//! Every `require_relative` in an emitted tree must resolve inside it.
//!
//! This is the guard for a defect that recurred five times in different
//! costumes: "which runtime files does target X need" was written down
//! in five places — `project::spinel_files`, both scaffold `main.rb`s,
//! `test/test_helper.rb`, and the two toolchain harnesses — and any two
//! of them could disagree without a single unit test noticing. The miss
//! only ever surfaced at RUN time, as `cannot load such file`, from
//! whichever consumer happened to load the most.
//!
//! A require edge is a fact about the emitted tree, so the tree can be
//! asked directly. No Ruby, no spinel, no fixture boot — just resolve
//! every edge against the file set that ships.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::{spinel_base_files, target_files, BuildTarget};

/// `require_relative "x/y"` occurrences in `content`, as the raw
/// argument text. Deliberately syntactic: a dynamic require is not a
/// static edge and is not this test's business.
fn require_relative_args(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, _) in content.match_indices("require_relative") {
        // Skip commented occurrences. Several runtime files quote their
        // own require line in a header comment (`runtime/base64.rb`,
        // `json.rb`, `gem_facades.rb`), which a naive scan reads as an
        // edge from the file to itself-under-itself.
        let line_start = content[..idx].rfind('\n').map_or(0, |p| p + 1);
        if content[line_start..idx].contains('#') {
            continue;
        }
        let rest = &content[idx + "require_relative".len()..];
        // Skip to the opening quote, but not past end-of-line — a bare
        // `require_relative` inside prose (this file's own comments, the
        // scaffold's) must not swallow the next line's string.
        let Some(line_end) = rest.find('\n') else { continue };
        let line = &rest[..line_end];
        let Some(open) = line.find('"') else { continue };
        let after = &line[open + 1..];
        let Some(close) = after.find('"') else { continue };
        let arg = &after[..close];
        // Interpolated or computed paths are not static edges.
        if arg.contains("#{") {
            continue;
        }
        out.push(arg.to_string());
    }
    out
}

/// Resolve `arg` (as written inside `from`'s directory) to a
/// tree-relative path, collapsing `..` segments.
fn resolve(from: &str, arg: &str) -> Option<String> {
    let base = Path::new(from).parent().unwrap_or(Path::new(""));
    let mut parts: Vec<String> = base
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    for seg in arg.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }
    Some(parts.join("/"))
}

fn assert_require_graph_closed(files: &[(String, String)], label: &str) {
    let present: BTreeSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();

    let mut missing: Vec<String> = Vec::new();
    for (path, content) in files {
        if !path.ends_with(".rb") {
            continue;
        }
        for arg in require_relative_args(content) {
            let Some(target) = resolve(path, &arg) else {
                missing.push(format!("{path}: `{arg}` escapes the tree root"));
                continue;
            };
            let rb = format!("{target}.rb");
            if !present.contains(rb.as_str()) && !present.contains(target.as_str()) {
                missing.push(format!("{path}: require_relative \"{arg}\" → {rb}"));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "{label}: {} unresolved require_relative edge(s) in the emitted tree.\n\
         Every one is a file some consumer will fail to load at run time.\n\n{}",
        missing.len(),
        missing.join("\n"),
    );
}

fn blog() -> (roundhouse::App, PathBuf) {
    let fixture = PathBuf::from("fixtures/real-blog");
    let mut app = ingest_app(&fixture).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);
    (app, fixture)
}

#[test]
fn ruby_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Ruby).expect("ruby target files");
    assert_require_graph_closed(&files, "ruby");
}

#[test]
fn jruby_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Jruby).expect("jruby target files");
    assert_require_graph_closed(&files, "jruby");
}

#[test]
fn spinel_base_require_graph_is_closed() {
    // The pre-`spin_shape` set — what `make spinel-test` drives, and the
    // one whose gap took `toolchain-spinel` down.
    let (app, fixture) = blog();
    let files = spinel_base_files(&app, &fixture).expect("spinel base files");
    assert_require_graph_closed(&files, "spinel");
}

#[test]
fn spinel_target_require_graph_is_closed() {
    let (app, fixture) = blog();
    let files = target_files(&app, &fixture, BuildTarget::Spinel).expect("spinel target files");
    assert_require_graph_closed(&files, "spinel (spin shape)");
}

/// A bundled library (`pathname`, `set`, `json`, …) that the tree names
/// but never requires.
///
/// Separate from the `require_relative` graph above and for the same
/// reason: the rule was written down once, inside `spin_shape`, so only
/// the spinel tree got it. Ruby 4.0 autoloads `Set` and `Pathname` and
/// hides the gap; Ruby 3.4 — what the scaffold claims and what
/// `campfire-conformance` runs — raises, and campfire lost two test
/// files to a `Pathname()` in a helper. Every ruby-family target is
/// checked here so the next target to be added is checked too.
fn assert_no_missing_bundled_requires(files: &[(String, String)], label: &str) {
    let gaps = roundhouse::project::missing_bundled_requires(files);
    assert!(
        gaps.is_empty(),
        "{label}: {} file(s) name a bundled-library constant with no require:\n{}",
        gaps.len(),
        gaps.join("\n"),
    );
}

#[test]
fn bundled_requires_are_written_for_every_target() {
    let (app, fixture) = blog();
    for (target, label) in [
        (BuildTarget::Ruby, "ruby"),
        (BuildTarget::Jruby, "jruby"),
        (BuildTarget::Spinel, "spinel"),
    ] {
        let files = target_files(&app, &fixture, target).expect("target files");
        assert_no_missing_bundled_requires(&files, label);
    }
}
