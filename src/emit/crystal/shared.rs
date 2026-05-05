//! Cross-cutting helpers used by Crystal emit modules.

pub(super) fn indent_lines(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}

/// Crystal reserved words that can't appear as identifiers
/// (param names, local vars). Append a trailing underscore — Crystal
/// allows that suffix and it's a stable, readable rename.
pub(super) fn escape_ident(name: &str) -> String {
    if is_crystal_reserved(name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// True when `name` is a Crystal reserved word that can't appear as
/// an identifier. Centralized so `expr.rs`'s method-name keep-self
/// check and `method.rs`/`expr.rs`'s identifier-position renames stay
/// in sync.
pub(super) fn is_crystal_reserved(name: &str) -> bool {
    matches!(
        name,
        "abstract"
            | "alias"
            | "as"
            | "asm"
            | "begin"
            | "break"
            | "case"
            | "class"
            | "def"
            | "do"
            | "else"
            | "elsif"
            | "end"
            | "ensure"
            | "enum"
            | "extend"
            | "false"
            | "for"
            | "fun"
            | "if"
            | "in"
            | "include"
            | "is_a?"
            | "lib"
            | "macro"
            | "module"
            | "next"
            | "nil"
            | "of"
            | "out"
            | "pointerof"
            | "private"
            | "protected"
            | "raise"
            | "rescue"
            | "responds_to?"
            | "return"
            | "select"
            | "self"
            | "sizeof"
            | "struct"
            | "super"
            | "then"
            | "true"
            | "type"
            | "typeof"
            | "uninitialized"
            | "union"
            | "unless"
            | "until"
            | "when"
            | "while"
            | "with"
            | "yield"
    )
}
