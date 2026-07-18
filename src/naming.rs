//! Rails naming conventions (snake_case, camelize, singular/plural).
//!
//! Deliberately naive. Real Rails uses `ActiveSupport::Inflector`'s rule
//! tables; we'll grow this as fixtures demand. If a test fails because of a
//! missed irregular plural, fix the rule here rather than working around it
//! in the caller.

pub fn snake_case(class_name: &str) -> String {
    let mut s = String::with_capacity(class_name.len() + 4);
    for (i, c) in class_name.char_indices() {
        if c.is_uppercase() && i > 0 {
            let prev = class_name.as_bytes()[i - 1] as char;
            if prev.is_lowercase() || prev.is_ascii_digit() {
                s.push('_');
            }
        }
        s.push(c.to_ascii_lowercase());
    }
    s
}

/// Rails `underscore`: like `snake_case`, but `::` becomes a path
/// separator (`ShortId::CandidateId` → `short_id/candidate_id`). Use for
/// file placement of possibly-namespaced classes — a literal `::` in a
/// filename breaks make dependency lists (parsed as a target separator)
/// and diverges from the Rails file convention.
pub fn underscore(class_name: &str) -> String {
    class_name
        .split("::")
        .map(snake_case)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn camelize(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = true;
    for c in snake.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Rails `camelize` on a `/`-separated path: each segment camelizes,
/// joined by `::` (`mod/activities` → `Mod::Activities`). Inverse of
/// `underscore`; slash-free input degrades to plain `camelize`.
pub fn camelize_path(path: &str) -> String {
    path.split('/')
        .map(camelize)
        .collect::<Vec<_>>()
        .join("::")
}

/// Singularize only the last `/` segment, leaving namespace segments
/// intact (`mod/activities` → `mod/activity`). Slash-free input
/// degrades to plain `singularize`.
pub fn singularize_last(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((ns, last)) => format!("{ns}/{}", singularize(last)),
        None => singularize(path),
    }
}

/// Ruby reserved words that cannot serve as local/parameter names.
/// Instance-variable names aren't keywords (`@for` is legal Ruby —
/// lobsters uses it), so the view lowering's ivar→local rewrite must
/// step around these.
const RESERVED_LOCALS: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "defined?",
    "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in",
    "module", "next", "nil", "not", "or", "redo", "rescue", "retry",
    "return", "self", "super", "then", "true", "undef", "unless",
    "until", "when", "while", "yield",
];

/// A name safe to use as a local/param identifier: reserved words get
/// a trailing `_` (`for` → `for_`), everything else passes through.
/// Must be applied at EVERY point an ivar name becomes a view-local
/// identifier (param lists, body rewrites, partial call-site args) so
/// the renamed forms agree; ivar emission sites (`@for`) stay raw.
pub fn safe_local(name: &str) -> String {
    if RESERVED_LOCALS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// Base (final) segment of a `/`-separated view-dir path or a
/// `::`-namespaced module name — the piece bare record/arg identifiers
/// derive from (`mod/activities` → `activities`, `Mod::Activities` →
/// `Activities`).
pub fn last_segment(name: &str) -> &str {
    name.rsplit(['/', ':']).next().unwrap_or(name)
}

pub fn pluralize_snake(class_name: &str) -> String {
    let snake = snake_case(class_name);
    if snake.ends_with('s') {
        format!("{snake}es")
    } else if let Some(stem) = snake.strip_suffix('y') {
        format!("{stem}ies")
    } else {
        format!("{snake}s")
    }
}

pub fn singularize(plural: &str) -> String {
    if let Some(stem) = plural.strip_suffix("ies") {
        return format!("{stem}y");
    }
    // "es" strips only when the stem ends in a sibilant (s, x, z) or a
    // sibilant digraph (sh, ch). Otherwise fall through to plain "s"
    // strip: "articles" → "article" (stem "articl" not sibilant),
    // "boxes" → "box" (stem "box" sibilant), "buses" → "bus" (stem "bus"
    // sibilant).
    if let Some(stem) = plural.strip_suffix("es") {
        let sibilant = stem.ends_with('s')
            || stem.ends_with('x')
            || stem.ends_with('z')
            || stem.ends_with("sh")
            || stem.ends_with("ch");
        if sibilant {
            return stem.to_string();
        }
        // Otherwise fall through — "es" was coincidental; just strip "s".
    }
    if let Some(s) = plural.strip_suffix('s') {
        return s.to_string();
    }
    plural.to_string()
}

pub fn singularize_camelize(plural_symbol: &str) -> String {
    camelize(&singularize(plural_symbol))
}

pub fn habtm_join_table(owner_class: &str, target_plural_sym: &str) -> String {
    let a = pluralize_snake(owner_class);
    let b = target_plural_sym.to_string();
    if a < b { format!("{a}_{b}") } else { format!("{b}_{a}") }
}
