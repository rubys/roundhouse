//! Small helpers shared across Python emission submodules.

pub(super) fn indent_py(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
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
    // Python's repr() gives a safely-escaped string literal.
    // Prefer double-quoted form when the string has no embedded
    // double-quote.
    if !s.contains('"') && !s.contains('\\') && !s.contains('\n') {
        return format!("\"{}\"", s);
    }
    format!("{:?}", s)
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
