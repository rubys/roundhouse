//! Cross-cutting helpers used by Crystal emit modules.

pub(super) fn indent_lines(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}
