//! Identifier mangling for Kotlin emit.
//!
//! Ruby identifiers are snake_case and admit a trailing `?`/`!`; Kotlin
//! convention is camelCase and forbids those suffixes. `camel` is the
//! single deterministic snake→camel map applied at BOTH definition and
//! call sites, so renames stay consistent for free (e.g. `escape_int`
//! and every `Db.escape_int(...)` call both become `escapeInt`). Hard
//! keywords are backtick-escaped.

/// Kotlin hard keywords — illegal as bare identifiers, escaped with
/// backticks. (Soft/modifier keywords like `data`, `value` are legal in
/// identifier position, so they're omitted.)
fn is_kotlin_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "break"
            | "class"
            | "continue"
            | "do"
            | "else"
            | "false"
            | "for"
            | "fun"
            | "if"
            | "in"
            | "interface"
            | "is"
            | "null"
            | "object"
            | "package"
            | "return"
            | "super"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typealias"
            | "typeof"
            | "val"
            | "var"
            | "when"
            | "while"
    )
}

/// A qualified Ruby class name → its Kotlin class name. Normally the last
/// `::` segment (the flat `roundhouse` package has no namespaces), but
/// framework classes whose last segment is `Base` — `ActiveRecord::Base` and
/// `ActionController::Base` — would collide in the flat package, so they
/// concatenate all segments (`ActiveRecordBase`, `ActionControllerBase`).
/// Applied at every class-name site (decl, parent, `Ty::Class` render,
/// `Const` reference) so the disambiguation stays consistent.
pub fn type_name(qualified: &str) -> String {
    let last = qualified.rsplit("::").next().unwrap_or(qualified);
    if last == "Base" && qualified.contains("::") {
        qualified.split("::").collect::<String>()
    } else {
        last.to_string()
    }
}

/// snake_case (with optional trailing `?`/`!` and leading underscores) →
/// camelCase, keyword-escaped. `created_at` → `createdAt`, `from_stmt` →
/// `fromStmt`, `step?` → `step`, `_adapter_insert` → `_adapterInsert`.
///
/// A trailing `!` (bang method) gets a `Bang` suffix so `save!` becomes
/// `saveBang` and doesn't collide with `save` once the punctuation is
/// dropped — the same disambiguation the TypeScript emitter does with its
/// `_bang` stem. Predicates (`?`) just drop the suffix; two methods that
/// differ only by `?` don't arise (and `valid?`/`persisted?` happily
/// coexist with same-named properties in Kotlin).
pub fn camel(raw: &str) -> String {
    let bang = raw.ends_with('!');
    let trimmed = raw.trim_end_matches(['?', '!']);
    let leading_us = trimmed.len() - trimmed.trim_start_matches('_').len();
    let core = &trimmed[leading_us..];

    let mut out = String::new();
    out.push_str(&"_".repeat(leading_us));

    let mut first = true;
    for part in core.split('_') {
        if part.is_empty() {
            continue;
        }
        if first {
            out.push_str(part);
            first = false;
        } else {
            let mut chars = part.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }

    if out.is_empty() || out.chars().all(|c| c == '_') {
        out = trimmed.to_string();
    } else if bang {
        out.push_str("Bang");
    }

    if is_kotlin_keyword(&out) {
        format!("`{out}`")
    } else {
        out
    }
}
