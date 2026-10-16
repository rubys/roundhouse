//! Every Ruby runtime source sets `# frozen_string_literal: true`.
//!
//! Spinel freezes every string literal, so the runtime is written to be
//! literal-safe already: a buffer that is mutated starts as `+""` or
//! `String.new`. The CRuby and JRuby trees ship these files as they are,
//! and without the comment each literal evaluation allocates a fresh String
//! there. The comment gives the Ruby-family targets the same literal
//! semantics the Spinel target has, and this test keeps a new runtime file
//! from leaving it out.

use std::path::{Path, PathBuf};

const ROOTS: [&str; 2] = ["runtime/ruby", "runtime/spinel"];

fn ruby_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            ruby_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path);
        }
    }
}

/// Whether the leading comment run (after an optional shebang, blank lines
/// allowed) carries `frozen_string_literal: true`, as Ruby reads it.
fn freezes_literals(text: &str) -> bool {
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if (n == 0 && line.starts_with("#!")) || line.is_empty() {
            continue;
        }
        let Some(body) = line.strip_prefix('#') else { return false };
        let body = body.trim();
        let pairs: Vec<&str> = match body.strip_prefix("-*-").and_then(|b| b.strip_suffix("-*-")) {
            Some(inner) => inner.split(';').collect(),
            None => vec![body],
        };
        for pair in pairs {
            if let Some((key, value)) = pair.split_once(':') {
                if key.trim().to_ascii_lowercase().replace('-', "_") == "frozen_string_literal" {
                    return value.trim().eq_ignore_ascii_case("true");
                }
            }
        }
    }
    false
}

#[test]
fn every_ruby_runtime_source_freezes_its_literals() {
    let mut files = Vec::new();
    for root in ROOTS {
        ruby_files(Path::new(root), &mut files);
    }
    assert!(files.len() > 100, "found only {} runtime .rb files; wrong working directory?", files.len());
    files.sort();
    let missing: Vec<String> = files
        .iter()
        .filter(|path| !freezes_literals(&std::fs::read_to_string(path).expect("read runtime file")))
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "these runtime files do not start with `# frozen_string_literal: true` \
         (after a shebang, if any):\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_reader_matches_ruby() {
    assert!(freezes_literals("# frozen_string_literal: true\nx = 1\n"));
    assert!(freezes_literals("#!/usr/bin/env ruby\n# frozen_string_literal: true\n"));
    assert!(freezes_literals("# -*- frozen-string-literal: true -*-\n"));
    assert!(!freezes_literals("# frozen_string_literal: false\n"));
    assert!(!freezes_literals("x = 1\n# frozen_string_literal: true\n"));
    assert!(!freezes_literals("module A; end\n"));
}
