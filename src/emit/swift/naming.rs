//! Identifier mangling for Swift emit.
//!
//! Ruby identifiers are snake_case and admit a trailing `?`/`!`; Swift
//! convention is camelCase and forbids those suffixes. `camel` is the
//! single deterministic snake→camel map applied at BOTH definition and
//! call sites, so renames stay consistent for free (e.g. `escape_int`
//! and every `Db.escape_int(...)` call both become `escapeInt`). Hard
//! keywords are backtick-escaped (Swift uses the same backtick escape
//! as Kotlin). Ported from `src/emit/kotlin/naming.rs`.

/// Swift hard keywords — illegal as bare identifiers, escaped with
/// backticks. (Contextual keywords like `open`, `get`, `set`, `lazy`,
/// `final` are legal in identifier position, so they're omitted.)
fn is_swift_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "associatedtype"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "continue"
            | "default"
            | "defer"
            | "deinit"
            | "do"
            | "else"
            | "enum"
            | "extension"
            | "fallthrough"
            | "false"
            | "fileprivate"
            | "for"
            | "func"
            | "guard"
            | "if"
            | "import"
            | "in"
            | "init"
            | "inout"
            | "internal"
            | "is"
            | "let"
            | "nil"
            | "operator"
            | "private"
            | "protocol"
            | "public"
            | "repeat"
            | "rethrows"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "subscript"
            | "super"
            | "switch"
            | "throw"
            | "throws"
            | "true"
            | "try"
            | "typealias"
            | "var"
            | "where"
            | "while"
    )
}

/// A qualified Ruby class name → its Swift type name. Normally the last
/// `::` segment (the flat single-module emit has no namespaces), but
/// framework classes whose last segment is `Base` — `ActiveRecord::Base`
/// and `ActionController::Base` — would collide in the flat module, so
/// they concatenate all segments (`ActiveRecordBase`,
/// `ActionControllerBase`). Applied at every class-name site (decl,
/// parent, `Ty::Class` render, `Const` reference) so the disambiguation
/// stays consistent. Same fix as Kotlin's `type_name`.
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
/// dropped — the same disambiguation Kotlin/TypeScript do. Predicates
/// (`?`) just drop the suffix.
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

    if is_swift_keyword(&out) {
        format!("`{out}`")
    } else {
        out
    }
}
