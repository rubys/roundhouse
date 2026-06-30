//! Identifier mangling for C# emit.
//!
//! Ruby identifiers are snake_case and admit a trailing `?`/`!`; idiomatic
//! C# is PascalCase for types/methods/properties and camelCase for
//! locals/params, neither of which admits those suffixes. `pascal`/`camel`
//! are the single deterministic snake→case maps applied at BOTH definition
//! and call sites, so renames stay consistent for free (e.g. `escape_int`
//! and every `Db.EscapeInt(...)` call both become `EscapeInt`). Reserved
//! keywords are `@`-escaped (`@class`), the C# verbatim-identifier form.
//!
//! Mirrors `src/emit/kotlin/naming.rs`; the only divergences are the case
//! (Pascal/camel vs Kotlin's camel-only) and the escape syntax (`@x` vs
//! Kotlin's backticks).
#![allow(dead_code)]

/// C# reserved keywords — illegal as bare identifiers, escaped with a
/// leading `@`. (Contextual keywords like `var`, `value`, `record` are
/// legal in identifier position, so they're omitted.)
fn is_csharp_keyword(s: &str) -> bool {
    matches!(
        s,
        "abstract" | "as" | "base" | "bool" | "break" | "byte" | "case" | "catch" | "char"
            | "checked" | "class" | "const" | "continue" | "decimal" | "default" | "delegate"
            | "do" | "double" | "else" | "enum" | "event" | "explicit" | "extern" | "false"
            | "finally" | "fixed" | "float" | "for" | "foreach" | "goto" | "if" | "implicit"
            | "in" | "int" | "interface" | "internal" | "is" | "lock" | "long" | "namespace"
            | "new" | "null" | "object" | "operator" | "out" | "override" | "params" | "private"
            | "protected" | "public" | "readonly" | "ref" | "return" | "sbyte" | "sealed"
            | "short" | "sizeof" | "stackalloc" | "static" | "string" | "struct" | "switch"
            | "this" | "throw" | "true" | "try" | "typeof" | "uint" | "ulong" | "unchecked"
            | "unsafe" | "ushort" | "using" | "virtual" | "void" | "volatile" | "while"
    )
}

/// A qualified Ruby class name → its C# class name. Normally the last
/// `::` segment (the flat `Roundhouse` namespace has no sub-namespaces),
/// but framework classes whose last segment is `Base` — `ActiveRecord::Base`
/// and `ActionController::Base` — would collide in the flat namespace, so
/// they concatenate all segments (`ActiveRecordBase`, `ActionControllerBase`).
/// Applied at every class-name site (decl, parent, `Ty::Class` render,
/// `Const` reference) so the disambiguation stays consistent. Same rule as
/// `kotlin/naming.rs::type_name`.
pub fn type_name(qualified: &str) -> String {
    let last = qualified.rsplit("::").next().unwrap_or(qualified);
    if last == "Base" && qualified.contains("::") {
        qualified.split("::").collect::<String>()
    } else {
        last.to_string()
    }
}

/// snake_case (with optional trailing `?`/`!` and leading underscores) →
/// PascalCase, keyword-escaped. For C# member names (methods, properties,
/// types): `created_at` → `CreatedAt`, `from_stmt` → `FromStmt`.
///
/// A trailing `!` (bang) gets a `Bang` suffix and a trailing `?`
/// (predicate) gets a `Pred` suffix, so `save!`→`SaveBang` and
/// `deleted_at?`→`DeletedAtPred` stay distinct from `Save`/`DeletedAt`
/// once punctuation is dropped. Both affixes are applied unconditionally —
/// the convention is uniform across generated and hand-written runtime
/// code, so call sites reproduce the rename without per-method context.
/// The `?` affix is mandatory because AR column predicates genuinely
/// collide: a `deleted_at` reader and a `deleted_at?` predicate coexist
/// on every column.
pub fn pascal(raw: &str) -> String {
    cased(raw, true)
}

/// snake_case → camelCase, keyword-escaped. For C# locals/params:
/// `article_id` → `articleId`, `class` → `@class`.
pub fn camel(raw: &str) -> String {
    cased(raw, false)
}

fn cased(raw: &str, upper_first: bool) -> String {
    let bang = raw.ends_with('!');
    let pred = raw.ends_with('?');
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
        let cap = !first || upper_first;
        if cap {
            let mut chars = part.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else {
            out.push_str(part);
        }
        first = false;
    }

    if out.is_empty() || out.chars().all(|c| c == '_') {
        out = trimmed.to_string();
    } else if bang {
        out.push_str("Bang");
    } else if pred {
        out.push_str("Pred");
    }

    if is_csharp_keyword(&out) {
        format!("@{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{camel, pascal};

    /// Regression guard for the AR column-predicate collision: a column
    /// reader (`deleted_at`) and its predicate (`deleted_at?`) must render
    /// to DISTINCT C# names. `?`/`!` get `Pred`/`Bang` affixes. (C# method
    /// names are camelCase today — see the idiomatic-PascalCase follow-up.)
    #[test]
    fn suffix_disambiguation_is_injective() {
        assert_ne!(pascal("deleted_at"), pascal("deleted_at?"));
        assert_ne!(camel("deleted_at"), camel("deleted_at?"));
        assert_ne!(pascal("save"), pascal("save!"));
        assert_eq!(pascal("deleted_at?"), "DeletedAtPred");
        assert_eq!(camel("deleted_at?"), "deletedAtPred");
    }
}
