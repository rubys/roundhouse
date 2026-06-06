//! Small helpers shared across Python emission submodules.

pub(super) fn indent_py(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Legalize a Ruby method name for Python, at both definition and call
/// sites so the two line up. Index operators map to the dunders that
/// back native subscript syntax (`flash["k"]`); `?`/`!` suffixes
/// (predicate / bang) become `_p` / `_bang`, mirroring go2's convention.
/// Builtin predicates with Python semantics (`nil?`, `is_a?`) are
/// intercepted by `emit_send` *before* this and never reach it.
pub(super) fn py_method_name(name: &str) -> String {
    match name {
        "[]" => "__getitem__".to_string(),
        "[]=" => "__setitem__".to_string(),
        // `?`/`!` only ever occur as a Ruby suffix, so a global replace
        // is safe (operator names like `<=>` carry neither).
        _ => name.replace('?', "_p").replace('!', "_bang"),
    }
}

pub(super) fn is_bare_py_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') { return false; }
    bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

pub(super) fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

pub(super) fn py_string_literal(s: &str) -> String {
    // A correct, double-quoted Python literal. Rust's `{:?}` is NOT a
    // valid Python escaper — it renders control chars in Rust's own
    // `\u{8}` form, which Python rejects ("truncated \uXXXX escape").
    // Escape char-by-char to Python's grammar instead; non-ASCII
    // printables pass through (Python source is UTF-8 by default).
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build a comma-separated Python parameter list, prepending the
/// receiver (`self` or `cls`) when present. Empty trailing args collapse.
pub(super) fn first_param(receiver: &str, params: &[crate::dialect::Param]) -> String {
    let mut out = String::from(receiver);
    for p in params {
        out.push_str(", ");
        out.push_str(p.name.as_str());
    }
    out
}

pub(super) fn test_name_snake_py(desc: &str) -> String {
    let mut s: String = desc
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

pub(super) fn py_view_fn(model_class: &str, suffix: &str) -> String {
    let plural = crate::naming::pluralize_snake(model_class);
    format!("render_{plural}_{}", suffix.to_lowercase())
}
