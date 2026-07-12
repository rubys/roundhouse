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
        // Ruby's constructor hook is `initialize`; Python's is `__init__`.
        // Mapped at both the `def` site and the `super().__init__(...)`
        // call target (see `emit::python::expr`'s `Super` arm) so they
        // line up.
        "initialize" => "__init__".to_string(),
        // `?`/`!` only ever occur as a Ruby suffix, so a global replace
        // is safe (operator names like `<=>` carry neither).
        //
        // `=`-suffix writer names deliberately pass through UNCHANGED
        // here: a Send call site renders `recv.x=(v)`, which Python
        // parses as an ASSIGNMENT with a parenthesized RHS — the
        // correct behavior for column writers, which collapse to plain
        // annotated fields (`is_accessor`). Only `def` emission must
        // legalize the name (see `py_def_name`).
        _ => name.replace('?', "_p").replace('!', "_bang"),
    }
}

/// Legalize a method name for a Python `def` line. Everything
/// `py_method_name` does, plus `=`-suffix writers (`article=`, from
/// association writers and app-defined custom writers) → `article_set`
/// — `def article=` isn't valid Python. Call sites do NOT share this
/// mapping (unlike `?`/`!`): a Send to `x=` renders as the assignment
/// `recv.x=(v)`, which is what column writers (collapsed to plain
/// fields) need. Routing writer-method call sites to `x_set(...)` is
/// the deferred cross-target extension.
pub(super) fn py_def_name(name: &str) -> String {
    match name.strip_suffix('=') {
        Some(base)
            if !base.is_empty()
                && base.chars().all(|c| c.is_alphanumeric() || c == '_') =>
        {
            format!("{base}_set")
        }
        _ => py_method_name(name),
    }
}

/// Legalize a local variable / parameter name for Python. Lowerers and
/// the TS-shared IR pick identifiers that are fine in Ruby/JS but shadow
/// a Python builtin the *emitter itself* generates as a call — most
/// visibly `len` (the lowered `length`-validation temp is named `len`,
/// and `.length`/`.size`/`.empty?` all lower to `len(...)`, so a local
/// `len` makes the builtin call later in the same scope raise
/// `UnboundLocalError`). Rename those to a trailing-underscore form at
/// every def/assign/read site so they stay consistent. Names that don't
/// collide (the overwhelming majority — `id`, `record`, `i`, …) pass
/// through untouched to keep the emit close to the source.
pub(super) fn py_ident(name: &str) -> String {
    // Builtins this emitter can emit as a *call* (so shadowing them with
    // a local is destructive), plus Python keywords (shadowing those is a
    // syntax error). Builtins like `id` that the emitter never calls are
    // deliberately omitted — renaming them would be churn with no payoff.
    const RESERVED: &[&str] = &[
        // builtins the emitter generates
        "len", "list", "dict", "set", "str", "int", "float", "bool", "type",
        "range", "sum", "min", "max", "sorted", "map", "filter", "next",
        "iter", "tuple", "bytes",
        // Python keywords
        "and", "as", "assert", "async", "await", "break", "class", "continue",
        "def", "del", "elif", "else", "except", "finally", "for", "from",
        "global", "if", "import", "in", "is", "lambda", "nonlocal", "not",
        "or", "pass", "raise", "return", "try", "while", "with", "yield",
        "None", "True", "False",
    ];
    if RESERVED.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
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
