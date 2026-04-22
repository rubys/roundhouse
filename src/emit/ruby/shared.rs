//! Cross-cutting helpers used by multiple Ruby emit modules.

use std::fmt::Write;

use crate::dialect::Comment;

/// Emit each preserved comment on its own line at the given indent
/// depth (`depth * 2` spaces). Comment text already starts with `#`,
/// so we just prefix indent + content + newline.
pub(super) fn emit_leading_comments(out: &mut String, comments: &[Comment], depth: usize) {
    let pad = "  ".repeat(depth);
    for c in comments {
        writeln!(out, "{pad}{}", c.text).unwrap();
    }
}

/// Write `body_text` line-by-line at `indent_depth * 2` spaces of indent.
/// Empty lines emit as bare `"\n"` (no trailing whitespace) — matches
/// scaffold conventions and keeps source-equivalence happy.
pub(super) fn emit_indented_body(out: &mut String, body_text: &str, indent_depth: usize) {
    let pad = "  ".repeat(indent_depth);
    for line in body_text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "{pad}{line}").unwrap();
        }
    }
}

pub(super) fn indent_lines(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}
